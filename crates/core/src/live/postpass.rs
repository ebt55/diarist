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
//!
//! Punctuation (plan 16 section 3.2), with the job's [`PunctSource`]: after
//! the echo step and the unknown-token check, before segmentation, both
//! channels are punctuated, each as its own stream
//! ([`crate::punct::punctuate_loaded`]); the report is the run's
//! `engine_info.punctuation`, and `echo_dropped` keeps the raw words. Names
//! carry over from the meeting's current run when it is a copy of the live
//! run (Restore, a punctuation rebuild; plan 16 section 3.9 step 7), else
//! from the live run. With `transcribe = false` and a punctuator that
//! changes the text of at least one word, the live run is not promoted as
//! it is: a `post_pass` run is built from its words (each under the speaker
//! row of its own segment, `punct::rebuild::source_run`), punctuated, with
//! `promoted_live`, the live run's `engine_info.echo` and `echo_dropped`,
//! every speaker row carried by identity; it becomes current and the live
//! run goes as after a transcribing post-pass. Without a punctuator (or
//! `off`), or when no word changed (a language the model does not cover, a
//! failure, nothing to mark), the live run is promoted as before.
//!
//! Echo (plan 15; nothing with `echo.mode = off`), on a thread of its own
//! while the loopback is transcribed ([`prepare_echo`]):
//! [`EchoProfile::scan`] judges the WAVs in 2 s blocks (always rescanned:
//! crash recovery and re-runs have no live monitor). When it found coupling
//! with an echo return loss under [`AEC_ERL_BELOW_DB`] (the AGCs' gain
//! tracks taken out, when the recording has them), or the mode is `always`,
//! Diarist's echo canceller ([`crate::audio::aec::offline`], chunk B2a)
//! runs over `mic.wav` with `loopback.wav` as the reference (its AGC
//! replayed backwards from `loopback.gain` when there is one) into a
//! temporary [`MIC_AEC_WAV`], which is transcribed instead of `mic.wav` and
//! deleted after the echo step. Then the layered gate
//! ([`crate::audio::echo::layers`], plan 15 section 9) drops the mic words
//! that are the far end's before segmentation: text matching against the
//! loopback's words and the near-end level (on what was transcribed) on
//! every recording, once the text matches show echo; the canceller's
//! attenuation of each word where it ran; the correlation, scored against
//! the raw `mic.wav` (the cancelled mic no longer correlates with the
//! loopback), when the profile found coupling, a canceller ran (here, or
//! `aec` in the live run's `engine_info.echo`) or the mode is `always`,
//! with a band of its own after the canceller ([`band_after_canceller`]).
//! Where the canceller ran here, the transfer gate
//! ([`crate::audio::echo::transfer`], plan 15 section 9.2) also decides on
//! the raw mic's words, which the speech model heard cleanly: the live
//! run's mic words ([`live_mic_words`]; crash recovery and re-runs keep
//! the live run until the post-pass is done), or, when it has none, one
//! more pass of the file engine over `mic.wav` (undiarized, its progress
//! the last quarter of the bar); the layered gate with the raw band on
//! them, against the raw mic, and its echo decisions carried over to the
//! cancelled words they overlap (reason `transfer`). A raw transcription
//! that fails leaves the post-canceller layers alone (`transfer.error`).
//! The loopback is never touched. The counts and parameters go to
//! `engine_info.echo` (with the canceller's `erle_db`, `aec_wall_s`,
//! `aec_wait_s` and `gain_track`, the gate's `band`, `layers` and
//! `floor_db` / `owner_db`, the transfer's `transfer`: its raw `source`,
//! counts per guard and the raw gate's), the dropped words with their
//! reason to the run's `echo_dropped` (for Restore), one count-only log
//! line. A recording whose echo handling the user turned off while it
//! ran (`user_off` in the live run's `engine_info.echo`) is scanned but
//! neither cancelled nor gated: the counts are written with mode
//! [`super::ECHO_OFF_FOR_RECORDING`], and there is no `echo_dropped`. The
//! echo step never fails the post-pass: on an error (an unreadable WAV) the
//! mic stays ungated, `engine_info.echo` is `{"error": "<message>"}` and the
//! error is logged; a canceller that fails leaves the raw mic to be
//! transcribed and gated, with `aec_error` in `engine_info.echo`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::audio::aec::offline::{cancel_wavs, OfflineOptions, OfflineReport};
use crate::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use crate::audio::echo::layers::{self, GateWord, LayerOptions};
use crate::audio::echo::presence::{floor_sources, presence_sources, Presence};
use crate::audio::echo::text::{norm, FarIndex, TextWindow};
use crate::audio::echo::transfer;
use crate::audio::echo::{
    score_sources, ClockPair, EchoMode, EchoProfile, GateBand, GatePad, PcmSource, WavSamples,
    WordScore, ACTIVE_DBFS,
};
use crate::audio::flac::encode_stereo;
use crate::audio::gain_track::{self, GainTrack};
use crate::engine::{DiarPreset, Engine, EngineEvent, EngineWord, ErrorCode, FileJob};
use crate::error::{CoreError, Result};
use crate::models::languages;
use crate::pipeline::CancelToken;
use crate::punct::rebuild::{descends_from, rebuild_transcript, source_run, texts_of};
use crate::punct::{punctuate_loaded, PunctSource, RunLanguage};
use crate::settings;
use crate::store::{CarryOverReport, NewAudioTrack, RunStore};
use crate::transcript::{
    build_transcript_channels, words_from_engine, SegmentOptions, TranscriptWord, ME_LABEL,
    UNKNOWN_LABEL,
};
use crate::types::{
    AudioFormat, Channel, EchoDroppedWord, JobId, LiveUtterance, MeetingId, MeetingStatus, RunId,
    RunKind, RunStatus, TrackChannel,
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
    /// Where the punctuator comes from (the context's data paths).
    pub punct: PunctSource,
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
    /// The canceller's output (temporary).
    pub fn mic_aec_wav(&self) -> PathBuf {
        self.folder.join(MIC_AEC_WAV)
    }
}

/// The echo canceller's output beside the WAVs, transcribed instead of
/// `mic.wav` and deleted right after (and with the WAVs).
pub const MIC_AEC_WAV: &str = "mic-aec.wav";

/// The canceller engages on a coupled recording whose echo return loss is
/// under this (dB; plan 15 section 3.3: meeting 12 has 10 to 13 dB, a leaky
/// headset 30 to 40 dB, which the gate handles alone).
pub const AEC_ERL_BELOW_DB: f32 = 20.0;

/// The gate's band over the cancelled mic's words (still scored on the raw
/// mic): keep under 0.60, drop at 0.95, in between the neighbours' median at
/// `echo.gate_threshold` + 0.20 (0.70 by default). The canceller has
/// already removed most of the echo (meeting 12: duplicates 12,260 → 681),
/// so the gate is there for its residue, while the owner's words spoken over
/// the far end, now recognised, still correlate with it in the raw mic.
/// With the 300 to 3,400 Hz correlator (round two) the owner's words over
/// continuous echo correlate higher than with the 100 Hz high-pass, and the
/// round-one band (0.40 / 0.60 / 0.80) took the spliced double-talk set's
/// word error rate from 7.6 to 9.0 percent; this band keeps it at 7.6 and
/// the FLEURS fixture's at 12.61, and leaves meeting 12 1.95 percent
/// duplicates among the kept words (docs/eval/2026-10-21-echo-round2.md).
/// Not self-calibrated: every calibrated band was lower and cost the
/// owner's words.
pub fn band_after_canceller(threshold: f32) -> GateBand {
    GateBand {
        keep_under: 0.60,
        threshold: (threshold + 0.20).clamp(0.60, 0.95),
        drop_at: 0.95,
    }
}

/// Deletes a file when dropped (the canceller's output, whatever happens).
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

/// The run the post-pass carries names and edits from (plan 16 section 3.9
/// step 7): the meeting's current run when it is a copy of `live_run` (a
/// Restore or a punctuation rebuild of it, or of a copy of it), so names
/// given on the copy survive; else `live_run`.
fn carry_source(store: &dyn RunStore, meeting: MeetingId, live_run: RunId) -> RunId {
    match store.get_meeting(meeting).map(|d| d.current_run) {
        Ok(Some(c)) if c.id != live_run && descends_from(store, &c, live_run) => c.id,
        _ => live_run,
    }
}

/// `transcribe = false` with a punctuator: a `post_pass` run built from the
/// live run's words, each under the speaker row of its own segment,
/// punctuated, made current. `Ok(None)` when no word's text changed (the
/// model did not run, or marked nothing): nothing is made and the live run
/// is promoted as it is. Returns the run, its carry-over and the unplaced
/// names.
fn promote_punctuated(
    store: &dyn RunStore,
    job: &PostPassJob,
    loaded: &mut crate::punct::Loaded,
    cancel: &CancelToken,
) -> Result<Option<(RunId, CarryOverReport, usize)>> {
    let source = source_run(store, job.live_run)?;
    let mut channels = source.channels.clone();
    let before = texts_of(&channels);
    let punctuation = punctuate_loaded(
        loaded,
        &mut channels,
        &job.segment,
        &RunLanguage::new(&job.language),
        &|| cancel.is_cancelled(),
    );
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    if texts_of(&channels) == before {
        log::info!(
            "post-pass meeting {}: punctuation changed no word of the live run ({}); it is promoted as it is",
            job.meeting,
            punctuation.reason.as_str()
        );
        return Ok(None);
    }
    let transcript = rebuild_transcript(&source, channels, &job.segment);
    let mut info = source.run.engine_info.clone();
    if !info.is_object() {
        info = serde_json::json!({});
    }
    info["promoted_live"] = serde_json::json!(job.live_run.0);
    info["live_run"] = serde_json::json!(job.live_run.0);
    info["language"] = serde_json::json!(job.language);
    info["diarize_mic"] = serde_json::json!(job.diarize_mic);
    info["segmentation"] = serde_json::json!({
        "max_gap_ms": job.segment.max_gap_ms,
        "sentence_gap_ms": job.segment.sentence_gap_ms,
        "max_words": job.segment.max_words,
    });
    info["punctuation"] = punctuation.to_json();
    let run = store.create_run(job.meeting, RunKind::PostPass, info)?;
    let built = (|| -> Result<(CarryOverReport, usize)> {
        if let Some(dropped) = store.echo_dropped(job.live_run)? {
            store.set_echo_dropped(run.id, &dropped)?;
        }
        store.insert_transcript(run.id, &transcript)?;
        // The same words, each with its own speaker row: speakers carry by
        // identity.
        let report = store.carry_over_identity(job.live_run, run.id)?;
        let unplaced = unplaced_names(store, job.live_run, &report)?;
        let mut info = store.get_run(run.id)?.engine_info;
        info["unplaced_names"] = serde_json::json!(unplaced);
        store.set_run_engine_info(run.id, info)?;
        store.finish_run(run.id, RunStatus::Done)?;
        Ok((report, unplaced))
    })();
    let (report, unplaced) = match built {
        Ok(r) => r,
        Err(e) => {
            let _ = store.delete_run(run.id);
            return Err(e);
        }
    };
    store.set_current_run(job.meeting, run.id)?;
    log::info!(
        "post-pass meeting {}: live run {} promoted as punctuated run {} ({}, {} marks), {} named, {} unplaced names",
        job.meeting,
        job.live_run,
        run.id,
        punctuation.reason.as_str(),
        punctuation.marks_added.unwrap_or(0),
        report.named.len(),
        unplaced
    );
    Ok(Some((run.id, report, unplaced)))
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

/// What the echo step did.
pub(super) struct EchoOutcome {
    /// `engine_info.echo`: counts and parameters only.
    pub(super) summary: serde_json::Value,
    /// For the run's `echo_dropped`.
    pub(super) dropped: Vec<EchoDroppedWord>,
    pub(super) gated: bool,
    /// The count-only log line.
    log: String,
}

/// The echo step's work before the mic is transcribed ([`prepare_echo`]).
pub(super) struct EchoPrep {
    profile: Result<EchoProfile>,
    scan_s: f64,
    /// The canceller, when it engaged.
    aec: Option<AecRun>,
    /// How long the mic's transcription waited for this after the
    /// loopback's (s).
    wait_s: f64,
}

/// One run of the canceller.
struct AecRun {
    /// Its report, or why it failed.
    result: Result<OfflineReport>,
    /// The return loss the engagement rule saw (dB; `None` with
    /// `echo.mode = always` on a recording without coupling).
    erl_db: Option<f32>,
    /// The loopback's AGC was replayed from its gain track (else the
    /// post-AGC loopback was the reference).
    gain_track: bool,
    /// The mic's AGC was undone and re-applied from its gain track.
    mic_gain_track: bool,
    wall_s: f64,
}

impl EchoPrep {
    /// The canceller ran: its output is the mic to transcribe.
    pub(super) fn cancelled(&self) -> bool {
        self.aec.as_ref().is_some_and(|a| a.result.is_ok())
    }
}

/// The median AGC gain (dB) over the 2 s blocks of `lb_wav` whose level is
/// at or above the echo detector's activity threshold: what the stored
/// levels carry on top of what the speakers played, for the return loss.
fn median_gain_db(lb_wav: &Path, track: &GainTrack) -> Result<Option<f32>> {
    let mut src = WavSamples::open(lb_wav)?;
    let mut buf = Vec::with_capacity(32_000);
    let mut gains = Vec::new();
    let mut at = 0u64;
    loop {
        buf.clear();
        let n = src.pull(&mut buf, 32_000);
        if n < 32_000 {
            break;
        }
        let e: f64 = buf.iter().map(|&v| (f64::from(v) / 32_768.0).powi(2)).sum();
        if 10.0 * (e / n as f64).max(1e-20).log10() >= f64::from(ACTIVE_DBFS) {
            let vals = &track.values()[(at / 160) as usize..];
            let k = vals.len().min(200);
            if k > 0 {
                gains.push(vals[..k].iter().sum::<f32>() / k as f32);
            }
        }
        at += n as u64;
    }
    gains.sort_by(f32::total_cmp);
    Ok((!gains.is_empty()).then(|| gains[gains.len() / 2]))
}

/// The echo step before the mic is transcribed: the scan, then the
/// canceller into `aec_wav` when it engages (see the module docs).
/// `cancelled` stops the canceller (the job was cancelled, or the loopback
/// failed meanwhile).
pub(super) fn prepare_echo(
    mic: &Path,
    lb: &Path,
    aec_wav: &Path,
    clocks: Option<ClockPair>,
    mode: EchoMode,
    live: &serde_json::Value,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> EchoPrep {
    let t0 = Instant::now();
    let profile = EchoProfile::scan(mic, lb, clocks);
    let scan_s = t0.elapsed().as_secs_f64();
    let user_off = live["user_off"].as_bool().unwrap_or(false);
    let Ok(p) = &profile else {
        return EchoPrep {
            profile,
            scan_s,
            aec: None,
            wait_s: 0.0,
        };
    };
    if user_off || mode == EchoMode::Off || !(p.coupled || mode == EchoMode::Always) {
        return EchoPrep {
            profile,
            scan_s,
            aec: None,
            wait_s: 0.0,
        };
    }
    let t1 = Instant::now();
    let read_track = |wav: &Path| match GainTrack::read(&gain_track::path_for(wav)) {
        Ok(t) => t.filter(|t| t.frames() > 0),
        Err(e) => {
            log::warn!("echo canceller: {e}; the stored levels are used as they are");
            None
        }
    };
    let (lb_gain, mic_gain) = (read_track(lb), read_track(mic));
    // The return loss on the levels the speakers played and the mic heard:
    // the WAVs are after the AGCs.
    let mut erl = p.erl_db;
    if let Some(e) = erl.as_mut() {
        for (track, sign) in [(&lb_gain, -1.0f32), (&mic_gain, 1.0)] {
            if let Some(t) = track {
                match median_gain_db(lb, t) {
                    Ok(Some(g)) => *e += sign * g,
                    Ok(None) => {}
                    Err(err) => log::warn!("echo canceller: gain track levels: {err}"),
                }
            }
        }
    }
    let engage = mode == EchoMode::Always || erl.is_some_and(|e| e < AEC_ERL_BELOW_DB);
    if !engage {
        log::info!(
            "echo canceller not engaged: return loss {erl:?} dB (at or over {AEC_ERL_BELOW_DB} dB)"
        );
        return EchoPrep {
            profile,
            scan_s,
            aec: None,
            wait_s: 0.0,
        };
    }
    let hint = |t: i64| p.lag_at(t);
    let mut opts = OfflineOptions::post_pass(p.clocks.as_ref().map(|c| (&c.mic, &c.loopback)));
    opts.lag_hint = Some(&hint);
    opts.loopback_gain = lb_gain.as_ref();
    opts.mic_gain = mic_gain.as_ref();
    opts.cancelled = cancelled;
    let result = cancel_wavs(mic, lb, aec_wav, &opts);
    let aec = AecRun {
        result,
        erl_db: erl,
        gain_track: lb_gain.is_some(),
        mic_gain_track: mic_gain.is_some(),
        wall_s: t1.elapsed().as_secs_f64(),
    };
    EchoPrep {
        profile,
        scan_s,
        aec: Some(aec),
        wait_s: 0.0,
    }
}

/// The mic words of the post-pass (in time order) with the loopback's.
pub(super) struct MicWords<'a> {
    pub(super) words: &'a [TranscriptWord],
    /// The loopback's words (meeting time), for the text layer.
    pub(super) far: &'a [TranscriptWord],
    /// After the canceller: the raw mic's words for the transfer gate
    /// ([`transfer`]), or why there are none (`None`: not asked for).
    pub(super) raw: Option<std::result::Result<RawMic, String>>,
}

/// Where the transfer gate's raw-mic words came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RawSource {
    /// The live run's mic words ([`live_mic_words`]).
    Live,
    /// The file engine over `mic.wav` (the live run has no mic words).
    Transcribed,
}

impl RawSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Transcribed => "transcribed",
        }
    }
}

/// The raw (uncancelled) mic's words, on meeting time, in time order.
pub(super) struct RawMic {
    pub(super) words: Vec<TranscriptWord>,
    pub(super) source: RawSource,
    /// How long getting them took (s).
    pub(super) wall_s: f64,
}

/// The live run's mic words: every word of each mic utterance's latest
/// revision, on meeting time as the live run stored them (through its own
/// clock maps, from the anchors the post-pass maps with), in time order.
/// Empty when the run or its mic words are gone.
pub(super) fn live_mic_words(store: &dyn RunStore, run: RunId) -> Vec<TranscriptWord> {
    let rows = match store.list_live_utterances(run) {
        Ok(r) => r,
        Err(e) => {
            log::info!("post-pass: no live mic words for the echo transfer ({e})");
            return Vec::new();
        }
    };
    let mut latest: BTreeMap<u32, LiveUtterance> = BTreeMap::new();
    for u in rows.into_iter().filter(|u| u.channel == Channel::Mic) {
        if latest.get(&u.utt).is_none_or(|old| u.rev >= old.rev) {
            latest.insert(u.utt, u);
        }
    }
    let mut words: Vec<TranscriptWord> = latest
        .values()
        .flat_map(|u| words_from_engine(Channel::Mic, &u.words, ME_LABEL))
        .collect();
    words.sort_by_key(|w| (w.start_ms, w.end_ms));
    words
}

/// The echo step of the post-pass for the mic words (meeting time) once
/// they are transcribed: the layered gate ([`layers::gate`]) unless the user
/// turned echo handling off for the recording (`user_off` in `live`, the
/// live run's `engine_info.echo`). The text and level layers run on every
/// recording, the canceller's attenuation where it ran, the correlation
/// layer (scored on the raw mic) where the recording correlates (coupled, a
/// canceller ran here or live, or `always`). Levels are taken on `heard`,
/// what was transcribed (the canceller's output when it ran). Returns the
/// drop flag of every word.
#[allow(clippy::too_many_arguments)]
fn gate_mic(
    mic: &Path,
    heard: &Path,
    lb: &Path,
    prep: EchoPrep,
    m: &MicWords,
    mode: EchoMode,
    threshold: f32,
    live: &serde_json::Value,
) -> Result<(Vec<bool>, EchoOutcome)> {
    let t0 = Instant::now();
    let words = m.words;
    let user_off = live["user_off"].as_bool().unwrap_or(false);
    let EchoPrep {
        profile,
        scan_s,
        aec: run,
        wait_s,
    } = prep;
    let profile = profile?;
    let ran = run.as_ref().is_some_and(|a| a.result.is_ok());
    let aec = ran || live["aec"].as_bool().unwrap_or(false);
    let correlation = !user_off && profile.gates(mode, aec);
    let fixed = if ran {
        band_after_canceller(threshold)
    } else {
        GateBand::with_threshold(threshold)
    };
    let before = words.len();
    let mut dropped = Vec::new();
    let mut report = None;
    let mut transfer_info: Option<serde_json::Value> = None;
    let mut transfer_log = String::new();
    let flags = if user_off {
        vec![false; words.len()]
    } else {
        // With clock maps the words are on meeting time, the profile's
        // timeline; without them both are channel time.
        let spans_of = |ws: &[TranscriptWord]| -> Vec<(i64, i64)> {
            ws.iter()
                .map(|w| (w.start_ms * 16, w.end_ms.max(w.start_ms) * 16))
                .collect()
        };
        let spans = spans_of(words);
        let score = |spans: &[(i64, i64)]| -> Result<Vec<WordScore>> {
            Ok(if correlation {
                score_sources(
                    spans,
                    WavSamples::open(mic)?,
                    WavSamples::open(lb)?,
                    &profile,
                    GatePad::DEFAULT,
                )
            } else {
                vec![WordScore::Undefined; spans.len()]
            })
        };
        let scores = score(&spans)?;
        // The canceller's attenuation, for the transfer's level guard (and
        // the attenuation layer, off: layers module).
        let raw: Option<Box<dyn PcmSource>> = if ran {
            Some(Box::new(WavSamples::open(mic)?))
        } else {
            None
        };
        let presence = presence_sources(
            &spans,
            WavSamples::open(heard)?,
            raw,
            WavSamples::open(lb)?,
            profile.clocks.clone(),
        );
        let floor = floor_sources(
            WavSamples::open(heard)?,
            WavSamples::open(lb)?,
            profile.clocks.clone(),
        )
        .floor_db();
        // The file engine's utterance is the whole undiarized mic: the
        // utterance rule's groups are its stretches with pauses up to
        // `POCKET_GAP_MS`.
        let gate_words =
            |ws: &[TranscriptWord], scores: &[WordScore], presence: &[Option<Presence>]| {
                let groups = layers::gap_groups(ws.iter().map(|w| (w.start_ms, w.end_ms)));
                ws.iter()
                    .enumerate()
                    .map(|(i, w)| GateWord {
                        start_ms: w.start_ms,
                        end_ms: w.end_ms,
                        norm: norm(&w.text),
                        group: groups[i],
                        score: scores[i],
                        presence: presence[i],
                    })
                    .collect::<Vec<GateWord>>()
            };
        let gw = gate_words(words, &scores, &presence);
        let far = FarIndex::new(m.far.iter().map(|w| (w.start_ms, norm(&w.text))));
        let opts = LayerOptions {
            correlation,
            canceller: ran,
            fixed,
            threshold,
            window: TextWindow::DEFAULT,
            calibrate: !ran,
        };
        let r = match m.raw.as_ref().filter(|_| ran) {
            // The transfer gate (plan 15 section 9.2): the layered gate on
            // the raw mic's words with the raw band, against the raw mic,
            // transferred onto the cancelled words.
            Some(Ok(raw_mic)) => {
                let t_gate = Instant::now();
                let rw = &raw_mic.words;
                let raw_spans = spans_of(rw);
                let raw_scores = score(&raw_spans)?;
                let raw_presence = presence_sources(
                    &raw_spans,
                    WavSamples::open(mic)?,
                    None,
                    WavSamples::open(lb)?,
                    profile.clocks.clone(),
                );
                let raw_floor = floor_sources(
                    WavSamples::open(mic)?,
                    WavSamples::open(lb)?,
                    profile.clocks.clone(),
                )
                .floor_db();
                let raw_gw = gate_words(rw, &raw_scores, &raw_presence);
                let raw_opts = LayerOptions {
                    canceller: false,
                    fixed: GateBand::with_threshold(threshold),
                    calibrate: true,
                    ..opts
                };
                let t = transfer::gate(
                    &raw_gw,
                    raw_floor,
                    &raw_opts,
                    &gw,
                    floor,
                    &opts,
                    &far,
                    transfer::GUARD,
                );
                let c = t.transfer;
                let rb = t.raw.calibration.band;
                let mut info = serde_json::json!({
                    "source": raw_mic.source.as_str(),
                    "raw_words": rw.len(),
                    "raw_dropped": t.raw.dropped(),
                    "raw_layers": t.raw.counts,
                    "raw_band": [super::round3(rb.keep_under), super::round3(rb.threshold), super::round3(rb.drop_at)],
                    "raw_band_calibrated": t.raw.calibration.calibrated,
                    "raw_wall_s": super::round3(raw_mic.wall_s as f32),
                    "guard": transfer::GUARD.as_str(),
                    "gate_s": super::round3(t_gate.elapsed().as_secs_f32()),
                    "removed": t.counts.transfer,
                });
                if let (Some(o), serde_json::Value::Object(cs)) =
                    (info.as_object_mut(), serde_json::json!(c))
                {
                    o.extend(cs);
                }
                // The raw gate's owner's level, floor and share of flagged
                // words, and the level guard's floor rule: its tier and
                // margin, null with the owner's level known (plan 16
                // sections 12.2.2 step 4 and 12.7).
                info["raw_owner_db"] = serde_json::json!(t.raw.owner_db.map(super::round3));
                info["raw_floor_db"] = serde_json::json!(t.raw.floor_db.map(super::round3));
                info["raw_echo_share"] =
                    serde_json::json!((t.raw_echo_share * 10_000.0).round() / 10_000.0);
                info["floor_rule"] = serde_json::json!({
                    "tier": t.floor_rule.map(|f| f.tier),
                    "f_db": t.floor_rule.map(|f| super::round3(f.f_db)),
                    "removed": c.floor,
                });
                transfer_info = Some(info);
                transfer_log = format!(
                    "; transfer from the {} mic words ({} of {} echo, {:.1} s): {} of {} candidates (text {}, level {}, plain {}), {} beyond the layers, {} without a raw word",
                    raw_mic.source.as_str(),
                    t.raw.dropped(),
                    rw.len(),
                    raw_mic.wall_s,
                    c.removed(),
                    c.candidates,
                    c.text,
                    c.level,
                    c.plain,
                    t.counts.transfer,
                    c.unmatched
                );
                if let Some(f) = t.floor_rule {
                    transfer_log.push_str(&format!(
                        " (floor rule, {} tier, at {} dB: {})",
                        f.tier.as_str(),
                        f.f_db,
                        c.floor
                    ));
                }
                let mut r = t.after;
                r.reasons = t.reasons;
                r.counts = t.counts;
                r
            }
            other => {
                if let Some(Err(e)) = other {
                    transfer_info = Some(serde_json::json!({ "error": e }));
                    transfer_log = format!("; no transfer ({e})");
                }
                layers::gate(&gw, &far, floor, &opts)
            }
        };
        for (i, w) in words.iter().enumerate() {
            if let Some(reason) = r.reasons[i] {
                dropped.push(EchoDroppedWord {
                    start_ms: w.start_ms,
                    end_ms: w.end_ms,
                    text: w.text.clone(),
                    coefficient: scores[i].coefficient().unwrap_or(0.0),
                    reason: Some(reason),
                });
            }
        }
        let flags = r.reasons.iter().map(Option::is_some).collect();
        report = Some(r);
        flags
    };
    let band = report.as_ref().map_or(fixed, |r| r.calibration.band);
    // The live monitor's return loss is on pre-AGC levels; the WAVs only
    // give the post-AGC one.
    let erl = live["erl_db"]
        .as_f64()
        .or(profile.erl_db.map(super::round3));
    let aec_report = run.as_ref().and_then(|a| a.result.as_ref().ok());
    let mut summary = serde_json::json!({
        "coupled": profile.coupled,
        "coupling": super::round3(profile.coupling),
        "lag_ms": profile.lag_ms.map(super::round3),
        "erl_db": erl,
        "mic_words_before": before,
        "dropped": dropped.len(),
        "threshold": super::round3(threshold),
        "band": [super::round3(band.keep_under), super::round3(band.threshold), super::round3(band.drop_at)],
        "aec": aec,
        "erle_db": aec_report.and_then(|r| r.erle_db).map(super::round3),
        "aec_wall_s": run.as_ref().map(|a| super::round3(a.wall_s as f32)),
        "aec_wait_s": run.as_ref().map(|_| super::round3(wait_s as f32)),
        "gain_track": run.as_ref().is_some_and(|a| a.gain_track),
        "mode": if user_off { super::ECHO_OFF_FOR_RECORDING } else { mode.as_str() },
        "user_off": user_off,
    });
    if let Some(r) = &report {
        layer_summary(&mut summary, r, correlation);
    }
    // Where the owner's level came from, and whether it is low (plan 16
    // sections 12.2.2 step 4 and 12.7).
    if let Some(r) = &report {
        summary["owner_source"] = serde_json::json!(r.owner_source);
        summary["owner_words"] = serde_json::json!(r.owner_words);
        summary["owner_low"] = serde_json::json!(r.owner_low);
    }
    if let Some(t) = transfer_info {
        summary["transfer"] = t;
    }
    if let Some(a) = &run {
        summary["aec_erl_db"] = serde_json::json!(a.erl_db.map(super::round3));
        if a.mic_gain_track {
            summary["mic_gain_track"] = serde_json::json!(true);
        }
        if let Err(e) = &a.result {
            summary["aec_error"] = serde_json::json!(e.to_string());
        }
    }
    let canceller = match (&run, aec_report) {
        (Some(a), Some(r)) => format!(
            "; canceller: ERLE {:?} dB over {} far-end blocks, lag {:?} ms, {} jumps, {} resets, {:.1} s ({}gain track), the mic waited {wait_s:.1} s",
            r.erle_db,
            r.far_blocks,
            r.lag_ms,
            r.jumps,
            r.stats.resets,
            a.wall_s,
            if a.gain_track { "" } else { "no " }
        ),
        (Some(a), None) => format!(
            "; canceller failed after {:.1} s, the raw mic was transcribed",
            a.wall_s
        ),
        (None, _) => String::new(),
    };
    let layer_log = report.as_ref().map_or(String::new(), |r| {
        let c = &r.counts;
        format!(
            " (text {}, attenuation {}, level {}, correlation {}, utterance {}, transfer {}; band {:.2}/{:.2}/{:.2}{}{transfer_log})",
            c.text,
            c.attenuation,
            c.level,
            c.correlation,
            c.utterance,
            c.transfer,
            band.keep_under,
            band.threshold,
            band.drop_at,
            if r.calibration.calibrated { " calibrated" } else { "" }
        )
    });
    let log = format!(
        "{:?} (coupling {:.2}, lag {:?} ms, {} of {} blocks active), {} of {before} mic words dropped{}{layer_log}; scan {scan_s:.1} s, gate {:.1} s{canceller}",
        profile.status,
        profile.coupling,
        profile.lag_ms,
        profile.active_blocks,
        profile.blocks,
        dropped.len(),
        if user_off {
            " (not gated: turned off for this recording)"
        } else {
            ""
        },
        t0.elapsed().as_secs_f64()
    );
    if let Some(Err(e)) = run.as_ref().map(|a| &a.result) {
        log::warn!("echo canceller failed: {e}");
    }
    // `echo_dropped` as before for a correlated recording, and whenever a
    // layer removed something.
    let gated = !user_off && (correlation || !dropped.is_empty());
    Ok((
        flags,
        EchoOutcome {
            summary,
            dropped,
            gated,
            log,
        },
    ))
}

/// The layered gate's counts and parameters in `engine_info.echo`.
pub(super) fn layer_summary(
    summary: &mut serde_json::Value,
    r: &layers::LayerReport,
    correlation: bool,
) {
    summary["layers"] = serde_json::json!(r.counts);
    summary["band_calibrated"] = serde_json::json!(r.calibration.calibrated);
    summary["correlation"] = serde_json::json!(correlation);
    summary["text_matched"] = serde_json::json!(r.run3);
    summary["text_pairs"] = serde_json::json!(r.run2);
    summary["floor_db"] = serde_json::json!(r.floor_db.map(super::round3));
    summary["owner_db"] = serde_json::json!(r.owner_db.map(super::round3));
}

/// The echo step of the post-pass after the mic is transcribed
/// ([`gate_mic`]), which never fails it: on an error the mic words stay as
/// they are, `engine_info.echo` records `{"error": "<message>"}` and there
/// is no `echo_dropped`. Returns the kept words.
#[allow(clippy::too_many_arguments)]
pub(super) fn echo_step(
    meeting: MeetingId,
    mic: &Path,
    heard: &Path,
    lb: &Path,
    prep: EchoPrep,
    m: MicWords,
    mode: EchoMode,
    threshold: f32,
    live: &serde_json::Value,
) -> (Vec<TranscriptWord>, EchoOutcome) {
    match gate_mic(mic, heard, lb, prep, &m, mode, threshold, live) {
        Ok((flags, e)) => {
            log::info!("post-pass meeting {meeting}: echo {}", e.log);
            let kept = m
                .words
                .iter()
                .zip(flags)
                .filter(|(_, d)| !d)
                .map(|(w, _)| w.clone())
                .collect();
            (kept, e)
        }
        Err(err) => {
            log::warn!(
                "post-pass meeting {meeting}: the echo step failed ({err}); the mic words are not gated"
            );
            (
                m.words.to_vec(),
                EchoOutcome {
                    summary: serde_json::json!({ "error": err.to_string() }),
                    dropped: Vec::new(),
                    gated: false,
                    log: String::new(),
                },
            )
        }
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

    // Echo: the settings now, and what the live run knew (a canceller).
    let stored = settings::effective(store)?;
    let echo_mode = settings::echo_mode(&stored);
    let echo_threshold = settings::echo_gate_threshold(&stored);
    let live_echo = store
        .get_run(job.live_run)
        .map(|r| r.engine_info["echo"].clone())
        .unwrap_or(serde_json::Value::Null);
    let mut echo: Option<EchoOutcome> = None;

    let mut transcribe_wall = Duration::ZERO;
    let mut carry = None;
    let mut unplaced = 0;
    let mut live_deleted = false;
    // A post-pass run replaced the live run (transcribed, or promoted with
    // punctuation).
    let mut replaced_live = job.transcribe;
    let current_run = if job.transcribe {
        let t0 = Instant::now();
        // One channel through the file engine, on meeting time; `part`:
        // the `k`-th of `n` equal parts of the progress.
        let transcribe_channel = |(k, n): (u64, u64),
                                  ch: Channel,
                                  wav: &Path,
                                  diarize: bool,
                                  frames: u64|
         -> Result<Vec<TranscriptWord>> {
            if cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let base = k * audio_ms;
            let words = transcribe(
                engine,
                FileJob {
                    job_id: JobId::new(),
                    wav_path: wav.to_path_buf(),
                    diarize,
                    language: job.language.clone(),
                    diar_preset: DiarPreset::Streaming,
                },
                cancel,
                &|done, _| progress(JobStage::Transcribing, (base + done) / n, total_ms),
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
            Ok(tw)
        };
        // The echo scan and the canceller run while the loopback is
        // transcribed (the engine works on the GPU); the canceller's output
        // goes whatever happens.
        let aec_wav = job.mic_aec_wav();
        let _remove_aec = RemoveOnDrop(aec_wav.clone());
        let stop = AtomicBool::new(false);
        let clocks = ClockPair::usable(map_of(Channel::Mic), map_of(Channel::Loopback));
        let (lb_words, prep) = std::thread::scope(|s| {
            let prep = (echo_mode != EchoMode::Off).then(|| {
                let (mic, lb, aec_wav, live_echo, stop) = (&mic, &lb, &aec_wav, &live_echo, &stop);
                let clocks = clocks.clone();
                s.spawn(move || {
                    let halt = || stop.load(Ordering::Relaxed) || cancel.is_cancelled();
                    prepare_echo(mic, lb, aec_wav, clocks, echo_mode, live_echo, &halt)
                })
            });
            let lb_words = transcribe_channel((0, 2), Channel::Loopback, &lb, true, lb_frames);
            if lb_words.is_err() {
                stop.store(true, Ordering::Relaxed);
            }
            let waited = Instant::now();
            let prep = prep.map(|h| {
                let mut p = h.join().unwrap_or_else(|_| EchoPrep {
                    profile: Err(CoreError::Engine("the echo step panicked".into())),
                    scan_s: 0.0,
                    aec: None,
                    wait_s: 0.0,
                });
                p.wait_s = waited.elapsed().as_secs_f64();
                p
            });
            (lb_words, prep)
        });
        let lb_words = lb_words?;
        let cancelled = prep.as_ref().is_some_and(EchoPrep::cancelled);
        let mic_source = if cancelled { &aec_wav } else { &mic };
        // The transfer gate's raw-mic words (plan 15 section 9.2): the live
        // run's, else one more pass of the file engine over `mic.wav`
        // (crash recovery and re-runs alike: the live run is kept until the
        // post-pass is done).
        let user_off = live_echo["user_off"].as_bool().unwrap_or(false);
        let t_live = Instant::now();
        let live_raw = (cancelled && !user_off).then(|| live_mic_words(store, job.live_run));
        let live_s = t_live.elapsed().as_secs_f64();
        let transcribe_raw = live_raw.as_ref().is_some_and(Vec::is_empty);
        let mic_part = if transcribe_raw { (2, 4) } else { (1, 2) };
        let mut mic_words = transcribe_channel(
            mic_part,
            Channel::Mic,
            mic_source,
            job.diarize_mic,
            mic_frames,
        )?;
        let raw = match live_raw {
            None => None,
            Some(words) if !words.is_empty() => Some(Ok(RawMic {
                words,
                source: RawSource::Live,
                wall_s: live_s,
            })),
            Some(_) => {
                let t_raw = Instant::now();
                match transcribe_channel((3, 4), Channel::Mic, &mic, false, mic_frames) {
                    Ok(words) => Some(Ok(RawMic {
                        words,
                        source: RawSource::Transcribed,
                        wall_s: t_raw.elapsed().as_secs_f64(),
                    })),
                    Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                    Err(e) => {
                        log::warn!(
                            "post-pass meeting {}: the raw mic's transcription for the echo transfer failed ({e}); the cancelled words are gated without it",
                            job.meeting
                        );
                        Some(Err(format!("raw mic transcription: {e}")))
                    }
                }
            }
        };
        if let Some(prep) = prep {
            // The levels are taken on what was transcribed: the canceller's
            // output while it is still there.
            let (kept, e) = echo_step(
                job.meeting,
                &mic,
                mic_source,
                &lb,
                prep,
                MicWords {
                    words: &mic_words,
                    far: &lb_words,
                    raw,
                },
                echo_mode,
                echo_threshold,
                &live_echo,
            );
            mic_words = kept;
            echo = Some(e);
        }
        let _ = std::fs::remove_file(&aec_wav);
        let mut channels: Vec<(Channel, Vec<TranscriptWord>)> =
            vec![(Channel::Loopback, lb_words), (Channel::Mic, mic_words)];
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
        // Punctuation after the echo gate, before segmentation; the
        // session is dropped right after.
        let punctuation = {
            let mut loaded = job.punct.load(store);
            punctuate_loaded(
                &mut loaded,
                &mut channels,
                &job.segment,
                &RunLanguage::new(&job.language),
                &|| cancel.is_cancelled(),
            )
        };
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
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
                "punctuation": punctuation.to_json(),
            }),
        )?;
        if let Some(e) = &echo {
            let mut info = run.engine_info.clone();
            info["echo"] = e.summary.clone();
            store.set_run_engine_info(run.id, info)?;
            if e.gated {
                store.set_echo_dropped(run.id, &e.dropped)?;
            }
        }
        if let Err(e) = store.insert_transcript(run.id, &transcript) {
            let _ = store.finish_run(run.id, RunStatus::Failed);
            return Err(e.into());
        }
        store.finish_run(run.id, RunStatus::Done)?;
        // From the live run, or from a copy of it the user named speakers
        // on (Restore, a punctuation rebuild).
        let source = carry_source(store, job.meeting, job.live_run);
        let report = store.carry_over(source, run.id)?;
        // Names given on the live run that reached no speaker of the new
        // run: the live run is kept for them (recorded before the WAVs go,
        // so crash recovery keeps it too).
        unplaced = unplaced_names(store, source, &report)?;
        let mut info = store.get_run(run.id)?.engine_info;
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
        match job.punct.load(store) {
            Ok(p) => {
                let mut loaded: crate::punct::Loaded = Ok(p);
                match promote_punctuated(store, job, &mut loaded, cancel)? {
                    Some((run, report, n)) => {
                        carry = Some(report);
                        unplaced = n;
                        replaced_live = true;
                        run
                    }
                    // No word changed: the live run is the final one.
                    None => job.live_run,
                }
            }
            // No punctuator (or `off`): the live run is the final one.
            Err(_) => job.live_run,
        }
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
    let _ = std::fs::remove_file(job.mic_aec_wav());

    // A live run with a name nobody in the new run received stays, as if
    // `keep_live_runs` were on for this meeting.
    if replaced_live && !job.keep_live_runs && unplaced == 0 {
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
