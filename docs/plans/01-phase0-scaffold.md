# Phase 0: scaffold

Status: delegated 15 Oct 2026 to an Opus 5.5 agent.

## Goal

A buildable monorepo skeleton matching the repository layout in `docs/01-architecture.md`, with domain types and the engine protocol typed and tested, a Tauri 2 + Svelte 5 window that opens and lists fixture meetings, and no business logic.

## Deliverables

1. `Cargo.toml` workspace (resolver 2) with members `crates/core`, `crates/cli`, `apps/desktop/src-tauri`; shared `[workspace.dependencies]` for serde, serde_json, thiserror, anyhow, tracing, ulid, clap.
2. `crates/core` (package `diarist-core`):
   - `types.rs`: `Meeting`, `AudioTrack`, `Speaker`, `Segment`, `Word`, enums `Source`, `Channel`, `MeetingStatus`, matching `docs/02-data-model.md`; serde derive; ids are `String` (ulid).
   - `protocol.rs`: `EngineCommand` (tag `cmd`) and `EngineEvent` (tag `ev`) enums matching `docs/03-engine-protocol.md`; serde round-trip tests using the JSON examples from that doc; frame header helpers (`u8 type + u32 LE len`) with tests.
   - `error.rs`: `thiserror` error type.
   - Empty modules `db`, `audio`, `engine`, `models`, `transcript`, each a `mod.rs` with a doc comment stating its responsibility (landing spots for Phases 1 to 3).
3. `crates/cli` (bin `diarist-cli`): clap with `version` and `transcribe <file>` (stub exits 2 with "not implemented").
4. `apps/desktop`: Tauri 2.12.x, product name `Diarist`, identifier `app.diarist.desktop`, Svelte 5 + TypeScript + Vite, bun as package manager. `src-tauri` depends on `diarist-core`. One command `list_meetings` returning three fixture `Meeting`s. Frontend: sidebar listing meetings from the command, main pane placeholder. Window `visible: false` in config, shown from the frontend after mount. Plain CSS with variables, light and dark via `prefers-color-scheme`. Bundle target NSIS, `webviewInstallMode: downloadBootstrapper`. Plugins: shell, dialog, log, with a capabilities file.
5. `manifests/engine.json`, `manifests/models.json`: schema `{name, version, kind, url, sha256, size_bytes, licence}` with placeholder entries for the NeMo-Speech.cpp Windows CUDA zip and the three GGUF models (values to be filled by the spike).
6. `tools/eval/README.md` placeholder. `README.md`, `docs/DEV.md` (commands verified to work), `LICENSE` (MIT), `.editorconfig`, `rustfmt.toml`.

## File boundaries

May create anything outside `docs/`. May create `docs/DEV.md` only. Must not edit other files in `docs/`.

## Acceptance criteria

- `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` all pass.
- `bun install` and `bun run build` in `apps/desktop` pass.
- `bun run tauri build --debug` (or `bunx @tauri-apps/cli build --debug`) produces an exe; launching it keeps a process alive for 5 s and it exits cleanly when stopped. If NSIS bundling cannot download in this environment, `--no-bundle` is acceptable; report it.
- No global tool installs (`cargo install` is not allowed). If Rust 1.88 is below some dependency's MSRV, stop and report the exact requirement instead of updating the toolchain.
- Small commits on `main` with message trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Report back

Files created (tree), verification command outputs (summarized), debug exe size, anything that deviated from this plan and why.
