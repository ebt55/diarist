//! Running a live meeting's post-pass again: after it failed or was
//! cancelled, from the recorded WAVs; on a finished meeting, whose WAVs went
//! with its first post-pass, from the stored `playback.flac` ("Process
//! again", plan 16 section 12.5 J).
//!
//! From the FLAC ([`PostPassJob::from_flac`]): the job decodes the two WAVs
//! beside `loopback.gain` (the gain track is found by the WAV's name) when
//! it starts, each channel cut back to the length stored with the FLAC's
//! track row (`audio_tracks.channel_frames`). Tracks registered before
//! beta.5 have none, so both channels get the FLAC's length: Stop pads the
//! shorter WAV to the longer anyway, and only a recording crash recovery
//! finished with channels of different lengths can then end its last word
//! a little later. `playback.flac` is the only copy of the audio: it is
//! only read, never encoded again, moved or deleted, and its row stays;
//! the decoded WAVs go when the job ends, whatever happens; a job that
//! fails or is cancelled leaves the meeting `done` with the transcript it
//! had. The new `post_pass` run becomes current and the earlier one stays.
//!
//! Known limits (beta.5): a re-run needs the live run, whose mic words feed
//! the echo transfer gate and whose names carry over (`keep_live_runs`, off
//! by default, keeps it; so does a live name the first post-pass could not
//! place); without one it is not offered. Names and edits carry over from
//! the live run, or from a Restore or punctuation copy of it that is current
//! (plan 16 section 3.9 step 7), never from a post-pass run: names given on
//! the current post-pass run stay on it, as an earlier run, and the new run
//! does not get them.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use ts_rs::TS;

use super::{
    live_run_of, register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob,
};
use crate::audio::flac::decode_stereo_to_wavs;
use crate::error::Result;
use crate::settings;
use crate::store::{RunStore, StoreError};
use crate::transcript::SegmentOptions;
use crate::types::{
    AudioFormat, AudioTrack, JobId, Meeting, MeetingId, MeetingStatus, RunId, RunKind, RunStatus,
    Source, TrackChannel,
};

/// Why a meeting without a live run cannot run again.
pub const NO_LIVE_RUN: &str = "This meeting has no live transcript to replace. Processing again \
     needs it: Settings, \"Keep the live transcript\", keeps it for meetings recorded from then on.";

/// Why a meeting without audio cannot run again.
pub const AUDIO_GONE: &str = "The recorded audio is gone (neither the two recorded channels nor \
     playback.flac are in the meeting's folder), so the post-pass cannot run again. The \
     transcript stays as it is.";

/// The audio a re-run reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RerunAudio {
    /// `mic.wav` and `loopback.wav`, still there after a failed or
    /// cancelled post-pass.
    Wavs,
    /// The registered `playback.flac`, decoded when the job starts.
    Flac,
}

/// Whether a meeting's post-pass can run again now ([`rerun_check`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RerunCheck {
    /// The audio it would read (`None`: there is none).
    pub audio: Option<RerunAudio>,
    /// Why it cannot run now, in the user's words (`None`: it can).
    pub blocked: Option<String>,
}

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

/// The meeting's registered playback FLAC when its file is `flac` and is
/// there: then it is the meeting's audio, which no post-pass encodes again
/// and a re-run decodes.
pub(super) fn playback_flac(
    store: &dyn RunStore,
    data_root: &Path,
    meeting: MeetingId,
    flac: &Path,
) -> Result<Option<AudioTrack>> {
    if !flac.is_file() {
        return Ok(None);
    }
    Ok(store.list_audio_tracks(meeting)?.into_iter().find(|t| {
        let p = Path::new(&t.path);
        let at = if p.is_absolute() {
            p.to_path_buf()
        } else {
            data_root.join(p)
        };
        t.channel == TrackChannel::Playback
            && t.format == AudioFormat::Flac
            && t.channels == 2
            && at == flac
    }))
}

/// The audio a re-run of `m` would read: the registered `playback.flac`
/// first (when it is there it holds everything: it is registered only once
/// encoded in full, so WAVs beside it are copies), else both WAVs.
fn audio_of(store: &dyn RunStore, data_root: &Path, m: &Meeting) -> Result<Option<RerunAudio>> {
    let folder = data_root.join("audio").join(&m.uid);
    if playback_flac(store, data_root, m.id, &folder.join("playback.flac"))?.is_some() {
        return Ok(Some(RerunAudio::Flac));
    }
    let wavs = ["mic.wav", "loopback.wav"]
        .iter()
        .all(|f| folder.join(f).is_file());
    Ok(wavs.then_some(RerunAudio::Wavs))
}

/// What stops `m` from running again, as a refusal: not a live recording,
/// still recording or processing, no live run, no audio. Otherwise the
/// live run and the audio.
fn ready(
    store: &dyn RunStore,
    data_root: &Path,
    m: &Meeting,
) -> std::result::Result<(RunId, RerunAudio), StoreError> {
    if m.source != Source::Live {
        return Err(StoreError::Invalid(
            "Only live recordings have a post-pass.".into(),
        ));
    }
    match m.status {
        MeetingStatus::Recording => {
            return Err(StoreError::Conflict(
                "This meeting is still recording.".into(),
            ))
        }
        MeetingStatus::Processing => {
            return Err(StoreError::Conflict(
                "A post-pass for this meeting is already queued or running.".into(),
            ))
        }
        MeetingStatus::Done | MeetingStatus::Failed => {}
    }
    let live_run = live_run_of(&store.list_runs(m.id)?)
        .map(|r| r.id)
        .ok_or_else(|| StoreError::Invalid(NO_LIVE_RUN.into()))?;
    let audio = audio_of(store, data_root, m)
        .map_err(|e| StoreError::Invalid(e.to_string()))?
        .ok_or_else(|| StoreError::Invalid(AUDIO_GONE.into()))?;
    Ok((live_run, audio))
}

/// Whether the post-pass of `meeting` can run again now, and from which
/// audio (the UI's "Process again"; [`rerun_post_pass`] refuses with the
/// same reason).
pub fn rerun_check(
    store: &dyn RunStore,
    data_root: &Path,
    meeting: MeetingId,
) -> Result<RerunCheck> {
    let m = store.get_meeting(meeting)?.meeting;
    let audio = audio_of(store, data_root, &m)?;
    Ok(RerunCheck {
        audio,
        blocked: ready(store, data_root, &m).err().map(|e| match e {
            StoreError::Invalid(s) | StoreError::Conflict(s) => s,
            other => other.to_string(),
        }),
    })
}

/// Queues the post-pass of a live meeting again, behind jobs already queued:
/// the file engine over the recorded `mic.wav` and `loopback.wav`, or, when
/// the meeting has its registered `playback.flac`, over the two WAVs decoded
/// from it when the job starts (see the module docs); with the echo
/// canceller when it engages, and its transfer gate on the live run's mic
/// words, or on `mic.wav` transcribed once more when the live run has none,
/// of the latest live run, never a copy Restore or a punctuation rebuild
/// made of it ([`super::live_run_of`]); names and edits carry over from the
/// current run when it is such a copy of the live run, else from the live
/// run (see [`super::postpass`]). The meeting becomes `processing`; the
/// handle reports the result.
///
/// Refuses (`Store(Invalid)`) a meeting that is not a live recording, has no
/// live run ([`NO_LIVE_RUN`]) or neither its FLAC nor both WAVs
/// ([`AUDIO_GONE`]), and (`Store(Conflict)`) one that is still `recording`
/// or already `processing`.
pub fn rerun_post_pass(ctx: &LiveContext, meeting: MeetingId) -> Result<PostPassHandle> {
    let store: &dyn RunStore = &*ctx.store;
    let m = store.get_meeting(meeting)?.meeting;
    let (live_run, audio) = ready(store, &ctx.data_root, &m)?;
    let folder = ctx.data_root.join("audio").join(&m.uid);
    discard_unfinished_post_passes(store, meeting, live_run)?;
    // The FLAC stays the playable track while WAVs decoded from it exist.
    if audio == RerunAudio::Wavs {
        let duration_ms = m.duration_ms.unwrap_or(0);
        register_wavs(store, meeting, &ctx.data_root, &folder, duration_ms)?;
    }
    store.set_meeting_status(meeting, MeetingStatus::Processing)?;
    let stored = settings::effective(store)?;
    log::info!(
        "post-pass of meeting {meeting} queued again from {}",
        match audio {
            RerunAudio::Wavs => "the recorded WAVs",
            RerunAudio::Flac => "playback.flac",
        }
    );
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
            from_flac: audio == RerunAudio::Flac,
        },
        false,
    ))
}

/// The WAVs a job decoded from `playback.flac`, deleted when this drops
/// (with the canceller's output); never the FLAC.
pub(super) struct DecodedCopies(Vec<PathBuf>);

impl Drop for DecodedCopies {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Audio between two progress reports of the decode (one minute).
const DECODE_REPORT_FRAMES: u64 = 60 * 16_000;

/// A FLAC-sourced job's first step ([`super::postpass::run_post_pass`]):
/// decodes the meeting's registered `playback.flac` into `mic.wav` and
/// `loopback.wav` with each channel's stored length. `progress(done_ms,
/// total_ms)`. The WAVs go when the returned guard drops (also when this
/// fails part way).
pub(super) fn decode_wavs(
    store: &dyn RunStore,
    job: &PostPassJob,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<DecodedCopies> {
    let (flac, mic, lb) = (job.flac(), job.mic_wav(), job.loopback_wav());
    let copies = DecodedCopies(vec![mic.clone(), lb.clone(), job.mic_aec_wav()]);
    let track = playback_flac(store, &job.data_root, job.meeting, &flac)?.ok_or_else(|| {
        StoreError::Invalid(
            "The stored audio (playback.flac) is not there any more, so the post-pass cannot \
             run again. The transcript stays as it is."
                .into(),
        )
    })?;
    let lengths = track
        .channel_frames
        .as_deref()
        .and_then(|f| <[u64; 2]>::try_from(f).ok());
    let t0 = Instant::now();
    let reported = Cell::new(0u64);
    let ms = |frames: u64| frames * 1_000 / 16_000;
    let got = decode_stereo_to_wavs(&flac, &mic, &lb, lengths, &|done, total| {
        if done >= reported.get() + DECODE_REPORT_FRAMES || done == total {
            reported.set(done);
            progress(ms(done), (total > 0).then(|| ms(total)));
        }
    })?;
    log::info!(
        "post-pass meeting {}: playback.flac decoded into the two WAVs ({} frames, channels {:?}{}, MD5 {}, {:.1} s); the FLAC stays as it is",
        job.meeting,
        got.frames,
        got.channel_frames,
        if lengths.is_some() { "" } else { ", no stored lengths" },
        if got.verified { "checked" } else { "not in the file" },
        t0.elapsed().as_secs_f64()
    );
    Ok(copies)
}

#[cfg(test)]
mod tests;
