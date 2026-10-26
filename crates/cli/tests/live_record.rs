//! Live recording end to end on real audio devices and the GPU, gated on
//! `DIARIST_LIVE_TEST=1` plus `DIARIST_ENGINE_DIR` (an extracted
//! runtime) and `DIARIST_MODELS_DIR` (EN model and Nemotron 3
//! Diarization). Needs VB-Audio Virtual Cable.
//!
//! The mic is made deterministic: `diarist-cli play` renders a 60 s AMI cut
//! into the cable's render side while the recorder captures "CABLE Output".
//! The loopback is the default render endpoint, fed with a different 60 s
//! cut by a second `diarist-cli play` (audible on the machine for a minute).
//! Checks: equal-length WAVs, each WAV cross-correlates with its source at
//! the expected offset, live words on both channels soon after playback,
//! the post-pass run is current with `me` on the mic and `spk_N` on the
//! loopback, the FLAC holds every sample, and a rename made on the live run
//! before Stop survives into the post-pass run. Prints the measurements.
//!
//! `DIARIST_LIVE_TEST_HALF=loopback` plays only the loopback cut (the
//! mic records cable silence) and checks only the loopback.
//!
//! Override the cable endpoints with `DIARIST_CABLE_RENDER` and
//! `DIARIST_CABLE_CAPTURE`, the AMI file with `DIARIST_AMI_WAV`, and
//! the loopback endpoint with `DIARIST_LOOPBACK_RENDER` (an endpoint
//! with software volume, e.g. S/PDIF, delivers loopback audio after the
//! volume slider: at a few percent the speech is too quiet to transcribe).

#![cfg(windows)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diarist_core::audio::capture::{
    find_device, list_devices, qpc_now_100ns, wasapi_source_factory, DeviceFlow,
};
use diarist_core::audio::flac::decode_i16;
use diarist_core::engine::{NemoCliEngine, NemoWsLiveEngine, ServeConfig, ServeSupervisor};
use diarist_core::live::{
    DeviceChange, DeviceSelection, LevelsUpdate, LiveContext, LiveSink, LiveWarning, Recorder,
    StartOptions, WordsUpdate, MIN_FREE_BYTES, MIN_FREE_VRAM_MB,
};
use diarist_core::pipeline::JobQueue;
use diarist_core::store::{RunStore, SqliteStore, Store};
use diarist_core::types::{Channel, JobId, RunKind};

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

fn vram_mb() -> Option<u32> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn cut(src: &Path, dst: &Path, from_s: u32, seconds: u32) -> Vec<i16> {
    let mut r = hound::WavReader::open(src).unwrap();
    let spec = r.spec();
    assert_eq!((spec.sample_rate, spec.channels), (16_000, 1));
    r.seek(from_s * 16_000).unwrap();
    let s: Vec<i16> = r
        .samples::<i16>()
        .take((seconds * 16_000) as usize)
        .map(|s| s.unwrap())
        .collect();
    let mut w = hound::WavWriter::create(dst, spec).unwrap();
    for &v in &s {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    s
}

fn read_wav(p: &Path) -> Vec<i16> {
    hound::WavReader::open(p)
        .unwrap()
        .samples::<i16>()
        .map(|s| s.unwrap())
        .collect()
}

/// Box-filter decimation to float.
fn decimate(x: &[i16], f: usize) -> Vec<f32> {
    x.chunks(f)
        .map(|c| c.iter().map(|&v| f32::from(v)).sum::<f32>() / c.len() as f32)
        .collect()
}

/// Normalized cross-correlation of `win` against `x` at `lag` (x index =
/// win index + lag).
fn ncc(x: &[f32], win: &[f32], lag: i64) -> f32 {
    let (mut s, mut ex, mut ew) = (0f64, 0f64, 0f64);
    for (i, &w) in win.iter().enumerate() {
        let j = i as i64 + lag;
        if j < 0 || j as usize >= x.len() {
            continue;
        }
        let v = f64::from(x[j as usize]);
        s += v * f64::from(w);
        ex += v * v;
        ew += f64::from(w) * f64::from(w);
    }
    if ex == 0.0 || ew == 0.0 {
        0.0
    } else {
        (s / (ex * ew).sqrt()) as f32
    }
}

/// Where `source` sits in `captured` (in samples), searching ±`search_s`
/// around `expected`: coarse at 1 kHz, then refined at 16 kHz. Returns
/// (lag, ncc).
fn locate(captured: &[i16], source: &[i16], expected: i64, search_s: i64) -> (i64, f32) {
    // A 20 s window from 10 s into the source.
    let (w0, wl) = (10 * 16_000usize, 20 * 16_000usize);
    let src = &source[w0..(w0 + wl).min(source.len())];
    let (xc, wc) = (decimate(captured, 16), decimate(src, 16));
    let e = (expected + w0 as i64) / 16;
    let mut best = (e, f32::MIN);
    for lag in e - search_s * 1_000..=e + search_s * 1_000 {
        let c = ncc(&xc, &wc, lag);
        if c > best.1 {
            best = (lag, c);
        }
    }
    let (xf, wf) = (decimate(captured, 1), decimate(&src[..32_000], 1));
    let mut fine = (best.0 * 16, f32::MIN);
    for lag in best.0 * 16 - 32..=best.0 * 16 + 32 {
        let c = ncc(&xf, &wf, lag);
        if c > fine.1 {
            fine = (lag, c);
        }
    }
    (fine.0 - w0 as i64, fine.1)
}

#[derive(Default)]
struct TestSink {
    /// First update carrying words, per channel: (arrival QPC, update).
    first: Mutex<BTreeMap<String, (i64, WordsUpdate)>>,
    /// First update with labelled (final) words, per channel.
    first_final: Mutex<BTreeMap<String, WordsUpdate>>,
    updates: Mutex<usize>,
    devices: Mutex<Vec<DeviceChange>>,
    /// Loopback levels with audio: (raw peak, recorded peak, gain).
    loopback_levels: Mutex<Vec<(f32, f32, f32)>>,
    warnings: Mutex<Vec<LiveWarning>>,
}

impl LiveSink for TestSink {
    fn words(&self, u: &WordsUpdate) {
        *self.updates.lock().unwrap() += 1;
        if u.words.is_empty() {
            return;
        }
        let now = qpc_now_100ns();
        self.first
            .lock()
            .unwrap()
            .entry(u.channel.to_string())
            .or_insert_with(|| (now, u.clone()));
        let is_final =
            u.channel == Channel::Mic || u.words.iter().any(|w| w.label != "spk_unknown");
        if is_final && u.rev > 0 {
            self.first_final
                .lock()
                .unwrap()
                .entry(u.channel.to_string())
                .or_insert_with(|| u.clone());
        }
    }
    fn device_changed(&self, d: &DeviceChange) {
        self.devices.lock().unwrap().push(d.clone());
    }
    fn levels(&self, l: &LevelsUpdate) {
        if l.channel == Channel::Loopback && l.raw_peak_dbfs > -85.0 {
            self.loopback_levels
                .lock()
                .unwrap()
                .push((l.raw_peak_dbfs, l.peak_dbfs, l.gain_db));
        }
    }
    fn warning(&self, w: &LiveWarning) {
        self.warnings.lock().unwrap().push(w.clone());
    }
}

fn play_qpc(out: &std::process::Output) -> i64 {
    let s = String::from_utf8_lossy(&out.stdout);
    s.split("from qpc ")
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "no start qpc in {s:?} / {}",
                String::from_utf8_lossy(&out.stderr)
            )
        })
}

#[test]
fn live_recording_end_to_end() {
    if std::env::var("DIARIST_LIVE_TEST").ok().as_deref() != Some("1") {
        return;
    }
    let (Some(engine_dir), Some(models)) = (
        std::env::var_os("DIARIST_ENGINE_DIR").map(PathBuf::from),
        std::env::var_os("DIARIST_MODELS_DIR").map(PathBuf::from),
    ) else {
        return;
    };
    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf").expect("EN model");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let ami = std::env::var_os("DIARIST_AMI_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            models
                .parent()
                .unwrap()
                .join("data/samples/ES2002a.Mix-Headset.wav")
        });
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (mic_src_path, lb_src_path) = (root.join("src_mic.wav"), root.join("src_loopback.wav"));
    let mic_src = cut(&ami, &mic_src_path, 56, 60);
    let lb_src = cut(&ami, &lb_src_path, 400, 60);

    let devs = list_devices().unwrap();
    let cap_q = std::env::var("DIARIST_CABLE_CAPTURE").unwrap_or_else(|_| "CABLE Output".into());
    let mic_dev = find_device(&devs, DeviceFlow::Capture, &cap_q).expect("CABLE Output");
    let cable_render = std::env::var("DIARIST_CABLE_RENDER")
        .ok()
        .unwrap_or_else(|| {
            [
                "CABLE Input",
                "Speakers (2- VB-Audio",
                "VB-Audio Virtual Cable",
            ]
            .iter()
            .filter_map(|q| find_device(&devs, DeviceFlow::Render, q))
            .find(|d| !d.name.contains("16 Ch"))
            .expect("the cable's render endpoint")
            .id
        });
    let default_render = match std::env::var("DIARIST_LOOPBACK_RENDER") {
        Ok(q) => find_device(&devs, DeviceFlow::Render, &q).expect("DIARIST_LOOPBACK_RENDER"),
        Err(_) => devs
            .iter()
            .find(|d| d.flow == DeviceFlow::Render && d.is_default)
            .expect("default render endpoint")
            .clone(),
    };
    let explicit_loopback = std::env::var("DIARIST_LOOPBACK_RENDER").is_ok();

    // The live server, as the app keeps it running.
    let vram_before = vram_mb();
    let mut cfg = ServeConfig::new(&engine_dir, &models, &asr, &diar);
    cfg.log_path = Some(root.join("serve.log"));
    cfg.idle_timeout = None;
    let sup = ServeSupervisor::new(cfg);
    let t = Instant::now();
    sup.ensure_ready(Duration::from_secs(120)).unwrap();
    let ready_s = t.elapsed().as_secs_f64();
    std::thread::sleep(Duration::from_secs(1));
    let vram_idle = vram_mb();

    let store = Arc::new(SqliteStore::open(root.join("diarist.db")).unwrap());
    let queue = Arc::new(JobQueue::new());
    let mut file_engine = NemoCliEngine::new(&engine_dir, &models, &asr, &diar);
    file_engine.pass_language = false;
    let ctx = LiveContext {
        store: store.clone(),
        queue: Arc::clone(&queue),
        data_root: root.clone(),
        live_engine: Arc::new(NemoWsLiveEngine::new(sup.clone(), false)),
        file_engine: Arc::new(file_engine),
        sources: wasapi_source_factory(),
        vram_free_mb: Arc::new(|| None),
        min_free_vram_mb: MIN_FREE_VRAM_MB,
        min_free_bytes: MIN_FREE_BYTES,
        live_engine_info: serde_json::json!({"runtime": "nemo-speech", "adapter": "nemo-ws"}),
        live_right_context: sup.config().right_context,
        on_job_event: None,
        punct: diarist_core::punct::PunctSource::None,
    };
    let sink = Arc::new(TestSink::default());
    let rec = Recorder::start(
        ctx,
        StartOptions {
            title: Some("live e2e".into()),
            language: Some("en".into()),
            diarize_mic: false,
            devices: DeviceSelection {
                mic: Some(mic_dev.id.clone()),
                // Default: follow the default output, as the app does.
                loopback: explicit_loopback.then(|| default_render.id.clone()),
            },
        },
        sink.clone(),
    )
    .unwrap();
    let origin = rec.origin_qpc();
    let run = rec.run_id();
    std::thread::sleep(Duration::from_secs(1));

    // Loopback source first (so the loopback stays on the default output),
    // then the mic source into the cable.
    let exe = env!("CARGO_BIN_EXE_diarist-cli");
    let lb_player = Command::new(exe)
        .args([
            "play",
            "--device",
            &default_render.id,
            lb_src_path.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let both = std::env::var("DIARIST_LIVE_TEST_HALF").ok().as_deref() != Some("loopback");
    let mic_player = both.then(|| {
        Command::new(exe)
            .args([
                "play",
                "--device",
                &cable_render,
                mic_src_path.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    });
    let mut vram_live = vram_idle;
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let a = lb_player.wait_with_output().unwrap();
        let b = mic_player.map(|p| p.wait_with_output().unwrap());
        done_tx.send((a, b)).unwrap();
    });
    let (lb_out, mic_out) = loop {
        if let Ok(r) = done_rx.recv_timeout(Duration::from_millis(500)) {
            break r;
        }
        vram_live = vram_live.max(vram_mb());
    };
    assert!(lb_out.status.success() && mic_out.as_ref().is_none_or(|o| o.status.success()));
    let lb_play = play_qpc(&lb_out);
    let mic_play = mic_out.as_ref().map(play_qpc);
    std::thread::sleep(Duration::from_millis(1_500));

    // A rename on the live run before Stop.
    let live_spk = store.live_speaker(run, Channel::Loopback, "spk_0").unwrap();
    store.rename_speaker(live_spk.id, "Alice", None).unwrap();

    // Hold the post-pass back until the WAVs are measured.
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    queue.submit(JobId::new(), move |_| {
        let _ = gate_rx.recv_timeout(Duration::from_secs(300));
    });
    let stop_t = Instant::now();
    let out = rec.stop().unwrap();
    let folder = root
        .join("audio")
        .join(&store.get_meeting(out.meeting_id).unwrap().meeting.uid);
    let mic = read_wav(&folder.join("mic.wav"));
    let lb = read_wav(&folder.join("loopback.wav"));
    let exp_mic = mic_play.map_or(0, |q| (q - origin) / 625);
    let exp_lb = (lb_play - origin) / 625;
    let (mic_lag, mic_ncc) = if both {
        locate(&mic, &mic_src, exp_mic, 2)
    } else {
        (0, 0.0)
    };
    let (lb_lag, lb_ncc) = locate(&lb, &lb_src, exp_lb, 2);
    // Level of each capture against its source over the aligned span.
    let gain_db = |cap: &[i16], src: &[i16], lag: i64| {
        let rms = |x: &[i16]| {
            let s: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
            (s / x.len().max(1) as f64).sqrt()
        };
        let start = lag.max(0) as usize;
        let n = src.len().min(cap.len().saturating_sub(start));
        20.0 * (rms(&cap[start..start + n]).max(1e-9) / rms(&src[..n]).max(1e-9)).log10()
    };
    let (mic_gain, lb_gain) = (
        gain_db(&mic, &mic_src, mic_lag),
        gain_db(&lb, &lb_src, lb_lag),
    );
    gate_tx.send(()).unwrap();
    let t_pp = Instant::now();
    let mut vram_pp = vram_live;
    let report = loop {
        if let Some(r) = out.post_pass.wait(Duration::from_millis(500)) {
            break r.unwrap();
        }
        vram_pp = vram_pp.max(vram_mb());
    };
    let pp_wall = t_pp.elapsed();
    let stop_to_done = stop_t.elapsed();
    sup.stop();

    // Live word timing (QPC is shared with the players).
    let first = sink.first.lock().unwrap().clone();
    let finals = sink.first_final.lock().unwrap().clone();
    let mut latency = BTreeMap::new();
    for (ch, play) in [("mic", mic_play), ("loopback", Some(lb_play))] {
        let Some(play) = play else { continue };
        let (arrival, _) = first
            .get(ch)
            .unwrap_or_else(|| panic!("no live words on {ch}"));
        let fin = finals
            .get(ch)
            .unwrap_or_else(|| panic!("no final words on {ch}"));
        let word_end_ms = fin.words[0].end_ms;
        let after_play_ms = (arrival - play) / 10_000;
        let after_word_ms = (arrival - origin) / 10_000 - word_end_ms;
        latency.insert(
            ch,
            (after_play_ms, after_word_ms, fin.words[0].text.clone()),
        );
    }

    let d = store.get_meeting(out.meeting_id).unwrap();
    let current = d.current_run.clone().unwrap();
    let speakers = store.run_speakers(current.id).unwrap();
    let segs = store.get_segments(out.meeting_id, 0, 100_000).unwrap();
    let label_of = |id| {
        speakers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.engine_label.clone())
            .unwrap_or_default()
    };
    let (pcm, channels) = decode_i16(&report.flac_path).unwrap();
    let flac_frames = pcm.len() as u64 / channels as u64;
    let alice = speakers
        .iter()
        .find(|s| s.display_name.as_deref() == Some("Alice"));

    eprintln!("---- live recording end to end ----");
    eprintln!(
        "default render: {} | cable render: {cable_render} | mic: {}",
        default_render.name, mic_dev.name
    );
    eprintln!(
        "devices seen: {:?}",
        sink.devices
            .lock()
            .unwrap()
            .iter()
            .map(|d| format!("{}={} ({:?})", d.channel, d.name, d.reason))
            .collect::<Vec<_>>()
    );
    eprintln!("server ready in {ready_s:.1} s");
    eprintln!(
        "WAV lengths: timeline before padding {:?}, files {} / {} samples ({:.1} s), file difference {:.1} ms",
        out.timeline_samples,
        mic.len(),
        lb.len(),
        mic.len() as f64 / 16_000.0,
        (mic.len() as f64 - lb.len() as f64).abs() / 16.0
    );
    eprintln!(
        "xcorr mic: peak at {:.1} ms, expected {:.1} ms (diff {:+.1} ms, ncc {mic_ncc:.3})",
        mic_lag as f64 / 16.0,
        exp_mic as f64 / 16.0,
        (mic_lag - exp_mic) as f64 / 16.0
    );
    eprintln!(
        "xcorr loopback: peak at {:.1} ms, expected {:.1} ms (diff {:+.1} ms, ncc {lb_ncc:.3})",
        lb_lag as f64 / 16.0,
        exp_lb as f64 / 16.0,
        (lb_lag - exp_lb) as f64 / 16.0
    );
    for (ch, (p, w, text)) in &latency {
        eprintln!("first live word {ch}: {p} ms after playback start, {w} ms after the word ended ({text:?})");
    }
    eprintln!("capture level vs source: mic {mic_gain:+.1} dB, loopback {lb_gain:+.1} dB");
    eprintln!(
        "live updates {} | issues {:?} | feed dropped {:?}",
        sink.updates.lock().unwrap(),
        out.issues,
        out.feed_dropped
    );
    eprintln!(
        "post-pass: {:.1} s wall ({:.1} s transcribing), stop to done {:.1} s | run {} ({}) | live run deleted {}",
        pp_wall.as_secs_f64(),
        report.transcribe_wall.as_secs_f64(),
        stop_to_done.as_secs_f64(),
        current.id,
        current.kind,
        report.live_run_deleted
    );
    eprintln!("speakers: {:?}", report.speakers);
    eprintln!("carry-over: {:?}", report.carry_over);
    eprintln!(
        "FLAC: {} frames, {:.2} MB = {:.1}% of {:.2} MB of WAVs",
        flac_frames,
        report.flac_bytes as f64 / 1e6,
        100.0 * report.flac_bytes as f64 / report.wav_bytes as f64,
        report.wav_bytes as f64 / 1e6
    );
    eprintln!("drift ppm: {:?}", report.drift_ppm);
    {
        let l = sink.loopback_levels.lock().unwrap();
        let max = |f: fn(&(f32, f32, f32)) -> f32| l.iter().map(f).fold(-200.0f32, f32::max);
        eprintln!(
            "loopback AGC: {} active frames, raw peak max {:.1} dBFS, recorded peak max {:.1} dBFS, gain max {:.1} dB, gain at end {:.1} dB",
            l.len(),
            max(|x| x.0),
            max(|x| x.1),
            max(|x| x.2),
            l.last().map_or(0.0, |x| x.2)
        );
    }
    eprintln!("warnings: {:?}", sink.warnings.lock().unwrap());
    eprintln!("VRAM MiB: before {vram_before:?} | server idle {vram_idle:?} | two sessions peak {vram_live:?} | post-pass peak {vram_pp:?}");

    // Lengths within 50 ms (before padding, too).
    assert!((out.mic_samples as i64 - out.loopback_samples as i64).abs() <= 800);
    let t: Vec<i64> = out.timeline_samples.values().map(|&v| v as i64).collect();
    assert!((t[0] - t[1]).abs() <= 800, "{:?}", out.timeline_samples);
    assert_eq!(mic.len() as u64, out.mic_samples);
    // Each capture lines up with its source within 100 ms.
    assert!(
        !both || (mic_ncc > 0.5 && (mic_lag - exp_mic).abs() <= 1_600),
        "mic {mic_lag} vs {exp_mic} ({mic_ncc})"
    );
    assert!(
        lb_ncc > 0.5 && (lb_lag - exp_lb).abs() <= 1_600,
        "loopback {lb_lag} vs {exp_lb} ({lb_ncc})"
    );
    // Live words within 1.5 s of the words being spoken.
    for (ch, (_, w, _)) in &latency {
        assert!(*w <= 1_500, "{ch}: first live word {w} ms after it ended");
    }
    // The post-pass is current; mic is `me`, loopback is `spk_N`.
    assert_eq!(current.kind, RunKind::PostPass);
    assert!(segs
        .iter()
        .filter(|s| s.channel == Channel::Mic)
        .all(|s| label_of(s.speaker_id) == "me"));
    assert!(!both || segs.iter().any(|s| s.channel == Channel::Mic));
    let lb_labels: Vec<String> = segs
        .iter()
        .filter(|s| s.channel == Channel::Loopback)
        .map(|s| label_of(s.speaker_id))
        .collect();
    assert!(!lb_labels.is_empty() && lb_labels.iter().all(|l| l.starts_with("spk_")));
    // The FLAC holds both WAVs sample for sample.
    assert_eq!((channels, flac_frames), (2, out.mic_samples));
    assert_eq!(flac_frames, out.loopback_samples);
    // The live rename survived.
    let alice = alice.expect("the name given on the live run is carried over");
    assert_eq!(alice.channel, Channel::Loopback);
}
