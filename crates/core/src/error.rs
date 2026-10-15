//! Error type shared by everything in `meet-core`.

use thiserror::Error;

/// Errors produced by `meet-core`.
///
/// Phase 0 only needs the parsing and framing variants. Later phases add their
/// own (database, audio device, engine process, download) as they land.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A string did not match any variant of a domain enum (for example a
    /// `channel` column holding something other than `mic`, `loopback`, `source`).
    #[error("invalid {kind} value: {value:?}")]
    InvalidEnum { kind: &'static str, value: String },

    /// Not enough bytes to decode a fixed-size header.
    #[error("{what} too short: need {need} bytes, got {got}")]
    ShortBuffer {
        what: &'static str,
        need: usize,
        got: usize,
    },

    /// The first byte of a stdin frame was not a known frame type.
    #[error("unknown frame type 0x{0:02x}")]
    UnknownFrameType(u8),

    /// A frame payload does not fit the `u32` length field.
    #[error("frame payload too large: {0} bytes")]
    FrameTooLarge(usize),

    /// JSON (de)serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `Result` with [`CoreError`] as the default error type.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
