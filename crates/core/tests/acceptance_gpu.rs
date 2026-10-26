//! Long-file acceptance through the whole import pipeline (decode, engine,
//! segments, SQLite): the 77-minute CIMC recording must give a word count
//! within 2 percent of the Nemotron 3.5 reference run (11,896), 8 speakers,
//! and no empty 60 s bucket where the reference had more than 20 words.
//!
//! Runs only with `DIARIST_LONGFILE_WAV` (the 16 kHz WAV), `DIARIST_ENGINE_DIR`
//! and `DIARIST_MODELS_DIR` set; takes about 2.5 minutes on an RTX 3080.
//! The reference is `tests/fixtures/cimc_nemotron35_buckets.json` (word counts
//! per minute of the research/08 run, no audio or text).

use std::path::{Path, PathBuf};

use diarist_core::audio::ChainDecoder;
use diarist_core::engine::NemoCliEngine;
use diarist_core::pipeline::{import_file, CancelToken, ImportOptions, ImportRequest};
use diarist_core::store::{SqliteStore, Store};
use diarist_core::types::JobId;

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

#[test]
fn cimc_77_minutes_matches_the_reference() {
    let (Some(wav), Some(engine_dir), Some(models)) = (
        std::env::var_os("DIARIST_LONGFILE_WAV"),
        std::env::var_os("DIARIST_ENGINE_DIR"),
        std::env::var_os("DIARIST_MODELS_DIR"),
    ) else {
        eprintln!("DIARIST_LONGFILE_WAV / ENGINE_DIR / MODELS_DIR not set; skipping");
        return;
    };
    let models = PathBuf::from(models);
    let reference: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/cimc_nemotron35_buckets.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let ref_words = reference["words"].as_u64().unwrap() as f64;
    let ref_buckets: Vec<u64> = reference["words_per_minute"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();

    let tmp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(tmp.path().join("diarist.db")).unwrap();
    let engine = NemoCliEngine::new(
        PathBuf::from(engine_dir),
        &models,
        find(&models, "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf").unwrap(),
        find(&models, "Nemotron-3-Diarization.q8_0.gguf").unwrap(),
    );
    let started = std::time::Instant::now();
    let id = import_file(
        &store,
        &engine,
        &ChainDecoder::new(),
        ImportRequest {
            job_id: JobId::new(),
            path: PathBuf::from(wav),
            title: Some("CIMC kickoff".into()),
            diarize: true,
        },
        &ImportOptions::new(
            &diarist_core::models::DataPaths::at(tmp.path()),
            &Default::default(),
        ),
        &CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    let wall = started.elapsed().as_secs_f64();

    let words = store.get_words(id, i64::MIN, i64::MAX).unwrap();
    let detail = store.get_meeting(id).unwrap();
    let diff = (words.len() as f64 - ref_words) / ref_words;
    let mut buckets = vec![0u64; ref_buckets.len().max(1)];
    for w in &words {
        let b = (w.start_ms / 60_000) as usize;
        if b < buckets.len() {
            buckets[b] += 1;
        }
    }
    let empty: Vec<usize> = ref_buckets
        .iter()
        .enumerate()
        .filter(|(i, &n)| n > 20 && buckets[*i] == 0)
        .map(|(i, _)| i)
        .collect();
    eprintln!(
        "CIMC: {} words ({:+.2}%), {} speakers, {:.0} s wall ({:.1}x real time)",
        words.len(),
        diff * 100.0,
        detail.speakers.len(),
        wall,
        detail.meeting.duration_ms.unwrap_or(0) as f64 / 1_000.0 / wall
    );
    assert!(diff.abs() <= 0.02, "word count off by {:.2}%", diff * 100.0);
    assert_eq!(detail.speakers.len(), 8);
    assert!(empty.is_empty(), "empty minutes: {empty:?}");
}
