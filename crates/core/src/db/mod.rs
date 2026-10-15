//! Persistence (Phase 1).
//!
//! Owns the single SQLite file (`rusqlite`, `bundled`, FTS5 on, WAL, foreign
//! keys on): numbered SQL migrations applied at startup via `user_version`,
//! row mapping to and from [`crate::types`], the FTS triggers, and the queries
//! the UI needs (meeting list, paged segments, full-text search, words in a
//! time window). Schema: `docs/02-data-model.md`.
