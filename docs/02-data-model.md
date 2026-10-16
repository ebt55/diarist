# Data model (SQLite)

Status: v0.3, 15 Oct 2026, after the architecture review (research/05) and adjudication (research/06). One SQLite file at `<data root>\diarist.db`, WAL mode, foreign keys on. Backups via the rusqlite backup API or `VACUUM INTO` a temp file followed by flush and rename; never by copying the live file.

## Principles

- The DB is the product. Everything the UI shows is a query.
- **Runs.** A meeting can have several transcripts over time: the live preview, the canonical post-pass, a later re-diarization or a new fusion algorithm. Each is a `transcript_run`; `meetings.current_run_id` says which one the UI shows. Nothing is ever deleted to make room for a better run.
- INTEGER primary keys everywhere (rowid aliases), so FTS5 external content stays stable across VACUUM and joins stay fast at tens of millions of words. The only ULID is `meetings.uid`, used for folder names and exports.
- The segment owns the speaker. Words keep the engine's raw label and, from v1.5, the fusion margin and overlap flag, so a smoothing pass can be compared with what the engine said.
- User edits survive new runs: display names and colours are carried over by time-overlap matching; segment reassignments are stored as time-ranged edits and re-applied.
- Audio is disposable, transcripts are not. Per-channel 16 kHz WAVs during recording; one stereo FLAC (left mic, right loopback) after the post-pass, WAVs deleted; retention default 30 days unless a meeting is pinned (`audio_keep`). Eval-set meetings are pinned by the harness. A later re-diarization or fusion re-run only reaches meetings whose audio still exists, and Settings says so. Imports keep the original file as the playback source.
- Schema migrates by numbered SQL files applied at startup (`PRAGMA user_version`).
- **Ids are never reused.** A rowid table hands out `max(id) + 1`, so deleting the newest meeting gave its id to the next one and anything that remembered the old meeting by id saw the new one. Since migration 0002, meeting and transcript run ids come from the `counters` table (`next_meeting_id`, `next_run_id`, started at `max(id) + 1` of a 0.1.0-beta.1 database) and advance in the same transaction as the insert. Speakers, segments and words still take `max(id) + 1`; they belong to one run and the UI reloads them with it.
- **A copy before every upgrade.** Before pending migrations run on an existing database, the backup API copies it to `diarist.db.bak-<old schema version>` next to it; only the newest copy is kept, and the open stops if the copy cannot be made. Copy diagnostics names it.

## Tables

```sql
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

CREATE TABLE counters (key TEXT PRIMARY KEY, value INTEGER NOT NULL);  -- 0002: next_meeting_id, next_run_id
```

## Run lifecycle

- **File import.** One `file` run. On success it becomes `current_run_id`.
- **Live.** A `live` run receives `live_utterances` while recording. On stop, the latest `rev` of each `utt` is materialised into segments and words of the live run (so a crash of the post-pass still leaves a readable transcript), then a `post_pass` run is created and, on success, becomes current. The live run is kept until the post-pass is done, then deleted unless `settings.keep_live_runs = 1`.
- **Re-runs** (`rediarize`, `refuse`): new run, carry-over, becomes current only when the user accepts it in the UI (or always, from the CLI with a flag). Old runs stay for comparison and for the eval harness.

## Carry-over from the previous current run

1. Build the overlap matrix between old speakers and new speakers on the same channel: seconds where a word of old speaker A and a word of new speaker X overlap in time. Old speakers with under 5 s of speech are ignored so spurious live labels cannot claim names.
2. Greedy one-to-one matching, highest overlap first. A match is accepted when it covers at least 60 percent of the smaller speaker's speech time. Copy `color` and `global_id` to the match.
3. Names follow the anchor, not the overlap: when an old speaker has `name_anchor_*` (the segment the user renamed from), the name goes to the new speaker who owns most words inside that range. If that speaker differs from the overlap winner, both get `needs_review = 1`. Old speakers without an anchor (named through a merge or from an earlier run) pass the name to the overlap winner. This handles the case where the live diarizer conflated two people and the user named one of them: the post-pass split gives the name to the person who was actually talking where the user pressed rename.
4. New speakers left unmatched get `needs_review = 1`; the UI shows them with a "new label" badge. (A post-pass that splits a live label into two usually means two people were conflated.)
5. Merges are not carried structurally; matched speakers inherit the merged root's name, so two new labels may share a name, and the UI offers "merge again" in one click.
6. `edits` of kind `reassign` are replayed in `created_at` order at word level: new words whose midpoints fall inside the edit's range belong to the edit's target. If that covers a whole segment, the segment is reassigned; otherwise the segment is split at the word boundary and the covered part reassigned. The target is resolved by display name against the new run after step 2; if no speaker has that name, a speaker row `user_N` with `needs_review = 1` is created rather than dropping the edit.

## Speaker rename, merge, reassign

- Rename: update `display_name` and set `name_anchor_start_ms/end_ms` to the segment the rename was invoked from (`rename_speaker` with a segment), or to an explicit time range while recording, when no segment exists yet (`rename_speaker_at` with the live utterance's range). NULL when renamed from the legend. Anchored names carry over even for speakers with under 5 s of speech; the 5 s rule applies only to unanchored overlap matches. Colour: update the row.
- Merge A into B: set `A.merged_into = root(B)`, and repoint any speaker whose `merged_into = A` to `root(B)` so chains stay one level deep; reject cycles. Queries resolve `COALESCE(merged_into, id)`.
- Reassign a segment: update `segments.speaker_id`, write an `edits` row. To reassign part of a segment, split it at a word boundary first (segments and words are updated in one transaction).

## Queries the UI needs

- Meeting list with duration, speaker count, first 200 chars of the current run.
- Segments of the current run ordered by `seq`, with resolved speaker name and colour, paged for virtualization.
- FTS: join `segments_fts` to `segments` and `meetings`, filter `segments.run_id = meetings.current_run_id`, order by `bm25`, `snippet()`.
- Words in a time window of the current run (click-to-seek, eval export).
- Audio for playback: the `playback` track for live meetings, the `original` for imports.
