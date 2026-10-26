//! Recorder tests without audio hardware or a GPU: synthetic packet sources
//! paced in real time, a fake live engine that emits a word per second, and
//! a fake file engine for the post-pass.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::*;
use crate::audio::capture::{Control, Packet, PacketData, PacketFlags};
use crate::audio::echo::EchoStatus;
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
                // Each channel says its own words (the text layer would
                // take the same words at the same time for echo).
                text: if self.unknown.load(Ordering::SeqCst) {
                    "<unk><unk><unk>e<unk>".to_owned()
                } else if job.diarize {
                    format!("q{s}")
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
        live_right_context: None,
        on_job_event: None,
        punct: crate::punct::PunctSource::None,
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
    // No live lookahead flag by default (docs/plans/16 section 5).
    assert_eq!(
        d.current_run.as_ref().unwrap().engine_info["live_right_context"],
        "runtime default"
    );
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
    // Every live name was placed: the live run goes.
    assert_eq!(report.unplaced_names, 0);
    assert_eq!(run.engine_info["unplaced_names"], 0);
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
    // The loopback AGC's gain track stays beside the FLAC, a 10 ms value
    // for every frame (the loud tone: 0 dB); the mic has no AGC and no track.
    let gains = crate::audio::gain_track::GainTrack::read(&folder.join("loopback.gain"))
        .unwrap()
        .expect("loopback.gain");
    assert!(
        gains.frames() as u64 * 1_600 + 3_200 >= out.loopback_samples,
        "{} frames for {} samples",
        gains.frames(),
        out.loopback_samples
    );
    assert!(gains.values().iter().all(|&g| g == 0.0));
    assert!(!folder.join("mic.gain").exists());
    assert!(!folder.join("mic-aec.wav").exists());
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
fn a_live_run_with_an_unplaced_name_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ctx, store, _) = context(dir.path(), None);
    // A server launched with a lookahead: the live run says which.
    ctx.live_right_context = Some(6);
    let rec = Recorder::start(
        ctx.clone(),
        StartOptions::default(),
        Arc::new(Sink::default()),
    )
    .unwrap();
    let m = rec.meeting_id();
    std::thread::sleep(Duration::from_millis(1_500));
    // A label that never says anything in the post-pass's time (here: no
    // words at all) is named: nobody can receive the name.
    let ghost = store
        .live_speaker(rec.run_id(), Channel::Loopback, "spk_7")
        .unwrap();
    store.rename_speaker(ghost.id, "Ghost", None).unwrap();
    let out = rec.stop().unwrap();
    let report = out
        .post_pass
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(report.unplaced_names, 1);
    assert!(
        !report.live_run_deleted,
        "keep_live_runs is off, the name keeps it"
    );
    let live = store.get_run(out.live_run).unwrap();
    assert_eq!(live.kind, RunKind::Live);
    assert_eq!(live.engine_info["live_right_context"], 6);
    let d = store.get_meeting(m).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    let current = d.current_run.unwrap();
    assert_eq!(current.id, report.current_run);
    assert_eq!(current.engine_info["unplaced_names"], 1);
    assert!(store
        .run_speakers(out.live_run)
        .unwrap()
        .iter()
        .any(|s| s.display_name.as_deref() == Some("Ghost")));
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
    // D: as C, but the post-pass recorded a live name it could not place.
    let (d_meeting, d_live) = interrupted_meeting(&store, dir.path(), "D", false);
    let d_post = store
        .create_run(
            d_meeting.id,
            RunKind::PostPass,
            serde_json::json!({"unplaced_names": 1}),
        )
        .unwrap()
        .id;
    store.finish_run(d_post, RunStatus::Done).unwrap();
    store.set_current_run(d_meeting.id, d_post).unwrap();

    let recovered = recover(&ctx).unwrap();
    assert_eq!(recovered.len(), 4);
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

    // D: finished, and the live run with the unplaced name stays.
    assert_eq!(of(d_meeting.id).action, RecoveryAction::Finished);
    let d = store.get_meeting(d_meeting.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, d_post);
    assert!(store.get_run(d_live).is_ok());

    // Nothing left to recover.
    assert!(recover(&ctx).unwrap().is_empty());
}

// ---- echo (plan 15) ------------------------------------------------------------------

/// Plays `signal(n)` (`n`: absolute sample index since `epoch`, so two
/// sources line up on QPC time) in 10 ms packets at `speed` times real time.
struct SignalSource {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

type Signal = Arc<dyn Fn(i64) -> i16 + Send + Sync>;

impl SignalSource {
    fn start(epoch: i64, speed: f64, signal: Signal, mut ring: rtrb::Producer<Packet>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
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
                let due = Duration::from_secs_f64(0.01 * k as f64 / speed);
                if let Some(d) = due.checked_sub(t0.elapsed()) {
                    std::thread::sleep(d);
                }
                let q = start + k * 100_000;
                let n0 = (q - epoch) / 625;
                let pcm: Vec<i16> = (0..160).map(|i| signal(n0 + i)).collect();
                let _ = ring.push(Packet::Audio {
                    qpc_100ns: q,
                    flags: PacketFlags::default(),
                    data: PacketData::Pcm16(pcm),
                });
                k += 1;
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl PacketSource for SignalSource {
    fn reopen(&self) {}
    fn status(&self) -> CaptureStatus {
        CaptureStatus::default()
    }
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Records what the sink saw, warnings included.
#[derive(Default)]
struct WarnSink {
    words: Mutex<Vec<WordsUpdate>>,
    warnings: Mutex<Vec<LiveWarning>>,
}
impl LiveSink for WarnSink {
    fn words(&self, u: &WordsUpdate) {
        self.words.lock().unwrap().push(u.clone());
    }
    fn warning(&self, w: &LiveWarning) {
        self.warnings.lock().unwrap().push(w.clone());
    }
}

/// The far end on the loopback and, 3 ms later and 10 dB down, on the mic:
/// a meeting on speakers, played four times faster than real time.
fn echo_context(dir: &Path) -> (LiveContext, Arc<SqliteStore>) {
    use crate::audio::echo::synth::{db, monologue};
    let (mut ctx, store, _) = context(dir, None);
    let (far, _) = monologue(41, 30 * 16_000, db(-28.0));
    let far = Arc::new(far);
    let epoch = qpc_now_100ns();
    ctx.sources = Arc::new(move |cfg: CaptureConfig, ring| {
        let far = Arc::clone(&far);
        let len = far.len() as i64;
        let signal: Signal = if cfg.channel == Channel::Mic {
            Arc::new(move |n| {
                let e = far[(n - 48).rem_euclid(len) as usize] * db(-10.0);
                (e * 32_768.0).round() as i16 + ((n * 7_919) % 13 - 6) as i16
            })
        } else {
            Arc::new(move |n| (far[n.rem_euclid(len) as usize] * 32_768.0).round() as i16)
        };
        Ok(Box::new(SignalSource::start(epoch, 4.0, signal, ring)) as Box<dyn PacketSource>)
    });
    (ctx, store)
}

/// Words of the latest revisions of `channel`.
fn latest_words(rows: &[crate::types::LiveUtterance], channel: Channel) -> usize {
    let mut best: BTreeMap<u32, &crate::types::LiveUtterance> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.channel == channel) {
        if best.get(&r.utt).is_none_or(|b| b.rev < r.rev) {
            best.insert(r.utt, r);
        }
    }
    best.values().map(|r| r.words.len()).sum()
}

#[test]
fn speakers_warn_once_and_echo_words_leave_the_live_run_and_the_post_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store) = echo_context(dir.path());
    // The live run stays for the checks below.
    store.set_setting(settings::KEEP_LIVE_RUNS, "true").unwrap();
    let sink = Arc::new(WarnSink::default());
    // The context (its queue) outlives the recorder, or the post-pass job
    // is dropped with it.
    let rec = Recorder::start(ctx.clone(), StartOptions::default(), sink.clone()).unwrap();
    let m = rec.meeting_id();
    // About 30 s of audio: coupling is decided on the tenth active 2 s block.
    std::thread::sleep(Duration::from_millis(7_500));
    assert_eq!(rec.echo_state().unwrap().status, EchoStatus::Coupled);
    let out = rec.stop().unwrap();
    let echo = out.echo.unwrap();
    assert_eq!(echo.status, EchoStatus::Coupled);
    assert!((echo.lag_ms.unwrap() - 3.0).abs() < 0.5, "{echo:?}");
    assert!((9.0..13.0).contains(&echo.erl_db.unwrap()), "{echo:?}");
    let warnings = sink.warnings.lock().unwrap().clone();
    let echoes: Vec<&LiveWarning> = warnings
        .iter()
        .filter(|w| matches!(w, LiveWarning::Echo { .. }))
        .collect();
    assert_eq!(echoes.len(), 1, "{warnings:?}");
    assert!(
        matches!(echoes[0], LiveWarning::Echo { aec: false, coupling, .. } if *coupling > 0.5),
        "{echoes:?}"
    );
    // Every mic word is stored; the live run leaves the echo out, and the
    // sink was told to hide what it had shown.
    let rows = store.list_live_utterances(out.live_run).unwrap();
    let stored_mic = latest_words(&rows, Channel::Mic);
    assert!(stored_mic >= 20, "{stored_mic}");
    assert!(
        out.echo_dropped + 2 >= stored_mic,
        "{} of {stored_mic}",
        out.echo_dropped
    );
    let live = store.get_run(out.live_run).unwrap();
    assert_eq!(live.engine_info["echo"]["coupled"], true);
    assert_eq!(live.engine_info["echo"]["aec"], false);
    assert_eq!(
        live.engine_info["echo"]["dropped"].as_u64(),
        Some(out.echo_dropped as u64)
    );
    let listed = store.echo_dropped(out.live_run).unwrap().unwrap();
    assert_eq!(listed.len(), out.echo_dropped);
    // The mic says only the far end's words, at its times: echo by text
    // (scored or not), or by correlation.
    assert!(listed.iter().all(|w| w.text.starts_with('w')
        && (w.coefficient >= 0.2 || w.reason == Some(crate::types::EchoReason::Text))));
    let hidden = sink
        .words
        .lock()
        .unwrap()
        .iter()
        .filter(|u| u.channel == Channel::Mic && u.words.is_empty())
        .count();
    assert!(hidden > 0, "no mic update was gated");
    // The post-pass rescans and gates its own mic words.
    let report = out
        .post_pass
        .wait(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (e["coupled"].as_bool(), e["mode"].as_str()),
        (Some(true), Some("auto"))
    );
    let before = e["mic_words_before"].as_u64().unwrap();
    let dropped = e["dropped"].as_u64().unwrap();
    assert!(before >= 20 && dropped + 2 >= before, "{e}");
    assert!(e.get("dropped_words").is_none());
    let mic_left = store
        .get_words(m, i64::MIN, i64::MAX)
        .unwrap()
        .iter()
        .filter(|w| w.text.starts_with('p') && w.raw_label.is_none())
        .count() as u64;
    assert_eq!(mic_left, before - dropped);
    assert_eq!(
        store
            .echo_dropped(report.current_run)
            .unwrap()
            .unwrap()
            .len() as u64,
        dropped
    );
    drop(ctx);
}

#[test]
fn a_reopened_view_sees_the_gate_and_turning_echo_off_ungates_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store) = echo_context(dir.path());
    store.set_setting(settings::KEEP_LIVE_RUNS, "true").unwrap();
    let sink = Arc::new(WarnSink::default());
    let rec = Recorder::start(ctx.clone(), StartOptions::default(), sink.clone()).unwrap();
    let m = rec.meeting_id();
    std::thread::sleep(Duration::from_millis(7_500));
    assert_eq!(rec.echo_state().unwrap().status, EchoStatus::Coupled);
    assert!(!rec.echo_off());
    // A view that opens now gets what the sink shows: the hidden words of
    // the latest revisions are left out of the snapshot.
    let hidden = rec.echo_hidden();
    assert!(!hidden.is_empty());
    assert!(hidden.iter().all(|k| k.0 == Channel::Mic));
    let mic_words = |snap: &[WordsUpdate]| -> usize {
        snap.iter()
            .filter(|u| u.channel == Channel::Mic)
            .map(|u| u.words.len())
            .sum()
    };
    // Both snapshots of one moment: the recording goes on, and an utterance
    // that lands between the two reads would count in one only.
    let (all, gated) = loop {
        let all = snapshot(&*store, m).unwrap();
        let gated =
            snapshot_except(&*store, m, &|c, u, r, i| hidden.contains(&(c, u, r, i))).unwrap();
        if snapshot(&*store, m).unwrap().len() == all.len() {
            break (all, gated);
        }
    };
    assert_eq!(gated.len(), all.len(), "hidden utterances stay, empty");
    assert!(
        mic_words(&gated) + hidden.len() <= mic_words(&all) + 2,
        "{} + {} against {}",
        mic_words(&gated),
        hidden.len(),
        mic_words(&all)
    );
    assert!(mic_words(&gated) < mic_words(&all));
    // Turn off for this recording: recorded on the live run at once, the
    // hidden words go back to the sink, nothing is hidden any more.
    let shown_before = sink.words.lock().unwrap().len();
    assert!(rec.turn_off_echo().unwrap());
    assert!(!rec.turn_off_echo().unwrap(), "already off");
    assert!(rec.echo_off());
    let info = store.get_run(rec.run_id()).unwrap().engine_info;
    assert_eq!(info["echo"]["user_off"], true);
    assert_eq!(info["echo"]["mode"], ECHO_OFF_FOR_RECORDING);
    std::thread::sleep(Duration::from_millis(1_000));
    assert!(rec.echo_hidden().is_empty());
    let resent: Vec<WordsUpdate> = sink.words.lock().unwrap()[shown_before..]
        .iter()
        .filter(|u| u.channel == Channel::Mic && !u.words.is_empty())
        .cloned()
        .collect();
    let first_hidden = hidden.iter().map(|k| k.1).min().unwrap();
    assert!(
        resent.iter().any(|u| u.utt == first_hidden),
        "utterance {first_hidden} was not sent again"
    );
    let now_hidden = rec.echo_hidden();
    let snap =
        snapshot_except(&*store, m, &|c, u, r, i| now_hidden.contains(&(c, u, r, i))).unwrap();
    assert_eq!(mic_words(&snap), mic_words(&snapshot(&*store, m).unwrap()));
    // Stop: the live run is not gated and has no removed words; the monitor
    // still says there was echo.
    let out = rec.stop().unwrap();
    assert_eq!(out.echo_dropped, 0);
    assert_eq!(out.echo.unwrap().status, EchoStatus::Coupled);
    let live = store.get_run(out.live_run).unwrap();
    let e = &live.engine_info["echo"];
    assert_eq!(
        (
            e["coupled"].as_bool(),
            e["user_off"].as_bool(),
            e["mode"].as_str(),
            e["dropped"].as_u64()
        ),
        (
            Some(true),
            Some(true),
            Some(ECHO_OFF_FOR_RECORDING),
            Some(0)
        ),
        "{e}"
    );
    assert_eq!(store.echo_dropped(out.live_run).unwrap(), None);
    let rows = store.list_live_utterances(out.live_run).unwrap();
    let stored_mic = latest_words(&rows, Channel::Mic);
    let live_mic = store
        .get_words(m, i64::MIN, i64::MAX)
        .unwrap()
        .iter()
        .filter(|w| w.text.starts_with('w') && w.raw_label.is_none())
        .count();
    assert_eq!(live_mic, stored_mic);
    // The post-pass honours it: scanned, not gated.
    let report = out
        .post_pass
        .wait(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (
            e["coupled"].as_bool(),
            e["user_off"].as_bool(),
            e["mode"].as_str(),
            e["dropped"].as_u64()
        ),
        (
            Some(true),
            Some(true),
            Some(ECHO_OFF_FOR_RECORDING),
            Some(0)
        ),
        "{e}"
    );
    assert_eq!(store.echo_dropped(report.current_run).unwrap(), None);
    let mic_left = store
        .get_words(m, i64::MIN, i64::MAX)
        .unwrap()
        .iter()
        .filter(|w| w.text.starts_with('p') && w.raw_label.is_none())
        .count() as u64;
    assert_eq!(Some(mic_left), e["mic_words_before"].as_u64());
    drop(ctx);
}

/// A view reloaded now: the snapshot through the recorder's live gate
/// against the sink's newest update of each utterance, read while the sink
/// is held (the event thread waits at its next update, so only the one
/// update in flight can differ). Returns (utterances compared, differing).
fn reload_against_sink(rec: &Recorder, store: &dyn RunStore, sink: &WarnSink) -> (usize, usize) {
    let sunk = sink.words.lock().unwrap();
    let hidden = rec.echo_hidden();
    let snap = snapshot_except(store, rec.meeting_id(), &|c, u, r, i| {
        hidden.contains(&(c, u, r, i))
    })
    .unwrap();
    let mut latest: HashMap<(Channel, u32), &WordsUpdate> = HashMap::new();
    for u in sunk.iter() {
        latest.insert((u.channel, u.utt), u);
    }
    let (mut compared, mut differ) = (0, 0);
    for s in &snap {
        if let Some(l) = latest.get(&(s.channel, s.utt)).filter(|l| l.rev == s.rev) {
            compared += 1;
            if serde_json::to_string(l).unwrap() != serde_json::to_string(s).unwrap() {
                differ += 1;
            }
        }
    }
    (compared, differ)
}

#[test]
fn a_reload_mid_recording_shows_what_the_sink_showed() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store) = echo_context(dir.path());
    let sink = Arc::new(WarnSink::default());
    let rec = Recorder::start(ctx.clone(), StartOptions::default(), sink.clone()).unwrap();
    // Before coupling, and after it (the gate hides the echo).
    std::thread::sleep(Duration::from_millis(3_000));
    let (n, differ) = reload_against_sink(&rec, &*store, &sink);
    assert!(
        n >= 10 && differ <= 1,
        "{differ} of {n} differ before coupling"
    );
    std::thread::sleep(Duration::from_millis(5_000));
    assert_eq!(rec.echo_state().unwrap().status, EchoStatus::Coupled);
    assert!(!rec.echo_hidden().is_empty());
    let (n, differ) = reload_against_sink(&rec, &*store, &sink);
    assert!(n >= 30 && differ <= 1, "{differ} of {n} differ while gated");
    drop(rec);
    drop(ctx);
}

/// The mic: one utterance that grows by a word per second of audio and is
/// revised every `every` seconds with all its words (each revision 4 ms
/// later than the one before, as an ASR re-stamping them), and once more at
/// close; the loopback: [`FakeSession`].
struct FakeLongLive {
    every: u64,
}

struct LongMicSession {
    job: JobId,
    tx: Sender<EngineEvent>,
    rx: Receiver<EngineEvent>,
    samples: u64,
    secs: Vec<u64>,
    rev: u32,
    every: u64,
    sent: usize,
}

impl LongMicSession {
    fn emit(&mut self) {
        let shift = u64::from(self.rev) * 4_000;
        let words = self
            .secs
            .iter()
            .map(|&s| EngineWord {
                t0_us: s * 1_000_000 + 100_000 + shift,
                t1_us: s * 1_000_000 + 800_000 + shift,
                text: format!("w{s}"),
                conf: None,
                spk: None,
            })
            .collect();
        let _ = self.tx.send(EngineEvent::Utterance {
            job_id: self.job.clone(),
            channel: Channel::Mic,
            utt: 0,
            rev: self.rev,
            words,
        });
        self.rev += 1;
        self.sent = self.secs.len();
    }
}

impl LiveSession for LongMicSession {
    fn push(&mut self, pcm: &[i16], _pts_us: u64) -> crate::error::Result<()> {
        let before = self.samples / 16_000;
        self.samples += pcm.len() as u64;
        for s in before..self.samples / 16_000 {
            self.secs.push(s);
            if (s + 1) % self.every == 0 {
                self.emit();
            }
        }
        Ok(())
    }
    fn events(&mut self) -> &mut Receiver<EngineEvent> {
        &mut self.rx
    }
    fn close(mut self: Box<Self>) -> crate::error::Result<()> {
        if self.sent < self.secs.len() {
            self.emit();
        }
        let _ = self.tx.send(EngineEvent::Done {
            job_id: self.job.clone(),
            stats: JobStats::default(),
        });
        Ok(())
    }
}

impl LiveEngine for FakeLongLive {
    fn open(
        &self,
        job: JobId,
        ch: Channel,
        opts: LiveOpts,
    ) -> crate::error::Result<Box<dyn LiveSession>> {
        if ch != Channel::Mic {
            return FakeLive.open(job, ch, opts);
        }
        let (tx, rx) = channel();
        Ok(Box::new(LongMicSession {
            job,
            tx,
            rx,
            samples: 0,
            secs: Vec::new(),
            rev: 0,
            every: self.every,
            sent: 0,
        }))
    }
}

#[test]
fn a_long_echoed_mic_utterance_keeps_its_first_words_out_of_the_view_and_the_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ctx, store) = echo_context(dir.path());
    ctx.live_engine = Arc::new(FakeLongLive { every: 10 });
    store.set_setting(settings::KEEP_LIVE_RUNS, "true").unwrap();
    let sink = Arc::new(WarnSink::default());
    let rec = Recorder::start(ctx.clone(), StartOptions::default(), sink.clone()).unwrap();
    let m = rec.meeting_id();
    // Longer than the monitor's ring (80 s with a 60 s ring), revised every
    // 10 s; played four times faster than real time (about 20 s). Waits for
    // the sink to get the revision at 80 s (bounded), not for a time: a busy
    // machine plays and revises it later.
    let secs = (crate::audio::echo::RING_SAMPLES / 16_000) as u64 + 20;
    let revs = (secs / 10) as u32;
    let latest = || {
        sink.words
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|u| u.channel == Channel::Mic && u.utt == 0)
            .cloned()
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    let view = loop {
        let view = latest();
        let coupled = rec
            .echo_state()
            .is_some_and(|s| s.status == EchoStatus::Coupled);
        if coupled && view.as_ref().is_some_and(|v| v.rev + 1 >= revs) {
            break view.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "rev {:?}, coupled {coupled}",
            view.map(|v| v.rev)
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(rec.echo_state().unwrap().status, EchoStatus::Coupled);
    assert!(view.rev + 1 >= revs, "rev {}", view.rev);
    let early = |words: &[(i64, &str)]| -> Vec<String> {
        words
            .iter()
            .filter(|(t, _)| (5_000..10_000).contains(t))
            .map(|(_, w)| w.to_string())
            .collect()
    };
    let shown: Vec<(i64, &str)> = view
        .words
        .iter()
        .map(|w| (w.start_ms, w.text.as_str()))
        .collect();
    assert!(early(&shown).is_empty(), "shown again: {:?}", early(&shown));
    assert!(shown.len() <= 3, "{shown:?}");
    let out = rec.stop().unwrap();
    let _ = out.post_pass.wait(Duration::from_secs(60));
    // The live run (kept) has them out too.
    store.set_current_run(m, out.live_run).unwrap();
    let words = store.get_words(m, i64::MIN, i64::MAX).unwrap();
    let mic: Vec<(i64, &str)> = words
        .iter()
        .filter(|w| w.raw_label.is_none() && w.text.starts_with('w'))
        .map(|w| (w.start_ms, w.text.as_str()))
        .collect();
    assert!(early(&mic).is_empty(), "in the live run: {:?}", early(&mic));
    assert!(mic.len() <= 3, "{mic:?}");
    let e = &store.get_run(out.live_run).unwrap().engine_info["echo"];
    assert!(
        e["dropped"].as_u64().unwrap() + 3 >= e["mic_words_before"].as_u64().unwrap(),
        "{e}"
    );
    drop(ctx);
}

#[test]
fn without_coupling_auto_shows_and_keeps_every_word_and_off_adds_nothing() {
    for mode in ["auto", "off"] {
        // Tones at 300 Hz on the mic and 500 Hz on the loopback: never coupled.
        let dir = tempfile::tempdir().unwrap();
        let (ctx, store, _) = context(dir.path(), None);
        store.set_setting(settings::ECHO_MODE, mode).unwrap();
        store.set_setting(settings::KEEP_LIVE_RUNS, "true").unwrap();
        let sink = Arc::new(WarnSink::default());
        let rec = Recorder::start(ctx.clone(), StartOptions::default(), sink.clone()).unwrap();
        let m = rec.meeting_id();
        std::thread::sleep(Duration::from_millis(5_500));
        match rec.echo_state() {
            Some(s) => {
                assert_eq!(mode, "auto");
                assert_ne!(s.status, EchoStatus::Coupled, "{s:?}");
            }
            None => assert_eq!(mode, "off"),
        }
        assert!(rec.echo_hidden().is_empty());
        // What the sink got is byte for byte what the store holds.
        let (n, differ) = reload_against_sink(&rec, &*store, &sink);
        assert!(n >= 6 && differ == 0, "{mode}: {differ} of {n} differ");
        let rows = store.list_live_utterances(rec.run_id()).unwrap();
        for u in sink.words.lock().unwrap().iter() {
            let stored = rows
                .iter()
                .find(|r| (r.channel, r.utt, r.rev) == (u.channel, u.utt, u.rev))
                .unwrap();
            assert_eq!(u.words.len(), stored.words.len(), "{mode}: {u:?}");
        }
        let out = rec.stop().unwrap();
        let _ = out.post_pass.wait(Duration::from_secs(60));
        assert_eq!(out.echo_dropped, 0);
        assert!(!sink
            .warnings
            .lock()
            .unwrap()
            .iter()
            .any(|w| matches!(w, LiveWarning::Echo { .. })));
        let live = store.get_run(out.live_run).unwrap();
        let rows = store.list_live_utterances(out.live_run).unwrap();
        let stored = latest_words(&rows, Channel::Mic) + latest_words(&rows, Channel::Loopback);
        if mode == "off" {
            assert!(
                live.engine_info.get("echo").is_none(),
                "{}",
                live.engine_info
            );
            assert!(out.echo.is_none());
        } else {
            let e = &live.engine_info["echo"];
            assert_eq!(
                (e["coupled"].as_bool(), e["dropped"].as_u64()),
                (Some(false), Some(0)),
                "{e}"
            );
            assert_eq!(
                e["mic_words_before"].as_u64(),
                Some(latest_words(&rows, Channel::Mic) as u64)
            );
        }
        assert_eq!(store.echo_dropped(out.live_run).unwrap(), None);
        // The live run holds every stored word.
        store.set_current_run(m, out.live_run).unwrap();
        assert_eq!(
            store.get_words(m, i64::MIN, i64::MAX).unwrap().len(),
            stored
        );
        drop(ctx);
    }
}

#[test]
fn an_echo_step_that_fails_leaves_the_mic_ungated_and_says_why() {
    let words = vec![crate::transcript::TranscriptWord {
        channel: Channel::Mic,
        start_ms: 1_000,
        end_ms: 1_400,
        text: "hello".into(),
        confidence: None,
        speaker: ME_LABEL.into(),
        raw_label: None,
        raw_text: None,
    }];
    let gone = Path::new("does-not-exist");
    let (mic, lb) = (gone.join("mic.wav"), gone.join("loopback.wav"));
    let prep = postpass::prepare_echo(
        &mic,
        &lb,
        &gone.join(postpass::MIC_AEC_WAV),
        None,
        EchoMode::Always,
        &serde_json::Value::Null,
        &|| false,
    );
    assert!(!prep.cancelled());
    let (kept, e) = postpass::echo_step(
        MeetingId(1),
        &mic,
        &mic,
        &lb,
        prep,
        postpass::MicWords {
            words: &words,
            far: &[],
            raw: None,
        },
        EchoMode::Auto,
        0.5,
        &serde_json::Value::Null,
    );
    assert_eq!(kept, words);
    assert!(!e.gated && e.dropped.is_empty());
    assert!(
        e.summary["error"]
            .as_str()
            .is_some_and(|m| m.contains("mic.wav")),
        "{}",
        e.summary
    );
    assert_eq!(e.summary.as_object().unwrap().len(), 1);
}

/// The echo fixture's WAVs in `folder`: the far end for the first 28 s of
/// the loopback, echoed into the mic 3 ms later and 10 dB down (with
/// `echo`), the owner alone over the last 28 s. With `gain`, the stored
/// loopback is what an AGC raising it by 0 to 12 dB made of it, and
/// `loopback.gain` says so.
fn echo_wavs(folder: &Path, echo: bool, gain: bool) {
    use crate::audio::echo::synth::{add_delayed, add_noise, db, monologue, to_i16};
    let n = 60 * 16_000;
    let (far, _) = monologue(51, 28 * 16_000, db(-28.0));
    let (own, _) = monologue(52, 28 * 16_000, db(-33.0));
    let mut lb = vec![0.0f32; n];
    lb[..far.len()].copy_from_slice(&far);
    add_noise(&mut lb, 53, db(-65.0));
    let mut mic = vec![0.0f32; n];
    mic[32 * 16_000..].copy_from_slice(&own);
    if echo {
        add_delayed(&mut mic, &lb, 48, db(-10.0));
    }
    add_noise(&mut mic, 54, db(-60.0));
    if gain {
        let path = crate::audio::gain_track::path_for(&folder.join("loopback.wav"));
        let mut w = crate::audio::gain_track::GainTrackWriter::create(&path).unwrap();
        let mut values = Vec::new();
        for f in 0..n / 1_600 {
            let rec: [f32; 10] = std::array::from_fn(|k| {
                let t = (f * 10 + k) as f32 / 100.0;
                6.0 + 6.0 * (t * 0.4).sin()
            });
            values.extend_from_slice(&rec);
            w.push((f * 1_600) as u64, &rec).unwrap();
        }
        w.flush().unwrap();
        crate::audio::gain_track::GainTrack::from_values(values).apply(0, &mut lb);
    }
    for (ch, x) in [("mic", &mic), ("loopback", &lb)] {
        let mut w = crate::audio::wav::WavSink::create(&folder.join(format!("{ch}.wav"))).unwrap();
        w.write(&to_i16(x)).unwrap();
        w.finalize().unwrap();
    }
}

/// What [`echo_post_pass_with`] ran.
struct EchoRun {
    store: Arc<SqliteStore>,
    report: PostPassReport,
    /// The WAV the fake engine transcribed for the mic.
    mic_job: PathBuf,
    /// The third job: the raw mic for the transfer gate (no live mic words).
    raw_job: Option<(PathBuf, bool)>,
    /// `mic-aec.wav` was left in the meeting folder.
    aec_left: bool,
}

/// The fake file engine's mic words (`p<s>` each second) stored as the live
/// run's mic utterances, one per second.
fn live_mic_like_the_file_engine(store: &SqliteStore, run: RunId, secs: u64) {
    for s in 0..secs {
        store
            .append_live_utterance(
                run,
                Channel::Mic,
                s as u32,
                0,
                &[EngineWord {
                    t0_us: s * 1_000_000 + 50_000,
                    t1_us: s * 1_000_000 + 900_000,
                    text: format!("p{s}"),
                    conf: None,
                    spk: None,
                }],
            )
            .unwrap();
    }
}

/// A stored meeting with [`echo_wavs`], then the real post-pass with the
/// fake engine's word per second. `user_off`: the live run says echo
/// handling was turned off for the recording; `gain`: the loopback has a
/// gain track.
fn echo_post_pass_with(mode: Option<&str>, echo: bool, user_off: bool, gain: bool) -> EchoRun {
    echo_post_pass_full(mode, echo, user_off, gain, false)
}

/// [`echo_post_pass_with`]; `live_mic`: the live run has the mic's words.
fn echo_post_pass_full(
    mode: Option<&str>,
    echo: bool,
    user_off: bool,
    gain: bool,
    live_mic: bool,
) -> EchoRun {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, store, file) = context(dir.path(), None);
    if let Some(m) = mode {
        store.set_setting(settings::ECHO_MODE, m).unwrap();
    }
    let (meeting, live) = interrupted_meeting(&store, dir.path(), "Echo", false);
    if live_mic {
        live_mic_like_the_file_engine(&store, live, 60);
    }
    if user_off {
        let mut info = store.get_run(live).unwrap().engine_info;
        info["echo"] = serde_json::json!({"user_off": true, "mode": ECHO_OFF_FOR_RECORDING});
        store.set_run_engine_info(live, info).unwrap();
    }
    let folder = dir.path().join("audio").join(&meeting.uid);
    echo_wavs(&folder, echo, gain);
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run: live,
        data_root: dir.path().to_path_buf(),
        folder: folder.clone(),
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: true,
        segment: SegmentOptions::default(),
        punct: crate::punct::PunctSource::None,
    };
    let report = run_post_pass(
        &*store,
        &*file,
        &job,
        &crate::pipeline::CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    let jobs = file.jobs.lock().unwrap().clone();
    assert!((2..=3).contains(&jobs.len()), "{jobs:?}");
    assert!(jobs[0].0.ends_with("loopback.wav"), "{jobs:?}");
    if gain {
        assert!(
            folder.join("loopback.gain").is_file(),
            "the gain track stays with the FLAC"
        );
    }
    EchoRun {
        store,
        report,
        mic_job: jobs[1].0.clone(),
        raw_job: jobs.get(2).cloned(),
        aec_left: folder.join(postpass::MIC_AEC_WAV).exists(),
    }
}

#[test]
fn post_pass_gates_the_echoed_half_and_off_changes_nothing() {
    let mic_words = |store: &SqliteStore, run: RunId| -> Vec<i64> {
        let m = store.get_run(run).unwrap().meeting_id;
        store
            .get_words(m, i64::MIN, i64::MAX)
            .unwrap()
            .iter()
            .filter(|w| w.text.starts_with('p') && w.raw_label.is_none())
            .map(|w| w.start_ms / 1_000)
            .collect()
    };
    // auto: the canceller runs (coupled, return loss 10 dB) and its output
    // is transcribed, then the echoed seconds go, the owner's stay.
    let r = echo_post_pass_with(None, true, false, false);
    let (store, report) = (r.store, r.report);
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(e["coupled"], true, "{e}");
    assert_eq!(
        (e["aec"].as_bool(), e["gain_track"].as_bool()),
        (Some(true), Some(false)),
        "{e}"
    );
    assert!(e["erle_db"].as_f64().unwrap() >= 20.0, "{e}");
    assert!(e["aec_wall_s"].as_f64().unwrap() > 0.0, "{e}");
    assert!(e["aec_erl_db"].as_f64().unwrap() < 20.0, "{e}");
    assert!(e.get("aec_error").is_none(), "{e}");
    // The cancelled mic's words get the gate's band after the canceller.
    assert_eq!(e["band"], serde_json::json!([0.6, 0.7, 0.95]), "{e}");
    assert!(
        r.mic_job.ends_with(postpass::MIC_AEC_WAV),
        "{:?}",
        r.mic_job
    );
    assert!(!r.aec_left);
    assert_eq!(
        (e["mic_words_before"].as_u64(), e["threshold"].as_f64()),
        (Some(60), Some(0.5))
    );
    let kept = mic_words(&store, report.current_run);
    assert!(kept.iter().all(|s| *s >= 27), "{kept:?}");
    assert!((32..60).all(|s| kept.contains(&s)), "{kept:?}");
    let dropped = store.echo_dropped(report.current_run).unwrap().unwrap();
    assert_eq!(dropped.len() as u64, e["dropped"].as_u64().unwrap());
    assert!(dropped
        .iter()
        .all(|w| w.start_ms < 30_000 && w.text.starts_with('p')));
    // The transfer gate: the live run has no mic words, so the raw mic was
    // transcribed (undiarized) and gated with the raw band; its echo words
    // carry over to the cancelled words of the same text.
    assert!(
        r.raw_job
            .as_ref()
            .is_some_and(|(p, diarize)| p.ends_with("mic.wav") && !diarize),
        "{:?}",
        r.raw_job
    );
    let t = &e["transfer"];
    assert_eq!(
        (
            t["source"].as_str(),
            t["raw_words"].as_u64(),
            t["guard"].as_str()
        ),
        (Some("transcribed"), Some(60), Some("level")),
        "{e}"
    );
    assert_eq!(t["raw_band"], serde_json::json!([0.3, 0.5, 0.7]), "{e}");
    assert!(t["raw_dropped"].as_u64().unwrap() >= 20, "{e}");
    assert_eq!(t["plain"].as_u64(), Some(0), "{e}");
    assert_eq!(
        t["removed"].as_u64(),
        e["layers"]["transfer"].as_u64().or(Some(0)),
        "{e}"
    );
    assert!(dropped
        .iter()
        .filter(|w| w.reason == Some(crate::types::EchoReason::Transfer))
        .all(|w| w.start_ms < 30_000));
    // off: no canceller, no echo key, no list, every word.
    let r = echo_post_pass_with(Some("off"), true, false, false);
    assert!(
        r.mic_job.ends_with("mic.wav") && !r.aec_left && r.raw_job.is_none(),
        "{:?}",
        r.mic_job
    );
    let (store, report) = (r.store, r.report);
    let run = store.get_run(report.current_run).unwrap();
    assert!(run.engine_info.get("echo").is_none());
    assert_eq!(store.echo_dropped(report.current_run).unwrap(), None);
    assert_eq!(mic_words(&store, report.current_run).len(), 60);
    // always, without echo: cancelled and gated, nothing correlates,
    // nothing goes. The raw gate takes the words the fake engine heard in
    // the mic's noise floor while the far end talked (its level layer, which
    // `always` engages, as live), but the canceller cut none of them: the
    // transfer's guard keeps them.
    let r = echo_post_pass_with(Some("always"), false, false, false);
    assert!(r.mic_job.ends_with(postpass::MIC_AEC_WAV) && !r.aec_left);
    assert!(r.raw_job.is_some());
    let (store, report) = (r.store, r.report);
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (
            e["coupled"].as_bool(),
            e["dropped"].as_u64(),
            e["aec"].as_bool()
        ),
        (Some(false), Some(0), Some(true))
    );
    let t = &e["transfer"];
    assert_eq!(t["raw_layers"]["correlation"], 0, "{e}");
    assert!(t["raw_layers"]["level"].as_u64().unwrap() > 0, "{e}");
    assert_eq!(t["removed"], 0, "{e}");
    assert_eq!(t["guarded"], t["candidates"], "{e}");
    assert_eq!(
        store.echo_dropped(report.current_run).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(mic_words(&store, report.current_run).len(), 60);
    // Turned off for the recording (the live run says so): scanned, neither
    // cancelled nor gated.
    let r = echo_post_pass_with(None, true, true, false);
    assert!(r.mic_job.ends_with("mic.wav") && !r.aec_left && r.raw_job.is_none());
    let (store, report) = (r.store, r.report);
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (
            e["coupled"].as_bool(),
            e["dropped"].as_u64(),
            e["mode"].as_str()
        ),
        (Some(true), Some(0), Some(ECHO_OFF_FOR_RECORDING)),
        "{e}"
    );
    assert_eq!(e["aec"], false, "{e}");
    assert_eq!(e["band"], serde_json::json!([0.3, 0.5, 0.7]), "{e}");
    assert_eq!(store.echo_dropped(report.current_run).unwrap(), None);
    assert_eq!(mic_words(&store, report.current_run).len(), 60);
    // With the loopback's gain track: replayed backwards for the reference.
    let r = echo_post_pass_with(None, true, false, true);
    let run = r.store.get_run(r.report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (e["aec"].as_bool(), e["gain_track"].as_bool()),
        (Some(true), Some(true)),
        "{e}"
    );
    assert!(e["erle_db"].as_f64().unwrap() >= 20.0, "{e}");
    let kept = mic_words(&r.store, r.report.current_run);
    assert!((32..60).all(|s| kept.contains(&s)), "{kept:?}");
}

#[test]
fn the_transfer_takes_the_live_runs_mic_words_when_there_are_any() {
    // The live run heard the mic: its words are the raw source, and the
    // engine runs twice (loopback, the canceller's output), not three times.
    let r = echo_post_pass_full(None, true, false, false, true);
    assert!(
        r.mic_job.ends_with(postpass::MIC_AEC_WAV),
        "{:?}",
        r.mic_job
    );
    assert!(r.raw_job.is_none(), "{:?}", r.raw_job);
    let run = r.store.get_run(r.report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    let t = &e["transfer"];
    assert_eq!(
        (t["source"].as_str(), t["raw_words"].as_u64()),
        (Some("live"), Some(60)),
        "{e}"
    );
    assert!(t["raw_dropped"].as_u64().unwrap() >= 20, "{e}");
    let kept: Vec<i64> = r
        .store
        .get_words(run.meeting_id, i64::MIN, i64::MAX)
        .unwrap()
        .iter()
        .filter(|w| w.text.starts_with('p') && w.raw_label.is_none())
        .map(|w| w.start_ms / 1_000)
        .collect();
    assert!(kept.iter().all(|s| *s >= 27), "{kept:?}");
    assert!((32..60).all(|s| kept.contains(&s)), "{kept:?}");
    // The post-pass run has no live utterances.
    assert!(postpass::live_mic_words(&*r.store, r.report.current_run).is_empty());
}

#[test]
fn live_mic_words_are_the_latest_revisions_of_the_mic_on_meeting_time() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, store, _file) = context(dir.path(), None);
    let (_meeting, live) = interrupted_meeting(&store, dir.path(), "Live words", false);
    let word = |t: u64, text: &str| EngineWord {
        t0_us: t * 1_000,
        t1_us: t * 1_000 + 300_000,
        text: text.into(),
        conf: None,
        spk: None,
    };
    for (utt, rev, words) in [
        (1u32, 0u32, vec![word(5_000, "later")]),
        (0, 0, vec![word(1_000, "first")]),
        (0, 1, vec![word(1_000, "first"), word(1_500, "draft")]),
        (0, 2, vec![word(1_000, "final"), word(1_400, "words")]),
    ] {
        store
            .append_live_utterance(live, Channel::Mic, utt, rev, &words)
            .unwrap();
    }
    let w = postpass::live_mic_words(&*store, live);
    let got: Vec<(i64, &str)> = w.iter().map(|w| (w.start_ms, w.text.as_str())).collect();
    // The loopback's "hello" is not the mic's.
    assert_eq!(got, [(1_000, "final"), (1_400, "words"), (5_000, "later")]);
    assert!(w
        .iter()
        .all(|w| w.channel == Channel::Mic && w.speaker == ME_LABEL));
    // A run that is gone: none.
    assert!(postpass::live_mic_words(&*store, RunId(9_999)).is_empty());
}

#[test]
fn recovery_and_a_rerun_cancel_the_echo_through_the_same_post_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    // A crash during the post-pass left the WAVs and a half-written
    // mic-aec.wav; the engine fails once (the loopback), then works.
    let (m, live) = interrupted_meeting(&store, dir.path(), "Echo", false);
    let folder = dir.path().join("audio").join(&m.uid);
    echo_wavs(&folder, true, true);
    std::fs::write(folder.join(postpass::MIC_AEC_WAV), b"RIFF").unwrap();
    file.fail_next.store(true, Ordering::SeqCst);
    let recovered = recover(&ctx).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].action, RecoveryAction::PostPassQueued);
    let err = recovered[0]
        .post_pass
        .as_ref()
        .unwrap()
        .wait(Duration::from_secs(60))
        .unwrap();
    assert!(err.unwrap_err().contains("fake crash"));
    // The failure took the canceller's output with it; the WAVs stay.
    assert!(!folder.join(postpass::MIC_AEC_WAV).exists());
    assert!(folder.join("mic.wav").is_file() && folder.join("loopback.wav").is_file());
    assert_eq!(
        store.get_meeting(m.id).unwrap().meeting.status,
        MeetingStatus::Failed
    );
    // The re-run goes through the same post-pass: cancelled, then gated.
    let report = rerun_post_pass(&ctx, m.id)
        .unwrap()
        .wait(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    let jobs = file.jobs.lock().unwrap().clone();
    let last: Vec<&PathBuf> = jobs.iter().rev().take(3).map(|j| &j.0).collect();
    // The live run has no mic words: the raw mic is transcribed last.
    assert!(last[0].ends_with("mic.wav"), "{jobs:?}");
    assert!(last[1].ends_with(postpass::MIC_AEC_WAV), "{jobs:?}");
    assert!(last[2].ends_with("loopback.wav"), "{jobs:?}");
    let run = store.get_run(report.current_run).unwrap();
    let e = &run.engine_info["echo"];
    assert_eq!(
        (e["aec"].as_bool(), e["gain_track"].as_bool()),
        (Some(true), Some(true)),
        "{e}"
    );
    assert_eq!(e["transfer"]["source"], "transcribed", "{e}");
    assert!(e["dropped"].as_u64().unwrap() > 0, "{e}");
    assert!(!folder.join(postpass::MIC_AEC_WAV).exists());
    assert!(!folder.join("mic.wav").exists());
    assert!(folder.join("loopback.gain").is_file());
    assert!(store.get_run(live).is_err(), "the live run went");
}

#[test]
fn restored_runs_are_not_the_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    // A post-pass that failed: the live run is current, with words the live
    // gate removed, and the WAVs are still there.
    let (m, live) = interrupted_meeting(&store, dir.path(), "Echo", true);
    store
        .set_echo_dropped(
            live,
            &[crate::types::EchoDroppedWord {
                start_ms: 1_000,
                end_ms: 1_300,
                text: "echo".into(),
                coefficient: 0.8,
                reason: None,
            }],
        )
        .unwrap();
    let mut info = store.get_run(live).unwrap().engine_info;
    info["echo"] = serde_json::json!({"coupled": true, "dropped": 1, "aec": false, "mode": "auto"});
    store.set_run_engine_info(live, info).unwrap();
    store
        .set_meeting_status(m.id, MeetingStatus::Failed)
        .unwrap();
    // Restore: a new live-kind run, current, that says where it came from.
    let restored =
        super::restore::restore_echo_dropped(&*store, &crate::punct::PunctSource::None, live)
            .unwrap();
    let r = store.get_run(restored.run).unwrap();
    assert_eq!(r.kind, RunKind::Live);
    assert_eq!(
        r.engine_info["echo"]["restored_from"].as_i64(),
        Some(live.0)
    );
    assert_eq!(
        store.get_meeting(m.id).unwrap().current_run.unwrap().id,
        restored.run
    );
    // The snapshot is the live run's utterances, not the restored copy's.
    let snap = snapshot(&*store, m.id).unwrap();
    assert!(
        !snap.is_empty() && snap.iter().all(|u| u.run_id == live),
        "{snap:?}"
    );
    // A name given on the restored copy (plan 16 section 3.9 step 7).
    let bea = store
        .run_speakers(restored.run)
        .unwrap()
        .into_iter()
        .find(|s| s.engine_label == "spk_0")
        .unwrap();
    store.rename_speaker(bea.id, "Bea", None).unwrap();
    // The re-run transcribes the live run's recording, carries names over
    // from the restored copy (the current run, a copy of the live run), and
    // replaces the live run; the restored copy is kept (not current).
    let report = rerun_post_pass(&ctx, m.id)
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    assert_eq!(run.kind, RunKind::PostPass);
    assert_eq!(run.engine_info["live_run"].as_i64(), Some(live.0));
    assert!(
        report
            .speakers
            .contains(&(Channel::Loopback, "spk_0".into(), Some("Bea".into()))),
        "{:?}",
        report.speakers
    );
    assert!(report.live_run_deleted);
    assert!(store.get_run(live).is_err());
    assert!(store.get_run(restored.run).is_ok());
    assert_eq!(file.jobs.lock().unwrap().len(), 2);
    // Crash recovery picks the live run too.
    let (m2, live2) = interrupted_meeting(&store, dir.path(), "Echo 2", true);
    store
        .set_meeting_status(m2.id, MeetingStatus::Done)
        .unwrap();
    store
        .set_echo_dropped(
            live2,
            &[crate::types::EchoDroppedWord {
                start_ms: 1_000,
                end_ms: 1_300,
                text: "echo".into(),
                coefficient: 0.8,
                reason: None,
            }],
        )
        .unwrap();
    let restored2 =
        super::restore::restore_echo_dropped(&*store, &crate::punct::PunctSource::None, live2)
            .unwrap();
    store
        .set_meeting_status(m2.id, MeetingStatus::Processing)
        .unwrap();
    let rec = recover(&ctx).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].live_run, Some(live2));
    let report = rec[0]
        .post_pass
        .as_ref()
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(
        store.get_run(report.current_run).unwrap().engine_info["live_run"].as_i64(),
        Some(live2.0)
    );
    assert!(store.get_run(restored2.run).is_ok());
}

// ---- punctuation (plan 16) ---------------------------------------------------

/// A post-pass job over `interrupted_meeting`'s WAVs.
fn punct_job(
    dir: &Path,
    meeting: &Meeting,
    live: RunId,
    transcribe: bool,
    punct: crate::punct::PunctSource,
) -> PostPassJob {
    PostPassJob {
        job_id: JobId::new(),
        meeting: meeting.id,
        live_run: live,
        data_root: dir.to_path_buf(),
        folder: dir.join("audio").join(&meeting.uid),
        language: "en".into(),
        diarize_mic: false,
        transcribe,
        keep_live_runs: false,
        segment: SegmentOptions::default(),
        punct,
    }
}

fn fixture(marks: &'static [(&'static str, crate::punct::Mark)]) -> crate::punct::PunctSource {
    crate::punct::PunctSource::factory(move || {
        crate::punct::fixture::FixturePunctuator::marks(marks)
    })
}

fn word_texts(store: &dyn RunStore, m: MeetingId) -> Vec<(String, Option<String>)> {
    store
        .get_words(m, i64::MIN, i64::MAX)
        .unwrap()
        .into_iter()
        .map(|w| (w.text, w.raw_text))
        .collect()
}

#[test]
fn the_post_pass_punctuates_after_the_echo_gate_and_before_segmentation() {
    use crate::punct::Mark;
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, store, file) = context(dir.path(), None);
    // With a punctuator: both channels, each its own stream.
    let (m, live) = interrupted_meeting(&store, dir.path(), "Punct", true);
    let job = punct_job(
        dir.path(),
        &m,
        live,
        true,
        fixture(&[("p2", Mark::Period), ("q2", Mark::Question)]),
    );
    let report = run_post_pass(
        &*store,
        &*file,
        &job,
        &crate::pipeline::CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    assert_eq!(run.kind, RunKind::PostPass);
    let p = &run.engine_info["punctuation"];
    assert_eq!(p["applied"], true, "{p}");
    assert_eq!(p["words"], 6);
    assert_eq!(p["channels"]["mic"], "ok");
    assert_eq!(p["channels"]["loopback"], "ok");
    let words = word_texts(&*store, m.id);
    let shown: Vec<&str> = words.iter().map(|w| w.0.as_str()).collect();
    assert_eq!(shown, ["P0", "Q0", "p1", "q1", "p2.", "q2?"]);
    assert_eq!(words[0].1.as_deref(), Some("p0"));
    assert_eq!(words[2].1, None);
    assert_eq!(file.jobs.lock().unwrap().len(), 2);

    // Without one: the ASR's words and the reason.
    let (m2, live2) = interrupted_meeting(&store, dir.path(), "Plain", true);
    let job = punct_job(
        dir.path(),
        &m2,
        live2,
        true,
        crate::punct::PunctSource::None,
    );
    let report = run_post_pass(
        &*store,
        &*file,
        &job,
        &crate::pipeline::CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    assert_eq!(
        run.engine_info["punctuation"],
        serde_json::json!({"applied": false, "reason": "no_model"})
    );
    let words = word_texts(&*store, m2.id);
    assert_eq!(words[0], ("p0".to_owned(), None));
}

#[test]
fn a_promoted_live_run_is_punctuated_as_a_post_pass_run() {
    use crate::punct::Mark;
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    let (m, live) = interrupted_meeting(&store, dir.path(), "Promote", true);
    // The live gate removed a word; the user named the loopback speaker.
    let dropped = [crate::types::EchoDroppedWord {
        start_ms: 1_000,
        end_ms: 1_300,
        text: "echo".into(),
        coefficient: 0.8,
        reason: None,
    }];
    store.set_echo_dropped(live, &dropped).unwrap();
    let mut info = store.get_run(live).unwrap().engine_info;
    info["echo"] = serde_json::json!({"coupled": true, "dropped": 1, "aec": false, "mode": "auto"});
    store.set_run_engine_info(live, info).unwrap();
    let alice = store
        .run_speakers(live)
        .unwrap()
        .into_iter()
        .find(|s| s.engine_label == "spk_0")
        .unwrap();
    store.rename_speaker(alice.id, "Alice", None).unwrap();

    let job = punct_job(
        dir.path(),
        &m,
        live,
        false,
        fixture(&[("hello", Mark::Period)]),
    );
    let report = run_post_pass(
        &*store,
        &*file,
        &job,
        &crate::pipeline::CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    assert!(report.promoted_live);
    assert_ne!(report.current_run, live);
    assert!(file.jobs.lock().unwrap().is_empty(), "nothing transcribed");
    let run = store.get_run(report.current_run).unwrap();
    assert_eq!((run.kind, run.status), (RunKind::PostPass, RunStatus::Done));
    assert_eq!(run.engine_info["promoted_live"].as_i64(), Some(live.0));
    assert_eq!(run.engine_info["live_run"].as_i64(), Some(live.0));
    assert_eq!(run.engine_info["runtime"], "fake live");
    assert_eq!(run.engine_info["echo"]["dropped"], 1);
    assert_eq!(run.engine_info["punctuation"]["applied"], true);
    assert_eq!(store.echo_dropped(run.id).unwrap().unwrap(), dropped);
    assert_eq!(
        word_texts(&*store, m.id),
        [("Hello.".to_owned(), Some("hello".to_owned()))]
    );
    assert!(report
        .speakers
        .contains(&(Channel::Loopback, "spk_0".into(), Some("Alice".into()))));
    let d = store.get_meeting(m.id).unwrap();
    assert_eq!(d.meeting.status, MeetingStatus::Done);
    assert_eq!(d.current_run.unwrap().id, run.id);
    // It replaced the live run, as a transcribing post-pass does.
    assert!(report.live_run_deleted);
    assert!(store.get_run(live).is_err());
    // Crash recovery sees a finished post-pass.
    store
        .set_meeting_status(m.id, MeetingStatus::Processing)
        .unwrap();
    let rec = recover(&ctx).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].action, RecoveryAction::Finished);
    assert_eq!(
        store.get_meeting(m.id).unwrap().current_run.unwrap().id,
        run.id
    );

    // Without a punctuator the live run is promoted as before.
    let (m2, live2) = interrupted_meeting(&store, dir.path(), "Plain", true);
    let job = punct_job(
        dir.path(),
        &m2,
        live2,
        false,
        crate::punct::PunctSource::None,
    );
    let report = run_post_pass(
        &*store,
        &*file,
        &job,
        &crate::pipeline::CancelToken::default(),
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.current_run, live2);
    assert!(!report.live_run_deleted);
    assert_eq!(
        store.get_meeting(m2.id).unwrap().current_run.unwrap().kind,
        RunKind::Live
    );
    assert_eq!(word_texts(&*store, m2.id), [("hello".to_owned(), None)]);
}

#[test]
fn rebuilt_copies_are_not_the_live_run_and_their_names_survive_a_re_run() {
    use crate::punct::{rebuild_run, Mark, RebuildWhy};
    let dir = tempfile::tempdir().unwrap();
    let (ctx, store, file) = context(dir.path(), None);
    // A post-pass that failed: the live run is current, the WAVs are there.
    let (m, live) = interrupted_meeting(&store, dir.path(), "Copy", true);
    store
        .set_meeting_status(m.id, MeetingStatus::Failed)
        .unwrap();
    // "Add punctuation": a copy of kind live, current, never the live run.
    let r = rebuild_run(
        &*store,
        &fixture(&[("hello", Mark::Period)]),
        m.id,
        RebuildWhy::Add,
        &|| false,
    )
    .unwrap();
    let copy = r.run.unwrap();
    assert_eq!(store.get_run(copy).unwrap().kind, RunKind::Live);
    assert_eq!(
        live_run_of(&store.list_runs(m.id).unwrap()).map(|r| r.id),
        Some(live)
    );
    // The snapshot is the live run's utterances.
    assert!(snapshot(&*store, m.id)
        .unwrap()
        .iter()
        .all(|u| u.run_id == live));
    // A name given on the copy survives the re-run of the post-pass.
    let spk = store
        .run_speakers(copy)
        .unwrap()
        .into_iter()
        .find(|s| s.engine_label == "spk_0")
        .unwrap();
    store.rename_speaker(spk.id, "Alice", None).unwrap();
    let report = rerun_post_pass(&ctx, m.id)
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let run = store.get_run(report.current_run).unwrap();
    assert_eq!(run.kind, RunKind::PostPass);
    assert_eq!(run.engine_info["live_run"].as_i64(), Some(live.0));
    assert!(
        report
            .speakers
            .contains(&(Channel::Loopback, "spk_0".into(), Some("Alice".into()))),
        "{:?}",
        report.speakers
    );
    assert_eq!(file.jobs.lock().unwrap().len(), 2);
    assert!(store.get_run(copy).is_ok(), "the copy is kept");

    // Crash recovery picks the live run, not a rebuilt copy.
    let (m2, live2) = interrupted_meeting(&store, dir.path(), "Copy 2", true);
    store
        .set_meeting_status(m2.id, MeetingStatus::Done)
        .unwrap();
    let r2 = rebuild_run(
        &*store,
        &fixture(&[("hello", Mark::Period)]),
        m2.id,
        RebuildWhy::Add,
        &|| false,
    )
    .unwrap();
    let copy2 = r2.run.unwrap();
    let spk = store
        .run_speakers(copy2)
        .unwrap()
        .into_iter()
        .find(|s| s.engine_label == "spk_0")
        .unwrap();
    store.rename_speaker(spk.id, "Cara", None).unwrap();
    store
        .set_meeting_status(m2.id, MeetingStatus::Processing)
        .unwrap();
    let rec = recover(&ctx).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].live_run, Some(live2));
    let report = rec[0]
        .post_pass
        .as_ref()
        .unwrap()
        .wait(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(
        store.get_run(report.current_run).unwrap().engine_info["live_run"].as_i64(),
        Some(live2.0)
    );
    // The copy stayed current while the post-pass waited: its name carried.
    assert!(report
        .speakers
        .contains(&(Channel::Loopback, "spk_0".into(), Some("Cara".into()))));
    assert!(store.get_run(copy2).is_ok());
}
