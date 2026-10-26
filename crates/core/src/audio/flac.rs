//! The stereo playback track of a live meeting (`docs/01`, "Recording
//! storage"; research/06 B): left = mic, right = loopback, 16 kHz, 16-bit
//! FLAC encoded with the pure-Rust `flac-codec` crate. Lossless: the test
//! decodes it with symphonia and compares every sample.
//!
//! [`decode_stereo_to_wavs`] is the way back, for running a finished
//! meeting's post-pass again from its only copy of the audio (plan 16
//! section 12.5 J): the FLAC is opened for reading only, and a file that
//! does not decode in full and to its own checksum is refused.

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
    /// Sample frames of each input (`left`, `right`) before the shorter one
    /// was padded with silence; stored with the playback track's row
    /// (`audio_tracks.channel_frames`) so [`decode_stereo_to_wavs`] can give
    /// each channel its own length back.
    pub channel_frames: [u64; 2],
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
    let channel_frames = [u64::from(l.duration()), u64::from(r.duration())];
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
        channel_frames,
    })
}

/// What [`decode_stereo_to_wavs`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedWavs {
    /// Sample frames in the FLAC.
    pub frames: u64,
    /// Sample frames written to each WAV (`left`, `right`).
    pub channel_frames: [u64; 2],
    /// The decoded audio matched the MD5 the FLAC carries (`false`: it
    /// carries none).
    pub verified: bool,
}

/// Decodes a stereo 16 kHz 16-bit FLAC ([`encode_stereo`]'s output) into
/// two 16 kHz mono PCM16 WAVs, the left channel into `left`. With
/// `channel_frames` (stored with the playback track's row since beta.5)
/// each WAV gets its channel's own length back and the silence that padded
/// the shorter one is cut off; without (older recordings) both get the
/// FLAC's length. `progress(done, total)` in frames (`total` 0 when the
/// header gives none).
///
/// `flac` is opened for reading only and never written. Refused, with both
/// WAVs removed: a file that is not 16 kHz 16-bit stereo FLAC, a packet
/// that does not decode, fewer frames than the header says, audio that
/// fails the FLAC's MD5, `channel_frames` that do not fit (the longer one
/// is not the FLAC's length, or the cut-off padding is not silence), and a
/// WAV path that is the FLAC's own.
pub fn decode_stereo_to_wavs(
    flac: &Path,
    left: &Path,
    right: &Path,
    channel_frames: Option<[u64; 2]>,
    progress: &dyn Fn(u64, u64),
) -> Result<DecodedWavs> {
    if left == flac || right == flac || left == right {
        return Err(audio_err(format!(
            "decode {}: the WAVs must be other files",
            flac.display()
        )));
    }
    let r = decode_into(flac, left, right, channel_frames, progress);
    if r.is_err() {
        let _ = std::fs::remove_file(left);
        let _ = std::fs::remove_file(right);
    }
    r
}

fn decode_into(
    flac: &Path,
    left: &Path,
    right: &Path,
    channel_frames: Option<[u64; 2]>,
    progress: &dyn Fn(u64, u64),
) -> Result<DecodedWavs> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_FLAC};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let name = flac.display();
    let err = |e: SymError| audio_err(format!("decode {name}: {e}"));
    let file = std::fs::File::open(flac)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(err)?
        .format;
    let track = format
        .default_track()
        .ok_or_else(|| audio_err(format!("decode {name}: no audio track")))?
        .clone();
    let p = &track.codec_params;
    let channels = p.channels.map_or(0, |c| c.count());
    if p.codec != CODEC_TYPE_FLAC
        || p.sample_rate != Some(CAPTURE_RATE)
        || p.bits_per_sample != Some(16)
        || channels != 2
    {
        return Err(audio_err(format!(
            "decode {name}: not 16 kHz 16-bit stereo FLAC ({:?} Hz, {:?} bits, {channels} channels)",
            p.sample_rate, p.bits_per_sample
        )));
    }
    let total = p.n_frames;
    if let (Some(n), Some([a, b])) = (total, channel_frames) {
        if a.max(b) != n {
            return Err(audio_err(format!(
                "decode {name}: the stored channel lengths ({a}, {b}) do not fit its {n} frames"
            )));
        }
    }
    let mut dec = symphonia::default::get_codecs()
        .make(p, &DecoderOptions { verify: true })
        .map_err(err)?;
    let create = |path: &Path| {
        hound::WavWriter::create(path, crate::audio::wav::spec())
            .map_err(|e| audio_err(format!("create {}: {e}", path.display())))
    };
    let (mut lw, mut rw) = (create(left)?, create(right)?);
    let keep = channel_frames.unwrap_or([u64::MAX; 2]);
    let mut written = [0u64; 2];
    let mut done = 0u64;
    let mut sb: Option<SampleBuffer<i16>> = None;
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
        // `capacity` in frames here, in samples (both channels) there.
        let frames = buf.capacity();
        if sb.as_ref().is_none_or(|s| s.capacity() < 2 * frames) {
            sb = Some(SampleBuffer::<i16>::new(frames as u64, *buf.spec()));
        }
        let s = sb.as_mut().expect("sample buffer");
        s.copy_interleaved_ref(buf);
        let samples = s.samples();
        let n = (samples.len() / 2) as u64;
        for (c, w) in [&mut lw, &mut rw].into_iter().enumerate() {
            // Frames `done..done + n` of this channel: those before its own
            // length go to its WAV, the rest must be the encoder's padding.
            let take = keep[c].saturating_sub(done).min(n) as usize;
            if samples
                .iter()
                .skip(c)
                .step_by(2)
                .skip(take)
                .any(|&v| v != 0)
            {
                return Err(audio_err(format!(
                    "decode {name}: the stored length of channel {c} ({}) cuts off audio",
                    keep[c]
                )));
            }
            let mut sw = w.get_i16_writer(take as u32);
            for &v in samples.iter().skip(c).step_by(2).take(take) {
                sw.write_sample(v);
            }
            sw.flush()
                .map_err(|e| audio_err(format!("write the WAVs of {name}: {e}")))?;
            written[c] += take as u64;
        }
        done += n;
        progress(done, total.unwrap_or(0));
    }
    if let Some(n) = total {
        if done != n {
            return Err(audio_err(format!(
                "decode {name}: {done} of its {n} frames decoded"
            )));
        }
    }
    let verify = dec.finalize().verify_ok;
    if verify == Some(false) {
        return Err(audio_err(format!(
            "decode {name}: the decoded audio does not match the file's MD5"
        )));
    }
    if let Some([a, b]) = channel_frames {
        if written != [a, b] {
            return Err(audio_err(format!(
                "decode {name}: the stored channel lengths ({a}, {b}) do not fit its {done} frames"
            )));
        }
    }
    for (w, path) in [(lw, left), (rw, right)] {
        w.finalize()
            .map_err(|e| audio_err(format!("finalize {}: {e}", path.display())))?;
    }
    Ok(DecodedWavs {
        frames: done,
        channel_frames: written,
        verified: verify == Some(true),
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
        assert_eq!(info.channel_frames, [48_000, 43_200]);
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

    fn sha256(p: &Path) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        Sha256::digest(std::fs::read(p).unwrap()).to_vec()
    }

    fn wav_samples(p: &Path) -> Vec<i16> {
        let mut r = hound::WavReader::open(p).unwrap();
        assert_eq!(r.spec(), crate::audio::wav::spec());
        r.samples::<i16>().map(|s| s.unwrap()).collect()
    }

    /// Two channels of `n` and `m` frames: noise with a chirp, and a square
    /// wave with a stretch of digital silence.
    fn two_wavs(dir: &Path, n: usize, m: usize) -> (Vec<i16>, Vec<i16>) {
        let mut seed = 11u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 4_001) as i32 - 2_000
        };
        let left: Vec<i16> = (0..n)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                let v = (t * (200.0 + 300.0 * t) * std::f32::consts::TAU).sin() * 12_000.0;
                (v as i32 + noise()).clamp(-32_768, 32_767) as i16
            })
            .collect();
        let right: Vec<i16> = (0..m)
            .map(|i| match i {
                8_000..12_000 => 0,
                _ if (i / 25) % 2 == 0 => 9_000,
                _ => -9_000,
            })
            .collect();
        for (p, s) in [("mic.wav", &left), ("loopback.wav", &right)] {
            let mut w = WavSink::create(&dir.join(p)).unwrap();
            w.write(s).unwrap();
            w.finalize().unwrap();
        }
        (left, right)
    }

    #[test]
    fn the_flac_decodes_back_into_its_two_wavs_and_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (left, right) = two_wavs(d, 40_000, 37_123);
        let flac = d.join("playback.flac");
        let info = encode_stereo(&d.join("mic.wav"), &d.join("loopback.wav"), &flac).unwrap();
        assert_eq!(info.channel_frames, [40_000, 37_123]);
        let before = sha256(&flac);
        let modified = std::fs::metadata(&flac).unwrap().modified().unwrap();
        let out = d.join("out");
        std::fs::create_dir(&out).unwrap();
        let (l, r) = (out.join("mic.wav"), out.join("loopback.wav"));
        // With the stored lengths: the WAVs as they were.
        let seen = std::sync::Mutex::new(Vec::new());
        let got = decode_stereo_to_wavs(&flac, &l, &r, Some(info.channel_frames), &|d, t| {
            seen.lock().unwrap().push((d, t))
        })
        .unwrap();
        assert_eq!(
            got,
            DecodedWavs {
                frames: 40_000,
                channel_frames: [40_000, 37_123],
                verified: true,
            }
        );
        assert_eq!(wav_samples(&l), left);
        assert_eq!(wav_samples(&r), right);
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.last(), Some(&(40_000, 40_000)));
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
        // Without them (a recording from before beta.5): both at the FLAC's
        // length, the shorter one padded with silence as it was encoded.
        let got = decode_stereo_to_wavs(&flac, &l, &r, None, &|_, _| {}).unwrap();
        assert_eq!(got.channel_frames, [40_000, 40_000]);
        assert_eq!(wav_samples(&l), left);
        let padded = wav_samples(&r);
        assert_eq!(padded[..37_123], right[..]);
        assert!(padded[37_123..].iter().all(|&v| v == 0));
        // Encoding the decoded WAVs again gives the same file.
        let again = out.join("again.flac");
        let got =
            decode_stereo_to_wavs(&flac, &l, &r, Some(info.channel_frames), &|_, _| {}).unwrap();
        assert!(got.verified);
        encode_stereo(&l, &r, &again).unwrap();
        assert_eq!(sha256(&again), before);
        // The FLAC itself was only read.
        assert_eq!(sha256(&flac), before);
        assert_eq!(
            std::fs::metadata(&flac).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn a_flac_that_does_not_decode_in_full_is_refused_and_leaves_no_wavs() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        two_wavs(d, 40_000, 37_123);
        let flac = d.join("playback.flac");
        let info = encode_stereo(&d.join("mic.wav"), &d.join("loopback.wav"), &flac).unwrap();
        let bytes = std::fs::read(&flac).unwrap();
        let (l, r) = (d.join("l.wav"), d.join("r.wav"));
        let refused = |path: &Path, frames: Option<[u64; 2]>| -> String {
            let before = sha256(path);
            let e = decode_stereo_to_wavs(path, &l, &r, frames, &|_, _| {})
                .err()
                .expect("refused")
                .to_string();
            assert!(!l.exists() && !r.exists(), "{e}: a WAV was left");
            assert_eq!(sha256(path), before, "{e}: the FLAC changed");
            e
        };
        // Cut short (a copy that stopped half way).
        let short = d.join("short.flac");
        std::fs::write(&short, &bytes[..bytes.len() / 2]).unwrap();
        let e = refused(&short, Some(info.channel_frames));
        assert!(e.contains("frames decoded") || e.contains("decode"), "{e}");
        // One byte of the audio changed: a frame's CRC or the MD5 sees it.
        let flipped = d.join("flipped.flac");
        let mut b = bytes.clone();
        let at = b.len() * 2 / 3;
        b[at] ^= 0x10;
        std::fs::write(&flipped, &b).unwrap();
        refused(&flipped, None);
        // Stored lengths that do not belong to this file.
        let e = refused(&flac, Some([40_000, 30_000]));
        assert!(e.contains("cuts off audio"), "{e}");
        let e = refused(&flac, Some([39_000, 37_123]));
        assert!(e.contains("do not fit"), "{e}");
        // Not the playback track's format: 8 kHz.
        let other = d.join("other.flac");
        let mut w =
            FlacSampleWriter::create(&other, Options::default(), 8_000, 16, 2, None).unwrap();
        w.write(&[0i32; 1_600]).unwrap();
        w.finalize().unwrap();
        let e = refused(&other, None);
        assert!(e.contains("not 16 kHz 16-bit stereo"), "{e}");
        // A WAV path that is the FLAC itself: nothing is opened for writing.
        let e = decode_stereo_to_wavs(&flac, &flac, &r, None, &|_, _| {})
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("must be other files"), "{e}");
        assert_eq!(std::fs::read(&flac).unwrap(), bytes);
    }
}
