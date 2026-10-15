//! Engine and model artefacts (Phase 1).
//!
//! Reads `manifests/engine.json` and `manifests/models.json`, downloads the
//! runtime zip and GGUF models into `%LOCALAPPDATA%\meet-assist` (or the
//! portable folder), verifies SHA-256 and size, unpacks the engine, and keeps
//! the installed registry (`models` table and installed `models.json`).
