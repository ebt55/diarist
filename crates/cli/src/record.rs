//! `diarist-cli record`: live capture with live words, then the post-pass.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diarist_core::audio::capture::{self, find_device, DeviceFlow};
use diarist_core::engine::{NemoWsLiveEngine, ServeConfig, ServeSupervisor};
use diarist_core::gpu;
use diarist_core::live::{
    DegradedUpdate, DeviceChange, DeviceSelection, LevelsUpdate, LiveContext, LiveSink,
    LiveWarning, Recorder, StartOptions, WordsUpdate, MIN_FREE_BYTES, MIN_FREE_VRAM_MB,
};
use diarist_core::models;
use diarist_core::pipeline::JobQueue;
use diarist_core::settings;
use diarist_core::store::{RunStore, Store};
use diarist_core::types::{Channel, ModelRole};
use diarist_core::views::{JobEvent, JobStage};

use super::{hms, CliError, CliResult, Ctx};

/// What `record` was asked to do.
pub struct RecordArgs {
    pub seconds: Option<u64>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub diarize_mic: bool,
    pub mic: Option<String>,
    pub loopback: Option<String>,
    /// `--device` instead of the runtime package's.
    pub device: Option<String>,
}

fn clock(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}.{}",
        ms / 60_000,
        (ms / 1_000) % 60,
        (ms % 1_000) / 100
    )
}

/// Prints finished utterances, warnings and device changes; keeps the
/// latest level per channel for the once-a-second meter line.
#[derive(Default)]
struct CliSink {
    /// Latest revision per (channel, utt) not printed yet.
    pending: Mutex<BTreeMap<(Channel, u32), WordsUpdate>>,
    /// (peak after AGC, highest AGC gain) per channel since the last meter line.
    levels: Mutex<BTreeMap<Channel, (f32, f32)>>,
}

impl CliSink {
    fn print_until(&self, channel: Channel, until_ms: Option<i64>) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let ready: Vec<(Channel, u32)> = pending
            .iter()
            .filter(|((c, _), u)| {
                *c == channel
                    && until_ms.is_none_or(|t| u.words.last().is_some_and(|w| w.end_ms <= t))
            })
            .map(|(k, _)| *k)
            .collect();
        for k in ready {
            if let Some(u) = pending.remove(&k) {
                if u.words.is_empty() {
                    continue;
                }
                // One line per label run.
                let mut start = 0;
                while start < u.words.len() {
                    let label = &u.words[start].label;
                    let end = u.words[start..]
                        .iter()
                        .position(|w| &w.label != label)
                        .map_or(u.words.len(), |p| start + p);
                    let text: Vec<&str> = u.words[start..end]
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect();
                    println!(
                        "[{}] {:<8} {:<11} {}",
                        clock(u.words[start].start_ms),
                        channel.as_str(),
                        label,
                        text.join(" ")
                    );
                    start = end;
                }
            }
        }
    }

    fn flush_all(&self) {
        for ch in [Channel::Mic, Channel::Loopback] {
            self.print_until(ch, None);
        }
    }
}

impl LiveSink for CliSink {
    fn words(&self, u: &WordsUpdate) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((u.channel, u.utt), u.clone());
    }

    fn text_final(&self, channel: Channel, until_ms: i64) {
        self.print_until(channel, Some(until_ms));
    }

    fn levels(&self, u: &LevelsUpdate) {
        let mut l = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        let e = l.entry(u.channel).or_insert((-100.0, 0.0));
        e.0 = e.0.max(u.peak_dbfs);
        e.1 = e.1.max(u.gain_db);
    }

    fn degraded(&self, u: &DegradedUpdate) {
        if u.degraded {
            eprintln!("!! {}: {}", u.channel, u.reason);
        } else {
            eprintln!("!! {}: live transcription resumed", u.channel);
        }
    }

    fn warning(&self, w: &LiveWarning) {
        match w {
            LiveWarning::LoopbackSilent { seconds } => eprintln!(
                "!! nothing from the loopback for {seconds} s while the mic is active (wrong output device?)"
            ),
            LiveWarning::LoopbackQuiet { device, peak_dbfs, .. } => eprintln!(
                "!! system audio from {device} is very quiet (peaks {peak_dbfs:.0} dBFS); raise its Windows volume or pick another output"
            ),
            LiveWarning::Capture { channel, message } => eprintln!("!! {channel}: {message}"),
            LiveWarning::Echo {
                coupling, lag_ms, ..
            } => eprintln!(
                "!! meeting sound reaches the mic (echo {coupling:.2}, {lag_ms:.0} ms); echoed words removed from \"me\""
            ),
        }
    }

    fn device_changed(&self, d: &DeviceChange) {
        eprintln!("-- {} capture on {} ({:?})", d.channel, d.name, d.reason);
    }
}

fn resolve(flow: DeviceFlow, q: &Option<String>) -> CliResult<Option<String>> {
    let Some(q) = q else { return Ok(None) };
    let devs = capture::list_devices()?;
    find_device(&devs, flow, q)
        .map(|d| Some(d.id))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "no {} device matches {q:?} (see `diarist-cli devices`)",
                if flow == DeviceFlow::Capture {
                    "capture"
                } else {
                    "render"
                }
            ))
        })
}

pub fn record(ctx: &Ctx, args: RecordArgs) -> CliResult {
    let mic = resolve(DeviceFlow::Capture, &args.mic)?;
    let loopback = resolve(DeviceFlow::Render, &args.loopback)?;
    // The recording's language as the tier table spells it (`ja` -> `ja-JP`),
    // the `default_language` setting when none is given; a model that does
    // not cover it is switched for one that does, as in the app and
    // `transcribe`.
    let language = models::languages::canonical(
        &args
            .language
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                settings::default_language(&settings::effective(&*ctx.store).unwrap_or_default())
            }),
    );
    let (built, switched) = models::select_engine_for(
        &ctx.paths,
        &*ctx.store,
        &language,
        false,
        &models::EngineOverrides {
            device: args.device.clone(),
            ..Default::default()
        },
    )?;
    // The runtime package's device (`cuda:0`, `vulkan:0`, `cpu`), or --device.
    let device = built.device.clone();
    if let Some(n) = models::cpu_notice_once(&device) {
        eprintln!("warning: {n}");
    }
    let sel = built.selection.clone();
    if let Some(reason) = switched {
        if let Some(selected) = models::selected_file(&*ctx.store, ModelRole::Asr)? {
            eprintln!(
                "{}",
                models::switch_notice(reason, &selected, &sel.asr_file, &language)
            );
        }
    }
    if built.experimental {
        return Err(CliError::Failed(format!(
            "{} is not a streaming model; select one for live recording (`diarist-cli models select asr ...`)",
            sel.asr_file.display_name
        )));
    }
    // The start gate: CUDA reads NVML, else doctor; Vulkan NVML only; CPU
    // never (models::VramCheck).
    let reading = models::gate_vram(&device, Some(&sel.engine_dir));
    let free = reading.map(|r| r.free_mb);
    eprintln!(
        "engine {} | device {device} | asr {} | diarizer {} | free VRAM {}",
        sel.engine_version.as_deref().unwrap_or("?"),
        sel.asr_file.display_name,
        sel.diarizer_file.display_name,
        reading.map_or("unknown".into(), |r| format!(
            "{} MB ({})",
            r.free_mb,
            r.source.as_str()
        ))
    );
    if let Some(f) = free {
        gpu::check_free_vram(f, MIN_FREE_VRAM_MB).map_err(|e| CliError::Failed(e.to_string()))?;
    }
    let mut cfg = ServeConfig::new(&sel.engine_dir, ctx.paths.models(), &sel.asr, &sel.diarizer);
    cfg.device = device.clone();
    // `engine.live_lookahead` or the model's own (docs/plans/16 section 5).
    let lookahead = models::asr_live_right_context(&ctx.paths, &*ctx.store, &sel.asr_file)?;
    if let Some(why) = &lookahead.refused {
        eprintln!("warning: live lookahead: {why}");
    }
    cfg.right_context = lookahead.right_context;
    eprintln!(
        "live lookahead {}",
        diarist_core::protocol::lookahead_text(cfg.right_context.map(i64::from))
    );
    cfg.idle_timeout = None;
    cfg.log_path = Some(ctx.paths.logs().join("serve.log"));
    let supervisor = ServeSupervisor::new(cfg);
    let t = Instant::now();
    supervisor.ensure_ready(Duration::from_secs(120))?;
    eprintln!("live engine ready in {:.1} s", t.elapsed().as_secs_f64());
    let multilingual = models::is_multilingual(&sel.asr_file);
    let store: Arc<dyn RunStore> = ctx.store.clone();
    let queue = Arc::new(JobQueue::new());
    let lctx = LiveContext {
        store: Arc::clone(&store),
        queue: Arc::clone(&queue),
        data_root: ctx.paths.root.clone(),
        // Sessions get a tag the model's prompt dictionary has.
        live_engine: Arc::new(models::languages::TaggedLiveEngine::new(
            Arc::new(NemoWsLiveEngine::new(supervisor.clone(), multilingual)),
            sel.asr_file.capabilities.languages.clone(),
        )),
        file_engine: built.engine,
        sources: capture::wasapi_source_factory(),
        // Measured above, before the live server loaded.
        vram_free_mb: Arc::new(move || free),
        min_free_vram_mb: MIN_FREE_VRAM_MB,
        min_free_bytes: MIN_FREE_BYTES,
        live_engine_info: serde_json::json!({
            "runtime": format!("nemo-speech {}", sel.engine_version.as_deref().unwrap_or("unknown")),
            "adapter": "nemo-ws",
            "models": {"asr": sel.asr_file.display_name, "diarizer": sel.diarizer_file.display_name},
            "device": device,
        }),
        live_right_context: supervisor.config().right_context,
        on_job_event: Some(Arc::new(|e| {
            if let JobEvent::Progress {
                stage: JobStage::Transcribing,
                done_ms,
                total_ms,
                ..
            } = e
            {
                eprint!(
                    "\rpost-pass {} / {}   ",
                    hms(done_ms as i64),
                    hms(total_ms.unwrap_or(0) as i64)
                );
            }
        })),
        punct: diarist_core::punct::PunctSource::Data(ctx.paths.clone()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    ctrlc::set_handler(move || s2.store(true, Ordering::SeqCst))
        .map_err(|e| CliError::Failed(format!("Ctrl-C handler: {e}")))?;
    let sink = Arc::new(CliSink::default());
    let rec = Recorder::start(
        lctx,
        StartOptions {
            title: args.title.clone(),
            language: Some(language.clone()),
            diarize_mic: args.diarize_mic,
            devices: DeviceSelection { mic, loopback },
        },
        sink.clone(),
    )?;
    eprintln!(
        "recording meeting {} ({}) into {}{}",
        rec.meeting_id(),
        rec.meeting().uid,
        rec.folder().display(),
        match args.seconds {
            Some(s) => format!(" for {s} s"),
            None => "; Ctrl-C stops".into(),
        }
    );
    let started = Instant::now();
    let mut next_meter = Duration::from_secs(1);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if args
            .seconds
            .is_some_and(|s| started.elapsed() >= Duration::from_secs(s))
        {
            break;
        }
        if started.elapsed() >= next_meter {
            next_meter += Duration::from_secs(1);
            let mut l = sink.levels.lock().unwrap_or_else(|e| e.into_inner());
            let fmt = |c: Channel| {
                l.get(&c).map_or("  --".into(), |(p, g)| {
                    if *g > 0.05 {
                        format!("{p:>4.0} (AGC +{g:.0} dB)")
                    } else {
                        format!("{p:>4.0}")
                    }
                })
            };
            eprintln!(
                "[{}] level dBFS peak: mic {} | loopback {}",
                hms(started.elapsed().as_millis() as i64),
                fmt(Channel::Mic),
                fmt(Channel::Loopback)
            );
            l.clear();
        }
    }
    eprintln!("stopping...");
    let out = rec.stop()?;
    sink.flush_all();
    eprintln!(
        "recorded {} | mic {} / loopback {} samples | issues {:?} | feed dropped {:?}",
        hms(out.duration_ms),
        out.mic_samples,
        out.loopback_samples,
        out.issues,
        out.feed_dropped
    );
    eprintln!(
        "post-pass {} (job {})",
        if out.post_pass.transcribe {
            "running"
        } else {
            "skipped (post_pass_mode = on_issues, clean recording)"
        },
        out.post_pass.job_id
    );
    let result = out
        .post_pass
        .wait(Duration::from_secs(3_600))
        .ok_or_else(|| CliError::Failed("post-pass did not finish within an hour".into()))?;
    eprintln!();
    supervisor.stop();
    let report = result.map_err(CliError::Failed)?;
    let detail = ctx.store.get_meeting(out.meeting_id)?;
    println!("meeting {} ({})", out.meeting_id, detail.meeting.uid);
    println!("  title      {}", detail.meeting.title);
    println!("  duration   {}", hms(out.duration_ms));
    println!(
        "  runs       live {}{} -> current {} ({})",
        out.live_run,
        if report.live_run_deleted {
            " (deleted)"
        } else {
            ""
        },
        report.current_run,
        if report.promoted_live {
            "live, promoted"
        } else {
            "post_pass"
        }
    );
    let speakers: Vec<String> = report
        .speakers
        .iter()
        .map(|(c, l, n)| match n {
            Some(n) => format!("{c}/{l} \"{n}\""),
            None => format!("{c}/{l}"),
        })
        .collect();
    println!("  speakers   {}", speakers.join(", "));
    println!("  words      {}", report.words);
    println!(
        "  playback   {} ({:.1} MB, {:.0}% of the WAVs)",
        report.flac_path.display(),
        report.flac_bytes as f64 / 1e6,
        100.0 * report.flac_bytes as f64 / report.wav_bytes.max(1) as f64
    );
    println!(
        "  drift      {}",
        report
            .drift_ppm
            .iter()
            .map(|(c, d)| format!("{c} {d:+.1} ppm"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  post-pass  {:.1} s wall ({:.1} s transcribing)",
        report.wall.as_secs_f64(),
        report.transcribe_wall.as_secs_f64()
    );
    Ok(())
}
