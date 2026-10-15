# Phase 1: core engine (file mode, no UI)

Status: draft, to be delegated after the runtime spike fills in the exact CLI contract (docs/research/03-runtime-spike.md).

## Goal

`meet-cli transcribe <audio file>` decodes any common audio container, runs NeMo-Speech.cpp with Nemotron 3 Diarization on the GPU, and writes a complete meeting (speakers, segments, words, FTS) into the SQLite database. `meet-cli export` writes it back out in several formats. Everything is testable without a GUI, and the GPU path is gated behind an environment variable.

## Owns

`crates/core` (all modules), `crates/meet-cli`, `crates/core/tests/fixtures/`. Must not touch `apps/desktop`.

## Modules and acceptance

### `db`
- rusqlite with `bundled` feature; WAL; foreign keys on; `busy_timeout` 5 s.
- Migrations as numbered SQL files embedded with `include_str!`, applied by `user_version`. `0001_init.sql` implements `docs/02-data-model.md` exactly, including the FTS5 external-content table and its insert/update/delete triggers.
- Repository API (sync, `&Connection` behind a `Db` struct with a mutex): `create_meeting`, `set_meeting_status`, `finish_meeting`, `insert_transcript(meeting_id, speakers, segments, words)` in one transaction, `list_meetings() -> Vec<MeetingSummary>` (duration, speaker count, first 200 chars), `get_meeting`, `get_speakers`, `get_segments(meeting_id, offset, limit) -> Vec<SegmentView>` with resolved speaker name and colour, `get_words(meeting_id, from_ms, to_ms)`, `search(query, limit) -> Vec<SearchHit>` with `snippet()` and `bm25` ordering, `rename_speaker`, `set_speaker_color`, `merge_speaker(from, into)`, `unmerge_speaker`, `reassign_segment`, `delete_meeting` (also removes the audio folder), settings get/set, models registry upsert/list, live checkpoint put/get/clear.
- Tests on `:memory:` databases: migration applies twice idempotently; insert then FTS search finds a word; merge resolves in `get_segments`; delete cascades.

### `models`
- Parse `manifests/engine.json` and `manifests/models.json` (serde). Compute the install layout under a configurable data dir (default `%LOCALAPPDATA%\meet-assist`, or the exe folder in portable mode when `portable.txt` exists).
- `download(entry, dest, progress_cb)`: HTTP with resume (Range), SHA-256 verify (`sha2`), atomic rename. Use `ureq` (sync, small) with rustls. Engine zip is extracted with the `zip` crate into `engine\<name>-<version>\`.
- `status() -> EngineStatus { engine_installed, models: Vec<(name, installed)>, engine_dir, models_dir }`.
- Tests: manifest parsing; sha mismatch rejected; resume logic on a local file served from a test HTTP server (`tiny_http` dev-dependency) or skipped if too costly.

### `audio::decode`
- `decode_to_wav16k(input, output) -> Result<DecodedInfo>`: symphonia (features: aac, isomp4, mp3, flac, wav, ogg, vorbis) to f32, downmix to mono, resample with rubato `Fft` (fixed ratio when integer, `Async` sinc otherwise) to 16 kHz, write PCM16 WAV with `hound`. Report duration and source sample rate.
- Tests: synthesized 48 kHz stereo WAV round-trips to 16 kHz mono with the right length; a 44.1 kHz input works. An `.m4a` test is gated on `MEET_ASSIST_SAMPLES` pointing at a folder with one.

### `engine`
- `trait Engine { fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>>; fn cancel(&self, job_id) }`.
- `NemoCliEngine { engine_dir, models_dir, device }`: spawns `nemo-speech.exe transcribe` with the flags documented in the spike report (diarize on, JSON output to a temp file, cuda device, offline diarizer preset, ASR model path, diarizer model path). Captures stderr to a log and, if the CLI prints progress, forwards `progress` events. On exit, parses the JSON (`words[{word,start,end,confidence,speaker?}]`, speaker 1-based and absent when 0), emits one or more `words` events with `final: true`, then `finalized`, then `done` with stats. Non-zero exit or missing JSON becomes an `error` event with the stderr tail.
- Fixtures: the real JSON produced by the spike, committed under `crates/core/tests/fixtures/nemo_cli_*.json`. Tests cover the mapping, the absent-speaker case and the error path. A GPU integration test runs the real binary when `MEET_ASSIST_ENGINE_DIR` and `MEET_ASSIST_MODELS_DIR` are set.

### `transcript`
- `label_for(speaker_int: Option<u32>) -> String` (`spk_0`.., or `spk_unknown` for absent).
- `segment(words, opts) -> Vec<Segment>`: contiguous same-speaker words; split on silence gap over `opts.max_gap_ms` (default 1500), or on sentence-final punctuation followed by a gap over 500 ms, or when a segment exceeds `opts.max_words` (default 80).
- `interleave(channels: Vec<(Channel, Vec<Word>)>) -> Vec<Word>` by start time (used by live post-pass later; write and test now).
- Deterministic colour palette: 8 to 12 distinguishable colours by speaker index.
- Tests on hand-written word lists.

### `export`
- `to_markdown`, `to_text`, `to_json`, `to_srt`, `to_vtt`, `to_rttm` (RTTM for the eval harness: one line per segment, `SPEAKER <id> 1 <start> <dur> <NA> <NA> <label> <NA> <NA>`). Golden-file tests.

### `meet-cli`
- `transcribe <file> [--db <path>] [--data-dir <dir>] [--no-diarize] [--title <s>] [--out <path> --format md|txt|json|srt|vtt|rttm]`: decode, run engine, segment, store, print a summary (duration, speakers, segments, RTF).
- `list`, `show <meeting_id>`, `search <query>`, `export <meeting_id> --format ... --out ...`, `rename <speaker_id> <name>`, `models status`, `models pull` (downloads what is missing with a progress bar), `doctor` (data dir, engine present, models present, `nvidia-smi` reachable).
- Exit codes: 0 ok, 1 error, 2 usage.

## Verification

- `cargo test --workspace`, clippy with `-D warnings`, fmt check.
- With the engine and models present: `meet-cli transcribe <one of the user's m4a>` completes; `meet-cli export --format md` produces a readable transcript with speaker labels; `meet-cli search <word from the transcript>` finds it. Report wall time and the number of speakers found.
- Report fixture JSON size and where it came from.

## Out of scope

Live capture, UI, downloading in the UI, speaker embeddings, our own fusion algorithm (v1.5).
