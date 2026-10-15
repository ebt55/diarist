//! Audio capture, decode and pipeline (Phases 1 and 3).
//!
//! File mode: decode any container (symphonia, then Media Foundation, then
//! ffmpeg) to 16 kHz mono PCM16 WAV, behind the [`Decoder`] trait; see
//! [`decode`] and [`ChainDecoder`]. [`peaks`] computes waveform envelopes
//! for the player; [`mp4`] reads the edit list that gapless decoding needs.
//! Live mode (Phase 3): WASAPI mic and render-endpoint loopback capture at
//! 16 kHz into lock-free rings, QPC clock anchors, zero-fill of gaps and
//! SILENT packets, per-channel WAV writers, and a bounded feed to the live
//! engine sessions. Real-time callbacks only copy; all work happens on
//! worker threads.

use std::path::Path;

use crate::error::Result;

pub mod capture;
pub mod clock;
pub mod decode;
pub mod feed;
pub mod flac;
#[cfg(windows)]
mod mf;
pub mod mp4;
pub mod peaks;
pub mod pipeline;
pub mod wav;

pub use decode::{
    find_on_path, ChainDecoder, FfmpegDecoder, MediaFoundationDecoder, SymphoniaDecoder,
    TARGET_RATE,
};
pub use peaks::{peaks, peaks_cached, peaks_with};

use serde::{Deserialize, Serialize};

/// Which decoder produced a WAV (recorded in the run's `engine_info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoderKind {
    Symphonia,
    MediaFoundation,
    Ffmpeg,
    /// A decoder outside the built-in chain (tests, custom setups).
    Other,
}

impl DecoderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            DecoderKind::Symphonia => "symphonia",
            DecoderKind::MediaFoundation => "media_foundation",
            DecoderKind::Ffmpeg => "ffmpeg",
            DecoderKind::Other => "other",
        }
    }
}

/// Facts about a decoded input file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedInfo {
    pub duration_ms: i64,
    /// Sample rate of the source before resampling.
    pub source_sample_rate: u32,
    /// Channel count of the source before downmixing.
    pub source_channels: u16,
    /// The decoder that ran.
    pub decoder: DecoderKind,
}

/// Converts an input audio or video file to a 16 kHz mono PCM16 WAV.
/// A trait so the import pipeline can be tested with fakes.
pub trait Decoder: Send + Sync {
    fn decode_to_wav16k(&self, input: &Path, output: &Path) -> Result<DecodedInfo>;
}
