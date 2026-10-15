//! The stereo playback track of a live meeting (`docs/01`, "Recording
//! storage"; research/06 B): left = mic, right = loopback, 16 kHz, 16-bit
//! FLAC encoded with the pure-Rust `flac-codec` crate. Lossless: the test
//! decodes it with symphonia and compares every sample.

use std::io::BufReader;
use std::path::Path;

use flac_codec::encode::{FlacSampleWriter, Options};

use crate::audio::capture::{audio_err, CAPTURE_RATE};
use crate::error::Result;

/// What [`encode_stereo`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlacInfo {
    /// Sample frames (per channel).
    pub frames: u64,
    pub bytes: u64,
}

/// Encodes two 16 kHz mono PCM16 WAVs into one stereo FLAC (`left` on the
/// left channel). The shorter input is padded with silence.
pub fn encode_stereo(left: &Path, right: &Path, out: &Path) -> Result<FlacInfo> {
    let open = |p: &Path| -> Result<hound::WavReader<BufReader<std::fs::File>>> {
        let r = hound::WavReader::open(p)
            .map_err(|e| audio_err(format!("open {}: {e}", p.display())))?;
        let s = r.spec();
        if s.channels != 1 || s.bits_per_sample != 16 || s.sample_rate != CAPTURE_RATE {
            return Err(audio_err(format!(
                "{} is not 16 kHz mono PCM16 ({s:?})",
                p.display()
            )));
        }
        Ok(r)
    };
    let mut l = open(left)?;
    let mut r = open(right)?;
    let frames = u64::from(l.duration().max(r.duration()));
    let mut w = FlacSampleWriter::create(
        out,
        Options::default().overwrite(),
        CAPTURE_RATE,
        16,
        2,
        None,
    )
    .map_err(|e| audio_err(format!("create {}: {e}", out.display())))?;
    let mut ls = l.samples::<i16>();
    let mut rs = r.samples::<i16>();
    const CHUNK: usize = 16_384;
    let mut buf: Vec<i32> = Vec::with_capacity(CHUNK * 2);
    let mut done = 0u64;
    while done < frames {
        buf.clear();
        let n = ((frames - done) as usize).min(CHUNK);
        for _ in 0..n {
            let a = ls
                .next()
                .transpose()
                .map_err(|e| audio_err(format!("read: {e}")))?;
            let b = rs
                .next()
                .transpose()
                .map_err(|e| audio_err(format!("read: {e}")))?;
            buf.push(i32::from(a.unwrap_or(0)));
            buf.push(i32::from(b.unwrap_or(0)));
        }
        w.write(&buf)
            .map_err(|e| audio_err(format!("encode {}: {e}", out.display())))?;
        done += n as u64;
    }
    w.finalize()
        .map_err(|e| audio_err(format!("finalize {}: {e}", out.display())))?;
    Ok(FlacInfo {
        frames,
        bytes: std::fs::metadata(out)?.len(),
    })
}

/// Decodes a FLAC (or any symphonia-readable file) to interleaved i16 and
/// its channel count. Used to verify the playback track.
pub fn decode_i16(path: &Path) -> Result<(Vec<i16>, usize)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let err = |e: SymError| audio_err(format!("decode {}: {e}", path.display()));
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(err)?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| audio_err("no audio track"))?
        .clone();
    let channels = track.codec_params.channels.map_or(1, |c| c.count());
    let mut dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(err)?;
    let mut out = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(err(e)),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let buf = dec.decode(&packet).map_err(err)?;
        let mut sb = SampleBuffer::<i16>::new(buf.capacity() as u64, *buf.spec());
        sb.copy_interleaved_ref(buf);
        out.extend_from_slice(sb.samples());
    }
    Ok((out, channels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::wav::WavSink;

    #[test]
    fn stereo_flac_decodes_bit_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let (mic, lb, out) = (
            dir.path().join("mic.wav"),
            dir.path().join("loopback.wav"),
            dir.path().join("playback.flac"),
        );
        // Speech-like content: a chirp with noise on the left, a gap of
        // digital silence and a square wave on the right; the right channel
        // is 0.3 s shorter.
        let mut seed = 7u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 2_001) as i32 - 1_000
        };
        let left: Vec<i16> = (0..48_000)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                let v = (t * (300.0 + 400.0 * t) * std::f32::consts::TAU).sin() * 9_000.0;
                (v as i32 + noise()).clamp(-32_768, 32_767) as i16
            })
            .collect();
        let right: Vec<i16> = (0..43_200)
            .map(|i| match i {
                16_000..24_000 => 0,
                _ if (i / 40) % 2 == 0 => 12_000,
                _ => -12_000,
            })
            .collect();
        for (p, s) in [(&mic, &left), (&lb, &right)] {
            let mut w = WavSink::create(p).unwrap();
            w.write(s).unwrap();
            w.finalize().unwrap();
        }
        let info = encode_stereo(&mic, &lb, &out).unwrap();
        assert_eq!(info.frames, 48_000);
        let wav_bytes = std::fs::metadata(&mic).unwrap().len() * 2;
        assert!(info.bytes < wav_bytes, "{} vs {wav_bytes}", info.bytes);
        let (decoded, channels) = decode_i16(&out).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(decoded.len(), 96_000);
        for i in 0..48_000 {
            assert_eq!(decoded[2 * i], left[i], "left sample {i}");
            let r = right.get(i).copied().unwrap_or(0);
            assert_eq!(decoded[2 * i + 1], r, "right sample {i}");
        }
    }
}
