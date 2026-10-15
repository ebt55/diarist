//! Per-channel recording WAVs (`docs/01`, "Recording storage"): 16 kHz mono
//! PCM16 written by the pipeline thread with `hound`, flushed (data plus
//! header sizes) every second so a crash loses at most a second, and
//! repaired from the file length at the next launch ([`repair_header`]).

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::audio::capture::{audio_err, CAPTURE_RATE};
use crate::error::Result;

/// Samples between flushes (one second).
pub const FLUSH_EVERY: u64 = CAPTURE_RATE as u64;

/// The 16 kHz mono PCM16 spec of every recording WAV.
pub fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: CAPTURE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

/// An open recording WAV.
pub struct WavSink {
    writer: hound::WavWriter<BufWriter<File>>,
    path: PathBuf,
    written: u64,
    since_flush: u64,
}

impl WavSink {
    /// Creates (truncates) `path`.
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let writer = hound::WavWriter::create(path, spec())
            .map_err(|e| audio_err(format!("create {}: {e}", path.display())))?;
        Ok(Self {
            writer,
            path: path.to_path_buf(),
            written: 0,
            since_flush: 0,
        })
    }

    /// Appends samples; flushes every [`FLUSH_EVERY`] samples.
    pub fn write(&mut self, pcm: &[i16]) -> Result<()> {
        let mut w = self.writer.get_i16_writer(pcm.len() as u32);
        for &s in pcm {
            w.write_sample(s);
        }
        w.flush()
            .map_err(|e| audio_err(format!("write {}: {e}", self.path.display())))?;
        self.written += pcm.len() as u64;
        self.since_flush += pcm.len() as u64;
        if self.since_flush >= FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes buffered data and updates the header sizes.
    pub fn flush(&mut self) -> Result<()> {
        self.since_flush = 0;
        self.writer
            .flush()
            .map_err(|e| audio_err(format!("flush {}: {e}", self.path.display())))
    }

    /// Appends zeros until the file holds `len` samples.
    pub fn pad_to(&mut self, len: u64) -> Result<()> {
        const CHUNK: usize = 16_000;
        let zeros = [0i16; CHUNK];
        while self.written < len {
            let n = ((len - self.written) as usize).min(CHUNK);
            self.write(&zeros[..n])?;
        }
        Ok(())
    }

    /// Samples written so far.
    pub fn samples(&self) -> u64 {
        self.written
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Finalizes the header; returns the sample count.
    pub fn finalize(self) -> Result<u64> {
        let n = self.written;
        let path = self.path.clone();
        self.writer
            .finalize()
            .map_err(|e| audio_err(format!("finalize {}: {e}", path.display())))?;
        Ok(n)
    }
}

/// What [`repair_header`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavRepair {
    /// Sample frames in the repaired file.
    pub frames: u64,
    /// The header was rewritten (its sizes did not match the file).
    pub fixed: bool,
}

/// Rewrites the RIFF and `data` chunk sizes of a WAV whose writer died
/// before finalizing, from the file length (a trailing partial frame is cut
/// off). The `data` chunk must be the last chunk, as `hound` writes it.
pub fn repair_header(path: &Path) -> Result<WavRepair> {
    let bad = |m: &str| audio_err(format!("{}: {m}", path.display()));
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; 12];
    f.read_exact(&mut head)
        .map_err(|_| bad("too short for a WAV header"))?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let mut pos = 12u64;
    let mut block_align = 2u64;
    loop {
        if pos + 8 > len {
            return Err(bad("no data chunk"));
        }
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)?;
        let size = u64::from(u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]));
        match &ch[0..4] {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                f.read_exact(&mut fmt)?;
                block_align = u64::from(u16::from_le_bytes([fmt[12], fmt[13]])).max(1);
            }
            b"data" => {
                let data_start = pos + 8;
                let mut data_len = len.saturating_sub(data_start);
                data_len -= data_len % block_align;
                let data_len = data_len.min(u64::from(u32::MAX) - 64);
                let new_len = data_start + data_len;
                let riff = (new_len - 8) as u32;
                let fixed = size != data_len || new_len != len || {
                    let old_riff = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
                    old_riff != riff
                };
                if fixed {
                    if new_len != len {
                        f.set_len(new_len)?;
                    }
                    f.seek(SeekFrom::Start(4))?;
                    f.write_all(&riff.to_le_bytes())?;
                    f.seek(SeekFrom::Start(pos + 4))?;
                    f.write_all(&(data_len as u32).to_le_bytes())?;
                    f.flush()?;
                }
                return Ok(WavRepair {
                    frames: data_len / block_align,
                    fixed,
                });
            }
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_flushes_pads_and_finalizes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a").join("mic.wav");
        let mut w = WavSink::create(&p).unwrap();
        w.write(&vec![7i16; 20_000]).unwrap();
        // The second-long flush already put a valid header on disk.
        assert!(hound::WavReader::open(&p).unwrap().duration() >= 16_000);
        w.pad_to(40_000).unwrap();
        assert_eq!(w.finalize().unwrap(), 40_000);
        let r = hound::WavReader::open(&p).unwrap();
        assert_eq!(r.duration(), 40_000);
        assert_eq!(
            repair_header(&p).unwrap(),
            WavRepair {
                frames: 40_000,
                fixed: false
            }
        );
    }

    #[test]
    fn repairs_a_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("loopback.wav");
        let mut w = WavSink::create(&p).unwrap();
        w.write(&(0..48_000).map(|i| (i % 1_000) as i16).collect::<Vec<_>>())
            .unwrap();
        // A crash: the last flush covered 48 000 samples, then 10 000 more
        // reached the disk without a header update, and the last one was cut
        // in half.
        w.flush().unwrap();
        w.write(&vec![5i16; 10_000]).unwrap();
        std::mem::forget(w);
        let f = OpenOptions::new().write(true).open(&p).unwrap();
        let len = f.metadata().unwrap().len();
        f.set_len(len - 1).unwrap();
        drop(f);
        let before = hound::WavReader::open(&p).unwrap().duration();
        let r = repair_header(&p).unwrap();
        assert!(r.fixed);
        let on_disk = (std::fs::metadata(&p).unwrap().len() - 44) / 2;
        assert_eq!(r.frames, on_disk);
        assert!(r.frames > u64::from(before), "{} vs {before}", r.frames);
        let reader = hound::WavReader::open(&p).unwrap();
        assert_eq!(u64::from(reader.duration()), r.frames);
        let s: Vec<i16> = reader.into_samples().map(|s| s.unwrap()).collect();
        assert_eq!(s[999], 999);
        assert_eq!(s.len() as u64, r.frames);
        // Garbage is rejected.
        let junk = dir.path().join("junk.wav");
        std::fs::write(&junk, b"not a wav file at all").unwrap();
        assert!(repair_header(&junk).is_err());
    }
}
