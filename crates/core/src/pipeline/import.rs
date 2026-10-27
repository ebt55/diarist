//! File import: decode, transcribe, store (`docs/01`, "File-mode flow").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, UNIX_EPOCH};

use super::{CancelToken, ImportRequest};
use crate::audio::Decoder;
use crate::engine::{DiarPreset, DiarizeRunner, Engine, EngineEvent, EngineWord, FileJob};
use crate::error::{CoreError, Result};
use crate::models::{free_disk_bytes, languages, DataPaths};
use crate::punct::{punctuate_loaded, PunctSource, RunLanguage};
use crate::settings;
use crate::store::{NewAudioTrack, NewMeeting, RunStore};
use crate::transcript::{
    build_transcript, loops, words_from_engine, SegmentOptions, UNKNOWN_LABEL,
};
use crate::types::{
    AudioFormat, Channel, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source, TrackChannel,
};
use crate::views::{JobEvent, JobStage, Settings};

/// Free space required before an import starts.
pub const MIN_FREE_BYTES: u64 = 2 << 30;

/// How an import runs: where, in which language, what to keep. Built from
/// the caller's data paths and the effective settings with
/// [`ImportOptions::new`]; fields can be overridden afterwards (the CLI's
/// `--language`, `--keep-wav`, `--rttm`).
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The data root: the 16 kHz WAV, the engine JSON and logs go to
    /// `<root>/audio/<meeting uid>/`.
    pub data_root: PathBuf,
    /// The settings the options were derived from (stored over defaults).
    pub settings: Settings,
    /// BCP-47 tag or `auto` (setting `default_language`).
    pub language: String,
    /// Keep and register `source.wav` (setting `keep_intermediate_wav`).
    pub keep_intermediate: bool,
    /// Refuse to start with less free space than this.
    pub min_free_bytes: u64,
    /// Runs the standalone diarization pass when [`Self::run_diarize_pass`].
    pub diarize_runner: Option<DiarizeRunner>,
    /// Run the standalone pass and keep `diarization.rttm`/`.json` in the
    /// meeting folder (setting `keep_diarization_segments`; needs a runner).
    pub run_diarize_pass: bool,
    pub segment: SegmentOptions,
    /// The punctuator (plan 16 section 3.2): the selected model of the
    /// caller's data paths, so `--models-dir` overrides apply.
    pub punct: PunctSource,
}

impl ImportOptions {
    /// Options for the data root of `paths` from `settings` (stored values;
    /// defaults are filled in here); punctuation from `paths`.
    pub fn new(paths: &DataPaths, settings: &Settings) -> Self {
        let settings = settings::with_defaults(settings.clone());
        Self {
            data_root: paths.root.clone(),
            punct: PunctSource::Data(paths.clone()),
            language: settings::default_language(&settings),
            keep_intermediate: settings::get_bool(&settings, settings::KEEP_INTERMEDIATE_WAV),
            run_diarize_pass: settings::get_bool(&settings, settings::KEEP_DIARIZATION_SEGMENTS),
            settings,
            min_free_bytes: MIN_FREE_BYTES,
            diarize_runner: None,
            segment: SegmentOptions::default(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Imports one audio or video file as a new meeting.
///
/// Steps, each reported through `on_event` (`JobEvent::Progress` with the
/// matching stage, then exactly one `Done`, `Error` or `Cancelled`):
/// 0. Refuse when the input is missing or under `opts.min_free_bytes` are
///    free on the data root's volume (no meeting is created).
/// 1. `store.create_meeting` (source `file`, status `processing`, language
///    `opts.language`).
/// 2. Decode to `audio/<uid>/source.wav` (16 kHz mono) with `decoder`;
///    register the original file as the `playback` track (format
///    `original`); `finish_meeting` with the decoded duration.
/// 3. `store.create_run(kind = file)` (engine, models, decoder and options
///    in `engine_info`), then `engine.transcribe_file` and drain its events
///    until `Done` or `Error`; a set `cancel` token (from the [`JobQueue`])
///    is forwarded to `engine.cancel`. Optionally the standalone diarize pass.
/// 4. Collapse the decoder's repeated-token loops (`transcript.collapse_loops`,
///    plan 16 section 12.3; `engine_info.loops`), punctuate the words
///    (`opts.punct`, plan 16 section 3.2; the report
///    goes to the run's `engine_info.punctuation` through
///    `set_run_engine_info`), build speakers, segments (`transcript`
///    module) and words; `store.insert_transcript`, `finish_run(done)`,
///    `set_current_run`,
///    status `done`; delete the 16 kHz intermediate unless kept.
///    Words that are mostly unknown-token markers are not stored: the job
///    fails with [`CoreError::UnknownTokens`] (the model cannot transcribe
///    the language, [`languages::unknown_token_output`]).
/// 5. On failure: `finish_run(failed)`, status `failed` (the meeting and
///    its playback track stay), `JobEvent::Error`.
///    On cancellation: the meeting is deleted (folder included) and
///    `JobEvent::Cancelled` is sent (with the meeting id only if the delete
///    failed and a `failed` row is left).
///
/// [`JobQueue`]: super::JobQueue
pub fn import_file(
    store: &dyn RunStore,
    engine: &dyn Engine,
    decoder: &dyn Decoder,
    req: ImportRequest,
    opts: &ImportOptions,
    cancel: &CancelToken,
    on_event: &dyn Fn(JobEvent),
) -> Result<MeetingId> {
    let job_id = req.job_id.clone();
    let fail_early = |e: CoreError| {
        on_event(JobEvent::Error {
            job_id: job_id.clone(),
            meeting_id: None,
            message: e.to_string(),
        });
        Err(e)
    };
    if !req.path.is_file() {
        return fail_early(CoreError::Decode(format!(
            "{} is not a file",
            req.path.display()
        )));
    }
    if let Some(free) = free_disk_bytes(&opts.data_root) {
        if free < opts.min_free_bytes {
            return fail_early(CoreError::InsufficientSpace {
                path: opts.data_root.display().to_string(),
                free_mb: free >> 20,
                needed_mb: opts.min_free_bytes >> 20,
            });
        }
    }
    let source = std::path::absolute(&req.path).unwrap_or_else(|_| req.path.clone());
    let title = req
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let started_at = mtime_ms(&source).unwrap_or_else(now_ms);
    let meeting = match store.create_meeting(NewMeeting {
        title,
        source: Source::File,
        source_path: Some(source.display().to_string()),
        language: opts.language.clone(),
        diarize_mic: false,
        started_at,
        status: MeetingStatus::Processing,
    }) {
        Ok(m) => m,
        Err(e) => return fail_early(e.into()),
    };
    let dir = opts.data_root.join("audio").join(&meeting.uid);
    let wav = dir.join("source.wav");
    let keep_wav = opts.keep_intermediate;
    let mut run: Option<RunId> = None;
    let result = run_import(
        store,
        engine,
        decoder,
        &req,
        opts,
        cancel,
        on_event,
        &meeting.uid,
        meeting.id,
        started_at,
        &source,
        &wav,
        keep_wav,
        &mut run,
    );
    if !keep_wav {
        let _ = std::fs::remove_file(&wav);
    }
    match result {
        Ok(()) => {
            on_event(JobEvent::Done {
                job_id,
                meeting_id: meeting.id,
            });
            Ok(meeting.id)
        }
        Err(e) => {
            if let Some(run) = run {
                let _ = store.finish_run(run, RunStatus::Failed);
            }
            let _ = store.set_meeting_status(meeting.id, MeetingStatus::Failed);
            if matches!(e, CoreError::Cancelled) {
                // The user did not want this meeting; keep a failed row only
                // when it cannot be removed.
                let left = store.delete_meeting(meeting.id).is_err();
                on_event(JobEvent::Cancelled {
                    job_id,
                    meeting_id: left.then_some(meeting.id),
                });
            } else {
                on_event(JobEvent::Error {
                    job_id,
                    meeting_id: Some(meeting.id),
                    message: e.to_string(),
                });
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_import(
    store: &dyn RunStore,
    engine: &dyn Engine,
    decoder: &dyn Decoder,
    req: &ImportRequest,
    opts: &ImportOptions,
    cancel: &CancelToken,
    on_event: &dyn Fn(JobEvent),
    uid: &str,
    meeting: MeetingId,
    started_at: i64,
    source: &Path,
    wav: &Path,
    keep_wav: bool,
    run_out: &mut Option<RunId>,
) -> Result<()> {
    let job_id = req.job_id.clone();
    let cancelled = || cancel.is_cancelled();
    let progress = |stage: JobStage, done_ms: u64, total_ms: Option<u64>| {
        on_event(JobEvent::Progress {
            job_id: job_id.clone(),
            stage,
            done_ms,
            total_ms,
        })
    };

    // 1. Decode into the meeting folder.
    progress(JobStage::Decoding, 0, None);
    if let Some(dir) = wav.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if cancelled() {
        return Err(CoreError::Cancelled);
    }
    let info = decoder.decode_to_wav16k(source, wav)?;
    if cancelled() {
        return Err(CoreError::Cancelled);
    }
    let audio_ms = info.duration_ms.max(0) as u64;
    let total_ms = Some(audio_ms);
    store.add_audio_track(NewAudioTrack {
        meeting_id: meeting,
        channel: TrackChannel::Playback,
        path: source.display().to_string(),
        format: AudioFormat::Original,
        sample_rate: info.source_sample_rate,
        channels: info.source_channels,
        duration_ms: Some(info.duration_ms),
        size_bytes: std::fs::metadata(source).ok().map(|m| m.len() as i64),
        channel_frames: None,
    })?;
    if keep_wav {
        store.add_audio_track(NewAudioTrack {
            meeting_id: meeting,
            channel: TrackChannel::Source,
            path: format!("audio/{uid}/source.wav"),
            format: AudioFormat::Wav,
            sample_rate: crate::audio::TARGET_RATE,
            channels: 1,
            duration_ms: Some(info.duration_ms),
            size_bytes: std::fs::metadata(wav).ok().map(|m| m.len() as i64),
            channel_frames: None,
        })?;
    }
    store.finish_meeting(meeting, started_at + info.duration_ms, info.duration_ms)?;

    // 2. A `file` run and the engine job.
    let engine_info = engine.info();
    let run = store.create_run(
        meeting,
        RunKind::File,
        serde_json::json!({
            "runtime": engine_info.runtime,
            "models": engine_info.models,
            "device": engine_info.device,
            "right_context": crate::protocol::lookahead_json(engine_info.right_context),
            "diar_preset": crate::protocol::diar_preset_text(engine_info.diar_preset.as_deref()),
            "language": opts.language,
            "diarize": req.diarize,
            "decoder": info.decoder.as_str(),
            "source_sample_rate": info.source_sample_rate,
            "source_channels": info.source_channels,
            "segmentation": {
                "max_gap_ms": opts.segment.max_gap_ms,
                "sentence_gap_ms": opts.segment.sentence_gap_ms,
                "max_words": opts.segment.max_words,
            },
        }),
    )?;
    *run_out = Some(run.id);
    progress(JobStage::Transcribing, 0, total_ms);
    let rx = engine.transcribe_file(FileJob {
        job_id: job_id.clone(),
        wav_path: wav.to_path_buf(),
        diarize: req.diarize,
        language: opts.language.clone(),
        diar_preset: DiarPreset::Streaming,
    })?;
    let mut utterances: BTreeMap<(Channel, u32), (u32, Vec<EngineWord>)> = BTreeMap::new();
    let mut cancel_sent = false;
    // The runtime's detected languages (Nemotron 3.5), for punctuation in
    // `auto` (plan 16 section 3.8).
    let detected = loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(EngineEvent::Progress { done_ms, .. }) => {
                progress(JobStage::Transcribing, done_ms, total_ms)
            }
            Ok(EngineEvent::Utterance {
                channel,
                utt,
                rev,
                words,
                ..
            }) => {
                let e = utterances
                    .entry((channel, utt))
                    .or_insert((rev, Vec::new()));
                if rev >= e.0 {
                    *e = (rev, words);
                }
            }
            Ok(EngineEvent::Done { stats, .. }) => break stats.languages,
            Ok(EngineEvent::Error { code, message, .. }) => {
                if cancel_sent || code == crate::engine::ErrorCode::Cancelled {
                    return Err(CoreError::Cancelled);
                }
                return Err(CoreError::Engine(format!("{code:?}: {message}")));
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if cancelled() && !cancel_sent {
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
    };

    // 3. Optional standalone diarization pass, kept for the eval harness.
    if let (true, true, Some(runner)) = (opts.run_diarize_pass, req.diarize, &opts.diarize_runner) {
        if cancelled() {
            return Err(CoreError::Cancelled);
        }
        progress(JobStage::Diarizing, 0, total_ms);
        let dir = wav.parent().unwrap_or(Path::new("."));
        runner.run(
            wav,
            &dir.join("diarization.rttm"),
            Some(&dir.join("diarization.json")),
        )?;
    }

    // 4. Segments, speakers and words in one transaction.
    progress(JobStage::Saving, audio_ms, total_ms);
    let default_label = if req.diarize { UNKNOWN_LABEL } else { "spk_0" };
    let mut words = Vec::new();
    for ((channel, _), (_, ws)) in &utterances {
        words.extend(words_from_engine(*channel, ws, default_label));
    }
    // Nemotron's last word can end after the audio does (research/08
    // pitfall 11): clamp to the decoded duration.
    let end = info.duration_ms.max(0);
    for w in &mut words {
        w.start_ms = w.start_ms.min(end);
        w.end_ms = w.end_ms.clamp(w.start_ms, end.max(w.start_ms));
    }
    words.sort_by_key(|w| w.start_ms);
    // A language the model cannot transcribe (declared in its prompt
    // dictionary, never trained): the run fails, the meeting stays.
    if languages::unknown_token_output(words.iter().map(|w| w.text.as_str())) {
        return Err(CoreError::UnknownTokens);
    }
    // Repeated-token loops collapse (plan 16 section 12.3): one channel, a
    // loop never spans two speaker labels.
    let mut channels = vec![(Channel::Source, words)];
    let loops = loops::collapse_channels(&mut channels, loops::enabled(store));
    // Punctuation: one channel, the diarized labels as they are; the
    // session is dropped right after. In `auto` it takes the language the
    // runtime detected.
    let detected = RunLanguage::most_detected(detected.iter().map(String::as_str));
    let punctuation = {
        let mut loaded = opts.punct.load(store);
        punctuate_loaded(
            &mut loaded,
            &mut channels,
            &opts.segment,
            &RunLanguage::new(&opts.language).with_detected(detected.clone()),
            &cancelled,
        )
    };
    if cancelled() {
        return Err(CoreError::Cancelled);
    }
    let words = channels.pop().map(|(_, w)| w).unwrap_or_default();
    let mut info = run.engine_info.clone();
    if let Some(d) = detected {
        // For a later "Add punctuation" (`punct::rebuild::run_language`).
        info["detected_language"] = serde_json::Value::String(d);
    }
    info["punctuation"] = punctuation.to_json();
    info["loops"] = loops.to_json();
    store.set_run_engine_info(run.id, info)?;
    let transcript = build_transcript(&words, &opts.segment);
    store.insert_transcript(run.id, &transcript)?;
    store.finish_run(run.id, RunStatus::Done)?;
    store.set_current_run(meeting, run.id)?;
    store.set_meeting_status(meeting, MeetingStatus::Done)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::DecodedInfo;
    use crate::audio::DecoderKind;
    use crate::engine::nemo_cli::{engine_words, file_events, parse_transcribe_json};
    use crate::engine::{EngineInfo, ErrorCode};
    use crate::store::{RunStore, SqliteStore, Store};
    use crate::types::JobId;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};

    /// Writes one second of silence; reports a 48 kHz stereo source.
    struct FakeDecoder;
    impl Decoder for FakeDecoder {
        fn decode_to_wav16k(&self, _: &Path, output: &Path) -> Result<DecodedInfo> {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(output, spec).unwrap();
            for _ in 0..16_000 {
                w.write_sample(0i16).unwrap();
            }
            w.finalize().unwrap();
            Ok(DecodedInfo {
                duration_ms: 300_032,
                source_sample_rate: 48_000,
                source_channels: 2,
                decoder: DecoderKind::Other,
            })
        }
    }

    enum Mode {
        Fixture,
        Fail,
        WaitForCancel,
        /// Runs of `<unk>`, as for a language the model was not trained on.
        UnknownTokens,
        /// A decoder loop: six copies of one word in one frame.
        Loop,
    }

    struct FakeEngine {
        mode: Mode,
        cancelled: Arc<AtomicBool>,
        seen_wav: Mutex<Option<PathBuf>>,
        /// What `Mode::Fixture`'s `Done` reports as detected.
        languages: Vec<String>,
    }

    impl FakeEngine {
        fn new(mode: Mode) -> Self {
            Self {
                mode,
                cancelled: Arc::default(),
                seen_wav: Mutex::new(None),
                languages: Vec::new(),
            }
        }
    }

    impl Engine for FakeEngine {
        fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>> {
            assert!(job.wav_path.is_file(), "engine gets the decoded WAV");
            *self.seen_wav.lock().unwrap() = Some(job.wav_path.clone());
            let (tx, rx) = channel();
            match self.mode {
                Mode::Fixture => {
                    let json = std::fs::read_to_string(format!(
                        "{}/tests/fixtures/nemotron35_transcribe_diarize_5min.json",
                        env!("CARGO_MANIFEST_DIR")
                    ))
                    .unwrap();
                    let out = parse_transcribe_json(&json).unwrap();
                    tx.send(EngineEvent::Progress {
                        job_id: job.job_id.clone(),
                        done_ms: 1_000,
                        total_ms: Some(300_032),
                    })
                    .unwrap();
                    for ev in file_events(
                        &job.job_id,
                        Channel::Source,
                        engine_words(&out),
                        300_032_000,
                        Duration::from_secs(11),
                        self.languages.clone(),
                    ) {
                        tx.send(ev).unwrap();
                    }
                }
                Mode::UnknownTokens => {
                    let words = [
                        "<unk><unk><unk>-l--<unk><unk>e<unk><unk><unk>",
                        "<unk>",
                        "a",
                    ]
                    .iter()
                    .enumerate()
                    .map(|(i, t)| EngineWord {
                        t0_us: i as u64 * 1_000_000,
                        t1_us: i as u64 * 1_000_000 + 500_000,
                        text: (*t).to_owned(),
                        conf: None,
                        spk: Some("spk_0".into()),
                    })
                    .collect();
                    for ev in file_events(
                        &job.job_id,
                        Channel::Source,
                        words,
                        300_032_000,
                        Duration::from_secs(1),
                        Vec::new(),
                    ) {
                        tx.send(ev).unwrap();
                    }
                }
                Mode::Loop => {
                    // "so", a loop of six on spk_0 (the last with a comma),
                    // four at human pace on spk_1, "end".
                    let at = |t_ms: u64, text: &str, spk: &str| EngineWord {
                        t0_us: t_ms * 1_000,
                        t1_us: t_ms * 1_000 + 80_000,
                        text: text.to_owned(),
                        conf: None,
                        spk: Some(spk.into()),
                    };
                    let mut words = vec![at(0, "so", "spk_0")];
                    words.extend(
                        (0..6).map(|k| at(1_000, if k == 5 { "the," } else { "the" }, "spk_0")),
                    );
                    words.extend((0..4).map(|k| at(2_000 + k * 240, "the", "spk_1")));
                    words.push(at(4_000, "end", "spk_1"));
                    for ev in file_events(
                        &job.job_id,
                        Channel::Source,
                        words,
                        300_032_000,
                        Duration::from_secs(1),
                        Vec::new(),
                    ) {
                        tx.send(ev).unwrap();
                    }
                }
                Mode::Fail => tx
                    .send(EngineEvent::Error {
                        job_id: job.job_id,
                        code: ErrorCode::Oom,
                        message: "CUDA out of memory".into(),
                    })
                    .unwrap(),
                Mode::WaitForCancel => {
                    let flag = Arc::clone(&self.cancelled);
                    std::thread::spawn(move || {
                        while !flag.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        let _ = tx.send(EngineEvent::Error {
                            job_id: job.job_id,
                            code: ErrorCode::Other,
                            message: "cancelled".into(),
                        });
                    });
                }
            }
            Ok(rx)
        }
        fn cancel(&self, _: JobId) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        fn info(&self) -> EngineInfo {
            EngineInfo {
                runtime: "fake 1.0".into(),
                ..Default::default()
            }
        }
    }

    fn request(path: &Path) -> ImportRequest {
        ImportRequest {
            job_id: JobId("job".into()),
            path: path.to_path_buf(),
            title: None,
            diarize: true,
        }
    }

    fn input(dir: &Path) -> PathBuf {
        let p = dir.join("Weekly sync.m4a");
        std::fs::write(&p, b"pretend audio").unwrap();
        p
    }

    #[test]
    fn imports_a_file_into_a_current_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let engine = FakeEngine::new(Mode::Fixture);
        let events = Mutex::new(Vec::new());
        let opts = ImportOptions {
            min_free_bytes: 0,
            ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
        };
        let id = import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&src),
            &opts,
            &CancelToken::default(),
            &|e| events.lock().unwrap().push(e),
        )
        .unwrap();
        let d = store.get_meeting(id).unwrap();
        assert_eq!(d.meeting.title, "Weekly sync");
        assert_eq!(d.meeting.status, MeetingStatus::Done);
        assert_eq!(d.meeting.duration_ms, Some(300_032));
        assert_eq!(d.current_run.as_ref().unwrap().status, RunStatus::Done);
        let info = d.current_run.unwrap().engine_info;
        assert_eq!(info["runtime"], "fake 1.0");
        assert_eq!(info["right_context"], "runtime default");
        assert_eq!(info["diar_preset"], "runtime default");
        assert_eq!(info["decoder"], "other");
        assert_eq!(info["language"], "en");
        // No punctuation model in this data root.
        assert_eq!(
            info["punctuation"],
            serde_json::json!({"applied": false, "reason": "no_model"})
        );
        assert_eq!(d.speakers.len(), 4);
        assert_eq!(store.get_words(id, 0, i64::MAX).unwrap().len(), 464);
        let segs = store.get_segments(id, 0, 10_000).unwrap();
        assert!(segs.len() > 20);
        assert!(!store.search("project", 5).unwrap().is_empty());
        let tracks = store.list_audio_tracks(id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].format, AudioFormat::Original);
        assert_eq!(tracks[0].channel, TrackChannel::Playback);
        assert_eq!(tracks[0].sample_rate, 48_000);
        // The intermediate WAV is gone; the meeting folder stays.
        let wav = engine.seen_wav.lock().unwrap().clone().unwrap();
        assert!(!wav.exists());
        assert!(wav.parent().unwrap().is_dir());
        let events = events.into_inner().unwrap();
        assert!(matches!(
            events.first(),
            Some(JobEvent::Progress {
                stage: JobStage::Decoding,
                ..
            })
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            JobEvent::Progress {
                stage: JobStage::Transcribing,
                done_ms: 1_000,
                ..
            }
        )));
        assert!(
            matches!(events.last(), Some(JobEvent::Done { meeting_id, .. }) if *meeting_id == id)
        );
    }

    #[test]
    fn imports_are_punctuated_without_changing_a_word() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let import = |punct: PunctSource| {
            let opts = ImportOptions {
                min_free_bytes: 0,
                punct,
                ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
            };
            import_file(
                &store,
                &FakeEngine::new(Mode::Fixture),
                &FakeDecoder,
                request(&src),
                &opts,
                &CancelToken::default(),
                &|_| {},
            )
            .unwrap()
        };
        let plain = import(PunctSource::None);
        let fixture = PunctSource::factory(|| {
            crate::punct::fixture::FixturePunctuator::new(|words, _| {
                Ok((0..words.len())
                    .map(|i| match i % 12 {
                        11 => crate::punct::WordLabel::post(crate::punct::Mark::Period),
                        4 => crate::punct::WordLabel::post(crate::punct::Mark::Comma),
                        _ => crate::punct::WordLabel::KEEP,
                    })
                    .collect())
            })
        });
        let punctuated = import(fixture);
        let words = |id| store.get_words(id, i64::MIN, i64::MAX).unwrap();
        let (a, b) = (words(plain), words(punctuated));
        assert_eq!(a.len(), 464);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((x.start_ms, x.end_ms), (y.start_ms, y.end_ms));
            assert_eq!(x.raw_label, y.raw_label);
            assert_eq!(
                crate::audio::echo::text::norm(&x.text),
                crate::audio::echo::text::norm(&y.text)
            );
            assert_eq!(y.asr_text(), x.text);
            assert_eq!(y.raw_text.is_some(), y.text != x.text);
        }
        assert!(b.iter().filter(|w| w.raw_text.is_some()).count() > 50);
        // The report went in through `set_run_engine_info`.
        let info = store
            .get_meeting(punctuated)
            .unwrap()
            .current_run
            .unwrap()
            .engine_info;
        let p = &info["punctuation"];
        assert_eq!(p["applied"], true, "{p}");
        assert_eq!(p["model"], "fixture");
        assert_eq!(p["rejected_words"], 0);
        assert_eq!(p["words"], 464);
        assert_eq!(info["runtime"], "fake 1.0", "the rest of engine_info stays");
        // Sentence ends split segments: more segments than without.
        let segs = |id| store.get_segments(id, 0, 10_000).unwrap().len();
        assert!(segs(punctuated) > segs(plain));
    }

    /// Plan 16 section 3.8: in `auto`, punctuation takes the language the
    /// runtime detected (`JobStats.languages`), and the run records it for
    /// a later rebuild; a meeting's own language comes first.
    #[test]
    fn an_auto_import_is_punctuated_in_the_detected_language() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let import = |language: &str, detected: &[&str], covers: &'static [&'static str]| {
            let opts = ImportOptions {
                min_free_bytes: 0,
                language: language.into(),
                punct: PunctSource::factory(move || {
                    crate::punct::fixture::FixturePunctuator::marks(&[]).languages(covers)
                }),
                ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
            };
            let mut engine = FakeEngine::new(Mode::Fixture);
            engine.languages = detected.iter().map(|l| (*l).to_owned()).collect();
            let id = import_file(
                &store,
                &engine,
                &FakeDecoder,
                request(&src),
                &opts,
                &CancelToken::default(),
                &|_| {},
            )
            .unwrap();
            store
                .get_meeting(id)
                .unwrap()
                .current_run
                .unwrap()
                .engine_info
        };
        let info = import("auto", &["de-DE"], &["de"]);
        let p = &info["punctuation"];
        assert_eq!(p["applied"], true, "{p}");
        assert_eq!(
            (&p["language"], &p["language_source"]),
            (&"de".into(), &"detected".into())
        );
        assert_eq!(info["detected_language"], "de-DE");
        // A model without the detected language leaves the run alone, where
        // the script alone (Latin) would have let a French model punctuate.
        let p = import("auto", &["de-DE"], &["fr"])["punctuation"].clone();
        assert_eq!(p["reason"], "language", "{p}");
        assert_eq!(p["language"], "de");
        // Nothing detected (the EN model): the script decides.
        let info = import("auto", &[], &["fr"]);
        assert_eq!(info["punctuation"]["language_source"], "script");
        assert!(info.get("detected_language").is_none());
        // The meeting's language outranks the detected one.
        let p = import("en", &["de-DE"], &["en"])["punctuation"].clone();
        assert_eq!(
            (&p["language"], &p["language_source"]),
            (&"en".into(), &"meeting".into())
        );
    }

    /// Plan 16 section 12.3: the loop becomes one word spanning it, the
    /// repeats at human pace (another speaker) stay, `engine_info.loops`
    /// counts it; with `transcript.collapse_loops = off` every word stays.
    #[test]
    fn imports_collapse_repeated_token_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let import = || {
            let opts = ImportOptions {
                min_free_bytes: 0,
                ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
            };
            import_file(
                &store,
                &FakeEngine::new(Mode::Loop),
                &FakeDecoder,
                request(&src),
                &opts,
                &CancelToken::default(),
                &|_| {},
            )
            .unwrap()
        };
        let words = |id| -> Vec<(i64, i64, String, Option<String>)> {
            store
                .get_words(id, i64::MIN, i64::MAX)
                .unwrap()
                .into_iter()
                .map(|w| (w.start_ms, w.end_ms, w.text, w.raw_label))
                .collect()
        };
        let loops_of = |id| {
            store
                .get_meeting(id)
                .unwrap()
                .current_run
                .unwrap()
                .engine_info["loops"]
                .clone()
        };
        let on = import();
        let w = words(on);
        assert_eq!(w.len(), 7, "{w:?}");
        assert_eq!(w[1], (1_000, 1_080, "the,".into(), Some("spk_0".into())));
        assert_eq!(
            w.iter().filter(|x| x.3.as_deref() == Some("spk_1")).count(),
            5,
            "the repeats at human pace and \"end\" stay"
        );
        let l = loops_of(on);
        assert_eq!(l["applied"], true);
        assert_eq!(
            (l["runs"].as_u64(), l["words_removed"].as_u64()),
            (Some(1), Some(5))
        );
        assert_eq!(l["per_channel"]["source"]["words_removed"], 5);
        assert_eq!(
            l["rule"],
            serde_json::json!({"min_run": 4, "frame_ms": 80, "split_ms": 400})
        );
        store
            .set_setting(crate::settings::TRANSCRIPT_COLLAPSE_LOOPS, "off")
            .unwrap();
        let off = import();
        assert_eq!(words(off).len(), 12);
        assert_eq!(loops_of(off)["applied"], false);
        assert_eq!(loops_of(off)["words_removed"], 0);
    }

    #[test]
    fn engine_failure_marks_meeting_and_run_failed() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        store
            .set_setting(crate::settings::KEEP_INTERMEDIATE_WAV, "1")
            .unwrap();
        let engine = FakeEngine::new(Mode::Fail);
        let events = Mutex::new(Vec::new());
        let opts = ImportOptions {
            min_free_bytes: 0,
            ..ImportOptions::new(&DataPaths::at(dir.path()), &store.get_settings().unwrap())
        };
        assert!(opts.keep_intermediate);
        let err = import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&src),
            &opts,
            &CancelToken::default(),
            &|e| events.lock().unwrap().push(e),
        )
        .unwrap_err();
        assert!(err.to_string().contains("out of memory"));
        let m = store.list_meetings().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].status, MeetingStatus::Failed);
        let runs = store.list_runs(m[0].id).unwrap();
        assert_eq!(runs[0].status, RunStatus::Failed);
        assert!(store.get_meeting(m[0].id).unwrap().current_run.is_none());
        // keep_intermediate_wav=1 keeps the WAV and registers it.
        assert!(engine.seen_wav.lock().unwrap().as_ref().unwrap().exists());
        assert_eq!(store.list_audio_tracks(m[0].id).unwrap().len(), 2);
        assert!(matches!(
            events.into_inner().unwrap().last(),
            Some(JobEvent::Error {
                meeting_id: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn unknown_tokens_fail_the_run_and_keep_the_meeting() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let engine = FakeEngine::new(Mode::UnknownTokens);
        let events = Mutex::new(Vec::new());
        let opts = ImportOptions {
            min_free_bytes: 0,
            language: "ml".into(),
            ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
        };
        let err = import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&src),
            &opts,
            &CancelToken::default(),
            &|e| events.lock().unwrap().push(e),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::UnknownTokens), "{err}");
        // The meeting stays (failed, no transcript) with its playback track.
        let m = store.list_meetings().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].status, MeetingStatus::Failed);
        assert!(store.get_meeting(m[0].id).unwrap().current_run.is_none());
        assert!(store.get_words(m[0].id, 0, i64::MAX).unwrap().is_empty());
        assert_eq!(
            store.list_runs(m[0].id).unwrap()[0].status,
            RunStatus::Failed
        );
        assert_eq!(store.list_audio_tracks(m[0].id).unwrap().len(), 1);
        match events.into_inner().unwrap().last() {
            Some(JobEvent::Error {
                meeting_id: Some(id),
                message,
                ..
            }) => {
                assert_eq!(*id, m[0].id);
                assert_eq!(message, languages::UNKNOWN_TOKENS_NOTICE);
            }
            other => panic!("ends with {other:?}"),
        }
    }

    #[test]
    fn cancel_reaches_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("diarist.db")).unwrap();
        let src = input(dir.path());
        let engine = FakeEngine::new(Mode::WaitForCancel);
        let token = CancelToken::default();
        let t2 = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            t2.cancel();
        });
        let opts = ImportOptions {
            min_free_bytes: 0,
            ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
        };
        let events = Mutex::new(Vec::new());
        let err = import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&src),
            &opts,
            &token,
            &|e| events.lock().unwrap().push(e),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
        // A cancelled import leaves no meeting and ends with `Cancelled`.
        assert!(store.list_meetings().unwrap().is_empty());
        assert!(matches!(
            events.into_inner().unwrap().last(),
            Some(JobEvent::Cancelled {
                meeting_id: None,
                ..
            })
        ));
    }

    #[test]
    fn refuses_without_free_space_or_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_in_memory().unwrap();
        let src = input(dir.path());
        let engine = FakeEngine::new(Mode::Fixture);
        let opts = ImportOptions {
            min_free_bytes: u64::MAX,
            ..ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default())
        };
        let events = Mutex::new(Vec::new());
        let err = import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&src),
            &opts,
            &CancelToken::default(),
            &|e| events.lock().unwrap().push(e),
        )
        .unwrap_err();
        if cfg!(windows) {
            assert!(matches!(err, CoreError::InsufficientSpace { .. }), "{err}");
            assert!(store.list_meetings().unwrap().is_empty());
            assert!(matches!(
                events.into_inner().unwrap().as_slice(),
                [JobEvent::Error {
                    meeting_id: None,
                    ..
                }]
            ));
        }
        let missing = dir.path().join("nope.m4a");
        assert!(import_file(
            &store,
            &engine,
            &FakeDecoder,
            request(&missing),
            &ImportOptions::new(&DataPaths::at(dir.path()), &Settings::default()),
            &CancelToken::default(),
            &|_| {}
        )
        .is_err());
    }
}
