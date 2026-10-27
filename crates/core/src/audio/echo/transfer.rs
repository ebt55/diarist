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
//!   [`super::layers::CalibrationStats::owner`] defines it after the
//!   canceller: from the words it left alone, unknown when there are too
//!   few or they sit at the floor). With the owner's level unknown (b) is
//!   the floor rule ([`Rule::floor_rule`], plan 16 sections 12.2.2 and
//!   12.7): cut by at least [`ATTENUATION_DB`] and at most F over the floor
//!   of the cancelled mic. F is [`FLOOR_MARGIN_DB`] in general, and
//!   [`FLOOR_MARGIN_SILENT_DB`] in the silent tier, when the recording
//!   shows a silent owner twice over: the owner's level is
//!   [`OwnerSource::Implausible`] (20 or more words the canceller left
//!   alone, all at the floor, which a talking owner's words never are) and
//!   the raw gate took at least [`SILENT_SHARE`] of the raw mic's words for
//!   echo (meeting 21, a talk on speakers with the owner nearly silent:
//!   99.93 percent; no other meeting over 89; fixtures whose owner speaks
//!   one to ten sentences only over loud far-end speech 92.9 to 99.92,
//!   their owner's level known or too few to tell, never implausible). The
//!   guards protect
//!   the owner's words that the canceller recovered from double talk, which
//!   overlap the raw transcript's echo words; (b) alone ships ([`GUARD`]:
//!   it also protects the owner's words the raw gate itself took for echo,
//!   which the canceller leaves untouched);
//! - a cancelled word with no raw word near it is kept: the canceller's
//!   recognition gain stays where it exists.
//!
//! The post-canceller layers still run on the cancelled words, and a word
//! goes when either removes it ([`gate`]; the post-canceller layer's reason
//! first, else [`EchoReason::Transfer`]).

use serde::Serialize;

use super::layers::{self, Context, GateWord, LayerCounts, LayerOptions, LayerReport, OwnerSource};
use super::text::FarIndex;
use crate::types::EchoReason;

/// A cancelled word and a raw word overlap when their spans, the cancelled
/// one widened by this on each side (ms), meet.
pub const OVERLAP_MS: i64 = 300;
/// Guard (b): the canceller removed at least this (dB) of the word's energy
/// over `[start − 300 ms, end + 80 ms]` ...
pub const ATTENUATION_DB: f32 = 3.0;
/// ... and its level after the canceller sits at least this far (dB) under
/// the owner's level ...
pub const MARGIN_DB: f32 = 12.0;
/// ... or, the owner's level unknown, at most this far (dB) over the floor
/// of the cancelled mic (the floor rule's general tier, `F_u`;
/// `f32::INFINITY` would be the cut alone). The largest of 9, 12, 15 and
/// 18 at which the owner's word error rate with the level forced unknown
/// stays within 2 points of the known guard's, measured with the
/// post-canceller correlation layer off so that it cannot hide the rule's
/// cost (docs/eval/2026-10-26-echo-owner-level.md, section 3): on the
/// FLEURS fixture whose owner talks only over the far end 23 dB over the
/// floor 10.90 percent at 9 and 12 dB against 10.90 known, 14.22 at 15,
/// 27.96 at 18; at 28 dB over the floor 7.11 at 12 against 8.06. With a
/// known owner's level the guard sits 15 to 18 dB over the floor on the
/// canceller meetings 12, 14, 16, 17, 18 and 20.
pub const FLOOR_MARGIN_DB: f32 = 12.0;
/// ... or this far (dB) in the silent tier (`F_s`), when two independent
/// signatures of a silent owner agree: the owner's level is
/// [`OwnerSource::Implausible`] (enough words the canceller left alone, all
/// at the floor) and the raw gate took at least [`SILENT_SHARE`] of the
/// raw mic's words for echo. Meeting 21's residue sits 0 to 21 dB over the
/// floor; at 18 dB it keeps 77 words with 42 cut far-active ones (288 and
/// 253 before the fix, 155 and 120 at 12 dB).
pub const FLOOR_MARGIN_SILENT_DB: f32 = 18.0;
/// The raw gate's share of flagged words the silent tier needs: 99.8
/// percent. Meeting 21 reaches 99.93; plan 16 section 12.7 set 99.5 unless
/// a fixture with a talking owner reached it, and the FLEURS fixture whose
/// owner talks only over loud far-end speech 23 dB over the floor does
/// (99.65). The share alone does not tell a silent owner from one who
/// speaks a few sentences over a long talk (the same fixture with one to
/// three utterances in 20 minutes: 99.91 and 99.92); the owner's level
/// does, which is why the tier needs both.
pub const SILENT_SHARE: f64 = 0.998;

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

/// The transfer's parameters: [`Rule::SHIPPED`], or the evaluation's
/// variants ([`transfer_with`], [`gate_with`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub guard: Guard,
    /// [`OVERLAP_MS`].
    pub overlap_ms: i64,
    /// [`FLOOR_MARGIN_DB`] (`F_u`).
    pub floor_margin_db: f32,
    /// [`FLOOR_MARGIN_SILENT_DB`] (`F_s`).
    pub floor_margin_silent_db: f32,
    /// [`SILENT_SHARE`].
    pub silent_share: f64,
}

impl Rule {
    pub const SHIPPED: Self = Self {
        guard: GUARD,
        overlap_ms: OVERLAP_MS,
        floor_margin_db: FLOOR_MARGIN_DB,
        floor_margin_silent_db: FLOOR_MARGIN_SILENT_DB,
        silent_share: SILENT_SHARE,
    };

    /// The floor rule in force: `None` with the owner's level known (the
    /// level guard is the owner's), else its tier and margin. The silent
    /// tier needs the owner's level [`OwnerSource::Implausible`] and the raw
    /// gate's flagged share `raw_echo_share` at least [`Rule::silent_share`].
    pub fn floor_rule(
        &self,
        owner_db: Option<f32>,
        owner_source: OwnerSource,
        raw_echo_share: f64,
    ) -> Option<FloorRule> {
        if owner_db.is_some() {
            return None;
        }
        Some(
            if owner_source == OwnerSource::Implausible && raw_echo_share >= self.silent_share {
                FloorRule {
                    tier: FloorTier::Silent,
                    f_db: self.floor_margin_silent_db,
                }
            } else {
                FloorRule {
                    tier: FloorTier::Unknown,
                    f_db: self.floor_margin_db,
                }
            },
        )
    }
}

/// The floor rule's tier (`engine_info.echo.transfer.floor_rule.tier`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FloorTier {
    /// The owner's level unknown: [`Rule::floor_margin_db`].
    Unknown,
    /// Both signatures of a silent owner: [`Rule::floor_margin_silent_db`].
    Silent,
}

impl FloorTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Silent => "silent",
        }
    }
}

/// The floor rule in force ([`Rule::floor_rule`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorRule {
    pub tier: FloorTier,
    /// dB over the cancelled mic's floor.
    pub f_db: f32,
}

/// The share of `raw` words the raw gate took for echo (0 without words).
pub fn echo_share(raw: &[RawWord]) -> f64 {
    if raw.is_empty() {
        return 0.0;
    }
    raw.iter().filter(|r| r.echo).count() as f64 / raw.len() as f64
}

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
    /// Removed by the level guard's floor rule (the owner's level unknown;
    /// also `engine_info.echo.transfer.floor_rule.removed`).
    pub floor: usize,
}

impl TransferCounts {
    pub fn removed(&self) -> usize {
        self.text + self.level + self.plain + self.floor
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
/// owner's level of the cancelled words and the cancelled mic's floor:
/// whether each cancelled word goes (see the module docs). Without the
/// owner's level the floor rule's general tier applies ([`transfer_with`]
/// decides the tier).
pub fn transfer(
    cancelled: &[GateWord],
    raw: &[RawWord],
    owner_db: Option<f32>,
    floor_db: Option<f32>,
    guard: Guard,
) -> (Vec<bool>, TransferCounts) {
    transfer_with(
        cancelled,
        raw,
        owner_db,
        OwnerSource::TooFew,
        0.0,
        floor_db,
        &Rule {
            guard,
            ..Rule::SHIPPED
        },
    )
}

/// [`transfer`] with every parameter in `rule`, the owner's level's source
/// and the raw gate's flagged share (the floor rule's tier,
/// [`Rule::floor_rule`]).
pub fn transfer_with(
    cancelled: &[GateWord],
    raw: &[RawWord],
    owner_db: Option<f32>,
    owner_source: OwnerSource,
    raw_echo_share: f64,
    floor_db: Option<f32>,
    rule: &Rule,
) -> (Vec<bool>, TransferCounts) {
    let floor_margin = rule
        .floor_rule(owner_db, owner_source, raw_echo_share)
        .map(|f| f.f_db);
    let overlap = rule.overlap_ms;
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
            let (lo, hi) = (c.start_ms - overlap, c.end_ms.max(c.start_ms) + overlap);
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
            // Guard (b): under the owner's level, or without it the floor
            // rule.
            let level = c.presence.is_some_and(|p| {
                p.attenuation_db.is_some_and(|a| a >= ATTENUATION_DB)
                    && match (owner_db, floor_margin) {
                        (Some(o), _) => p.level_db <= o - MARGIN_DB,
                        (None, Some(m)) => {
                            m.is_infinite() || floor_db.is_some_and(|f| p.level_db <= f + m)
                        }
                        (None, None) => false,
                    }
            });
            let level_slot = if owner_db.is_some() {
                &mut counts.level
            } else {
                &mut counts.floor
            };
            let slot = match rule.guard {
                Guard::Plain => Some(&mut counts.plain),
                Guard::Text => text.then_some(&mut counts.text),
                Guard::Level => level.then_some(level_slot),
                Guard::Either if text => Some(&mut counts.text),
                Guard::Either => level.then_some(level_slot),
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
    /// The share of the raw words the raw gate took for echo.
    pub raw_echo_share: f64,
    /// The floor rule in force (`None`: the owner's level was known).
    pub floor_rule: Option<FloorRule>,
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
    gate_with(
        raw,
        raw_floor,
        raw_opts,
        cancelled,
        floor,
        opts,
        far,
        &Rule {
            guard,
            ..Rule::SHIPPED
        },
        &|_| {},
    )
}

/// [`gate`] with every transfer parameter in `rule` and the post-canceller
/// layers' parameters changed by `tweak` (the evaluation's sweeps; the
/// transfer uses the owner's level and floor the layers used, after it).
#[allow(clippy::too_many_arguments)]
pub fn gate_with(
    raw: &[GateWord],
    raw_floor: Option<f32>,
    raw_opts: &LayerOptions,
    cancelled: &[GateWord],
    floor: Option<f32>,
    opts: &LayerOptions,
    far: &FarIndex,
    rule: &Rule,
    tweak: &dyn Fn(&mut Context),
) -> TransferReport {
    let raw_report = layers::gate(raw, far, raw_floor, raw_opts);
    let after = layers::gate_with(cancelled, far, floor, opts, tweak);
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
    let raw_echo_share = echo_share(&raw_words);
    let (moved, counts) = transfer_with(
        cancelled,
        &raw_words,
        after.owner_db,
        after.owner_source,
        raw_echo_share,
        after.floor_db,
        rule,
    );
    let floor_rule = rule.floor_rule(after.owner_db, after.owner_source, raw_echo_share);
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
        raw_echo_share,
        floor_rule,
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
    const FLOOR: Option<f32> = Some(-60.0);

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
        let (d, c) = transfer(&cancelled, &raw, OWNER, FLOOR, Guard::Either);
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
                floor: 0,
            }
        );
        let (d, _) = transfer(&cancelled, &raw, OWNER, FLOOR, Guard::Text);
        assert_eq!(d, [true, false, false, true, false, false]);
        let (d, _) = transfer(&cancelled, &raw, OWNER, FLOOR, Guard::Level);
        assert_eq!(d, [true, true, false, false, false, false]);
        let (d, c) = transfer(&cancelled, &raw, OWNER, FLOOR, Guard::Plain);
        assert_eq!(d, [true, true, false, true, true, false]);
        assert_eq!((c.plain, c.removed()), (4, 4));
        // Without the owner's level and the floor the level guard never
        // fires.
        let (d, _) = transfer(&cancelled, &raw, None, None, Guard::Level);
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
            FLOOR,
            Guard::Plain,
        );
        assert_eq!(d, [true, false, true, false]);
        assert_eq!(c.unmatched, 2);
    }

    /// Five candidates over one flagged raw word, cut by `att` dB, `over`
    /// dB over the floor of −60 dBFS.
    fn floor_case(cases: &[(f32, f32)]) -> Vec<GateWord> {
        cases
            .iter()
            .enumerate()
            .map(|(k, &(over, att))| cw(1_000 + 10 * k as i64, "x", -60.0 + over, att))
            .collect()
    }

    fn rule(f: f32) -> Rule {
        Rule {
            floor_margin_db: f,
            ..Rule::SHIPPED
        }
    }

    /// The general tier: the owner's level unknown, no silent owner.
    fn general(
        cancelled: &[GateWord],
        raw: &[RawWord],
        floor: Option<f32>,
        rule: &Rule,
    ) -> (Vec<bool>, TransferCounts) {
        transfer_with(cancelled, raw, None, OwnerSource::TooFew, 1.0, floor, rule)
    }

    #[test]
    fn without_the_owners_level_the_floor_rule_decides() {
        let raw = [rw(1_000, "echo", true)];
        let cancelled = floor_case(&[
            // Residue: cut and near the floor.
            (5.0, 8.0),
            // Exactly at the margin: goes.
            (12.0, 8.0),
            // Over it: stays.
            (13.0, 8.0),
            // Near the floor but not cut: stays.
            (2.0, 1.0),
            // Loud and cut (the owner over the far end): stays.
            (25.0, 6.0),
        ]);
        let (d, c) = general(&cancelled, &raw, FLOOR, &rule(12.0));
        assert_eq!(d, [true, true, false, false, false]);
        assert_eq!(
            (c.candidates, c.floor, c.level, c.guarded, c.removed()),
            (5, 2, 0, 3, 2)
        );
        // Infinite: the cut alone, the floor not needed.
        let (d, c) = general(&cancelled, &raw, FLOOR, &rule(f32::INFINITY));
        assert_eq!(d, [true, true, true, false, true]);
        assert_eq!(c.floor, 4);
        let (d, _) = general(&cancelled, &raw, None, &rule(f32::INFINITY));
        assert_eq!(d, [true, true, true, false, true]);
        // A finite margin without the floor: nothing goes.
        let (d, _) = general(&cancelled, &raw, None, &rule(12.0));
        assert_eq!(d, [false; 5]);
        // The shipped margin.
        let (d, _) = transfer(&cancelled, &raw, None, FLOOR, Guard::Level);
        let want: Vec<bool> = [
            (5.0, 8.0),
            (12.0, 8.0),
            (13.0, 8.0),
            (2.0, 1.0),
            (25.0, 6.0),
        ]
        .iter()
        .map(|&(over, att)| att >= ATTENUATION_DB && over <= FLOOR_MARGIN_DB)
        .collect();
        assert_eq!(d, want);
    }

    #[test]
    fn with_the_owners_level_the_floor_rule_is_not_used() {
        let raw = [rw(1_000, "echo", true)];
        // The owner at −20 dBFS: 12 dB under it is −32, 28 dB over the floor.
        let cancelled = floor_case(&[(5.0, 8.0), (27.0, 8.0), (29.0, 8.0), (5.0, 1.0)]);
        for f in [9.0, 12.0, f32::INFINITY] {
            for source in [OwnerSource::Quiet, OwnerSource::Implausible] {
                let (d, c) = transfer_with(&cancelled, &raw, OWNER, source, 1.0, FLOOR, &rule(f));
                assert_eq!(d, [true, true, false, false], "{f}");
                assert_eq!((c.level, c.floor), (2, 0));
            }
        }
        assert_eq!(
            Rule::SHIPPED.floor_rule(OWNER, OwnerSource::Implausible, 1.0),
            None
        );
    }

    #[test]
    fn the_silent_tier_needs_an_implausible_owner_and_a_raw_mic_all_echo() {
        let r = Rule {
            floor_margin_db: 12.0,
            floor_margin_silent_db: 18.0,
            ..Rule::SHIPPED
        };
        let tier = |source: OwnerSource, share: f64| r.floor_rule(None, source, share);
        let silent = Some(FloorRule {
            tier: FloorTier::Silent,
            f_db: 18.0,
        });
        let unknown = Some(FloorRule {
            tier: FloorTier::Unknown,
            f_db: 12.0,
        });
        // Meeting 21: 7,606 of 7,611 raw words flagged.
        assert_eq!(tier(OwnerSource::Implausible, 7_606.0 / 7_611.0), silent);
        assert_eq!(tier(OwnerSource::Implausible, SILENT_SHARE), silent);
        assert_eq!(tier(OwnerSource::Implausible, 0.997_9), unknown);
        // The FLEURS fixture whose owner talks only over loud far-end
        // speech: 2,258 of 2,266 (99.65 percent), under the threshold.
        assert_eq!(tier(OwnerSource::Implausible, 2_258.0 / 2_266.0), unknown);
        // Too few words to tell, or a share of 89 percent (meeting 20): the
        // general tier.
        assert_eq!(tier(OwnerSource::TooFew, 1.0), unknown);
        assert_eq!(tier(OwnerSource::Implausible, 0.89), unknown);
        // Forced unknown with an estimate (the evaluation's sweeps).
        assert_eq!(tier(OwnerSource::Clean, 1.0), unknown);
        assert_eq!(SILENT_SHARE, 0.998);
        assert_eq!((FLOOR_MARGIN_DB, FLOOR_MARGIN_SILENT_DB), (12.0, 18.0));
        // The share of the raw gate's flags.
        let raw = [rw(0, "a", true), rw(300, "b", false), rw(600, "c", true)];
        assert!((echo_share(&raw) - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(echo_share(&[]), 0.0);
        // The tier moves the margin: a candidate 15 dB over the floor.
        let raw = [rw(1_000, "echo", true)];
        let cancelled = floor_case(&[(10.0, 8.0), (15.0, 8.0), (20.0, 8.0)]);
        let go = |source: OwnerSource, share: f64| {
            let (d, c) = transfer_with(&cancelled, &raw, None, source, share, FLOOR, &r);
            assert_eq!(c.floor, d.iter().filter(|x| **x).count());
            d
        };
        assert_eq!(go(OwnerSource::Implausible, 1.0), [true, true, false]);
        assert_eq!(go(OwnerSource::Implausible, 0.9), [true, false, false]);
        assert_eq!(go(OwnerSource::TooFew, 1.0), [true, false, false]);
    }

    /// The raw mic heard the far end cleanly, `n` words in one run (every
    /// one flagged by text), against loopback words 250 ms apart from
    /// 1,000 ms; the cancelled words are the caller's.
    fn echoed(n: usize) -> (FarIndex, Vec<GateWord>) {
        let text: Vec<String> = (0..n).map(|k| format!("w{k}")).collect();
        let far = FarIndex::new(
            text.iter()
                .enumerate()
                .map(|(k, s)| (1_000 + 250 * k as i64, norm(s))),
        );
        let raw = text
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
        (far, raw)
    }

    fn layer_opts(canceller: bool) -> LayerOptions {
        LayerOptions {
            correlation: false,
            canceller,
            fixed: GateBand::DEFAULT,
            threshold: GateBand::DEFAULT.threshold,
            window: TextWindow::DEFAULT,
            calibrate: false,
        }
    }

    #[test]
    fn the_gate_without_the_owners_level_applies_the_floor_rule_by_tier() {
        let r = Rule {
            floor_margin_db: 12.0,
            floor_margin_silent_db: 18.0,
            ..Rule::SHIPPED
        };
        let floor = Some(-60.0);
        let (far, raw) = echoed(40);
        // Under the raw words' span (1,350 to 11,300 ms): residue cut by
        // 8 dB at 10 and 15 dB over the floor, and the owner's word left
        // alone, loud.
        let residue = |t: i64, over: f32| cw(t, &format!("r{t}"), -60.0 + over, 8.0);
        let mut cancelled = vec![
            residue(2_000, 10.0),
            residue(4_000, 15.0),
            cw(6_000, "okay", -30.0, 0.5),
        ];
        let run = |cancelled: &[GateWord]| {
            gate_with(
                &raw,
                floor,
                &layer_opts(false),
                cancelled,
                floor,
                &layer_opts(true),
                &far,
                &r,
                &|_| {},
            )
        };
        // Too few words the canceller left alone: unknown, the general tier
        // (F_u, 12 dB), though the raw mic is all echo.
        let t = run(&cancelled);
        assert_eq!(t.raw.dropped(), 40);
        assert_eq!(t.raw_echo_share, 1.0);
        assert_eq!(
            (t.after.owner_source, t.after.owner_db),
            (OwnerSource::TooFew, None)
        );
        assert_eq!(
            t.floor_rule,
            Some(FloorRule {
                tier: FloorTier::Unknown,
                f_db: 12.0
            })
        );
        assert_eq!(t.reasons[..3], [Some(EchoReason::Transfer), None, None]);
        assert_eq!((t.transfer.floor, t.transfer.guarded), (1, 2));
        // Twenty-five words the canceller left alone at the floor after the
        // far end stopped (a silent owner's noise): implausible, and with
        // the raw mic all echo the silent tier (F_s, 18 dB) takes both
        // fragments; the owner's loud word stays.
        for k in 0..25 {
            let mut w = cw(20_000 + 400 * k, &format!("n{k}"), -59.0, 0.3);
            w.presence = w.presence.map(|p| Presence {
                far_active: false,
                ..p
            });
            cancelled.push(w);
        }
        // (Of the 26 words it left alone, "okay" among them, the median.)
        let t = run(&cancelled);
        assert_eq!(
            (t.after.owner_source, t.after.owner_words, t.after.owner_db),
            (OwnerSource::Implausible, 26, None)
        );
        assert_eq!(
            t.floor_rule,
            Some(FloorRule {
                tier: FloorTier::Silent,
                f_db: 18.0
            })
        );
        assert_eq!(
            t.reasons[..3],
            [Some(EchoReason::Transfer), Some(EchoReason::Transfer), None]
        );
        assert_eq!(t.transfer.floor, 2);
        // The noise words have no raw word near them: kept.
        assert!(t.reasons[3..].iter().all(Option::is_none));
        // One raw word in 40 the raw gate kept (97.5 percent): the general
        // tier again.
        let mut raw2 = raw.clone();
        raw2.push(cw(30_000, "thanks", -25.0, 0.0));
        let t = gate_with(
            &raw2,
            floor,
            &layer_opts(false),
            &cancelled,
            floor,
            &layer_opts(true),
            &far,
            &r,
            &|_| {},
        );
        assert!((t.raw_echo_share - 40.0 / 41.0).abs() < 1e-12);
        assert_eq!(t.floor_rule.map(|f| f.tier), Some(FloorTier::Unknown));
        assert_eq!(t.transfer.floor, 1);
        // With the owner's level known no floor rule is in force.
        let t = gate_with(
            &raw,
            floor,
            &layer_opts(false),
            &cancelled,
            floor,
            &layer_opts(true),
            &far,
            &r,
            &|c| c.owner_db = Some(-30.0),
        );
        assert_eq!((t.floor_rule, t.transfer.floor), (None, 0));
        assert_eq!(t.transfer.level, 2);
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
