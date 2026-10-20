//! Acoustic echo: the far end reaching the microphone through the speakers
//! (plan 15, chunk A: `docs/plans/15-echo-without-headphones.md`).
//!
//! With the meeting on speakers the loopback reaches the mic a few
//! milliseconds later and about 10 dB down, and the mic transcript repeats
//! it as "me". Everything here is one statistic: the normalised
//! cross-correlation (NCC) of the loopback against the mic, both high-passed
//! at 100 Hz, computed by FFT at sample resolution ([`Correlator`]: a
//! 4,096-point block convolution) over a lag window, on a common timeline
//! (meeting time through the clock maps, or raw indices without them).
//!
//! - [`EchoMonitor`] (live) keeps a 20 s high-passed PCM ring per channel on
//!   meeting time ([`PcmRing`], fed from the pipelines' non-blocking taps)
//!   and judges 2 s blocks ([`BlockDetector`]).
//! - [`EchoProfile::scan`] (post-pass) runs the same detector over the
//!   stored WAVs, so the post-pass never depends on the live monitor (crash
//!   recovery, re-runs). It returns the per-block lag track.
//! - [`gate_words`] is the one word gate, for the post-pass and live: per
//!   mic word the NCC over `[start − 350 ms, end + 160 ms]` at the tracked
//!   lag ±20 ms, then [`decide`] with hysteresis.
//!
//! The numbers behind the constants (meeting 12, 19 Oct 2026, 111 minutes,
//! speakers; plan 15 section 2):
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
//!   causal (−5 to +60 ms at first: sidetone or "Listen to this device" puts
//!   the owner on the loopback with the mic leading), then ±25 ms around the
//!   tracked lag; −5 to +500 ms after a device change (Bluetooth and HDMI
//!   add 100 to 300 ms).
//! - The mic ASR stamps echo words late (+75 ms median, +275 ms p90), hence
//!   the asymmetric gate span. Word NCC: echo words median 0.74, the owner's
//!   words during far-end speech 0.06 (p90 0.23). Hysteresis: drop at 0.5,
//!   keep under 0.2, in between the neighbours decide at
//!   `echo.gate_threshold` (0.35).
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
/// acquisition (−5 to +60 ms), after a device change (−5 to +500 ms), and
/// tracking (±25 ms around the tracked lag).
pub const ACQUIRE_LAGS: (i64, i64) = (-80, 960);
pub const WIDE_LAGS: (i64, i64) = (-80, 8_000);
pub const TRACK_HALF: i64 = 400;
/// Active blocks without a confident lag before the track is dropped.
const LOSE_AFTER: u32 = 30;
/// Gate span around a word: 350 ms before, 160 ms after; lags ±20 ms.
pub const GATE_BEFORE: i64 = 5_600;
pub const GATE_AFTER: i64 = 2_560;
pub const GATE_LAG_HALF: i64 = 320;
/// Hysteresis: drop at or above, keep under; in between the neighbours
/// decide at `echo.gate_threshold`.
pub const GATE_DROP_AT: f32 = 0.5;
pub const GATE_KEEP_UNDER: f32 = 0.2;
/// `echo.gate_threshold` default and accepted range.
pub const GATE_THRESHOLD: f32 = 0.35;
pub const GATE_THRESHOLD_RANGE: (f32, f32) = (0.2, 0.6);
/// Live PCM ring per channel: 20 s.
pub const RING_SAMPLES: usize = 320_000;
/// High-pass corner of both channels (as the measurement).
const HIGHPASS_HZ: f64 = 100.0;
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
        let n = x.len();
        if n == 0 || y.len() < n {
            return None;
        }
        let ex: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
        if ex <= 0.0 {
            return None;
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
            r2c.process_with_scratch(&mut self.buf, &mut self.sx, &mut self.scratch)
                .ok()?;
            let ylen = (len + lags - 1).min(y.len() - s0);
            self.buf[..ylen].copy_from_slice(&y[s0..s0 + ylen]);
            self.buf[ylen..].fill(0.0);
            r2c.process_with_scratch(&mut self.buf, &mut self.sy, &mut self.scratch)
                .ok()?;
            for ((a, p), q) in self.acc.iter_mut().zip(&self.sx).zip(&self.sy) {
                *a += p.conj() * q;
            }
            s0 += len;
        }
        self.acc[0].im = 0.0;
        self.acc[bins - 1].im = 0.0;
        c2r.process_with_scratch(&mut self.acc, &mut self.out, &mut self.scratch)
            .ok()?;
        let scale = 1.0 / size as f64;
        let floor = ex * 1e-10;
        let mut best = (0usize, 0.0f32);
        for k in 0..lags {
            let ey = self.pre[k + n] - self.pre[k];
            if ey <= floor {
                continue;
            }
            let c = (f64::from(self.out[k]).abs() * scale / (ex * ey).sqrt()).min(1.0) as f32;
            if c > best.1 {
                best = (k, c);
            }
        }
        Some(best)
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

/// A 20 s ring of high-passed samples on meeting time (live), with the AGC
/// gain of every frame.
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
    hp: HighPass,
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
            hp: HighPass::default(),
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

/// The lag track: the median of the last five confident block lags, held
/// once three of them agree within ±3 ms; on raw indices (no clock maps)
/// the older lags and the prediction are moved forward with the drift
/// fitted over the last 64 confident blocks once they span two minutes.
#[derive(Debug, Clone, Default)]
struct LagTrack {
    raw: bool,
    lags: VecDeque<(f64, f32)>,
    center: Option<f32>,
    at: f64,
    misses: u32,
    wide: bool,
    fit: VecDeque<(f64, f32)>,
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

    fn locked(&self) -> bool {
        self.center.is_some() && self.misses < LOSE_AFTER
    }

    fn search(&self, t: f64) -> (i64, i64) {
        if self.wide && !self.locked() {
            return WIDE_LAGS;
        }
        match self.predicted(t) {
            Some(c) if self.locked() => {
                let c = c.round() as i64;
                (c - TRACK_HALF, c + TRACK_HALF)
            }
            Some(c) if self.raw => {
                let c = c.round() as i64;
                (c + ACQUIRE_LAGS.0, c + ACQUIRE_LAGS.1)
            }
            _ => ACQUIRE_LAGS,
        }
    }

    fn observe(&mut self, t: f64, lag: f32, coef: f32) {
        if coef < LAG_CONFIDENT {
            self.misses += 1;
            if self.misses >= LOSE_AFTER && !self.raw {
                self.center = None;
                self.lags.clear();
            }
            return;
        }
        self.misses = 0;
        self.lags.push_back((t, lag));
        if self.lags.len() > 5 {
            self.lags.pop_front();
        }
        if self.lags.len() >= 3 {
            // On raw indices the older lags are moved forward by the drift.
            let d = self.drift().unwrap_or(0.0);
            let v: Vec<f32> = self
                .lags
                .iter()
                .map(|&(ti, l)| l + (d * (t - ti)) as f32)
                .collect();
            let m = median(&mut v.clone()).unwrap_or(lag);
            let agree = v
                .iter()
                .filter(|l| (*l - m).abs() <= CONSISTENT_SAMPLES)
                .count();
            if agree >= 3 {
                self.center = Some(m);
                self.at = t;
                self.wide = false;
            }
        }
        if self.center.is_some() {
            self.fit.push_back((t, lag));
            if self.fit.len() > DRIFT_POINTS {
                self.fit.pop_front();
            }
        }
    }

    fn restart(&mut self) {
        *self = Self {
            raw: self.raw,
            wide: true,
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

    /// The lag window (samples) for a block at `t`.
    pub fn search(&self, t: i64) -> (i64, i64) {
        self.track.search(t as f64)
    }

    /// The tracked lag (samples) at `t`, if any.
    pub fn lag_at(&self, t: i64) -> Option<f32> {
        if self.track.locked() || self.track.raw {
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
    /// the lag window, correlates, updates the state.
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
            status: self.status,
        };
        if !r.active {
            return Some(r);
        }
        let (lo, hi) = self.search(t);
        if audio.read(Channel::Mic, t + lo, BLOCK + (hi - lo) as usize, y) != Avail::Ready {
            return None;
        }
        let (k, c) = corr.peak(x, y).unwrap_or((0, 0.0));
        r.coefficient = c;
        r.lag = (lo + k as i64) as f32;
        r.mic_dbfs = rms_dbfs(&y[k..k + BLOCK]);
        self.observe(&mut r, &*audio);
        Some(r)
    }

    fn observe(&mut self, r: &mut BlockResult, audio: &dyn EchoAudio) {
        self.track.observe(r.t as f64, r.lag, r.coefficient);
        self.recent.push_back((r.coefficient, r.lag));
        if self.recent.len() > MEDIAN_BLOCKS {
            self.recent.pop_front();
        }
        let mut c: Vec<f32> = self.recent.iter().map(|v| v.0).collect();
        let med = median(&mut c).unwrap_or(0.0);
        let consistent = self.track.center.map_or(0, |center| {
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

#[derive(Debug, Default)]
struct Placement {
    hp: HighPass,
    /// Timeline minus channel sample, refreshed every 10 frames.
    offset: i64,
    frames: u32,
    next: Option<u64>,
    scratch: Vec<f32>,
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
/// into a 20 s ring each on meeting time, 2 s blocks judged as they
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
    /// Frames the tap could not deliver (counted by the recorder).
    pub dropped_frames: u64,
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
            dropped_frames: 0,
        }
    }

    pub fn state(&self) -> EchoState {
        self.detector.state()
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

    fn timeline_offset(&self, ch: Channel, frame: &Frame) -> i64 {
        let Some((mic, lb)) = &self.clocks else {
            return 0;
        };
        let map = if ch == Channel::Mic { mic } else { lb };
        let map = map.lock().unwrap_or_else(|e| e.into_inner());
        let t = (map.qpc_at(frame.start_sample as f64) - map.origin_qpc as f64)
            / NOMINAL_HNS_PER_SAMPLE;
        t.round() as i64 - frame.start_sample as i64
    }

    /// Adds one 100 ms frame of either channel; returns the new state when
    /// the status changed.
    pub fn push(&mut self, frame: &Frame) -> Option<EchoState> {
        let idx = match frame.channel {
            Channel::Mic => 0,
            Channel::Loopback => 1,
            Channel::Source => return None,
        };
        let refresh = {
            let p = &self.place[idx];
            p.frames.is_multiple_of(10) || p.next != Some(frame.start_sample)
        };
        if refresh {
            self.place[idx].offset = self.timeline_offset(frame.channel, frame);
        }
        let p = &mut self.place[idx];
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
            let (_, hi) = self.detector.search(t);
            if self.mic.head() < t + BLOCK as i64 + hi {
                // Wait for the mic unless the loopback is far ahead.
                if self.loopback.head() - self.mic.head() < (RING_SAMPLES / 2) as i64 {
                    break;
                }
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
            .map(|&s| score_word(&mut audio, &mut self.corr, s, lag, &mut x, &mut y))
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
    /// Per confident block: (timeline sample, tracked lag in samples).
    pub track: Vec<(i64, f32)>,
    /// The timeline is meeting time through these maps (else raw indices).
    pub clocks: Option<ClockPair>,
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
        let mut track = Vec::new();
        let mut blocks = 0usize;
        let mut erl = Vec::new();
        for k in 0i64.. {
            let t = k * BLOCK as i64;
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
            if let Some(e) = r.erl_db {
                erl.push(e);
            }
            if r.coefficient >= LAG_CONFIDENT {
                if let Some(l) = det.lag_at(t) {
                    track.push((t, l));
                }
            }
        }
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
            track,
            clocks,
        }
    }

    /// The tracked lag (samples) at timeline sample `t`: linear between the
    /// track's points; past its ends the end value (plus the fitted drift on
    /// raw indices). `None` without a track.
    pub fn lag_at(&self, t: i64) -> Option<f32> {
        let tr = &self.track;
        let (first, last) = (tr.first()?, tr.last()?);
        let drift = if self.clocks.is_none() {
            fit_slope(tr).unwrap_or(0.0)
        } else {
            0.0
        };
        if t <= first.0 {
            return Some(first.1 + (drift * (t - first.0) as f64) as f32);
        }
        if t >= last.0 {
            return Some(last.1 + (drift * (t - last.0) as f64) as f32);
        }
        let i = tr.partition_point(|p| p.0 <= t);
        let (a, b) = (tr[i - 1], tr[i]);
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

/// Scores one mic word (timeline samples of the mic): the loopback that
/// would have produced it (the padded span `[start − 350 ms, end + 160 ms]`
/// moved back by `lag`) against the mic at `lag` ±20 ms (the causal
/// acquisition window without a lag).
fn score_word(
    audio: &mut dyn EchoAudio,
    corr: &mut Correlator,
    (start, end): (i64, i64),
    lag: Option<f32>,
    x: &mut Vec<f32>,
    y: &mut Vec<f32>,
) -> WordScore {
    let base = lag.map_or(0, |l| l.round() as i64);
    let p0 = start - GATE_BEFORE - base;
    let p1 = end.max(start) + GATE_AFTER - base;
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

/// The hysteresis over a word sequence in time order: drop at or above 0.5,
/// keep under 0.2, in between drop when the median of the two neighbouring
/// words on each side (those with a coefficient; the word's own without
/// any) is at or above `threshold`. Words without a coefficient are kept.
pub fn decide(scores: &[WordScore], threshold: f32) -> Vec<bool> {
    (0..scores.len())
        .map(|i| match scores[i].coefficient() {
            None => false,
            Some(c) if c >= GATE_DROP_AT => true,
            Some(c) if c < GATE_KEEP_UNDER => false,
            Some(c) => {
                let mut nb: Vec<f32> = [i.wrapping_sub(2), i.wrapping_sub(1), i + 1, i + 2]
                    .iter()
                    .filter_map(|&j| scores.get(j).and_then(|s| s.coefficient()))
                    .collect();
                median(&mut nb).unwrap_or(c) >= threshold
            }
        })
        .collect()
}

/// What [`gate_words`] decided, in the order of the input spans.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateReport {
    pub scores: Vec<WordScore>,
    pub drop: Vec<bool>,
    pub dropped: usize,
    pub kept: usize,
}

/// The word gate (post-pass and live): scores every span (timeline samples,
/// i.e. meeting ms × 16 with clock maps) against `audio` at the lag `lag_at`
/// gives for its time, then [`decide`] over the spans in time order.
pub fn gate_words(
    spans: &[(i64, i64)],
    audio: &mut dyn EchoAudio,
    lag_at: &dyn Fn(i64) -> Option<f32>,
    threshold: f32,
) -> GateReport {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i]);
    let mut corr = Correlator::default();
    let (mut x, mut y) = (Vec::new(), Vec::new());
    let mut sorted = Vec::with_capacity(spans.len());
    for &i in &order {
        let s = spans[i];
        sorted.push(score_word(audio, &mut corr, s, lag_at(s.0), &mut x, &mut y));
    }
    let drops = decide(&sorted, threshold);
    let mut scores = vec![WordScore::Undefined; spans.len()];
    let mut drop = vec![false; spans.len()];
    for (k, &i) in order.iter().enumerate() {
        scores[i] = sorted[k];
        drop[i] = drops[k];
    }
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
    threshold: f32,
) -> GateReport {
    let mut audio = StreamAudio {
        mic: Stream::new(mic),
        loopback: Stream::new(loopback),
        clocks: profile.clocks.clone(),
    };
    gate_words(spans, &mut audio, &|t| profile.lag_at(t), threshold)
}

/// [`gate_sources`] over the two WAVs.
pub fn gate_wavs(
    spans: &[(i64, i64)],
    mic_wav: &Path,
    loopback_wav: &Path,
    profile: &EchoProfile,
    threshold: f32,
) -> Result<GateReport> {
    Ok(gate_sources(
        spans,
        WavSamples::open(mic_wav)?,
        WavSamples::open(loopback_wav)?,
        profile,
        threshold,
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

#[cfg(test)]
pub(crate) mod synth;

#[cfg(test)]
mod tests;
