# Data model (SQLite)

Status: draft v0.1. Runtime-agnostic. One SQLite file per user profile at `%LOCALAPPDATA%\meet-assist\meet-assist.db`, WAL mode, foreign keys on.

## Principles

- The DB is the product. Everything the UI shows is a query; nothing lives only in memory except the live preview buffer.
- Words are the atomic unit. Segments are derived (same speaker, contiguous) but stored, because the UI and FTS work on segments.
- Audio is kept, per channel, as 16 kHz mono FLAC next to the DB (`%LOCALAPPDATA%\meet-assist\audio\<meeting_id>\mic.flac`, `loopback.flac`, or `source.flac` for files). Re-diarization and the eval harness need it. User can disable retention or purge per meeting.
- Speaker labels are per meeting. A separate table links them to a global identity (v3, embeddings).
- Schema is migrated by numbered SQL files applied at startup (`user_version` pragma).

## Tables

```sql
CREATE TABLE meetings (
  id            TEXT PRIMARY KEY,            -- ulid
  title         TEXT NOT NULL DEFAULT '',
  source        TEXT NOT NULL CHECK (source IN ('live','file')),
  source_path   TEXT,                        -- original file path for 'file'
  started_at    INTEGER NOT NULL,            -- unix ms, wall clock
  ended_at      INTEGER,
  duration_ms   INTEGER,
  status        TEXT NOT NULL CHECK (status IN ('recording','processing','done','failed')),
  engine_info   TEXT,                        -- JSON: model names, versions, runtime version
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE audio_tracks (
  id            TEXT PRIMARY KEY,
  meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  channel       TEXT NOT NULL CHECK (channel IN ('mic','loopback','source')),
  path          TEXT NOT NULL,               -- relative to audio root
  sample_rate   INTEGER NOT NULL,
  duration_ms   INTEGER,
  offset_ms     INTEGER NOT NULL DEFAULT 0   -- alignment offset vs meeting t=0
);

CREATE TABLE speakers (
  id            TEXT PRIMARY KEY,
  meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  engine_label  TEXT NOT NULL,               -- 'spk_0'..'spk_7' from diarizer, or 'me' for the mic channel
  channel       TEXT NOT NULL CHECK (channel IN ('mic','loopback','source')),
  display_name  TEXT,                        -- user-set; NULL means show "Speaker N"
  color         TEXT,                        -- hex, assigned deterministically, user-overridable
  global_id     TEXT REFERENCES global_speakers(id) ON DELETE SET NULL,
  merged_into   TEXT REFERENCES speakers(id) ON DELETE SET NULL,  -- non-null when user merged this label into another
  UNIQUE (meeting_id, channel, engine_label)
);

CREATE TABLE segments (
  id            TEXT PRIMARY KEY,
  meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  speaker_id    TEXT NOT NULL REFERENCES speakers(id),
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  text          TEXT NOT NULL,
  seq           INTEGER NOT NULL             -- display order within meeting
);
CREATE INDEX segments_meeting_start ON segments(meeting_id, start_ms);

CREATE TABLE words (
  id            INTEGER PRIMARY KEY,
  meeting_id    TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  segment_id    TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
  speaker_id    TEXT NOT NULL REFERENCES speakers(id),
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  text          TEXT NOT NULL,
  confidence    REAL
);
CREATE INDEX words_segment ON words(segment_id);

-- Full-text search over segments (external content table)
CREATE VIRTUAL TABLE segments_fts USING fts5(
  text, content='segments', content_rowid='rowid',
  tokenize='porter unicode61'
);
-- triggers keep segments_fts in sync (insert/update/delete)

CREATE TABLE global_speakers (           -- v3: cross-meeting identity
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);
CREATE TABLE speaker_embeddings (        -- v3
  id            INTEGER PRIMARY KEY,
  global_id     TEXT REFERENCES global_speakers(id) ON DELETE CASCADE,
  speaker_id    TEXT REFERENCES speakers(id) ON DELETE CASCADE,
  model         TEXT NOT NULL,
  dim           INTEGER NOT NULL,
  vector        BLOB NOT NULL               -- float32 little-endian; sqlite-vec later if needed
);

CREATE TABLE live_checkpoints (          -- crash safety during live mode
  meeting_id    TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
  payload       TEXT NOT NULL,             -- JSON of unfinalized segments/words
  updated_at    INTEGER NOT NULL
);

CREATE TABLE models (                    -- what is installed in the models folder
  name          TEXT PRIMARY KEY,          -- e.g. 'parakeet-tdt-0.6b-v3-q8'
  kind          TEXT NOT NULL,             -- asr_offline | asr_streaming | diarizer | embedder
  version       TEXT NOT NULL,
  path          TEXT NOT NULL,
  sha256        TEXT NOT NULL,
  size_bytes    INTEGER NOT NULL,
  downloaded_at INTEGER NOT NULL
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

## Live-mode write policy

- Engine emits partial words and provisional speaker labels. These live in a Rust-side buffer and are mirrored to `live_checkpoints` every 2 seconds.
- When the engine marks a region final (label stable, no more revisions), its words and segments are inserted in one transaction and removed from the checkpoint.
- On restart after a crash, the checkpoint is replayed into `segments`/`words` as-is, marked in `engine_info` as "recovered".

## Speaker rename and merge

- Rename: update `speakers.display_name`. Nothing else moves.
- Merge A into B: set `speakers.merged_into = B` on A; queries resolve `COALESCE(merged_into, id)`. Undoable by clearing the field. No rewriting of `words`.
- Reassign one segment to a different speaker: update `segments.speaker_id` and its `words.speaker_id`.

## Queries the UI needs (and must be fast)

- Meeting list with duration, speaker count, first 200 chars.
- Segments for a meeting, ordered by `seq`, with resolved speaker name/color, paged by range for virtualization.
- FTS: `SELECT ... FROM segments_fts WHERE segments_fts MATCH ? ORDER BY bm25(segments_fts)` with `snippet()`; joined to meetings for grouping.
- Words in a time window (for click-to-seek and for the eval export).
