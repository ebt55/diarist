//! The live gate on its own, driven frame by frame (no recorder, no real
//! time): the monitor on raw channel indices, a word per second, the event
//! loop's reactions to status changes and mic frames.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::audio::echo::synth::{add_delayed, add_noise, db, monologue, to_i16};
use crate::audio::echo::{decide, COUPLED_AT, GATE_DROP_AT, GATE_KEEP_UNDER, GATE_THRESHOLD};
use crate::audio::pipeline::{Frame, Level};
use crate::types::Channel;

const SEC: usize = 16_000;

/// The far end on the loopback and, 3 ms later and 10 dB down, on the mic
/// (a meeting on speakers), plus `owner` added to the mic.
fn speakers(secs: usize, owner: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let (far, _) = monologue(41, secs * SEC, db(-28.0));
    let mut lb = far.clone();
    add_noise(&mut lb, 3, db(-65.0));
    let mut mic = vec![0.0f32; secs * SEC];
    add_delayed(&mut mic, &far, 48, db(-10.0));
    for (m, o) in mic.iter_mut().zip(owner) {
        *m += o;
    }
    add_noise(&mut mic, 4, db(-60.0));
    (mic, lb)
}

/// The word of second `sec`, `shift_ms` later than its first revision.
fn word(sec: i64, shift_ms: i64) -> LiveWord {
    LiveWord {
        start_ms: sec * 1_000 + 100 + shift_ms,
        end_ms: sec * 1_000 + 800 + shift_ms,
        text: format!("w{sec}"),
        label: "me".into(),
        speaker_id: None,
    }
}

/// The gate with its monitor, fed like the recorder's event loop does.
struct Run {
    gate: LiveEcho,
    monitor: Arc<Mutex<EchoMonitor>>,
    mic: Vec<i16>,
    lb: Option<Vec<i16>>,
    at: usize,
    /// What the sink shows per utterance: (rev, words).
    shown: BTreeMap<u32, (u32, Vec<LiveWord>)>,
    warnings: usize,
    /// The most utterances ever waiting to be scored.
    most_pending: usize,
}

impl Run {
    /// `lb: None`: the loopback delivers nothing (a stalled capture).
    fn new(mode: EchoMode, mic: &[f32], lb: Option<&[f32]>) -> Self {
        let monitor = Arc::new(Mutex::new(EchoMonitor::new(COUPLED_AT, None)));
        Self {
            gate: LiveEcho::new(
                Arc::clone(&monitor),
                mode,
                GATE_THRESHOLD,
                Arc::new(AtomicBool::new(false)),
                Arc::default(),
            ),
            monitor,
            mic: to_i16(mic),
            lb: lb.map(to_i16),
            at: 0,
            shown: BTreeMap::new(),
            warnings: 0,
            most_pending: 0,
        }
    }

    fn frame(channel: Channel, at: usize, pcm: &[i16]) -> Frame {
        Frame {
            channel,
            start_sample: at as u64,
            pts_us: at as u64 * 1_000 / 16,
            pcm: pcm.to_vec(),
            level: Level::default(),
            gain_db: 0.0,
            raw_peak_dbfs: -100.0,
        }
    }

    fn apply(&mut self, updates: Vec<Update>) {
        for (utt, rev, words) in updates {
            self.shown.insert(utt, (rev, words));
        }
    }

    /// Plays both channels up to second `secs` in 100 ms frames (loopback
    /// first), reacting to status changes and to mic frames.
    fn play(&mut self, secs: usize) {
        while self.at + 1_600 <= (secs * SEC).min(self.mic.len()) {
            let mut frames = Vec::new();
            if let Some(lb) = &self.lb {
                frames.push(Self::frame(
                    Channel::Loopback,
                    self.at,
                    &lb[self.at..self.at + 1_600],
                ));
            }
            frames.push(Self::frame(
                Channel::Mic,
                self.at,
                &self.mic[self.at..self.at + 1_600],
            ));
            for f in frames {
                let change = self.monitor.lock().unwrap().push(&f);
                if let Some(state) = change {
                    let (warning, updates) = self.gate.status(state);
                    self.warnings += usize::from(warning.is_some());
                    self.apply(updates);
                }
                if f.channel == Channel::Mic {
                    let updates = self.gate.refresh();
                    self.apply(updates);
                }
            }
            self.most_pending = self.most_pending.max(self.gate.pending.len());
            self.at += 1_600;
        }
    }

    fn revise(&mut self, utt: u32, rev: u32, words: Vec<LiveWord>) {
        let (shown, updates) = self.gate.revision(utt, rev, words);
        self.shown.insert(utt, (rev, shown));
        self.apply(updates);
        self.most_pending = self.most_pending.max(self.gate.pending.len());
    }

    fn texts(&self, utt: u32) -> Vec<String> {
        self.shown[&utt].1.iter().map(|w| w.text.clone()).collect()
    }
}

#[test]
fn a_long_echoed_utterance_keeps_its_first_words_out_after_they_leave_the_ring() {
    // One mic utterance longer than the ring, revised every 10 s, every
    // revision moving all words 4 ms later (as the ASR re-stamps them).
    let secs = (RING_MS / 1_000) as usize + 25;
    let (mic, lb) = speakers(secs, &[]);
    let mut run = Run::new(EchoMode::Auto, &mic, Some(&lb));
    let mut ends: Vec<usize> = (10..secs).step_by(10).collect();
    ends.push(secs);
    for (rev, &t) in ends.iter().enumerate() {
        run.play(t);
        let words = (0..t as i64).map(|s| word(s, rev as i64 * 4)).collect();
        run.revise(0, rev as u32, words);
        if t == 10 {
            // Not coupled yet: nothing is scored, everything is shown.
            assert!(!run.gate.coupled_once);
            assert_eq!(run.texts(0).len(), 10);
            assert!(run.gate.utts[&0]
                .scores
                .iter()
                .all(|s| *s == WordScore::Pending));
        }
    }
    assert!(run.gate.coupled_once);
    assert_eq!(run.warnings, 1);
    // The first 10 s are far older than the ring at the last revision, and
    // every span moved: their words keep the scores they had.
    let u = &run.gate.utts[&0];
    assert!(
        u.words
            .iter()
            .zip(&u.scores)
            .all(|(w, s)| !(1_000..10_000).contains(&w.start_ms)
                || matches!(s, WordScore::Scored(c) if *c >= GATE_DROP_AT)),
        "{:?}",
        u.scores
    );
    let shown = &run.shown[&0].1;
    let early: Vec<&str> = shown
        .iter()
        .filter(|w| w.start_ms < 10_000)
        .map(|w| w.text.as_str())
        .collect();
    assert!(
        early.iter().all(|t| *t == "w0"),
        "only the word before the ring's start may show: {early:?}"
    );
    assert!(shown.len() <= 2, "{shown:?}");
    // Stop leaves them out of the live run too.
    let last = run.shown[&0].0;
    let gate = run.gate.finish(0);
    assert!(gate.gated);
    for i in 1..10 {
        assert!(gate.skip.contains(&(0, last, i)), "w{i} kept");
    }
    assert!(
        gate.dropped.len() + 2 >= secs,
        "{} of {secs}",
        gate.dropped.len()
    );
    assert_eq!(
        gate.info["dropped"].as_u64(),
        Some(gate.dropped.len() as u64)
    );
}

#[test]
fn hysteresis_decides_middle_band_words_by_their_neighbours_in_the_utterance() {
    // The owner, 10 dB over the echo, over one word (w5) inside far-end
    // speech, and over a turn of five words (w13 to w17).
    let secs = 26;
    let (owner, _) = monologue(52, secs * SEC, db(-28.0));
    let mask =
        |t: usize| (5 * SEC..5 * SEC + 14_400).contains(&t) || (204_800..286_400).contains(&t);
    let owner: Vec<f32> = owner
        .iter()
        .enumerate()
        .map(|(t, v)| if mask(t) { *v } else { 0.0 })
        .collect();
    let (mic, lb) = speakers(secs, &owner);
    let mut run = Run::new(EchoMode::Auto, &mic, Some(&lb));
    for (rev, t) in [10usize, 20, 26].into_iter().enumerate() {
        run.play(t);
        run.revise(
            0,
            rev as u32,
            (0..t as i64 - 1).map(|s| word(s, 0)).collect(),
        );
    }
    let u = &run.gate.utts[&0];
    let coef = |i: usize| u.scores[i].coefficient().unwrap();
    let band = GATE_KEEP_UNDER..GATE_DROP_AT;
    // w5 sits in the middle band; its echoed neighbours drop it.
    assert!(band.contains(&coef(5)), "w5 at {}", coef(5));
    assert!(coef(4) >= GATE_DROP_AT && coef(7) >= GATE_DROP_AT);
    // The owner's turn sits under the drop bound; its own neighbours keep it.
    for i in 14..=16 {
        assert!(coef(i) < GATE_DROP_AT, "w{i} at {}", coef(i));
    }
    let shown = run.texts(0);
    assert!(!shown.contains(&"w5".to_owned()), "{shown:?}");
    for t in ["w14", "w15", "w16"] {
        assert!(shown.contains(&t.to_owned()), "{t} not shown: {shown:?}");
    }
    assert_eq!(
        u.shown,
        decide(&u.scores, GateBand::with_threshold(GATE_THRESHOLD))
    );
}

#[test]
fn without_coupling_nothing_is_scored_and_every_word_is_shown() {
    // Headphones: the far end on the loopback, the owner alone on the mic.
    let secs = 30;
    let (far, _) = monologue(41, secs * SEC, db(-28.0));
    let (mut mic, _) = monologue(52, secs * SEC, db(-33.0));
    add_noise(&mut mic, 4, db(-60.0));
    let mut run = Run::new(EchoMode::Auto, &mic, Some(&far));
    for s in 1..secs as i64 {
        run.play(s as usize);
        for rev in 0..2 {
            let w = vec![word(s - 1, rev * 30)];
            run.revise((s - 1) as u32, rev as u32, w.clone());
            assert_eq!(run.shown[&((s - 1) as u32)].1, w);
        }
    }
    assert_ne!(
        run.monitor.lock().unwrap().state().status,
        EchoStatus::Coupled
    );
    assert!(!run.gate.coupled_once && run.warnings == 0);
    assert_eq!(run.most_pending, 0, "nothing waits to be scored");
    assert!(run
        .gate
        .utts
        .values()
        .all(|u| u.scores.iter().all(|s| *s == WordScore::Pending)));
    assert!(run.gate.hidden.lock().unwrap().is_empty());
    let gate = run.gate.finish(0);
    assert!(!gate.gated && gate.skip.is_empty() && gate.dropped.is_empty());
    assert_eq!(gate.info["coupled"], false);
    assert_eq!(gate.info["dropped"].as_u64(), Some(0));
    assert_eq!(
        gate.info["mic_words_before"].as_u64(),
        Some(secs as u64 - 1)
    );
}

#[test]
fn a_stalled_loopback_does_not_grow_the_pending_list_past_the_ring() {
    // `always`: scored from the start, but the loopback never arrives, so
    // no word can be scored; words older than the ring are given up.
    let secs = (RING_MS / 1_000) as usize + 15;
    let (mic, _) = speakers(secs, &[]);
    let mut run = Run::new(EchoMode::Always, &mic, None);
    for s in 1..secs as i64 {
        run.play(s as usize);
        run.revise((s - 1) as u32, 0, vec![word(s - 1, 0)]);
    }
    let ring_words = (RING_MS / 1_000) as usize;
    assert!(
        run.most_pending <= ring_words + 2,
        "{} utterances pending",
        run.most_pending
    );
    let old = &run.gate.utts[&0];
    assert_eq!(old.scores, [WordScore::Undefined]);
    assert!(!run.gate.pending.contains(&0));
    // Nothing could be scored, so everything is shown and kept.
    assert!(run.shown.values().all(|(_, w)| w.len() == 1));
    let gate = run.gate.finish(0);
    assert!(gate.gated && gate.dropped.is_empty());
}

#[test]
fn turning_the_gate_off_shows_everything_again_and_stops_scoring() {
    let secs = 26;
    let (mic, lb) = speakers(secs, &[]);
    let mut run = Run::new(EchoMode::Auto, &mic, Some(&lb));
    run.play(22);
    run.revise(0, 0, (0..21).map(|s| word(s, 0)).collect());
    // Only words whose audio had left the ring before they were scored show.
    let ring_start = 22_000 - RING_MS;
    assert!(
        run.shown[&0]
            .1
            .iter()
            .all(|w| w.start_ms - 350 < ring_start),
        "{:?}",
        run.texts(0)
    );
    assert!(!run.gate.hidden.lock().unwrap().is_empty());
    run.gate.user_off.store(true, Ordering::SeqCst);
    let updates = run.gate.turned_off();
    run.apply(updates);
    assert_eq!(run.texts(0).len(), 21);
    assert!(run.gate.hidden.lock().unwrap().is_empty());
    run.play(secs);
    run.revise(0, 1, (0..25).map(|s| word(s, 4)).collect());
    assert_eq!(run.texts(0).len(), 25);
    assert!(!run.gate.has_pending());
    let gate = run.gate.finish(0);
    assert!(!gate.gated && gate.dropped.is_empty());
    assert_eq!(
        (gate.info["user_off"].as_bool(), gate.info["mode"].as_str()),
        (Some(true), Some(ECHO_OFF_FOR_RECORDING))
    );
}

fn said(t_ms: i64, text: &str) -> LiveWord {
    LiveWord {
        start_ms: t_ms,
        end_ms: t_ms + 200,
        text: text.into(),
        label: "me".into(),
        speaker_id: None,
    }
}

#[test]
fn far_end_words_arriving_after_the_mic_hide_its_copy_until_the_utterance_settles() {
    // `always`, the owner alone on the mic (nothing correlates): the text
    // layer alone. The mic utterance repeats the far end's words 0.3 s
    // later, but the loopback's revision reaches the gate after it.
    let secs = 24;
    let (far, _) = monologue(41, secs * SEC, db(-28.0));
    let (mut mic, _) = monologue(52, secs * SEC, db(-33.0));
    add_noise(&mut mic, 4, db(-60.0));
    let mut run = Run::new(EchoMode::Always, &mic, Some(&far));
    run.play(6);
    let texts = ["alpha", "bravo", "charlie", "delta"];
    let mic_words: Vec<LiveWord> = texts
        .iter()
        .enumerate()
        .map(|(k, t)| said(4_300 + 250 * k as i64, t))
        .collect();
    run.revise(0, 0, mic_words);
    assert_eq!(run.texts(0).len(), 4, "nothing known yet: all shown");
    let lb_words: Vec<LiveWord> = texts
        .iter()
        .enumerate()
        .map(|(k, t)| said(4_000 + 250 * k as i64, t))
        .collect();
    let updates = run.gate.loopback(0, 0, &lb_words);
    assert_eq!(updates.len(), 1, "the mic utterance is sent again");
    run.apply(updates);
    assert!(run.texts(0).is_empty(), "{:?}", run.texts(0));
    // Within the 3 s the decision can still change ...
    let updates = run.gate.loopback(0, 1, &[said(4_000, "echo")]);
    run.apply(updates);
    assert_eq!(run.texts(0).len(), 4);
    let updates = run.gate.loopback(0, 2, &lb_words);
    run.apply(updates);
    assert!(run.texts(0).is_empty());
    // ... after it, it is final.
    run.play(12);
    let updates = run.gate.loopback(0, 3, &[said(4_000, "echo")]);
    assert!(updates.is_empty(), "{updates:?}");
    assert!(run.texts(0).is_empty());
    // Stop decides with everything known: the far end's last revision
    // no longer repeats them.
    let gate = run.gate.finish(0);
    assert!(gate.skip.is_empty(), "{:?}", gate.dropped);
}
