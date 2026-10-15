//! Inference engine integration (Phase 1 onwards).
//!
//! The `Engine` trait the rest of the app talks to, the supervisor that spawns,
//! watches and relaunches engine child processes, and the adapters behind it:
//! `nemo_cli` (v1 file jobs via `nemo-speech.exe transcribe --diarize --json`)
//! and `nemo_live` (v2 live sessions). Adapters translate runtime output into
//! [`crate::protocol::EngineEvent`]s; nothing runtime-specific leaks past here.
