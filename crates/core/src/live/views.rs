//! Payloads of the desktop app's recording commands and `live:*` events
//! that are not the recorder's own outputs (those are in the parent module:
//! [`super::WordsUpdate`], [`super::LevelsUpdate`], [`super::LiveWarning`],
//! [`super::DeviceChange`], ...). Like [`crate::views`], every type derives
//! [`ts_rs::TS`] and is exported to `apps/desktop/src/lib/generated/`, so
//! the WebView uses the same shapes the app emits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{DegradedUpdate, DeviceChange, LevelsUpdate, LiveIssues, LiveWarning, RecorderStatus};
use crate::engine::ServeSnapshot;
use crate::types::{Channel, JobId, MeetingId, RunId};

/// Where the app's one recorder slot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LivePhase {
    Idle,
    Starting,
    Recording,
    Stopping,
}

/// A post-pass the queue holds or runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PostPassView {
    pub job_id: JobId,
    /// `None` until crash recovery has reported which meeting it is for.
    pub meeting_id: Option<MeetingId>,
    pub title: String,
}

/// A warning with the time the app received it (`live:warning`, and the
/// list in [`LiveStateView`]).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct WarningNote {
    #[serde(flatten)]
    #[ts(flatten)]
    pub warning: LiveWarning,
    /// Unix ms.
    pub at: i64,
}

/// Payload of `live:state` and `recording_status().state`.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct LiveStateView {
    pub phase: LivePhase,
    pub meeting_id: Option<MeetingId>,
    pub run_id: Option<RunId>,
    pub title: Option<String>,
    /// Unix ms of meeting time zero.
    pub started_at: Option<i64>,
    pub elapsed_ms: u64,
    /// The endpoint each channel records from (`mic`, `loopback`).
    pub devices: BTreeMap<String, DeviceChange>,
    /// Channels whose live transcription is paused (record-only).
    pub degraded: BTreeMap<String, DegradedUpdate>,
    /// Warnings of this recording, oldest first.
    pub warnings: Vec<WarningNote>,
    pub post_passes: Vec<PostPassView>,
    /// The GPU queue holds new jobs while a recording runs
    /// (`JobQueue::is_paused`): queued imports show "waiting for the
    /// recording to stop".
    pub queue_paused: bool,
    /// Why the last start or stop failed.
    pub error: Option<String>,
}

/// Payload of `live:text_final`: text of `channel` before `until_ms`
/// (meeting time) is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TextFinal {
    pub channel: Channel,
    pub until_ms: i64,
}

/// Payload of `live:levels`, at most 10 Hz: the newest reading per channel
/// since the previous event, with the highest peaks in between; `null` for
/// a channel without a new reading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct LiveLevels {
    pub mic: Option<LevelsUpdate>,
    pub loopback: Option<LevelsUpdate>,
}

impl LiveLevels {
    /// Folds one reading in: the newest RMS and gain, the highest peaks.
    pub fn add(&mut self, u: &LevelsUpdate) {
        let slot = match u.channel {
            Channel::Mic => &mut self.mic,
            _ => &mut self.loopback,
        };
        *slot = Some(match slot {
            Some(prev) => LevelsUpdate {
                peak_dbfs: prev.peak_dbfs.max(u.peak_dbfs),
                raw_peak_dbfs: prev.raw_peak_dbfs.max(u.raw_peak_dbfs),
                ..*u
            },
            None => *u,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.mic.is_none() && self.loopback.is_none()
    }
}

/// What the global shortcut or the tray did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToggleAction {
    Started,
    Stopped,
    Failed,
}

/// Where a toggle came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToggleVia {
    Shortcut,
    Tray,
}

/// Payload of `live:toggled` (the UI shows a toast).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct Toggled {
    pub action: ToggleAction,
    pub via: ToggleVia,
    pub meeting_id: Option<MeetingId>,
    pub message: Option<String>,
}

/// What `start_recording` takes from the start sheet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StartRequest {
    pub title: Option<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub diarize_mic: bool,
    /// Capture endpoint id; `None` = the default communications microphone.
    pub mic: Option<String>,
    /// Render endpoint id; `None` = the default output, following the app
    /// that plays.
    pub loopback: Option<String>,
}

/// What `stop_recording` returns ([`super::StopOutcome::summary`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct StopSummary {
    pub meeting_id: MeetingId,
    pub live_run: RunId,
    pub duration_ms: i64,
    /// The post-pass job.
    pub job_id: JobId,
    /// False when `post_pass_mode = on_issues` kept the live run.
    pub transcribe: bool,
    pub issues: LiveIssues,
}

/// The live engine server for Diagnostics: the supervisor's state, uptime,
/// restarts and pid, and what the app knows about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ServeView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub server: ServeSnapshot,
    /// VRAM the idle server holds (free before launch minus free once ready).
    pub resident_vram_mb: Option<u32>,
    pub asr_model: String,
    pub diar_model: String,
    pub log_path: Option<String>,
}

/// What `recording_status` returns.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct LiveStatusView {
    pub state: LiveStateView,
    pub recorder: Option<RecorderStatus>,
    pub server: Option<ServeView>,
    pub queue_running: Option<JobId>,
    pub queue_waiting: Vec<JobId>,
    pub shortcut: String,
    pub shortcut_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_note_flattens_the_warning() {
        let n = WarningNote {
            warning: LiveWarning::LoopbackSilent { seconds: 60 },
            at: 5,
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({"kind": "loopback_silent", "seconds": 60, "at": 5})
        );
    }

    #[test]
    fn levels_keep_the_newest_reading_and_the_highest_peaks() {
        let reading = |at_ms, peak, raw| LevelsUpdate {
            channel: Channel::Loopback,
            at_ms,
            rms_dbfs: -30.0 - at_ms as f32,
            peak_dbfs: peak,
            gain_db: 12.0,
            raw_peak_dbfs: raw,
        };
        let mut l = LiveLevels::default();
        assert!(l.is_empty());
        l.add(&reading(1, -10.0, -40.0));
        l.add(&reading(2, -20.0, -30.0));
        let lb = l.loopback.unwrap();
        assert_eq!((lb.at_ms, lb.rms_dbfs), (2, -32.0));
        assert_eq!((lb.peak_dbfs, lb.raw_peak_dbfs), (-10.0, -30.0));
        assert!(l.mic.is_none());
    }
}
