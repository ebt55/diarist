//! Live recording in the desktop app (Phase 3b): the [`LiveSink`] that
//! forwards a recording's output to the WebView, the one recorder slot, the
//! long-lived `nemo-speech serve` behind it, crash recovery at launch and
//! the bookkeeping of queued post-passes.
//!
//! Events (payloads are the `meet_core::live` types serialized as they are,
//! except where noted; TypeScript mirrors them in `src/lib/live.ts`):
//!
//! - `live:words` ([`WordsUpdate`]): a later `rev` of the same channel and
//!   utterance replaces the earlier one.
//! - `live:text_final` ([`TextFinal`]): text of a channel before `until_ms`
//!   is final.
//! - `live:levels` ([`LevelsPayload`]): both channels, at most 10 Hz.
//! - `live:degraded` ([`DegradedUpdate`]), `live:warning` (`LiveWarning`,
//!   tagged by `kind`), `live:device_changed` ([`DeviceChange`]).
//! - `live:state` ([`LiveStateView`]): phase changes, post-passes queued or
//!   finished, warnings.
//! - `live:toggled` ([`Toggled`]): the global shortcut or the tray started
//!   or stopped a recording (the UI shows a toast).
//!
//! Post-pass progress arrives as the ordinary `job:*` events.
//!
//! The recorder runs on its own threads (`meet_core::live::Recorder`); the
//! commands here only start and stop it on the blocking pool. Starting
//! pauses the process-wide `JobQueue` (the recorder does it), so imports
//! queued meanwhile wait until Stop.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use meet_core::audio::capture::{self, find_device, DeviceFlow, DeviceInfo as AudioDevice};
use meet_core::engine::{NemoWsLiveEngine, ServeConfig, ServeStatus, ServeSupervisor};
use meet_core::live::{
    self, DegradedUpdate, DeviceChange, DeviceSelection, LevelsUpdate, LiveContext, LiveIssues,
    LiveSink, LiveWarning, PostPassJob, Recorder, RecorderStatus, StartOptions, WordsUpdate,
    MIN_FREE_BYTES, MIN_FREE_VRAM_MB,
};
use meet_core::models::{self, DataPaths, ResolvedSelection, SelectedEngine};
use meet_core::pipeline::JobQueue;
use meet_core::settings;
use meet_core::store::RunStore;
use meet_core::transcript::SegmentOptions;
use meet_core::types::{Channel, JobId, MeetingId, MeetingStatus, RunId, RunKind, Source};
use meet_core::views::JobEvent;
use meet_core::CoreError;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::emit_job;
use crate::{tray, AppPaths, AppStore, GpuCache, Jobs};

/// Where live sessions and post-passes run.
const DEVICE: &str = "cuda:0";
/// Setting: the microphone endpoint id for new recordings ("" = default).
pub const MIC_DEVICE: &str = "record.mic_device";
/// Setting: the render endpoint id whose loopback is recorded ("" = default).
pub const LOOPBACK_DEVICE: &str = "record.loopback_device";
/// Setting: diarize the mic by default (several people in the room).
pub const DIARIZE_MIC: &str = "record.diarize_mic";
/// The global shortcut that toggles recording.
pub const SHORTCUT: &str = "Ctrl+Alt+R";
/// A doctor reading (free VRAM) younger than this is reused at Start.
const VRAM_READING_TTL: Duration = Duration::from_secs(15);
/// `live:levels` is emitted at most this often.
const LEVELS_EVERY: Duration = Duration::from_millis(100);
/// Warnings kept for `live:state` (the newest ones).
const MAX_WARNINGS: usize = 20;

pub const EV_WORDS: &str = "live:words";
pub const EV_TEXT_FINAL: &str = "live:text_final";
pub const EV_LEVELS: &str = "live:levels";
pub const EV_DEGRADED: &str = "live:degraded";
pub const EV_WARNING: &str = "live:warning";
pub const EV_DEVICE_CHANGED: &str = "live:device_changed";
pub const EV_STATE: &str = "live:state";
pub const EV_TOGGLED: &str = "live:toggled";

type CmdResult<T> = Result<T, String>;

// ---- payloads ------------------------------------------------------------------------

/// Where the one recorder slot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Starting,
    Recording,
    Stopping,
}

/// A post-pass the queue holds or runs.
#[derive(Debug, Clone, Serialize)]
pub struct PostPassView {
    pub job_id: JobId,
    /// `None` until crash recovery has reported which meeting it is for.
    pub meeting_id: Option<MeetingId>,
    pub title: String,
}

/// Payload of `live:state`.
#[derive(Debug, Clone, Serialize)]
pub struct LiveStateView {
    pub phase: Phase,
    pub meeting_id: Option<MeetingId>,
    pub run_id: Option<RunId>,
    pub title: Option<String>,
    /// Unix ms of meeting time zero.
    pub started_at: Option<i64>,
    pub elapsed_ms: u64,
    /// The endpoint each channel records from ("mic", "loopback").
    pub devices: BTreeMap<String, DeviceChange>,
    /// Channels whose live transcription is paused (record-only).
    pub degraded: BTreeMap<String, DegradedUpdate>,
    /// Warnings of this recording, oldest first (`LiveWarning` as JSON).
    pub warnings: Vec<serde_json::Value>,
    pub post_passes: Vec<PostPassView>,
    /// Why the last start or stop failed.
    pub error: Option<String>,
}

/// Payload of `live:text_final`.
#[derive(Debug, Clone, Serialize)]
pub struct TextFinal {
    pub channel: Channel,
    pub until_ms: i64,
}

/// One channel's meter reading in `live:levels`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LevelOut {
    pub at_ms: i64,
    pub rms_dbfs: f32,
    /// Highest peak since the previous `live:levels`.
    pub peak_dbfs: f32,
    /// Gain the automatic gain control applied (0 on a channel without it).
    pub gain_db: Option<f32>,
}

/// Payload of `live:levels`: channels without a reading since the last
/// event are `null`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LevelsPayload {
    pub mic: Option<LevelOut>,
    pub loopback: Option<LevelOut>,
}

/// Payload of `live:toggled`.
#[derive(Debug, Clone, Serialize)]
pub struct Toggled {
    /// `started`, `stopped` or `failed`.
    pub action: &'static str,
    /// `shortcut` or `tray`.
    pub via: &'static str,
    pub meeting_id: Option<MeetingId>,
    pub message: Option<String>,
}

/// What `start_recording` takes from the start sheet.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StartRequest {
    pub title: Option<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub diarize_mic: bool,
    /// Capture endpoint id; `None` = the default communications microphone.
    pub mic: Option<String>,
    /// Render endpoint id; `None` = the default output, following the app
    /// that plays.
    pub loopback: Option<String>,
}

/// What `stop_recording` returns.
#[derive(Debug, Clone, Serialize)]
pub struct StopView {
    pub meeting_id: MeetingId,
    pub live_run: RunId,
    pub duration_ms: i64,
    pub job_id: JobId,
    /// False when `post_pass_mode = on_issues` kept the live run.
    pub transcribe: bool,
    pub issues: LiveIssues,
}

/// The live engine server for Diagnostics.
#[derive(Debug, Clone, Serialize)]
pub struct ServeView {
    pub status: ServeStatus,
    /// Since the server last became ready.
    pub uptime_ms: Option<u64>,
    pub restarts: u32,
    pub pid: Option<u32>,
    /// VRAM the idle server holds (free before launch minus free once ready).
    pub resident_vram_mb: Option<u32>,
    pub asr_model: String,
    pub diar_model: String,
    pub log_path: Option<String>,
}

/// What `recording_status` returns.
#[derive(Debug, Clone, Serialize)]
pub struct LiveStatusView {
    pub state: LiveStateView,
    pub recorder: Option<RecorderStatus>,
    pub server: Option<ServeView>,
    pub queue_running: Option<JobId>,
    pub queue_waiting: Vec<JobId>,
    pub shortcut: String,
    pub shortcut_error: Option<String>,
}

/// One word of `live_snapshot` (the same shape as `live:words`).
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub label: String,
    pub speaker_id: Option<meet_core::types::SpeakerId>,
}

/// One utterance of `live_snapshot` (the same shape as `live:words`).
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotUtterance {
    pub meeting_id: MeetingId,
    pub run_id: RunId,
    pub channel: Channel,
    pub utt: u32,
    pub rev: u32,
    pub words: Vec<SnapshotWord>,
}

// ---- state -----------------------------------------------------------------------------

/// What the sink learns while recording (kept for `live:state`).
#[derive(Default)]
struct Notes {
    devices: BTreeMap<String, DeviceChange>,
    degraded: BTreeMap<String, DegradedUpdate>,
    warnings: Vec<serde_json::Value>,
}

struct Session {
    meeting_id: MeetingId,
    run_id: RunId,
    title: String,
    started_at: i64,
    started: Instant,
    notes: Arc<Mutex<Notes>>,
}

struct Slot {
    phase: Phase,
    recorder: Option<Recorder>,
    session: Option<Session>,
    error: Option<String>,
}

/// The paths a server was launched with; another selection needs another server.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServeKey {
    engine_dir: PathBuf,
    asr: PathBuf,
    diar: PathBuf,
}

struct Serve {
    key: ServeKey,
    supervisor: ServeSupervisor,
    multilingual: bool,
    resident_mb: Option<u32>,
    ready_since: Option<Instant>,
    log_path: Option<PathBuf>,
}

/// The recorder slot, the live server and the post-pass bookkeeping.
pub(crate) struct LiveState {
    slot: Mutex<Slot>,
    serve: Mutex<Option<Serve>>,
    post_passes: Mutex<BTreeMap<String, PostPassView>>,
    recovered: Mutex<bool>,
    recovered_cv: Condvar,
    shortcut_error: Mutex<Option<String>>,
}

/// Managed handle (cheap to clone into threads).
#[derive(Clone)]
pub(crate) struct LiveHandle(pub(crate) Arc<LiveState>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl LiveState {
    pub(crate) fn new() -> Self {
        Self {
            slot: Mutex::new(Slot {
                phase: Phase::Idle,
                recorder: None,
                session: None,
                error: None,
            }),
            serve: Mutex::new(None),
            post_passes: Mutex::new(BTreeMap::new()),
            recovered: Mutex::new(false),
            recovered_cv: Condvar::new(),
            shortcut_error: Mutex::new(None),
        }
    }

    pub(crate) fn phase(&self) -> Phase {
        lock(&self.slot).phase
    }

    pub(crate) fn is_recording(&self) -> bool {
        matches!(
            self.phase(),
            Phase::Recording | Phase::Starting | Phase::Stopping
        )
    }

    pub(crate) fn set_shortcut_error(&self, e: Option<String>) {
        *lock(&self.shortcut_error) = e;
    }

    fn set_recovered(&self) {
        *lock(&self.recovered) = true;
        self.recovered_cv.notify_all();
    }

    /// Waits until crash recovery has run (it must finish before a recorder starts).
    fn wait_recovered(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut done = lock(&self.recovered);
        while !*done {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            done = self
                .recovered_cv
                .wait_timeout(done, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }

    fn view(&self) -> LiveStateView {
        let slot = lock(&self.slot);
        let mut v = LiveStateView {
            phase: slot.phase,
            meeting_id: None,
            run_id: None,
            title: None,
            started_at: None,
            elapsed_ms: 0,
            devices: BTreeMap::new(),
            degraded: BTreeMap::new(),
            warnings: Vec::new(),
            post_passes: lock(&self.post_passes).values().cloned().collect(),
            error: slot.error.clone(),
        };
        if let Some(s) = &slot.session {
            v.meeting_id = Some(s.meeting_id);
            v.run_id = Some(s.run_id);
            v.title = Some(s.title.clone());
            v.started_at = Some(s.started_at);
            v.elapsed_ms = s.started.elapsed().as_millis() as u64;
            let n = lock(&s.notes);
            v.devices = n.devices.clone();
            v.degraded = n.degraded.clone();
            v.warnings = n.warnings.clone();
        }
        v
    }

    /// Notes a post-pass job (merging what is already known about it).
    fn register_post_pass(&self, job_id: &JobId, meeting: Option<MeetingId>, title: &str) {
        let mut pp = lock(&self.post_passes);
        let e = pp.entry(job_id.0.clone()).or_insert_with(|| PostPassView {
            job_id: job_id.clone(),
            meeting_id: None,
            title: String::new(),
        });
        if meeting.is_some() {
            e.meeting_id = meeting;
        }
        if !title.is_empty() {
            e.title = title.to_owned();
        }
    }

    /// Drops the recorder without finishing the meeting (it stays in
    /// `recording`, so the next launch recovers it) and stops the server.
    pub(crate) fn shutdown(&self) {
        let rec = {
            let mut slot = lock(&self.slot);
            slot.session = None;
            slot.phase = Phase::Idle;
            slot.recorder.take()
        };
        if let Some(rec) = rec {
            log::warn!(
                "exiting while recording meeting {}: it is recovered at the next launch",
                rec.meeting_id()
            );
            drop(rec);
        }
        if let Some(s) = lock(&self.serve).take() {
            s.supervisor.stop();
        }
    }
}

pub(crate) fn handle(app: &AppHandle) -> Arc<LiveState> {
    Arc::clone(&app.state::<LiveHandle>().0)
}

/// Emits `live:state` and updates the tray.
pub(crate) fn emit_state(app: &AppHandle) {
    let live = handle(app);
    let view = live.view();
    tray::update(app, view.phase, view.started_at);
    if let Err(e) = app.emit(EV_STATE, &view) {
        log::warn!("emit {EV_STATE} failed: {e}");
    }
}

fn emit<S: Serialize + Clone>(app: &AppHandle, name: &str, payload: S) {
    if let Err(e) = app.emit(name, payload) {
        log::warn!("emit {name} failed: {e}");
    }
}

// ---- the sink ----------------------------------------------------------------------------

#[derive(Default)]
struct LevelThrottle {
    last: Option<Instant>,
    pending: LevelsPayload,
}

/// Forwards a recording's output to the WebView (see the module docs).
struct TauriSink {
    app: AppHandle,
    notes: Arc<Mutex<Notes>>,
    levels: Mutex<LevelThrottle>,
}

impl LiveSink for TauriSink {
    fn words(&self, update: &WordsUpdate) {
        emit(&self.app, EV_WORDS, update);
    }

    fn text_final(&self, channel: Channel, until_ms: i64) {
        emit(&self.app, EV_TEXT_FINAL, TextFinal { channel, until_ms });
    }

    fn levels(&self, u: &LevelsUpdate) {
        let mut t = lock(&self.levels);
        let slot = match u.channel {
            Channel::Mic => &mut t.pending.mic,
            _ => &mut t.pending.loopback,
        };
        let peak = slot.map_or(u.peak_dbfs, |p| p.peak_dbfs.max(u.peak_dbfs));
        *slot = Some(LevelOut {
            at_ms: u.at_ms,
            rms_dbfs: u.rms_dbfs,
            peak_dbfs: peak,
            gain_db: Some(u.gain_db),
        });
        if t.last.is_none_or(|l| l.elapsed() >= LEVELS_EVERY) {
            t.last = Some(Instant::now());
            let payload = std::mem::take(&mut t.pending);
            drop(t);
            emit(&self.app, EV_LEVELS, payload);
        }
    }

    fn degraded(&self, u: &DegradedUpdate) {
        {
            let mut n = lock(&self.notes);
            if u.degraded {
                n.degraded.insert(u.channel.to_string(), u.clone());
            } else {
                n.degraded.remove(u.channel.as_str());
            }
        }
        if u.degraded {
            log::warn!("live {}: {}", u.channel, u.reason);
        } else {
            log::info!("live {}: transcription resumed", u.channel);
        }
        emit(&self.app, EV_DEGRADED, u);
    }

    fn warning(&self, w: &LiveWarning) {
        let mut v = serde_json::to_value(w).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(o) = v.as_object_mut() {
            o.insert("at".into(), serde_json::json!(now_ms()));
        }
        log::warn!("live warning: {v}");
        {
            let mut n = lock(&self.notes);
            n.warnings.push(v.clone());
            let extra = n.warnings.len().saturating_sub(MAX_WARNINGS);
            n.warnings.drain(..extra);
        }
        emit(&self.app, EV_WARNING, v);
    }

    fn device_changed(&self, d: &DeviceChange) {
        log::info!(
            "live {} capture on {} ({:?}) at {} ms",
            d.channel,
            d.name,
            d.reason,
            d.at_ms
        );
        lock(&self.notes)
            .devices
            .insert(d.channel.to_string(), d.clone());
        emit(&self.app, EV_DEVICE_CHANGED, d);
    }
}

// ---- engine, VRAM, context ----------------------------------------------------------------------

fn app_parts(app: &AppHandle) -> (Arc<dyn RunStore>, DataPaths, Arc<JobQueue>, bool) {
    let store = Arc::clone(&app.state::<AppStore>().0);
    let paths = app.state::<AppPaths>();
    let queue = Arc::clone(&app.state::<Jobs>().0);
    (store, paths.paths.clone(), queue, paths.demo)
}

fn select(paths: &DataPaths, store: &dyn RunStore) -> CmdResult<SelectedEngine> {
    let built = models::select_engine(paths, store, DEVICE).map_err(|e| e.to_string())?;
    if built.experimental {
        return Err(format!(
            "{} is not a streaming model, so it cannot transcribe live. Select a streaming \
             speech-to-text model in Settings.",
            built.selection.asr_file.display_name
        ));
    }
    Ok(built)
}

/// Free VRAM from `nemo-speech doctor`, reusing a reading younger than
/// `max_age`; also when it was taken.
fn vram_reading(app: &AppHandle, max_age: Duration) -> (Option<u32>, Instant) {
    let (store, paths, _, _) = app_parts(app);
    let cache = Arc::clone(&app.state::<GpuCache>().0);
    let mut c = lock(&cache);
    if let Some((at, report)) = c.as_ref() {
        if at.elapsed() < max_age {
            return (report.as_ref().and_then(|r| r.free_vram_mb()), *at);
        }
    }
    let report = models::selected_file(&*store, meet_core::types::ModelRole::Engine)
        .ok()
        .flatten()
        .and_then(|e| models::doctor(&paths.absolute(&e.path)).ok());
    let at = Instant::now();
    let free = report.as_ref().and_then(|r| r.free_vram_mb());
    *c = Some((at, report));
    (free, at)
}

/// The supervisor for `sel`, started (a no-op when it runs). A server for
/// another selection is stopped first. The first launch measures how much
/// VRAM the idle server holds, in the background.
fn ensure_server(app: &AppHandle, sel: &ResolvedSelection) -> (ServeSupervisor, bool) {
    let key = ServeKey {
        engine_dir: sel.engine_dir.clone(),
        asr: sel.asr.clone(),
        diar: sel.diarizer.clone(),
    };
    let live = handle(app);
    let mut serve = lock(&live.serve);
    if let Some(s) = serve.as_mut() {
        if s.key == key {
            if !matches!(s.supervisor.status(), ServeStatus::Ready { .. }) {
                // Stopped after the idle timeout (or failed): launch again.
                s.ready_since = None;
                s.supervisor.start();
                watch_ready(app, key, s.supervisor.clone(), None);
            }
            return (s.supervisor.clone(), s.multilingual);
        }
    }
    if let Some(old) = serve.take() {
        log::info!("live server: selection changed, stopping the old server");
        old.supervisor.stop();
    }
    let (_, paths, _, _) = app_parts(app);
    let mut cfg = ServeConfig::new(&sel.engine_dir, paths.models(), &sel.asr, &sel.diarizer);
    cfg.device = DEVICE.into();
    let log_path = paths.logs().join("serve.log");
    let _ = std::fs::create_dir_all(paths.logs());
    cfg.log_path = Some(log_path.clone());
    let supervisor = ServeSupervisor::new(cfg);
    let multilingual = models::is_multilingual(&sel.asr_file);
    *serve = Some(Serve {
        key: key.clone(),
        supervisor: supervisor.clone(),
        multilingual,
        resident_mb: None,
        ready_since: None,
        log_path: Some(log_path),
    });
    drop(serve);
    // Free VRAM before the models load, then once the server is ready.
    let (before, _) = vram_reading(app, Duration::from_secs(5));
    supervisor.start();
    log::info!(
        "live server starting: asr {} | diarizer {} | free VRAM before {}",
        sel.asr_file.display_name,
        sel.diarizer_file.display_name,
        before.map_or("unknown".into(), |m| format!("{m} MB"))
    );
    watch_ready(app, key, supervisor.clone(), Some(before));
    (supervisor, multilingual)
}

/// Waits (on its own thread) for the server to become ready and notes when;
/// with `before` (free VRAM before launch) also how much VRAM it holds.
fn watch_ready(
    app: &AppHandle,
    key: ServeKey,
    supervisor: ServeSupervisor,
    before: Option<Option<u32>>,
) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("serve-warmup".into())
        .spawn(move || {
            let t = Instant::now();
            match supervisor.ensure_ready(Duration::from_secs(120)) {
                Ok(ep) => {
                    let secs = t.elapsed().as_secs_f64();
                    let resident = before.map(|b| {
                        let (after, _) = vram_reading(&app, Duration::ZERO);
                        b.zip(after)
                            .map(|(b, a)| b.saturating_sub(a))
                            // doctor may not see the server's allocations: 0 is unknown.
                            .filter(|m| *m > 0)
                    });
                    log::info!(
                        "live server ready on port {} in {secs:.1} s{}",
                        ep.port,
                        match resident {
                            Some(Some(m)) => format!("; holds {m} MB of VRAM"),
                            _ => String::new(),
                        }
                    );
                    let live = handle(&app);
                    let mut serve = lock(&live.serve);
                    if let Some(s) = serve.as_mut().filter(|s| s.key == key) {
                        if let Some(r) = resident {
                            s.resident_mb = r;
                        }
                        s.ready_since = Some(Instant::now());
                    }
                }
                Err(e) => log::warn!("live server did not become ready: {e}"),
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot watch the live server: {e}");
    }
}

/// Free VRAM for the start gate, counted as if the live server were not
/// loaded yet (the gate's 2.5 GB includes it).
fn gate_vram(app: &AppHandle) -> Option<u32> {
    let (free, at) = vram_reading(app, VRAM_READING_TTL);
    let live = handle(app);
    let serve = lock(&live.serve);
    let resident = serve
        .as_ref()
        .filter(|s| s.ready_since.is_some_and(|r| r <= at))
        .and_then(|s| s.resident_mb)
        .unwrap_or(0);
    free.map(|f| f + resident)
}

/// Records post-pass events and forwards them as `job:*`. `meeting`
/// identifies the post-pass of one recording (filled after Start); crash
/// recovery passes an empty slot and registers its jobs afterwards.
fn job_event_fn(
    app: &AppHandle,
    meeting: Arc<Mutex<Option<(MeetingId, String)>>>,
) -> meet_core::live::JobEventFn {
    let app = app.clone();
    Arc::new(move |e: JobEvent| {
        let live = handle(&app);
        let job = e.job_id().clone();
        let known = lock(&live.post_passes).contains_key(&job.0);
        if e.is_terminal() {
            if lock(&live.post_passes).remove(&job.0).is_some() || known {
                emit_state(&app);
            }
        } else if !known {
            let m = lock(&meeting).clone();
            live.register_post_pass(
                &job,
                m.as_ref().map(|(id, _)| *id),
                m.as_ref().map_or("", |(_, t)| t.as_str()),
            );
            emit_state(&app);
        }
        match &e {
            JobEvent::Done { meeting_id, .. } => log::info!("post-pass done: meeting {meeting_id}"),
            JobEvent::Error {
                meeting_id,
                message,
                ..
            } => log::error!("post-pass failed: meeting {meeting_id:?}: {message}"),
            JobEvent::Cancelled { meeting_id, .. } => {
                log::info!("post-pass cancelled: meeting {meeting_id:?}")
            }
            JobEvent::Progress { .. } => {}
        }
        emit_job(&app, &e);
    })
}

fn engine_info(sel: &ResolvedSelection) -> serde_json::Value {
    serde_json::json!({
        "runtime": format!("nemo-speech {}", sel.engine_version.as_deref().unwrap_or("unknown")),
        "adapter": "nemo-ws",
        "models": {"asr": sel.asr_file.display_name, "diarizer": sel.diarizer_file.display_name},
        "device": DEVICE,
        "app": format!("meet-assist {}", meet_core::VERSION),
    })
}

// ---- start / stop ----------------------------------------------------------------------------

/// A device id that is connected, else `None` (strict: an error) for a
/// stale or unknown id.
fn resolve_device(
    devices: &[AudioDevice],
    flow: DeviceFlow,
    id: Option<String>,
    strict: bool,
) -> CmdResult<Option<String>> {
    let Some(id) = id.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    if let Some(d) = find_device(devices, flow, &id) {
        return Ok(Some(d.id));
    }
    if strict {
        Err(format!(
            "The {} device is not connected any more; pick another one.",
            if flow == DeviceFlow::Capture {
                "microphone"
            } else {
                "system audio"
            }
        ))
    } else {
        log::warn!("remembered {flow:?} device {id} is not connected; using the default");
        Ok(None)
    }
}

/// A local "Meeting 15 Oct 2026 14:05" title.
pub(crate) fn default_title() -> String {
    let fmt = time::macros::format_description!(
        "[day padding:none] [month repr:short] [year] [hour]:[minute]"
    );
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!("Meeting {}", now.format(&fmt).unwrap_or_default())
}

/// Starts a recording (blocking; run it off the main thread).
pub(crate) fn start(app: &AppHandle, req: StartRequest, strict: bool) -> CmdResult<LiveStateView> {
    let live = handle(app);
    if !live.wait_recovered(Duration::from_secs(60)) {
        return Err("Still recovering an interrupted recording; try again in a moment.".into());
    }
    {
        let mut slot = lock(&live.slot);
        if slot.phase != Phase::Idle {
            return Err("A recording is already running.".into());
        }
        slot.phase = Phase::Starting;
        slot.error = None;
    }
    emit_state(app);
    let meeting_slot: Arc<Mutex<Option<(MeetingId, String)>>> = Arc::new(Mutex::new(None));
    let notes = Arc::new(Mutex::new(Notes::default()));
    let result = (|| -> CmdResult<Recorder> {
        let (store, paths, queue, demo) = app_parts(app);
        if demo {
            return Err("Recording is not available in the demo.".into());
        }
        let built = select(&paths, &*store)?;
        let devices = capture::list_devices().map_err(|e| e.to_string())?;
        let mic = resolve_device(&devices, DeviceFlow::Capture, req.mic.clone(), strict)?;
        let loopback = resolve_device(&devices, DeviceFlow::Render, req.loopback.clone(), strict)?;
        let (supervisor, multilingual) = ensure_server(app, &built.selection);
        let free = gate_vram(app);
        let ctx = LiveContext {
            store: Arc::clone(&store),
            queue,
            data_root: paths.root.clone(),
            live_engine: Arc::new(NemoWsLiveEngine::new(supervisor, multilingual)),
            file_engine: built.engine,
            sources: capture::wasapi_source_factory(),
            vram_free_mb: Arc::new(move || free),
            min_free_vram_mb: MIN_FREE_VRAM_MB,
            min_free_bytes: MIN_FREE_BYTES,
            live_engine_info: engine_info(&built.selection),
            on_job_event: Some(job_event_fn(app, Arc::clone(&meeting_slot))),
        };
        let title = req
            .title
            .clone()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(default_title);
        let sink = Arc::new(TauriSink {
            app: app.clone(),
            notes: Arc::clone(&notes),
            levels: Mutex::new(LevelThrottle::default()),
        });
        log::info!(
            "recording start: title {title:?}, language {:?}, diarize_mic {}, mic {:?}, loopback {:?}, free VRAM {}",
            req.language,
            req.diarize_mic,
            mic,
            loopback,
            free.map_or("unknown".into(), |m| format!("{m} MB"))
        );
        Recorder::start(
            ctx,
            StartOptions {
                title: Some(title),
                language: req.language.clone().filter(|l| !l.trim().is_empty()),
                diarize_mic: req.diarize_mic,
                devices: DeviceSelection { mic, loopback },
            },
            sink,
        )
        .map_err(|e| match e {
            CoreError::Engine(m) if m.contains("GPU memory") => format!("Not enough GPU memory: {m}"),
            CoreError::InsufficientSpace {
                path,
                free_mb,
                needed_mb,
            } => format!(
                "Not enough disk space: {free_mb} MB free on {path}, recording needs {needed_mb} MB."
            ),
            other => other.to_string(),
        })
    })();
    match result {
        Ok(rec) => {
            let title = rec.meeting().title.clone();
            *lock(&meeting_slot) = Some((rec.meeting_id(), title.clone()));
            log::info!(
                "recording meeting {} ({}) into {}",
                rec.meeting_id(),
                rec.meeting().uid,
                rec.folder().display()
            );
            {
                let mut slot = lock(&live.slot);
                slot.session = Some(Session {
                    meeting_id: rec.meeting_id(),
                    run_id: rec.run_id(),
                    title,
                    started_at: rec.meeting().started_at,
                    started: Instant::now(),
                    notes,
                });
                slot.recorder = Some(rec);
                slot.phase = Phase::Recording;
            }
            emit_state(app);
            Ok(live.view())
        }
        Err(e) => {
            log::warn!("recording did not start: {e}");
            {
                let mut slot = lock(&live.slot);
                slot.phase = Phase::Idle;
                slot.error = Some(e.clone());
            }
            emit_state(app);
            Err(e)
        }
    }
}

/// Stops the recording and queues the post-pass (blocking).
pub(crate) fn stop(app: &AppHandle) -> CmdResult<StopView> {
    let live = handle(app);
    let (rec, title) = {
        let mut slot = lock(&live.slot);
        if slot.phase != Phase::Recording {
            return Err("Nothing is recording.".into());
        }
        let Some(rec) = slot.recorder.take() else {
            return Err("Nothing is recording.".into());
        };
        slot.phase = Phase::Stopping;
        let title = slot
            .session
            .as_ref()
            .map(|s| s.title.clone())
            .unwrap_or_default();
        (rec, title)
    };
    emit_state(app);
    let t = Instant::now();
    let result = rec.stop();
    let out = {
        let mut slot = lock(&live.slot);
        slot.phase = Phase::Idle;
        slot.session = None;
        match result {
            Ok(o) => {
                live.register_post_pass(&o.post_pass.job_id, Some(o.meeting_id), &title);
                slot.error = None;
                Ok(StopView {
                    meeting_id: o.meeting_id,
                    live_run: o.live_run,
                    duration_ms: o.duration_ms,
                    job_id: o.post_pass.job_id.clone(),
                    transcribe: o.post_pass.transcribe,
                    issues: o.issues.clone(),
                })
            }
            Err(e) => {
                let msg = format!("Stopping failed: {e}");
                slot.error = Some(msg.clone());
                Err(msg)
            }
        }
    };
    match &out {
        Ok(v) => log::info!(
            "recording stopped: meeting {} ({} ms) in {:.1} s; post-pass job {} ({}); issues {:?}",
            v.meeting_id,
            v.duration_ms,
            t.elapsed().as_secs_f64(),
            v.job_id,
            if v.transcribe {
                "transcribe"
            } else {
                "audio only"
            },
            v.issues
        ),
        Err(e) => log::error!("{e}"),
    }
    emit_state(app);
    out
}

/// Start with the remembered choices, or stop: the global shortcut and the
/// tray. Runs on its own thread; reports through `live:toggled`.
pub(crate) fn toggle(app: &AppHandle, via: &'static str) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("live-toggle".into())
        .spawn(move || {
            let live = handle(&app);
            let toggled = match live.phase() {
                Phase::Idle => {
                    let req = remembered_request(&app);
                    match start(&app, req, false) {
                        Ok(v) => Toggled {
                            action: "started",
                            via,
                            meeting_id: v.meeting_id,
                            message: None,
                        },
                        Err(e) => Toggled {
                            action: "failed",
                            via,
                            meeting_id: None,
                            message: Some(e),
                        },
                    }
                }
                Phase::Recording => match stop(&app) {
                    Ok(v) => Toggled {
                        action: "stopped",
                        via,
                        meeting_id: Some(v.meeting_id),
                        message: None,
                    },
                    Err(e) => Toggled {
                        action: "failed",
                        via,
                        meeting_id: None,
                        message: Some(e),
                    },
                },
                // A start or stop is under way; a second press does nothing.
                Phase::Starting | Phase::Stopping => return,
            };
            emit(&app, EV_TOGGLED, toggled);
        });
    if let Err(e) = spawned {
        log::error!("cannot toggle recording: {e}");
    }
}

/// The start sheet's choices from settings (for the shortcut and the tray).
fn remembered_request(app: &AppHandle) -> StartRequest {
    let store = Arc::clone(&app.state::<AppStore>().0);
    let s = settings::effective(&*store).unwrap_or_default();
    let get = |k: &str| {
        let v = settings::get(&s, k).trim().to_owned();
        (!v.is_empty()).then_some(v)
    };
    StartRequest {
        title: None,
        language: None,
        diarize_mic: settings::get_bool(&s, DIARIZE_MIC),
        mic: get(MIC_DEVICE),
        loopback: get(LOOPBACK_DEVICE),
    }
}

// ---- launch: recovery and warm-up ------------------------------------------------------------

/// Finishes meetings a crash left in `recording`, then starts the live
/// server in the background so the first Record is fast. Never blocks.
pub(crate) fn startup(app: &AppHandle) {
    let live = handle(app);
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("live-startup".into())
        .spawn(move || {
            let live = handle(&app);
            if let Err(e) = recover_meetings(&app) {
                log::error!("crash recovery failed: {e}");
            }
            live.set_recovered();
            let (store, paths, _, demo) = app_parts(&app);
            if demo {
                return;
            }
            match select(&paths, &*store) {
                Ok(built) => {
                    ensure_server(&app, &built.selection);
                }
                Err(e) => log::info!("live server not started: {e}"),
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start crash recovery: {e}");
        live.set_recovered();
    }
}

fn recover_meetings(app: &AppHandle) -> CmdResult<()> {
    let (store, paths, queue, demo) = app_parts(app);
    if demo {
        return Ok(());
    }
    let pending: Vec<_> = store
        .list_meetings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|m| m.status == MeetingStatus::Recording && m.source == Source::Live)
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    log::warn!(
        "recovering {} interrupted recording(s): {:?}",
        pending.len(),
        pending.iter().map(|m| m.id.0).collect::<Vec<_>>()
    );
    let built = models::select_engine(&paths, &*store, DEVICE).map_err(|e| {
        format!(
            "cannot recover {} interrupted recording(s) without an engine: {e}",
            pending.len()
        )
    })?;
    let ctx = LiveContext {
        store: Arc::clone(&store),
        queue,
        data_root: paths.root.clone(),
        // Recovery opens no live session.
        live_engine: Arc::new(NemoWsLiveEngine::with_endpoint("ws://127.0.0.1:9/", "")),
        file_engine: built.engine,
        sources: capture::wasapi_source_factory(),
        vram_free_mb: Arc::new(|| None),
        min_free_vram_mb: MIN_FREE_VRAM_MB,
        min_free_bytes: MIN_FREE_BYTES,
        live_engine_info: engine_info(&built.selection),
        on_job_event: Some(job_event_fn(app, Arc::new(Mutex::new(None)))),
    };
    let recovered = live::recover(&ctx).map_err(|e| e.to_string())?;
    let live = handle(app);
    for r in recovered {
        let title = pending
            .iter()
            .find(|m| m.id == r.meeting_id)
            .map(|m| m.title.clone())
            .unwrap_or_default();
        log::info!(
            "recovered meeting {}: mic {} / loopback {} frames, post-pass {}",
            r.meeting_id,
            r.mic_frames,
            r.loopback_frames,
            r.post_pass
                .as_ref()
                .map_or("none (no audio)".to_owned(), |p| p.job_id.to_string())
        );
        if let Some(pp) = &r.post_pass {
            live.register_post_pass(&pp.job_id, Some(r.meeting_id), &title);
        }
    }
    emit_state(app);
    Ok(())
}

// ---- retry -------------------------------------------------------------------------------------

/// Queues the post-pass of a live meeting again (after it failed or was
/// cancelled): the recorded WAVs must still be there.
// TODO(phase3b): call a core `live::rerun_post_pass` once it exists; this
// mirrors the core's private `submit_post_pass`.
pub(crate) fn requeue_post_pass(app: &AppHandle, meeting: MeetingId) -> CmdResult<JobId> {
    let live = handle(app);
    if lock(&live.slot)
        .session
        .as_ref()
        .is_some_and(|s| s.meeting_id == meeting)
    {
        return Err("This meeting is still recording.".into());
    }
    if lock(&live.post_passes)
        .values()
        .any(|p| p.meeting_id == Some(meeting))
    {
        return Err("A post-pass for this meeting is already queued.".into());
    }
    let (store, paths, queue, _) = app_parts(app);
    let detail = store.get_meeting(meeting).map_err(|e| e.to_string())?;
    let m = detail.meeting;
    if m.source != Source::Live {
        return Err("Only live recordings have a post-pass.".into());
    }
    let runs = store.list_runs(meeting).map_err(|e| e.to_string())?;
    let live_run = runs
        .iter()
        .rev()
        .find(|r| r.kind == RunKind::Live)
        .map(|r| r.id)
        .ok_or("This meeting has no live transcript to replace.")?;
    let folder = paths.audio().join(&m.uid);
    for f in ["mic.wav", "loopback.wav"] {
        if !folder.join(f).is_file() {
            return Err(format!(
                "The recorded audio ({f}) is gone, so the post-pass cannot run again. \
                 The live transcript stays."
            ));
        }
    }
    let built = models::select_engine(&paths, &*store, DEVICE).map_err(|e| e.to_string())?;
    let stored = settings::effective(&*store).map_err(|e| e.to_string())?;
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting,
        live_run,
        data_root: paths.root.clone(),
        folder,
        language: m.language.clone(),
        diarize_mic: m.diarize_mic,
        transcribe: true,
        keep_live_runs: settings::get_bool(&stored, settings::KEEP_LIVE_RUNS),
        segment: SegmentOptions::default(),
    };
    store
        .set_meeting_status(meeting, MeetingStatus::Processing)
        .map_err(|e| e.to_string())?;
    let job_id = job.job_id.clone();
    live.register_post_pass(&job_id, Some(meeting), &m.title);
    let on_event = job_event_fn(app, Arc::new(Mutex::new(Some((meeting, m.title.clone())))));
    let engine = built.engine;
    log::info!("post-pass queued again for meeting {meeting} (job {job_id})");
    queue.submit(job_id.clone(), move |token| {
        let result = live::run_post_pass(&*store, &*engine, &job, token, &|e| on_event(e));
        let terminal = match result {
            Ok(_) => JobEvent::Done {
                job_id: job.job_id.clone(),
                meeting_id: job.meeting,
            },
            Err(CoreError::Cancelled) => {
                let _ = store.set_meeting_status(job.meeting, MeetingStatus::Done);
                JobEvent::Cancelled {
                    job_id: job.job_id.clone(),
                    meeting_id: Some(job.meeting),
                }
            }
            Err(e) => {
                let _ = store.set_meeting_status(job.meeting, MeetingStatus::Failed);
                JobEvent::Error {
                    job_id: job.job_id.clone(),
                    meeting_id: Some(job.meeting),
                    message: e.to_string(),
                }
            }
        };
        on_event(terminal);
    });
    emit_state(app);
    Ok(job_id)
}

/// A post-pass was taken off the queue before it ran (`cancel_job`): the
/// live transcript stays, so the meeting is done. Returns its meeting.
pub(crate) fn dequeued_post_pass(app: &AppHandle, job: &JobId) -> Option<MeetingId> {
    let live = handle(app);
    let entry = lock(&live.post_passes).remove(&job.0)?;
    if let Some(m) = entry.meeting_id {
        let (store, _, _, _) = app_parts(app);
        if let Err(e) = store.set_meeting_status(m, MeetingStatus::Done) {
            log::warn!("meeting {m}: status after a cancelled post-pass: {e}");
        }
        log::info!("post-pass of meeting {m} cancelled before it ran; the live transcript stays");
    }
    emit_state(app);
    entry.meeting_id
}

// ---- status ----------------------------------------------------------------------------------

pub(crate) fn status(app: &AppHandle) -> LiveStatusView {
    let live = handle(app);
    let state = live.view();
    let recorder = lock(&live.slot).recorder.as_ref().map(Recorder::status);
    let server = {
        let mut serve = lock(&live.serve);
        serve.as_mut().map(|s| {
            let status = s.supervisor.status();
            match status {
                ServeStatus::Ready { .. } => {
                    s.ready_since.get_or_insert_with(Instant::now);
                }
                _ => s.ready_since = None,
            }
            ServeView {
                status,
                uptime_ms: s.ready_since.map(|t| t.elapsed().as_millis() as u64),
                restarts: s.supervisor.restarts(),
                pid: s.supervisor.pid(),
                resident_vram_mb: s.resident_mb,
                asr_model: file_name(&s.key.asr),
                diar_model: file_name(&s.key.diar),
                log_path: s.log_path.as_ref().map(|p| p.display().to_string()),
            }
        })
    };
    let queue = Arc::clone(&app.state::<Jobs>().0);
    let shortcut_error = lock(&live.shortcut_error).clone();
    LiveStatusView {
        state,
        recorder,
        server,
        queue_running: queue.running(),
        queue_waiting: queue.queued(),
        shortcut: SHORTCUT.into(),
        shortcut_error,
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The newest revision of every utterance of the recording's live run, in
/// `live:words` shape (for a view that opens after words arrived).
pub(crate) fn snapshot(app: &AppHandle, meeting: MeetingId) -> CmdResult<Vec<SnapshotUtterance>> {
    let (store, _, _, _) = app_parts(app);
    let runs = store.list_runs(meeting).map_err(|e| e.to_string())?;
    let Some(run) = runs.iter().rev().find(|r| r.kind == RunKind::Live) else {
        return Ok(Vec::new());
    };
    let diarize_mic = store
        .get_meeting(meeting)
        .map(|d| d.meeting.diarize_mic)
        .unwrap_or(false);
    let speakers: BTreeMap<(String, String), meet_core::types::SpeakerId> = store
        .run_speakers(run.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| ((s.channel.to_string(), s.engine_label), s.id))
        .collect();
    let mut latest: BTreeMap<(String, u32), meet_core::types::LiveUtterance> = BTreeMap::new();
    for u in store
        .list_live_utterances(run.id)
        .map_err(|e| e.to_string())?
    {
        let k = (u.channel.to_string(), u.utt);
        if latest.get(&k).is_none_or(|old| u.rev >= old.rev) {
            latest.insert(k, u);
        }
    }
    Ok(latest
        .into_values()
        .map(|u| {
            let ch = u.channel.to_string();
            let words = u
                .words
                .iter()
                .map(|w| {
                    let label = if u.channel == Channel::Mic && !diarize_mic {
                        meet_core::transcript::ME_LABEL.to_owned()
                    } else {
                        w.spk.clone().unwrap_or_else(|| {
                            if u.channel == Channel::Mic {
                                meet_core::transcript::ME_LABEL.to_owned()
                            } else {
                                meet_core::transcript::UNKNOWN_LABEL.to_owned()
                            }
                        })
                    };
                    SnapshotWord {
                        start_ms: (w.t0_us / 1_000) as i64,
                        end_ms: (w.t1_us / 1_000) as i64,
                        text: w.text.clone(),
                        speaker_id: speakers.get(&(ch.clone(), label.clone())).copied(),
                        label,
                    }
                })
                .collect();
            SnapshotUtterance {
                meeting_id: meeting,
                run_id: run.id,
                channel: u.channel,
                utt: u.utt,
                rev: u.rev,
                words,
            }
        })
        .collect())
}

// ---- commands ----------------------------------------------------------------------------------

async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Active capture and render endpoints (render first).
#[tauri::command]
pub async fn list_audio_devices() -> CmdResult<Vec<AudioDevice>> {
    blocking(|| capture::list_devices().map_err(|e| e.to_string())).await
}

/// The start sheet opened: start the live server if it is not running and
/// refresh the free-VRAM reading, so Start is quick.
#[tauri::command]
pub async fn prepare_recording(app: AppHandle) -> CmdResult<()> {
    blocking(move || {
        let (store, paths, _, demo) = app_parts(&app);
        if demo || handle(&app).is_recording() {
            return Ok(());
        }
        let built = select(&paths, &*store)?;
        ensure_server(&app, &built.selection);
        vram_reading(&app, VRAM_READING_TTL);
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_recording(app: AppHandle, opts: StartRequest) -> CmdResult<LiveStateView> {
    blocking(move || start(&app, opts, true)).await
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> CmdResult<StopView> {
    blocking(move || stop(&app)).await
}

#[tauri::command]
pub async fn recording_status(app: AppHandle) -> CmdResult<LiveStatusView> {
    blocking(move || Ok(status(&app))).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn live_snapshot(
    app: AppHandle,
    meeting_id: MeetingId,
) -> CmdResult<Vec<SnapshotUtterance>> {
    blocking(move || snapshot(&app, meeting_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rerun_post_pass(app: AppHandle, meeting_id: MeetingId) -> CmdResult<JobId> {
    blocking(move || requeue_post_pass(&app, meeting_id)).await
}
