-- 0001_init: the schema of docs/02-data-model.md v0.3.
-- Applied once by `SqliteStore` (PRAGMA user_version 0 -> 1) inside a transaction.

CREATE TABLE meetings (
  id              INTEGER PRIMARY KEY,
  uid             TEXT NOT NULL UNIQUE,         -- ulid for folder names and exports
  title           TEXT NOT NULL DEFAULT '',
  source          TEXT NOT NULL CHECK (source IN ('live','file')),
  source_path     TEXT,                          -- original file for 'file'
  language        TEXT NOT NULL DEFAULT 'en',    -- BCP-47 or 'auto'
  diarize_mic     INTEGER NOT NULL DEFAULT 0,    -- live: 1 when the mic hears a room with several people
  started_at      INTEGER NOT NULL,              -- unix ms
  ended_at        INTEGER,
  duration_ms     INTEGER,
  status          TEXT NOT NULL CHECK (status IN ('recording','processing','done','failed')),
  current_run_id  INTEGER REFERENCES transcript_runs(id) ON DELETE SET NULL,
  audio_keep      INTEGER NOT NULL DEFAULT 0,    -- 1 = pinned, exempt from retention
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE TABLE transcript_runs (
  id            INTEGER PRIMARY KEY,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  kind          TEXT NOT NULL CHECK (kind IN ('live','file','post_pass','rediarize','refuse')),
  engine_info   TEXT NOT NULL,                   -- JSON: runtime version, model names and hashes, presets, fusion settings
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  status        TEXT NOT NULL CHECK (status IN ('running','done','failed'))
);
CREATE INDEX runs_meeting ON transcript_runs(meeting_id);

CREATE TABLE audio_tracks (
  id            INTEGER PRIMARY KEY,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  channel       TEXT NOT NULL CHECK (channel IN ('mic','loopback','source','playback')),
  path          TEXT NOT NULL,                   -- relative to the data root, or absolute for 'original'
  format        TEXT NOT NULL CHECK (format IN ('wav','flac','original')),
  sample_rate   INTEGER NOT NULL,
  channels      INTEGER NOT NULL DEFAULT 1,
  duration_ms   INTEGER,
  size_bytes    INTEGER
);
CREATE INDEX tracks_meeting ON audio_tracks(meeting_id);

-- Live alignment: one (sample index, QPC) anchor per channel per second.
-- Word times are mapped piecewise-linearly between anchors when channels are interleaved.
CREATE TABLE clock_anchors (
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  channel       TEXT NOT NULL,
  sample_index  INTEGER NOT NULL,
  qpc_100ns     INTEGER NOT NULL,
  PRIMARY KEY (meeting_id, channel, sample_index)
) WITHOUT ROWID;

CREATE TABLE speakers (
  id            INTEGER PRIMARY KEY,
  run_id        INTEGER NOT NULL REFERENCES transcript_runs(id) ON DELETE CASCADE,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  channel       TEXT NOT NULL CHECK (channel IN ('mic','loopback','source')),
  engine_label  TEXT NOT NULL,                   -- 'spk_0'.., 'me', 'spk_unknown'
  display_name  TEXT,                            -- NULL shows "Speaker N"
  color         TEXT,                            -- hex
  global_id     INTEGER REFERENCES global_speakers(id) ON DELETE SET NULL,
  merged_into   INTEGER REFERENCES speakers(id) ON DELETE SET NULL,  -- always points at a root; chains are flattened on merge
  needs_review  INTEGER NOT NULL DEFAULT 0,      -- 1 when carry-over from a previous run was ambiguous
  name_anchor_start_ms INTEGER,                  -- time range of the segment the user renamed from (set on rename)
  name_anchor_end_ms   INTEGER,                  -- used by carry-over to give the name to whoever speaks there in the new run
  UNIQUE (run_id, channel, engine_label)
);
CREATE INDEX speakers_meeting ON speakers(meeting_id);
CREATE INDEX speakers_merged ON speakers(merged_into);

CREATE TABLE segments (
  id            INTEGER PRIMARY KEY,
  run_id        INTEGER NOT NULL REFERENCES transcript_runs(id) ON DELETE CASCADE,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  speaker_id    INTEGER NOT NULL REFERENCES speakers(id),
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  text          TEXT NOT NULL,
  seq           INTEGER NOT NULL
);
CREATE INDEX segments_run_seq ON segments(run_id, seq);
CREATE INDEX segments_meeting_start ON segments(meeting_id, start_ms);
CREATE INDEX segments_speaker ON segments(speaker_id);

CREATE TABLE words (
  id            INTEGER PRIMARY KEY,
  segment_id    INTEGER NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  text          TEXT NOT NULL,
  confidence    REAL,
  raw_label     TEXT,                            -- engine's speaker label before smoothing or edits
  margin        REAL,                            -- v1.5: top speaker score minus runner-up
  overlap       INTEGER NOT NULL DEFAULT 0       -- v1.5: 1 when another speaker was active
);
CREATE INDEX words_segment ON words(segment_id);
CREATE INDEX words_meeting_start ON words(meeting_id, start_ms);

-- Live preview: append-only, one row per utterance revision. Materialised into
-- segments/words when recording stops or on crash recovery. Replaces any JSON checkpoint blob.
CREATE TABLE live_utterances (
  id            INTEGER PRIMARY KEY,
  run_id        INTEGER NOT NULL REFERENCES transcript_runs(id) ON DELETE CASCADE,
  channel       TEXT NOT NULL,
  utt           INTEGER NOT NULL,
  rev           INTEGER NOT NULL,
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  raw_label     TEXT,
  words_json    TEXT NOT NULL,                   -- [{t0,t1,text,conf,spk}]
  UNIQUE (run_id, channel, utt, rev)
);

-- Time-ranged user edits that must survive a new run.
CREATE TABLE edits (
  id            INTEGER PRIMARY KEY,
  meeting_id    INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  kind          TEXT NOT NULL CHECK (kind IN ('reassign')),
  channel       TEXT NOT NULL,
  start_ms      INTEGER NOT NULL,
  end_ms        INTEGER NOT NULL,
  payload       TEXT NOT NULL,                   -- JSON: {"to_display_name": "...", "to_engine_label": "..."}
  created_at    INTEGER NOT NULL
);
CREATE INDEX edits_meeting ON edits(meeting_id);

-- Full-text search over segments. content_rowid is the INTEGER PRIMARY KEY, stable across VACUUM.
CREATE VIRTUAL TABLE segments_fts USING fts5(
  text, content='segments', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER segments_ai AFTER INSERT ON segments BEGIN
  INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER segments_ad AFTER DELETE ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER segments_au AFTER UPDATE OF text ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE global_speakers (                   -- v3: cross-meeting identity
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);
CREATE TABLE speaker_embeddings (                -- v3
  id            INTEGER PRIMARY KEY,
  global_id     INTEGER REFERENCES global_speakers(id) ON DELETE CASCADE,
  speaker_id    INTEGER REFERENCES speakers(id) ON DELETE CASCADE,
  model         TEXT NOT NULL,
  dim           INTEGER NOT NULL,
  vector        BLOB NOT NULL                    -- float32 LE
);

-- Everything the scanner found under models\ and engine\ (ComfyUI-style drop-in).
CREATE TABLE model_files (
  id            INTEGER PRIMARY KEY,
  path          TEXT NOT NULL UNIQUE,            -- relative to the data root
  role          TEXT NOT NULL CHECK (role IN ('asr','diarizer','embedder','punctuation','llm','engine','unknown')),
  architecture  TEXT,                            -- GGUF general.architecture, or runtime version for engines
  display_name  TEXT NOT NULL,                   -- GGUF general.name, or file stem
  quant         TEXT,                            -- e.g. Q8_0
  size_bytes    INTEGER NOT NULL,
  sha256        TEXT,                            -- computed lazily or taken from the manifest / .verified sidecar
  source        TEXT NOT NULL CHECK (source IN ('manifest','pull','user')),
  capabilities  TEXT NOT NULL,                   -- JSON: {streaming, diarize, languages[], vram_mb_measured?, vram_mb_estimate?}
  manifest_name TEXT,                            -- set when the file matches a manifests/models.json entry
  first_seen_at INTEGER NOT NULL,
  last_seen_at  INTEGER NOT NULL,
  missing       INTEGER NOT NULL DEFAULT 0       -- 1 when the file disappeared since the last scan
);
CREATE INDEX model_files_role ON model_files(role);

-- Selection per role lives in settings: selected_model.asr, selected_model.diarizer, selected_engine.

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
