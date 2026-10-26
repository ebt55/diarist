//! The layered echo decision (plan 15 section 9, revised in 9.1): one
//! function for the live view, the live run at Stop and the post-pass.
//!
//! Per mic word, in time order ([`decide`]), the first layer that fires
//! names the reason ([`EchoReason`]):
//!
//! 1. **Text** ([`super::text`]): the word is in a delay-consistent run of
//!    three or more loopback matches; in a run of two (with a content word)
//!    only with the acoustic tie-breaker: on a correlated recording the
//!    word's coefficient at or above the band's keep bound, otherwise its
//!    level within [`PRESENCE_DB`] of the noise floor.
//! 2. **Attenuation** (post-pass, after the canceller; off unless
//!    [`Context::attenuation`]): the canceller removed at least
//!    [`ATTENUATION_DB`] of the word's energy and the word is at least
//!    [`ATTENUATION_MARGIN_DB`] under the owner's level. Measured and not
//!    engaged: every threshold that cut meeting 12's residual duplicates
//!    under 1 percent took the owner's own words spoken over continuous
//!    echo (the spliced double-talk set from 7.6 to 12.1 percent WER and
//!    more, docs/eval/2026-10-21-echo-round2.md).
//! 3. **Level** (not after the canceller, [`LEVEL_AFTER_CANCELLER`]): the
//!    far end is active and the word's level is within [`PRESENCE_DB`] of the
//!    floor, and not within [`PRESENCE_DB`] of the owner's level (never near
//!    the owner). After the canceller its residual suppression lowers the
//!    owner's own frames under the far end (the spliced set lost 22 words).
//! 4. **Correlation** (only where the recording correlates: coupled, a
//!    canceller ran, or `echo.mode = always`): the hysteresis band
//!    ([`super::decide`]), self-calibrated from the text matches
//!    ([`CalibrationStats::band`]) on the raw mic's transcript, fixed after
//!    the canceller.
//! 5. **Utterance**: in an engine utterance whose words are at least
//!    [`UTTERANCE_SHARE`] flagged, an unflagged pocket of at most
//!    [`POCKET_WORDS`] words next to a flagged word (within [`POCKET_GAP_MS`])
//!    while the far end is active, unless its level shows the owner.
//!
//! The owner's level ([`CalibrationStats::owner`], its source in
//! [`OwnerSource`]) is the median level of words in no text run, from at
//! least [`OWNER_MIN_WORDS`] of them:
//!
//! - before the canceller (the raw gate of the transfer, the live gate and
//!   Stop): of those spoken while the far end was silent, else of all words
//!   in no run, else unknown;
//! - after the canceller only of words it left alone (cut by under
//!   [`super::transfer::ATTENUATION_DB`]: its residue is cut, the owner's own
//!   words are not), the far end silent, else any, else unknown; and unknown
//!   when the estimate sits under the floor plus [`OWNER_PLAUSIBLE_DB`]. On
//!   meeting 21 (a talk on speakers, the owner nearly silent) the words in no
//!   run were the canceller's residue and read 11.2 dB over the floor, the
//!   owner sits 21 to 30 dB over it on meetings 13 and 15 to 20; the transfer
//!   gate then uses its floor rule (plan 16 section 12.2.2).
//!
//! After the post-pass canceller the decision also runs on the raw mic's
//! words and is carried over to the cancelled ones ([`super::transfer`],
//! reason [`EchoReason::Transfer`]).

use std::collections::BTreeMap;

use serde::Serialize;

use super::presence::{percentile, Presence};
use super::text::{runs, FarIndex, Runs, TextWindow};
use super::{GateBand, WordScore};
use crate::types::EchoReason;

/// The presence margin: a level within this of the floor is not near-end
/// speech; within this of the owner's level it is.
pub const PRESENCE_DB: f32 = 6.0;
/// The attenuation layer runs (module docs: measured, off).
pub const ATTENUATION_LAYER: bool = false;
/// When it runs: a word the canceller took this much energy from (dB) ...
pub const ATTENUATION_DB: f32 = 6.0;
/// ... at least this far under the owner's level (dB) ...
pub const ATTENUATION_MARGIN_DB: f32 = 10.0;
/// ... and, when set, correlating at least this much.
pub const ATTENUATION_MIN_COEFFICIENT: Option<f32> = None;
/// After the canceller the correlation layer removes only words the
/// canceller attenuated by at least this much (`None`: any).
pub const CORRELATION_MIN_ATTENUATION_DB: Option<f32> = None;
/// The level layer after the canceller (module docs: off).
pub const LEVEL_AFTER_CANCELLER: bool = false;
/// An utterance at least this much flagged is mostly echo ...
pub const UTTERANCE_SHARE: f64 = 0.6;
/// ... and its unflagged pockets of at most this many words ...
pub const POCKET_WORDS: usize = 2;
/// ... within this of a flagged word go with it.
pub const POCKET_GAP_MS: i64 = 300;
/// Words the owner's level needs.
pub const OWNER_MIN_WORDS: usize = 20;
/// After the canceller an owner's level under the floor plus this (dB) is
/// unknown (module docs).
pub const OWNER_PLAUSIBLE_DB: f32 = 15.0;
/// Text-matched words with a coefficient the calibration needs.
pub const CALIBRATE_MIN_MATCHED: usize = 20;
/// Owner-level words with a coefficient the keep bound needs.
pub const CALIBRATE_MIN_OWNER: usize = 10;
/// The calibrated drop bound's range, and the keep bound's.
pub const CALIBRATED_DROP: (f32, f32) = (0.55, 0.80);
pub const CALIBRATED_KEEP: (f32, f32) = (0.15, 0.80);
/// Echo shows on a recording that does not correlate once at least this
/// many words are in runs of three or more ...
pub const EVIDENCE_WORDS: usize = 8;
/// ... and they are at least this share of the mic words (chance is 0 to
/// 0.02 percent of them, a leak 6 to 13 percent).
pub const EVIDENCE_SHARE: f64 = 0.01;

/// Whether `run3` words in runs of three or more among `words` mic words
/// show echo ([`EVIDENCE_WORDS`], [`EVIDENCE_SHARE`]).
pub fn evidence(run3: usize, words: usize) -> bool {
    run3 >= EVIDENCE_WORDS && run3 as f64 >= EVIDENCE_SHARE * words as f64
}

/// One mic word as the decision sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct GateWord {
    /// Meeting time (ms).
    pub start_ms: i64,
    pub end_ms: i64,
    /// [`super::text::norm`] of its text.
    pub norm: String,
    /// The engine utterance it belongs to.
    pub group: u64,
    /// Correlation with the loopback (`Undefined` when not scored).
    pub score: WordScore,
    pub presence: Option<Presence>,
}

impl GateWord {
    fn level(&self) -> Option<f32> {
        self.presence.map(|p| p.level_db)
    }
}

/// What the decision knows about the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Context {
    pub floor_db: Option<f32>,
    pub owner_db: Option<f32>,
    pub band: GateBand,
    /// The correlation layer runs (and is the tie-breaker of two-word runs).
    pub correlation: bool,
    /// A canceller ran: the attenuation layer runs.
    pub canceller: bool,
    /// The text matches show echo ([`evidence`]). Without it, on a
    /// recording that does not correlate, no layer runs: a headphone
    /// recording without a leak is left exactly as it was.
    pub evidence: bool,
    /// The attenuation layer runs (after the canceller) ...
    pub attenuation: bool,
    /// ... at this threshold (dB) ...
    pub attenuation_db: f32,
    /// ... and how far under the owner's level a word must be for it (dB).
    pub attenuation_margin_db: f32,
    /// After the canceller, the correlation layer only removes words the
    /// canceller attenuated by at least this (dB; `None`: any word).
    pub correlation_min_attenuation_db: Option<f32>,
    /// The attenuation layer also needs the word's correlation with the
    /// loopback to reach this (`None`: no condition).
    pub attenuation_min_coefficient: Option<f32>,
    /// The level layer runs after the canceller too.
    pub level_after_canceller: bool,
}

impl Context {
    /// The shipped parameters around what the recording gives.
    pub fn new(
        floor_db: Option<f32>,
        owner_db: Option<f32>,
        band: GateBand,
        correlation: bool,
        canceller: bool,
        evidence: bool,
    ) -> Self {
        Self {
            floor_db,
            owner_db,
            band,
            correlation,
            canceller,
            evidence,
            attenuation: ATTENUATION_LAYER,
            attenuation_db: ATTENUATION_DB,
            attenuation_margin_db: ATTENUATION_MARGIN_DB,
            correlation_min_attenuation_db: CORRELATION_MIN_ATTENUATION_DB,
            attenuation_min_coefficient: ATTENUATION_MIN_COEFFICIENT,
            level_after_canceller: LEVEL_AFTER_CANCELLER,
        }
    }

    /// The attenuation layer fires on `w`.
    fn attenuated(&self, w: &GateWord) -> bool {
        self.attenuation
            && self.canceller
            && w.presence
                .and_then(|p| p.attenuation_db)
                .is_some_and(|a| a >= self.attenuation_db)
            && matches!((w.level(), self.owner_db), (Some(l), Some(o)) if l < o - self.attenuation_margin_db)
            && self
                .attenuation_min_coefficient
                .is_none_or(|c| w.score.coefficient().is_some_and(|v| v >= c))
    }

    /// The correlation layer may remove `w`.
    fn correlation_may(&self, w: &GateWord) -> bool {
        match (self.canceller, self.correlation_min_attenuation_db) {
            (true, Some(min)) => w
                .presence
                .and_then(|p| p.attenuation_db)
                .is_some_and(|a| a >= min),
            _ => true,
        }
    }

    fn near_floor(&self, w: &GateWord) -> bool {
        matches!((w.level(), self.floor_db), (Some(l), Some(f)) if l <= f + PRESENCE_DB)
    }

    /// Unknown (no level or no owner's level yet) counts as near.
    fn near_owner(&self, w: &GateWord) -> bool {
        match (w.level(), self.owner_db) {
            (Some(l), Some(o)) => l >= o - PRESENCE_DB,
            _ => true,
        }
    }

    fn tie_breaker(&self, w: &GateWord) -> bool {
        if self.correlation {
            w.score
                .coefficient()
                .is_some_and(|c| c >= self.band.keep_under)
                || self.attenuated(w)
        } else {
            self.near_floor(w) && !self.near_owner(w)
        }
    }
}

/// Groups for the utterance rule where the engine gives none (the file
/// engine's undiarized mic is one utterance): words in time order, a new
/// group after a pause over [`POCKET_GAP_MS`].
pub fn gap_groups(spans: impl IntoIterator<Item = (i64, i64)>) -> Vec<u64> {
    let mut g = 0u64;
    let mut last: Option<i64> = None;
    spans
        .into_iter()
        .map(|(s, e)| {
            if last.is_some_and(|l| s - l > POCKET_GAP_MS) {
                g += 1;
            }
            last = Some(last.map_or(e, |l| l.max(e)));
            g
        })
        .collect()
}

/// The text runs of words in time order.
pub fn text_runs(words: &[GateWord], far: &FarIndex, window: TextWindow) -> Runs {
    let mic: Vec<(i64, &str)> = words
        .iter()
        .map(|w| (w.start_ms, w.norm.as_str()))
        .collect();
    runs(&mic, far, window)
}

/// The layered decision over words in time order (see the module docs);
/// `runs` from [`text_runs`] over the same words.
pub fn decide(words: &[GateWord], runs: &Runs, ctx: &Context) -> Vec<Option<EchoReason>> {
    let n = words.len();
    if !(ctx.correlation || ctx.evidence) {
        return vec![None; n];
    }
    let corr = if ctx.correlation {
        let scores: Vec<WordScore> = words.iter().map(|w| w.score).collect();
        super::decide(&scores, ctx.band)
    } else {
        vec![false; n]
    };
    let mut out: Vec<Option<EchoReason>> = (0..n)
        .map(|i| {
            let w = &words[i];
            if runs.run3[i] || (runs.run2[i] && ctx.tie_breaker(w)) {
                Some(EchoReason::Text)
            } else if ctx.attenuated(w) {
                Some(EchoReason::Attenuation)
            } else if (!ctx.canceller || ctx.level_after_canceller)
                && w.presence.is_some_and(|p| p.far_active)
                && ctx.near_floor(w)
                && !ctx.near_owner(w)
            {
                Some(EchoReason::Level)
            } else if corr[i] && ctx.correlation_may(w) {
                Some(EchoReason::Correlation)
            } else {
                None
            }
        })
        .collect();
    // The utterance rule, per engine utterance (in time order).
    let mut groups: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, w) in words.iter().enumerate() {
        groups.entry(w.group).or_default().push(i);
    }
    for idx in groups.values() {
        let flagged = idx.iter().filter(|&&i| out[i].is_some()).count();
        if flagged == idx.len() || (flagged as f64) < UTTERANCE_SHARE * idx.len() as f64 {
            continue;
        }
        let mut k = 0;
        while k < idx.len() {
            if out[idx[k]].is_some() {
                k += 1;
                continue;
            }
            let a = k;
            while k < idx.len() && out[idx[k]].is_none() {
                k += 1;
            }
            let pocket = &idx[a..k];
            let next_to = |j: Option<usize>, gap: i64| j.is_some_and(|_| gap <= POCKET_GAP_MS);
            let before = (a > 0).then(|| idx[a - 1]);
            let after = idx.get(k).copied();
            let first = &words[pocket[0]];
            let last = &words[pocket[pocket.len() - 1]];
            let adjacent =
                next_to(
                    before,
                    before.map_or(0, |b| first.start_ms - words[b].end_ms),
                ) || next_to(after, after.map_or(0, |b| words[b].start_ms - last.end_ms));
            let sweep = pocket.len() <= POCKET_WORDS
                && adjacent
                && pocket.iter().all(|&i| {
                    let w = &words[i];
                    w.presence.is_some_and(|p| p.far_active) && !ctx.near_owner(w)
                });
            if sweep {
                for &i in pocket {
                    out[i] = Some(EchoReason::Utterance);
                }
            }
        }
    }
    out
}

/// What the calibration and the owner's level are built from: words whose
/// decision is settled (all of them in the post-pass and at Stop).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalibrationStats {
    /// Words, and of them in runs of three or more.
    words: usize,
    run3: usize,
    /// Coefficients of the words in runs of three or more.
    matched: Vec<f32>,
    /// Levels of the words in no run, the far end silent ...
    quiet: Vec<f32>,
    /// ... and of all words in no run, with their coefficients.
    unmatched: Vec<(f32, Option<f32>)>,
    /// After the canceller: levels of the words in no run it cut by under
    /// [`super::transfer::ATTENUATION_DB`], the far end silent ...
    clean_quiet: Vec<f32>,
    /// ... and of all of them.
    clean: Vec<f32>,
}

/// Where the owner's level came from ([`CalibrationStats::owner`];
/// `engine_info.echo.owner_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerSource {
    /// Words in no text run spoken while the far end was silent (after the
    /// canceller: of them, those it left alone).
    Quiet,
    /// After the canceller: any word in no run it left alone.
    Clean,
    /// Before the canceller: any word in no run.
    Unmatched,
    /// Too few words, or after the canceller an implausible estimate.
    None,
}

impl OwnerSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Clean => "clean",
            Self::Unmatched => "unmatched",
            Self::None => "none",
        }
    }
}

/// The owner's level and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnerLevel {
    /// dBFS (`None`: unknown).
    pub db: Option<f32>,
    pub source: OwnerSource,
    /// The words of the set it came from; when unknown, of the last set
    /// tried (too few, or with a median under the floor plus
    /// [`OWNER_PLAUSIBLE_DB`]).
    pub words: usize,
}

/// How many settled words each set of [`CalibrationStats::owner`] holds
/// (evaluation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OwnerCounts {
    /// Words in no run, the far end silent.
    pub quiet: usize,
    /// Words in no run.
    pub unmatched: usize,
    /// Words in no run the canceller cut by under 3 dB, the far end silent.
    pub clean_quiet: usize,
    /// Words in no run the canceller cut by under 3 dB.
    pub clean: usize,
}

/// The band the correlation layer uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub band: GateBand,
    /// Set per recording (else the fixed band).
    pub calibrated: bool,
    /// Text-matched words with a coefficient.
    pub matched: usize,
}

impl CalibrationStats {
    /// One settled word: `run3` in a run of three or more, `in_run` in any.
    pub fn add(&mut self, w: &GateWord, run3: bool, in_run: bool) {
        self.words += 1;
        self.run3 += usize::from(run3);
        if run3 {
            if let Some(c) = w.score.coefficient() {
                self.matched.push(c);
            }
        }
        if in_run {
            return;
        }
        if let Some(p) = w.presence {
            if !p.far_active {
                self.quiet.push(p.level_db);
            }
            self.unmatched.push((p.level_db, w.score.coefficient()));
            if p.attenuation_db
                .is_some_and(|a| a < super::transfer::ATTENUATION_DB)
            {
                if !p.far_active {
                    self.clean_quiet.push(p.level_db);
                }
                self.clean.push(p.level_db);
            }
        }
    }

    /// From words in time order and their runs.
    pub fn of(words: &[GateWord], runs: &Runs) -> Self {
        let mut s = Self::default();
        for (i, w) in words.iter().enumerate() {
            s.add(w, runs.run3[i], runs.any(i));
        }
        s
    }

    pub fn matched(&self) -> usize {
        self.matched.len()
    }

    /// The text matches show echo ([`evidence`]).
    pub fn evidence(&self) -> bool {
        evidence(self.run3, self.words)
    }

    /// [`Self::evidence`] with `run3` of `words` more not settled yet (the
    /// live view's open utterances).
    pub fn evidence_with(&self, run3: usize, words: usize) -> bool {
        evidence(self.run3 + run3, self.words + words)
    }

    /// The owner's level before the canceller (module docs; the live view's).
    pub fn owner_db(&self) -> Option<f32> {
        self.owner(false, None).db
    }

    /// The owner's level (module docs): `canceller` when the words were
    /// transcribed from the canceller's output, `floor_db` the floor of
    /// what was transcribed (the plausibility check, after the canceller).
    pub fn owner(&self, canceller: bool, floor_db: Option<f32>) -> OwnerLevel {
        let unmatched: Vec<f32>;
        let sets: [(&[f32], OwnerSource); 2] = if canceller {
            [
                (&self.clean_quiet, OwnerSource::Quiet),
                (&self.clean, OwnerSource::Clean),
            ]
        } else {
            unmatched = self.unmatched.iter().map(|u| u.0).collect();
            [
                (&self.quiet, OwnerSource::Quiet),
                (&unmatched, OwnerSource::Unmatched),
            ]
        };
        let Some(&(set, source)) = sets.iter().find(|s| s.0.len() >= OWNER_MIN_WORDS) else {
            return OwnerLevel {
                db: None,
                source: OwnerSource::None,
                words: sets[1].0.len(),
            };
        };
        let db = percentile(&mut set.to_vec(), 0.5);
        let implausible =
            canceller && matches!((db, floor_db), (Some(o), Some(f)) if o < f + OWNER_PLAUSIBLE_DB);
        if implausible {
            return OwnerLevel {
                db: None,
                source: OwnerSource::None,
                words: set.len(),
            };
        }
        OwnerLevel {
            db,
            source,
            words: set.len(),
        }
    }

    /// The sizes of the owner's level's sets (evaluation).
    pub fn owner_counts(&self) -> OwnerCounts {
        OwnerCounts {
            quiet: self.quiet.len(),
            unmatched: self.unmatched.len(),
            clean_quiet: self.clean_quiet.len(),
            clean: self.clean.len(),
        }
    }

    /// The correlation band: the drop bound at the 10th percentile of the
    /// text-matched words' coefficients (within [`CALIBRATED_DROP`]), the
    /// keep bound at the 90th percentile of the coefficients of the words in
    /// no run within [`PRESENCE_DB`] of the owner's level (within
    /// [`CALIBRATED_KEEP`]; `fixed`'s without enough of them), the
    /// neighbours' bound halfway, moved as `echo.gate_threshold` moves from
    /// its default. `fixed` (the shipped band for the recording) under
    /// [`CALIBRATE_MIN_MATCHED`] matches or when the bounds come within 0.05.
    pub fn band(&self, owner_db: Option<f32>, fixed: GateBand, threshold: f32) -> Calibration {
        let fallback = Calibration {
            band: fixed,
            calibrated: false,
            matched: self.matched.len(),
        };
        if self.matched.len() < CALIBRATE_MIN_MATCHED {
            return fallback;
        }
        let drop = percentile(&mut self.matched.clone(), 0.1)
            .unwrap_or(fixed.drop_at)
            .clamp(CALIBRATED_DROP.0, CALIBRATED_DROP.1);
        let mut near: Vec<f32> = self
            .unmatched
            .iter()
            .filter(|u| owner_db.is_some_and(|o| (u.0 - o).abs() <= PRESENCE_DB))
            .filter_map(|u| u.1)
            .collect();
        let keep = if near.len() >= CALIBRATE_MIN_OWNER {
            percentile(&mut near, 0.9)
                .unwrap_or(fixed.keep_under)
                .clamp(CALIBRATED_KEEP.0, CALIBRATED_KEEP.1)
        } else {
            fixed.keep_under
        };
        if drop - keep < 0.05 {
            return fallback;
        }
        let mid = keep + (drop - keep) / 2.0 + (threshold - super::GATE_THRESHOLD);
        Calibration {
            band: GateBand {
                keep_under: keep,
                threshold: mid.clamp(keep, drop),
                drop_at: drop,
            },
            calibrated: true,
            matched: self.matched.len(),
        }
    }
}

/// How the layered gate runs on a recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerOptions {
    pub correlation: bool,
    pub canceller: bool,
    /// The shipped band for the recording (raw or after the canceller).
    pub fixed: GateBand,
    /// `echo.gate_threshold`.
    pub threshold: f32,
    pub window: TextWindow,
    /// Calibrate the band from the text matches.
    pub calibrate: bool,
}

/// Words removed per layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LayerCounts {
    pub text: usize,
    pub attenuation: usize,
    pub level: usize,
    pub correlation: usize,
    pub utterance: usize,
    /// The post-pass transfer gate ([`super::transfer`]; only there).
    #[serde(skip_serializing_if = "is_zero")]
    pub transfer: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl LayerCounts {
    pub fn of(reasons: &[Option<EchoReason>]) -> Self {
        let mut c = Self::default();
        for r in reasons.iter().flatten() {
            *match r {
                EchoReason::Text => &mut c.text,
                EchoReason::Attenuation => &mut c.attenuation,
                EchoReason::Level => &mut c.level,
                EchoReason::Correlation => &mut c.correlation,
                EchoReason::Utterance => &mut c.utterance,
                EchoReason::Transfer => &mut c.transfer,
            } += 1;
        }
        c
    }

    pub fn total(&self) -> usize {
        self.text
            + self.attenuation
            + self.level
            + self.correlation
            + self.utterance
            + self.transfer
    }
}

/// What [`gate`] decided, in the input order.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerReport {
    pub reasons: Vec<Option<EchoReason>>,
    pub counts: LayerCounts,
    /// Words in runs of three or more, and of two (before the tie-breaker).
    pub run3: usize,
    pub run2: usize,
    pub calibration: Calibration,
    /// The owner's level and the floor the decision used.
    pub owner_db: Option<f32>,
    pub floor_db: Option<f32>,
    /// Where the owner's level came from, and from how many words
    /// ([`OwnerLevel`]).
    pub owner_source: OwnerSource,
    pub owner_words: usize,
}

impl LayerReport {
    pub fn dropped(&self) -> usize {
        self.counts.total()
    }
}

/// The whole decision over every word of a recording, given in any order:
/// the runs, the owner's level and the band from all of them, then
/// [`decide`]. The post-pass and Stop.
pub fn gate(
    words: &[GateWord],
    far: &FarIndex,
    floor_db: Option<f32>,
    opts: &LayerOptions,
) -> LayerReport {
    gate_with(words, far, floor_db, opts, &|_| {})
}

/// [`gate`] with the decision's parameters changed by `tweak` (the
/// evaluation's sweeps; the report's owner's level and floor are the ones
/// the decision used, after `tweak`).
pub fn gate_with(
    words: &[GateWord],
    far: &FarIndex,
    floor_db: Option<f32>,
    opts: &LayerOptions,
    tweak: &dyn Fn(&mut Context),
) -> LayerReport {
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| (words[i].start_ms, words[i].end_ms, i));
    let sorted: Vec<GateWord> = order.iter().map(|&i| words[i].clone()).collect();
    let r = text_runs(&sorted, far, opts.window);
    let stats = CalibrationStats::of(&sorted, &r);
    let owner = stats.owner(opts.canceller, floor_db);
    let owner_db = owner.db;
    let calibration = if opts.correlation && opts.calibrate {
        stats.band(owner_db, opts.fixed, opts.threshold)
    } else {
        Calibration {
            band: opts.fixed,
            calibrated: false,
            matched: stats.matched(),
        }
    };
    let mut ctx = Context::new(
        floor_db,
        owner_db,
        calibration.band,
        opts.correlation,
        opts.canceller,
        stats.evidence(),
    );
    tweak(&mut ctx);
    let d = decide(&sorted, &r, &ctx);
    let mut reasons = vec![None; words.len()];
    for (k, &i) in order.iter().enumerate() {
        reasons[i] = d[k];
    }
    LayerReport {
        counts: LayerCounts::of(&reasons),
        reasons,
        run3: r.count3(),
        run2: r.count2(),
        calibration,
        owner_db: ctx.owner_db,
        floor_db: ctx.floor_db,
        owner_source: owner.source,
        owner_words: owner.words,
    }
}

#[cfg(test)]
mod tests;
