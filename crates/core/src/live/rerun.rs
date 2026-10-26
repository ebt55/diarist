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
//! a little later (and scan a little more silence for echo).
//! `playback.flac` is the only copy of the audio: it is only read, never
//! encoded again, moved or deleted, and its row stays. Before the decode
//! the job writes [`RERUN_MARKER`] beside it (flushed to the disk): crash
//! recovery then knows the WAVs for copies whatever it finds, and deletes
//! only the run the job made. The decode writes `.part` files and never
//! goes over WAVs that are there: a run from the FLAC is refused while
//! `mic.wav`, `loopback.wav` or the marker of an unfinished run are beside
//! it ([`LEFTOVERS`]), or when the disk has no room for the decoded audio.
//! The decoded WAVs go when the job ends, whatever happens, then the
//! marker. A job that fails or is cancelled leaves the meeting `done` with
//! the transcript it had: everything that can fail comes before the new
//! run becomes current. The new `post_pass` run becomes current and every
//! earlier run stays, the live run too (whatever `keep_live_runs` says).
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
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    live_run_of, register_wavs, submit_post_pass, LiveContext, PostPassHandle, PostPassJob,
};
use crate::audio::capture::audio_err;
use crate::audio::flac::{check_copies, decode_stereo_to_wavs, part_path, sync_dir};
use crate::error::{CoreError, Result};
use crate::models::free_disk_bytes;
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

/// Why a run from the FLAC does not start while files of an earlier one
/// are beside it (followed by their names).
pub const LEFTOVERS: &str = "Audio files are already beside this meeting's recording; finish or \
     clear them first. Restarting Diarist finishes an interrupted post-pass and deletes them \
     once they are copies of playback.flac; it leaves any it cannot match, for you to move out \
     of the meeting's folder. The transcript stays as it is.";

/// The marker a run from the stored audio keeps beside the WAVs it decodes,
/// from before the decode until they are gone (see the module docs).
pub const RERUN_MARKER: &str = "rerun-from-flac.json";

/// Room a run from the FLAC keeps free besides the decoded audio (the
/// database, the engine's log).
const SPACE_HEADROOM: u64 = 64 << 20;

/// What [`RERUN_MARKER`] holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RerunMarker {
    /// The job that decodes the WAVs.
    #[serde(default)]
    pub job: String,
    /// The `post_pass` run the job made, once it made one (recorded before
    /// anything is written to it): crash recovery deletes it unless it
    /// became current, and no other run.
    #[serde(default)]
    pub run: Option<i64>,
}

/// The marker in `folder`: `Ok(None)` only when there is none (`NotFound`);
/// one that does not parse still marks a run from the FLAC, its run
/// unknown; any other error is returned (the caller must not guess).
pub(super) fn read_marker(folder: &Path) -> std::io::Result<Option<RerunMarker>> {
    match std::fs::read(folder.join(RERUN_MARKER)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).unwrap_or_default())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes the marker durably: a temporary file flushed to the disk, renamed
/// over the marker, the folder flushed where the platform allows.
fn write_marker(folder: &Path, marker: &RerunMarker) -> Result<()> {
    let path = folder.join(RERUN_MARKER);
    let tmp = folder.join(format!("{RERUN_MARKER}.tmp"));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec(marker).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        audio_err(format!("write {}: {e}", path.display()))
    })?;
    sync_dir(Some(folder));
    Ok(())
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

/// The meeting's registered playback FLAC when its file is `flac`: then it
/// is the meeting's audio, which no post-pass encodes again and a re-run
/// decodes. `Ok(None)` when there is no such row or no such file, and
/// "no such file" means `NotFound` only: a file that cannot be looked at
/// for any other reason (locked, no access, a folder in its place) is an
/// error, so that nobody takes the only copy of the audio for gone.
pub(super) fn playback_flac(
    store: &dyn RunStore,
    data_root: &Path,
    meeting: MeetingId,
    flac: &Path,
) -> Result<Option<AudioTrack>> {
    match std::fs::metadata(flac) {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return Err(audio_err(format!(
                "{} is there but is not a file",
                flac.display()
            )))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(audio_err(format!("cannot look at {}: {e}", flac.display()))),
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

/// Why WAVs beside a registered FLAC must stay ([`flac_holds`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotHeld {
    /// The FLAC cannot be read now (locked, no access): nothing is known.
    Unreadable(String),
    /// It was read and is not what the WAVs hold (another size than its
    /// row, a frame or the MD5 fails, other samples).
    Differs(String),
}

impl std::fmt::Display for NotHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotHeld::Unreadable(s) | NotHeld::Differs(s) => f.write_str(s),
        }
    }
}

/// Before WAVs beside a registered FLAC are deleted: the FLAC has the size
/// its row recorded, decodes in full and to its MD5, and holds what each
/// WAV holds ([`check_copies`]). Why not, when it does not.
pub(super) fn flac_holds(
    flac: &Path,
    track: &AudioTrack,
    wavs: [Option<&Path>; 2],
) -> std::result::Result<(), NotHeld> {
    let bytes = std::fs::metadata(flac)
        .map_err(|e| NotHeld::Unreadable(format!("cannot look at {}: {e}", flac.display())))?
        .len();
    if let Some(row) = track.size_bytes {
        if row >= 0 && bytes != row as u64 {
            return Err(NotHeld::Differs(format!(
                "{} is {bytes} bytes, its track row says {row}",
                flac.display()
            )));
        }
    }
    if let Err(e) = std::fs::File::open(flac) {
        return Err(NotHeld::Unreadable(format!(
            "cannot read {}: {e}",
            flac.display()
        )));
    }
    check_copies(flac, wavs).map_err(|e| NotHeld::Differs(e.to_string()))
}

/// The files of an earlier run beside the recording that stop a run from
/// the FLAC: the marker of one that did not finish, `mic.wav`,
/// `loopback.wav` (a decode never goes over WAVs it did not make).
fn no_leftovers(folder: &Path) -> Result<()> {
    let mut there = Vec::new();
    for f in [RERUN_MARKER, "mic.wav", "loopback.wav"] {
        match std::fs::symlink_metadata(folder.join(f)) {
            Ok(_) => there.push(f),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(StoreError::Invalid(format!(
                    "Cannot look at {f} beside this meeting's recording ({e}). The transcript \
                     stays as it is."
                ))
                .into())
            }
        }
    }
    if there.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("{LEFTOVERS} ({})", there.join(", "))).into())
    }
}

/// Bytes a run from the FLAC writes beside it: the two decoded WAVs (about
/// 115 MB per channel-hour), the canceller's `mic-aec.wav` (as long as the
/// mic) and [`SPACE_HEADROOM`].
pub(super) fn decode_space(track: &AudioTrack) -> u64 {
    let flac_frames = track.duration_ms.unwrap_or(0).max(0) as u64 * 16;
    let [mic, lb] = track
        .channel_frames
        .as_deref()
        .and_then(|f| <[u64; 2]>::try_from(f).ok())
        .unwrap_or([flac_frames; 2]);
    (2 * mic + lb) * 2 + 3 * 44 + SPACE_HEADROOM
}

/// Refuses when `free` (bytes on the volume of `folder`, `None` unknown) is
/// under `needed`.
pub(super) fn space_check(folder: &Path, free: Option<u64>, needed: u64) -> Result<()> {
    match free {
        Some(free) if free < needed => Err(CoreError::InsufficientSpace {
            path: folder.display().to_string(),
            free_mb: free >> 20,
            needed_mb: needed.div_ceil(1 << 20),
        }),
        _ => Ok(()),
    }
}

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

/// The audio a re-run of `m` would read: the registered `playback.flac`
/// first (when it is there it holds everything: it is registered only once
/// encoded in full), else both WAVs. An error when the FLAC cannot be
/// looked at ([`playback_flac`]).
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
/// same reason, and also, with their own message, while files of an
/// earlier run are beside the FLAC or the disk is short of room).
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
/// run (see [`super::postpass`]). The meeting becomes `processing` last,
/// once nothing else can fail; the handle reports the result.
///
/// Refuses (`Store(Invalid)`) a meeting that is not a live recording, has no
/// live run ([`NO_LIVE_RUN`]) or neither its FLAC nor both WAVs
/// ([`AUDIO_GONE`]), a run from the FLAC while files of an earlier one are
/// beside it ([`LEFTOVERS`]), (`InsufficientSpace`) one from the FLAC
/// without room for the decoded audio, and (`Store(Conflict)`) one that is
/// still `recording` or already `processing`.
pub fn rerun_post_pass(ctx: &LiveContext, meeting: MeetingId) -> Result<PostPassHandle> {
    let store: &dyn RunStore = &*ctx.store;
    let m = store.get_meeting(meeting)?.meeting;
    let (live_run, audio) = ready(store, &ctx.data_root, &m)?;
    let folder = ctx.data_root.join("audio").join(&m.uid);
    if audio == RerunAudio::Flac {
        no_leftovers(&folder)?;
        let track = playback_flac(
            store,
            &ctx.data_root,
            meeting,
            &folder.join("playback.flac"),
        )?
        .ok_or_else(|| StoreError::Invalid(AUDIO_GONE.into()))?;
        space_check(&folder, free_disk_bytes(&folder), decode_space(&track))?;
    }
    let stored = settings::effective(store)?;
    discard_unfinished_post_passes(store, meeting, live_run)?;
    // The FLAC stays the playable track while WAVs decoded from it exist.
    if audio == RerunAudio::Wavs {
        let duration_ms = m.duration_ms.unwrap_or(0);
        register_wavs(store, meeting, &ctx.data_root, &folder, duration_ms)?;
    }
    // Last: a refusal or a store error above leaves the meeting as it was.
    store.set_meeting_status(meeting, MeetingStatus::Processing)?;
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

/// The files a job made beside `playback.flac` (the decode's `.part` files,
/// then the two WAVs), deleted when this drops, never the FLAC nor a file
/// the job did not make; then [`RERUN_MARKER`], once they are all gone.
pub(super) struct DecodedCopies {
    folder: PathBuf,
    files: Vec<PathBuf>,
    marker: RerunMarker,
}

impl DecodedCopies {
    /// Records the job's new run in the marker (before anything is written
    /// to the run): crash recovery deletes it unless it became current.
    pub(super) fn record_run(&mut self, run: RunId) -> Result<()> {
        self.marker.run = Some(run.0);
        write_marker(&self.folder, &self.marker)
    }
}

impl Drop for DecodedCopies {
    fn drop(&mut self) {
        let mut gone = true;
        for p in &self.files {
            match std::fs::remove_file(p) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    gone = false;
                    log::warn!(
                        "{}: cannot delete this decoded copy of playback.flac ({e}); the next start deletes it",
                        p.display()
                    );
                }
            }
        }
        if gone {
            match std::fs::remove_file(self.folder.join(RERUN_MARKER)) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => log::warn!(
                    "{}: cannot delete {RERUN_MARKER} ({e}); the next start deletes it",
                    self.folder.display()
                ),
            }
        }
    }
}

/// Audio between two progress reports of the decode (one minute).
const DECODE_REPORT_FRAMES: u64 = 60 * 16_000;

/// A FLAC-sourced job's first step ([`super::postpass::run_post_pass`]):
/// checks again that nothing of an earlier run is beside the FLAC and that
/// the decoded audio fits on the disk, writes [`RERUN_MARKER`], and decodes
/// the meeting's registered `playback.flac` into `mic.wav` and
/// `loopback.wav` with each channel's stored length. `progress(done_ms,
/// total_ms)`. The WAVs go when the returned guard drops (and the `.part`
/// files when this fails part way), then the marker.
pub(super) fn decode_wavs(
    store: &dyn RunStore,
    job: &PostPassJob,
    progress: &dyn Fn(u64, Option<u64>),
) -> Result<DecodedCopies> {
    let (flac, mic, lb) = (job.flac(), job.mic_wav(), job.loopback_wav());
    let track = playback_flac(store, &job.data_root, job.meeting, &flac)?.ok_or_else(|| {
        StoreError::Invalid(
            "The stored audio (playback.flac) is not there any more, so the post-pass cannot \
             run again. The transcript stays as it is."
                .into(),
        )
    })?;
    no_leftovers(&job.folder)?;
    space_check(
        &job.folder,
        free_disk_bytes(&job.folder),
        decode_space(&track),
    )?;
    let marker = RerunMarker {
        job: job.job_id.to_string(),
        run: None,
    };
    write_marker(&job.folder, &marker)?;
    let mut copies = DecodedCopies {
        folder: job.folder.clone(),
        files: vec![part_path(&mic), part_path(&lb)],
        marker,
    };
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
    copies.files.extend([mic, lb]);
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
