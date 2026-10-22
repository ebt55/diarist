//! The post-pass echo canceller (plan 15 chunk B2a): [`Aec`] over a whole
//! stored recording, `mic.wav` against `loopback.wav`, into `mic-aec.wav`.
//! This is the configuration the spike measured best on meeting 12
//! (`docs/research/14-echo-cancellation.md` section 2, "lag track + fine"
//! with residual suppression), made to run in bounded memory: the WAVs are
//! read by position, a few seconds at a time.
//!
//! # Alignment
//!
//! The mic is never resampled: the output is sample-aligned with
//! `mic.wav` and maps to meeting time through the mic's clock map. The
//! reference (the loopback) is placed on the mic's clock in three stages,
//! one knot every 10 ms, read with [`SincInterpolator`]:
//!
//! 1. Clock anchors: mic sample → QPC (mic map) → loopback sample (the
//!    loopback map's inverse), fractional. Without clock maps the raw
//!    sample indices.
//! 2. Lag track: the echo lag under stage 1 per 2 s window (hop 1 s) by
//!    GCC-PHAT over 200 Hz to 6 kHz, searched within ±10 ms of the echo
//!    detector's tracked lag for that time (`EchoProfile::lag_at`; without
//!    one, −4 to +16 ms), then a line per segment between jumps of more than
//!    25 samples ([`LagTrack`]). The reference is re-placed so that the lag
//!    stays at [`LAG0`]. On meeting 12 this follows the 8.8 ppm slide and the
//!    5 ms jumps every 9.45 minutes that no anchor sees; with the detector's
//!    lag as the centre it also follows an output with 100 to 500 ms of
//!    latency or a switch to one, and raw indices drifting 3 ms a minute.
//! 3. Fine wobble: per 1 s window (hop 0.5 s) the slope of the
//!    cross-spectrum phase over 300 Hz to 3 kHz against the average of the
//!    surrounding ±15 s (40 to 85 µs on meeting 12), median of three.
//!
//! Both trackers look ahead (centred windows): this is the offline best
//! case, which only the post-pass can have.
//!
//! # The AGC
//!
//! The reference should be the loopback before the app's AGC. With a gain
//! track ([`GainTrack`], recorded since this chunk) each loopback sample is
//! divided by the gain it got before it is placed; without one the stored
//! (post-AGC) loopback is the reference (the spike's 12.2 dB case). A mic
//! recorded with its own AGC has its track undone before the canceller and
//! re-applied after it, so `mic-aec.wav` keeps the level the archive has.
//!
//! # Return loss enhancement
//!
//! [`OfflineReport::erle_db`]: mic energy over output energy (100 Hz
//! high-passed) summed over the far-end-only 2 s blocks: the loopback is
//! active (at or above −45 dBFS after the AGC, the detector's threshold)
//! and the mic stands at most 2 dB above the level the median return loss
//! of the active blocks predicts from the reference (more is the owner
//! talking). No word labels are needed, so the figure is measured on every
//! recording.
//!
//! # Cost
//!
//! The trackers run on up to 8 threads, the canceller on one (140 to 160 µs
//! per 100 ms frame) with the final placement on a second; memory is a few
//! MB per thread plus 6 MB per hour of knots and 20 MB per hour of
//! cross-spectra.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::time::Instant;

use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;

use super::{Aec, AecConfig, AecStats, ResidualConfig, SincInterpolator, FRAME_100MS};
use crate::audio::capture::audio_err;
use crate::audio::clock::ClockMap;
use crate::audio::gain_track::GainTrack;
use crate::error::{CoreError, Result};

/// The lag (samples) the re-placed reference keeps: 2.5 ms, inside the
/// canceller's 5 ms lookahead.
pub const LAG0: f64 = 40.0;
/// Knot spacing (10 ms).
const STEP: usize = 160;
const SR: f64 = 16_000.0;
/// High-pass lead-in before every window (100 ms).
const PRE: usize = 1_600;
/// Lag tracker: window, hop, FFT size.
const LAG_W: usize = 32_000;
const LAG_HOP: usize = 16_000;
const LAG_N: usize = 65_536;
/// Lags searched around the detector's lag (±10 ms), or without one.
const LAG_AROUND: i64 = 160;
const LAG_DEFAULT: (i64, i64) = (-64, 256);
/// Fine tracker: window, hop, FFT size and hop inside a window.
const FINE_L: usize = 16_000;
const FINE_HOP: usize = 8_000;
const FINE_N: usize = 2_048;
const FINE_FH: usize = 1_024;
/// Windows on each side of the fine tracker's average (±15 s).
const FINE_AVG: usize = 30;
/// A window or block is active at this reference level (dBFS).
const ACTIVE_DBFS: f64 = -45.0;
/// ERLE blocks (2 s).
const ERLE_BLOCK: usize = 32_000;
/// Mic over the predicted echo level beyond which a block is double talk.
const DOUBLE_TALK_DB: f64 = 2.0;
/// Windows per parallel work item.
const LAG_GROUP: usize = 32;
const FINE_GROUP: usize = 64;

/// Random access to 16 kHz mono samples (full scale 1.0).
pub trait Samples: Sync {
    /// Samples in the source.
    fn frames(&self) -> u64;
    /// `out[i]` is sample `start + i`; zero outside the source.
    fn read(&self, start: i64, out: &mut [f32]) -> std::io::Result<()>;
}

impl Samples for &[i16] {
    fn frames(&self) -> u64 {
        self.len() as u64
    }

    fn read(&self, start: i64, out: &mut [f32]) -> std::io::Result<()> {
        for (i, o) in out.iter_mut().enumerate() {
            let s = start + i as i64;
            *o = if s >= 0 && (s as usize) < self.len() {
                f32::from(self[s as usize]) / 32_768.0
            } else {
                0.0
            };
        }
        Ok(())
    }
}

/// A 16 kHz mono PCM16 WAV read by position (several threads at once).
pub struct WavFile {
    file: File,
    data_start: u64,
    frames: u64,
}

impl WavFile {
    pub fn open(path: &Path) -> Result<Self> {
        let err = |e: String| audio_err(format!("open {}: {e}", path.display()));
        let r = hound::WavReader::open(path).map_err(|e| err(e.to_string()))?;
        let s = r.spec();
        if s.channels != 1
            || s.bits_per_sample != 16
            || s.sample_rate != 16_000
            || s.sample_format != hound::SampleFormat::Int
        {
            return Err(err(format!("not 16 kHz mono PCM16 ({s:?})")));
        }
        let frames = u64::from(r.duration());
        let mut inner = r.into_inner();
        let data_start =
            std::io::Seek::stream_position(&mut inner).map_err(|e| err(e.to_string()))?;
        Ok(Self {
            file: File::open(path).map_err(|e| err(e.to_string()))?,
            data_start,
            frames,
        })
    }
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

impl Samples for WavFile {
    fn frames(&self) -> u64 {
        self.frames
    }

    fn read(&self, start: i64, out: &mut [f32]) -> std::io::Result<()> {
        out.fill(0.0);
        let a = start.max(0) as u64;
        let b = ((start + out.len() as i64).max(0) as u64).min(self.frames);
        if a >= b {
            return Ok(());
        }
        let mut bytes = vec![0u8; ((b - a) * 2) as usize];
        let mut got = 0usize;
        while got < bytes.len() {
            match read_at(
                &self.file,
                &mut bytes[got..],
                self.data_start + a * 2 + got as u64,
            ) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let o = (a as i64 - start) as usize;
        for (d, b) in out[o..]
            .iter_mut()
            .zip(bytes[..got & !1].as_chunks::<2>().0)
        {
            *d = f32::from(i16::from_le_bytes(*b)) / 32_768.0;
        }
        Ok(())
    }
}

/// 4th-order Butterworth high-pass at 100 Hz (two biquads), streaming.
#[derive(Debug, Clone, Default)]
struct HighPass {
    z: [[f64; 2]; 2],
}

impl HighPass {
    fn coefs() -> [[f64; 5]; 2] {
        let mut c = [[0.0; 5]; 2];
        for (k, q) in [0.541_196_1f64, 1.306_563].into_iter().enumerate() {
            let w0 = std::f64::consts::TAU * 100.0 / SR;
            let alpha = w0.sin() / (2.0 * q);
            let cw = w0.cos();
            let a0 = 1.0 + alpha;
            c[k] = [
                (1.0 + cw) / 2.0 / a0,
                -(1.0 + cw) / a0,
                (1.0 + cw) / 2.0 / a0,
                -2.0 * cw / a0,
                (1.0 - alpha) / a0,
            ];
        }
        c
    }

    fn run(&mut self, x: &mut [f32]) {
        let c = Self::coefs();
        for v in x.iter_mut() {
            let mut y = f64::from(*v);
            for (cc, z) in c.iter().zip(self.z.iter_mut()) {
                let out = cc[0] * y + z[0];
                z[0] = cc[1] * y - cc[3] * out + z[1];
                z[1] = cc[2] * y - cc[4] * out;
                y = out;
            }
            *v = y as f32;
        }
    }
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

fn dbfs_power(e: f64, n: usize) -> f64 {
    10.0 * (e / n.max(1) as f64).max(1e-20).log10()
}

fn median(v: &mut Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

/// Runs `f(state, i)` for `i` in `0..n` on up to `threads` threads (one
/// `init()` state each); the results in index order.
fn par_map<S, T: Send>(
    n: usize,
    threads: usize,
    init: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, usize) -> T + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::with_capacity(n));
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, n.max(1)) {
            s.spawn(|| {
                let mut st = init();
                let mut mine = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    mine.push((i, f(&mut st, i)));
                }
                done.lock().unwrap_or_else(|e| e.into_inner()).extend(mine);
            });
        }
    });
    let mut v = done.into_inner().unwrap_or_else(|e| e.into_inner());
    v.sort_by_key(|p| p.0);
    v.into_iter().map(|p| p.1).collect()
}

/// Where each mic sample's reference comes from: a fractional loopback
/// sample per knot (every [`STEP`] mic samples), linear between knots.
struct Knots(Vec<f64>);

impl Knots {
    fn at(&self, m: usize) -> f64 {
        let k = &self.0;
        let b = (m / STEP).min(k.len() - 2);
        let r = (m as f64 - (b * STEP) as f64) / STEP as f64;
        k[b] + (k[b + 1] - k[b]) * r
    }

    /// The same knots moved by `shift(mic sample)` (subtracted).
    fn shifted(&self, shift: impl Fn(f64) -> f64) -> Self {
        Self(
            self.0
                .iter()
                .enumerate()
                .map(|(b, &p)| p - shift((b * STEP) as f64))
                .collect(),
        )
    }
}

/// Places the loopback for mic samples `m0..m0 + out.len()`: band-limited
/// at the knots' fractional positions, after the AGC's gain is divided out
/// when `gain` is given. Returns the post-AGC loopback energy and sample
/// count over the loopback span those mic samples cover.
fn place(
    lb: &dyn Samples,
    knots: &Knots,
    m0: usize,
    out: &mut [f32],
    gain: Option<&GainTrack>,
    interp: &SincInterpolator,
    buf: &mut Vec<f32>,
) -> std::io::Result<(f64, usize)> {
    if out.is_empty() {
        return Ok((0.0, 0));
    }
    let n = out.len();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for b in (m0 / STEP)..=((m0 + n) / STEP + 1) {
        let p = knots.at((b * STEP).max(m0).min(m0 + n));
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let base = lo.floor() as i64 - 20;
    let len = (hi.ceil() as i64 + 21 - base).max(1) as usize;
    buf.resize(len, 0.0);
    lb.read(base, buf)?;
    // The span the block's own samples come from (the post-AGC level).
    let (a, b) = (
        (knots.at(m0).floor() as i64 - base).clamp(0, len as i64) as usize,
        (knots.at(m0 + n).floor() as i64 - base).clamp(0, len as i64) as usize,
    );
    let (a, b) = (a.min(b), a.max(b));
    let level = (energy(&buf[a..b]), b - a);
    if let Some(g) = gain {
        // Samples before 0 are zeros anyway.
        let first = base.max(0);
        if let Some(tail) = buf.get_mut((first - base) as usize..) {
            g.undo(first as u64, tail);
        }
    }
    for (j, o) in out.iter_mut().enumerate() {
        *o = interp.at(buf, knots.at(m0 + j) - base as f64);
    }
    Ok(level)
}

/// One lag estimate: window centre (mic sample), lag (samples, the mic
/// after the reference), peak height (0..1).
type LagPoint = (f64, f64, f64);

/// The echo lag as a piecewise-linear function of the mic sample: segments
/// split where the running median (9 windows) jumps by more than 25
/// samples, a least-squares line per segment through the points within 12
/// samples of the running median (fewer than 10: the segment's median).
#[derive(Debug, Clone)]
pub struct LagTrack {
    /// (first mic sample, intercept, slope) per segment, in order.
    segs: Vec<(f64, f64, f64)>,
    /// (mic sample, step in samples) per jump.
    pub jumps: Vec<(f64, f64)>,
}

impl LagTrack {
    fn fit(pts: &[LagPoint]) -> Option<Self> {
        let n = pts.len();
        if n == 0 {
            return None;
        }
        let lags: Vec<f64> = pts.iter().map(|p| p.1).collect();
        let med: Vec<f64> = (0..n)
            .map(|i| median(&mut lags[i.saturating_sub(4)..(i + 5).min(n)].to_vec()).unwrap_or(0.0))
            .collect();
        let mut bounds = vec![0usize];
        let mut jumps = Vec::new();
        for i in 1..n {
            if (med[i] - med[i - 1]).abs() > 25.0 && i - bounds[bounds.len() - 1] >= 5 {
                bounds.push(i);
                jumps.push((0.5 * (pts[i - 1].0 + pts[i].0), med[i] - med[i - 1]));
            }
        }
        bounds.push(n);
        let mut segs = Vec::new();
        for (k, w) in bounds.windows(2).enumerate() {
            let idx: Vec<usize> = (w[0]..w[1])
                .filter(|&i| (lags[i] - med[i]).abs() < 12.0)
                .collect();
            let start = if k == 0 {
                f64::NEG_INFINITY
            } else {
                0.5 * (pts[w[0] - 1].0 + pts[w[0]].0)
            };
            let (a, b) = if idx.len() >= 10 {
                let mx = idx.iter().map(|&i| pts[i].0).sum::<f64>() / idx.len() as f64;
                let my = idx.iter().map(|&i| lags[i]).sum::<f64>() / idx.len() as f64;
                let sxy: f64 = idx.iter().map(|&i| (pts[i].0 - mx) * (lags[i] - my)).sum();
                let sxx: f64 = idx.iter().map(|&i| (pts[i].0 - mx).powi(2)).sum();
                let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
                (my - b * mx, b)
            } else {
                (median(&mut lags[w[0]..w[1]].to_vec()).unwrap_or(LAG0), 0.0)
            };
            segs.push((start, a, b));
        }
        Some(Self { segs, jumps })
    }

    fn at(&self, m: f64) -> f64 {
        let k = self.segs.partition_point(|s| s.0 <= m).max(1) - 1;
        let (_, a, b) = self.segs[k];
        a + b * m
    }
}

/// Which alignment stages run (all three in the post-pass; fewer for the
/// evaluation's "anchors only" rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stages {
    pub lag_track: bool,
    pub fine: bool,
}

impl Default for Stages {
    fn default() -> Self {
        Self {
            lag_track: true,
            fine: true,
        }
    }
}

/// What [`cancel_recording`] needs besides the two channels.
pub struct OfflineOptions<'a> {
    /// The mic's and the loopback's clock maps (`None`: raw indices).
    pub clocks: Option<(&'a ClockMap, &'a ClockMap)>,
    /// The echo detector's lag (samples) at a timeline sample (meeting
    /// time × 16 with clock maps, else the mic index): centres the lag
    /// search.
    pub lag_hint: Option<&'a (dyn Fn(i64) -> Option<f32> + Sync)>,
    /// The loopback AGC's gain track, divided out of the reference.
    pub loopback_gain: Option<&'a GainTrack>,
    /// The mic AGC's gain track, divided out before and re-applied after.
    pub mic_gain: Option<&'a GainTrack>,
    pub config: AecConfig,
    pub stages: Stages,
    /// Threads for the trackers.
    pub threads: usize,
    /// Polled between stages and every second of audio.
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<'a> OfflineOptions<'a> {
    /// The post-pass configuration: 200 ms filter, 5 ms lookahead, residual
    /// suppression, all stages, up to 8 threads.
    pub fn post_pass(clocks: Option<(&'a ClockMap, &'a ClockMap)>) -> Self {
        Self {
            clocks,
            lag_hint: None,
            loopback_gain: None,
            mic_gain: None,
            config: post_pass_config(),
            stages: Stages::default(),
            threads: std::thread::available_parallelism().map_or(4, |n| n.get().min(8)),
            cancelled: &|| false,
        }
    }
}

/// [`AecConfig::default`] with residual suppression.
pub fn post_pass_config() -> AecConfig {
    AecConfig {
        residual: Some(ResidualConfig::default()),
        ..AecConfig::default()
    }
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OfflineReport {
    /// Mic samples processed (the output has as many).
    pub frames: u64,
    /// Return loss enhancement over the far-end-only blocks (module docs).
    pub erle_db: Option<f32>,
    pub far_blocks: usize,
    /// 2 s blocks with the loopback active.
    pub active_blocks: usize,
    /// Lag estimates found, and their median (ms) under the anchors.
    pub lag_points: usize,
    pub lag_ms: Option<f32>,
    /// Jumps the lag track split at.
    pub jumps: usize,
    pub fine_points: usize,
    pub stats: AecStats,
    /// Alignment (stages 1 to 3), canceller, whole run (s).
    pub align_s: f64,
    pub cancel_s: f64,
    pub wall_s: f64,
}

/// The lag points of stage 2 (see the module docs).
fn lag_points(
    mic: &dyn Samples,
    lb: &dyn Samples,
    knots: &Knots,
    frames: usize,
    hint: &(dyn Fn(usize) -> Option<f64> + Sync),
    threads: usize,
) -> std::io::Result<Vec<LagPoint>> {
    let starts: Vec<usize> = (0..frames.saturating_sub(LAG_W)).step_by(LAG_HOP).collect();
    let groups = starts.len().div_ceil(LAG_GROUP);
    let (k0, k1) = (200 * LAG_N / 16_000, 6_000 * LAG_N / 16_000);
    struct St {
        fwd: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
        inv: std::sync::Arc<dyn realfft::ComplexToReal<f32>>,
        interp: SincInterpolator,
        bufs: (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>),
    }
    let res = par_map(
        groups,
        threads,
        || {
            let mut p = RealFftPlanner::<f32>::new();
            St {
                fwd: p.plan_fft_forward(LAG_N),
                inv: p.plan_fft_inverse(LAG_N),
                interp: SincInterpolator::default(),
                bufs: Default::default(),
            }
        },
        |st, g| -> std::io::Result<Vec<LagPoint>> {
            let ws = &starts[g * LAG_GROUP..((g + 1) * LAG_GROUP).min(starts.len())];
            let a = ws[0].saturating_sub(PRE);
            let b = ws[ws.len() - 1] + LAG_W;
            let (mic_h, ref_h, buf, cc) = &mut st.bufs;
            mic_h.resize(b - a, 0.0);
            mic.read(a as i64, mic_h)?;
            ref_h.resize(b - a, 0.0);
            place(lb, knots, a, ref_h, None, &st.interp, buf)?;
            HighPass::default().run(mic_h);
            HighPass::default().run(ref_h);
            let (mut xa, mut xb) = (vec![0.0f32; LAG_N], vec![0.0f32; LAG_N]);
            let (mut sa, mut sb) = (st.fwd.make_output_vec(), st.fwd.make_output_vec());
            cc.resize(LAG_N, 0.0);
            let mut pts = Vec::new();
            for &m0 in ws {
                let o = m0 - a;
                let y = &ref_h[o..o + LAG_W];
                if dbfs_power(energy(y), LAG_W) < ACTIVE_DBFS {
                    continue;
                }
                xa.fill(0.0);
                xb.fill(0.0);
                xa[..LAG_W].copy_from_slice(&mic_h[o..o + LAG_W]);
                xb[..LAG_W].copy_from_slice(y);
                st.fwd
                    .process(&mut xa, &mut sa)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                st.fwd
                    .process(&mut xb, &mut sb)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                for (k, (p, q)) in sa.iter_mut().zip(&sb).enumerate() {
                    let gk = *p * q.conj();
                    let mag = gk.norm();
                    *p = if (k0..k1).contains(&k) && mag > 0.0 {
                        gk / mag
                    } else {
                        Complex32::default()
                    };
                }
                st.inv
                    .process(&mut sa, cc)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                let norm = 2.0 * (k1 - k0) as f32;
                let at = |lag: i64| cc[lag.rem_euclid(LAG_N as i64) as usize] / norm;
                let (lo, hi) = match hint(m0 + LAG_W / 2) {
                    Some(c) => {
                        let c = c.round() as i64;
                        (c - LAG_AROUND, c + LAG_AROUND)
                    }
                    None => LAG_DEFAULT,
                };
                let (mut best, mut bl) = (f32::MIN, lo);
                for lag in lo..=hi {
                    let v = at(lag);
                    if v > best {
                        best = v;
                        bl = lag;
                    }
                }
                if best < 0.07 || bl == lo || bl == hi {
                    continue;
                }
                let (l, c, r) = (at(bl - 1), best, at(bl + 1));
                let den = l - 2.0 * c + r;
                let frac = if den != 0.0 { 0.5 * (l - r) / den } else { 0.0 };
                pts.push((
                    (m0 + LAG_W / 2) as f64,
                    bl as f64 + f64::from(frac),
                    f64::from(best),
                ));
            }
            Ok(pts)
        },
    );
    let mut out = Vec::new();
    for r in res {
        out.extend(r?);
    }
    Ok(out)
}

/// Stage 3: (mic sample, delay in samples) per 1 s window with the
/// reference active (see the module docs).
fn fine_points(
    mic: &dyn Samples,
    lb: &dyn Samples,
    knots: &Knots,
    frames: usize,
    threads: usize,
) -> std::io::Result<Vec<(f64, f64)>> {
    let starts: Vec<usize> = (0..frames.saturating_sub(FINE_L))
        .step_by(FINE_HOP)
        .collect();
    let groups = starts.len().div_ceil(FINE_GROUP);
    let (k0, k1) = (300 * FINE_N / 16_000, 3_000 * FINE_N / 16_000);
    let nb = k1 - k0;
    let win: Vec<f32> = (0..FINE_N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FINE_N as f32).cos())
        .collect();
    struct St {
        fwd: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
        interp: SincInterpolator,
        bufs: (Vec<f32>, Vec<f32>, Vec<f32>),
    }
    let res = par_map(
        groups,
        threads,
        || St {
            fwd: RealFftPlanner::<f32>::new().plan_fft_forward(FINE_N),
            interp: SincInterpolator::default(),
            bufs: Default::default(),
        },
        |st, g| -> std::io::Result<Vec<Option<Vec<Complex32>>>> {
            let ws = &starts[g * FINE_GROUP..((g + 1) * FINE_GROUP).min(starts.len())];
            let a = ws[0].saturating_sub(PRE);
            let b = ws[ws.len() - 1] + FINE_L;
            let (mic_h, ref_h, buf) = &mut st.bufs;
            mic_h.resize(b - a, 0.0);
            mic.read(a as i64, mic_h)?;
            ref_h.resize(b - a, 0.0);
            place(lb, knots, a, ref_h, None, &st.interp, buf)?;
            HighPass::default().run(mic_h);
            HighPass::default().run(ref_h);
            let (mut ta, mut tb) = (vec![0.0f32; FINE_N], vec![0.0f32; FINE_N]);
            let (mut sa, mut sb) = (st.fwd.make_output_vec(), st.fwd.make_output_vec());
            let mut out = Vec::with_capacity(ws.len());
            for &w0 in ws {
                let o = w0 - a;
                if dbfs_power(energy(&ref_h[o..o + FINE_L]), FINE_L) < ACTIVE_DBFS {
                    out.push(None);
                    continue;
                }
                let mut acc = vec![Complex32::default(); nb];
                let mut i = o;
                while i + FINE_N <= o + FINE_L {
                    for j in 0..FINE_N {
                        ta[j] = ref_h[i + j] * win[j];
                        tb[j] = mic_h[i + j] * win[j];
                    }
                    st.fwd
                        .process(&mut ta, &mut sa)
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    st.fwd
                        .process(&mut tb, &mut sb)
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    for (k, c) in acc.iter_mut().enumerate() {
                        *c += sa[k0 + k].conj() * sb[k0 + k];
                    }
                    i += FINE_FH;
                }
                out.push(Some(acc));
            }
            Ok(out)
        },
    );
    let mut cs: Vec<Option<Vec<Complex32>>> = Vec::with_capacity(starts.len());
    for r in res {
        cs.extend(r?);
    }
    let mut raw = Vec::new();
    for (w, c) in cs.iter().enumerate() {
        let Some(c) = c else { continue };
        let mut avg = vec![Complex32::default(); nb];
        for v in cs[w.saturating_sub(FINE_AVG)..(w + FINE_AVG + 1).min(cs.len())]
            .iter()
            .flatten()
        {
            for (a, x) in avg.iter_mut().zip(v) {
                *a += *x;
            }
        }
        let (mut num, mut den) = (0f64, 0f64);
        for (k, (x, a)) in c.iter().zip(&avg).enumerate() {
            let f = (k0 + k) as f64 * SR / FINE_N as f64;
            let wt = f64::from(x.norm() * a.norm());
            let ph = f64::from((*x * a.conj()).arg());
            num += wt * f * ph;
            den += wt * f * f;
        }
        if den > 0.0 {
            let delay_s = -num / den / std::f64::consts::TAU;
            raw.push(((starts[w] + FINE_L / 2) as f64, delay_s * SR));
        }
    }
    // A three-window median against single-window outliers.
    Ok((0..raw.len())
        .map(|i| {
            let mut v: Vec<f64> = raw[i.saturating_sub(1)..(i + 2).min(raw.len())]
                .iter()
                .map(|p| p.1)
                .collect();
            (raw[i].0, median(&mut v).unwrap_or(0.0))
        })
        .collect())
}

/// Linear interpolation of `(x, y)` points (sorted by x), held at the ends;
/// 0 without points.
fn interp_points(pts: &[(f64, f64)], x: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let k = pts.partition_point(|p| p.0 <= x);
    if k == 0 {
        return pts[0].1;
    }
    if k == pts.len() {
        return pts[k - 1].1;
    }
    let (a, b) = (pts[k - 1], pts[k]);
    a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
}

fn io(e: std::io::Error) -> CoreError {
    audio_err(format!("echo canceller: {e}"))
}

/// Per 2 s block of the mic: high-passed energies of the mic, the output
/// and the reference fed to the canceller, and the post-AGC loopback level.
#[derive(Debug, Clone, Copy, Default)]
struct Block {
    mic: f64,
    out: f64,
    reference: f64,
    lb: f64,
    lb_n: usize,
}

/// Cancels the echo of `lb` in `mic` (see the module docs); the output, as
/// many samples as `mic`, goes to `sink` in 100 ms blocks.
pub fn cancel_recording(
    mic: &dyn Samples,
    lb: &dyn Samples,
    opts: &OfflineOptions,
    sink: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<OfflineReport> {
    let started = Instant::now();
    let frames = mic.frames() as usize;
    let check = || {
        if (opts.cancelled)() {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    let mut report = OfflineReport {
        frames: frames as u64,
        ..Default::default()
    };
    // Stage 1: the clock anchors.
    let timeline = |m: f64| -> f64 {
        match opts.clocks {
            Some((mc, _)) => (mc.qpc_at(m) - mc.origin_qpc as f64) / 625.0,
            None => m,
        }
    };
    let k0 = Knots(
        (0..frames / STEP + 2)
            .map(|b| {
                let m = (b * STEP) as f64;
                match opts.clocks {
                    Some((mc, lc)) => lc.sample_at(mc.qpc_at(m)),
                    None => m,
                }
            })
            .collect(),
    );
    // Stage 2: the lag track.
    let hint = |m: usize| -> Option<f64> {
        let h = opts.lag_hint?;
        h(timeline(m as f64).round() as i64).map(f64::from)
    };
    let mut knots = k0;
    if opts.stages.lag_track {
        check()?;
        let pts = lag_points(mic, lb, &knots, frames, &hint, opts.threads).map_err(io)?;
        report.lag_points = pts.len();
        report.lag_ms =
            median(&mut pts.iter().map(|p| p.1).collect::<Vec<_>>()).map(|l| (l / 16.0) as f32);
        if let Some(track) = LagTrack::fit(&pts) {
            report.jumps = track.jumps.len();
            knots = knots.shifted(|m| track.at(m) - LAG0);
        } else if let Some(c) = hint(frames / 2) {
            // No far end to track: the detector's lag, if it has one.
            knots = knots.shifted(|_| c - LAG0);
        }
    }
    // Stage 3: the fine wobble.
    if opts.stages.fine {
        check()?;
        let fine = fine_points(mic, lb, &knots, frames, opts.threads).map_err(io)?;
        report.fine_points = fine.len();
        knots = knots.shifted(|m| interp_points(&fine, m));
    }
    report.align_s = started.elapsed().as_secs_f64();
    check()?;

    // The canceller, fed by a placement thread.
    let t0 = Instant::now();
    let mut aec = Aec::new(opts.config).map_err(audio_err)?;
    let lat = aec.latency();
    let blocks_in = (frames + lat).div_ceil(FRAME_100MS);
    let mut stats_blocks = vec![Block::default(); frames.div_ceil(ERLE_BLOCK).max(1)];
    let knots = &knots;
    let result = std::thread::scope(|s| -> Result<()> {
        let (tx, rx) = sync_channel::<std::io::Result<(Vec<f32>, Vec<f32>, (f64, usize))>>(32);
        s.spawn(move || {
            let interp = SincInterpolator::default();
            let mut buf = Vec::new();
            for k in 0..blocks_in {
                let m0 = k * FRAME_100MS;
                let mut mb = vec![0.0f32; FRAME_100MS];
                let mut rb = vec![0.0f32; FRAME_100MS];
                let n = frames.saturating_sub(m0).min(FRAME_100MS);
                let r = mic.read(m0 as i64, &mut mb[..n]).and_then(|()| {
                    place(
                        lb,
                        knots,
                        m0,
                        &mut rb[..n],
                        opts.loopback_gain,
                        &interp,
                        &mut buf,
                    )
                });
                let item = r.map(|level| (mb, rb, level));
                let failed = item.is_err();
                if tx.send(item).is_err() || failed {
                    return;
                }
            }
        });
        let mut hp = (
            HighPass::default(),
            HighPass::default(),
            HighPass::default(),
        );
        let mut out = vec![0.0f32; FRAME_100MS];
        let mut tmp = vec![0.0f32; FRAME_100MS];
        let mut skip = lat;
        let mut emitted = 0usize;
        for k in 0..blocks_in {
            if k % 10 == 0 {
                check()?;
            }
            let (mut mb, rb, level) = rx
                .recv()
                .map_err(|_| audio_err("echo canceller: the placement thread stopped"))?
                .map_err(io)?;
            let m0 = k * FRAME_100MS;
            let n = frames.saturating_sub(m0).min(FRAME_100MS);
            if n > 0 {
                let bi = m0 / ERLE_BLOCK;
                tmp[..n].copy_from_slice(&mb[..n]);
                hp.0.run(&mut tmp[..n]);
                stats_blocks[bi].mic += energy(&tmp[..n]);
                tmp[..n].copy_from_slice(&rb[..n]);
                hp.1.run(&mut tmp[..n]);
                stats_blocks[bi].reference += energy(&tmp[..n]);
                stats_blocks[bi].lb += level.0;
                stats_blocks[bi].lb_n += level.1;
                if let Some(g) = opts.mic_gain {
                    g.undo(m0 as u64, &mut mb[..n]);
                }
            }
            aec.process(&mb, &rb, &mut out);
            let from = skip.min(FRAME_100MS);
            skip -= from;
            let take = (FRAME_100MS - from).min(frames - emitted);
            if take > 0 {
                let o = &mut out[from..from + take];
                if let Some(g) = opts.mic_gain {
                    g.apply(emitted as u64, o);
                }
                sink(o)?;
                let mut j = 0;
                while j < take {
                    let bi = (emitted + j) / ERLE_BLOCK;
                    let end = ((bi + 1) * ERLE_BLOCK - emitted).min(take);
                    tmp[..end - j].copy_from_slice(&o[j..end]);
                    hp.2.run(&mut tmp[..end - j]);
                    stats_blocks[bi].out += energy(&tmp[..end - j]);
                    j = end;
                }
                emitted += take;
            }
        }
        drop(rx);
        Ok(())
    });
    result?;
    report.stats = aec.stats();
    report.cancel_s = t0.elapsed().as_secs_f64();

    // ERLE over the far-end-only blocks.
    let active: Vec<&Block> = stats_blocks
        .iter()
        .filter(|b| b.lb_n > 0 && dbfs_power(b.lb, b.lb_n) >= ACTIVE_DBFS && b.reference > 0.0)
        .collect();
    report.active_blocks = active.len();
    let rel = |b: &Block| 10.0 * (b.mic.max(1e-20) / b.reference.max(1e-20)).log10();
    let mut d: Vec<f64> = active.iter().map(|b| rel(b)).collect();
    if let Some(med) = median(&mut d) {
        let far: Vec<&&Block> = active
            .iter()
            .filter(|b| rel(b) <= med + DOUBLE_TALK_DB)
            .collect();
        report.far_blocks = far.len();
        let (em, eo) = far
            .iter()
            .fold((0f64, 0f64), |acc, b| (acc.0 + b.mic, acc.1 + b.out));
        if em > 0.0 && eo > 0.0 {
            report.erle_db = Some((10.0 * (em / eo).log10()) as f32);
        }
    }
    report.wall_s = started.elapsed().as_secs_f64();
    Ok(report)
}

/// [`cancel_recording`] from `mic_wav` and `loopback_wav` into `out_wav`
/// (16 kHz mono PCM16, as long as `mic_wav`). A failed or cancelled run
/// removes `out_wav`.
pub fn cancel_wavs(
    mic_wav: &Path,
    loopback_wav: &Path,
    out_wav: &Path,
    opts: &OfflineOptions,
) -> Result<OfflineReport> {
    let mic = WavFile::open(mic_wav)?;
    let lb = WavFile::open(loopback_wav)?;
    let r = (|| {
        let mut w = hound::WavWriter::create(out_wav, crate::audio::wav::spec())
            .map_err(|e| audio_err(format!("create {}: {e}", out_wav.display())))?;
        let werr = |e: hound::Error| audio_err(format!("write {}: {e}", out_wav.display()));
        let report = cancel_recording(&mic, &lb, opts, &mut |block| {
            let mut iw = w.get_i16_writer(block.len() as u32);
            for &v in block {
                iw.write_sample((v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16);
            }
            iw.flush().map_err(werr)
        })?;
        w.finalize().map_err(werr)?;
        Ok(report)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(out_wav);
    }
    r
}

#[cfg(test)]
mod tests;
