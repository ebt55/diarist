//! Running a live meeting's post-pass again (after it failed or was
//! cancelled): the recorded WAVs must still be there.

use super::{
    live_run_of, register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob,
};
use crate::error::Result;
use crate::settings;
use crate::store::{RunStore, StoreError};
use crate::transcript::SegmentOptions;
use crate::types::{JobId, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source};

/// Deletes the `post_pass` runs of `meeting` newer than `live_run` that never
/// finished (an interrupted post-pass leaves one `running`); a new post-pass
/// replaces them.
pub(super) fn discard_unfinished_post_passes(
    store: &dyn RunStore,
    meeting: MeetingId,
    live_run: RunId,
) -> Result<()> {
    for r in store.list_runs(meeting)? {
        if r.kind == RunKind::PostPass && r.id.0 > live_run.0 && r.status == RunStatus::Running {
            store.delete_run(r.id)?;
        }
    }
    Ok(())
}

/// Queues the post-pass of a live meeting again, behind jobs already queued:
/// the file engine over the recorded `mic.wav` and `loopback.wav` (with the
/// echo canceller when it engages, and its transfer gate on the live run's
/// mic words, or on `mic.wav` transcribed once more when the live run has
/// none) of the latest live run, never a copy Restore or a punctuation
/// rebuild made of it ([`super::live_run_of`]); names and edits carry over
/// from the current run when it is such a copy of the live run, else from
/// the live run (see [`super::postpass`]).
/// The meeting becomes `processing`; the handle reports the result.
///
/// Refuses (`Store(Invalid)`) a meeting that is not a live recording, has no
/// live run or no longer has both WAVs, and (`Store(Conflict)`) one that is
/// still `recording` or already `processing`.
pub fn rerun_post_pass(ctx: &LiveContext, meeting: MeetingId) -> Result<PostPassHandle> {
    let store: &dyn RunStore = &*ctx.store;
    let m = store.get_meeting(meeting)?.meeting;
    if m.source != Source::Live {
        return Err(StoreError::Invalid("Only live recordings have a post-pass.".into()).into());
    }
    match m.status {
        MeetingStatus::Recording => {
            return Err(StoreError::Conflict("This meeting is still recording.".into()).into())
        }
        MeetingStatus::Processing => {
            return Err(StoreError::Conflict(
                "A post-pass for this meeting is already queued or running.".into(),
            )
            .into())
        }
        MeetingStatus::Done | MeetingStatus::Failed => {}
    }
    let live_run = live_run_of(&store.list_runs(meeting)?)
        .map(|r| r.id)
        .ok_or_else(|| {
            StoreError::Invalid("This meeting has no live transcript to replace.".into())
        })?;
    let folder = ctx.data_root.join("audio").join(&m.uid);
    for f in ["mic.wav", "loopback.wav"] {
        if !folder.join(f).is_file() {
            return Err(StoreError::Invalid(format!(
                "The recorded audio ({f}) is gone, so the post-pass cannot run again. \
                 The live transcript stays."
            ))
            .into());
        }
    }
    discard_unfinished_post_passes(store, meeting, live_run)?;
    let duration_ms = m.duration_ms.unwrap_or(0);
    register_wavs(store, meeting, &ctx.data_root, &folder, duration_ms)?;
    store.set_meeting_status(meeting, MeetingStatus::Processing)?;
    let stored = settings::effective(store)?;
    Ok(submit_post_pass(
        ctx,
        PostPassJob {
            job_id: JobId::new(),
            meeting,
            live_run,
            data_root: ctx.data_root.clone(),
            folder,
            language: m.language.clone(),
            diarize_mic: m.diarize_mic,
            transcribe: true,
            keep_live_runs: settings::get_bool(&stored, settings::KEEP_LIVE_RUNS),
            segment: SegmentOptions::default(),
            punct: ctx.punct.clone(),
        },
        false,
    ))
}
