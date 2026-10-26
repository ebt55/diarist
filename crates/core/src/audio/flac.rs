//! The stereo playback track of a live meeting (`docs/01`, "Recording
//! storage"; research/06 B): left = mic, right = loopback, 16 kHz, 16-bit
//! FLAC encoded with the pure-Rust `flac-codec` crate. Lossless: the test
//! decodes it with symphonia and compares every sample.
//!
//! [`decode_stereo_to_wavs`] is the way back, for running a finished
//! meeting's post-pass again from its only copy of the audio (plan 16
//! section 12.5 J): the FLAC is opened for reading only, a file that does
//! not decode in full and to its own checksum is refused, and the WAVs are
//! written as `.part` files first and never over WAVs that are there.
//! [`check_copies`] tells whether WAVs beside a FLAC are copies of what it
//! holds, before anyone deletes them.

use std::io::BufReader;
use std::path::{Path, PathBuf};

use flac_codec::encode::{FlacSampleWriter, Options};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::Decoder;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatReader;

use crate::audio::capture::{audio_err, CAPTURE_RATE};
use crate::error::Result;

/// Makes a written file durable: its data is flushed to the disk, then its
/// folder's entries where the platform allows it (best effort: Windows
/// opens a folder only with backup semantics and some file systems refuse
/// the flush; the file's own flush is what counts).
pub(crate) fn sync_file_and_dir(path: &Path) -> Result<()> {
    // FlushFileBuffers needs a handle with write access; nothing is written.
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    sync_dir(path.parent());
    Ok(())
}

/// Best effort: flushes a folder's entries (a rename or a new file in it).
pub(crate) fn sync_dir(dir: Option<&Path>) {
    let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) else {
        return;
    };
    #[cfg(windows)]
    let f = {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS: the only way to open a folder.
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(0x0200_0000)
            .open(dir)
    };
    #[cfg(not(windows))]
    let f = std::fs::File::open(dir);
    if let Ok(f) = f {
        let _ = f.sync_all();
    }
}

/// The file a WAV is decoded into before it is complete and checked:
/// `mic.wav` → `mic.wav.part`.
pub fn part_path(wav: &Path) -> PathBuf {
    let mut s = wav.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

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
    // On the disk before the caller registers it as the meeting's audio
    // and deletes the WAVs it came from.
    sync_file_and_dir(out)
        .map_err(|e| audio_err(format!("flush {} to the disk: {e}", out.display())))?;
    Ok(FlacInfo {
        frames,
        bytes: std::fs::metadata(out)?.len(),
        channel_frames,
    })
}

/// A stereo 16 kHz 16-bit FLAC read front to back, every frame's CRC and
/// the file's MD5 checked ([`decode_stereo_to_wavs`], [`check_copies`]).
/// The file is opened for reading only.
struct StereoReader {
    name: String,
    format: Box<dyn FormatReader>,
    dec: Box<dyn Decoder>,
    track: u32,
    /// Frames the header gives.
    total: Option<u64>,
    /// Frames read so far.
    done: u64,
    sb: Option<SampleBuffer<i16>>,
}

impl StereoReader {
    fn open(flac: &Path) -> Result<Self> {
        use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_FLAC};
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let name = flac.display().to_string();
        let err = |e: SymError| audio_err(format!("decode {name}: {e}"));
        let file = std::fs::File::open(flac)
            .map_err(|e| audio_err(format!("open {name} for reading: {e}")))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let format = symphonia::default::get_probe()
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
        let dec = symphonia::default::get_codecs()
            .make(p, &DecoderOptions { verify: true })
            .map_err(err)?;
        Ok(Self {
            total: p.n_frames,
            track: track.id,
            name,
            format,
            dec,
            done: 0,
            sb: None,
        })
    }

    /// The next packet's frames, interleaved (left, right), with the frame
    /// they start at; `None` at the end of the file.
    fn next_block(&mut self) -> Result<Option<(u64, &[i16])>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(e) => return Err(audio_err(format!("decode {}: {e}", self.name))),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let buf = self
                .dec
                .decode(&packet)
                .map_err(|e| audio_err(format!("decode {}: {e}", self.name)))?;
            // `capacity` in frames here, in samples (both channels) there.
            let frames = buf.capacity();
            if self.sb.as_ref().is_none_or(|s| s.capacity() < 2 * frames) {
                self.sb = Some(SampleBuffer::<i16>::new(frames as u64, *buf.spec()));
            }
            let s = self.sb.as_mut().expect("sample buffer");
            s.copy_interleaved_ref(buf);
            let at = self.done;
            self.done += (s.samples().len() / 2) as u64;
            return Ok(Some((at, s.samples())));
        }
    }

    /// After the last packet: every frame the header names was read, and
    /// the audio matches the file's MD5. `true` when it carries one.
    fn finish(mut self) -> Result<bool> {
        if let Some(n) = self.total {
            if self.done != n {
                return Err(audio_err(format!(
                    "decode {}: {} of its {n} frames decoded",
                    self.name, self.done
                )));
            }
        }
        let verify = self.dec.finalize().verify_ok;
        if verify == Some(false) {
            return Err(audio_err(format!(
                "decode {}: the decoded audio does not match the file's MD5",
                self.name
            )));
        }
        Ok(verify == Some(true))
    }
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
/// `flac` is opened for reading only and never written. The audio goes to
/// `<left>.part` and `<right>.part` ([`part_path`]), which become `left`
/// and `right` only once the whole file decoded and checked out; a WAV
/// that is already at `left` or `right` is never written over, truncated
/// or deleted (the decode is refused). Refused, with the `.part` files
/// removed: a WAV already there, a file that is not 16 kHz 16-bit stereo
/// FLAC, a packet that does not decode, fewer frames than the header says,
/// audio that fails the FLAC's MD5, `channel_frames` that do not fit (the
/// longer one is not the FLAC's length, or the cut-off padding is not
/// silence), and a WAV path that is the FLAC's own.
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
    for p in [left, right] {
        vacant(p).map_err(|e| audio_err(format!("decode {}: {e}", flac.display())))?;
    }
    let (lp, rp) = (part_path(left), part_path(right));
    let r = decode_into(flac, &lp, &rp, channel_frames, progress).and_then(|got| {
        place(&lp, left)?;
        if let Err(e) = place(&rp, right) {
            // `left` is this call's own file.
            let _ = std::fs::remove_file(left);
            return Err(e);
        }
        Ok(got)
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&lp);
        let _ = std::fs::remove_file(&rp);
    }
    r
}

/// `Ok` when nothing is at `p` (only `NotFound` counts as nothing).
fn vacant(p: &Path) -> std::result::Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "{} is already there; it is left as it is",
            p.display()
        )),
        Err(e) => Err(format!("cannot check {}: {e}", p.display())),
    }
}

/// A finished `.part` file becomes its WAV, never over a file that is there.
fn place(part: &Path, dest: &Path) -> Result<()> {
    vacant(dest).map_err(audio_err)?;
    std::fs::rename(part, dest).map_err(|e| {
        audio_err(format!(
            "rename {} to {}: {e}",
            part.display(),
            dest.display()
        ))
    })
}

fn decode_into(
    flac: &Path,
    left: &Path,
    right: &Path,
    channel_frames: Option<[u64; 2]>,
    progress: &dyn Fn(u64, u64),
) -> Result<DecodedWavs> {
    let name = flac.display();
    let mut reader = StereoReader::open(flac)?;
    let total = reader.total;
    if let (Some(n), Some([a, b])) = (total, channel_frames) {
        if a.max(b) != n {
            return Err(audio_err(format!(
                "decode {name}: the stored channel lengths ({a}, {b}) do not fit its {n} frames"
            )));
        }
    }
    let create = |path: &Path| {
        hound::WavWriter::create(path, crate::audio::wav::spec())
            .map_err(|e| audio_err(format!("create {}: {e}", path.display())))
    };
    let (mut lw, mut rw) = (create(left)?, create(right)?);
    let keep = channel_frames.unwrap_or([u64::MAX; 2]);
    let mut written = [0u64; 2];
    while let Some((done, samples)) = reader.next_block()? {
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
        progress(done + n, total.unwrap_or(0));
    }
    let done = reader.done;
    let verified = reader.finish()?;
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
        verified,
    })
}

/// Whether the WAVs beside a playback FLAC (`[left, right]`, either may be
/// absent) are copies of what it holds, before anyone deletes them: the
/// FLAC decodes in full, to its MD5 when it carries one, and each WAV given
/// is 16 kHz mono PCM16 holding its channel's samples, the rest of the
/// channel silence (the encoder's padding of the shorter input). Both are
/// only read. The error says where they part.
pub fn check_copies<P: AsRef<Path>>(flac: &Path, wavs: [Option<P>; 2]) -> Result<()> {
    let name = flac.display().to_string();
    let mut readers = Vec::new();
    for (c, p) in wavs.iter().enumerate() {
        let Some(p): Option<&Path> = p.as_ref().map(|p| p.as_ref()) else {
            continue;
        };
        let r = hound::WavReader::open(p)
            .map_err(|e| audio_err(format!("open {}: {e}", p.display())))?;
        if r.spec() != crate::audio::wav::spec() {
            return Err(audio_err(format!(
                "{} is not 16 kHz mono PCM16 ({:?})",
                p.display(),
                r.spec()
            )));
        }
        readers.push((c, p, r.into_samples::<i16>()));
    }
    let mut reader = StereoReader::open(flac)?;
    while let Some((done, samples)) = reader.next_block()? {
        for (c, p, wav) in readers.iter_mut() {
            for (i, &v) in samples.iter().skip(*c).step_by(2).enumerate() {
                let w = wav
                    .next()
                    .transpose()
                    .map_err(|e| audio_err(format!("read {}: {e}", p.display())))?;
                if w.unwrap_or(0) != v {
                    return Err(audio_err(format!(
                        "{} differs from channel {c} of {name} at frame {}",
                        p.display(),
                        done + i as u64
                    )));
                }
            }
        }
    }
    let frames = reader.done;
    reader.finish()?;
    for (_, p, wav) in readers.iter_mut() {
        if wav.next().is_some() {
            return Err(audio_err(format!(
                "{} is longer than the {frames} frames of {name}",
                p.display()
            )));
        }
    }
    Ok(())
}

/// Decodes a FLAC (or any symphonia-readable file) to interleaved i16 and
/// its channel count. Used to verify the playback track.
pub fn decode_i16(path: &Path) -> Result<(Vec<i16>, usize)> {
    use symphonia::core::codecs::DecoderOptions;
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
        assert!(!part_path(&l).exists() && !part_path(&r).exists());
        // They are copies of what the FLAC holds; the original WAVs too.
        check_copies(&flac, [Some(&l), Some(&r)]).unwrap();
        check_copies(
            &flac,
            [Some(&d.join("mic.wav")), Some(&d.join("loopback.wav"))],
        )
        .unwrap();
        check_copies(&flac, [None, Some(&r)]).unwrap();
        // Without them (a recording from before beta.5): both at the FLAC's
        // length, the shorter one padded with silence as it was encoded.
        std::fs::remove_file(&l).unwrap();
        std::fs::remove_file(&r).unwrap();
        let got = decode_stereo_to_wavs(&flac, &l, &r, None, &|_, _| {}).unwrap();
        assert_eq!(got.channel_frames, [40_000, 40_000]);
        assert_eq!(wav_samples(&l), left);
        let padded = wav_samples(&r);
        assert_eq!(padded[..37_123], right[..]);
        assert!(padded[37_123..].iter().all(|&v| v == 0));
        // The padded copy is a copy too.
        check_copies(&flac, [Some(&l), Some(&r)]).unwrap();
        // Encoding the decoded WAVs again gives the same file.
        std::fs::remove_file(&l).unwrap();
        std::fs::remove_file(&r).unwrap();
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
                .expect_err("refused")
                .to_string();
            assert!(!l.exists() && !r.exists(), "{e}: a WAV was left");
            assert!(
                !part_path(&l).exists() && !part_path(&r).exists(),
                "{e}: a .part file was left"
            );
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
        // A damaged FLAC does not pass for the WAVs it was made from.
        let e = check_copies(
            &flipped,
            [Some(&d.join("mic.wav")), Some(&d.join("loopback.wav"))],
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("decode") || e.contains("differs"), "{e}");
    }

    #[test]
    fn a_decode_never_writes_over_wavs_that_are_there() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        two_wavs(d, 40_000, 37_123);
        let flac = d.join("playback.flac");
        let info = encode_stereo(&d.join("mic.wav"), &d.join("loopback.wav"), &flac).unwrap();
        let (mic, lb) = (d.join("mic.wav"), d.join("loopback.wav"));
        let (m0, l0) = (sha256(&mic), sha256(&lb));
        // Both there, or only one: refused, both as they were, no .part left.
        for only in [None, Some(&mic), Some(&lb)] {
            if let Some(p) = only {
                let keep = d.join("keep.wav");
                std::fs::rename(p, &keep).unwrap();
                let e =
                    decode_stereo_to_wavs(&flac, &mic, &lb, Some(info.channel_frames), &|_, _| {})
                        .unwrap_err()
                        .to_string();
                assert!(e.contains("already there"), "{e}");
                // The decode did not make the one that was missing either.
                assert!(!p.exists());
                std::fs::rename(&keep, p).unwrap();
            } else {
                let e = decode_stereo_to_wavs(&flac, &mic, &lb, None, &|_, _| {})
                    .unwrap_err()
                    .to_string();
                assert!(e.contains("already there"), "{e}");
            }
            assert_eq!((sha256(&mic), sha256(&lb)), (m0.clone(), l0.clone()));
            assert!(!part_path(&mic).exists() && !part_path(&lb).exists());
        }
        // Not copies: another WAV, a shorter one, a longer one.
        let other = d.join("other.wav");
        let mut w = WavSink::create(&other).unwrap();
        w.write(&vec![5i16; 40_000]).unwrap();
        w.finalize().unwrap();
        let e = check_copies(&flac, [Some(&other), None])
            .unwrap_err()
            .to_string();
        assert!(e.contains("differs from channel 0"), "{e}");
        let short = d.join("short.wav");
        let mut w = WavSink::create(&short).unwrap();
        w.write(&wav_samples(&mic)[..30_000]).unwrap();
        w.finalize().unwrap();
        let e = check_copies(&flac, [Some(&short), None])
            .unwrap_err()
            .to_string();
        assert!(e.contains("differs from channel 0 of"), "{e}");
        let long = d.join("long.wav");
        let mut w = WavSink::create(&long).unwrap();
        let mut s = wav_samples(&mic);
        s.push(1);
        w.write(&s).unwrap();
        w.finalize().unwrap();
        let e = check_copies(&flac, [Some(&long), None])
            .unwrap_err()
            .to_string();
        assert!(e.contains("is longer than"), "{e}");
    }
}
