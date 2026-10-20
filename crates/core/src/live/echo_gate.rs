//! The live side of the echo gate (plan 15; the parent module docs say
//! where it sits), on the recorder's event thread.
//!
//! - Nothing is scored until the gate is active: from the first coupling
//!   (`echo.mode = auto`; it then stays active for the recording, like
//!   Stop's decision, so a release such as headphones put on mid-meeting
//!   brings nothing back) or from the start (`always`), and never after the
//!   user turned it off ([`LiveEcho::turned_off`]). A recording that never
//!   couples costs no scoring at all.
//! - When the gate becomes active, the earlier words still inside the
//!   monitor's ring are scored once; older words are kept (`Undefined`).
//! - A revision keeps the score of every word whose span did not change;
//!   only new or moved words are scored. A moved word whose audio has left
//!   the ring keeps the score of the word it replaces (the same index with
//!   overlapping time, else the same text with overlapping time), so a long
//!   echoed utterance does not show its first words again on a late
//!   revision. A word not scored yet whose span has not fully arrived is
//!   shown and scored again at the next mic frame; one older than the ring
//!   (a loopback stall) is given up as `Undefined` and kept.
//! - The sink gets the hysteresis ([`decide`]) per utterance; Stop decides
//!   it over every live mic word of the run in time order
//!   ([`LiveEcho::finish`]), so a middle-band word at an utterance's edge
//!   can be shown and still be left out of the live run, or the reverse.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{round3, LiveWarning, LiveWord, ECHO_OFF_FOR_RECORDING};
use crate::audio::echo::{
    decide, EchoMode, EchoMonitor, EchoState, EchoStatus, GateBand, WordScore, RING_SAMPLES,
};
use crate::types::EchoDroppedWord;

/// Per mic utterance whose shown words leave some out: the revision the
/// sink got and its drop flags (word identity: channel, utt, rev, index).
pub(super) type HiddenWords = Arc<Mutex<HashMap<u32, (u32, Vec<bool>)>>>;

/// A mic utterance to send to the sink again: `(utt, rev, words to show)`.
pub(super) type Update = (u32, u32, Vec<LiveWord>);

/// The monitor's ring in ms: a span older than this cannot be scored.
const RING_MS: i64 = (RING_SAMPLES / 16) as i64;

/// One mic utterance's latest revision as the live gate saw it.
struct MicUtt {
    rev: u32,
    words: Vec<LiveWord>,
    /// Timeline samples (meeting ms × 16).
    spans: Vec<(i64, i64)>,
    /// `Pending`: not scored yet.
    scores: Vec<WordScore>,
    /// The coefficient of the word this one replaced in an earlier
    /// revision, for when its own span can no longer be scored.
    prior: Vec<Option<f32>>,
    /// The drop flags the sink last got.
    shown: Vec<bool>,
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
pub(super) struct LiveEcho {
    monitor: Arc<Mutex<EchoMonitor>>,
    mode: EchoMode,
    threshold: f32,
    state: EchoState,
    warned: bool,
    /// The monitor reported `coupled` at some point.
    coupled_once: bool,
    utts: BTreeMap<u32, MicUtt>,
    /// Utterances with words the active gate has not scored yet.
    pending: BTreeSet<u32>,
    /// The newest mic word's end (meeting ms): the ring's horizon.
    latest_ms: i64,
    /// "Turn off for this recording" ([`super::Recorder::turn_off_echo`]).
    user_off: Arc<AtomicBool>,
    /// The drop flags the sink got, for [`super::Recorder::echo_hidden`].
    hidden: HiddenWords,
}

/// What Stop materialises from the live gate.
pub(super) struct LiveGate {
    pub gated: bool,
    pub state: EchoState,
    /// `(utt, rev, word index)` left out of the live run.
    pub skip: std::collections::HashSet<(u32, u32, usize)>,
    pub dropped: Vec<EchoDroppedWord>,
    pub words: usize,
    pub info: serde_json::Value,
}

impl LiveEcho {
    pub(super) fn new(
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
            latest_ms: 0,
            user_off,
            hidden,
        }
    }

    /// The gate scores and hides words: coupled once in this recording, or
    /// `always`; never after the user turned it off.
    fn gating(&self) -> bool {
        !self.user_off.load(Ordering::SeqCst)
            && (self.mode == EchoMode::Always || self.coupled_once)
    }

    /// A device changed on either channel (the monitor re-acquires the lag).
    pub(super) fn device_changed(&self) {
        self.monitor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .device_changed();
    }

    /// Words wait for a later mic frame to be scored.
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn score(&self, spans: &[(i64, i64)]) -> Vec<WordScore> {
        self.monitor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .score_words(spans)
    }

    /// Scores the words of `u` not scored yet (one call for all of them);
    /// a word that ended longer than the ring ago is not scored (`Undefined`,
    /// or the score of the word it replaced).
    fn score_pending(&self, u: &mut MicUtt) {
        let horizon = self.latest_ms - RING_MS;
        let mut todo = Vec::new();
        for i in 0..u.scores.len() {
            if u.scores[i] != WordScore::Pending {
                continue;
            }
            if u.words[i].end_ms < horizon {
                u.scores[i] = settle(WordScore::Undefined, u.prior[i]);
            } else {
                todo.push(i);
            }
        }
        if todo.is_empty() {
            return;
        }
        let spans: Vec<(i64, i64)> = todo.iter().map(|&i| u.spans[i]).collect();
        for (k, s) in self.score(&spans).into_iter().enumerate() {
            let i = todo[k];
            u.scores[i] = settle(s, u.prior[i]);
        }
    }

    /// The drop flags the sink should show for `u` now.
    fn drops(&self, u: &MicUtt) -> Vec<bool> {
        if self.gating() {
            decide(&u.scores, GateBand::with_threshold(self.threshold))
        } else {
            vec![false; u.words.len()]
        }
    }

    /// A new mic revision (stored already); returns the words to show.
    pub(super) fn revision(&mut self, utt: u32, rev: u32, words: Vec<LiveWord>) -> Vec<LiveWord> {
        let spans: Vec<(i64, i64)> = words.iter().map(span_of).collect();
        if let Some(end) = words.iter().map(|w| w.end_ms).max() {
            self.latest_ms = self.latest_ms.max(end);
        }
        let n = words.len();
        let mut scores = vec![WordScore::Pending; n];
        let mut prior = vec![None; n];
        let old = self.utts.remove(&utt);
        if let (Some(o), true) = (&old, self.gating()) {
            let at: HashMap<(i64, i64), usize> =
                o.spans.iter().enumerate().map(|(j, s)| (*s, j)).collect();
            for i in 0..n {
                match at.get(&spans[i]) {
                    // The same span: its score stands.
                    Some(&j) => {
                        scores[i] = o.scores[j];
                        prior[i] = o.prior[j].or(o.scores[j].coefficient());
                    }
                    // New or moved: scored below.
                    None => prior[i] = o.prior_for(i, &words[i], spans[i]),
                }
            }
        }
        let mut u = MicUtt {
            rev,
            words,
            spans,
            scores,
            prior,
            shown: Vec::new(),
        };
        if self.gating() {
            self.score_pending(&mut u);
            if u.scores.contains(&WordScore::Pending) {
                self.pending.insert(utt);
            } else {
                self.pending.remove(&utt);
            }
        }
        u.shown = self.drops(&u);
        publish(&self.hidden, utt, &u);
        let shown = visible(&u);
        self.utts.insert(utt, u);
        shown
    }

    /// Scores the words still pending (at a mic frame); returns the
    /// utterances whose shown words changed.
    pub(super) fn refresh(&mut self) -> Vec<Update> {
        let keys: Vec<u32> = std::mem::take(&mut self.pending).into_iter().collect();
        if !self.gating() {
            return Vec::new();
        }
        for &utt in &keys {
            let Some(mut u) = self.utts.remove(&utt) else {
                continue;
            };
            self.score_pending(&mut u);
            if u.scores.contains(&WordScore::Pending) {
                self.pending.insert(utt);
            }
            self.utts.insert(utt, u);
        }
        self.reshow(keys)
    }

    /// Recomputes the drop flags of `keys`; returns those that changed.
    fn reshow(&mut self, keys: Vec<u32>) -> Vec<Update> {
        let mut out = Vec::new();
        for utt in keys {
            let Some(drops) = self.utts.get(&utt).map(|u| self.drops(u)) else {
                continue;
            };
            if let Some(u) = self.utts.get_mut(&utt) {
                if u.shown != drops {
                    u.shown = drops;
                    publish(&self.hidden, utt, u);
                    out.push((utt, u.rev, visible(u)));
                }
            }
        }
        out
    }

    /// The gate became active: every word not scored yet is scored (those
    /// older than the ring are kept).
    fn activate(&mut self) -> Vec<Update> {
        if !self.gating() {
            return Vec::new();
        }
        self.pending = self
            .utts
            .iter()
            .filter(|(_, u)| u.scores.contains(&WordScore::Pending))
            .map(|(k, _)| *k)
            .collect();
        self.refresh()
    }

    /// The monitor's status changed: the warning to show (the first
    /// coupling), and what to send again.
    pub(super) fn status(&mut self, state: EchoState) -> (Option<LiveWarning>, Vec<Update>) {
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
    pub(super) fn turned_off(&mut self) -> Vec<Update> {
        self.pending.clear();
        let keys = self.utts.keys().copied().collect();
        self.reshow(keys)
    }

    /// At Stop: the last pending words scored, the hysteresis over every
    /// live mic word in time order, and what the live run leaves out.
    pub(super) fn finish(&mut self, tap_frames_dropped: u64) -> LiveGate {
        let ever = self
            .monitor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ever_coupled();
        if ever && !self.coupled_once {
            self.coupled_once = true;
            let _ = self.activate();
        }
        let _ = self.refresh();
        let mon = self.monitor.lock().unwrap_or_else(|e| e.into_inner());
        let state = mon.state();
        let user_off = self.user_off.load(Ordering::SeqCst);
        let gated = self.gating();
        let mut all: Vec<(i64, u32, u32, usize, WordScore)> = Vec::new();
        for (&utt, u) in &self.utts {
            for (i, s) in u.scores.iter().enumerate() {
                all.push((u.spans[i].0, utt, u.rev, i, *s));
            }
        }
        all.sort_by_key(|w| (w.0, w.1, w.3));
        let mut skip = std::collections::HashSet::new();
        let mut dropped = Vec::new();
        if gated {
            let flags = decide(
                &all.iter().map(|w| w.4).collect::<Vec<_>>(),
                GateBand::with_threshold(self.threshold),
            );
            for (w, d) in all.iter().zip(&flags) {
                if *d {
                    skip.insert((w.1, w.2, w.3));
                    let lw = &self.utts[&w.1].words[w.3];
                    dropped.push(EchoDroppedWord {
                        start_ms: lw.start_ms,
                        end_ms: lw.end_ms,
                        text: lw.text.clone(),
                        coefficient: w.4.coefficient().unwrap_or(0.0),
                    });
                }
            }
        }
        let info = serde_json::json!({
            "status": state.status,
            "coupled": mon.ever_coupled(),
            "coupling": round3(state.coupling),
            "lag_ms": state.lag_ms.map(round3),
            "erl_db": state.erl_db.map(round3),
            "coupled_at_ms": mon.coupled_at_ms(),
            "mic_words_before": all.len(),
            "dropped": dropped.len(),
            "threshold": round3(self.threshold),
            "aec": false,
            "mode": if user_off { ECHO_OFF_FOR_RECORDING } else { self.mode.as_str() },
            "user_off": user_off,
            "tap_frames_dropped": tap_frames_dropped,
        });
        LiveGate {
            gated,
            state,
            skip,
            dropped,
            words: all.len(),
            info,
        }
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
