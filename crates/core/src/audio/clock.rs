//! Clock anchors (`docs/01`, "Alignment"): one `(sample_index, qpc_100ns)`
//! pair per channel per second, plus extra anchors at gaps and device
//! changes. [`ClockMap`] maps a channel's sample time to meeting time
//! piecewise-linearly between anchors; meeting time is QPC time since the
//! session origin (the QPC at which both channels' sample 0 sits). Drift is
//! reported in ppm for diagnostics; it is corrected only here, offline,
//! never by steering a resampler live.

use crate::types::{Channel, ClockAnchor};

/// 100 ns units per sample at 16 kHz.
pub const NOMINAL_HNS_PER_SAMPLE: f64 = 625.0;

/// Piecewise-linear map from channel sample index to QPC time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClockMap {
    /// QPC (100 ns) of meeting time zero.
    pub origin_qpc: i64,
    /// `(sample_index, qpc_100ns)`, strictly increasing in sample index.
    points: Vec<(i64, i64)>,
}

impl ClockMap {
    pub fn new(origin_qpc: i64) -> Self {
        Self {
            origin_qpc,
            points: Vec::new(),
        }
    }

    /// From stored anchors of one channel (any order; duplicates keep the last).
    pub fn from_anchors(origin_qpc: i64, anchors: &[ClockAnchor]) -> Self {
        let mut points: Vec<(i64, i64)> = anchors
            .iter()
            .map(|a| (a.sample_index, a.qpc_100ns))
            .collect();
        points.sort_by_key(|p| p.0);
        points.dedup_by(|b, a| {
            if a.0 == b.0 {
                a.1 = b.1;
                true
            } else {
                false
            }
        });
        Self { origin_qpc, points }
    }

    /// Appends an anchor (ignored unless it is after the last one).
    pub fn push(&mut self, sample_index: i64, qpc_100ns: i64) {
        match self.points.last_mut() {
            Some(last) if last.0 == sample_index => last.1 = qpc_100ns,
            Some(last) if last.0 > sample_index => {}
            _ => self.points.push((sample_index, qpc_100ns)),
        }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn points(&self) -> &[(i64, i64)] {
        &self.points
    }

    /// Least-squares slope in 100 ns per sample (`None` with fewer than two
    /// anchors spanning at least a second).
    fn fitted_slope(&self) -> Option<f64> {
        let (first, last) = (self.points.first()?, self.points.last()?);
        if last.0 - first.0 < 16_000 {
            return None;
        }
        let n = self.points.len() as f64;
        let x0 = first.0 as f64;
        let y0 = first.1 as f64;
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for &(x, y) in &self.points {
            let (x, y) = (x as f64 - x0, y as f64 - y0);
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let den = n * sxx - sx * sx;
        (den.abs() > f64::EPSILON).then(|| (n * sxy - sx * sy) / den)
    }

    /// Clock drift of the channel against QPC in parts per million
    /// (positive: the device delivers samples more slowly than nominal, so
    /// each sample spans more QPC time).
    pub fn drift_ppm(&self) -> Option<f64> {
        self.fitted_slope()
            .map(|s| (s / NOMINAL_HNS_PER_SAMPLE - 1.0) * 1e6)
    }

    /// The fitted slope, else the nominal one (extrapolation only: it is a
    /// pass over every anchor).
    fn slope(&self) -> f64 {
        self.fitted_slope().unwrap_or(NOMINAL_HNS_PER_SAMPLE)
    }

    /// QPC time of (fractional) sample `s`: linear between the surrounding
    /// anchors; outside them, extrapolated with the fitted slope.
    pub fn qpc_at(&self, s: f64) -> f64 {
        match self.points.as_slice() {
            [] => self.origin_qpc as f64 + s * NOMINAL_HNS_PER_SAMPLE,
            [p] => p.1 as f64 + (s - p.0 as f64) * self.slope(),
            pts => {
                let first = pts[0];
                let last = pts[pts.len() - 1];
                if s <= first.0 as f64 {
                    return first.1 as f64 + (s - first.0 as f64) * self.slope();
                }
                if s >= last.0 as f64 {
                    return last.1 as f64 + (s - last.0 as f64) * self.slope();
                }
                let i = pts.partition_point(|p| (p.0 as f64) <= s);
                let (a, b) = (pts[i - 1], pts[i]);
                let t = (s - a.0 as f64) / (b.0 - a.0) as f64;
                a.1 as f64 + t * (b.1 - a.1) as f64
            }
        }
    }

    /// The inverse of [`Self::qpc_at`]: the (fractional) sample of this
    /// channel captured at QPC time `qpc`. Inside a jump (QPC moved with no
    /// samples) it stays between the last sample before and the first after.
    pub fn sample_at(&self, qpc: f64) -> f64 {
        match self.points.as_slice() {
            [] => (qpc - self.origin_qpc as f64) / NOMINAL_HNS_PER_SAMPLE,
            [p] => p.0 as f64 + (qpc - p.1 as f64) / self.slope(),
            pts => {
                let first = pts[0];
                let last = pts[pts.len() - 1];
                if qpc <= first.1 as f64 {
                    return first.0 as f64 + (qpc - first.1 as f64) / self.slope();
                }
                if qpc >= last.1 as f64 {
                    return last.0 as f64 + (qpc - last.1 as f64) / self.slope();
                }
                let i = pts.partition_point(|p| (p.1 as f64) <= qpc).max(1);
                let (a, b) = (pts[i - 1], pts[i]);
                if b.1 <= a.1 {
                    return b.0 as f64;
                }
                let t = (qpc - a.1 as f64) / (b.1 - a.1) as f64;
                a.0 as f64 + t * (b.0 - a.0) as f64
            }
        }
    }

    /// Meeting time (µs since the origin) of channel-sample time `us`.
    pub fn meeting_us(&self, channel_us: u64) -> u64 {
        let s = channel_us as f64 * 16_000.0 / 1e6;
        ((self.qpc_at(s) - self.origin_qpc as f64) / 10.0)
            .round()
            .max(0.0) as u64
    }

    /// Meeting time (ms) of channel-sample time `ms`.
    pub fn meeting_ms(&self, channel_ms: i64) -> i64 {
        (self.meeting_us(channel_ms.max(0) as u64 * 1_000) as f64 / 1_000.0).round() as i64
    }
}

/// The session origin from stored anchors: the earliest anchor at sample 0
/// over all channels (every channel's timeline starts at the origin), else
/// the earliest anchor extrapolated back to sample 0 at the nominal rate.
pub fn origin_from_anchors(anchors: &[ClockAnchor]) -> Option<i64> {
    anchors
        .iter()
        .filter(|a| a.sample_index == 0)
        .map(|a| a.qpc_100ns)
        .min()
        .or_else(|| {
            anchors
                .iter()
                .map(|a| a.qpc_100ns - a.sample_index * 625)
                .min()
        })
}

/// Anchors of `channel` from a mixed list.
pub fn channel_anchors(anchors: &[ClockAnchor], channel: Channel) -> Vec<ClockAnchor> {
    anchors
        .iter()
        .filter(|a| a.channel == channel)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_without_anchors() {
        let m = ClockMap::new(1_000);
        assert_eq!(m.meeting_ms(2_500), 2_500);
        assert_eq!(m.drift_ppm(), None);
    }

    #[test]
    fn interpolates_and_reports_drift() {
        // 200 ppm slow device: each second of samples spans 1.0002 s of QPC.
        let mut m = ClockMap::new(10_000_000);
        for k in 0..=60i64 {
            m.push(
                k * 16_000,
                10_000_000 + (k as f64 * 1.0002e7).round() as i64,
            );
        }
        let ppm = m.drift_ppm().unwrap();
        assert!((ppm - 200.0).abs() < 1.0, "{ppm}");
        // 30 s of samples are 30.006 s of meeting time.
        assert_eq!(m.meeting_ms(30_000), 30_006);
        // Past the last anchor: extrapolated with the same slope.
        assert_eq!(m.meeting_ms(70_000), 70_014);
        // The inverse, inside, before and after the anchors.
        for s in [-1_000.0, 0.0, 12_345.5, 480_000.0, 959_999.0, 1_200_000.0] {
            let back = m.sample_at(m.qpc_at(s));
            assert!((back - s).abs() < 1e-3, "{s} -> {back}");
        }
    }

    #[test]
    fn inverse_inside_a_jump_stays_between_its_two_samples() {
        let mut m = ClockMap::new(0);
        m.push(0, 0);
        m.push(15_999, 9_999_375);
        // Suspended for an hour: the next sample is 3,600 s later.
        m.push(16_000, 36_010_000_000);
        m.push(32_000, 36_020_000_000);
        let inside = m.sample_at(20_000_000_000.0);
        assert!((15_999.0..=16_000.0).contains(&inside), "{inside}");
        assert!((m.sample_at(36_015_000_000.0) - 24_000.0).abs() < 1e-6);
        assert_eq!(ClockMap::new(1_000).sample_at(1_000.0 + 625.0 * 16.0), 16.0);
    }

    #[test]
    fn gap_anchors_make_a_kink() {
        let mut m = ClockMap::new(0);
        m.push(0, 0);
        m.push(16_000, 10_000_000);
        // A 0.5 s jump in QPC with no samples (a glitch nobody filled).
        m.push(16_001, 15_000_625);
        m.push(32_001, 25_000_625);
        assert_eq!(m.meeting_ms(500), 500);
        assert_eq!(m.meeting_ms(1_500), 2_000);
    }

    #[test]
    fn origin_prefers_sample_zero() {
        let a = |ch, s, q| ClockAnchor {
            meeting_id: crate::types::MeetingId(1),
            channel: ch,
            sample_index: s,
            qpc_100ns: q,
        };
        let anchors = vec![
            a(Channel::Mic, 16_000, 20_000_000),
            a(Channel::Mic, 0, 10_000_000),
            a(Channel::Loopback, 0, 10_000_100),
        ];
        assert_eq!(origin_from_anchors(&anchors), Some(10_000_000));
        assert_eq!(channel_anchors(&anchors, Channel::Loopback).len(), 1);
        let late = vec![a(Channel::Mic, 32_000, 30_000_000)];
        assert_eq!(origin_from_anchors(&late), Some(10_000_000));
        let from = ClockMap::from_anchors(10_000_000, &channel_anchors(&anchors, Channel::Mic));
        assert_eq!(from.len(), 2);
    }
}
