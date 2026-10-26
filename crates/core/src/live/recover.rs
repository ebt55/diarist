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
//! - A meeting with its registered `playback.flac` (a post-pass that got as
//!   far as registering it), or with the marker of a run from the stored
//!   audio ([`super::rerun::RERUN_MARKER`], plan 16 section 12.5 J), is
//!   finished as it is instead, whatever WAVs are beside it: the FLAC is
//!   never touched nor encoded again; `mic.wav` and `loopback.wav` go only
//!   once the FLAC has its recorded size, decodes in full and holds what
//!   they hold ([`super::rerun::flac_holds`]); the `.part` files and
//!   `mic-aec.wav` go; the run the marker names goes unless it is current,
//!   with any unfinished `post_pass` run, and no other run; the marker goes
//!   last; the meeting is `done` (`failed` without a current run). This also
//!   runs for a `done` or `failed` meeting that still has such files beside
//!   its FLAC ([`needs_recovery`]), so a restart finishes what a job could
//!   not delete.
//! - When it cannot tell (the FLAC cannot be looked at for any reason but
//!   `NotFound`, the marker is there and the FLAC is not, the WAVs are not
//!   copies of the FLAC), recovery changes nothing but the status: the
//!   meeting is `failed`, [`RecoveredMeeting::note`] says why, and every
//!   file and run stays ([`RecoveryAction::Untouched`]).

use std::io::ErrorKind;
use std::path::Path;

use super::rerun::{
    discard_unfinished_post_passes, flac_holds, playback_flac, read_marker, NotHeld, RerunMarker,
    RERUN_MARKER,
};
use super::{
    live_run_of, register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob,
};
use crate::audio::flac::part_path;
use crate::audio::wav::repair_header;
use crate::error::Result;
use crate::punct::rebuild::descends_from;
use crate::settings;
use crate::store::{RunStore, StoreError};
use crate::transcript::SegmentOptions;
use crate::types::{
    AudioTrack, JobId, Meeting, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source,
};
use crate::views::MeetingSummary;

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
    /// Recovery could not tell what the files beside the recording are, so
    /// it changed nothing but the status (`failed`); the note says why.
    Untouched,
}

/// One meeting [`recover`] picked up.
pub struct RecoveredMeeting {
    pub meeting_id: MeetingId,
    /// `recording` or `processing`; `done` or `failed` for a meeting with
    /// files of an interrupted post-pass beside its FLAC.
    pub was: MeetingStatus,
    pub action: RecoveryAction,
    pub live_run: Option<RunId>,
    /// Sample frames found in each WAV (after repair; 0 when missing).
    pub mic_frames: u64,
    pub loopback_frames: u64,
    /// The queued post-pass (`None` when there was no audio to process).
    pub post_pass: Option<PostPassHandle>,
    /// For the user: why the meeting was left as it is, or what could not
    /// be deleted (the next start tries again).
    pub note: Option<String>,
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
        from_flac: false,
    })
}

/// Whether [`recover`] has something to do for `m`: a live meeting still
/// `recording` or `processing`, or a `done` or `failed` one with the files
/// of an interrupted post-pass beside its recording (the marker of a run
/// from the FLAC, or `mic.wav` / `loopback.wav` beside its registered
/// `playback.flac`). What cannot be looked at counts as nothing to do here.
pub fn needs_recovery(store: &dyn RunStore, data_root: &Path, m: &MeetingSummary) -> bool {
    if m.source != Source::Live {
        return false;
    }
    match m.status {
        MeetingStatus::Recording | MeetingStatus::Processing => true,
        MeetingStatus::Done | MeetingStatus::Failed => {
            let folder = data_root.join("audio").join(&m.uid);
            let there = |f: &str| std::fs::symlink_metadata(folder.join(f)).is_ok();
            there(RERUN_MARKER)
                || ((there("mic.wav") || there("loopback.wav"))
                    && matches!(
                        playback_flac(store, data_root, m.id, &folder.join("playback.flac")),
                        Ok(Some(_))
                    ))
        }
    }
}

/// Finishes every live meeting a crash left in `recording` or `processing`,
/// and every finished one with files of an interrupted post-pass beside its
/// recording (see the module docs). Never call it while a
/// [`super::Recorder`] runs or a post-pass is queued.
pub fn recover(ctx: &LiveContext) -> Result<Vec<RecoveredMeeting>> {
    let store: &dyn RunStore = &*ctx.store;
    let mut out = Vec::new();
    for m in store.list_meetings()? {
        if !needs_recovery(store, &ctx.data_root, &m) {
            continue;
        }
        let r = match m.status {
            MeetingStatus::Recording => recover_recording(ctx, m.id)?,
            MeetingStatus::Processing => recover_processing(ctx, m.id)?,
            MeetingStatus::Done | MeetingStatus::Failed => {
                match beside_flac(store, &ctx.data_root, m.id, m.status)? {
                    Some(r) => r,
                    None => continue,
                }
            }
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
        note: None,
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
    if let Some(r) = beside_flac(store, &ctx.data_root, id, MeetingStatus::Processing)? {
        return Ok(r);
    }
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
        note: None,
    };
    let current = detail.current_run;
    match live {
        // The WAVs are the recording (no registered FLAC, no marker: see
        // `beside_flac`).
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

/// A meeting whose audio is its registered `playback.flac`, or that a run
/// from it marked: finished as it is ([`finish_beside_flac`]), or left
/// untouched when recovery cannot tell (see the module docs). `None`: no
/// marker and no registered FLAC (`NotFound`), the WAVs are the recording.
fn beside_flac(
    store: &dyn RunStore,
    data_root: &Path,
    id: MeetingId,
    was: MeetingStatus,
) -> Result<Option<RecoveredMeeting>> {
    let meeting = store.get_meeting(id)?.meeting;
    let folder = data_root.join("audio").join(&meeting.uid);
    let flac = folder.join("playback.flac");
    let marker = match read_marker(&folder) {
        Ok(m) => m,
        Err(e) => {
            return untouched(
                store,
                id,
                was,
                format!(
                    "{RERUN_MARKER} beside the recording cannot be read ({e}), so recovery \
                     cannot tell what the files beside it are and changed nothing."
                ),
            )
            .map(Some)
        }
    };
    let track = match playback_flac(store, data_root, id, &flac) {
        Ok(Some(t)) => t,
        Ok(None) if marker.is_some() => {
            return untouched(
                store,
                id,
                was,
                format!(
                    "playback.flac is not in the meeting's folder ({}) or not registered, and an \
                     interrupted Process again left the files it decoded from it there. Nothing \
                     was deleted. Put playback.flac back and restart Diarist; if it is lost, \
                     Retry runs the post-pass from the two WAVs (copies of it, once both are \
                     there) and stores them again.",
                    folder.display()
                ),
            )
            .map(Some)
        }
        Ok(None) => return Ok(None),
        Err(e) => return untouched(store, id, was, flac_out_of_reach(&e.to_string())).map(Some),
    };
    finish_beside_flac(store, id, was, &folder, &flac, &track, marker).map(Some)
}

/// The note for a registered FLAC that cannot be looked at or read now.
fn flac_out_of_reach(e: &str) -> String {
    format!(
        "{e}. It is this meeting's only copy of the audio, so recovery changed nothing: the \
         transcripts, playback.flac and the files beside it are as they were. Once it can be \
         read again, restart Diarist."
    )
}

/// Changes nothing but the status: `failed`, with why.
fn untouched(
    store: &dyn RunStore,
    id: MeetingId,
    was: MeetingStatus,
    note: String,
) -> Result<RecoveredMeeting> {
    store.set_meeting_status(id, MeetingStatus::Failed)?;
    log::warn!("recovery: meeting {id} left as it is (failed): {note}");
    Ok(RecoveredMeeting {
        meeting_id: id,
        was,
        action: RecoveryAction::Untouched,
        live_run: live_run_of(&store.list_runs(id)?).map(|r| r.id),
        mic_frames: 0,
        loopback_frames: 0,
        post_pass: None,
        note: Some(note),
    })
}

/// The files beside a registered `playback.flac` (see the module docs):
/// the WAVs go only as copies of it, never the FLAC; the marked run unless
/// current and any unfinished `post_pass` run go, no other run; the current
/// run stays and the meeting is `done` (`failed` without any run to show).
fn finish_beside_flac(
    store: &dyn RunStore,
    id: MeetingId,
    was: MeetingStatus,
    folder: &Path,
    flac: &Path,
    track: &AudioTrack,
    marker: Option<RerunMarker>,
) -> Result<RecoveredMeeting> {
    let (mic, lb) = (folder.join("mic.wav"), folder.join("loopback.wav"));
    let mut wavs: [Option<&Path>; 2] = [None, None];
    for (slot, p) in wavs.iter_mut().zip([&mic, &lb]) {
        match std::fs::symlink_metadata(p) {
            Ok(_) => *slot = Some(p.as_path()),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return untouched(
                    store,
                    id,
                    was,
                    format!(
                        "{} cannot be looked at ({e}), so recovery changed nothing.",
                        p.display()
                    ),
                )
            }
        }
    }
    if wavs.iter().any(Option::is_some) {
        match flac_holds(flac, track, wavs) {
            Ok(()) => {}
            Err(NotHeld::Unreadable(e)) => {
                return untouched(store, id, was, flac_out_of_reach(&e));
            }
            Err(NotHeld::Differs(why)) => {
                return untouched(
                    store,
                    id,
                    was,
                    format!(
                        "The WAVs beside playback.flac could not be matched to it ({why}), so \
                         recovery deleted nothing: the transcripts, playback.flac and the WAVs \
                         are as they were. Keep whichever holds the recording and move the rest \
                         out of {}.",
                        folder.display()
                    ),
                );
            }
        }
    }
    let mut stuck = Vec::new();
    for p in [
        mic.clone(),
        lb.clone(),
        part_path(&mic),
        part_path(&lb),
        folder.join(super::postpass::MIC_AEC_WAV),
    ] {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => stuck.push(format!("{} ({e})", p.display())),
        }
    }
    for t in store.list_audio_tracks(id)? {
        if t.format == crate::types::AudioFormat::Wav
            && matches!(
                t.channel,
                crate::types::TrackChannel::Mic | crate::types::TrackChannel::Loopback
            )
        {
            store.delete_audio_track(t.id)?;
        }
    }
    let current = store.get_meeting(id)?.current_run.map(|r| r.id);
    // The run a run from the FLAC made, when it did not become current
    // (finished or not); never any other.
    if let Some(run) = marker.as_ref().and_then(|m| m.run).map(RunId) {
        if Some(run) != current {
            match store.delete_run(run) {
                Ok(()) | Err(StoreError::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    let live = live_run_of(&store.list_runs(id)?).map(|r| r.id);
    if let Some(run) = live {
        discard_unfinished_post_passes(store, id, run)?;
    }
    if marker.is_some() && stuck.is_empty() {
        match std::fs::remove_file(folder.join(RERUN_MARKER)) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => stuck.push(format!("{RERUN_MARKER} ({e})")),
        }
    }
    let (status, action) = if current.is_some() {
        (MeetingStatus::Done, RecoveryAction::Finished)
    } else {
        (MeetingStatus::Failed, RecoveryAction::Failed)
    };
    store.set_meeting_status(id, status)?;
    let note = (!stuck.is_empty()).then(|| {
        format!(
            "Could not delete {}; the next start tries again.",
            stuck.join(", ")
        )
    });
    log::info!(
        "recovery: meeting {id} keeps its playback.flac and its current run; the files beside it went{}",
        note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default()
    );
    Ok(RecoveredMeeting {
        meeting_id: id,
        was,
        action,
        live_run: live,
        mic_frames: 0,
        loopback_frames: 0,
        post_pass: None,
        note,
    })
}
