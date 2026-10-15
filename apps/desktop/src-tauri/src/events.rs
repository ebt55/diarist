//! Event plumbing: backend-to-UI pushes. Names and payloads come from
//! `meet_core::views` so the TypeScript side uses the generated types.
//!
//! - `job:progress`, `job:done`, `job:error`: [`JobEvent`] (name from
//!   [`JobEvent::event_name`]).
//! - `transcript:words` ([`LiveWordsEvent`]) and `transcript:relabel`
//!   ([`LiveRelabelEvent`]): reserved for live mode (Phase 3); the emitters
//!   exist so the payload contract is fixed, but nothing produces them yet.

use meet_core::views::{JobEvent, LiveRelabelEvent, LiveWordsEvent};
use tauri::{AppHandle, Emitter};

pub fn emit_job(app: &AppHandle, event: &JobEvent) {
    if let Err(e) = app.emit(event.event_name(), event) {
        log::warn!("emit {} failed: {e}", event.event_name());
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
