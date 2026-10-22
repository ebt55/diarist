//! Speech-like test signals for the echo tests (no audio files): syllables
//! of a pulse train through two formant resonators (voiced) or of noise
//! through a high resonator (unvoiced), Hann-shaped, grouped into words and
//! turns of a two-party conversation.

/// xorshift64*.
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }

    pub fn noise(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

fn resonate(x: &[f32], f: f32, bw: f32) -> Vec<f32> {
    let r = (-std::f32::consts::PI * bw / 16_000.0).exp();
    let a1 = 2.0 * r * (std::f32::consts::TAU * f / 16_000.0).cos();
    let a2 = -r * r;
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    x.iter()
        .map(|&v| {
            let y = v + a1 * y1 + a2 * y2;
            y2 = y1;
            y1 = y;
            y
        })
        .collect()
}

/// One syllable of `len` samples at RMS `rms` (±1 scale).
fn syllable(rng: &mut Rng, len: usize, rms: f32) -> Vec<f32> {
    let mut s = if rng.unit() < 0.75 {
        let f0 = rng.range(95.0, 210.0);
        let glide = rng.range(-0.15, 0.15);
        let mut phase = 0.0f32;
        let exc: Vec<f32> = (0..len)
            .map(|i| {
                phase += f0 * (1.0 + glide * i as f32 / len as f32) / 16_000.0;
                let pulse = if phase >= 1.0 {
                    phase -= 1.0;
                    1.0
                } else {
                    0.0
                };
                pulse + 0.03 * rng.noise()
            })
            .collect();
        let a = resonate(&exc, rng.range(300.0, 850.0), 90.0);
        let b = resonate(&exc, rng.range(900.0, 2_300.0), 150.0);
        a.iter()
            .zip(&b)
            .map(|(p, q)| p + 0.5 * q)
            .collect::<Vec<f32>>()
    } else {
        let exc: Vec<f32> = (0..len).map(|_| rng.noise()).collect();
        resonate(&exc, rng.range(2_500.0, 5_000.0), 1_200.0)
    };
    for (i, v) in s.iter_mut().enumerate() {
        let w = (std::f32::consts::PI * i as f32 / len as f32).sin();
        *v *= w * w;
    }
    let e = (s.iter().map(|v| v * v).sum::<f32>() / len as f32).sqrt();
    if e > 0.0 {
        s.iter_mut().for_each(|v| *v *= rms / e);
    }
    s
}

/// Words (1 to 3 syllables of 100 to 260 ms) with 60 to 200 ms pauses into
/// `out[from..to]` at RMS `rms`; returns the word spans.
pub(crate) fn talk(
    rng: &mut Rng,
    out: &mut [f32],
    from: usize,
    to: usize,
    rms: f32,
) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut t = from;
    loop {
        let syl = 1 + (rng.next_u64() % 3) as usize;
        let lens: Vec<usize> = (0..syl)
            .map(|_| (rng.range(100.0, 260.0) * 16.0) as usize)
            .collect();
        let total: usize = lens.iter().sum();
        if t + total > to.min(out.len()) {
            break;
        }
        let start = t;
        for len in lens {
            let s = syllable(rng, len, rms);
            for (o, v) in out[t..t + len].iter_mut().zip(&s) {
                *o += v;
            }
            t += len;
        }
        spans.push((start, t));
        t += (rng.range(60.0, 200.0) * 16.0) as usize;
    }
    spans
}

/// A two-party conversation of `len` samples: far-end turns of 3 to 7 s and
/// near-end turns of 1.5 to 4 s, alternating, with 0.5 to 0.9 s between
/// turns (so the gate's padded spans never reach the other side's speech).
pub(crate) struct Conversation {
    pub far: Vec<f32>,
    pub near: Vec<f32>,
    pub far_words: Vec<(usize, usize)>,
    pub near_words: Vec<(usize, usize)>,
}

pub(crate) fn conversation(seed: u64, len: usize, far_rms: f32, near_rms: f32) -> Conversation {
    let mut rng = Rng::new(seed);
    let mut c = Conversation {
        far: vec![0.0; len],
        near: vec![0.0; len],
        far_words: Vec::new(),
        near_words: Vec::new(),
    };
    let mut t = 8_000;
    let mut far_turn = true;
    while t < len {
        let (lo, hi) = if far_turn { (3.0, 7.0) } else { (1.5, 4.0) };
        let end = (t + (rng.range(lo, hi) * 16_000.0) as usize).min(len);
        let words = if far_turn {
            talk(&mut rng, &mut c.far, t, end, far_rms)
        } else {
            talk(&mut rng, &mut c.near, t, end, near_rms)
        };
        let last = words.last().map_or(t, |w| w.1);
        if far_turn {
            c.far_words.extend(words);
        } else {
            c.near_words.extend(words);
        }
        t = last + (rng.range(0.5, 0.9) * 16_000.0) as usize;
        far_turn = !far_turn;
    }
    c
}

/// Far-end speech only (one long turn), for drift bursts.
pub(crate) fn monologue(seed: u64, len: usize, rms: f32) -> (Vec<f32>, Vec<(usize, usize)>) {
    let mut rng = Rng::new(seed);
    let mut out = vec![0.0; len];
    let words = talk(&mut rng, &mut out, 0, len, rms);
    (out, words)
}

/// Adds white noise at RMS `rms`.
pub(crate) fn add_noise(x: &mut [f32], seed: u64, rms: f32) {
    let mut rng = Rng::new(seed);
    // Uniform noise in ±a has RMS a / sqrt(3).
    let a = rms * 3f32.sqrt();
    x.iter_mut().for_each(|v| *v += a * rng.noise());
}

/// `out[i] += gain * x[i - delay]`.
pub(crate) fn add_delayed(out: &mut [f32], x: &[f32], delay: usize, gain: f32) {
    for (i, o) in out.iter_mut().enumerate().skip(delay) {
        if let Some(v) = x.get(i - delay) {
            *o += gain * v;
        }
    }
}

pub(crate) fn to_i16(x: &[f32]) -> Vec<i16> {
    x.iter()
        .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect()
}

/// dBFS to linear RMS.
pub(crate) fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}
