//! The echo gate's double-talk fixture (plan 15 section 4A; dev only, GPU):
//! the owner is stood in by FLEURS English utterances with known
//! transcripts, placed at random times over the far end of a meeting held
//! on speakers, with meeting 12's echo path (3 ms, 10 dB return loss) into
//! the mic; the real post-pass runs with `echo.mode = off` and `auto`, and
//! "me" is scored against the transcripts. `#[ignore]`d and gated:
//!
//! - `DIARIST_ECHO_FLEURS`: a `tools/eval` FLEURS folder (`audio.wav`,
//!   `refs.jsonl`; `uv run python -I fleurs_fetch.py --langs en` writes
//!   `tools/eval/.work/fleurs/en`).
//! - `DIARIST_ECHO_SAMPLE`: a meeting folder whose `playback.flac` loopback
//!   (right) is the far end (read only).
//! - `DIARIST_ENGINE_DIR`, `DIARIST_MODELS_DIR`: runtime and models (EN
//!   model, Nemotron 3 Diarization), as for `live_carry_repro`.
//! - `DIARIST_ECHO_MINUTES` (default 10), `DIARIST_ECHO_OUT` (scratch
//!   folder, default `%TEMP%\diarist-echo-fleurs`).
//!
//! Per utterance, the hypothesis is the "me" words whose middle falls in
//! the utterance's span (−0.2 s, +0.4 s); WER is summed edits over summed
//! reference words, for all utterances and for those spoken over the far
//! end (more than 30 percent of their 20 ms frames with the loopback over
//! −50 dBFS). Echo words are "me" words outside every utterance span.
//!
//! `cargo test -p diarist-core --release --test echo_fleurs -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use diarist_core::audio::flac::decode_i16;
use diarist_core::engine::NemoCliEngine;
use diarist_core::live::{run_post_pass, PostPassJob};
use diarist_core::pipeline::CancelToken;
use diarist_core::settings;
use diarist_core::store::{NewMeeting, RunStore, SqliteStore, Store};
use diarist_core::transcript::SegmentOptions;
use diarist_core::types::{Channel, JobId, MeetingStatus, RunKind, RunStatus, Source};

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

fn norm_words(t: &str) -> Vec<String> {
    t.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn edits(r: &[String], h: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut cur = vec![i + 1; h.len() + 1];
        for (j, hw) in h.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(rw != hw))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[h.len()]
}

struct Utt {
    /// Placement in the meeting (samples).
    at: usize,
    len: usize,
    words: Vec<String>,
    double_talk: bool,
}

struct Scores {
    wer: f64,
    wer_double_talk: f64,
    wer_clear: f64,
    echo_words: usize,
    me_words: usize,
    double_talk_words: usize,
}

fn score(utts: &[Utt], me: &[(i64, i64, String)]) -> Scores {
    let span = |u: &Utt| {
        (
            (u.at as f64 / 16.0) as i64 - 200,
            ((u.at + u.len) as f64 / 16.0) as i64 + 400,
        )
    };
    let (mut e_all, mut n_all, mut e_dt, mut n_dt, mut dt_words) = (0, 0, 0, 0, 0);
    let (mut e_cl, mut n_cl) = (0, 0);
    let mut inside = vec![false; me.len()];
    for u in utts {
        let (a, b) = span(u);
        let mut hyp = Vec::new();
        for (k, w) in me.iter().enumerate() {
            let mid = (w.0 + w.1) / 2;
            if (a..b).contains(&mid) {
                inside[k] = true;
                hyp.extend(norm_words(&w.2));
            }
        }
        let e = edits(&u.words, &hyp);
        e_all += e;
        n_all += u.words.len();
        if u.double_talk {
            e_dt += e;
            n_dt += u.words.len();
            dt_words += hyp.len();
        } else {
            e_cl += e;
            n_cl += u.words.len();
        }
    }
    Scores {
        wer: 100.0 * e_all as f64 / n_all.max(1) as f64,
        wer_double_talk: 100.0 * e_dt as f64 / n_dt.max(1) as f64,
        wer_clear: 100.0 * e_cl as f64 / n_cl.max(1) as f64,
        echo_words: inside.iter().filter(|i| !**i).count(),
        me_words: me.len(),
        double_talk_words: dt_words,
    }
}

/// One post-pass over the fixture with `mode`; the "me" words.
fn post_pass(
    out: &Path,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    mode: &str,
) -> Vec<(i64, i64, String)> {
    let db = out.join(format!("{mode}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    let store = SqliteStore::open(&db).unwrap();
    store.set_setting(settings::ECHO_MODE, mode).unwrap();
    if let Ok(t) = std::env::var("DIARIST_ECHO_THRESHOLD") {
        store
            .set_setting(settings::ECHO_GATE_THRESHOLD, &t)
            .unwrap();
    }
    let meeting = store
        .create_meeting(NewMeeting {
            title: format!("echo fixture {mode}"),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 0,
            status: MeetingStatus::Processing,
        })
        .unwrap();
    let ms = (mic.len() / 16) as i64;
    store.finish_meeting(meeting.id, ms, ms).unwrap();
    let live = store
        .create_run(meeting.id, RunKind::Live, serde_json::json!({}))
        .unwrap();
    store
        .materialise_live_run(live.id, &SegmentOptions::default())
        .unwrap();
    store.finish_run(live.id, RunStatus::Done).unwrap();
    let folder = out.join(mode);
    std::fs::create_dir_all(&folder).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    for (name, src) in [("mic.wav", mic), ("loopback.wav", lb)] {
        let mut w = hound::WavWriter::create(folder.join(name), spec).unwrap();
        for &v in src {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
    }
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run: live.id,
        data_root: out.to_path_buf(),
        folder,
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: true,
        segment: SegmentOptions::default(),
    };
    let report = run_post_pass(&store, engine, &job, &CancelToken::default(), &|_| {}).unwrap();
    let run = store.get_run(report.current_run).unwrap();
    eprintln!("{mode}: engine_info.echo {}", run.engine_info["echo"]);
    if let Some(d) = store.echo_dropped(report.current_run).unwrap() {
        let mut c: Vec<f32> = d.iter().map(|w| w.coefficient).collect();
        c.sort_by(f32::total_cmp);
        let q = |f: f64| {
            c.get(((c.len() as f64 - 1.0) * f) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        eprintln!(
            "{mode}: dropped coefficients p10 {:.2} p50 {:.2} p90 {:.2}; {} at or over 0.5",
            q(0.1),
            q(0.5),
            q(0.9),
            c.iter().filter(|v| **v >= 0.5).count()
        );
        std::fs::write(
            out.join(format!("{mode}.dropped.json")),
            serde_json::to_string(&d).unwrap(),
        )
        .unwrap();
    }
    let segs = store.get_segments(meeting.id, 0, u32::MAX).unwrap();
    let mic_seg: std::collections::HashSet<_> = segs
        .iter()
        .filter(|s| s.channel == Channel::Mic)
        .map(|s| s.id)
        .collect();
    store
        .get_words(meeting.id, i64::MIN, i64::MAX)
        .unwrap()
        .into_iter()
        .filter(|w| mic_seg.contains(&w.segment_id))
        .map(|w| (w.start_ms, w.end_ms, w.text))
        .collect()
}

#[test]
#[ignore]
fn fleurs_owner_over_a_speaker_meeting() {
    let (Some(fleurs), Some(sample), Some(engine_dir), Some(models)) = (
        std::env::var_os("DIARIST_ECHO_FLEURS").map(PathBuf::from),
        std::env::var_os("DIARIST_ECHO_SAMPLE").map(PathBuf::from),
        std::env::var_os("DIARIST_ENGINE_DIR").map(PathBuf::from),
        std::env::var_os("DIARIST_MODELS_DIR").map(PathBuf::from),
    ) else {
        eprintln!(
            "DIARIST_ECHO_FLEURS, DIARIST_ECHO_SAMPLE, DIARIST_ENGINE_DIR and DIARIST_MODELS_DIR are needed"
        );
        return;
    };
    let minutes: usize = std::env::var("DIARIST_ECHO_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let out = std::env::var_os("DIARIST_ECHO_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("diarist-echo-fleurs"));
    std::fs::create_dir_all(&out).unwrap();

    // The far end: the loopback of the speaker meeting, from minute 1.
    let (pcm, ch) = decode_i16(&sample.join("playback.flac")).unwrap();
    assert_eq!(ch, 2);
    let n = minutes * 960_000;
    let lb: Vec<i16> = pcm
        .iter()
        .skip(1)
        .step_by(2)
        .skip(960_000)
        .take(n)
        .copied()
        .collect();
    drop(pcm);
    let n = lb.len();
    // Far-end activity per 20 ms frame (over −50 dBFS).
    let active: Vec<bool> = lb
        .chunks(320)
        .map(|c| {
            let e: f64 = c.iter().map(|&v| (f64::from(v) / 32_768.0).powi(2)).sum();
            10.0 * (e / c.len() as f64 + 1e-14).log10() > -50.0
        })
        .collect();

    // The owner: FLEURS utterances at random, non-overlapping times.
    let audio = hound::WavReader::open(fleurs.join("audio.wav"))
        .unwrap()
        .into_samples::<i16>()
        .map(Result::unwrap)
        .collect::<Vec<i16>>();
    let refs: Vec<serde_json::Value> = std::fs::read_to_string(fleurs.join("refs.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut owner = vec![0f32; n];
    let mut utts = Vec::new();
    let mut t = 16_000usize;
    for r in &refs {
        let a = (r["start"].as_f64().unwrap() * 16_000.0) as usize;
        let b = ((r["end"].as_f64().unwrap() * 16_000.0) as usize).min(audio.len());
        let len = b.saturating_sub(a);
        t += 16_000 + (rand() % (12 * 16_000)) as usize;
        if len == 0 || t + len >= n {
            break;
        }
        // −33 dBFS RMS: about 5 dB over the echo, as the owner in meeting 12.
        let seg: Vec<f32> = audio[a..b]
            .iter()
            .map(|&v| f32::from(v) / 32_768.0)
            .collect();
        let rms = (seg.iter().map(|v| v * v).sum::<f32>() / len as f32)
            .sqrt()
            .max(1e-6);
        let g = 10f32.powf(-33.0 / 20.0) / rms;
        for (o, v) in owner[t..t + len].iter_mut().zip(&seg) {
            *o = v * g;
        }
        let (f0, f1) = (t / 320, ((t + len) / 320).min(active.len()));
        let dt = active[f0..f1].iter().filter(|a| **a).count() as f64 / (f1 - f0).max(1) as f64;
        utts.push(Utt {
            at: t,
            len,
            words: norm_words(r["transcription"].as_str().unwrap_or("")),
            double_talk: dt > 0.3,
        });
        t += len;
    }
    // The mic: owner + the far end 3 ms later and 10 dB down + the room's
    // floor (−59 dBFS).
    let mut noise = 0x2545_F491_4F6C_DD1Du64;
    let mic: Vec<i16> = (0..n)
        .map(|i| {
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let w = ((noise >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
                * 10f32.powf(-59.0 / 20.0)
                * 3f32.sqrt();
            let e = if i >= 48 {
                f32::from(lb[i - 48]) / 32_768.0 * 10f32.powf(-10.0 / 20.0)
            } else {
                0.0
            };
            ((owner[i] + e + w) * 32_768.0)
                .round()
                .clamp(-32_768.0, 32_767.0) as i16
        })
        .collect();
    eprintln!(
        "fixture: {minutes} min, {} utterances ({} over the far end), {} reference words",
        utts.len(),
        utts.iter().filter(|u| u.double_talk).count(),
        utts.iter().map(|u| u.words.len()).sum::<usize>()
    );

    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf").expect("EN model");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let mut engine = NemoCliEngine::new(&engine_dir, &models, &asr, &diar);
    engine.pass_language = false;
    engine.right_context = Some(13);
    engine.diar_preset = Some("v3-offline".into());
    let off = score(&utts, &post_pass(&out, &engine, &mic, &lb, "off"));
    let on = score(&utts, &post_pass(&out, &engine, &mic, &lb, "auto"));
    eprintln!("| | gate off | gate on |");
    eprintln!("|---|---|---|");
    eprintln!(
        "| \"me\" WER on the stood-in speech | {:.2} % | {:.2} % |",
        off.wer, on.wer
    );
    eprintln!(
        "| the same, utterances over the far end | {:.2} % | {:.2} % |",
        off.wer_double_talk, on.wer_double_talk
    );
    eprintln!(
        "| the same, utterances in far-end pauses | {:.2} % | {:.2} % |",
        off.wer_clear, on.wer_clear
    );
    eprintln!("| \"me\" words | {} | {} |", off.me_words, on.me_words);
    eprintln!(
        "| echo words (outside every utterance) | {} | {} ({:.2} % removed) |",
        off.echo_words,
        on.echo_words,
        100.0 * (1.0 - on.echo_words as f64 / off.echo_words.max(1) as f64)
    );
    eprintln!(
        "| words recognised in double-talk utterances | {} | {} |",
        off.double_talk_words, on.double_talk_words
    );
}
