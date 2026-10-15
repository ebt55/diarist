//! File decode to 16 kHz mono PCM16 WAV (`docs/01`, "Audio decode").
//!
//! A chain of [`Decoder`]s tried in order by [`ChainDecoder`]:
//! 1. [`SymphoniaDecoder`]: pure Rust (AAC-LC in MP4/M4A, MP3, FLAC, WAV,
//!    Ogg/Vorbis) to f32, downmixed to mono and resampled to 16 kHz with
//!    rubato (`Fft` for integer ratios such as 48 kHz, `Async` sinc
//!    otherwise, e.g. 44.1 kHz), written with hound. Gapless: symphonia's
//!    `enable_gapless` is on, and for MP4/M4A the edit list's priming and
//!    padding are trimmed here (see [`super::mp4`]), so decoded time 0 is
//!    the original file's time 0.
//! 2. [`MediaFoundationDecoder`] (Windows): `IMFSourceReader` decodes what
//!    Windows can (HE-AAC, video containers), same downmix and resampler.
//! 3. [`FfmpegDecoder`]: `ffmpeg` when it is on `PATH`.
//! 4. Otherwise an error that shows the ffmpeg command the user could run.
//!
//! Decoding streams: memory stays bounded for multi-hour files.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, Fft, FixedAsync, FixedSync, Indexing, Resampler, SincInterpolationParameters};

use super::{DecodedInfo, Decoder, DecoderKind};
use crate::error::{CoreError, Result};

/// Output sample rate of every decoder.
pub const TARGET_RATE: u32 = 16_000;

fn decode_err(msg: impl Into<String>) -> CoreError {
    CoreError::Decode(msg.into())
}

// ---- resample + write ---------------------------------------------------------

/// Downmixes interleaved f32 to mono, resamples to 16 kHz and writes PCM16.
/// The resampler's start-up delay is trimmed and the output has exactly
/// `round(input_frames * 16000 / rate)` frames.
pub struct Pcm16kWriter {
    writer: hound::WavWriter<std::io::BufWriter<File>>,
    rate: u32,
    channels: usize,
    resampler: Option<Box<dyn Resampler<f32>>>,
    /// `resampler` was created (it is taken out while it runs).
    resampling: bool,
    pending: Vec<f32>,
    out: Vec<f32>,
    to_trim: usize,
    frames_in: u64,
    frames_out: u64,
    /// Stop accepting input after this many frames (container frame count).
    max_frames_in: Option<u64>,
}

impl Pcm16kWriter {
    /// Creates `output` for audio at `rate` Hz with `channels` channels.
    pub fn create(output: &Path, rate: u32, channels: usize) -> Result<Self> {
        if rate == 0 || channels == 0 {
            return Err(decode_err(format!(
                "unsupported stream: {rate} Hz, {channels} channels"
            )));
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: TARGET_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(output, spec)
            .map_err(|e| decode_err(format!("create {}: {e}", output.display())))?;
        let resampler: Option<Box<dyn Resampler<f32>>> = if rate == TARGET_RATE {
            None
        } else if rate.is_multiple_of(TARGET_RATE) || TARGET_RATE.is_multiple_of(rate) {
            Some(Box::new(
                Fft::<f32>::new(
                    rate as usize,
                    TARGET_RATE as usize,
                    4_096,
                    1,
                    FixedSync::Input,
                )
                .map_err(|e| decode_err(format!("resampler: {e}")))?,
            ))
        } else {
            Some(Box::new(
                Async::<f32>::new_sinc(
                    f64::from(TARGET_RATE) / f64::from(rate),
                    1.1,
                    &SincInterpolationParameters::default(),
                    1_024,
                    1,
                    FixedAsync::Input,
                )
                .map_err(|e| decode_err(format!("resampler: {e}")))?,
            ))
        };
        let to_trim = resampler.as_ref().map_or(0, |r| r.output_delay());
        Ok(Self {
            writer,
            rate,
            channels,
            resampling: resampler.is_some(),
            resampler,
            pending: Vec::new(),
            out: Vec::new(),
            to_trim,
            frames_in: 0,
            frames_out: 0,
            max_frames_in: None,
        })
    }

    /// Caps the number of input frames (e.g. the container's frame count, to
    /// drop codec padding).
    pub fn limit_input_frames(&mut self, frames: Option<u64>) {
        self.max_frames_in = frames.filter(|f| *f > 0);
    }

    /// Feeds interleaved samples (`channels` per frame).
    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<()> {
        let ch = self.channels;
        let mut frames = samples.len() / ch;
        if let Some(max) = self.max_frames_in {
            frames = frames.min(max.saturating_sub(self.frames_in) as usize);
        }
        if frames == 0 {
            return Ok(());
        }
        self.frames_in += frames as u64;
        let scale = 1.0 / ch as f32;
        let mono = samples[..frames * ch]
            .chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() * scale);
        if !self.resampling {
            let mono: Vec<f32> = mono.collect();
            return self.emit(&mono);
        }
        self.pending.extend(mono);
        self.drain(false)
    }

    fn drain(&mut self, flush: bool) -> Result<()> {
        let Some(mut r) = self.resampler.take() else {
            return Ok(());
        };
        let result = (|| -> Result<()> {
            loop {
                let need = r.input_frames_next();
                let have = self.pending.len();
                let partial = if have >= need {
                    None
                } else if flush {
                    Some(have)
                } else {
                    break;
                };
                let out_len = r.output_frames_next();
                self.out.resize(out_len, 0.0);
                let input_buf: Vec<f32>;
                let input: &[f32] = if have >= need {
                    &self.pending[..need]
                } else {
                    input_buf = {
                        let mut v = self.pending.clone();
                        v.resize(need, 0.0);
                        v
                    };
                    &input_buf
                };
                let adapter_in = InterleavedSlice::new(input, 1, need)
                    .map_err(|e| decode_err(format!("resampler input: {e}")))?;
                let mut adapter_out = InterleavedSlice::new_mut(&mut self.out, 1, out_len)
                    .map_err(|e| decode_err(format!("resampler output: {e}")))?;
                let indexing = partial.map(|p| Indexing::new().partial_len(p));
                let (used, produced) = r
                    .process_into_buffer(&adapter_in, &mut adapter_out, indexing.as_ref())
                    .map_err(|e| decode_err(format!("resample: {e}")))?;
                let consumed = used.min(have);
                self.pending.drain(..consumed);
                let produced_buf = std::mem::take(&mut self.out);
                let mut chunk = &produced_buf[..produced];
                let trim = self.to_trim.min(chunk.len());
                self.to_trim -= trim;
                chunk = &chunk[trim..];
                let res = self.emit(chunk);
                self.out = produced_buf;
                res?;
                if flush {
                    let expected = self.expected_out();
                    if self.frames_out >= expected {
                        break;
                    }
                    // Pump silence until the delay has been flushed out.
                    if self.pending.is_empty() && partial == Some(0) && produced == 0 {
                        break;
                    }
                }
            }
            Ok(())
        })();
        self.resampler = Some(r);
        result
    }

    fn expected_out(&self) -> u64 {
        (self.frames_in * u64::from(TARGET_RATE) + u64::from(self.rate) / 2) / u64::from(self.rate)
    }

    fn emit(&mut self, mono: &[f32]) -> Result<()> {
        let limit = if self.resampling {
            // Never write past the exact expected length (the flush pads with silence).
            let expected = self.expected_out();
            (expected.saturating_sub(self.frames_out) as usize).min(mono.len())
        } else {
            mono.len()
        };
        for &s in &mono[..limit] {
            let v = (s * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16;
            self.writer
                .write_sample(v)
                .map_err(|e| decode_err(format!("write wav: {e}")))?;
        }
        self.frames_out += limit as u64;
        Ok(())
    }

    /// Flushes the resampler and finalizes the WAV. Returns the facts about
    /// the decoded file.
    pub fn finish(mut self, source_channels: u16, decoder: DecoderKind) -> Result<DecodedInfo> {
        if self.resampling {
            self.drain(true)?;
        }
        let frames = self.frames_out;
        self.writer
            .finalize()
            .map_err(|e| decode_err(format!("finalize wav: {e}")))?;
        Ok(DecodedInfo {
            duration_ms: (frames * 1_000 / u64::from(TARGET_RATE)) as i64,
            source_sample_rate: self.rate,
            source_channels,
            decoder,
        })
    }
}

// ---- symphonia -----------------------------------------------------------------

/// Receives `(sample rate, channels, interleaved samples)` per decoded block.
pub(crate) type BlockSink<'a> = dyn FnMut(u32, usize, &[f32]) -> Result<()> + 'a;

/// Facts about a symphonia decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamInfo {
    pub rate: u32,
    pub channels: usize,
    /// Frames delivered after trimming.
    pub frames: u64,
}

/// Decodes `input` with symphonia and hands interleaved f32 blocks to
/// `on_block(rate, channels, samples)`. With `gapless`, the MP4 edit list
/// (priming and padding) is applied; otherwise only the container's frame
/// count caps the output. More than 1 percent undecodable packets is an
/// error, so the decoder chain can try the next decoder.
pub(crate) fn symphonia_stream(
    input: &Path,
    gapless: bool,
    on_block: &mut BlockSink<'_>,
) -> Result<StreamInfo> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file =
        File::open(input).map_err(|e| decode_err(format!("open {}: {e}", input.display())))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let fmt_opts = FormatOptions {
        enable_gapless: gapless,
        ..Default::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &MetadataOptions::default())
        .map_err(|e| decode_err(format!("symphonia probe: {e}")))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| decode_err("symphonia: no audio track"))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| decode_err(format!("symphonia codec: {e}")))?;
    let edit = if gapless {
        super::mp4::audio_edit(input)
    } else {
        None
    };

    let mut info = StreamInfo {
        rate: params.sample_rate.unwrap_or(0),
        channels: params.channels.map(|c| c.count()).unwrap_or(0),
        frames: 0,
    };
    // Trim window in source frames: [skip, skip + keep).
    let mut window: Option<(u64, Option<u64>)> = None;
    let mut position = 0u64;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut bad_packets = 0usize;
    let mut packets = 0usize;
    let mut started = false;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(decode_err(format!("symphonia read: {e}"))),
        };
        if packet.track_id() != track_id {
            continue;
        }
        packets += 1;
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => {
                bad_packets += 1;
                continue;
            }
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(decode_err(format!("symphonia decode: {e}"))),
        };
        let spec = *decoded.spec();
        if !started {
            started = true;
            info.rate = spec.rate;
            info.channels = spec.channels.count();
            window = Some(match edit.filter(|e| !e.is_identity()) {
                Some(e) => (e.skip_frames(spec.rate), e.present_frames(spec.rate)),
                None => (0, params.n_frames.filter(|n| *n > 0)),
            });
        }
        let cap = decoded.capacity() as u64;
        let buf = match &mut sample_buf {
            Some(b) if b.capacity() as u64 >= cap => b,
            _ => sample_buf.insert(SampleBuffer::<f32>::new(cap, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        let ch = info.channels.max(1);
        let samples = buf.samples();
        let frames = (samples.len() / ch) as u64;
        let (skip, keep) = window.unwrap_or((0, None));
        let from = skip.saturating_sub(position).min(frames);
        let end_abs = keep.map_or(u64::MAX, |k| skip.saturating_add(k));
        let to = end_abs.saturating_sub(position).min(frames);
        position += frames;
        if to > from {
            info.frames += to - from;
            on_block(
                info.rate,
                ch,
                &samples[from as usize * ch..to as usize * ch],
            )?;
        }
    }
    // More than 1 percent of packets undecodable: let the next decoder try.
    if packets == 0 || bad_packets * 100 > packets {
        return Err(decode_err(format!(
            "symphonia: {bad_packets} of {packets} packets failed to decode"
        )));
    }
    if !started {
        return Err(decode_err("symphonia: no audio decoded"));
    }
    Ok(info)
}

/// Pure-Rust decoder (symphonia): AAC-LC/MP4, MP3, FLAC, WAV, Ogg/Vorbis.
#[derive(Debug, Clone, Copy, Default)]
pub struct SymphoniaDecoder;

/// [`SymphoniaDecoder`] with gapless trimming on or off (off only for tests
/// that measure the priming).
pub(crate) fn symphonia_to_wav16k(
    input: &Path,
    output: &Path,
    gapless: bool,
) -> Result<DecodedInfo> {
    let mut writer: Option<Pcm16kWriter> = None;
    let info = symphonia_stream(input, gapless, &mut |rate, channels, samples| {
        let w = match &mut writer {
            Some(w) => w,
            None => writer.insert(Pcm16kWriter::create(output, rate, channels)?),
        };
        w.push_interleaved(samples)
    })?;
    let writer = match writer {
        Some(w) => w,
        // Everything trimmed away: an empty but valid WAV.
        None => Pcm16kWriter::create(output, info.rate.max(1), info.channels.max(1))?,
    };
    writer.finish(info.channels as u16, DecoderKind::Symphonia)
}

impl Decoder for SymphoniaDecoder {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo> {
        symphonia_to_wav16k(input, output, true)
    }
}

// ---- Media Foundation ---------------------------------------------------------------

/// Windows Media Foundation (`IMFSourceReader`) decoder: whatever codecs
/// Windows has (HE-AAC, video containers, WMA). Output goes through the same
/// downmix and resampler as [`SymphoniaDecoder`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaFoundationDecoder;

#[cfg(windows)]
impl Decoder for MediaFoundationDecoder {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo> {
        super::mf::decode(input, output)
    }
}

#[cfg(not(windows))]
impl Decoder for MediaFoundationDecoder {
    fn decode_to_wav16k(&self, _input: &Path, _output: &Path) -> Result<DecodedInfo> {
        Err(decode_err("Media Foundation is only available on Windows"))
    }
}

// ---- ffmpeg ---------------------------------------------------------------------------

/// Runs `ffmpeg` (found on `PATH`, or an explicit binary).
#[derive(Debug, Clone)]
pub struct FfmpegDecoder {
    pub ffmpeg: PathBuf,
}

impl FfmpegDecoder {
    /// `ffmpeg` from `PATH`, if present.
    pub fn from_path() -> Option<Self> {
        find_on_path("ffmpeg").map(|ffmpeg| Self { ffmpeg })
    }

    /// The command line this decoder runs (also shown to users when nothing can decode a file).
    pub fn command_line(input: &Path, output: &Path) -> String {
        format!(
            "ffmpeg -nostdin -i \"{}\" -vn -ac 1 -ar 16000 -c:a pcm_s16le \"{}\"",
            input.display(),
            output.display()
        )
    }
}

impl Decoder for FfmpegDecoder {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo> {
        let out = Command::new(&self.ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(input)
            .args([
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
            ])
            .arg(output)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| decode_err(format!("run {}: {e}", self.ffmpeg.display())))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(decode_err(format!(
                "ffmpeg failed ({}): {}",
                out.status,
                tail(&err, 1_000)
            )));
        }
        let (source_sample_rate, source_channels) = probe_stream(&self.ffmpeg, input);
        let reader = hound::WavReader::open(output)
            .map_err(|e| decode_err(format!("read ffmpeg output: {e}")))?;
        let frames = u64::from(reader.duration());
        Ok(DecodedInfo {
            duration_ms: (frames * 1_000 / u64::from(TARGET_RATE)) as i64,
            source_sample_rate,
            source_channels,
            decoder: DecoderKind::Ffmpeg,
        })
    }
}

/// Sample rate and channel count of the first audio stream via `ffprobe`
/// (next to `ffmpeg`, or on `PATH`); zeros when unknown.
fn probe_stream(ffmpeg: &Path, input: &Path) -> (u32, u16) {
    let ffprobe = ffmpeg
        .parent()
        .map(|d| {
            d.join(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            })
        })
        .filter(|p| p.is_file())
        .or_else(|| find_on_path("ffprobe"));
    let Some(ffprobe) = ffprobe else {
        return (0, 0);
    };
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,channels",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .stdin(Stdio::null())
        .output();
    let Ok(out) = out else {
        return (0, 0);
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split(',');
    let rate = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let channels = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    (rate, channels)
}

/// Finds an executable on `PATH` (adds `.exe` on Windows).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let file = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(&file))
        .find(|p| p.is_file())
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s.trim();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].trim()
}

// ---- chain ------------------------------------------------------------------------------

/// Tries each decoder in order; the first success wins. A failed attempt's
/// partial output is removed before the next one runs.
pub struct ChainDecoder {
    decoders: Vec<(&'static str, Box<dyn Decoder>)>,
    last: Mutex<Option<&'static str>>,
}

impl ChainDecoder {
    /// symphonia, then Media Foundation (Windows), then ffmpeg when on `PATH`.
    pub fn new() -> Self {
        let mut decoders: Vec<(&'static str, Box<dyn Decoder>)> =
            vec![("symphonia", Box::new(SymphoniaDecoder))];
        if cfg!(windows) {
            decoders.push(("media_foundation", Box::new(MediaFoundationDecoder)));
        }
        if let Some(ff) = FfmpegDecoder::from_path() {
            decoders.push(("ffmpeg", Box::new(ff)));
        }
        Self::with(decoders)
    }

    /// A chain of the given decoders (tests, custom setups).
    pub fn with(decoders: Vec<(&'static str, Box<dyn Decoder>)>) -> Self {
        Self {
            decoders,
            last: Mutex::new(None),
        }
    }

    /// Name of the decoder that handled the most recent successful call.
    pub fn last_decoder(&self) -> Option<&'static str> {
        *self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Names of the decoders in the chain, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.decoders.iter().map(|(n, _)| *n).collect()
    }
}

impl Default for ChainDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for ChainDecoder {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo> {
        if !input.is_file() {
            return Err(decode_err(format!("{} is not a file", input.display())));
        }
        if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut errors = Vec::new();
        for (name, d) in &self.decoders {
            match d.decode_to_wav16k(input, output) {
                Ok(info) => {
                    *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(name);
                    return Ok(info);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(output);
                    errors.push(format!("{name}: {e}"));
                }
            }
        }
        let has_ffmpeg = self.decoders.iter().any(|(n, _)| *n == "ffmpeg");
        let hint = if has_ffmpeg {
            String::new()
        } else {
            format!(
                ". ffmpeg is not on PATH; convert the file yourself and import the WAV: {}",
                FfmpegDecoder::command_line(input, &input.with_extension("16k.wav"))
            )
        };
        Err(decode_err(format!(
            "could not decode {} ({}){hint}",
            input.display(),
            errors.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a sine (one tone per channel) as 16-bit WAV.
    fn sine_wav(path: &Path, rate: u32, channels: u16, seconds: f64) -> u64 {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let frames = (f64::from(rate) * seconds).round() as u64;
        for i in 0..frames {
            let t = i as f64 / f64::from(rate);
            for c in 0..channels {
                let f = 440.0 * f64::from(c + 1);
                let v = (t * f * std::f64::consts::TAU).sin() * 0.5;
                w.write_sample((v * 32_767.0) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
        frames
    }

    fn read_mono16k(path: &Path) -> Vec<i16> {
        let r = hound::WavReader::open(path).unwrap();
        let spec = r.spec();
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.bits_per_sample, 16);
        r.into_samples::<i16>().map(|s| s.unwrap()).collect()
    }

    fn rms(s: &[i16]) -> f64 {
        (s.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn stereo_48k_becomes_mono_16k_with_exact_length() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        let output = dir.path().join("out.wav");
        sine_wav(&input, 48_000, 2, 3.25);
        let info = SymphoniaDecoder.decode_to_wav16k(&input, &output).unwrap();
        assert_eq!(info.source_sample_rate, 48_000);
        assert_eq!(info.source_channels, 2);
        assert_eq!(info.duration_ms, 3_250);
        let samples = read_mono16k(&output);
        assert_eq!(samples.len(), 52_000);
        // Two half-amplitude tones averaged: RMS around 0.5 * 0.707 * 32767 / sqrt(2).
        let mid = &samples[8_000..44_000];
        let level = rms(mid);
        assert!((7_000.0..9_500.0).contains(&level), "rms {level}");
        // No start-up delay left: the signal starts within the first 2 ms.
        assert!(samples[..32].iter().any(|&s| s.abs() > 100));
    }

    #[test]
    fn mono_44k1_uses_the_async_resampler() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        let output = dir.path().join("out.wav");
        sine_wav(&input, 44_100, 1, 2.0);
        let info = SymphoniaDecoder.decode_to_wav16k(&input, &output).unwrap();
        assert_eq!(info.source_sample_rate, 44_100);
        assert_eq!(info.duration_ms, 2_000);
        let samples = read_mono16k(&output);
        assert_eq!(samples.len(), 32_000);
        let level = rms(&samples[4_000..28_000]);
        // 0.5 amplitude sine: RMS 0.354 * 32767 = 11585.
        assert!((10_500.0..12_500.0).contains(&level), "rms {level}");
    }

    #[test]
    fn already_16k_is_copied() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        let output = dir.path().join("out.wav");
        let frames = sine_wav(&input, 16_000, 1, 1.0);
        let info = SymphoniaDecoder.decode_to_wav16k(&input, &output).unwrap();
        assert_eq!(read_mono16k(&output).len() as u64, frames);
        assert_eq!(info.duration_ms, 1_000);
    }

    #[test]
    fn chain_falls_through_and_explains_ffmpeg() {
        struct Fails;
        impl Decoder for Fails {
            fn decode_to_wav16k(&self, _: &Path, output: &Path) -> Result<DecodedInfo> {
                std::fs::write(output, b"partial").unwrap();
                Err(decode_err("nope"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.xyz");
        std::fs::write(&input, b"not audio").unwrap();
        let output = dir.path().join("sub").join("out.wav");
        let chain = ChainDecoder::with(vec![
            ("fails", Box::new(Fails)),
            ("symphonia", Box::new(SymphoniaDecoder)),
        ]);
        let err = chain
            .decode_to_wav16k(&input, &output)
            .unwrap_err()
            .to_string();
        assert!(err.contains("fails: "), "{err}");
        assert!(err.contains("ffmpeg -nostdin -i"), "{err}");
        assert!(!output.exists(), "partial output removed");
        assert_eq!(chain.last_decoder(), None);

        let wav = dir.path().join("ok.wav");
        sine_wav(&wav, 8_000, 1, 0.5);
        chain.decode_to_wav16k(&wav, &output).unwrap();
        assert_eq!(chain.last_decoder(), Some("symphonia"));
        assert_eq!(read_mono16k(&output).len(), 8_000);
    }

    /// `aac_priming_1s.m4a`: 1 s of 440 Hz from ffmpeg's AAC encoder, whose
    /// edit list skips 1024 priming frames (48 kHz) and presents 48000.
    #[test]
    fn m4a_priming_is_trimmed() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("aac_priming_1s.m4a");
        let edit = super::super::mp4::audio_edit(&fixture).unwrap();
        assert_eq!(edit.skip_frames(48_000), 1_024);
        assert_eq!(edit.present_frames(48_000), Some(48_000));
        let dir = tempfile::tempdir().unwrap();
        let raw_out = dir.path().join("raw.wav");
        let out = dir.path().join("trimmed.wav");
        symphonia_to_wav16k(&fixture, &raw_out, false).unwrap();
        let info = SymphoniaDecoder.decode_to_wav16k(&fixture, &out).unwrap();
        assert_eq!(info.decoder, DecoderKind::Symphonia);
        let raw = read_mono16k(&raw_out);
        let trimmed = read_mono16k(&out);
        // The container's presented length (what ffprobe reports) ...
        assert_eq!(trimmed.len(), 16_000);
        assert_eq!(info.duration_ms, 1_000);
        // ... is the untrimmed decode minus the priming (1024 / 3 frames at 16 kHz).
        assert_eq!(raw.len() - trimmed.len(), 341);
        // And the trimmed audio is the untrimmed one shifted by the priming.
        let diff_rms = |offset: usize| {
            let n = 12_000;
            let e: f64 = (0..n)
                .map(|i| (f64::from(raw[i + offset]) - f64::from(trimmed[i])).powi(2))
                .sum();
            (e / n as f64).sqrt()
        };
        let level = rms(&trimmed[1_000..13_000]);
        assert!(
            diff_rms(341) < 0.2 * level,
            "aligned {} vs {level}",
            diff_rms(341)
        );
        assert!(
            diff_rms(0) > 0.5 * level,
            "unaligned {} vs {level}",
            diff_rms(0)
        );
    }

    #[test]
    fn missing_input_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = ChainDecoder::new()
            .decode_to_wav16k(&dir.path().join("nope.m4a"), &dir.path().join("o.wav"))
            .unwrap_err();
        assert!(err.to_string().contains("not a file"));
    }
}
