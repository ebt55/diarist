//! Crash recovery at launch (`docs/01`, "Live flow" step 5): meetings left
//! in `recording` get their WAV headers repaired from the file lengths, the
//! live run materialised into a readable transcript, and the post-pass
//! queued like after a normal Stop.

use std::path::Path;

use super::{register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob};
use crate::audio::wav::repair_header;
use crate::error::Result;
use crate::settings;
use crate::store::{RunStore, StoreError};
use crate::transcript::SegmentOptions;
use crate::types::{
    JobId, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source, TranscriptRun,
};

/// One meeting [`recover`] picked up.
pub struct RecoveredMeeting {
    pub meeting_id: MeetingId,
    pub live_run: Option<RunId>,
    /// Sample frames found in each WAV after repair.
    pub mic_frames: u64,
    pub loopback_frames: u64,
    /// The queued post-pass (`None` when there was no audio to process).
    pub post_pass: Option<PostPassHandle>,
}

fn latest_live_run(runs: &[TranscriptRun]) -> Option<&TranscriptRun> {
    runs.iter().rev().find(|r| r.kind == RunKind::Live)
}

/// Finishes every meeting a crash left in `recording` (see the module docs).
/// Never call it while a [`super::Recorder`] is running.
pub fn recover(ctx: &LiveContext) -> Result<Vec<RecoveredMeeting>> {
    let store: &dyn RunStore = &*ctx.store;
    let mut out = Vec::new();
    for m in store.list_meetings()? {
        if m.status != MeetingStatus::Recording || m.source != Source::Live {
            continue;
        }
        let meeting = store.get_meeting(m.id)?.meeting;
        let folder = ctx.data_root.join("audio").join(&meeting.uid);
        let repair = |p: &Path| repair_header(p).map(|r| r.frames).unwrap_or(0);
        let mic_frames = repair(&folder.join("mic.wav"));
        let loopback_frames = repair(&folder.join("loopback.wav"));
        let runs = store.list_runs(m.id)?;
        let live = latest_live_run(&runs).map(|r| r.id);
        if let Some(run) = live {
            match store.materialise_live_run(run, &SegmentOptions::default()) {
                Ok(()) | Err(StoreError::Conflict(_)) => {}
                Err(e) => return Err(e.into()),
            }
            store.finish_run(run, RunStatus::Done)?;
            store.set_current_run(m.id, run)?;
        }
        let frames = mic_frames.max(loopback_frames);
        let duration_ms = (frames * 1_000 / 16_000) as i64;
        store.finish_meeting(m.id, meeting.started_at + duration_ms, duration_ms)?;
        let have_audio = mic_frames > 0 && loopback_frames > 0 && live.is_some();
        let post_pass = if have_audio {
            store.set_meeting_status(m.id, MeetingStatus::Processing)?;
            register_wavs(store, m.id, &ctx.data_root, &folder, duration_ms)?;
            let stored = settings::effective(store)?;
            Some(submit_post_pass(
                ctx,
                PostPassJob {
                    job_id: JobId::new(),
                    meeting: m.id,
                    live_run: live.expect("checked"),
                    data_root: ctx.data_root.clone(),
                    folder,
                    language: meeting.language.clone(),
                    diarize_mic: meeting.diarize_mic,
                    transcribe: true,
                    keep_live_runs: settings::get_bool(&stored, settings::KEEP_LIVE_RUNS),
                    segment: SegmentOptions::default(),
                },
            ))
        } else {
            // Nothing to re-run: keep whatever transcript there is.
            store.set_meeting_status(
                m.id,
                if live.is_some() {
                    MeetingStatus::Done
                } else {
                    MeetingStatus::Failed
                },
            )?;
            None
        };
        out.push(RecoveredMeeting {
            meeting_id: m.id,
            live_run: live,
            mic_frames,
            loopback_frames,
            post_pass,
        });
    }
    Ok(out)
}
