//! Event plumbing: backend-to-UI pushes. Names and payloads come from
//! `meet_core::views` and `meet_core::live` so the TypeScript side uses the
//! generated types.
//!
//! - `job:progress`, `job:done`, `job:error`, `job:cancelled`: [`JobEvent`]
//!   (name from [`JobEvent::event_name`]).
//! - `models:progress`: [`DownloadProgress`] while `install_engine` or
//!   `pull_model` runs (Settings, "Download recommended").
//! - `live:*`: the recording's output, see `live.rs`.

use meet_core::views::{DownloadProgress, JobEvent};
use tauri::{AppHandle, Emitter};

pub fn emit_job(app: &AppHandle, event: &JobEvent) {
    if let Err(e) = app.emit(event.event_name(), event) {
        log::warn!("emit {} failed: {e}", event.event_name());
    }
}

pub fn emit_download(app: &AppHandle, progress: &DownloadProgress) {
    if let Err(e) = app.emit(DownloadProgress::NAME, progress) {
        log::warn!("emit {} failed: {e}", DownloadProgress::NAME);
    }
}
