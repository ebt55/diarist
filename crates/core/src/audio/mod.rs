//! Audio capture, decode and pipeline (Phases 1 and 3).
//!
//! File mode: decode any container (symphonia, then Media Foundation, then
//! ffmpeg) to 16 kHz mono PCM16 WAV, behind the [`Decoder`] trait; see
//! [`decode`] and [`ChainDecoder`].
//! Live mode (Phase 3): WASAPI mic and render-endpoint loopback capture at
//! 16 kHz into lock-free rings, QPC clock anchors, zero-fill of gaps and
//! SILENT packets, per-channel WAV writers, and a bounded feed to the live
//! engine sessions. Real-time callbacks only copy; all work happens on
//! worker threads.

use std::path::Path;

use crate::error::Result;

pub mod decode;
#[cfg(windows)]
mod mf;

pub use decode::{
    find_on_path, ChainDecoder, FfmpegDecoder, MediaFoundationDecoder, SymphoniaDecoder,
    TARGET_RATE,
};

/// Facts about a decoded input file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedInfo {
    pub duration_ms: i64,
    /// Sample rate of the source before resampling.
    pub source_sample_rate: u32,
    /// Channel count of the source before downmixing.
    pub source_channels: u16,
}

/// Converts an input audio or video file to a 16 kHz mono PCM16 WAV.
/// A trait so the import pipeline can be tested with fakes.
pub trait Decoder: Send + Sync {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo>;
}
