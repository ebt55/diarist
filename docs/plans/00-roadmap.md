# Roadmap and delegation plan

Status: v0.1, 15 Oct 2026. Each phase gets its own plan file in this folder with acceptance criteria, file boundaries and verification steps. Opus 5.5 agents implement; Fable reviews between chunks.

| Phase | Deliverable | Depends on | Parallel with |
|---|---|---|---|
| 0 Scaffold | Cargo workspace, `crates/core` with public types, `crates/meet-cli`, Tauri 2 + Svelte 5 app that builds and opens a window, manifests, CI-free `cargo test` and `bun run build` green | spike results for engine paths | nothing (short) |
| 1 Core engine | `db` (migrations, FTS, queries), `models` (download, verify, registry), `engine::nemo_cli` adapter, `transcript` segmenter, `symphonia` decode; `meet-cli transcribe file` writes a meeting into the DB | 0 | 2 |
| 2 Desktop UI | Meeting list, import with progress, virtualized transcript view, speaker rename/merge/colour, FTS search with highlights, export (md, txt, json, srt, vtt), diagnostics pane | 0 (types), fixtures | 1 |
| 3 Live capture | wasapi mic + loopback, QPC alignment, zero-fill, rubato, WAV writers, live engine session adapter, preview buffer, checkpoints, post-pass, speaker carry-over | 1, spike | 4 |
| 4 Eval harness | `tools/eval`: DER and words-to-wrong-speaker vs RTTM; AMI ES2002a baseline; user meetings when provided | 1 | 3 |
| 5 Ship | First-run downloader UI, NSIS installer, portable mode, tray + shortcuts + autostart, crash recovery, settings, README, licence audit | 2, 3 | nothing |
| 6 Identity (v3) | Speaker embeddings, cross-meeting speaker matching, sqlite-vec | 5 | nothing |

## Delegation rules

- One agent per phase chunk; two agents never edit the same crate at the same time. Phase 1 owns `crates/core` and `crates/meet-cli`; Phase 2 owns `apps/desktop` and may only add to `crates/core/src/types.rs` through an agreed interface file.
- Every brief states: goal, files it may touch, files it must not touch, acceptance criteria, exact verification commands, and what to report back.
- Agents commit on their own branch of work in small commits; Fable reviews the diff and merges or sends fixes back to the same (warm) agent.
- No Python in shipped code. Python only under `tools/eval`.
