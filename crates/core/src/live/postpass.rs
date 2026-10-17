//! The canonical post-pass of a live meeting (`docs/01`, "Live flow" step 4).
//!
//! Runs as a [`crate::pipeline::JobQueue`] job after Stop (or after crash
//! recovery): the file engine over `loopback.wav` (diarized) and `mic.wav`
//! (speaker `me`, diarized only with `diarize_mic`), word times mapped from
//! channel time to meeting time through the clock anchors, each channel
//! segmented on its own and the segments ordered by start time
//! ([`build_transcript_channels`]) into a `post_pass` run, names and edits carried
//! over from the live run, the new run made current, the stereo
//! `playback.flac` (left mic, right loopback) encoded and registered, the
//! WAVs deleted, and the live run deleted unless `keep_live_runs` or a
//! display name of the live run reached nobody in the new run (counted in
//! the new run's `engine_info.unplaced_names`, and logged with the rest of
//! the carry-over's counts). With
//! `transcribe = false` (`post_pass_mode = on_issues` and a clean
//! recording) the live run is promoted instead and only the audio work runs.
//! Words that come back mostly unknown-token markers fail the post-pass
//! before anything is stored ([`CoreError::UnknownTokens`]): the live run
//! and the WAVs stay, so it can run again.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use crate::audio::flac::encode_stereo;
use crate::engine::{DiarPreset, Engine, EngineEvent, EngineWord, ErrorCode, FileJob};
use crate::error::{CoreError, Result};
use crate::models::languages;
use crate::pipeline::CancelToken;
use crate::store::{CarryOverReport, NewAudioTrack, RunStore};
use crate::transcript::{
    build_transcript_channels, words_from_engine, SegmentOptions, TranscriptWord, ME_LABEL,
    UNKNOWN_LABEL,
};
use crate::types::{
    AudioFormat, Channel, JobId, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, TrackChannel,
};
use crate::views::{JobEvent, JobStage};

/// One post-pass to run.
#[derive(Debug, Clone)]
pub struct PostPassJob {
    pub job_id: JobId,
    pub meeting: MeetingId,
    pub live_run: RunId,
    /// The data root (tracks are stored relative to it).
    pub data_root: PathBuf,
    /// `audio/<uid>` under the data root.
    pub folder: PathBuf,
    pub language: String,
    pub diarize_mic: bool,
    /// Run the engine (false: promote the live run, audio work only).
    pub transcribe: bool,
    pub keep_live_runs: bool,
    pub segment: SegmentOptions,
}

impl PostPassJob {
    pub fn mic_wav(&self) -> PathBuf {
        self.folder.join("mic.wav")
    }
    pub fn loopback_wav(&self) -> PathBuf {
        self.folder.join("loopback.wav")
    }
    pub fn flac(&self) -> PathBuf {
        self.folder.join("playback.flac")
    }
}

/// What a finished post-pass did.
#[derive(Debug, Clone, PartialEq)]
pub struct PostPassReport {
    pub meeting: MeetingId,
    /// The run that is now current (the post-pass run, or the promoted live run).
    pub current_run: RunId,
    pub promoted_live: bool,
    pub live_run_deleted: bool,
    /// `(channel, engine label, display name)` of the current run's speakers.
    pub speakers: Vec<(Channel, String, Option<String>)>,
    pub words: usize,
    pub carry_over: Option<CarryOverReport>,
    /// Display names of the live run that no speaker of the new run
    /// received (the live run is then kept; also `engine_info.unplaced_names`).
    pub unplaced_names: usize,
    pub flac_path: PathBuf,
    pub flac_frames: u64,
    pub flac_bytes: u64,
    /// Both WAVs together, before they were deleted.
    pub wav_bytes: u64,
    /// Drift of each channel against QPC (diagnostics).
    pub drift_ppm: BTreeMap<String, f64>,
    pub transcribe_wall: Duration,
    pub wall: Duration,
}

fn wav_frames(p: &Path) -> Result<u64> {
    let r = hound::WavReader::open(p)
        .map_err(|e| CoreError::Decode(format!("open {}: {e}", p.display())))?;
    Ok(u64::from(r.duration()))
}

/// Runs one file job to completion; returns the latest revision of every
/// utterance's words, in order.
fn transcribe(
    engine: &dyn Engine,
    job: FileJob,
    cancel: &CancelToken,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> Result<Vec<EngineWord>> {
    let job_id = job.job_id.clone();
    let rx = engine.transcribe_file(job)?;
    let mut utterances: BTreeMap<u32, (u32, Vec<EngineWord>)> = BTreeMap::new();
    let mut cancel_sent = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(EngineEvent::Progress {
                done_ms, total_ms, ..
            }) => on_progress(done_ms, total_ms),
            Ok(EngineEvent::Utterance {
                utt, rev, words, ..
            }) => {
                let e = utterances.entry(utt).or_insert((rev, Vec::new()));
                if rev >= e.0 {
                    *e = (rev, words);
                }
            }
            Ok(EngineEvent::Done { .. }) => break,
            Ok(EngineEvent::Error { code, message, .. }) => {
                if cancel_sent || code == ErrorCode::Cancelled {
                    return Err(CoreError::Cancelled);
                }
                return Err(CoreError::Engine(format!("{code:?}: {message}")));
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() && !cancel_sent {
                    engine.cancel(job_id.clone());
                    cancel_sent = true;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CoreError::Engine(
                    "engine stopped without Done or Error".into(),
                ))
            }
        }
    }
    Ok(utterances.into_values().flat_map(|(_, w)| w).collect())
}

/// How many distinct display names of `live_run` (as the user sees them: a
/// merged speaker shows its root's) no speaker received in the carry-over.
fn unplaced_names(
    store: &dyn RunStore,
    live_run: RunId,
    report: &CarryOverReport,
) -> Result<usize> {
    let speakers = store.run_speakers(live_run)?;
    let name_of = |id| {
        speakers
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.display_name.clone())
    };
    let placed: HashSet<&str> = report.named.iter().map(|(_, n)| n.as_str()).collect();
    let unplaced: HashSet<String> = speakers
        .iter()
        .filter_map(|s| match s.merged_into {
            Some(root) => name_of(root),
            None => s.display_name.clone(),
        })
        .filter(|n| !placed.contains(n.as_str()))
        .collect();
    Ok(unplaced.len())
}

/// Maps a channel's words from channel time to meeting time and clamps
/// them to the channel's end (Nemotron's last word can end after the audio).
pub fn map_words(words: &mut [TranscriptWord], map: &ClockMap, channel_end_ms: i64) {
    let end = map.meeting_ms(channel_end_ms);
    for w in words.iter_mut() {
        let s = w.start_ms.min(channel_end_ms);
        let e = w.end_ms.clamp(s, channel_end_ms);
        w.start_ms = map.meeting_ms(s).min(end);
        w.end_ms = map.meeting_ms(e).clamp(w.start_ms, end.max(w.start_ms));
    }
}

/// Runs the post-pass (see the module docs). Reports `Progress` events;
/// the caller sends the terminal `Done` / `Error` / `Cancelled`.
pub fn run_post_pass(
    store: &dyn RunStore,
    engine: &dyn Engine,
    job: &PostPassJob,
    cancel: &CancelToken,
    on_event: &dyn Fn(JobEvent),
) -> Result<PostPassReport> {
    let started = Instant::now();
    let progress = |stage: JobStage, done_ms: u64, total_ms: Option<u64>| {
        on_event(JobEvent::Progress {
            job_id: job.job_id.clone(),
            stage,
            done_ms,
            total_ms,
        })
    };
    let (mic, lb) = (job.mic_wav(), job.loopback_wav());
    let mic_frames = wav_frames(&mic)?;
    let lb_frames = wav_frames(&lb)?;
    let wav_bytes = std::fs::metadata(&mic)?.len() + std::fs::metadata(&lb)?.len();
    let audio_ms = mic_frames.max(lb_frames) * 1_000 / 16_000;
    let total_ms = Some(audio_ms);

    // Clock maps from the stored anchors.
    let mut anchors = store.clock_anchors(job.meeting, Channel::Mic)?;
    anchors.extend(store.clock_anchors(job.meeting, Channel::Loopback)?);
    let origin = origin_from_anchors(&anchors).unwrap_or(0);
    let map_of = |ch: Channel| ClockMap::from_anchors(origin, &channel_anchors(&anchors, ch));
    let mut drift_ppm = BTreeMap::new();
    for ch in [Channel::Mic, Channel::Loopback] {
        if let Some(d) = map_of(ch).drift_ppm() {
            drift_ppm.insert(ch.to_string(), d);
        }
    }

    let mut transcribe_wall = Duration::ZERO;
    let mut carry = None;
    let mut unplaced = 0;
    let mut live_deleted = false;
    let current_run = if job.transcribe {
        let t0 = Instant::now();
        let mut channels: Vec<(Channel, Vec<TranscriptWord>)> = Vec::new();
        for (i, (ch, wav, diarize, frames)) in [
            (Channel::Loopback, &lb, true, lb_frames),
            (Channel::Mic, &mic, job.diarize_mic, mic_frames),
        ]
        .into_iter()
        .enumerate()
        {
            if cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let base = i as u64 * audio_ms;
            let words = transcribe(
                engine,
                FileJob {
                    job_id: JobId::new(),
                    wav_path: wav.clone(),
                    diarize,
                    language: job.language.clone(),
                    diar_preset: DiarPreset::Streaming,
                },
                cancel,
                &|done, _| progress(JobStage::Transcribing, (base + done) / 2, total_ms),
            )?;
            let default = if ch == Channel::Mic && !diarize {
                ME_LABEL
            } else {
                UNKNOWN_LABEL
            };
            let mut tw = words_from_engine(ch, &words, default);
            if ch == Channel::Mic && !diarize {
                for w in &mut tw {
                    w.speaker = ME_LABEL.to_owned();
                }
            }
            map_words(&mut tw, &map_of(ch), (frames * 1_000 / 16_000) as i64);
            tw.sort_by_key(|w| w.start_ms);
            channels.push((ch, tw));
        }
        transcribe_wall = t0.elapsed();
        // A language the model cannot transcribe: nothing is stored, and the
        // live transcript and the WAVs stay for a run with another model.
        if languages::unknown_token_output(
            channels
                .iter()
                .flat_map(|(_, ws)| ws.iter().map(|w| w.text.as_str())),
        ) {
            return Err(CoreError::UnknownTokens);
        }
        progress(JobStage::Saving, audio_ms, total_ms);
        // Mic first on equal start times.
        channels.reverse();
        let transcript = build_transcript_channels(channels, &job.segment);
        let info = engine.info();
        let run = store.create_run(
            job.meeting,
            RunKind::PostPass,
            serde_json::json!({
                "runtime": info.runtime,
                "models": info.models,
                "device": info.device,
                "right_context": crate::protocol::lookahead_json(info.right_context),
                "diar_preset": crate::protocol::diar_preset_text(info.diar_preset.as_deref()),
                "language": job.language,
                "diarize_mic": job.diarize_mic,
                "live_run": job.live_run.0,
                "clock_drift_ppm": drift_ppm,
                "segmentation": {
                    "max_gap_ms": job.segment.max_gap_ms,
                    "sentence_gap_ms": job.segment.sentence_gap_ms,
                    "max_words": job.segment.max_words,
                },
            }),
        )?;
        if let Err(e) = store.insert_transcript(run.id, &transcript) {
            let _ = store.finish_run(run.id, RunStatus::Failed);
            return Err(e.into());
        }
        store.finish_run(run.id, RunStatus::Done)?;
        let report = store.carry_over(job.live_run, run.id)?;
        // Names given on the live run that reached no speaker of the new
        // run: the live run is kept for them (recorded before the WAVs go,
        // so crash recovery keeps it too).
        unplaced = unplaced_names(store, job.live_run, &report)?;
        let mut info = run.engine_info.clone();
        info["unplaced_names"] = serde_json::json!(unplaced);
        store.set_run_engine_info(run.id, info)?;
        log::info!(
            "post-pass meeting {}: carry-over shift_ms {:?}, {} matched, {} named, {} need review, {} unplaced names",
            job.meeting,
            report.shift_ms,
            report.matched.len(),
            report.named.len(),
            report.needs_review.len(),
            unplaced
        );
        carry = Some(report);
        store.set_current_run(job.meeting, run.id)?;
        run.id
    } else {
        store.set_current_run(job.meeting, job.live_run)?;
        job.live_run
    };

    // The playback track: stereo FLAC, then the WAVs go. A post-pass that
    // was interrupted after registering its FLAC left a row to replace.
    let flac = job.flac();
    let info = encode_stereo(&mic, &lb, &flac)?;
    for t in store.list_audio_tracks(job.meeting)? {
        if t.channel == TrackChannel::Playback {
            store.delete_audio_track(t.id)?;
        }
    }
    let rel = |p: &Path| -> String {
        p.strip_prefix(&job.data_root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| p.display().to_string())
    };
    store.add_audio_track(NewAudioTrack {
        meeting_id: job.meeting,
        channel: TrackChannel::Playback,
        path: rel(&flac),
        format: AudioFormat::Flac,
        sample_rate: 16_000,
        channels: 2,
        duration_ms: Some((info.frames * 1_000 / 16_000) as i64),
        size_bytes: Some(info.bytes as i64),
    })?;
    for t in store.list_audio_tracks(job.meeting)? {
        if t.format == AudioFormat::Wav
            && matches!(t.channel, TrackChannel::Mic | TrackChannel::Loopback)
        {
            store.delete_audio_track(t.id)?;
        }
    }
    let _ = std::fs::remove_file(&mic);
    let _ = std::fs::remove_file(&lb);

    // A live run with a name nobody in the new run received stays, as if
    // `keep_live_runs` were on for this meeting.
    if job.transcribe && !job.keep_live_runs && unplaced == 0 {
        store.delete_run(job.live_run)?;
        live_deleted = true;
    }
    store.set_meeting_status(job.meeting, MeetingStatus::Done)?;

    let speakers = store
        .run_speakers(current_run)?
        .into_iter()
        .map(|s| (s.channel, s.engine_label, s.display_name))
        .collect();
    let words = store.get_words(job.meeting, i64::MIN, i64::MAX)?.len();
    Ok(PostPassReport {
        meeting: job.meeting,
        current_run,
        promoted_live: !job.transcribe,
        live_run_deleted: live_deleted,
        speakers,
        words,
        carry_over: carry,
        unplaced_names: unplaced,
        flac_path: flac,
        flac_frames: info.frames,
        flac_bytes: info.bytes,
        wav_bytes,
        drift_ppm,
        transcribe_wall,
        wall: started.elapsed(),
    })
}
