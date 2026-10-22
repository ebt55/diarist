//! Per-channel pipeline (plan 04 chunk 3b): capture packets in, a gapless
//! 16 kHz timeline out.
//!
//! [`ChannelPipeline`] places every packet on a QPC timeline that starts at
//! the session origin (`session_start_qpc`): leading silence before the
//! first packet, zero-fill for any gap longer than 5 ms (gap duration × 16
//! kHz samples), zeros for SILENT packets, and the head of a packet cut off
//! when it overlaps what is already on the timeline. Continuity is judged
//! packet to packet, so slow clock drift never triggers fills; it shows up
//! in the clock anchors instead: `(sample_index, qpc)` at sample 0, once per
//! second, and on both sides of every gap and device change. Output is cut
//! into 100 ms [`Frame`]s with `pts_us` and a level. With
//! `capture.autoconvert = false`, mix-format packets are downmixed and
//! resampled with rubato first ([`MixResampler`]).
//!
//! [`spawn_pipeline`] runs one on its own thread: it drains the capture
//! ring, writes the WAV ([`super::wav::WavSink`], flushed every second) and,
//! with AGC, the gain track beside it ([`super::gain_track`]: the gain of
//! every 10 ms of the timeline, which the post-pass echo canceller replays
//! backwards), copies every frame to an optional [`FrameTap`] (the echo monitor; never
//! blocking, before the feed so a degraded feed does not starve it), offers
//! frames to the engine feed (never blocking), records anchors
//! through [`RunStore::add_clock_anchors`] in batches and into a shared
//! [`ClockMap`], and reports levels, gaps, device changes and feed
//! transitions through a callback.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, Fft, FixedAsync, FixedSync, Resampler, SincInterpolationParameters};

use crate::audio::capture::{
    audio_err, ChangeReason, Control, Packet, PacketData, PacketFlags, CAPTURE_RATE, HNS_PER_SAMPLE,
};
use crate::audio::clock::ClockMap;
use crate::audio::feed::{FeedTransition, FeedTx};
use crate::audio::gain::{dbfs, Agc, AgcConfig};
use crate::audio::gain_track::{GainTrackWriter, SAMPLES_PER_VALUE, VALUES_PER_RECORD};
use crate::audio::wav::WavSink;
use crate::error::Result;
use crate::store::RunStore;
use crate::types::{Channel, ClockAnchor, MeetingId};

/// Samples per frame (100 ms).
pub const FRAME_SAMPLES: usize = 1_600;
/// One anchor per second of samples.
pub const ANCHOR_EVERY: u64 = CAPTURE_RATE as u64;
/// Timestamp jitter tolerated before a gap is filled or an overlap cut (5 ms).
pub const GAP_TOLERANCE_HNS: i64 = 50_000;

/// Level of one frame in dBFS (−100 for digital silence).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Level {
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            rms_dbfs: -100.0,
            peak_dbfs: -100.0,
        }
    }
}

impl Level {
    pub fn of(pcm: &[i16]) -> Self {
        if pcm.is_empty() {
            return Self::default();
        }
        let mut sum = 0f64;
        let mut peak = 0u16;
        for &s in pcm {
            sum += f64::from(s) * f64::from(s);
            peak = peak.max(s.unsigned_abs());
        }
        let db = |v: f64| {
            if v <= 0.0 {
                -100.0
            } else {
                (20.0 * (v / 32_768.0).log10()).max(-100.0) as f32
            }
        };
        Self {
            rms_dbfs: db((sum / pcm.len() as f64).sqrt()),
            peak_dbfs: db(f64::from(peak)),
        }
    }
}

/// 100 ms of one channel on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub channel: Channel,
    /// Index of the first sample in the channel's 16 kHz stream.
    pub start_sample: u64,
    /// `start_sample` in microseconds (channel-sample time).
    pub pts_us: u64,
    pub pcm: Vec<i16>,
    /// Level of `pcm` (after the AGC: what the WAV and the engine get).
    pub level: Level,
    /// AGC gain at the end of the frame (0 without AGC).
    pub gain_db: f32,
    /// Peak before the AGC (dBFS, -100 for silence).
    pub raw_peak_dbfs: f32,
}

/// Something worth logging that happened on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    /// `samples` zeros were inserted at `at_sample` for missing audio.
    Gap { at_sample: u64, samples: u64 },
    /// `dropped` samples at the head of a packet overlapped the timeline.
    Overlap { at_sample: u64, dropped: u64 },
    /// The capture (re)opened on a device.
    DeviceChanged {
        at_sample: u64,
        device_id: String,
        name: String,
        reason: ChangeReason,
    },
    /// The capture lost its device and is retrying.
    DeviceLost { at_sample: u64, error: String },
    /// QPC jumped by `hns` with no audio (suspend/resume); not zero-filled,
    /// only anchored.
    Jump { at_sample: u64, hns: i64 },
}

/// Counters for diagnostics and `post_pass_mode = on_issues`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PipelineStats {
    pub samples: u64,
    pub gaps: u64,
    pub gap_samples: u64,
    pub overlap_samples: u64,
    pub silent_packets: u64,
    pub synthetic_packets: u64,
    pub discontinuities: u64,
    pub device_changes: u32,
}

/// What one [`ChannelPipeline::push`] produced. Reused between calls; the
/// caller clears it.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Timeline samples in order (what the WAV gets).
    pub samples: Vec<i16>,
    pub frames: Vec<Frame>,
    /// `(sample_index, qpc_100ns)`.
    pub anchors: Vec<(u64, i64)>,
    pub events: Vec<PipelineEvent>,
    /// With AGC: the gain record of every finished frame (the gain track).
    pub gains: Vec<GainRecord>,
}

impl Outputs {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.frames.clear();
        self.anchors.clear();
        self.events.clear();
        self.gains.clear();
    }
}

/// The AGC gain over one 100 ms frame of the timeline: a value per 10 ms
/// (the mean gain of its samples), a record of the gain track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainRecord {
    /// The frame's first sample.
    pub start_sample: u64,
    pub values: [f32; VALUES_PER_RECORD],
}

/// Places one channel's packets on the session timeline.
pub struct ChannelPipeline {
    channel: Channel,
    origin_qpc: i64,
    position: u64,
    /// QPC of the next timeline sample according to the last packet.
    next_qpc: Option<i64>,
    next_anchor: u64,
    frame: Vec<i16>,
    frame_start: u64,
    /// Force a gap check (and gap anchor) on the next packet.
    pending_break: bool,
    /// The next gap is a suspend: anchor it, do not fill it.
    no_fill_next: bool,
    /// The previous packet was synthesized silence: the capture stays
    /// 100 ms behind real time while idle, so the step to the first real
    /// packet is filled but is not a lost-audio gap.
    after_synthetic: bool,
    mix: Option<MixResampler>,
    agc: Option<Agc>,
    /// With AGC: the gain of each sample of the packet being placed.
    packet_gains: Vec<f32>,
    /// With AGC: gain sums and sample counts per 10 ms of the open frame.
    gain_sum: [f64; VALUES_PER_RECORD],
    gain_n: [u32; VALUES_PER_RECORD],
    /// The gain of the last sample placed (a zero fill keeps it).
    fill_gain: f32,
    stats: PipelineStats,
}

impl ChannelPipeline {
    pub fn new(channel: Channel, origin_qpc: i64) -> Self {
        Self {
            channel,
            origin_qpc,
            position: 0,
            next_qpc: None,
            next_anchor: 0,
            frame: Vec::with_capacity(FRAME_SAMPLES),
            frame_start: 0,
            pending_break: false,
            no_fill_next: false,
            after_synthetic: false,
            mix: None,
            agc: None,
            packet_gains: Vec::new(),
            gain_sum: [0.0; VALUES_PER_RECORD],
            gain_n: [0; VALUES_PER_RECORD],
            fill_gain: 0.0,
            stats: PipelineStats::default(),
        }
    }

    /// The gain of each sample the AGC just processed (empty without AGC).
    fn take_block_gains(&mut self) {
        self.packet_gains.clear();
        if let Some(agc) = &self.agc {
            for &(n, g) in agc.last_blocks() {
                self.packet_gains.extend(std::iter::repeat_n(g, n));
            }
        }
    }

    /// Applies automatic gain ([`Agc`]) to everything before it reaches the
    /// timeline (WAV and engine alike).
    pub fn with_agc(mut self, cfg: Option<AgcConfig>) -> Self {
        self.agc = cfg.map(Agc::new);
        self
    }

    /// Current AGC gain (0 without AGC).
    pub fn gain_db(&self) -> f32 {
        self.agc.as_ref().map_or(0.0, Agc::gain_db)
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    /// Samples on the timeline so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn stats(&self) -> &PipelineStats {
        &self.stats
    }

    /// QPC of the timeline end (where the next sample goes).
    pub fn end_qpc(&self) -> i64 {
        self.next_qpc
            .unwrap_or(self.origin_qpc + self.position as i64 * HNS_PER_SAMPLE)
    }

    /// Processes one packet.
    pub fn push(&mut self, packet: Packet, out: &mut Outputs) -> Result<()> {
        match packet {
            Packet::Control { control, .. } => {
                match control {
                    Control::DeviceChanged {
                        device_id,
                        name,
                        reason,
                    } => {
                        self.stats.device_changes += 1;
                        if reason == ChangeReason::Resumed {
                            self.no_fill_next = true;
                        }
                        out.events.push(PipelineEvent::DeviceChanged {
                            at_sample: self.position,
                            device_id,
                            name,
                            reason,
                        });
                    }
                    Control::DeviceLost { error } => out.events.push(PipelineEvent::DeviceLost {
                        at_sample: self.position,
                        error,
                    }),
                    Control::Resumed { .. } => self.no_fill_next = true,
                }
                self.pending_break = true;
                Ok(())
            }
            Packet::Audio {
                qpc_100ns,
                flags,
                data,
            } => {
                if flags.silent {
                    self.stats.silent_packets += 1;
                }
                if flags.synthetic {
                    self.stats.synthetic_packets += 1;
                }
                if flags.discontinuity {
                    self.stats.discontinuities += 1;
                }
                let result = match data {
                    PacketData::Pcm16(mut samples) => {
                        if let Some(agc) = self.agc.as_mut() {
                            if flags.silent {
                                samples.iter_mut().for_each(|s| *s = 0);
                            }
                            agc.process_i16(&mut samples);
                        }
                        self.take_block_gains();
                        self.place(qpc_100ns, samples, flags, out);
                        Ok(())
                    }
                    PacketData::Float {
                        rate,
                        channels,
                        samples,
                    } => {
                        let mix = match &mut self.mix {
                            Some(m) if m.rate == rate && m.channels == channels => m,
                            slot => {
                                *slot = Some(MixResampler::new(rate, channels)?);
                                slot.as_mut().expect("just set")
                            }
                        };
                        let chunks = mix.push(qpc_100ns, &samples, flags.silent)?;
                        for (q, mut chunk) in chunks {
                            if let Some(agc) = self.agc.as_mut() {
                                agc.process_f32(&mut chunk);
                            }
                            self.take_block_gains();
                            let pcm: Vec<i16> = chunk.into_iter().map(to_i16).collect();
                            let f = PacketFlags {
                                discontinuity: flags.discontinuity,
                                ..Default::default()
                            };
                            self.place(q, pcm, f, out);
                        }
                        Ok(())
                    }
                };
                self.after_synthetic = flags.synthetic;
                result
            }
        }
    }

    fn place(&mut self, q: i64, mut samples: Vec<i16>, flags: PacketFlags, out: &mut Outputs) {
        let n = samples.len();
        let first = self.next_qpc.is_none();
        let expected = self.next_qpc.unwrap_or(self.origin_qpc);
        let gap = q - expected;
        let mut skip = 0usize;
        let check =
            first || flags.discontinuity || self.pending_break || gap.abs() > GAP_TOLERANCE_HNS;
        if check {
            let tol = if first { 0 } else { GAP_TOLERANCE_HNS };
            if self.no_fill_next && !first && gap > tol {
                // Suspend: the timeline jumps; the anchors carry the jump
                // (the last sample before it and the first after it).
                if self.position > 0 {
                    out.anchors
                        .push((self.position - 1, expected - HNS_PER_SAMPLE));
                }
                out.events.push(PipelineEvent::Jump {
                    at_sample: self.position,
                    hns: gap,
                });
            } else if gap > tol {
                let fill = ((gap as f64) / HNS_PER_SAMPLE as f64).round() as u64;
                if fill > 0 {
                    if !first {
                        out.anchors.push((self.position, expected));
                    }
                    if !first && !self.after_synthetic {
                        self.stats.gaps += 1;
                        self.stats.gap_samples += fill;
                        out.events.push(PipelineEvent::Gap {
                            at_sample: self.position,
                            samples: fill,
                        });
                    }
                    self.emit_zeros(fill, expected, out);
                }
            } else if gap < -tol {
                skip = (((-gap) as f64 / HNS_PER_SAMPLE as f64).round() as usize).min(n);
                if skip > 0 {
                    self.stats.overlap_samples += skip as u64;
                    out.events.push(PipelineEvent::Overlap {
                        at_sample: self.position,
                        dropped: skip as u64,
                    });
                }
            }
            if !first && skip < n {
                out.anchors
                    .push((self.position, q + skip as i64 * HNS_PER_SAMPLE));
            }
            self.pending_break = false;
            self.no_fill_next = false;
        }
        if flags.silent {
            samples.iter_mut().for_each(|s| *s = 0);
        }
        if skip < n {
            let q0 = q + skip as i64 * HNS_PER_SAMPLE;
            let gains = std::mem::take(&mut self.packet_gains);
            self.emit(&samples[skip..], gains.get(skip..n), q0, out);
            self.packet_gains = gains;
        }
        self.next_qpc = Some(q + n as i64 * HNS_PER_SAMPLE);
    }

    fn emit_zeros(&mut self, mut n: u64, mut q0: i64, out: &mut Outputs) {
        let zeros = [0i16; FRAME_SAMPLES];
        while n > 0 {
            let k = (n as usize).min(FRAME_SAMPLES);
            self.emit(&zeros[..k], None, q0, out);
            q0 += k as i64 * HNS_PER_SAMPLE;
            n -= k as u64;
        }
    }

    /// Appends samples whose first one is at QPC `q0`; with AGC, `gains`
    /// holds the gain of each (`None`: zero fill, at the gain of the last
    /// sample placed).
    fn emit(&mut self, pcm: &[i16], gains: Option<&[f32]>, q0: i64, out: &mut Outputs) {
        let len = pcm.len() as u64;
        while self.next_anchor < self.position + len {
            let idx = self.next_anchor;
            out.anchors
                .push((idx, q0 + (idx - self.position) as i64 * HNS_PER_SAMPLE));
            self.next_anchor += ANCHOR_EVERY;
        }
        out.samples.extend_from_slice(pcm);
        let tracked = self.agc.is_some();
        let current = self.fill_gain;
        let mut rest = pcm;
        let mut done = 0usize;
        while !rest.is_empty() {
            let k = (FRAME_SAMPLES - self.frame.len()).min(rest.len());
            if tracked {
                let at = self.frame.len();
                for j in 0..k {
                    let g = gains
                        .and_then(|g| g.get(done + j))
                        .copied()
                        .unwrap_or(current);
                    let b = (at + j) / SAMPLES_PER_VALUE;
                    self.gain_sum[b] += f64::from(g);
                    self.gain_n[b] += 1;
                }
            }
            done += k;
            if let Some(&g) = gains.and_then(|g| g.get(..done)).and_then(<[f32]>::last) {
                self.fill_gain = g;
            }
            self.frame.extend_from_slice(&rest[..k]);
            rest = &rest[k..];
            if self.frame.len() == FRAME_SAMPLES {
                self.finish_frame(out);
            }
        }
        self.position += len;
        self.stats.samples = self.position;
    }

    fn finish_frame(&mut self, out: &mut Outputs) {
        let pcm = std::mem::replace(&mut self.frame, Vec::with_capacity(FRAME_SAMPLES));
        let start = self.frame_start;
        self.frame_start += pcm.len() as u64;
        let level = Level::of(&pcm);
        let (gain_db, raw_peak_dbfs) = match self.agc.as_mut() {
            Some(agc) => (agc.gain_db(), dbfs(agc.take_raw_peak()).max(-100.0)),
            None => (0.0, level.peak_dbfs),
        };
        if self.agc.is_some() {
            // Blocks a partial last frame did not reach repeat its last one.
            let mut values = [gain_db; VALUES_PER_RECORD];
            if let Some(first) = self.gain_n.iter().position(|&n| n > 0) {
                let mut last = 0.0f32;
                for (k, v) in values.iter_mut().enumerate().skip(first) {
                    if self.gain_n[k] > 0 {
                        last = (self.gain_sum[k] / f64::from(self.gain_n[k])) as f32;
                    }
                    *v = last;
                }
            }
            out.gains.push(GainRecord {
                start_sample: start,
                values,
            });
            self.gain_sum = [0.0; VALUES_PER_RECORD];
            self.gain_n = [0; VALUES_PER_RECORD];
        }
        out.frames.push(Frame {
            channel: self.channel,
            start_sample: start,
            pts_us: start * 1_000_000 / u64::from(CAPTURE_RATE),
            level,
            gain_db,
            raw_peak_dbfs,
            pcm,
        });
    }

    /// Zero-fills the timeline up to QPC `qpc` (stop, resume).
    pub fn fill_to(&mut self, qpc: i64, out: &mut Outputs) {
        let end = self.end_qpc();
        if qpc - end > HNS_PER_SAMPLE {
            let n = ((qpc - end) as f64 / HNS_PER_SAMPLE as f64).round() as u64;
            if self.next_qpc.is_some() {
                out.anchors.push((self.position, end));
            }
            self.emit_zeros(n, end, out);
            self.next_qpc = Some(end + n as i64 * HNS_PER_SAMPLE);
        }
    }

    /// Zero-fills up to `len` samples (both WAVs end at the same length).
    pub fn pad_to_len(&mut self, len: u64, out: &mut Outputs) {
        if len > self.position {
            let end = self.end_qpc();
            let n = len - self.position;
            self.emit_zeros(n, end, out);
            self.next_qpc = Some(end + n as i64 * HNS_PER_SAMPLE);
        }
    }

    /// Emits the partial last frame and a final anchor at the end.
    pub fn finish(&mut self, out: &mut Outputs) {
        if !self.frame.is_empty() {
            self.finish_frame(out);
        }
        out.anchors.push((self.position, self.end_qpc()));
    }
}

/// Downmix and resample a mix-format stream to 16 kHz mono PCM16 while
/// keeping QPC time (`capture.autoconvert = false`). Gaps and overlaps are
/// handled at the input rate, so the resampler always sees a continuous
/// stream and its output carries the input's timestamps minus its delay.
pub struct MixResampler {
    rate: u32,
    channels: u16,
    resampler: Option<Box<dyn Resampler<f32>>>,
    fifo: Vec<f32>,
    /// QPC of `fifo[0]`.
    fifo_qpc: Option<f64>,
    /// Output frames still to drop (resampler start-up delay).
    to_trim: usize,
    delay_hns: f64,
    out: Vec<f32>,
}

impl MixResampler {
    pub fn new(rate: u32, channels: u16) -> Result<Self> {
        let err = |e: String| audio_err(format!("resampler {rate} Hz: {e}"));
        let chunk = (rate / 100).max(1) as usize;
        let resampler: Option<Box<dyn Resampler<f32>>> = if rate == CAPTURE_RATE {
            None
        } else if rate.is_multiple_of(CAPTURE_RATE) || CAPTURE_RATE.is_multiple_of(rate) {
            Some(Box::new(
                Fft::<f32>::new(
                    rate as usize,
                    CAPTURE_RATE as usize,
                    chunk,
                    1,
                    FixedSync::Input,
                )
                .map_err(|e| err(e.to_string()))?,
            ))
        } else {
            Some(Box::new(
                Async::<f32>::new_sinc(
                    f64::from(CAPTURE_RATE) / f64::from(rate),
                    1.1,
                    &SincInterpolationParameters::default(),
                    chunk,
                    1,
                    FixedAsync::Input,
                )
                .map_err(|e| err(e.to_string()))?,
            ))
        };
        let to_trim = resampler.as_ref().map_or(0, |r| r.output_delay());
        Ok(Self {
            rate,
            channels: channels.max(1),
            delay_hns: to_trim as f64 * HNS_PER_SAMPLE as f64,
            resampler,
            fifo: Vec::new(),
            fifo_qpc: None,
            to_trim,
            out: Vec::new(),
        })
    }

    fn hns_per_in(&self) -> f64 {
        1e7 / f64::from(self.rate)
    }

    /// Adds a packet (interleaved, `channels` per frame) at QPC `q`; returns
    /// 16 kHz mono float chunks with the QPC of their first sample.
    pub fn push(
        &mut self,
        q: i64,
        interleaved: &[f32],
        silent: bool,
    ) -> Result<Vec<(i64, Vec<f32>)>> {
        let ch = usize::from(self.channels);
        let scale = 1.0 / ch as f32;
        let mono: Vec<f32> = interleaved
            .chunks_exact(ch)
            .map(|f| {
                if silent {
                    0.0
                } else {
                    f.iter().sum::<f32>() * scale
                }
            })
            .collect();
        let per = self.hns_per_in();
        let mut start = 0usize;
        match self.fifo_qpc {
            None => self.fifo_qpc = Some(q as f64),
            Some(fq) => {
                let end = fq + self.fifo.len() as f64 * per;
                let gap = q as f64 - end;
                if gap > GAP_TOLERANCE_HNS as f64 {
                    let n = (gap / per).round() as usize;
                    self.fifo.extend(std::iter::repeat_n(0.0, n));
                } else if gap < -(GAP_TOLERANCE_HNS as f64) {
                    start = ((-gap / per).round() as usize).min(mono.len());
                }
            }
        }
        self.fifo.extend_from_slice(&mono[start..]);
        let mut chunks = Vec::new();
        let Some(r) = self.resampler.as_mut() else {
            let fq = self.fifo_qpc.unwrap_or(q as f64);
            let pcm = std::mem::take(&mut self.fifo);
            self.fifo_qpc = Some(fq + pcm.len() as f64 * per);
            if !pcm.is_empty() {
                chunks.push((fq.round() as i64, pcm));
            }
            return Ok(chunks);
        };
        loop {
            let need = r.input_frames_next();
            if self.fifo.len() < need {
                break;
            }
            let out_len = r.output_frames_next();
            self.out.resize(out_len, 0.0);
            let adapter_in = InterleavedSlice::new(&self.fifo[..need], 1, need)
                .map_err(|e| audio_err(format!("resampler input: {e}")))?;
            let mut adapter_out = InterleavedSlice::new_mut(&mut self.out, 1, out_len)
                .map_err(|e| audio_err(format!("resampler output: {e}")))?;
            let (used, produced) = r
                .process_into_buffer(&adapter_in, &mut adapter_out, None)
                .map_err(|e| audio_err(format!("resample: {e}")))?;
            let chunk_qpc = self.fifo_qpc.unwrap_or(q as f64);
            self.fifo.drain(..used);
            self.fifo_qpc = Some(chunk_qpc + used as f64 * per);
            let trim = self.to_trim.min(produced);
            self.to_trim -= trim;
            if produced > trim {
                let qpc = chunk_qpc - self.delay_hns + trim as f64 * HNS_PER_SAMPLE as f64;
                let pcm = self.out[trim..produced].to_vec();
                chunks.push((qpc.round() as i64, pcm));
            }
        }
        Ok(chunks)
    }
}

fn to_i16(s: f32) -> i16 {
    (s * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16
}

// ---- the pipeline thread ----------------------------------------------------------

/// What the pipeline thread reports.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineNotice {
    /// One 100 ms frame's level (10 Hz per channel).
    Level {
        at_sample: u64,
        level: Level,
        gain_db: f32,
        raw_peak_dbfs: f32,
    },
    Event(PipelineEvent),
    Feed(FeedTransition),
    /// A WAV or store write failed (recording continues where it can).
    Error(String),
}

/// Commands for a running pipeline thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineCommand {
    /// Flush the WAV and write pending anchors now (before suspend).
    Flush,
    /// Drain the ring, zero-fill up to `at_qpc`, write the last anchors and
    /// return.
    Stop { at_qpc: i64 },
}

/// A copy of every frame for another consumer (the echo monitor, plan 15):
/// a bounded channel the pipeline only ever `try_send`s to; a frame that
/// does not fit (the consumer stalled) is dropped and counted. The copy a
/// full channel hands back is kept and copied into for the next offer, so a
/// stalled consumer costs the pipeline thread no allocation.
pub struct FrameTap {
    tx: SyncSender<Frame>,
    dropped: Arc<AtomicU64>,
    /// The copy the channel refused last (each clone keeps its own).
    spare: Mutex<Option<Frame>>,
}

impl Clone for FrameTap {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            dropped: Arc::clone(&self.dropped),
            spare: Mutex::new(None),
        }
    }
}

/// `src` copied into `dst`, reusing its sample buffer.
fn copy_frame(dst: &mut Frame, src: &Frame) {
    let Frame {
        channel,
        start_sample,
        pts_us,
        pcm,
        level,
        gain_db,
        raw_peak_dbfs,
    } = src;
    dst.pcm.clear();
    dst.pcm.extend_from_slice(pcm);
    dst.channel = *channel;
    dst.start_sample = *start_sample;
    dst.pts_us = *pts_us;
    dst.level = *level;
    dst.gain_db = *gain_db;
    dst.raw_peak_dbfs = *raw_peak_dbfs;
}

impl FrameTap {
    /// A tap holding up to `capacity` frames, and its receiving end.
    pub fn new(capacity: usize) -> (Self, Receiver<Frame>) {
        let (tx, rx) = sync_channel(capacity);
        (
            Self {
                tx,
                dropped: Arc::new(AtomicU64::new(0)),
                spare: Mutex::new(None),
            },
            rx,
        )
    }

    /// Copies `frame` if there is room; never waits.
    pub fn offer(&self, frame: &Frame) {
        let mut spare = self.spare.lock().unwrap_or_else(|e| e.into_inner());
        let copy = match spare.take() {
            Some(mut f) => {
                copy_frame(&mut f, frame);
                f
            }
            None => frame.clone(),
        };
        match self.tx.try_send(copy) {
            Ok(()) => {}
            Err(TrySendError::Full(f) | TrySendError::Disconnected(f)) => {
                *spare = Some(f);
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Frames that did not fit (over every clone of this tap).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The drop counter alone (holding it does not keep the channel open).
    pub fn drop_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dropped)
    }
}

/// Where a pipeline thread writes.
pub struct PipelineThreadConfig {
    pub channel: Channel,
    pub origin_qpc: i64,
    pub wav_path: PathBuf,
    /// The engine feed; `None` records only.
    pub feed: Option<FeedTx>,
    /// Anchors go here in batches (with `meeting`).
    pub store: Option<Arc<dyn RunStore>>,
    pub meeting: MeetingId,
    /// Anchors also go here for live time mapping.
    pub clock: Arc<Mutex<ClockMap>>,
    /// Automatic gain before the WAV and the feed (`None`: off).
    pub agc: Option<AgcConfig>,
    /// With AGC, the gain track goes here (`audio::gain_track`; `None`: not
    /// written).
    pub gain_path: Option<PathBuf>,
    /// Every frame is also offered here (`None`: no copy).
    pub tap: Option<FrameTap>,
    pub notify: Arc<dyn Fn(Channel, PipelineNotice) + Send + Sync>,
}

/// What a finished pipeline thread hands back: the WAV is not finalized, so
/// the recorder can pad both channels to the same length first.
pub struct PipelineResult {
    pub pipeline: ChannelPipeline,
    pub wav: WavSink,
    /// Frames the feed did not send to the engine.
    pub feed_dropped: u64,
}

/// Anchors written per store batch.
const ANCHOR_BATCH: usize = 10;

/// Starts the pipeline thread for one channel.
pub fn spawn_pipeline(
    cfg: PipelineThreadConfig,
    mut ring: rtrb::Consumer<Packet>,
    commands: Receiver<PipelineCommand>,
) -> Result<JoinHandle<Result<PipelineResult>>> {
    let mut wav = WavSink::create(&cfg.wav_path)?;
    let mut gains = match (&cfg.gain_path, cfg.agc) {
        (Some(p), Some(_)) => Some(GainTrackWriter::create(p)?),
        _ => None,
    };
    std::thread::Builder::new()
        .name(format!("pipeline-{}", cfg.channel))
        .spawn(move || {
            let mut cfg = cfg;
            let mut p = ChannelPipeline::new(cfg.channel, cfg.origin_qpc).with_agc(cfg.agc);
            let mut out = Outputs::default();
            let mut pending: Vec<ClockAnchor> = Vec::new();
            let mut stop_at: Option<i64> = None;
            loop {
                match commands.try_recv() {
                    Ok(PipelineCommand::Flush) => {
                        if let Err(e) = wav.flush() {
                            (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                        }
                        flush_gains(&cfg, &mut gains);
                        write_anchors(&cfg, &mut pending, true);
                    }
                    Ok(PipelineCommand::Stop { at_qpc }) => stop_at = Some(at_qpc),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        stop_at.get_or_insert(p.end_qpc());
                    }
                }
                let mut worked = false;
                while let Ok(packet) = ring.pop() {
                    worked = true;
                    if let Err(e) = p.push(packet, &mut out) {
                        (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                    }
                    deliver(&mut cfg, &mut wav, &mut gains, &mut out, &mut pending);
                }
                if let Some(at) = stop_at {
                    p.fill_to(at, &mut out);
                    p.finish(&mut out);
                    deliver(&mut cfg, &mut wav, &mut gains, &mut out, &mut pending);
                    if let Err(e) = wav.flush() {
                        (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                    }
                    flush_gains(&cfg, &mut gains);
                    write_anchors(&cfg, &mut pending, true);
                    let feed_dropped = cfg.feed.as_ref().map_or(0, FeedTx::dropped);
                    // Dropping the feed tells the worker the stream has ended.
                    drop(cfg.feed.take());
                    return Ok(PipelineResult {
                        pipeline: p,
                        wav,
                        feed_dropped,
                    });
                }
                if !worked {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        })
        .map_err(|e| audio_err(format!("spawn pipeline thread: {e}")))
}

fn flush_gains(cfg: &PipelineThreadConfig, gains: &mut Option<GainTrackWriter>) {
    if let Some(Err(e)) = gains.as_mut().map(GainTrackWriter::flush) {
        (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
    }
}

/// Writes outputs to the WAV, gain track, feed, clock and notices; clears
/// `out`.
fn deliver(
    cfg: &mut PipelineThreadConfig,
    wav: &mut WavSink,
    gains: &mut Option<GainTrackWriter>,
    out: &mut Outputs,
    pending: &mut Vec<ClockAnchor>,
) {
    if !out.samples.is_empty() {
        if let Err(e) = wav.write(&out.samples) {
            (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
        }
    }
    if let Some(w) = gains.as_mut() {
        for r in &out.gains {
            if let Err(e) = w.push(r.start_sample, &r.values) {
                (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                break;
            }
        }
    }
    if !out.anchors.is_empty() {
        let mut clock = cfg.clock.lock().unwrap_or_else(|e| e.into_inner());
        for &(s, q) in &out.anchors {
            clock.push(s as i64, q);
            pending.push(ClockAnchor {
                meeting_id: cfg.meeting,
                channel: cfg.channel,
                sample_index: s as i64,
                qpc_100ns: q,
            });
        }
    }
    for ev in out.events.drain(..) {
        (cfg.notify)(cfg.channel, PipelineNotice::Event(ev));
    }
    for frame in out.frames.drain(..) {
        (cfg.notify)(
            cfg.channel,
            PipelineNotice::Level {
                at_sample: frame.start_sample,
                level: frame.level,
                gain_db: frame.gain_db,
                raw_peak_dbfs: frame.raw_peak_dbfs,
            },
        );
        if let Some(tap) = &cfg.tap {
            tap.offer(&frame);
        }
        if let Some(feed) = cfg.feed.as_mut() {
            if let Some(t) = feed.offer(frame) {
                (cfg.notify)(cfg.channel, PipelineNotice::Feed(t));
            }
        }
    }
    out.clear();
    write_anchors(cfg, pending, false);
}

fn write_anchors(cfg: &PipelineThreadConfig, pending: &mut Vec<ClockAnchor>, force: bool) {
    if pending.is_empty() || (!force && pending.len() < ANCHOR_BATCH) {
        return;
    }
    if let Some(store) = &cfg.store {
        if let Err(e) = store.add_clock_anchors(pending) {
            (cfg.notify)(
                cfg.channel,
                PipelineNotice::Error(format!("clock anchors: {e}")),
            );
            return;
        }
    }
    pending.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::feed::feed;

    const ORIGIN: i64 = 1_000_000_000;

    fn pcm(q: i64, n: usize, v: i16) -> Packet {
        Packet::Audio {
            qpc_100ns: q,
            flags: PacketFlags::default(),
            data: PacketData::Pcm16(vec![v; n]),
        }
    }

    fn run(p: &mut ChannelPipeline, packets: Vec<Packet>) -> Outputs {
        let mut all = Outputs::default();
        let mut out = Outputs::default();
        for pk in packets {
            p.push(pk, &mut out).unwrap();
            all.samples.append(&mut out.samples);
            all.frames.append(&mut out.frames);
            all.anchors.append(&mut out.anchors);
            all.events.append(&mut out.events);
        }
        all
    }

    #[test]
    fn gap_of_300_ms_is_exactly_4800_zeros() {
        let mut p = ChannelPipeline::new(Channel::Mic, ORIGIN);
        let out = run(
            &mut p,
            vec![
                pcm(ORIGIN, 1_600, 5),
                // 100 ms of audio, then 300 ms with no packets.
                pcm(ORIGIN + 1_000_000 + 3_000_000, 1_600, 7),
            ],
        );
        assert_eq!(out.samples.len(), 1_600 + 4_800 + 1_600);
        assert!(out.samples[1_600..6_400].iter().all(|&s| s == 0));
        assert_eq!(out.samples[6_400], 7);
        assert_eq!(
            out.events,
            vec![PipelineEvent::Gap {
                at_sample: 1_600,
                samples: 4_800
            }]
        );
        assert_eq!(p.stats().gap_samples, 4_800);
        // Anchor at 0, the gap's two sides.
        assert_eq!(out.anchors[0], (0, ORIGIN));
        assert!(out.anchors.contains(&(1_600, ORIGIN + 1_000_000)));
        assert!(out.anchors.contains(&(6_400, ORIGIN + 4_000_000)));
        // Five 100 ms frames with pts.
        assert_eq!(out.frames.len(), 5);
        assert_eq!(out.frames[4].pts_us, 400_000);
    }

    #[test]
    fn leading_silence_jitter_and_overlap() {
        let mut p = ChannelPipeline::new(Channel::Loopback, ORIGIN);
        // The first packet arrives 50 ms after the origin: 800 zeros first.
        let out = run(&mut p, vec![pcm(ORIGIN + 500_000, 160, 1)]);
        assert_eq!(out.samples.len(), 960);
        assert!(out.samples[..800].iter().all(|&s| s == 0));
        // 2 ms of timestamp jitter: no fill.
        let q = ORIGIN + 500_000 + 160 * 625 + 20_000;
        let out = run(&mut p, vec![pcm(q, 160, 2)]);
        assert_eq!(out.samples.len(), 160);
        assert!(out.events.is_empty());
        // A packet starting 10 ms before the timeline end: its head is cut.
        let end = q + 160 * 625;
        let out = run(&mut p, vec![pcm(end - 100_000, 320, 3)]);
        assert_eq!(out.samples.len(), 160);
        assert_eq!(p.stats().overlap_samples, 160);
    }

    #[test]
    fn a_resume_is_anchored_not_filled() {
        let mut p = ChannelPipeline::new(Channel::Mic, ORIGIN);
        let hour = 36_000_000_000i64;
        let out = run(
            &mut p,
            vec![
                pcm(ORIGIN, 1_600, 1),
                Packet::Control {
                    qpc_100ns: ORIGIN + hour,
                    control: Control::Resumed { gap_hns: hour },
                },
                pcm(ORIGIN + 1_000_000 + hour, 1_600, 2),
            ],
        );
        assert_eq!(out.samples.len(), 3_200);
        assert!(out.anchors.contains(&(1_600, ORIGIN + 1_000_000 + hour)));
        assert!(matches!(
            out.events[0],
            PipelineEvent::Jump {
                at_sample: 1_600,
                ..
            }
        ));
        let mut map = ClockMap::new(ORIGIN);
        for (s, q) in &out.anchors {
            map.push(*s as i64, *q);
        }
        // A word in the second packet is an hour later in meeting time.
        assert_eq!(map.meeting_ms(150), 3_600_150);
        assert_eq!(map.meeting_ms(50), 50);
    }

    #[test]
    fn leaving_synthesized_silence_is_not_a_gap() {
        let mut p = ChannelPipeline::new(Channel::Loopback, ORIGIN);
        let out = run(
            &mut p,
            vec![
                Packet::Audio {
                    qpc_100ns: ORIGIN,
                    flags: PacketFlags {
                        silent: true,
                        synthetic: true,
                        ..Default::default()
                    },
                    data: PacketData::Pcm16(vec![0; 3_200]),
                },
                // Playback starts: the first real packet is 100 ms past the
                // synthesized silence.
                pcm(ORIGIN + 3_000_000, 160, 4),
            ],
        );
        assert_eq!(out.samples.len(), 4_800 + 160);
        assert!(out.events.is_empty(), "{:?}", out.events);
        assert_eq!(p.stats().gaps, 0);
        assert_eq!(p.stats().synthetic_packets, 1);
    }

    #[test]
    fn agc_raises_quiet_audio_before_the_wav_and_reports_its_gain() {
        let mut p =
            ChannelPipeline::new(Channel::Loopback, ORIGIN).with_agc(Some(AgcConfig::default()));
        // 3 s of a -70 dBFS tone (about 10 LSB) in 10 ms packets.
        let packets: Vec<Packet> = (0..300i64)
            .map(|k| Packet::Audio {
                qpc_100ns: ORIGIN + k * 100_000,
                flags: PacketFlags::default(),
                data: PacketData::Pcm16(
                    (0..160)
                        .map(|i| {
                            let t = (k * 160 + i) as f32 / 16_000.0;
                            ((t * 300.0 * std::f32::consts::TAU).sin() * 10.4).round() as i16
                        })
                        .collect(),
                ),
            })
            .collect();
        let out = run(&mut p, packets);
        assert_eq!(out.samples.len(), 48_000);
        let last = out.frames.last().unwrap();
        assert!((last.gain_db - 40.0).abs() < 0.1, "{}", last.gain_db);
        assert!(
            (last.raw_peak_dbfs - -70.0).abs() < 1.5,
            "{}",
            last.raw_peak_dbfs
        );
        // What the WAV and the engine get: near -30 dBFS (the +40 dB ceiling).
        assert!(
            (last.level.peak_dbfs - -30.0).abs() < 1.5,
            "{}",
            last.level.peak_dbfs
        );
        let peak = out.samples[16_000..]
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap();
        assert!((900..1_100).contains(&peak), "{peak}");
        // Without AGC the frame reports 0 dB and the raw peak is the level.
        let mut q = ChannelPipeline::new(Channel::Mic, ORIGIN);
        let out = run(&mut q, vec![pcm(ORIGIN, 1_600, 100)]);
        assert_eq!(out.frames[0].gain_db, 0.0);
        assert_eq!(out.frames[0].raw_peak_dbfs, out.frames[0].level.peak_dbfs);
    }

    #[test]
    fn silent_packets_are_zeroed() {
        let mut p = ChannelPipeline::new(Channel::Mic, ORIGIN);
        let out = run(
            &mut p,
            vec![
                pcm(ORIGIN, 160, 9),
                Packet::Audio {
                    qpc_100ns: ORIGIN + 100_000,
                    flags: PacketFlags {
                        silent: true,
                        ..Default::default()
                    },
                    data: PacketData::Pcm16(vec![1_234; 160]),
                },
            ],
        );
        assert_eq!(out.samples.len(), 320);
        assert!(out.samples[..160].iter().all(|&s| s == 9));
        assert!(out.samples[160..].iter().all(|&s| s == 0));
        assert_eq!(p.stats().silent_packets, 1);
    }

    #[test]
    fn anchors_recover_100_ppm_drift() {
        // The device is 100 ppm slow against QPC: every 10 ms packet spans
        // 10.001 ms of QPC. Packet-to-packet jitter stays far under the
        // tolerance, so nothing is filled; the anchors carry the drift.
        let mut p = ChannelPipeline::new(Channel::Mic, ORIGIN);
        let per_packet = 160.0 * 625.0 * (1.0 + 100e-6);
        let packets: Vec<Packet> = (0..6_000)
            .map(|i| pcm(ORIGIN + (i as f64 * per_packet).round() as i64, 160, 1))
            .collect();
        let out = run(&mut p, packets);
        assert_eq!(out.samples.len(), 960_000);
        assert!(out.events.is_empty());
        let mut map = ClockMap::new(ORIGIN);
        for (s, q) in &out.anchors {
            map.push(*s as i64, *q);
        }
        assert_eq!(map.len(), 60);
        let ppm = map.drift_ppm().unwrap();
        assert!((ppm - 100.0).abs() < 10.0, "drift {ppm} ppm");
        // 59 s of samples map to 59.0059 s of meeting time.
        assert_eq!(map.meeting_ms(59_000), 59_006);
    }

    #[test]
    fn mix_format_48k_stereo_sine_resamples_to_16k() {
        let mut p = ChannelPipeline::new(Channel::Loopback, ORIGIN);
        let mut out = Outputs::default();
        let mut samples = Vec::new();
        // 2 s of a 1 kHz sine, stereo 48 kHz, in 10 ms packets.
        for k in 0..200 {
            let mut data = Vec::with_capacity(960);
            for i in 0..480 {
                let t = (k * 480 + i) as f32 / 48_000.0;
                let v = (t * 1_000.0 * std::f32::consts::TAU).sin() * 0.5;
                data.push(v);
                data.push(v);
            }
            p.push(
                Packet::Audio {
                    qpc_100ns: ORIGIN + k as i64 * 100_000,
                    flags: PacketFlags::default(),
                    data: PacketData::Float {
                        rate: 48_000,
                        channels: 2,
                        samples: data,
                    },
                },
                &mut out,
            )
            .unwrap();
            samples.append(&mut out.samples);
            assert!(out.events.is_empty(), "{:?}", out.events);
            out.clear();
        }
        // The resampler holds back at most one chunk.
        assert!(
            (31_800..=32_000).contains(&samples.len()),
            "{}",
            samples.len()
        );
        // Frequency from zero crossings over the steady part.
        let s = &samples[1_600..samples.len() - 1_600];
        let crossings = s.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        let hz = crossings as f64 / (s.len() as f64 / 16_000.0);
        assert!((hz - 1_000.0).abs() < 5.0, "{hz} Hz");
        let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!((15_000..=17_500).contains(&peak), "peak {peak}");
    }

    #[test]
    fn stalled_consumer_degrades_and_recovers_without_losing_wav_samples() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("mic.wav");
        // A 1 s feed so the test is quick; the worker stalls for 2 s.
        let (tx, rx) = feed(10, 2);
        // A tap nobody reads: it fills, drops and counts; the WAV and the
        // feed are not affected.
        let (tap, tap_rx) = FrameTap::new(4);
        let tap_count = tap.clone();
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n2 = Arc::clone(&notices);
        let cfg = PipelineThreadConfig {
            channel: Channel::Mic,
            origin_qpc: ORIGIN,
            wav_path: wav.clone(),
            feed: Some(tx),
            store: None,
            meeting: MeetingId(1),
            clock: Arc::new(Mutex::new(ClockMap::new(ORIGIN))),
            agc: None,
            gain_path: Some(dir.path().join("mic.gain")),
            tap: Some(tap),
            notify: Arc::new(move |_, n| {
                if let PipelineNotice::Feed(t) = n {
                    n2.lock().unwrap().push(t);
                }
            }),
        };
        let (mut prod, cons) = rtrb::RingBuffer::new(8_192);
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let handle = spawn_pipeline(cfg, cons, cmd_rx).unwrap();
        let consumer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2_000));
            let mut got = 0;
            while let Ok(f) = rx.recv_timeout(Duration::from_millis(5_000)) {
                assert_eq!(f.pcm.len(), FRAME_SAMPLES);
                got += 1;
            }
            got
        });
        // 6 s of audio at 4x real time.
        for i in 0..600i64 {
            prod.push(pcm(ORIGIN + i * 100_000, 160, (i % 100) as i16 + 1))
                .unwrap();
            if i % 4 == 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        std::thread::sleep(Duration::from_millis(2_500));
        // More audio after the consumer caught up.
        for i in 600..800i64 {
            prod.push(pcm(ORIGIN + i * 100_000, 160, 1)).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        cmd_tx
            .send(PipelineCommand::Stop {
                at_qpc: ORIGIN + 800 * 100_000,
            })
            .unwrap();
        let res = handle.join().unwrap().unwrap();
        let frames_got = consumer.join().unwrap();
        assert_eq!(res.wav.finalize().unwrap(), 128_000);
        assert_eq!(hound::WavReader::open(&wav).unwrap().duration(), 128_000);
        let t = notices.lock().unwrap().clone();
        assert_eq!(t.first(), Some(&FeedTransition::Degraded), "{t:?}");
        assert!(t.contains(&FeedTransition::Recovered), "{t:?}");
        assert!(res.feed_dropped > 0);
        assert_eq!(frames_got as u64 + res.feed_dropped, 80);
        assert_eq!(tap_rx.try_iter().count(), 4);
        assert_eq!(tap_count.dropped(), 76);
        assert!(
            !dir.path().join("mic.gain").exists(),
            "no AGC, no gain track"
        );
    }

    /// 10 ms packets: a -70 dBFS tone, 300 ms without packets, then a
    /// -12 dBFS tone (the gain falls; the limiter caps the first block).
    fn agc_packets() -> (Vec<Packet>, Vec<i16>) {
        let tone = |k: i64, amp: f32| -> Vec<i16> {
            (0..160)
                .map(|i| {
                    let t = (k * 160 + i) as f32 / 16_000.0;
                    ((t * 300.0 * std::f32::consts::TAU).sin() * amp).round() as i16
                })
                .collect()
        };
        let mut packets = Vec::new();
        let mut timeline = Vec::new();
        for k in 0..100i64 {
            let x = tone(k, 10.4);
            timeline.extend_from_slice(&x);
            packets.push(Packet::Audio {
                qpc_100ns: ORIGIN + k * 100_000,
                flags: PacketFlags::default(),
                data: PacketData::Pcm16(x),
            });
        }
        timeline.extend(std::iter::repeat_n(0, 4_800));
        for k in 130..250i64 {
            let x = tone(k, 8_200.0);
            timeline.extend_from_slice(&x);
            packets.push(Packet::Audio {
                qpc_100ns: ORIGIN + k * 100_000,
                flags: PacketFlags::default(),
                data: PacketData::Pcm16(x),
            });
        }
        (packets, timeline)
    }

    #[test]
    fn the_gain_track_undoes_the_agc_on_the_wav() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("loopback.wav");
        let gain_path = crate::audio::gain_track::path_for(&wav);
        let cfg = PipelineThreadConfig {
            channel: Channel::Loopback,
            origin_qpc: ORIGIN,
            wav_path: wav.clone(),
            feed: None,
            store: None,
            meeting: MeetingId(1),
            clock: Arc::new(Mutex::new(ClockMap::new(ORIGIN))),
            agc: Some(AgcConfig::default()),
            gain_path: Some(gain_path.clone()),
            tap: None,
            notify: Arc::new(|_, _| {}),
        };
        let (mut prod, cons) = rtrb::RingBuffer::new(1_024);
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let handle = spawn_pipeline(cfg, cons, cmd_rx).unwrap();
        let (packets, raw) = agc_packets();
        for pk in packets {
            prod.push(pk).unwrap();
        }
        cmd_tx
            .send(PipelineCommand::Stop {
                at_qpc: ORIGIN + 250 * 100_000,
            })
            .unwrap();
        let res = handle.join().unwrap().unwrap();
        assert_eq!(res.wav.finalize().unwrap(), raw.len() as u64);
        let stored: Vec<f32> = hound::WavReader::open(&wav)
            .unwrap()
            .into_samples::<i16>()
            .map(|s| f32::from(s.unwrap()))
            .collect();
        let track = crate::audio::gain_track::GainTrack::read(&gain_path)
            .unwrap()
            .unwrap();
        assert_eq!(track.frames(), raw.len().div_ceil(FRAME_SAMPLES));
        // Raised by 40 dB at the ceiling, held over the gap, then down.
        assert!((track.gain_db_at(15_000) - 40.0).abs() < 0.01);
        assert!((track.gain_db_at(17_000) - 40.0).abs() < 0.01);
        let g_loud = track.gain_db_at(21_000);
        assert!(g_loud > 0.0 && g_loud < 12.0, "{g_loud}");
        assert_eq!(track.gain_db_at(raw.len() as u64 - 1), 0.0);
        // Replayed backwards, the WAV is the capture again, to the PCM16
        // rounding after the gain (and the limiter's clamp at -1 dBFS, which
        // the first loud block's peak touches).
        let mut back = stored.clone();
        track.undo(0, &mut back);
        for (i, (b, r)) in back.iter().zip(&raw).enumerate() {
            let tol = 1.0 / 10f32.powf(track.gain_db_at(i as u64) / 20.0) + 1e-3;
            assert!(
                (b - f32::from(*r)).abs() <= tol,
                "sample {i}: {b} against {r}"
            );
        }
    }

    #[test]
    fn gain_records_follow_the_timeline_not_the_packets() {
        // 250-sample packets: the AGC's 10 ms blocks straddle the
        // timeline's; each record value is the mean gain of its samples.
        let mut p =
            ChannelPipeline::new(Channel::Loopback, ORIGIN).with_agc(Some(AgcConfig::default()));
        let mut out = Outputs::default();
        let mut gains_per_sample = Vec::new();
        let mut records = Vec::new();
        for k in 0..64i64 {
            let x: Vec<i16> = (0..250)
                .map(|i| {
                    let t = (k * 250 + i) as f32 / 16_000.0;
                    ((t * 440.0 * std::f32::consts::TAU).sin()
                        * if k < 32 { 30.0 } else { 3_000.0 })
                    .round() as i16
                })
                .collect();
            p.push(
                Packet::Audio {
                    qpc_100ns: ORIGIN + k * 250 * 625,
                    flags: PacketFlags::default(),
                    data: PacketData::Pcm16(x),
                },
                &mut out,
            )
            .unwrap();
            gains_per_sample.extend_from_slice(&p.packet_gains);
            records.append(&mut out.gains);
            out.clear();
        }
        p.finish(&mut out);
        records.append(&mut out.gains);
        assert_eq!(records.len(), 10, "16,000 samples: ten frames");
        for (f, r) in records.iter().enumerate() {
            assert_eq!(r.start_sample, f as u64 * 1_600);
            for (b, v) in r.values.iter().enumerate() {
                let a = f * 1_600 + b * 160;
                let mean = gains_per_sample[a..a + 160]
                    .iter()
                    .map(|&g| f64::from(g))
                    .sum::<f64>()
                    / 160.0;
                assert!((f64::from(*v) - mean).abs() < 1e-4, "frame {f} block {b}");
            }
        }
        assert!(records[0].values[0] > 25.0 && records[9].values[9] < 10.0);
    }

    #[test]
    fn a_full_tap_copies_into_the_frame_it_refused() {
        let (tap, rx) = FrameTap::new(1);
        let frame = |k: u64, v: i16| Frame {
            channel: Channel::Mic,
            start_sample: k * 1_600,
            pts_us: k * 100_000,
            pcm: vec![v; FRAME_SAMPLES],
            level: Level::default(),
            gain_db: 0.5,
            raw_peak_dbfs: -20.0,
        };
        tap.offer(&frame(0, 1));
        tap.offer(&frame(1, 2));
        let buf = |t: &FrameTap| {
            t.spare
                .lock()
                .unwrap()
                .as_ref()
                .map(|f| (f.pcm.as_ptr(), f.start_sample))
        };
        let (ptr, at) = buf(&tap).unwrap();
        assert_eq!(at, 1_600);
        // Still full: the same buffer takes the next frame.
        tap.offer(&frame(2, 3));
        assert_eq!(buf(&tap), Some((ptr, 3_200)));
        assert_eq!(tap.dropped(), 2);
        assert_eq!(rx.try_recv().unwrap(), frame(0, 1));
        // Room again: the spare goes out with the next frame's contents.
        tap.offer(&frame(3, 4));
        assert_eq!(buf(&tap), None);
        let got = rx.try_recv().unwrap();
        assert_eq!((got.pcm.as_ptr(), &got), (ptr, &frame(3, 4)));
        assert_eq!(tap.clone().dropped(), 2);
    }
}
