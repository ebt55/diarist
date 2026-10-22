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
    for k in 0..30 {
        let own = w(
            k * 1_000,
            "o",
            2,
            OWNER + (k % 3) as f32,
            k % 2 == 0,
            Some(0.01 * k as f32),
        );
        s.add(&own, false, false);
    }
    let owner = s.owner_db();
    assert!(
        owner.is_some_and(|o| (o - OWNER - 1.0).abs() < 1.01),
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
