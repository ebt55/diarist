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
//! ring, writes the WAV ([`super::wav::WavSink`], flushed every second),
//! offers frames to the engine feed (never blocking), records anchors
//! through [`RunStore::add_clock_anchors`] in batches and into a shared
//! [`ClockMap`], and reports levels, gaps, device changes and feed
//! transitions through a callback.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
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
    pub level: Level,
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
}

impl Outputs {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.frames.clear();
        self.anchors.clear();
        self.events.clear();
    }
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
    mix: Option<MixResampler>,
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
            mix: None,
            stats: PipelineStats::default(),
        }
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
                match data {
                    PacketData::Pcm16(samples) => {
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
                        for (q, pcm) in chunks {
                            let f = PacketFlags {
                                discontinuity: flags.discontinuity,
                                ..Default::default()
                            };
                            self.place(q, pcm, f, out);
                        }
                        Ok(())
                    }
                }
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
            if gap > tol {
                let fill = ((gap as f64) / HNS_PER_SAMPLE as f64).round() as u64;
                if fill > 0 {
                    if !first {
                        out.anchors.push((self.position, expected));
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
        }
        if flags.silent {
            samples.iter_mut().for_each(|s| *s = 0);
        }
        if skip < n {
            let q0 = q + skip as i64 * HNS_PER_SAMPLE;
            self.emit(&samples[skip..], q0, out);
        }
        self.next_qpc = Some(q + n as i64 * HNS_PER_SAMPLE);
    }

    fn emit_zeros(&mut self, mut n: u64, mut q0: i64, out: &mut Outputs) {
        let zeros = [0i16; FRAME_SAMPLES];
        while n > 0 {
            let k = (n as usize).min(FRAME_SAMPLES);
            self.emit(&zeros[..k], q0, out);
            q0 += k as i64 * HNS_PER_SAMPLE;
            n -= k as u64;
        }
    }

    /// Appends samples whose first one is at QPC `q0`.
    fn emit(&mut self, pcm: &[i16], q0: i64, out: &mut Outputs) {
        let len = pcm.len() as u64;
        while self.next_anchor < self.position + len {
            let idx = self.next_anchor;
            out.anchors
                .push((idx, q0 + (idx - self.position) as i64 * HNS_PER_SAMPLE));
            self.next_anchor += ANCHOR_EVERY;
        }
        out.samples.extend_from_slice(pcm);
        let mut rest = pcm;
        while !rest.is_empty() {
            let k = (FRAME_SAMPLES - self.frame.len()).min(rest.len());
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
        out.frames.push(Frame {
            channel: self.channel,
            start_sample: start,
            pts_us: start * 1_000_000 / u64::from(CAPTURE_RATE),
            level: Level::of(&pcm),
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
    /// 16 kHz chunks with the QPC of their first sample.
    pub fn push(
        &mut self,
        q: i64,
        interleaved: &[f32],
        silent: bool,
    ) -> Result<Vec<(i64, Vec<i16>)>> {
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
            let pcm = self.fifo.drain(..).map(to_i16).collect::<Vec<_>>();
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
                let pcm = self.out[trim..produced]
                    .iter()
                    .copied()
                    .map(to_i16)
                    .collect();
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
    std::thread::Builder::new()
        .name(format!("pipeline-{}", cfg.channel))
        .spawn(move || {
            let mut cfg = cfg;
            let mut p = ChannelPipeline::new(cfg.channel, cfg.origin_qpc);
            let mut out = Outputs::default();
            let mut pending: Vec<ClockAnchor> = Vec::new();
            let mut stop_at: Option<i64> = None;
            loop {
                match commands.try_recv() {
                    Ok(PipelineCommand::Flush) => {
                        if let Err(e) = wav.flush() {
                            (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                        }
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
                    deliver(&mut cfg, &mut wav, &mut out, &mut pending);
                }
                if let Some(at) = stop_at {
                    p.fill_to(at, &mut out);
                    p.finish(&mut out);
                    deliver(&mut cfg, &mut wav, &mut out, &mut pending);
                    if let Err(e) = wav.flush() {
                        (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
                    }
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

/// Writes outputs to the WAV, feed, clock and notices; clears `out`.
fn deliver(
    cfg: &mut PipelineThreadConfig,
    wav: &mut WavSink,
    out: &mut Outputs,
    pending: &mut Vec<ClockAnchor>,
) {
    if !out.samples.is_empty() {
        if let Err(e) = wav.write(&out.samples) {
            (cfg.notify)(cfg.channel, PipelineNotice::Error(e.to_string()));
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
            },
        );
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
    }
}
