//! The post-pass canceller on synthetic recordings: a two-party
//! conversation (`audio::echo::synth`), the far end into the mic through a
//! small room 10 dB down, the owner 5 dB over the echo, a −70 dBFS floor.

use super::*;
use crate::audio::echo::synth::{add_noise, conversation, db, to_i16};
use crate::types::{Channel, ClockAnchor, MeetingId};

const ORIGIN: i64 = 1_000_000_000;

/// The room: a 3 ms direct path 10 dB down, three reflections and a
/// decaying tail to 120 ms.
fn room() -> Vec<f32> {
    let mut h = vec![0.0f32; 2_000];
    h[48] = db(-10.0);
    h[130] = 0.4 * db(-10.0);
    h[260] = -0.25 * db(-10.0);
    h[410] = 0.15 * db(-10.0);
    let mut seed = 0x1234_5678u32;
    for (i, v) in h.iter_mut().enumerate().skip(500) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let n = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
        *v += 0.02 * db(-10.0) * n * (-((i - 500) as f32) / 400.0).exp();
    }
    h
}

fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    let mut y = vec![0.0f32; x.len()];
    let taps: Vec<(usize, f32)> = h
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0.0)
        .map(|(i, v)| (i, *v))
        .collect();
    for (i, o) in y.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for &(k, g) in &taps {
            if k <= i {
                acc += g * x[i - k];
            }
        }
        *o = acc;
    }
    y
}

fn hp(x: &[f32]) -> Vec<f32> {
    let mut y = x.to_vec();
    HighPass::default().run(&mut y);
    y
}

fn span_energy(x: &[f32], spans: &[(usize, usize)], from: usize) -> f64 {
    spans
        .iter()
        .filter(|s| s.0 >= from)
        .map(|&(a, b)| energy(&x[a..b.min(x.len())]))
        .sum()
}

fn db_ratio(a: f64, b: f64) -> f64 {
    10.0 * (a / b).log10()
}

/// A stored loopback with an AGC that raises the far end by 0 to 18 dB in
/// a slow pattern, and its gain track.
fn agc(raw: &[f32]) -> (Vec<f32>, GainTrack) {
    let values: Vec<f32> = (0..raw.len().div_ceil(160))
        .map(|k| {
            let t = k as f32 / 100.0;
            (9.0 + 9.0 * (t * 0.31).sin()).max(0.0)
        })
        .collect();
    let track = GainTrack::from_values(values);
    let mut stored = raw.to_vec();
    track.apply(0, &mut stored);
    (stored, track)
}

fn run(
    mic: &[i16],
    lb: &[i16],
    clocks: Option<(&ClockMap, &ClockMap)>,
    gain: Option<&GainTrack>,
) -> (Vec<f32>, OfflineReport) {
    let mut opts = OfflineOptions::post_pass(clocks);
    opts.loopback_gain = gain;
    let mut out = Vec::new();
    let report = cancel_recording(&mic, &lb, &opts, &mut |b| {
        out.extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    assert_eq!(out.len(), mic.len());
    (out, report)
}

#[test]
fn removes_20_db_of_echo_and_leaves_the_owner_alone() {
    let n = 50 * 16_000;
    let c = conversation(71, n, db(-26.0), db(-31.0));
    let (stored, track) = agc(&c.far);
    let echo = convolve(&c.far, &room());
    let mut mic: Vec<f32> = echo.iter().zip(&c.near).map(|(e, v)| e + v).collect();
    add_noise(&mut mic, 72, db(-70.0));
    let (mic, lb) = (to_i16(&mic), to_i16(&stored));
    let (out, report) = run(&mic, &lb, None, Some(&track));
    let mic_h = hp(&mic
        .iter()
        .map(|&v| f32::from(v) / 32_768.0)
        .collect::<Vec<_>>());
    let out_h = hp(&out);
    // Far end only (each word plus the room's tail), after 10 s.
    let far: Vec<(usize, usize)> = c.far_words.iter().map(|w| (w.0, w.1 + 2_000)).collect();
    let erle = db_ratio(
        span_energy(&mic_h, &far, 160_000),
        span_energy(&out_h, &far, 160_000),
    );
    assert!(erle >= 20.0, "ERLE {erle:.1} dB, {report:?}");
    // The owner alone: unchanged within 0.5 dB.
    let near_change = db_ratio(
        span_energy(&out_h, &c.near_words, 160_000),
        span_energy(&mic_h, &c.near_words, 160_000),
    );
    eprintln!("ERLE {erle:.1} dB, owner {near_change:+.2} dB, {report:?}");
    assert!(near_change.abs() <= 0.5, "owner {near_change:+.2} dB");
    // The label-free figure agrees, and the lag was found (3 ms).
    let e = f64::from(report.erle_db.unwrap());
    assert!(
        e >= 15.0 && (e - erle).abs() < 8.0,
        "{e:.1} against {erle:.1}"
    );
    assert!((report.lag_ms.unwrap() - 3.0).abs() < 0.2, "{report:?}");
    assert!(report.far_blocks >= 10 && report.far_blocks <= report.active_blocks);
    assert_eq!(report.stats.resets, 0);
}

/// Anchors once a second for a clock running `ppm` slow against QPC.
fn map(channel: Channel, frames: usize, ppm: f64) -> ClockMap {
    let anchors: Vec<ClockAnchor> = (0..=frames / 16_000)
        .map(|k| ClockAnchor {
            meeting_id: MeetingId(1),
            channel,
            sample_index: (k * 16_000) as i64,
            qpc_100ns: ORIGIN + ((k * 16_000) as f64 * 625.0 * (1.0 + ppm * 1e-6)).round() as i64,
        })
        .collect();
    ClockMap::from_anchors(ORIGIN, &anchors)
}

#[test]
fn follows_a_drifting_loopback_through_the_clock_maps() {
    // The loopback clock runs 50 ppm slow: 3 ms over the minute, as on
    // meeting 12 (-51.6 ppm). The far end exists on the mic's clock; the
    // loopback samples it at its own.
    let n = 60 * 16_000;
    let c = conversation(81, n, db(-26.0), db(-31.0));
    let interp = SincInterpolator::default();
    let ratio = 1.0 + 50e-6;
    let lb_raw: Vec<f32> = (0..n)
        .map(|s| interp.at(&c.far, s as f64 * ratio))
        .collect();
    let (stored, track) = agc(&lb_raw);
    let echo = convolve(&c.far, &room());
    let mut mic: Vec<f32> = echo.iter().zip(&c.near).map(|(e, v)| e + v).collect();
    add_noise(&mut mic, 82, db(-70.0));
    let (mic, lb) = (to_i16(&mic), to_i16(&stored));
    let mic_map = map(Channel::Mic, n, 0.0);
    let lb_map = map(Channel::Loopback, n, 50.0);
    let (out, report) = run(&mic, &lb, Some((&mic_map, &lb_map)), Some(&track));
    let mic_h = hp(&mic
        .iter()
        .map(|&v| f32::from(v) / 32_768.0)
        .collect::<Vec<_>>());
    let out_h = hp(&out);
    let far: Vec<(usize, usize)> = c.far_words.iter().map(|w| (w.0, w.1 + 2_000)).collect();
    let erle = db_ratio(
        span_energy(&mic_h, &far, 30 * 16_000),
        span_energy(&out_h, &far, 30 * 16_000),
    );
    assert!(
        erle >= 20.0,
        "ERLE {erle:.1} dB in the second half, {report:?}"
    );
    let near_change = db_ratio(
        span_energy(&out_h, &c.near_words, 160_000),
        span_energy(&mic_h, &c.near_words, 160_000),
    );
    eprintln!("drift: ERLE {erle:.1} dB, owner {near_change:+.2} dB, {report:?}");
    assert!(near_change.abs() <= 0.5, "owner {near_change:+.2} dB");
}

#[test]
fn a_detector_lag_far_outside_the_default_search_is_followed() {
    // An output with 250 ms of latency: the default search (-4 to +16 ms)
    // cannot see it; the detector's lag centres the search.
    let n = 40 * 16_000;
    let c = conversation(91, n, db(-26.0), db(-31.0));
    let mut h = room();
    h.splice(0..0, std::iter::repeat_n(0.0, 4_000 - 48));
    let echo = convolve(&c.far, &h);
    let mut mic: Vec<f32> = echo.iter().zip(&c.near).map(|(e, v)| e + v).collect();
    add_noise(&mut mic, 92, db(-70.0));
    let (mic, lb) = (to_i16(&mic), to_i16(&c.far));
    let hint = |_t: i64| Some(4_002.0f32);
    let mut opts = OfflineOptions::post_pass(None);
    opts.lag_hint = Some(&hint);
    let mut out = Vec::new();
    let (m, l) = (&mic[..], &lb[..]);
    let report = cancel_recording(&m, &l, &opts, &mut |b| {
        out.extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    assert!((report.lag_ms.unwrap() - 250.0).abs() < 0.5, "{report:?}");
    let mic_h = hp(&mic
        .iter()
        .map(|&v| f32::from(v) / 32_768.0)
        .collect::<Vec<_>>());
    let out_h = hp(&out);
    let far: Vec<(usize, usize)> = c
        .far_words
        .iter()
        .map(|w| (w.0 + 4_000, w.1 + 6_000))
        .collect();
    let erle = db_ratio(
        span_energy(&mic_h, &far, 160_000),
        span_energy(&out_h, &far, 160_000),
    );
    eprintln!("250 ms: ERLE {erle:.1} dB, {report:?}");
    assert!(erle >= 20.0, "ERLE {erle:.1} dB");
}

#[test]
fn wavs_in_and_out_and_a_cancelled_run_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let n = 12 * 16_000 + 123;
    let c = conversation(61, n, db(-26.0), db(-31.0));
    let echo = convolve(&c.far, &room());
    let mic: Vec<f32> = echo.iter().zip(&c.near).map(|(e, v)| e + v).collect();
    for (name, x) in [("mic.wav", &mic), ("loopback.wav", &c.far)] {
        let mut w = crate::audio::wav::WavSink::create(&dir.path().join(name)).unwrap();
        w.write(&to_i16(x)).unwrap();
        w.finalize().unwrap();
    }
    let (m, l, o) = (
        dir.path().join("mic.wav"),
        dir.path().join("loopback.wav"),
        dir.path().join("mic-aec.wav"),
    );
    // The file reader is the slice reader.
    let f = WavFile::open(&m).unwrap();
    let (mut a, mut b) = (vec![0.0f32; 500], vec![0.0f32; 500]);
    f.read(n as i64 - 300, &mut a).unwrap();
    let pcm = to_i16(&mic);
    (&pcm[..]).read(n as i64 - 300, &mut b).unwrap();
    assert_eq!(a, b);
    assert!(a[300..].iter().all(|&v| v == 0.0), "zero past the end");
    let r = cancel_wavs(&m, &l, &o, &OfflineOptions::post_pass(None)).unwrap();
    assert_eq!(r.frames, n as u64);
    assert_eq!(hound::WavReader::open(&o).unwrap().duration(), n as u32);
    std::fs::remove_file(&o).unwrap();
    let mut opts = OfflineOptions::post_pass(None);
    opts.cancelled = &|| true;
    assert!(matches!(
        cancel_wavs(&m, &l, &o, &opts),
        Err(CoreError::Cancelled)
    ));
    assert!(!o.exists());
}
