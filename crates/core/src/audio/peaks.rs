//! Waveform peaks for the player (`docs/01`, "Playback in UI": precomputed
//! in Rust, never by fetching the file into the page).
//!
//! [`peaks`] decodes the track with symphonia at its native rate and
//! channels (WAV, FLAC, M4A/AAC, MP3, Ogg; gapless like the import, so the
//! waveform lines up with the transcript) and falls back to a
//! [`Decoder`] (the chain: Media Foundation, ffmpeg) into a temporary 16 kHz
//! mono WAV for containers symphonia cannot read. [`peaks_cached`] keeps the
//! result in a JSON file keyed by the source's size, modification time and
//! the bucket count; [`cache_path`] says where (`<track>.peaks.json` next to
//! audio under the data root, the meeting folder for an import's original).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::decode::symphonia_stream;
use super::Decoder;
use crate::error::{CoreError, Result};
use crate::types::{AudioFormat, AudioTrack};
use crate::views::Waveform;

/// Bucket counts outside this range are clamped.
pub const MIN_BUCKETS: usize = 16;
pub const MAX_BUCKETS: usize = 8_192;
/// Resolution of the intermediate envelope: one maximum per 10 ms.
const BLOCKS_PER_SECOND: u32 = 100;
const CACHE_VERSION: u32 = 1;

/// Running per-block maxima of the absolute sample value.
struct Envelope {
    channels: usize,
    block: u64,
    frames: u64,
    /// `blocks[channel][block]` in 0.0..=1.0.
    blocks: Vec<Vec<f32>>,
}

impl Envelope {
    fn new(rate: u32, channels: usize) -> Self {
        Self {
            channels,
            block: u64::from((rate / BLOCKS_PER_SECOND).max(1)),
            frames: 0,
            blocks: vec![Vec::new(); channels],
        }
    }

    fn push(&mut self, samples: &[f32], source_channels: usize) {
        for frame in samples.chunks_exact(source_channels.max(1)) {
            let b = (self.frames / self.block) as usize;
            for (ch, row) in self.blocks.iter_mut().enumerate() {
                if row.len() <= b {
                    row.resize(b + 1, 0.0);
                }
                let v = if self.channels == source_channels {
                    frame[ch].abs()
                } else {
                    // Downmixed to one row: the loudest channel.
                    frame.iter().fold(0.0f32, |m, s| m.max(s.abs()))
                };
                if v > row[b] {
                    row[b] = v;
                }
            }
            self.frames += 1;
        }
    }

    fn into_waveform(self, rate: u32, buckets: usize) -> Waveform {
        let buckets = buckets.clamp(MIN_BUCKETS, MAX_BUCKETS);
        let peaks = self
            .blocks
            .iter()
            .map(|row| {
                let n = row.len();
                (0..buckets)
                    .map(|i| {
                        if n == 0 {
                            return 0;
                        }
                        let from = i * n / buckets;
                        let to = ((i + 1) * n / buckets).max(from + 1).min(n);
                        let peak = row[from.min(n - 1)..to]
                            .iter()
                            .fold(0.0f32, |m, v| m.max(*v));
                        (peak.min(1.0) * 255.0).round() as u8
                    })
                    .collect()
            })
            .collect();
        Waveform {
            channels: self.channels as u16,
            sample_rate: rate,
            duration_ms: (self.frames * 1_000 / u64::from(rate.max(1))) as i64,
            peaks,
        }
    }
}

fn decode_envelope(path: &Path, buckets: usize) -> Result<Waveform> {
    let mut env: Option<Envelope> = None;
    let info = symphonia_stream(path, true, &mut |rate, channels, samples| {
        let e = env.get_or_insert_with(|| {
            // More than two channels (video soundtracks) become one row.
            Envelope::new(rate, if channels <= 2 { channels } else { 1 })
        });
        e.push(samples, channels);
        Ok(())
    })?;
    let env = env.unwrap_or_else(|| Envelope::new(info.rate.max(1), info.channels.clamp(1, 2)));
    Ok(env.into_waveform(info.rate, buckets))
}

/// Peak envelope of `path` with `buckets` columns per channel (clamped to
/// 16..=8192): symphonia first, then `fallback` into a temporary 16 kHz WAV.
pub fn peaks_with(path: &Path, buckets: usize, fallback: &dyn Decoder) -> Result<Waveform> {
    if !path.is_file() {
        return Err(CoreError::Decode(format!(
            "{} is not a file",
            path.display()
        )));
    }
    match decode_envelope(path, buckets) {
        Ok(w) => Ok(w),
        Err(first) => {
            let tmp = tempfile_path(path);
            let result = fallback
                .decode_to_wav16k(path, &tmp)
                .and_then(|_| decode_envelope(&tmp, buckets));
            let _ = std::fs::remove_file(&tmp);
            result.map_err(|e| CoreError::Decode(format!("{first}; fallback: {e}")))
        }
    }
}

/// [`peaks_with`] the default decoder chain.
pub fn peaks(path: &Path, buckets: usize) -> Result<Waveform> {
    peaks_with(path, buckets, &super::ChainDecoder::new())
}

fn tempfile_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "diarist-peaks-{}-{}-{stem}.wav",
        std::process::id(),
        crate::types::new_id()
    ))
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    source_size: u64,
    source_mtime_ms: i64,
    buckets: usize,
    waveform: Waveform,
}

fn source_stamp(path: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((m.len(), mtime))
}

/// [`peaks_with`], reusing `cache` when it was computed from the same file
/// (size and modification time) with the same bucket count, and rewriting it
/// otherwise. A cache that cannot be written is not an error.
pub fn peaks_cached(
    path: &Path,
    buckets: usize,
    cache: &Path,
    fallback: &dyn Decoder,
) -> Result<Waveform> {
    let buckets = buckets.clamp(MIN_BUCKETS, MAX_BUCKETS);
    let stamp = source_stamp(path);
    if let (Some((size, mtime)), Ok(text)) = (stamp, std::fs::read_to_string(cache)) {
        if let Ok(c) = serde_json::from_str::<CacheFile>(&text) {
            if c.version == CACHE_VERSION
                && c.source_size == size
                && c.source_mtime_ms == mtime
                && c.buckets == buckets
            {
                return Ok(c.waveform);
            }
        }
    }
    let waveform = peaks_with(path, buckets, fallback)?;
    if let Some((source_size, source_mtime_ms)) = stamp {
        let file = CacheFile {
            version: CACHE_VERSION,
            source_size,
            source_mtime_ms,
            buckets,
            waveform: waveform.clone(),
        };
        if let Ok(json) = serde_json::to_string(&file) {
            let mut tmp = cache.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, cache).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    Ok(waveform)
}

/// Where the peaks of `track` are cached: `<track file>.peaks.json` next to
/// audio stored under the data root; for an import's `original` (the user's
/// file, which we never write next to) `audio/<uid>/original.peaks.json`.
/// `track_file` is the track's resolved absolute path.
pub fn cache_path(
    data_root: &Path,
    meeting_uid: &str,
    track: &AudioTrack,
    track_file: &Path,
) -> PathBuf {
    if track.format == AudioFormat::Original || !track_file.starts_with(data_root) {
        data_root
            .join("audio")
            .join(meeting_uid)
            .join(format!("track-{}.peaks.json", track.id))
    } else {
        let mut p = track_file.as_os_str().to_owned();
        p.push(".peaks.json");
        PathBuf::from(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ChainDecoder, DecodedInfo};
    use crate::types::{MeetingId, TrackChannel, TrackId};

    /// 16-bit WAV: left speaks the first half, right the second half.
    fn turns_wav(path: &Path, rate: u32, seconds: u32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let frames = rate * seconds;
        for i in 0..frames {
            let t = f64::from(i) / f64::from(rate);
            let v = ((t * 440.0 * std::f64::consts::TAU).sin() * 0.5 * 32_767.0) as i16;
            let first = i < frames / 2;
            w.write_sample(if first { v } else { 0 }).unwrap();
            w.write_sample(if first { 0 } else { v }).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn stereo_wav_peaks_by_channel() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        turns_wav(&wav, 16_000, 4);
        let w = peaks(&wav, 40).unwrap();
        assert_eq!(
            (w.channels, w.sample_rate, w.duration_ms),
            (2, 16_000, 4_000)
        );
        assert_eq!(w.peaks.len(), 2);
        assert_eq!(w.peaks[0].len(), 40);
        assert!(w.peaks[0][5] > 100 && w.peaks[1][5] == 0);
        assert!(w.peaks[1][30] > 100 && w.peaks[0][30] == 0);
        // Clamped bucket count.
        assert_eq!(peaks(&wav, 1).unwrap().peaks[0].len(), MIN_BUCKETS);
    }

    #[test]
    fn cache_is_reused_and_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        turns_wav(&wav, 8_000, 2);
        let cache = dir.path().join("t.wav.peaks.json");
        let chain = ChainDecoder::new();
        let a = peaks_cached(&wav, 100, &cache, &chain).unwrap();
        assert!(cache.is_file());
        // A tampered cache with matching keys is served as is (proves reuse).
        let mut c: CacheFile =
            serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
        c.waveform.duration_ms = 42;
        std::fs::write(&cache, serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(
            peaks_cached(&wav, 100, &cache, &chain).unwrap().duration_ms,
            42
        );
        // Another bucket count recomputes.
        let b = peaks_cached(&wav, 50, &cache, &chain).unwrap();
        assert_eq!(b.duration_ms, a.duration_ms);
        assert_eq!(b.peaks[0].len(), 50);
    }

    #[test]
    fn falls_back_to_the_decoder_for_unreadable_containers() {
        struct Tone;
        impl Decoder for Tone {
            fn decode_to_wav16k(&self, _: &Path, output: &Path) -> Result<DecodedInfo> {
                turns_wav_mono(output);
                Ok(DecodedInfo {
                    duration_ms: 1_000,
                    source_sample_rate: 16_000,
                    source_channels: 1,
                    decoder: crate::audio::DecoderKind::Other,
                })
            }
        }
        fn turns_wav_mono(path: &Path) {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(path, spec).unwrap();
            for i in 0..16_000 {
                w.write_sample(if i % 2 == 0 { 8_000i16 } else { -8_000 })
                    .unwrap();
            }
            w.finalize().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("clip.xyz");
        std::fs::write(&odd, b"not something symphonia reads").unwrap();
        let w = peaks_with(&odd, 20, &Tone).unwrap();
        assert_eq!((w.channels, w.duration_ms), (1, 1_000));
        assert!(w.peaks[0].iter().all(|&p| p > 50));
    }

    #[test]
    fn cache_paths() {
        let root = Path::new("D:/data");
        let mut track = AudioTrack {
            id: TrackId(7),
            meeting_id: MeetingId(1),
            channel: TrackChannel::Playback,
            path: "C:/rec/a.m4a".into(),
            format: AudioFormat::Original,
            sample_rate: 48_000,
            channels: 2,
            duration_ms: None,
            size_bytes: None,
        };
        assert_eq!(
            cache_path(root, "U1", &track, Path::new("C:/rec/a.m4a")),
            root.join("audio").join("U1").join("track-7.peaks.json")
        );
        track.format = AudioFormat::Flac;
        let file = root.join("audio").join("U1").join("playback.flac");
        assert_eq!(
            cache_path(root, "U1", &track, &file),
            root.join("audio")
                .join("U1")
                .join("playback.flac.peaks.json")
        );
    }
}
