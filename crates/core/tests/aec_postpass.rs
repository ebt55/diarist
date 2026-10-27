//! The post-pass echo canceller (plan 15 chunk B2a) on a real speaker
//! recording: the whole post-pass path (`run_post_pass`: scan, canceller
//! into `mic-aec.wav`, its transcription, the gate on the raw mic) against
//! the same post-pass with `echo.mode = off`, scored with the meeting 12
//! measurement's word classes. `docs/eval/2026-10-20-echo.md`, "Post-pass
//! canceller (B2a)". `#[ignore]`d and gated:
//!
//! - `DIARIST_ECHO_SAMPLE`: the meeting folder (`playback.flac`,
//!   `loopback.words.json`), read only.
//! - `DIARIST_ECHO_DB`: a copy of the database (the clock anchors), opened
//!   read only.
//! - `DIARIST_ECHO_CLASSES`: the measurement's per-word CSV of the stored
//!   mic transcript (`i,start_s,end_s,text,norm,single_match,...,
//!   trigram_covered`, meeting time).
//! - `DIARIST_ENGINE_DIR`, `DIARIST_MODELS_DIR`: runtime and models (EN
//!   model, Nemotron 3 Diarization).
//! - `DIARIST_ECHO_OUT`: a scratch folder. Every WAV, FLAC, database and
//!   transcript the test writes there is deleted at the end unless
//!   `DIARIST_ECHO_KEEP=1`.
//! - `DIARIST_ECHO_AGC_REPLAY=1` (optional): one more post-pass with a gain
//!   track made by replaying the AGC's gain law backwards over the stored
//!   loopback (what a recording made since chunk B2a carries), and the
//!   signal-level comparison of a 10 ms track against one value per frame.
//! - `DIARIST_ECHO_DUMP` (optional): a scratch folder for the spliced set's
//!   per-word features with their text (local analysis; delete it after).
//!
//! The spliced set is also run through the layered gate (plan 15 section
//! 9) with each after-canceller variant (attenuation, correlation band,
//! level layer), the owner's word error rate per variant, and through the
//! transfer gate (plan 15 section 9.2) with every guard: the raw spliced
//! window's words (transcribed here) gated with the raw band against the
//! raw spliced mic, carried over to the cancelled words. Since the
//! transfer gate, the real post-pass (`auto`, its live run has no mic
//! words) transcribes the raw mic once more; `engine_info.echo.transfer`
//! has its wall time.
//!
//! Word classes (plan 15 section 2): a duplicate is a mic word with the
//! text of a loopback word starting within ±0.8 s, or inside a matched
//! 3-word run; the owner's words are the labelled non-duplicates whose ±5
//! neighbours are at most 30 percent duplicates; "during far-end speech" a
//! 20 ms loopback frame (100 Hz high-passed) over −50 dBFS in the word.
//! An owner's word is kept when the new transcript has its text within
//! ±0.8 s; the fair count also keeps a missing word that is one of a
//! repeated-token loop of the stored transcript (the same token four or
//! more times in a row, under 150 ms apart: on meeting 12 one token twenty
//! times in 0.88 s, 17 of them labelled the owner's), or a spelling variant
//! of a word the new transcript has within ±0.8 s (one contains the other,
//! it is the word merged with its neighbour or split in two, or one edit
//! apart at four letters or more).
//!
//! `cargo test -p diarist-core --release --test aec_postpass -- --ignored --nocapture`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use diarist_core::audio::aec::offline::{cancel_recording, OfflineOptions, OfflineReport};
use diarist_core::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use diarist_core::audio::echo::layers::{self, GateWord, LayerOptions};
use diarist_core::audio::echo::presence::{floor_sources, presence_sources};
use diarist_core::audio::echo::text::{FarIndex, TextWindow};
use diarist_core::audio::echo::transfer::{self, Guard};
use diarist_core::audio::echo::{
    decide_in_order, gate_sources, score_sources, ClockPair, EchoProfile, GateBand, GatePad,
    HighPass, PcmSource, WordScore,
};
use diarist_core::audio::flac::decode_i16;
use diarist_core::audio::gain_track::{GainTrack, GainTrackWriter};
use diarist_core::engine::{Engine, EngineEvent, NemoCliEngine};
use diarist_core::live::postpass::band_after_canceller;
use diarist_core::live::{run_post_pass, PostPassJob};
use diarist_core::pipeline::CancelToken;
use diarist_core::protocol::{DiarPreset, FileJob};
use diarist_core::settings;
use diarist_core::store::{NewMeeting, RunStore, SqliteStore, Store};
use diarist_core::transcript::SegmentOptions;
use diarist_core::types::{
    Channel, ClockAnchor, JobId, MeetingId, MeetingStatus, RunKind, RunStatus, Source,
};

const SR: f64 = 16_000.0;
const WIN: f64 = 0.8;

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
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

// ---------------------------------------------------------------- words

/// A word on meeting time (seconds) with its normalised text.
#[derive(Debug, Clone)]
struct W {
    start: f64,
    end: f64,
    norm: String,
}

fn norm(t: &str) -> String {
    t.to_lowercase()
        .chars()
        .filter(|c| (c.is_alphanumeric() || c.is_whitespace()) && *c != '_')
        .collect::<String>()
        .trim()
        .to_owned()
}

fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// The measurement's labels of the stored mic transcript.
struct Classes {
    words: Vec<W>,
    dup: Vec<bool>,
    own: Vec<bool>,
}

fn classes(path: &Path) -> Classes {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    let head = csv_fields(lines.next().unwrap());
    let col = |n: &str| head.iter().position(|h| h == n).unwrap();
    let (cn, cs, ce, c1, c3) = (
        col("norm"),
        col("start_s"),
        col("end_s"),
        col("single_match"),
        col("trigram_covered"),
    );
    let mut words = Vec::new();
    let mut dup = Vec::new();
    for l in lines.filter(|l| !l.is_empty()) {
        let f = csv_fields(l);
        dup.push(f[c1] == "1" || f[c3] == "1");
        words.push(W {
            start: f[cs].parse().unwrap(),
            end: f[ce].parse().unwrap(),
            norm: f[cn].clone(),
        });
    }
    let n = dup.len();
    let own = (0..n)
        .map(|i| {
            if dup[i] {
                return false;
            }
            let nb: Vec<bool> = (i.saturating_sub(5)..i)
                .chain(i + 1..(i + 6).min(n))
                .map(|j| dup[j])
                .collect();
            !nb.is_empty() && nb.iter().filter(|d| **d).count() as f64 / nb.len() as f64 <= 0.3
        })
        .collect();
    Classes { words, dup, own }
}

/// The loopback words, for the matcher.
struct LbIndex {
    single: HashMap<String, Vec<f64>>,
    tri: HashMap<(String, String, String), Vec<f64>>,
}

impl LbIndex {
    fn new(lb: &[W]) -> Self {
        let mut single: HashMap<String, Vec<f64>> = HashMap::new();
        for w in lb.iter().filter(|w| !w.norm.is_empty()) {
            single.entry(w.norm.clone()).or_default().push(w.start);
        }
        let mut tri: HashMap<(String, String, String), Vec<f64>> = HashMap::new();
        for j in 0..lb.len().saturating_sub(2) {
            let (a, b, c) = (&lb[j].norm, &lb[j + 1].norm, &lb[j + 2].norm);
            if !a.is_empty() && !b.is_empty() && !c.is_empty() {
                tri.entry((a.clone(), b.clone(), c.clone()))
                    .or_default()
                    .push(lb[j].start);
            }
        }
        for v in single.values_mut().chain(tri.values_mut()) {
            v.sort_by(f64::total_cmp);
        }
        Self { single, tri }
    }

    fn near(list: Option<&Vec<f64>>, t: f64) -> bool {
        let Some(l) = list else { return false };
        let k = l.partition_point(|&s| s < t - WIN);
        k < l.len() && l[k] <= t + WIN
    }

    /// Duplicate flags (single match or inside a matched 3-word run) of
    /// words sorted by start, shifted by `shift` s (the null).
    fn dups(&self, mic: &[W], shift: f64) -> Vec<bool> {
        let mut d: Vec<bool> = mic
            .iter()
            .map(|w| !w.norm.is_empty() && Self::near(self.single.get(&w.norm), w.start + shift))
            .collect();
        for i in 0..mic.len().saturating_sub(2) {
            let (a, b, c) = (&mic[i].norm, &mic[i + 1].norm, &mic[i + 2].norm);
            if a.is_empty() || b.is_empty() || c.is_empty() {
                continue;
            }
            if Self::near(
                self.tri.get(&(a.clone(), b.clone(), c.clone())),
                mic[i].start + shift,
            ) {
                d[i] = true;
                d[i + 1] = true;
                d[i + 2] = true;
            }
        }
        d
    }
}

fn has_word(words: &[W], n: &str, t: f64, win: f64) -> bool {
    let k = words.partition_point(|w| w.start < t - win);
    words[k..]
        .iter()
        .take_while(|w| w.start <= t + win)
        .any(|w| w.norm == n)
}

fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Whether labelled word `i` (missing from `new`) is a spelling variant of
/// a word `new` has near it (module docs).
fn variant(raw: &[W], i: usize, new: &[W]) -> bool {
    let w = &raw[i];
    let n = w.norm.as_str();
    if n.is_empty() {
        return false;
    }
    let k = new.partition_point(|v| v.start < w.start - WIN);
    let near: Vec<usize> = (k..new.len())
        .take_while(|&j| new[j].start <= w.start + WIN)
        .collect();
    let next_raw = raw.get(i + 1).map(|x| x.norm.as_str()).unwrap_or("");
    let prev_raw = if i > 0 { raw[i - 1].norm.as_str() } else { "" };
    near.iter().any(|&j| {
        let v = new[j].norm.as_str();
        let v2 = new.get(j + 1).map(|x| x.norm.as_str()).unwrap_or("");
        let short = v.len().min(n.len());
        (short >= 3 && (v.contains(n) || n.contains(v)))
            || v == format!("{n}{next_raw}")
            || v == format!("{prev_raw}{n}")
            || n == format!("{v}{v2}")
            || (short >= 4 && levenshtein(v.as_bytes(), n.as_bytes()) <= 1)
    })
}

/// Repeated-token loops of the stored transcript: indices in runs of four
/// or more consecutive words with one text, under 150 ms apart on average
/// (the engine repeating a token at one instant, not someone repeating a
/// word).
fn loops(raw: &[W]) -> Vec<bool> {
    let mut out = vec![false; raw.len()];
    let mut i = 0;
    while i < raw.len() {
        let mut j = i;
        while j + 1 < raw.len() && raw[j + 1].norm == raw[i].norm && !raw[i].norm.is_empty() {
            j += 1;
        }
        if j + 1 - i >= 4 && (raw[j].start - raw[i].start) / ((j - i) as f64) < 0.15 {
            out[i..=j].iter_mut().for_each(|o| *o = true);
        }
        i = j + 1;
    }
    out
}

// ---------------------------------------------------------------- audio

fn load_anchors(db: &Path, uid: &str) -> Vec<ClockAnchor> {
    let uri = format!(
        "file:{}?mode=ro",
        db.display().to_string().replace('\\', "/")
    );
    let c = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let id: i64 = c
        .query_row("SELECT id FROM meetings WHERE uid = ?1", [uid], |r| {
            r.get(0)
        })
        .unwrap();
    let mut st = c
        .prepare("SELECT channel, sample_index, qpc_100ns FROM clock_anchors WHERE meeting_id = ?1")
        .unwrap();
    st.query_map([id], |r| {
        let ch: String = r.get(0)?;
        Ok(ClockAnchor {
            meeting_id: MeetingId(id),
            channel: ch.parse().unwrap(),
            sample_index: r.get(1)?,
            qpc_100ns: r.get(2)?,
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn write_wav(path: &Path, x: &[i16]) {
    let mut w = diarist_core::audio::wav::WavSink::create(path).unwrap();
    w.write(x).unwrap();
    w.finalize().unwrap();
}

fn to_i16(x: &[f32]) -> Vec<i16> {
    x.iter()
        .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect()
}

fn hp(x: &[i16]) -> Vec<f32> {
    let mut h = HighPass::default();
    let mut out = Vec::with_capacity(x.len());
    h.extend(x, &mut out);
    out
}

fn energy(x: &[f32], a: usize, b: usize) -> f64 {
    let b = b.min(x.len());
    if a >= b {
        return 0.0;
    }
    x[a..b].iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

fn db(num: f64, den: f64) -> f64 {
    10.0 * (num.max(1e-30) / den.max(1e-30)).log10()
}

/// The loopback AGC (`audio::gain`, default settings) replayed backwards
/// over the stored loopback: per 160-sample block, the raw block peak whose
/// forward gain reproduces the stored peak (bisection), and that gain. As
/// in `tests/aec_spike.rs`. Returns the gain per block (dB).
fn replay_agc(stored: &[i16]) -> Vec<f32> {
    const TARGET: f32 = -20.0;
    const MAX_GAIN: f32 = 40.0;
    const FLOOR: f32 = -85.0;
    const DOWN: f32 = 40.0 * 0.01;
    const UP: f32 = 40.0 / 5.0 * 0.01;
    let dbfs = |p: f32| if p <= 0.0 { -120.0 } else { 20.0 * p.log10() };
    let lin = |g: f32| 10f32.powf(g / 20.0);
    let mut win: std::collections::VecDeque<f32> = std::collections::VecDeque::with_capacity(201);
    let (mut gain, mut locked) = (0.0f32, false);
    let mut gains = Vec::with_capacity(stored.len() / 160 + 1);
    for block in stored.chunks(160) {
        let skip = usize::from(win.len() >= 200);
        let rest = win.iter().skip(skip).copied().fold(0.0f32, f32::max);
        let gain_for = |p: f32| -> f32 {
            let wdb = dbfs(rest.max(p));
            let mut g = gain;
            if wdb > FLOOR {
                let desired = (TARGET - wdb).clamp(0.0, MAX_GAIN);
                g = if !locked {
                    desired
                } else if desired < g {
                    (g - DOWN).max(desired)
                } else {
                    (g + UP).min(desired)
                };
            }
            if p > 0.0 {
                let safe = (-1.0 - dbfs(p)).max(0.0);
                if g > safe {
                    g = safe;
                }
            }
            g
        };
        let peak_i = block
            .iter()
            .map(|v| f32::from(v.unsigned_abs()))
            .fold(0.0, f32::max);
        let p = if peak_i == 0.0 {
            0.0
        } else {
            let f = |p: f32| {
                let g = gain_for(p);
                p * 32_768.0 * if g > 0.0 { lin(g) } else { 1.0 }
            };
            let lo0 = peak_i / 32_768.0 / lin(MAX_GAIN) * 0.99;
            let hi0 = peak_i / 32_768.0 * 1.01;
            let bisect = |want: f32, above: bool| -> f32 {
                let (mut lo, mut hi) = (lo0.ln(), hi0.ln());
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    let v = f(mid.exp());
                    if (above && v >= want) || (!above && v > want) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                (0.5 * (lo + hi)).exp()
            };
            let p_lo = bisect(peak_i - 0.5, true);
            let p_hi = bisect(peak_i + 0.5, false);
            (p_lo * p_hi.max(p_lo)).sqrt()
        };
        let g = gain_for(p);
        if dbfs(rest.max(p)) > FLOOR {
            locked = true;
        }
        gain = g;
        win.push_back(p);
        while win.len() > 200 {
            win.pop_front();
        }
        gains.push(g);
    }
    gains
}

// ---------------------------------------------------------------- engine

/// A word: start and end (s) and text.
type Timed = (f64, f64, String);

fn transcribe(engine: &dyn Engine, wav: &Path) -> Vec<Timed> {
    let job_id = JobId::new();
    let rx = engine
        .transcribe_file(FileJob {
            job_id,
            wav_path: wav.to_path_buf(),
            diarize: false,
            language: "en".into(),
            diar_preset: DiarPreset::Streaming,
        })
        .unwrap();
    let mut utts: std::collections::BTreeMap<u32, (u32, Vec<Timed>)> = Default::default();
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(EngineEvent::Utterance {
                utt, rev, words, ..
            }) => {
                let e = utts.entry(utt).or_insert((rev, Vec::new()));
                if rev >= e.0 {
                    *e = (
                        rev,
                        words
                            .iter()
                            .map(|w| (w.t0_us as f64 / 1e6, w.t1_us as f64 / 1e6, w.text.clone()))
                            .collect(),
                    );
                }
            }
            Ok(EngineEvent::Done { .. }) => break,
            Ok(EngineEvent::Error { message, .. }) => panic!("engine: {message}"),
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("engine stopped"),
        }
    }
    let mut out: Vec<Timed> = utts.into_values().flat_map(|(_, w)| w).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// What one real post-pass over the recording gave.
struct Pass {
    wall_s: f64,
    transcribe_s: f64,
    echo: serde_json::Value,
    /// The mic words after both steps, and the gate's dropped words.
    mic: Vec<W>,
    dropped: Vec<W>,
}

fn words_json(w: &[W]) -> serde_json::Value {
    w.iter()
        .map(|w| serde_json::json!([w.start, w.end, w.norm]))
        .collect()
}

fn words_of(v: &serde_json::Value) -> Vec<W> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|w| W {
            start: w[0].as_f64().unwrap(),
            end: w[1].as_f64().unwrap(),
            norm: w[2].as_str().unwrap().to_owned(),
        })
        .collect()
}

/// [`post_pass`], or its result from `pass-<name>.json` in `out` with
/// `DIARIST_ECHO_REUSE=1` (written by every run; deleted with the rest).
#[allow(clippy::too_many_arguments)]
fn post_pass_cached(
    out: &Path,
    name: &str,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    anchors: &[ClockAnchor],
    mode: &str,
    gains: Option<&[f32]>,
) -> Pass {
    let cache = out.join(format!("pass-{name}.json"));
    if std::env::var("DIARIST_ECHO_REUSE").as_deref() == Ok("1") && cache.is_file() {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
        return Pass {
            wall_s: v["wall_s"].as_f64().unwrap(),
            transcribe_s: v["transcribe_s"].as_f64().unwrap(),
            echo: v["echo"].clone(),
            mic: words_of(&v["mic"]),
            dropped: words_of(&v["dropped"]),
        };
    }
    let p = post_pass(out, name, engine, mic, lb, anchors, mode, gains);
    let v = serde_json::json!({
        "wall_s": p.wall_s,
        "transcribe_s": p.transcribe_s,
        "echo": p.echo,
        "mic": words_json(&p.mic),
        "dropped": words_json(&p.dropped),
    });
    std::fs::write(&cache, serde_json::to_vec(&v).unwrap()).unwrap();
    p
}

#[allow(clippy::too_many_arguments)]
fn post_pass(
    out: &Path,
    name: &str,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    anchors: &[ClockAnchor],
    mode: &str,
    gains: Option<&[f32]>,
) -> Pass {
    let db = out.join(format!("{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    let store = SqliteStore::open(&db).unwrap();
    store.set_setting(settings::ECHO_MODE, mode).unwrap();
    // The words as the gate leaves them, comparable with the figures from
    // before the loop collapse (plan 16 section 12.3; the collapse runs
    // after the gate and never changes `engine_info.echo`).
    store
        .set_setting(settings::TRANSCRIPT_COLLAPSE_LOOPS, "off")
        .unwrap();
    let ms = (mic.len() / 16) as i64;
    let meeting = store
        .create_meeting(NewMeeting {
            title: format!("canceller {name}"),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 0,
            status: MeetingStatus::Processing,
        })
        .unwrap();
    store.finish_meeting(meeting.id, ms, ms).unwrap();
    let moved: Vec<ClockAnchor> = anchors
        .iter()
        .map(|a| ClockAnchor {
            meeting_id: meeting.id,
            ..*a
        })
        .collect();
    store.add_clock_anchors(&moved).unwrap();
    let live = store
        .create_run(meeting.id, RunKind::Live, serde_json::json!({}))
        .unwrap();
    store
        .materialise_live_run(live.id, &SegmentOptions::default())
        .unwrap();
    store.finish_run(live.id, RunStatus::Done).unwrap();
    let folder = out.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    write_wav(&folder.join("mic.wav"), mic);
    write_wav(&folder.join("loopback.wav"), lb);
    if let Some(g) = gains {
        let mut w = GainTrackWriter::create(&folder.join("loopback.gain")).unwrap();
        for (f, rec) in g.chunks(10).enumerate() {
            let mut r = [rec[rec.len() - 1]; 10];
            r[..rec.len()].copy_from_slice(rec);
            w.push(f as u64 * 1_600, &r).unwrap();
        }
        w.flush().unwrap();
    }
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run: live.id,
        data_root: out.to_path_buf(),
        folder: folder.clone(),
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: true,
        segment: SegmentOptions::default(),
        punct: diarist_core::punct::PunctSource::None,
        from_flac: false,
    };
    let report = run_post_pass(&store, engine, &job, &CancelToken::default(), &|_| {}).unwrap();
    assert!(!folder.join("mic-aec.wav").exists(), "mic-aec.wav was left");
    let run = store.get_run(report.current_run).unwrap();
    let echo = run.engine_info["echo"].clone();
    let segs = store.get_segments(meeting.id, 0, u32::MAX).unwrap();
    let mic_seg: std::collections::HashSet<_> = segs
        .iter()
        .filter(|s| s.channel == Channel::Mic)
        .map(|s| s.id)
        .collect();
    let mut words: Vec<W> = store
        .get_words(meeting.id, i64::MIN, i64::MAX)
        .unwrap()
        .into_iter()
        .filter(|w| mic_seg.contains(&w.segment_id))
        .map(|w| W {
            start: w.start_ms as f64 / 1_000.0,
            end: w.end_ms as f64 / 1_000.0,
            norm: norm(&w.text),
        })
        .collect();
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut dropped: Vec<W> = store
        .echo_dropped(report.current_run)
        .unwrap()
        .unwrap_or_default()
        .into_iter()
        .map(|d| W {
            start: d.start_ms as f64 / 1_000.0,
            end: d.end_ms as f64 / 1_000.0,
            norm: norm(&d.text),
        })
        .collect();
    dropped.sort_by(|a, b| a.start.total_cmp(&b.start));
    drop(store);
    if std::env::var("DIARIST_ECHO_KEEP").as_deref() != Ok("1") {
        let _ = std::fs::remove_dir_all(&folder);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
    }
    Pass {
        wall_s: report.wall.as_secs_f64(),
        transcribe_s: report.transcribe_wall.as_secs_f64(),
        echo,
        mic: words,
        dropped,
    }
}

// ---------------------------------------------------------------- scoring

struct Ctx {
    raw: Classes,
    act: Vec<bool>,
    loops: Vec<bool>,
    lb_index: LbIndex,
}

/// One transcript of "me" against the classes.
#[derive(Debug, Default)]
struct Score {
    words: usize,
    dups: usize,
    chance: usize,
    own: usize,
    own_kept: usize,
    own_fair: usize,
    looped: usize,
    variants: usize,
    dt: usize,
    dt_kept: usize,
    dt_fair: usize,
}

fn score(ctx: &Ctx, words: &[W]) -> Score {
    let d = ctx.lb_index.dups(words, 0.0);
    let null = ctx.lb_index.dups(words, 7.0);
    let mut s = Score {
        words: words.len(),
        dups: d.iter().filter(|x| **x).count(),
        chance: null.iter().filter(|x| **x).count(),
        ..Default::default()
    };
    let raw = &ctx.raw.words;
    for i in (0..raw.len()).filter(|&i| ctx.raw.own[i]) {
        let w = &raw[i];
        let kept = has_word(words, &w.norm, w.start, WIN);
        let looped = !kept && ctx.loops[i];
        let var = !kept && !looped && variant(raw, i, words);
        s.own += 1;
        s.own_kept += usize::from(kept);
        s.looped += usize::from(looped);
        s.variants += usize::from(var);
        s.own_fair += usize::from(kept || looped || var);
        if ctx.act[i] {
            s.dt += 1;
            s.dt_kept += usize::from(kept);
            s.dt_fair += usize::from(kept || looped || var);
        }
    }
    s
}

fn row(name: &str, s: &Score) {
    let pct = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
    eprintln!(
        "| {name} | {} | {} ({:.1} %) | {:.1} % | {} ({:.2} %) | {} ({:.2} %; {} loop, {} variants) | {} of {} ({} fair) |",
        s.words,
        s.dups,
        pct(s.dups, s.words),
        pct(s.chance, s.words),
        s.own_kept,
        pct(s.own_kept, s.own),
        s.own_fair,
        pct(s.own_fair, s.own),
        s.looped,
        s.variants,
        s.dt_kept,
        s.dt,
        s.dt_fair
    );
}

/// The labelled owner's words a transcript lacks, by kind (local log only).
fn missing(ctx: &Ctx, words: &[W]) {
    let raw = &ctx.raw.words;
    let mut lines = Vec::new();
    for i in (0..raw.len()).filter(|&i| ctx.raw.own[i]) {
        let w = &raw[i];
        if has_word(words, &w.norm, w.start, WIN) {
            continue;
        }
        let run = {
            let (mut a, mut b) = (i, i);
            while a > 0 && raw[a - 1].norm == w.norm {
                a -= 1;
            }
            while b + 1 < raw.len() && raw[b + 1].norm == w.norm {
                b += 1;
            }
            (b + 1 - a, raw[b].start - raw[a].start)
        };
        lines.push(format!(
            "  missing #{i} at {:.2} s: run of {} over {:.2} s, variant {}, far end {}",
            w.start,
            run.0,
            run.1,
            variant(raw, i, words),
            ctx.act[i]
        ));
    }
    eprintln!("missing owner's words: {}", lines.len());
    for l in lines {
        eprintln!("{l}");
    }
}

fn merged(a: &[W], b: &[W]) -> Vec<W> {
    let mut v: Vec<W> = a.iter().chain(b).cloned().collect();
    v.sort_by(|x, y| x.start.total_cmp(&y.start));
    v
}

// ---------------------------------------------------------------- the test

#[test]
#[ignore]
fn the_post_pass_canceller_on_a_speaker_meeting() {
    let (Some(sample), Some(dbp), Some(csv), Some(engine_dir), Some(models), Some(out)) = (
        env_path("DIARIST_ECHO_SAMPLE"),
        env_path("DIARIST_ECHO_DB"),
        env_path("DIARIST_ECHO_CLASSES"),
        env_path("DIARIST_ENGINE_DIR"),
        env_path("DIARIST_MODELS_DIR"),
        env_path("DIARIST_ECHO_OUT"),
    ) else {
        eprintln!(
            "skipped: DIARIST_ECHO_SAMPLE, DIARIST_ECHO_DB, DIARIST_ECHO_CLASSES, DIARIST_ENGINE_DIR, DIARIST_MODELS_DIR and DIARIST_ECHO_OUT are needed"
        );
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let uid = sample.file_name().unwrap().to_string_lossy().to_string();
    let anchors = load_anchors(&dbp, &uid);
    let origin = origin_from_anchors(&anchors).unwrap();
    let map = |ch| ClockMap::from_anchors(origin, &channel_anchors(&anchors, ch));
    let (mic_map, lb_map) = (map(Channel::Mic), map(Channel::Loopback));
    let clocks = ClockPair::usable(mic_map.clone(), lb_map.clone()).unwrap();
    let (pcm, ch) = decode_i16(&sample.join("playback.flac")).unwrap();
    assert_eq!(ch, 2);
    let mic: Vec<i16> = pcm.iter().step_by(2).copied().collect();
    let lb: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
    drop(pcm);
    eprintln!(
        "meeting {uid}: {:.1} min, anchors mic {} loopback {}",
        mic.len() as f64 / SR / 60.0,
        mic_map.len(),
        lb_map.len()
    );

    // Classes and the matcher.
    let raw = classes(&csv);
    let lb_words: Vec<W> = {
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sample.join("loopback.words.json")).unwrap())
                .unwrap();
        let mut w: Vec<W> = v["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                let ms = |k: &str| (w[k].as_f64().unwrap_or(0.0) * 1_000.0).round() as i64;
                W {
                    start: lb_map.meeting_ms(ms("start")) as f64 / 1_000.0,
                    end: lb_map.meeting_ms(ms("end")) as f64 / 1_000.0,
                    norm: norm(w["word"].as_str().unwrap_or("")),
                }
            })
            .collect();
        w.sort_by(|a, b| a.start.total_cmp(&b.start));
        w
    };
    let lb_index = LbIndex::new(&lb_words);
    let lb_frames_db: Vec<f32> = hp(&lb)
        .chunks(320)
        .map(|c| (10.0 * (energy(c, 0, c.len()) / c.len() as f64).max(1e-14).log10()) as f32)
        .collect();
    let lb_active = |t0: f64, t1: f64| -> bool {
        let s = |t: f64| lb_map.sample_at(origin as f64 + t * 1e7);
        let f0 = (s(t0) / 320.0).floor().max(0.0) as usize;
        let f1 = ((s(t1) / 320.0).ceil().max(0.0) as usize).max(f0 + 1);
        lb_frames_db[f0.min(lb_frames_db.len())..f1.min(lb_frames_db.len())]
            .iter()
            .any(|&d| d > -50.0)
    };
    let act: Vec<bool> = raw
        .words
        .iter()
        .map(|w| lb_active(w.start, w.end))
        .collect();
    let loops = loops(&raw.words);
    let ctx = Ctx {
        act,
        loops,
        lb_index,
        raw,
    };
    let own_n = ctx.raw.own.iter().filter(|o| **o).count();
    eprintln!(
        "classes: {} mic words, {} duplicates, {} owner's ({} during far-end speech); {} in repeated-token loops",
        ctx.raw.words.len(),
        ctx.raw.dup.iter().filter(|d| **d).count(),
        own_n,
        (0..ctx.raw.words.len())
            .filter(|&i| ctx.raw.own[i] && ctx.act[i])
            .count(),
        ctx.loops.iter().filter(|l| **l).count()
    );

    // The engine, as the app's post-pass runs it.
    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf").expect("EN model");
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer");
    let mut engine = NemoCliEngine::new(&engine_dir, &models, &asr, &diar);
    engine.pass_language = false;
    engine.right_context = Some(13);
    engine.diar_preset = Some("v3-offline".into());

    // 1. The post-pass without echo handling, and the gate alone on its
    // words (what chunk A shipped), with the post-pass's own scan.
    let off = post_pass_cached(&out, "off", &engine, &mic, &lb, &anchors, "off", None);
    let profile = EchoProfile::scan_sources(&mic[..], &lb[..], Some(clocks.clone()));
    let spans: Vec<(i64, i64)> = off
        .mic
        .iter()
        .map(|w| {
            let s = (w.start * 16_000.0).round() as i64;
            (s, ((w.end * 16_000.0).round() as i64).max(s))
        })
        .collect();
    let t = Instant::now();
    let g = gate_sources(&spans, &mic[..], &lb[..], &profile, GateBand::default());
    let gate_s = t.elapsed().as_secs_f64();
    let gate_alone: Vec<W> = off
        .mic
        .iter()
        .zip(&g.drop)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| w.clone())
        .collect();

    // 2. The post-pass as shipped: canceller, then the gate.
    let auto = post_pass_cached(&out, "auto", &engine, &mic, &lb, &anchors, "auto", None);
    eprintln!("auto: engine_info.echo {}", auto.echo);
    eprintln!(
        "auto: transfer from the {} mic words in {} s",
        auto.echo["transfer"]["source"], auto.echo["transfer"]["raw_wall_s"]
    );
    assert_eq!(auto.echo["aec"], true, "the canceller did not engage");
    let before_gate = merged(&auto.mic, &auto.dropped);

    eprintln!();
    eprintln!("| Transcript of \"me\" | Words | Duplicates (share) | Chance (7 s null) | Owner's words kept (of {own_n}) | Fair count | During far-end speech kept |");
    eprintln!("|---|---|---|---|---|---|---|");
    row("stored mic transcript", &score(&ctx, &ctx.raw.words));
    row("post-pass, echo off", &score(&ctx, &off.mic));
    row("gate alone (chunk A)", &score(&ctx, &gate_alone));
    row("canceller, before the gate", &score(&ctx, &before_gate));
    row(
        "canceller + transfer gate (shipped)",
        &score(&ctx, &auto.mic),
    );

    // 3. Wall time.
    eprintln!();
    eprintln!(
        "wall: off {:.1} s (transcription {:.1} s); auto {:.1} s (transcription {:.1} s); delta {:+.1} s; canceller {} s, mic waited {} s; gate alone {gate_s:.1} s",
        off.wall_s,
        off.transcribe_s,
        auto.wall_s,
        auto.transcribe_s,
        auto.wall_s - off.wall_s,
        auto.echo["aec_wall_s"],
        auto.echo["aec_wait_s"]
    );
    missing(&ctx, &auto.mic);

    // What the gate dropped after the canceller, by how much of the raw
    // mic's energy the canceller left over each word (an echo word loses
    // 10 to 20 dB, the owner over the far end about 1 dB).
    let hint = |t: i64| profile.lag_at(t);
    let (cancelled, _) = cancel(&mic, &lb, &options(&clocks, &hint));
    let (mic_h, out_h) = (hp(&mic), hp(&to_i16(&cancelled)));
    let to_sample = |t: f64| mic_map.sample_at(origin as f64 + t * 1e7).max(0.0) as usize;
    let kept_db = |w: &W| {
        let (a, b) = (to_sample(w.start), to_sample(w.end.max(w.start + 0.02)));
        db(energy(&out_h, a, b), energy(&mic_h, a, b))
    };
    let r: Vec<f64> = auto.dropped.iter().map(kept_db).collect();
    let dd = ctx.lb_index.dups(&auto.dropped, 0.0);
    let q = |v: &mut Vec<f64>, f: f64| {
        v.sort_by(f64::total_cmp);
        v.get(((v.len().max(1) - 1) as f64 * f) as usize)
            .copied()
            .unwrap_or(f64::NAN)
    };
    let mut rd: Vec<f64> = r.iter().zip(&dd).filter(|p| *p.1).map(|p| *p.0).collect();
    let mut rn: Vec<f64> = r.iter().zip(&dd).filter(|p| !*p.1).map(|p| *p.0).collect();
    eprintln!(
        "dropped after the canceller: {} duplicates keep {:.1} / {:.1} / {:.1} dB (p10 / p50 / p90), {} others {:.1} / {:.1} / {:.1} dB",
        rd.len(),
        q(&mut rd, 0.1),
        q(&mut rd, 0.5),
        q(&mut rd, 0.9),
        rn.len(),
        q(&mut rn, 0.1),
        q(&mut rn, 0.5),
        q(&mut rn, 0.9)
    );
    for thr in [-3.0, -4.5, -6.0, -9.0] {
        let back: Vec<W> = auto
            .dropped
            .iter()
            .zip(&r)
            .filter(|p| *p.1 >= thr)
            .map(|p| p.0.clone())
            .collect();
        row(
            &format!("canceller + gate, a drop kept at {thr} dB or more"),
            &score(&ctx, &merged(&auto.mic, &back)),
        );
    }
    drop((mic_h, out_h, cancelled));

    // 4. The spliced double talk (research 14 section 2.5), rebuilt.
    let dt = spliced(
        &ctx, &engine, &mic, &lb, &lb_words, &mic_map, &clocks, &profile, &out,
    );

    // The gate's band after the canceller: meeting 12's cancelled words and
    // the spliced set's, scored once, decided per band.
    let pre = merged(&auto.mic, &auto.dropped);
    let pre_spans: Vec<(i64, i64)> = pre
        .iter()
        .map(|w| {
            let s = (w.start * 16_000.0).round() as i64;
            (s, ((w.end * 16_000.0).round() as i64).max(s))
        })
        .collect();
    let pre_g = gate_sources(
        &pre_spans,
        &mic[..],
        &lb[..],
        &profile,
        band_after_canceller(GateBand::DEFAULT.threshold),
    );
    let same = pre_g.dropped == auto.dropped.len();
    eprintln!();
    eprintln!(
        "gate after the canceller, rescored: {} dropped ({} in the post-pass){}",
        pre_g.dropped,
        auto.dropped.len(),
        if same { "" } else { ": differs" }
    );
    eprintln!("| Band after the canceller (keep / neighbours / drop) | \"me\" words | Duplicates (share) | Owner's words kept, fair | Spliced WER |");
    eprintln!("|---|---|---|---|---|");
    let mut bands = vec![
        band_after_canceller(GateBand::DEFAULT.threshold),
        GateBand::default(),
    ];
    for keep_under in [0.2f32, 0.3, 0.4, 0.45, 0.5] {
        for threshold in [0.5f32, 0.55, 0.6, 0.65, 0.7] {
            for drop_at in [0.6f32, 0.65, 0.7, 0.75, 0.8] {
                if keep_under <= threshold && threshold <= drop_at {
                    bands.push(GateBand {
                        keep_under,
                        threshold,
                        drop_at,
                    });
                }
            }
        }
    }
    for b in bands {
        let d = decide_in_order(&pre_spans, &pre_g.scores, b);
        let kept: Vec<W> = pre
            .iter()
            .zip(&d)
            .filter(|(_, d)| !**d)
            .map(|(w, _)| w.clone())
            .collect();
        let sc = score(&ctx, &kept);
        eprintln!(
            "| {:.2} / {:.2} / {:.2} | {} | {} ({:.2} %) | {:.2} % | {:.1} % |",
            b.keep_under,
            b.threshold,
            b.drop_at,
            sc.words,
            sc.dups,
            100.0 * sc.dups as f64 / sc.words.max(1) as f64,
            100.0 * sc.own_fair as f64 / sc.own.max(1) as f64,
            dt.wer(b)
        );
    }

    // 5. Optional: a recorded gain track, stood in for by the replay.
    if std::env::var("DIARIST_ECHO_AGC_REPLAY").as_deref() == Ok("1") {
        let t = Instant::now();
        let gains = replay_agc(&lb);
        eprintln!(
            "AGC replayed in {:.1} s: gain above 0.1 dB in {:.1} percent of 10 ms blocks",
            t.elapsed().as_secs_f64(),
            100.0 * gains.iter().filter(|&&g| g > 0.1).count() as f64 / gains.len() as f64
        );
        signal_rows(&ctx, &mic, &lb, &mic_map, &clocks, &profile, &gains);
        let replayed = post_pass_cached(
            &out,
            "replay",
            &engine,
            &mic,
            &lb,
            &anchors,
            "auto",
            Some(&gains),
        );
        eprintln!("replay: engine_info.echo {}", replayed.echo);
        row("canceller + gate, gain track", &score(&ctx, &replayed.mic));
        row(
            "canceller, gain track, before the gate",
            &score(&ctx, &merged(&replayed.mic, &replayed.dropped)),
        );
    }
    // `DIARIST_ECHO_TIMING=N`: N more pairs of post-passes, off then auto,
    // for the wall time (the engine's own time varies by 10 s and more).
    let pairs: usize = std::env::var("DIARIST_ECHO_TIMING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for k in 0..pairs {
        let a = post_pass(&out, "t-off", &engine, &mic, &lb, &anchors, "off", None);
        let b = post_pass(&out, "t-auto", &engine, &mic, &lb, &anchors, "auto", None);
        eprintln!(
            "timing pair {k}: off {:.1} s (transcription {:.1} s), auto {:.1} s (transcription {:.1} s), delta {:+.1} s; canceller {} s, mic waited {} s",
            a.wall_s,
            a.transcribe_s,
            b.wall_s,
            b.transcribe_s,
            b.wall_s - a.wall_s,
            b.echo["aec_wall_s"],
            b.echo["aec_wait_s"]
        );
    }
    if std::env::var("DIARIST_ECHO_KEEP").as_deref() != Ok("1") {
        for name in ["off", "auto", "replay"] {
            let _ = std::fs::remove_file(out.join(format!("pass-{name}.json")));
        }
    }
}

fn options<'a>(
    clocks: &'a ClockPair,
    hint: &'a (dyn Fn(i64) -> Option<f32> + Sync),
) -> OfflineOptions<'a> {
    let mut o = OfflineOptions::post_pass(Some((&clocks.mic, &clocks.loopback)));
    o.lag_hint = Some(hint);
    o
}

fn cancel(mic: &[i16], lb: &[i16], opts: &OfflineOptions) -> (Vec<f32>, OfflineReport) {
    let mut out = Vec::with_capacity(mic.len());
    let (m, l) = (mic, lb);
    let r = cancel_recording(&m, &l, opts, &mut |b| {
        out.extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    (out, r)
}

/// Signal-level rows: no gain track (the stored loopback), a 10 ms track
/// and one value per frame (the frame's last), on the measurement's spans.
fn signal_rows(
    ctx: &Ctx,
    mic: &[i16],
    lb: &[i16],
    mic_map: &ClockMap,
    clocks: &ClockPair,
    profile: &EchoProfile,
    gains: &[f32],
) {
    let span = |w: &W| {
        let s = |t: f64| {
            mic_map
                .sample_at(mic_map.origin_qpc as f64 + t * 1e7)
                .max(0.0) as usize
        };
        (s(w.start), s(w.end.max(w.start + 0.02)).min(mic.len()))
    };
    let mic_h = hp(mic);
    let per_frame: Vec<f32> = gains
        .chunks(10)
        .flat_map(|c| std::iter::repeat_n(c[c.len() - 1], c.len()))
        .collect();
    let tracks = [
        ("no gain track (post-AGC reference)", None),
        (
            "gain track, 10 ms values",
            Some(GainTrack::from_values(gains.to_vec())),
        ),
        (
            "gain track, one value per frame",
            Some(GainTrack::from_values(per_frame)),
        ),
    ];
    eprintln!();
    eprintln!("| Reference | ERLE, far end only (labelled) | ERLE (label-free, as in engine_info) | Owner alone: level | Double talk: level | Resets | Canceller s |");
    eprintln!("|---|---|---|---|---|---|---|");
    let hint = |t: i64| profile.lag_at(t);
    for (name, track) in &tracks {
        let mut o = options(clocks, &hint);
        o.loopback_gain = track.as_ref();
        let (out, r) = cancel(mic, lb, &o);
        let out_h = hp(&to_i16(&out));
        let (mut fm, mut fo, mut om, mut oo, mut dm, mut d_o) =
            (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        for (i, w) in ctx.raw.words.iter().enumerate() {
            let (a, b) = span(w);
            let (em, eo) = (energy(&mic_h, a, b), energy(&out_h, a, b));
            if ctx.raw.dup[i] && ctx.act[i] {
                fm += em;
                fo += eo;
            } else if ctx.raw.own[i] && !ctx.act[i] {
                om += em;
                oo += eo;
            } else if ctx.raw.own[i] && ctx.act[i] {
                dm += em;
                d_o += eo;
            }
        }
        eprintln!(
            "| {name} | {:.1} dB | {:?} dB | {:+.2} dB | {:+.2} dB | {} | {:.1} |",
            db(fm, fo),
            r.erle_db,
            db(oo, om),
            db(d_o, dm),
            r.stats.resets,
            r.cancel_s
        );
    }
}

/// The spliced double talk of research 14 section 2.5 through the
/// production canceller: the owner's own utterances recorded while the
/// loopback was silent, placed 1.5 s apart on the 10-minute far-end window
/// with the most duplicates (no owner's word in it or the minute before),
/// added to the real mic; the canceller over the whole spliced meeting; the
/// window's raw, cancelled, and cancelled-then-gated transcripts against
/// the owner's clips transcribed alone.
#[allow(clippy::too_many_arguments)]
fn spliced(
    ctx: &Ctx,
    engine: &NemoCliEngine,
    mic: &[i16],
    lb: &[i16],
    far_words: &[W],
    mic_map: &ClockMap,
    clocks: &ClockPair,
    profile: &EchoProfile,
    out: &Path,
) -> SplicedEval {
    let words = &ctx.raw.words;
    let nw = words.len();
    let sample_of = |t: f64| mic_map.sample_at(mic_map.origin_qpc as f64 + t * 1e7);
    let span = |t0: f64, t1: f64| {
        let a = sample_of(t0).max(0.0) as usize;
        let b = (sample_of(t1).max(0.0) as usize).min(mic.len());
        (a.min(b), b)
    };
    let lb_frames_db: Vec<f32> = hp(lb)
        .chunks(320)
        .map(|c| (10.0 * (energy(c, 0, c.len()) / c.len() as f64).max(1e-14).log10()) as f32)
        .collect();
    let lb_map = &clocks.loopback;
    let lb_active = |t0: f64, t1: f64| -> bool {
        let s = |t: f64| lb_map.sample_at(lb_map.origin_qpc as f64 + t * 1e7);
        let f0 = (s(t0) / 320.0).floor().max(0.0) as usize;
        let f1 = ((s(t1) / 320.0).ceil().max(0.0) as usize).max(f0 + 1);
        lb_frames_db[f0.min(lb_frames_db.len())..f1.min(lb_frames_db.len())]
            .iter()
            .any(|&d| d > -50.0)
    };
    // The owner-alone utterances.
    let mut utts: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < nw {
        if !(ctx.raw.own[i] && !ctx.act[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < nw
            && ctx.raw.own[j + 1]
            && !ctx.act[j + 1]
            && words[j + 1].start - words[j].end < 0.6
        {
            j += 1;
        }
        let (t0, t1) = (words[i].start - 0.25, words[j].end + 0.35);
        if t1 - t0 >= 0.8 && !lb_active(t0, t1) {
            utts.push((t0, t1));
        }
        i = j + 1;
    }
    // The window.
    let len_s = 600.0;
    let end_s = words.last().map_or(0.0, |w| w.end);
    let mut best = (0usize, 0.0f64);
    let mut w0 = 60.0;
    while w0 + len_s < end_s {
        let in_win = |i: usize| words[i].start >= w0 - 60.0 && words[i].start < w0 + len_s;
        let owners = (0..nw).filter(|&i| in_win(i) && ctx.raw.own[i]).count();
        let dups = (0..nw)
            .filter(|&i| in_win(i) && ctx.raw.dup[i] && ctx.act[i])
            .count();
        if owners == 0 && dups > best.0 {
            best = (dups, w0);
        }
        w0 += 60.0;
    }
    let w0 = best.1;
    let (m0, m1) = span(w0, w0 + len_s);
    let wl = m1 - m0;
    let mut near = vec![0i16; wl];
    let mut clips = Vec::new();
    let mut pos = 5 * 16_000usize;
    for &(t0, t1) in &utts {
        let (a, b) = span(t0, t1);
        let n = b - a;
        if pos + n + 16_000 > wl {
            break;
        }
        near[pos..pos + n].copy_from_slice(&mic[a..b]);
        clips.push((pos as f64 / SR, (pos + n) as f64 / SR));
        pos += n + 24_000;
    }
    eprintln!();
    eprintln!(
        "spliced double talk: {} of {} owner-alone utterances on {:.0} to {:.0} min ({} far-end duplicates)",
        clips.len(),
        utts.len(),
        w0 / 60.0,
        (w0 + len_s) / 60.0,
        best.0
    );
    let mut spliced_mic = mic.to_vec();
    for (k, &v) in near.iter().enumerate() {
        let s = &mut spliced_mic[m0 + k];
        *s = (i32::from(*s) + i32::from(v)).clamp(-32_768, 32_767) as i16;
    }
    let hint = |t: i64| profile.lag_at(t);
    let (cancelled, r) = cancel(&spliced_mic, lb, &options(clocks, &hint));
    eprintln!("spliced: canceller {r:?}");
    let dir = out.join("dt");
    std::fs::create_dir_all(&dir).unwrap();
    let wavs = [
        ("dt-near", near.clone()),
        ("dt-mix", spliced_mic[m0..m1].to_vec()),
        ("dt-mix-aec", to_i16(&cancelled[m0..m1])),
    ];
    let mut asr: HashMap<&str, Vec<W>> = HashMap::new();
    for (name, x) in &wavs {
        let p = dir.join(format!("{name}.wav"));
        write_wav(&p, x);
        let w = transcribe(engine, &p)
            .into_iter()
            .map(|(s, e, t)| W {
                start: s,
                end: e,
                norm: norm(&t),
            })
            .collect();
        asr.insert(name, w);
    }
    // The gate over the cancelled words, scored on the raw spliced mic.
    let meeting_s = |t: f64| (mic_map.qpc_at(m0 as f64 + t * SR) - mic_map.origin_qpc as f64) / 1e7;
    let aec_words = &asr["dt-mix-aec"];
    let spans: Vec<(i64, i64)> = aec_words
        .iter()
        .map(|w| {
            let s = (meeting_s(w.start) * 16_000.0).round() as i64;
            (s, ((meeting_s(w.end) * 16_000.0).round() as i64).max(s))
        })
        .collect();
    let g = gate_sources(
        &spans,
        &spliced_mic[..],
        lb,
        profile,
        band_after_canceller(GateBand::DEFAULT.threshold),
    );
    let gated: Vec<W> = aec_words
        .iter()
        .zip(&g.drop)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| w.clone())
        .collect();
    // The energy each word kept through the canceller.
    let win_raw = hp(&spliced_mic[m0..m1]);
    let win_out = hp(&to_i16(&cancelled[m0..m1]));
    let kept_db = |w: &W| {
        let a = (w.start * SR) as usize;
        let b = ((w.end.max(w.start + 0.02)) * SR) as usize;
        db(energy(&win_out, a, b), energy(&win_raw, a, b))
    };
    let vetoed = |thr: f64| -> Vec<W> {
        aec_words
            .iter()
            .zip(&g.drop)
            .filter(|(w, d)| !**d || kept_db(w) >= thr)
            .map(|(w, _)| w.clone())
            .collect()
    };
    let in_any = |w: &W| {
        clips.iter().any(|c| {
            let mid = (w.start + w.end) / 2.0;
            mid >= c.0 - 0.1 && mid <= c.1 + 0.1
        })
    };
    let mut dropped_in: Vec<f64> = aec_words
        .iter()
        .zip(&g.drop)
        .filter(|(w, d)| **d && in_any(w))
        .map(|(w, _)| kept_db(w))
        .collect();
    dropped_in.sort_by(f64::total_cmp);
    eprintln!(
        "spliced: the gate dropped {} words inside the owner's clips (energy kept {:?} dB) and {} outside",
        dropped_in.len(),
        dropped_in.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>(),
        aec_words.iter().zip(&g.drop).filter(|(w, d)| **d && !in_any(w)).count()
    );
    let veto: Vec<(String, Vec<W>)> = [-3.0, -4.5, -6.0, -9.0]
        .into_iter()
        .map(|t| {
            (
                format!("canceller + gate, a drop kept at {t} dB or more"),
                vetoed(t),
            )
        })
        .collect();
    let reference = &asr["dt-near"];
    let in_clip = |w: &W, &(a, b): &(f64, f64)| {
        let mid = (w.start + w.end) / 2.0;
        mid >= a - 0.1 && mid <= b + 0.1
    };
    let ref_n: usize = clips
        .iter()
        .map(|c| reference.iter().filter(|w| in_clip(w, c)).count())
        .sum();
    eprintln!(
        "| Spliced double talk (reference: the clips transcribed alone, {ref_n} words) | Owner's word error rate | Words outside the clips |"
    );
    eprintln!("|---|---|---|");
    let mut rows: Vec<(&str, &Vec<W>)> = vec![
        ("raw mic", &asr["dt-mix"]),
        ("canceller", aec_words),
        ("canceller + gate", &gated),
    ];
    rows.extend(veto.iter().map(|(n, w)| (n.as_str(), w)));
    for (name, hyp) in rows {
        let mut edits = 0usize;
        for c in &clips {
            let r: Vec<&str> = reference
                .iter()
                .filter(|w| in_clip(w, c))
                .map(|w| w.norm.as_str())
                .collect();
            let h: Vec<&str> = hyp
                .iter()
                .filter(|w| in_clip(w, c))
                .map(|w| w.norm.as_str())
                .collect();
            edits += levenshtein(&r, &h);
        }
        let outside = hyp
            .iter()
            .filter(|w| !clips.iter().any(|c| in_clip(w, c)))
            .count();
        eprintln!(
            "| {name} | {:.1} % | {outside} |",
            100.0 * edits as f64 / ref_n.max(1) as f64
        );
    }
    // The layered gate over the same cancelled words (plan 15 section 9):
    // levels on the canceller's output, the attenuation against the raw
    // spliced mic, text against the loopback's words, each variant of the
    // after-canceller parameters decided over the same features.
    {
        let heard = to_i16(&cancelled);
        let raw: Option<Box<dyn PcmSource>> = Some(Box::new(&spliced_mic[..]));
        let pres = presence_sources(&spans, &heard[..], raw, lb, Some(clocks.clone()));
        let floor = floor_sources(&heard[..], lb, Some(clocks.clone())).floor_db();
        let ms: Vec<(i64, i64)> = spans.iter().map(|s| (s.0 / 16, s.1 / 16)).collect();
        let groups = layers::gap_groups(ms.iter().copied());
        let gw: Vec<GateWord> = (0..aec_words.len())
            .map(|i| GateWord {
                start_ms: ms[i].0,
                end_ms: ms[i].1,
                norm: aec_words[i].norm.clone(),
                group: groups[i],
                score: g.scores[i],
                presence: pres[i],
            })
            .collect();
        let fi = FarIndex::new(
            far_words
                .iter()
                .map(|w| ((w.start * 1_000.0).round() as i64, w.norm.clone())),
        );
        let opts = LayerOptions {
            correlation: true,
            canceller: true,
            fixed: band_after_canceller(GateBand::DEFAULT.threshold),
            threshold: GateBand::DEFAULT.threshold,
            window: TextWindow::DEFAULT,
            calibrate: true,
        };
        let reference = &asr["dt-near"];
        let in_clip = |w: &W, &(a, b): &(f64, f64)| {
            let mid = (w.start + w.end) / 2.0;
            mid >= a - 0.1 && mid <= b + 0.1
        };
        let wer = |hyp: &[&W]| -> (f64, usize) {
            let (mut e, mut n) = (0usize, 0usize);
            for c in &clips {
                let r: Vec<&str> = reference
                    .iter()
                    .filter(|w| in_clip(w, c))
                    .map(|w| w.norm.as_str())
                    .collect();
                let h: Vec<&str> = hyp
                    .iter()
                    .filter(|w| in_clip(w, c))
                    .map(|w| w.norm.as_str())
                    .collect();
                e += levenshtein(&r, &h);
                n += r.len();
            }
            let outside = hyp
                .iter()
                .filter(|w| !clips.iter().any(|c| in_clip(w, c)))
                .count();
            (100.0 * e as f64 / n.max(1) as f64, outside)
        };
        if let Some(dir) = env_path("DIARIST_ECHO_DUMP") {
            // Every word's features (local analysis; delete after).
            std::fs::create_dir_all(&dir).unwrap();
            let r = layers::gate(&gw, &fi, floor, &opts);
            let raw_pres =
                presence_sources(&spans, &spliced_mic[..], None, lb, Some(clocks.clone()));
            let mut t = String::from(
                "start\tend\ttext\tin_clip\tlevel\tfar\tatt\tcoef\treason\traw_level\tspan_att\n",
            );
            for (i, w) in aec_words.iter().enumerate() {
                let p = gw[i].presence;
                t.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\n",
                    gw[i].start_ms,
                    gw[i].end_ms,
                    w.norm,
                    u8::from(clips.iter().any(|c| in_clip(w, c))),
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
                    -kept_db(w)
                ));
            }
            std::fs::write(dir.join("spliced.tsv"), t).unwrap();
            let mut raw_t = String::from("start\tend\ttext\tin_clip\n");
            for w in &asr["dt-mix"] {
                raw_t.push_str(&format!(
                    "{:.3}\t{:.3}\t{}\t{}\n",
                    w.start,
                    w.end,
                    w.norm,
                    u8::from(clips.iter().any(|c| in_clip(w, c)))
                ));
            }
            std::fs::write(dir.join("spliced.raw.tsv"), raw_t).unwrap();
        }
        eprintln!("| Spliced, layered: attenuation at / margin (dB) | and coefficient at | Correlation band | Level layer | Dropped | Owner's word error rate | Words outside the clips |");
        eprintln!("|---|---|---|---|---|---|---|");
        for v in aec_variants() {
            let r = layers::gate_with(&gw, &fi, floor, &opts, &|c| v.apply(c));
            let kept: Vec<&W> = aec_words
                .iter()
                .zip(&r.reasons)
                .filter(|p| p.1.is_none())
                .map(|p| p.0)
                .collect();
            let (e, outside) = wer(&kept);
            eprintln!(
                "{} {} ({:?}) | {e:.1} % | {outside} |",
                v.label(),
                r.dropped(),
                r.counts
            );
        }
        // The transfer gate (plan 15 section 9.2): the raw window's words
        // on meeting time, gated with the raw band against the raw spliced
        // mic, carried over to the cancelled words with each guard; the
        // post-canceller layers as the post-pass runs them (fixed band).
        let raw_words = &asr["dt-mix"];
        let raw_spans: Vec<(i64, i64)> = raw_words
            .iter()
            .map(|w| {
                let s = (meeting_s(w.start) * 16_000.0).round() as i64;
                (s, ((meeting_s(w.end) * 16_000.0).round() as i64).max(s))
            })
            .collect();
        let raw_scores = score_sources(&raw_spans, &spliced_mic[..], lb, profile, GatePad::DEFAULT);
        let raw_pres =
            presence_sources(&raw_spans, &spliced_mic[..], None, lb, Some(clocks.clone()));
        let raw_floor = floor_sources(&spliced_mic[..], lb, Some(clocks.clone())).floor_db();
        let raw_ms: Vec<(i64, i64)> = raw_spans.iter().map(|s| (s.0 / 16, s.1 / 16)).collect();
        let raw_groups = layers::gap_groups(raw_ms.iter().copied());
        let raw_gw: Vec<GateWord> = (0..raw_words.len())
            .map(|i| GateWord {
                start_ms: raw_ms[i].0,
                end_ms: raw_ms[i].1,
                norm: raw_words[i].norm.clone(),
                group: raw_groups[i],
                score: raw_scores[i],
                presence: raw_pres[i],
            })
            .collect();
        let after = LayerOptions {
            calibrate: false,
            ..opts
        };
        let raw_opts = LayerOptions {
            canceller: false,
            fixed: GateBand::default(),
            calibrate: true,
            ..opts
        };
        let in_clips = |i: usize| clips.iter().any(|c| in_clip(&aec_words[i], c));
        eprintln!("| Spliced, transfer gate: guard | Raw words echo | Candidates | Removed (text, level, plain) | Dropped | Of them in the owner's clips | Owner's word error rate | Words outside the clips |");
        eprintln!("|---|---|---|---|---|---|---|---|");
        for spread_ms in [300i64, 500] {
            let o = LayerOptions {
                window: TextWindow {
                    spread_ms,
                    ..TextWindow::DEFAULT
                },
                ..after
            };
            let r = layers::gate(&gw, &fi, floor, &o);
            let kept: Vec<&W> = aec_words
                .iter()
                .zip(&r.reasons)
                .filter(|p| p.1.is_none())
                .map(|p| p.0)
                .collect();
            let (e, outside) = wer(&kept);
            eprintln!(
                "| layers alone, spread {spread_ms} ms | - | - | - | {} {:?} | {} | {e:.1} % | {outside} |",
                r.dropped(),
                r.counts,
                (0..aec_words.len()).filter(|&i| r.reasons[i].is_some() && in_clips(i)).count()
            );
        }
        for guard in Guard::ALL {
            let t = transfer::gate(
                &raw_gw, raw_floor, &raw_opts, &gw, floor, &after, &fi, guard,
            );
            let kept: Vec<&W> = aec_words
                .iter()
                .zip(&t.reasons)
                .filter(|p| p.1.is_none())
                .map(|p| p.0)
                .collect();
            let (e, outside) = wer(&kept);
            let c = t.transfer;
            eprintln!(
                "| {} | {} of {} {:?} | {} | {} ({}, {}, {}) | {} {:?} | {} | {e:.1} % | {outside} |",
                guard.as_str(),
                t.raw.dropped(),
                raw_words.len(),
                t.raw.counts,
                c.candidates,
                c.removed(),
                c.text,
                c.level,
                c.plain,
                t.dropped(),
                t.counts,
                (0..aec_words.len()).filter(|&i| t.reasons[i].is_some() && in_clips(i)).count()
            );
        }
    }
    if std::env::var("DIARIST_ECHO_KEEP").as_deref() != Ok("1") {
        let _ = std::fs::remove_dir_all(&dir);
    }
    SplicedEval {
        words: aec_words.clone(),
        spans,
        scores: g.scores,
        reference: reference.clone(),
        clips,
    }
}

/// The spliced set's cancelled words and their gate scores, to score other
/// bands.
struct SplicedEval {
    words: Vec<W>,
    spans: Vec<(i64, i64)>,
    scores: Vec<WordScore>,
    reference: Vec<W>,
    clips: Vec<(f64, f64)>,
}

impl SplicedEval {
    /// The owner's WER with `band` over the cancelled words.
    fn wer(&self, band: GateBand) -> f64 {
        let drop = decide_in_order(&self.spans, &self.scores, band);
        let in_clip = |w: &W, &(a, b): &(f64, f64)| {
            let mid = (w.start + w.end) / 2.0;
            mid >= a - 0.1 && mid <= b + 0.1
        };
        let (mut edits, mut n) = (0usize, 0usize);
        for c in &self.clips {
            let r: Vec<&str> = self
                .reference
                .iter()
                .filter(|w| in_clip(w, c))
                .map(|w| w.norm.as_str())
                .collect();
            let h: Vec<&str> = self
                .words
                .iter()
                .zip(&drop)
                .filter(|(w, d)| !**d && in_clip(w, c))
                .map(|(w, _)| w.norm.as_str())
                .collect();
            edits += levenshtein(&r, &h);
            n += r.len();
        }
        100.0 * edits as f64 / n.max(1) as f64
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
