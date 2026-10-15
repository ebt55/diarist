# Roadmap and delegation plan

Status: v0.2, 15 Oct 2026. Each phase gets its own plan file in this folder with acceptance criteria, file boundaries and verification steps. Opus 5.5 agents implement; Fable reviews between chunks.

Delivered on 15 Oct 2026: Phases 0, 0.5, 1, 2, 1.5, 3a, 3b and 4. In progress: 3.5 (live follow-ups). Next: 5 (ship). The app imports and records live end to end on the development machine; see docs/eval for the measured baseline.

| Phase | Deliverable | Depends on | Parallel with |
|---|---|---|---|
| 0 Scaffold | Cargo workspace, `crates/core` with public types, `crates/cli`, Tauri 2 + Svelte 5 app that builds and opens a window, manifests, CI-free `cargo test` and `bun run build` green | nothing | spike |
| 0.5 Shared interfaces | `core::views`, `trait Store` + `MemoryStore` + conformance suite, `pipeline::import` signature, `JobQueue`, ts-rs generated TypeScript; types updated to docs/02 v0.2 and docs/03 v0.2 | 0 | nothing (one hour, same agent as 0) |
| 1 Core engine | `SqliteStore` (migrations, FTS, runs, queries), `models` (download, verify, registry), `engine::nemo_cli` adapter, decoder chain, `transcript` segmenter, `export`, `pipeline::import`; `diarist-cli transcribe file` writes a meeting into the DB | 0.5, spike | 2 |
| 2 Desktop UI | Meeting list, import with progress, virtualized transcript view, speaker rename/merge/colour, FTS search with highlights, export (md, txt, json, srt, vtt), diagnostics pane, all against `MemoryStore` | 0.5 | 1 |
| 1.5 Integration | Merge Phase 1; move export, peaks, palette, settings into core; seam additions (Cancelled, playback_track, mark_reviewed); real `SqliteStore`, pipeline and models behind the UI; import a real recording from the app | 1, 2 | nothing |
| 3 Live capture | wasapi mic + loopback, QPC alignment, zero-fill, rubato, WAV writers, live engine session adapter, preview buffer, checkpoints, post-pass, speaker carry-over | 1, spike | 4 |
| 4 Eval harness | `tools/eval`: DER and words-to-wrong-speaker vs RTTM; AMI ES2002a baseline; user meetings when provided | 1 | 3 |
| 5 Ship | First-run downloader UI, NSIS installer, portable mode, tray + shortcuts + autostart, crash recovery, settings, README, licence audit | 2, 3 | nothing |
| 6 Identity (v3) | Speaker embeddings, cross-meeting speaker matching, sqlite-vec | 5 | nothing |

## Delegation rules

- One agent per phase chunk; two agents never edit the same file at the same time. After Phase 0.5, Phase 1 owns `crates/core/src/{store/sqlite,models,audio,engine,transcript,export,pipeline}` and `crates/cli`; Phase 2 owns `apps/desktop/**` and codes only against `diarist_core::{views, store::Store, types}` and the generated TypeScript. Changes to `views.rs` or the `Store` trait go through the orchestrator.
- Agents may run `rustup update stable` when a dependency requires it, and must say so in their report.
- Every brief states: goal, files it may touch, files it must not touch, acceptance criteria, exact verification commands, and what to report back.
- Agents commit on their own branch of work in small commits; Fable reviews the diff and merges or sends fixes back to the same (warm) agent.
- No Python in shipped code. Python only under `tools/eval`.
