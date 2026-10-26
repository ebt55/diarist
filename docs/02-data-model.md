# Data model (SQLite)

Status: v0.3, 15 Oct 2026, after the architecture review (research/05) and adjudication (research/06); v0.4, 26 Oct 2026: `words.raw_text` (migration 0004), punctuation rebuilds and the identity carry (plan 16, chunk A). One SQLite file at `<data root>\diarist.db`, WAL mode, foreign keys on. Backups via the rusqlite backup API or `VACUUM INTO` a temp file followed by flush and rename; never by copying the live file.

## Principles

- The DB is the product. Everything the UI shows is a query.
- **Runs.** A meeting can have several transcripts over time: the live preview, the canonical post-pass, a later re-diarization or a new fusion algorithm. Each is a `transcript_run`; `meetings.current_run_id` says which one the UI shows. Nothing is ever deleted to make room for a better run.
- INTEGER primary keys everywhere (rowid aliases), so FTS5 external content stays stable across VACUUM and joins stay fast at tens of millions of words. The only ULID is `meetings.uid`, used for folder names and exports.
- The segment owns the speaker. Words keep the engine's raw label and, from v1.5, the fusion margin and overlap flag, so a smoothing pass can be compared with what the engine said.
- User edits survive new runs: display names and colours are carried over by time-overlap matching; segment reassignments are stored as time-ranged edits and re-applied.
- Audio is disposable, transcripts are not. Per-channel 16 kHz WAVs during recording; one stereo FLAC (left mic, right loopback) after the post-pass, WAVs deleted; retention default 30 days unless a meeting is pinned (`audio_keep`). Eval-set meetings are pinned by the harness. A later re-diarization or fusion re-run only reaches meetings whose audio still exists, and Settings says so. Imports keep the original file as the playback source.
- **Gain tracks.** A channel recorded with automatic gain (the loopback by default, the mic with `capture.mic_agc`) also writes `<channel>.gain` beside its WAV: the gain in dB the AGC applied to every 10 ms of the timeline, a 16-byte header (`DGTK`, version 1, 10 values per record, 16,000 Hz, 160 samples per value) and one record of ten little-endian `f32` per 100 ms frame, appended and flushed every second with the WAV. A crash leaves whole records plus at most a partial one, which the reader drops, so nothing needs repairing; 1.44 MB per hour. The post-pass echo canceller divides the loopback by it to get the reference the speakers played (`crates/core/src/audio/gain_track.rs`, `audio/aec/offline.rs`). It is not an `audio_tracks` row: it stays in the meeting folder as long as the FLAC, goes with the folder when the meeting is deleted, and audio retention deletes it with the FLAC. The post-pass's echo-cancelled mic, `mic-aec.wav`, exists only while the mic is transcribed and is deleted right after (and with the WAVs).
- Schema migrates by numbered SQL files applied at startup (`PRAGMA user_version`).
- **Ids are never reused.** A rowid table hands out `max(id) + 1`, so deleting the newest meeting gave its id to the next one and anything that remembered the old meeting by id saw the new one. Since migration 0002, meeting and transcript run ids come from the `counters` table (`next_meeting_id`, `next_run_id`, started at `max(id) + 1` of a 0.1.0-beta.1 database) and advance in the same transaction as the insert. Speakers, segments and words still take `max(id) + 1`; they belong to one run and the UI reloads them with it.
- **A copy before every upgrade.** Before pending migrations run on an existing database, the backup API copies it to `diarist.db.bak-<old schema version>` next to it; only the newest copy is kept, and the copy of the newest release's schema (`RELEASED_SCHEMA`, 3 for 0.1.0-beta.4, raised when a release ships), so builds that migrate one step at a time (3 to 4, then 4 to 5) keep `bak-3` beside `bak-4`. The open stops if the copy cannot be made. Copy diagnostics names it. The first 0.1.0-beta.5 start leaves `diarist.db.bak-3` (schema 3 to 5); a beta.4 build refuses a newer database, so going back to beta.4 means restoring that copy.
- **Display text and the ASR's text.** Punctuation (plan 16) is applied by Diarist to the finished words and never changes a word, its times or its speaker. `words.text` holds what the UI shows (punctuated, sentence-start capitals), because splitting, reassigning and edit replay rebuild `segments.text` from `words.text`; `words.raw_text` holds the speech model's text where it differs, NULL where it is the same. Every rebuild starts from `COALESCE(raw_text, text)`. What ran, or why not, is the run's `engine_info.punctuation`: `{applied, reason, model, revision, sha256, runtime: {name, version, path}, language, language_source, unit, policy, asr_marks_per_100: {<channel>: n}, channels: {<channel>: reason}, words, marks_added, case_upgrades, rejected_words, inner_marks_dropped, streams_refused, wall_s, threads, rebuilt_from}`, replaced wholesale on every build; `reason` is `ok`, `off`, `no_model`, `language`, `asr_punctuates`, `runtime`, `error` (with `message`) or `invariant`. `runtime.path` is relative to the data root.

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
  status        TEXT NOT NULL CHECK (status IN ('running','done','failed')),
  echo_dropped  TEXT                             -- 0003, JSON [{start_ms, end_ms, text, coefficient}]: mic words the echo gate removed (NULL: the gate never ran; [] on a run built by Restore)
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
  size_bytes    INTEGER,
  channel_frames TEXT                            -- JSON [mic, loopback]: sample frames of each channel before the shorter was padded (0005; NULL before beta.5)
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
  overlap       INTEGER NOT NULL DEFAULT 0,      -- v1.5: 1 when another speaker was active
  raw_text      TEXT                             -- 0004: the ASR's text where `text` (display, punctuated) differs; NULL = same as text
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
- **Live.** A `live` run receives `live_utterances` while recording. On stop, the latest `rev` of each `utt` is materialised into segments and words of the live run (so a crash of the post-pass still leaves a readable transcript), then a `post_pass` run is created and, on success, becomes current. The live run is kept until the post-pass is done, then deleted unless `settings.keep_live_runs = 1` or a display name of the live run reached no speaker of the new run (the post-pass run's `engine_info.unplaced_names` counts them; crash recovery honours it too).
- **Process again** (plan 16 section 12.5 J): the post-pass of a live meeting runs once more, from the WAVs a failed or cancelled post-pass left or, on a finished meeting, from its registered `playback.flac`. It needs the live run (`keep_live_runs`, or a live name the first post-pass could not place, keeps it) and adds a `post_pass` run that becomes current; every earlier run stays, from the FLAC the live run too whatever `keep_live_runs` says. From the FLAC, the job first writes `rerun-from-flac.json` beside it (flushed to the disk; the job's id, then its new run's id), decodes `mic.wav` and `loopback.wav` beside `loopback.gain` through `.part` files, each channel cut back to its length in `audio_tracks.channel_frames` (tracks from before beta.5 have none, and both channels get the FLAC's length), and when it ends, whatever happens, deletes the files it made and then the marker. It is refused while `mic.wav`, `loopback.wav` or a marker are beside the FLAC (it never writes over or deletes WAVs it did not make) and when the disk has no room for the decoded audio (about 115 MB per channel-hour, three channels with the canceller's `mic-aec.wav`, plus 64 MB). `playback.flac` is the only copy of the audio and is only read: a post-pass from the FLAC never encodes it or touches its row, and a post-pass from the WAVs encodes it only when its file is gone (`NotFound`; it then replaces the row) and fails, the WAVs kept, when the file cannot be looked at or does not hold what the WAVs hold; a decode that is short, fails a frame or the file's MD5 is refused; everything that can fail comes before the new run becomes current, so a run that fails or is cancelled leaves the meeting `done` with the transcript it had (any post-pass that fails deletes the run it made unless that run became current). Crash recovery finishes a meeting with a registered FLAC or a marker (`processing`, or `done` / `failed` with such files left beside the FLAC) without touching the FLAC: `mic.wav` and `loopback.wav` go only once the FLAC has its row's `size_bytes`, decodes in full and holds what they hold; the `.part` files and `mic-aec.wav` go; the marker's run goes unless it is current, with any unfinished `post_pass` run, and no other run; the marker goes last. When it cannot tell (the FLAC cannot be looked at for a reason other than `NotFound`, the marker is there and the FLAC is not, the WAVs are not copies), it changes nothing but the status, `failed`, and says why. Names carry over from the live run (or a current Restore or punctuation copy of it), not from a post-pass run, so names given on the current post-pass run stay on that earlier run only. A track row with `channel_frames` NULL (registered before beta.5) decodes both channels at the FLAC's length: on a recording crash recovery finished with channels of different lengths, the last word's end clamp and `engine_info.echo` (the echo scan and the canceller then see the padding) can differ from the stored run's.
- **Re-runs** (`rediarize`, `refuse`): new run, carry-over, becomes current only when the user accepts it in the UI (or always, from the CLI with a flag). Old runs stay for comparison and for the eval harness.
- **Punctuation** (plan 16 section 3.2). The post-pass and imports punctuate the words after the echo gate and before segmentation, with the selected punctuation model of the caller's data paths. With `post_pass_mode = on_issues` and a clean recording, a punctuator that changes the text of at least one word turns the promotion of the live run into a `post_pass` run built from the live run's words (raw text, each word under its own speaker row, see below), punctuated, with `engine_info.promoted_live` (the live run's id), the live run's `engine_info.echo` and `echo_dropped`, speakers carried by identity; it becomes current and the live run goes as after a transcribing post-pass. Without a punctuator, or when no word changed (a language the model does not cover, a failure), the live run is promoted as before.
- **Rebuilds** ("Add punctuation", "Remove punctuation", the bulk action, `diarist-cli punctuate`): the current run's words, back to the ASR's text, punctuated (or not) and segmented with the run's own options into a new run of the same kind, with `engine_info` copied, `punctuation` replaced and `punctuation.rebuilt_from` naming the root run (followed through `rebuilt_from` and `echo.restored_from`, so Add, Remove, Add never chain copies of copies), the current run's `echo_dropped` copied, speakers carried by identity; it becomes current only while the run it was built from is still current and the meeting is `done` or `failed` (`RunStore::set_current_run_if`, one compare-and-set): otherwise the new run, which nobody has seen, is deleted and the caller gets "This meeting's transcript changed while this ran ... Try again." (`punct::rebuild::MEETING_CHANGED`). Nothing else is deleted. Restore works the same way (the removed words added on the mic as `me`, punctuated again) and still replaces an earlier restored copy of the same run, after the new run is current; a punctuation copy of a restored run (which inherits `echo.restored_from`) is never deleted. A run a crash left half-built (still `running`, not current, with `rebuilt_from` or `restored_from`, of a `done` or `failed` meeting) is deleted at the next launch (`punct::rebuild::sweep_interrupted_rebuilds`, run with the import recovery). Copies (`rebuilt_from` or `restored_from`) are never the live run of a re-run, crash recovery or a snapshot; a re-run of the post-pass carries names over from the current run when it is such a copy of the live run, and crash recovery keeps such a copy current while the post-pass waits.

## Carry-over from the previous current run

First the two runs are lined up per channel, because their word times need not agree: the post-pass after Stop runs the file lookahead, whose word starts come 80 to 190 ms earlier than the live run's (docs/research/12, "Word times"). Carry-over searches shifts within Â±500 ms in 10 ms steps for the one at which the old run's words overlap the new run's most (on a tie, the smallest), and applies it to the old run's word times, name anchors and edit ranges before the rules below; runs whose words already line up keep shift 0 and are handled as before. The shift per channel is in the report (`CarryOverReport::shift_ms`). Known limit: after alignment, edits and anchors still match by word midpoint, so a word shorter than twice its leftover offset from the session's typical shift (for example a 150 ms word more than 75 ms off) can fall outside an edit or anchor range. Untested, for beta.3: an edit made on a post-pass result and replayed again from the live run (re-running a post-pass after editing) is off by the shift, because edits are assumed to be in the old run's time.

1. Build the overlap matrix between old speakers and new speakers on the same channel: time where a speech region of old speaker A and one of new speaker X overlap. Regions, not words: both engines emit words as short spikes (median 80 ms, one encoder frame) and the post-pass places each word a little differently from the live run (median 160 ms earlier, 400 ms earlier to 0 at p10 / p90), so after the alignment only about half of the word time overlaps even when both runs agree on every speaker; that lost the names given during two recorded meetings. A region is built in channel order: consecutive words of one speaker at most 150 ms apart (`JOIN_GAP_MS`), ended by any word of another speaker so it never swallows a backchannel, then widened by 100 ms on both sides (`REGION_PAD_MS`) but never by more than half the gap to the nearest word of another speaker. `spk_unknown` rows bound other speakers' regions but take no part in the matching. Speakers, old or new, with under 10 s of region time are not matched, so spurious live labels cannot claim names and a tiny new label inside a long live speaker cannot take its name (region time is about 2.1 times word time on recorded speech, so this keeps the earlier 5 s of word time). The constants were the smallest pair, of joins 150 / 300 / 500 ms and pads 0 / 100 / 200 ms, that matched every speaker of two replayed meetings at 75 percent or more and kept every wrong pair of 200 seeded adversarial meetings under 40 percent.
2. Greedy one-to-one matching, highest overlap first. A match is accepted when it covers at least 60 percent of the smaller speaker's region time. Copy `color` and `global_id` to the match.
3. Names follow the anchor, not the overlap: when an old speaker has `name_anchor_*` (the segment the user renamed from), the name goes to the new speaker who owns most words inside that range. If that speaker differs from the overlap winner, both get `needs_review = 1`. Old speakers without an anchor (named through a merge or from an earlier run) pass the name to the overlap winner. This handles the case where the live diarizer conflated two people and the user named one of them: the post-pass split gives the name to the person who was actually talking where the user pressed rename. A name that neither its anchor nor a match placed (a legend rename of someone who spoke under the minimum, or of a live label the post-pass split) falls back, larger old speakers first, to the new speaker with the most region overlap when that covers at least half of the old speaker's own region time; it never replaces a name given in this carry-over, a name is given once, and the receiver gets `needs_review = 1`.
4. New speakers left unmatched get `needs_review = 1`; the UI shows them with a "new label" badge. (A post-pass that splits a live label into two usually means two people were conflated.)
5. Merges are not carried structurally; matched speakers inherit the merged root's name, so two new labels may share a name, and the UI offers "merge again" in one click.
6. `edits` of kind `reassign` are replayed in `created_at` order at word level: new words whose midpoints fall inside the edit's range belong to the edit's target. If that covers a whole segment, the segment is reassigned; otherwise the segment is split at the word boundary and the covered part reassigned. The target is resolved by display name against the new run after step 2; if no speaker has that name, a speaker row `user_N` with `needs_review = 1` is created rather than dropping the edit.

## Identity carry for rebuilt runs

A run rebuilt from another run's words (a punctuation rebuild, Restore, the promoted live run) never changes a word's speaker. The rebuild reads the source run's words with the speaker row of each word's own segment (`RunStore::run_segments` and `run_words`; the row itself, not the merge root the views show), segments them again under that row's engine label, which is unique per channel within a run, so a new segment never spans two source speakers, and adds every source speaker row that got no word (`user_N` rows, a provisional live speaker that never spoke, a speaker whose words the echo gate took). The engine's own defaults (`spk_0` for an undiarized import, `spk_unknown` for the untagged words of a diarized mic, `me` for an undiarized mic) and every earlier reassignment therefore come along as stored. Then `RunStore::carry_over_identity(from, to)` copies every speaker row by `(channel, engine_label)` with its display name, colour, `global_id`, merge (mapped to the new run's rows), `needs_review` and name anchor, however little the speaker said. It replays no `reassign` edit: the words already sit where the edits put them, and a replay by display name would move them again after a rename (reassign to Bob, rename Bob to Robert: the replay would make a `user_1` "Bob"). Margin, overlap, confidence and `raw_label` are copied from the source words. Only a real re-transcription (a post-pass, a re-run) uses the time-overlap carry and its replay. Restore's restored words go to `me` on the mic.

## Speaker rename, merge, reassign

- Rename: update `display_name` and set `name_anchor_start_ms/end_ms` to the segment the rename was invoked from (`rename_speaker` with a segment), or to an explicit time range while recording, when no segment exists yet (`rename_speaker_at` with the live utterance's range). A rename from the legend (no anchor) keeps the anchor an earlier rename set, or leaves it NULL; a blank name clears name and anchor. Anchored names carry over even for speakers with under the 10 s region-time minimum; the minimum applies only to overlap matches. Colour: update the row.
- Merge A into B: set `A.merged_into = root(B)`, and repoint any speaker whose `merged_into = A` to `root(B)` so chains stay one level deep; reject cycles. Queries resolve `COALESCE(merged_into, id)`.
- Reassign a segment: update `segments.speaker_id`, write an `edits` row. To reassign part of a segment, split it at a word boundary first (segments and words are updated in one transaction).

## Queries the UI needs

- Meeting list with duration, speaker count, first 200 chars of the current run.
- Segments of the current run ordered by `seq`, with resolved speaker name and colour, paged for virtualization.
- FTS: join `segments_fts` to `segments` and `meetings`, filter `segments.run_id = meetings.current_run_id`, order by `bm25`, `snippet()`.
- Words in a time window of the current run (click-to-seek, eval export).
- Audio for playback: the `playback` track for live meetings, the `original` for imports.
