//! The AGC's gain track (plan 15 chunk B2a): what [`super::gain::Agc`]
//! applied to a channel while it recorded, written beside the channel's WAV
//! so that the post-pass echo canceller can replay it backwards. The stored
//! loopback is after the AGC, which raises a quiet far end by up to 40 dB
//! and moves at up to 40 dB/s; the canceller needs the loopback as the
//! speakers played it (`docs/research/14-echo-cancellation.md` section 2).
//! A mic recorded with `capture.mic_agc` gets a track too.
//!
//! # File format
//!
//! `<channel>.gain` (`loopback.gain`, `mic.gain`) in the meeting folder,
//! little-endian. A 16-byte header:
//!
//! | Offset | Type | Value |
//! |---|---|---|
//! | 0 | 4 bytes | `DGTK` |
//! | 4 | u16 | format version, 1 |
//! | 6 | u16 | values per record, 10 |
//! | 8 | u32 | sample rate, 16,000 |
//! | 12 | u32 | samples per value, 160 |
//!
//! then one record per 100 ms frame of the channel's timeline (the WAV's
//! samples), in order from sample 0: `values per record` × `f32`, the gain
//! in dB the AGC applied to each 160-sample block of the frame (the mean
//! over the block where the AGC's own 10 ms blocks straddle it: they follow
//! the capture packets, not the timeline). Value `k` of the file covers
//! samples `[160·k, 160·k + 160)`. The gain is never negative; 0 dB means
//! the samples passed unchanged.
//!
//! One value per 10 ms, not one per frame: the AGC moves up to 4 dB within
//! a 100 ms frame (attack 40 dB/s, and an instant drop when a block would
//! pass the limiter), which a per-frame value would leave in the reference
//! as an echo path that changes inside the frame.
//!
//! # Writing, crashes, lifetime
//!
//! The pipeline thread appends a record per frame and flushes every second
//! with the WAV ([`GainTrackWriter`]). Nothing in the file depends on its
//! length, so a crash needs no repair: the reader drops a trailing partial
//! record. 400 bytes per second, 1.44 MB per hour. The file is not an
//! audio track in the store: it lives in the meeting folder, goes with the
//! folder when the meeting is deleted, and goes with the FLAC when audio
//! retention deletes the meeting's audio (`crate::retention`).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::audio::capture::audio_err;
use crate::error::Result;

/// File magic.
pub const MAGIC: [u8; 4] = *b"DGTK";
/// Format version.
pub const VERSION: u16 = 1;
/// Header bytes.
pub const HEADER_LEN: usize = 16;
/// Values per record (10 ms blocks per 100 ms frame).
pub const VALUES_PER_RECORD: usize = 10;
/// Samples each value covers.
pub const SAMPLES_PER_VALUE: usize = 160;
/// Samples per record.
pub const SAMPLES_PER_RECORD: usize = VALUES_PER_RECORD * SAMPLES_PER_VALUE;
const RATE: u32 = 16_000;
/// Records between flushes (one second).
const FLUSH_EVERY: u32 = 10;

/// The gain track file of a channel WAV: `mic.wav` → `mic.gain`.
pub fn path_for(wav: &Path) -> PathBuf {
    wav.with_extension("gain")
}

fn header() -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(&MAGIC);
    h[4..6].copy_from_slice(&VERSION.to_le_bytes());
    h[6..8].copy_from_slice(&(VALUES_PER_RECORD as u16).to_le_bytes());
    h[8..12].copy_from_slice(&RATE.to_le_bytes());
    h[12..16].copy_from_slice(&(SAMPLES_PER_VALUE as u32).to_le_bytes());
    h
}

/// Appends records to a gain track while recording.
pub struct GainTrackWriter {
    w: BufWriter<File>,
    path: PathBuf,
    records: u64,
    since_flush: u32,
    last: [f32; VALUES_PER_RECORD],
}

impl GainTrackWriter {
    /// Creates (truncates) `path` and writes the header.
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let err = |e: std::io::Error| audio_err(format!("create {}: {e}", path.display()));
        let mut w = BufWriter::new(File::create(path).map_err(err)?);
        w.write_all(&header()).map_err(err)?;
        w.flush().map_err(err)?;
        Ok(Self {
            w,
            path: path.to_path_buf(),
            records: 0,
            since_flush: 0,
            last: [0.0; VALUES_PER_RECORD],
        })
    }

    /// Appends the record of the frame starting at `start_sample` (a
    /// multiple of [`SAMPLES_PER_RECORD`]). Frames come in order; a frame
    /// before the next expected one is ignored and a missing one repeats
    /// the last record. Flushes every second.
    pub fn push(&mut self, start_sample: u64, values: &[f32; VALUES_PER_RECORD]) -> Result<()> {
        let index = start_sample / SAMPLES_PER_RECORD as u64;
        if index < self.records {
            return Ok(());
        }
        while self.records < index {
            let last = self.last;
            self.write_record(&last)?;
        }
        self.write_record(values)?;
        self.last = *values;
        if self.since_flush >= FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    fn write_record(&mut self, values: &[f32; VALUES_PER_RECORD]) -> Result<()> {
        let mut b = [0u8; VALUES_PER_RECORD * 4];
        for (c, v) in b.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *c = v.to_le_bytes();
        }
        self.w
            .write_all(&b)
            .map_err(|e| audio_err(format!("write {}: {e}", self.path.display())))?;
        self.records += 1;
        self.since_flush += 1;
        Ok(())
    }

    /// Writes buffered records to the file.
    pub fn flush(&mut self) -> Result<()> {
        self.since_flush = 0;
        self.w
            .flush()
            .map_err(|e| audio_err(format!("flush {}: {e}", self.path.display())))
    }

    /// Records written so far.
    pub fn records(&self) -> u64 {
        self.records
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A gain track read back: one gain (dB) per [`SAMPLES_PER_VALUE`] samples.
#[derive(Debug, Clone, PartialEq)]
pub struct GainTrack {
    values: Vec<f32>,
}

fn lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

impl GainTrack {
    /// Reads `path`; `Ok(None)` when there is no such file. A trailing
    /// partial record (a crash mid-write) is dropped; a header that is not a
    /// version 1 gain track at 16 kHz with 10 values of 160 samples is an
    /// error.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(audio_err(format!("read {}: {e}", path.display()))),
        };
        Self::parse(&bytes)
            .map(Some)
            .map_err(|m| audio_err(format!("{}: {m}", path.display())))
    }

    /// Parses a whole file's bytes.
    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        if bytes.len() < HEADER_LEN || bytes[0..4] != MAGIC {
            return Err("not a gain track".into());
        }
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let (version, per, rate, spv) = (u16_at(4), u16_at(6), u32_at(8), u32_at(12));
        if version != VERSION
            || usize::from(per) != VALUES_PER_RECORD
            || rate != RATE
            || spv as usize != SAMPLES_PER_VALUE
        {
            return Err(format!(
                "unsupported gain track (version {version}, {per} values of {spv} samples at {rate} Hz)"
            ));
        }
        let record = VALUES_PER_RECORD * 4;
        let whole = (bytes.len() - HEADER_LEN) / record * record;
        let values = bytes[HEADER_LEN..HEADER_LEN + whole]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 })
            .collect();
        Ok(Self { values })
    }

    /// A track from values (one per [`SAMPLES_PER_VALUE`] samples).
    pub fn from_values(values: Vec<f32>) -> Self {
        Self { values }
    }

    /// One gain (dB) per 160 samples.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Whole frames in the file.
    pub fn frames(&self) -> usize {
        self.values.len() / VALUES_PER_RECORD
    }

    /// The gain (dB) applied to sample `s`; past the end the last value
    /// (a WAV padded at Stop, a track that lost its last second), 0 for an
    /// empty track.
    pub fn gain_db_at(&self, s: u64) -> f32 {
        let i = (s / SAMPLES_PER_VALUE as u64) as usize;
        self.values
            .get(i)
            .or(self.values.last())
            .copied()
            .unwrap_or(0.0)
    }

    /// `x` holds samples from `start` on: each is divided by its gain (the
    /// AGC undone).
    pub fn undo(&self, start: u64, x: &mut [f32]) {
        self.scale(start, x, true);
    }

    /// `x` holds samples from `start` on: each is multiplied by its gain.
    pub fn apply(&self, start: u64, x: &mut [f32]) {
        self.scale(start, x, false);
    }

    fn scale(&self, start: u64, x: &mut [f32], undo: bool) {
        let mut i = 0usize;
        while i < x.len() {
            let s = start + i as u64;
            let block_end = (s / SAMPLES_PER_VALUE as u64 + 1) * SAMPLES_PER_VALUE as u64;
            let n = ((block_end - s) as usize).min(x.len() - i);
            let g = self.gain_db_at(s);
            if g > 0.0 {
                let k = if undo { 1.0 / lin(g) } else { lin(g) };
                x[i..i + n].iter_mut().for_each(|v| *v *= k);
            }
            i += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(base: f32) -> [f32; VALUES_PER_RECORD] {
        std::array::from_fn(|k| base + k as f32 * 0.25)
    }

    #[test]
    fn writes_reads_and_survives_a_truncated_tail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audio").join("loopback.gain");
        assert_eq!(
            path_for(&dir.path().join("loopback.wav")).file_name(),
            p.file_name()
        );
        let mut w = GainTrackWriter::create(&p).unwrap();
        for k in 0..25u64 {
            w.push(k * SAMPLES_PER_RECORD as u64, &rec(k as f32))
                .unwrap();
        }
        // Ten records reach the disk at the one-second flush, before any
        // explicit flush.
        let on_disk = std::fs::metadata(&p).unwrap().len() as usize;
        assert!(on_disk >= HEADER_LEN + 20 * 40, "{on_disk}");
        w.flush().unwrap();
        assert_eq!(w.records(), 25);
        drop(w);
        let t = GainTrack::read(&p).unwrap().unwrap();
        assert_eq!(t.frames(), 25);
        assert_eq!(t.values()[0..10], rec(0.0));
        assert_eq!(t.values()[240..250], rec(24.0));
        assert_eq!(t.gain_db_at(24 * 1_600 + 9 * 160 + 3), 24.0 + 9.0 * 0.25);
        assert_eq!(t.gain_db_at(10_000_000), 24.0 + 9.0 * 0.25, "past the end");
        // A crash in the middle of a record: the whole records stay.
        let bytes = std::fs::read(&p).unwrap();
        std::fs::write(&p, &bytes[..bytes.len() - 13]).unwrap();
        let t = GainTrack::read(&p).unwrap().unwrap();
        assert_eq!(t.frames(), 24);
        assert_eq!(t.values()[230..240], rec(23.0));
        // Only the header: an empty track, gain 0.
        std::fs::write(&p, &bytes[..HEADER_LEN + 7]).unwrap();
        let t = GainTrack::read(&p).unwrap().unwrap();
        assert_eq!((t.frames(), t.gain_db_at(0)), (0, 0.0));
        // No file, a foreign file, another layout.
        assert_eq!(GainTrack::read(&dir.path().join("mic.gain")).unwrap(), None);
        std::fs::write(&p, b"RIFF....WAVE....").unwrap();
        assert!(GainTrack::read(&p).is_err());
        let mut h = header();
        h[6] = 5;
        std::fs::write(&p, h).unwrap();
        assert!(GainTrack::read(&p).is_err());
    }

    #[test]
    fn frames_out_of_order_or_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mic.gain");
        let mut w = GainTrackWriter::create(&p).unwrap();
        w.push(0, &rec(1.0)).unwrap();
        // Frame 1 is missing: it repeats frame 0; a stale frame 0 is ignored.
        w.push(2 * 1_600, &rec(3.0)).unwrap();
        w.push(0, &rec(9.0)).unwrap();
        w.flush().unwrap();
        let t = GainTrack::read(&p).unwrap().unwrap();
        assert_eq!(t.frames(), 3);
        assert_eq!(t.values()[10..20], rec(1.0));
        assert_eq!(t.values()[20..30], rec(3.0));
    }

    #[test]
    fn replay_undoes_and_reapplies_the_gain() {
        // Gains of 0, 6 and 20 dB on three blocks; a run starting mid-block.
        let t = GainTrack::from_values(vec![0.0, 6.0, 20.0]);
        let raw: Vec<f32> = (0..480).map(|i| ((i as f32) * 0.37).sin() * 0.01).collect();
        let mut stored = raw.clone();
        t.apply(0, &mut stored);
        assert_eq!(stored[..160], raw[..160], "0 dB passes unchanged");
        assert!((stored[200] / raw[200] - lin(6.0)).abs() < 1e-5);
        assert!((stored[400] / raw[400] - 10.0).abs() < 1e-4);
        let mut back = stored[100..].to_vec();
        t.undo(100, &mut back);
        for (a, b) in back.iter().zip(&raw[100..]) {
            assert!((a - b).abs() < 1e-7, "{a} {b}");
        }
    }
}
