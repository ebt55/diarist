//! The post-pass transfer gate (plan 15 section 9.2).
//!
//! After the canceller the residue of loud echo comes out of the speech
//! model as fragments the post-canceller layers miss (meeting 16: 21 of the
//! 293 kept words, garbled copies stamped up to 0.7 s late), while the
//! uncancelled mic, which the model heard cleanly, still carries the echo
//! evidence: its echo words repeat the far end in clean runs. So the
//! layered gate ([`super::layers`]) decides on the raw mic's words (the
//! live run's, else a transcription of `mic.wav`) with the raw band against
//! the raw signals, and the decision is transferred onto the cancelled
//! transcript by time ([`transfer`]):
//!
//! - a cancelled word whose span, widened by [`OVERLAP_MS`] on each side,
//!   overlaps a flagged raw word and no unflagged one is a candidate;
//! - a candidate goes when the guard lets it ([`Guard`]): (a) its
//!   normalised text equals, or is one edit from, an overlapping raw
//!   word's ([`Guard::Text`]), or (b) the canceller cut it by at least
//!   [`ATTENUATION_DB`] over its level window and its level after the
//!   canceller sits at least [`MARGIN_DB`] under the owner's level
//!   ([`Guard::Level`]; the owner's level of the cancelled words, as
//!   [`super::layers::CalibrationStats::owner_db`] defines it). The guards
//!   protect the owner's words that the canceller recovered from double
//!   talk, which overlap the raw transcript's echo words; (b) alone ships
//!   ([`GUARD`]: it also protects the owner's words the raw gate itself
//!   took for echo, which the canceller leaves untouched);
//! - a cancelled word with no raw word near it is kept: the canceller's
//!   recognition gain stays where it exists.
//!
//! The post-canceller layers still run on the cancelled words, and a word
//! goes when either removes it ([`gate`]; the post-canceller layer's reason
//! first, else [`EchoReason::Transfer`]).

use serde::Serialize;

use super::layers::{self, GateWord, LayerCounts, LayerOptions, LayerReport};
use super::text::FarIndex;
use crate::types::EchoReason;

/// A cancelled word and a raw word overlap when their spans, the cancelled
/// one widened by this on each side (ms), meet.
pub const OVERLAP_MS: i64 = 300;
/// Guard (b): the canceller removed at least this (dB) of the word's energy
/// over `[start − 300 ms, end + 80 ms]` ...
pub const ATTENUATION_DB: f32 = 3.0;
/// ... and its level after the canceller sits at least this far (dB) under
/// the owner's level.
pub const MARGIN_DB: f32 = 12.0;

/// What a candidate needs besides its raw counterpart's echo decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Guard {
    /// Nothing: every candidate goes.
    Plain,
    /// (a) its text equals, or is one edit from, an overlapping raw word's.
    Text,
    /// (b) cut by the canceller and well under the owner's level.
    Level,
    /// (a) or (b).
    Either,
}

impl Guard {
    pub const ALL: [Self; 4] = [Self::Plain, Self::Text, Self::Level, Self::Either];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Text => "text",
            Self::Level => "level",
            Self::Either => "either",
        }
    }
}

/// The shipped guard: the canceller's cut and the level alone
/// (docs/eval/2026-10-21-echo-round2.md, "Transfer gate"). The text guard
/// cannot tell echo from a mistake of the raw gate: when the raw gate takes
/// an owner's backchannel for echo (double talk), the cancelled transcript
/// has the same word, and the text guard confirms it (meeting 12: 2 of the
/// labelled owner's words; the spliced double-talk set 8.7 percent word
/// error rate with either guard against 8.1 with this one).
pub const GUARD: Guard = Guard::Level;

/// One raw-mic word with the raw gate's decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWord<'a> {
    /// Meeting time (ms).
    pub start_ms: i64,
    pub end_ms: i64,
    /// [`super::text::norm`] of its text.
    pub norm: &'a str,
    /// The raw gate took it for echo.
    pub echo: bool,
}

/// What the transfer did with the cancelled words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TransferCounts {
    /// Overlapping a flagged raw word and no unflagged one.
    pub candidates: usize,
    /// Removed by the text guard (whether or not the level guard agreed).
    pub text: usize,
    /// Removed by the attenuation and level guard alone.
    pub level: usize,
    /// Removed without a guard ([`Guard::Plain`]).
    pub plain: usize,
    /// Candidates the guard kept.
    pub guarded: usize,
    /// Cancelled words with no raw word near them (kept).
    pub unmatched: usize,
}

impl TransferCounts {
    pub fn removed(&self) -> usize {
        self.text + self.level + self.plain
    }
}

/// `a` and `b` are equal or one insertion, deletion or substitution apart.
pub fn within_one_edit(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (s, l) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if l.len() - s.len() > 1 {
        return false;
    }
    let i = s.iter().zip(&l).take_while(|(x, y)| x == y).count();
    if s.len() == l.len() {
        s[i + 1..] == l[i + 1..]
    } else {
        s[i..] == l[i + 1..]
    }
}

/// The transfer of the raw words' decisions (any order) onto the cancelled
/// words (any order; levels and attenuation after the canceller), with the
/// owner's level of the cancelled words: whether each cancelled word goes
/// (see the module docs).
pub fn transfer(
    cancelled: &[GateWord],
    raw: &[RawWord],
    owner_db: Option<f32>,
    guard: Guard,
) -> (Vec<bool>, TransferCounts) {
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by_key(|&j| (raw[j].start_ms, raw[j].end_ms));
    let starts: Vec<i64> = order.iter().map(|&j| raw[j].start_ms).collect();
    let longest = raw
        .iter()
        .map(|r| (r.end_ms - r.start_ms).max(0))
        .max()
        .unwrap_or(0);
    let mut counts = TransferCounts::default();
    let mut near: Vec<usize> = Vec::new();
    let out = cancelled
        .iter()
        .map(|c| {
            let (lo, hi) = (
                c.start_ms - OVERLAP_MS,
                c.end_ms.max(c.start_ms) + OVERLAP_MS,
            );
            let a = starts.partition_point(|&s| s < lo - longest);
            let b = starts.partition_point(|&s| s <= hi);
            near.clear();
            near.extend(
                order[a..b]
                    .iter()
                    .copied()
                    .filter(|&j| raw[j].end_ms.max(raw[j].start_ms) >= lo),
            );
            if near.is_empty() {
                counts.unmatched += 1;
                return false;
            }
            if !near.iter().all(|&j| raw[j].echo) {
                return false;
            }
            counts.candidates += 1;
            let text = !c.norm.is_empty()
                && near
                    .iter()
                    .any(|&j| !raw[j].norm.is_empty() && within_one_edit(&c.norm, raw[j].norm));
            let level = c.presence.is_some_and(|p| {
                p.attenuation_db.is_some_and(|a| a >= ATTENUATION_DB)
                    && owner_db.is_some_and(|o| p.level_db <= o - MARGIN_DB)
            });
            let slot = match guard {
                Guard::Plain => Some(&mut counts.plain),
                Guard::Text => text.then_some(&mut counts.text),
                Guard::Level => level.then_some(&mut counts.level),
                Guard::Either if text => Some(&mut counts.text),
                Guard::Either => level.then_some(&mut counts.level),
            };
            match slot {
                Some(n) => {
                    *n += 1;
                    true
                }
                None => {
                    counts.guarded += 1;
                    false
                }
            }
        })
        .collect();
    (out, counts)
}

/// What [`gate`] decided over the cancelled words, in their input order.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferReport {
    pub reasons: Vec<Option<EchoReason>>,
    /// Of `reasons` (the transfer counts words no post-canceller layer took).
    pub counts: LayerCounts,
    /// The layered gate on the raw words (in their input order).
    pub raw: LayerReport,
    /// The post-canceller layers on the cancelled words.
    pub after: LayerReport,
    pub transfer: TransferCounts,
}

impl TransferReport {
    pub fn dropped(&self) -> usize {
        self.counts.total()
    }
}

/// The post-pass decision after the canceller: the layered gate on the raw
/// words (`raw_opts`: the raw band, no canceller; levels on the raw mic,
/// `raw_floor` its floor), transferred onto the cancelled words with
/// `guard`, and the post-canceller layers (`opts`, `floor`) on the
/// cancelled words; both against the loopback's words `far`.
#[allow(clippy::too_many_arguments)]
pub fn gate(
    raw: &[GateWord],
    raw_floor: Option<f32>,
    raw_opts: &LayerOptions,
    cancelled: &[GateWord],
    floor: Option<f32>,
    opts: &LayerOptions,
    far: &FarIndex,
    guard: Guard,
) -> TransferReport {
    let raw_report = layers::gate(raw, far, raw_floor, raw_opts);
    let after = layers::gate(cancelled, far, floor, opts);
    let raw_words: Vec<RawWord> = raw
        .iter()
        .zip(&raw_report.reasons)
        .map(|(w, r)| RawWord {
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            norm: &w.norm,
            echo: r.is_some(),
        })
        .collect();
    let (moved, counts) = transfer(cancelled, &raw_words, after.owner_db, guard);
    let reasons: Vec<Option<EchoReason>> = after
        .reasons
        .iter()
        .zip(&moved)
        .map(|(r, &m)| r.or(m.then_some(EchoReason::Transfer)))
        .collect();
    TransferReport {
        counts: LayerCounts::of(&reasons),
        reasons,
        raw: raw_report,
        after,
        transfer: counts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::echo::presence::Presence;
    use crate::audio::echo::text::{norm, TextWindow};
    use crate::audio::echo::{GateBand, WordScore};

    /// A cancelled word at `t` ms, 200 ms long, with its level and the
    /// canceller's attenuation.
    fn cw(t: i64, text: &str, level: f32, att: f32) -> GateWord {
        GateWord {
            start_ms: t,
            end_ms: t + 200,
            norm: norm(text),
            group: 0,
            score: WordScore::Undefined,
            presence: Some(Presence {
                level_db: level,
                far_active: true,
                attenuation_db: Some(att),
            }),
        }
    }

    fn rw(t: i64, text: &str, echo: bool) -> RawWord<'_> {
        RawWord {
            start_ms: t,
            end_ms: t + 200,
            norm: text,
            echo,
        }
    }

    const OWNER: Option<f32> = Some(-20.0);

    #[test]
    fn one_edit_apart() {
        assert!(within_one_edit("ship", "ship"));
        assert!(within_one_edit("ship", "shop"));
        assert!(within_one_edit("ship", "ships"));
        assert!(within_one_edit("ships", "hips"));
        assert!(!within_one_edit("ship", "shape"));
        assert!(!within_one_edit("ship", "hop"));
        assert!(within_one_edit("", "a") && !within_one_edit("", "ab"));
    }

    #[test]
    fn a_fragment_under_an_echo_word_goes_and_the_owner_stays() {
        let raw = [
            rw(1_000, "starlink", true),
            rw(3_000, "coverage", true),
            rw(3_300, "thanks", false),
            rw(9_000, "launch", true),
        ];
        let cancelled = [
            // The same word, garbled: text guard.
            cw(1_100, "starling", -45.0, 12.0),
            // Another text, cut and quiet: the level guard.
            cw(1_050, "the", -40.0, 8.0),
            // Next to an unflagged raw word: never a candidate.
            cw(3_100, "coverage", -45.0, 12.0),
            // One edit from the flagged word: the text guard, cut or not.
            cw(9_050, "lunch", -22.0, 0.4),
            // Over the same flagged word, but another text, neither cut nor
            // quiet: an owner's word the canceller recovered stays.
            cw(9_100, "please", -22.0, 0.4),
            // No raw word near it: kept.
            cw(20_000, "hello", -45.0, 12.0),
        ];
        let (d, c) = transfer(&cancelled, &raw, OWNER, Guard::Either);
        assert_eq!(d, [true, true, false, true, false, false]);
        assert_eq!(
            c,
            TransferCounts {
                candidates: 4,
                text: 2,
                level: 1,
                plain: 0,
                guarded: 1,
                unmatched: 1,
            }
        );
        let (d, _) = transfer(&cancelled, &raw, OWNER, Guard::Text);
        assert_eq!(d, [true, false, false, true, false, false]);
        let (d, _) = transfer(&cancelled, &raw, OWNER, Guard::Level);
        assert_eq!(d, [true, true, false, false, false, false]);
        let (d, c) = transfer(&cancelled, &raw, OWNER, Guard::Plain);
        assert_eq!(d, [true, true, false, true, true, false]);
        assert_eq!((c.plain, c.removed()), (4, 4));
        // Without the owner's level the level guard never fires.
        let (d, _) = transfer(&cancelled, &raw, None, Guard::Level);
        assert_eq!(d, [false; 6]);
    }

    #[test]
    fn overlap_is_the_cancelled_span_widened_by_300_ms() {
        let raw = [rw(1_000, "echo", true)];
        // Raw 1,000 to 1,200: a cancelled word ending at 700 reaches it
        // (700 + 300), one ending at 690 does not; one starting at 1,500
        // (1,500 − 300 = 1,200) does, at 1,510 not.
        let w = |t: i64| cw(t, "echo", -45.0, 12.0);
        let (d, c) = transfer(
            &[w(500), w(490), w(1_500), w(1_510)],
            &raw,
            OWNER,
            Guard::Plain,
        );
        assert_eq!(d, [true, false, true, false]);
        assert_eq!(c.unmatched, 2);
    }

    #[test]
    fn the_gate_keeps_the_post_canceller_reason_and_adds_the_transfer() {
        let text = [
            "we", "need", "more", "time", "to", "ship", "the", "next", "release",
        ];
        let far = FarIndex::new(
            text.iter()
                .enumerate()
                .map(|(k, s)| (1_000 + 250 * k as i64, norm(s))),
        );
        // The raw mic heard the echo cleanly: a run of nine (evidence).
        let raw: Vec<GateWord> = text
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let mut w = cw(1_350 + 250 * k as i64, s, -30.0, 0.0);
                w.presence = w.presence.map(|p| Presence {
                    attenuation_db: None,
                    ..p
                });
                w
            })
            .collect();
        // After the canceller: two fragments, one garbled, and the owner's
        // word after the far end stopped.
        let cancelled = vec![
            cw(1_610, "kneed", -45.0, 14.0),
            cw(3_100, "release", -46.0, 15.0),
            cw(6_000, "okay", -20.0, 0.1),
        ];
        let opts = |canceller: bool| LayerOptions {
            correlation: false,
            canceller,
            fixed: GateBand::DEFAULT,
            threshold: GateBand::DEFAULT.threshold,
            window: TextWindow::DEFAULT,
            calibrate: false,
        };
        let r = gate(
            &raw,
            Some(-60.0),
            &opts(false),
            &cancelled,
            Some(-60.0),
            &opts(true),
            &far,
            Guard::Either,
        );
        assert_eq!(r.raw.counts.text, 9);
        assert_eq!(r.after.dropped(), 0);
        assert_eq!(
            r.reasons,
            [Some(EchoReason::Transfer), Some(EchoReason::Transfer), None]
        );
        assert_eq!(
            (
                r.counts.transfer,
                r.transfer.text,
                r.transfer.unmatched,
                r.dropped()
            ),
            (2, 2, 1, 2)
        );
    }
}
