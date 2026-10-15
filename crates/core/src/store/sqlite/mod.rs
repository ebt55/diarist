//! [`SqliteStore`]: the persistent [`Store`] over one SQLite file
//! (`docs/02-data-model.md` v0.3).
//!
//! - rusqlite with the bundled SQLite (FTS5 compiled in), WAL, foreign keys on,
//!   `busy_timeout` 5 s.
//! - Schema changes are numbered SQL files under `migrations/`, embedded with
//!   `include_str!` and applied in order by `PRAGMA user_version`.
//! - Full-text search uses the external-content FTS5 table `segments_fts`
//!   (kept in sync by triggers) and only returns segments of each meeting's
//!   current run. Snippets are split into [`SnippetPart`]s, never HTML.
//! - Beyond the trait: runs and clock anchors, live utterances and their
//!   materialisation ([`SqliteStore::append_live_utterance`],
//!   [`SqliteStore::materialise_live_run`]), name and edit carry-over between
//!   runs ([`SqliteStore::carry_over`]) and online backups
//!   ([`SqliteStore::backup`]).
//!
//! One connection behind a mutex: the desktop app calls the store from
//! `spawn_blocking`, and SQLite serialises writers anyway.

mod carry_over;
mod live;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

pub use carry_over::CarryOverReport;

use super::{
    is_hex_color, NewAudioTrack, NewMeeting, NewTranscript, ScannedModelFile, Store, StoreError,
    StoreResult,
};
use crate::error::CoreError;
use crate::types::{
    new_id, AudioTrack, Channel, ClockAnchor, Edit, EditKind, Meeting, MeetingId, MeetingStatus,
    ModelCapabilities, ModelFile, ModelFileId, ReassignTarget, RunId, RunKind, RunStatus,
    SegmentId, Speaker, SpeakerId, TrackId, TranscriptRun, Word, WordId,
};
use crate::views::{
    default_speaker_color, default_speaker_name, MeetingDetail, MeetingSummary, SearchHit,
    SegmentView, Settings, SnippetPart, SpeakerView,
};

/// Numbered migrations; entry `i` moves `user_version` from `i` to `i + 1`.
const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("migrations/0001_init.sql"))];

/// Characters of transcript text in [`MeetingSummary::preview`].
const PREVIEW_CHARS: usize = 200;
/// Tokens of context in a search snippet (FTS5 `snippet()` limit is 64).
const SNIPPET_TOKENS: i64 = 32;
/// Private-use characters that delimit hits in `snippet()` output.
const HIT_OPEN: char = '\u{E000}';
const HIT_CLOSE: char = '\u{E001}';

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Backend(e.to_string())
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The persistent [`Store`]. See the module docs.
pub struct SqliteStore {
    conn: Mutex<Connection>,
    /// The database file (`None` for an in-memory store).
    path: Option<PathBuf>,
    /// `<data root>/audio`: meeting folders `audio/<uid>` removed by
    /// `delete_meeting` (`None` for an in-memory store).
    audio_root: Option<PathBuf>,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path` and applies pending
    /// migrations. Meeting audio folders are expected in `audio/` next to it
    /// (`docs/01` data layout); see [`SqliteStore::with_audio_root`].
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Backend(format!("create {}: {e}", parent.display())))?;
        }
        let conn = Connection::open(path)?;
        let audio_root = path.parent().map(|p| p.join("audio"));
        Self::init(conn, Some(path.to_owned()), audio_root)
    }

    /// A fresh in-memory database (tests, conformance suite).
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?, None, None)
    }

    /// Overrides where meeting audio folders live (`<root>/<uid>`).
    pub fn with_audio_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.audio_root = Some(root.into());
        self
    }

    fn init(
        mut conn: Connection,
        path: Option<PathBuf>,
        audio_root: Option<PathBuf>,
    ) -> StoreResult<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // In-memory databases answer "memory"; that is fine.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
            audio_root,
        })
    }

    fn lock(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Backend("sqlite store lock poisoned".into()))
    }

    /// The database file, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Folder of a meeting's audio files (`<audio root>/<uid>`).
    pub fn audio_dir(&self, uid: &str) -> Option<PathBuf> {
        self.audio_root.as_ref().map(|r| r.join(uid))
    }

    /// `PRAGMA user_version`: number of migrations applied.
    pub fn schema_version(&self) -> StoreResult<u32> {
        Ok(self
            .lock()?
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Applies pending migrations (a no-op when up to date). Returns the
    /// resulting schema version.
    pub fn migrate(&self) -> StoreResult<u32> {
        migrate(&mut *self.lock()?)
    }

    /// Writes a consistent copy of the database to `dest` with the online
    /// backup API: into `<dest>.tmp` first, then renamed over `dest`.
    pub fn backup(&self, dest: impl AsRef<Path>) -> StoreResult<()> {
        let dest = dest.as_ref();
        let mut tmp = dest.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let _ = std::fs::remove_file(&tmp);
        {
            let conn = self.lock()?;
            conn.backup(rusqlite::MAIN_DB, &tmp, None)?;
        }
        std::fs::rename(&tmp, dest)
            .map_err(|e| StoreError::Backend(format!("rename backup to {}: {e}", dest.display())))
    }

    // ---- runs ----------------------------------------------------------------

    pub fn get_run(&self, run: RunId) -> StoreResult<TranscriptRun> {
        get_run(&*self.lock()?, run)
    }

    /// Runs of a meeting, oldest first.
    pub fn list_runs(&self, meeting: MeetingId) -> StoreResult<Vec<TranscriptRun>> {
        let conn = self.lock()?;
        meeting_exists(&conn, meeting)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM transcript_runs WHERE meeting_id = ?1 ORDER BY id"
        ))?;
        let runs = stmt
            .query_map([meeting.0], run_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(runs)
    }

    /// Deletes a run and its speakers, segments, words and live utterances.
    /// The meeting's `current_run_id` is cleared if it pointed there.
    pub fn delete_run(&self, run: RunId) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        get_run(&tx, run)?;
        // Segments first: `segments.speaker_id` has no ON DELETE action.
        tx.execute("DELETE FROM segments WHERE run_id = ?1", [run.0])?;
        tx.execute("DELETE FROM transcript_runs WHERE id = ?1", [run.0])?;
        tx.commit()?;
        Ok(())
    }

    /// Every speaker row of a run, by id.
    pub fn run_speakers(&self, run: RunId) -> StoreResult<Vec<Speaker>> {
        let conn = self.lock()?;
        get_run(&conn, run)?;
        run_speakers(&conn, run)
    }

    /// Time-ranged user edits of a meeting in replay order.
    pub fn list_edits(&self, meeting: MeetingId) -> StoreResult<Vec<Edit>> {
        let conn = self.lock()?;
        meeting_exists(&conn, meeting)?;
        list_edits(&conn, meeting)
    }

    /// Looks a meeting up by its ULID.
    pub fn meeting_id_by_uid(&self, uid: &str) -> StoreResult<Option<MeetingId>> {
        Ok(self
            .lock()?
            .query_row("SELECT id FROM meetings WHERE uid = ?1", [uid], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .map(MeetingId))
    }

    // ---- clock anchors -------------------------------------------------------

    /// Inserts (or replaces) clock anchors.
    pub fn add_clock_anchors(&self, anchors: &[ClockAnchor]) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO clock_anchors (meeting_id, channel, sample_index, qpc_100ns)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for a in anchors {
                meeting_exists(&tx, a.meeting_id)?;
                stmt.execute(params![
                    a.meeting_id.0,
                    a.channel.as_str(),
                    a.sample_index,
                    a.qpc_100ns
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Anchors of one channel by sample index.
    pub fn clock_anchors(
        &self,
        meeting: MeetingId,
        channel: Channel,
    ) -> StoreResult<Vec<ClockAnchor>> {
        let conn = self.lock()?;
        meeting_exists(&conn, meeting)?;
        let mut stmt = conn.prepare(
            "SELECT sample_index, qpc_100ns FROM clock_anchors
             WHERE meeting_id = ?1 AND channel = ?2 ORDER BY sample_index",
        )?;
        let rows = stmt
            .query_map(params![meeting.0, channel.as_str()], |r| {
                Ok(ClockAnchor {
                    meeting_id: meeting,
                    channel,
                    sample_index: r.get(0)?,
                    qpc_100ns: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn migrate(conn: &mut Connection) -> StoreResult<u32> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current as usize > MIGRATIONS.len() {
        return Err(StoreError::Backend(format!(
            "database schema version {current} is newer than this build supports ({})",
            MIGRATIONS.len()
        )));
    }
    for (i, (name, sql)) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .map_err(|e| StoreError::Backend(format!("migration {name}: {e}")))?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
    }
    Ok(MIGRATIONS.len() as u32)
}

// ---- row mapping -------------------------------------------------------------

fn parse_enum<T: FromStr<Err = CoreError>>(r: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    s.parse::<T>()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn parse_json<T: serde::de::DeserializeOwned>(r: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    serde_json::from_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn to_json<T: serde::Serialize>(v: &T) -> StoreResult<String> {
    serde_json::to_string(v).map_err(|e| StoreError::Backend(format!("json: {e}")))
}

const MEETING_COLS: &str = "id, uid, title, source, source_path, language, diarize_mic, \
    started_at, ended_at, duration_ms, status, current_run_id, audio_keep, created_at, updated_at";

fn meeting_from_row(r: &Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: MeetingId(r.get(0)?),
        uid: r.get(1)?,
        title: r.get(2)?,
        source: parse_enum(r, 3)?,
        source_path: r.get(4)?,
        language: r.get(5)?,
        diarize_mic: r.get(6)?,
        started_at: r.get(7)?,
        ended_at: r.get(8)?,
        duration_ms: r.get(9)?,
        status: parse_enum(r, 10)?,
        current_run_id: r.get::<_, Option<i64>>(11)?.map(RunId),
        audio_keep: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

const RUN_COLS: &str = "id, meeting_id, kind, engine_info, started_at, finished_at, status";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<TranscriptRun> {
    Ok(TranscriptRun {
        id: RunId(r.get(0)?),
        meeting_id: MeetingId(r.get(1)?),
        kind: parse_enum(r, 2)?,
        engine_info: parse_json(r, 3)?,
        started_at: r.get(4)?,
        finished_at: r.get(5)?,
        status: parse_enum(r, 6)?,
    })
}

const SPEAKER_COLS: &str = "id, run_id, meeting_id, channel, engine_label, display_name, color, \
    global_id, merged_into, needs_review, name_anchor_start_ms, name_anchor_end_ms";

fn speaker_from_row(r: &Row<'_>) -> rusqlite::Result<Speaker> {
    Ok(Speaker {
        id: SpeakerId(r.get(0)?),
        run_id: RunId(r.get(1)?),
        meeting_id: MeetingId(r.get(2)?),
        channel: parse_enum(r, 3)?,
        engine_label: r.get(4)?,
        display_name: r.get(5)?,
        color: r.get(6)?,
        global_id: r.get(7)?,
        merged_into: r.get::<_, Option<i64>>(8)?.map(SpeakerId),
        needs_review: r.get(9)?,
        name_anchor_start_ms: r.get(10)?,
        name_anchor_end_ms: r.get(11)?,
    })
}

const WORD_COLS: &str =
    "w.id, w.segment_id, w.meeting_id, w.start_ms, w.end_ms, w.text, w.confidence, \
     w.raw_label, w.margin, w.overlap";

fn word_from_row(r: &Row<'_>) -> rusqlite::Result<Word> {
    Ok(Word {
        id: WordId(r.get(0)?),
        segment_id: SegmentId(r.get(1)?),
        meeting_id: MeetingId(r.get(2)?),
        start_ms: r.get(3)?,
        end_ms: r.get(4)?,
        text: r.get(5)?,
        confidence: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
        raw_label: r.get(7)?,
        margin: r.get::<_, Option<f64>>(8)?.map(|v| v as f32),
        overlap: r.get(9)?,
    })
}

const TRACK_COLS: &str =
    "id, meeting_id, channel, path, format, sample_rate, channels, duration_ms, size_bytes";

fn track_from_row(r: &Row<'_>) -> rusqlite::Result<AudioTrack> {
    Ok(AudioTrack {
        id: TrackId(r.get(0)?),
        meeting_id: MeetingId(r.get(1)?),
        channel: parse_enum(r, 2)?,
        path: r.get(3)?,
        format: parse_enum(r, 4)?,
        sample_rate: r.get(5)?,
        channels: r.get(6)?,
        duration_ms: r.get(7)?,
        size_bytes: r.get(8)?,
    })
}

const MODEL_COLS: &str = "id, path, role, architecture, display_name, quant, size_bytes, sha256, \
    source, capabilities, manifest_name, first_seen_at, last_seen_at, missing";

fn model_from_row(r: &Row<'_>) -> rusqlite::Result<ModelFile> {
    Ok(ModelFile {
        id: ModelFileId(r.get(0)?),
        path: r.get(1)?,
        role: parse_enum(r, 2)?,
        architecture: r.get(3)?,
        display_name: r.get(4)?,
        quant: r.get(5)?,
        size_bytes: r.get(6)?,
        sha256: r.get(7)?,
        source: parse_enum(r, 8)?,
        capabilities: parse_json::<ModelCapabilities>(r, 9)?,
        manifest_name: r.get(10)?,
        first_seen_at: r.get(11)?,
        last_seen_at: r.get(12)?,
        missing: r.get(13)?,
    })
}

// ---- shared queries ------------------------------------------------------------

fn get_meeting_row(conn: &Connection, id: MeetingId) -> StoreResult<Meeting> {
    conn.query_row(
        &format!("SELECT {MEETING_COLS} FROM meetings WHERE id = ?1"),
        [id.0],
        meeting_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::not_found("meeting", id))
}

fn meeting_exists(conn: &Connection, id: MeetingId) -> StoreResult<()> {
    let found: Option<i64> = conn
        .query_row("SELECT id FROM meetings WHERE id = ?1", [id.0], |r| {
            r.get(0)
        })
        .optional()?;
    found
        .map(|_| ())
        .ok_or_else(|| StoreError::not_found("meeting", id))
}

fn get_run(conn: &Connection, run: RunId) -> StoreResult<TranscriptRun> {
    conn.query_row(
        &format!("SELECT {RUN_COLS} FROM transcript_runs WHERE id = ?1"),
        [run.0],
        run_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::not_found("run", run))
}

fn get_speaker(conn: &Connection, id: SpeakerId) -> StoreResult<Speaker> {
    conn.query_row(
        &format!("SELECT {SPEAKER_COLS} FROM speakers WHERE id = ?1"),
        [id.0],
        speaker_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::not_found("speaker", id))
}

fn run_speakers(conn: &Connection, run: RunId) -> StoreResult<Vec<Speaker>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SPEAKER_COLS} FROM speakers WHERE run_id = ?1 ORDER BY id"
    ))?;
    let rows = stmt
        .query_map([run.0], speaker_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn list_edits(conn: &Connection, meeting: MeetingId) -> StoreResult<Vec<Edit>> {
    let mut stmt = conn.prepare(
        "SELECT id, meeting_id, kind, channel, start_ms, end_ms, payload, created_at
         FROM edits WHERE meeting_id = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt
        .query_map([meeting.0], |r| {
            Ok(Edit {
                id: r.get(0)?,
                meeting_id: MeetingId(r.get(1)?),
                kind: parse_enum::<EditKind>(r, 2)?,
                channel: parse_enum(r, 3)?,
                start_ms: r.get(4)?,
                end_ms: r.get(5)?,
                payload: parse_json::<ReassignTarget>(r, 6)?,
                created_at: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn display_name(label: &str, name: Option<String>) -> String {
    name.unwrap_or_else(|| default_speaker_name(label))
}

fn display_color(label: &str, color: Option<String>) -> String {
    color.unwrap_or_else(|| default_speaker_color(label).to_owned())
}

/// A word row as used by segment splitting.
#[derive(Debug, Clone)]
struct WordRow {
    id: i64,
    start_ms: i64,
    end_ms: i64,
    text: String,
}

fn segment_words(conn: &Connection, segment: i64) -> StoreResult<Vec<WordRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, start_ms, end_ms, text FROM words WHERE segment_id = ?1 ORDER BY start_ms, id",
    )?;
    let rows = stmt
        .query_map([segment], |r| {
            Ok(WordRow {
                id: r.get(0)?,
                start_ms: r.get(1)?,
                end_ms: r.get(2)?,
                text: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A segment row as used by splitting.
#[derive(Debug, Clone, Copy)]
struct SegRow {
    id: i64,
    run_id: i64,
    meeting_id: i64,
    speaker_id: i64,
    start_ms: i64,
    end_ms: i64,
    seq: i64,
}

fn get_seg_row(conn: &Connection, id: i64) -> StoreResult<SegRow> {
    conn.query_row(
        "SELECT id, run_id, meeting_id, speaker_id, start_ms, end_ms, seq FROM segments WHERE id = ?1",
        [id],
        |r| {
            Ok(SegRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                meeting_id: r.get(2)?,
                speaker_id: r.get(3)?,
                start_ms: r.get(4)?,
                end_ms: r.get(5)?,
                seq: r.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| StoreError::not_found("segment", id))
}

fn join_words(words: &[WordRow]) -> String {
    crate::transcript::join_text(words.iter().map(|w| w.text.as_str()))
}

/// Rewrites one segment as consecutive groups of its words (`groups` covers
/// all words in order, each with its speaker). The first group keeps the
/// segment row; the others become new segments right after it, and later
/// `seq` values shift. Returns the ids of all resulting segments.
fn regroup_segment(
    tx: &Transaction<'_>,
    seg: SegRow,
    groups: &[(&[WordRow], i64)],
) -> StoreResult<Vec<i64>> {
    debug_assert!(!groups.is_empty());
    let extra = groups.len() as i64 - 1;
    let mut ids = vec![seg.id];
    let last = groups.len() - 1;
    let bounds = |i: usize, words: &[WordRow]| {
        let start = if i == 0 {
            seg.start_ms
        } else {
            words[0].start_ms
        };
        let end = if i == last {
            seg.end_ms
        } else {
            words.iter().map(|w| w.end_ms).max().unwrap_or(start)
        };
        (start, end.max(start))
    };
    let (first_words, first_speaker) = groups[0];
    let (s0, e0) = bounds(0, first_words);
    tx.execute(
        "UPDATE segments SET start_ms = ?2, end_ms = ?3, text = ?4, speaker_id = ?5 WHERE id = ?1",
        params![seg.id, s0, e0, join_words(first_words), first_speaker],
    )?;
    if extra == 0 {
        return Ok(ids);
    }
    tx.execute(
        "UPDATE segments SET seq = seq + ?3 WHERE run_id = ?1 AND seq > ?2",
        params![seg.run_id, seg.seq, extra],
    )?;
    let mut move_word = tx.prepare("UPDATE words SET segment_id = ?2 WHERE id = ?1")?;
    for (i, (words, speaker)) in groups.iter().enumerate().skip(1) {
        let (s, e) = bounds(i, words);
        tx.execute(
            "INSERT INTO segments (run_id, meeting_id, speaker_id, start_ms, end_ms, text, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                seg.run_id,
                seg.meeting_id,
                speaker,
                s,
                e,
                join_words(words),
                seg.seq + i as i64
            ],
        )?;
        let new_id = tx.last_insert_rowid();
        for w in words.iter() {
            move_word.execute(params![w.id, new_id])?;
        }
        ids.push(new_id);
    }
    Ok(ids)
}

/// Builds an FTS5 query: every whitespace-separated term must match, each as
/// a quoted prefix phrase. Terms without letters or digits are dropped.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| {
            let clean: String = t
                .chars()
                .filter(|c| *c != HIT_OPEN && *c != HIT_CLOSE)
                .collect();
            format!("\"{}\"*", clean.replace('"', "\"\""))
        })
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

/// Splits `snippet()` output (hits wrapped in [`HIT_OPEN`]/[`HIT_CLOSE`]) into parts.
fn snippet_parts(marked: &str) -> Vec<SnippetPart> {
    let mut parts: Vec<SnippetPart> = Vec::new();
    let mut cur = String::new();
    let mut hit = false;
    let flush = |cur: &mut String, hit: bool, parts: &mut Vec<SnippetPart>| {
        if !cur.is_empty() {
            parts.push(SnippetPart {
                text: std::mem::take(cur),
                hit,
            });
        }
    };
    for c in marked.chars() {
        match c {
            HIT_OPEN => {
                flush(&mut cur, hit, &mut parts);
                hit = true;
            }
            HIT_CLOSE => {
                flush(&mut cur, hit, &mut parts);
                hit = false;
            }
            c => cur.push(c),
        }
    }
    flush(&mut cur, hit, &mut parts);
    parts
}

fn speaker_views(conn: &Connection, run: RunId) -> StoreResult<Vec<SpeakerView>> {
    let mut stmt = conn.prepare(
        "SELECT sp.id, sp.channel, sp.engine_label, sp.display_name, sp.color, sp.merged_into,
                sp.needs_review, COUNT(s.id), COALESCE(SUM(s.end_ms - s.start_ms), 0)
         FROM speakers sp LEFT JOIN segments s ON s.speaker_id = sp.id
         WHERE sp.run_id = ?1 GROUP BY sp.id ORDER BY sp.id",
    )?;
    struct Row0 {
        id: SpeakerId,
        channel: Channel,
        label: String,
        name: Option<String>,
        color: Option<String>,
        merged_into: Option<SpeakerId>,
        needs_review: bool,
        count: u32,
        talk: i64,
    }
    let rows: Vec<Row0> = stmt
        .query_map([run.0], |r| {
            Ok(Row0 {
                id: SpeakerId(r.get(0)?),
                channel: parse_enum(r, 1)?,
                label: r.get(2)?,
                name: r.get(3)?,
                color: r.get(4)?,
                merged_into: r.get::<_, Option<i64>>(5)?.map(SpeakerId),
                needs_review: r.get(6)?,
                count: r.get(7)?,
                talk: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut child_totals: HashMap<SpeakerId, (u32, i64)> = HashMap::new();
    for r in &rows {
        if let Some(root) = r.merged_into {
            let e = child_totals.entry(root).or_default();
            e.0 += r.count;
            e.1 += r.talk;
        }
    }
    let mut views: Vec<SpeakerView> = rows
        .into_iter()
        .map(|r| {
            let (mut count, mut talk) = (r.count, r.talk);
            if r.merged_into.is_none() {
                if let Some((c, t)) = child_totals.get(&r.id) {
                    count += c;
                    talk += t;
                }
            }
            SpeakerView {
                id: r.id,
                channel: r.channel,
                name: display_name(&r.label, r.name.clone()),
                color: display_color(&r.label, r.color),
                engine_label: r.label,
                display_name: r.name,
                merged_into: r.merged_into,
                needs_review: r.needs_review,
                segment_count: count,
                talk_ms: talk,
            }
        })
        .collect();
    views.sort_by(|a, b| {
        (a.merged_into.is_some(), -a.talk_ms, a.id).cmp(&(
            b.merged_into.is_some(),
            -b.talk_ms,
            b.id,
        ))
    });
    Ok(views)
}

fn summary(conn: &Connection, m: Meeting) -> StoreResult<MeetingSummary> {
    let (speaker_count, preview) = match m.current_run_id {
        None => (0, String::new()),
        Some(run) => {
            let count: u32 = conn.query_row(
                "SELECT COUNT(DISTINCT COALESCE(sp.merged_into, sp.id))
                 FROM segments s JOIN speakers sp ON sp.id = s.speaker_id WHERE s.run_id = ?1",
                [run.0],
                |r| r.get(0),
            )?;
            let mut stmt =
                conn.prepare_cached("SELECT text FROM segments WHERE run_id = ?1 ORDER BY seq")?;
            let mut rows = stmt.query([run.0])?;
            let mut preview = String::new();
            let mut chars = 0usize;
            while let Some(row) = rows.next()? {
                if chars >= PREVIEW_CHARS {
                    break;
                }
                let text: String = row.get(0)?;
                if !preview.is_empty() {
                    preview.push(' ');
                    chars += 1;
                }
                for c in text.chars() {
                    if chars >= PREVIEW_CHARS {
                        break;
                    }
                    preview.push(c);
                    chars += 1;
                }
            }
            (count, preview)
        }
    };
    Ok(MeetingSummary {
        id: m.id,
        uid: m.uid,
        title: m.title,
        source: m.source,
        status: m.status,
        started_at: m.started_at,
        duration_ms: m.duration_ms,
        speaker_count,
        preview,
        audio_keep: m.audio_keep,
    })
}

fn not_found_if_zero(n: usize, what: &'static str, id: i64) -> StoreResult<()> {
    if n == 0 {
        Err(StoreError::not_found(what, id))
    } else {
        Ok(())
    }
}

/// Validates a transcript like `MemoryStore` does.
fn validate_transcript(t: &NewTranscript) -> StoreResult<()> {
    let mut labels = HashSet::new();
    for s in &t.speakers {
        if !labels.insert((s.channel, s.engine_label.as_str())) {
            return Err(StoreError::Invalid(format!(
                "duplicate speaker {} on channel {}",
                s.engine_label, s.channel
            )));
        }
        if let Some(c) = &s.color {
            if !is_hex_color(c) {
                return Err(StoreError::Invalid(format!("bad colour {c:?}")));
            }
        }
    }
    for (i, seg) in t.segments.iter().enumerate() {
        if seg.speaker >= t.speakers.len() {
            return Err(StoreError::Invalid(format!(
                "segment {i} refers to speaker index {}",
                seg.speaker
            )));
        }
        if seg.end_ms < seg.start_ms {
            return Err(StoreError::Invalid(format!(
                "segment {i} ends before it starts"
            )));
        }
    }
    for (i, w) in t.words.iter().enumerate() {
        if w.segment >= t.segments.len() {
            return Err(StoreError::Invalid(format!(
                "word {i} refers to segment index {}",
                w.segment
            )));
        }
    }
    Ok(())
}

/// Writes a validated transcript into `run` (inside the caller's transaction).
fn write_transcript(
    tx: &Transaction<'_>,
    run: RunId,
    meeting: MeetingId,
    t: &NewTranscript,
) -> StoreResult<()> {
    let mut speaker_ids = Vec::with_capacity(t.speakers.len());
    {
        let mut stmt = tx.prepare(
            "INSERT INTO speakers (run_id, meeting_id, channel, engine_label, display_name, color, needs_review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for s in &t.speakers {
            stmt.execute(params![
                run.0,
                meeting.0,
                s.channel.as_str(),
                s.engine_label,
                s.display_name,
                s.color.as_ref().map(|c| c.to_ascii_lowercase()),
                s.needs_review
            ])?;
            speaker_ids.push(tx.last_insert_rowid());
        }
    }
    let mut segment_ids = Vec::with_capacity(t.segments.len());
    {
        let mut stmt = tx.prepare(
            "INSERT INTO segments (run_id, meeting_id, speaker_id, start_ms, end_ms, text, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (i, s) in t.segments.iter().enumerate() {
            stmt.execute(params![
                run.0,
                meeting.0,
                speaker_ids[s.speaker],
                s.start_ms,
                s.end_ms,
                s.text,
                i as i64
            ])?;
            segment_ids.push(tx.last_insert_rowid());
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO words (segment_id, meeting_id, start_ms, end_ms, text, confidence, raw_label, margin, overlap)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for w in &t.words {
            stmt.execute(params![
                segment_ids[w.segment],
                meeting.0,
                w.start_ms,
                w.end_ms,
                w.text,
                w.confidence.map(f64::from),
                w.raw_label,
                w.margin.map(f64::from),
                w.overlap
            ])?;
        }
    }
    Ok(())
}

fn run_has_transcript(conn: &Connection, run: RunId) -> StoreResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM speakers WHERE run_id = ?1)
             OR EXISTS(SELECT 1 FROM segments WHERE run_id = ?1)",
        [run.0],
        |r| r.get(0),
    )?)
}

impl Store for SqliteStore {
    fn list_meetings(&self) -> StoreResult<Vec<MeetingSummary>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MEETING_COLS} FROM meetings ORDER BY started_at DESC, id DESC"
        ))?;
        let meetings = stmt
            .query_map([], meeting_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        meetings.into_iter().map(|m| summary(&conn, m)).collect()
    }

    fn get_meeting(&self, id: MeetingId) -> StoreResult<MeetingDetail> {
        let conn = self.lock()?;
        let meeting = get_meeting_row(&conn, id)?;
        let (current_run, speakers) = match meeting.current_run_id {
            Some(run) => (Some(get_run(&conn, run)?), speaker_views(&conn, run)?),
            None => (None, Vec::new()),
        };
        Ok(MeetingDetail {
            meeting,
            current_run,
            speakers,
        })
    }

    fn get_segments(
        &self,
        id: MeetingId,
        offset: u32,
        limit: u32,
    ) -> StoreResult<Vec<SegmentView>> {
        let conn = self.lock()?;
        meeting_exists(&conn, id)?;
        let mut stmt = conn.prepare_cached(
            "SELECT s.id, s.seq, s.start_ms, s.end_ms, s.text, own.channel, r.id,
                    r.engine_label, r.display_name, r.color, r.needs_review
             FROM meetings m
             JOIN segments s ON s.run_id = m.current_run_id
             JOIN speakers own ON own.id = s.speaker_id
             JOIN speakers r ON r.id = COALESCE(own.merged_into, own.id)
             WHERE m.id = ?1 ORDER BY s.seq LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![id.0, limit, offset], |r| {
                let label: String = r.get(7)?;
                Ok(SegmentView {
                    id: SegmentId(r.get(0)?),
                    seq: r.get(1)?,
                    start_ms: r.get(2)?,
                    end_ms: r.get(3)?,
                    text: r.get(4)?,
                    channel: parse_enum(r, 5)?,
                    speaker_id: SpeakerId(r.get(6)?),
                    speaker_name: display_name(&label, r.get(8)?),
                    speaker_color: display_color(&label, r.get(9)?),
                    needs_review: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn get_words(&self, id: MeetingId, from_ms: i64, to_ms: i64) -> StoreResult<Vec<Word>> {
        let conn = self.lock()?;
        meeting_exists(&conn, id)?;
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {WORD_COLS}
             FROM words w JOIN segments s ON s.id = w.segment_id
             JOIN meetings m ON m.id = s.meeting_id
             WHERE w.meeting_id = ?1 AND m.id = ?1 AND s.run_id = m.current_run_id
               AND w.end_ms > ?2 AND w.start_ms < ?3
             ORDER BY w.start_ms, w.id"
        ))?;
        let rows = stmt
            .query_map(params![id.0, from_ms, to_ms], word_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn search(&self, query: &str, limit: u32) -> StoreResult<Vec<SearchHit>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare_cached(
            "SELECT m.id, m.title, m.started_at, s.id, s.start_ms, r.engine_label, r.display_name,
                    snippet(segments_fts, 0, ?2, ?3, '…', ?4)
             FROM segments_fts
             JOIN segments s ON s.id = segments_fts.rowid
             JOIN meetings m ON m.id = s.meeting_id AND m.current_run_id = s.run_id
             JOIN speakers own ON own.id = s.speaker_id
             JOIN speakers r ON r.id = COALESCE(own.merged_into, own.id)
             WHERE segments_fts MATCH ?1
             ORDER BY bm25(segments_fts), m.started_at DESC, s.seq
             LIMIT ?5",
        )?;
        let rows = stmt
            .query_map(
                params![
                    fts,
                    HIT_OPEN.to_string(),
                    HIT_CLOSE.to_string(),
                    SNIPPET_TOKENS,
                    limit
                ],
                |r| {
                    let label: String = r.get(5)?;
                    let marked: String = r.get(7)?;
                    Ok(SearchHit {
                        meeting_id: MeetingId(r.get(0)?),
                        meeting_title: r.get(1)?,
                        meeting_started_at: r.get(2)?,
                        segment_id: SegmentId(r.get(3)?),
                        start_ms: r.get(4)?,
                        speaker_name: display_name(&label, r.get(6)?),
                        snippet: snippet_parts(&marked),
                    })
                },
            )
            .map_err(|e| StoreError::Invalid(format!("search: {e}")))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn list_audio_tracks(&self, id: MeetingId) -> StoreResult<Vec<AudioTrack>> {
        let conn = self.lock()?;
        meeting_exists(&conn, id)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACK_COLS} FROM audio_tracks WHERE meeting_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt
            .query_map([id.0], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn rename_speaker(
        &self,
        id: SpeakerId,
        name: &str,
        anchor_segment: Option<SegmentId>,
    ) -> StoreResult<()> {
        let conn = self.lock()?;
        let sp = get_speaker(&conn, id)?;
        let anchor = match anchor_segment {
            Some(sid) => {
                let seg = get_seg_row(&conn, sid.0)?;
                if seg.meeting_id != sp.meeting_id.0 {
                    return Err(StoreError::Invalid(format!(
                        "segment {sid} is not in the speaker's meeting"
                    )));
                }
                Some((seg.start_ms, seg.end_ms))
            }
            None => None,
        };
        let name = name.trim();
        if name.is_empty() {
            conn.execute(
                "UPDATE speakers SET display_name = NULL, name_anchor_start_ms = NULL,
                 name_anchor_end_ms = NULL WHERE id = ?1",
                [id.0],
            )?;
        } else {
            conn.execute(
                "UPDATE speakers SET display_name = ?2, name_anchor_start_ms = ?3,
                 name_anchor_end_ms = ?4 WHERE id = ?1",
                params![id.0, name, anchor.map(|a| a.0), anchor.map(|a| a.1)],
            )?;
        }
        Ok(())
    }

    fn set_speaker_color(&self, id: SpeakerId, color: &str) -> StoreResult<()> {
        if !is_hex_color(color) {
            return Err(StoreError::Invalid(format!(
                "colour must be #rrggbb, got {color:?}"
            )));
        }
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE speakers SET color = ?2 WHERE id = ?1",
            params![id.0, color.to_ascii_lowercase()],
        )?;
        not_found_if_zero(n, "speaker", id.0)
    }

    fn merge_speakers(&self, from: SpeakerId, into: SpeakerId) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let a = get_speaker(&tx, from)?;
        let b = get_speaker(&tx, into)?;
        if a.run_id != b.run_id {
            return Err(StoreError::Invalid(
                "speakers belong to different runs".into(),
            ));
        }
        let target = b.merged_into.unwrap_or(b.id);
        if target == from {
            return Err(StoreError::Invalid(format!(
                "merging speaker {from} into {into} would create a cycle"
            )));
        }
        tx.execute(
            "UPDATE speakers SET merged_into = ?2 WHERE id = ?1 OR merged_into = ?1",
            params![from.0, target.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn unmerge_speaker(&self, id: SpeakerId) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE speakers SET merged_into = NULL WHERE id = ?1",
            [id.0],
        )?;
        not_found_if_zero(n, "speaker", id.0)
    }

    fn reassign_segment(&self, segment: SegmentId, to: SpeakerId) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let seg = get_seg_row(&tx, segment.0)?;
        let target = get_speaker(&tx, to)?;
        if target.run_id.0 != seg.run_id {
            return Err(StoreError::Invalid(format!(
                "speaker {to} is not in the segment's run"
            )));
        }
        let channel = get_speaker(&tx, SpeakerId(seg.speaker_id))?.channel;
        let root = match target.merged_into {
            Some(r) => get_speaker(&tx, r)?,
            None => target,
        };
        let payload = ReassignTarget {
            to_display_name: root.display_name,
            to_engine_label: Some(root.engine_label),
        };
        tx.execute(
            "INSERT INTO edits (meeting_id, kind, channel, start_ms, end_ms, payload, created_at)
             VALUES (?1, 'reassign', ?2, ?3, ?4, ?5, ?6)",
            params![
                seg.meeting_id,
                channel.as_str(),
                seg.start_ms,
                seg.end_ms,
                to_json(&payload)?,
                now_ms()
            ],
        )?;
        tx.execute(
            "UPDATE segments SET speaker_id = ?2 WHERE id = ?1",
            params![segment.0, to.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn split_segment(&self, segment: SegmentId, at_word: WordId) -> StoreResult<SegmentId> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let seg = get_seg_row(&tx, segment.0)?;
        let words = segment_words(&tx, segment.0)?;
        let idx = words
            .iter()
            .position(|w| w.id == at_word.0)
            .ok_or_else(|| {
                StoreError::Invalid(format!("word {at_word} is not in segment {segment}"))
            })?;
        if idx == 0 {
            return Err(StoreError::Invalid(
                "cannot split before the first word".into(),
            ));
        }
        let (first, second) = words.split_at(idx);
        let ids = regroup_segment(
            &tx,
            seg,
            &[(first, seg.speaker_id), (second, seg.speaker_id)],
        )?;
        tx.commit()?;
        Ok(SegmentId(ids[1]))
    }

    fn set_meeting_title(&self, id: MeetingId, title: &str) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.0, title.trim(), now_ms()],
        )?;
        not_found_if_zero(n, "meeting", id.0)
    }

    fn set_audio_keep(&self, id: MeetingId, keep: bool) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE meetings SET audio_keep = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.0, keep, now_ms()],
        )?;
        not_found_if_zero(n, "meeting", id.0)
    }

    fn delete_meeting(&self, id: MeetingId) -> StoreResult<()> {
        let uid = {
            let mut conn = self.lock()?;
            let tx = conn.transaction()?;
            let uid = get_meeting_row(&tx, id)?.uid;
            // Segments first: `segments.speaker_id` has no ON DELETE action, so
            // deleting them explicitly keeps the cascade order irrelevant.
            tx.execute("DELETE FROM segments WHERE meeting_id = ?1", [id.0])?;
            tx.execute("DELETE FROM meetings WHERE id = ?1", [id.0])?;
            tx.commit()?;
            uid
        };
        if let Some(dir) = self.audio_dir(&uid) {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(StoreError::Backend(format!(
                        "meeting deleted but its audio folder {} was not: {e}",
                        dir.display()
                    )))
                }
            }
        }
        Ok(())
    }

    fn get_settings(&self) -> StoreResult<Settings> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let map = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Settings(map))
    }

    fn set_setting(&self, key: &str, value: &str) -> StoreResult<()> {
        if key.trim().is_empty() {
            return Err(StoreError::Invalid("setting key must not be empty".into()));
        }
        self.lock()?.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn list_model_files(&self) -> StoreResult<Vec<ModelFile>> {
        list_model_files(&*self.lock()?)
    }

    fn sync_model_files(
        &self,
        found: &[ScannedModelFile],
        now_ms: i64,
    ) -> StoreResult<Vec<ModelFile>> {
        let seen: HashSet<&str> = found.iter().map(|f| f.path.as_str()).collect();
        if seen.len() != found.len() {
            return Err(StoreError::Invalid("duplicate path in scan".into()));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        {
            let mut update = tx.prepare(
                "UPDATE model_files SET role = ?2, architecture = ?3, display_name = ?4, quant = ?5,
                    size_bytes = ?6, sha256 = ?7, source = ?8, capabilities = ?9, manifest_name = ?10,
                    last_seen_at = ?11, missing = 0
                 WHERE path = ?1",
            )?;
            let mut insert = tx.prepare(
                "INSERT INTO model_files (path, role, architecture, display_name, quant, size_bytes,
                    sha256, source, capabilities, manifest_name, first_seen_at, last_seen_at, missing)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, 0)",
            )?;
            for f in found {
                let caps = to_json(&f.capabilities)?;
                let values = params![
                    f.path,
                    f.role.as_str(),
                    f.architecture,
                    f.display_name,
                    f.quant,
                    f.size_bytes,
                    f.sha256,
                    f.source.as_str(),
                    caps,
                    f.manifest_name,
                    now_ms
                ];
                if update.execute(values)? == 0 {
                    insert.execute(values)?;
                }
            }
            let mut all = tx.prepare("SELECT id, path FROM model_files")?;
            let rows: Vec<(i64, String)> = all
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut mark = tx.prepare("UPDATE model_files SET missing = 1 WHERE id = ?1")?;
            for (id, path) in rows {
                if !seen.contains(path.as_str()) {
                    mark.execute([id])?;
                }
            }
        }
        let files = list_model_files(&tx)?;
        tx.commit()?;
        Ok(files)
    }

    fn create_meeting(&self, new: NewMeeting) -> StoreResult<Meeting> {
        let conn = self.lock()?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO meetings (uid, title, source, source_path, language, diarize_mic,
                started_at, status, audio_keep, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?9)",
            params![
                new_id(),
                new.title,
                new.source.as_str(),
                new.source_path,
                new.language,
                new.diarize_mic,
                new.started_at,
                new.status.as_str(),
                now
            ],
        )?;
        get_meeting_row(&conn, MeetingId(conn.last_insert_rowid()))
    }

    fn set_meeting_status(&self, id: MeetingId, status: MeetingStatus) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE meetings SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.0, status.as_str(), now_ms()],
        )?;
        not_found_if_zero(n, "meeting", id.0)
    }

    fn finish_meeting(&self, id: MeetingId, ended_at: i64, duration_ms: i64) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE meetings SET ended_at = ?2, duration_ms = ?3, updated_at = ?4 WHERE id = ?1",
            params![id.0, ended_at, duration_ms, now_ms()],
        )?;
        not_found_if_zero(n, "meeting", id.0)
    }

    fn add_audio_track(&self, track: NewAudioTrack) -> StoreResult<AudioTrack> {
        let conn = self.lock()?;
        meeting_exists(&conn, track.meeting_id)?;
        conn.execute(
            "INSERT INTO audio_tracks (meeting_id, channel, path, format, sample_rate, channels,
                duration_ms, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                track.meeting_id.0,
                track.channel.as_str(),
                track.path,
                track.format.as_str(),
                track.sample_rate,
                track.channels,
                track.duration_ms,
                track.size_bytes
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(conn.query_row(
            &format!("SELECT {TRACK_COLS} FROM audio_tracks WHERE id = ?1"),
            [id],
            track_from_row,
        )?)
    }

    fn create_run(
        &self,
        meeting: MeetingId,
        kind: RunKind,
        engine_info: serde_json::Value,
    ) -> StoreResult<TranscriptRun> {
        let conn = self.lock()?;
        meeting_exists(&conn, meeting)?;
        conn.execute(
            "INSERT INTO transcript_runs (meeting_id, kind, engine_info, started_at, status)
             VALUES (?1, ?2, ?3, ?4, 'running')",
            params![meeting.0, kind.as_str(), to_json(&engine_info)?, now_ms()],
        )?;
        get_run(&conn, RunId(conn.last_insert_rowid()))
    }

    fn insert_transcript(&self, run: RunId, transcript: &NewTranscript) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let meeting = get_run(&tx, run)?.meeting_id;
        if run_has_transcript(&tx, run)? {
            return Err(StoreError::Conflict(format!(
                "run {run} already has a transcript"
            )));
        }
        validate_transcript(transcript)?;
        write_transcript(&tx, run, meeting, transcript)?;
        tx.commit()?;
        Ok(())
    }

    fn finish_run(&self, run: RunId, status: RunStatus) -> StoreResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE transcript_runs SET status = ?2, finished_at = ?3 WHERE id = ?1",
            params![run.0, status.as_str(), now_ms()],
        )?;
        not_found_if_zero(n, "run", run.0)
    }

    fn set_current_run(&self, meeting: MeetingId, run: RunId) -> StoreResult<()> {
        let conn = self.lock()?;
        if get_run(&conn, run)?.meeting_id != meeting {
            return Err(StoreError::Invalid(format!(
                "run {run} does not belong to meeting {meeting}"
            )));
        }
        let n = conn.execute(
            "UPDATE meetings SET current_run_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting.0, run.0, now_ms()],
        )?;
        not_found_if_zero(n, "meeting", meeting.0)
    }
}

fn list_model_files(conn: &Connection) -> StoreResult<Vec<ModelFile>> {
    let mut stmt = conn.prepare(&format!("SELECT {MODEL_COLS} FROM model_files"))?;
    let mut files = stmt
        .query_map([], model_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by(|a, b| (a.role, &a.display_name, a.id).cmp(&(b.role, &b.display_name, b.id)));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewSegment, NewSpeaker, NewWord};
    use crate::types::Source;

    fn meeting(store: &SqliteStore, title: &str) -> Meeting {
        store
            .create_meeting(NewMeeting {
                title: title.into(),
                source: Source::File,
                source_path: None,
                language: "fr".into(),
                diarize_mic: false,
                started_at: 1_700_000_000_000,
                status: MeetingStatus::Done,
            })
            .unwrap()
    }

    /// One speaker, one segment per text, words split on spaces.
    fn transcript(texts: &[&str]) -> NewTranscript {
        let mut t = NewTranscript {
            speakers: vec![NewSpeaker {
                channel: Channel::Source,
                engine_label: "spk_0".into(),
                display_name: None,
                color: None,
                needs_review: false,
            }],
            ..Default::default()
        };
        for (i, text) in texts.iter().enumerate() {
            let start = i as i64 * 10_000;
            t.segments.push(NewSegment {
                speaker: 0,
                start_ms: start,
                end_ms: start + 5_000,
                text: (*text).into(),
            });
            for (k, w) in text.split(' ').enumerate() {
                t.words.push(NewWord {
                    segment: i,
                    start_ms: start + k as i64 * 400,
                    end_ms: start + k as i64 * 400 + 300,
                    text: w.into(),
                    confidence: None,
                    raw_label: Some("spk_0".into()),
                    margin: None,
                    overlap: false,
                });
            }
        }
        t
    }

    fn current(store: &SqliteStore, m: MeetingId, texts: &[&str]) -> RunId {
        let run = store
            .create_run(m, RunKind::File, serde_json::json!({}))
            .unwrap();
        store.insert_transcript(run.id, &transcript(texts)).unwrap();
        store.set_current_run(m, run.id).unwrap();
        run.id
    }

    fn fts_rows(store: &SqliteStore) -> (i64, i64) {
        let conn = store.lock().unwrap();
        let docs: i64 = conn
            .query_row("SELECT COUNT(*) FROM segments_fts_docsize", [], |r| {
                r.get(0)
            })
            .unwrap();
        let segs: i64 = conn
            .query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0))
            .unwrap();
        (docs, segs)
    }

    fn integrity_ok(store: &SqliteStore) {
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO segments_fts(segments_fts, rank) VALUES ('integrity-check', 1)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db").join("meet-assist.db");
        {
            let store = SqliteStore::open(&path).unwrap();
            assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
            assert_eq!(store.migrate().unwrap(), MIGRATIONS.len() as u32);
            meeting(&store, "kept");
            let mode: String = store
                .lock()
                .unwrap()
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode.to_lowercase(), "wal");
            let fk: i64 = store
                .lock()
                .unwrap()
                .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap();
            assert_eq!(fk, 1);
        }
        // Reopening applies nothing and keeps the data.
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
        assert_eq!(store.list_meetings().unwrap().len(), 1);
        assert_eq!(
            store.audio_dir("X"),
            Some(path.parent().unwrap().join("audio").join("X"))
        );
    }

    #[test]
    fn refuses_a_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(migrate(&mut conn), Err(StoreError::Backend(_))));
    }

    #[test]
    fn fts_ignores_diacritics_and_case() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = meeting(&store, "Réunion");
        current(
            &store,
            m.id,
            &["Le café est fermé aujourd'hui", "Nothing to see"],
        );
        for q in ["cafe", "CAFÉ", "ferme", "fermé café", "caf"] {
            let hits = store.search(q, 10).unwrap();
            assert_eq!(hits.len(), 1, "query {q:?}");
            let marked: Vec<&str> = hits[0]
                .snippet
                .iter()
                .filter(|p| p.hit)
                .map(|p| p.text.as_str())
                .collect();
            assert!(!marked.is_empty(), "no hit parts for {q:?}");
            let joined: String = hits[0].snippet.iter().map(|p| p.text.as_str()).collect();
            assert_eq!(joined, "Le café est fermé aujourd'hui");
        }
        assert!(store.search("\"", 10).unwrap().is_empty());
        assert!(store.search("cafe zzz", 10).unwrap().is_empty());
        // A quote inside a term cannot break the query syntax.
        assert!(store.search("caf\"e", 10).unwrap().is_empty());
    }

    #[test]
    fn delete_leaves_no_fts_orphans_and_removes_audio() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("meet-assist.db")).unwrap();
        let a = meeting(&store, "A");
        let b = meeting(&store, "B");
        current(&store, a.id, &["alpha one", "alpha two"]);
        let run_b = current(&store, b.id, &["beta one"]);
        // A second, non-current run on B.
        let run2 = store
            .create_run(b.id, RunKind::Rediarize, serde_json::json!({}))
            .unwrap();
        store
            .insert_transcript(run2.id, &transcript(&["beta two", "beta three"]))
            .unwrap();
        let audio = store.audio_dir(&a.uid).unwrap();
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(audio.join("source.wav"), b"x").unwrap();
        assert_eq!(fts_rows(&store), (5, 5));

        store.delete_meeting(a.id).unwrap();
        assert!(!audio.exists());
        assert_eq!(fts_rows(&store), (3, 3));
        integrity_ok(&store);
        assert!(store.search("alpha", 10).unwrap().is_empty());

        store.delete_run(run2.id).unwrap();
        assert_eq!(fts_rows(&store), (1, 1));
        store.delete_meeting(b.id).unwrap();
        assert_eq!(fts_rows(&store), (0, 0));
        integrity_ok(&store);
        let conn = store.lock().unwrap();
        for table in ["words", "speakers", "transcript_runs", "edits"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} not empty");
        }
        let _ = run_b;
    }

    #[test]
    fn split_reindexes_fts() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = meeting(&store, "Split");
        current(&store, m.id, &["red green blue"]);
        let seg = store.get_segments(m.id, 0, 10).unwrap()[0].id;
        let words = store.get_words(m.id, 0, 100_000).unwrap();
        store.split_segment(seg, words[2].id).unwrap();
        let hits = store.search("blue", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_ne!(hits[0].segment_id, seg);
        assert_eq!(store.search("red", 10).unwrap()[0].segment_id, seg);
        integrity_ok(&store);
    }

    #[test]
    fn backup_writes_a_readable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("a.db")).unwrap();
        let m = meeting(&store, "Backed up");
        current(&store, m.id, &["hello world"]);
        let dest = dir.path().join("backup.db");
        store.backup(&dest).unwrap();
        let copy = SqliteStore::open(&dest).unwrap();
        assert_eq!(copy.list_meetings().unwrap()[0].title, "Backed up");
        assert_eq!(copy.search("hello", 5).unwrap().len(), 1);
    }

    #[test]
    fn runs_anchors_and_edits_helpers() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = meeting(&store, "Helpers");
        let run = current(&store, m.id, &["a b c"]);
        assert_eq!(store.list_runs(m.id).unwrap().len(), 1);
        assert_eq!(store.get_run(run).unwrap().kind, RunKind::File);
        assert_eq!(store.meeting_id_by_uid(&m.uid).unwrap(), Some(m.id));
        assert_eq!(store.meeting_id_by_uid("nope").unwrap(), None);
        store
            .add_clock_anchors(&[
                ClockAnchor {
                    meeting_id: m.id,
                    channel: Channel::Mic,
                    sample_index: 16_000,
                    qpc_100ns: 2,
                },
                ClockAnchor {
                    meeting_id: m.id,
                    channel: Channel::Mic,
                    sample_index: 0,
                    qpc_100ns: 1,
                },
            ])
            .unwrap();
        let anchors = store.clock_anchors(m.id, Channel::Mic).unwrap();
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].sample_index, 0);
        let speakers = store.run_speakers(run).unwrap();
        let seg = store.get_segments(m.id, 0, 1).unwrap()[0].id;
        store.reassign_segment(seg, speakers[0].id).unwrap();
        let edits = store.list_edits(m.id).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].payload.to_engine_label.as_deref(), Some("spk_0"));
        assert_eq!(edits[0].channel, Channel::Source);
    }
}
