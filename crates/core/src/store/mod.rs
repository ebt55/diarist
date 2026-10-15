//! The persistence seam: the [`Store`] trait (what the UI and the import
//! pipeline need), [`RunStore`] (run-level operations for the live path and
//! the post-pass), their error type, and both implementations:
//! [`MemoryStore`] (UI mock, reference) and [`SqliteStore`]
//! (`docs/02-data-model.md`). Both pass the same conformance suite
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

mod carry_over;
mod live;
mod memory;
mod seed;
mod sqlite;

pub use carry_over::CarryOverReport;
pub use memory::MemoryStore;
pub use seed::SeedOptions;
pub use sqlite::SqliteStore;

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::EngineWord;
use crate::transcript::SegmentOptions;
use crate::types::{
    AudioFormat, AudioTrack, Channel, ClockAnchor, Edit, LiveUtterance, Meeting, MeetingId,
    MeetingStatus, ModelCapabilities, ModelFile, ModelRole, ModelSource, RunId, RunKind, RunStatus,
    SegmentId, Source, Speaker, SpeakerId, TrackChannel, TrackId, TranscriptRun, Word, WordId,
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

    /// Audio files of a meeting, by id.
    fn list_audio_tracks(&self, id: MeetingId) -> StoreResult<Vec<AudioTrack>>;

    /// The track the UI plays (see [`pick_playback_track`]): the newest
    /// `playback` track of a live meeting, the newest `original` of an
    /// import, otherwise the newest track. `None` when the meeting has no
    /// tracks. Whether the file still exists is the caller's business.
    fn playback_track(&self, id: MeetingId) -> StoreResult<Option<AudioTrack>>;

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

    /// Clears `needs_review` (the "new label" badge) on one speaker row.
    fn mark_reviewed(&self, id: SpeakerId) -> StoreResult<()>;

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

    /// Deletes a setting so its default applies again; a no-op when absent.
    fn remove_setting(&self, key: &str) -> StoreResult<()>;

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

/// Run-level operations beyond [`Store`]: runs, live utterances and their
/// materialisation, carry-over between runs, clock anchors, uid lookup and
/// backups. The live recorder, the post-pass and the desktop app take
/// `&dyn RunStore` (which upcasts to `&dyn Store`).
pub trait RunStore: Store {
    fn get_run(&self, run: RunId) -> StoreResult<TranscriptRun>;

    /// Runs of a meeting, oldest first.
    fn list_runs(&self, meeting: MeetingId) -> StoreResult<Vec<TranscriptRun>>;

    /// Deletes a run and its speakers, segments, words and live utterances.
    /// The meeting's `current_run_id` is cleared if it pointed there.
    fn delete_run(&self, run: RunId) -> StoreResult<()>;

    /// Every speaker row of a run, by id.
    fn run_speakers(&self, run: RunId) -> StoreResult<Vec<Speaker>>;

    /// Time-ranged user edits of a meeting in replay order (`created_at`, id).
    fn list_edits(&self, meeting: MeetingId) -> StoreResult<Vec<Edit>>;

    /// Carries display names, colours, global ids and `reassign` edits from
    /// `from_run` (normally the meeting's current run) to `to_run` (a newer
    /// run of the same meeting), atomically. Does not change the current run.
    /// Rules: `docs/02-data-model.md`, "Carry-over from the previous current run".
    fn carry_over(&self, from_run: RunId, to_run: RunId) -> StoreResult<CarryOverReport>;

    /// Appends one revision of a live utterance (append-only; a later `rev`
    /// of the same `(channel, utt)` supersedes earlier ones). Word times are
    /// stored in whole milliseconds. Returns the row id. A repeated
    /// `(run, channel, utt, rev)` is a `Conflict`.
    fn append_live_utterance(
        &self,
        run: RunId,
        channel: Channel,
        utt: u32,
        rev: u32,
        words: &[EngineWord],
    ) -> StoreResult<i64>;

    /// Every stored revision of a run's live utterances, in insertion order.
    fn list_live_utterances(&self, run: RunId) -> StoreResult<Vec<LiveUtterance>>;

    /// Turns the latest revision of every live utterance of `run` into the
    /// run's speakers, segments and words (one transaction). Mic words
    /// without a speaker become `me`; other untagged words `spk_unknown`.
    /// Speaker rows created earlier with [`RunStore::live_speaker`] are
    /// reused with everything the user set on them (name, colour, merges).
    /// `Conflict` if the run already has segments.
    fn materialise_live_run(&self, run: RunId, opts: &SegmentOptions) -> StoreResult<()>;

    /// The speaker row of `(channel, engine_label)` in a live run that has
    /// not been materialised yet, created on first use, so provisional
    /// speakers can be renamed, recoloured or merged while recording.
    /// `Invalid` for a run that is not `live`; `Conflict` once the run has
    /// segments.
    fn live_speaker(
        &self,
        run: RunId,
        channel: Channel,
        engine_label: &str,
    ) -> StoreResult<Speaker>;

    /// Removes one audio track row (the file is the caller's business), e.g.
    /// the per-channel WAVs once the stereo FLAC replaced them.
    fn delete_audio_track(&self, track: TrackId) -> StoreResult<()>;

    /// Inserts (or replaces) clock anchors.
    fn add_clock_anchors(&self, anchors: &[ClockAnchor]) -> StoreResult<()>;

    /// Anchors of one channel by sample index.
    fn clock_anchors(&self, meeting: MeetingId, channel: Channel) -> StoreResult<Vec<ClockAnchor>>;

    /// Looks a meeting up by its ULID.
    fn meeting_id_by_uid(&self, uid: &str) -> StoreResult<Option<MeetingId>>;

    /// Writes a consistent copy of the database to `dest` (temp file, then
    /// rename). `MemoryStore` has nothing to back up and returns `Invalid`.
    fn backup(&self, dest: &Path) -> StoreResult<()>;
}

/// The track to play for a meeting of `source`: a live meeting's newest
/// `playback` track (the stereo FLAC), an import's newest `original` (the
/// user's file), otherwise the newest track of any kind (e.g. the per-channel
/// WAVs while a post-pass has not encoded the FLAC yet).
pub fn pick_playback_track(source: Source, tracks: &[AudioTrack]) -> Option<AudioTrack> {
    let newest =
        |f: &dyn Fn(&AudioTrack) -> bool| tracks.iter().filter(|t| f(t)).max_by_key(|t| t.id);
    let preferred = match source {
        Source::Live => newest(&|t| t.channel == TrackChannel::Playback),
        Source::File => newest(&|t| t.format == AudioFormat::Original),
    };
    preferred.or_else(|| newest(&|_| true)).cloned()
}

/// Validates a `#rrggbb` colour.
pub(crate) fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
