//! Audio helpers for playback: asset-protocol URLs, WAV header parsing and
//! waveform peaks. Peaks are computed here so the UI never fetches the file.
//!
//! Mock-only until core grows `audio::peaks` and a `views::Waveform`
//! (proposed in the Phase 2 report); only 16-bit PCM WAV is read for now.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;

/// Min/max-free peak envelope of a track, one row per channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waveform {
    pub channels: u16,
    pub sample_rate: u32,
    pub duration_ms: i64,
    /// `peaks[channel][bucket]`: the largest absolute sample in the bucket,
    /// scaled to 0..=255.
    pub peaks: Vec<Vec<u8>>,
}

/// What the RIFF header says about a PCM WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub data_offset: u64,
    pub data_len: u64,
}

impl WavInfo {
    pub fn frames(&self) -> u64 {
        let frame = u64::from(self.channels) * u64::from(self.bits_per_sample / 8);
        self.data_len.checked_div(frame).unwrap_or(0)
    }

    pub fn duration_ms(&self) -> i64 {
        (self.frames() * 1000)
            .checked_div(u64::from(self.sample_rate))
            .unwrap_or(0) as i64
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Reads the `fmt ` and `data` chunk positions of a WAV file.
pub fn wav_info(path: &Path) -> io::Result<WavInfo> {
    let mut f = BufReader::new(File::open(path)?);
    let mut riff = [0u8; 12];
    f.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(invalid("not a RIFF/WAVE file"));
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pos = 12u64;
    loop {
        let mut head = [0u8; 8];
        if f.read_exact(&mut head).is_err() {
            return Err(invalid("no data chunk"));
        }
        let len = u64::from(u32::from_le_bytes([head[4], head[5], head[6], head[7]]));
        pos += 8;
        match &head[0..4] {
            b"fmt " => {
                let mut body = [0u8; 16];
                f.read_exact(&mut body)?;
                fmt = Some((
                    u16::from_le_bytes([body[0], body[1]]),
                    u16::from_le_bytes([body[2], body[3]]),
                    u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                    u16::from_le_bytes([body[14], body[15]]),
                ));
                f.seek(SeekFrom::Start(pos + len + (len & 1)))?;
            }
            b"data" => {
                let (format, channels, sample_rate, bits_per_sample) =
                    fmt.ok_or_else(|| invalid("data chunk before fmt chunk"))?;
                // 1 = PCM, 0xFFFE = WAVE_FORMAT_EXTENSIBLE (PCM sub-format assumed).
                if format != 1 && format != 0xFFFE {
                    return Err(invalid("not PCM"));
                }
                if channels == 0 || sample_rate == 0 || bits_per_sample == 0 {
                    return Err(invalid("empty format"));
                }
                return Ok(WavInfo {
                    channels,
                    sample_rate,
                    bits_per_sample,
                    data_offset: pos,
                    data_len: len,
                });
            }
            _ => {
                f.seek(SeekFrom::Current((len + (len & 1)) as i64))?;
            }
        }
        pos += len + (len & 1);
    }
}

/// Peak envelope with `buckets` columns per channel (16-bit PCM only).
pub fn wav_peaks(path: &Path, buckets: usize) -> io::Result<Waveform> {
    let info = wav_info(path)?;
    if info.bits_per_sample != 16 {
        return Err(invalid("only 16-bit PCM WAV has a waveform for now"));
    }
    let buckets = buckets.clamp(16, 8192);
    let channels = usize::from(info.channels);
    let frames = info.frames().max(1);
    let mut peaks = vec![vec![0u16; buckets]; channels];

    let mut f = BufReader::with_capacity(1 << 18, File::open(path)?);
    f.seek(SeekFrom::Start(info.data_offset))?;
    let mut reader = f.take(info.data_len);
    let frame_bytes = channels * 2;
    let mut buf = vec![0u8; frame_bytes * 16_384];
    let mut frame_index = 0u64;
    let mut carry = 0usize;
    loop {
        let n = reader.read(&mut buf[carry..])?;
        if n == 0 {
            break;
        }
        let avail = carry + n;
        let whole = avail / frame_bytes * frame_bytes;
        for frame in buf[..whole].chunks_exact(frame_bytes) {
            let bucket = ((frame_index * buckets as u64) / frames) as usize;
            let bucket = bucket.min(buckets - 1);
            for (ch, sample) in frame.as_chunks::<2>().0.iter().enumerate() {
                let v = i16::from_le_bytes(*sample).unsigned_abs();
                let slot = &mut peaks[ch][bucket];
                if v > *slot {
                    *slot = v;
                }
            }
            frame_index += 1;
        }
        buf.copy_within(whole..avail, 0);
        carry = avail - whole;
    }
    Ok(Waveform {
        channels: info.channels,
        sample_rate: info.sample_rate,
        duration_ms: info.duration_ms(),
        peaks: peaks
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|v| ((u32::from(v) * 255 + 16_384) / 32_768).min(255) as u8)
                    .collect()
            })
            .collect(),
    })
}

/// Mock playback track: `seconds` of 16 kHz stereo PCM16 with 440 Hz bursts
/// on the left (mic) and 660 Hz bursts on the right (loopback), taking turns
/// like a conversation. Returns the file size.
pub fn write_demo_wav(path: &Path, seconds: u32) -> io::Result<u64> {
    const RATE: u32 = 16_000;
    let frames = RATE * seconds;
    let data_len = frames * 4;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("wav.part");
    {
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(b"RIFF")?;
        w.write_all(&(36 + data_len).to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&1u16.to_le_bytes())?; // PCM
        w.write_all(&2u16.to_le_bytes())?; // stereo
        w.write_all(&RATE.to_le_bytes())?;
        w.write_all(&(RATE * 4).to_le_bytes())?; // byte rate
        w.write_all(&4u16.to_le_bytes())?; // block align
        w.write_all(&16u16.to_le_bytes())?; // bits
        w.write_all(b"data")?;
        w.write_all(&data_len.to_le_bytes())?;
        let tau = std::f32::consts::TAU;
        for i in 0..frames {
            let t = i as f32 / RATE as f32;
            // 4 s turns: left speaks 0.2..1.8 s, right 2.2..3.8 s of each turn.
            let phase = t % 4.0;
            let syllables = 0.55 + 0.45 * (tau * 3.5 * t).sin().abs();
            let envelope = |from: f32, to: f32| {
                if phase < from || phase > to {
                    0.0
                } else {
                    ((phase - from).min(to - phase) / 0.02).min(1.0) * syllables
                }
            };
            let left = 0.32 * envelope(0.2, 1.8) * (tau * 440.0 * t).sin();
            let right = 0.32 * envelope(2.2, 3.8) * (tau * 660.0 * t).sin();
            for s in [left, right] {
                w.write_all(&((s * 32_767.0) as i16).to_le_bytes())?;
            }
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(u64::from(44 + data_len))
}

/// The asset-protocol URL for a local file, as `convertFileSrc` builds it.
pub fn asset_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut encoded = String::with_capacity(raw.len() * 3);
    for b in raw.bytes() {
        // encodeURIComponent leaves exactly these unescaped.
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    if cfg!(any(windows, target_os = "android")) {
        format!("http://asset.localhost/{encoded}")
    } else {
        format!("asset://localhost/{encoded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_url_matches_encode_uri_component() {
        let url = asset_url(Path::new("C:\\Users\\me\\a b(1).wav"));
        let tail = url.rsplit('/').next().unwrap();
        assert_eq!(tail, "C%3A%5CUsers%5Cme%5Ca%20b(1).wav");
    }

    #[test]
    fn demo_wav_round_trips_through_peaks() {
        let dir = std::env::temp_dir().join(format!("meet-assist-test-{}", std::process::id()));
        let path = dir.join("demo.wav");
        let size = write_demo_wav(&path, 4).unwrap();
        assert_eq!(size, 44 + 4 * 16_000 * 4);
        let info = wav_info(&path).unwrap();
        assert_eq!((info.channels, info.sample_rate), (2, 16_000));
        assert_eq!(info.duration_ms(), 4_000);
        let wf = wav_peaks(&path, 40).unwrap();
        assert_eq!(wf.peaks.len(), 2);
        assert_eq!(wf.peaks[0].len(), 40);
        // Left speaks in the first half of the turn, right in the second.
        assert!(wf.peaks[0][5] > 40 && wf.peaks[1][5] == 0);
        assert!(wf.peaks[1][30] > 40 && wf.peaks[0][30] == 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
