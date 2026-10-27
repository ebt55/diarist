//! The layered decision on hand-made words (no audio).

use super::*;
use crate::audio::echo::text::norm;

const FLOOR: f32 = -62.0;
const OWNER: f32 = -42.0;

/// A word at `t` ms, 200 ms long, in utterance `group`.
fn w(t: i64, text: &str, group: u64, level: f32, far: bool, coef: Option<f32>) -> GateWord {
    GateWord {
        start_ms: t,
        end_ms: t + 200,
        norm: norm(text),
        group,
        score: coef.map_or(WordScore::Undefined, WordScore::Scored),
        presence: Some(Presence {
            level_db: level,
            far_active: far,
            attenuation_db: None,
        }),
    }
}

fn ctx(correlation: bool) -> Context {
    Context::new(
        Some(FLOOR),
        Some(OWNER),
        GateBand::DEFAULT,
        correlation,
        false,
        true,
    )
}

fn far(words: &[(i64, &str)]) -> FarIndex {
    FarIndex::new(words.iter().map(|(t, s)| (*t, norm(s))))
}

fn run(words: &[GateWord], far: &FarIndex, c: &Context) -> Vec<Option<EchoReason>> {
    let r = text_runs(words, far, TextWindow::DEFAULT);
    decide(words, &r, c)
}

#[test]
fn a_quiet_leak_goes_by_text_and_level_and_the_owner_stays() {
    // Headphones: the far end leaks in at the floor, the owner speaks loud.
    let f = far(&[
        (1_000, "we"),
        (1_250, "should"),
        (1_500, "ship"),
        (1_750, "today"),
        (4_000, "fine"),
    ]);
    let words = vec![
        w(1_350, "we", 1, -60.0, true, None),
        w(1_600, "should", 1, -59.0, true, None),
        w(1_850, "ship", 1, -60.0, true, None),
        // Misheard: no text match, but at the floor while the far end talks.
        w(2_100, "toady", 1, -60.5, true, None),
        // The owner, loud, while the far end talks; then alone.
        w(4_300, "fine", 2, -40.0, true, None),
        w(6_000, "thanks", 3, -41.0, false, None),
    ];
    let r = run(&words, &f, &ctx(false));
    assert_eq!(
        r,
        [
            Some(EchoReason::Text),
            Some(EchoReason::Text),
            Some(EchoReason::Text),
            Some(EchoReason::Level),
            None,
            None
        ]
    );
}

#[test]
fn the_level_layer_never_fires_near_the_owner_or_without_the_far_end() {
    let f = far(&[]);
    // A quiet owner, 8 dB over the floor: within 6 dB of their own level.
    let c = Context {
        owner_db: Some(FLOOR + 8.0),
        ..ctx(false)
    };
    let words = [w(1_000, "hmm", 1, FLOOR + 4.0, true, None)];
    assert_eq!(run(&words, &f, &c), [None]);
    // At the floor, but the far end is silent.
    let words = [w(1_000, "hmm", 1, FLOOR + 2.0, false, None)];
    assert_eq!(run(&words, &f, &ctx(false)), [None]);
    // The owner's level unknown: nothing fires.
    let c = Context {
        owner_db: None,
        ..ctx(false)
    };
    let words = [w(1_000, "hmm", 1, FLOOR + 2.0, true, None)];
    assert_eq!(run(&words, &f, &c), [None]);
}

#[test]
fn two_word_runs_need_the_tie_breaker() {
    let f = far(&[(1_000, "starlink"), (1_300, "coverage")]);
    let loud = [
        w(1_300, "starlink", 1, -40.0, true, None),
        w(1_600, "coverage", 1, -40.0, true, None),
    ];
    // Uncorrelated recording, the words are loud: the owner quoting.
    assert_eq!(run(&loud, &f, &ctx(false)), [None, None]);
    // At the floor: echo (by text, the level confirming).
    let quiet = [
        w(1_300, "starlink", 1, -60.0, false, None),
        w(1_600, "coverage", 1, -60.0, false, None),
    ];
    assert_eq!(
        run(&quiet, &f, &ctx(false)),
        [Some(EchoReason::Text), Some(EchoReason::Text)]
    );
    // Correlated recording: the coefficient decides.
    let corr = [
        w(1_300, "starlink", 1, -40.0, true, Some(0.45)),
        w(1_600, "coverage", 1, -40.0, true, Some(0.05)),
    ];
    assert_eq!(run(&corr, &f, &ctx(true)), [Some(EchoReason::Text), None]);
}

#[test]
fn the_utterance_rule_sweeps_small_pockets_next_to_echo_only() {
    let f = far(&[
        (1_000, "a1"),
        (1_200, "a2"),
        (1_400, "a3"),
        (1_600, "a4"),
        (1_800, "a5"),
    ]);
    let mut words = vec![
        w(1_300, "a1", 7, -45.0, true, None),
        w(1_500, "a2", 7, -45.0, true, None),
        w(1_700, "a3", 7, -45.0, true, None),
        // An unmatched fragment right after, quiet: swept.
        w(1_950, "uh", 7, -55.0, true, None),
        w(2_400, "a5x", 7, -52.0, true, None),
    ];
    let r = run(&words, &f, &ctx(false));
    assert_eq!(&r[..3], [Some(EchoReason::Text); 3]);
    assert_eq!(r[3], Some(EchoReason::Utterance));
    // A pocket of two: the last word goes with it.
    assert_eq!(r[4], Some(EchoReason::Utterance));
    // The owner's level shows: kept.
    words[3].presence.as_mut().unwrap().level_db = OWNER;
    words[4].presence.as_mut().unwrap().level_db = OWNER + 2.0;
    let r = run(&words, &f, &ctx(false));
    assert_eq!((r[3], r[4]), (None, None));
}

#[test]
fn the_utterance_rule_leaves_a_mostly_clean_utterance_alone() {
    let f = far(&[(1_000, "a1"), (1_200, "a2"), (1_400, "a3")]);
    let mut words = vec![
        w(1_300, "a1", 1, -45.0, true, None),
        w(1_500, "a2", 1, -45.0, true, None),
        w(1_700, "a3", 1, -45.0, true, None),
    ];
    for k in 0..4 {
        words.push(w(2_000 + 250 * k, &format!("o{k}"), 1, -55.0, true, None));
    }
    let r = run(&words, &f, &ctx(false));
    assert!(r[3..].iter().all(Option::is_none), "{r:?}");
}

#[test]
fn the_band_calibrates_from_text_matches_within_its_limits() {
    let mut s = CalibrationStats::default();
    let fixed = GateBand::DEFAULT;
    for k in 0..30 {
        let echo = w(k * 1_000, "e", 1, -45.0, true, Some(0.60 + 0.01 * k as f32));
        s.add(&echo, true, true);
    }
    // The owner: 20 of 30 words while the far end is silent, at OWNER + 1
    // and OWNER + 2; the others at OWNER over the far end.
    for k in 0..30 {
        let own = w(
            k * 1_000,
            "o",
            2,
            OWNER + (k % 3) as f32,
            k % 3 == 0,
            Some(0.01 * k as f32),
        );
        s.add(&own, false, false);
    }
    let level = s.owner(false, Some(FLOOR));
    assert_eq!((level.source, level.words), (OwnerSource::Quiet, 20));
    let owner = s.owner_db();
    assert_eq!(owner, level.db);
    assert!(
        owner.is_some_and(|o| (o - OWNER - 1.5).abs() < 0.51),
        "{owner:?}"
    );
    let c = s.band(owner, fixed, GATE_THRESHOLD_DEFAULT);
    assert!(c.calibrated);
    assert!((c.band.drop_at - 0.63).abs() < 0.011, "{:?}", c.band);
    assert!((c.band.keep_under - 0.26).abs() < 0.011, "{:?}", c.band);
    assert!(c.band.keep_under < c.band.threshold && c.band.threshold < c.band.drop_at);
    // Too few matches: the fixed band.
    let mut few = CalibrationStats::default();
    for k in 0..10 {
        few.add(&w(k, "e", 1, -45.0, true, Some(0.9)), true, true);
    }
    let c = few.band(owner, fixed, GATE_THRESHOLD_DEFAULT);
    assert!(!c.calibrated && c.band == fixed);
}

const GATE_THRESHOLD_DEFAULT: f32 = crate::audio::echo::GATE_THRESHOLD;

/// `x` as heard after the canceller, which cut it by `att` dB.
fn cut(mut x: GateWord, att: f32) -> GateWord {
    x.presence.as_mut().unwrap().attenuation_db = Some(att);
    x
}

/// Settled words in no text run.
fn stats(words: &[GateWord]) -> CalibrationStats {
    let mut s = CalibrationStats::default();
    for x in words {
        s.add(x, false, false);
    }
    s
}

#[test]
fn after_the_canceller_the_owners_level_comes_from_words_it_left_alone() {
    // A talk on speakers: 25 residue words in no run after the far end
    // stopped, cut 10 dB, 8 dB over the floor; 22 of the owner's, left alone.
    let mut words: Vec<GateWord> = (0..25)
        .map(|k| cut(w(k * 1_000, "r", 1, FLOOR + 8.0, false, None), 10.0))
        .collect();
    words.extend((0..22).map(|k| cut(w(k * 1_000 + 500, "o", 2, OWNER, false, None), 0.4)));
    let s = stats(&words);
    // Before the canceller's attenuation counts (as today): the residue.
    assert_eq!(s.owner_db(), Some(FLOOR + 8.0));
    let after = s.owner(true, Some(FLOOR));
    assert_eq!(
        after,
        OwnerLevel {
            db: Some(OWNER),
            source: OwnerSource::Quiet,
            words: 22,
            low: false,
            // The owner's 22, 20 dB over the floor; the cut residue is not
            // counted.
            high: 22,
        }
    );
    assert_eq!(
        s.owner_counts(),
        OwnerCounts {
            quiet: 47,
            unmatched: 47,
            clean_quiet: 22,
            clean: 22,
        }
    );
}

#[test]
fn the_owners_level_falls_back_in_order() {
    // After the canceller: 10 clean words with the far end silent, 15 over
    // it: the clean words; with fewer than 20 of those, unknown.
    let mut words: Vec<GateWord> = (0..10)
        .map(|k| cut(w(k * 1_000, "o", 1, OWNER, false, None), 0.2))
        .collect();
    words.extend((0..15).map(|k| cut(w(k * 1_000 + 500, "o", 2, OWNER + 2.0, true, None), 1.5)));
    let s = stats(&words);
    let l = s.owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db),
        (OwnerSource::Clean, 25, Some(OWNER + 2.0))
    );
    let l = stats(&words[..19]).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db, l.low),
        (OwnerSource::TooFew, 19, None, false)
    );
    // Cut words never count after the canceller, whatever their level.
    let loud: Vec<GateWord> = (0..30)
        .map(|k| cut(w(k * 1_000, "o", 1, OWNER, false, None), 3.0))
        .collect();
    let l = stats(&loud).owner(true, Some(FLOOR));
    assert_eq!((l.source, l.words), (OwnerSource::TooFew, 0));
    // Before the canceller: quiet words, else every word in no run.
    let raw: Vec<GateWord> = (0..25)
        .map(|k| w(k * 1_000, "o", 1, OWNER, k < 10, None))
        .collect();
    let l = stats(&raw).owner(false, Some(FLOOR));
    assert_eq!((l.source, l.words), (OwnerSource::Unmatched, 25));
    let l = stats(&raw[..19]).owner(false, Some(FLOOR));
    assert_eq!((l.source, l.words, l.db), (OwnerSource::TooFew, 19, None));
}

#[test]
fn an_owners_level_near_the_floor_is_unknown_after_the_canceller_only() {
    let at = |over: f32| -> Vec<GateWord> {
        (0..30)
            .map(|k| cut(w(k * 1_000, "x", 1, FLOOR + over, false, None), 0.5))
            .collect()
    };
    // Meeting 21's 35 clean words sat 3.3 dB over the floor: implausible.
    let l = stats(&at(3.3)).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db, l.low),
        (OwnerSource::Implausible, 30, None, false)
    );
    // Just under and at the plausibility margin, the presence margin.
    assert_eq!(OWNER_PLAUSIBLE_DB, PRESENCE_DB);
    let l = stats(&at(OWNER_PLAUSIBLE_DB - 0.1)).owner(true, Some(FLOOR));
    assert_eq!((l.source, l.db), (OwnerSource::Implausible, None));
    let l = stats(&at(OWNER_PLAUSIBLE_DB)).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.db, l.low),
        (OwnerSource::Quiet, Some(FLOOR + OWNER_PLAUSIBLE_DB), true)
    );
    // A quiet owner 6 to 15 dB over the floor is trusted and marked low;
    // from 15 dB on not low.
    let l = stats(&at(11.2)).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.db, l.low),
        (OwnerSource::Quiet, Some(FLOOR + 11.2), true)
    );
    let l = stats(&at(OWNER_LOW_DB - 0.1)).owner(true, Some(FLOOR));
    assert!(l.low && l.db.is_some());
    let l = stats(&at(OWNER_LOW_DB)).owner(true, Some(FLOOR));
    assert_eq!((l.db, l.low), (Some(FLOOR + OWNER_LOW_DB), false));
    // No floor to check against: the estimate stands, not low.
    let l = stats(&at(3.3)).owner(true, None);
    assert_eq!((l.db, l.low), (Some(FLOOR + 3.3), false));
    // Before the canceller the estimator is today's: no check, never low.
    let l = stats(&at(3.3)).owner(false, Some(FLOOR));
    assert_eq!(
        (l.source, l.db, l.low),
        (OwnerSource::Quiet, Some(FLOOR + 3.3), false)
    );
}

#[test]
fn an_implausible_set_falls_through_to_the_next() {
    // After the canceller: 25 words left alone at the floor while the far
    // end was silent (a silent owner's noise), and 40 of the owner's over
    // the far end, left alone, 25 dB over it.
    let mut words: Vec<GateWord> = (0..25)
        .map(|k| cut(w(k * 1_000, "n", 1, FLOOR + 2.0, false, None), 0.3))
        .collect();
    words.extend((0..40).map(|k| cut(w(k * 1_000 + 500, "o", 2, FLOOR + 25.0, true, None), 1.0)));
    let l = stats(&words).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db, l.low, l.high),
        (OwnerSource::Clean, 65, Some(FLOOR + 25.0), false, 40)
    );
    // Before the canceller nothing is high (plan 16 section 12.8).
    assert_eq!(stats(&words).owner(false, Some(FLOOR)).high, 0);
    // Every set with enough words implausible: the last one tried counts.
    let mut words: Vec<GateWord> = (0..25)
        .map(|k| cut(w(k * 1_000, "n", 1, FLOOR + 2.0, false, None), 0.3))
        .collect();
    words.extend((0..10).map(|k| cut(w(k * 1_000 + 500, "r", 2, FLOOR + 4.0, true, None), 1.0)));
    let l = stats(&words).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db, l.high),
        (OwnerSource::Implausible, 35, None, 0)
    );
    // The first set too small, the second implausible (meeting 21: 16 clean
    // words with the far end silent, 35 in all at +3.3 dB).
    let mut words: Vec<GateWord> = (0..16)
        .map(|k| cut(w(k * 1_000, "n", 1, FLOOR + 3.3, false, None), 0.3))
        .collect();
    words.extend((0..19).map(|k| cut(w(k * 1_000 + 500, "r", 2, FLOOR + 3.3, true, None), 1.0)));
    let l = stats(&words).owner(true, Some(FLOOR));
    assert_eq!(
        (l.source, l.words, l.db, l.high),
        (OwnerSource::Implausible, 35, None, 0)
    );
    assert_eq!(OwnerSource::Implausible.as_str(), "implausible");
    assert_eq!(OwnerSource::TooFew.as_str(), "too_few");
}

#[test]
fn high_counts_the_words_the_canceller_left_alone_high_over_the_floor() {
    // Ten words left alone at the margin, ten just under it, ten cut at the
    // margin: only the first ten count, whatever the estimate's source.
    let mut words: Vec<GateWord> = (0..10)
        .map(|k| cut(w(k * 1_000, "o", 1, FLOOR + OWNER_HIGH_DB, true, None), 1.0))
        .collect();
    words.extend((0..10).map(|k| {
        cut(
            w(
                k * 1_000 + 300,
                "q",
                1,
                FLOOR + OWNER_HIGH_DB - 0.1,
                true,
                None,
            ),
            1.0,
        )
    }));
    words.extend((0..10).map(|k| cut(w(k * 1_000 + 600, "r", 2, FLOOR + 25.0, true, None), 3.0)));
    let s = stats(&words);
    let l = s.owner(true, Some(FLOOR));
    assert_eq!((l.source, l.words, l.high), (OwnerSource::Clean, 20, 10));
    // Too few to estimate: still counted.
    let l = stats(&words[..10]).owner(true, Some(FLOOR));
    assert_eq!((l.source, l.high), (OwnerSource::TooFew, 10));
    // Without a floor, or before the canceller: 0.
    assert_eq!(s.owner(true, None).high, 0);
    assert_eq!(s.owner(false, Some(FLOOR)).high, 0);
    assert_eq!(OWNER_HIGH_DB, 18.0);
    // The gate reports it.
    let opts = LayerOptions {
        correlation: false,
        canceller: true,
        fixed: GateBand::DEFAULT,
        threshold: GateBand::DEFAULT.threshold,
        window: TextWindow::DEFAULT,
        calibrate: false,
    };
    let r = gate(&words, &far(&[]), Some(FLOOR), &opts);
    assert_eq!((r.owner_source, r.owner_high), (OwnerSource::Clean, 10));
    let r = gate(
        &words,
        &far(&[]),
        Some(FLOOR),
        &LayerOptions {
            canceller: false,
            ..opts
        },
    );
    assert_eq!(r.owner_high, 0);
}

#[test]
fn the_gate_reports_the_owners_level_and_its_source() {
    let f = far(&[]);
    let words: Vec<GateWord> = (0..30)
        .map(|k| cut(w(k * 1_000, "x", 1, FLOOR + 10.0, false, None), 9.0))
        .collect();
    let opts = |canceller: bool| LayerOptions {
        correlation: false,
        canceller,
        fixed: GateBand::DEFAULT,
        threshold: GateBand::DEFAULT.threshold,
        window: TextWindow::DEFAULT,
        calibrate: false,
    };
    let r = gate(&words, &f, Some(FLOOR), &opts(true));
    assert_eq!(
        (r.owner_source, r.owner_words, r.owner_db, r.floor_db),
        (OwnerSource::TooFew, 0, None, Some(FLOOR))
    );
    let r = gate(&words, &f, Some(FLOOR), &opts(false));
    assert_eq!(
        (r.owner_source, r.owner_words, r.owner_db, r.owner_low),
        (OwnerSource::Quiet, 30, Some(FLOOR + 10.0), false)
    );
    // Left alone by the canceller at +10: trusted and low.
    let alone: Vec<GateWord> = words.iter().cloned().map(|x| cut(x, 0.5)).collect();
    let r = gate(&alone, &f, Some(FLOOR), &opts(true));
    assert_eq!(
        (r.owner_source, r.owner_words, r.owner_db, r.owner_low),
        (OwnerSource::Quiet, 30, Some(FLOOR + 10.0), true)
    );
    // A tweak's owner's level is the one reported (the evaluation's).
    let r = gate_with(&words, &f, Some(FLOOR), &opts(false), &|c| {
        c.owner_db = None
    });
    assert_eq!((r.owner_source, r.owner_db), (OwnerSource::Quiet, None));
}

#[test]
fn without_evidence_or_correlation_nothing_is_decided() {
    // A headphone recording without a leak: one chance run and a word at
    // the floor, but nothing shows echo.
    let f = far(&[(1_000, "a1"), (1_200, "a2"), (1_400, "a3")]);
    let words = [
        w(1_300, "a1", 1, -45.0, true, None),
        w(1_500, "a2", 1, -45.0, true, None),
        w(1_700, "a3", 1, -45.0, true, None),
        w(4_000, "uh", 2, FLOOR + 1.0, true, None),
    ];
    let c = Context {
        evidence: false,
        ..ctx(false)
    };
    assert_eq!(run(&words, &f, &c), [None; 4]);
    // The share of matched words decides the evidence.
    assert!(!evidence(7, 100) && !evidence(8, 1_000) && evidence(8, 800));
}

#[test]
fn the_attenuation_layer_spares_words_near_the_owner() {
    let f = far(&[]);
    let mut c = ctx(true);
    c.canceller = true;
    c.attenuation = true;
    let att = |level: f32, a: f32| {
        let mut x = w(1_000, "word", 1, level, true, Some(0.1));
        x.presence.as_mut().unwrap().attenuation_db = Some(a);
        run(&[x], &f, &c)[0]
    };
    // Residual echo: far under the owner, most of it cancelled.
    assert_eq!(
        att(OWNER - 20.0, ATTENUATION_DB + 6.0),
        Some(EchoReason::Attenuation)
    );
    // The owner over the far end: some echo cancelled under the word.
    assert_eq!(att(OWNER - 3.0, ATTENUATION_DB + 6.0), None);
    // Not cancelled enough.
    assert_eq!(att(OWNER - 20.0, ATTENUATION_DB - 1.0), None);
}
