//! Acoustic echo: the far end reaching the microphone through the speakers
//! (plan 15, chunk A: `docs/plans/15-echo-without-headphones.md`).
//!
//! With the meeting on speakers the loopback reaches the mic a few
//! milliseconds later and about 10 dB down, and the mic transcript repeats
//! it as "me". The detector and the correlation layer are one statistic:
//! the normalised cross-correlation (NCC) of the loopback against the mic,
//! both band-passed to 300 to 3,400 Hz ([`SpeechBand`]; plan 15 section
//! 9.1, until round two a 100 Hz high-pass), computed by FFT at sample
//! resolution ([`Correlator`]: a 4,096-point block convolution) over a lag
//! window, on a common timeline (meeting time through the clock maps, or
//! raw indices without them). The mic word gate is layered on top of it
//! ([`layers`]: text matching against the loopback's words ([`text`]), the
//! near-end level and the canceller's attenuation ([`presence`]), the
//! correlation, an utterance rule).
//!
//! - [`EchoMonitor`] (live) keeps a 60 s high-passed PCM ring per channel on
//!   meeting time ([`PcmRing`], fed from the pipelines' non-blocking taps;
//!   960,000 f32 samples, 3.84 MB per channel, 7.7 MB for both) and judges
//!   2 s blocks ([`BlockDetector`]). Sixty seconds let the live gate score a
//!   word that a revision restates up to about a minute after it was said.
//! - [`EchoProfile::scan`] (post-pass) runs the same detector over the
//!   stored WAVs, so the post-pass never depends on the live monitor (crash
//!   recovery, re-runs). It returns the per-block lag track.
//! - [`score_spans`] scores a mic word for the correlation layer, for the
//!   post-pass and live: the NCC over `[start − 350 ms, end + 160 ms]` at
//!   the tracked lag ±20 ms; [`decide`] is its hysteresis, [`gate_words`]
//!   the correlation gate alone (chunk A's).
//!
//! The numbers behind the constants (meeting 12, 19 Oct 2026, 111 minutes,
//! speakers; plan 15 section 2; measured with the 100 Hz high-pass and kept
//! after the band-pass's re-measurement in docs/eval/2026-10-21-echo-round2.md:
//! echo blocks now median 0.78 on meeting 12 and 0.67 on meeting 14, the
//! nulls' p99 0.12 to 0.20, a headphone leak 0.17):
//!
//! - Loopback 2 s windows: speech at −35 to −25 dBFS, silence and noise at
//!   −50 or below: a block is active at −45 dBFS ([`ACTIVE_DBFS`]).
//! - Echo blocks: NCC median 0.72 (97.5 percent above 0.2); null (mic 7 s
//!   later): median 0.075, p99 0.21. On 100 ms frames the null reaches 0.35
//!   one frame in seven, so detection uses 2 s blocks ([`BLOCK`]) and a lag
//!   consistency count (echo 80 percent of blocks within ±3 ms of the tracked
//!   lag, null 6 percent): coupled at a median of at least 0.35 over the last
//!   10 active blocks with 7 of them consistent; released after 30 active
//!   blocks with the median under 0.2.
//! - Echo lag on meeting time 1 to 6 ms (median 3.3), flat; on raw indices
//!   10 to 346 ms, drifting 3.1 ms per minute (mic clock −51.6 ppm). Lags are
//!   causal (sidetone or "Listen to this device" puts the owner on the
//!   loopback with the mic leading). The peak that counts lies in −5 to
//!   +60 ms for the first [`NARROW_BLOCKS`] active blocks, then in −5 to
//!   +500 ms (Bluetooth and HDMI outputs add 100 to 300 ms) until a lock,
//!   then within ±25 ms of the tracked lag; a lock that [`LOST_BLOCKS`]
//!   active blocks in a row miss, or a device change, goes back to −5 to
//!   +500 ms and re-locks (an output switched mid-meeting, with or without
//!   the device-change event). One FFT yields every lag of the span it
//!   reads, so the windows only decide which peaks count; the
//!   lag-consistency rule (7 of 10 within ±3 ms) keeps a spurious peak in
//!   the wide window from coupling.
//! - The mic ASR stamps echo words late (+75 ms median, +275 ms p90), hence
//!   the asymmetric gate span. Word NCC: echo words median 0.74, the owner's
//!   words during far-end speech 0.06 (p90 0.23). Under full double-talk a
//!   word's NCC is about sqrt(echo share of the mic power): 0.47 for the
//!   owner 5 dB over the echo, so the band sits above that. Hysteresis
//!   ([`GateBand`]): keep under 0.20, drop at 0.60, in between the
//!   neighbours decide at `echo.gate_threshold` (0.50). Chosen from a sweep
//!   of 125 bands (docs/eval/2026-10-20-echo.md), the owner's words ranked
//!   over residual duplicates: meeting 12 duplicates dropped 98.97 percent,
//!   misrecognised echo during far-end speech 95.05, the owner's words kept
//!   98.84 (97.09 during far-end speech); the FLEURS double-talk fixture's
//!   "me" WER −0.68 points. Drop at 0.55 would take 0.07 points more
//!   duplicates for +0.25 on the fixture; the first band 0.20 / 0.35 / 0.50
//!   took 99.37 percent of duplicates, kept 98.50 of the owner's words and
//!   cost +15.91 points.
//! - Echo return loss median 10.3 dB; per coupled block
//!   `(loopback − loopback gain) − (mic − mic gain) − 20·log10(NCC)`, so the
//!   AGCs do not move it and the NCC term isolates the echo's share of the
//!   mic power.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio::capture::audio_err;
use crate::audio::clock::{ClockMap, NOMINAL_HNS_PER_SAMPLE};
use crate::audio::pipeline::Frame;
use crate::error::Result;
use crate::types::Channel;

/// Samples per millisecond (16 kHz).
const PER_MS: f64 = 16.0;
/// Detection block: 2 s.
pub const BLOCK: usize = 32_000;
/// A loopback block at or above this RMS (high-passed, after the AGC) is
/// examined.
pub const ACTIVE_DBFS: f32 = -45.0;
/// Median NCC of the last [`MEDIAN_BLOCKS`] active blocks for `coupled`.
pub const COUPLED_AT: f32 = 0.35;
pub const MEDIAN_BLOCKS: usize = 10;
/// Of the last [`MEDIAN_BLOCKS`] active blocks, this many must peak within
/// [`CONSISTENT_SAMPLES`] of the tracked lag.
pub const CONSISTENT_BLOCKS: usize = 7;
/// ±3 ms.
pub const CONSISTENT_SAMPLES: f32 = 48.0;
/// Coupling is released when the median stays under this ...
pub const RELEASE_BELOW: f32 = 0.2;
/// ... for this many active blocks.
pub const RELEASE_BLOCKS: u32 = 30;
/// A block's peak lag counts for the lag track at this NCC.
pub const LAG_CONFIDENT: f32 = 0.3;
/// Lag windows in samples relative to the timeline position: causal
/// acquisition (−5 to +60 ms), wide (−5 to +500 ms: no lock after
/// [`NARROW_BLOCKS`], a lost lock, a device change), and tracking (±25 ms
/// around the tracked lag).
pub const ACQUIRE_LAGS: (i64, i64) = (-80, 960);
pub const WIDE_LAGS: (i64, i64) = (-80, 8_000);
pub const TRACK_HALF: i64 = 400;
/// Active blocks acquired in the causal window before the wide one counts.
pub const NARROW_BLOCKS: u32 = 5;
/// Active blocks in a row that miss a lock (no confident peak within ±3 ms
/// of it) before the wide window counts again.
pub const LOST_BLOCKS: u32 = 3;
/// QPC time a clock-map segment runs past its samples before it is a jump
/// (a suspend): 10 ms. Drift leaves under 0.1 ms per one-second segment,
/// and a filled gap none.
const JUMP_HNS: f64 = 100_000.0;
/// Gate span around a word: 350 ms before, 160 ms after; lags ±20 ms.
pub const GATE_BEFORE: i64 = 5_600;
pub const GATE_AFTER: i64 = 2_560;
pub const GATE_LAG_HALF: i64 = 320;
/// Hysteresis ([`GateBand::DEFAULT`]): drop at or above, keep under; in
/// between the neighbours decide at `echo.gate_threshold`.
pub const GATE_DROP_AT: f32 = 0.7;
pub const GATE_KEEP_UNDER: f32 = 0.3;
/// `echo.gate_threshold` default and accepted range.
pub const GATE_THRESHOLD: f32 = 0.5;
pub const GATE_THRESHOLD_RANGE: (f32, f32) = (0.2, 0.6);
/// Live PCM ring per channel: 60 s (960,000 f32 samples, 3.84 MB).
pub const RING_SAMPLES: usize = 960_000;
/// High-pass corner of [`HighPass`] (the chunk A measurement's filter).
const HIGHPASS_HZ: f64 = 100.0;
/// [`SpeechBand`]'s corners: what the correlator, the presence levels and
/// the floor see.
pub const BAND_LOW_HZ: f64 = 300.0;
pub const BAND_HIGH_HZ: f64 = 3_400.0;
/// Coupled blocks whose return loss is kept for the median.
const ERL_BLOCKS: usize = 600;
/// Drift prediction on raw indices: confident blocks kept, and the span they
/// must cover (2 min).
const DRIFT_POINTS: usize = 64;
const DRIFT_SPAN: f64 = 1_920_000.0;
/// Below this NCC the return-loss estimate is not taken (the echo is not
/// what the block holds).
const ERL_MIN_COEF: f32 = 0.2;

// ---- settings-level types ---------------------------------------------------------

/// `echo.mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoMode {
    /// Engage on detected coupling.
    #[default]
    Auto,
    /// Nothing runs, nothing is gated.
    Off,
    /// Gate regardless of detection.
    Always,
}

impl EchoMode {
    /// `off` and `always` (any case); anything else is `auto`.
    pub fn parse(v: &str) -> Self {
        match v.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "always" => Self::Always,
            _ => Self::Auto,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Always => "always",
        }
    }
}

/// What the detector says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EchoStatus {
    /// No active loopback block yet: nothing is known.
    #[default]
    Unknown,
    Clear,
    Coupled,
}

/// The monitor's state (`coupling`: median NCC of the last 10 active
/// blocks; `lag_ms`: tracked lag, on meeting time with clock maps;
/// `erl_db`: median echo return loss of the coupled blocks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct EchoState {
    pub status: EchoStatus,
    pub coupling: f32,
    pub lag_ms: Option<f32>,
    pub erl_db: Option<f32>,
}

// ---- signal helpers ---------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// RBJ high-pass.
    fn highpass(f0: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * f0 / 16_000.0;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ low-pass.
    fn lowpass(f0: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * f0 / 16_000.0;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 4th-order Butterworth high-pass at 100 Hz (two biquads); output scaled
/// to ±1.
#[derive(Debug, Clone, Copy)]
pub struct HighPass([Biquad; 2]);

impl Default for HighPass {
    fn default() -> Self {
        Self([
            Biquad::highpass(HIGHPASS_HZ, 0.541_196_1),
            Biquad::highpass(HIGHPASS_HZ, 1.306_563),
        ])
    }
}

impl HighPass {
    /// Appends `pcm` filtered.
    pub fn extend(&mut self, pcm: &[i16], out: &mut Vec<f32>) {
        let settled = self
            .0
            .iter()
            .all(|b| b.z1.abs() < 1e-9 && b.z2.abs() < 1e-9);
        if settled && pcm.iter().all(|&s| s == 0) {
            for b in &mut self.0 {
                b.z1 = 0.0;
                b.z2 = 0.0;
            }
            out.resize(out.len() + pcm.len(), 0.0);
            return;
        }
        out.reserve(pcm.len());
        for &s in pcm {
            let a = self.0[0].run(f64::from(s) / 32_768.0);
            out.push(self.0[1].run(a) as f32);
        }
    }
}

/// The speech band every echo statistic is taken in (plan 15 section 9.1):
/// a 2nd-order Butterworth high-pass at 300 Hz and a 2nd-order low-pass at
/// 3,400 Hz, output scaled to ±1. Below 300 Hz the room's rumble and above
/// 3,400 Hz the jitter-decohered highs only add to the normalisation's
/// energies; band-limited, meeting 14's echo blocks correlate at 0.72 to
/// 0.80 instead of 0.50 to 0.57.
#[derive(Debug, Clone, Copy)]
pub struct SpeechBand([Biquad; 2]);

impl Default for SpeechBand {
    fn default() -> Self {
        Self([
            Biquad::highpass(BAND_LOW_HZ, std::f64::consts::FRAC_1_SQRT_2),
            Biquad::lowpass(BAND_HIGH_HZ, std::f64::consts::FRAC_1_SQRT_2),
        ])
    }
}

impl SpeechBand {
    /// Appends `pcm` filtered.
    pub fn extend(&mut self, pcm: &[i16], out: &mut Vec<f32>) {
        let settled = self
            .0
            .iter()
            .all(|b| b.z1.abs() < 1e-9 && b.z2.abs() < 1e-9);
        if settled && pcm.iter().all(|&s| s == 0) {
            for b in &mut self.0 {
                b.z1 = 0.0;
                b.z2 = 0.0;
            }
            out.resize(out.len() + pcm.len(), 0.0);
            return;
        }
        out.reserve(pcm.len());
        for &s in pcm {
            let a = self.0[0].run(f64::from(s) / 32_768.0);
            out.push(self.0[1].run(a) as f32);
        }
    }
}

/// RMS in dBFS of ±1 samples (−100 for silence).
pub fn rms_dbfs(x: &[f32]) -> f32 {
    if x.is_empty() {
        return -100.0;
    }
    let e: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    let ms = e / x.len() as f64;
    if ms <= 1e-10 {
        -100.0
    } else {
        (10.0 * ms.log10()) as f32
    }
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// A forward and an inverse real FFT of one size.
type FftPair = (Arc<dyn RealToComplex<f32>>, Arc<dyn ComplexToReal<f32>>);

/// Normalised cross-correlation by FFT at sample resolution: a block
/// convolution with at least 4,096 points (larger only for lag windows over
/// 2,048 samples), plans cached per size.
pub struct Correlator {
    planner: RealFftPlanner<f32>,
    plans: HashMap<usize, FftPair>,
    buf: Vec<f32>,
    sx: Vec<Complex<f32>>,
    sy: Vec<Complex<f32>>,
    acc: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    out: Vec<f32>,
    pre: Vec<f64>,
    /// |NCC| per lag of the last [`Self::correlate`].
    ncc: Vec<f32>,
}

impl Default for Correlator {
    fn default() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            plans: HashMap::new(),
            buf: Vec::new(),
            sx: Vec::new(),
            sy: Vec::new(),
            acc: Vec::new(),
            scratch: Vec::new(),
            out: Vec::new(),
            pre: Vec::new(),
            ncc: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Correlator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Correlator")
            .field("sizes", &self.plans.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Correlator {
    /// The peak |NCC| of `x` against `y` over the lags
    /// `0..=y.len() - x.len()` (`x[i]` against `y[i + k]`): `(k, coefficient)`.
    /// `None` when `x` has no energy (the coefficient is undefined); a
    /// silent `y` gives 0.
    pub fn peak(&mut self, x: &[f32], y: &[f32]) -> Option<(usize, f32)> {
        self.correlate(x, y)
            .then(|| self.peak_in(0, y.len() - x.len()))
    }

    /// The peak of the last [`Self::correlate`] over the lags `lo..=hi`
    /// (clamped to what was correlated): `(k, coefficient)`, the first of
    /// equal peaks; `(lo, 0)` when nothing was.
    pub fn peak_in(&self, lo: usize, hi: usize) -> (usize, f32) {
        let mut best = (lo, 0.0f32);
        for (k, &c) in self
            .ncc
            .iter()
            .enumerate()
            .take(hi.saturating_add(1))
            .skip(lo)
        {
            if c > best.1 {
                best = (k, c);
            }
        }
        best
    }

    /// The |NCC| of `x` against `y` at every lag `0..=y.len() - x.len()`,
    /// kept for [`Self::peak_in`] (one FFT pass serves several lag windows);
    /// false when `x` has no energy (undefined) or `y` is shorter than `x`.
    pub fn correlate(&mut self, x: &[f32], y: &[f32]) -> bool {
        self.ncc.clear();
        let n = x.len();
        if n == 0 || y.len() < n {
            return false;
        }
        let ex: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
        if ex <= 0.0 {
            return false;
        }
        let lags = y.len() - n + 1;
        self.pre.clear();
        self.pre.reserve(y.len() + 1);
        self.pre.push(0.0);
        let mut acc = 0.0f64;
        for &v in y {
            acc += f64::from(v) * f64::from(v);
            self.pre.push(acc);
        }
        let size = (2 * lags).next_power_of_two().max(4_096);
        let seg = size - lags + 1;
        let (r2c, c2r) = {
            let planner = &mut self.planner;
            self.plans
                .entry(size)
                .or_insert_with(|| {
                    (
                        planner.plan_fft_forward(size),
                        planner.plan_fft_inverse(size),
                    )
                })
                .clone()
        };
        let bins = size / 2 + 1;
        self.buf.resize(size, 0.0);
        self.sx.resize(bins, Complex::default());
        self.sy.resize(bins, Complex::default());
        self.acc.clear();
        self.acc.resize(bins, Complex::default());
        let scratch = r2c.get_scratch_len().max(c2r.get_scratch_len());
        self.scratch.resize(scratch, Complex::default());
        self.out.resize(size, 0.0);
        let mut s0 = 0;
        while s0 < n {
            let len = seg.min(n - s0);
            self.buf[..len].copy_from_slice(&x[s0..s0 + len]);
            self.buf[len..].fill(0.0);
            if r2c
                .process_with_scratch(&mut self.buf, &mut self.sx, &mut self.scratch)
                .is_err()
            {
                return false;
            }
            let ylen = (len + lags - 1).min(y.len() - s0);
            self.buf[..ylen].copy_from_slice(&y[s0..s0 + ylen]);
            self.buf[ylen..].fill(0.0);
            if r2c
                .process_with_scratch(&mut self.buf, &mut self.sy, &mut self.scratch)
                .is_err()
            {
                return false;
            }
            for ((a, p), q) in self.acc.iter_mut().zip(&self.sx).zip(&self.sy) {
                *a += p.conj() * q;
            }
            s0 += len;
        }
        self.acc[0].im = 0.0;
        self.acc[bins - 1].im = 0.0;
        if c2r
            .process_with_scratch(&mut self.acc, &mut self.out, &mut self.scratch)
            .is_err()
        {
            return false;
        }
        let scale = 1.0 / size as f64;
        let floor = ex * 1e-10;
        let (pre, out) = (&self.pre, &self.out);
        self.ncc.extend((0..lags).map(|k| {
            let ey = pre[k + n] - pre[k];
            if ey <= floor {
                0.0
            } else {
                (f64::from(out[k]).abs() * scale / (ex * ey).sqrt()).min(1.0) as f32
            }
        }));
        true
    }
}

// ---- audio on the common timeline -------------------------------------------------

/// Whether a span can be read yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    Ready,
    /// Not fully arrived (live): try again later.
    NotYet,
    /// Already overwritten (live ring) or never coming.
    Gone,
}

/// High-passed audio of both channels on one timeline (meeting samples
/// through the clock maps, or raw indices).
pub trait EchoAudio {
    /// Fills `out` with `len` samples of `channel` from timeline sample `start`.
    fn read(&mut self, channel: Channel, start: i64, len: usize, out: &mut Vec<f32>) -> Avail;
    /// AGC gain of `channel` at `t` (0 when unknown, e.g. stored WAVs).
    fn gain_db(&self, _channel: Channel, _t: i64) -> f32 {
        0.0
    }
}

/// A ring of high-passed samples on meeting time (live; [`RING_SAMPLES`]),
/// with the AGC gain of every frame.
#[derive(Debug, Clone)]
pub struct PcmRing {
    buf: Vec<f32>,
    /// Timeline sample of the next write (everything before it, back to
    /// `head - capacity`, is readable; never written means silence).
    head: i64,
    gains: VecDeque<(i64, f32)>,
}

impl Default for PcmRing {
    fn default() -> Self {
        Self::new(RING_SAMPLES)
    }
}

impl PcmRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity.max(1)],
            head: 0,
            gains: VecDeque::new(),
        }
    }

    pub fn head(&self) -> i64 {
        self.head
    }

    fn cap(&self) -> i64 {
        self.buf.len() as i64
    }

    fn slot(&self, t: i64) -> usize {
        t.rem_euclid(self.cap()) as usize
    }

    /// Writes `pcm` at timeline sample `t0` (gaps are silence; an overlap
    /// overwrites; a jump past the ring's length starts over).
    pub fn write(&mut self, t0: i64, pcm: &[f32], gain_db: f32) {
        let cap = self.cap();
        if t0 - self.head >= cap {
            self.buf.fill(0.0);
            self.head = t0;
        }
        while self.head < t0 {
            let i = self.slot(self.head);
            self.buf[i] = 0.0;
            self.head += 1;
        }
        let skip = (self.head - cap - t0).max(0) as usize;
        for (k, &v) in pcm.iter().enumerate().skip(skip) {
            let i = self.slot(t0 + k as i64);
            self.buf[i] = v;
        }
        self.head = self.head.max(t0 + pcm.len() as i64);
        self.gains.push_back((t0, gain_db));
        while self.gains.len() > 1 && self.gains[1].0 <= self.head - cap {
            self.gains.pop_front();
        }
    }

    pub fn read(&self, start: i64, len: usize, out: &mut Vec<f32>) -> Avail {
        if start + len as i64 > self.head {
            return Avail::NotYet;
        }
        if start < self.head - self.cap() {
            return Avail::Gone;
        }
        out.clear();
        out.extend((0..len as i64).map(|k| self.buf[self.slot(start + k)]));
        Avail::Ready
    }

    /// The gain of the frame that holds `t`.
    pub fn gain_at(&self, t: i64) -> f32 {
        self.gains
            .iter()
            .rev()
            .find(|(s, _)| *s <= t)
            .or(self.gains.front())
            .map_or(0.0, |g| g.1)
    }
}

/// The two live rings as [`EchoAudio`].
pub struct RingAudio<'a> {
    pub mic: &'a PcmRing,
    pub loopback: &'a PcmRing,
}

impl EchoAudio for RingAudio<'_> {
    fn read(&mut self, channel: Channel, start: i64, len: usize, out: &mut Vec<f32>) -> Avail {
        match channel {
            Channel::Mic => self.mic.read(start, len, out),
            _ => self.loopback.read(start, len, out),
        }
    }
    fn gain_db(&self, channel: Channel, t: i64) -> f32 {
        match channel {
            Channel::Mic => self.mic.gain_at(t),
            _ => self.loopback.gain_at(t),
        }
    }
}

/// One channel's samples, high-passed as they are read, kept from a little
/// before the last request (reads must move forward, as words and blocks do).
struct Stream<S> {
    src: S,
    hp: SpeechBand,
    raw: Vec<i16>,
    buf: Vec<f32>,
    base: i64,
    done: bool,
}

const STREAM_CHUNK: usize = 16_384;
/// Kept behind the newest request (requests may step back this much).
const STREAM_KEEP: i64 = 64_000;

impl<S: PcmSource> Stream<S> {
    fn new(src: S) -> Self {
        Self {
            src,
            hp: SpeechBand::default(),
            raw: Vec::with_capacity(STREAM_CHUNK),
            buf: Vec::new(),
            base: 0,
            done: false,
        }
    }

    fn end(&self) -> i64 {
        self.base + self.buf.len() as i64
    }

    fn fill_to(&mut self, end: i64) {
        while !self.done && self.end() < end {
            self.raw.clear();
            if self.src.pull(&mut self.raw, STREAM_CHUNK) < STREAM_CHUNK {
                self.done = true;
            }
            self.hp.extend(&self.raw, &mut self.buf);
        }
    }

    /// Whether any sample at or after `t` exists.
    fn has(&mut self, t: i64) -> bool {
        self.fill_to(t + 1);
        self.end() > t
    }

    fn read(&mut self, start: i64, len: usize, out: &mut Vec<f32>) {
        let end = start + len as i64;
        self.fill_to(end);
        out.clear();
        out.resize(len, 0.0);
        let (a, b) = (start.max(self.base), end.min(self.end()));
        if a < b {
            out[(a - start) as usize..(b - start) as usize]
                .copy_from_slice(&self.buf[(a - self.base) as usize..(b - self.base) as usize]);
        }
        let cut = start - STREAM_KEEP - self.base;
        if cut > 0 && cut as usize * 2 >= self.buf.len() {
            let cut = (cut as usize).min(self.buf.len());
            self.buf.drain(..cut);
            self.base += cut as i64;
        }
    }
}

/// The clock maps of both channels: timeline sample `t` (meeting time at
/// 16 kHz) is channel sample `map.sample_at(origin + t * 625)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockPair {
    pub mic: ClockMap,
    pub loopback: ClockMap,
}

impl ClockPair {
    /// Both maps, when each has at least two anchors.
    pub fn usable(mic: ClockMap, loopback: ClockMap) -> Option<Self> {
        (mic.len() >= 2 && loopback.len() >= 2).then_some(Self { mic, loopback })
    }

    fn channel_sample(&self, channel: Channel, t: i64) -> i64 {
        let map = match channel {
            Channel::Mic => &self.mic,
            _ => &self.loopback,
        };
        map.sample_at(map.origin_qpc as f64 + t as f64 * NOMINAL_HNS_PER_SAMPLE)
            .round() as i64
    }

    /// Whether timeline sample `t` falls inside a jump on either channel
    /// (QPC moved on with no samples: a suspend). Every timeline sample of a
    /// jump maps to the first channel sample after it.
    fn in_jump(&self, t: i64) -> bool {
        [&self.mic, &self.loopback].iter().any(|map| {
            let q = map.origin_qpc as f64 + t as f64 * NOMINAL_HNS_PER_SAMPLE;
            let pts = map.points();
            let i = pts.partition_point(|p| (p.1 as f64) <= q);
            i > 0 && i < pts.len() && {
                let (a, b) = (pts[i - 1], pts[i]);
                let sampled = a.1 as f64 + (b.0 - a.0) as f64 * NOMINAL_HNS_PER_SAMPLE;
                q > sampled + JUMP_HNS
            }
        })
    }

    /// Timeline sample of a channel sample.
    pub fn timeline(&self, channel: Channel, s: f64) -> f64 {
        let map = match channel {
            Channel::Mic => &self.mic,
            _ => &self.loopback,
        };
        (map.qpc_at(s) - map.origin_qpc as f64) / NOMINAL_HNS_PER_SAMPLE
    }
}

/// Stored channels as [`EchoAudio`] (each read in order, high-passed on the
/// way).
struct StreamAudio<I, J> {
    mic: Stream<I>,
    loopback: Stream<J>,
    clocks: Option<ClockPair>,
}

impl<I: PcmSource, J: PcmSource> EchoAudio for StreamAudio<I, J> {
    fn read(&mut self, channel: Channel, start: i64, len: usize, out: &mut Vec<f32>) -> Avail {
        let s = self
            .clocks
            .as_ref()
            .map_or(start, |c| c.channel_sample(channel, start));
        match channel {
            Channel::Mic => self.mic.read(s, len, out),
            _ => self.loopback.read(s, len, out),
        }
        Avail::Ready
    }
}

/// The samples of a 16 kHz mono PCM16 WAV, read in blocks.
pub struct WavSamples {
    r: BufReader<File>,
    left: u64,
    buf: Vec<u8>,
    pos: usize,
}

impl WavSamples {
    pub fn open(path: &Path) -> Result<Self> {
        let r = hound::WavReader::open(path)
            .map_err(|e| audio_err(format!("open {}: {e}", path.display())))?;
        let s = r.spec();
        if s.channels != 1 || s.bits_per_sample != 16 || s.sample_rate != 16_000 {
            return Err(audio_err(format!(
                "{} is not 16 kHz mono PCM16 ({s:?})",
                path.display()
            )));
        }
        let left = u64::from(r.duration());
        Ok(Self {
            r: r.into_inner(),
            left,
            buf: Vec::new(),
            pos: 0,
        })
    }
}

/// A forward-only source of 16 kHz PCM16 samples, read in blocks.
pub trait PcmSource {
    /// Appends up to `max` samples to `out`; returns how many (fewer only at
    /// the end).
    fn pull(&mut self, out: &mut Vec<i16>, max: usize) -> usize;
}

impl<P: PcmSource + ?Sized> PcmSource for Box<P> {
    fn pull(&mut self, out: &mut Vec<i16>, max: usize) -> usize {
        (**self).pull(out, max)
    }
}

impl PcmSource for &[i16] {
    fn pull(&mut self, out: &mut Vec<i16>, max: usize) -> usize {
        let k = max.min(self.len());
        out.extend_from_slice(&self[..k]);
        *self = &self[k..];
        k
    }
}

impl PcmSource for WavSamples {
    fn pull(&mut self, out: &mut Vec<i16>, max: usize) -> usize {
        let mut n = 0;
        while n < max {
            if self.pos + 2 > self.buf.len() {
                if self.left == 0 {
                    break;
                }
                let want = (self.left * 2).min(1 << 16) as usize;
                self.buf.resize(want, 0);
                let mut got = 0;
                while got < want {
                    match self.r.read(&mut self.buf[got..want]) {
                        Ok(0) => break,
                        Ok(k) => got += k,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                got &= !1;
                if got == 0 {
                    // A truncated file ends here.
                    self.left = 0;
                    break;
                }
                self.buf.truncate(got);
                self.pos = 0;
                self.left -= (got / 2) as u64;
            }
            let take = ((self.buf.len() - self.pos) / 2).min(max - n);
            out.extend(
                self.buf[self.pos..self.pos + 2 * take]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes(*b)),
            );
            self.pos += 2 * take;
            n += take;
        }
        n
    }
}

// ---- the 2 s block detector -------------------------------------------------------

/// The lag track (plan 15 section 3.1, re-acquisition included).
///
/// - Acquiring, the peak of the causal window (−5 to +60 ms) counts for the
///   first [`NARROW_BLOCKS`] active blocks, then the wide one (−5 to
///   +500 ms). The wide window is correlated all along (one FFT covers
///   both), so its recent peaks are at hand when it starts to count.
/// - A lock is the median of the last five confident lags of the window
///   that counts once three of them agree within ±3 ms; then ±25 ms around
///   it counts and the median keeps following it.
/// - A lock that [`LOST_BLOCKS`] active blocks in a row do not confirm (no
///   confident peak within ±3 ms: the output moved to HDMI or Bluetooth
///   speakers, or the echo stopped) goes back to the wide window, and
///   re-locks at once when three wide peaks of those blocks agree. A device
///   change starts over in the wide window.
/// - On raw indices (no clock maps) the older lags and the prediction are
///   moved forward with the drift fitted over the last 64 confident blocks
///   once they span two minutes, and the wide window sits on the predicted
///   lag.
#[derive(Debug, Clone, Default)]
struct LagTrack {
    raw: bool,
    /// Confident peaks `(t, lag)` of the window that counts, the last five.
    lags: VecDeque<(f64, f32)>,
    /// Confident wide-window peaks since the lock was last confirmed.
    wide_peaks: VecDeque<(f64, f32)>,
    center: Option<f32>,
    at: f64,
    locked: bool,
    /// Active blocks in a row that missed the lock.
    lost: u32,
    /// Active blocks acquired in the causal window.
    narrow: u32,
    /// The wide window counts.
    wide: bool,
    fit: VecDeque<(f64, f32)>,
    /// Blocks before a fresh lock that agree with it.
    backfill: Vec<f64>,
    ever_locked: bool,
    relocks: u32,
}

/// Appends to a window of the last five.
fn push_last5(v: &mut VecDeque<(f64, f32)>, p: (f64, f32)) {
    v.push_back(p);
    if v.len() > 5 {
        v.pop_front();
    }
}

impl LagTrack {
    fn drift(&self) -> Option<f64> {
        if !self.raw || self.fit.len() < 5 {
            return None;
        }
        let (t0, t1) = (self.fit.front()?.0, self.fit.back()?.0);
        if t1 - t0 < DRIFT_SPAN {
            return None;
        }
        let n = self.fit.len() as f64;
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for &(t, l) in &self.fit {
            let (x, y) = (t - t0, f64::from(l));
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let den = n * sxx - sx * sx;
        (den.abs() > f64::EPSILON).then(|| (n * sxy - sx * sy) / den)
    }

    /// The tracked lag predicted at `t`.
    fn predicted(&self, t: f64) -> Option<f32> {
        let c = self.center?;
        Some(c + self.drift().map_or(0.0, |d| (d * (t - self.at)) as f32))
    }

    /// The wide window at `t` (on raw indices around the predicted lag).
    fn wide_range(&self, t: f64) -> (i64, i64) {
        let base = if self.raw {
            self.predicted(t).map_or(0, |c| c.round() as i64)
        } else {
            0
        };
        (base + WIDE_LAGS.0, base + WIDE_LAGS.1)
    }

    /// The window whose peak counts at `t`.
    fn search(&self, t: f64) -> (i64, i64) {
        match self.predicted(t) {
            Some(c) if self.locked => {
                let c = c.round() as i64;
                (c - TRACK_HALF, c + TRACK_HALF)
            }
            _ if self.wide => self.wide_range(t),
            _ => ACQUIRE_LAGS,
        }
    }

    /// Whether a block's peak confirms the lock.
    fn confirms(&self, t: f64, lag: f32, coef: f32) -> bool {
        self.locked
            && coef >= LAG_CONFIDENT
            && self
                .predicted(t)
                .is_some_and(|c| (lag - c).abs() <= CONSISTENT_SAMPLES)
    }

    /// One active block: the peak of the window that counted, and the wide
    /// window's when it was correlated.
    fn observe(&mut self, t: f64, lag: f32, coef: f32, wide: Option<(f32, f32)>) {
        if let Some((l, c)) = wide {
            if c >= LAG_CONFIDENT {
                push_last5(&mut self.wide_peaks, (t, l));
            }
        }
        let was_locked = self.locked;
        if coef >= LAG_CONFIDENT {
            push_last5(&mut self.lags, (t, lag));
            self.try_lock(t);
            if self.locked {
                self.fit.push_back((t, lag));
                if self.fit.len() > DRIFT_POINTS {
                    self.fit.pop_front();
                }
            }
        }
        if was_locked {
            if self.confirms(t, lag, coef) {
                self.lost = 0;
                self.wide_peaks.clear();
            } else {
                self.lost += 1;
                if self.lost >= LOST_BLOCKS {
                    self.locked = false;
                    self.lost = 0;
                    if !self.raw {
                        self.center = None;
                    }
                    self.widen(t);
                }
            }
        } else if !self.locked && !self.wide {
            self.narrow += 1;
            if self.narrow >= NARROW_BLOCKS {
                self.widen(t);
            }
        }
    }

    /// The wide window counts from now, with its recent peaks.
    fn widen(&mut self, t: f64) {
        self.wide = true;
        self.lags = self.wide_peaks.clone();
        self.try_lock(t);
    }

    /// Locks, or follows the lock, when three of the last five lags agree.
    fn try_lock(&mut self, t: f64) {
        if self.lags.len() < 3 {
            return;
        }
        // On raw indices the older lags are moved forward by the drift.
        let d = self.drift().unwrap_or(0.0);
        let v: Vec<f32> = self
            .lags
            .iter()
            .map(|&(ti, l)| l + (d * (t - ti)) as f32)
            .collect();
        let Some(m) = median(&mut v.clone()) else {
            return;
        };
        let agree = |l: &f32| (*l - m).abs() <= CONSISTENT_SAMPLES;
        if v.iter().filter(|l| agree(l)).count() < 3 {
            return;
        }
        if !self.locked {
            // A step past the tracking window is a new path: the drift
            // fitted on the old one does not carry over.
            if self
                .predicted(t)
                .is_some_and(|p| (p - m).abs() > TRACK_HALF as f32)
            {
                self.fit.clear();
            }
            self.backfill = self
                .lags
                .iter()
                .zip(&v)
                .filter(|(_, l)| agree(l))
                .map(|(p, _)| p.0)
                .collect();
            self.relocks += u32::from(self.ever_locked);
            self.ever_locked = true;
            self.wide_peaks.clear();
            self.narrow = 0;
            self.lost = 0;
        }
        self.center = Some(m);
        self.at = t;
        self.locked = true;
        self.wide = false;
    }

    /// A device changed: the wide window counts, nothing carries over.
    fn restart(&mut self) {
        *self = Self {
            raw: self.raw,
            wide: true,
            ever_locked: self.ever_locked,
            relocks: self.relocks,
            ..Self::default()
        };
    }
}

/// One judged 2 s block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockResult {
    /// Timeline sample of the block start.
    pub t: i64,
    /// Loopback RMS (high-passed, after the AGC).
    pub loopback_dbfs: f32,
    pub active: bool,
    /// Peak NCC and its lag in samples (active blocks).
    pub coefficient: f32,
    pub lag: f32,
    /// Mic RMS over the block at the peak lag.
    pub mic_dbfs: f32,
    /// Echo return loss of a coupled block (pre-AGC).
    pub erl_db: Option<f32>,
    /// The tracked lag (samples), when the block's peak confirmed the lock.
    pub tracked: Option<f32>,
    pub status: EchoStatus,
}

/// The state machine over 2 s blocks, shared by [`EchoMonitor`] and
/// [`EchoProfile::scan`].
#[derive(Debug, Clone)]
pub struct BlockDetector {
    threshold: f32,
    track: LagTrack,
    recent: VecDeque<(f32, f32)>,
    low: u32,
    status: EchoStatus,
    erl: VecDeque<f32>,
    ever_coupled: bool,
    coupled_at: Option<i64>,
}

impl BlockDetector {
    /// `raw`: the timeline is raw channel indices (no clock maps), so the lag
    /// drifts and is predicted.
    pub fn new(threshold: f32, raw: bool) -> Self {
        Self {
            threshold,
            track: LagTrack {
                raw,
                ..LagTrack::default()
            },
            recent: VecDeque::new(),
            low: 0,
            status: EchoStatus::Unknown,
            erl: VecDeque::new(),
            ever_coupled: false,
            coupled_at: None,
        }
    }

    /// The lag window (samples) whose peak counts for a block at `t`.
    pub fn search(&self, t: i64) -> (i64, i64) {
        self.track.search(t as f64)
    }

    /// How far past a block at `t` its mic span reaches (samples): the
    /// counting window or the wide one, whichever ends later.
    pub fn reach(&self, t: i64) -> i64 {
        let t = t as f64;
        self.track.search(t).1.max(self.track.wide_range(t).1)
    }

    /// The lag track holds a lock.
    pub fn locked(&self) -> bool {
        self.track.locked
    }

    /// Fresh locks after the first: a lock that [`LOST_BLOCKS`] blocks
    /// missed found again (at a new lag after an output switch, or at the
    /// same one after a stretch the echo did not confirm), or after a device
    /// change.
    pub fn relocks(&self) -> u32 {
        self.track.relocks
    }

    /// The blocks before the newest fresh lock that agree with it
    /// (timeline sample, tracked lag), handed out once: the offline track
    /// then starts where the new lag did, not where it was confirmed.
    pub fn take_backfill(&mut self) -> Vec<(i64, f32)> {
        std::mem::take(&mut self.track.backfill)
            .into_iter()
            .filter_map(|t| self.track.predicted(t).map(|l| (t as i64, l)))
            .collect()
    }

    /// The tracked lag (samples) at `t`, if any.
    pub fn lag_at(&self, t: i64) -> Option<f32> {
        if self.track.locked || self.track.raw {
            self.track.predicted(t as f64)
        } else {
            None
        }
    }

    /// A device changed on either channel: wide search, lag track restarts.
    pub fn device_changed(&mut self) {
        self.track.restart();
    }

    pub fn status(&self) -> EchoStatus {
        self.status
    }

    pub fn ever_coupled(&self) -> bool {
        self.ever_coupled
    }

    /// Timeline sample of the first block that was judged coupled.
    pub fn coupled_at(&self) -> Option<i64> {
        self.coupled_at
    }

    pub fn state(&self) -> EchoState {
        let mut c: Vec<f32> = self.recent.iter().map(|r| r.0).collect();
        let mut e: Vec<f32> = self.erl.iter().copied().collect();
        EchoState {
            status: self.status,
            coupling: median(&mut c).unwrap_or(0.0),
            lag_ms: self.track.center.map(|l| (f64::from(l) / PER_MS) as f32),
            erl_db: median(&mut e),
        }
    }

    /// Judges one block: reads the loopback block at `t` and the mic over
    /// the lag window, correlates, updates the state. Acquiring, one
    /// correlation covers the counting window and the wide one; a block
    /// that misses a lock is also correlated over the wide window (the
    /// peaks re-acquisition needs).
    pub fn block(
        &mut self,
        audio: &mut dyn EchoAudio,
        corr: &mut Correlator,
        t: i64,
        x: &mut Vec<f32>,
        y: &mut Vec<f32>,
    ) -> Option<BlockResult> {
        if audio.read(Channel::Loopback, t, BLOCK, x) != Avail::Ready {
            return None;
        }
        let loopback_dbfs = rms_dbfs(x);
        let mut r = BlockResult {
            t,
            loopback_dbfs,
            active: loopback_dbfs >= ACTIVE_DBFS,
            coefficient: 0.0,
            lag: 0.0,
            mic_dbfs: -100.0,
            erl_db: None,
            tracked: None,
            status: self.status,
        };
        if !r.active {
            return Some(r);
        }
        let tf = t as f64;
        let (lo, hi) = self.track.search(tf);
        let (wlo, whi) = self.track.wide_range(tf);
        let locked = self.track.locked;
        let (rlo, rhi) = if locked {
            (lo, hi)
        } else {
            (lo.min(wlo), hi.max(whi))
        };
        if audio.read(Channel::Mic, t + rlo, BLOCK + (rhi - rlo) as usize, y) != Avail::Ready {
            return None;
        }
        let correlated = corr.correlate(x, y);
        let (k, c) = corr.peak_in((lo - rlo) as usize, (hi - rlo) as usize);
        r.coefficient = c;
        r.lag = (rlo + k as i64) as f32;
        r.mic_dbfs = rms_dbfs(&y[k..k + BLOCK]);
        let wide = if !locked {
            correlated.then(|| {
                let (k, c) = corr.peak_in((wlo - rlo) as usize, (whi - rlo) as usize);
                ((rlo + k as i64) as f32, c)
            })
        } else if self.track.confirms(tf, r.lag, c) {
            None
        } else {
            match audio.read(Channel::Mic, t + wlo, BLOCK + (whi - wlo) as usize, y) {
                Avail::Ready => corr.peak(x, y).map(|(k, c)| ((wlo + k as i64) as f32, c)),
                _ => None,
            }
        };
        self.observe(&mut r, wide, &*audio);
        Some(r)
    }

    fn observe(&mut self, r: &mut BlockResult, wide: Option<(f32, f32)>, audio: &dyn EchoAudio) {
        let tf = r.t as f64;
        self.track.observe(tf, r.lag, r.coefficient, wide);
        r.tracked = if self.track.confirms(tf, r.lag, r.coefficient) {
            self.track.predicted(tf)
        } else {
            None
        };
        self.recent.push_back((r.coefficient, r.lag));
        if self.recent.len() > MEDIAN_BLOCKS {
            self.recent.pop_front();
        }
        let mut c: Vec<f32> = self.recent.iter().map(|v| v.0).collect();
        let med = median(&mut c).unwrap_or(0.0);
        let consistent = self
            .track
            .center
            .filter(|_| self.track.locked)
            .map_or(0, |center| {
                self.recent
                    .iter()
                    .filter(|v| (v.1 - center).abs() <= CONSISTENT_SAMPLES)
                    .count()
            });
        self.status = match self.status {
            EchoStatus::Coupled => {
                if med < RELEASE_BELOW {
                    self.low += 1;
                } else {
                    self.low = 0;
                }
                if self.low >= RELEASE_BLOCKS {
                    EchoStatus::Clear
                } else {
                    EchoStatus::Coupled
                }
            }
            _ => {
                if self.recent.len() >= MEDIAN_BLOCKS
                    && med >= self.threshold
                    && consistent >= CONSISTENT_BLOCKS
                {
                    self.low = 0;
                    self.ever_coupled = true;
                    self.coupled_at.get_or_insert(r.t);
                    EchoStatus::Coupled
                } else {
                    EchoStatus::Clear
                }
            }
        };
        r.status = self.status;
        if self.status == EchoStatus::Coupled && r.coefficient >= ERL_MIN_COEF {
            let gain_lb = audio.gain_db(Channel::Loopback, r.t + BLOCK as i64 / 2);
            let gain_mic = audio.gain_db(Channel::Mic, r.t + BLOCK as i64 / 2);
            let erl = (r.loopback_dbfs - gain_lb)
                - (r.mic_dbfs - gain_mic)
                - 20.0 * r.coefficient.log10();
            r.erl_db = Some(erl);
            self.erl.push_back(erl);
            if self.erl.len() > ERL_BLOCKS {
                self.erl.pop_front();
            }
        }
    }
}

// ---- live monitor -----------------------------------------------------------------

/// The clocks of both channels while recording (mic, loopback).
pub type SharedClocks = (Arc<Mutex<ClockMap>>, Arc<Mutex<ClockMap>>);

/// [`ClockMap`]'s fitted slope (100 ns per sample), kept up to date as
/// anchors are appended: the map refits over every anchor on each
/// extrapolation (14,400 of them at 4 h, under the lock the pipeline also
/// takes to add anchors). The same sums in the same order: the same value.
#[derive(Debug, Default, Clone)]
struct SlopeFit {
    n: usize,
    first: (i64, i64),
    last: (i64, i64),
    sums: [f64; 4],
}

impl SlopeFit {
    fn slope(&mut self, pts: &[(i64, i64)]) -> f64 {
        let stale = pts.len() < self.n
            || (self.n > 0 && (pts[0] != self.first || pts[self.n - 1] != self.last));
        if stale {
            *self = Self::default();
        }
        let Some(&first) = pts.first() else {
            return NOMINAL_HNS_PER_SAMPLE;
        };
        self.first = first;
        let (x0, y0) = (first.0 as f64, first.1 as f64);
        let [sx, sy, sxx, sxy] = &mut self.sums;
        for &(x, y) in &pts[self.n..] {
            let (x, y) = (x as f64 - x0, y as f64 - y0);
            *sx += x;
            *sy += y;
            *sxx += x * x;
            *sxy += x * y;
        }
        self.n = pts.len();
        self.last = pts[pts.len() - 1];
        if self.last.0 - first.0 < 16_000 {
            return NOMINAL_HNS_PER_SAMPLE;
        }
        let n = self.n as f64;
        let den = n * *sxx - *sx * *sx;
        if den.abs() > f64::EPSILON {
            (n * *sxy - *sx * *sy) / den
        } else {
            NOMINAL_HNS_PER_SAMPLE
        }
    }
}

/// [`ClockMap::qpc_at`], extrapolating with the slope `fit` keeps.
fn qpc_cached(map: &ClockMap, fit: &mut SlopeFit, s: f64) -> f64 {
    let pts = map.points();
    match pts {
        [] => map.origin_qpc as f64 + s * NOMINAL_HNS_PER_SAMPLE,
        [p] => p.1 as f64 + (s - p.0 as f64) * fit.slope(pts),
        [first, .., last] => {
            if s <= first.0 as f64 {
                first.1 as f64 + (s - first.0 as f64) * fit.slope(pts)
            } else if s >= last.0 as f64 {
                last.1 as f64 + (s - last.0 as f64) * fit.slope(pts)
            } else {
                map.qpc_at(s)
            }
        }
    }
}

/// Where one channel's frames land on the timeline.
#[derive(Debug, Default)]
struct Placement {
    hp: SpeechBand,
    /// Timeline minus channel sample.
    offset: i64,
    frames: u32,
    next: Option<u64>,
    /// The offset holds below this channel sample: the next anchor after
    /// the last refresh (a jump or a gap starts a new segment at one).
    until: u64,
    /// Anchors the map had at the last refresh.
    seen: usize,
    fit: SlopeFit,
    scratch: Vec<f32>,
}

impl Placement {
    /// Refreshes the offset for the frame at channel sample `s` when it may
    /// sit elsewhere: every 10 frames, on a sample discontinuity, past the
    /// next anchor, or when anchors arrived. A `Jump` or a `Gap` anchors both
    /// its sides before its frames reach the tap, while `start_sample` runs
    /// on contiguously across a jump.
    fn place(&mut self, map: &ClockMap, s: u64) {
        let due = self.frames.is_multiple_of(10)
            || self.next != Some(s)
            || s >= self.until
            || map.len() != self.seen;
        if !due {
            return;
        }
        let sf = s as f64;
        let q = qpc_cached(map, &mut self.fit, sf);
        self.offset =
            ((q - map.origin_qpc as f64) / NOMINAL_HNS_PER_SAMPLE).round() as i64 - s as i64;
        let pts = map.points();
        self.until = pts
            .get(pts.partition_point(|p| p.0 as f64 <= sf))
            .map_or(u64::MAX, |p| p.0.max(0) as u64);
        self.seen = pts.len();
    }
}

/// One live mic word's gate score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WordScore {
    /// The padded span has not fully arrived: shown, scored again later.
    Pending,
    /// The loopback span is all zeros, or the audio is gone: kept.
    Undefined,
    Scored(f32),
}

impl WordScore {
    pub fn coefficient(self) -> Option<f32> {
        match self {
            Self::Scored(c) => Some(c),
            _ => None,
        }
    }
}

/// The live coupling monitor: both channels' frames from the pipeline taps
/// into a 60 s ring each on meeting time, 2 s blocks judged as they
/// complete, and the live word gate scored from the same rings.
#[derive(Debug)]
pub struct EchoMonitor {
    clocks: Option<SharedClocks>,
    mic: PcmRing,
    loopback: PcmRing,
    place: [Placement; 2],
    detector: BlockDetector,
    corr: Correlator,
    next_block: i64,
    x: Vec<f32>,
    y: Vec<f32>,
    blocks: u64,
    active_blocks: u64,
    skipped_blocks: u64,
    /// The mic's noise floor so far (loopback-silent frames of every judged
    /// block).
    floor: presence::FloorTracker,
    levels: Vec<f32>,
}

impl EchoMonitor {
    /// `clocks`: the shared clock maps (frames are placed on meeting time);
    /// `None` places them on their channel index.
    pub fn new(threshold: f32, clocks: Option<SharedClocks>) -> Self {
        let raw = clocks.is_none();
        Self {
            clocks,
            mic: PcmRing::default(),
            loopback: PcmRing::default(),
            place: Default::default(),
            detector: BlockDetector::new(threshold, raw),
            corr: Correlator::default(),
            next_block: 0,
            x: Vec::new(),
            y: Vec::new(),
            blocks: 0,
            active_blocks: 0,
            skipped_blocks: 0,
            floor: presence::FloorTracker::default(),
            levels: Vec::new(),
        }
    }

    pub fn state(&self) -> EchoState {
        self.detector.state()
    }

    /// The mic's noise floor so far (dBFS, speech band).
    pub fn floor_db(&self) -> Option<f32> {
        self.floor.floor_db()
    }

    /// Meeting time (ms) up to which the mic has arrived.
    pub fn mic_head_ms(&self) -> i64 {
        (self.mic.head() as f64 / PER_MS) as i64
    }

    pub fn ever_coupled(&self) -> bool {
        self.detector.ever_coupled()
    }

    /// Meeting time (ms) when coupling was first detected.
    pub fn coupled_at_ms(&self) -> Option<i64> {
        self.detector
            .coupled_at()
            .map(|t| (t as f64 / PER_MS) as i64)
    }

    /// Blocks judged, of them active, and blocks skipped (a channel stalled).
    pub fn counts(&self) -> (u64, u64, u64) {
        (self.blocks, self.active_blocks, self.skipped_blocks)
    }

    /// A device changed on either channel.
    pub fn device_changed(&mut self) {
        self.detector.device_changed();
    }

    /// The tracked lag in samples (`None`: not locked).
    pub fn lag(&self) -> Option<f32> {
        self.detector.lag_at(self.next_block)
    }

    /// Lag re-acquisitions so far (see [`BlockDetector::relocks`]).
    pub fn relocks(&self) -> u32 {
        self.detector.relocks()
    }

    /// Adds one 100 ms frame of either channel; returns the new state when
    /// the status changed.
    pub fn push(&mut self, frame: &Frame) -> Option<EchoState> {
        let idx = match frame.channel {
            Channel::Mic => 0,
            Channel::Loopback => 1,
            Channel::Source => return None,
        };
        let p = &mut self.place[idx];
        if let Some((mic, lb)) = &self.clocks {
            let map = if idx == 0 { mic } else { lb };
            p.place(
                &map.lock().unwrap_or_else(|e| e.into_inner()),
                frame.start_sample,
            );
        }
        p.frames = p.frames.wrapping_add(1);
        p.next = Some(frame.start_sample + frame.pcm.len() as u64);
        p.scratch.clear();
        p.hp.extend(&frame.pcm, &mut p.scratch);
        let t0 = frame.start_sample as i64 + p.offset;
        let ring = if idx == 0 {
            &mut self.mic
        } else {
            &mut self.loopback
        };
        ring.write(t0, &p.scratch, frame.gain_db);
        self.run_blocks()
    }

    fn run_blocks(&mut self) -> Option<EchoState> {
        let before = self.detector.status();
        let mut changed = None;
        loop {
            let t = self.next_block;
            if self.loopback.head() < t + BLOCK as i64 {
                break;
            }
            // A block that fell out of the ring (a stalled mic) is skipped.
            if t < self.loopback.head() - RING_SAMPLES as i64 + WIDE_LAGS.1 {
                self.next_block = self.loopback.head() - BLOCK as i64;
                self.next_block -= self.next_block.rem_euclid(BLOCK as i64);
                self.skipped_blocks += 1;
                continue;
            }
            if self.mic.head() < t + BLOCK as i64 + self.detector.reach(t) {
                // Wait for the mic unless the loopback is far ahead.
                if self.loopback.head() - self.mic.head() < (RING_SAMPLES / 2) as i64 {
                    break;
                }
            }
            if self.mic.read(t, BLOCK, &mut self.x) == Avail::Ready
                && self.loopback.read(t, BLOCK, &mut self.y) == Avail::Ready
            {
                self.floor.push(&self.x, &self.y);
            }
            let mut audio = RingAudio {
                mic: &self.mic,
                loopback: &self.loopback,
            };
            match self
                .detector
                .block(&mut audio, &mut self.corr, t, &mut self.x, &mut self.y)
            {
                Some(r) => {
                    self.blocks += 1;
                    self.active_blocks += u64::from(r.active);
                }
                None => self.skipped_blocks += 1,
            }
            self.next_block += BLOCK as i64;
            if self.detector.status() != before {
                changed = Some(self.detector.state());
            }
        }
        changed
    }

    /// The presence of live mic words (timeline spans in samples) from the
    /// rings ([`presence::presence`]); `Err(NotYet)` for a span not fully
    /// arrived, `Err(Gone)` for one overwritten.
    pub fn presence_words(
        &mut self,
        spans: &[(i64, i64)],
    ) -> Vec<std::result::Result<presence::Presence, Avail>> {
        let mut audio = RingAudio {
            mic: &self.mic,
            loopback: &self.loopback,
        };
        let mut x = std::mem::take(&mut self.x);
        let mut levels = std::mem::take(&mut self.levels);
        let out = spans
            .iter()
            .map(|&s| presence::presence(&mut audio, None, s, &mut x, &mut levels))
            .collect();
        self.x = x;
        self.levels = levels;
        out
    }

    /// Scores live mic words (timeline spans in samples, i.e. meeting ms ×
    /// 16) from the rings with [`gate_words`]' scoring.
    pub fn score_words(&mut self, spans: &[(i64, i64)]) -> Vec<WordScore> {
        let lag = self.lag();
        let mut audio = RingAudio {
            mic: &self.mic,
            loopback: &self.loopback,
        };
        let mut x = std::mem::take(&mut self.x);
        let mut y = std::mem::take(&mut self.y);
        let out = spans
            .iter()
            .map(|&s| {
                score_word(
                    &mut audio,
                    &mut self.corr,
                    s,
                    lag,
                    GatePad::DEFAULT,
                    &mut x,
                    &mut y,
                )
            })
            .collect();
        self.x = x;
        self.y = y;
        out
    }
}

// ---- offline profile --------------------------------------------------------------

/// The detector run over a stored recording (2 s blocks, FFT correlation).
#[derive(Debug, Clone, PartialEq)]
pub struct EchoProfile {
    /// Status after the last block.
    pub status: EchoStatus,
    /// Coupled at some point (what the post-pass gates on).
    pub coupled: bool,
    /// Median NCC of the active blocks.
    pub coupling: f32,
    /// Median tracked lag (ms) of the confident blocks.
    pub lag_ms: Option<f32>,
    /// Drift of the lag (ms per minute; about 0 on meeting time).
    pub drift_ms_per_min: Option<f32>,
    /// Median return loss of the coupled blocks; the WAVs are after the AGC,
    /// so this reads the post-AGC levels (the live monitor strips the gains).
    pub erl_db: Option<f32>,
    pub blocks: usize,
    pub active_blocks: usize,
    /// Timeline ms of the first coupled block.
    pub coupled_at_ms: Option<i64>,
    /// Per block that confirmed the lock, and the blocks a fresh lock
    /// agrees with before it: (timeline sample, tracked lag in samples).
    pub track: Vec<(i64, f32)>,
    /// Lag re-acquisitions ([`BlockDetector::relocks`]); meeting 12 has 13
    /// in 111 minutes, its track never leaving 0.7 to 7.2 ms.
    pub relocks: usize,
    /// The timeline is meeting time through these maps (else raw indices).
    pub clocks: Option<ClockPair>,
    /// The peak coefficient of every active block, in time order (the
    /// evaluation's distributions).
    pub active: Vec<f32>,
    /// The track's drift slope on raw indices (0 on meeting time), for
    /// [`Self::lag_at`].
    slope: f64,
}

impl EchoProfile {
    /// Scans two 16 kHz mono WAVs (`clocks`: their maps, when usable).
    pub fn scan(mic_wav: &Path, loopback_wav: &Path, clocks: Option<ClockPair>) -> Result<Self> {
        Ok(Self::scan_sources(
            WavSamples::open(mic_wav)?,
            WavSamples::open(loopback_wav)?,
            clocks,
        ))
    }

    /// Scans two sample sources (slices of the stereo FLAC's channels,
    /// synthetic tests).
    pub fn scan_sources(
        mic: impl PcmSource,
        loopback: impl PcmSource,
        clocks: Option<ClockPair>,
    ) -> Self {
        let mut det = BlockDetector::new(COUPLED_AT, clocks.is_none());
        let mut audio = StreamAudio {
            mic: Stream::new(mic),
            loopback: Stream::new(loopback),
            clocks: clocks.clone(),
        };
        let mut corr = Correlator::default();
        let (mut x, mut y) = (Vec::new(), Vec::new());
        let mut coefs = Vec::new();
        let mut active = Vec::new();
        let mut track = Vec::new();
        let mut blocks = 0usize;
        let mut erl = Vec::new();
        for k in 0i64.. {
            let t = k * BLOCK as i64;
            // A block inside a suspend would judge the first samples after it
            // once more (the whole jump maps there).
            if audio.clocks.as_ref().is_some_and(|c| c.in_jump(t)) {
                continue;
            }
            let s = audio
                .clocks
                .as_ref()
                .map_or(t, |c| c.channel_sample(Channel::Loopback, t));
            if !audio.loopback.has(s + BLOCK as i64 - 1) {
                break;
            }
            let Some(r) = det.block(&mut audio, &mut corr, t, &mut x, &mut y) else {
                break;
            };
            blocks += 1;
            if !r.active {
                continue;
            }
            coefs.push(r.coefficient);
            active.push(r.coefficient);
            if let Some(e) = r.erl_db {
                erl.push(e);
            }
            // A fresh lock also covers the earlier blocks that agree with it
            // (after an output switch, from the switch on).
            track.extend(det.take_backfill());
            if let Some(l) = r.tracked {
                track.push((t, l));
            }
        }
        track.sort_by_key(|p| p.0);
        track.dedup_by_key(|p| p.0);
        let active_blocks = coefs.len();
        let mut lags: Vec<f32> = track.iter().map(|p| p.1).collect();
        let drift = fit_slope(&track)
            .filter(|_| track.last().map_or(0, |l| l.0) - track[0].0 >= DRIFT_SPAN as i64);
        Self {
            status: det.status(),
            coupled: det.ever_coupled(),
            coupling: median(&mut coefs).unwrap_or(0.0),
            lag_ms: median(&mut lags).map(|l| (f64::from(l) / PER_MS) as f32),
            drift_ms_per_min: drift.map(|d| (d * 960_000.0 / PER_MS) as f32),
            erl_db: median(&mut erl),
            blocks,
            active_blocks,
            coupled_at_ms: det.coupled_at().map(|t| (t as f64 / PER_MS) as i64),
            relocks: det.relocks() as usize,
            slope: if clocks.is_none() {
                fit_slope(&track).unwrap_or(0.0)
            } else {
                0.0
            },
            track,
            clocks,
            active,
        }
    }

    /// The tracked lag (samples) at timeline sample `t`: linear between the
    /// track's points, except across a step the gate's ±20 ms cannot cover
    /// (an output switched): there the lag of the block that holds `t`, the
    /// nearer one across blocks neither covers. Past the track's ends the
    /// end value (plus the fitted drift on raw indices). `None` without a
    /// track.
    pub fn lag_at(&self, t: i64) -> Option<f32> {
        let tr = &self.track;
        let (first, last) = (tr.first()?, tr.last()?);
        let carried = |p: &(i64, f32)| p.1 + (self.slope * (t - p.0) as f64) as f32;
        if t <= first.0 {
            return Some(carried(first));
        }
        if t >= last.0 {
            return Some(carried(last));
        }
        let i = tr.partition_point(|p| p.0 <= t);
        let (a, b) = (tr[i - 1], tr[i]);
        let step = f64::from(b.1 - a.1) - self.slope * (b.0 - a.0) as f64;
        if step.abs() > GATE_LAG_HALF as f64 {
            let edge = ((a.0 + b.0 + BLOCK as i64) / 2).min(b.0);
            return Some(if t < edge { a.1 } else { b.1 });
        }
        let f = (t - a.0) as f32 / (b.0 - a.0).max(1) as f32;
        Some(a.1 + f * (b.1 - a.1))
    }

    /// Whether the post-pass gates: coupled, a canceller ran (`aec`), or
    /// `echo.mode = always`.
    pub fn gates(&self, mode: EchoMode, aec: bool) -> bool {
        mode != EchoMode::Off && (self.coupled || aec || mode == EchoMode::Always)
    }

    /// Timeline sample of a channel time in ms (the post-pass maps words to
    /// meeting time; without clock maps the timeline is channel time).
    pub fn timeline_of_ms(&self, channel: Channel, ms: i64) -> i64 {
        let s = ms as f64 * PER_MS;
        self.clocks
            .as_ref()
            .map_or(s, |c| c.timeline(channel, s))
            .round() as i64
    }
}

/// Least-squares slope of (t, lag).
fn fit_slope(points: &[(i64, f32)]) -> Option<f64> {
    if points.len() < 5 {
        return None;
    }
    let t0 = points[0].0 as f64;
    let n = points.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for &(t, l) in points {
        let (x, y) = (t as f64 - t0, f64::from(l));
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let den = n * sxx - sx * sx;
    (den.abs() > f64::EPSILON).then(|| (n * sxy - sx * sy) / den)
}

// ---- the word gate ----------------------------------------------------------------

/// The padding of a word's correlation span (samples before its start and
/// after its end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatePad {
    pub before: i64,
    pub after: i64,
}

impl GatePad {
    pub const DEFAULT: Self = Self {
        before: GATE_BEFORE,
        after: GATE_AFTER,
    };
}

impl Default for GatePad {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Scores one mic word (timeline samples of the mic): the loopback that
/// would have produced it (the padded span `[start − before, end + after]`
/// moved back by `lag`) against the mic at `lag` ±20 ms (the causal
/// acquisition window without a lag).
fn score_word(
    audio: &mut dyn EchoAudio,
    corr: &mut Correlator,
    (start, end): (i64, i64),
    lag: Option<f32>,
    pad: GatePad,
    x: &mut Vec<f32>,
    y: &mut Vec<f32>,
) -> WordScore {
    let base = lag.map_or(0, |l| l.round() as i64);
    let p0 = start - pad.before - base;
    let p1 = end.max(start) + pad.after - base;
    let len = (p1 - p0) as usize;
    let (lo, hi) = match lag {
        Some(_) => (base - GATE_LAG_HALF, base + GATE_LAG_HALF),
        None => ACQUIRE_LAGS,
    };
    match audio.read(Channel::Loopback, p0, len, x) {
        Avail::Ready => {}
        Avail::NotYet => return WordScore::Pending,
        Avail::Gone => return WordScore::Undefined,
    }
    match audio.read(Channel::Mic, p0 + lo, len + (hi - lo) as usize, y) {
        Avail::Ready => {}
        Avail::NotYet => return WordScore::Pending,
        Avail::Gone => return WordScore::Undefined,
    }
    match corr.peak(x, y) {
        Some((_, c)) => WordScore::Scored(c),
        None => WordScore::Undefined,
    }
}

/// The gate's hysteresis band: a word is kept under `keep_under`, dropped
/// at or above `drop_at`, and in between dropped when its neighbours' median
/// reaches `threshold` (`echo.gate_threshold`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateBand {
    pub keep_under: f32,
    pub threshold: f32,
    pub drop_at: f32,
}

impl GateBand {
    pub const DEFAULT: Self = Self {
        keep_under: GATE_KEEP_UNDER,
        threshold: GATE_THRESHOLD,
        drop_at: GATE_DROP_AT,
    };

    /// The default bounds around `threshold`.
    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            threshold,
            ..Self::DEFAULT
        }
    }
}

impl Default for GateBand {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The hysteresis over a word sequence in time order: drop at or above
/// `band.drop_at`, keep under `band.keep_under`, in between drop when the
/// median of the two neighbouring words on each side (those with a
/// coefficient; the word's own without any) is at or above `band.threshold`.
/// Words without a coefficient are kept.
pub fn decide(scores: &[WordScore], band: GateBand) -> Vec<bool> {
    (0..scores.len())
        .map(|i| match scores[i].coefficient() {
            None => false,
            Some(c) if c >= band.drop_at => true,
            Some(c) if c < band.keep_under => false,
            Some(c) => {
                let mut nb: Vec<f32> = [i.wrapping_sub(2), i.wrapping_sub(1), i + 1, i + 2]
                    .iter()
                    .filter_map(|&j| scores.get(j).and_then(|s| s.coefficient()))
                    .collect();
                median(&mut nb).unwrap_or(c) >= band.threshold
            }
        })
        .collect()
}

/// [`decide`] over words given in any order (`spans` decide the time order);
/// the flags in the input order.
pub fn decide_in_order(spans: &[(i64, i64)], scores: &[WordScore], band: GateBand) -> Vec<bool> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i]);
    let sorted: Vec<WordScore> = order.iter().map(|&i| scores[i]).collect();
    let mut drop = vec![false; spans.len()];
    for (k, d) in decide(&sorted, band).into_iter().enumerate() {
        drop[order[k]] = d;
    }
    drop
}

/// What [`gate_words`] decided, in the order of the input spans.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateReport {
    pub scores: Vec<WordScore>,
    pub drop: Vec<bool>,
    pub dropped: usize,
    pub kept: usize,
}

/// Scores every span (timeline samples, i.e. meeting ms × 16 with clock
/// maps) against `audio` at the lag `lag_at` gives for its time, in time
/// order (stored channels are read forward); the scores in input order.
pub fn score_spans(
    spans: &[(i64, i64)],
    audio: &mut dyn EchoAudio,
    lag_at: &dyn Fn(i64) -> Option<f32>,
    pad: GatePad,
) -> Vec<WordScore> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i]);
    let mut corr = Correlator::default();
    let (mut x, mut y) = (Vec::new(), Vec::new());
    let mut scores = vec![WordScore::Undefined; spans.len()];
    for &i in &order {
        let s = spans[i];
        scores[i] = score_word(audio, &mut corr, s, lag_at(s.0), pad, &mut x, &mut y);
    }
    scores
}

/// The correlation gate alone (chunk A's): [`score_spans`], then [`decide`]
/// over the spans in time order.
pub fn gate_words(
    spans: &[(i64, i64)],
    audio: &mut dyn EchoAudio,
    lag_at: &dyn Fn(i64) -> Option<f32>,
    band: GateBand,
) -> GateReport {
    let scores = score_spans(spans, audio, lag_at, GatePad::DEFAULT);
    let drop = decide_in_order(spans, &scores, band);
    let dropped = drop.iter().filter(|d| **d).count();
    GateReport {
        kept: spans.len() - dropped,
        dropped,
        scores,
        drop,
    }
}

/// [`gate_words`] over stored channels, on the profile's timeline and lag
/// track.
pub fn gate_sources(
    spans: &[(i64, i64)],
    mic: impl PcmSource,
    loopback: impl PcmSource,
    profile: &EchoProfile,
    band: GateBand,
) -> GateReport {
    let mut audio = StreamAudio {
        mic: Stream::new(mic),
        loopback: Stream::new(loopback),
        clocks: profile.clocks.clone(),
    };
    gate_words(spans, &mut audio, &|t| profile.lag_at(t), band)
}

/// [`score_spans`] over stored channels, on the profile's timeline and lag
/// track.
pub fn score_sources(
    spans: &[(i64, i64)],
    mic: impl PcmSource,
    loopback: impl PcmSource,
    profile: &EchoProfile,
    pad: GatePad,
) -> Vec<WordScore> {
    let mut audio = StreamAudio {
        mic: Stream::new(mic),
        loopback: Stream::new(loopback),
        clocks: profile.clocks.clone(),
    };
    score_spans(spans, &mut audio, &|t| profile.lag_at(t), pad)
}

/// [`gate_sources`] over the two WAVs.
pub fn gate_wavs(
    spans: &[(i64, i64)],
    mic_wav: &Path,
    loopback_wav: &Path,
    profile: &EchoProfile,
    band: GateBand,
) -> Result<GateReport> {
    Ok(gate_sources(
        spans,
        WavSamples::open(mic_wav)?,
        WavSamples::open(loopback_wav)?,
        profile,
        band,
    ))
}

/// `echo.gate_threshold` clamped to the accepted range.
pub fn clamp_threshold(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(GATE_THRESHOLD_RANGE.0, GATE_THRESHOLD_RANGE.1)
    } else {
        GATE_THRESHOLD
    }
}

pub mod layers;
pub mod presence;
pub mod text;

#[cfg(test)]
pub(crate) mod synth;

#[cfg(test)]
mod tests;
