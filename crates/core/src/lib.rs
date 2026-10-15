//! `meet-core`: everything in meet-assist that is not UI.
//!
//! The desktop app (`apps/desktop/src-tauri`) and the dev CLI (`crates/meet-cli`)
//! are thin shells over this crate. All logic lives here so it can be tested
//! without a GUI (see `docs/01-architecture.md`, "Repository layout").
//!
//! The seam to the UI is frozen in [`views`] (types, exported to TypeScript),
//! [`store::Store`] (every query and mutation) and [`pipeline`] (import and the
//! GPU [`pipeline::JobQueue`]). The engine contract is [`engine`] over the
//! data types in [`protocol`]. [`store::SqliteStore`] persists everything;
//! [`audio`] decodes files, [`transcript`] builds segments, [`fusion`] tags
//! words with diarization segments, [`export`] writes transcripts out and
//! [`models`] finds, selects and downloads engines and models.

pub mod audio;
pub mod db;
pub mod diagnostics;
pub mod engine;
pub mod error;
pub mod export;
pub mod fusion;
pub mod gpu;
pub mod live;
pub mod models;
pub mod pipeline;
pub mod protocol;
pub mod retention;
pub mod settings;
pub mod store;
pub mod transcript;
pub mod types;
pub mod views;

pub use error::{CoreError, Result};
pub use types::{
    new_id, AudioTrack, Channel, JobId, Meeting, MeetingId, MeetingStatus, RunId, RunKind,
    RunStatus, Segment, SegmentId, Source, Speaker, SpeakerId, TranscriptRun, Word, WordId,
};

/// Version of `meet-core` (and of the workspace; all crates share it).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
