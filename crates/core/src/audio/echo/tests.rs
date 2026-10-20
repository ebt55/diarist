//! Echo monitor, profile and gate on synthetic speech-like signals (plan 15
//! section 4A): a coupled conversation, uncorrelated channels, −50 ppm
//! drift with clock maps (live) and on raw indices (offline, minute 100).

use super::synth::{add_delayed, add_noise, conversation, db, monologue, to_i16, Rng};
use super::*;
use crate::audio::pipeline::Level;

/// Direct NCC over every lag (the reference for the FFT path).
fn direct(x: &[f32], y: &[f32]) -> Vec<f32> {
    let ex: f64 = x.iter().map(|&v| f64::from(v).powi(2)).sum();
    (0..=y.len() - x.len())
        .map(|k| {
            let w = &y[k..k + x.len()];
            let ey: f64 = w.iter().map(|&v| f64::from(v).powi(2)).sum();
            let c: f64 = x
                .iter()
                .zip(w)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            if ey <= 0.0 {
                0.0
            } else {
                (c.abs() / (ex * ey).sqrt()) as f32
            }
        })
        .collect()
}

#[test]
fn fft_correlation_matches_the_direct_sum_at_sample_resolution() {
    let mut rng = Rng::new(3);
    for (n, lags, at) in [
        (3_000usize, 2_001usize, 700usize),
        (2_000, 8_081, 6_123),
        (32_000, 1_041, 61),
    ] {
        let y: Vec<f32> = (0..n + lags - 1).map(|_| rng.noise()).collect();
        let x: Vec<f32> = y[at..at + n]
            .iter()
            .map(|v| 0.5 * v + 0.2 * rng.noise())
            .collect();
        let (k, c) = Correlator::default().peak(&x, &y).unwrap();
        let d = direct(&x, &y);
        let (dk, dc) = d
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
        assert_eq!((k, dk), (at, at), "n {n} lags {lags}");
        assert!((c - dc).abs() < 1e-3, "{c} vs {dc}");
        assert!(c > 0.9, "{c}");
    }
    let mut corr = Correlator::default();
    // No loopback energy: undefined; a silent mic: 0.
    assert_eq!(corr.peak(&[0.0; 100], &[1.0; 150]), None);
    assert_eq!(corr.peak(&[1.0; 100], &[0.0; 150]), Some((0, 0.0)));
}

/// Loopback = far end + noise; mic = owner + the loopback 3 ms later at
/// −10 dB (`echo`) + noise. Far end −28 dBFS, owner −33 dBFS (5 dB over the
/// echo, as in meeting 12).
fn fixture(seed: u64, secs: usize, echo: bool) -> (Vec<i16>, Vec<i16>, super::synth::Conversation) {
    let n = secs * 16_000;
    let c = conversation(seed, n, db(-28.0), db(-33.0));
    let mut lb = c.far.clone();
    add_noise(&mut lb, seed + 1, db(-65.0));
    let mut mic = c.near.clone();
    if echo {
        add_delayed(&mut mic, &lb, 48, db(-10.0));
    }
    add_noise(&mut mic, seed + 2, db(-60.0));
    (to_i16(&mic), to_i16(&lb), c)
}

fn spans(words: &[(usize, usize)], shift: i64, rng: Option<&mut Rng>) -> Vec<(i64, i64)> {
    let mut rng = rng;
    words
        .iter()
        .map(|&(a, b)| {
            // The mic ASR stamps echo words late: 0 to 275 ms.
            let late = rng.as_mut().map_or(0, |r| (r.unit() * 4_400.0) as i64);
            (a as i64 + shift + late, b as i64 + shift + late)
        })
        .collect()
}

#[test]
fn coupled_conversation_drops_every_echo_word_and_keeps_the_owner() {
    let (mic, lb, c) = fixture(11, 120, true);
    let p = EchoProfile::scan_sources(&mic[..], &lb[..], None);
    assert!(p.coupled && p.status == EchoStatus::Coupled, "{p:?}");
    assert!(p.active_blocks >= 20, "{}", p.active_blocks);
    assert!(p.coupling > 0.6, "coupling {}", p.coupling);
    let lag = p.lag_ms.unwrap();
    assert!((lag - 3.0).abs() < 0.2, "lag {lag} ms");
    let erl = p.erl_db.unwrap();
    assert!((erl - 10.0).abs() < 1.5, "erl {erl} dB");

    let mut rng = Rng::new(5);
    let echo = spans(&c.far_words, 48, Some(&mut rng));
    let near = spans(&c.near_words, 0, None);
    let all: Vec<(i64, i64)> = echo.iter().chain(&near).copied().collect();
    let band = GateBand::DEFAULT;
    let g = gate_sources(&all, &mic[..], &lb[..], &p, band);
    let (e, o) = g.scores.split_at(echo.len());
    let coef = |s: &WordScore| s.coefficient().unwrap_or(0.0);
    let min_echo = e.iter().map(coef).fold(1.0, f32::min);
    let max_own = o.iter().map(coef).fold(0.0, f32::max);
    // Every echo word over the drop bound, every owner word under the keep
    // bound: the 0.3 margin is the hysteresis band between them.
    assert!(min_echo >= band.drop_at, "lowest echo word {min_echo}");
    assert!(max_own < band.keep_under, "highest owner word {max_own}");
    assert!(min_echo - max_own >= 0.3, "{min_echo} vs {max_own}");
    assert!(g.drop[..echo.len()].iter().all(|d| *d));
    assert!(g.drop[echo.len()..].iter().all(|d| !*d));
    assert_eq!(g.dropped, echo.len());
    assert!(
        echo.len() > 50 && near.len() > 30,
        "{} {}",
        echo.len(),
        near.len()
    );
}

#[test]
fn uncorrelated_channels_are_clear_and_nothing_drops() {
    let (mic, lb, c) = fixture(21, 120, false);
    let p = EchoProfile::scan_sources(&mic[..], &lb[..], None);
    assert!(!p.coupled && p.status == EchoStatus::Clear, "{p:?}");
    assert!(p.coupling < 0.2, "{}", p.coupling);
    // Gated anyway (`echo.mode = always`): nothing correlates.
    let all: Vec<(i64, i64)> = spans(&c.far_words, 48, None)
        .into_iter()
        .chain(spans(&c.near_words, 0, None))
        .collect();
    assert!(p.gates(EchoMode::Always, false) && !p.gates(EchoMode::Auto, false));
    assert!(!p.gates(EchoMode::Off, true) && p.gates(EchoMode::Auto, true));
    let g = gate_sources(&all, &mic[..], &lb[..], &p, GateBand::DEFAULT);
    assert_eq!(
        g.dropped,
        0,
        "{:?}",
        g.scores
            .iter()
            .filter_map(|s| s.coefficient())
            .fold(0.0, f32::max)
    );
    // A silent loopback: unknown, and every word's coefficient undefined.
    let zeros = vec![0i16; lb.len()];
    let p = EchoProfile::scan_sources(&mic[..], &zeros[..], None);
    assert_eq!((p.status, p.active_blocks), (EchoStatus::Unknown, 0));
    let g = gate_sources(&all, &mic[..], &zeros[..], &p, GateBand::DEFAULT);
    assert!(g.scores.iter().all(|s| *s == WordScore::Undefined) && g.dropped == 0);
}

#[test]
fn hysteresis_lets_the_neighbours_decide_the_middle_band() {
    let s = |v: &[Option<f32>]| -> Vec<WordScore> {
        v.iter()
            .map(|c| c.map_or(WordScore::Undefined, WordScore::Scored))
            .collect()
    };
    let band = GateBand {
        keep_under: 0.2,
        threshold: 0.35,
        drop_at: 0.5,
    };
    // 0.55 drops, 0.1 keeps, the 0.3 between echo words drops, the 0.3
    // between owner words stays; undefined is kept.
    let d = decide(
        &s(&[
            Some(0.7),
            Some(0.6),
            Some(0.3),
            Some(0.8),
            Some(0.55),
            None,
            Some(0.1),
        ]),
        band,
    );
    assert_eq!(d, vec![true, true, true, true, true, false, false]);
    let d = decide(
        &s(&[Some(0.05), Some(0.1), Some(0.3), Some(0.02), Some(0.6)]),
        band,
    );
    assert_eq!(d, vec![false, false, false, false, true]);
    // A lone middle word: its own coefficient against the threshold.
    assert_eq!(decide(&s(&[Some(0.4)]), band), vec![true]);
    assert_eq!(decide(&s(&[Some(0.3)]), band), vec![false]);
    assert_eq!(decide(&[WordScore::Pending], band), vec![false]);
    // The flags come back in the input order whatever order the words are in.
    let spans = [(30, 40), (10, 20), (20, 30)];
    let scores = s(&[Some(0.1), Some(0.9), Some(0.3)]);
    assert_eq!(
        decide_in_order(&spans, &scores, band),
        vec![false, true, true]
    );
}

#[test]
fn ring_reads_what_arrived_and_forgets_after_twenty_seconds() {
    let mut r = PcmRing::new(1_000);
    let mut out = Vec::new();
    r.write(100, &[1.0; 50], 3.0);
    assert_eq!(r.read(120, 40, &mut out), Avail::NotYet);
    assert_eq!(r.read(90, 60, &mut out), Avail::Ready);
    assert_eq!((out[9], out[10], out[59]), (0.0, 1.0, 1.0));
    // A gap is silence, an overlap overwrites.
    r.write(200, &[2.0; 10], 4.0);
    r.write(205, &[3.0; 10], 5.0);
    assert_eq!(r.read(150, 65, &mut out), Avail::Ready);
    assert_eq!(
        (out[0], out[49], out[50], out[55], out[64]),
        (0.0, 0.0, 2.0, 3.0, 3.0)
    );
    assert_eq!((r.gain_at(120), r.gain_at(206)), (3.0, 5.0));
    r.write(1_500, &[1.0; 10], 0.0);
    assert_eq!(r.read(100, 10, &mut out), Avail::Gone);
    assert_eq!(r.read(1_400, 110, &mut out), Avail::Ready);
}

#[test]
fn settings_forms() {
    assert_eq!(EchoMode::parse(" OFF "), EchoMode::Off);
    assert_eq!(EchoMode::parse("always"), EchoMode::Always);
    assert_eq!(EchoMode::parse("whatever"), EchoMode::Auto);
    assert_eq!(clamp_threshold(0.9), 0.6);
    assert_eq!(clamp_threshold(0.1), 0.2);
    assert_eq!(clamp_threshold(f32::NAN), GATE_THRESHOLD);
}

/// Frames of `pcm` from channel sample `start` (100 ms each).
fn frames(channel: Channel, start: u64, pcm: &[i16]) -> Vec<Frame> {
    pcm.chunks(1_600)
        .enumerate()
        .map(|(k, c)| Frame {
            channel,
            start_sample: start + k as u64 * 1_600,
            pts_us: (start + k as u64 * 1_600) * 1_000 / 16,
            pcm: c.to_vec(),
            level: Level::default(),
            gain_db: 0.0,
            raw_peak_dbfs: -100.0,
        })
        .collect()
}

/// Linear interpolation of `x` at fractional `pos` (0 outside).
fn at(x: &[f32], pos: f64) -> f32 {
    if pos < 0.0 {
        return 0.0;
    }
    let i = pos.floor() as usize;
    let f = (pos - i as f64) as f32;
    match (x.get(i), x.get(i + 1)) {
        (Some(a), Some(b)) => a + f * (b - a),
        (Some(a), None) => *a * (1.0 - f),
        _ => 0.0,
    }
}

#[test]
fn monitor_couples_holds_the_lag_through_four_hours_of_drift_and_releases() {
    // The mic clock runs −50 ppm: mic sample m is meeting time
    // m × (1 − 50e-6). The clock maps carry it, as the live anchors do.
    const PPM: f64 = -50e-6;
    let origin = 1_000_000_000i64;
    let hours = 4 * 3_600 + 120;
    let (mut mic_map, mut lb_map) = (ClockMap::new(origin), ClockMap::new(origin));
    for k in 0..=hours as i64 {
        lb_map.push(k * 16_000, origin + k * 10_000_000);
        mic_map.push(
            k * 16_000,
            origin + (k as f64 * 16_000.0 * 625.0 * (1.0 + PPM)).round() as i64,
        );
    }
    let clocks: SharedClocks = (Arc::new(Mutex::new(mic_map)), Arc::new(Mutex::new(lb_map)));
    let mut mon = EchoMonitor::new(COUPLED_AT, Some(clocks));
    assert_eq!(mon.state().status, EchoStatus::Unknown);
    let (far, _) = monologue(7, 24 * 16_000, db(-28.0));
    let mut noise = Rng::new(9);
    let mut first = None;
    let mut push_burst = |mon: &mut EchoMonitor, t0: u64, echo: bool| -> Vec<Option<EchoState>> {
        // 26 s around a 24 s far-end turn starting at meeting time t0 + 1 s.
        let lb: Vec<f32> = (0..26 * 16_000)
            .map(|i| at(&far, i as f64 - 16_000.0) + 1e-4 * noise.noise())
            .collect();
        let m0 = (t0 as f64 / (1.0 + PPM)).round() as u64;
        let mic: Vec<f32> = (0..26 * 16_000u64)
            .map(|i| {
                let tau = (m0 + i) as f64 * (1.0 + PPM) - t0 as f64 - 16_000.0 - 48.0;
                let e = if echo { db(-10.0) * at(&far, tau) } else { 0.0 };
                e + 1e-3 * noise.noise()
            })
            .collect();
        let (lf, mf) = (
            frames(Channel::Loopback, t0, &to_i16(&lb)),
            frames(Channel::Mic, m0, &to_i16(&mic)),
        );
        lf.iter()
            .zip(&mf)
            .flat_map(|(l, m)| [mon.push(l), mon.push(m)])
            .collect()
    };
    // Every 10 minutes for four hours.
    for b in 0..24u64 {
        let t0 = b * 600 * 16_000 + 32_000;
        let changes = push_burst(&mut mon, t0, true);
        if b == 0 {
            // Unknown, then clear at the first active block, then coupled.
            let seen: Vec<EchoStatus> = changes.iter().flatten().map(|s| s.status).collect();
            assert_eq!(seen, vec![EchoStatus::Clear, EchoStatus::Coupled]);
            first = changes.iter().flatten().last().copied();
        }
        let st = mon.state();
        assert_eq!(st.status, EchoStatus::Coupled, "burst {b}: {st:?}");
        let lag = st.lag_ms.unwrap();
        assert!((lag - 3.0).abs() < 0.3, "burst {b}: lag {lag} ms");
        // 10 dB, plus up to 2 dB the linear interpolation of the fractional
        // delay takes off the echo's highs.
        let erl = st.erl_db.unwrap();
        assert!((9.5..12.5).contains(&erl), "burst {b}: erl {erl}");
    }
    let first = first.expect("a state change in the first burst");
    assert_eq!(first.status, EchoStatus::Coupled);
    assert!(mon.coupled_at_ms().unwrap() < 30_000);
    // Headphones on: 60 s of far-end speech that never reaches the mic.
    let t0 = 24 * 600 * 16_000 + 32_000;
    let mut released = false;
    for k in 0..3u64 {
        released |= push_burst(&mut mon, t0 + k * 26 * 16_000, false)
            .iter()
            .flatten()
            .any(|s| s.status == EchoStatus::Clear);
    }
    assert!(released && mon.state().status == EchoStatus::Clear);
    assert!(mon.ever_coupled());
}

#[test]
fn raw_indices_without_clock_maps_still_gate_at_minute_one_hundred() {
    // −50 ppm on raw indices: the echo of loopback sample s lands at mic
    // sample (s + 48) / (1 − 50e-6), so the raw lag grows by 3 ms a minute
    // (303 ms at minute 100). 20 s of far-end speech every two minutes, the
    // owner talking for 6 s in between.
    const PPM: f64 = -50e-6;
    const PERIOD: u64 = 120 * 16_000;
    const TURN: u64 = 20 * 16_000;
    let periods = 51u64; // 102 minutes
    let (far, far_words) = monologue(31, TURN as usize, db(-28.0));
    let (own, own_words) = monologue(32, 6 * 16_000, db(-33.0));
    let (far_q, own_q) = (to_i16(&far), to_i16(&own));
    let far_at = |s: u64| -> f32 { far.get((s % PERIOD) as usize).copied().unwrap_or(0.0) };
    // Generated a period at a time (mostly zeros), so a debug build reads
    // 100 minutes in seconds.
    struct Periods<F: FnMut(u64) -> Vec<i16>> {
        make: F,
        next: u64,
        periods: u64,
        cur: Vec<i16>,
        pos: usize,
    }
    impl<F: FnMut(u64) -> Vec<i16>> PcmSource for Periods<F> {
        fn pull(&mut self, out: &mut Vec<i16>, max: usize) -> usize {
            let mut n = 0;
            while n < max {
                if self.pos == self.cur.len() {
                    if self.next == self.periods {
                        break;
                    }
                    self.cur = (self.make)(self.next);
                    self.next += 1;
                    self.pos = 0;
                }
                let k = (max - n).min(self.cur.len() - self.pos);
                out.extend_from_slice(&self.cur[self.pos..self.pos + k]);
                self.pos += k;
                n += k;
            }
            n
        }
    }
    let lb_period = |_: u64| {
        let mut v = vec![0i16; PERIOD as usize];
        v[..TURN as usize].copy_from_slice(&far_q);
        v
    };
    let mic_period = |p: u64| {
        let mut v = vec![0i16; PERIOD as usize];
        for (o, x) in v.iter_mut().enumerate().take(TURN as usize + 32_000) {
            let s = (p * PERIOD + o as u64) as f64 * (1.0 + PPM) - 48.0;
            if s >= 0.0 {
                let i = s.floor() as u64;
                let f = (s - i as f64) as f32;
                let e = far_at(i) * (1.0 - f) + far_at(i + 1) * f;
                *x = (db(-10.0) * e * 32_768.0).round() as i16;
            }
        }
        v[40 * 16_000..46 * 16_000].copy_from_slice(&own_q);
        v
    };
    fn periods_of<F: FnMut(u64) -> Vec<i16>>(make: F, periods: u64) -> Periods<F> {
        Periods {
            make,
            next: 0,
            periods,
            cur: Vec::new(),
            pos: 0,
        }
    }
    let p = EchoProfile::scan_sources(
        periods_of(mic_period, periods),
        periods_of(lb_period, periods),
        None,
    );
    assert!(
        p.coupled && p.status == EchoStatus::Coupled,
        "{:?}",
        (p.status, p.coupling)
    );
    let drift = p.drift_ms_per_min.unwrap();
    assert!((drift - 3.0).abs() < 0.2, "drift {drift} ms/min");
    // The words of the turn at minute 100 and the owner's after it.
    let base = 50 * PERIOD;
    let raw_lag = |s: u64| ((s as f64 + 48.0) / (1.0 + PPM) - s as f64) as i64;
    let echo: Vec<(i64, i64)> = far_words
        .iter()
        .map(|&(a, b)| {
            let (a, b) = (base + a as u64, base + b as u64);
            (a as i64 + raw_lag(a) + 1_200, b as i64 + raw_lag(b) + 1_200)
        })
        .collect();
    let owner: Vec<(i64, i64)> = own_words
        .iter()
        .map(|&(a, b)| {
            (
                (base + 40 * 16_000) as i64 + a as i64,
                (base + 40 * 16_000) as i64 + b as i64,
            )
        })
        .collect();
    let lag_ms = p.lag_at(echo[0].0).unwrap() / 16.0;
    assert!(
        (lag_ms - 303.0).abs() < 3.0,
        "lag at minute 100: {lag_ms} ms"
    );
    let all: Vec<(i64, i64)> = echo.iter().chain(&owner).copied().collect();
    let g = gate_sources(
        &all,
        periods_of(mic_period, periods),
        periods_of(lb_period, periods),
        &p,
        GateBand::DEFAULT,
    );
    assert!(
        g.drop[..echo.len()].iter().all(|d| *d),
        "{:?}",
        &g.scores[..echo.len()]
    );
    assert!(
        g.drop[echo.len()..].iter().all(|d| !*d),
        "{:?}",
        &g.scores[echo.len()..]
    );
}
