# Phase 1: core engine (file mode, no UI)

Status: draft, to be delegated after the runtime spike fills in the exact CLI contract (docs/research/03-runtime-spike.md).

## Goal

`meet-cli transcribe <audio file>` decodes any common audio container, runs NeMo-Speech.cpp with Nemotron 3 Diarization on the GPU, and writes a complete meeting (speakers, segments, words, FTS) into the SQLite database. `meet-cli export` writes it back out in several formats. Everything is testable without a GUI, and the GPU path is gated behind an environment variable.

## Owns

`crates/core/src/{store/sqlite*,models,audio,engine,transcript,export,pipeline}`, `crates/meet-cli`, `crates/core/tests/fixtures/`. Must not touch `apps/desktop`, `core::views`, or the `Store` trait (report needed changes to the orchestrator).

## Modules and acceptance

### `store::sqlite` (implements `Store` from Phase 0.5)
- rusqlite with `bundled` feature; WAL; foreign keys on; `busy_timeout` 5 s; backups via the backup API.
- Migrations as numbered SQL files embedded with `include_str!`, applied by `user_version`. `0001_init.sql` implements `docs/02-data-model.md` v0.2 exactly: INTEGER primary keys, `transcript_runs`, `current_run_id`, `live_utterances`, `edits`, `clock_anchors`, every index listed, the FTS5 external-content table on `content_rowid='id'` with `unicode61 remove_diacritics 2` and its three triggers.
- Beyond the `Store` trait: `create_run`, `finish_run`, `insert_run_transcript(run_id, speakers, segments, words)` in one transaction, `set_current_run`, `append_live_utterance`, `materialise_live_run`, `carry_over(from_run, to_run)` (overlap matrix, greedy one-to-one at 60 percent, `needs_review`, re-apply `edits` at 80 percent), `merge_speaker` with chain flattening and cycle rejection, `delete_meeting` (also removes the audio folder), models registry, settings, anchors.
- Search filters `segments.run_id = meetings.current_run_id`.
- Tests on `:memory:`: the Phase 0.5 conformance suite passes; migration applies twice idempotently; FTS finds a word with diacritics stripped; merge chain stays one level; carry-over on a synthetic two-run meeting copies names and flags an unmatched speaker; delete cascades and FTS has no orphans.

### `models`
- Parse `manifests/engine.json` and `manifests/models.json` (serde). Compute the install layout under a configurable data dir (default `%LOCALAPPDATA%\meet-assist`, or the exe folder in portable mode when `portable.txt` exists).
- `download(entry, dest, progress_cb)`: HTTP with resume (Range), SHA-256 verify (`sha2`), atomic rename. Use `ureq` (sync, small) with rustls. Engine zip is extracted with the `zip` crate into `engine\<name>-<version>\`.
- `status() -> EngineStatus { engine_installed, models: Vec<(name, installed)>, engine_dir, models_dir }`.
- Tests: manifest parsing; sha mismatch rejected; resume logic on a local file served from a test HTTP server (`tiny_http` dev-dependency) or skipped if too costly.

### `audio::decode`
- `trait Decoder { fn decode_to_wav16k(&self, input, output) -> Result<DecodedInfo> }` with a chain: `SymphoniaDecoder` (features: aac, isomp4, mp3, flac, wav, ogg, vorbis) to f32, downmix to mono, resample with rubato `Fft` (integer ratio) or `Async` sinc to 16 kHz, write PCM16 WAV with `hound`; then `MediaFoundationDecoder` (`IMFSourceReader` via the `windows` crate, requesting PCM output); then `FfmpegDecoder` if `ffmpeg` is on PATH; then an error whose message contains the ffmpeg command the user could run. Report duration, source rate, channels and which decoder succeeded.
- Tests: synthesized 48 kHz stereo WAV round-trips to 16 kHz mono with the right length; a 44.1 kHz input works. With `MEET_ASSIST_SAMPLES` set to a folder, decode every `*.m4a` under it (the user's 23 files) and assert the duration is within 50 ms of the container's (`ffprobe` or the MP4 header).

### `engine`
- Implements the `Engine` trait from docs/03 v0.2.
- `NemoCliEngine { engine_dir, models_dir, device }`: two child processes per job, command lines from docs/research/08-runtime-cli-contract.md: `transcribe` with Parakeet TDT v3 and no diarize (JSON to a file in the meeting folder), then `diarize` with the streaming preset (output kept in the meeting folder). Sets `NEMO_SPEECH_MODEL_DIR`. Captures stderr to a log; forwards `Progress` if the CLI prints any. Parses both outputs, calls `fusion::assign_words`, emits `Utterance` events per contiguous same-speaker stretch, `TextFinalUntil`, `SpeakersFinalUntil`, `Done` with stats (wall time, RTF, speakers). Non-zero exit or missing output becomes `Error` with the stderr tail. Cancel kills the child.
- `NemoCliFallbackEngine`: Nemotron-en `--stream --endpointing --diarize --json`, used when the settings choose low-VRAM mode or when the Parakeet pass fails with an out-of-memory signature.

### `fusion`
- `assign_words(words, segments) -> Vec<TaggedWord>`: start-anchored window rule from docs/03; nearest-segment fallback; `spk_N` labels in order of first appearance; keeps `raw_label`.
- Tests on hand-written cases: a word straddling a boundary goes to the speaker active at its start; a word in a gap goes to the nearest segment; label numbering is stable. A fixture test re-tags `parakeet_transcribe_5min.json` with `diarize_5min` from `docs/research/fixtures/` and checks agreement with `nemotron_transcribe_diarize_5min.json` above 95 percent on words both have.
- Fixtures: the real JSON produced by the spike, committed under `crates/core/tests/fixtures/nemo_cli_*.json`. Tests cover the mapping, the absent-speaker case and the error path. A GPU integration test runs the real binary when `MEET_ASSIST_ENGINE_DIR` and `MEET_ASSIST_MODELS_DIR` are set.
- Long files: a GPU-gated acceptance test transcribes the 77-minute CIMC recording and a 2-hour file (two eval files concatenated) and logs peak VRAM (`nvidia-smi` sampled). If the runtime fails or degrades, implement the fallback from docs/03: whole-file `diarize` pass plus ASR over silence-cut chunks of at most 10 minutes, fused by the start-anchored window rule.
- `models::download` uses rustls with `rustls-platform-verifier` so the Windows certificate store and system proxy are honoured; HF URLs are pinned to commit SHAs in the manifest and redirects are re-resolved when resuming.

### `pipeline`
- Fill in `pipeline::import` from Phase 0.5: decode, create a `file` run, run the engine through `JobQueue`, segment, store, set current run, delete the intermediate WAV unless kept, emit `JobEvent`s, support cancel.
- Free-space check before decoding (refuse under 2 GB free).

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
