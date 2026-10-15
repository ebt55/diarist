# Phase 0.5: shared interfaces between core and UI

Status: to be run by the Phase 0 agent right after the scaffold, before Phases 1 and 2 start. About one hour.

## Goal

Freeze the seam between `crates/core` and `apps/desktop` so Phase 1 (core) and Phase 2 (UI) can proceed in parallel without ever editing the same file, and so "swap the mock for the real DB" is a one-line change.

## Deliverables (all in `crates/core`, plus generated TypeScript)

1. `core::views`: `MeetingSummary`, `MeetingDetail`, `SpeakerView`, `SegmentView`, `SearchHit`, `EngineStatus`, `JobEvent` (`Progress`, `Done`, `Error`), `DeviceInfo`, `LiveWordsEvent`, `LiveRelabelEvent`. serde and `ts_rs::TS` derives; `#[ts(export)]` writes `apps/desktop/src/lib/generated/*.ts` on `cargo test`.
2. `core::store::Store` trait: every query and mutation the UI needs, mirroring the command list in plans/03-phase2-desktop-ui.md, returning `Result<_, StoreError>`. Sync methods; the Tauri layer wraps them in `spawn_blocking`.
3. `core::store::MemoryStore`: in-memory implementation with a `seed(n_meetings, segments_per_meeting)` generator for the UI mock (500 meetings, three with 2,000 segments). Lives in core so Phase 1 can reuse it in tests.
4. `core::pipeline::import`: signature only, documented: `fn import_file(store: &dyn Store, engine: &dyn Engine, decoder: &dyn Decoder, req: ImportRequest, on_event: &dyn Fn(JobEvent)) -> Result<MeetingId>`, plus a `JobQueue` type that runs one GPU job at a time with cancel. Phase 1 fills in the body.
5. Update `types.rs` to v0.2 of docs/02-data-model.md (INTEGER ids as `i64`, `uid`, `run_id`, `raw_label`, `margin`, `overlap`, `language`, `diarize_mic`, `audio_keep`) and `protocol.rs` to docs/03-engine-protocol.md v0.2 (traits, `EngineEvent` with `Utterance`, `TextFinalUntil`, `SpeakersFinalUntil`; drop the binary framing from the main path, keep it as an `ffi_wire` module behind a feature flag).

## Acceptance

- `cargo test --workspace` regenerates the TypeScript types and passes; `bun run check` in `apps/desktop` passes against the generated types.
- `MemoryStore` passes a small conformance test suite in `crates/core/tests/store_conformance.rs` that Phase 1's `SqliteStore` must also pass (the test is written as a function taking `&dyn Store`).
- The scaffold's `list_meetings` command now reads from `MemoryStore` via the trait.

## Boundaries after this phase

- Phase 1 owns `crates/core/src/{db,models,audio,engine,transcript,export,pipeline}` and `crates/meet-cli`. It implements `SqliteStore: Store` and must not change `views.rs` or the `Store` trait without a note to the orchestrator.
- Phase 2 owns `apps/desktop/**` and codes only against `meet_core::{views, store::Store, types}` and the generated TypeScript.
