//! Event plumbing: backend-to-UI pushes. Names and payloads come from
//! `meet_core::views` so the TypeScript side uses the generated types.
//!
//! - `job:progress`, `job:done`, `job:error`, `job:cancelled`: [`JobEvent`]
//!   (name from [`JobEvent::event_name`]).
//! - `models:progress`: [`DownloadProgress`] while `install_engine` or
//!   `pull_model` runs. Not a view type yet: no screen consumes it until the
//!   first-run downloader UI (Phase 5) promotes it into `meet_core::views`.
//! - `transcript:words` ([`LiveWordsEvent`]) and `transcript:relabel`
//!   ([`LiveRelabelEvent`]): reserved for live mode (Phase 3); the emitters
//!   exist so the payload contract is fixed, but nothing produces them yet.

use meet_core::views::{JobEvent, LiveRelabelEvent, LiveWordsEvent};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub fn emit_job(app: &AppHandle, event: &JobEvent) {
    if let Err(e) = app.emit(event.event_name(), event) {
        log::warn!("emit {} failed: {e}", event.event_name());
    }
}

/// Progress of an engine or model download.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    /// `engine` or `model`.
    pub kind: &'static str,
    /// Engine folder name or manifest model name.
    pub name: String,
    pub done_bytes: u64,
    pub total_bytes: Option<u64>,
    /// The download (and the rescan after it) finished, successfully or not.
    pub finished: bool,
    pub error: Option<String>,
}

impl DownloadProgress {
    pub const NAME: &'static str = "models:progress";
}

pub fn emit_download(app: &AppHandle, progress: &DownloadProgress) {
    if let Err(e) = app.emit(DownloadProgress::NAME, progress) {
        log::warn!("emit {} failed: {e}", DownloadProgress::NAME);
    }
}

/// Live preview words (Phase 3 producer).
pub fn emit_live_words(app: &AppHandle, event: &LiveWordsEvent) {
    if let Err(e) = app.emit(LiveWordsEvent::NAME, event) {
        log::warn!("emit {} failed: {e}", LiveWordsEvent::NAME);
    }
}

/// Live label remap (Phase 3 producer).
pub fn emit_live_relabel(app: &AppHandle, event: &LiveRelabelEvent) {
    if let Err(e) = app.emit(LiveRelabelEvent::NAME, event) {
        log::warn!("emit {} failed: {e}", LiveRelabelEvent::NAME);
    }
}
