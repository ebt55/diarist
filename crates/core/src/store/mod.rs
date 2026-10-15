//! The persistence seam: the [`Store`] trait, its error type, and the
//! in-memory implementation [`MemoryStore`].
//!
//! The UI (through thin Tauri commands) and the pipeline only ever see
//! `&dyn Store`. Phase 1 adds `SqliteStore` (`docs/02-data-model.md`), which
//! must pass the same conformance suite as `MemoryStore`
//! (`crates/core/tests/store_conformance.rs`). Methods are synchronous; the
//! Tauri layer calls them inside `spawn_blocking`.
//!
//! Semantics shared by all implementations:
//! - "Current run" means `meetings.current_run_id`; transcript queries return
//!   nothing (not an error) for a meeting without one.
//! - Speakers resolve through merges (`COALESCE(merged_into, id)`); names and
//!   colours fall back to [`crate::views::default_speaker_name`] and
//!   [`crate::views::default_speaker_color`].
//! - A missing id is [`StoreError::NotFound`]; bad arguments are
//!   [`StoreError::Invalid`]; writes that would break an invariant (a second
//!   transcript into the same run) are [`StoreError::Conflict`].

mod memory;
mod seed;
mod sqlite;

pub use memory::MemoryStore;
pub use seed::SeedOptions;
pub use sqlite::{CarryOverReport, SqliteStore};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    AudioFormat, AudioTrack, Channel, Meeting, MeetingId, MeetingStatus, ModelCapabilities,
    ModelFile, ModelRole, ModelSource, RunId, RunKind, RunStatus, SegmentId, Source, SpeakerId,
    TrackChannel, TranscriptRun, Word, WordId,
};
use crate::views::{MeetingDetail, MeetingSummary, SearchHit, SegmentView, Settings};

/// Errors returned by [`Store`] methods.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    /// No row with this id (`what` is the table's singular, e.g. "meeting").
    #[error("{what} {id} not found")]
    NotFound { what: &'static str, id: i64 },
    /// The arguments are not acceptable (bad colour, cycle, foreign speaker, ...).
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The write would break an invariant.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The storage backend failed (SQLite error, poisoned lock, I/O).
    #[error("storage error: {0}")]
    Backend(String),
}

impl StoreError {
    pub fn not_found(what: &'static str, id: impl Into<i64>) -> Self {
        StoreError::NotFound {
            what,
            id: id.into(),
        }
    }
}

/// `Result` with [`StoreError`].
pub type StoreResult<T> = Result<T, StoreError>;

/// A meeting to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMeeting {
    pub title: String,
    pub source: Source,
    pub source_path: Option<String>,
    pub language: String,
    pub diarize_mic: bool,
    /// Unix ms, wall clock.
    pub started_at: i64,
    pub status: MeetingStatus,
}

/// An audio file to register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAudioTrack {
    pub meeting_id: MeetingId,
    pub channel: TrackChannel,
    pub path: String,
    pub format: AudioFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: Option<i64>,
    pub size_bytes: Option<i64>,
}

/// A complete transcript for one run, written in one transaction by
/// [`Store::insert_transcript`]. Rows refer to each other by index because
/// ids do not exist yet; `seq` is the segment's index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewTranscript {
    pub speakers: Vec<NewSpeaker>,
    pub segments: Vec<NewSegment>,
    pub words: Vec<NewWord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSpeaker {
    pub channel: Channel,
    /// Unique per channel within the run.
    pub engine_label: String,
    pub display_name: Option<String>,
    pub color: Option<String>,
    pub needs_review: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSegment {
    /// Index into [`NewTranscript::speakers`].
    pub speaker: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWord {
    /// Index into [`NewTranscript::segments`].
    pub segment: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub confidence: Option<f32>,
    pub raw_label: Option<String>,
    pub margin: Option<f32>,
    pub overlap: bool,
}

/// One file as found by the model scanner, before it has a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedModelFile {
    /// Relative to the data root; the identity of the row.
    pub path: String,
    pub role: ModelRole,
    pub architecture: Option<String>,
    pub display_name: String,
    pub quant: Option<String>,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub source: ModelSource,
    pub capabilities: ModelCapabilities,
    pub manifest_name: Option<String>,
}

/// Every query and mutation the UI and the import pipeline need.
///
/// Mirrors the command list in `docs/plans/03-phase2-desktop-ui.md`.
/// `engine_status` is not here: the Tauri layer fills
/// [`crate::views::EngineStatus`] from the `models` module.
pub trait Store: Send + Sync {
    // ---- Library and meeting view -------------------------------------

    /// All meetings, newest `started_at` first.
    fn list_meetings(&self) -> StoreResult<Vec<MeetingSummary>>;

    fn get_meeting(&self, id: MeetingId) -> StoreResult<MeetingDetail>;

    /// Segments of the current run in `seq` order, `offset`/`limit` for
    /// virtualized paging.
    fn get_segments(&self, id: MeetingId, offset: u32, limit: u32)
        -> StoreResult<Vec<SegmentView>>;

    /// Words of the current run overlapping `[from_ms, to_ms)`, by start time.
    fn get_words(&self, id: MeetingId, from_ms: i64, to_ms: i64) -> StoreResult<Vec<Word>>;

    /// Full-text search over the current runs of all meetings. Every
    /// whitespace-separated term must match (case-insensitive). A blank query
    /// returns nothing. Ranking is implementation-defined.
    fn search(&self, query: &str, limit: u32) -> StoreResult<Vec<SearchHit>>;

    /// Audio files of a meeting (the UI plays the `playback` track).
    fn list_audio_tracks(&self, id: MeetingId) -> StoreResult<Vec<AudioTrack>>;

    // ---- User edits -----------------------------------------------------

    /// Sets the display name (trimmed); a blank name restores the default.
    /// `anchor_segment` is the segment the rename was invoked from: its time
    /// range is stored as the name anchor used by carry-over. `None` (rename
    /// from the legend) clears the anchor.
    fn rename_speaker(
        &self,
        id: SpeakerId,
        name: &str,
        anchor_segment: Option<SegmentId>,
    ) -> StoreResult<()>;

    /// `color` must be `#rrggbb`.
    fn set_speaker_color(&self, id: SpeakerId, color: &str) -> StoreResult<()>;

    /// Merges `from` into the root of `into` (same run). Speakers merged into
    /// `from` are repointed so chains stay one level deep. Merging a speaker
    /// into itself or into one of its own children is `Invalid`.
    fn merge_speakers(&self, from: SpeakerId, into: SpeakerId) -> StoreResult<()>;

    /// Clears `merged_into`; a no-op for a root.
    fn unmerge_speaker(&self, id: SpeakerId) -> StoreResult<()>;

    /// Gives the whole segment to `to` (a speaker of the same run) and records
    /// a time-ranged `reassign` edit so the change survives new runs.
    fn reassign_segment(&self, segment: SegmentId, to: SpeakerId) -> StoreResult<()>;

    /// Splits a segment before `at_word` (which must be one of its words, not
    /// the first). The second half becomes a new segment with the same
    /// speaker, inserted right after; later `seq` values shift by one.
    /// Returns the new segment's id.
    fn split_segment(&self, segment: SegmentId, at_word: WordId) -> StoreResult<SegmentId>;

    fn set_meeting_title(&self, id: MeetingId, title: &str) -> StoreResult<()>;

    /// Pin (`true`) or unpin the meeting's audio against retention.
    fn set_audio_keep(&self, id: MeetingId, keep: bool) -> StoreResult<()>;

    /// Deletes the meeting and everything that belongs to it. `SqliteStore`
    /// also removes the meeting's audio folder.
    fn delete_meeting(&self, id: MeetingId) -> StoreResult<()>;

    // ---- Settings -------------------------------------------------------

    fn get_settings(&self) -> StoreResult<Settings>;

    fn set_setting(&self, key: &str, value: &str) -> StoreResult<()>;

    // ---- Model files (ComfyUI-style drop-in) -----------------------------

    /// Everything the scanner has seen, missing files included, ordered by
    /// role, then display name. Selections live in settings
    /// ([`crate::views::selection_key`]).
    fn list_model_files(&self) -> StoreResult<Vec<ModelFile>>;

    /// Applies one scan: rows are matched by `path`; found files are inserted
    /// or updated (`last_seen_at = now_ms`, `missing = false`, `first_seen_at`
    /// and `id` kept); known files not in `found` get `missing = true`.
    /// Returns the same list as [`Store::list_model_files`].
    fn sync_model_files(
        &self,
        found: &[ScannedModelFile],
        now_ms: i64,
    ) -> StoreResult<Vec<ModelFile>>;

    // ---- Pipeline writes ------------------------------------------------

    /// Creates a meeting with a fresh `uid`, no run and `audio_keep = false`.
    fn create_meeting(&self, new: NewMeeting) -> StoreResult<Meeting>;

    fn set_meeting_status(&self, id: MeetingId, status: MeetingStatus) -> StoreResult<()>;

    fn finish_meeting(&self, id: MeetingId, ended_at: i64, duration_ms: i64) -> StoreResult<()>;

    fn add_audio_track(&self, track: NewAudioTrack) -> StoreResult<AudioTrack>;

    /// Starts a run (`status = running`). It is not current until
    /// [`Store::set_current_run`].
    fn create_run(
        &self,
        meeting: MeetingId,
        kind: RunKind,
        engine_info: serde_json::Value,
    ) -> StoreResult<TranscriptRun>;

    /// Writes speakers, segments and words of a run in one transaction.
    /// `Conflict` if the run already has a transcript; `Invalid` for a bad
    /// index or a duplicate (channel, engine_label).
    fn insert_transcript(&self, run: RunId, transcript: &NewTranscript) -> StoreResult<()>;

    /// Marks a run finished with `status` (`done` or `failed`).
    fn finish_run(&self, run: RunId, status: RunStatus) -> StoreResult<()>;

    /// Makes `run` (which must belong to `meeting`) the one the UI shows.
    fn set_current_run(&self, meeting: MeetingId, run: RunId) -> StoreResult<()>;
}

/// Validates a `#rrggbb` colour.
pub(crate) fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
