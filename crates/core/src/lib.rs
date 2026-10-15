//! `meet-core`: everything in meet-assist that is not UI.
//!
//! The desktop app (`apps/desktop/src-tauri`) and the dev CLI (`crates/meet-cli`)
//! are thin shells over this crate. All logic lives here so it can be tested
//! without a GUI (see `docs/01-architecture.md`, "Repository layout").
//!
//! Phase 0 ships only the typed domain model ([`types`]), the engine wire
//! protocol ([`protocol`]) and the error type ([`error`]). The other modules are
//! landing spots for later phases.

pub mod audio;
pub mod db;
pub mod engine;
pub mod error;
pub mod models;
pub mod protocol;
pub mod transcript;
pub mod types;

pub use error::{CoreError, Result};
pub use types::{
    new_id, AudioTrack, Channel, Meeting, MeetingStatus, Segment, Source, Speaker, Word,
};

/// Version of `meet-core` (and of the workspace; all crates share it).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
