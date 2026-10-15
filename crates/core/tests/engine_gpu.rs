//! GPU integration tests for the nemo-speech adapters. They run only when
//! `MEET_ASSIST_ENGINE_DIR` (an extracted runtime) and
//! `MEET_ASSIST_MODELS_DIR` (holding the Nemotron 3.5 and Nemotron 3
//! Diarization GGUFs) are set. Audio: a 30 s cut of the AMI ES2002a WAV from
//! `MEET_ASSIST_AMI_WAV`, or `<models dir>/../data/samples/ES2002a.Mix-Headset.wav`.
//! The cut is written to a temp folder.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use diarist_core::engine::{
    DiarPreset, DiarizeRunner, Engine, EngineEvent, ErrorCode, FileJob, NemoCascadeEngine,
    NemoCliEngine,
};
use diarist_core::types::JobId;

struct Env {
    engine: PathBuf,
    models: PathBuf,
    asr: PathBuf,
    diar: PathBuf,
    parakeet: Option<PathBuf>,
    ami: PathBuf,
}

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

fn env() -> Option<Env> {
    let engine = PathBuf::from(std::env::var_os("MEET_ASSIST_ENGINE_DIR")?);
    let models = PathBuf::from(std::env::var_os("MEET_ASSIST_MODELS_DIR")?);
    let ami = std::env::var_os("MEET_ASSIST_AMI_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            models
                .parent()
                .unwrap()
                .join("data/samples/ES2002a.Mix-Headset.wav")
        });
    Some(Env {
        asr: find(&models, "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf").expect("nemotron 3.5"),
        diar: find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer"),
        parakeet: find(&models, "parakeet-tdt-0.6b-v3.q8_0.gguf"),
        engine,
        models,
        ami,
    })
}

/// Writes `seconds` of `src` starting at `from_s` as a new WAV.
fn cut(src: &Path, dst: &Path, from_s: u32, seconds: u32) {
    let mut r = hound::WavReader::open(src).unwrap();
    let spec = r.spec();
    r.seek(from_s * spec.sample_rate).unwrap();
    let n = (seconds * spec.sample_rate) as usize * spec.channels as usize;
    let mut w = hound::WavWriter::create(dst, spec).unwrap();
    for s in r.samples::<i16>().take(n) {
        w.write_sample(s.unwrap()).unwrap();
    }
    w.finalize().unwrap();
}

fn job(wav: &Path) -> FileJob {
    FileJob {
        job_id: JobId::new(),
        wav_path: wav.to_path_buf(),
        diarize: true,
        language: "en".into(),
        diar_preset: DiarPreset::Streaming,
    }
}

fn drain(rx: Receiver<EngineEvent>) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(300)) {
        let last = ev.is_terminal();
        out.push(ev);
        if last {
            break;
        }
    }
    out
}

#[test]
fn transcribes_a_30s_ami_cut() {
    let Some(env) = env() else {
        eprintln!("MEET_ASSIST_ENGINE_DIR / MEET_ASSIST_MODELS_DIR not set; skipping");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("ami30.wav");
    cut(&env.ami, &wav, 60, 30);
    let engine = NemoCliEngine::new(&env.engine, &env.models, &env.asr, &env.diar);
    let t = Instant::now();
    let events = drain(engine.transcribe_file(job(&wav)).unwrap());
    let wall = t.elapsed();
    let last = events.last().expect("events");
    assert!(
        matches!(last, EngineEvent::Done { .. }),
        "last event: {last:?}"
    );
    let words: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Utterance { words, .. } => Some(words),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(words.len() > 30, "only {} words", words.len());
    assert!(words.iter().all(|w| w.t1_us <= 31_000_000));
    let speakers: std::collections::BTreeSet<_> =
        words.iter().filter_map(|w| w.spk.as_deref()).collect();
    assert!(!speakers.is_empty() && speakers.len() <= 8);
    assert!(events
        .iter()
        .any(|e| matches!(e, EngineEvent::SpeakersFinalUntil { .. })));
    assert!(tmp.path().join("ami30.words.json").is_file(), "JSON kept");
    assert!(tmp.path().join("engine.log").is_file(), "stderr logged");
    eprintln!(
        "30 s cut: {} words, {} speakers, {:.1} s wall",
        words.len(),
        speakers.len(),
        wall.as_secs_f64()
    );
}

#[test]
fn cancel_kills_the_child() {
    let Some(env) = env() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("ami120.wav");
    cut(&env.ami, &wav, 0, 120);
    let engine = NemoCliEngine::new(&env.engine, &env.models, &env.asr, &env.diar);
    let j = job(&wav);
    let id = j.job_id.clone();
    let rx = engine.transcribe_file(j).unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    engine.cancel(id);
    let events = drain(rx);
    match events.last() {
        Some(EngineEvent::Error { code, message, .. }) => {
            assert_eq!(*code, ErrorCode::Cancelled);
            assert_eq!(message, "cancelled");
        }
        // A very fast machine may finish before the cancel lands.
        Some(EngineEvent::Done { .. }) => {}
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn bad_model_path_maps_to_an_error() {
    let Some(env) = env() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("short.wav");
    cut(&env.ami, &wav, 60, 3);
    // A file that exists but is not a model.
    let fake = tmp.path().join("fake.gguf");
    std::fs::write(&fake, b"not a gguf").unwrap();
    let engine = NemoCliEngine::new(&env.engine, &env.models, &fake, &env.diar);
    let events = drain(engine.transcribe_file(job(&wav)).unwrap());
    let Some(EngineEvent::Error { message, .. }) = events.last() else {
        panic!("expected an error, got {events:?}");
    };
    assert!(!message.is_empty());
    eprintln!(
        "runtime error mapped: {}",
        message.lines().next().unwrap_or("")
    );
}

#[test]
fn standalone_diarize_and_experimental_cascade() {
    let Some(env) = env() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("ami30.wav");
    cut(&env.ami, &wav, 60, 30);
    let runner = DiarizeRunner::new(&env.engine, &env.models, &env.diar);
    let segs = runner
        .run(
            &wav,
            &tmp.path().join("ami30.rttm"),
            Some(&tmp.path().join("ami30.diar.json")),
        )
        .unwrap();
    assert!(!segs.is_empty());
    assert!(tmp.path().join("ami30.diar.json").is_file());

    let Some(parakeet) = env.parakeet else {
        return;
    };
    let cascade = NemoCascadeEngine::new(&env.engine, &env.models, &parakeet, &env.diar);
    let events = drain(cascade.transcribe_file(job(&wav)).unwrap());
    assert!(
        matches!(events.last(), Some(EngineEvent::Done { .. })),
        "{:?}",
        events.last()
    );
    let tagged = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Utterance { words, .. } => Some(words),
            _ => None,
        })
        .flatten()
        .filter(|w| w.spk.is_some())
        .count();
    assert!(tagged > 30);
}
