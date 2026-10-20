//! The echo monitor, profile and word gate on a stored live meeting (plan 15
//! section 4A). Gated on `DIARIST_ECHO_SAMPLE` (a meeting folder: its
//! `playback.flac`, left mic and right loopback on raw per-channel indices,
//! and the runtime's `mic.words.json` / `loopback.words.json` in the same
//! raw channel time); run with `--release`. Nothing is written anywhere.
//!
//! Env:
//! - `DIARIST_ECHO_SAMPLE`: the folder (read only).
//! - `DIARIST_ECHO_DB` (optional): a copy of the database the meeting came
//!   from, for its clock anchors (meeting time); without it the timeline is
//!   raw channel time and the lag is tracked through the drift.
//! - `DIARIST_ECHO_CLASSES` (optional): the measurement's per-word CSV
//!   (`i,start_s,end_s,text,norm,single_match,...,trigram_covered`, meeting
//!   time, the database's word order) labelling every mic word duplicate,
//!   misrecognised echo ("other") or the owner's; prints the per-class table.
//! - `DIARIST_ECHO_THRESHOLD` (default 0.35).
//! - `DIARIST_ECHO_WAV_DIR` (optional): a scratch folder; the channels are
//!   written there as `mic.wav` / `loopback.wav` (and removed after) to time
//!   the post-pass path, which reads WAVs.
//!
//! Prints the profile, the live monitor replayed over the whole recording
//! (and over a 7 s shifted mic, the null), the gate's counts and timings.
//!
//! `cargo test -p diarist-core --release --test echo_sample -- --nocapture`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use diarist_core::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use diarist_core::audio::echo::{
    gate_sources, rms_dbfs, ClockPair, EchoMonitor, EchoProfile, EchoStatus, HighPass, COUPLED_AT,
    GATE_THRESHOLD,
};
use diarist_core::audio::flac::decode_i16;
use diarist_core::audio::pipeline::{Frame, Level};
use diarist_core::types::{Channel, ClockAnchor, MeetingId};

struct Word {
    start: f64,
    end: f64,
    text: String,
}

fn words(path: &Path) -> Vec<Word> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut out: Vec<Word> = v["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| Word {
            start: w["start"].as_f64().unwrap(),
            end: w["end"].as_f64().unwrap(),
            text: w["word"].as_str().unwrap_or("").to_owned(),
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The measurement's normalisation: lower case, no punctuation.
fn norm(t: &str) -> String {
    t.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Splits one CSV line (quoted fields with doubled quotes).
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

/// Per mic word: duplicate (single match or inside a matched 3-run), and
/// the owner's (a non-duplicate whose ±5 neighbours are at most 30 percent
/// duplicates); the rest of the non-duplicates are misrecognised echo.
struct Classes {
    dup: Vec<bool>,
    own: Vec<bool>,
    norm: Vec<String>,
    start_s: Vec<f64>,
    end_s: Vec<f64>,
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
    let mut c = Classes {
        dup: Vec::new(),
        own: Vec::new(),
        norm: Vec::new(),
        start_s: Vec::new(),
        end_s: Vec::new(),
    };
    for l in lines.filter(|l| !l.is_empty()) {
        let f = csv_fields(l);
        c.dup.push(f[c1] == "1" || f[c3] == "1");
        c.norm.push(f[cn].clone());
        c.start_s.push(f[cs].parse().unwrap());
        c.end_s.push(f[ce].parse().unwrap());
    }
    let n = c.dup.len();
    c.own = (0..n)
        .map(|i| {
            if c.dup[i] {
                return false;
            }
            let nb: Vec<bool> = (i.saturating_sub(5)..i)
                .chain(i + 1..(i + 6).min(n))
                .map(|j| c.dup[j])
                .collect();
            !nb.is_empty() && nb.iter().filter(|d| **d).count() as f64 / nb.len() as f64 <= 0.3
        })
        .collect();
    c
}

fn anchors(db: &Path, uid: &str) -> Option<ClockPair> {
    let uri = format!(
        "file:{}?mode=ro",
        db.display().to_string().replace('\\', "/")
    );
    let c = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let id: i64 = c
        .query_row("SELECT id FROM meetings WHERE uid = ?1", [uid], |r| {
            r.get(0)
        })
        .ok()?;
    let mut st = c
        .prepare("SELECT channel, sample_index, qpc_100ns FROM clock_anchors WHERE meeting_id = ?1")
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

/// Replays both channels through the live monitor in 100 ms frames.
/// Returns (status changes with their meeting time, final state, seconds).
fn replay(
    mic: &[i16],
    lb: &[i16],
    clocks: Option<&ClockPair>,
) -> (Vec<(f64, EchoStatus)>, String, f64, u64) {
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
    let t0 = Instant::now();
    let n = mic.len().min(lb.len()) / 1_600;
    for k in 0..n {
        for (ch, src) in [(Channel::Loopback, lb), (Channel::Mic, mic)] {
            if let Some(s) = mon.push(&frame(ch, k, &src[k * 1_600..(k + 1) * 1_600])) {
                changes.push((k as f64 / 10.0, s.status));
            }
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let (blocks, active, skipped) = mon.counts();
    (
        changes,
        format!(
            "{:?}, {blocks} blocks ({active} active, {skipped} skipped), coupled at {:?} ms",
            mon.state(),
            mon.coupled_at_ms()
        ),
        secs,
        active,
    )
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        f64::NAN
    } else {
        100.0 * a as f64 / b as f64
    }
}

#[test]
fn echo_on_a_stored_meeting() {
    let Some(folder) = std::env::var_os("DIARIST_ECHO_SAMPLE").map(PathBuf::from) else {
        eprintln!("DIARIST_ECHO_SAMPLE is not set: skipped");
        return;
    };
    let threshold = std::env::var("DIARIST_ECHO_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(GATE_THRESHOLD);
    let uid = folder.file_name().unwrap().to_string_lossy().into_owned();
    let t = Instant::now();
    let (pcm, ch) = decode_i16(&folder.join("playback.flac")).unwrap();
    assert_eq!(ch, 2);
    let mic: Vec<i16> = pcm.iter().step_by(2).copied().collect();
    let lb: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
    drop(pcm);
    eprintln!(
        "{uid}: {:.1} min, decoded in {:.1} s",
        mic.len() as f64 / 960_000.0,
        t.elapsed().as_secs_f64()
    );
    let mic_words = words(&folder.join("mic.words.json"));
    let lb_words = words(&folder.join("loopback.words.json"));
    let clocks = std::env::var_os("DIARIST_ECHO_DB")
        .map(PathBuf::from)
        .and_then(|db| anchors(&db, &uid));
    eprintln!(
        "timeline: {}",
        if clocks.is_some() {
            "meeting time (clock anchors)"
        } else {
            "raw channel indices"
        }
    );

    // Offline profile.
    let t = Instant::now();
    let p = EchoProfile::scan_sources(&mic[..], &lb[..], clocks.clone());
    let scan_s = t.elapsed().as_secs_f64();
    eprintln!(
        "profile: {:?} coupled {} coupling {:.3} lag {:?} ms drift {:?} ms/min erl {:?} dB, {} blocks ({} active), coupled at {:?} ms; scan {scan_s:.2} s",
        p.status, p.coupled, p.coupling, p.lag_ms, p.drift_ms_per_min, p.erl_db, p.blocks, p.active_blocks, p.coupled_at_ms
    );

    // The live monitor over the whole recording, and over the null.
    let (changes, end, secs, active) = replay(&mic, &lb, clocks.as_ref());
    eprintln!(
        "monitor: {end}; changes {changes:?}; {secs:.2} s, {:.0} us per active block",
        secs * 1e6 / active.max(1) as f64
    );
    let shift = 7 * 16_000;
    if mic.len() > shift {
        let (nc, nend, _, _) = replay(&mic[shift..], &lb[..lb.len() - shift], clocks.as_ref());
        eprintln!("null (mic 7 s later): {nend}; changes {nc:?}");
        assert!(
            nc.iter().all(|c| c.1 != EchoStatus::Coupled),
            "the null coupled"
        );
    }

    // The gate: mic words on the timeline.
    let spans: Vec<(i64, i64)> = mic_words
        .iter()
        .map(|w| {
            (
                p.timeline_of_ms(Channel::Mic, (w.start * 1_000.0).round() as i64),
                p.timeline_of_ms(Channel::Mic, (w.end * 1_000.0).round() as i64),
            )
        })
        .collect();
    let t = Instant::now();
    let g = gate_sources(&spans, &mic[..], &lb[..], &p, threshold);
    let gate_s = t.elapsed().as_secs_f64();
    eprintln!(
        "gate at {threshold}: {} mic words, {} dropped, {} kept; {gate_s:.2} s ({:.0} us per word); would gate: {}",
        spans.len(),
        g.dropped,
        g.kept,
        gate_s * 1e6 / spans.len().max(1) as f64,
        p.gates(diarist_core::audio::echo::EchoMode::Auto, false)
    );

    // Duplicates by the matcher (same normalised text within ±0.8 s of a
    // loopback word, on the gate's timeline in seconds).
    let lb_t: Vec<(f64, String)> = lb_words
        .iter()
        .map(|w| {
            (
                p.timeline_of_ms(Channel::Loopback, (w.start * 1_000.0).round() as i64) as f64
                    / 16_000.0,
                norm(&w.text),
            )
        })
        .collect();
    let is_dup = |i: usize| -> bool {
        let t = spans[i].0 as f64 / 16_000.0;
        let n = norm(&mic_words[i].text);
        let k = lb_t.partition_point(|w| w.0 < t - 0.8);
        !n.is_empty()
            && lb_t[k..]
                .iter()
                .take_while(|w| w.0 <= t + 0.8)
                .any(|w| w.1 == n)
    };
    let dup_before = (0..spans.len()).filter(|&i| is_dup(i)).count();
    let kept: Vec<usize> = (0..spans.len()).filter(|&i| !g.drop[i]).collect();
    let dup_after = kept.iter().filter(|&&i| is_dup(i)).count();
    eprintln!(
        "matcher duplicates: before {dup_before} of {} ({:.1} %), after {dup_after} of {} ({:.1} %)",
        spans.len(),
        pct(dup_before, spans.len()),
        kept.len(),
        pct(dup_after, kept.len())
    );
    if spans.is_empty() {
        assert_eq!(g.dropped, 0);
    }
    if let Some(dir) = std::env::var_os("DIARIST_ECHO_WAV_DIR").map(PathBuf::from) {
        let (mw, lw) = (dir.join("mic.wav"), dir.join("loopback.wav"));
        for (path, x) in [(&mw, &mic), (&lw, &lb)] {
            let mut w = diarist_core::audio::wav::WavSink::create(path).unwrap();
            w.write(x).unwrap();
            w.finalize().unwrap();
        }
        let t = Instant::now();
        let pw = EchoProfile::scan(&mw, &lw, clocks.clone()).unwrap();
        let scan_wav = t.elapsed().as_secs_f64();
        let gw = diarist_core::audio::echo::gate_wavs(&spans, &mw, &lw, &pw, threshold).unwrap();
        let total = t.elapsed().as_secs_f64();
        let _ = (std::fs::remove_file(&mw), std::fs::remove_file(&lw));
        assert_eq!((pw.coupled, gw.dropped), (p.coupled, g.dropped));
        eprintln!(
            "from WAVs (the post-pass path): scan {scan_wav:.2} s, scan and gate {total:.2} s"
        );
    }

    // Per class, from the measurement's labels.
    if let Some(csv) = std::env::var_os("DIARIST_ECHO_CLASSES").map(PathBuf::from) {
        let c = classes(&csv);
        assert_eq!(c.dup.len(), mic_words.len(), "word counts differ");
        let mismatched = (0..c.norm.len())
            .filter(|&i| c.norm[i] != norm(&mic_words[i].text))
            .count();
        eprintln!(
            "labels: {} words, {mismatched} with another text at the same index",
            c.norm.len()
        );
        // Far-end activity: a 20 ms loopback frame (high-passed) over
        // −50 dBFS inside the word's span (meeting time; the loopback's raw
        // time is meeting time).
        let mut hp = HighPass::default();
        let mut lbf = Vec::with_capacity(lb.len());
        hp.extend(&lb, &mut lbf);
        let frame_db: Vec<f32> = lbf.chunks(320).map(rms_dbfs).collect();
        let during = |i: usize| -> bool {
            let a = (c.start_s[i] / 0.02).floor() as usize;
            let b = ((c.end_s[i] / 0.02).ceil() as usize).max(a + 1);
            frame_db[a.min(frame_db.len())..b.min(frame_db.len())]
                .iter()
                .any(|&d| d > -50.0)
        };
        let n = c.dup.len();
        let count = |f: &dyn Fn(usize) -> bool| (0..n).filter(|&i| f(i)).count();
        let dups = count(&|i| c.dup[i]);
        let dups_dropped = count(&|i| c.dup[i] && g.drop[i]);
        let other = count(&|i| !c.dup[i] && !c.own[i]);
        let other_dropped = count(&|i| !c.dup[i] && !c.own[i] && g.drop[i]);
        let own = count(&|i| c.own[i]);
        let own_kept = count(&|i| c.own[i] && !g.drop[i]);
        let own_far = count(&|i| c.own[i] && during(i));
        let own_far_kept = count(&|i| c.own[i] && during(i) && !g.drop[i]);
        let kept_n = count(&|i| !g.drop[i]);
        let kept_dups = count(&|i| c.dup[i] && !g.drop[i]);
        eprintln!("| Class | Words | Result |");
        eprintln!("|---|---|---|");
        eprintln!(
            "| Duplicates dropped | {dups} | {:.2} % |",
            pct(dups_dropped, dups)
        );
        eprintln!(
            "| Misrecognised echo dropped | {other} | {:.2} % |",
            pct(other_dropped, other)
        );
        eprintln!(
            "| Owner's words kept | {own} | {:.2} % |",
            pct(own_kept, own)
        );
        eprintln!(
            "| Owner's words kept during far-end speech | {own_far} | {:.2} % |",
            pct(own_far_kept, own_far)
        );
        eprintln!(
            "| Duplicates among kept words | {kept_n} kept | {:.2} % |",
            pct(kept_dups, kept_n)
        );
        // What the kept misrecognised echo looks like.
        let mut ko: Vec<f32> = (0..n)
            .filter(|&i| !c.dup[i] && !c.own[i] && !g.drop[i])
            .map(|i| g.scores[i].coefficient().unwrap_or(-1.0))
            .collect();
        ko.sort_by(f32::total_cmp);
        let q = |v: &[f32], f: f64| v.get(((v.len() as f64 - 1.0) * f) as usize).copied();
        let ko_far = count(&|i| !c.dup[i] && !c.own[i] && !g.drop[i] && during(i));
        let other_far = count(&|i| !c.dup[i] && !c.own[i] && during(i));
        eprintln!(
            "| Misrecognised echo during far-end speech dropped | {other_far} | {:.2} % |",
            pct(other_far - ko_far, other_far)
        );
        eprintln!(
            "kept other: {} (during far-end {ko_far}), coefficient p10 {:?} p50 {:?} p90 {:?}, undefined {}",
            ko.len(), q(&ko, 0.1), q(&ko, 0.5), q(&ko, 0.9), ko.iter().filter(|v| **v < 0.0).count()
        );
        let dropped_own: Vec<f32> = (0..n)
            .filter(|&i| c.own[i] && g.drop[i])
            .map(|i| g.scores[i].coefficient().unwrap_or(-1.0))
            .collect();
        eprintln!("dropped own coefficients: {dropped_own:?}");
    }
}
