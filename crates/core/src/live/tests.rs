//! Recorder tests without audio hardware or a GPU: synthetic packet sources
//! paced in real time, a fake live engine that emits a word per second, and
//! a fake file engine for the post-pass.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::*;
use crate::audio::capture::{Control, Packet, PacketData, PacketFlags};
use crate::engine::{EngineInfo, FileJob, JobStats, LiveSession};
use crate::store::{SqliteStore, Store};
use crate::types::{ModelRole, SegmentId};

/// Emits 10 ms packets of a tone (`hz`, every other second) in real time.
struct FakeSource {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    reopened: Arc<AtomicU32>,
}

impl FakeSource {
    fn start(hz: f32, amp: f32, mut ring: rtrb::Producer<Packet>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let reopened = Arc::new(AtomicU32::new(0));
        let s = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let _ = ring.push(Packet::Control {
                qpc_100ns: qpc_now_100ns(),
                control: Control::DeviceChanged {
                    device_id: "fake".into(),
                    name: "Fake device".into(),
                    reason: ChangeReason::Initial,
                },
            });
            let start = qpc_now_100ns();
            let t0 = Instant::now();
            let mut k = 0i64;
            while !s.load(Ordering::SeqCst) {
                let due = Duration::from_millis(10 * k as u64);
                if let Some(d) = due.checked_sub(t0.elapsed()) {
                    std::thread::sleep(d);
                }
                let pcm: Vec<i16> = (0..160)
                    .map(|i| {
                        let n = k * 160 + i;
                        if (n / 16_000) % 2 == 0 {
                            let t = n as f32 / 16_000.0;
                            ((t * hz * std::f32::consts::TAU).sin() * amp).round() as i16
                        } else {
                            0
                        }
                    })
                    .collect();
                let _ = ring.push(Packet::Audio {
                    qpc_100ns: start + k * 100_000,
                    flags: PacketFlags::default(),
                    data: PacketData::Pcm16(pcm),
                });
                k += 1;
            }
        });
        Self {
            stop,
            thread: Some(thread),
            reopened,
        }
    }
}

impl PacketSource for FakeSource {
    fn reopen(&self) {
        self.reopened.fetch_add(1, Ordering::SeqCst);
    }
    fn status(&self) -> CaptureStatus {
        CaptureStatus {
            device_name: Some("Fake device".into()),
            ..Default::default()
        }
    }
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A word per second of audio: a partial, then the final with a speaker
/// (loopback) and TextFinalUntil.
struct FakeSession {
    job: JobId,
    channel: Channel,
    diarize: bool,
    tx: Sender<EngineEvent>,
    rx: Receiver<EngineEvent>,
    samples: u64,
    utt: u32,
}

impl LiveSession for FakeSession {
    fn push(&mut self, pcm: &[i16], pts_us: u64) -> crate::error::Result<()> {
        let before = self.samples / 16_000;
        self.samples += pcm.len() as u64;
        if self.samples / 16_000 > before {
            let sec = pts_us / 1_000_000;
            let word = |rev| EngineEvent::Utterance {
                job_id: self.job.clone(),
                channel: self.channel,
                utt: self.utt,
                rev,
                words: vec![EngineWord {
                    t0_us: sec * 1_000_000,
                    t1_us: sec * 1_000_000 + 900_000,
                    text: format!("w{sec}"),
                    conf: None,
                    spk: (self.diarize && rev == 1).then(|| "spk_0".to_owned()),
                }],
            };
            let _ = self.tx.send(word(0));
            let _ = self.tx.send(word(1));
            let _ = self.tx.send(EngineEvent::TextFinalUntil {
                job_id: self.job.clone(),
                channel: self.channel,
                t_us: sec * 1_000_000 + 900_000,
            });
            self.utt += 1;
        }
        Ok(())
    }
    fn events(&mut self) -> &mut Receiver<EngineEvent> {
        &mut self.rx
    }
    fn close(self: Box<Self>) -> crate::error::Result<()> {
        let _ = self.tx.send(EngineEvent::Done {
            job_id: self.job.clone(),
            stats: JobStats::default(),
        });
        Ok(())
    }
}

struct FakeLive;
impl LiveEngine for FakeLive {
    fn open(
        &self,
        job: JobId,
        ch: Channel,
        opts: LiveOpts,
    ) -> crate::error::Result<Box<dyn LiveSession>> {
        let (tx, rx) = channel();
        Ok(Box::new(FakeSession {
            job,
            channel: ch,
            diarize: opts.diarize,
            tx,
            rx,
            samples: 0,
            utt: 0,
        }))
    }
}

/// The post-pass engine: a word per second over the WAV, `spk_0` when diarizing.
struct FakeFile {
    jobs: Mutex<Vec<(PathBuf, bool)>>,
    /// The next job fails (an engine crash).
    fail_next: AtomicBool,
    /// Jobs return runs of `<unk>` (a language the model cannot transcribe).
    unknown: AtomicBool,
}
impl Engine for FakeFile {
    fn transcribe_file(&self, job: FileJob) -> crate::error::Result<Receiver<EngineEvent>> {
        self.jobs
            .lock()
            .unwrap()
            .push((job.wav_path.clone(), job.diarize));
        let secs = hound::WavReader::open(&job.wav_path).unwrap().duration() / 16_000;
        let (tx, rx) = channel();
        if self.fail_next.swap(false, Ordering::SeqCst) {
            tx.send(EngineEvent::Error {
                job_id: job.job_id,
                code: crate::engine::ErrorCode::Crash,
                message: "fake crash".into(),
            })
            .unwrap();
            return Ok(rx);
        }
        let words = (0..u64::from(secs))
            .map(|s| EngineWord {
                t0_us: s * 1_000_000 + 50_000,
                t1_us: s * 1_000_000 + 900_000,
                text: if self.unknown.load(Ordering::SeqCst) {
                    "<unk><unk><unk>e<unk>".to_owned()
                } else {
                    format!("p{s}")
                },
                conf: None,
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
    fn cancel(&self, _: JobId) {}
    fn info(&self) -> EngineInfo {
        EngineInfo {
            runtime: "fake 1".into(),
            right_context: Some(13),
            diar_preset: Some("v3-offline".into()),
            ..Default::default()
        }
    }
}

/// Records what the sink saw.
#[derive(Default)]
struct Sink {
    words: Mutex<Vec<WordsUpdate>>,
    finals: Mutex<Vec<(Channel, i64)>>,
    levels: Mutex<Vec<LevelsUpdate>>,
    devices: Mutex<Vec<DeviceChange>>,
}
impl LiveSink for Sink {
    fn words(&self, u: &WordsUpdate) {
        self.words.lock().unwrap().push(u.clone());
    }
    fn text_final(&self, c: Channel, t: i64) {
        self.finals.lock().unwrap().push((c, t));
    }
    fn levels(&self, u: &LevelsUpdate) {
        self.levels.lock().unwrap().push(*u);
    }
    fn device_changed(&self, d: &DeviceChange) {
        self.devices.lock().unwrap().push(d.clone());
    }
}

fn context(dir: &Path, vram: Option<u32>) -> (LiveContext, Arc<SqliteStore>, Arc<FakeFile>) {
    let store = Arc::new(SqliteStore::open(dir.join("diarist.db")).unwrap());
    let file = Arc::new(FakeFile {
        jobs: Mutex::new(Vec::new()),
        fail_next: AtomicBool::new(false),
        unknown: AtomicBool::new(false),
    });
    let sources: Arc<SourceFactory> = Arc::new(|cfg: CaptureConfig, ring| {
        let hz = if cfg.channel == Channel::Mic {
            300.0
        } else {
            500.0
        };
        Ok(Box::new(FakeSource::start(hz, 8_000.0, ring)) as Box<dyn PacketSource>)
    });
    let ctx = LiveContext {
        store: store.clone(),
        queue: Arc::new(JobQueue::new()),
        data_root: dir.to_path_buf(),
        live_engine: Arc::new(FakeLive),
        file_engine: file.clone(),
        sources,
        vram_free_mb: Arc::new(move || vram),
        min_free_vram_mb: MIN_FREE_VRAM_MB,
        min_free_bytes: 0,
        live_engine_info: serde_json::json!({"runtime": "fake live"}),
        on_job_event: None,
    };
    (ctx, store, file)
}

#[test]
fn records_streams_and_runs_the_post_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), Some(8_000));
    let sink = Arc::new(Sink::default());
    let rec = Recorder::start(
        ctx.clone(),
        StartOptions {
            title: Some("Standup".into()),
            ..Default::default()
        },
        sink.clone(),
    )
    .unwrap();
    let m = rec.meeting_id();
    assert!(ctx.queue.queued().is_empty());
    // While recording: the live run is current and has speaker rows. Long
    // enough for the 10 s of speech-region time a name needs to carry over
    // without an anchor (`store::carry_over`, steps 1-2; it was 7.3 s while
    // the minimum was 5 s of word time).
    std::thread::sleep(Duration::from_millis(11_300));
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Recording);
    assert_eq!(d.current_run.as_ref().unwrap().kind, RunKind::Live);
    let alice = store
        .live_speaker(rec.run_id(), Channel::Loopback, "spk_0")
        .unwrap();
    store.rename_speaker(alice.id, "Alice", None).unwrap();
    // A view that opens now gets the newest revision of every utterance.
    let snap = snapshot(&*store, m).unwrap();
    assert!(snap.iter().filter(|u| u.channel == Channel::Mic).count() >= 5);
    assert!(snap.iter().all(|u| u.run_id == rec.run_id()));
    let lb_snap: Vec<&WordsUpdate> = snap
        .iter()
        .filter(|u| u.channel == Channel::Loopback)
        .collect();
    assert!(lb_snap
        .iter()
        .all(|u| u.words[0].label == "spk_0" && u.words[0].speaker_id == Some(alice.id)));
    assert!(snap.iter().filter(|u| u.channel == Channel::Mic).all(|u| u
        .words
        .iter()
        .all(|w| w.label == "me" && w.speaker_id.is_some())));
    let st = rec.status();
    assert!(
        st.revisions["mic"] >= 10 && st.revisions["loopback"] >= 10,
        "{st:?}"
    );
    // An import queued while recording waits, and runs after the post-pass.
    assert!(ctx.queue.is_paused());
    let (import_tx, import_rx) = channel();
    let s2 = store.clone();
    ctx.queue.submit(JobId::new(), move |_| {
        let status = s2.get_meeting(m).unwrap().meeting.status;
        import_tx.send(status).unwrap();
    });
    let out = rec.stop().unwrap();
    assert_eq!(out.mic_samples, out.loopback_samples);
    assert!(out.mic_samples >= 7 * 16_000, "{}", out.mic_samples);
    assert!(!out.issues.any(), "{:?}", out.issues);
    assert!(out.post_pass.transcribe);
    let report = out
        .post_pass
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert!(!ctx.queue.is_paused());
    assert_eq!(
        import_rx.recv_timeout(Duration::from_secs(30)).unwrap(),
        MeetingStatus::Done,
        "the import ran before the post-pass"
    );

    // Sink: words with replacement semantics, finals, 10 Hz levels, devices.
    let words = sink.words.lock().unwrap();
    let lb: Vec<&WordsUpdate> = words
        .iter()
        .filter(|w| w.channel == Channel::Loopback)
        .collect();
    assert_eq!((lb[0].utt, lb[0].rev), (0, 0));
    assert_eq!(lb[1].words[0].label, "spk_0");
    assert!(lb[1].words[0].speaker_id.is_some());
    let mic: Vec<&WordsUpdate> = words.iter().filter(|w| w.channel == Channel::Mic).collect();
    assert!(mic.iter().all(|u| u.words.iter().all(|w| w.label == "me")));
    // Live times are meeting time: word `w3` sits at 3 s.
    let w3 = lb.iter().find(|u| u.words[0].text == "w3").unwrap();
    assert!(
        (w3.words[0].start_ms - 3_000).abs() < 50,
        "{}",
        w3.words[0].start_ms
    );
    assert!(sink.finals.lock().unwrap().len() >= 12);
    let levels = sink.levels.lock().unwrap();
    let mic_levels = levels.iter().filter(|l| l.channel == Channel::Mic).count();
    assert!((105..=120).contains(&mic_levels), "{mic_levels} mic levels");
    assert!(levels.iter().any(|l| l.peak_dbfs > -20.0));
    // A loud loopback is not amplified (AGC on by default); the mic has none.
    assert!(
        levels.iter().all(|l| l.gain_db == 0.0),
        "loud input got gain"
    );
    // Without gain, raw and recorded peaks agree (over the recording; per
    // frame they are attributed at packet boundaries).
    let lb_max = |f: fn(&LevelsUpdate) -> f32| {
        levels
            .iter()
            .filter(|l| l.channel == Channel::Loopback)
            .map(f)
            .fold(-200.0f32, f32::max)
    };
    assert!((lb_max(|l| l.raw_peak_dbfs) - lb_max(|l| l.peak_dbfs)).abs() < 0.1);
    assert_eq!(sink.devices.lock().unwrap().len(), 2);

    // Post-pass: the new run is current, names carried over, FLAC in, WAVs out.
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    let run = d.current_run.unwrap();
    assert_eq!(run.kind, RunKind::PostPass);
    assert_eq!(run.engine_info["right_context"], 13, "the lookahead passed");
    assert_eq!(
        run.engine_info["diar_preset"], "v3-offline",
        "the preset passed"
    );
    assert_eq!(run.id, report.current_run);
    assert!(report.live_run_deleted);
    assert!(store.get_run(out.live_run).is_err());
    assert!(report
        .speakers
        .contains(&(Channel::Loopback, "spk_0".into(), Some("Alice".into()))));
    assert!(report
        .speakers
        .iter()
        .any(|(c, l, _)| *c == Channel::Mic && l == "me"));
    let jobs = file.jobs.lock().unwrap().clone();
    assert_eq!(jobs.len(), 2);
    assert!(jobs
        .iter()
        .any(|(p, diar)| p.ends_with("loopback.wav") && *diar));
    assert!(jobs
        .iter()
        .any(|(p, diar)| p.ends_with("mic.wav") && !*diar));
    let folder = dir.path().join("audio").join(&d.meeting.uid);
    assert!(!folder.join("mic.wav").exists() && !folder.join("loopback.wav").exists());
    assert_eq!(report.flac_frames, out.mic_samples);
    let (pcm, ch) = crate::audio::flac::decode_i16(&folder.join("playback.flac")).unwrap();
    assert_eq!((ch, pcm.len() as u64), (2, 2 * out.mic_samples));
    let tracks = store.list_audio_tracks(m).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].format, AudioFormat::Flac);
    assert_eq!(tracks[0].channels, 2);
    // Post-pass word times are meeting time too.
    let w = store.get_words(m, 2_000, 2_999).unwrap();
    assert!(w
        .iter()
        .any(|w| w.text == "p2" && (w.start_ms - 2_050).abs() < 50));
    // Anchors were stored for both channels from sample 0.
    for c in [Channel::Mic, Channel::Loopback] {
        let a = store.clock_anchors(m, c).unwrap();
        assert_eq!(a[0].sample_index, 0);
        assert!(a.len() >= 8, "{c}: {} anchors", a.len());
    }
    let _ = SegmentId(0);
}

#[test]
fn quiet_loopback_is_raised_and_warned_about() {
    // The loopback arrives at -70 dBFS (a low Windows volume).
    let dir = tempfile::tempdir().unwrap();
    let (mut ctx, store, _) = context(dir.path(), None);
    ctx.sources = Arc::new(|cfg: CaptureConfig, ring| {
        assert_eq!(
            cfg.float,
            cfg.channel == Channel::Loopback,
            "float only with AGC"
        );
        let amp = if cfg.channel == Channel::Mic {
            8_000.0
        } else {
            10.4
        };
        Ok(Box::new(FakeSource::start(440.0, amp, ring)) as Box<dyn PacketSource>)
    });
    let sink = Arc::new(Sink::default());
    let rec = Recorder::start(ctx, StartOptions::default(), sink.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(3_200));
    rec.stop().unwrap();
    let levels = sink.levels.lock().unwrap();
    let lb: Vec<&LevelsUpdate> = levels
        .iter()
        .filter(|l| l.channel == Channel::Loopback && l.raw_peak_dbfs > -85.0)
        .collect();
    let last = lb.last().unwrap();
    assert!((last.gain_db - 40.0).abs() < 0.1, "{}", last.gain_db);
    assert!(
        (last.raw_peak_dbfs - -70.0).abs() < 1.5,
        "{}",
        last.raw_peak_dbfs
    );
    assert!((last.peak_dbfs - -30.0).abs() < 1.5, "{}", last.peak_dbfs);
    // Gain reached within the first active second.
    assert!(lb.iter().take(10).any(|l| l.gain_db > 39.0));
    // Mic AGC is off by default.
    assert!(levels
        .iter()
        .filter(|l| l.channel == Channel::Mic)
        .all(|l| l.gain_db == 0.0));
    let _ = store;
}

#[test]
fn quiet_detector_needs_ten_quiet_seconds_with_audio() {
    let mut q = QuietDetector::default();
    // Silence for a minute: nothing.
    for t in 0..600 {
        assert_eq!(q.feed(t * 100, -100.0), None);
    }
    // 10 s of -70 dBFS audio with pauses: fires once at 10 s.
    let mut fired = Vec::new();
    for t in 0..150 {
        let raw = if t % 4 == 3 {
            -100.0
        } else {
            -70.0 + (t % 3) as f32
        };
        if let Some(p) = q.feed(60_000 + t * 100, raw) {
            fired.push((t, p));
        }
    }
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert!(fired[0].0 >= 100 && fired[0].0 <= 103, "{fired:?}");
    assert!((fired[0].1 - -68.0).abs() < 0.01);
    // A normal peak re-arms; quiet again for 10 s fires again.
    assert_eq!(q.feed(80_000, -12.0), None);
    let again = (1..=101)
        .filter_map(|t| q.feed(80_000 + t * 100, -60.0))
        .count();
    assert_eq!(again, 1);
    // Mixed audio that reaches -50 dBFS now and then never fires.
    let mut q = QuietDetector::default();
    let n = (0..300)
        .filter_map(|t| q.feed(t * 100, if t % 50 == 0 { -40.0 } else { -70.0 }))
        .count();
    assert_eq!(n, 0);
    assert!(agc_setting(
        &crate::views::Settings::default(),
        Channel::Loopback
    ));
    assert!(!agc_setting(
        &crate::views::Settings::default(),
        Channel::Mic
    ));
    let mut s = crate::views::Settings::default();
    s.0.insert(CAPTURE_LOOPBACK_AGC.into(), "false".into());
    s.0.insert(CAPTURE_MIC_AGC.into(), "1".into());
    assert!(!agc_setting(&s, Channel::Loopback) && agc_setting(&s, Channel::Mic));
}

#[test]
fn on_issues_promotes_a_clean_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    store
        .set_setting(settings::POST_PASS_MODE, "on_issues")
        .unwrap();
    // The app keeps the queue alive; a dropped queue drops its queued jobs.
    let rec = Recorder::start(
        ctx.clone(),
        StartOptions::default(),
        Arc::new(Sink::default()),
    )
    .unwrap();
    let m = rec.meeting_id();
    std::thread::sleep(Duration::from_millis(2_500));
    let out = rec.stop().unwrap();
    assert!(!out.post_pass.transcribe);
    let report = out
        .post_pass
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert!(report.promoted_live);
    assert_eq!(report.current_run, out.live_run);
    assert!(file.jobs.lock().unwrap().is_empty());
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().kind, RunKind::Live);
    assert!(d.meeting.title.starts_with("Recording "));
    assert!(!store.get_segments(m, 0, 100).unwrap().is_empty());
}

#[test]
fn a_failed_post_pass_runs_again() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    let rec = Recorder::start(
        ctx.clone(),
        StartOptions::default(),
        Arc::new(Sink::default()),
    )
    .unwrap();
    let m = rec.meeting_id();
    std::thread::sleep(Duration::from_millis(1_500));
    let refused = rerun_post_pass(&ctx, m).err().unwrap();
    assert!(refused.to_string().contains("still recording"), "{refused}");
    file.fail_next.store(true, Ordering::SeqCst);
    let out = rec.stop().unwrap();
    let err = out.post_pass.wait(Duration::from_secs(30)).unwrap();
    assert!(err.unwrap_err().contains("fake crash"));
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Failed);
    assert_eq!(d.current_run.unwrap().kind, RunKind::Live);

    let again = rerun_post_pass(&ctx, m).unwrap();
    assert!(again.transcribe);
    assert_eq!(
        store.get_meeting(m).unwrap().meeting.status,
        MeetingStatus::Processing
    );
    let report = again.wait(Duration::from_secs(30)).unwrap().unwrap();
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, report.current_run);
    assert!(!report.promoted_live && report.live_run_deleted);
    // The WAVs are gone now: nothing to run again.
    let gone = rerun_post_pass(&ctx, m).err().unwrap();
    assert!(gone.to_string().contains("no live transcript"), "{gone}");
}

#[test]
fn unknown_tokens_fail_the_post_pass_and_keep_the_wavs() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    let rec = Recorder::start(
        ctx.clone(),
        StartOptions::default(),
        Arc::new(Sink::default()),
    )
    .unwrap();
    let m = rec.meeting_id();
    std::thread::sleep(Duration::from_millis(1_500));
    file.unknown.store(true, Ordering::SeqCst);
    let out = rec.stop().unwrap();
    let err = out.post_pass.wait(Duration::from_secs(30)).unwrap();
    assert_eq!(
        err.unwrap_err(),
        crate::models::languages::UNKNOWN_TOKENS_NOTICE
    );
    // Nothing stored: the live run stays current, the WAVs stay.
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Failed);
    assert_eq!(d.current_run.unwrap().kind, RunKind::Live);
    assert!(store
        .list_runs(m)
        .unwrap()
        .iter()
        .all(|r| r.kind == RunKind::Live));
    // Another model (here: the fake again, now transcribing) can run it.
    file.unknown.store(false, Ordering::SeqCst);
    let report = rerun_post_pass(&ctx, m)
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(
        store.get_meeting(m).unwrap().current_run.unwrap().id,
        report.current_run
    );
}

#[test]
fn refuses_without_vram() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, _) = context(dir.path(), Some(1_000));
    let err = Recorder::start(
        ctx.clone(),
        StartOptions::default(),
        Arc::new(Sink::default()),
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("1000 MB"), "{err}");
    assert!(store.list_meetings().unwrap().is_empty());
    // A failing capture also leaves nothing behind and unpauses the queue.
    let mut ctx2 = ctx;
    ctx2.vram_free_mb = Arc::new(|| Some(9_000));
    ctx2.sources = Arc::new(|_, _| Err(CoreError::Engine("no such device".into())));
    assert!(Recorder::start(
        ctx2.clone(),
        StartOptions::default(),
        Arc::new(Sink::default())
    )
    .is_err());
    assert!(store.list_meetings().unwrap().is_empty());
    let ran = Arc::new(AtomicBool::new(false));
    let r2 = Arc::clone(&ran);
    ctx2.queue
        .submit(JobId::new(), move |_| r2.store(true, Ordering::SeqCst));
    ctx2.queue.wait_idle();
    assert!(ran.load(Ordering::SeqCst));
    let _ = ModelRole::Asr;
}

#[test]
fn recovers_a_crashed_recording() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    // A recording that died: meeting in `recording`, live utterances,
    // anchors, WAVs whose headers were never finalized.
    let meeting = store
        .create_meeting(NewMeeting {
            title: "Crashed".into(),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_791_000_000_000,
            status: MeetingStatus::Recording,
        })
        .unwrap();
    let run = store
        .create_run(meeting.id, RunKind::Live, serde_json::json!({}))
        .unwrap();
    store
        .append_live_utterance(
            run.id,
            Channel::Loopback,
            0,
            0,
            &[EngineWord {
                t0_us: 500_000,
                t1_us: 900_000,
                text: "hello".into(),
                conf: None,
                spk: Some("spk_0".into()),
            }],
        )
        .unwrap();
    let folder = dir.path().join("audio").join(&meeting.uid);
    for (ch, n) in [("mic", 48_000), ("loopback", 47_000)] {
        let mut w = crate::audio::wav::WavSink::create(&folder.join(format!("{ch}.wav"))).unwrap();
        w.write(&vec![100i16; n]).unwrap();
        std::mem::forget(w);
    }
    let recovered = recover(&ctx).unwrap();
    assert_eq!(recovered.len(), 1);
    let r = &recovered[0];
    assert_eq!(r.live_run, Some(run.id));
    assert!(r.mic_frames >= 40_000 && r.loopback_frames >= 40_000);
    let report = r
        .post_pass
        .as_ref()
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(file.jobs.lock().unwrap().len(), 2);
    let d = store.get_meeting(meeting.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, report.current_run);
    assert!(folder.join("playback.flac").is_file());
    // Nothing left to recover.
    assert!(recover(&ctx).unwrap().is_empty());
}

/// A live meeting whose post-pass was interrupted: status `processing`, a
/// materialised live run (current) and, optionally, both WAVs (3 s).
fn interrupted_meeting(
    store: &SqliteStore,
    dir: &Path,
    title: &str,
    wavs: bool,
) -> (Meeting, RunId) {
    let meeting = store
        .create_meeting(NewMeeting {
            title: title.into(),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_791_000_000_000,
            status: MeetingStatus::Processing,
        })
        .unwrap();
    store
        .finish_meeting(meeting.id, meeting.started_at + 3_000, 3_000)
        .unwrap();
    let run = store
        .create_run(
            meeting.id,
            RunKind::Live,
            serde_json::json!({"runtime": "fake live"}),
        )
        .unwrap();
    store
        .append_live_utterance(
            run.id,
            Channel::Loopback,
            0,
            0,
            &[EngineWord {
                t0_us: 500_000,
                t1_us: 900_000,
                text: "hello".into(),
                conf: None,
                spk: Some("spk_0".into()),
            }],
        )
        .unwrap();
    store
        .materialise_live_run(run.id, &SegmentOptions::default())
        .unwrap();
    store.finish_run(run.id, RunStatus::Done).unwrap();
    store.set_current_run(meeting.id, run.id).unwrap();
    if wavs {
        let folder = dir.join("audio").join(&meeting.uid);
        for ch in ["mic", "loopback"] {
            let mut w =
                crate::audio::wav::WavSink::create(&folder.join(format!("{ch}.wav"))).unwrap();
            w.write(&vec![100i16; 48_000]).unwrap();
            w.finalize().unwrap();
        }
    }
    (meeting, run.id)
}

#[test]
fn recovers_interrupted_post_passes() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    // A: the WAVs are there and the post-pass left a half-written run.
    let (a, a_live) = interrupted_meeting(&store, dir.path(), "A", true);
    let stale = store
        .create_run(a.id, RunKind::PostPass, serde_json::json!({}))
        .unwrap()
        .id;
    // B: the WAVs are gone, the live run is all there is (plus a stale run).
    let (b, b_live) = interrupted_meeting(&store, dir.path(), "B", false);
    let b_stale = store
        .create_run(b.id, RunKind::PostPass, serde_json::json!({}))
        .unwrap()
        .id;
    // C: the post-pass run was written and made current, then the WAVs went.
    let (c, c_live) = interrupted_meeting(&store, dir.path(), "C", false);
    let c_post = store
        .create_run(c.id, RunKind::PostPass, serde_json::json!({}))
        .unwrap()
        .id;
    store.finish_run(c_post, RunStatus::Done).unwrap();
    store.set_current_run(c.id, c_post).unwrap();

    let recovered = recover(&ctx).unwrap();
    assert_eq!(recovered.len(), 3);
    let of = |id: MeetingId| recovered.iter().find(|r| r.meeting_id == id).unwrap();
    assert!(recovered.iter().all(|r| r.was == MeetingStatus::Processing));

    // A: queued again; the stale run is gone, the new post-pass run is current.
    let ra = of(a.id);
    assert_eq!(ra.action, RecoveryAction::PostPassQueued);
    assert_eq!((ra.mic_frames, ra.loopback_frames), (48_000, 48_000));
    let report = ra
        .post_pass
        .as_ref()
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(file.jobs.lock().unwrap().len(), 2);
    assert!(store.get_run(stale).is_err());
    assert!(store.get_run(a_live).is_err(), "live run replaced");
    let d = store.get_meeting(a.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, report.current_run);
    let tracks = store.list_audio_tracks(a.id).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].channel, TrackChannel::Playback);

    // B: the live run is promoted, with a note.
    let rb = of(b.id);
    assert_eq!(rb.action, RecoveryAction::PromotedLive);
    assert!(rb.post_pass.is_none());
    let d = store.get_meeting(b.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    let cur = d.current_run.unwrap();
    assert_eq!(cur.id, b_live);
    assert_eq!(cur.engine_info["recovery"], PROMOTED_NOTE);
    assert_eq!(cur.engine_info["runtime"], "fake live");
    assert!(store.get_run(b_stale).is_err());
    assert_eq!(store.get_segments(b.id, 0, 10).unwrap()[0].text, "hello");

    // C: finished as it was; the live run goes (keep_live_runs is off).
    let rc = of(c.id);
    assert_eq!(rc.action, RecoveryAction::Finished);
    let d = store.get_meeting(c.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, c_post);
    assert!(store.get_run(c_live).is_err());

    // Nothing left to recover.
    assert!(recover(&ctx).unwrap().is_empty());
}
