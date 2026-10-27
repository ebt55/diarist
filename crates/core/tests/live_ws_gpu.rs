//! The live WebSocket adapter against the real `nemo-speech serve`, gated on
//! `DIARIST_ENGINE_DIR` (an extracted runtime) and
//! `DIARIST_MODELS_DIR`. A 30 s cut of the AMI ES2002a mix (from
//! `DIARIST_AMI_WAV`, default `<models>/../data/samples/...`) is streamed
//! in real time as 100 ms frames; words and speaker tags must arrive.
//! Uses the EN model (the default selection), or Nemotron 3.5 when it is
//! the only one present. The `DIARIST_LIVE_*` knobs of
//! [`streams_ami_in_real_time`] set the lookahead, the length, two sessions
//! and the model for the live lookahead measurement (docs/plans/16 section 5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use diarist_core::engine::{
    EngineEvent, LiveEngine, LiveOpts, NemoWsLiveEngine, ServeConfig, ServeSupervisor,
};
use diarist_core::types::{Channel, JobId};

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

pub fn vram_used_mb() -> Option<u32> {
    let out = std::process::Command::new("nvidia-smi")
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

/// Per utterance, in order: when its first revision carrying a word
/// arrived, minus the end of the first word of its last revision (the
/// final, with the runtime's word times). The audio is streamed in real time
/// from 0, so stream time and wall time agree.
fn first_word_latencies(events: &[(f64, EngineEvent)]) -> Vec<f64> {
    let mut first: BTreeMap<u32, f64> = BTreeMap::new();
    let mut last: BTreeMap<u32, Option<f64>> = BTreeMap::new();
    for (at, e) in events {
        if let EngineEvent::Utterance { utt, words, .. } = e {
            if !words.is_empty() {
                first.entry(*utt).or_insert(*at);
            }
            last.insert(*utt, words.first().map(|w| w.t1_us as f64 / 1e6));
        }
    }
    first
        .iter()
        .filter_map(|(u, at)| last.get(u).copied().flatten().map(|t1| at - t1))
        .collect()
}

/// Nearest-rank percentile `p` (0 to 1) of `v`.
fn percentile(v: &[f64], p: f64) -> Option<f64> {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    (n > 0).then(|| v[((p * n as f64).ceil() as usize).clamp(1, n) - 1])
}

/// Knobs for the live lookahead measurement (docs/plans/16 section 5);
/// unset, each takes the value in brackets:
/// - `DIARIST_LIVE_RIGHT_CONTEXT`: `ServeConfig::right_context` (no flag:
///   the runtime default, 1 frame, as the app ran live up to 0.1.0-beta.4).
///   The app ships 6 since 0.1.0-beta.5 (both Nemotron entries'
///   `live_right_context`), so `DIARIST_LIVE_RIGHT_CONTEXT=6` tests what
///   it ships;
/// - `DIARIST_LIVE_SECONDS`: seconds of AMI from 0:56 (30);
/// - `DIARIST_LIVE_SESSIONS=2`: a second, undiarized session on the mic
///   channel fed the same audio, as a recording runs two (1);
/// - `DIARIST_LIVE_ASR`: the ASR file name (the EN model, else 3.5).
///
/// Prints per session the first-word latency of every utterance (median,
/// p90, max), and the VRAM before, idle and at peak.
#[test]
fn streams_ami_in_real_time() {
    let (Some(engine_dir), Some(models)) = (
        std::env::var_os("DIARIST_ENGINE_DIR").map(PathBuf::from),
        std::env::var_os("DIARIST_MODELS_DIR").map(PathBuf::from),
    ) else {
        return;
    };
    let env_u32 = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    };
    let right_context = env_u32("DIARIST_LIVE_RIGHT_CONTEXT");
    let seconds = env_u32("DIARIST_LIVE_SECONDS").unwrap_or(30) as usize;
    let two = env_u32("DIARIST_LIVE_SESSIONS") == Some(2);
    let asr = match std::env::var("DIARIST_LIVE_ASR") {
        Ok(name) => find(&models, name.trim()).expect("DIARIST_LIVE_ASR under the models folder"),
        Err(_) => find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf")
            .or_else(|| find(&models, "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf"))
            .expect("a streaming ASR model"),
    };
    let multilingual = asr.to_string_lossy().contains("3.5");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let ami = std::env::var_os("DIARIST_AMI_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            models
                .parent()
                .unwrap()
                .join("data/samples/ES2002a.Mix-Headset.wav")
        });
    let mut r = hound::WavReader::open(&ami).unwrap();
    assert_eq!(r.spec().sample_rate, 16_000);
    r.seek(56 * 16_000).unwrap();
    let pcm: Vec<i16> = r
        .samples::<i16>()
        .take(seconds * 16_000)
        .map(|s| s.unwrap())
        .collect();

    let dir = tempfile::tempdir().unwrap();
    let vram_before = vram_used_mb();
    let mut cfg = ServeConfig::new(&engine_dir, &models, &asr, &diar);
    cfg.log_path = Some(dir.path().join("serve.log"));
    cfg.right_context = right_context;
    let sup = ServeSupervisor::new(cfg);
    let t = Instant::now();
    sup.ensure_ready(Duration::from_secs(120)).unwrap();
    let ready_s = t.elapsed().as_secs_f64();
    assert!(sup.status().is_ready());
    let vram_idle = vram_used_mb();
    let engine = NemoWsLiveEngine::new(sup.clone(), multilingual);
    let channels: &[(Channel, bool)] = if two {
        &[(Channel::Loopback, true), (Channel::Mic, false)]
    } else {
        &[(Channel::Loopback, true)]
    };
    let mut sessions = Vec::new();
    for &(channel, diarize) in channels {
        let s = engine
            .open(
                JobId::new(),
                channel,
                LiveOpts {
                    diarize,
                    language: "en".into(),
                },
            )
            .unwrap();
        sessions.push((channel, diarize, s, Vec::<(f64, EngineEvent)>::new()));
    }
    let start = Instant::now();
    let mut vram_peak = vram_idle;
    for (i, frame) in pcm.chunks(1_600).enumerate() {
        let due = start + Duration::from_millis(100 * i as u64);
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        for (_, _, s, events) in &mut sessions {
            s.push(frame, i as u64 * 100_000).unwrap();
            while let Ok(ev) = s.events().try_recv() {
                events.push((start.elapsed().as_secs_f64(), ev));
            }
        }
        if i % 50 == 0 {
            vram_peak = vram_peak.max(vram_used_mb());
        }
    }
    let mut done = Vec::new();
    for (channel, diarize, mut s, mut events) in sessions {
        let (_k, dummy) = std::sync::mpsc::channel();
        let rx = std::mem::replace(s.events(), dummy);
        s.close().unwrap();
        for ev in rx.try_iter() {
            events.push((start.elapsed().as_secs_f64(), ev));
        }
        done.push((channel, diarize, events));
    }
    drop(engine);
    sup.stop();

    eprintln!(
        "lookahead {} | {} session(s) | {seconds} s | ready {ready_s:.1} s | VRAM MiB before {vram_before:?} idle {vram_idle:?} peak {vram_peak:?}",
        right_context.map_or("runtime default".into(), |n| n.to_string()),
        done.len(),
    );
    let mut pooled = Vec::new();
    for (channel, diarize, events) in &done {
        let latencies = first_word_latencies(events);
        // The final of an utterance is its last revision.
        let mut finals: BTreeMap<u32, &Vec<_>> = BTreeMap::new();
        for (_, e) in events {
            if let EngineEvent::Utterance { utt, words, .. } = e {
                finals.insert(*utt, words);
            }
        }
        let words: usize = finals.values().map(|w| w.len()).sum();
        let speakers: std::collections::BTreeSet<String> = finals
            .values()
            .flat_map(|ws| ws.iter().filter_map(|w| w.spk.clone()))
            .collect();
        let tfu = events
            .iter()
            .filter(|(_, e)| matches!(e, EngineEvent::TextFinalUntil { .. }))
            .count();
        let s = |p: f64| percentile(&latencies, p).map_or("-".into(), |v| format!("{v:.2}"));
        eprintln!(
            "{channel}: utterances {} | words {words} | speakers {speakers:?} | TextFinalUntil {tfu} | first-word latency s: first {}, median {}, p90 {}, max {}",
            finals.len(),
            latencies.first().map_or("-".into(), |v| format!("{v:.2}")),
            s(0.5),
            s(0.9),
            s(1.0),
        );
        for ws in finals.values().take(3) {
            eprintln!(
                "  {}",
                ws.iter()
                    .map(|w| format!("{}[{}]", w.text, w.spk.as_deref().unwrap_or("-")))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        assert!(words >= 40, "{channel}: {words} words");
        if *diarize {
            assert!(!speakers.is_empty() && speakers.iter().all(|s| s.starts_with("spk_")));
        }
        assert!(tfu >= 3, "{channel}");
        let first = latencies.first().copied().expect("a first word");
        assert!(
            first < 1.5,
            "{channel}: first word {first:.2} s after it ended"
        );
        assert!(matches!(events.last(), Some((_, EngineEvent::Done { .. }))));
        pooled.extend(latencies);
    }
    if done.len() > 1 {
        let s = |p: f64| percentile(&pooled, p).map_or("-".into(), |v| format!("{v:.2}"));
        eprintln!(
            "pooled first-word latency s: median {}, p90 {}, max {} ({} utterances)",
            s(0.5),
            s(0.9),
            s(1.0),
            pooled.len()
        );
    }
}
