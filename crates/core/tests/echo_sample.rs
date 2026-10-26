//! The echo gate on stored live meetings (plan 15 sections 4A and 9): the
//! detector, and the layered gate on both of its paths, one invocation over
//! a list of meeting folders. Gated on `DIARIST_ECHO_MEETINGS`; run with
//! `--release`. Nothing is written outside `DIARIST_ECHO_OUT`.
//!
//! Env:
//! - `DIARIST_ECHO_MEETINGS`: meeting folders, `;`-separated (read only):
//!   `playback.flac` (left mic, right loopback, raw per-channel indices),
//!   the runtime's `loopback.words.json` and `mic.words.json` or
//!   `mic-aec.words.json` (channel time), `loopback.gain` when recorded.
//!   `DIARIST_ECHO_SAMPLE` (one folder) still works.
//! - `DIARIST_ECHO_DB`: a copy of the database the meetings came from
//!   (clock anchors, the live runs' `live_utterances`), opened read only.
//! - `DIARIST_ECHO_CLASSES` (optional): the meeting 12 measurement's
//!   per-word CSV (`i,start_s,end_s,text,norm,single_match,...,
//!   trigram_covered`, meeting time) of the stored mic transcript; used for
//!   the meeting whose `mic.words.json` has as many words.
//! - `DIARIST_ENGINE_DIR`, `DIARIST_MODELS_DIR` (optional): where the
//!   canceller engages and the folder has no `mic-aec.words.json`, its
//!   output is transcribed (EN model, lookahead 13) once and cached in
//!   `DIARIST_ECHO_OUT` (`<uid>.aec-words.json`, deleted unless
//!   `DIARIST_ECHO_KEEP=1`).
//! - `DIARIST_ECHO_OUT`: a scratch folder.
//! - `DIARIST_ECHO_SWEEP=1`: the parameter sweeps (correlation look-back,
//!   text window, floor percentile, bands).
//! - `DIARIST_ECHO_NULL=1`: the live monitor replayed over the 7 s null too
//!   (slow on long recordings; the offline null always runs).
//! - `DIARIST_ECHO_DUMP` (optional): a scratch folder for one TSV per
//!   meeting and path with every word's features and text (local analysis;
//!   delete it after); the transfer TSV also has each cancelled word's level
//!   over the cancelled mic's floor, whether it is a transfer candidate, in a
//!   text run, and of the clean owner's class.
//! - `DIARIST_ECHO_CLASSES_OUT` (optional): a scratch folder; for a meeting
//!   without a live run whose stored `mic.words.json` has words and no
//!   labels were given, the classes CSV `DIARIST_ECHO_CLASSES` reads is
//!   written there from the measurement's matcher (`<uid>.classes.csv`, with
//!   text: local only, delete it after).
//!
//! Per meeting and path it prints the acceptance row: mic words before the
//! gate, dropped per layer, kept, residual duplicates among the kept words
//! by the measurement's matcher (same normalised text within ±0.8 s, or
//! inside a matched 3-word run) with its chance level (the kept words moved
//! 45 s later), the owner's words kept (non-duplicates whose ±5 neighbours
//! are at most 30 percent duplicates, classed on the words before the gate;
//! meeting 12 also against the measurement's labels), and the band.
//!
//! - Post-pass path: the canceller where the post-pass would run it
//!   (coupled, return loss under 20 dB), then the layered gate on the words
//!   the post-pass transcribed (`mic-aec.words.json`, or the canceller's
//!   output transcribed here; `mic.words.json` where it does not engage),
//!   with the levels on the canceller's output. Where the canceller runs,
//!   the transfer gate (plan 15 section 9.2) as the post-pass runs it: the
//!   raw mic's words from the live run (its latest mic revisions) or, for
//!   a meeting without one, the raw mic transcribed here once (EN model,
//!   lookahead 13, timed; cached as `<uid>.raw-words.json`, else
//!   `mic.words.json`), and every guard of [`transfer::Guard`] for both
//!   sources (`transfer` lines; "clean owner's": the owner's words the
//!   canceller cut by under 3 dB).
//!   Plan 16 section 12.2 (`owner`, `rule`, `hist` and `raw-transcript`
//!   lines): the owner's level per path (the sizes of the estimator's sets,
//!   today's branch and the shipped one); per raw source, today's decision
//!   (today's owner's level on the cancelled words), the shipped one, the
//!   floor rule at each F of [`F_SWEEP`] with the owner's level as
//!   estimated and forced unknown, `OVERLAP_MS` 150 and 200, each with the
//!   kept words spoken while the far end was active that the canceller cut
//!   by 3 dB or more ("far-active cut kept", also a column of the table);
//!   histograms of the level over the cancelled mic's floor (kept
//!   candidates cut by 3 dB or more, the clean owner's class, the owner's
//!   class among the cut candidates); and the raw transcript, gated by the
//!   raw gate, in place of the cancelled one.
//! - Both paths: the text layer's lag spread at 300 ms (round two) and
//!   500 ms (`spread` lines: run words, duplicates and owner's among them,
//!   chance with the mic 45 s later).
//! - Live path: the live run's every revision (`live_utterances` in arrival
//!   order) through the live gate ([`LiveEcho`]) with the monitor fed both
//!   channels in 100 ms frames up to each revision's arrival (its last word
//!   plus 150 ms); the live run Stop materialises, and what the view shows
//!   at the end. A meeting without a live run (meeting 12) stands in its
//!   stored transcripts, cut into utterances at pauses over 0.8 s, one
//!   revision each.
//!
//! `cargo test -p diarist-core --release --test echo_sample -- --nocapture`

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diarist_core::audio::aec::offline::{cancel_recording, OfflineOptions};
use diarist_core::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use diarist_core::audio::echo::layers::{self, GateWord, LayerCounts, LayerOptions};
use diarist_core::audio::echo::presence::{floor_sources, presence_sources};
use diarist_core::audio::echo::text::{self, norm, FarIndex, TextWindow};
use diarist_core::audio::echo::transfer::{self, Guard};
use diarist_core::audio::echo::{
    decide_in_order, score_sources, ClockPair, EchoMode, EchoMonitor, EchoProfile, EchoStatus,
    GateBand, GatePad, PcmSource, WordScore, COUPLED_AT, GATE_AFTER,
};
use diarist_core::audio::flac::decode_i16;
use diarist_core::audio::gain_track::{self, GainTrack};
use diarist_core::audio::pipeline::{Frame, Level};
use diarist_core::engine::{Engine, EngineEvent, NemoCliEngine};
use diarist_core::live::echo_gate::LiveEcho;
use diarist_core::live::postpass::{band_after_canceller, AEC_ERL_BELOW_DB};
use diarist_core::live::LiveWord;
use diarist_core::protocol::{DiarPreset, FileJob};
use diarist_core::types::{Channel, ClockAnchor, EchoReason, JobId, MeetingId};

const WIN: f64 = 0.8;
const CHANCE_SHIFT: f64 = 45.0;

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn flag(k: &str) -> bool {
    std::env::var(k).as_deref() == Ok("1")
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

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        f64::NAN
    } else {
        100.0 * a as f64 / b as f64
    }
}

fn quantiles(v: &[f32]) -> String {
    let mut v = v.to_vec();
    v.sort_by(f32::total_cmp);
    if v.is_empty() {
        return "none".into();
    }
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    format!(
        "p10 {:.3} p50 {:.3} p90 {:.3} p99 {:.3} (n {})",
        q(0.1),
        q(0.5),
        q(0.9),
        q(0.99),
        v.len()
    )
}

// ---------------------------------------------------------------- words

/// A word on meeting time (ms) with its text.
#[derive(Debug, Clone)]
struct W {
    start: i64,
    end: i64,
    text: String,
    norm: String,
}

impl W {
    fn new(start: i64, end: i64, text: &str) -> Self {
        Self {
            start,
            end: end.max(start),
            text: text.to_owned(),
            norm: norm(text),
        }
    }
}

/// The runtime's words (channel time), mapped to meeting time.
fn words_json(path: &Path, map: Option<&ClockMap>) -> Vec<W> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let ms = |s: f64| {
        let c = (s * 1_000.0).round() as i64;
        map.map_or(c, |m| m.meeting_ms(c))
    };
    let mut out: Vec<W> = v["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            W::new(
                ms(w["start"].as_f64().unwrap_or(0.0)),
                ms(w["end"].as_f64().unwrap_or(0.0)),
                w["word"].as_str().unwrap_or(""),
            )
        })
        .collect();
    out.sort_by_key(|w| w.start);
    out
}

/// The measurement's matcher (dups.py): a mic word is a duplicate when a
/// loopback word with its text starts within ±0.8 s, or it is inside a mic
/// 3-word run equal to a loopback 3-word run whose first word starts within
/// ±0.8 s of the run's.
struct Matcher {
    single: HashMap<String, Vec<f64>>,
    tri: HashMap<(String, String, String), Vec<f64>>,
}

impl Matcher {
    fn new(lb: &[W]) -> Self {
        let mut single: HashMap<String, Vec<f64>> = HashMap::new();
        for w in lb.iter().filter(|w| !w.norm.is_empty()) {
            single
                .entry(w.norm.clone())
                .or_default()
                .push(w.start as f64 / 1e3);
        }
        let mut tri: HashMap<(String, String, String), Vec<f64>> = HashMap::new();
        for j in 0..lb.len().saturating_sub(2) {
            let (a, b, c) = (&lb[j].norm, &lb[j + 1].norm, &lb[j + 2].norm);
            if !a.is_empty() && !b.is_empty() && !c.is_empty() {
                tri.entry((a.clone(), b.clone(), c.clone()))
                    .or_default()
                    .push(lb[j].start as f64 / 1e3);
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

    /// The single-word matches and the matched 3-word runs' words, apart.
    fn parts(&self, mic: &[&W]) -> (Vec<bool>, Vec<bool>) {
        let t = |w: &W| w.start as f64 / 1e3;
        let single: Vec<bool> = mic
            .iter()
            .map(|w| !w.norm.is_empty() && Self::near(self.single.get(&w.norm), t(w)))
            .collect();
        let mut tri = vec![false; mic.len()];
        for i in 0..mic.len().saturating_sub(2) {
            let (a, b, c) = (&mic[i].norm, &mic[i + 1].norm, &mic[i + 2].norm);
            if a.is_empty() || b.is_empty() || c.is_empty() {
                continue;
            }
            if Self::near(self.tri.get(&(a.clone(), b.clone(), c.clone())), t(mic[i])) {
                tri[i..i + 3].iter_mut().for_each(|x| *x = true);
            }
        }
        (single, tri)
    }

    /// Duplicate flags of words in time order, moved `shift` s later.
    fn dups(&self, mic: &[&W], shift: f64) -> Vec<bool> {
        let t = |w: &W| w.start as f64 / 1e3 + shift;
        let mut d: Vec<bool> = mic
            .iter()
            .map(|w| !w.norm.is_empty() && Self::near(self.single.get(&w.norm), t(w)))
            .collect();
        for i in 0..mic.len().saturating_sub(2) {
            let (a, b, c) = (&mic[i].norm, &mic[i + 1].norm, &mic[i + 2].norm);
            if a.is_empty() || b.is_empty() || c.is_empty() {
                continue;
            }
            if Self::near(self.tri.get(&(a.clone(), b.clone(), c.clone())), t(mic[i])) {
                d[i] = true;
                d[i + 1] = true;
                d[i + 2] = true;
            }
        }
        d
    }
}

/// Owner's words: non-duplicates whose ±5 neighbours are at most 30
/// percent duplicates.
fn owners(dup: &[bool]) -> Vec<bool> {
    let n = dup.len();
    (0..n)
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
        .collect()
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

/// The meeting 12 measurement's labels: (word, duplicate) in its order.
fn labels(path: &Path) -> Vec<(W, bool)> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    let head = csv_fields(lines.next().unwrap());
    let col = |n: &str| head.iter().position(|h| h == n).unwrap();
    let (ct, cs, ce, c1, c3) = (
        col("text"),
        col("start_s"),
        col("end_s"),
        col("single_match"),
        col("trigram_covered"),
    );
    lines
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f = csv_fields(l);
            let s = |k: usize| (f[k].parse::<f64>().unwrap() * 1_000.0).round() as i64;
            (W::new(s(cs), s(ce), &f[ct]), f[c1] == "1" || f[c3] == "1")
        })
        .collect()
}

fn has_word(words: &[&W], n: &str, t: i64) -> bool {
    let k = words.partition_point(|w| w.start < t - 800);
    words[k..]
        .iter()
        .take_while(|w| w.start <= t + 800)
        .any(|w| w.norm == n)
}

// ---------------------------------------------------------------- database

struct Db(rusqlite::Connection);

/// One stored live revision.
struct Rev {
    channel: Channel,
    utt: u32,
    rev: u32,
    words: Vec<LiveWord>,
}

impl Db {
    fn open(path: &Path) -> Self {
        let uri = format!(
            "file:{}?mode=ro",
            path.display().to_string().replace('\\', "/")
        );
        Self(
            rusqlite::Connection::open_with_flags(
                uri,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .unwrap(),
        )
    }

    fn meeting(&self, uid: &str) -> Option<i64> {
        self.0
            .query_row("SELECT id FROM meetings WHERE uid = ?1", [uid], |r| {
                r.get(0)
            })
            .ok()
    }

    fn clocks(&self, id: i64) -> Option<ClockPair> {
        let mut st = self
            .0
            .prepare(
                "SELECT channel, sample_index, qpc_100ns FROM clock_anchors WHERE meeting_id = ?1",
            )
            .ok()?;
        let rows: Vec<ClockAnchor> = st
            .query_map([id], |r| {
                let ch: String = r.get(0)?;
                Ok(ClockAnchor {
                    meeting_id: MeetingId(id),
                    channel: ch.parse().unwrap(),
                    sample_index: r.get(1)?,
                    qpc_100ns: r.get(2)?,
                })
            })
            .ok()?
            .collect::<Result<_, _>>()
            .ok()?;
        let origin = origin_from_anchors(&rows)?;
        let map = |ch| ClockMap::from_anchors(origin, &channel_anchors(&rows, ch));
        ClockPair::usable(map(Channel::Mic), map(Channel::Loopback))
    }

    /// The live run's revisions in arrival order (none without a live run).
    fn live(&self, id: i64) -> Vec<Rev> {
        let run: Option<i64> = self
            .0
            .query_row(
                "SELECT id FROM transcript_runs WHERE meeting_id = ?1 AND kind = 'live' ORDER BY id DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .ok();
        let Some(run) = run else {
            return Vec::new();
        };
        let mut st = self
            .0
            .prepare("SELECT channel, utt, rev, words_json FROM live_utterances WHERE run_id = ?1 ORDER BY id")
            .unwrap();
        st.query_map([run], |r| {
            let ch: String = r.get(0)?;
            let json: String = r.get(3)?;
            Ok((ch, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?, json))
        })
        .unwrap()
        .map(|r| {
            let (ch, utt, rev, json) = r.unwrap();
            let channel: Channel = ch.parse().unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            let words = v
                .as_array()
                .unwrap()
                .iter()
                .map(|w| {
                    let t0 = w["t0"].as_i64().unwrap_or(0);
                    LiveWord {
                        start_ms: t0,
                        end_ms: w["t1"].as_i64().unwrap_or(t0).max(t0),
                        text: w["text"].as_str().unwrap_or("").to_owned(),
                        label: if channel == Channel::Mic { "me" } else { "spk" }.into(),
                        speaker_id: None,
                    }
                })
                .collect();
            Rev {
                channel,
                utt,
                rev,
                words,
            }
        })
        .collect()
    }
}

// ---------------------------------------------------------------- engine

/// A word: start and end (s) and text.
type Timed = (f64, f64, String);

fn transcribe(engine: &dyn Engine, wav: &Path) -> Vec<Timed> {
    let rx = engine
        .transcribe_file(FileJob {
            job_id: JobId::new(),
            wav_path: wav.to_path_buf(),
            diarize: false,
            language: "en".into(),
            diar_preset: DiarPreset::Streaming,
        })
        .unwrap();
    let mut utts: BTreeMap<u32, (u32, Vec<Timed>)> = BTreeMap::new();
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
    utts.into_values().flat_map(|(_, w)| w).collect()
}

fn engine() -> Option<NemoCliEngine> {
    let (dir, models) = (
        env_path("DIARIST_ENGINE_DIR")?,
        env_path("DIARIST_MODELS_DIR")?,
    );
    let asr = find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf")?;
    let diar = find(&models, "Nemotron-3-Diarization.q8_0.gguf")?;
    let mut e = NemoCliEngine::new(&dir, &models, &asr, &diar);
    e.pass_language = false;
    e.right_context = Some(13);
    e.diar_preset = Some("v3-offline".into());
    Some(e)
}

// ---------------------------------------------------------------- a meeting

struct Meeting {
    uid: String,
    folder: PathBuf,
    mic: Vec<i16>,
    lb: Vec<i16>,
    clocks: Option<ClockPair>,
    profile: EchoProfile,
    lb_words: Vec<W>,
    /// The stored mic transcript (the post-pass's without a canceller).
    raw_words: Option<Vec<W>>,
    labels: Option<Vec<(W, bool)>>,
    live: Vec<Rev>,
}

/// One path's numbers.
#[derive(Default)]
struct Row {
    path: String,
    before: usize,
    counts: LayerCounts,
    kept: usize,
    dups_before: usize,
    dups_kept: usize,
    chance_kept: usize,
    own: usize,
    own_kept: usize,
    /// Against the meeting 12 labels (the canceller's own losses included):
    /// kept, the fair count (also a word in the stored transcript's
    /// repeated-token loop, or a spelling variant of a kept word), of.
    labelled: Option<(usize, usize, usize)>,
    band: Option<GateBand>,
    calibrated: bool,
    floor: Option<f32>,
    owner: Option<f32>,
    text3: usize,
    /// Where the owner's level came from (plan 16 section 12.2.2).
    owner_source: Option<&'static str>,
    /// Kept words spoken while the far end was active that the canceller
    /// cut by 3 dB or more (after the canceller; plan 16 section 12.2.3).
    far_cut: Option<usize>,
}

/// Kept words (`drop` false) spoken while the far end was active that the
/// canceller cut by [`transfer::ATTENUATION_DB`] or more.
fn far_cut(gw: &[GateWord], drop: &[bool]) -> usize {
    (0..gw.len())
        .filter(|&i| !drop[i] && cut_far(&gw[i]))
        .count()
}

/// Spoken while the far end was active and cut by the canceller.
fn cut_far(w: &GateWord) -> bool {
    w.presence.is_some_and(|p| {
        p.far_active
            && p.attenuation_db
                .is_some_and(|a| a >= transfer::ATTENUATION_DB)
    })
}

/// The F sweep of the floor rule (plan 16 section 12.2.2 step 3), dB over
/// the cancelled mic's floor; infinite: the canceller's cut alone.
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

/// The owner's level on one path (plan 16 section 12.2.3, first step): the
/// sizes of the estimator's sets, today's branch (the fallback included,
/// no plausibility check) and the shipped one (`canceller`: from the words
/// the canceller left alone, checked against `floor`). Returns today's
/// estimate.
fn owner_line(
    tag: &str,
    gw: &[GateWord],
    far: &FarIndex,
    window: TextWindow,
    floor: Option<f32>,
    canceller: bool,
) -> Option<f32> {
    let s = owner_stats(gw, far, window);
    let c = s.owner_counts();
    let today = s.owner(false, None);
    let now = s.owner(canceller, floor);
    let over = |o: Option<f32>| match (o, floor) {
        (Some(o), Some(f)) => format!("{:+.1} dB over the floor", o - f),
        _ => "-".into(),
    };
    let r1 = |v: Option<f32>| v.map(|x| (x * 10.0).round() / 10.0);
    eprintln!(
        "owner {tag}: {} words; quiet {}, unmatched {}, clean quiet {}, clean {}; today {} {:?} ({} words, {}); shipped {} {:?} ({} words, {}); floor {:?}",
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
        r1(floor)
    );
    today.db
}

/// The owner's level's statistics of words in any order, as the gate builds
/// them.
fn owner_stats(gw: &[GateWord], far: &FarIndex, window: TextWindow) -> layers::CalibrationStats {
    let mut order: Vec<usize> = (0..gw.len()).collect();
    order.sort_by_key(|&i| (gw[i].start_ms, gw[i].end_ms, i));
    let sorted: Vec<GateWord> = order.iter().map(|&i| gw[i].clone()).collect();
    layers::CalibrationStats::of(&sorted, &layers::text_runs(&sorted, far, window))
}

/// The measurement's classes CSV of a stored mic transcript (the format
/// `DIARIST_ECHO_CLASSES` reads), from [`Matcher`]: written where the
/// original is gone (`DIARIST_ECHO_CLASSES_OUT`; text, local only).
fn write_classes(path: &Path, words: &[W], lb: &[W]) {
    let m = Matcher::new(lb);
    let all: Vec<&W> = words.iter().collect();
    let (single, tri) = m.parts(&all);
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut s = String::from("i,start_s,end_s,text,norm,single_match,trigram_covered\n");
    for (i, w) in words.iter().enumerate() {
        s.push_str(&format!(
            "{i},{:.3},{:.3},{},{},{},{}\n",
            w.start as f64 / 1e3,
            w.end as f64 / 1e3,
            q(&w.text),
            q(&w.norm),
            u8::from(single[i]),
            u8::from(tri[i])
        ));
    }
    std::fs::write(path, s).unwrap();
    let dup: Vec<bool> = (0..words.len()).map(|i| single[i] || tri[i]).collect();
    eprintln!(
        "classes written: {} words, {} duplicates, {} owner's",
        words.len(),
        dup.iter().filter(|d| **d).count(),
        owners(&dup).iter().filter(|o| **o).count()
    );
}

fn print_row(uid: &str, r: &Row) {
    let band = r.band.map_or("-".into(), |b| {
        format!(
            "{:.2}/{:.2}/{:.2}{}",
            b.keep_under,
            b.threshold,
            b.drop_at,
            if r.calibrated { " cal" } else { "" }
        )
    });
    let c = &r.counts;
    eprintln!(
        "| {uid} | {} | {} | {} | {} | {} | {} | {} | {} | {} ({:.2} %) | {:.2} % | {} of {} ({:.2} %){} | {band} | {:?} / {:?} | {} | {} | {} |",
        r.path,
        r.before,
        c.text,
        c.attenuation,
        c.level,
        c.correlation,
        c.utterance,
        r.kept,
        r.dups_kept,
        pct(r.dups_kept, r.kept),
        pct(r.chance_kept, r.kept),
        r.own_kept,
        r.own,
        pct(r.own_kept, r.own),
        r.labelled.map_or(String::new(), |(k, f, n)| format!(
            "; labels {k} of {n} ({:.2} %; fair {:.2} %, {} loop or variant)",
            pct(k, n),
            pct(f, n),
            f - k
        )),
        r.floor.map(|f| (f * 10.0).round() / 10.0),
        r.owner.map(|f| (f * 10.0).round() / 10.0),
        r.text3,
        r.owner_source.unwrap_or("-"),
        r.far_cut.map_or("-".into(), |n| n.to_string()),
    );
}

/// Every word's features (`DIARIST_ECHO_DUMP`).
fn dump(
    name: &str,
    words: &[W],
    gw: &[GateWord],
    reasons: &[Option<EchoReason>],
    lb: &[W],
    extra: Option<&[String]>,
) {
    let Some(dir) = env_path("DIARIST_ECHO_DUMP") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let m = Matcher::new(lb);
    let all: Vec<&W> = words.iter().collect();
    let dup = m.dups(&all, 0.0);
    let own = owners(&dup);
    let mut s = String::from(
        "i\tstart\tend\ttext\tdup\town\tlevel\tfar\tatt\tcoef\tgroup\treason\traw_level\tspan_att\n",
    );
    for (i, w) in words.iter().enumerate() {
        let p = gw[i].presence;
        s.push_str(&format!(
            "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            w.start,
            w.end,
            w.text.replace('\t', " "),
            u8::from(dup[i]),
            u8::from(own[i]),
            p.map_or(String::new(), |p| format!("{:.2}", p.level_db)),
            p.map_or(String::new(), |p| u8::from(p.far_active).to_string()),
            p.and_then(|p| p.attenuation_db)
                .map_or(String::new(), |a| format!("{a:.2}")),
            gw[i]
                .score
                .coefficient()
                .map_or(String::new(), |c| format!("{c:.3}")),
            gw[i].group,
            reasons[i].map_or("", |r| r.as_str()),
            extra.and_then(|e| e.get(i)).map_or("\t", |e| e.as_str())
        ));
    }
    std::fs::write(dir.join(format!("{name}.tsv")), s).unwrap();
    let mut l = String::from("start\tend\ttext\n");
    for w in lb {
        l.push_str(&format!(
            "{}\t{}\t{}\n",
            w.start,
            w.end,
            w.text.replace('\t', " ")
        ));
    }
    std::fs::write(dir.join(format!("{name}.lb.tsv")), l).unwrap();
}

/// Kept-word metrics of `words` (time order) with `drop` flags.
fn score_row(row: &mut Row, words: &[W], drop: &[bool], lb: &[W], label_own: Option<&[(W, bool)]>) {
    let m = Matcher::new(lb);
    let all: Vec<&W> = words.iter().collect();
    let dup = m.dups(&all, 0.0);
    let own = owners(&dup);
    let kept: Vec<&W> = words
        .iter()
        .zip(drop)
        .filter(|p| !*p.1)
        .map(|p| p.0)
        .collect();
    row.before = words.len();
    row.kept = kept.len();
    row.dups_before = dup.iter().filter(|d| **d).count();
    row.dups_kept = m.dups(&kept, 0.0).iter().filter(|d| **d).count();
    row.chance_kept = m.dups(&kept, CHANCE_SHIFT).iter().filter(|d| **d).count();
    row.own = own.iter().filter(|o| **o).count();
    row.own_kept = (0..words.len()).filter(|&i| own[i] && !drop[i]).count();
    if let Some(lab) = label_own {
        let found: Vec<bool> = lab
            .iter()
            .map(|(w, _)| has_word(&kept, &w.norm, w.start))
            .collect();
        let k = found.iter().filter(|f| **f).count();
        let fair = (0..lab.len())
            .filter(|&i| found[i] || lab[i].1 || variant(lab, i, &kept))
            .count();
        row.labelled = Some((k, fair, lab.len()));
    }
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

/// Whether labelled owner's word `i` (missing from `new`) is a spelling
/// variant of a word `new` has within ±0.8 s (as tests/aec_postpass.rs: one
/// contains the other, it is merged with a neighbour or split in two, or one
/// edit apart at four letters or more). The labels are the owner's words
/// only, so the neighbours are taken among them.
fn variant(lab: &[(W, bool)], i: usize, new: &[&W]) -> bool {
    let w = &lab[i].0;
    let n = w.norm.as_str();
    if n.is_empty() {
        return false;
    }
    let k = new.partition_point(|v| v.start < w.start - 800);
    let near: Vec<usize> = (k..new.len())
        .take_while(|&j| new[j].start <= w.start + 800)
        .collect();
    let next = lab.get(i + 1).map(|x| x.0.norm.as_str()).unwrap_or("");
    let prev = if i > 0 {
        lab[i - 1].0.norm.as_str()
    } else {
        ""
    };
    near.iter().any(|&j| {
        let v = new[j].norm.as_str();
        let v2 = new.get(j + 1).map(|x| x.norm.as_str()).unwrap_or("");
        let short = v.len().min(n.len());
        (short >= 3 && (v.contains(n) || n.contains(v)))
            || v == format!("{n}{next}")
            || v == format!("{prev}{n}")
            || n == format!("{v}{v2}")
            || (short >= 4 && levenshtein(v.as_bytes(), n.as_bytes()) <= 1)
    })
}

/// The meeting 12 owner's words from the labels, each with whether it is in
/// a repeated-token loop of the stored transcript (four or more words of
/// one text under 150 ms apart on average: the engine repeating a token at
/// one instant; on meeting 12 one token twenty times in 0.88 s, 17 of them
/// labelled the owner's), counted as kept in the fair count as in
/// docs/eval/2026-10-20-echo.md.
fn label_owners(labels: &[(W, bool)]) -> Vec<(W, bool)> {
    let dup: Vec<bool> = labels.iter().map(|l| l.1).collect();
    let mut looped = vec![false; labels.len()];
    let mut i = 0;
    while i < labels.len() {
        let mut j = i;
        while j + 1 < labels.len()
            && labels[j + 1].0.norm == labels[i].0.norm
            && !labels[i].0.norm.is_empty()
        {
            j += 1;
        }
        if j + 1 - i >= 4
            && (labels[j].0.start - labels[i].0.start) as f64 / ((j - i) as f64) < 150.0
        {
            looped[i..=j].iter_mut().for_each(|o| *o = true);
        }
        i = j + 1;
    }
    owners(&dup)
        .into_iter()
        .zip(labels)
        .zip(looped)
        .filter(|p| p.0 .0)
        .map(|p| (p.0 .1 .0.clone(), p.1))
        .collect()
}

fn spans_of(words: &[W]) -> Vec<(i64, i64)> {
    words.iter().map(|w| (w.start * 16, w.end * 16)).collect()
}

/// The post-pass path (module docs).
fn post_pass_path(m: &Meeting, out: &Path, engine: Option<&NemoCliEngine>) -> Option<Row> {
    let p = &m.profile;
    let engage = p.coupled && p.erl_db.is_some_and(|e| e < AEC_ERL_BELOW_DB);
    let mic_map = m.clocks.as_ref().map(|c| &c.mic);
    let mut aec: Option<Vec<i16>> = None;
    let mut erle: Option<f32> = None;
    if engage {
        let t = Instant::now();
        let gain = GainTrack::read(&gain_track::path_for(&m.folder.join("loopback.wav")))
            .ok()
            .flatten()
            .filter(|g| g.frames() > 0);
        let hint = |t: i64| p.lag_at(t);
        let mut o = OfflineOptions::post_pass(m.clocks.as_ref().map(|c| (&c.mic, &c.loopback)));
        o.lag_hint = Some(&hint);
        o.loopback_gain = gain.as_ref();
        let mut y = Vec::with_capacity(m.mic.len());
        let (mm, ll) = (&m.mic[..], &m.lb[..]);
        let r = cancel_recording(&mm, &ll, &o, &mut |b| {
            y.extend(
                b.iter()
                    .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16),
            );
            Ok(())
        })
        .unwrap();
        eprintln!(
            "{}: canceller ERLE {:?} dB over {} far-end blocks ({}gain track), {:.1} s",
            m.uid,
            r.erle_db,
            r.far_blocks,
            if gain.is_some() { "" } else { "no " },
            t.elapsed().as_secs_f64()
        );
        erle = r.erle_db;
        aec = Some(y);
    }
    let words = if let Some(y) = &aec {
        let stored = m.folder.join("mic-aec.words.json");
        let cache = out.join(format!("{}.aec-words.json", m.uid));
        if stored.is_file() {
            words_json(&stored, mic_map)
        } else if cache.is_file() {
            words_json(&cache, mic_map)
        } else {
            let engine = engine?;
            let wav = out.join(format!("{}.mic-aec.wav", m.uid));
            let mut w = diarist_core::audio::wav::WavSink::create(&wav).unwrap();
            w.write(y).unwrap();
            w.finalize().unwrap();
            let t = Instant::now();
            let ws = transcribe(engine, &wav);
            let _ = std::fs::remove_file(&wav);
            eprintln!(
                "{}: transcribed the canceller's output in {:.1} s",
                m.uid,
                t.elapsed().as_secs_f64()
            );
            let v = serde_json::json!({"words": ws.iter().map(|(s, e, t)| serde_json::json!({"start": s, "end": e, "word": t})).collect::<Vec<_>>()});
            std::fs::write(&cache, serde_json::to_vec(&v).unwrap()).unwrap();
            words_json(&cache, mic_map)
        }
    } else {
        m.raw_words.clone()?
    };
    let spans = spans_of(&words);
    let heard: &[i16] = aec.as_deref().unwrap_or(&m.mic);
    let correlation = p.gates(EchoMode::Auto, aec.is_some());
    let fixed = if aec.is_some() {
        band_after_canceller(0.5)
    } else {
        GateBand::default()
    };
    let scores = if correlation {
        score_sources(&spans, &m.mic[..], &m.lb[..], p, GatePad::DEFAULT)
    } else {
        vec![WordScore::Undefined; spans.len()]
    };
    let raw: Option<Box<dyn PcmSource>> = aec
        .as_ref()
        .map(|_| Box::new(&m.mic[..]) as Box<dyn PcmSource>);
    let pres = presence_sources(&spans, heard, raw, &m.lb[..], m.clocks.clone());
    let floor = floor_sources(heard, &m.lb[..], m.clocks.clone());
    let groups = layers::gap_groups(words.iter().map(|w| (w.start, w.end)));
    let gw: Vec<GateWord> = (0..words.len())
        .map(|i| GateWord {
            start_ms: words[i].start,
            end_ms: words[i].end,
            norm: words[i].norm.clone(),
            group: groups[i],
            score: scores[i],
            presence: pres[i],
        })
        .collect();
    let far = FarIndex::new(m.lb_words.iter().map(|w| (w.start, w.norm.clone())));
    let opts = LayerOptions {
        correlation,
        canceller: aec.is_some(),
        fixed,
        threshold: 0.5,
        window: TextWindow::DEFAULT,
        // As the post-pass: the band is fixed after the canceller.
        calibrate: aec.is_none(),
    };
    let r = layers::gate(&gw, &far, floor.floor_db(), &opts);
    let drop: Vec<bool> = r.reasons.iter().map(Option::is_some).collect();
    owner_line(
        &format!("{} post-pass", m.uid),
        &gw,
        &far,
        opts.window,
        floor.floor_db(),
        aec.is_some(),
    );
    if env_path("DIARIST_ECHO_DUMP").is_some() {
        // The raw mic's level, and the canceller's attenuation over the
        // word's own span (no look-back).
        let raw_pres = presence_sources(&spans, &m.mic[..], None, &m.lb[..], m.clocks.clone());
        let to_sample = |ms: i64| -> usize {
            m.clocks
                .as_ref()
                .map_or(ms * 16, |c| {
                    c.mic.sample_at(c.mic.origin_qpc as f64 + ms as f64 * 1e4) as i64
                })
                .max(0) as usize
        };
        let energy = |x: &[i16], a: usize, b: usize| -> f64 {
            x[a.min(x.len())..b.min(x.len())]
                .iter()
                .map(|&v| (f64::from(v) / 32_768.0).powi(2))
                .sum()
        };
        let extra: Vec<String> = (0..words.len())
            .map(|i| {
                let lvl = raw_pres[i].map_or(String::new(), |p| format!("{:.2}", p.level_db));
                let att = aec.as_ref().map_or(String::new(), |y| {
                    let (a, b) = (
                        to_sample(words[i].start),
                        to_sample(words[i].end.max(words[i].start + 20)),
                    );
                    let (er, ec) = (energy(&m.mic, a, b), energy(y, a, b));
                    if er > 0.0 && ec > 0.0 {
                        format!("{:.2}", 10.0 * (er / ec).log10())
                    } else {
                        String::new()
                    }
                });
                format!("{lvl}\t{att}")
            })
            .collect();
        dump(
            &format!("{}.pp", m.uid),
            &words,
            &gw,
            &r.reasons,
            &m.lb_words,
            Some(&extra),
        );
    }
    let mut row = Row {
        path: "post-pass".into(),
        counts: r.counts,
        band: correlation.then_some(r.calibration.band),
        calibrated: r.calibration.calibrated,
        floor: r.floor_db,
        owner: r.owner_db,
        text3: r.run3,
        owner_source: Some(r.owner_source.as_str()),
        far_cut: aec.is_some().then(|| far_cut(&gw, &drop)),
        ..Row::default()
    };
    let lab = m.labels.as_ref().map(|l| label_owners(l));
    score_row(&mut row, &words, &drop, &m.lb_words, lab.as_deref());
    spread_check(&format!("{} post-pass", m.uid), &words, &m.lb_words);
    if aec.is_none() {
        // Round two's lag spread under the whole decision: the same words?
        let narrow = layers::gate(
            &gw,
            &far,
            floor.floor_db(),
            &LayerOptions {
                window: TextWindow {
                    spread_ms: 300,
                    ..TextWindow::DEFAULT
                },
                ..opts
            },
        );
        let differ = (0..gw.len())
            .filter(|&i| narrow.reasons[i].is_some() != r.reasons[i].is_some())
            .count();
        eprintln!(
            "spread {} post-pass: the whole gate at 300 ms drops {} {:?}, at 500 ms {} {:?}; {differ} decisions differ",
            m.uid,
            narrow.dropped(),
            narrow.counts,
            r.dropped(),
            r.counts
        );
    }
    if aec.is_some() {
        // The shipped decision is the transfer gate's; the row above was
        // the post-canceller layers alone.
        print_row(&format!("{} (layers alone)", m.uid), &row);
        let narrow = layers::gate(
            &gw,
            &far,
            floor.floor_db(),
            &LayerOptions {
                window: TextWindow {
                    spread_ms: 300,
                    ..TextWindow::DEFAULT
                },
                ..opts
            },
        );
        let mut x = Row {
            path: "post-pass, layers alone, spread 300 (round two)".into(),
            counts: narrow.counts,
            band: correlation.then_some(narrow.calibration.band),
            ..Row::default()
        };
        let d: Vec<bool> = narrow.reasons.iter().map(Option::is_some).collect();
        score_row(&mut x, &words, &d, &m.lb_words, lab.as_deref());
        print_row(&m.uid, &x);
        let sources = raw_sources(m, out, engine);
        if let Some(t) = transfer_rows(
            m,
            &words,
            &gw,
            &far,
            floor.floor_db(),
            &opts,
            &sources,
            lab.as_deref(),
            erle,
        ) {
            row = Row {
                path: format!("post-pass (transfer, {})", t.0),
                band: correlation.then_some(r.calibration.band),
                calibrated: r.calibration.calibrated,
                floor: r.floor_db,
                owner: r.owner_db,
                text3: r.run3,
                owner_source: Some(r.owner_source.as_str()),
                ..t.1
            };
        }
    }
    if flag("DIARIST_ECHO_SWEEP") {
        sweep(
            m,
            "post-pass",
            &words,
            &gw,
            &far,
            &floor,
            &opts,
            aec.as_deref(),
        );
        if aec.is_some() {
            for v in aec_variants() {
                let r = layers::gate_with(&gw, &far, floor.floor_db(), &opts, &|c| v.apply(c));
                let d: Vec<bool> = r.reasons.iter().map(Option::is_some).collect();
                let mut x = Row::default();
                score_row(&mut x, &words, &d, &m.lb_words, lab.as_deref());
                eprintln!(
                    "aec variant {}: {} dropped {} {:?}; kept {}, duplicates {} ({:.2} %), owner-class {} of {} ({:.2} %){}",
                    m.uid,
                    v.label(),
                    r.dropped(),
                    r.counts,
                    x.kept,
                    x.dups_kept,
                    pct(x.dups_kept, x.kept),
                    x.own_kept,
                    x.own,
                    pct(x.own_kept, x.own),
                    x.labelled.map_or(String::new(), |(k, f, n)| format!("; labels {k} ({:.2} %), fair {f} ({:.2} %) of {n}", pct(k, n), pct(f, n)))
                );
            }
        }
    }
    Some(row)
}

/// The text layer alone at round two's lag spread and the shipped one
/// (module docs), over words in time order against `lb`.
fn spread_check(tag: &str, words: &[W], lb: &[W]) {
    let mt = Matcher::new(lb);
    let all: Vec<&W> = words.iter().collect();
    let dup = mt.dups(&all, 0.0);
    let own = owners(&dup);
    let far = FarIndex::new(lb.iter().map(|w| (w.start, w.norm.clone())));
    let mic: Vec<(i64, &str)> = words.iter().map(|w| (w.start, w.norm.as_str())).collect();
    let shifted: Vec<(i64, &str)> = words
        .iter()
        .map(|w| (w.start + (CHANCE_SHIFT * 1e3) as i64, w.norm.as_str()))
        .collect();
    for spread_ms in [300i64, 500] {
        let window = TextWindow {
            spread_ms,
            ..TextWindow::DEFAULT
        };
        let r = text::runs(&mic, &far, window);
        let c = text::runs(&shifted, &far, window);
        let n = |f: &dyn Fn(usize) -> bool| (0..words.len()).filter(|&i| f(i)).count();
        eprintln!(
            "spread {tag}: {spread_ms} ms: run3 {} words (duplicates {}, owner's {}), run2 {} (owner's {}); chance run3 {} run2 {}; of {} words ({} duplicates, {} owner's)",
            r.count3(),
            n(&|i| r.run3[i] && dup[i]),
            n(&|i| r.run3[i] && own[i]),
            r.count2(),
            n(&|i| r.run2[i] && own[i]),
            c.count3(),
            c.count2(),
            words.len(),
            n(&|i| dup[i]),
            n(&|i| own[i])
        );
    }
}

/// The raw mic's words for the transfer gate (module docs): the live run's
/// latest mic revisions, and the raw mic transcribed (with its wall time).
fn raw_sources(
    m: &Meeting,
    out: &Path,
    engine: Option<&NemoCliEngine>,
) -> Vec<(&'static str, Vec<W>, Option<f64>)> {
    let mut v = Vec::new();
    if !m.live.is_empty() {
        let mut latest: BTreeMap<u32, &Rev> = BTreeMap::new();
        for r in m.live.iter().filter(|r| r.channel == Channel::Mic) {
            if latest.get(&r.utt).is_none_or(|o| r.rev >= o.rev) {
                latest.insert(r.utt, r);
            }
        }
        let mut w: Vec<W> = latest
            .values()
            .flat_map(|r| r.words.iter())
            .map(|w| W::new(w.start_ms, w.end_ms, &w.text))
            .collect();
        w.sort_by_key(|w| (w.start, w.end));
        v.push(("live", w, None));
    }
    let mic_map = m.clocks.as_ref().map(|c| &c.mic);
    let cache = out.join(format!("{}.raw-words.json", m.uid));
    if !cache.is_file() {
        if let Some(engine) = engine {
            let wav = out.join(format!("{}.mic.wav", m.uid));
            let mut w = diarist_core::audio::wav::WavSink::create(&wav).unwrap();
            w.write(&m.mic).unwrap();
            w.finalize().unwrap();
            let t = Instant::now();
            let ws = transcribe(engine, &wav);
            let secs = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_file(&wav);
            eprintln!(
                "{}: transcribed the raw mic ({:.1} min) in {secs:.1} s",
                m.uid,
                m.mic.len() as f64 / 960_000.0
            );
            let j = serde_json::json!({"secs": secs, "words": ws.iter().map(|(s, e, t)| serde_json::json!({"start": s, "end": e, "word": t})).collect::<Vec<_>>()});
            std::fs::write(&cache, serde_json::to_vec(&j).unwrap()).unwrap();
        }
    }
    if cache.is_file() {
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
        v.push((
            "transcribed",
            words_json(&cache, mic_map),
            j["secs"].as_f64(),
        ));
    } else if let Some(w) = &m.raw_words {
        v.push(("transcribed (stored mic.words.json)", w.clone(), None));
    }
    v
}

/// The transfer gate over the cancelled words `words` (their gate words
/// `gw`, the post-canceller options `opts`) per raw source and guard; the
/// shipped one's row (the live source when there is one).
#[allow(clippy::too_many_arguments)]
fn transfer_rows(
    m: &Meeting,
    words: &[W],
    gw: &[GateWord],
    far: &FarIndex,
    floor_db: Option<f32>,
    opts: &LayerOptions,
    sources: &[(&'static str, Vec<W>, Option<f64>)],
    lab: Option<&[(W, bool)]>,
    erle: Option<f32>,
) -> Option<(&'static str, Row)> {
    let mt = Matcher::new(&m.lb_words);
    let all: Vec<&W> = words.iter().collect();
    let own = owners(&mt.dups(&all, 0.0));
    let clean: Vec<bool> = (0..words.len())
        .map(|i| {
            own[i]
                && gw[i]
                    .presence
                    .and_then(|p| p.attenuation_db)
                    .is_none_or(|a| a < 3.0)
        })
        .collect();
    let n_clean = clean.iter().filter(|c| **c).count();
    // The post-canceller layers alone, at both lag spreads.
    for spread_ms in [300i64, 500] {
        let o = LayerOptions {
            window: TextWindow {
                spread_ms,
                ..TextWindow::DEFAULT
            },
            ..*opts
        };
        let r = layers::gate(gw, far, floor_db, &o);
        let d: Vec<bool> = r.reasons.iter().map(Option::is_some).collect();
        let mut x = Row::default();
        score_row(&mut x, words, &d, &m.lb_words, lab);
        let clean_kept = (0..words.len()).filter(|&i| clean[i] && !d[i]).count();
        eprintln!(
            "transfer {} layers alone, spread {spread_ms} ms: dropped {} {:?}; kept {}, residual {} ({:.2} %, chance {:.2} %), owner's {} of {} ({:.2} %), clean owner's {clean_kept} of {n_clean} ({:.2} %){}",
            m.uid,
            r.dropped(),
            r.counts,
            x.kept,
            x.dups_kept,
            pct(x.dups_kept, x.kept),
            pct(x.chance_kept, x.kept),
            x.own_kept,
            x.own,
            pct(x.own_kept, x.own),
            pct(clean_kept, n_clean),
            x.labelled.map_or(String::new(), |(k, f, n)| format!(
                "; labels {k} ({:.2} %), fair {f} ({:.2} %) of {n}",
                pct(k, n),
                pct(f, n)
            ))
        );
    }
    let raw_floor = floor_sources(&m.mic[..], &m.lb[..], m.clocks.clone()).floor_db();
    let mut shipped = None;
    for (name, raw, secs) in sources {
        let spans = spans_of(raw);
        let scores = if opts.correlation {
            score_sources(&spans, &m.mic[..], &m.lb[..], &m.profile, GatePad::DEFAULT)
        } else {
            vec![WordScore::Undefined; spans.len()]
        };
        let pres = presence_sources(&spans, &m.mic[..], None, &m.lb[..], m.clocks.clone());
        let groups = layers::gap_groups(raw.iter().map(|w| (w.start, w.end)));
        let raw_gw: Vec<GateWord> = (0..raw.len())
            .map(|i| GateWord {
                start_ms: raw[i].start,
                end_ms: raw[i].end,
                norm: raw[i].norm.clone(),
                group: groups[i],
                score: scores[i],
                presence: pres[i],
            })
            .collect();
        let raw_opts = LayerOptions {
            canceller: false,
            fixed: GateBand::default(),
            calibrate: true,
            ..*opts
        };
        let mut decisions: Vec<Vec<Option<EchoReason>>> = Vec::new();
        let mut raw_reasons: Vec<Option<EchoReason>> = Vec::new();
        for guard in Guard::ALL {
            let t = transfer::gate(
                &raw_gw, raw_floor, &raw_opts, gw, floor_db, opts, far, guard,
            );
            let d: Vec<bool> = t.reasons.iter().map(Option::is_some).collect();
            let mut x = Row {
                path: format!("post-pass, transfer from {name}, guard {}", guard.as_str()),
                counts: t.counts,
                ..Row::default()
            };
            score_row(&mut x, words, &d, &m.lb_words, lab);
            let clean_kept = (0..words.len()).filter(|&i| clean[i] && !d[i]).count();
            let c = t.transfer;
            let rb = t.raw.calibration.band;
            eprintln!(
                "transfer {} {name} {}: raw {} words ({}), {} echo {:?} (band {:.2}/{:.2}/{:.2}{}); candidates {}, removed {} (text {}, level {}, plain {}), guarded {}, unmatched {}; dropped {} {:?}; kept {}, residual {} ({:.2} %, chance {:.2} %), owner's {} of {} ({:.2} %), clean owner's {} of {n_clean} ({:.2} %){}",
                m.uid,
                guard.as_str(),
                raw.len(),
                secs.map_or("-".into(), |s| format!("{s:.1} s")),
                t.raw.dropped(),
                t.raw.counts,
                rb.keep_under,
                rb.threshold,
                rb.drop_at,
                if t.raw.calibration.calibrated { " cal" } else { "" },
                c.candidates,
                c.removed(),
                c.text,
                c.level,
                c.plain,
                c.guarded,
                c.unmatched,
                t.dropped(),
                t.counts,
                x.kept,
                x.dups_kept,
                pct(x.dups_kept, x.kept),
                pct(x.chance_kept, x.kept),
                x.own_kept,
                x.own,
                pct(x.own_kept, x.own),
                clean_kept,
                pct(clean_kept, n_clean),
                x.labelled.map_or(String::new(), |(k, f, n)| format!(
                    "; labels {k} ({:.2} %), fair {f} ({:.2} %) of {n}",
                    pct(k, n),
                    pct(f, n)
                ))
            );
            if guard == transfer::GUARD && shipped.is_none() {
                x.owner_source = Some(t.after.owner_source.as_str());
                x.far_cut = Some(far_cut(gw, &d));
                shipped = Some((*name, x));
            }
            decisions.push(t.reasons.clone());
            raw_reasons = t.raw.reasons.clone();
        }
        // Plan 16 section 12.2: the raw gate's owner's level; today's
        // decision (today's estimator on the cancelled words), the shipped
        // one, the floor rule's F as estimated and with the owner's level
        // forced unknown, the overlap variants; histograms of level over the
        // floor; the raw transcript in place of the cancelled one.
        owner_line(
            &format!("{} raw gate ({name})", m.uid),
            &raw_gw,
            far,
            raw_opts.window,
            raw_floor,
            false,
        );
        let today = owner_stats(gw, far, opts.window).owner_db();
        let decide = |rule: transfer::Rule, owner: Option<Option<f32>>| {
            transfer::gate_with(
                &raw_gw,
                raw_floor,
                &raw_opts,
                gw,
                floor_db,
                opts,
                far,
                &rule,
                &move |c: &mut layers::Context| {
                    if let Some(o) = owner {
                        c.owner_db = o;
                    }
                },
            )
        };
        let line = |label: &str, t: &transfer::TransferReport| {
            let d: Vec<bool> = t.reasons.iter().map(Option::is_some).collect();
            let mut x = Row::default();
            score_row(&mut x, words, &d, &m.lb_words, lab);
            let clean_kept = (0..words.len()).filter(|&i| clean[i] && !d[i]).count();
            eprintln!(
                "rule {} {name} {label}: owner {:?} ({}); removed {} (level {}, floor rule {}), guarded {}; dropped {} {:?}; kept {}, far-active cut kept {}, residual {} ({:.2} %, chance {:.2} %), owner's {} of {} ({:.2} %), clean owner's {clean_kept} of {n_clean}{}",
                m.uid,
                t.after.owner_db.map(|v| (v * 10.0).round() / 10.0),
                t.after.owner_source.as_str(),
                t.transfer.removed(),
                t.transfer.level,
                t.transfer.floor,
                t.transfer.guarded,
                t.dropped(),
                t.counts,
                x.kept,
                far_cut(gw, &d),
                x.dups_kept,
                pct(x.dups_kept, x.kept),
                pct(x.chance_kept, x.kept),
                x.own_kept,
                x.own,
                pct(x.own_kept, x.own),
                x.labelled.map_or(String::new(), |(k, f, n)| format!(
                    "; labels {k} ({:.2} %), fair {f} ({:.2} %) of {n}",
                    pct(k, n),
                    pct(f, n)
                ))
            );
        };
        let t_today = decide(transfer::Rule::SHIPPED, Some(today));
        line("today (today's owner's level)", &t_today);
        line(
            &format!("shipped (F {} dB)", transfer::FLOOR_MARGIN_DB),
            &transfer::gate(
                &raw_gw,
                raw_floor,
                &raw_opts,
                gw,
                floor_db,
                opts,
                far,
                transfer::GUARD,
            ),
        );
        for f in F_SWEEP {
            let rule = transfer::Rule {
                floor_margin_db: f,
                ..transfer::Rule::SHIPPED
            };
            line(&format!("F {f} as estimated"), &decide(rule, None));
            line(&format!("F {f} forced unknown"), &decide(rule, Some(None)));
        }
        for ov in [150i64, 200] {
            let rule = transfer::Rule {
                overlap_ms: ov,
                ..transfer::Rule::SHIPPED
            };
            line(&format!("overlap {ov} ms"), &decide(rule, None));
        }
        {
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
            let (cand, _) = transfer::transfer(gw, &raw_words, None, None, Guard::Plain);
            let over = |i: usize| gw[i].presence.zip(floor_db).map(|(p, f)| p.level_db - f);
            let cut = |i: usize| {
                gw[i]
                    .presence
                    .and_then(|p| p.attenuation_db)
                    .is_some_and(|a| a >= transfer::ATTENUATION_DB)
            };
            let kept = |i: usize| t_today.reasons[i].is_none();
            let h = |sel: &dyn Fn(usize) -> bool| -> String {
                let v: Vec<f32> = (0..gw.len()).filter(|&i| sel(i)).filter_map(over).collect();
                hist(&v)
            };
            eprintln!(
                "hist {} {name}: kept today, candidates cut 3 dB or more: {}",
                m.uid,
                h(&|i| kept(i) && cand[i] && cut(i))
            );
            eprintln!(
                "hist {} {name}: kept today, far-active cut words: {}",
                m.uid,
                h(&|i| kept(i) && cut_far(&gw[i]))
            );
            eprintln!("hist {} {name}: clean owner's: {}", m.uid, h(&|i| clean[i]));
            eprintln!(
                "hist {} {name}: owner's, candidates cut 3 dB or more: {}",
                m.uid,
                h(&|i| own[i] && cand[i] && cut(i))
            );
            eprintln!(
                "hist {} {name}: owner's, cut 3 dB or more: {}",
                m.uid,
                h(&|i| own[i] && cut(i))
            );
            // The raw mic's transcript, gated by the raw gate, in place of
            // the cancelled one (plan 16 section 12.2.4).
            let d: Vec<bool> = t_today.raw.reasons.iter().map(Option::is_some).collect();
            let mut x = Row::default();
            score_row(&mut x, raw, &d, &m.lb_words, None);
            let far_kept = |g: &[GateWord], d: &[bool]| {
                (0..g.len())
                    .filter(|&i| !d[i] && g[i].presence.is_some_and(|p| p.far_active))
                    .count()
            };
            let dc: Vec<bool> = t_today.reasons.iter().map(Option::is_some).collect();
            eprintln!(
                "raw-transcript {} {name} (canceller ERLE {:?} dB): raw gate drops {} {:?}; kept {}, far-active kept {} (cancelled today: {} of {} kept), residual {} ({:.2} %, chance {:.2} %), owner's {} of {} ({:.2} %)",
                m.uid,
                erle.map(|e| (e * 10.0).round() / 10.0),
                t_today.raw.dropped(),
                t_today.raw.counts,
                x.kept,
                far_kept(&raw_gw, &d),
                far_kept(gw, &dc),
                dc.iter().filter(|d| !**d).count(),
                x.dups_kept,
                pct(x.dups_kept, x.kept),
                pct(x.chance_kept, x.kept),
                x.own_kept,
                x.own,
                pct(x.own_kept, x.own)
            );
        }
        if let Some(dir) = env_path("DIARIST_ECHO_DUMP") {
            // Every cancelled word with its raw neighbours and each guard's
            // decision (local analysis; delete it after).
            std::fs::create_dir_all(&dir).unwrap();
            let dup = mt.dups(&all, 0.0);
            // Plan 16 section 12.2.2: the level over the cancelled mic's
            // floor, candidate, in a text run, the clean owner's class.
            let raw_flags: Vec<transfer::RawWord> = raw_gw
                .iter()
                .zip(&raw_reasons)
                .map(|(w, r)| transfer::RawWord {
                    start_ms: w.start_ms,
                    end_ms: w.end_ms,
                    norm: &w.norm,
                    echo: r.is_some(),
                })
                .collect();
            let (cand, _) = transfer::transfer(gw, &raw_flags, None, None, Guard::Plain);
            let runs = layers::text_runs(gw, far, opts.window);
            let mut f = String::from("start\tend\ttext\tdup\town\tatt\tlevel\tfar\tcoef");
            for g in Guard::ALL {
                f.push_str(&format!("\t{}", g.as_str()));
            }
            f.push_str("\tover_floor\tcand\trun\tclean_owner\traw\n");
            for (i, w) in words.iter().enumerate() {
                let p = gw[i].presence;
                let near: Vec<String> = raw
                    .iter()
                    .zip(&raw_reasons)
                    .filter(|(r, _)| {
                        r.start <= w.end.max(w.start) + transfer::OVERLAP_MS
                            && r.end.max(r.start) >= w.start - transfer::OVERLAP_MS
                    })
                    .map(|(r, why)| format!("{}:{}", r.text, why.map_or("", |x| x.as_str())))
                    .collect();
                f.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    w.start,
                    w.end,
                    w.text.replace('\t', " "),
                    u8::from(dup[i]),
                    u8::from(own[i]),
                    p.and_then(|p| p.attenuation_db)
                        .map_or(String::new(), |a| format!("{a:.2}")),
                    p.map_or(String::new(), |p| format!("{:.2}", p.level_db)),
                    p.map_or(String::new(), |p| u8::from(p.far_active).to_string()),
                    gw[i]
                        .score
                        .coefficient()
                        .map_or(String::new(), |c| format!("{c:.3}")),
                ));
                for d in &decisions {
                    f.push_str(&format!("\t{}", d[i].map_or("", |r| r.as_str())));
                }
                f.push_str(&format!(
                    "\t{}\t{}\t{}\t{}\t{}\n",
                    p.zip(floor_db)
                        .map_or(String::new(), |(p, fl)| format!("{:.2}", p.level_db - fl)),
                    u8::from(cand[i]),
                    u8::from(runs.any(i)),
                    u8::from(clean[i]),
                    near.join("|")
                ));
            }
            std::fs::write(
                dir.join(format!(
                    "{}.transfer.{}.tsv",
                    m.uid,
                    name.split(' ').next().unwrap_or(name)
                )),
                f,
            )
            .unwrap();
        }
    }
    shipped
}

/// The parameter sweeps over one path's words (module docs).
#[allow(clippy::too_many_arguments)]
fn sweep(
    m: &Meeting,
    path: &str,
    words: &[W],
    gw: &[GateWord],
    far: &FarIndex,
    floor: &diarist_core::audio::echo::presence::FloorTracker,
    opts: &LayerOptions,
    aec: Option<&[i16]>,
) {
    let mt = Matcher::new(&m.lb_words);
    let all: Vec<&W> = words.iter().collect();
    let dup = mt.dups(&all, 0.0);
    let own = owners(&dup);
    let n_dup = dup.iter().filter(|d| **d).count();
    let n_own = own.iter().filter(|o| **o).count();
    let tag = format!("{} {path}", m.uid);
    // Text layer alone: window edges, chance with the mic 45 s later.
    for lo in [-500i64, -150] {
        let window = TextWindow {
            lo_ms: lo,
            ..TextWindow::DEFAULT
        };
        let mic: Vec<(i64, &str)> = gw.iter().map(|w| (w.start_ms, w.norm.as_str())).collect();
        let r = text::runs(&mic, far, window);
        let shifted: Vec<(i64, &str)> = gw
            .iter()
            .map(|w| (w.start_ms + (CHANCE_SHIFT * 1e3) as i64, w.norm.as_str()))
            .collect();
        let rc = text::runs(&shifted, far, window);
        let t3 = |i: usize| r.run3[i];
        let t2 = |i: usize| r.run2[i];
        eprintln!(
            "sweep {tag}: text window {lo:+}..+1000 ms: run3 {} words (duplicates {} of {n_dup}, owner's {} of {n_own}), run2 {} (owner's {}); chance run3 {} run2 {}",
            r.count3(),
            (0..gw.len()).filter(|&i| t3(i) && dup[i]).count(),
            (0..gw.len()).filter(|&i| t3(i) && own[i]).count(),
            r.count2(),
            (0..gw.len()).filter(|&i| t2(i) && own[i]).count(),
            rc.count3(),
            rc.count2()
        );
    }
    // The floor percentile: the level layer alone.
    for p in [0.1, 0.2, 0.3] {
        let f = floor.percentile(p);
        let ctx = layers::Context::new(
            f,
            layers::CalibrationStats::of(gw, &layers::text_runs(gw, far, opts.window)).owner_db(),
            opts.fixed,
            false,
            false,
            true,
        );
        let empty = text::Runs {
            run3: vec![false; gw.len()],
            run2: vec![false; gw.len()],
        };
        let d = layers::decide(gw, &empty, &ctx);
        let lvl = |i: usize| d[i] == Some(EchoReason::Level);
        eprintln!(
            "sweep {tag}: floor p{:.0} = {:?} dB (owner {:?}): level layer alone drops {} (duplicates {}, owner's {})",
            p * 100.0,
            f,
            ctx.owner_db,
            (0..gw.len()).filter(|&i| lvl(i)).count(),
            (0..gw.len()).filter(|&i| lvl(i) && dup[i]).count(),
            (0..gw.len()).filter(|&i| lvl(i) && own[i]).count()
        );
    }
    // Attenuation alone (after the canceller).
    if aec.is_some() {
        let mut att_d: Vec<f32> = Vec::new();
        let mut att_o: Vec<f32> = Vec::new();
        for (i, w) in gw.iter().enumerate() {
            if let Some(a) = w.presence.and_then(|p| p.attenuation_db) {
                if dup[i] {
                    att_d.push(a);
                } else if own[i] {
                    att_o.push(a);
                }
            }
        }
        eprintln!(
            "sweep {tag}: attenuation: duplicates {}; owner's {}",
            quantiles(&att_d),
            quantiles(&att_o)
        );
        for thr in [2.0f32, 3.0, 4.0, 6.0] {
            let hit = |i: usize| {
                gw[i]
                    .presence
                    .and_then(|p| p.attenuation_db)
                    .is_some_and(|a| a >= thr)
            };
            eprintln!(
                "sweep {tag}: attenuation >= {thr} dB drops duplicates {} of {n_dup}, owner's {} of {n_own}",
                (0..gw.len()).filter(|&i| hit(i) && dup[i]).count(),
                (0..gw.len()).filter(|&i| hit(i) && own[i]).count()
            );
        }
    }
    // Level distributions.
    let lv = |sel: &dyn Fn(usize) -> bool| -> Vec<f32> {
        (0..gw.len())
            .filter(|&i| sel(i))
            .filter_map(|i| gw[i].presence.map(|p| p.level_db))
            .collect()
    };
    let far_on = |i: usize| gw[i].presence.is_some_and(|p| p.far_active);
    eprintln!(
        "sweep {tag}: levels: duplicates {}; owner's, far end silent {}; owner's, far end active {}",
        quantiles(&lv(&|i| dup[i])),
        quantiles(&lv(&|i| own[i] && !far_on(i))),
        quantiles(&lv(&|i| own[i] && far_on(i)))
    );
    // The correlation look-back and the fixed bands, on these words.
    if opts.correlation {
        let spans = spans_of(words);
        for before_ms in [350i64, 450, 550] {
            let pad = GatePad {
                before: before_ms * 16,
                after: GATE_AFTER,
            };
            let sc = score_sources(&spans, &m.mic[..], &m.lb[..], &m.profile, pad);
            let cd: Vec<f32> = (0..sc.len())
                .filter(|&i| dup[i])
                .filter_map(|i| sc[i].coefficient())
                .collect();
            let co: Vec<f32> = (0..sc.len())
                .filter(|&i| own[i])
                .filter_map(|i| sc[i].coefficient())
                .collect();
            eprintln!(
                "sweep {tag}: look-back {before_ms} ms: duplicates {}; owner's {}",
                quantiles(&cd),
                quantiles(&co)
            );
            let mut bands = vec![opts.fixed];
            for keep_under in [0.2f32, 0.3, 0.4, 0.5] {
                for threshold in [0.5f32, 0.6, 0.7] {
                    for drop_at in [0.6f32, 0.7, 0.8, 0.9] {
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
                let d = decide_in_order(&spans, &sc, b);
                eprintln!(
                    "sweep {tag}: look-back {before_ms} band {:.2}/{:.2}/{:.2}: correlation alone drops duplicates {} of {n_dup} ({:.2} %), owner's {} of {n_own}",
                    b.keep_under,
                    b.threshold,
                    b.drop_at,
                    (0..d.len()).filter(|&i| d[i] && dup[i]).count(),
                    pct((0..d.len()).filter(|&i| d[i] && dup[i]).count(), n_dup),
                    (0..d.len()).filter(|&i| d[i] && own[i]).count()
                );
            }
        }
    }
}

/// The live path (module docs).
fn live_path(m: &Meeting) -> Option<Row> {
    let stand_in = m.live.is_empty();
    let revs: Vec<Rev> = if stand_in {
        let raw = m.raw_words.as_ref()?;
        let mut v = Vec::new();
        for (ch, words) in [(Channel::Mic, raw), (Channel::Loopback, &m.lb_words)] {
            let mut utt = 0u32;
            let mut cur: Vec<LiveWord> = Vec::new();
            for w in words {
                if cur.last().is_some_and(|l| w.start - l.end_ms > 800) || cur.len() >= 40 {
                    v.push(Rev {
                        channel: ch,
                        utt,
                        rev: 0,
                        words: std::mem::take(&mut cur),
                    });
                    utt += 1;
                }
                cur.push(LiveWord {
                    start_ms: w.start,
                    end_ms: w.end,
                    text: w.text.clone(),
                    label: "me".into(),
                    speaker_id: None,
                });
            }
            if !cur.is_empty() {
                v.push(Rev {
                    channel: ch,
                    utt,
                    rev: 0,
                    words: cur,
                });
            }
        }
        v.sort_by_key(|r| r.words.iter().map(|w| w.end_ms).max().unwrap_or(0));
        v
    } else {
        m.live
            .iter()
            .map(|r| Rev {
                channel: r.channel,
                utt: r.utt,
                rev: r.rev,
                words: r.words.clone(),
            })
            .collect()
    };
    let shared = m.clocks.as_ref().map(|c| {
        (
            Arc::new(Mutex::new(c.mic.clone())),
            Arc::new(Mutex::new(c.loopback.clone())),
        )
    });
    let monitor = Arc::new(Mutex::new(EchoMonitor::new(COUPLED_AT, shared)));
    let mut gate = LiveEcho::new(
        Arc::clone(&monitor),
        EchoMode::Auto,
        0.5,
        Arc::new(AtomicBool::new(false)),
        Arc::default(),
    );
    let frame = |ch, k: usize, pcm: &[i16]| Frame {
        channel: ch,
        start_sample: (k * 1_600) as u64,
        pts_us: (k * 100_000) as u64,
        pcm: pcm.to_vec(),
        level: Level::default(),
        gain_db: 0.0,
        raw_peak_dbfs: -100.0,
    };
    let frames = m.mic.len().min(m.lb.len()) / 1_600;
    let mut k = 0usize;
    let mut resent = 0usize;
    let t = Instant::now();
    let mut push_until = |until_ms: i64, gate: &mut LiveEcho, resent: &mut usize| {
        while k < frames && (k as i64) * 100 < until_ms {
            for (ch, src) in [(Channel::Loopback, &m.lb), (Channel::Mic, &m.mic)] {
                let change =
                    monitor
                        .lock()
                        .unwrap()
                        .push(&frame(ch, k, &src[k * 1_600..(k + 1) * 1_600]));
                if let Some(s) = change {
                    let (_, u) = gate.status(s);
                    *resent += u.len();
                }
                if ch == Channel::Mic {
                    *resent += gate.refresh().len();
                }
            }
            k += 1;
        }
    };
    let mut arrival = 0i64;
    let mut latest: BTreeMap<u32, Vec<LiveWord>> = BTreeMap::new();
    let mut lb_latest: BTreeMap<u32, Vec<LiveWord>> = BTreeMap::new();
    for r in &revs {
        let end = r.words.iter().map(|w| w.end_ms).max().unwrap_or(arrival);
        let lat = if r.channel == Channel::Mic { 150 } else { 120 };
        arrival = arrival.max(end + lat);
        push_until(arrival, &mut gate, &mut resent);
        if r.channel == Channel::Mic {
            let (_, u) = gate.revision(r.utt, r.rev, r.words.clone());
            resent += u.len();
            latest.insert(r.utt, r.words.clone());
        } else {
            resent += gate.loopback(r.utt, r.rev, &r.words).len();
            lb_latest.insert(r.utt, r.words.clone());
        }
    }
    push_until(i64::MAX, &mut gate, &mut resent);
    let view = gate.shown_flags();
    let fin = gate.finish(0);
    let replay_s = t.elapsed().as_secs_f64();
    // The live words in time order, with Stop's and the view's decisions.
    let mut items: Vec<(W, bool, bool, u32)> = Vec::new();
    for (utt, words) in &latest {
        let (rev, flags) = view.get(utt).cloned().unwrap_or((0, Vec::new()));
        for (i, w) in words.iter().enumerate() {
            items.push((
                W::new(w.start_ms, w.end_ms, &w.text),
                fin.skip.contains(&(*utt, rev, i)),
                flags.get(i).copied().unwrap_or(false),
                *utt,
            ));
        }
    }
    items.sort_by_key(|x| x.0.start);
    {
        // What Stop removes and the view at the end shows, by Stop's reason.
        let mut by: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        let reason_of: HashMap<(i64, i64, &str), &str> = fin
            .dropped
            .iter()
            .map(|d| {
                (
                    (d.start_ms, d.end_ms, d.text.as_str()),
                    d.reason.map_or("", |r| r.as_str()),
                )
            })
            .collect();
        for x in &items {
            if x.1 {
                let r = reason_of
                    .get(&(x.0.start, x.0.end, x.0.text.as_str()))
                    .copied()
                    .unwrap_or("?");
                let e = by.entry(r).or_default();
                e.0 += 1;
                e.1 += usize::from(!x.2);
            }
        }
        eprintln!(
            "{}: Stop's drops by reason (of them shown at the end): {by:?}",
            m.uid
        );
    }
    let words: Vec<W> = items.iter().map(|x| x.0.clone()).collect();
    let stop: Vec<bool> = items.iter().map(|x| x.1).collect();
    let shown: Vec<bool> = items.iter().map(|x| x.2).collect();
    let mut lbw: Vec<W> = lb_latest
        .values()
        .flatten()
        .map(|w| W::new(w.start_ms, w.end_ms, &w.text))
        .collect();
    lbw.sort_by_key(|w| w.start);
    spread_check(&format!("{} live", m.uid), &words, &lbw);
    {
        // The owner's level of Stop's decision (before the canceller: as
        // today), from the live words' levels on the raw mic.
        let spans = spans_of(&words);
        let pres = presence_sources(&spans, &m.mic[..], None, &m.lb[..], m.clocks.clone());
        let gw: Vec<GateWord> = (0..words.len())
            .map(|i| GateWord {
                start_ms: words[i].start,
                end_ms: words[i].end,
                norm: words[i].norm.clone(),
                group: 0,
                score: WordScore::Undefined,
                presence: pres[i],
            })
            .collect();
        let far = FarIndex::new(lbw.iter().map(|w| (w.start, w.norm.clone())));
        owner_line(
            &format!("{} live Stop", m.uid),
            &gw,
            &far,
            TextWindow::DEFAULT,
            fin.info["floor_db"].as_f64().map(|v| v as f32),
            false,
        );
    }
    let info = &fin.info;
    let counts = LayerCounts {
        text: info["layers"]["text"].as_u64().unwrap_or(0) as usize,
        attenuation: 0,
        level: info["layers"]["level"].as_u64().unwrap_or(0) as usize,
        correlation: info["layers"]["correlation"].as_u64().unwrap_or(0) as usize,
        utterance: info["layers"]["utterance"].as_u64().unwrap_or(0) as usize,
        transfer: 0,
    };
    let band = info["band"].as_array().map(|b| GateBand {
        keep_under: b[0].as_f64().unwrap_or(0.0) as f32,
        threshold: b[1].as_f64().unwrap_or(0.0) as f32,
        drop_at: b[2].as_f64().unwrap_or(0.0) as f32,
    });
    let corr = info["correlation"].as_bool().unwrap_or(false);
    let mut row = Row {
        path: if stand_in { "live (stand-in)" } else { "live" }.into(),
        counts,
        band: band.filter(|_| corr),
        calibrated: info["band_calibrated"].as_bool().unwrap_or(false),
        floor: info["floor_db"].as_f64().map(|v| v as f32),
        owner: info["owner_db"].as_f64().map(|v| v as f32),
        text3: info["text_matched"].as_u64().unwrap_or(0) as usize,
        ..Row::default()
    };
    // Meeting 12's stand-in words are the labelled transcript itself.
    let lab = m
        .labels
        .as_ref()
        .filter(|_| stand_in)
        .map(|l| label_owners(l));
    score_row(&mut row, &words, &stop, &lbw, lab.as_deref());
    let mut view_row = Row {
        path: "live view".into(),
        counts: LayerCounts::default(),
        ..Row::default()
    };
    score_row(&mut view_row, &words, &shown, &lbw, None);
    eprintln!(
        "{}: live replay {:.1} s, {} revisions, {} re-sent utterances; status {:?} coupled at {:?} ms; the view at the end hides {} words, Stop {} ({} differ); view: kept {} with {} duplicates ({:.2} %), owner's {} of {}",
        m.uid,
        replay_s,
        revs.len(),
        resent,
        fin.state.status,
        info["coupled_at_ms"],
        shown.iter().filter(|s| **s).count(),
        stop.iter().filter(|s| **s).count(),
        stop.iter().zip(&shown).filter(|p| p.0 != p.1).count(),
        view_row.kept,
        view_row.dups_kept,
        pct(view_row.dups_kept, view_row.kept),
        view_row.own_kept,
        view_row.own
    );
    if flag("DIARIST_ECHO_SWEEP") || env_path("DIARIST_ECHO_DUMP").is_some() {
        // The live words scored offline, for the look-back and band sweeps.
        let spans = spans_of(&words);
        let p = &m.profile;
        let correlation = p.gates(EchoMode::Auto, false);
        let scores = if correlation {
            score_sources(&spans, &m.mic[..], &m.lb[..], p, GatePad::DEFAULT)
        } else {
            vec![WordScore::Undefined; spans.len()]
        };
        let pres = presence_sources(&spans, &m.mic[..], None, &m.lb[..], m.clocks.clone());
        let floor = floor_sources(&m.mic[..], &m.lb[..], m.clocks.clone());
        let gw: Vec<GateWord> = (0..words.len())
            .map(|i| GateWord {
                start_ms: words[i].start,
                end_ms: words[i].end,
                norm: words[i].norm.clone(),
                group: 0,
                score: scores[i],
                presence: pres[i],
            })
            .collect();
        let far = FarIndex::new(lbw.iter().map(|w| (w.start, w.norm.clone())));
        let reasons: Vec<Option<EchoReason>> = stop
            .iter()
            .map(|d| d.then_some(EchoReason::Utterance))
            .collect();
        dump(
            &format!("{}.live", m.uid),
            &words,
            &gw,
            &reasons,
            &lbw,
            None,
        );
        if !flag("DIARIST_ECHO_SWEEP") {
            return Some(row);
        }
        // The raw band (no canceller) under the whole layered decision, on
        // the live words scored offline.
        if correlation {
            // The live utterances of the words, for the utterance rule.
            let groups: Vec<u64> = items.iter().map(|x| u64::from(x.3)).collect();
            let gw2: Vec<GateWord> = gw
                .iter()
                .zip(&groups)
                .map(|(w, g)| GateWord {
                    group: *g,
                    ..w.clone()
                })
                .collect();
            let opts = LayerOptions {
                correlation,
                canceller: false,
                fixed: GateBand::default(),
                threshold: 0.5,
                window: TextWindow::DEFAULT,
                calibrate: true,
            };
            let lab = m
                .labels
                .as_ref()
                .filter(|_| stand_in)
                .map(|l| label_owners(l));
            for band in [
                None,
                Some((0.2f32, 0.5f32, 0.6f32)),
                Some((0.2, 0.5, 0.7)),
                Some((0.3, 0.5, 0.7)),
                Some((0.3, 0.6, 0.8)),
                Some((0.4, 0.6, 0.8)),
                Some((0.5, 0.7, 0.9)),
            ] {
                let r = layers::gate_with(&gw2, &far, floor.floor_db(), &opts, &|c| {
                    if let Some((k, t, d)) = band {
                        c.band = GateBand {
                            keep_under: k,
                            threshold: t,
                            drop_at: d,
                        };
                    }
                });
                let d: Vec<bool> = r.reasons.iter().map(Option::is_some).collect();
                let mut v = Row::default();
                score_row(&mut v, &words, &d, &lbw, lab.as_deref());
                eprintln!(
                    "raw band {}: {}: dropped {} {:?}; kept {}, duplicates {} ({:.2} %), owner-class {} of {} ({:.2} %){}",
                    m.uid,
                    band.map_or(format!("as built ({:.2}/{:.2}/{:.2})", r.calibration.band.keep_under, r.calibration.band.threshold, r.calibration.band.drop_at), |b| format!("{:.2}/{:.2}/{:.2}", b.0, b.1, b.2)),
                    r.dropped(),
                    r.counts,
                    v.kept,
                    v.dups_kept,
                    pct(v.dups_kept, v.kept),
                    v.own_kept,
                    v.own,
                    pct(v.own_kept, v.own),
                    v.labelled.map_or(String::new(), |(k, f, n)| format!("; labels {k} ({:.2} %), fair {f} ({:.2} %) of {n}", pct(k, n), pct(f, n)))
                );
            }
        }
        let mm = Meeting {
            lb_words: lbw.clone(),
            ..clone_meeting(m)
        };
        sweep(
            &mm,
            "live",
            &words,
            &gw,
            &far,
            &floor,
            &LayerOptions {
                correlation,
                canceller: false,
                fixed: GateBand::default(),
                threshold: 0.5,
                window: TextWindow::DEFAULT,
                calibrate: true,
            },
            None,
        );
    }
    Some(row)
}

fn clone_meeting(m: &Meeting) -> Meeting {
    Meeting {
        uid: m.uid.clone(),
        folder: m.folder.clone(),
        mic: m.mic.clone(),
        lb: m.lb.clone(),
        clocks: m.clocks.clone(),
        profile: m.profile.clone(),
        lb_words: m.lb_words.clone(),
        raw_words: None,
        labels: None,
        live: Vec::new(),
    }
}

/// The live monitor over a whole recording (both channels in 100 ms
/// frames): status changes with their time, the final state.
fn replay_monitor(
    mic: &[i16],
    lb: &[i16],
    clocks: Option<&ClockPair>,
) -> (Vec<(f64, EchoStatus)>, String) {
    let shared = clocks.map(|c| {
        (
            Arc::new(Mutex::new(c.mic.clone())),
            Arc::new(Mutex::new(c.loopback.clone())),
        )
    });
    let mut mon = EchoMonitor::new(COUPLED_AT, shared);
    let frame = |ch, k: usize, pcm: &[i16]| Frame {
        channel: ch,
        start_sample: (k * 1_600) as u64,
        pts_us: (k * 100_000) as u64,
        pcm: pcm.to_vec(),
        level: Level::default(),
        gain_db: 0.0,
        raw_peak_dbfs: -100.0,
    };
    let mut changes = Vec::new();
    let n = mic.len().min(lb.len()) / 1_600;
    for k in 0..n {
        for (ch, src) in [(Channel::Loopback, lb), (Channel::Mic, mic)] {
            if let Some(s) = mon.push(&frame(ch, k, &src[k * 1_600..(k + 1) * 1_600])) {
                changes.push((k as f64 / 10.0, s.status));
            }
        }
    }
    let (blocks, active, skipped) = mon.counts();
    (
        changes,
        format!(
            "{:?}, {blocks} blocks ({active} active, {skipped} skipped), coupled at {:?} ms, floor {:?} dB",
            mon.state(),
            mon.coupled_at_ms(),
            mon.floor_db()
        ),
    )
}

fn load(folder: &Path, db: Option<&Db>, label_csv: Option<&Path>) -> Meeting {
    let uid = folder.file_name().unwrap().to_string_lossy().into_owned();
    let (pcm, ch) = decode_i16(&folder.join("playback.flac")).unwrap();
    assert_eq!(ch, 2);
    let mic: Vec<i16> = pcm.iter().step_by(2).copied().collect();
    let lb: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
    drop(pcm);
    let id = db.and_then(|d| d.meeting(&uid));
    let clocks = id.and_then(|i| db.and_then(|d| d.clocks(i)));
    let profile = EchoProfile::scan_sources(&mic[..], &lb[..], clocks.clone());
    let lb_words = words_json(
        &folder.join("loopback.words.json"),
        clocks.as_ref().map(|c| &c.loopback),
    );
    let raw_path = folder.join("mic.words.json");
    let raw_words = raw_path
        .is_file()
        .then(|| words_json(&raw_path, clocks.as_ref().map(|c| &c.mic)));
    let labels = label_csv
        .map(labels)
        .filter(|l| raw_words.as_ref().is_some_and(|r| r.len() == l.len()));
    let live = id.map(|i| db.unwrap().live(i)).unwrap_or_default();
    Meeting {
        uid,
        folder: folder.to_path_buf(),
        mic,
        lb,
        clocks,
        profile,
        lb_words,
        raw_words,
        labels,
        live,
    }
}

#[test]
fn echo_on_stored_meetings() {
    let folders: Vec<PathBuf> = match std::env::var("DIARIST_ECHO_MEETINGS")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(v) => v
            .split(';')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect(),
        None => match env_path("DIARIST_ECHO_SAMPLE") {
            Some(p) => vec![p],
            None => {
                eprintln!("DIARIST_ECHO_MEETINGS is not set: skipped");
                return;
            }
        },
    };
    let db = env_path("DIARIST_ECHO_DB").map(|p| Db::open(&p));
    let csv = env_path("DIARIST_ECHO_CLASSES");
    let out = env_path("DIARIST_ECHO_OUT")
        .unwrap_or_else(|| std::env::temp_dir().join("diarist-echo-sample"));
    std::fs::create_dir_all(&out).unwrap();
    let engine = engine();
    let mut rows: Vec<(String, Row)> = Vec::new();
    for folder in &folders {
        let t = Instant::now();
        let m = load(folder, db.as_ref(), csv.as_deref());
        if let (Some(dir), Some(raw)) = (env_path("DIARIST_ECHO_CLASSES_OUT"), &m.raw_words) {
            if m.labels.is_none() && m.live.is_empty() && !raw.is_empty() {
                std::fs::create_dir_all(&dir).unwrap();
                write_classes(
                    &dir.join(format!("{}.classes.csv", m.uid)),
                    raw,
                    &m.lb_words,
                );
            }
        }
        let p = &m.profile;
        eprintln!(
            "== {}: {:.1} min, {} ({:.1} s to load and scan); live revisions {}, labels {}",
            m.uid,
            m.mic.len() as f64 / 960_000.0,
            if m.clocks.is_some() {
                "meeting time"
            } else {
                "raw channel indices"
            },
            t.elapsed().as_secs_f64(),
            m.live.len(),
            m.labels.is_some()
        );
        eprintln!(
            "profile: {:?} coupled {} coupling {:.3} lag {:?} ms erl {:?} dB, {} blocks ({} active), coupled at {:?} ms, {} relocks; active blocks {}",
            p.status, p.coupled, p.coupling, p.lag_ms, p.erl_db, p.blocks, p.active_blocks, p.coupled_at_ms, p.relocks,
            quantiles(&p.active)
        );
        let shift = 7 * 16_000;
        if m.mic.len() > shift {
            let pn = EchoProfile::scan_sources(
                &m.mic[shift..],
                &m.lb[..m.lb.len() - shift],
                m.clocks.clone(),
            );
            eprintln!(
                "null (mic 7 s later), offline: {:?} coupled {} coupling {:.3} lag {:?} ms; active blocks {}",
                pn.status,
                pn.coupled,
                pn.coupling,
                pn.lag_ms,
                quantiles(&pn.active)
            );
            assert!(!pn.coupled, "the null coupled offline");
            if flag("DIARIST_ECHO_NULL") {
                let (nc, nend) = replay_monitor(
                    &m.mic[shift..],
                    &m.lb[..m.lb.len() - shift],
                    m.clocks.as_ref(),
                );
                eprintln!("null, live monitor: {nend}; changes {nc:?}");
                assert!(
                    nc.iter().all(|c| c.1 != EchoStatus::Coupled),
                    "the null coupled"
                );
            }
        }
        if m.raw_words.as_ref().is_some_and(|w| w.is_empty()) && m.live.is_empty() {
            // A silent mic: the monitor alone.
            let (c, end) = replay_monitor(&m.mic, &m.lb, m.clocks.as_ref());
            eprintln!("monitor: {end}; changes {c:?}");
        }
        if let Some(r) = post_pass_path(&m, &out, engine.as_ref()) {
            rows.push((m.uid.clone(), r));
        }
        if let Some(r) = live_path(&m) {
            rows.push((m.uid.clone(), r));
        }
    }
    eprintln!();
    eprintln!("| Meeting | Path | Mic words | Text | Attenuation | Level | Correlation | Utterance | Kept | Residual duplicates | Chance | Owner's words kept | Band | Floor / owner (dBFS) | Text-matched | Owner source | Far-active cut kept |");
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (uid, r) in &rows {
        print_row(uid, r);
    }
    if !flag("DIARIST_ECHO_KEEP") {
        for f in &folders {
            let uid = f.file_name().unwrap().to_string_lossy().into_owned();
            let _ = std::fs::remove_file(out.join(format!("{uid}.aec-words.json")));
            let _ = std::fs::remove_file(out.join(format!("{uid}.raw-words.json")));
        }
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
