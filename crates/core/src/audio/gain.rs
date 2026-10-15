//! Automatic gain per channel (on by default for the loopback, off for the
//! mic; settings `capture.loopback_agc` / `capture.mic_agc`).
//!
//! Loopback audio arrives after the output's software volume, so a user
//! whose Windows volume sits at a few percent would feed the engine (and the
//! archive) near-silence. [`Agc`] measures the peak over the last 2 s in
//! 10 ms blocks and steers a gain so peaks sit near -20 dBFS: up to +40 dB,
//! never below 0 dB (loud input passes unchanged). The first active block
//! locks the gain at once; after that it falls at up to 40 dB/s ("attack",
//! about 1 s over the full range) and rises at up to 8 dB/s ("release",
//! about 5 s). No block is ever amplified past the limiter (-1 dBFS): the
//! gain drops instantly for a block that would, and a hard clamp catches the
//! rest. While the 2 s window stays under the noise floor (-85 dBFS) the
//! gain does not move, so silence is never amplified.
//!
//! It runs in the pipeline before the WAV writer and the engine feed, on
//! float samples when the capture delivers them (quiet audio is then not
//! quantized to a few bits first).

use std::collections::VecDeque;

/// AGC parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgcConfig {
    pub target_dbfs: f32,
    pub max_gain_db: f32,
    pub limiter_dbfs: f32,
    /// Below this window peak the gain holds.
    pub noise_floor_dbfs: f32,
    /// Peak window.
    pub window_s: f32,
    /// Seconds for the gain to fall over its whole range.
    pub attack_s: f32,
    /// Seconds for the gain to rise over its whole range.
    pub release_s: f32,
}

impl Default for AgcConfig {
    fn default() -> Self {
        Self {
            target_dbfs: -20.0,
            max_gain_db: 40.0,
            limiter_dbfs: -1.0,
            noise_floor_dbfs: -85.0,
            window_s: 2.0,
            attack_s: 1.0,
            release_s: 5.0,
        }
    }
}

/// Samples per AGC block (10 ms at 16 kHz).
const BLOCK: usize = 160;
const RATE: f32 = 16_000.0;

/// dBFS of a linear peak (−120 for zero).
pub fn dbfs(peak: f32) -> f32 {
    if peak <= 0.0 {
        -120.0
    } else {
        20.0 * peak.log10()
    }
}

fn lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// One channel's gain state.
#[derive(Debug, Clone)]
pub struct Agc {
    cfg: AgcConfig,
    gain_db: f32,
    locked: bool,
    /// Block peaks (linear) over the window.
    window: VecDeque<f32>,
    window_blocks: usize,
    /// Highest raw (pre-gain) peak since the last [`Agc::take_raw_peak`].
    raw_peak: f32,
}

impl Agc {
    pub fn new(cfg: AgcConfig) -> Self {
        let window_blocks = ((cfg.window_s * RATE) as usize / BLOCK).max(1);
        Self {
            cfg,
            gain_db: 0.0,
            locked: false,
            window: VecDeque::with_capacity(window_blocks + 1),
            window_blocks,
            raw_peak: 0.0,
        }
    }

    pub fn config(&self) -> &AgcConfig {
        &self.cfg
    }

    /// Gain applied to the latest block.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// The gain is at its ceiling.
    pub fn at_ceiling(&self) -> bool {
        self.gain_db >= self.cfg.max_gain_db - 0.01
    }

    /// Highest raw peak (linear, 0..1) since the last call.
    pub fn take_raw_peak(&mut self) -> f32 {
        std::mem::take(&mut self.raw_peak)
    }

    /// Updates the gain for a block of `n` samples whose raw peak is
    /// `peak`; returns the gain in dB to apply to it.
    fn update(&mut self, peak: f32, n: usize) -> f32 {
        self.raw_peak = self.raw_peak.max(peak);
        self.window.push_back(peak);
        while self.window.len() > self.window_blocks {
            self.window.pop_front();
        }
        let window_peak = self.window.iter().copied().fold(0.0f32, f32::max);
        let window_db = dbfs(window_peak);
        let c = &self.cfg;
        if window_db > c.noise_floor_dbfs {
            let desired = (c.target_dbfs - window_db).clamp(0.0, c.max_gain_db);
            if !self.locked {
                self.gain_db = desired;
                self.locked = true;
            } else {
                let dt = n as f32 / RATE;
                if desired < self.gain_db {
                    let step = c.max_gain_db / c.attack_s.max(1e-3) * dt;
                    self.gain_db = (self.gain_db - step).max(desired);
                } else {
                    let step = c.max_gain_db / c.release_s.max(1e-3) * dt;
                    self.gain_db = (self.gain_db + step).min(desired);
                }
            }
        }
        // Never push this block past the limiter.
        if peak > 0.0 {
            let safe = (c.limiter_dbfs - dbfs(peak)).max(0.0);
            if self.gain_db > safe {
                self.gain_db = safe;
            }
        }
        self.gain_db
    }

    /// Applies the gain in place to float samples (full scale 1.0).
    pub fn process_f32(&mut self, x: &mut [f32]) {
        let limit = lin(self.cfg.limiter_dbfs);
        for block in x.chunks_mut(BLOCK) {
            let peak = block.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let g_db = self.update(peak, block.len());
            if g_db > 0.0 {
                let g = lin(g_db);
                for s in block.iter_mut() {
                    *s = (*s * g).clamp(-limit, limit);
                }
            }
        }
    }

    /// Applies the gain in place to PCM16. At 0 dB samples pass bit-exact.
    pub fn process_i16(&mut self, x: &mut [i16]) {
        let limit = lin(self.cfg.limiter_dbfs) * 32_768.0;
        for block in x.chunks_mut(BLOCK) {
            let peak = block.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32_768.0;
            let g_db = self.update(peak, block.len());
            if g_db > 0.0 {
                let g = lin(g_db);
                for s in block.iter_mut() {
                    *s = (f32::from(*s) * g).round().clamp(-limit, limit) as i16;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speech-like: 200 ms tone bursts (syllables) with 100 ms gaps and a
    /// slow amplitude wobble, peaking at `peak_dbfs`.
    fn speechish(seconds: f32, peak_dbfs: f32) -> Vec<f32> {
        let a = lin(peak_dbfs);
        (0..(seconds * RATE) as usize)
            .map(|i| {
                let t = i as f32 / RATE;
                let on = (i / 1_600) % 3 != 2;
                let env = 0.6 + 0.4 * (t * 1.3 * std::f32::consts::TAU).sin().abs();
                if on {
                    a * env * (t * 220.0 * std::f32::consts::TAU).sin()
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn peak_db(x: &[f32]) -> f32 {
        dbfs(x.iter().fold(0.0f32, |m, s| m.max(s.abs())))
    }

    #[test]
    fn quiet_speech_is_raised_to_the_target_within_2_s() {
        // -60 dBFS: inside the +40 dB range, so it lands at -20 dBFS.
        let mut agc = Agc::new(AgcConfig::default());
        let mut x = speechish(4.0, -60.0);
        agc.process_f32(&mut x);
        let after_2s = peak_db(&x[32_000..]);
        assert!((after_2s - -20.0).abs() < 1.5, "{after_2s} dBFS");
        assert!(agc.gain_db() > 38.0, "{}", agc.gain_db());
        // -70 dBFS: the +40 dB ceiling caps it at -30 dBFS.
        let mut agc = Agc::new(AgcConfig::default());
        let mut x = speechish(4.0, -70.0);
        agc.process_f32(&mut x);
        let p = peak_db(&x[32_000..]);
        assert!((p - -30.0).abs() < 1.5, "{p} dBFS");
        assert!(agc.at_ceiling());
        assert!((dbfs(agc.take_raw_peak()) - -70.0).abs() < 0.5);
        // The same through PCM16 (already quantized: ~10 LSB peaks).
        let mut agc = Agc::new(AgcConfig::default());
        let mut q: Vec<i16> = speechish(4.0, -70.0)
            .iter()
            .map(|s| (s * 32_768.0).round() as i16)
            .collect();
        agc.process_i16(&mut q);
        let p = dbfs(q[32_000..].iter().map(|s| s.unsigned_abs()).max().unwrap() as f32 / 32_768.0);
        assert!((p - -30.0).abs() < 2.0, "{p} dBFS");
    }

    #[test]
    fn loud_input_is_neither_amplified_nor_clipped() {
        let mut agc = Agc::new(AgcConfig::default());
        let x: Vec<i16> = speechish(3.0, -3.0)
            .iter()
            .map(|s| (s * 32_768.0).round() as i16)
            .collect();
        let mut y = x.clone();
        agc.process_i16(&mut y);
        assert_eq!(x, y, "a -3 dBFS input passes bit-exact");
        assert_eq!(agc.gain_db(), 0.0);
        // Quiet, then suddenly loud: the gain drops at once, nothing clips.
        let mut agc = Agc::new(AgcConfig::default());
        let mut z = speechish(2.0, -60.0);
        z.extend(speechish(2.0, -6.0));
        agc.process_f32(&mut z);
        assert!(peak_db(&z) <= -1.0 + 1e-3, "{}", peak_db(&z));
        assert!(peak_db(&z[40_000..]) > -10.0);
    }

    #[test]
    fn silence_stays_silence() {
        let mut agc = Agc::new(AgcConfig::default());
        let mut z = vec![0f32; 48_000];
        agc.process_f32(&mut z);
        assert!(z.iter().all(|&s| s == 0.0));
        assert_eq!(agc.gain_db(), 0.0);
        // One-LSB noise (-90 dBFS) is under the floor: not amplified.
        let mut n: Vec<i16> = (0..48_000)
            .map(|i| if i % 3 == 0 { 1 } else { -1 })
            .collect();
        let before = n.clone();
        agc.process_i16(&mut n);
        assert_eq!(n, before);
        // After speech, a pause keeps the gain (no pumping on silence).
        let mut agc = Agc::new(AgcConfig::default());
        let mut s = speechish(2.0, -60.0);
        agc.process_f32(&mut s);
        let g = agc.gain_db();
        let mut pause = vec![0f32; 80_000];
        agc.process_f32(&mut pause);
        assert_eq!(agc.gain_db(), g);
        assert!(pause.iter().all(|&s| s == 0.0));
    }
}
