//! Live recording (`docs/01`, "Live flow"; plan 04 chunk 3d).
//!
//! [`Recorder::start`] checks free VRAM (refuses under 2.5 GB, from NVML or
//! the runtime's doctor report: [`crate::gpu`]) and free disk, creates the meeting (status
//! `recording`) and its `live` run (made current, so the UI shows its
//! speakers), starts per channel a capture ([`crate::audio::capture`]), a
//! pipeline thread ([`crate::audio::pipeline`]: WAV, clock anchors, frames)
//! and an engine feed with its live session ([`crate::audio::feed`]; mic:
//! no diarization unless `diarize_mic`, speaker `me`; loopback: diarized),
//! pauses the [`JobQueue`], keeps the system awake and listens for suspend
//! and resume. One event thread appends every utterance revision to
//! `live_utterances` (word times mapped to meeting time through the live
//! clock anchors), gives each provisional label a speaker row
//! ([`RunStore::live_speaker`], so it can be renamed before Stop), and
//! forwards everything to a [`LiveSink`]: words, final-text marks, levels
//! at 10 Hz per channel, degraded / recovered, warnings (loopback silent
//! for 60 s while the mic is active) and device changes.
//!
//! [`Recorder::stop`] stops the captures, ends both WAVs at the same length,
//! closes the sessions (their last finals still land), materialises the live
//! run, resumes the queue and submits the post-pass ([`postpass`]) ahead of
//! imports queued meanwhile, unless `post_pass_mode = on_issues` and nothing
//! went wrong, in which case only the audio work runs and the live run
//! stays. [`recover`] finishes meetings a crash left in `recording` or
//! `processing`; [`rerun_post_pass`] queues a failed or cancelled post-pass
//! again.

pub mod postpass;
pub mod power;
mod recover;
mod rerun;
mod snapshot;
pub mod views;

pub use postpass::{run_post_pass, PostPassJob, PostPassReport};
pub use power::{KeepAwake, PowerEvent, PowerNotifications};
pub use recover::{recover, RecoveredMeeting, RecoveryAction, PROMOTED_NOTE};
pub use rerun::rerun_post_pass;
pub use snapshot::snapshot;
pub use views::{
    LiveLevels, LivePhase, LiveStateView, LiveStatusView, PostPassView, ServeView, StartRequest,
    StopSummary, TextFinal, ToggleAction, ToggleVia, Toggled, WarningNote,
};

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use crate::audio::capture::{
    qpc_now_100ns, CaptureConfig, CaptureStatus, ChangeReason, PacketSource, SourceFactory,
};
use crate::audio::clock::ClockMap;
use crate::audio::feed::{
    feed, spawn_feed_worker, FeedNotice, FeedReport, FeedTransition, FeedWorker, FEED_CAPACITY,
    FEED_RECOVER_BELOW,
};
use crate::audio::gain::AgcConfig;
use crate::audio::pipeline::{
    spawn_pipeline, Level, PipelineCommand, PipelineEvent, PipelineNotice, PipelineResult,
    PipelineThreadConfig,
};
use crate::engine::{Engine, EngineEvent, EngineWord, LiveEngine, LiveOpts};
use crate::error::{CoreError, Result};
use crate::models::free_disk_bytes;
use crate::pipeline::JobQueue;
use crate::settings;
use crate::store::{NewAudioTrack, NewMeeting, RunStore};
use crate::transcript::{SegmentOptions, ME_LABEL, UNKNOWN_LABEL};
use crate::types::{
    AudioFormat, Channel, ClockAnchor, JobId, Meeting, MeetingId, MeetingStatus, RunId, RunKind,
    RunStatus, Source, SpeakerId, TrackChannel, TranscriptRun,
};
use crate::views::JobEvent;

pub use crate::settings::{CAPTURE_AUTOCONVERT, CAPTURE_LOOPBACK_AGC, CAPTURE_MIC_AGC};
/// Loopback peaks (before the AGC) under this for [`LOOPBACK_QUIET_WINDOW`]
/// while there is audio: the output's volume is too low.
pub const LOOPBACK_QUIET_DBFS: f32 = -50.0;
pub const LOOPBACK_QUIET_WINDOW: Duration = Duration::from_secs(10);
/// Free VRAM needed to start (research/08 section 5).
pub const MIN_FREE_VRAM_MB: u32 = 2_560;
/// Free disk needed to start.
pub const MIN_FREE_BYTES: u64 = 2 << 30;
/// Loopback silence while the mic is active before a warning.
pub const LOOPBACK_SILENT_WARNING: Duration = Duration::from_secs(60);
/// Capture rings hold this many packets (about 20 s of 10 ms packets).
const RING_PACKETS: usize = 2_048;

/// Free VRAM in MB (`None` when unknown), as it is at Start
/// ([`crate::gpu::free_vram`]).
pub type VramProbe = Arc<dyn Fn() -> Option<u32> + Send + Sync>;
/// Receives post-pass job events.
pub type JobEventFn = Arc<dyn Fn(JobEvent) + Send + Sync>;

/// Everything a recorder needs that outlives one recording.
#[derive(Clone)]
pub struct LiveContext {
    pub store: Arc<dyn RunStore>,
    /// The process-wide GPU queue; paused while recording.
    pub queue: Arc<JobQueue>,
    /// Data root: recordings go to `audio/<uid>/`.
    pub data_root: PathBuf,
    pub live_engine: Arc<dyn LiveEngine>,
    /// The post-pass engine (`NemoCliEngine`).
    pub file_engine: Arc<dyn Engine>,
    /// Starts captures ([`crate::audio::capture::wasapi_source_factory`]).
    pub sources: Arc<SourceFactory>,
    pub vram_free_mb: VramProbe,
    pub min_free_vram_mb: u32,
    pub min_free_bytes: u64,
    /// Runtime and models, stored in the live run's `engine_info`.
    pub live_engine_info: serde_json::Value,
    /// Post-pass progress and its terminal event.
    pub on_job_event: Option<JobEventFn>,
}

/// Which endpoints to record (ids from [`crate::audio::capture::list_devices`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceSelection {
    /// `None`: the default communications capture endpoint.
    pub mic: Option<String>,
    /// `None`: the default render endpoint, following foreign playback.
    pub loopback: Option<String>,
}

/// What to record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOptions {
    /// Default: "Recording <date time>".
    pub title: Option<String>,
    /// BCP-47 or `auto`; default: setting `default_language`.
    pub language: Option<String>,
    /// Diarize the mic too (several people in the room).
    pub diarize_mic: bool,
    pub devices: DeviceSelection,
}

/// One live word as the UI shows it. Times are meeting time.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct LiveWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// `me`, `spk_N` or `spk_unknown`; always provisional while recording.
    pub label: String,
    /// The live run's speaker row (rename it with `Store::rename_speaker`).
    pub speaker_id: Option<SpeakerId>,
}

/// A revision of one utterance: it replaces every earlier `rev` of the same
/// `(channel, utt)` entirely (`live:words`, `live_snapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct WordsUpdate {
    pub meeting_id: MeetingId,
    pub run_id: RunId,
    pub channel: Channel,
    pub utt: u32,
    pub rev: u32,
    pub words: Vec<LiveWord>,
}

/// Reserved: relabel a window (no current engine produces it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelabelUpdate {
    pub channel: Channel,
    pub from_ms: i64,
    pub to_ms: i64,
    pub map: BTreeMap<String, String>,
}

/// One 100 ms level reading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct LevelsUpdate {
    pub channel: Channel,
    /// Channel time of the frame.
    pub at_ms: i64,
    /// After the AGC (what is recorded and transcribed).
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
    /// Gain the AGC applied (0 when the channel has no AGC).
    pub gain_db: f32,
    /// Peak before the AGC.
    pub raw_peak_dbfs: f32,
}

/// The engine feed of a channel stopped (record-only) or resumed
/// (`live:degraded`).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct DegradedUpdate {
    pub channel: Channel,
    pub degraded: bool,
    pub reason: String,
}

/// Something the user should know.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum LiveWarning {
    /// Nothing came from the loopback for this long while the mic was active.
    LoopbackSilent { seconds: u32 },
    /// The loopback carries audio, but for 10 s its peaks stayed under
    /// -50 dBFS before the AGC: the output's Windows volume is very low.
    /// UI: "System audio from <device> is very quiet; raise its Windows
    /// volume or pick another output".
    LoopbackQuiet {
        device: String,
        /// Highest raw peak in the window.
        peak_dbfs: f32,
        /// AGC gain at the time.
        gain_db: f32,
    },
    /// A capture or storage problem (recording continues).
    Capture { channel: Channel, message: String },
}

/// A capture opened on a (new) endpoint (`live:device_changed`).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct DeviceChange {
    pub channel: Channel,
    pub device_id: String,
    pub name: String,
    pub reason: ChangeReason,
    /// Channel time where the change happened.
    pub at_ms: i64,
}

/// Where a recording's live output goes (the CLI prints, the desktop app
/// forwards to the WebView). Called from the recorder's event thread;
/// implementations must return quickly.
pub trait LiveSink: Send + Sync {
    fn words(&self, update: &WordsUpdate);
    /// Text of `channel` before `until_ms` (meeting time) is final; labels
    /// stay provisional until the post-pass. Without it, treat text older
    /// than 10 s as final.
    fn text_final(&self, _channel: Channel, _until_ms: i64) {}
    /// Reserved.
    fn relabel(&self, _update: &RelabelUpdate) {}
    /// 10 Hz per channel.
    fn levels(&self, _update: &LevelsUpdate) {}
    fn degraded(&self, _update: &DegradedUpdate) {}
    fn warning(&self, _warning: &LiveWarning) {}
    fn device_changed(&self, _change: &DeviceChange) {}
}

/// What went wrong while recording (decides `post_pass_mode = on_issues`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct LiveIssues {
    /// Feed overflows (engine too slow; record-only for a while).
    pub degraded: u32,
    /// Sessions that failed or could not be opened.
    pub engine_down: u32,
    /// Sessions reopened after a failure.
    pub restarts: u32,
    /// Zero-filled gaps and their total length.
    pub gaps: u32,
    pub gap_ms: u64,
    /// Suspend/resume jumps.
    pub jumps: u32,
    /// Endpoint changes after the first open.
    pub device_changes: u32,
}

impl LiveIssues {
    /// Degraded feed, engine restarts or gaps happened.
    pub fn any(&self) -> bool {
        self.degraded + self.engine_down + self.restarts + self.gaps + self.jumps > 0
    }
}

/// A snapshot of a running recording.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct RecorderStatus {
    pub meeting_id: MeetingId,
    pub run_id: RunId,
    pub elapsed_ms: u64,
    pub issues: LiveIssues,
    pub captures: BTreeMap<String, CaptureStatus>,
    /// Utterance revisions stored so far, per channel.
    pub revisions: BTreeMap<String, u32>,
}

/// The post-pass submitted at Stop.
pub struct PostPassHandle {
    pub job_id: JobId,
    /// False when `on_issues` promoted the live run (audio work only).
    pub transcribe: bool,
    rx: Receiver<std::result::Result<PostPassReport, String>>,
}

impl PostPassHandle {
    /// Waits for the job (`None` on timeout). A job cancelled while queued
    /// never runs and reports `Err("cancelled")`.
    pub fn wait(&self, timeout: Duration) -> Option<std::result::Result<PostPassReport, String>> {
        match self.rx.recv_timeout(timeout) {
            Ok(r) => Some(r),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err("cancelled".into())),
        }
    }
}

/// What Stop did.
pub struct StopOutcome {
    pub meeting_id: MeetingId,
    pub live_run: RunId,
    pub duration_ms: i64,
    /// Samples in each (equal-length) WAV.
    pub mic_samples: u64,
    pub loopback_samples: u64,
    /// Samples each pipeline had placed on the timeline before the shorter
    /// WAV was padded to the longer one.
    pub timeline_samples: BTreeMap<String, u64>,
    pub issues: LiveIssues,
    /// Frames each channel's feed did not send to the engine.
    pub feed_dropped: BTreeMap<String, u64>,
    pub feed_reports: BTreeMap<String, FeedReport>,
    pub post_pass: PostPassHandle,
}

impl StopOutcome {
    /// What the UI needs of it (`stop_recording`).
    pub fn summary(&self) -> StopSummary {
        StopSummary {
            meeting_id: self.meeting_id,
            live_run: self.live_run,
            duration_ms: self.duration_ms,
            job_id: self.post_pass.job_id.clone(),
            transcribe: self.post_pass.transcribe,
            issues: self.issues.clone(),
        }
    }
}

enum Ev {
    Pipeline(Channel, PipelineNotice),
    Feed(Channel, FeedNotice),
    Power(PowerEvent),
    Stop,
}

struct Rig {
    channel: Channel,
    cmd: Sender<PipelineCommand>,
    pipeline: Option<JoinHandle<Result<PipelineResult>>>,
    feed: Option<FeedWorker>,
}

type Sources = Arc<Mutex<Vec<(Channel, Box<dyn PacketSource>)>>>;

/// A running recording. See the module docs.
pub struct Recorder {
    ctx: LiveContext,
    meeting: Meeting,
    run: TranscriptRun,
    folder: PathBuf,
    origin_qpc: i64,
    started: Instant,
    language: String,
    diarize_mic: bool,
    rigs: Vec<Rig>,
    sources: Sources,
    events_tx: Sender<Ev>,
    event_thread: Option<JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
    clocks: HashMap<Channel, Arc<Mutex<ClockMap>>>,
    _power: Option<PowerNotifications>,
    stopped: bool,
}

/// State the event thread shares with the recorder.
#[derive(Default)]
struct Shared {
    issues: LiveIssues,
    revisions: BTreeMap<String, u32>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `capture.autoconvert` (default true).
pub fn autoconvert_setting(s: &crate::views::Settings) -> bool {
    settings::get_bool(s, CAPTURE_AUTOCONVERT)
}

/// Whether `channel` gets automatic gain (`capture.loopback_agc`, default
/// true; `capture.mic_agc`, default false).
pub fn agc_setting(s: &crate::views::Settings, channel: Channel) -> bool {
    match channel {
        Channel::Loopback => settings::get_bool(s, CAPTURE_LOOPBACK_AGC),
        _ => settings::get_bool(s, CAPTURE_MIC_AGC),
    }
}

/// Watches the loopback's raw peaks for [`LiveWarning::LoopbackQuiet`]:
/// fires once when, for [`LOOPBACK_QUIET_WINDOW`] of channel time with
/// audio present, no raw peak reached [`LOOPBACK_QUIET_DBFS`]; re-arms once
/// a peak does.
#[derive(Debug, Default, Clone)]
pub struct QuietDetector {
    /// Channel time (ms) of the first quiet-but-active frame of the window.
    since_ms: Option<i64>,
    max_peak: f32,
    fired: bool,
}

/// Raw peaks at or under this count as no audio.
const QUIET_FLOOR_DBFS: f32 = -85.0;

impl QuietDetector {
    /// Feeds one level reading; returns the window's highest raw peak when
    /// the warning should be shown now.
    pub fn feed(&mut self, at_ms: i64, raw_peak_dbfs: f32) -> Option<f32> {
        if raw_peak_dbfs >= LOOPBACK_QUIET_DBFS {
            *self = Self::default();
            return None;
        }
        if raw_peak_dbfs <= QUIET_FLOOR_DBFS {
            // Silence neither proves nor disproves anything.
            return None;
        }
        let since = *self.since_ms.get_or_insert(at_ms);
        self.max_peak = if at_ms == since {
            raw_peak_dbfs
        } else {
            self.max_peak.max(raw_peak_dbfs)
        };
        if !self.fired && at_ms - since >= LOOPBACK_QUIET_WINDOW.as_millis() as i64 {
            self.fired = true;
            return Some(self.max_peak);
        }
        None
    }

    /// Starts over (e.g. the loopback moved to another endpoint).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Recorder {
    /// Starts a recording (see the module docs).
    pub fn start(ctx: LiveContext, opts: StartOptions, sink: Arc<dyn LiveSink>) -> Result<Self> {
        if let Some(free) = (ctx.vram_free_mb)() {
            crate::gpu::check_free_vram(free, ctx.min_free_vram_mb)?;
        }
        if let Some(free) = free_disk_bytes(&ctx.data_root) {
            if free < ctx.min_free_bytes {
                return Err(CoreError::InsufficientSpace {
                    path: ctx.data_root.display().to_string(),
                    free_mb: free >> 20,
                    needed_mb: ctx.min_free_bytes >> 20,
                });
            }
        }
        let store = Arc::clone(&ctx.store);
        let stored = settings::effective(&*store)?;
        let language = opts
            .language
            .clone()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| settings::default_language(&stored));
        let autoconvert = autoconvert_setting(&stored);
        let agc_for = |ch: Channel| agc_setting(&stored, ch).then(AgcConfig::default);
        let started_at = now_ms();
        let title = opts
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                let d = crate::export::utc_datetime(started_at);
                format!("Recording {}", d.get(..16).unwrap_or(&d))
            });
        let meeting = store.create_meeting(NewMeeting {
            title,
            source: Source::Live,
            source_path: None,
            language: language.clone(),
            diarize_mic: opts.diarize_mic,
            started_at,
            status: MeetingStatus::Recording,
        })?;
        let mut info = ctx.live_engine_info.clone();
        if !info.is_object() {
            info = serde_json::json!({});
        }
        info["language"] = serde_json::json!(language);
        info["diarize_mic"] = serde_json::json!(opts.diarize_mic);
        info["capture_autoconvert"] = serde_json::json!(autoconvert);
        info["agc"] = serde_json::json!({
            "loopback": agc_setting(&stored, Channel::Loopback),
            "mic": agc_setting(&stored, Channel::Mic),
        });
        let run = match store
            .create_run(meeting.id, RunKind::Live, info)
            .and_then(|r| store.set_current_run(meeting.id, r.id).map(|_| r))
        {
            Ok(r) => r,
            Err(e) => {
                let _ = store.delete_meeting(meeting.id);
                return Err(e.into());
            }
        };
        let folder = ctx.data_root.join("audio").join(&meeting.uid);
        let origin_qpc = qpc_now_100ns();
        ctx.queue.set_paused(true);

        let (events_tx, events_rx) = mpsc::channel::<Ev>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let sources: Sources = Arc::new(Mutex::new(Vec::new()));
        let mut clocks = HashMap::new();
        for ch in [Channel::Mic, Channel::Loopback] {
            clocks.insert(ch, Arc::new(Mutex::new(ClockMap::new(origin_qpc))));
        }
        let mut me = Self {
            ctx: ctx.clone(),
            meeting: meeting.clone(),
            run: run.clone(),
            folder: folder.clone(),
            origin_qpc,
            started: Instant::now(),
            language: language.clone(),
            diarize_mic: opts.diarize_mic,
            rigs: Vec::new(),
            sources: Arc::clone(&sources),
            events_tx: events_tx.clone(),
            event_thread: None,
            shared: Arc::clone(&shared),
            clocks: clocks.clone(),
            _power: None,
            stopped: false,
        };
        let el = EventLoop {
            store: Arc::clone(&store),
            sink,
            meeting: meeting.id,
            run: run.id,
            diarize_mic: opts.diarize_mic,
            clocks: clocks.clone(),
            sources: Arc::clone(&sources),
            cmds: Vec::new(),
            utt_base: HashMap::new(),
            max_utt: HashMap::new(),
            speakers: HashMap::new(),
            shared: Arc::clone(&shared),
            loopback_active: Instant::now(),
            mic_active: None,
            warned: false,
            quiet: QuietDetector::default(),
            loopback_device: String::new(),
        };
        let mut cmds = Vec::new();
        let result = (|| -> Result<()> {
            for ch in [Channel::Mic, Channel::Loopback] {
                let (cmd_tx, cmd_rx) = mpsc::channel();
                let (prod, cons) = rtrb::RingBuffer::new(RING_PACKETS);
                let (feed_tx, feed_rx) = feed(FEED_CAPACITY, FEED_RECOVER_BELOW);
                let tx = events_tx.clone();
                let notify = Arc::new(move |c: Channel, n: PipelineNotice| {
                    let _ = tx.send(Ev::Pipeline(c, n));
                });
                let pipeline = spawn_pipeline(
                    PipelineThreadConfig {
                        channel: ch,
                        origin_qpc,
                        wav_path: folder.join(format!("{ch}.wav")),
                        feed: Some(feed_tx),
                        store: Some(Arc::clone(&store)),
                        meeting: meeting.id,
                        clock: Arc::clone(&clocks[&ch]),
                        agc: agc_for(ch),
                        notify,
                    },
                    cons,
                    cmd_rx,
                )?;
                let engine = Arc::clone(&ctx.live_engine);
                let live_opts = LiveOpts {
                    diarize: ch == Channel::Loopback || opts.diarize_mic,
                    language: language.clone(),
                };
                let job = JobId(format!("live-{}-{ch}", meeting.uid));
                let tx = events_tx.clone();
                let feed_worker = spawn_feed_worker(
                    ch,
                    feed_rx,
                    Box::new(move || engine.open(job.clone(), ch, live_opts.clone())),
                    Arc::new(move |c, n| {
                        let _ = tx.send(Ev::Feed(c, n));
                    }),
                    Duration::from_secs(5),
                );
                cmds.push((ch, cmd_tx.clone()));
                me.rigs.push(Rig {
                    channel: ch,
                    cmd: cmd_tx,
                    pipeline: Some(pipeline),
                    feed: Some(feed_worker),
                });
                let device = match ch {
                    Channel::Mic => opts.devices.mic.clone(),
                    _ => opts.devices.loopback.clone(),
                };
                let mut capture = CaptureConfig::new(ch, device, autoconvert);
                // With AGC, take float samples so quiet audio is not
                // quantized to a few bits before the gain.
                capture.float = autoconvert && agc_for(ch).is_some();
                let source = (ctx.sources)(capture, prod)?;
                sources
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((ch, source));
            }
            Ok(())
        })();
        let mut el = el;
        el.cmds = cmds;
        me.event_thread = Some(
            std::thread::Builder::new()
                .name("live-events".into())
                .spawn(move || el.run(events_rx))
                .map_err(|e| CoreError::Engine(format!("spawn live event thread: {e}")))?,
        );
        if let Err(e) = result {
            me.abort();
            let _ = store.delete_meeting(meeting.id);
            ctx.queue.set_paused(false);
            return Err(e);
        }
        let tx = events_tx;
        me._power = PowerNotifications::register(move |e| {
            let _ = tx.send(Ev::Power(e));
        });
        Ok(me)
    }

    pub fn meeting_id(&self) -> MeetingId {
        self.meeting.id
    }

    pub fn run_id(&self) -> RunId {
        self.run.id
    }

    pub fn meeting(&self) -> &Meeting {
        &self.meeting
    }

    /// `audio/<uid>` under the data root.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// QPC (100 ns) of meeting time zero.
    pub fn origin_qpc(&self) -> i64 {
        self.origin_qpc
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Drift of a channel against QPC so far (diagnostics).
    pub fn drift_ppm(&self, channel: Channel) -> Option<f64> {
        self.clocks
            .get(&channel)
            .and_then(|c| c.lock().unwrap_or_else(|e| e.into_inner()).drift_ppm())
    }

    pub fn status(&self) -> RecorderStatus {
        let sh = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        RecorderStatus {
            meeting_id: self.meeting.id,
            run_id: self.run.id,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            issues: sh.issues.clone(),
            captures: self
                .sources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(c, s)| (c.to_string(), s.status()))
                .collect(),
            revisions: sh.revisions.clone(),
        }
    }

    /// Stops everything without finishing the meeting (it stays in
    /// `recording` for [`recover`]).
    fn abort(&mut self) {
        let sources: Vec<_> = self
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for (_, s) in sources {
            s.stop();
        }
        let now = qpc_now_100ns();
        for rig in &mut self.rigs {
            let _ = rig.cmd.send(PipelineCommand::Stop { at_qpc: now });
            if let Some(p) = rig.pipeline.take() {
                if let Ok(Ok(r)) = p.join() {
                    let _ = r.wav.finalize();
                }
            }
            if let Some(f) = rig.feed.take() {
                f.abort();
            }
        }
        let _ = self.events_tx.send(Ev::Stop);
        if let Some(t) = self.event_thread.take() {
            let _ = t.join();
        }
        self.stopped = true;
    }

    /// Stops the recording, finishes the live run and submits the post-pass.
    pub fn stop(mut self) -> Result<StopOutcome> {
        let store = Arc::clone(&self.ctx.store);
        let stop_qpc = qpc_now_100ns();
        // The queue resumes when this returns, however it returns: after the
        // post-pass was submitted ahead of imports queued meanwhile.
        let _resume = ResumeQueue(Arc::clone(&self.ctx.queue));
        // 1. Captures.
        let sources: Vec<_> = self
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for (_, s) in sources {
            s.stop();
        }
        // 2. Pipelines: drain, fill to the stop time, hand back the WAVs.
        let mut results: Vec<(Channel, PipelineResult)> = Vec::new();
        let mut first_err = None;
        for rig in &mut self.rigs {
            let _ = rig.cmd.send(PipelineCommand::Stop { at_qpc: stop_qpc });
            if let Some(p) = rig.pipeline.take() {
                match p.join() {
                    Ok(Ok(r)) => results.push((rig.channel, r)),
                    Ok(Err(e)) => first_err = first_err.or(Some(e)),
                    Err(_) => {
                        first_err =
                            first_err.or(Some(CoreError::Engine("pipeline thread panicked".into())))
                    }
                }
            }
        }
        // 3. Same length for both WAVs, then finalize.
        let len = results
            .iter()
            .map(|(_, r)| r.wav.samples())
            .max()
            .unwrap_or(0);
        let mut samples: BTreeMap<Channel, u64> = BTreeMap::new();
        let mut feed_dropped = BTreeMap::new();
        let mut timeline_samples = BTreeMap::new();
        let mut tail = Vec::new();
        for (ch, mut r) in results {
            let before = r.wav.samples();
            timeline_samples.insert(ch.to_string(), before);
            r.wav.pad_to(len)?;
            if len > before {
                let end = r.pipeline.end_qpc() + (len - before) as i64 * 625;
                tail.push(ClockAnchor {
                    meeting_id: self.meeting.id,
                    channel: ch,
                    sample_index: len as i64,
                    qpc_100ns: end,
                });
            }
            feed_dropped.insert(ch.to_string(), r.feed_dropped);
            samples.insert(ch, r.wav.finalize()?);
        }
        if !tail.is_empty() {
            store.add_clock_anchors(&tail)?;
        }
        // 4. Sessions close (commit, last finals), then the event thread.
        let mut feed_reports = BTreeMap::new();
        for rig in &mut self.rigs {
            if let Some(f) = rig.feed.take() {
                feed_reports.insert(rig.channel.to_string(), f.join());
            }
        }
        let _ = self.events_tx.send(Ev::Stop);
        if let Some(t) = self.event_thread.take() {
            let _ = t.join();
        }
        self.stopped = true;
        let issues = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issues
            .clone();
        if let Some(e) = first_err {
            return Err(e);
        }
        // 5. The live run becomes a readable transcript.
        let duration_ms = (len * 1_000 / 16_000) as i64;
        let ended_at = self.meeting.started_at + duration_ms;
        store.finish_meeting(self.meeting.id, ended_at, duration_ms)?;
        store.set_meeting_status(self.meeting.id, MeetingStatus::Processing)?;
        store.materialise_live_run(self.run.id, &SegmentOptions::default())?;
        store.finish_run(self.run.id, RunStatus::Done)?;
        store.set_current_run(self.meeting.id, self.run.id)?;
        register_wavs(
            &*store,
            self.meeting.id,
            &self.ctx.data_root,
            &self.folder,
            duration_ms,
        )?;
        // 6. The post-pass goes to the front of the (still paused) queue.
        let stored = settings::effective(&*store)?;
        let on_issues = settings::get(&stored, settings::POST_PASS_MODE).trim() == "on_issues";
        let transcribe = !on_issues || issues.any();
        let job = PostPassJob {
            job_id: JobId::new(),
            meeting: self.meeting.id,
            live_run: self.run.id,
            data_root: self.ctx.data_root.clone(),
            folder: self.folder.clone(),
            language: self.language.clone(),
            diarize_mic: self.diarize_mic,
            transcribe,
            keep_live_runs: settings::get_bool(&stored, settings::KEEP_LIVE_RUNS),
            segment: SegmentOptions::default(),
        };
        let post_pass = submit_post_pass(&self.ctx, job, true);
        Ok(StopOutcome {
            meeting_id: self.meeting.id,
            live_run: self.run.id,
            duration_ms,
            mic_samples: samples.get(&Channel::Mic).copied().unwrap_or(0),
            loopback_samples: samples.get(&Channel::Loopback).copied().unwrap_or(0),
            timeline_samples,
            issues,
            feed_dropped,
            feed_reports,
            post_pass,
        })
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if !self.stopped {
            self.abort();
            self.ctx.queue.set_paused(false);
        }
    }
}

/// Unpauses the queue when dropped.
struct ResumeQueue(Arc<JobQueue>);

impl Drop for ResumeQueue {
    fn drop(&mut self) {
        self.0.set_paused(false);
    }
}

/// Registers the per-channel WAVs (playable until the FLAC replaces them).
fn register_wavs(
    store: &dyn RunStore,
    meeting: MeetingId,
    data_root: &Path,
    folder: &Path,
    duration_ms: i64,
) -> Result<()> {
    let existing = store.list_audio_tracks(meeting)?;
    for (ch, tc) in [
        (Channel::Mic, TrackChannel::Mic),
        (Channel::Loopback, TrackChannel::Loopback),
    ] {
        if existing.iter().any(|t| t.channel == tc) {
            continue;
        }
        let path = folder.join(format!("{ch}.wav"));
        let rel = path
            .strip_prefix(data_root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.display().to_string());
        store.add_audio_track(NewAudioTrack {
            meeting_id: meeting,
            channel: tc,
            path: rel,
            format: AudioFormat::Wav,
            sample_rate: 16_000,
            channels: 1,
            duration_ms: Some(duration_ms),
            size_bytes: std::fs::metadata(&path).ok().map(|m| m.len() as i64),
        })?;
    }
    Ok(())
}

/// Submits the post-pass to the queue (with `front`, ahead of jobs queued
/// with a plain submit); the handle receives its result.
pub(crate) fn submit_post_pass(ctx: &LiveContext, job: PostPassJob, front: bool) -> PostPassHandle {
    let (tx, rx) = mpsc::channel();
    let store = Arc::clone(&ctx.store);
    let engine = Arc::clone(&ctx.file_engine);
    let on_event = ctx.on_job_event.clone();
    let job_id = job.job_id.clone();
    let transcribe = job.transcribe;
    let run = move |token: &crate::pipeline::CancelToken| {
        let emit = |e: JobEvent| {
            if let Some(f) = &on_event {
                f(e);
            }
        };
        let result = run_post_pass(&*store, &*engine, &job, token, &emit);
        match &result {
            Ok(_) => emit(JobEvent::Done {
                job_id: job.job_id.clone(),
                meeting_id: job.meeting,
            }),
            Err(CoreError::Cancelled) => {
                // The live transcript stays; the WAVs stay for a later run.
                let _ = store.set_meeting_status(job.meeting, MeetingStatus::Done);
                emit(JobEvent::Cancelled {
                    job_id: job.job_id.clone(),
                    meeting_id: Some(job.meeting),
                });
            }
            Err(e) => {
                let _ = store.set_meeting_status(job.meeting, MeetingStatus::Failed);
                emit(JobEvent::Error {
                    job_id: job.job_id.clone(),
                    meeting_id: Some(job.meeting),
                    message: e.to_string(),
                });
            }
        }
        let _ = tx.send(result.map_err(|e| e.to_string()));
    };
    if front {
        ctx.queue.submit_front(job_id.clone(), run);
    } else {
        ctx.queue.submit(job_id.clone(), run);
    }
    PostPassHandle {
        job_id,
        transcribe,
        rx,
    }
}

// ---- the event thread ------------------------------------------------------------------

struct EventLoop {
    store: Arc<dyn RunStore>,
    sink: Arc<dyn LiveSink>,
    meeting: MeetingId,
    run: RunId,
    diarize_mic: bool,
    clocks: HashMap<Channel, Arc<Mutex<ClockMap>>>,
    sources: Sources,
    cmds: Vec<(Channel, Sender<PipelineCommand>)>,
    /// Added to a session's `utt` so a reopened session continues numbering.
    utt_base: HashMap<Channel, u32>,
    max_utt: HashMap<Channel, u32>,
    speakers: HashMap<(Channel, String), SpeakerId>,
    shared: Arc<Mutex<Shared>>,
    loopback_active: Instant,
    mic_active: Option<Instant>,
    warned: bool,
    quiet: QuietDetector,
    loopback_device: String,
}

/// Level above which a channel counts as active.
const ACTIVE_DBFS: f32 = -50.0;

impl EventLoop {
    fn run(mut self, rx: Receiver<Ev>) {
        let _awake = KeepAwake::new();
        while let Ok(ev) = rx.recv() {
            match ev {
                Ev::Stop => break,
                Ev::Pipeline(ch, n) => self.on_pipeline(ch, n),
                Ev::Feed(ch, n) => self.on_feed(ch, n),
                Ev::Power(PowerEvent::Suspend) => {
                    for (_, c) in &self.cmds {
                        let _ = c.send(PipelineCommand::Flush);
                    }
                }
                Ev::Power(PowerEvent::Resume) => {
                    for (_, s) in self
                        .sources
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                    {
                        s.reopen();
                    }
                }
            }
        }
        // Drain what is left (late finals after Stop was queued behind them).
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Ev::Feed(ch, n) => self.on_feed(ch, n),
                Ev::Pipeline(ch, n) => self.on_pipeline(ch, n),
                _ => {}
            }
        }
    }

    fn issues(&self, f: impl FnOnce(&mut LiveIssues)) {
        f(&mut self.shared.lock().unwrap_or_else(|e| e.into_inner()).issues);
    }

    fn meeting_us(&self, ch: Channel, us: u64) -> u64 {
        self.clocks
            .get(&ch)
            .map(|c| c.lock().unwrap_or_else(|e| e.into_inner()).meeting_us(us))
            .unwrap_or(us)
    }

    fn speaker(&mut self, ch: Channel, label: &str) -> Option<SpeakerId> {
        if let Some(id) = self.speakers.get(&(ch, label.to_owned())) {
            return Some(*id);
        }
        let id = self.store.live_speaker(self.run, ch, label).ok()?.id;
        self.speakers.insert((ch, label.to_owned()), id);
        Some(id)
    }

    fn on_feed(&mut self, ch: Channel, n: FeedNotice) {
        match n {
            FeedNotice::Engine(EngineEvent::Utterance {
                utt, rev, words, ..
            }) => {
                let utt = utt + self.utt_base.get(&ch).copied().unwrap_or(0);
                let m = self.max_utt.entry(ch).or_insert(0);
                *m = (*m).max(utt);
                let words: Vec<EngineWord> = words
                    .into_iter()
                    .map(|w| EngineWord {
                        t0_us: self.meeting_us(ch, w.t0_us),
                        t1_us: self.meeting_us(ch, w.t1_us),
                        spk: if ch == Channel::Mic && !self.diarize_mic {
                            None
                        } else {
                            w.spk
                        },
                        ..w
                    })
                    .collect();
                if let Err(e) = self
                    .store
                    .append_live_utterance(self.run, ch, utt, rev, &words)
                {
                    self.sink.warning(&LiveWarning::Capture {
                        channel: ch,
                        message: format!("live utterance not stored: {e}"),
                    });
                }
                *self
                    .shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .revisions
                    .entry(ch.to_string())
                    .or_insert(0) += 1;
                let default = if ch == Channel::Mic {
                    ME_LABEL
                } else {
                    UNKNOWN_LABEL
                };
                let live: Vec<LiveWord> = words
                    .iter()
                    .map(|w| {
                        let label = w.spk.clone().unwrap_or_else(|| default.to_owned());
                        LiveWord {
                            start_ms: (w.t0_us / 1_000) as i64,
                            end_ms: (w.t1_us / 1_000) as i64,
                            text: w.text.clone(),
                            speaker_id: self.speaker(ch, &label),
                            label,
                        }
                    })
                    .collect();
                self.sink.words(&WordsUpdate {
                    meeting_id: self.meeting,
                    run_id: self.run,
                    channel: ch,
                    utt,
                    rev,
                    words: live,
                });
            }
            FeedNotice::Engine(EngineEvent::TextFinalUntil { t_us, .. }) => {
                let until = (self.meeting_us(ch, t_us) / 1_000) as i64;
                self.sink.text_final(ch, until);
            }
            FeedNotice::Engine(EngineEvent::Relabel {
                from_us,
                to_us,
                map,
                ..
            }) => self.sink.relabel(&RelabelUpdate {
                channel: ch,
                from_ms: (self.meeting_us(ch, from_us) / 1_000) as i64,
                to_ms: (self.meeting_us(ch, to_us) / 1_000) as i64,
                map,
            }),
            FeedNotice::Engine(_) => {}
            FeedNotice::Down { reason } => {
                self.issues(|i| i.engine_down += 1);
                self.sink.degraded(&DegradedUpdate {
                    channel: ch,
                    degraded: true,
                    reason: format!("live transcription stopped: {reason}"),
                });
            }
            FeedNotice::Up { restarts } => {
                if restarts > 0 {
                    self.issues(|i| i.restarts += 1);
                }
                // A new session numbers utterances from 0 again.
                let next = self.max_utt.get(&ch).map_or(0, |m| m + 1);
                self.utt_base.insert(ch, next);
                self.sink.degraded(&DegradedUpdate {
                    channel: ch,
                    degraded: false,
                    reason: String::new(),
                });
            }
        }
    }

    fn on_pipeline(&mut self, ch: Channel, n: PipelineNotice) {
        match n {
            PipelineNotice::Level {
                at_sample,
                level,
                gain_db,
                raw_peak_dbfs,
            } => self.on_level(ch, at_sample, level, gain_db, raw_peak_dbfs),
            PipelineNotice::Event(PipelineEvent::Gap { samples, .. }) => {
                self.issues(|i| {
                    i.gaps += 1;
                    i.gap_ms += samples / 16;
                });
            }
            PipelineNotice::Event(PipelineEvent::Jump { .. }) => self.issues(|i| i.jumps += 1),
            PipelineNotice::Event(PipelineEvent::DeviceChanged {
                at_sample,
                device_id,
                name,
                reason,
            }) => {
                if reason != ChangeReason::Initial {
                    self.issues(|i| i.device_changes += 1);
                }
                if ch == Channel::Loopback {
                    self.loopback_device = name.clone();
                    self.quiet.reset();
                }
                self.sink.device_changed(&DeviceChange {
                    channel: ch,
                    device_id,
                    name,
                    reason,
                    at_ms: (at_sample / 16) as i64,
                });
            }
            PipelineNotice::Event(PipelineEvent::DeviceLost { error, .. }) => {
                self.sink.warning(&LiveWarning::Capture {
                    channel: ch,
                    message: error,
                });
            }
            PipelineNotice::Event(PipelineEvent::Overlap { .. }) => {}
            PipelineNotice::Feed(FeedTransition::Degraded) => {
                self.issues(|i| i.degraded += 1);
                self.sink.degraded(&DegradedUpdate {
                    channel: ch,
                    degraded: true,
                    reason: "the engine fell behind; recording only".into(),
                });
            }
            PipelineNotice::Feed(FeedTransition::Recovered) => {
                self.sink.degraded(&DegradedUpdate {
                    channel: ch,
                    degraded: false,
                    reason: String::new(),
                });
            }
            PipelineNotice::Error(message) => {
                self.sink.warning(&LiveWarning::Capture {
                    channel: ch,
                    message,
                });
            }
        }
    }

    fn on_level(
        &mut self,
        ch: Channel,
        at_sample: u64,
        level: Level,
        gain_db: f32,
        raw_peak_dbfs: f32,
    ) {
        let at_ms = (at_sample / 16) as i64;
        self.sink.levels(&LevelsUpdate {
            channel: ch,
            at_ms,
            rms_dbfs: level.rms_dbfs,
            peak_dbfs: level.peak_dbfs,
            gain_db,
            raw_peak_dbfs,
        });
        if ch == Channel::Loopback {
            if let Some(peak) = self.quiet.feed(at_ms, raw_peak_dbfs) {
                self.sink.warning(&LiveWarning::LoopbackQuiet {
                    device: self.loopback_device.clone(),
                    peak_dbfs: peak,
                    gain_db,
                });
            }
        }
        let active = level.peak_dbfs > ACTIVE_DBFS;
        let now = Instant::now();
        match ch {
            Channel::Loopback if active => {
                self.loopback_active = now;
                self.warned = false;
            }
            Channel::Mic if active => self.mic_active = Some(now),
            _ => {}
        }
        let silent_for = now.duration_since(self.loopback_active);
        let mic_recent = self
            .mic_active
            .is_some_and(|t| now.duration_since(t) < LOOPBACK_SILENT_WARNING);
        if !self.warned && silent_for >= LOOPBACK_SILENT_WARNING && mic_recent {
            self.warned = true;
            self.sink.warning(&LiveWarning::LoopbackSilent {
                seconds: silent_for.as_secs() as u32,
            });
        }
    }
}

#[cfg(test)]
mod tests;
