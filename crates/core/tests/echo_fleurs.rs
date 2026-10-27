//! The echo gate's double-talk fixture (plan 15 section 4A; dev only, GPU):
//! the owner is stood in by FLEURS English utterances with known
//! transcripts, placed at random times over the far end of a meeting held
//! on speakers, with meeting 12's echo path (3 ms, 10 dB return loss) into
//! the mic; the real post-pass runs with `echo.mode = off` and `auto`
//! (since plan 15 chunk B2a the canceller, then the gate; since section 9
//! the layered gate), and "me" is scored against the transcripts, with the
//! layered gate alone (no canceller, over the `off` run's words) between
//! them. Then the control: the same owner with no echo at all (a headphone
//! recording without a leak), `off` against `auto`, which must give the
//! same words. `#[ignore]`d and gated:
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
//! - `DIARIST_ECHO_BANDS` (optional, `keep/threshold/drop;...`): each band
//!   applied to the gate-off run's "me" words at the correlation look-backs
//!   350, 450 and 550 ms, the correlation layer alone and the layered gate.
//! - `DIARIST_ECHO_AEC_SWEEP=1`: the after-canceller variants of the layered
//!   gate (attenuation, correlation band, level layer) over the auto run's
//!   words before the gate, the canceller run again here as the post-pass
//!   runs it.
//! - The transfer gate (plan 15 section 9.2) per guard over the same
//!   words (`DIARIST_ECHO_TRANSFER=0` skips it): the off run's "me" words
//!   are the raw mic's transcription, as the post-pass makes it when the
//!   live run has no mic words (the auto run's live run has none, so its
//!   post-pass transcribes the raw mic once more).
//!   Since plan 16 section 12.2 it also prints the owner's level of both
//!   gates (`owner` lines: the estimator's sets, today's branch and the
//!   shipped one), today's decision, the shipped one and the floor rule at
//!   each F with the owner's level as estimated and forced unknown, and
//!   histograms of the level over the cancelled mic's floor (`hist` lines).
//! - `DIARIST_ECHO_UNKNOWN=a` (plan 16 section 12.2.3): the forced-unknown
//!   fixture instead, about ten owner utterances
//!   (`DIARIST_ECHO_UTTERANCES`, default 10), each placed over far-end
//!   speech (from the start of its slot of the far end, the first place
//!   where 80 percent of its frames carry the far end); `b` the same with
//!   the owner at [`UNKNOWN_B_OWNER_DBFS`], about 21 dB over the floor by
//!   the gate's level. `DIARIST_ECHO_OWNER_DBFS` sets the owner's RMS of
//!   any variant (default −33).
//! - Plan 16 section 12.7 (`share`, `unmasked` and `lost` lines): the raw
//!   gate's share of flagged words beside the silent tier's threshold, the
//!   floor rule with the post-canceller correlation layer off (the known
//!   guard against the owner's level forced unknown at each F, and at the
//!   owner's true level), and the owner's words the shipped decision
//!   removes that the known guard keeps.
//! - `DIARIST_ECHO_REUSE=1`: the post-passes' words from `pass-off.json`
//!   and `pass-auto.json` in `DIARIST_ECHO_OUT` (written by every run), so
//!   the in-test decisions run again without the GPU. `DIARIST_ECHO_KEEP=1`
//!   keeps them, the post-pass folders and databases, and writes the
//!   fixture's audio there (`fixture-mic.wav`, `fixture-loopback.wav`, the
//!   in-test canceller's `fixture-cancelled.wav`) with its owner's
//!   utterances (`fixture-utts.json`).
//! - `DIARIST_ECHO_CONTROL=0` skips the headphone control;
//!   `DIARIST_ECHO_DUMP` (a scratch folder) writes every after-canceller
//!   word's features with its text (local analysis; delete it after).
//!
//! Per utterance, the hypothesis is the "me" words whose middle falls in
//! the utterance's span (−0.2 s, +0.4 s); WER is summed edits over summed
//! reference words, for all utterances and for those spoken over the far
//! end (more than 30 percent of their 20 ms frames with the loopback over
//! −50 dBFS). Echo words are "me" words outside every utterance span.
//!
//! `cargo test -p diarist-core --release --test echo_fleurs -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use diarist_core::audio::aec::offline::{cancel_recording, OfflineOptions};
use diarist_core::audio::echo::layers::{self, GateWord, LayerCounts, LayerOptions};
use diarist_core::audio::echo::presence::{floor_sources, presence_sources};
use diarist_core::audio::echo::text::{self, norm, FarIndex, TextWindow};
use diarist_core::audio::echo::transfer::{self, Guard};
use diarist_core::audio::echo::{
    decide_in_order, score_sources, EchoMode, EchoProfile, GateBand, GatePad, PcmSource, WordScore,
};
use diarist_core::audio::flac::decode_i16;
use diarist_core::engine::NemoCliEngine;
use diarist_core::live::postpass::band_after_canceller;
use diarist_core::live::{run_post_pass, PostPassJob};
use diarist_core::pipeline::CancelToken;
use diarist_core::settings;
use diarist_core::store::{NewMeeting, RunStore, SqliteStore, Store};
use diarist_core::transcript::SegmentOptions;
use diarist_core::types::{Channel, JobId, MeetingStatus, RunKind, RunStatus, Source};

/// The owner's RMS in the forced-unknown fixture (b): about 21 dB over the
/// cancelled mic's floor by the gate's own level (plan 16 section 12.2.3;
/// at −33 dBFS the words the canceller left alone sit 32.5 dB over it).
const UNKNOWN_B_OWNER_DBFS: f32 = -44.5;

/// The floor rule's F sweep (plan 16 section 12.2.2 step 3).
const F_SWEEP: [f32; 5] = [9.0, 12.0, 15.0, 18.0, f32::INFINITY];

/// Counts of `v` (dB over the floor) per 3 dB bin: under 0, 0 to 3, ...,
/// 27 to 30, 30 and over.
fn hist(v: &[f32]) -> String {
    let mut bins = [0usize; 12];
    for &x in v {
        let k = if x < 0.0 {
            0
        } else {
            ((x / 3.0).floor() as usize + 1).min(11)
        };
        bins[k] += 1;
    }
    let mut s = format!("n {}:", v.len());
    for (k, n) in bins.iter().enumerate() {
        let label = match k {
            0 => "<0".to_owned(),
            11 => ">=30".to_owned(),
            _ => format!("{}-{}", 3 * (k - 1), 3 * k),
        };
        s.push_str(&format!(" {label} {n}"));
    }
    s
}

/// The owner's level of words in any order (plan 16 section 12.2.3, first
/// step): the estimator's sets, today's branch and the shipped one; returns
/// today's estimate.
fn owner_line(
    tag: &str,
    gw: &[GateWord],
    far: &FarIndex,
    floor: Option<f32>,
    canceller: bool,
) -> Option<f32> {
    let mut order: Vec<usize> = (0..gw.len()).collect();
    order.sort_by_key(|&i| (gw[i].start_ms, gw[i].end_ms, i));
    let sorted: Vec<GateWord> = order.iter().map(|&i| gw[i].clone()).collect();
    let s = layers::CalibrationStats::of(
        &sorted,
        &layers::text_runs(&sorted, far, TextWindow::DEFAULT),
    );
    let c = s.owner_counts();
    let today = s.owner(false, None);
    let now = s.owner(canceller, floor);
    let over = |o: Option<f32>| match (o, floor) {
        (Some(o), Some(f)) => format!("{:+.1} dB over the floor", o - f),
        _ => "-".into(),
    };
    let r1 = |v: Option<f32>| v.map(|x| (x * 10.0).round() / 10.0);
    eprintln!(
        "owner {tag}: {} words; quiet {}, unmatched {}, clean quiet {}, clean {}; today {} {:?} ({} words, {}); shipped {} {:?} ({} words, {}; high {}); floor {:?}",
        gw.len(),
        c.quiet,
        c.unmatched,
        c.clean_quiet,
        c.clean,
        today.source.as_str(),
        r1(today.db),
        today.words,
        over(today.db),
        now.source.as_str(),
        r1(now.db),
        now.words,
        over(now.db),
        now.high,
        r1(floor)
    );
    today.db
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

/// What one post-pass over the fixture gave.
struct Pass {
    /// "Me" words after the echo step.
    me: Vec<(i64, i64, String)>,
    /// The words the gate dropped.
    dropped: Vec<(i64, i64, String)>,
    /// The loopback's words.
    far: Vec<(i64, i64, String)>,
    echo: serde_json::Value,
}

fn words_json(w: &[(i64, i64, String)]) -> serde_json::Value {
    w.iter()
        .map(|w| serde_json::json!([w.0, w.1, w.2]))
        .collect()
}

fn words_of(v: &serde_json::Value) -> Vec<(i64, i64, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w[0].as_i64().unwrap(),
                w[1].as_i64().unwrap(),
                w[2].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// [`post_pass`], or its result from `pass-<name>.json` in `out` with
/// `DIARIST_ECHO_REUSE=1` (written by every run; deleted at the end unless
/// `DIARIST_ECHO_KEEP=1`).
fn post_pass_cached(
    out: &Path,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    mode: &str,
    name: &str,
) -> Pass {
    let cache = out.join(format!("pass-{name}.json"));
    if std::env::var("DIARIST_ECHO_REUSE").as_deref() == Ok("1") && cache.is_file() {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
        eprintln!("{name}: reused {}", cache.display());
        eprintln!("{name}: engine_info.echo {}", v["echo"]);
        return Pass {
            me: words_of(&v["me"]),
            dropped: words_of(&v["dropped"]),
            far: words_of(&v["far"]),
            echo: v["echo"].clone(),
        };
    }
    let p = post_pass(out, engine, mic, lb, mode, name);
    let v = serde_json::json!({
        "mode": mode,
        "echo": p.echo,
        "me": words_json(&p.me),
        "dropped": words_json(&p.dropped),
        "far": words_json(&p.far),
    });
    std::fs::write(&cache, serde_json::to_vec(&v).unwrap()).unwrap();
    p
}

/// A mono 16 kHz WAV of `x`.
fn write_wav(path: &Path, x: &[i16]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &v in x {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

/// One post-pass over the fixture with `mode` (`name` names its files).
fn post_pass(
    out: &Path,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    mode: &str,
    name: &str,
) -> Pass {
    let db = out.join(format!("{name}.db"));
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
    let folder = out.join(name);
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
        punct: diarist_core::punct::PunctSource::None,
    };
    let report = run_post_pass(&store, engine, &job, &CancelToken::default(), &|_| {}).unwrap();
    let run = store.get_run(report.current_run).unwrap();
    eprintln!("{name}: engine_info.echo {}", run.engine_info["echo"]);
    let mut dropped = Vec::new();
    if let Some(d) = store.echo_dropped(report.current_run).unwrap() {
        dropped = d
            .iter()
            .map(|w| (w.start_ms, w.end_ms, w.text.clone()))
            .collect();
        let mut c: Vec<f32> = d.iter().map(|w| w.coefficient).collect();
        c.sort_by(f32::total_cmp);
        let q = |f: f64| {
            c.get(((c.len() as f64 - 1.0) * f) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        eprintln!(
            "{name}: dropped coefficients p10 {:.2} p50 {:.2} p90 {:.2}; {} at or over 0.5",
            q(0.1),
            q(0.5),
            q(0.9),
            c.iter().filter(|v| **v >= 0.5).count()
        );
    }
    let segs = store.get_segments(meeting.id, 0, u32::MAX).unwrap();
    let mic_seg: std::collections::HashSet<_> = segs
        .iter()
        .filter(|s| s.channel == Channel::Mic)
        .map(|s| s.id)
        .collect();
    let (mut me, mut far) = (Vec::new(), Vec::new());
    for w in store.get_words(meeting.id, i64::MIN, i64::MAX).unwrap() {
        let v = (w.start_ms, w.end_ms, w.text);
        if mic_seg.contains(&w.segment_id) {
            me.push(v);
        } else {
            far.push(v);
        }
    }
    me.sort();
    far.sort();
    drop(store);
    if std::env::var("DIARIST_ECHO_KEEP").as_deref() != Ok("1") {
        let _ = std::fs::remove_dir_all(out.join(name));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
    }
    Pass {
        me,
        dropped,
        far,
        echo: run.engine_info["echo"].clone(),
    }
}

/// The layered gate over the echo-off run's "me" words without the
/// canceller (the gate alone): correlation (with `pad` and `band`) where
/// the fixture couples, levels on the raw mic, text against the loopback's
/// words. Returns the kept words and the counts per layer.
#[allow(clippy::too_many_arguments)]
fn layered_alone(
    words: &[(i64, i64, String)],
    far: &[(i64, i64, String)],
    mic: &[i16],
    lb: &[i16],
    p: &EchoProfile,
    pad: GatePad,
    band: GateBand,
    correlation_only: bool,
) -> (Vec<(i64, i64, String)>, LayerCounts) {
    let spans: Vec<(i64, i64)> = words
        .iter()
        .map(|w| (w.0 * 16, w.1.max(w.0) * 16))
        .collect();
    let correlation = p.gates(EchoMode::Auto, false);
    let scores = if correlation {
        score_sources(&spans, mic, lb, p, pad)
    } else {
        vec![WordScore::Undefined; spans.len()]
    };
    if correlation_only {
        let d = decide_in_order(&spans, &scores, band);
        let kept = words
            .iter()
            .zip(&d)
            .filter(|p| !*p.1)
            .map(|p| p.0.clone())
            .collect();
        return (kept, LayerCounts::default());
    }
    let pres = presence_sources(&spans, mic, None, lb, None);
    let floor = floor_sources(mic, lb, None).floor_db();
    let groups = layers::gap_groups(words.iter().map(|w| (w.0, w.1)));
    let gw: Vec<GateWord> = (0..words.len())
        .map(|i| GateWord {
            start_ms: words[i].0,
            end_ms: words[i].1,
            norm: norm(&words[i].2),
            group: groups[i],
            score: scores[i],
            presence: pres[i],
        })
        .collect();
    let fi = FarIndex::new(far.iter().map(|w| (w.0, norm(&w.2))));
    let r = layers::gate(
        &gw,
        &fi,
        floor,
        &LayerOptions {
            correlation,
            canceller: false,
            fixed: band,
            threshold: band.threshold,
            window: TextWindow::DEFAULT,
            calibrate: true,
        },
    );
    let kept = words
        .iter()
        .zip(&r.reasons)
        .filter(|p| p.1.is_none())
        .map(|p| p.0.clone())
        .collect();
    (kept, r.counts)
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
    // `DIARIST_ECHO_UNKNOWN=a|b` (plan 16 section 12.2.3): about ten owner
    // utterances, each placed over far-end speech; (b) with the owner
    // quieter (`DIARIST_ECHO_OWNER_DBFS`), about 21 dB over the floor.
    let unknown = std::env::var("DIARIST_ECHO_UNKNOWN")
        .ok()
        .filter(|v| v == "a" || v == "b");
    let owner_dbfs: f32 = std::env::var("DIARIST_ECHO_OWNER_DBFS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if unknown.as_deref() == Some("b") {
            UNKNOWN_B_OWNER_DBFS
        } else {
            -33.0
        });
    if let Some(v) = &unknown {
        let want: usize = std::env::var("DIARIST_ECHO_UTTERANCES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let share = |a: usize, len: usize| {
            let (f0, f1) = (a / 320, ((a + len) / 320).min(active.len()));
            active[f0..f1].iter().filter(|x| **x).count() as f64 / (f1 - f0).max(1) as f64
        };
        let slot = n / want;
        for r in &refs {
            if utts.len() >= want {
                break;
            }
            let a = (r["start"].as_f64().unwrap() * 16_000.0) as usize;
            let b = ((r["end"].as_f64().unwrap() * 16_000.0) as usize).min(audio.len());
            let len = b.saturating_sub(a);
            if len == 0 || len + 16_000 > slot {
                continue;
            }
            // From the start of its slot, the first place where 80 percent of
            // its frames carry the far end, else the slot's best.
            let lo = slot * utts.len() + 16_000;
            let hi = (slot * (utts.len() + 1)).min(n) - len;
            let mut best = (-1.0f64, lo);
            let mut s = lo;
            while s < hi {
                let sh = share(s, len);
                if sh > best.0 {
                    best = (sh, s);
                }
                if sh >= 0.8 {
                    break;
                }
                s += 4_000;
            }
            let t = best.1;
            let seg: Vec<f32> = audio[a..b]
                .iter()
                .map(|&v| f32::from(v) / 32_768.0)
                .collect();
            let rms = (seg.iter().map(|v| v * v).sum::<f32>() / len as f32)
                .sqrt()
                .max(1e-6);
            let g = 10f32.powf(owner_dbfs / 20.0) / rms;
            for (o, v) in owner[t..t + len].iter_mut().zip(&seg) {
                *o = v * g;
            }
            utts.push(Utt {
                at: t,
                len,
                words: norm_words(r["transcription"].as_str().unwrap_or("")),
                double_talk: share(t, len) > 0.3,
            });
        }
        eprintln!(
            "forced-unknown fixture ({v}): {} utterances over far-end speech at {owner_dbfs} dBFS, far-end share of their frames {:?}",
            utts.len(),
            utts.iter()
                .map(|u| (share(u.at, u.len) * 100.0).round() / 100.0)
                .collect::<Vec<_>>()
        );
    }
    let mut t = 16_000usize;
    for r in refs.iter().filter(|_| unknown.is_none()) {
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
        let g = 10f32.powf(owner_dbfs / 20.0) / rms;
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
    // The mic: owner + the far end 3 ms later and 10 dB down (`echo`) + the
    // room's floor (−59 dBFS). Without the echo it is a headphone recording
    // with no leak: the control.
    let make_mic = |echo: bool| -> Vec<i16> {
        let mut noise = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|i| {
                noise ^= noise << 13;
                noise ^= noise >> 7;
                noise ^= noise << 17;
                let w = ((noise >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
                    * 10f32.powf(-59.0 / 20.0)
                    * 3f32.sqrt();
                let e = if echo && i >= 48 {
                    f32::from(lb[i - 48]) / 32_768.0 * 10f32.powf(-10.0 / 20.0)
                } else {
                    0.0
                };
                ((owner[i] + e + w) * 32_768.0)
                    .round()
                    .clamp(-32_768.0, 32_767.0) as i16
            })
            .collect()
    };
    let mic = make_mic(true);
    eprintln!(
        "fixture: {minutes} min, {} utterances ({} over the far end), {} reference words",
        utts.len(),
        utts.iter().filter(|u| u.double_talk).count(),
        utts.iter().map(|u| u.words.len()).sum::<usize>()
    );
    // `DIARIST_ECHO_KEEP=1`: the fixture's audio and its owner's utterances
    // stay in `out` with the passes' words (plan 16 section 12.7, chunk M):
    // `fixture-mic.wav`, `fixture-loopback.wav`, `fixture-utts.json`
    // (placement in samples, reference words), `pass-off.json`,
    // `pass-auto.json`, and the in-test canceller's output
    // `fixture-cancelled.wav`.
    let keep = std::env::var("DIARIST_ECHO_KEEP").as_deref() == Ok("1");
    if keep {
        write_wav(&out.join("fixture-mic.wav"), &mic);
        write_wav(&out.join("fixture-loopback.wav"), &lb);
        let u: Vec<serde_json::Value> = utts
            .iter()
            .map(|u| {
                serde_json::json!({
                    "at": u.at,
                    "len": u.len,
                    "words": u.words,
                    "double_talk": u.double_talk,
                })
            })
            .collect();
        std::fs::write(
            out.join("fixture-utts.json"),
            serde_json::to_vec(&serde_json::json!({
                "minutes": minutes,
                "unknown": unknown,
                "owner_dbfs": owner_dbfs,
                "utterances": u,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf").expect("EN model");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let mut engine = NemoCliEngine::new(&engine_dir, &models, &asr, &diar);
    engine.pass_language = false;
    engine.right_context = Some(13);
    engine.diar_preset = Some("v3-offline".into());
    let off_pass = post_pass_cached(&out, &engine, &mic, &lb, "off", "off");
    let off = score(&utts, &off_pass.me);
    let on_pass = post_pass_cached(&out, &engine, &mic, &lb, "auto", "auto");
    let on = score(&utts, &on_pass.me);
    // The gate alone (no canceller) over the gate-off run's words, with the
    // post-pass's own scan (no clock maps in the fixture: meeting time is
    // channel time): the layered gate, and the correlation layer by itself.
    let p = EchoProfile::scan_sources(&mic[..], &lb[..], None);
    eprintln!(
        "fixture profile: {:?} coupling {:.3} lag {:?} ms erl {:?} dB; active blocks p50 {:?}",
        p.status,
        p.coupling,
        p.lag_ms,
        p.erl_db,
        {
            let mut a = p.active.clone();
            a.sort_by(f32::total_cmp);
            a.get(a.len() / 2).copied()
        }
    );
    let (alone_words, alone_counts) = layered_alone(
        &off_pass.me,
        &off_pass.far,
        &mic,
        &lb,
        &p,
        GatePad::DEFAULT,
        GateBand::default(),
        false,
    );
    let alone = score(&utts, &alone_words);
    eprintln!("layered gate alone: {alone_counts:?}");
    let bands_of = |var: &str| -> Vec<GateBand> {
        std::env::var(var)
            .ok()
            .map(|v| {
                v.split(';')
                    .filter_map(|b| {
                        let f: Vec<f32> =
                            b.split('/').filter_map(|x| x.trim().parse().ok()).collect();
                        match f[..] {
                            [keep_under, threshold, drop_at] => Some(GateBand {
                                keep_under,
                                threshold,
                                drop_at,
                            }),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    // `DIARIST_ECHO_BANDS`: the gate alone per band and look-back (350,
    // 450, 550 ms), the correlation layer by itself and the layered gate.
    let sweep = bands_of("DIARIST_ECHO_BANDS");
    if !sweep.is_empty() {
        eprintln!("| Look-back | Band (keep / neighbours / drop) | Correlation alone: \"me\" WER | over the far end | Echo words left | Layered: \"me\" WER | over the far end | Echo words left |");
        eprintln!("|---|---|---|---|---|---|---|---|");
        for before_ms in [350i64, 450, 550] {
            let pad = GatePad {
                before: before_ms * 16,
                after: GatePad::DEFAULT.after,
            };
            for b in &sweep {
                let (c, _) =
                    layered_alone(&off_pass.me, &off_pass.far, &mic, &lb, &p, pad, *b, true);
                let (l, _) =
                    layered_alone(&off_pass.me, &off_pass.far, &mic, &lb, &p, pad, *b, false);
                let (c, l) = (score(&utts, &c), score(&utts, &l));
                eprintln!(
                    "| {before_ms} ms | {:.2} / {:.2} / {:.2} | {:.2} % | {:.2} % | {} | {:.2} % | {:.2} % | {} |",
                    b.keep_under,
                    b.threshold,
                    b.drop_at,
                    c.wer,
                    c.wer_double_talk,
                    c.echo_words,
                    l.wer,
                    l.wer_double_talk,
                    l.echo_words
                );
            }
        }
    }
    let removed = |s: &Scores| 100.0 * (1.0 - s.echo_words as f64 / off.echo_words.max(1) as f64);
    eprintln!("| | echo off | layered gate alone | canceller + layered gate |");
    eprintln!("|---|---|---|---|");
    eprintln!(
        "| \"me\" WER on the stood-in speech | {:.2} % | {:.2} % | {:.2} % |",
        off.wer, alone.wer, on.wer
    );
    eprintln!(
        "| the same, utterances over the far end | {:.2} % | {:.2} % | {:.2} % |",
        off.wer_double_talk, alone.wer_double_talk, on.wer_double_talk
    );
    eprintln!(
        "| the same, utterances in far-end pauses | {:.2} % | {:.2} % | {:.2} % |",
        off.wer_clear, alone.wer_clear, on.wer_clear
    );
    eprintln!(
        "| \"me\" words | {} | {} | {} |",
        off.me_words, alone.me_words, on.me_words
    );
    eprintln!(
        "| echo words (outside every utterance) | {} | {} ({:.2} % removed) | {} ({:.2} % removed) |",
        off.echo_words,
        alone.echo_words,
        removed(&alone),
        on.echo_words,
        removed(&on)
    );
    eprintln!(
        "| words recognised in double-talk utterances | {} | {} | {} |",
        off.double_talk_words, alone.double_talk_words, on.double_talk_words
    );
    eprintln!(
        "auto: {} words dropped; layers {}",
        on_pass.dropped.len(),
        on_pass.echo["layers"]
    );
    if std::env::var("DIARIST_ECHO_AEC_SWEEP").as_deref() == Ok("1") {
        aec_sweep(&utts, &on_pass, &mic, &lb, &p);
    }
    if std::env::var("DIARIST_ECHO_TRANSFER").as_deref() != Ok("0") {
        let cancelled = transfer_sweep(&utts, &on_pass, &off_pass, &mic, &lb, &p);
        if keep {
            write_wav(&out.join("fixture-cancelled.wav"), &cancelled);
        }
    }

    // The control: the same owner with headphones and no leak. `auto` must
    // give exactly what `off` gives.
    if std::env::var("DIARIST_ECHO_CONTROL").as_deref() != Ok("0") {
        let hp = make_mic(false);
        let a = post_pass(&out, &engine, &hp, &lb, "off", "hp-off");
        let b = post_pass(&out, &engine, &hp, &lb, "auto", "hp-auto");
        let fi = FarIndex::new(b.far.iter().map(|w| (w.0, norm(&w.2))));
        let mic_t: Vec<(i64, String)> = b.me.iter().map(|w| (w.0, norm(&w.2))).collect();
        let mic_r: Vec<(i64, &str)> = mic_t.iter().map(|w| (w.0, w.1.as_str())).collect();
        let runs = text::runs(&mic_r, &fi, TextWindow::DEFAULT);
        let shifted: Vec<(i64, &str)> =
            mic_t.iter().map(|w| (w.0 + 45_000, w.1.as_str())).collect();
        let chance = text::runs(&shifted, &fi, TextWindow::DEFAULT);
        eprintln!(
            "headphone control: {} \"me\" words off, {} auto, identical {}; status {}, dropped {}, text runs {} (chance {}), layers {}",
            a.me.len(),
            b.me.len(),
            a.me == b.me,
            b.echo["coupled"],
            b.echo["dropped"],
            runs.count3() + runs.count2(),
            chance.count3() + chance.count2(),
            b.echo["layers"]
        );
        assert_eq!(a.me, b.me, "the headphone control changed");
        assert_eq!(b.echo["dropped"].as_u64(), Some(0));
    }
    if !keep {
        for name in ["off", "auto"] {
            let _ = std::fs::remove_file(out.join(format!("pass-{name}.json")));
        }
    }
}

/// The transfer gate's guards (module docs) over the auto run's words
/// before the gate, the canceller run again here as the post-pass runs it,
/// the off run's words as the raw mic's; and the post-canceller layers
/// alone at both lag spreads. Returns the canceller's output.
fn transfer_sweep(
    utts: &[Utt],
    auto: &Pass,
    off: &Pass,
    mic: &[i16],
    lb: &[i16],
    p: &EchoProfile,
) -> Vec<i16> {
    let mut words: Vec<(i64, i64, String)> = auto.me.iter().chain(&auto.dropped).cloned().collect();
    words.sort();
    let hint = |t: i64| p.lag_at(t);
    let mut o = OfflineOptions::post_pass(None);
    o.lag_hint = Some(&hint);
    let mut y: Vec<i16> = Vec::with_capacity(mic.len());
    cancel_recording(&mic, &lb, &o, &mut |b| {
        y.extend(
            b.iter()
                .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16),
        );
        Ok(())
    })
    .unwrap();
    let gate_words = |ws: &[(i64, i64, String)], heard: &[i16], cancelled: bool| {
        let spans: Vec<(i64, i64)> = ws.iter().map(|w| (w.0 * 16, w.1.max(w.0) * 16)).collect();
        let scores = score_sources(&spans, mic, lb, p, GatePad::DEFAULT);
        let raw: Option<Box<dyn PcmSource>> =
            cancelled.then(|| Box::new(mic) as Box<dyn PcmSource>);
        let pres = presence_sources(&spans, heard, raw, lb, None);
        let groups = layers::gap_groups(ws.iter().map(|w| (w.0, w.1)));
        (0..ws.len())
            .map(|i| GateWord {
                start_ms: ws[i].0,
                end_ms: ws[i].1,
                norm: norm(&ws[i].2),
                group: groups[i],
                score: scores[i],
                presence: pres[i],
            })
            .collect::<Vec<GateWord>>()
    };
    let gw = gate_words(&words, &y, true);
    let raw_gw = gate_words(&off.me, mic, false);
    let floor = floor_sources(&y[..], lb, None).floor_db();
    let raw_floor = floor_sources(mic, lb, None).floor_db();
    let fi = FarIndex::new(auto.far.iter().map(|w| (w.0, norm(&w.2))));
    let after = LayerOptions {
        correlation: true,
        canceller: true,
        fixed: band_after_canceller(0.5),
        threshold: 0.5,
        window: TextWindow::DEFAULT,
        calibrate: false,
    };
    let raw_opts = LayerOptions {
        canceller: false,
        fixed: GateBand::default(),
        calibrate: true,
        ..after
    };
    let line = |name: &str, reasons: &[Option<diarist_core::types::EchoReason>], extra: String| {
        let kept: Vec<(i64, i64, String)> = words
            .iter()
            .zip(reasons)
            .filter(|p| p.1.is_none())
            .map(|p| p.0.clone())
            .collect();
        let s = score(utts, &kept);
        eprintln!(
            "| {name} | {extra} | {} | {:.2} % | {:.2} % | {:.2} % | {} | {} | {:.2} % | {} of {} |",
            words.len() - kept.len(),
            s.wer,
            s.wer_double_talk,
            s.wer_clear,
            s.echo_words,
            s.me_words,
            100.0 * s.echo_words as f64 / s.me_words.max(1) as f64,
            s.me_words - s.echo_words,
            score(utts, &words).me_words - score(utts, &words).echo_words
        );
    };
    eprintln!("| Transfer gate (FLEURS): guard | Raw echo, candidates, removed (text, level, plain) | Dropped | \"me\" WER | over the far end | in far-end pauses | Echo words left | Kept | Echo share of kept | Owner's words kept |");
    eprintln!("|---|---|---|---|---|---|---|---|---|---|");
    line(
        "canceller, no gate",
        &vec![None; words.len()],
        String::new(),
    );
    for spread_ms in [300i64, 500] {
        let o = LayerOptions {
            window: TextWindow {
                spread_ms,
                ..TextWindow::DEFAULT
            },
            ..after
        };
        let r = layers::gate(&gw, &fi, floor, &o);
        line(
            &format!("layers alone, spread {spread_ms} ms"),
            &r.reasons,
            format!("{:?}", r.counts),
        );
    }
    for guard in Guard::ALL {
        let t = transfer::gate(
            &raw_gw, raw_floor, &raw_opts, &gw, floor, &after, &fi, guard,
        );
        let c = t.transfer;
        line(
            guard.as_str(),
            &t.reasons,
            format!(
                "{} of {} {:?}; {}, {} ({}, {}, {}); {:?}",
                t.raw.dropped(),
                raw_gw.len(),
                t.raw.counts,
                c.candidates,
                c.removed(),
                c.text,
                c.level,
                c.plain,
                t.counts
            ),
        );
    }
    // Plan 16 section 12.2: the owner's level of both gates; today's
    // decision (today's owner's level on the cancelled words), the shipped
    // one, the floor rule at each F with the owner's level as estimated and
    // forced unknown; histograms of the level over the cancelled mic's floor.
    let today = owner_line("FLEURS cancelled", &gw, &fi, floor, true);
    owner_line("FLEURS raw (off run)", &raw_gw, &fi, raw_floor, false);
    let decide = |rule: transfer::Rule, owner: Option<Option<f32>>| {
        transfer::gate_with(
            &raw_gw,
            raw_floor,
            &raw_opts,
            &gw,
            floor,
            &after,
            &fi,
            &rule,
            &move |c: &mut layers::Context| {
                if let Some(o) = owner {
                    c.owner_db = o;
                }
            },
        )
    };
    let rule_line = |name: &str, t: &transfer::TransferReport| {
        let c = t.transfer;
        line(
            name,
            &t.reasons,
            format!(
                "owner {:?} ({}); removed {} (level {}, floor rule {} {:?}), guarded {}; {:?}",
                t.after.owner_db.map(|v| (v * 10.0).round() / 10.0),
                t.after.owner_source.as_str(),
                c.removed(),
                c.level,
                c.floor,
                t.floor_rule.map(|f| (f.tier.as_str(), f.f_db)),
                c.guarded,
                t.counts
            ),
        );
    };
    let t_today = decide(transfer::Rule::SHIPPED, Some(today));
    rule_line("today (today's owner's level)", &t_today);
    rule_line(
        &format!(
            "shipped (F_u {} dB, F_s {} dB)",
            transfer::FLOOR_MARGIN_DB,
            transfer::FLOOR_MARGIN_SILENT_DB
        ),
        &decide(transfer::Rule::SHIPPED, None),
    );
    // The floor rule at F in either tier.
    let at = |f: f32| transfer::Rule {
        floor_margin_db: f,
        floor_margin_silent_db: f,
        ..transfer::Rule::SHIPPED
    };
    for f in F_SWEEP {
        rule_line(&format!("F {f} as estimated"), &decide(at(f), None));
        rule_line(&format!("F {f} forced unknown"), &decide(at(f), Some(None)));
    }
    let raw_words: Vec<transfer::RawWord> = raw_gw
        .iter()
        .zip(&t_today.raw.reasons)
        .map(|(w, r)| transfer::RawWord {
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            norm: &w.norm,
            echo: r.is_some(),
        })
        .collect();
    let (cand, _) = transfer::transfer(&gw, &raw_words, None, None, Guard::Plain);
    let inside = |w: &(i64, i64, String)| {
        utts.iter().any(|u| {
            let (a, b) = (
                (u.at as f64 / 16.0) as i64 - 200,
                ((u.at + u.len) as f64 / 16.0) as i64 + 400,
            );
            (a..b).contains(&((w.0 + w.1) / 2))
        })
    };
    let over = |i: usize| gw[i].presence.zip(floor).map(|(p, f)| p.level_db - f);
    let cut = |i: usize| {
        gw[i]
            .presence
            .and_then(|p| p.attenuation_db)
            .is_some_and(|a| a >= transfer::ATTENUATION_DB)
    };
    let h = |sel: &dyn Fn(usize) -> bool| -> String {
        let v: Vec<f32> = (0..gw.len()).filter(|&i| sel(i)).filter_map(over).collect();
        hist(&v)
    };
    eprintln!(
        "hist FLEURS: owner's words (in the utterances), candidates cut 3 dB or more: {}",
        h(&|i| inside(&words[i]) && cand[i] && cut(i))
    );
    eprintln!(
        "hist FLEURS: owner's words, cut 3 dB or more: {}",
        h(&|i| inside(&words[i]) && cut(i))
    );
    eprintln!(
        "hist FLEURS: owner's words, cut under 3 dB: {}",
        h(&|i| inside(&words[i]) && !cut(i))
    );
    eprintln!(
        "hist FLEURS: echo words kept today, candidates cut 3 dB or more: {}",
        h(&|i| !inside(&words[i]) && t_today.reasons[i].is_none() && cand[i] && cut(i))
    );
    let mut own_clean: Vec<f32> = (0..gw.len())
        .filter(|&i| inside(&words[i]) && !cut(i))
        .filter_map(over)
        .collect();
    own_clean.sort_by(f32::total_cmp);
    eprintln!(
        "FLEURS: the owner's words the canceller left alone sit {:?} dB over the floor (median of {})",
        own_clean.get(own_clean.len() / 2),
        own_clean.len()
    );

    // Plan 16 section 12.7 (ii): the raw gate's share of flagged words
    // beside the silent tier's threshold; the shipped decision's owner's
    // level, source and tier; then the floor rule's own cost with the
    // post-canceller correlation layer off (it masks the rule: a word two
    // layers remove counts once), the known guard against the owner's
    // level forced unknown at each F, and the known guard at the owner's
    // true level (the median after the canceller of the owner's words it
    // left alone, else of all of them).
    let mut att: Vec<f32> = (0..gw.len())
        .filter(|&i| inside(&words[i]))
        .filter_map(|i| gw[i].presence.and_then(|p| p.attenuation_db))
        .collect();
    att.sort_by(f32::total_cmp);
    let q = |f: f64| att.get(((att.len().max(1) - 1) as f64 * f).round() as usize);
    eprintln!(
        "att FLEURS: the owner's words (in the utterances), {} with a cut: p50 {:?} p90 {:?} dB; {} cut by 3 dB or more",
        att.len(),
        q(0.5),
        q(0.9),
        att.iter().filter(|a| **a >= transfer::ATTENUATION_DB).count()
    );
    let shipped = decide(transfer::Rule::SHIPPED, None);
    eprintln!(
        "share FLEURS: raw gate flagged {} of {} ({:.2} %; silent tier at {:.1} %); shipped: owner {:?} ({}, {} words, low {}, high {}), floor rule {:?}",
        shipped.raw.dropped(),
        raw_gw.len(),
        100.0 * shipped.raw_echo_share,
        100.0 * transfer::SILENT_SHARE,
        shipped.after.owner_db.map(|v| (v * 10.0).round() / 10.0),
        shipped.after.owner_source.as_str(),
        shipped.after.owner_words,
        shipped.after.owner_low,
        shipped.after.owner_high,
        shipped.floor_rule.map(|f| (f.tier.as_str(), f.f_db))
    );
    let true_level = {
        let mut v: Vec<f32> = (0..gw.len())
            .filter(|&i| inside(&words[i]) && !cut(i))
            .filter_map(|i| gw[i].presence.map(|p| p.level_db))
            .collect();
        if v.len() < 5 {
            v = (0..gw.len())
                .filter(|&i| inside(&words[i]))
                .filter_map(|i| gw[i].presence.map(|p| p.level_db))
                .collect();
        }
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let unmasked = |rule: transfer::Rule, owner: Option<Option<f32>>| {
        transfer::gate_with(
            &raw_gw,
            raw_floor,
            &raw_opts,
            &gw,
            floor,
            &after,
            &fi,
            &rule,
            &move |c: &mut layers::Context| {
                c.correlation_min_attenuation_db = Some(f32::INFINITY);
                if let Some(o) = owner {
                    c.owner_db = o;
                }
            },
        )
    };
    eprintln!(
        "unmasked FLEURS: the owner's true level {:?} dBFS ({:?} dB over the floor)",
        true_level.map(|v| (v * 10.0).round() / 10.0),
        true_level
            .zip(floor)
            .map(|(o, f)| ((o - f) * 10.0).round() / 10.0)
    );
    let known_est = unmasked(transfer::Rule::SHIPPED, None);
    let known_true = unmasked(transfer::Rule::SHIPPED, Some(true_level));
    rule_line("unmasked, shipped (as estimated)", &known_est);
    rule_line("unmasked, known guard (true level)", &known_true);
    for f in [9.0f32, 12.0, 15.0, 18.0] {
        rule_line(
            &format!("unmasked, F {f} forced unknown"),
            &unmasked(at(f), Some(None)),
        );
    }
    rule_line(
        "masked (shipped), known guard (true level)",
        &decide(transfer::Rule::SHIPPED, Some(true_level)),
    );
    // The owner's words the shipped decision removes and the known guard
    // at the true level keeps, by level over the floor (both masked and
    // unmasked): the natural-unknown runs' bar.
    for (name, a, b) in [
        (
            "masked",
            &shipped,
            &decide(transfer::Rule::SHIPPED, Some(true_level)),
        ),
        ("unmasked", &known_est, &known_true),
    ] {
        let lost: Vec<f32> = (0..gw.len())
            .filter(|&i| inside(&words[i]) && a.reasons[i].is_some() && b.reasons[i].is_none())
            .filter_map(over)
            .map(|v| (v * 10.0).round() / 10.0)
            .collect();
        let gained = (0..gw.len())
            .filter(|&i| inside(&words[i]) && a.reasons[i].is_none() && b.reasons[i].is_some())
            .count();
        eprintln!(
            "lost FLEURS {name}: the shipped decision removes {} owner's words the known guard (true level) keeps, at {lost:?} dB over the floor ({} within {} dB of it); keeps {gained} it removes",
            lost.len(),
            lost.iter().filter(|v| **v <= layers::PRESENCE_DB).count(),
            layers::PRESENCE_DB
        );
    }
    y
}

/// The after-canceller variants of the layered gate (`DIARIST_ECHO_AEC_SWEEP=1`):
/// the auto run's words before the gate, the canceller run again here as
/// the post-pass runs it, each variant decided over the same features.
fn aec_sweep(utts: &[Utt], auto: &Pass, mic: &[i16], lb: &[i16], p: &EchoProfile) {
    let mut words: Vec<(i64, i64, String)> = auto.me.iter().chain(&auto.dropped).cloned().collect();
    words.sort();
    let hint = |t: i64| p.lag_at(t);
    let mut o = OfflineOptions::post_pass(None);
    o.lag_hint = Some(&hint);
    let mut y: Vec<i16> = Vec::with_capacity(mic.len());
    cancel_recording(&mic, &lb, &o, &mut |b| {
        y.extend(
            b.iter()
                .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16),
        );
        Ok(())
    })
    .unwrap();
    let spans: Vec<(i64, i64)> = words
        .iter()
        .map(|w| (w.0 * 16, w.1.max(w.0) * 16))
        .collect();
    let scores = score_sources(&spans, mic, lb, p, GatePad::DEFAULT);
    let raw: Option<Box<dyn PcmSource>> = Some(Box::new(mic));
    let pres = presence_sources(&spans, &y[..], raw, lb, None);
    let floor = floor_sources(&y[..], lb, None).floor_db();
    let groups = layers::gap_groups(words.iter().map(|w| (w.0, w.1)));
    let gw: Vec<GateWord> = (0..words.len())
        .map(|i| GateWord {
            start_ms: words[i].0,
            end_ms: words[i].1,
            norm: norm(&words[i].2),
            group: groups[i],
            score: scores[i],
            presence: pres[i],
        })
        .collect();
    let fi = FarIndex::new(auto.far.iter().map(|w| (w.0, norm(&w.2))));
    let opts = LayerOptions {
        correlation: true,
        canceller: true,
        fixed: band_after_canceller(0.5),
        threshold: 0.5,
        window: TextWindow::DEFAULT,
        calibrate: true,
    };
    if let Some(dir) = std::env::var_os("DIARIST_ECHO_DUMP").map(std::path::PathBuf::from) {
        // Every word's features (local analysis; delete after).
        std::fs::create_dir_all(&dir).unwrap();
        let r = layers::gate(&gw, &fi, floor, &opts);
        let raw_pres = presence_sources(&spans, mic, None, lb, None);
        let energy = |x: &[i16], a: usize, b: usize| -> f64 {
            x[a.min(x.len())..b.min(x.len())]
                .iter()
                .map(|&v| (f64::from(v) / 32_768.0).powi(2))
                .sum()
        };
        let inside = |w: &(i64, i64, String)| {
            utts.iter().any(|u| {
                let (a, b) = (
                    (u.at as f64 / 16.0) as i64 - 200,
                    ((u.at + u.len) as f64 / 16.0) as i64 + 400,
                );
                let mid = (w.0 + w.1) / 2;
                (a..b).contains(&mid)
            })
        };
        let mut t = String::from(
            "start\tend\ttext\tin_utt\tlevel\tfar\tatt\tcoef\treason\traw_level\tspan_att\n",
        );
        for (i, w) in words.iter().enumerate() {
            let p = gw[i].presence;
            let (a, b) = (w.0 as usize * 16, (w.1.max(w.0 + 20)) as usize * 16);
            let (er, ec) = (energy(mic, a, b), energy(&y, a, b));
            let span_att = if er > 0.0 && ec > 0.0 {
                10.0 * (er / ec).log10()
            } else {
                0.0
            };
            t.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\n",
                w.0,
                w.1,
                norm(&w.2),
                u8::from(inside(w)),
                p.map_or(String::new(), |p| format!("{:.2}", p.level_db)),
                p.map_or(String::new(), |p| u8::from(p.far_active).to_string()),
                p.and_then(|p| p.attenuation_db)
                    .map_or(String::new(), |a| format!("{a:.2}")),
                gw[i]
                    .score
                    .coefficient()
                    .map_or(String::new(), |c| format!("{c:.3}")),
                r.reasons[i].map_or("", |r| r.as_str()),
                raw_pres[i].map_or(String::new(), |p| format!("{:.2}", p.level_db)),
                span_att
            ));
        }
        std::fs::write(dir.join("fleurs.tsv"), t).unwrap();
    }
    let none = score(utts, &words);
    eprintln!(
        "after the canceller, no gate: WER {:.2} % (over the far end {:.2} %), {} echo words",
        none.wer, none.wer_double_talk, none.echo_words
    );
    eprintln!("| Attenuation at / margin (dB) | and coefficient at | Correlation band | Level layer | Dropped | \"me\" WER | over the far end | Echo words left |");
    eprintln!("|---|---|---|---|---|---|---|---|");
    for v in aec_variants() {
        let r = layers::gate_with(&gw, &fi, floor, &opts, &|c| v.apply(c));
        let kept: Vec<(i64, i64, String)> = words
            .iter()
            .zip(&r.reasons)
            .filter(|p| p.1.is_none())
            .map(|p| p.0.clone())
            .collect();
        let s = score(utts, &kept);
        eprintln!(
            "{} {} ({:?}) | {:.2} % | {:.2} % | {} |",
            v.label(),
            r.dropped(),
            r.counts,
            s.wer,
            s.wer_double_talk,
            s.echo_words
        );
    }
}

/// One after-canceller variant of the layered gate's parameters.
#[derive(Debug, Clone, Copy)]
struct AecVariant {
    attenuation_db: f32,
    margin_db: f32,
    min_coefficient: Option<f32>,
    /// The correlation band (`None`: the correlation layer off).
    band: Option<GateBand>,
    level: bool,
}

impl AecVariant {
    fn apply(&self, c: &mut layers::Context) {
        c.attenuation = self.attenuation_db.is_finite();
        c.attenuation_db = self.attenuation_db;
        c.attenuation_margin_db = self.margin_db;
        c.attenuation_min_coefficient = self.min_coefficient;
        c.level_after_canceller = self.level;
        match self.band {
            Some(b) => {
                c.band = b;
                c.correlation_min_attenuation_db = None;
            }
            None => c.correlation_min_attenuation_db = Some(f32::INFINITY),
        }
    }

    fn label(&self) -> String {
        format!(
            "| {} / {} | {} | {} | {} |",
            self.attenuation_db,
            self.margin_db,
            self.min_coefficient.map_or("-".into(), |c| c.to_string()),
            self.band.map_or("off".into(), |b| format!(
                "{:.2}/{:.2}/{:.2}",
                b.keep_under, b.threshold, b.drop_at
            )),
            if self.level { "on" } else { "off" }
        )
    }
}

/// The after-canceller variants (the same list in tests/echo_sample.rs,
/// tests/echo_fleurs.rs and tests/aec_postpass.rs).
fn aec_variants() -> Vec<AecVariant> {
    let mut v = Vec::new();
    for keep_under in [0.5f32, 0.6] {
        for threshold in [0.7f32, 0.75, 0.8] {
            for drop_at in [0.8f32, 0.85, 0.9, 0.95] {
                for level in [false, true] {
                    v.push(AecVariant {
                        attenuation_db: f32::INFINITY,
                        margin_db: 10.0,
                        min_coefficient: None,
                        band: Some(GateBand {
                            keep_under,
                            threshold,
                            drop_at,
                        }),
                        level,
                    });
                }
            }
        }
    }
    for level in [false, true] {
        v.push(AecVariant {
            attenuation_db: f32::INFINITY,
            margin_db: 10.0,
            min_coefficient: None,
            band: None,
            level,
        });
    }
    v
}
