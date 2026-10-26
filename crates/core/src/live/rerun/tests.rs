//! Running a finished meeting's post-pass again from its `playback.flac`
//! (plan 16 section 12.5 J). Every test hashes the FLAC before and after.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::*;
use crate::engine::{Engine, EngineEvent, EngineInfo, EngineWord, FileJob, JobStats};
use crate::engine::{LiveEngine, LiveOpts, LiveSession};
use crate::error::CoreError;
use crate::live::{recover, run_post_pass, RecoveryAction};
use crate::pipeline::{CancelToken, JobQueue};
use crate::protocol::ErrorCode;
use crate::store::{NewAudioTrack, NewMeeting, SqliteStore, Store};
use crate::types::{Channel, Word};
use crate::views::{JobEvent, JobStage};

/// Mic and loopback lengths of the fixture: the loopback 0.44 s shorter,
/// as after a recording crash recovery finished (Stop pads them equal).
const MIC: u64 = 5 * 16_000;
const LB: u64 = 4 * 16_000 + 7_000;

/// What the engine was given: the WAV's name, frames and a hash of its
/// samples.
#[derive(Debug, Clone, PartialEq)]
struct Heard {
    name: String,
    frames: u64,
    sha: Vec<u8>,
}

/// The post-pass engine: a word per second of the WAV (`q<s>` on a diarized
/// job with `spk_0`, else `p<s>`); remembers what it heard; can fail or
/// hang (until cancelled) the job with a given index.
#[derive(Default)]
struct Ears {
    heard: Mutex<Vec<Heard>>,
    fail_at: Mutex<Option<usize>>,
    hang_at: Mutex<Option<usize>>,
    hanging: AtomicBool,
    cancelled: Arc<AtomicBool>,
    /// Runs when the job with the given index starts (the test's hand in
    /// the middle of a post-pass).
    on_job: Mutex<Option<OnJob>>,
}

/// A job index and what to do when that job starts.
type OnJob = (usize, Box<dyn Fn() + Send>);

impl Ears {
    fn jobs(&self) -> usize {
        self.heard.lock().unwrap().len()
    }
}

impl Engine for Ears {
    fn transcribe_file(&self, job: FileJob) -> crate::error::Result<Receiver<EngineEvent>> {
        let mut r = hound::WavReader::open(&job.wav_path).unwrap();
        let frames = u64::from(r.duration());
        let mut h = Sha256::new();
        for s in r.samples::<i16>() {
            h.update(s.unwrap().to_le_bytes());
        }
        let index = {
            let mut heard = self.heard.lock().unwrap();
            heard.push(Heard {
                name: job
                    .wav_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                frames,
                sha: h.finalize().to_vec(),
            });
            heard.len() - 1
        };
        if let Some((at, f)) = &*self.on_job.lock().unwrap() {
            if *at == index {
                f();
            }
        }
        let (tx, rx) = channel();
        if *self.fail_at.lock().unwrap() == Some(index) {
            tx.send(EngineEvent::Error {
                job_id: job.job_id,
                code: ErrorCode::Crash,
                message: "fake crash".into(),
            })
            .unwrap();
            return Ok(rx);
        }
        if *self.hang_at.lock().unwrap() == Some(index) {
            self.hanging.store(true, Ordering::SeqCst);
            let cancelled = Arc::clone(&self.cancelled);
            std::thread::spawn(move || {
                while !cancelled.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = tx.send(EngineEvent::Error {
                    job_id: job.job_id,
                    code: ErrorCode::Cancelled,
                    message: "cancelled".into(),
                });
            });
            return Ok(rx);
        }
        let words = (0..frames / 16_000)
            .map(|s| EngineWord {
                t0_us: s * 1_000_000 + 50_000,
                t1_us: s * 1_000_000 + 900_000,
                text: if job.diarize {
                    format!("q{s}")
                } else {
                    format!("p{s}")
                },
                conf: Some(0.5),
                spk: job.diarize.then(|| "spk_0".to_owned()),
            })
            .collect();
        tx.send(EngineEvent::Utterance {
            job_id: job.job_id.clone(),
            channel: Channel::Source,
            utt: 0,
            rev: 0,
            words,
        })
        .unwrap();
        tx.send(EngineEvent::Done {
            job_id: job.job_id,
            stats: JobStats::default(),
        })
        .unwrap();
        Ok(rx)
    }
    fn cancel(&self, _: JobId) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    fn info(&self) -> EngineInfo {
        EngineInfo {
            runtime: "fake 1".into(),
            right_context: Some(13),
            diar_preset: Some("v3-offline".into()),
            ..Default::default()
        }
    }
}

/// No live sessions in these tests.
struct NoLive;
impl LiveEngine for NoLive {
    fn open(
        &self,
        _: JobId,
        _: Channel,
        _: LiveOpts,
    ) -> crate::error::Result<Box<dyn LiveSession>> {
        Err(CoreError::Engine("no live engine in this test".into()))
    }
}

fn sha256(p: &Path) -> Vec<u8> {
    Sha256::digest(std::fs::read(p).unwrap()).to_vec()
}

/// Deterministic noise for a WAV (`seed` tells the channels apart).
fn noise(seed: u32, n: u64) -> Vec<i16> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            ((s % 8_001) as i32 - 4_000) as i16
        })
        .collect()
}

fn write_wav(p: &Path, pcm: &[i16]) {
    let mut w = crate::audio::wav::WavSink::create(p).unwrap();
    w.write(pcm).unwrap();
    w.finalize().unwrap();
}

/// A finished live meeting as the first post-pass left it: its
/// `post_pass` run current, `playback.flac` registered with each
/// channel's length, the WAVs gone; the live run kept with `keep_live`.
struct Finished {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ctx: LiveContext,
    store: Arc<SqliteStore>,
    ears: Arc<Ears>,
    events: Arc<Mutex<Vec<JobEvent>>>,
    meeting: MeetingId,
    live: RunId,
    first: RunId,
    folder: PathBuf,
    /// What the first post-pass heard.
    heard: Vec<Heard>,
}

impl Finished {
    fn flac(&self) -> PathBuf {
        self.folder.join("playback.flac")
    }

    /// The things a run from the FLAC must leave as they are: the FLAC's
    /// bytes, the gain track's bytes and the track rows.
    fn untouched(&self) -> (Vec<u8>, Vec<u8>, Vec<AudioTrack>) {
        (
            sha256(&self.flac()),
            sha256(&self.folder.join("loopback.gain")),
            self.store.list_audio_tracks(self.meeting).unwrap(),
        )
    }

    /// Nothing a post-pass makes beside the FLAC is left: the WAVs, their
    /// `.part` files, the canceller's output, the marker.
    fn no_wavs_left(&self) {
        for f in [
            "mic.wav",
            "loopback.wav",
            "mic.wav.part",
            "loopback.wav.part",
            "mic-aec.wav",
            RERUN_MARKER,
        ] {
            assert!(!self.folder.join(f).exists(), "{f} was left");
        }
    }

    fn status(&self) -> MeetingStatus {
        self.store.get_meeting(self.meeting).unwrap().meeting.status
    }

    fn current(&self) -> RunId {
        self.store
            .get_meeting(self.meeting)
            .unwrap()
            .current_run
            .unwrap()
            .id
    }

    fn runs(&self) -> Vec<(RunId, RunKind, RunStatus)> {
        self.store
            .list_runs(self.meeting)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.kind, r.status))
            .collect()
    }

    fn wait(&self, h: PostPassHandle) -> std::result::Result<super::super::PostPassReport, String> {
        h.wait(Duration::from_secs(30)).expect("the job ended")
    }
}

fn finished(keep_live: bool) -> Finished {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = Arc::new(SqliteStore::open(root.join("diarist.db")).unwrap());
    let ears = Arc::new(Ears::default());
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let sink = Arc::clone(&events);
    let ctx = LiveContext {
        store: store.clone(),
        queue: Arc::new(JobQueue::new()),
        data_root: root.clone(),
        live_engine: Arc::new(NoLive),
        file_engine: ears.clone(),
        sources: Arc::new(|_, _| Err(CoreError::Engine("no capture in this test".into()))),
        vram_free_mb: Arc::new(|| None),
        min_free_vram_mb: 0,
        min_free_bytes: 0,
        live_engine_info: serde_json::json!({"runtime": "fake live"}),
        on_job_event: Some(Arc::new(move |e| sink.lock().unwrap().push(e))),
        punct: crate::punct::PunctSource::None,
    };
    store
        .set_setting(
            settings::KEEP_LIVE_RUNS,
            if keep_live { "true" } else { "false" },
        )
        .unwrap();
    let meeting = store
        .create_meeting(NewMeeting {
            title: "Standup".into(),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_791_000_000_000,
            status: MeetingStatus::Processing,
        })
        .unwrap();
    let ms = (MIC * 1_000 / 16_000) as i64;
    store
        .finish_meeting(meeting.id, meeting.started_at + ms, ms)
        .unwrap();
    let live = store
        .create_run(
            meeting.id,
            RunKind::Live,
            serde_json::json!({"runtime": "fake live"}),
        )
        .unwrap()
        .id;
    for (ch, text, spk) in [
        (Channel::Loopback, "hello", Some("spk_0")),
        (Channel::Mic, "hi", None),
    ] {
        store
            .append_live_utterance(
                live,
                ch,
                0,
                0,
                &[EngineWord {
                    t0_us: 1_050_000,
                    t1_us: 1_900_000,
                    text: text.into(),
                    conf: None,
                    spk: spk.map(str::to_owned),
                }],
            )
            .unwrap();
    }
    store
        .materialise_live_run(live, &SegmentOptions::default())
        .unwrap();
    store.finish_run(live, RunStatus::Done).unwrap();
    store.set_current_run(meeting.id, live).unwrap();
    let folder = root.join("audio").join(&meeting.uid);
    write_wav(&folder.join("mic.wav"), &noise(7, MIC));
    write_wav(&folder.join("loopback.wav"), &noise(99, LB));
    // The loopback AGC's gain track, which the decoded WAV finds by name.
    let mut g =
        crate::audio::gain_track::GainTrackWriter::create(&folder.join("loopback.gain")).unwrap();
    for k in 0..LB / 1_600 {
        g.push(k * 1_600, &[3.0; 10]).unwrap();
    }
    g.flush().unwrap();
    drop(g);
    register_wavs(&*store, meeting.id, &root, &folder, ms).unwrap();
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run: live,
        data_root: root.clone(),
        folder: folder.clone(),
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: keep_live,
        segment: SegmentOptions::default(),
        punct: crate::punct::PunctSource::None,
        from_flac: false,
    };
    let report = run_post_pass(&*store, &*ears, &job, &CancelToken::default(), &|_| {}).unwrap();
    let heard = ears.heard.lock().unwrap().clone();
    let f = Finished {
        _dir: dir,
        root,
        ctx,
        store,
        ears,
        events,
        meeting: meeting.id,
        live,
        first: report.current_run,
        folder,
        heard,
    };
    assert_eq!(f.status(), MeetingStatus::Done);
    f.no_wavs_left();
    assert_eq!(report.live_run_deleted, !keep_live);
    f
}

/// Every word of a run with its speaker's channel and label, in order.
fn words_of(store: &dyn RunStore, run: RunId) -> Vec<(Channel, String, Word)> {
    let speakers = store.run_speakers(run).unwrap();
    let segments = store.run_segments(run).unwrap();
    let mut out: Vec<(Channel, String, Word)> = store
        .run_words(run)
        .unwrap()
        .into_iter()
        .map(|mut w| {
            let seg = segments.iter().find(|s| s.id == w.segment_id).unwrap();
            let spk = speakers.iter().find(|s| s.id == seg.speaker_id).unwrap();
            w.id = crate::types::WordId(0);
            w.segment_id = crate::types::SegmentId(0);
            (spk.channel, spk.engine_label.clone(), w)
        })
        .collect();
    out.sort_by(|a, b| (a.0, a.2.start_ms, &a.2.text).cmp(&(b.0, b.2.start_ms, &b.2.text)));
    out
}

/// `engine_info.echo` without its wall times.
fn echo_without_walls(info: &serde_json::Value) -> serde_json::Value {
    let mut e = info["echo"].clone();
    if let Some(o) = e.as_object_mut() {
        o.remove("aec_wall_s");
        o.remove("aec_wait_s");
        if let Some(t) = o.get_mut("transfer").and_then(|t| t.as_object_mut()) {
            t.remove("raw_wall_s");
            t.remove("gate_s");
        }
    }
    e
}

#[test]
fn a_finished_meeting_runs_again_from_its_flac_and_the_flac_stays_as_it_is() {
    let f = finished(true);
    // The first post-pass stored each channel's own length with the FLAC.
    let tracks = f.store.list_audio_tracks(f.meeting).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].channel_frames, Some(vec![MIC, LB]));
    let before = f.untouched();
    assert_eq!(
        rerun_check(&*f.store, &f.root, f.meeting).unwrap(),
        RerunCheck {
            audio: Some(RerunAudio::Flac),
            blocked: None
        }
    );

    let h = rerun_post_pass(&f.ctx, f.meeting).unwrap();
    let report = f.wait(h).unwrap();

    // The FLAC, the gain track and the track row are as they were; the
    // decoded WAVs are gone.
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    assert_eq!(report.flac_frames, MIC);
    assert_eq!(
        report.flac_bytes,
        std::fs::metadata(f.flac()).unwrap().len()
    );
    // The engine heard the recorded WAVs again, sample for sample and each
    // at its own length.
    let again = f.ears.heard.lock().unwrap()[f.heard.len()..].to_vec();
    assert_eq!(again, f.heard);
    assert!(again
        .iter()
        .any(|h| h.name == "loopback.wav" && h.frames == LB));
    // The new run is current, the earlier one and the live run stay.
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.current(), report.current_run);
    assert_ne!(report.current_run, f.first);
    let first = f.store.get_run(f.first).unwrap();
    assert_eq!(
        (first.kind, first.status),
        (RunKind::PostPass, RunStatus::Done)
    );
    assert!(!report.live_run_deleted);
    assert_eq!(f.store.get_run(f.live).unwrap().kind, RunKind::Live);
    // Same audio, same engine: the same words, echo summary and drops.
    let new = f.store.get_run(report.current_run).unwrap();
    assert_eq!(
        words_of(&*f.store, report.current_run),
        words_of(&*f.store, f.first)
    );
    assert!(!words_of(&*f.store, f.first).is_empty());
    assert_eq!(
        echo_without_walls(&new.engine_info),
        echo_without_walls(&first.engine_info)
    );
    assert_eq!(
        f.store.echo_dropped(report.current_run).unwrap(),
        f.store.echo_dropped(f.first).unwrap()
    );
    // The decode showed as "Reading the recording" first (this engine
    // reports no progress of its own).
    let events = f.events.lock().unwrap().clone();
    let stages: Vec<JobStage> = events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Progress { stage, .. } => Some(*stage),
            _ => None,
        })
        .collect();
    assert_eq!(stages, vec![JobStage::Decoding, JobStage::Saving]);
    assert!(events.iter().any(|e| matches!(
        e,
        JobEvent::Progress { stage: JobStage::Decoding, done_ms, total_ms: Some(t), .. }
            if *done_ms == *t && *t == MIC * 1_000 / 16_000
    )));
    assert!(events.iter().any(|e| matches!(e, JobEvent::Done { .. })));

    // Once more: every earlier run stays.
    let report2 = f.wait(rerun_post_pass(&f.ctx, f.meeting).unwrap()).unwrap();
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    let post: Vec<RunId> = f
        .runs()
        .into_iter()
        .filter(|r| r.1 == RunKind::PostPass)
        .map(|r| r.0)
        .collect();
    assert_eq!(post, vec![f.first, report.current_run, report2.current_run]);
    assert_eq!(f.current(), report2.current_run);
}

#[test]
fn a_flac_from_before_beta5_decodes_both_channels_at_its_length() {
    let f = finished(true);
    // The row as an older post-pass registered it: no channel lengths.
    let t = f.store.list_audio_tracks(f.meeting).unwrap().remove(0);
    f.store.delete_audio_track(t.id).unwrap();
    f.store
        .add_audio_track(NewAudioTrack {
            meeting_id: t.meeting_id,
            channel: t.channel,
            path: t.path.clone(),
            format: t.format,
            sample_rate: t.sample_rate,
            channels: t.channels,
            duration_ms: t.duration_ms,
            size_bytes: t.size_bytes,
            channel_frames: None,
        })
        .unwrap();
    let before = f.untouched();
    let report = f.wait(rerun_post_pass(&f.ctx, f.meeting).unwrap()).unwrap();
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    let again = f.ears.heard.lock().unwrap()[f.heard.len()..].to_vec();
    let lb = again.iter().find(|h| h.name == "loopback.wav").unwrap();
    assert_eq!(lb.frames, MIC, "padded to the FLAC's length");
    let mic = again.iter().find(|h| h.name == "mic.wav").unwrap();
    assert_eq!(mic, f.heard.iter().find(|h| h.name == "mic.wav").unwrap());
    assert_eq!(f.current(), report.current_run);
}

#[test]
fn a_run_from_the_flac_that_fails_midway_leaves_the_flac_and_the_transcript() {
    let f = finished(true);
    let before = f.untouched();
    let runs = f.runs();
    // The engine fails on the mic, after the loopback was transcribed.
    *f.ears.fail_at.lock().unwrap() = Some(f.ears.jobs() + 1);
    let err = f
        .wait(rerun_post_pass(&f.ctx, f.meeting).unwrap())
        .unwrap_err();
    assert!(err.contains("fake crash"), "{err}");
    assert_eq!(f.ears.jobs(), f.heard.len() + 2);
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    // Nothing changed: the meeting is done with the transcript it had.
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.current(), f.first);
    assert_eq!(f.runs(), runs);
    assert!(f
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, JobEvent::Error { message, .. } if message.contains("fake crash"))));

    // The decode itself fails: stored lengths that do not fit the FLAC.
    let t = f.store.list_audio_tracks(f.meeting).unwrap().remove(0);
    f.store.delete_audio_track(t.id).unwrap();
    f.store
        .add_audio_track(NewAudioTrack {
            meeting_id: t.meeting_id,
            channel: t.channel,
            path: t.path.clone(),
            format: t.format,
            sample_rate: t.sample_rate,
            channels: t.channels,
            duration_ms: t.duration_ms,
            size_bytes: t.size_bytes,
            channel_frames: Some(vec![MIC, MIC + 16_000]),
        })
        .unwrap();
    let jobs = f.ears.jobs();
    let err = f
        .wait(rerun_post_pass(&f.ctx, f.meeting).unwrap())
        .unwrap_err();
    assert!(err.contains("do not fit"), "{err}");
    assert_eq!(f.ears.jobs(), jobs, "nothing was transcribed");
    assert_eq!(sha256(&f.flac()), before.0);
    f.no_wavs_left();
    assert!(!f.folder.join(RERUN_MARKER).exists());
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.current(), f.first);
    assert_eq!(f.runs(), runs);
}

#[test]
fn a_run_from_the_flac_that_fails_after_making_its_run_leaves_the_transcript() {
    // The FLAC goes out of reach while the mic is transcribed: the run is
    // made and filled, then the size check before the switch fails. The
    // transcript is the one the meeting had and the new run is gone.
    let f = finished(true);
    let runs = f.runs();
    let flac = f.flac();
    let away = f.folder.join("moved.flac");
    let (from, to) = (flac.clone(), away.clone());
    *f.ears.on_job.lock().unwrap() = Some((
        f.ears.jobs() + 1,
        Box::new(move || std::fs::rename(&from, &to).unwrap()),
    ));
    let err = f
        .wait(rerun_post_pass(&f.ctx, f.meeting).unwrap())
        .unwrap_err();
    assert!(
        err.contains("cannot look at") && err.contains("playback.flac"),
        "{err}"
    );
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.current(), f.first);
    assert_eq!(f.runs(), runs, "the new run was deleted");
    assert!(!f.folder.join("mic.wav").exists() && !f.folder.join("loopback.wav").exists());
    assert!(!f.folder.join(RERUN_MARKER).exists());
    std::fs::rename(&away, &flac).unwrap();
}

#[test]
fn a_cancelled_run_from_the_flac_leaves_the_flac_and_the_transcript() {
    let f = finished(true);
    let before = f.untouched();
    let runs = f.runs();
    *f.ears.hang_at.lock().unwrap() = Some(f.ears.jobs());
    let h = rerun_post_pass(&f.ctx, f.meeting).unwrap();
    for _ in 0..500 {
        if f.ears.hanging.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(f.ears.hanging.load(Ordering::SeqCst));
    // The decoded WAVs exist while the job runs, with the marker beside
    // them (no run yet: the engine has not answered).
    assert!(f.folder.join("mic.wav").is_file() && f.folder.join("loopback.wav").is_file());
    let marker = read_marker(&f.folder).unwrap().unwrap();
    assert_eq!(marker.job, h.job_id.to_string());
    assert_eq!(marker.run, None);
    f.ctx.queue.cancel(&h.job_id);
    let err = f.wait(h).unwrap_err();
    assert!(err.contains("ancel"), "{err}");
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    assert!(!f.folder.join(RERUN_MARKER).exists());
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.current(), f.first);
    assert_eq!(f.runs(), runs);
}

/// How far a run from the FLAC got when the app died.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Crash {
    /// Half way through the decode: `.part` files.
    Decoding,
    /// Transcribing: the decoded WAVs and the canceller's output.
    Transcribing,
    /// Its run made and named in the marker, not finished.
    RunMade,
    /// Its run finished, not current yet (between `finish_run` and
    /// `set_current_run`).
    RunFinished,
    /// Its run current, the decoded copies not deleted yet.
    RunCurrent,
}

/// Leaves the meeting as a run from the FLAC that died at `at` left it:
/// `processing`, the marker, the files of that stage. Returns the run the
/// job made, if it got that far.
fn crash_mid_rerun(f: &Finished, at: Crash) -> Option<RunId> {
    f.store
        .set_meeting_status(f.meeting, MeetingStatus::Processing)
        .unwrap();
    let (mic, lb) = (f.folder.join("mic.wav"), f.folder.join("loopback.wav"));
    let run = matches!(at, Crash::RunMade | Crash::RunFinished | Crash::RunCurrent).then(|| {
        let id = f
            .store
            .create_run(f.meeting, RunKind::PostPass, serde_json::json!({}))
            .unwrap()
            .id;
        if at != Crash::RunMade {
            f.store.finish_run(id, RunStatus::Done).unwrap();
        }
        if at == Crash::RunCurrent {
            f.store.set_current_run(f.meeting, id).unwrap();
        }
        id
    });
    write_marker(
        &f.folder,
        &RerunMarker {
            job: "job-that-died".into(),
            run: run.map(|r| r.0),
        },
    )
    .unwrap();
    if at == Crash::Decoding {
        write_wav(&part_path(&mic), &noise(7, MIC / 2));
        let mut w = crate::audio::wav::WavSink::create(&part_path(&lb)).unwrap();
        w.write(&noise(99, 20_000)).unwrap();
        std::mem::forget(w);
    } else {
        decode_stereo_to_wavs(&f.flac(), &mic, &lb, Some([MIC, LB]), &|_, _| {}).unwrap();
        std::fs::write(f.folder.join("mic-aec.wav"), b"RIFF").unwrap();
    }
    run
}

#[test]
fn crash_recovery_after_a_run_from_the_flac_keeps_the_flac_and_every_earlier_run() {
    for at in [
        Crash::Decoding,
        Crash::Transcribing,
        Crash::RunMade,
        Crash::RunFinished,
        Crash::RunCurrent,
    ] {
        let f = finished(true);
        let before = f.untouched();
        let runs = f.runs();
        let made = crash_mid_rerun(&f, at);

        let recovered = recover(&f.ctx).unwrap();
        assert_eq!(recovered.len(), 1, "{at:?}");
        let r = &recovered[0];
        assert_eq!(r.action, RecoveryAction::Finished, "{at:?}");
        assert!(r.note.is_none(), "{at:?}: {:?}", r.note);
        assert!(
            r.post_pass.is_none(),
            "{at:?}: nothing runs over the copies"
        );
        assert_eq!(r.live_run, Some(f.live));
        // The FLAC, its row and the gain track as they were; every file the
        // job made is gone, the marker last.
        assert_eq!(f.untouched(), before, "{at:?}");
        f.no_wavs_left();
        for p in [
            part_path(&f.folder.join("mic.wav")),
            part_path(&f.folder.join("loopback.wav")),
            f.folder.join(RERUN_MARKER),
        ] {
            assert!(!p.exists(), "{at:?}: {} was left", p.display());
        }
        assert_eq!(f.status(), MeetingStatus::Done, "{at:?}");
        if at == Crash::RunCurrent {
            // It had become current: it stays, with every earlier run.
            let made = made.unwrap();
            assert_eq!(f.current(), made);
            let mut want = runs.clone();
            want.push((made, RunKind::PostPass, RunStatus::Done));
            assert_eq!(f.runs(), want);
        } else {
            // The job's own run went (unfinished, or finished and never
            // current); every earlier run stays.
            if let Some(run) = made {
                assert!(f.store.get_run(run).is_err(), "{at:?}");
            }
            assert_eq!(f.runs(), runs, "{at:?}");
            assert_eq!(f.current(), f.first);
        }
        assert!(recover(&f.ctx).unwrap().is_empty(), "{at:?}");
        // And it can run again.
        let report = f.wait(rerun_post_pass(&f.ctx, f.meeting).unwrap()).unwrap();
        assert_eq!(f.current(), report.current_run);
        assert_eq!(f.untouched(), before);
        assert!(f.store.get_run(f.first).is_ok());
    }
}

/// Holds `p` open with no sharing (another program holding the file), so
/// nothing else can open it until this drops; looking at it still works.
#[cfg(windows)]
fn lock(p: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(p)
        .unwrap()
}

#[test]
fn a_crash_mid_rerun_with_the_flac_out_of_reach_changes_nothing() {
    // The FLAC renamed away, a folder in its place, and (Windows) locked.
    let mut cases = vec!["renamed away", "a folder in its place"];
    if cfg!(windows) {
        cases.push("locked");
    }
    for case in cases {
        let f = finished(true);
        let made = crash_mid_rerun(&f, Crash::RunFinished).unwrap();
        let flac_sha = sha256(&f.flac());
        let runs = f.runs();
        let tracks = f.store.list_audio_tracks(f.meeting).unwrap();
        let (mic, lb) = (f.folder.join("mic.wav"), f.folder.join("loopback.wav"));
        let copies = (sha256(&mic), sha256(&lb));
        let away = f.folder.join("moved.flac");
        #[cfg(windows)]
        let mut held = None;
        match case {
            "renamed away" => std::fs::rename(f.flac(), &away).unwrap(),
            "a folder in its place" => {
                std::fs::rename(f.flac(), &away).unwrap();
                std::fs::create_dir(f.flac()).unwrap();
            }
            _ => {
                #[cfg(windows)]
                {
                    held = Some(lock(&f.flac()));
                }
            }
        }

        let recovered = recover(&f.ctx).unwrap();
        assert_eq!(recovered.len(), 1, "{case}");
        let r = &recovered[0];
        assert_eq!(r.action, RecoveryAction::Untouched, "{case}");
        let note = r.note.clone().unwrap();
        assert!(
            note.contains("playback.flac") && note.contains("changed nothing")
                || note.contains("Nothing was deleted"),
            "{case}: {note}"
        );
        // Failed, with a clear message; every run and every file as it was;
        // nothing encoded.
        assert_eq!(f.status(), MeetingStatus::Failed, "{case}");
        assert_eq!(f.runs(), runs, "{case}");
        assert!(f.store.get_run(made).is_ok());
        assert_eq!(f.current(), f.first);
        assert_eq!(f.store.list_audio_tracks(f.meeting).unwrap(), tracks);
        assert_eq!((sha256(&mic), sha256(&lb)), copies, "{case}");
        assert!(f.folder.join(RERUN_MARKER).is_file());
        // A run again is refused while the files are there.
        #[cfg(windows)]
        let still_locked = held.is_some();
        #[cfg(not(windows))]
        let still_locked = false;
        if !still_locked && case != "renamed away" {
            let e = rerun_post_pass(&f.ctx, f.meeting)
                .err()
                .unwrap()
                .to_string();
            assert!(
                e.contains("not a file") || e.contains("Audio files"),
                "{case}: {e}"
            );
        }
        // Put back: the next start finishes it, the earlier runs stay.
        #[cfg(windows)]
        drop(held.take());
        if case != "locked" {
            if case == "a folder in its place" {
                std::fs::remove_dir(f.flac()).unwrap();
            }
            std::fs::rename(&away, f.flac()).unwrap();
        }
        assert_eq!(sha256(&f.flac()), flac_sha, "{case}: the FLAC changed");
        let recovered = recover(&f.ctx).unwrap();
        assert_eq!(recovered.len(), 1, "{case}");
        assert_eq!(recovered[0].was, MeetingStatus::Failed);
        assert_eq!(recovered[0].action, RecoveryAction::Finished, "{case}");
        assert_eq!(f.status(), MeetingStatus::Done);
        f.no_wavs_left();
        assert!(!f.folder.join(RERUN_MARKER).exists());
        assert!(f.store.get_run(made).is_err(), "the job's orphan run went");
        assert_eq!(f.current(), f.first);
        assert_eq!(sha256(&f.flac()), flac_sha);
        assert!(recover(&f.ctx).unwrap().is_empty());
    }
}

#[test]
fn a_damaged_flac_with_the_original_wavs_beside_it_keeps_them() {
    // A post-pass that registered its FLAC and died before deleting the
    // WAVs it was made from; then the FLAC was damaged.
    for damage in ["a byte flipped", "cut short", "none"] {
        let f = finished(true);
        let flac_bytes = std::fs::read(f.flac()).unwrap();
        f.store
            .set_meeting_status(f.meeting, MeetingStatus::Processing)
            .unwrap();
        let (mic, lb) = (f.folder.join("mic.wav"), f.folder.join("loopback.wav"));
        write_wav(&mic, &noise(7, MIC));
        write_wav(&lb, &noise(99, LB));
        register_wavs(&*f.store, f.meeting, &f.root, &f.folder, 5_000).unwrap();
        let originals = (sha256(&mic), sha256(&lb));
        let runs = f.runs();
        match damage {
            "a byte flipped" => {
                let mut b = flac_bytes.clone();
                let at = b.len() * 2 / 3;
                b[at] ^= 0x10;
                std::fs::write(f.flac(), &b).unwrap();
            }
            "cut short" => {
                std::fs::write(f.flac(), &flac_bytes[..flac_bytes.len() - 1_000]).unwrap()
            }
            _ => {}
        }
        let damaged = sha256(&f.flac());
        let recovered = recover(&f.ctx).unwrap();
        assert_eq!(recovered.len(), 1);
        if damage == "none" {
            // A good FLAC: the WAVs were copies of it and go.
            assert_eq!(recovered[0].action, RecoveryAction::Finished);
            f.no_wavs_left();
            assert_eq!(f.store.list_audio_tracks(f.meeting).unwrap().len(), 1);
            assert_eq!(f.status(), MeetingStatus::Done);
            continue;
        }
        assert_eq!(recovered[0].action, RecoveryAction::Untouched, "{damage}");
        let note = recovered[0].note.clone().unwrap();
        assert!(note.contains("could not be matched"), "{damage}: {note}");
        // The originals survive, with their rows; the FLAC is not encoded
        // again (still the damaged bytes); the runs as they were.
        assert_eq!((sha256(&mic), sha256(&lb)), originals, "{damage}");
        assert_eq!(f.store.list_audio_tracks(f.meeting).unwrap().len(), 3);
        assert_eq!(sha256(&f.flac()), damaged);
        assert_eq!(f.runs(), runs);
        assert_eq!(f.status(), MeetingStatus::Failed);
        // Every start says so again, and changes nothing.
        let again = recover(&f.ctx).unwrap();
        assert_eq!(again[0].action, RecoveryAction::Untouched);
        assert_eq!((sha256(&mic), sha256(&lb)), originals);
        // And a run from the FLAC is refused.
        let e = rerun_post_pass(&f.ctx, f.meeting)
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("Audio files are already beside"), "{e}");
        assert_eq!((sha256(&mic), sha256(&lb)), originals);
    }
}

#[test]
fn a_run_from_the_flac_never_goes_over_files_beside_it() {
    let f = finished(true);
    let before = f.untouched();
    let runs = f.runs();
    let (mic, lb) = (f.folder.join("mic.wav"), f.folder.join("loopback.wav"));
    // Any of mic.wav, loopback.wav or a marker beside the FLAC: refused
    // before anything changes, the file as it was.
    for name in ["mic.wav", "loopback.wav", RERUN_MARKER] {
        let p = f.folder.join(name);
        std::fs::write(&p, b"somebody's file").unwrap();
        assert_eq!(
            rerun_check(&*f.store, &f.root, f.meeting).unwrap().audio,
            Some(RerunAudio::Flac)
        );
        let e = rerun_post_pass(&f.ctx, f.meeting)
            .err()
            .unwrap()
            .to_string();
        assert!(
            e.contains("Audio files are already beside") && e.contains(name),
            "{e}"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"somebody's file");
        assert_eq!(f.status(), MeetingStatus::Done);
        assert_eq!(f.runs(), runs);
        std::fs::remove_file(&p).unwrap();
    }
    // One that appears while the job waits in the queue: the job refuses it
    // at its start and leaves the file, the transcript and the FLAC.
    f.ctx.queue.set_paused(true);
    let h = rerun_post_pass(&f.ctx, f.meeting).unwrap();
    assert_eq!(f.status(), MeetingStatus::Processing);
    write_wav(&lb, &noise(5, 1_000));
    let theirs = sha256(&lb);
    f.ctx.queue.set_paused(false);
    let e = f.wait(h).unwrap_err();
    assert!(e.contains("Audio files are already beside"), "{e}");
    assert_eq!(sha256(&lb), theirs);
    assert!(!mic.exists() && !part_path(&mic).exists() && !part_path(&lb).exists());
    assert!(!f.folder.join(RERUN_MARKER).exists());
    assert_eq!(f.untouched(), before);
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.runs(), runs);
    assert_eq!(f.current(), f.first);
}

#[test]
fn processing_again_keeps_the_live_run_with_keep_live_runs_off() {
    // The live run kept (it was on then), the setting off now, and the live
    // run current (say a Restore made it so).
    let f = finished(true);
    f.store
        .set_setting(settings::KEEP_LIVE_RUNS, "false")
        .unwrap();
    f.store.set_current_run(f.meeting, f.live).unwrap();
    let runs = f.runs();
    let report = f.wait(rerun_post_pass(&f.ctx, f.meeting).unwrap()).unwrap();
    assert!(!report.live_run_deleted);
    assert_eq!(f.store.get_run(f.live).unwrap().kind, RunKind::Live);
    assert!(f.store.get_run(f.first).is_ok());
    let mut want = runs;
    want.push((report.current_run, RunKind::PostPass, RunStatus::Done));
    assert_eq!(f.runs(), want);
    assert_eq!(f.current(), report.current_run);
    f.no_wavs_left();
}

#[test]
fn a_run_from_the_flac_needs_room_for_the_decoded_audio() {
    let f = finished(true);
    let track = f.store.list_audio_tracks(f.meeting).unwrap().remove(0);
    // Both channels and the canceller's output at 2 bytes a frame, three
    // WAV headers and the headroom: about 115 MB per channel-hour.
    let need = decode_space(&track);
    assert_eq!(need, (2 * MIC + LB) * 2 + 3 * 44 + SPACE_HEADROOM);
    let hour = AudioTrack {
        channel_frames: None,
        duration_ms: Some(3_600_000),
        ..track.clone()
    };
    let per_channel_hour = (decode_space(&hour) - SPACE_HEADROOM - 3 * 44) / 3;
    assert_eq!(per_channel_hour, 115_200_000);
    assert!(space_check(&f.folder, Some(need), need).is_ok());
    assert!(space_check(&f.folder, None, need).is_ok());
    let e = space_check(&f.folder, Some(need - 1), need).unwrap_err();
    assert!(matches!(e, CoreError::InsufficientSpace { .. }), "{e}");
    assert!(e.to_string().contains("not enough free disk space"), "{e}");
}

#[test]
fn no_post_pass_encodes_over_a_registered_flac() {
    let f = finished(true);
    let before = f.untouched();
    // WAVs that are not what the FLAC holds (silence), beside it, and a
    // post-pass over them as a run from the WAVs: the FLAC is kept, and so
    // are they.
    write_wav(&f.folder.join("mic.wav"), &vec![0; MIC as usize]);
    write_wav(&f.folder.join("loopback.wav"), &vec![0; MIC as usize]);
    let silence = sha256(&f.folder.join("mic.wav"));
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting: f.meeting,
        live_run: f.live,
        data_root: f.root.clone(),
        folder: f.folder.clone(),
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: true,
        segment: SegmentOptions::default(),
        punct: crate::punct::PunctSource::None,
        from_flac: false,
    };
    let e = run_post_pass(&*f.store, &*f.ears, &job, &CancelToken::default(), &|_| {})
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("does not hold what mic.wav and loopback.wav hold"),
        "{e}"
    );
    assert_eq!(f.untouched(), before);
    assert_eq!(sha256(&f.folder.join("mic.wav")), silence);
    // The WAVs it was made from: kept, and they go.
    write_wav(&f.folder.join("mic.wav"), &noise(7, MIC));
    write_wav(&f.folder.join("loopback.wav"), &noise(99, LB));
    let report =
        run_post_pass(&*f.store, &*f.ears, &job, &CancelToken::default(), &|_| {}).unwrap();
    assert_eq!(f.untouched(), before);
    f.no_wavs_left();
    assert_eq!(report.flac_frames, MIC);
    // A row whose file is gone is replaced by a new encode.
    std::fs::rename(f.flac(), f.folder.join("moved.flac")).unwrap();
    write_wav(&f.folder.join("mic.wav"), &noise(7, MIC));
    write_wav(&f.folder.join("loopback.wav"), &noise(99, LB));
    run_post_pass(&*f.store, &*f.ears, &job, &CancelToken::default(), &|_| {}).unwrap();
    let tracks = f.store.list_audio_tracks(f.meeting).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_ne!(tracks[0].id, before.2[0].id);
    assert_eq!(tracks[0].channel_frames, Some(vec![MIC, LB]));
    assert_eq!(sha256(&f.flac()), before.0, "the same WAVs, the same FLAC");
}

#[test]
fn a_run_again_needs_the_live_run_and_the_audio() {
    // keep_live_runs off: the live run went with the first post-pass.
    let f = finished(false);
    let before = f.untouched();
    let check = rerun_check(&*f.store, &f.root, f.meeting).unwrap();
    assert_eq!(check.audio, Some(RerunAudio::Flac));
    assert_eq!(check.blocked.as_deref(), Some(NO_LIVE_RUN));
    let e = rerun_post_pass(&f.ctx, f.meeting)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("Keep the live transcript"), "{e}");
    assert_eq!(f.status(), MeetingStatus::Done);
    assert_eq!(f.untouched(), before);

    // With the live run but neither the FLAC nor the WAVs.
    let f = finished(true);
    std::fs::rename(f.flac(), f.folder.join("elsewhere.flac")).unwrap();
    let check = rerun_check(&*f.store, &f.root, f.meeting).unwrap();
    assert_eq!(
        check,
        RerunCheck {
            audio: None,
            blocked: Some(AUDIO_GONE.into())
        }
    );
    let e = rerun_post_pass(&f.ctx, f.meeting)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("audio is gone"), "{e}");
    assert_eq!(f.status(), MeetingStatus::Done);
    // Only one of the WAVs: still gone.
    write_wav(&f.folder.join("mic.wav"), &noise(7, MIC));
    assert_eq!(
        rerun_check(&*f.store, &f.root, f.meeting)
            .unwrap()
            .blocked
            .as_deref(),
        Some(AUDIO_GONE)
    );
    // Both WAVs (a cancelled post-pass left them): from the WAVs.
    write_wav(&f.folder.join("loopback.wav"), &noise(99, LB));
    assert_eq!(
        rerun_check(&*f.store, &f.root, f.meeting).unwrap(),
        RerunCheck {
            audio: Some(RerunAudio::Wavs),
            blocked: None
        }
    );
    // While a post-pass is queued or running, and for an import.
    f.store
        .set_meeting_status(f.meeting, MeetingStatus::Processing)
        .unwrap();
    let check = rerun_check(&*f.store, &f.root, f.meeting).unwrap();
    assert!(check.blocked.unwrap().contains("already queued"));
    let import = f
        .store
        .create_meeting(NewMeeting {
            title: "File".into(),
            source: Source::File,
            source_path: Some("C:/rec/a.m4a".into()),
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_791_000_000_000,
            status: MeetingStatus::Done,
        })
        .unwrap();
    let check = rerun_check(&*f.store, &f.root, import.id).unwrap();
    assert!(check.blocked.unwrap().contains("Only live recordings"));
}

// ---- acceptance on a copy of a real data root (GPU) -------------------------------------

/// The files under `dir` named `name` (the models folder's layout is
/// `<org>/<model>/<revision>/<file>`).
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

/// Plan 16 section 12.5 J acceptance: a stored meeting of a COPY of a data
/// root runs again from its `playback.flac` with the engine its current
/// run names (runtime, models, lookahead, diarizer preset, device), and
/// must reproduce that run's words, `engine_info.echo` and `echo_dropped`
/// exactly, apart from wall times and, without stored channel lengths, the
/// end of each channel's last word. Prints counts only (no transcript
/// text). `#[ignore]`d and gated:
///
/// - `DIARIST_RERUN_ROOT`: the copy (`diarist.db` with its `-wal` and
///   `-shm`, and the meeting's `audio/<uid>/`); it is migrated and written.
/// - `DIARIST_RERUN_MEETING`: the meeting's id.
/// - `DIARIST_ENGINE_DIR`, `DIARIST_MODELS_DIR`: the runtime folder and the
///   models folder, only read.
///
/// `cargo test -p diarist-core --release --lib live::rerun::tests::a_stored -- --ignored --nocapture`
#[test]
#[ignore]
fn a_stored_meeting_runs_again_from_its_flac_exactly() {
    use crate::engine::NemoCliEngine;
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
    let root = PathBuf::from(var("DIARIST_RERUN_ROOT"));
    let id = MeetingId(var("DIARIST_RERUN_MEETING").parse().unwrap());
    let engine_dir = PathBuf::from(var("DIARIST_ENGINE_DIR"));
    let models = PathBuf::from(var("DIARIST_MODELS_DIR"));
    let store = Arc::new(SqliteStore::open(root.join("diarist.db")).unwrap());
    let detail = store.get_meeting(id).unwrap();
    let m = detail.meeting;
    let old = detail.current_run.expect("a current run");
    assert_eq!(old.kind, RunKind::PostPass);
    let live = live_run_of(&store.list_runs(id).unwrap())
        .expect("the live run")
        .id;
    let info = &old.engine_info;
    let asr = find_file(&models, info["models"]["asr"].as_str().unwrap()).expect("ASR model");
    let diar = find_file(&models, info["models"]["diarizer"].as_str().unwrap()).expect("diarizer");
    let mut engine = NemoCliEngine::new(&engine_dir, &models, &asr, &diar);
    engine.version = info["runtime"]
        .as_str()
        .and_then(|r| r.strip_prefix("nemo-speech "))
        .map(str::to_owned);
    engine.device = info["device"].as_str().unwrap().to_owned();
    engine.pass_language = false;
    engine.right_context = info["right_context"].as_u64().map(|v| v as u32);
    engine.diar_preset = info["diar_preset"].as_str().map(str::to_owned);
    let folder = root.join("audio").join(&m.uid);
    let flac = folder.join("playback.flac");
    let gain = folder.join("loopback.gain");
    let flac_sha = sha256(&flac);
    let gain_sha = gain.is_file().then(|| sha256(&gain));
    let tracks = store.list_audio_tracks(id).unwrap();
    let old_words = words_of(&*store, old.id);
    let old_dropped = store.echo_dropped(old.id).unwrap();
    let ctx = LiveContext {
        store: store.clone(),
        queue: Arc::new(JobQueue::new()),
        data_root: root.clone(),
        live_engine: Arc::new(NoLive),
        file_engine: Arc::new(engine),
        sources: Arc::new(|_, _| Err(CoreError::Engine("no capture here".into()))),
        vram_free_mb: Arc::new(|| None),
        min_free_vram_mb: 0,
        min_free_bytes: 0,
        live_engine_info: serde_json::json!({}),
        on_job_event: None,
        // The stored runs predate punctuation (beta.4).
        punct: crate::punct::PunctSource::None,
    };
    let t0 = std::time::Instant::now();
    let report = rerun_post_pass(&ctx, id)
        .unwrap()
        .wait(Duration::from_secs(3 * 3_600))
        .expect("the job ended")
        .unwrap();
    let wall = t0.elapsed().as_secs_f64();

    let flac_same = sha256(&flac) == flac_sha;
    let gain_same = gain_sha.is_none_or(|g| sha256(&gain) == g);
    let tracks_same = store.list_audio_tracks(id).unwrap() == tracks;
    let wavs_gone = [
        "mic.wav",
        "loopback.wav",
        "mic.wav.part",
        "loopback.wav.part",
        "mic-aec.wav",
        RERUN_MARKER,
    ]
    .iter()
    .all(|f| !folder.join(f).exists());
    let d = store.get_meeting(id).unwrap();
    let current_new = d.current_run.as_ref().map(|r| r.id) == Some(report.current_run)
        && report.current_run != old.id;
    let old_stays = store
        .get_run(old.id)
        .is_ok_and(|r| r.status == RunStatus::Done);
    let live_stays = store.get_run(live).is_ok();
    let new = store.get_run(report.current_run).unwrap();
    let new_words = words_of(&*store, report.current_run);
    // Words: identical, or differing only in the end of a channel's last word.
    let last =
        |ws: &[(Channel, String, Word)], i: usize| ws.get(i + 1).is_none_or(|n| n.0 != ws[i].0);
    let (mut same, mut end_only, mut other) = (0usize, 0usize, 0usize);
    for (i, (a, b)) in old_words.iter().zip(&new_words).enumerate() {
        if a == b {
            same += 1;
        } else {
            let mut b2 = b.clone();
            b2.2.end_ms = a.2.end_ms;
            if &b2 == a && last(&old_words, i) && last(&new_words, i) {
                end_only += 1;
            } else {
                other += 1;
            }
        }
    }
    let echo_same = echo_without_walls(&new.engine_info) == echo_without_walls(&old.engine_info);
    let echo_diff: Vec<String> = match (
        echo_without_walls(&old.engine_info),
        echo_without_walls(&new.engine_info),
    ) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => a
            .keys()
            .chain(b.keys())
            .filter(|k| a.get(*k) != b.get(*k))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        _ => vec!["(not an object)".into()],
    };
    let new_dropped = store.echo_dropped(report.current_run).unwrap();
    let dropped_same = new_dropped == old_dropped;
    eprintln!(
        "meeting {}: {:.1} min of audio, {wall:.0} s; words {} stored, {} new: {same} identical, {end_only} last-word end only, {other} other; echo identical {echo_same} (differing keys {echo_diff:?}); echo_dropped identical {dropped_same} ({} and {}); playback.flac byte-identical {flac_same}, loopback.gain {gain_same}, track rows {tracks_same}; decoded WAVs gone {wavs_gone}; new run current {current_new}, earlier run stays {old_stays}, live run stays {live_stays}",
        id,
        report.flac_frames as f64 / 16_000.0 / 60.0,
        old_words.len(),
        new_words.len(),
        old_dropped.as_ref().map_or(0, Vec::len),
        new_dropped.as_ref().map_or(0, Vec::len),
    );
    assert!(flac_same && gain_same && tracks_same && wavs_gone);
    assert!(current_new && old_stays && live_stays);
    assert_eq!(old_words.len(), new_words.len());
    assert_eq!(other, 0);
    assert!(echo_same, "{echo_diff:?}");
    assert!(dropped_same);
}
