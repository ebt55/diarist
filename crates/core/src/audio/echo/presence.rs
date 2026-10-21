//! Near-end presence, the echo gate's level features (plan 15 section 9,
//! item 2, as revised in section 9.1).
//!
//! - [`Presence::level_db`]: the 90th percentile of the 20 ms speech-band
//!   frame levels of the mic the speech model heard (the canceller's output
//!   in a post-pass that ran it) over `[start − 300 ms, end + 80 ms]`: a
//!   word's stamp is an 80 ms spike after its energy peak, so the span
//!   itself can miss a quiet owner word's energy.
//! - [`Presence::far_active`]: a loopback 20 ms frame at or above
//!   [`FAR_ACTIVE_DBFS`] over `[start − 600 ms, end + 100 ms]` (an echo
//!   word's far-end audio lies up to about 0.65 s before its stamp).
//! - [`Presence::attenuation_db`] (post-pass, after the canceller):
//!   `10·log10(E_mic / E_mic-aec)` over the level's window. Meeting 14: the
//!   owner's words lose 0.2 dB (median, p90 0.4), the surviving duplicates
//!   17.5 dB (median, p10 3.3).
//! - [`FloorTracker`]: the mic's noise floor, a low percentile of the 20 ms
//!   speech-band frame levels where the loopback frame at the same time is
//!   silent (on speakers echo would lift a global percentile), digital
//!   silence left out.

use super::{Avail, ClockPair, EchoAudio, PcmSource, Stream, StreamAudio, BLOCK};
use crate::types::Channel;

/// 20 ms frames.
pub const FRAME: usize = 320;
/// The level's window around a word (samples): 300 ms before, 80 ms after.
pub const LEVEL_BEFORE: i64 = 4_800;
pub const LEVEL_AFTER: i64 = 1_280;
/// The level is this percentile of the window's frame levels.
pub const LEVEL_PERCENTILE: f64 = 0.9;
/// The loopback's window around a word (samples): 600 ms before, 100 ms
/// after.
pub const FAR_BEFORE: i64 = 9_600;
pub const FAR_AFTER: i64 = 1_600;
/// A loopback frame at or above this (speech band, after the AGC) is far-end
/// activity; under it the frame is silent for the floor.
pub const FAR_ACTIVE_DBFS: f32 = -50.0;
/// Mic frames under this are digital silence (a muted device), not the
/// room: left out of the floor.
pub const SILENT_DBFS: f32 = -90.0;
/// The floor is this percentile of the loopback-silent mic frames.
pub const FLOOR_PERCENTILE: f64 = 0.2;
/// Frames before the floor is known (10 s).
pub const FLOOR_MIN_FRAMES: u64 = 500;

/// One mic word's level features.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Presence {
    pub level_db: f32,
    pub far_active: bool,
    /// After the canceller (post-pass): the energy it removed (dB).
    pub attenuation_db: Option<f32>,
}

/// dBFS of each 20 ms frame of `x` (±1 samples; −100 for silence).
pub fn frame_levels(x: &[f32]) -> impl Iterator<Item = f32> + '_ {
    x.chunks(FRAME).map(super::rms_dbfs)
}

/// The `p` percentile (0 to 1, nearest rank) of `v` (reordered).
pub fn percentile(v: &mut [f32], p: f64) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    let k = ((v.len() - 1) as f64 * p).round() as usize;
    let (_, x, _) = v.select_nth_unstable_by(k, f32::total_cmp);
    Some(*x)
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

/// The level window of a word span (timeline samples).
pub fn level_window((start, end): (i64, i64)) -> (i64, usize) {
    let a = start - LEVEL_BEFORE;
    let b = end.max(start) + LEVEL_AFTER;
    (a, (b - a) as usize)
}

/// One word's presence from `heard` (its mic channel is what the speech
/// model heard, its loopback channel the far end) and, after a canceller,
/// `raw` (its mic channel is the uncancelled mic). `Err` when a span has
/// not arrived (live) or is gone.
pub fn presence(
    heard: &mut dyn EchoAudio,
    raw: Option<&mut dyn EchoAudio>,
    span: (i64, i64),
    x: &mut Vec<f32>,
    levels: &mut Vec<f32>,
) -> Result<Presence, Avail> {
    let (a, len) = level_window(span);
    match heard.read(Channel::Mic, a, len, x) {
        Avail::Ready => {}
        other => return Err(other),
    }
    levels.clear();
    levels.extend(frame_levels(x));
    let level_db = percentile(levels, LEVEL_PERCENTILE).unwrap_or(-100.0);
    let e_heard = energy(x);
    let attenuation_db = match raw {
        Some(r) => match r.read(Channel::Mic, a, len, x) {
            Avail::Ready => {
                let e_raw = energy(x);
                Some(if e_raw <= 1e-12 || e_heard <= 1e-12 {
                    0.0
                } else {
                    (10.0 * (e_raw / e_heard).log10()) as f32
                })
            }
            other => return Err(other),
        },
        None => None,
    };
    let fa = span.0 - FAR_BEFORE;
    let fl = (span.1.max(span.0) + FAR_AFTER - fa) as usize;
    match heard.read(Channel::Loopback, fa, fl, x) {
        Avail::Ready => {}
        other => return Err(other),
    }
    let far_active = frame_levels(x).any(|d| d >= FAR_ACTIVE_DBFS);
    Ok(Presence {
        level_db,
        far_active,
        attenuation_db,
    })
}

/// 0.25 dB bins from −100 dBFS.
const BINS: usize = 400;

/// The mic's noise floor (see the module docs), from equal-length mic and
/// loopback blocks on the timeline.
#[derive(Debug, Clone)]
pub struct FloorTracker {
    hist: Vec<u32>,
    n: u64,
}

impl Default for FloorTracker {
    fn default() -> Self {
        Self {
            hist: vec![0; BINS],
            n: 0,
        }
    }
}

impl FloorTracker {
    /// Adds the frames of one block (speech band, the same timeline span).
    pub fn push(&mut self, mic: &[f32], loopback: &[f32]) {
        for (m, l) in mic.chunks(FRAME).zip(loopback.chunks(FRAME)) {
            if m.len() < FRAME || super::rms_dbfs(l) >= FAR_ACTIVE_DBFS {
                continue;
            }
            let d = super::rms_dbfs(m);
            if d < SILENT_DBFS {
                continue;
            }
            let k = (((d + 100.0) * 4.0).floor().max(0.0) as usize).min(BINS - 1);
            self.hist[k] += 1;
            self.n += 1;
        }
    }

    /// Frames counted.
    pub fn frames(&self) -> u64 {
        self.n
    }

    /// The floor (dBFS), once [`FLOOR_MIN_FRAMES`] frames are in.
    pub fn floor_db(&self) -> Option<f32> {
        self.percentile(FLOOR_PERCENTILE)
    }

    /// Another percentile of the same frames (evaluation).
    pub fn percentile(&self, p: f64) -> Option<f32> {
        if self.n < FLOOR_MIN_FRAMES {
            return None;
        }
        let want = ((self.n - 1) as f64 * p).round() as u64;
        let mut seen = 0u64;
        for (k, &c) in self.hist.iter().enumerate() {
            seen += u64::from(c);
            if seen > want {
                return Some(k as f32 / 4.0 - 100.0 + 0.125);
            }
        }
        None
    }
}

/// The floor of a stored recording: `heard` against `loopback` on the
/// timeline (meeting time through `clocks`), 2 s blocks.
pub fn floor_sources(
    heard: impl PcmSource,
    loopback: impl PcmSource,
    clocks: Option<ClockPair>,
) -> FloorTracker {
    let mut audio = StreamAudio {
        mic: Stream::new(heard),
        loopback: Stream::new(loopback),
        clocks,
    };
    let mut f = FloorTracker::default();
    let (mut x, mut y) = (Vec::new(), Vec::new());
    for k in 0i64.. {
        let t = k * BLOCK as i64;
        let s = audio
            .clocks
            .as_ref()
            .map_or(t, |c| c.channel_sample(Channel::Mic, t));
        if !audio.mic.has(s + BLOCK as i64 - 1) {
            break;
        }
        audio.read(Channel::Mic, t, BLOCK, &mut x);
        audio.read(Channel::Loopback, t, BLOCK, &mut y);
        f.push(&x, &y);
    }
    f
}

/// Presence of every span (timeline samples, in any order) of a stored
/// recording: `heard` is what the speech model transcribed, `raw` the
/// uncancelled mic when a canceller ran (for the attenuation).
pub fn presence_sources(
    spans: &[(i64, i64)],
    heard: impl PcmSource,
    raw: Option<Box<dyn PcmSource + '_>>,
    loopback: impl PcmSource,
    clocks: Option<ClockPair>,
) -> Vec<Option<Presence>> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i]);
    let mut heard = StreamAudio {
        mic: Stream::new(heard),
        loopback: Stream::new(loopback),
        clocks: clocks.clone(),
    };
    let mut raw = raw.map(|r| StreamAudio {
        mic: Stream::new(r),
        loopback: Stream::new(&[][..]),
        clocks,
    });
    let (mut x, mut l) = (Vec::new(), Vec::new());
    let mut out = vec![None; spans.len()];
    for i in order {
        let r = raw.as_mut().map(|r| r as &mut dyn EchoAudio);
        out[i] = presence(&mut heard, r, spans[i], &mut x, &mut l).ok();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_ignores_frames_with_the_far_end_and_digital_silence() {
        let mut f = FloorTracker::default();
        let noise: Vec<f32> = (0..BLOCK)
            .map(|i| if i % 2 == 0 { 0.001 } else { -0.001 })
            .collect();
        let loud: Vec<f32> = noise.iter().map(|v| v * 100.0).collect();
        let quiet = vec![0.0f32; BLOCK];
        for _ in 0..10 {
            f.push(&noise, &quiet);
            // Echo while the far end talks: not the floor.
            f.push(&loud, &loud);
            // A muted mic: not the room.
            f.push(&quiet, &quiet);
        }
        let floor = f.floor_db().unwrap();
        assert!((floor + 60.0).abs() < 0.5, "{floor}");
        assert_eq!(f.frames(), 10 * (BLOCK / FRAME) as u64);
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&mut v, 0.0), Some(1.0));
        assert_eq!(percentile(&mut v, 0.5), Some(3.0));
        assert_eq!(percentile(&mut v, 0.9), Some(5.0));
        assert_eq!(percentile(&mut [], 0.5), None);
    }
}
