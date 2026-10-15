# Phase 1.5: integration (real store behind the real UI)

Status: draft, 15 Oct 2026. Runs after the Phase 1 branch merges. Core side goes to the Phase 1 agent (warm), UI side to the Phase 2 agent (warm, small follow-up only).

## Goal

The desktop app imports one of the user's real recordings through the GPU pipeline and shows the transcript, search, speaker editing, export and playback against `SqliteStore`. Every piece of logic the UI agent had to mock in `src-tauri` moves into `crates/core`.

## Core side (Phase 1 agent) in this order

1. Merge main into the Phase 1 branch (Cargo.lock conflicts: regenerate), make `cargo test --workspace` green, then the orchestrator merges to main.
2. Move into core and delete the `src-tauri` copies: `export` (replaces `src-tauri/export.rs`), waveform peaks (`audio::peaks(path, buckets) -> views::Waveform`, WAV and FLAC via symphonia, cached per track), `SPEAKER_PALETTE` (exported to TypeScript), settings keys and defaults (`core::settings`: `audio_retention_days`, `theme`, `default_language`, `post_pass_mode`, `keep_live_runs`, `keep_diarization_segments`, selections).
3. Seam additions (the only allowed changes to `views.rs` and the `Store` trait, applied by this agent and regenerated into TypeScript): `JobEvent::Cancelled`; `Store::playback_track(id) -> Option<AudioTrack>` (playback for live meetings, original for imports, newest existing otherwise); `Store::mark_reviewed(speaker_id)`; `EngineInstall.display_name`; `MemoryStore` updated to match; conformance cases added.
4. `src-tauri` wiring: `SqliteStore::open(data_root.join("meet-assist.db"))` in `run()`; `import_file` calls `pipeline::import_file` on the `JobQueue` with the real `NemoCliEngine` and decoder chain, forwarding `JobEvent`s; `engine_status`, `list_model_files`, `select_model`, `rescan_models`, `install_engine`, `pull_model` wired to `models::*` with progress events; `last_engine_stderr` reads the last job's log. Mock import and demo audio are removed; the demo seed stays behind a `--demo` flag for screenshots.
5. End-to-end (GPU): launch the debug build, import `C:\Users\ebin\glm-ground\meets\5 Stephen Casper (Harvard Kennedy School)\*.m4a` via the command, wait for `job:done`, confirm segments, search, rename, export and playback of the original m4a work over the DevTools protocol. Report wall time and VRAM.

## UI side (Phase 2 agent, after core lands)

- Replace local copies with the generated types and core-backed commands; use `JobEvent::Cancelled`; "mark reviewed" action on the new-label badge; display names in the Engine group; Settings reads defaults from core.
- Re-verify `bun run check`, clippy, debug build, launch; refresh the screenshots with a real transcript (user's recording titles must not appear in committed screenshots: use the AMI file).

## Acceptance

- `cargo test --workspace` and all Phase 1 and Phase 2 checks green on main.
- A real meeting imported from the UI is readable, searchable and exportable; renames survive an app restart.
- No logic left in `src-tauri` beyond command wrappers and event plumbing.
