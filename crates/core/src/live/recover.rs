//! Crash recovery at launch (`docs/01`, "Live flow" step 5).
//!
//! - Meetings left in `recording`: WAV headers repaired from the file
//!   lengths, the live run materialised into a readable transcript, and the
//!   post-pass queued like after a normal Stop (its echo transfer gate takes
//!   the live run's stored mic words, or transcribes `mic.wav` when there
//!   are none: [`super::postpass`]).
//! - Meetings left in `processing` (the post-pass was queued or running):
//!   with both WAVs still there, the output of the interrupted post-pass is
//!   discarded and the post-pass queued again. Without them (the post-pass
//!   got as far as deleting the WAVs), a finished `post_pass` run that is
//!   current is kept and the meeting finished (the live run goes unless
//!   `keep_live_runs` or the post-pass recorded `unplaced_names`);
//!   otherwise the materialised live run becomes current with a note in its
//!   `engine_info`.

use std::path::Path;

use super::rerun::discard_unfinished_post_passes;
use super::{
    live_run_of, register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob,
};
use crate::audio::wav::repair_header;
use crate::error::Result;
use crate::punct::rebuild::descends_from;
use crate::settings;
use crate::store::{RunStore, StoreError};
use crate::transcript::SegmentOptions;
use crate::types::{JobId, Meeting, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source};

/// The note a promoted live run gets in its `engine_info` (key `recovery`).
pub const PROMOTED_NOTE: &str = "The post-pass was interrupted and the recorded audio is gone, \
     so the live transcript is the final one.";

/// What [`recover`] did with one meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// The post-pass was queued ([`RecoveredMeeting::post_pass`]).
    PostPassQueued,
    /// A finished post-pass run was already current: the meeting is done.
    Finished,
    /// The live run became current and final (no audio to run again).
    PromotedLive,
    /// Neither audio nor a live transcript: the meeting is `failed`.
    Failed,
}

/// One meeting [`recover`] picked up.
pub struct RecoveredMeeting {
    pub meeting_id: MeetingId,
    /// `recording` or `processing`.
    pub was: MeetingStatus,
    pub action: RecoveryAction,
    pub live_run: Option<RunId>,
    /// Sample frames found in each WAV (after repair; 0 when missing).
    pub mic_frames: u64,
    pub loopback_frames: u64,
    /// The queued post-pass (`None` when there was no audio to process).
    pub post_pass: Option<PostPassHandle>,
}

/// Makes sure the live run has its transcript, is finished and is current,
/// unless the current run is a copy of it (Restore, a punctuation rebuild):
/// that stays current, so the post-pass carries its names over (plan 16
/// section 3.9 step 7).
fn settle_live_run(store: &dyn RunStore, meeting: MeetingId, run: RunId) -> Result<()> {
    match store.materialise_live_run(run, &SegmentOptions::default()) {
        Ok(()) | Err(StoreError::Conflict(_)) => {}
        Err(e) => return Err(e.into()),
    }
    if store.get_run(run)?.status == RunStatus::Running {
        store.finish_run(run, RunStatus::Done)?;
    }
    let copy = store
        .get_meeting(meeting)?
        .current_run
        .is_some_and(|c| c.id != run && descends_from(store, &c, run));
    if !copy {
        store.set_current_run(meeting, run)?;
    }
    Ok(())
}

fn post_pass_job(
    ctx: &LiveContext,
    meeting: &Meeting,
    live_run: RunId,
    folder: std::path::PathBuf,
) -> Result<PostPassJob> {
    let stored = settings::effective(&*ctx.store)?;
    Ok(PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run,
        data_root: ctx.data_root.clone(),
        folder,
        language: meeting.language.clone(),
        diarize_mic: meeting.diarize_mic,
        transcribe: true,
        keep_live_runs: settings::get_bool(&stored, settings::KEEP_LIVE_RUNS),
        segment: SegmentOptions::default(),
        punct: ctx.punct.clone(),
    })
}

/// Finishes every live meeting a crash left in `recording` or `processing`
/// (see the module docs). Never call it while a [`super::Recorder`] runs or
/// a post-pass is queued.
pub fn recover(ctx: &LiveContext) -> Result<Vec<RecoveredMeeting>> {
    let store: &dyn RunStore = &*ctx.store;
    let mut out = Vec::new();
    for m in store.list_meetings()? {
        if m.source != Source::Live {
            continue;
        }
        let r = match m.status {
            MeetingStatus::Recording => recover_recording(ctx, m.id)?,
            MeetingStatus::Processing => recover_processing(ctx, m.id)?,
            MeetingStatus::Done | MeetingStatus::Failed => continue,
        };
        out.push(r);
    }
    Ok(out)
}

/// A meeting whose recorder died.
fn recover_recording(ctx: &LiveContext, id: MeetingId) -> Result<RecoveredMeeting> {
    let store: &dyn RunStore = &*ctx.store;
    let meeting = store.get_meeting(id)?.meeting;
    let folder = ctx.data_root.join("audio").join(&meeting.uid);
    let repair = |p: &Path| repair_header(p).map(|r| r.frames).unwrap_or(0);
    let mic_frames = repair(&folder.join("mic.wav"));
    let loopback_frames = repair(&folder.join("loopback.wav"));
    let runs = store.list_runs(id)?;
    let live = live_run_of(&runs).map(|r| r.id);
    if let Some(run) = live {
        settle_live_run(store, id, run)?;
    }
    let frames = mic_frames.max(loopback_frames);
    let duration_ms = (frames * 1_000 / 16_000) as i64;
    store.finish_meeting(id, meeting.started_at + duration_ms, duration_ms)?;
    let mut r = RecoveredMeeting {
        meeting_id: id,
        was: MeetingStatus::Recording,
        action: RecoveryAction::Failed,
        live_run: live,
        mic_frames,
        loopback_frames,
        post_pass: None,
    };
    match live {
        Some(run) if mic_frames > 0 && loopback_frames > 0 => {
            store.set_meeting_status(id, MeetingStatus::Processing)?;
            register_wavs(store, id, &ctx.data_root, &folder, duration_ms)?;
            let job = post_pass_job(ctx, &meeting, run, folder)?;
            r.post_pass = Some(submit_post_pass(ctx, job, true));
            r.action = RecoveryAction::PostPassQueued;
        }
        // Nothing to run again: keep whatever transcript there is.
        Some(_) => {
            store.set_meeting_status(id, MeetingStatus::Done)?;
            r.action = RecoveryAction::PromotedLive;
        }
        None => store.set_meeting_status(id, MeetingStatus::Failed)?,
    }
    Ok(r)
}

/// A meeting whose post-pass was queued or running.
fn recover_processing(ctx: &LiveContext, id: MeetingId) -> Result<RecoveredMeeting> {
    let store: &dyn RunStore = &*ctx.store;
    let detail = store.get_meeting(id)?;
    let meeting = detail.meeting;
    let folder = ctx.data_root.join("audio").join(&meeting.uid);
    let frames = |p: &Path| {
        if p.is_file() {
            repair_header(p).map(|r| r.frames).unwrap_or(0)
        } else {
            0
        }
    };
    let mic_frames = frames(&folder.join("mic.wav"));
    let loopback_frames = frames(&folder.join("loopback.wav"));
    let runs = store.list_runs(id)?;
    let live = live_run_of(&runs).map(|r| r.id);
    let mut r = RecoveredMeeting {
        meeting_id: id,
        was: MeetingStatus::Processing,
        action: RecoveryAction::Failed,
        live_run: live,
        mic_frames,
        loopback_frames,
        post_pass: None,
    };
    let current = detail.current_run;
    match live {
        Some(run) if mic_frames > 0 && loopback_frames > 0 => {
            // Whatever the interrupted post-pass wrote is made again.
            for p in &runs {
                if p.kind == RunKind::PostPass && p.id.0 > run.0 {
                    store.delete_run(p.id)?;
                }
            }
            settle_live_run(store, id, run)?;
            let duration_ms = meeting
                .duration_ms
                .unwrap_or((mic_frames.max(loopback_frames) * 1_000 / 16_000) as i64);
            register_wavs(store, id, &ctx.data_root, &folder, duration_ms)?;
            let job = post_pass_job(ctx, &meeting, run, folder)?;
            r.post_pass = Some(submit_post_pass(ctx, job, true));
            r.action = RecoveryAction::PostPassQueued;
        }
        _ if current
            .as_ref()
            .is_some_and(|c| c.kind == RunKind::PostPass && c.status == RunStatus::Done) =>
        {
            // The post-pass wrote its run and deleted the WAVs; only the
            // bookkeeping after that was lost. A live run with names the
            // carry-over could not place stays, as after a normal post-pass.
            let stored = settings::effective(store)?;
            let unplaced = current
                .as_ref()
                .and_then(|c| c.engine_info["unplaced_names"].as_u64())
                .unwrap_or(0);
            if let Some(run) = live {
                if !settings::get_bool(&stored, settings::KEEP_LIVE_RUNS) && unplaced == 0 {
                    store.delete_run(run)?;
                }
            }
            store.set_meeting_status(id, MeetingStatus::Done)?;
            r.action = RecoveryAction::Finished;
        }
        Some(run) => {
            discard_unfinished_post_passes(store, id, run)?;
            settle_live_run(store, id, run)?;
            let mut info = store.get_run(run)?.engine_info;
            if !info.is_object() {
                info = serde_json::json!({});
            }
            info["recovery"] = serde_json::json!(PROMOTED_NOTE);
            store.set_run_engine_info(run, info)?;
            store.set_meeting_status(id, MeetingStatus::Done)?;
            r.action = RecoveryAction::PromotedLive;
        }
        None => store.set_meeting_status(id, MeetingStatus::Failed)?,
    }
    Ok(r)
}
