//! The live side of the echo gate (plan 15; the parent module docs say
//! where it sits), on the recorder's event thread: the layered decision of
//! [`crate::audio::echo::layers`] over the live words.
//!
//! - Every mic word gets its presence (level and far-end activity) from the
//!   monitor's rings as soon as its window has arrived; its correlation once
//!   the correlation layer is active: from the first coupling (`echo.mode =
//!   auto`; it then stays active for the recording, like Stop's decision,
//!   so a release such as headphones put on mid-meeting brings nothing
//!   back) or from the start (`always`). Never after the user turned echo
//!   handling off ([`LiveEcho::turned_off`]).
//! - The loopback's words come in through [`LiveEcho::loopback`] (every
//!   revision replaces its utterance's words), for the text layer.
//! - A mic utterance is decided whenever something it depends on arrives
//!   (its own revision, a loopback revision, a pending score, a new band),
//!   over a window of the recent utterances; its decision may change for
//!   [`SETTLE_MS`] after its latest revision, through the re-send of the
//!   utterance ([`Update`]); then it is settled, and its words feed the
//!   calibration (the band and the owner's level) the later decisions use.
//!   The first coupling unsettles the utterances still in the ring once:
//!   their correlation is scored and they are decided again.
//! - A revision keeps the score of every word whose span did not change;
//!   only new or moved words are scored. A moved word whose audio has left
//!   the ring keeps the score of the word it replaces (the same index with
//!   overlapping time, else the same text with overlapping time), so a long
//!   echoed utterance does not show its first words again on a late
//!   revision. A word not scored yet whose span has not fully arrived is
//!   shown and scored again at the next mic frame; one older than the ring
//!   (a loopback stall) is given up (kept).
//! - Stop decides every live mic word of the run at once with everything
//!   known ([`LiveEcho::finish`], the same [`layers::gate`] as the
//!   post-pass), so a word can be shown and still be left out of the live
//!   run, or the reverse.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{round3, LiveWarning, LiveWord, ECHO_OFF_FOR_RECORDING};
use crate::audio::echo::layers::{
    self, Calibration, CalibrationStats, Context, GateWord, LayerOptions,
};
use crate::audio::echo::presence::Presence;
use crate::audio::echo::text::{norm, FarIndex, TextWindow};
use crate::audio::echo::{
    Avail, EchoMode, EchoMonitor, EchoState, EchoStatus, GateBand, WordScore, RING_SAMPLES,
};
use crate::types::EchoDroppedWord;

/// Per mic utterance whose shown words leave some out: the revision the
/// sink got and its drop flags (word identity: channel, utt, rev, index).
pub type HiddenWords = Arc<Mutex<HashMap<u32, (u32, Vec<bool>)>>>;

/// A mic utterance to send to the sink again: `(utt, rev, words to show)`.
pub type Update = (u32, u32, Vec<LiveWord>);

/// The monitor's ring in ms: a span older than this cannot be scored.
pub const RING_MS: i64 = (RING_SAMPLES / 16) as i64;

/// A mic utterance's decision may change for this long after its latest
/// revision (loopback words for the same moment can arrive after it).
pub const SETTLE_MS: i64 = 3_000;

/// Utterances before the oldest open one that the decision sees as
/// context (text runs and neighbours across utterance boundaries).
const CONTEXT_UTTS: u32 = 12;

/// The loopback words the text layer looks at around a window: before its
/// first mic word (the causal window's 1 s and a run's 2 s span) and after
/// the newest audio.
const FAR_BEFORE_MS: i64 = 3_500;
const FAR_AFTER_MS: i64 = 1_000;

/// A live word's presence.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pres {
    /// Its window has not fully arrived.
    Pending,
    /// Gone from the ring before it could be measured.
    Unknown,
    Known(Presence),
}

impl Pres {
    fn get(self) -> Option<Presence> {
        match self {
            Self::Known(p) => Some(p),
            _ => None,
        }
    }
}

/// One mic utterance's latest revision as the live gate saw it.
struct MicUtt {
    rev: u32,
    words: Vec<LiveWord>,
    norms: Vec<String>,
    /// Timeline samples (meeting ms × 16).
    spans: Vec<(i64, i64)>,
    /// `Pending`: not scored yet.
    scores: Vec<WordScore>,
    /// The coefficient of the word this one replaced in an earlier
    /// revision, for when its own span can no longer be scored.
    prior: Vec<Option<f32>>,
    presence: Vec<Pres>,
    /// The drop flags the sink last got.
    shown: Vec<bool>,
    /// Meeting time (ms) of the latest revision (by the mic's arrival).
    at_ms: i64,
    settled: bool,
}

impl MicUtt {
    /// The coefficient the earlier revision had for the word that `w` (at
    /// index `i`, span `span`) replaces: the same index with overlapping
    /// time, else the same text with overlapping time.
    fn prior_for(&self, i: usize, w: &LiveWord, span: (i64, i64)) -> Option<f32> {
        let coef = |j: usize| self.scores[j].coefficient().or(self.prior[j]);
        let overlaps = |s: (i64, i64)| s.0 < span.1.max(span.0 + 1) && span.0 < s.1.max(s.0 + 1);
        if i < self.spans.len() && overlaps(self.spans[i]) {
            if let Some(c) = coef(i) {
                return Some(c);
            }
        }
        (0..self.words.len())
            .find(|&j| self.words[j].text == w.text && overlaps(self.spans[j]))
            .and_then(coef)
    }

    fn gate_words(&self, utt: u32) -> impl Iterator<Item = GateWord> + '_ {
        (0..self.words.len()).map(move |i| GateWord {
            start_ms: self.words[i].start_ms,
            end_ms: self.words[i].end_ms,
            norm: self.norms[i].clone(),
            group: u64::from(utt),
            score: self.scores[i],
            presence: self.presence[i].get(),
        })
    }
}

/// A fresh score, or the earlier revision's when the span is gone.
fn settle(score: WordScore, prior: Option<f32>) -> WordScore {
    match (score, prior) {
        (WordScore::Undefined, Some(c)) => WordScore::Scored(c),
        _ => score,
    }
}

fn span_of(w: &LiveWord) -> (i64, i64) {
    (w.start_ms * 16, w.end_ms.max(w.start_ms) * 16)
}

/// The live side of the echo gate (see the module docs).
pub struct LiveEcho {
    monitor: Arc<Mutex<EchoMonitor>>,
    mode: EchoMode,
    threshold: f32,
    state: EchoState,
    warned: bool,
    /// The monitor reported `coupled` at some point.
    coupled_once: bool,
    utts: BTreeMap<u32, MicUtt>,
    /// Utterances with words not scored yet.
    pending: BTreeSet<u32>,
    /// Utterances not settled yet.
    open: BTreeSet<u32>,
    /// The newest mic word's end (meeting ms): the ring's horizon.
    latest_ms: i64,
    /// "Turn off for this recording" ([`super::Recorder::turn_off_echo`]).
    user_off: Arc<AtomicBool>,
    /// The drop flags the sink got, for [`super::Recorder::echo_hidden`].
    hidden: HiddenWords,
    /// Loopback words by (start, utterance, index), and each loopback
    /// utterance's keys.
    far: BTreeMap<(i64, u32, usize), String>,
    far_keys: HashMap<u32, Vec<(i64, u32, usize)>>,
    /// The settled words' statistics, and what they give (`None`: stale).
    stats: CalibrationStats,
    calibration: Option<(Calibration, Option<f32>)>,
    /// Something a decision depends on changed.
    dirty: bool,
}

/// What Stop materialises from the live gate.
pub struct LiveGate {
    /// `echo_dropped` is written (a layer ran and removed words, or the
    /// correlation layer ran).
    pub gated: bool,
    pub state: EchoState,
    /// `(utt, rev, word index)` left out of the live run.
    pub skip: std::collections::HashSet<(u32, u32, usize)>,
    pub dropped: Vec<EchoDroppedWord>,
    pub words: usize,
    pub info: serde_json::Value,
}

impl LiveEcho {
    pub fn new(
        monitor: Arc<Mutex<EchoMonitor>>,
        mode: EchoMode,
        threshold: f32,
        user_off: Arc<AtomicBool>,
        hidden: HiddenWords,
    ) -> Self {
        Self {
            monitor,
            mode,
            threshold,
            state: EchoState::default(),
            warned: false,
            coupled_once: false,
            utts: BTreeMap::new(),
            pending: BTreeSet::new(),
            open: BTreeSet::new(),
            latest_ms: 0,
            user_off,
            hidden,
            far: BTreeMap::new(),
            far_keys: HashMap::new(),
            stats: CalibrationStats::default(),
            calibration: None,
            dirty: false,
        }
    }

    /// The gate hides words: always, unless the user turned it off.
    fn active(&self) -> bool {
        !self.user_off.load(Ordering::SeqCst)
    }

    /// The correlation layer runs: coupled once in this recording, or
    /// `always`; never after the user turned it off.
    fn gating(&self) -> bool {
        self.active() && (self.mode == EchoMode::Always || self.coupled_once)
    }

    /// A device changed on either channel (the monitor re-acquires the lag).
    pub fn device_changed(&self) {
        self.lock().device_changed();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EchoMonitor> {
        self.monitor.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Words wait for a later mic frame to be scored.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether `u` still waits for a score.
    fn waits(&self, u: &MicUtt) -> bool {
        u.presence.contains(&Pres::Pending)
            || (self.gating() && u.scores.contains(&WordScore::Pending))
    }

    /// Scores the words of `u` not scored yet (one call for all of them):
    /// presence always, correlation when the layer runs; a word that ended
    /// longer than the ring ago is given up (`Undefined` or the score of
    /// the word it replaced; presence unknown).
    fn score_pending(&self, u: &mut MicUtt) {
        let horizon = self.latest_ms - RING_MS;
        let gating = self.gating();
        let mut corr = Vec::new();
        let mut pres = Vec::new();
        for i in 0..u.words.len() {
            let old = u.words[i].end_ms < horizon;
            if gating && u.scores[i] == WordScore::Pending {
                if old {
                    u.scores[i] = settle(WordScore::Undefined, u.prior[i]);
                } else {
                    corr.push(i);
                }
            }
            if u.presence[i] == Pres::Pending {
                if old {
                    u.presence[i] = Pres::Unknown;
                } else {
                    pres.push(i);
                }
            }
        }
        if corr.is_empty() && pres.is_empty() {
            return;
        }
        let mut mon = self.lock();
        if !corr.is_empty() {
            let spans: Vec<(i64, i64)> = corr.iter().map(|&i| u.spans[i]).collect();
            for (k, s) in mon.score_words(&spans).into_iter().enumerate() {
                let i = corr[k];
                u.scores[i] = settle(s, u.prior[i]);
            }
        }
        if !pres.is_empty() {
            let spans: Vec<(i64, i64)> = pres.iter().map(|&i| u.spans[i]).collect();
            for (k, p) in mon.presence_words(&spans).into_iter().enumerate() {
                u.presence[pres[k]] = match p {
                    Ok(p) => Pres::Known(p),
                    Err(Avail::Gone) => Pres::Unknown,
                    Err(_) => Pres::Pending,
                };
            }
        }
    }

    /// Scores what `utt` still lacks and keeps the pending set.
    fn score_utt(&mut self, utt: u32) {
        let Some(mut u) = self.utts.remove(&utt) else {
            return;
        };
        self.score_pending(&mut u);
        if self.waits(&u) {
            self.pending.insert(utt);
        } else {
            self.pending.remove(&utt);
        }
        self.utts.insert(utt, u);
    }

    /// A new mic revision (stored already): the words to show for it, and
    /// other mic utterances whose shown words changed.
    pub fn revision(
        &mut self,
        utt: u32,
        rev: u32,
        words: Vec<LiveWord>,
    ) -> (Vec<LiveWord>, Vec<Update>) {
        let spans: Vec<(i64, i64)> = words.iter().map(span_of).collect();
        if let Some(end) = words.iter().map(|w| w.end_ms).max() {
            self.latest_ms = self.latest_ms.max(end);
        }
        let n = words.len();
        let mut scores = vec![WordScore::Pending; n];
        let mut prior = vec![None; n];
        let mut presence = vec![Pres::Pending; n];
        let old = self.utts.remove(&utt);
        if let Some(o) = &old {
            let at: HashMap<(i64, i64), usize> =
                o.spans.iter().enumerate().map(|(j, s)| (*s, j)).collect();
            for i in 0..n {
                match at.get(&spans[i]) {
                    // The same span: its scores stand.
                    Some(&j) => {
                        scores[i] = o.scores[j];
                        prior[i] = o.prior[j].or(o.scores[j].coefficient());
                        presence[i] = o.presence[j];
                    }
                    // New or moved: scored below.
                    None => prior[i] = o.prior_for(i, &words[i], spans[i]),
                }
            }
        }
        let at_ms = self.lock().mic_head_ms();
        let shown = old.map_or_else(
            || vec![false; n],
            |o| {
                if o.shown.len() == n {
                    o.shown
                } else {
                    vec![false; n]
                }
            },
        );
        let u = MicUtt {
            rev,
            norms: words.iter().map(|w| norm(&w.text)).collect(),
            words,
            spans,
            scores,
            prior,
            presence,
            shown,
            at_ms,
            settled: false,
        };
        self.utts.insert(utt, u);
        self.open.insert(utt);
        if self.active() {
            self.score_utt(utt);
        }
        self.dirty = true;
        let mut updates = self.redecide();
        // This utterance goes out whole as the caller's own update.
        let mine = updates.iter().position(|x| x.0 == utt);
        if let Some(k) = mine {
            updates.remove(k);
        }
        let u = &self.utts[&utt];
        publish(&self.hidden, utt, u);
        (visible(u), updates)
    }

    /// A new loopback revision: its words replace the utterance's for the
    /// text layer; returns the mic utterances whose shown words changed.
    pub fn loopback(&mut self, utt: u32, _rev: u32, words: &[LiveWord]) -> Vec<Update> {
        if let Some(keys) = self.far_keys.remove(&utt) {
            for k in keys {
                self.far.remove(&k);
            }
        }
        let keys: Vec<(i64, u32, usize)> = words
            .iter()
            .enumerate()
            .map(|(i, w)| (w.start_ms, utt, i))
            .collect();
        for (k, w) in keys.iter().zip(words) {
            self.far.insert(*k, norm(&w.text));
        }
        self.far_keys.insert(utt, keys);
        self.dirty = true;
        self.redecide()
    }

    /// At a mic frame: scores the words still pending, settles what is due
    /// and decides again what changed; returns the utterances whose shown
    /// words changed.
    pub fn refresh(&mut self) -> Vec<Update> {
        if !self.active() {
            self.pending.clear();
            return Vec::new();
        }
        let keys: Vec<u32> = self.pending.iter().copied().collect();
        for utt in keys {
            self.score_utt(utt);
            self.dirty = true;
        }
        let now = self.lock().mic_head_ms();
        if self.open.iter().any(|k| {
            !self.pending.contains(k)
                && self.utts.get(k).is_some_and(|u| now - u.at_ms >= SETTLE_MS)
        }) {
            self.dirty = true;
        }
        self.redecide()
    }

    /// The band and the owner's level from the settled words.
    fn calibration(&mut self) -> (Calibration, Option<f32>) {
        if let Some(c) = self.calibration {
            return c;
        }
        let fixed = GateBand::with_threshold(self.threshold);
        let owner = self.stats.owner_db();
        let c = if self.gating() {
            self.stats.band(owner, fixed, self.threshold)
        } else {
            Calibration {
                band: fixed,
                calibrated: false,
                matched: self.stats.matched(),
            }
        };
        self.calibration = Some((c, owner));
        (c, owner)
    }

    /// Decides the open utterances over a window of the recent ones (see
    /// the module docs); settles those due; returns the utterances whose
    /// shown words changed.
    fn redecide(&mut self) -> Vec<Update> {
        if !self.dirty || !self.active() || self.open.is_empty() {
            return Vec::new();
        }
        self.dirty = false;
        let (now, floor_db) = {
            let m = self.lock();
            (m.mic_head_ms(), m.floor_db())
        };
        let (cal, owner_db) = self.calibration();
        let first = *self.open.iter().next().unwrap_or(&0);
        let lo = first.saturating_sub(CONTEXT_UTTS);
        let mut words: Vec<(GateWord, u32, usize)> = Vec::new();
        for (&utt, u) in self.utts.range(lo..) {
            for (i, w) in u.gate_words(utt).enumerate() {
                words.push((w, utt, i));
            }
        }
        words.sort_by_key(|w| (w.0.start_ms, w.0.end_ms, w.1, w.2));
        let t0 = words.first().map_or(now, |w| w.0.start_ms) - FAR_BEFORE_MS;
        let far = FarIndex::new(
            self.far
                .range((t0, 0, 0)..(now + FAR_AFTER_MS, 0, 0))
                .map(|(k, v)| (k.0, v.clone())),
        );
        let gw: Vec<GateWord> = words.iter().map(|w| w.0.clone()).collect();
        let runs = layers::text_runs(&gw, &far, TextWindow::DEFAULT);
        // The evidence counts the open utterances' matches too: a leak's
        // first burst shows it while it is still on screen.
        let (open_run3, open_words) = words
            .iter()
            .enumerate()
            .filter(|(_, w)| self.open.contains(&w.1))
            .fold((0, 0), |(r, n), (k, _)| {
                (r + usize::from(runs.run3[k]), n + 1)
            });
        let ctx = Context::new(
            floor_db,
            owner_db,
            cal.band,
            self.gating(),
            false,
            self.stats.evidence_with(open_run3, open_words),
        );
        let reasons = layers::decide(&gw, &runs, &ctx);
        let mut flags: HashMap<u32, Vec<bool>> = HashMap::new();
        for (k, (_, utt, i)) in words.iter().enumerate() {
            if !self.open.contains(utt) {
                continue;
            }
            let f = flags
                .entry(*utt)
                .or_insert_with(|| vec![false; self.utts[utt].words.len()]);
            f[*i] = reasons[k].is_some();
        }
        // Settle what is due: its words feed the calibration.
        let due: Vec<u32> = self
            .open
            .iter()
            .copied()
            .filter(|k| {
                !self.pending.contains(k)
                    && self.utts.get(k).is_some_and(|u| now - u.at_ms >= SETTLE_MS)
            })
            .collect();
        for &utt in &due {
            for (k, (w, u, _)) in words.iter().enumerate() {
                if *u == utt {
                    self.stats.add(w, runs.run3[k], runs.any(k));
                }
            }
            if let Some(u) = self.utts.get_mut(&utt) {
                u.settled = true;
            }
            self.open.remove(&utt);
            self.calibration = None;
        }
        if !due.is_empty() && !self.open.is_empty() {
            // A new band or owner's level: the open ones are decided again.
            self.dirty = true;
        }
        let mut out = Vec::new();
        for (utt, f) in flags {
            let Some(u) = self.utts.get_mut(&utt) else {
                continue;
            };
            if u.shown != f {
                u.shown = f;
                publish(&self.hidden, utt, u);
                out.push((utt, u.rev, visible(u)));
            }
        }
        out.sort_by_key(|x| x.0);
        out
    }

    /// The correlation layer became active: every word still in the ring is
    /// scored, and its utterance decided again.
    fn activate(&mut self) -> Vec<Update> {
        if !self.gating() {
            return Vec::new();
        }
        let horizon = self.latest_ms - RING_MS;
        let keys: Vec<u32> = self
            .utts
            .iter()
            .filter(|(_, u)| u.words.iter().any(|w| w.end_ms >= horizon))
            .map(|(k, _)| *k)
            .collect();
        let at_ms = self.lock().mic_head_ms();
        for utt in keys {
            if let Some(u) = self.utts.get_mut(&utt) {
                u.settled = false;
                u.at_ms = at_ms;
            }
            self.open.insert(utt);
            self.score_utt(utt);
        }
        self.calibration = None;
        self.dirty = true;
        self.redecide()
    }

    /// The monitor's status changed: the warning to show (the first
    /// coupling), and what to send again.
    pub fn status(&mut self, state: EchoState) -> (Option<LiveWarning>, Vec<Update>) {
        self.state = state;
        if state.status != EchoStatus::Coupled {
            return (None, Vec::new());
        }
        let warning = (!self.warned).then(|| {
            self.warned = true;
            LiveWarning::Echo {
                coupling: state.coupling,
                lag_ms: state.lag_ms.unwrap_or(0.0),
                aec: false,
            }
        });
        if self.coupled_once {
            return (warning, Vec::new());
        }
        self.coupled_once = true;
        let updates = if self.mode == EchoMode::Always {
            Vec::new()
        } else {
            self.activate()
        };
        (warning, updates)
    }

    /// The user turned the gate off: every utterance shown gated is sent
    /// again whole.
    pub fn turned_off(&mut self) -> Vec<Update> {
        self.pending.clear();
        let mut out = Vec::new();
        for (&utt, u) in self.utts.iter_mut() {
            if u.shown.iter().any(|d| *d) {
                u.shown = vec![false; u.words.len()];
                publish(&self.hidden, utt, u);
                out.push((utt, u.rev, visible(u)));
            }
        }
        out
    }

    /// At Stop: the last pending words scored, the layered decision over
    /// every live mic word with every loopback word ([`layers::gate`]), and
    /// what the live run leaves out.
    pub fn finish(&mut self, tap_frames_dropped: u64) -> LiveGate {
        let ever = self.lock().ever_coupled();
        if ever && !self.coupled_once {
            self.coupled_once = true;
            let _ = self.activate();
        }
        let _ = self.refresh();
        let user_off = !self.active();
        let correlation = self.gating();
        let (state, floor_db, coupled, coupled_at_ms) = {
            let m = self.lock();
            (m.state(), m.floor_db(), m.ever_coupled(), m.coupled_at_ms())
        };
        let mut words: Vec<GateWord> = Vec::new();
        let mut ids: Vec<(u32, u32, usize)> = Vec::new();
        for (&utt, u) in &self.utts {
            for (i, w) in u.gate_words(utt).enumerate() {
                words.push(w);
                ids.push((utt, u.rev, i));
            }
        }
        let mut skip = std::collections::HashSet::new();
        let mut dropped = Vec::new();
        let report = (!user_off).then(|| {
            let far = FarIndex::new(self.far.iter().map(|(k, v)| (k.0, v.clone())));
            layers::gate(
                &words,
                &far,
                floor_db,
                &LayerOptions {
                    correlation,
                    canceller: false,
                    fixed: GateBand::with_threshold(self.threshold),
                    threshold: self.threshold,
                    window: TextWindow::DEFAULT,
                    calibrate: true,
                },
            )
        });
        if let Some(r) = &report {
            let mut order: Vec<usize> = (0..words.len()).collect();
            order.sort_by_key(|&k| (words[k].start_ms, ids[k].0, ids[k].2));
            for k in order {
                let Some(reason) = r.reasons[k] else {
                    continue;
                };
                let (utt, rev, i) = ids[k];
                skip.insert((utt, rev, i));
                let lw = &self.utts[&utt].words[i];
                dropped.push(EchoDroppedWord {
                    start_ms: lw.start_ms,
                    end_ms: lw.end_ms,
                    text: lw.text.clone(),
                    coefficient: words[k].score.coefficient().unwrap_or(0.0),
                    reason: Some(reason),
                });
            }
        }
        let band = report
            .as_ref()
            .map_or(GateBand::with_threshold(self.threshold), |r| {
                r.calibration.band
            });
        let mut info = serde_json::json!({
            "status": state.status,
            "coupled": coupled,
            "coupling": round3(state.coupling),
            "lag_ms": state.lag_ms.map(round3),
            "erl_db": state.erl_db.map(round3),
            "coupled_at_ms": coupled_at_ms,
            "mic_words_before": words.len(),
            "dropped": dropped.len(),
            "threshold": round3(self.threshold),
            "band": [round3(band.keep_under), round3(band.threshold), round3(band.drop_at)],
            "aec": false,
            "mode": if user_off { ECHO_OFF_FOR_RECORDING } else { self.mode.as_str() },
            "user_off": user_off,
            "tap_frames_dropped": tap_frames_dropped,
        });
        if let Some(r) = &report {
            super::postpass::layer_summary(&mut info, r, correlation);
        }
        LiveGate {
            gated: !user_off && (correlation || !dropped.is_empty()),
            state,
            skip,
            dropped,
            words: words.len(),
            info,
        }
    }

    /// What the sink shows now per mic utterance: (revision, drop flags)
    /// (evaluation).
    pub fn shown_flags(&self) -> BTreeMap<u32, (u32, Vec<bool>)> {
        self.utts
            .iter()
            .map(|(&k, u)| (k, (u.rev, u.shown.clone())))
            .collect()
    }

    /// Whether every utterance has settled (evaluation).
    pub fn all_settled(&self) -> bool {
        self.open.is_empty()
    }
}

/// Records the drop flags the sink got for `utt` (none dropped: nothing).
fn publish(hidden: &HiddenWords, utt: u32, u: &MicUtt) {
    let mut h = hidden.lock().unwrap_or_else(|e| e.into_inner());
    if u.shown.iter().any(|d| *d) {
        h.insert(utt, (u.rev, u.shown.clone()));
    } else {
        h.remove(&utt);
    }
}

fn visible(u: &MicUtt) -> Vec<LiveWord> {
    u.words
        .iter()
        .zip(&u.shown)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| w.clone())
        .collect()
}

#[cfg(test)]
mod tests;
