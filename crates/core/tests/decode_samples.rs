//! Decodes real recordings when `MEET_ASSIST_SAMPLES` names one or more
//! folders (separated like `PATH`). Every `*.m4a` under them, recursively,
//! goes through the decoder chain; the 16 kHz output must be within 50 ms of
//! the container duration reported by `ffprobe`.
//!
//! The files are only read; outputs go to a temp folder and are deleted.
//! Run in release mode, it decodes hours of audio:
//! `MEET_ASSIST_SAMPLES=... cargo test --release -p diarist-core --test decode_samples -- --nocapture`
//! `MEET_ASSIST_SAMPLES_LIMIT=n` decodes only the first n files.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use diarist_core::audio::{ChainDecoder, Decoder, MediaFoundationDecoder, SymphoniaDecoder};

const TOLERANCE_MS: f64 = 50.0;

fn sample_files() -> Option<Vec<PathBuf>> {
    let roots = std::env::var_os("MEET_ASSIST_SAMPLES")?;
    let mut files = Vec::new();
    for root in std::env::split_paths(&roots) {
        collect(&root, &mut files);
    }
    files.sort();
    if let Some(limit) = std::env::var("MEET_ASSIST_SAMPLES_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        files.truncate(limit);
    }
    Some(files)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("m4a")) {
            out.push(p);
        }
    }
}

fn ffprobe_duration_ms(file: &Path) -> f64 {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(file)
        .output()
        .expect("ffprobe on PATH");
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<f64>().expect("ffprobe duration") * 1_000.0
}

fn wav_ms(path: &Path) -> f64 {
    let r = hound::WavReader::open(path).unwrap();
    assert_eq!(r.spec().sample_rate, 16_000);
    assert_eq!(r.spec().channels, 1);
    f64::from(r.duration()) / 16.0
}

#[test]
fn decodes_every_sample_within_50_ms() {
    let Some(files) = sample_files() else {
        eprintln!("MEET_ASSIST_SAMPLES not set; skipping");
        return;
    };
    assert!(!files.is_empty(), "no *.m4a under MEET_ASSIST_SAMPLES");
    let tmp = tempfile::tempdir().unwrap();
    let chain = ChainDecoder::new();
    let mut failures = Vec::new();
    let mut by_decoder = std::collections::BTreeMap::<&str, usize>::new();
    let total = Instant::now();
    for (i, file) in files.iter().enumerate() {
        let out = tmp.path().join(format!("{i}.wav"));
        let expected = ffprobe_duration_ms(file);
        let t = Instant::now();
        match chain.decode_to_wav16k(file, &out) {
            Ok(info) => {
                let got = wav_ms(&out);
                let decoder = chain.last_decoder().unwrap_or("?");
                *by_decoder.entry(decoder).or_default() += 1;
                let diff = got - expected;
                eprintln!(
                    "{:>2} {decoder:<16} {:>6.1} min  diff {:+6.1} ms  {} Hz x{}  {:.1}s  {}",
                    i + 1,
                    expected / 60_000.0,
                    diff,
                    info.source_sample_rate,
                    info.source_channels,
                    t.elapsed().as_secs_f64(),
                    file.file_name().unwrap().to_string_lossy()
                );
                if diff.abs() > TOLERANCE_MS {
                    failures.push(format!("{}: {diff:+.1} ms", file.display()));
                }
            }
            Err(e) => failures.push(format!("{}: {e}", file.display())),
        }
        let _ = std::fs::remove_file(&out);
    }
    eprintln!(
        "{} files in {:.0}s, by decoder: {by_decoder:?}",
        files.len(),
        total.elapsed().as_secs_f64()
    );
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

/// The Media Foundation fallback on the first sample (Windows only).
#[test]
fn media_foundation_matches_symphonia_on_first_sample() {
    if !cfg!(windows) {
        return;
    }
    let Some(files) = sample_files() else {
        return;
    };
    let Some(file) = files.first() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let expected = ffprobe_duration_ms(file);
    let mf_out = tmp.path().join("mf.wav");
    let t = Instant::now();
    MediaFoundationDecoder
        .decode_to_wav16k(file, &mf_out)
        .unwrap();
    let mf_ms = wav_ms(&mf_out);
    eprintln!(
        "media foundation: diff {:+.1} ms in {:.1}s",
        mf_ms - expected,
        t.elapsed().as_secs_f64()
    );
    assert!((mf_ms - expected).abs() <= TOLERANCE_MS);
    let sym_out = tmp.path().join("sym.wav");
    SymphoniaDecoder.decode_to_wav16k(file, &sym_out).unwrap();
    assert!((wav_ms(&sym_out) - mf_ms).abs() <= TOLERANCE_MS);
}
