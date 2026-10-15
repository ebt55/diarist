//! Error type shared by everything in `meet-core` except the store, which
//! has its own [`StoreError`] (wrapped here as [`CoreError::Store`]).

use thiserror::Error;

use crate::store::StoreError;

/// Errors produced by `meet-core`.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A string did not match any variant of a domain enum (for example a
    /// `channel` column holding something other than `mic`, `loopback`, `source`).
    #[error("invalid {kind} value: {value:?}")]
    InvalidEnum { kind: &'static str, value: String },

    /// A documented entry point whose body lands in a later phase.
    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),

    /// A [`crate::store::Store`] call failed.
    #[error(transparent)]
    Store(#[from] StoreError),

    /// No decoder could turn the input into 16 kHz mono PCM16.
    #[error("decode failed: {0}")]
    Decode(String),

    /// The inference engine failed to start or reported an error.
    #[error("engine error: {0}")]
    Engine(String),

    /// Model or engine management (scan, manifest, pull, download) failed.
    #[error("models: {0}")]
    Models(String),

    /// The job was cancelled.
    #[error("cancelled")]
    Cancelled,

    /// Not enough free disk space for the operation.
    #[error("not enough free disk space in {path}: {free_mb} MB free, {needed_mb} MB needed")]
    InsufficientSpace {
        path: String,
        free_mb: u64,
        needed_mb: u64,
    },

    /// Not enough bytes to decode a fixed-size header (`ffi-wire`).
    #[error("{what} too short: need {need} bytes, got {got}")]
    ShortBuffer {
        what: &'static str,
        need: usize,
        got: usize,
    },

    /// The first byte of a sidecar frame was not a known frame type (`ffi-wire`).
    #[error("unknown frame type 0x{0:02x}")]
    UnknownFrameType(u8),

    /// A frame payload does not fit the `u32` length field (`ffi-wire`).
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
