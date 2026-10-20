//! The app-side echo canceller (`audio::aec`) on a real speaker recording:
//! `docs/research/14-echo-cancellation.md` section 2. Skipped unless set:
//!
//! ```text
//! DIARIST_ECHO_SAMPLE=<meeting audio folder: playback.flac, mic/loopback.words.json>
//! DIARIST_ECHO_DB=<diarist.db, opened read-only>
//! DIARIST_ECHO_OUT=<output folder>
//! cargo test --release -p diarist-core --test aec_spike -- --nocapture
//! ```
//!
//! Reads the meeting folder and the database read-only. Writes to the
//! output folder: `mic.wav` and `loopback.wav` (the raw channels);
//! `mic-aec.wav` (the linear canceller), `mic-aec-res.wav` (with residual
//! suppression), `mic-aec-raw.wav` and `mic-aec-raw-res.wav` (the same with
//! the loopback AGC undone); `mic-aec-anchor-res.wav` and
//! `mic-aec-anchor-raw-res.wav` (the causal configurations: clock anchors
//! only, with suppression); `reference.wav` and `reference-raw.wav` (the
//! aligned references); the spliced double-talk set `dt-near.wav`,
//! `dt-mix.wav`, `dt-mix-aec*.wav`; and `report.txt`. When the
//! re-transcriptions are there as `<wav stem>.asr.json` (the engine's words
//! JSON of `diarist-cli transcribe --no-diarize`), they are scored too.
//!
//! The mic is never resampled; the reference is placed on the mic's clock
//! in three stages, each reported: the clock anchors (mic sample to QPC to
//! loopback sample), then the echo lag tracked from the signals (GCC-PHAT
//! per 2 s, a line per segment between 5 ms jumps), then the sub-second
//! wobble (cross-spectrum phase slope per 1 s).
//!
//! Word classes are recomputed from the stored word files the way the
//! meeting 12 measurement labelled them: a mic word is a duplicate when a
//! loopback word with the same normalised text starts within ±0.8 s (or it
//! sits in a matching 3-word run); an owner's word is a non-duplicate whose
//! ±5 neighbours are at most 30 percent duplicates; "far end active" means a
//! 20 ms loopback frame (100 Hz high-passed) above −50 dBFS in the word span.

use std::collections::HashMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use diarist_core::audio::aec::{
    Aec, AecConfig, AecStats, Notch, ResidualConfig, SincInterpolator, FRAME_100MS,
};
use diarist_core::audio::clock::{origin_from_anchors, ClockMap};
use diarist_core::audio::flac::decode_i16;
use diarist_core::types::{Channel, ClockAnchor, MeetingId};
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

type Res<T> = Result<T, Box<dyn Error>>;

const SR: f64 = 16_000.0;

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

// ---------------------------------------------------------------- clocks

/// A channel's clock map plus its inverse.
struct Map {
    cm: ClockMap,
    pts: Vec<(i64, i64)>,
    slope: f64,
    origin: f64,
}

impl Map {
    fn new(origin: i64, anchors: &[ClockAnchor]) -> Self {
        let cm = ClockMap::from_anchors(origin, anchors);
        let pts = cm.points().to_vec();
        let slope = 625.0 * (1.0 + cm.drift_ppm().unwrap_or(0.0) * 1e-6);
        Self {
            cm,
            pts,
            slope,
            origin: origin as f64,
        }
    }

    fn qpc(&self, s: f64) -> f64 {
        self.cm.qpc_at(s)
    }

    /// Fractional sample at QPC `q` (piecewise linear, extrapolated with
    /// the fitted slope).
    fn sample_at(&self, q: f64) -> f64 {
        let p = &self.pts;
        if p.is_empty() {
            return (q - self.origin) / 625.0;
        }
        let (first, last) = (p[0], p[p.len() - 1]);
        if q <= first.1 as f64 {
            return first.0 as f64 + (q - first.1 as f64) / self.slope;
        }
        if q >= last.1 as f64 {
            return last.0 as f64 + (q - last.1 as f64) / self.slope;
        }
        let i = p.partition_point(|a| (a.1 as f64) <= q).max(1);
        let (a, b) = (p[i - 1], p[i]);
        if b.1 <= a.1 {
            return b.0 as f64;
        }
        a.0 as f64 + (q - a.1 as f64) / (b.1 - a.1) as f64 * (b.0 - a.0) as f64
    }

    fn meeting_s(&self, s: f64) -> f64 {
        (self.qpc(s) - self.origin) / 1e7
    }

    fn sample_of(&self, t: f64) -> f64 {
        self.sample_at(self.origin + t * 1e7)
    }

    /// Channel seconds to meeting seconds the way the post-pass maps words.
    fn word_s(&self, channel_s: f64) -> f64 {
        self.cm.meeting_ms((channel_s * 1_000.0).round() as i64) as f64 / 1_000.0
    }
}

fn load_anchors(db: &Path, uid: &str) -> Res<(i64, Vec<ClockAnchor>)> {
    use rusqlite::OpenFlags;
    let uri = format!(
        "file:///{}?mode=ro",
        db.display().to_string().replace('\\', "/")
    );
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let id: i64 = conn.query_row("select id from meetings where uid = ?1", [uid], |r| {
        r.get(0)
    })?;
    let mut st = conn.prepare(
        "select channel, sample_index, qpc_100ns from clock_anchors \
         where meeting_id = ?1 order by channel, sample_index",
    )?;
    let rows = st.query_map([id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (ch, s, q) = row?;
        let channel = match ch.as_str() {
            "mic" => Channel::Mic,
            "loopback" => Channel::Loopback,
            _ => continue,
        };
        out.push(ClockAnchor {
            meeting_id: MeetingId(id),
            channel,
            sample_index: s,
            qpc_100ns: q,
        });
    }
    Ok((id, out))
}

// ---------------------------------------------------------------- signals

/// 4th-order Butterworth high-pass at 100 Hz (two biquads), causal.
fn highpass(x: &[f32]) -> Vec<f32> {
    let mut y: Vec<f32> = x.to_vec();
    for q in [0.541_196_1f64, 1.306_563] {
        let w0 = std::f64::consts::TAU * 100.0 / SR;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = ((1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0);
        let (a1, a2) = (-2.0 * c / a0, (1.0 - alpha) / a0);
        let (mut z1, mut z2) = (0f64, 0f64);
        for v in y.iter_mut() {
            let xin = f64::from(*v);
            let out = b0 * xin + z1;
            z1 = b1 * xin - a1 * out + z2;
            z2 = b2 * xin - a2 * out;
            *v = out as f32;
        }
    }
    y
}

fn energy(x: &[f32], a: usize, b: usize) -> f64 {
    let b = b.min(x.len());
    if a >= b {
        return 0.0;
    }
    x[a..b].iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

fn db(num: f64, den: f64) -> f64 {
    10.0 * (num.max(1e-30) / den.max(1e-30)).log10()
}

fn dbfs_power(e: f64, n: usize) -> f64 {
    10.0 * (e / n.max(1) as f64).max(1e-20).log10()
}

fn pct(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return f64::NAN;
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = (s.len() - 1) as f64 * q;
    let (f, c) = (k.floor() as usize, (k.ceil() as usize).min(s.len() - 1));
    s[f] + (s[c] - s[f]) * (k - f as f64)
}

fn write_wav(path: &Path, x: &[f32]) -> Res<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    {
        let mut iw = w.get_i16_writer(x.len() as u32);
        for &v in x {
            iw.write_sample((v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16);
        }
        iw.flush()?;
    }
    w.finalize()?;
    Ok(())
}

/// The loopback AGC (`audio::gain`, default settings) replayed backwards:
/// for each 160-sample block of the stored (post-AGC) loopback, the raw
/// block peak whose forward gain reproduces the stored peak (bisection;
/// where several do, the middle in dB), then the block divided by that
/// gain. Assumes the AGC's blocks start at multiples of 160 (one packet per
/// 10 ms). Returns the raw signal and the gain per block in dB.
fn undo_agc(stored: &[f32]) -> (Vec<f32>, Vec<f32>) {
    const TARGET: f32 = -20.0;
    const MAX_GAIN: f32 = 40.0;
    const FLOOR: f32 = -85.0;
    const DOWN: f32 = 40.0 / 1.0 * 0.01;
    const UP: f32 = 40.0 / 5.0 * 0.01;
    let dbfs = |p: f32| if p <= 0.0 { -120.0 } else { 20.0 * p.log10() };
    let lin = |g: f32| 10f32.powf(g / 20.0);
    let mut win: std::collections::VecDeque<f32> = std::collections::VecDeque::with_capacity(201);
    let (mut gain, mut locked) = (0.0f32, false);
    let mut raw = vec![0.0f32; stored.len()];
    let mut gains = Vec::with_capacity(stored.len() / 160 + 1);
    for (bi, block) in stored.chunks(160).enumerate() {
        // The window after this block's push, without this block.
        let skip = usize::from(win.len() >= 200);
        let rest = win.iter().skip(skip).copied().fold(0.0f32, f32::max);
        let gain_for = |p: f32| -> f32 {
            let wdb = dbfs(rest.max(p));
            let mut g = gain;
            if wdb > FLOOR {
                let desired = (TARGET - wdb).clamp(0.0, MAX_GAIN);
                g = if !locked {
                    desired
                } else if desired < g {
                    (g - DOWN).max(desired)
                } else {
                    (g + UP).min(desired)
                };
            }
            if p > 0.0 {
                let safe = (-1.0 - dbfs(p)).max(0.0);
                if g > safe {
                    g = safe;
                }
            }
            g
        };
        let peak_i = block
            .iter()
            .fold(0.0f32, |m, v| m.max((v * 32_768.0).abs()));
        let p = if peak_i == 0.0 {
            0.0
        } else {
            let f = |p: f32| {
                let g = gain_for(p);
                p * 32_768.0 * if g > 0.0 { lin(g) } else { 1.0 }
            };
            let lo0 = peak_i / 32_768.0 / lin(MAX_GAIN) * 0.99;
            let hi0 = peak_i / 32_768.0 * 1.01;
            let bisect = |want: f32, above: bool| -> f32 {
                let (mut lo, mut hi) = (lo0.ln(), hi0.ln());
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    let v = f(mid.exp());
                    if (above && v >= want) || (!above && v > want) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                (0.5 * (lo + hi)).exp()
            };
            let p_lo = bisect(peak_i - 0.5, true);
            let p_hi = bisect(peak_i + 0.5, false);
            (p_lo * p_hi.max(p_lo)).sqrt()
        };
        let g = gain_for(p);
        if dbfs(rest.max(p)) > FLOOR {
            locked = true;
        }
        gain = g;
        win.push_back(p);
        while win.len() > 200 {
            win.pop_front();
        }
        let scale = if g > 0.0 { 1.0 / lin(g) } else { 1.0 };
        for (r, &v) in raw[bi * 160..].iter_mut().zip(block) {
            *r = v * scale;
        }
        gains.push(g);
    }
    (raw, gains)
}

type Plans = (Arc<dyn RealToComplex<f32>>, Arc<dyn ComplexToReal<f32>>);

/// FFT cross-correlation with cached plans.
struct Xcorr {
    planner: RealFftPlanner<f32>,
    plans: HashMap<usize, Plans>,
}

impl Xcorr {
    fn new() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            plans: HashMap::new(),
        }
    }

    /// Peak |normalised cross-correlation| of `x` against `y` over lags
    /// 0..=`maxlag` (`y` holds `x.len() + maxlag` samples).
    fn peak_ncc(&mut self, x: &[f32], y: &[f32], maxlag: usize) -> f64 {
        let n = (x.len() + y.len()).next_power_of_two();
        let (fwd, inv) = {
            let planner = &mut self.planner;
            self.plans
                .entry(n)
                .or_insert_with(|| (planner.plan_fft_forward(n), planner.plan_fft_inverse(n)))
                .clone()
        };
        let mut a = vec![0.0f32; n];
        a[..x.len()].copy_from_slice(x);
        let mut b = vec![0.0f32; n];
        b[..y.len()].copy_from_slice(y);
        let mut sa = fwd.make_output_vec();
        let mut sb = fwd.make_output_vec();
        fwd.process(&mut a, &mut sa).unwrap();
        fwd.process(&mut b, &mut sb).unwrap();
        for (p, q) in sa.iter_mut().zip(&sb) {
            *p = p.conj() * q;
        }
        sa[0].im = 0.0;
        if let Some(l) = sa.last_mut() {
            l.im = 0.0;
        }
        let mut c = vec![0.0f32; n];
        inv.process(&mut sa, &mut c).unwrap();
        let ex: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
        let mut cs = vec![0f64; y.len() + 1];
        for (i, &v) in y.iter().enumerate() {
            cs[i + 1] = cs[i] + f64::from(v) * f64::from(v);
        }
        let mut best = 0f64;
        for lag in 0..=maxlag.min(y.len().saturating_sub(x.len())) {
            let ey = cs[lag + x.len()] - cs[lag];
            let den = (ex * ey).sqrt();
            if den > 0.0 {
                best = best.max((f64::from(c[lag]) / n as f64).abs() / den);
            }
        }
        best
    }
}

/// Mean over frames of the RMS dB difference of two spectra (32 ms Hann
/// frames, hop 16 ms, 100 Hz to 7 kHz, frames of `a` above −55 dBFS).
fn lsd(a: &[f32], b: &[f32], spans: &[(usize, usize)]) -> f64 {
    let n = 512;
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let win: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let (mut ta, mut tb) = (vec![0.0f32; n], vec![0.0f32; n]);
    let (mut sa, mut sb) = (fwd.make_output_vec(), fwd.make_output_vec());
    let (lo, hi) = (100 * n / 16_000, 7_000 * n / 16_000);
    let (mut sum, mut frames) = (0f64, 0usize);
    for &(s0, s1) in spans {
        let mut i = s0;
        while i + n <= s1.min(a.len()) {
            let e = energy(a, i, i + n);
            if dbfs_power(e, n) > -55.0 {
                for j in 0..n {
                    ta[j] = a[i + j] * win[j];
                    tb[j] = b[i + j] * win[j];
                }
                fwd.process(&mut ta, &mut sa).unwrap();
                fwd.process(&mut tb, &mut sb).unwrap();
                let mut acc = 0f64;
                for k in lo..hi {
                    let d = 10.0 * (f64::from(sb[k].norm_sqr()) + 1e-12).log10()
                        - 10.0 * (f64::from(sa[k].norm_sqr()) + 1e-12).log10();
                    acc += d * d;
                }
                sum += (acc / (hi - lo) as f64).sqrt();
                frames += 1;
            }
            i += n / 2;
        }
    }
    if frames == 0 {
        f64::NAN
    } else {
        sum / frames as f64
    }
}

// ---------------------------------------------------------------- words

#[derive(Debug, Clone)]
struct Word {
    start: f64,
    end: f64,
    norm: String,
}

fn norm(t: &str) -> String {
    t.to_lowercase()
        .chars()
        .filter(|c| (c.is_alphanumeric() || c.is_whitespace()) && *c != '_')
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The engine's words JSON, channel time, sorted by start.
fn load_words(path: &Path) -> Res<Vec<(f64, f64, String)>> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut out: Vec<(f64, f64, String)> = v["words"]
        .as_array()
        .ok_or("no words array")?
        .iter()
        .map(|w| {
            (
                w["start"].as_f64().unwrap_or(0.0),
                w["end"].as_f64().unwrap_or(0.0),
                w["word"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    Ok(out)
}

fn to_meeting(words: &[(f64, f64, String)], map: &Map) -> Vec<Word> {
    words
        .iter()
        .map(|(s, e, t)| Word {
            start: map.word_s(*s),
            end: map.word_s(*e),
            norm: norm(t),
        })
        .collect()
}

const WIN: f64 = 0.8;

struct LbIndex {
    single: HashMap<String, Vec<f64>>,
    tri: HashMap<(String, String, String), Vec<f64>>,
}

impl LbIndex {
    fn new(lb: &[Word]) -> Self {
        let mut single: HashMap<String, Vec<f64>> = HashMap::new();
        for w in lb.iter().filter(|w| !w.norm.is_empty()) {
            single.entry(w.norm.clone()).or_default().push(w.start);
        }
        let mut tri: HashMap<(String, String, String), Vec<f64>> = HashMap::new();
        for j in 0..lb.len().saturating_sub(2) {
            let (a, b, c) = (&lb[j].norm, &lb[j + 1].norm, &lb[j + 2].norm);
            if !a.is_empty() && !b.is_empty() && !c.is_empty() {
                tri.entry((a.clone(), b.clone(), c.clone()))
                    .or_default()
                    .push(lb[j].start);
            }
        }
        for v in single.values_mut().chain(tri.values_mut()) {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
        Self { single, tri }
    }

    fn near(list: Option<&Vec<f64>>, t: f64) -> bool {
        let Some(l) = list else { return false };
        let k = l.partition_point(|&s| s < t - WIN);
        k < l.len() && l[k] <= t + WIN
    }

    /// Single-word matches and duplicates (single or inside a matched 3-run).
    fn classify(&self, mic: &[Word]) -> (Vec<bool>, Vec<bool>) {
        let single: Vec<bool> = mic
            .iter()
            .map(|w| !w.norm.is_empty() && Self::near(self.single.get(&w.norm), w.start))
            .collect();
        let mut covered = vec![false; mic.len()];
        for i in 0..mic.len().saturating_sub(2) {
            let (a, b, c) = (&mic[i].norm, &mic[i + 1].norm, &mic[i + 2].norm);
            if a.is_empty() || b.is_empty() || c.is_empty() {
                continue;
            }
            if Self::near(
                self.tri.get(&(a.clone(), b.clone(), c.clone())),
                mic[i].start,
            ) {
                covered[i] = true;
                covered[i + 1] = true;
                covered[i + 2] = true;
            }
        }
        let dup = single.iter().zip(&covered).map(|(&s, &c)| s || c).collect();
        (single, dup)
    }
}

/// Whether some word of `words` (sorted by start) has text `n` and starts
/// within ±`win` of `t`.
fn has_word(words: &[Word], n: &str, t: f64, win: f64) -> bool {
    let k = words.partition_point(|w| w.start < t - win);
    words[k..]
        .iter()
        .take_while(|w| w.start <= t + win)
        .any(|w| w.norm == n)
}

fn levenshtein(a: &[&str], b: &[&str]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

// ---------------------------------------------------------------- canceller runs

/// Runs `cfg` over whole signals, output aligned with `mic`; `freeze` per
/// block of `cfg.block`, true = no adaptation.
fn cancel(
    cfg: AecConfig,
    mic: &[f32],
    reference: &[f32],
    freeze: Option<&[bool]>,
) -> (Vec<f32>, AecStats, f64) {
    let mut aec = Aec::new(cfg).expect("config");
    let lat = aec.latency();
    let chunk = if freeze.is_some() {
        cfg.block
    } else {
        FRAME_100MS
    };
    let total = (mic.len() + lat).div_ceil(chunk) * chunk;
    let mut out = vec![0.0f32; total];
    let (mut tm, mut tr) = (vec![0.0f32; chunk], vec![0.0f32; chunk]);
    let started = Instant::now();
    let mut i = 0;
    while i < total {
        if let Some(fr) = freeze {
            aec.set_adaptation(!fr.get(i / cfg.block).copied().unwrap_or(false));
        }
        let o = &mut out[i..i + chunk];
        if i + chunk <= mic.len() {
            aec.process(&mic[i..i + chunk], &reference[i..i + chunk], o);
        } else {
            tm.fill(0.0);
            tr.fill(0.0);
            if i < mic.len() {
                let n = mic.len() - i;
                tm[..n].copy_from_slice(&mic[i..]);
                tr[..n].copy_from_slice(&reference[i..]);
            }
            aec.process(&tm, &tr, o);
        }
        i += chunk;
    }
    let secs = started.elapsed().as_secs_f64();
    out.drain(..lat);
    out.truncate(mic.len());
    (out, aec.stats(), secs)
}

// ---------------------------------------------------------------- alignment

/// Places `src` (a loopback signal on its own clock) on the mic's clock:
/// sample `m` of the result is `src` at fractional position `knots` (one
/// knot per `step` mic samples, linear between them), band-limited, or the
/// nearest whole sample with `round`.
fn place(src: &[f32], knots: &[f64], step: usize, frames: usize, round: bool) -> Vec<f32> {
    let interp = SincInterpolator::default();
    let mut out = vec![0.0f32; frames];
    let threads = std::thread::available_parallelism()
        .map_or(8, |n| n.get())
        .min(16);
    let per = frames.div_ceil(threads).div_ceil(step) * step;
    std::thread::scope(|s| {
        for (c, chunk) in out.chunks_mut(per).enumerate() {
            let interp = &interp;
            s.spawn(move || {
                for (j, v) in chunk.iter_mut().enumerate() {
                    let m = c * per + j;
                    let (b, r) = (m / step, (m % step) as f64 / step as f64);
                    let p = knots[b] + (knots[b + 1] - knots[b]) * r;
                    *v = if round {
                        let k = p.round();
                        if k >= 0.0 && (k as usize) < src.len() {
                            src[k as usize]
                        } else {
                            0.0
                        }
                    } else {
                        interp.at(src, p)
                    };
                }
            });
        }
    });
    out
}

/// One lag estimate: window centre (mic sample), lag (samples, the mic
/// after the reference), peak height (0..1).
type LagPoint = (f64, f64, f64);

/// The echo lag of the mic behind the reference per 2 s window (hop 1 s)
/// by GCC-PHAT over 200 Hz to 6 kHz, sub-sample (parabolic), lags −64 to
/// +256 samples, windows with the reference above −45 dBFS and a clear
/// peak.
fn lag_points(mic_h: &[f32], ref_h: &[f32]) -> Vec<LagPoint> {
    const W: usize = 32_000;
    const HOP: usize = 16_000;
    const N: usize = 65_536;
    let (k0, k1) = (200 * N / 16_000, 6_000 * N / 16_000);
    let starts: Vec<usize> = (0..mic_h.len().saturating_sub(W)).step_by(HOP).collect();
    let threads = std::thread::available_parallelism()
        .map_or(8, |n| n.get())
        .min(16);
    let per = starts.len().div_ceil(threads).max(1);
    let mut out: Vec<LagPoint> = std::thread::scope(|s| {
        let hs: Vec<_> = starts
            .chunks(per)
            .map(|chunk| {
                s.spawn(move || {
                    let mut planner = RealFftPlanner::<f32>::new();
                    let fwd = planner.plan_fft_forward(N);
                    let inv = planner.plan_fft_inverse(N);
                    let (mut a, mut b) = (vec![0.0f32; N], vec![0.0f32; N]);
                    let (mut sa, mut sb) = (fwd.make_output_vec(), fwd.make_output_vec());
                    let mut cc = vec![0.0f32; N];
                    let mut pts = Vec::new();
                    for &m0 in chunk {
                        let y = &ref_h[m0..m0 + W];
                        if dbfs_power(energy(y, 0, W), W) < -45.0 {
                            continue;
                        }
                        a.fill(0.0);
                        b.fill(0.0);
                        a[..W].copy_from_slice(&mic_h[m0..m0 + W]);
                        b[..W].copy_from_slice(y);
                        fwd.process(&mut a, &mut sa).unwrap();
                        fwd.process(&mut b, &mut sb).unwrap();
                        for (k, (p, q)) in sa.iter_mut().zip(&sb).enumerate() {
                            let g = *p * q.conj();
                            let mag = g.norm();
                            *p = if (k0..k1).contains(&k) && mag > 0.0 {
                                g / mag
                            } else {
                                Default::default()
                            };
                        }
                        inv.process(&mut sa, &mut cc).unwrap();
                        let norm = 2.0 * (k1 - k0) as f32;
                        let at = |lag: i64| cc[lag.rem_euclid(N as i64) as usize] / norm;
                        let (mut best, mut bl) = (f32::MIN, 0i64);
                        for lag in -64..=256i64 {
                            let v = at(lag);
                            if v > best {
                                best = v;
                                bl = lag;
                            }
                        }
                        if best < 0.07 || bl == -64 || bl == 256 {
                            continue;
                        }
                        let (l, c, r) = (at(bl - 1), best, at(bl + 1));
                        let den = l - 2.0 * c + r;
                        let frac = if den != 0.0 { 0.5 * (l - r) / den } else { 0.0 };
                        pts.push((
                            (m0 + W / 2) as f64,
                            bl as f64 + f64::from(frac),
                            f64::from(best),
                        ));
                    }
                    pts
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out
}

/// The lag as a piecewise-linear function of the mic sample: segments
/// split where the running median (9 windows) jumps by more than 25
/// samples, a least-squares line per segment through the points within
/// 12 samples of the running median.
struct LagTrack {
    /// (first mic sample, intercept, slope) per segment, in order.
    segs: Vec<(f64, f64, f64)>,
    jumps: Vec<(f64, f64)>,
    resid_std: f64,
}

impl LagTrack {
    fn fit(pts: &[LagPoint]) -> Self {
        let n = pts.len();
        let lags: Vec<f64> = pts.iter().map(|p| p.1).collect();
        let med: Vec<f64> = (0..n)
            .map(|i| pct(&lags[i.saturating_sub(4)..(i + 5).min(n)], 0.5))
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
        let (mut rs, mut rn) = (0f64, 0usize);
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
                (pct(&lags[w[0]..w[1]], 0.5), 0.0)
            };
            for &i in &idx {
                rs += (lags[i] - (a + b * pts[i].0)).powi(2);
                rn += 1;
            }
            segs.push((start, a, b));
        }
        Self {
            segs,
            jumps,
            resid_std: (rs / rn.max(1) as f64).sqrt(),
        }
    }

    fn at(&self, m: f64) -> f64 {
        let k = self.segs.partition_point(|s| s.0 <= m).max(1) - 1;
        let (_, a, b) = self.segs[k];
        a + b * m
    }
}

/// The sub-second wobble of the echo delay: per 1 s window (hop 0.5 s),
/// the slope of the cross-spectrum phase over 300 Hz to 3 kHz against the
/// average cross-spectrum of the surrounding ±15 s, in samples (positive:
/// the mic later than usual). Windows with the reference under −45 dBFS
/// are left out.
fn fine_delay(mic_h: &[f32], ref_h: &[f32]) -> Vec<(f64, f64)> {
    const L: usize = 16_000;
    const HOP: usize = 8_000;
    const N: usize = 2_048;
    const FH: usize = 1_024;
    let (k0, k1) = (300 * N / 16_000, 3_000 * N / 16_000);
    let nb = k1 - k0;
    let starts: Vec<usize> = (0..mic_h.len().saturating_sub(L)).step_by(HOP).collect();
    let threads = std::thread::available_parallelism()
        .map_or(8, |n| n.get())
        .min(16);
    let per = starts.len().div_ceil(threads).max(1);
    let win: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos())
        .collect();
    let cs: Vec<Option<Vec<Complex32>>> = std::thread::scope(|s| {
        let hs: Vec<_> = starts
            .chunks(per)
            .map(|chunk| {
                let win = &win;
                s.spawn(move || {
                    let mut planner = RealFftPlanner::<f32>::new();
                    let fwd = planner.plan_fft_forward(N);
                    let (mut a, mut b) = (vec![0.0f32; N], vec![0.0f32; N]);
                    let (mut sa, mut sb) = (fwd.make_output_vec(), fwd.make_output_vec());
                    chunk
                        .iter()
                        .map(|&w0| {
                            if dbfs_power(energy(ref_h, w0, w0 + L), L) < -45.0 {
                                return None;
                            }
                            let mut acc = vec![Complex32::default(); nb];
                            let mut i = w0;
                            while i + N <= w0 + L {
                                for j in 0..N {
                                    a[j] = ref_h[i + j] * win[j];
                                    b[j] = mic_h[i + j] * win[j];
                                }
                                fwd.process(&mut a, &mut sa).unwrap();
                                fwd.process(&mut b, &mut sb).unwrap();
                                for (k, c) in acc.iter_mut().enumerate() {
                                    *c += sa[k0 + k].conj() * sb[k0 + k];
                                }
                                i += FH;
                            }
                            Some(acc)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut out = Vec::new();
    for (w, c) in cs.iter().enumerate() {
        let Some(c) = c else { continue };
        let mut avg = vec![Complex32::default(); nb];
        for v in cs[w.saturating_sub(30)..(w + 31).min(cs.len())]
            .iter()
            .flatten()
        {
            for (a, x) in avg.iter_mut().zip(v) {
                *a += *x;
            }
        }
        let (mut num, mut den) = (0f64, 0f64);
        for (k, (x, a)) in c.iter().zip(&avg).enumerate() {
            let f = (k0 + k) as f64 * 16_000.0 / N as f64;
            let wt = f64::from(x.norm() * a.norm());
            let ph = f64::from((*x * a.conj()).arg());
            num += wt * f * ph;
            den += wt * f * f;
        }
        if den > 0.0 {
            let delay_s = -num / den / std::f64::consts::TAU;
            out.push(((starts[w] + L / 2) as f64, delay_s * 16_000.0));
        }
    }
    out
}

/// Linear interpolation of `(x, y)` points (sorted by x) at `x`, held at
/// the ends.
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

/// Energy of `x` band-passed to `lo`..`hi` Hz (4th-order Butterworth
/// high-pass and low-pass) over `spans`, each filtered with 50 ms of
/// lead-in.
fn band_energy(x: &[f32], spans: &[(usize, usize)], lo: f64, hi: f64) -> f64 {
    let coefs = |f0: f64, q: f64, high: bool| {
        let w0 = std::f64::consts::TAU * f0 / SR;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high {
            ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0)
        } else {
            ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0)
        };
        [b0 / a0, b1 / a0, b2 / a0, -2.0 * c / a0, (1.0 - alpha) / a0]
    };
    let mut sections = Vec::new();
    for q in [0.541_196_1f64, 1.306_563] {
        if lo > 0.0 {
            sections.push(coefs(lo, q, true));
        }
        if hi < SR / 2.0 {
            sections.push(coefs(hi, q, false));
        }
    }
    let mut total = 0f64;
    for &(a, b) in spans {
        let start = a.saturating_sub(800);
        let mut st = vec![[0f64; 2]; sections.len()];
        for (i, &v) in x[start..b.min(x.len())].iter().enumerate() {
            let mut y = f64::from(v);
            for (c, z) in sections.iter().zip(st.iter_mut()) {
                let out = c[0] * y + z[0];
                z[0] = c[1] * y - c[3] * out + z[1];
                z[1] = c[2] * y - c[4] * out;
                y = out;
            }
            if start + i >= a {
                total += y * y;
            }
        }
    }
    total
}

const BANDS: [(f64, f64); 4] = [
    (100.0, 400.0),
    (400.0, 1_000.0),
    (1_000.0, 2_500.0),
    (2_500.0, 8_000.0),
];

fn lag_summary(pts: &[LagPoint]) -> String {
    let lags: Vec<f64> = pts.iter().map(|p| p.1).collect();
    let steps: Vec<f64> = lags.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    format!(
        "{} windows, lag median {:.1} samples ({:.2} ms), p5 {:.1}, p95 {:.1}; window-to-window change median {:.2}, p95 {:.2} samples",
        pts.len(),
        pct(&lags, 0.5),
        pct(&lags, 0.5) / 16.0,
        pct(&lags, 0.05),
        pct(&lags, 0.95),
        pct(&steps, 0.5),
        pct(&steps, 0.95)
    )
}

// ---------------------------------------------------------------- the spike

struct Ctx {
    mic: Vec<f32>,
    mic_h: Vec<f32>,
    refr: Vec<f32>,
    ref_h: Vec<f32>,
    mic_map: Map,
    lb_frames_db: Vec<f32>,
    lb_map: Map,
    words: Vec<Word>,
    dup: Vec<bool>,
    own: Vec<bool>,
    act: Vec<bool>,
    lb_words: Vec<Word>,
    lb_index: LbIndex,
}

impl Ctx {
    fn span(&self, t0: f64, t1: f64) -> (usize, usize) {
        let a = self.mic_map.sample_of(t0).max(0.0) as usize;
        let b = (self.mic_map.sample_of(t1).max(0.0) as usize).min(self.mic.len());
        (a.min(b), b)
    }

    fn word_span(&self, i: usize) -> (usize, usize) {
        let w = &self.words[i];
        self.span(w.start, w.end.max(w.start + 0.02))
    }

    fn lb_active(&self, t0: f64, t1: f64) -> bool {
        let f0 = (self.lb_map.sample_of(t0) / 320.0).floor().max(0.0) as usize;
        let f1 = ((self.lb_map.sample_of(t1) / 320.0).ceil().max(0.0) as usize).max(f0 + 1);
        self.lb_frames_db[f0.min(self.lb_frames_db.len())..f1.min(self.lb_frames_db.len())]
            .iter()
            .any(|&d| d > -50.0)
    }

    fn word_ncc(&self, xc: &mut Xcorr, i: usize, sig: &[f32]) -> f64 {
        let w = &self.words[i];
        let (mut t0, mut t1) = (w.start - 0.16, w.end.max(w.start + 0.08) + 0.16);
        if t1 - t0 < 0.4 {
            let c = (t0 + t1) / 2.0;
            t0 = c - 0.2;
            t1 = c + 0.2;
        }
        let (a, b) = self.span(t0, t1);
        let maxlag = 320;
        if a == 0 || b + maxlag > sig.len() || b <= a {
            return f64::NAN;
        }
        let x = &self.ref_h[a..b];
        if x.iter().all(|&v| v == 0.0) {
            return f64::NAN;
        }
        xc.peak_ncc(x, &sig[a..b + maxlag], maxlag)
    }
}

#[derive(Default)]
struct Row {
    name: String,
    erle: f64,
    erle_word_p10: f64,
    erle_word_p50: f64,
    erle_word_p90: f64,
    buckets: Vec<(f64, usize)>,
    ceiling: f64,
    near_change: f64,
    near_word_p10: f64,
    near_word_p90: f64,
    near_lsd: f64,
    dt_change: f64,
    dt_word_p50: f64,
    ncc_dup_p50: f64,
    ncc_dup_ge035: f64,
    ncc_dt_p50: f64,
    louder_2s: usize,
    stats: AecStats,
    us_per_frame: f64,
    /// ERLE per band of [`BANDS`] on the far-end-only spans.
    bands: Vec<f64>,
}

fn metrics(ctx: &Ctx, name: &str, out: &[f32], stats: AecStats, secs: f64, ncc: bool) -> Row {
    let out_h = highpass(out);
    let mut row = Row {
        name: name.to_owned(),
        stats,
        us_per_frame: secs * 1e6 / (out.len() as f64 / FRAME_100MS as f64),
        ..Row::default()
    };
    // Far end only: the duplicates' spans.
    let (mut em, mut eo, mut per_word) = (0f64, 0f64, Vec::new());
    let nb = (ctx.words.last().map_or(0.0, |w| w.start) / 600.0) as usize + 1;
    let mut bm = vec![0f64; nb];
    let mut bo = vec![0f64; nb];
    let mut bn = vec![0usize; nb];
    for i in (0..ctx.words.len()).filter(|&i| ctx.dup[i] && ctx.act[i]) {
        let (a, b) = ctx.word_span(i);
        let (m, o) = (energy(&ctx.mic_h, a, b), energy(&out_h, a, b));
        em += m;
        eo += o;
        per_word.push(db(m, o));
        let k = (ctx.words[i].start / 600.0) as usize;
        bm[k] += m;
        bo[k] += o;
        bn[k] += 1;
    }
    row.erle = db(em, eo);
    let spans: Vec<(usize, usize)> = (0..ctx.words.len())
        .filter(|&i| ctx.dup[i] && ctx.act[i])
        .map(|i| ctx.word_span(i))
        .collect();
    row.bands = BANDS
        .iter()
        .map(|&(lo, hi)| {
            db(
                band_energy(&ctx.mic_h, &spans, lo, hi),
                band_energy(&out_h, &spans, lo, hi),
            )
        })
        .collect();
    row.erle_word_p10 = pct(&per_word, 0.1);
    row.erle_word_p50 = pct(&per_word, 0.5);
    row.erle_word_p90 = pct(&per_word, 0.9);
    row.buckets = (0..nb).map(|k| (db(bm[k], bo[k]), bn[k])).collect();
    // The noise floor caps what can be measured: 2 s windows with the
    // loopback silent and no mic word.
    let mut floor = Vec::new();
    let mut wi = 0usize;
    let mut t = 0.0;
    while ctx.span(t, t + 2.0).1 < ctx.mic.len() {
        while wi < ctx.words.len() && ctx.words[wi].end < t {
            wi += 1;
        }
        let busy = wi < ctx.words.len() && ctx.words[wi].start < t + 2.0;
        if !busy && !ctx.lb_active(t, t + 2.0) {
            let (a, b) = ctx.span(t, t + 2.0);
            floor.push(energy(&ctx.mic_h, a, b) / (b - a).max(1) as f64);
        }
        t += 2.0;
    }
    let floor_p50 = pct(&floor, 0.5);
    let dup_samples: usize = (0..ctx.words.len())
        .filter(|&i| ctx.dup[i] && ctx.act[i])
        .map(|i| {
            let (a, b) = ctx.word_span(i);
            b - a
        })
        .sum();
    row.ceiling = db(em / dup_samples.max(1) as f64, floor_p50);
    // The owner alone (loopback silent).
    let (mut nm, mut no, mut nw, mut spans) = (0f64, 0f64, Vec::new(), Vec::new());
    for i in (0..ctx.words.len()).filter(|&i| ctx.own[i] && !ctx.act[i]) {
        let (a, b) = ctx.word_span(i);
        let (m, o) = (energy(&ctx.mic_h, a, b), energy(&out_h, a, b));
        nm += m;
        no += o;
        nw.push(db(o, m));
        spans.push((a, b));
    }
    row.near_change = db(no, nm);
    row.near_word_p10 = pct(&nw, 0.1);
    row.near_word_p90 = pct(&nw, 0.9);
    row.near_lsd = lsd(&ctx.mic_h, &out_h, &merge_spans(&spans, 256));
    // Double talk: the owner's words while the far end talks.
    let (mut dm, mut dout, mut dw) = (0f64, 0f64, Vec::new());
    for i in (0..ctx.words.len()).filter(|&i| ctx.own[i] && ctx.act[i]) {
        let (a, b) = ctx.word_span(i);
        let (m, o) = (energy(&ctx.mic_h, a, b), energy(&out_h, a, b));
        dm += m;
        dout += o;
        dw.push(db(o, m));
    }
    row.dt_change = db(dout, dm);
    row.dt_word_p50 = pct(&dw, 0.5);
    // Residual coupling, the word gate's statistic.
    if ncc {
        let mut xc = Xcorr::new();
        let mut d = Vec::new();
        let mut dtn = Vec::new();
        for i in 0..ctx.words.len() {
            if ctx.dup[i] && ctx.act[i] {
                d.push(ctx.word_ncc(&mut xc, i, &out_h));
            } else if ctx.own[i] && ctx.act[i] {
                dtn.push(ctx.word_ncc(&mut xc, i, &out_h));
            }
        }
        let d: Vec<f64> = d.into_iter().filter(|v| v.is_finite()).collect();
        row.ncc_dup_p50 = pct(&d, 0.5);
        row.ncc_dup_ge035 = d.iter().filter(|&&v| v >= 0.35).count() as f64 / d.len().max(1) as f64;
        row.ncc_dt_p50 = pct(&dtn, 0.5);
    } else {
        row.ncc_dup_p50 = f64::NAN;
        row.ncc_dup_ge035 = f64::NAN;
        row.ncc_dt_p50 = f64::NAN;
    }
    // Divergence: 2 s windows where the output is louder than the mic.
    let w2 = 32_000;
    row.louder_2s = (0..out_h.len() / w2)
        .filter(|&k| {
            let (m, o) = (
                energy(&ctx.mic_h, k * w2, (k + 1) * w2),
                energy(&out_h, k * w2, (k + 1) * w2),
            );
            dbfs_power(m, w2) > -60.0 && db(o, m) > 1.0
        })
        .count();
    row
}

fn merge_spans(spans: &[(usize, usize)], min_len: usize) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = spans.to_vec();
    v.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out.retain(|&(a, b)| b - a >= min_len);
    out
}

#[test]
fn aec_spike_meeting() -> Res<()> {
    let (Some(sample), Some(dbp), Some(outdir)) = (
        env_path("DIARIST_ECHO_SAMPLE"),
        env_path("DIARIST_ECHO_DB"),
        env_path("DIARIST_ECHO_OUT"),
    ) else {
        eprintln!("skipped: set DIARIST_ECHO_SAMPLE, DIARIST_ECHO_DB and DIARIST_ECHO_OUT");
        return Ok(());
    };
    std::fs::create_dir_all(&outdir)?;
    let mut rep = String::new();
    macro_rules! say {
        ($($t:tt)*) => {{
            let line = format!($($t)*);
            println!("{line}");
            rep.push_str(&line);
            rep.push('\n');
        }};
    }
    let t_all = Instant::now();
    let uid = sample
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("meeting folder name")?
        .to_owned();

    // Clocks.
    let (meeting_id, anchors) = load_anchors(&dbp, &uid)?;
    let origin = origin_from_anchors(&anchors).ok_or("no anchors")?;
    let pick = |c: Channel| -> Vec<ClockAnchor> {
        anchors.iter().filter(|a| a.channel == c).copied().collect()
    };
    let mic_map = Map::new(origin, &pick(Channel::Mic));
    let lb_map = Map::new(origin, &pick(Channel::Loopback));
    say!(
        "meeting {meeting_id} ({uid}): anchors mic {} (drift {:+.1} ppm), loopback {} ({:+.1} ppm)",
        mic_map.pts.len(),
        mic_map.cm.drift_ppm().unwrap_or(f64::NAN),
        lb_map.pts.len(),
        lb_map.cm.drift_ppm().unwrap_or(f64::NAN)
    );

    // Audio.
    let t0 = Instant::now();
    let (pcm, ch) = decode_i16(&sample.join("playback.flac"))?;
    assert_eq!(ch, 2, "stereo playback track");
    let frames = pcm.len() / 2;
    let mic: Vec<f32> = (0..frames)
        .map(|i| f32::from(pcm[2 * i]) / 32_768.0)
        .collect();
    let lb: Vec<f32> = (0..frames)
        .map(|i| f32::from(pcm[2 * i + 1]) / 32_768.0)
        .collect();
    drop(pcm);
    say!(
        "decoded {:.1} min in {:.1} s",
        frames as f64 / SR / 60.0,
        t0.elapsed().as_secs_f64()
    );
    write_wav(&outdir.join("mic.wav"), &mic)?;
    write_wav(&outdir.join("loopback.wav"), &lb)?;

    // The loopback AGC, replayed backwards (an experiment: what a canceller
    // fed the pre-AGC loopback would get).
    let (lb_raw, gains) = undo_agc(&lb);
    {
        let n = gains.len().max(1) as f64;
        let on = |t: f32| gains.iter().filter(|&&g| g > t).count() as f64 / n;
        let active: Vec<f64> = gains
            .iter()
            .filter(|&&g| g > 0.1)
            .map(|&g| f64::from(g))
            .collect();
        say!(
            "loopback AGC replayed: gain above 0.1 dB in {:.1} percent of 10 ms blocks, above 6 dB in {:.1}, above 20 dB in {:.1}; median of those above 0.1 dB {:.1} dB",
            100.0 * on(0.1),
            100.0 * on(6.0),
            100.0 * on(20.0),
            pct(&active, 0.5)
        );
    }

    // The reference on the mic's clock through the anchors: mic sample ->
    // QPC -> loopback sample.
    let t0 = Instant::now();
    let pos_at = |m: usize| lb_map.sample_at(mic_map.qpc(m as f64));
    let step = 160usize;
    let knots: Vec<f64> = (0..=frames / step + 1).map(|b| pos_at(b * step)).collect();
    let ref_anchor = place(&lb, &knots, step, frames, false);
    let off = |m: usize| (pos_at(m) - m as f64) / 16.0;
    say!(
        "reference placed on the mic clock in {:.1} s: loopback minus mic index {:+.1} ms at 0, {:+.1} ms at the middle, {:+.1} ms at the end",
        t0.elapsed().as_secs_f64(),
        off(0),
        off(frames / 2),
        off(frames - 1)
    );
    let mic_h = highpass(&mic);
    let ref_anchor_h = highpass(&ref_anchor);

    // The echo lag under the anchors, tracked from the signals, and the
    // reference re-placed so that the lag stays at LAG0.
    const LAG0: f64 = 40.0;
    let t0 = Instant::now();
    let pts = lag_points(&mic_h, &ref_anchor_h);
    let track = LagTrack::fit(&pts);
    say!("echo lag under the anchor mapping: {}", lag_summary(&pts));
    let slopes: Vec<f64> = track.segs.iter().map(|s| s.2 * 1e6).collect();
    say!(
        "lag track: {} segments; jumps at {} min; slope within segments median {:+.2} ppm (p10 {:+.2}, p90 {:+.2}); residual about the fit {:.2} samples; {:.1} s",
        track.segs.len(),
        track
            .jumps
            .iter()
            .map(|(m, d)| format!("{:.1} ({:+.0})", m / SR / 60.0, d))
            .collect::<Vec<_>>()
            .join(", "),
        pct(&slopes, 0.5),
        pct(&slopes, 0.1),
        pct(&slopes, 0.9),
        track.resid_std,
        t0.elapsed().as_secs_f64()
    );
    let knots_t: Vec<f64> = knots
        .iter()
        .enumerate()
        .map(|(b, &p)| p - (track.at((b * step) as f64) - LAG0))
        .collect();
    let ref_tracked = place(&lb, &knots_t, step, frames, false);
    let ref_tracked_h = highpass(&ref_tracked);
    say!(
        "echo lag after re-placing with the track: {}",
        lag_summary(&lag_points(&mic_h, &ref_tracked_h))
    );
    drop(ref_anchor_h);

    // The sub-second wobble on top, from the cross-spectrum phase.
    let t0 = Instant::now();
    let fine = fine_delay(&mic_h, &ref_tracked_h);
    let fine_s: Vec<f64> = {
        // A 3-window median against single-window outliers.
        let v: Vec<f64> = fine.iter().map(|p| p.1).collect();
        (0..v.len())
            .map(|i| pct(&v[i.saturating_sub(1)..(i + 2).min(v.len())], 0.5))
            .collect()
    };
    let fine_pts: Vec<(f64, f64)> = fine.iter().zip(&fine_s).map(|(p, &d)| (p.0, d)).collect();
    let wob = |v: &[f64]| {
        let us: Vec<f64> = v.iter().map(|d| d / 16.0 * 1_000.0).collect();
        let mean = us.iter().sum::<f64>() / us.len().max(1) as f64;
        let std =
            (us.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / us.len().max(1) as f64).sqrt();
        let steps: Vec<f64> = us.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        format!("std {std:.1} us, p95 |step| {:.1} us", pct(&steps, 0.95))
    };
    let knots_f: Vec<f64> = knots_t
        .iter()
        .enumerate()
        .map(|(b, &p)| p - interp_points(&fine_pts, (b * step) as f64))
        .collect();
    let ref_fine = place(&lb, &knots_f, step, frames, false);
    let ref_fine_h = highpass(&ref_fine);
    let after: Vec<f64> = fine_delay(&mic_h, &ref_fine_h)
        .iter()
        .map(|p| p.1)
        .collect();
    say!(
        "fine delay (1 s windows, phase slope 300 Hz to 3 kHz, {} windows): {} after the lag track; {} after correcting it; {:.1} s",
        fine.len(),
        wob(&fine.iter().map(|p| p.1).collect::<Vec<_>>()),
        wob(&after),
        t0.elapsed().as_secs_f64()
    );
    drop(ref_tracked_h);

    // Word classes.
    let mic_words = to_meeting(&load_words(&sample.join("mic.words.json"))?, &mic_map);
    let lb_words = to_meeting(&load_words(&sample.join("loopback.words.json"))?, &lb_map);
    let lb_index = LbIndex::new(&lb_words);
    let (single, dup) = lb_index.classify(&mic_words);
    let nw = mic_words.len();
    let own: Vec<bool> = (0..nw)
        .map(|i| {
            if dup[i] {
                return false;
            }
            let lo = i.saturating_sub(5);
            let hi = (i + 6).min(nw);
            let nb: Vec<bool> = (lo..hi).filter(|&j| j != i).map(|j| dup[j]).collect();
            !nb.is_empty() && nb.iter().filter(|&&d| d).count() as f64 / nb.len() as f64 <= 0.3
        })
        .collect();
    let lb_h = highpass(&lb);
    let lb_frames_db: Vec<f32> = lb_h
        .as_chunks::<320>()
        .0
        .iter()
        .map(|c| (10.0 * (energy(c, 0, 320) / 320.0).max(1e-14).log10()) as f32)
        .collect();
    drop(lb_h);
    let mut ctx = Ctx {
        mic,
        mic_h,
        refr: ref_fine,
        ref_h: ref_fine_h,
        mic_map,
        lb_frames_db,
        lb_map,
        words: mic_words,
        dup,
        own,
        act: Vec::new(),
        lb_words,
        lb_index,
    };
    ctx.act = (0..nw)
        .map(|i| {
            let w = &ctx.words[i];
            ctx.lb_active(w.start, w.end)
        })
        .collect();
    let count = |f: &dyn Fn(usize) -> bool| (0..nw).filter(|&i| f(i)).count();
    let runs = {
        let mut r: Vec<(usize, usize)> = Vec::new();
        for i in (0..nw).filter(|&i| ctx.own[i]) {
            match r.last_mut() {
                Some(l) if l.1 + 1 == i => l.1 = i,
                _ => r.push((i, i)),
            }
        }
        r
    };
    say!(
        "word classes: mic {nw}, loopback {}; single-match {:.1} percent; duplicates {}; non-duplicates {}; owner's {} in {} runs; owner's during far-end speech {}; misrecognised echo during far-end speech {}",
        ctx.lb_words.len(),
        100.0 * single.iter().filter(|&&s| s).count() as f64 / nw as f64,
        count(&|i| ctx.dup[i]),
        count(&|i| !ctx.dup[i]),
        count(&|i| ctx.own[i]),
        runs.len(),
        count(&|i| ctx.own[i] && ctx.act[i]),
        count(&|i| !ctx.dup[i] && !ctx.own[i] && ctx.act[i]),
    );

    // Canceller variants, in two batches (memory), metrics in the same
    // thread. References: the stored (post-AGC) loopback through the
    // anchors, the same rounded to whole samples, through the anchors and
    // the lag track, and the AGC undone (anchors only; with the track).
    let ref_round = place(&lb, &knots, step, frames, true);
    let ref_raw = place(&lb_raw, &knots, step, frames, false);
    let ref_raw_tracked = place(&lb_raw, &knots_f, step, frames, false);
    drop(lb);
    drop(lb_raw);
    let base = AecConfig::default();
    let res = AecConfig {
        residual: Some(ResidualConfig::default()),
        ..base
    };
    let refs: [&[f32]; 6] = [
        &ref_anchor,
        &ref_round,
        &ref_tracked,
        &ref_raw,
        &ctx.refr,
        &ref_raw_tracked,
    ];
    // (name, config, reference index, keep the output, gate statistic)
    let variants: Vec<(&str, AecConfig, usize, bool, bool)> = vec![
        ("anchors, 200 ms", base, 0, false, true),
        ("anchors, 200 ms + residual", res, 0, true, false),
        ("anchors, whole-sample reference", base, 1, false, false),
        ("anchors, AGC undone", base, 3, false, false),
        ("lag track, 200 ms", base, 2, false, false),
        ("lag track + fine, 200 ms", base, 4, true, true),
        ("lag track + fine, 200 ms + residual", res, 4, true, true),
        (
            "lag track + fine, 130 ms",
            AecConfig {
                filter_len: 2_048,
                ..base
            },
            4,
            false,
            false,
        ),
        (
            "lag track + fine, 260 ms",
            AecConfig {
                filter_len: 4_096,
                ..base
            },
            4,
            false,
            false,
        ),
        (
            "lag track + fine, 20 ms blocks",
            AecConfig { block: 320, ..base },
            4,
            false,
            false,
        ),
        ("lag track + fine, AGC undone", base, 5, true, true),
        (
            "lag track + fine, AGC undone + residual",
            res,
            5,
            true,
            true,
        ),
        // The causal configurations: clock anchors only, no look-ahead
        // trackers (live; the post-pass once the gain track exists).
        ("anchors, AGC undone + residual", res, 3, true, false),
    ];
    let t0 = Instant::now();
    let mut rows: Vec<Row> = Vec::new();
    let mut kept: HashMap<usize, Vec<f32>> = HashMap::new();
    for batch in (0..variants.len()).collect::<Vec<_>>().chunks(4) {
        let done: Vec<(usize, Row, Option<Vec<f32>>)> = std::thread::scope(|s| {
            let hs: Vec<_> = batch
                .iter()
                .map(|&k| {
                    let (name, cfg, ri, keep, ncc) = variants[k];
                    let (ctx, r) = (&ctx, refs[ri]);
                    s.spawn(move || {
                        let (out, st, secs) = cancel(cfg, &ctx.mic, r, None);
                        let row = metrics(ctx, name, &out, st, secs, ncc);
                        (k, row, keep.then_some(out))
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (k, row, out) in done {
            rows.push(row);
            if let Some(o) = out {
                kept.insert(k, o);
            }
        }
    }
    rows.sort_by_key(|r| variants.iter().position(|v| v.0 == r.name));
    say!(
        "canceller: {} variants over the meeting in {:.1} s (4 at a time)",
        variants.len(),
        t0.elapsed().as_secs_f64()
    );
    write_wav(&outdir.join("mic-aec.wav"), &kept[&5])?;
    write_wav(&outdir.join("mic-aec-res.wav"), &kept[&6])?;
    write_wav(&outdir.join("mic-aec-raw.wav"), &kept[&10])?;
    write_wav(&outdir.join("mic-aec-raw-res.wav"), &kept[&11])?;
    write_wav(&outdir.join("mic-aec-anchor-res.wav"), &kept[&1])?;
    write_wav(&outdir.join("mic-aec-anchor-raw-res.wav"), &kept[&12])?;
    write_wav(&outdir.join("reference.wav"), &ctx.refr)?;
    write_wav(&outdir.join("reference-raw.wav"), &ref_raw_tracked)?;
    drop(ref_round);
    drop(ref_tracked);

    say!("");
    say!("Signal metrics (100 Hz high-passed; far end only = the duplicates' spans while the loopback is active; owner alone = owner's words with the loopback silent; double talk = owner's words while it is active)");
    say!("| Variant | ERLE, far end only | per word p10 / p50 / p90 | Owner alone: level change | per word p10 / p90 | Log-spectral distance | Double talk: level change | Resets | 2 s windows louder | us per 100 ms |");
    say!("|---|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        say!(
            "| {} | {:.1} dB | {:.1} / {:.1} / {:.1} | {:+.2} dB | {:+.2} / {:+.2} | {:.2} dB | {:+.2} dB | {} | {} | {:.0} |",
            r.name,
            r.erle,
            r.erle_word_p10,
            r.erle_word_p50,
            r.erle_word_p90,
            r.near_change,
            r.near_word_p10,
            r.near_word_p90,
            r.near_lsd,
            r.dt_change,
            r.stats.resets,
            r.louder_2s,
            r.us_per_frame
        );
    }
    say!(
        "measurement ceiling: the echo at the mic over the mic noise floor is {:.1} dB on the duplicates' spans",
        rows[0].ceiling
    );
    say!("");
    say!("ERLE per band, far end only:");
    say!("| Variant | 100-400 Hz | 400-1000 Hz | 1-2.5 kHz | 2.5-8 kHz |");
    say!("|---|---|---|---|---|");
    for r in &rows {
        say!(
            "| {} | {} |",
            r.name,
            r.bands
                .iter()
                .map(|b| format!("{b:.1} dB"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    say!("");
    say!("ERLE per 10-minute bucket (far end only), dB (duplicate words in the bucket):");
    for r in &rows {
        let mut line = format!("| {} |", r.name);
        for (k, &(e, n)) in r.buckets.iter().enumerate() {
            let _ = write!(line, " {}: {:.1} ({n}) |", k * 10, e);
        }
        say!("{line}");
    }
    say!("");
    for r in rows.iter().filter(|r| r.ncc_dup_p50.is_finite()) {
        say!(
            "{}: word-gate statistic on the output: duplicates' NCC median {:.2} (raw mic 0.74), {:.1} percent at or above 0.35; owner's double-talk words' NCC median {:.2} (raw 0.06); {:?}",
            r.name,
            r.ncc_dup_p50,
            100.0 * r.ncc_dup_ge035,
            r.ncc_dt_p50,
            r.stats
        );
    }

    // The double-talk words against an oracle: the same canceller frozen
    // over them (±0.3 s), so its echo estimate there is the one it learnt
    // on the far end alone.
    for (name, k, r) in [
        ("lag track + fine", 5usize, &ctx.refr[..]),
        ("lag track + fine, AGC undone", 10, &ref_raw_tracked[..]),
    ] {
        let mut freeze = vec![false; ctx.mic.len() / base.block + 2];
        for i in (0..nw).filter(|&i| ctx.own[i] && ctx.act[i]) {
            let w = &ctx.words[i];
            let (a, b) = ctx.span(w.start - 0.3, w.end + 0.3);
            for f in freeze
                .iter_mut()
                .take(b / base.block + 1)
                .skip(a / base.block)
            {
                *f = true;
            }
        }
        let (frozen, _, _) = cancel(base, &ctx.mic, r, Some(&freeze));
        let notched = Notch::apply(&ctx.mic);
        let est_h = highpass(
            &notched
                .iter()
                .zip(&frozen)
                .map(|(m, o)| m - o)
                .collect::<Vec<_>>(),
        );
        drop(notched);
        let frozen_h = highpass(&frozen);
        let out_h = highpass(&kept[&k]);
        let (mut pm, mut pe, mut pf, mut po) = (0f64, 0f64, 0f64, 0f64);
        for i in (0..nw).filter(|&i| ctx.own[i] && ctx.act[i]) {
            let (a, b) = ctx.word_span(i);
            pm += energy(&ctx.mic_h, a, b);
            pe += energy(&est_h, a, b);
            pf += energy(&frozen_h, a, b);
            po += energy(&out_h, a, b);
        }
        say!("");
        say!(
            "double talk, {name} ({} words), against the canceller frozen over them: its echo estimate {:+.1} dB against the mic, its output (owner plus residual) {:+.2} dB against the mic; the adaptive output {:+.2} dB against the frozen one",
            count(&|i| ctx.own[i] && ctx.act[i]),
            db(pe, pm),
            db(pf, pm),
            db(po, pf)
        );
    }

    // Spliced double talk with ground truth.
    let dt = spliced_double_talk(
        &ctx,
        &ref_raw_tracked,
        &ref_anchor,
        &ref_raw,
        &outdir,
        &mut rep,
    )?;
    drop(ref_raw_tracked);
    drop(ref_anchor);
    drop(ref_raw);

    // Re-transcriptions, when present.
    score_transcripts(&ctx, &runs, &sample, &outdir, &dt, &mut rep)?;

    say!("");
    say!("total {:.1} s", t_all.elapsed().as_secs_f64());
    std::fs::write(outdir.join("report.txt"), &rep)?;
    Ok(())
}

/// Where the spliced owner's clips sit (seconds from the window start) and
/// the window's first mic sample.
struct Spliced {
    m0: usize,
    clips: Vec<(f64, f64)>,
}

/// The owner's own words recorded while the loopback was silent, added
/// onto a far-end-only stretch of the same recording (its real echo, room
/// and AGC'd reference): the residual echo under the owner is then known
/// exactly.
fn spliced_double_talk(
    ctx: &Ctx,
    ref_raw: &[f32],
    ref_anchor: &[f32],
    ref_anchor_raw: &[f32],
    outdir: &Path,
    rep: &mut String,
) -> Res<Spliced> {
    macro_rules! say {
        ($($t:tt)*) => {{
            let line = format!($($t)*);
            println!("{line}");
            rep.push_str(&line);
            rep.push('\n');
        }};
    }
    let nw = ctx.words.len();
    // Utterances of the owner alone: consecutive owner's words, gaps under
    // 0.6 s, the loopback silent over the padded span.
    let mut utts: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < nw {
        if !(ctx.own[i] && !ctx.act[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < nw
            && ctx.own[j + 1]
            && !ctx.act[j + 1]
            && ctx.words[j + 1].start - ctx.words[j].end < 0.6
        {
            j += 1;
        }
        let (t0, t1) = (ctx.words[i].start - 0.25, ctx.words[j].end + 0.35);
        if t1 - t0 >= 0.8 && !ctx.lb_active(t0, t1) {
            utts.push((t0, t1));
        }
        i = j + 1;
    }
    // The far-end window: 10 minutes, most far-end duplicates, no owner's
    // word within it or the minute before.
    let len_s = 600.0;
    let end_s = ctx.words.last().map_or(0.0, |w| w.end);
    let mut best = (0usize, 0.0f64);
    let mut w0 = 60.0;
    while w0 + len_s < end_s {
        let in_win = |i: usize| ctx.words[i].start >= w0 - 60.0 && ctx.words[i].start < w0 + len_s;
        let owners = (0..nw).filter(|&i| in_win(i) && ctx.own[i]).count();
        let dups = (0..nw)
            .filter(|&i| in_win(i) && ctx.dup[i] && ctx.act[i])
            .count();
        if owners == 0 && dups > best.0 {
            best = (dups, w0);
        }
        w0 += 60.0;
    }
    let w0 = best.1;
    let (m0, m1) = ctx.span(w0, w0 + len_s);
    let wl = m1 - m0;
    // Place the clips one after another with 1.5 s between them.
    let mut near = vec![0.0f32; wl];
    let mut clips = Vec::new();
    let mut pos = 5.0 * SR;
    for &(t0, t1) in &utts {
        let (a, b) = ctx.span(t0, t1);
        let n = b - a;
        if pos as usize + n + 16_000 > wl {
            break;
        }
        let p = pos as usize;
        near[p..p + n].copy_from_slice(&ctx.mic[a..b]);
        clips.push((p as f64 / SR, (p + n) as f64 / SR));
        pos += n as f64 + 1.5 * SR;
    }
    let clip_samples: Vec<(usize, usize)> = clips
        .iter()
        .map(|&(a, b)| ((a * SR) as usize, (b * SR) as usize))
        .collect();
    let over_far: usize = clip_samples
        .iter()
        .map(|&(a, b)| {
            (a..b)
                .step_by(320)
                .filter(|&s| {
                    let t = ctx.mic_map.meeting_s((m0 + s) as f64);
                    ctx.lb_active(t, t + 0.02)
                })
                .count()
                * 320
        })
        .sum();
    let clip_total: usize = clip_samples.iter().map(|&(a, b)| b - a).sum();
    say!("");
    say!(
        "spliced double talk: {} of {} owner-alone utterances ({:.0} s) placed on the far-end window {:.0} to {:.0} min ({} far-end duplicate words); {:.0} percent of the owner's samples fall on active far-end frames",
        clips.len(),
        utts.len(),
        clip_total as f64 / SR,
        w0 / 60.0,
        (w0 + len_s) / 60.0,
        best.0,
        100.0 * over_far as f64 / clip_total.max(1) as f64
    );
    // Run from a minute before the window so the filter has converged.
    let pre = m0.saturating_sub(60 * 16_000);
    let echo_full: Vec<f32> = ctx.mic[pre..m1].to_vec();
    let mut near_full = vec![0.0f32; m1 - pre];
    near_full[m0 - pre..].copy_from_slice(&near);
    let mix_full: Vec<f32> = echo_full
        .iter()
        .zip(&near_full)
        .map(|(e, n)| e + n)
        .collect();
    let notch_near = highpass(&Notch::apply(&near_full));
    let notch_echo = highpass(&Notch::apply(&echo_full));
    let mix_h = highpass(&mix_full);
    let base = AecConfig::default();
    let res = AecConfig {
        residual: Some(ResidualConfig::default()),
        ..base
    };
    let mut outs = Vec::new();
    say!("| Variant | Owner over echo, before | after (owner over residual echo) | Echo removed under the owner | Echo removed, rest of the window | Output against the owner alone |");
    say!("|---|---|---|---|---|---|");
    for (name, cfg, reference) in [
        ("lag track + fine, 200 ms", base, &ctx.refr[pre..m1]),
        (
            "lag track + fine, 200 ms + residual",
            res,
            &ctx.refr[pre..m1],
        ),
        ("lag track + fine, AGC undone", base, &ref_raw[pre..m1]),
        (
            "lag track + fine, AGC undone + residual",
            res,
            &ref_raw[pre..m1],
        ),
        ("anchors, 200 ms + residual", res, &ref_anchor[pre..m1]),
        (
            "anchors, AGC undone + residual",
            res,
            &ref_anchor_raw[pre..m1],
        ),
    ] {
        let (out, _, _) = cancel(cfg, &mix_full, reference, None);
        let out_h = highpass(&out);
        // out = notch(echo + near) - y, so out - notch(near) is the residual
        // echo exactly (for the linear filter).
        let resid: Vec<f32> = out_h.iter().zip(&notch_near).map(|(o, n)| o - n).collect();
        let (mut pn, mut pe, mut pr, mut po) = (0f64, 0f64, 0f64, 0f64);
        for &(a, b) in &clip_samples {
            let (a, b) = (a + m0 - pre, b + m0 - pre);
            pn += energy(&notch_near, a, b);
            pe += energy(&notch_echo, a, b);
            pr += energy(&resid, a, b);
            po += energy(&out_h, a, b);
        }
        // The rest of the window (far end only).
        let mut gaps = Vec::new();
        let mut last = m0 - pre;
        for &(a, b) in &clip_samples {
            gaps.push((last, a + m0 - pre));
            last = b + m0 - pre;
        }
        gaps.push((last, m1 - pre));
        let (ge, go) = gaps.iter().fold((0f64, 0f64), |acc, &(a, b)| {
            (acc.0 + energy(&mix_h, a, b), acc.1 + energy(&out_h, a, b))
        });
        say!(
            "| {name} | {:+.1} dB | {:+.1} dB | {:.1} dB | {:.1} dB | {:+.2} dB |",
            db(pn, pe),
            db(pn, pr),
            db(pe, pr),
            db(ge, go),
            db(po, pn)
        );
        outs.push(out);
    }
    write_wav(&outdir.join("dt-near.wav"), &near)?;
    write_wav(&outdir.join("dt-mix.wav"), &mix_full[m0 - pre..])?;
    write_wav(&outdir.join("dt-mix-aec.wav"), &outs[0][m0 - pre..])?;
    write_wav(&outdir.join("dt-mix-aec-res.wav"), &outs[1][m0 - pre..])?;
    write_wav(&outdir.join("dt-mix-aec-raw.wav"), &outs[2][m0 - pre..])?;
    write_wav(
        &outdir.join("dt-mix-aec-anchor-res.wav"),
        &outs[4][m0 - pre..],
    )?;
    write_wav(
        &outdir.join("dt-mix-aec-anchor-raw-res.wav"),
        &outs[5][m0 - pre..],
    )?;
    Ok(Spliced { m0, clips })
}

fn score_transcripts(
    ctx: &Ctx,
    runs: &[(usize, usize)],
    sample: &Path,
    outdir: &Path,
    dt: &Spliced,
    rep: &mut String,
) -> Res<()> {
    macro_rules! say {
        ($($t:tt)*) => {{
            let line = format!($($t)*);
            println!("{line}");
            rep.push_str(&line);
            rep.push('\n');
        }};
    }
    let nw = ctx.words.len();
    let names = [
        "mic",
        "mic-aec",
        "mic-aec-res",
        "mic-aec-raw",
        "mic-aec-raw-res",
        "mic-aec-anchor-res",
        "mic-aec-anchor-raw-res",
    ];
    let present: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| outdir.join(format!("{n}.asr.json")).is_file())
        .collect();
    say!("");
    if present.is_empty() {
        say!("re-transcriptions: none in the output folder yet (<wav stem>.asr.json)");
    } else {
        // Baseline: the re-run on mic.wav against the stored mic.words.json.
        if present.contains(&"mic") {
            let stored = load_words(&sample.join("mic.words.json"))?;
            let rerun = load_words(&outdir.join("mic.asr.json"))?;
            let sw: Vec<Word> = stored
                .iter()
                .map(|(s, e, t)| Word {
                    start: *s,
                    end: *e,
                    norm: norm(t),
                })
                .collect();
            let same = rerun
                .iter()
                .filter(|(s, _, t)| has_word(&sw, &norm(t), *s, 0.05))
                .count();
            say!(
                "baseline: mic.wav re-transcribed {} words against the stored {}; {:.1} percent of the re-run's words are in the stored file (same text, start within 50 ms)",
                rerun.len(),
                stored.len(),
                100.0 * same as f64 / rerun.len().max(1) as f64
            );
        }
        let own_words: Vec<usize> = (0..nw).filter(|&i| ctx.own[i]).collect();
        let dt_words: Vec<usize> = (0..nw).filter(|&i| ctx.own[i] && ctx.act[i]).collect();
        let run_spans: Vec<(f64, f64)> = runs
            .iter()
            .map(|&(a, b)| (ctx.words[a].start, ctx.words[b].end))
            .collect();
        let own_list: Vec<Word> = own_words.iter().map(|&i| ctx.words[i].clone()).collect();
        say!("| Transcript | Words | Duplicates (share) | Non-duplicates outside the owner's stretches | In the owner's stretches: words | labelled owner's words kept (of {}) | new non-duplicate words | duplicates | Double-talk words kept (of {}) | words inside the double-talk words' spans |", own_words.len(), dt_words.len());
        say!("|---|---|---|---|---|---|---|---|---|---|");
        for n in &present {
            let words = to_meeting(
                &load_words(&outdir.join(format!("{n}.asr.json")))?,
                &ctx.mic_map,
            );
            let (_, dup) = ctx.lb_index.classify(&words);
            let ndup = dup.iter().filter(|&&d| d).count();
            let in_runs = |w: &Word| {
                let mid = (w.start + w.end) / 2.0;
                run_spans.iter().any(|&(a, b)| mid >= a && mid <= b)
            };
            let mut in_run = 0;
            let mut new = 0;
            let mut dup_run = 0;
            let mut garbled = 0;
            for (k, w) in words.iter().enumerate() {
                if !in_runs(w) {
                    garbled += usize::from(!dup[k]);
                    continue;
                }
                in_run += 1;
                if dup[k] {
                    dup_run += 1;
                } else if !has_word(&own_list, &w.norm, w.start, WIN) {
                    new += 1;
                }
            }
            let kept = own_words
                .iter()
                .filter(|&&i| has_word(&words, &ctx.words[i].norm, ctx.words[i].start, WIN))
                .count();
            let dt_kept = dt_words
                .iter()
                .filter(|&&i| has_word(&words, &ctx.words[i].norm, ctx.words[i].start, WIN))
                .count();
            let in_dt = words
                .iter()
                .filter(|w| {
                    let mid = (w.start + w.end) / 2.0;
                    dt_words
                        .iter()
                        .any(|&i| mid >= ctx.words[i].start - 0.1 && mid <= ctx.words[i].end + 0.1)
                })
                .count();
            say!(
                "| {n} | {} | {ndup} ({:.1} percent) | {garbled} | {in_run} | {kept} | {new} | {dup_run} | {dt_kept} | {in_dt} |",
                words.len(),
                100.0 * ndup as f64 / words.len().max(1) as f64
            );
        }
    }

    // The spliced double talk.
    let dt_names = [
        "dt-near",
        "dt-mix",
        "dt-mix-aec",
        "dt-mix-aec-res",
        "dt-mix-aec-raw",
        "dt-mix-aec-anchor-res",
        "dt-mix-aec-anchor-raw-res",
    ];
    if !outdir.join("dt-near.asr.json").is_file() {
        say!("spliced double talk: dt-near.asr.json not there yet");
        return Ok(());
    }
    let load_dt = |n: &str| -> Res<Vec<Word>> {
        let w = load_words(&outdir.join(format!("{n}.asr.json")))?;
        Ok(w.iter()
            .map(|(s, e, t)| Word {
                start: *s,
                end: *e,
                norm: norm(t),
            })
            .collect())
    };
    let reference = load_dt("dt-near")?;
    let in_clip = |w: &Word, &(a, b): &(f64, f64)| {
        let mid = (w.start + w.end) / 2.0;
        mid >= a - 0.1 && mid <= b + 0.1
    };
    let ref_n: usize = dt
        .clips
        .iter()
        .map(|c| reference.iter().filter(|w| in_clip(w, c)).count())
        .sum();
    say!(
        "| Spliced double talk | Owner's words (reference: dt-near, {ref_n}) word error rate | owner's words matched | words in the owner's clips that are echo duplicates | duplicates outside the clips | non-duplicates outside the clips |"
    );
    say!("|---|---|---|---|---|---|");
    for n in dt_names.iter().skip(1) {
        if !outdir.join(format!("{n}.asr.json")).is_file() {
            continue;
        }
        let hyp = load_dt(n)?;
        // Echo duplicates: against the loopback words in meeting time.
        let meet: Vec<Word> = hyp
            .iter()
            .map(|w| Word {
                start: ctx.mic_map.meeting_s(dt.m0 as f64 + w.start * SR),
                end: ctx.mic_map.meeting_s(dt.m0 as f64 + w.end * SR),
                norm: w.norm.clone(),
            })
            .collect();
        let (_, dup) = ctx.lb_index.classify(&meet);
        let (mut edits, mut matched, mut dup_in, mut dup_out) = (0usize, 0usize, 0usize, 0usize);
        for c in &dt.clips {
            let r: Vec<&str> = reference
                .iter()
                .filter(|w| in_clip(w, c))
                .map(|w| w.norm.as_str())
                .collect();
            let h: Vec<&str> = hyp
                .iter()
                .filter(|w| in_clip(w, c))
                .map(|w| w.norm.as_str())
                .collect();
            edits += levenshtein(&r, &h);
            matched += r.iter().filter(|&&t| h.contains(&t)).count().min(h.len());
        }
        let mut other_out = 0usize;
        for (k, w) in hyp.iter().enumerate() {
            let inside = dt.clips.iter().any(|c| in_clip(w, c));
            match (dup[k], inside) {
                (true, true) => dup_in += 1,
                (true, false) => dup_out += 1,
                (false, false) => other_out += 1,
                (false, true) => {}
            }
        }
        say!(
            "| {n} | {:.1} percent | {matched} | {dup_in} | {dup_out} | {other_out} |",
            100.0 * edits as f64 / ref_n.max(1) as f64
        );
    }
    Ok(())
}
