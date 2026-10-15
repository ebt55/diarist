//! The live WebSocket adapter against the real `nemo-speech serve`, gated on
//! `MEET_ASSIST_ENGINE_DIR` (an extracted runtime) and
//! `MEET_ASSIST_MODELS_DIR`. A 30 s cut of the AMI ES2002a mix (from
//! `MEET_ASSIST_AMI_WAV`, default `<models>/../data/samples/...`) is streamed
//! in real time as 100 ms frames; words and speaker tags must arrive.
//! Uses the EN model (the default selection), or Nemotron 3.5 when it is
//! the only one present.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use meet_core::engine::{
    EngineEvent, LiveEngine, LiveOpts, NemoWsLiveEngine, ServeConfig, ServeSupervisor,
};
use meet_core::types::{Channel, JobId};

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

#[test]
fn streams_ami_in_real_time() {
    let (Some(engine_dir), Some(models)) = (
        std::env::var_os("MEET_ASSIST_ENGINE_DIR").map(PathBuf::from),
        std::env::var_os("MEET_ASSIST_MODELS_DIR").map(PathBuf::from),
    ) else {
        return;
    };
    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf")
        .or_else(|| find(&models, "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf"))
        .expect("a streaming ASR model");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let ami = std::env::var_os("MEET_ASSIST_AMI_WAV")
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
        .take(30 * 16_000)
        .map(|s| s.unwrap())
        .collect();

    let dir = tempfile::tempdir().unwrap();
    let vram_before = vram_used_mb();
    let mut cfg = ServeConfig::new(&engine_dir, &models, &asr, &diar);
    cfg.log_path = Some(dir.path().join("serve.log"));
    let sup = ServeSupervisor::new(cfg);
    let t = Instant::now();
    sup.ensure_ready(Duration::from_secs(120)).unwrap();
    let ready_s = t.elapsed().as_secs_f64();
    assert!(sup.status().is_ready());
    let vram_idle = vram_used_mb();
    let engine = NemoWsLiveEngine::new(sup.clone(), false);
    let job = JobId::new();
    let mut s = engine
        .open(
            job.clone(),
            Channel::Loopback,
            LiveOpts {
                diarize: true,
                language: "en".into(),
            },
        )
        .unwrap();
    let start = Instant::now();
    let mut events: Vec<(f64, EngineEvent)> = Vec::new();
    let mut vram_peak = vram_idle;
    for (i, frame) in pcm.chunks(1_600).enumerate() {
        let due = start + Duration::from_millis(100 * i as u64);
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        s.push(frame, i as u64 * 100_000).unwrap();
        while let Ok(ev) = s.events().try_recv() {
            events.push((start.elapsed().as_secs_f64(), ev));
        }
        if i % 50 == 0 {
            vram_peak = vram_peak.max(vram_used_mb());
        }
    }
    let (_k, dummy) = std::sync::mpsc::channel();
    let rx = std::mem::replace(s.events(), dummy);
    s.close().unwrap();
    for ev in rx.try_iter() {
        events.push((start.elapsed().as_secs_f64(), ev));
    }
    drop(engine);
    sup.stop();

    // First-word latency: arrival of the first utterance carrying a word
    // (a partial), minus the time that word ends according to its final.
    let first_at = events.iter().find_map(|(at, e)| match e {
        EngineEvent::Utterance { words, .. } if !words.is_empty() => Some(*at),
        _ => None,
    });
    let first_end = events.iter().find_map(|(_, e)| match e {
        EngineEvent::Utterance { words, .. } if words.iter().any(|w| w.spk.is_some()) => {
            Some(words[0].t1_us as f64 / 1e6)
        }
        _ => None,
    });
    let first = first_at.zip(first_end);
    let finals: Vec<&Vec<_>> = events
        .iter()
        .filter_map(|(_, e)| match e {
            EngineEvent::Utterance { words, .. } if words.iter().any(|w| w.spk.is_some()) => {
                Some(words)
            }
            _ => None,
        })
        .collect();
    let words: usize = finals.iter().map(|w| w.len()).sum();
    let speakers: std::collections::BTreeSet<String> = finals
        .iter()
        .flat_map(|ws| ws.iter().filter_map(|w| w.spk.clone()))
        .collect();
    let tfu = events
        .iter()
        .filter(|(_, e)| matches!(e, EngineEvent::TextFinalUntil { .. }))
        .count();
    eprintln!(
        "ready {ready_s:.1} s | first word at {:?} | finals {} | words {words} | speakers {speakers:?} | TextFinalUntil {tfu} | VRAM MiB before {vram_before:?} idle {vram_idle:?} peak {vram_peak:?}",
        first.map(|(at, t1)| format!("{at:.2} s (latency {:.2} s)", at - t1)),
        finals.len(),
    );
    for ws in finals.iter().take(3) {
        eprintln!(
            "  {}",
            ws.iter()
                .map(|w| format!("{}[{}]", w.text, w.spk.as_deref().unwrap_or("-")))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    assert!(words >= 40, "{words} words");
    assert!(!speakers.is_empty() && speakers.iter().all(|s| s.starts_with("spk_")));
    assert!(tfu >= 3);
    let (at, t1) = first.unwrap();
    assert!(at - t1 < 1.5, "first word {:.2} s after it ended", at - t1);
    assert!(matches!(events.last(), Some((_, EngineEvent::Done { .. }))));
}
