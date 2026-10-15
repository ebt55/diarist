//! WASAPI side of [`super`]: device listing, the capture thread, endpoint
//! following and the `play_wav` helper. Every function that touches COM
//! runs on a thread that initialized the multi-threaded apartment.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, Device, DeviceEnumerator,
    DeviceEventCallbacks, Direction, Role, SampleType, SessionState, StreamMode, WasapiError,
    WaveFormat,
};

use super::{
    audio_err, choose_loopback_endpoint, qpc_now_100ns, CaptureConfig, CaptureStatus, ChangeReason,
    Control, DeviceFlow, DeviceInfo, EndpointActivity, EndpointFollower, Packet, PacketData,
    PacketFlags, PacketSource, SharedCaptureState, CAPTURE_RATE, FOLLOW_INTERVAL_MS,
};
use crate::error::Result;
use crate::types::Channel;

/// `AUDCLNT_E_DEVICE_INVALIDATED`.
const DEVICE_INVALIDATED: u32 = 0x8889_0004;
/// Shared-mode buffer we ask for (200 ms): the poll loop runs every 10 ms.
const BUFFER_HNS: i64 = 2_000_000;
/// Poll interval of the capture loop.
const POLL: Duration = Duration::from_millis(10);
/// Synthesize silence after this long without a packet...
const IDLE_HNS: i64 = 2_000_000;
/// ...up to this far behind now, so a late real packet never overlaps it.
const SYNTH_LAG_HNS: i64 = 1_000_000;
/// A longer idle span means the thread did not run (suspend): no silence
/// is synthesized for it.
const MAX_SYNTH_HNS: i64 = 100_000_000;

fn werr(context: &str, e: WasapiError) -> crate::error::CoreError {
    audio_err(format!("{context}: {e}"))
}

fn is_invalidated(e: &WasapiError) -> bool {
    matches!(e, WasapiError::Windows(w) if w.code().0 as u32 == DEVICE_INVALIDATED)
}

/// Runs `f` on a fresh MTA thread (COM may already be STA on the caller's).
fn on_mta_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    std::thread::Builder::new()
        .name("wasapi-call".into())
        .spawn(move || {
            let _ = initialize_mta();
            f()
        })
        .map_err(|e| audio_err(format!("spawn: {e}")))?
        .join()
        .map_err(|_| audio_err("audio thread panicked"))?
}

fn device_info(d: &Device, flow: DeviceFlow, default: &str, comms: &str) -> Option<DeviceInfo> {
    let id = d.get_id().ok()?;
    let name = d.get_friendlyname().unwrap_or_else(|_| id.clone());
    Some(DeviceInfo {
        is_default: id == default,
        is_default_comms: id == comms,
        id,
        name,
        flow,
    })
}

pub(super) fn list_devices() -> Result<Vec<DeviceInfo>> {
    on_mta_thread(|| {
        let en = DeviceEnumerator::new().map_err(|e| werr("device enumerator", e))?;
        let mut out = Vec::new();
        for (dir, flow) in [
            (Direction::Render, DeviceFlow::Render),
            (Direction::Capture, DeviceFlow::Capture),
        ] {
            let default_of = |role: Role| {
                en.get_default_device_for_role(&dir, &role)
                    .and_then(|d| d.get_id())
                    .unwrap_or_default()
            };
            let default = default_of(Role::Console);
            let comms = default_of(Role::Communications);
            let coll = en
                .get_device_collection(&dir)
                .map_err(|e| werr("device list", e))?;
            let n = coll
                .get_nbr_devices()
                .map_err(|e| werr("device count", e))?;
            for i in 0..n {
                if let Ok(d) = coll.get_device_at_index(i) {
                    out.extend(device_info(&d, flow, &default, &comms));
                }
            }
        }
        Ok(out)
    })
}

/// Foreign playback per active render endpoint.
fn render_activity(en: &DeviceEnumerator, own_pid: u32) -> Vec<EndpointActivity> {
    let Ok(coll) = en.get_device_collection(&Direction::Render) else {
        return Vec::new();
    };
    let n = coll.get_nbr_devices().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..n {
        let Ok(dev) = coll.get_device_at_index(i) else {
            continue;
        };
        let Ok(id) = dev.get_id() else { continue };
        let mut act = EndpointActivity {
            device_id: id,
            foreign_sessions: 0,
            peak: 0.0,
        };
        if let Ok(sessions) = dev
            .get_iaudiosessionmanager()
            .and_then(|m| m.get_audiosessionenumerator())
        {
            for s in 0..sessions.get_count().unwrap_or(0) {
                let Ok(ctl) = sessions.get_session(s) else {
                    continue;
                };
                if !matches!(ctl.get_state(), Ok(SessionState::Active)) {
                    continue;
                }
                let pid = ctl.get_process_id().unwrap_or(0);
                if pid == 0 || pid == own_pid {
                    continue;
                }
                act.foreign_sessions += 1;
                if let Ok(p) = ctl
                    .get_audiometerinformation()
                    .and_then(|m| m.get_peak_value())
                {
                    act.peak = act.peak.max(p);
                }
            }
        }
        out.push(act);
    }
    out
}

/// How samples arrive from the opened client.
#[derive(Debug, Clone, Copy)]
enum Layout {
    /// 16 kHz mono PCM16 (autoconvert).
    Pcm16,
    /// Mix format.
    Mix {
        rate: u32,
        channels: u16,
        float: bool,
        bits: u16,
    },
}

impl Layout {
    fn rate(&self) -> u32 {
        match self {
            Layout::Pcm16 => CAPTURE_RATE,
            Layout::Mix { rate, .. } => *rate,
        }
    }

    fn block_align(&self) -> usize {
        match self {
            Layout::Pcm16 => 2,
            Layout::Mix { channels, bits, .. } => usize::from(*channels) * usize::from(*bits / 8),
        }
    }

    fn data(&self, bytes: &[u8]) -> PacketData {
        match *self {
            Layout::Pcm16 => PacketData::Pcm16(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes(*b))
                    .collect(),
            ),
            Layout::Mix {
                rate,
                channels,
                float,
                bits,
            } => {
                let samples: Vec<f32> = match (float, bits) {
                    (true, 32) => bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect(),
                    (false, 16) => bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32_768.0)
                        .collect(),
                    (false, 32) => bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0)
                        .collect(),
                    (false, 24) => bytes
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|b| {
                            (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
                        })
                        .collect(),
                    _ => vec![0.0; bytes.len() / usize::from((bits / 8).max(1))],
                };
                PacketData::Float {
                    rate,
                    channels,
                    samples,
                }
            }
        }
    }

    fn silence(&self, frames: usize) -> PacketData {
        match *self {
            Layout::Pcm16 => PacketData::Pcm16(vec![0; frames]),
            Layout::Mix { rate, channels, .. } => PacketData::Float {
                rate,
                channels,
                samples: vec![0.0; frames * usize::from(channels)],
            },
        }
    }
}

/// An opened stream.
struct Stream {
    client: AudioClient,
    capture: AudioCaptureClient,
    layout: Layout,
    id: String,
    name: String,
    buf: Vec<u8>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}

fn resolve_device(
    en: &DeviceEnumerator,
    channel: Channel,
    id: Option<&str>,
) -> std::result::Result<Device, WasapiError> {
    match (id, channel) {
        (Some(id), _) => en.get_device(id),
        (None, Channel::Loopback) => {
            en.get_default_device_for_role(&Direction::Render, &Role::Console)
        }
        (None, _) => en.get_default_device_for_role(&Direction::Capture, &Role::Communications),
    }
}

fn open_stream(
    en: &DeviceEnumerator,
    cfg: &CaptureConfig,
    id: Option<&str>,
) -> std::result::Result<Stream, WasapiError> {
    let device = resolve_device(en, cfg.channel, id)?;
    let dev_id = device.get_id()?;
    let name = device.get_friendlyname().unwrap_or_else(|_| dev_id.clone());
    let mut client = device.get_iaudioclient()?;
    let (format, layout, autoconvert) = if cfg.autoconvert {
        (
            WaveFormat::new(16, 16, &SampleType::Int, CAPTURE_RATE as usize, 1, None),
            Layout::Pcm16,
            true,
        )
    } else {
        let mix = client.get_mixformat()?;
        let layout = Layout::Mix {
            rate: mix.get_samplespersec(),
            channels: mix.get_nchannels(),
            float: matches!(mix.get_subformat(), Ok(SampleType::Float)),
            bits: mix.get_bitspersample(),
        };
        (mix, layout, false)
    };
    // Loopback: initializing a render device for capture sets AUDCLNT_STREAMFLAGS_LOOPBACK.
    client.initialize_client(
        &format,
        &Direction::Capture,
        &StreamMode::PollingShared {
            autoconvert,
            buffer_duration_hns: BUFFER_HNS,
        },
    )?;
    let frames = client.get_buffer_size()? as usize;
    let capture = client.get_audiocaptureclient()?;
    client.start_stream()?;
    Ok(Stream {
        buf: vec![0u8; frames.max(4_800) * layout.block_align()],
        client,
        capture,
        layout,
        id: dev_id,
        name,
    })
}

/// Writes packets into the ring and synthesizes silence when idle.
struct Emitter {
    ring: rtrb::Producer<Packet>,
    shared: Arc<SharedCaptureState>,
    /// QPC just after the last emitted audio sample.
    last_end: i64,
    layout: Layout,
}

impl Emitter {
    fn push(&mut self, p: Packet) {
        if self.ring.push(p).is_err() {
            self.shared.overflow.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn control(&mut self, control: Control) {
        self.push(Packet::Control {
            qpc_100ns: qpc_now_100ns(),
            control,
        });
    }

    fn audio(&mut self, qpc: i64, flags: PacketFlags, data: PacketData) {
        let frames = data.frames() as i64;
        let rate = i64::from(data.rate().max(1));
        self.last_end = qpc + frames * 10_000_000 / rate;
        self.push(Packet::Audio {
            qpc_100ns: qpc,
            flags,
            data,
        });
    }

    /// Emits silence from `last_end` to `now - SYNTH_LAG` when nothing has
    /// arrived for `IDLE_HNS`.
    fn synthesize_if_idle(&mut self) {
        let now = qpc_now_100ns();
        if now - self.last_end < IDLE_HNS {
            return;
        }
        let span = now - SYNTH_LAG_HNS - self.last_end;
        if span > MAX_SYNTH_HNS {
            self.last_end = now - SYNTH_LAG_HNS;
            self.control(Control::Resumed { gap_hns: span });
            return;
        }
        let rate = i64::from(self.layout.rate());
        let frames = span * rate / 10_000_000;
        if frames <= 0 {
            return;
        }
        let data = self.layout.silence(frames as usize);
        let flags = PacketFlags {
            silent: true,
            synthetic: true,
            ..Default::default()
        };
        let qpc = self.last_end;
        self.audio(qpc, flags, data);
    }

    /// Sleeps `d` in poll steps, synthesizing silence, unless stopped.
    fn idle(&mut self, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until && !self.shared.stop.load(Ordering::SeqCst) {
            self.synthesize_if_idle();
            std::thread::sleep(POLL);
        }
    }
}

enum LoopEnd {
    Stop,
    Reopen(ChangeReason),
    Follow(String),
    Lost(WasapiError),
}

/// The running capture thread's handle.
pub(super) struct WasapiCapture {
    shared: Arc<SharedCaptureState>,
    thread: Option<JoinHandle<()>>,
}

impl WasapiCapture {
    pub fn start(cfg: CaptureConfig, ring: rtrb::Producer<Packet>) -> Result<Self> {
        let shared = Arc::new(SharedCaptureState::default());
        let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();
        let thread_shared = Arc::clone(&shared);
        let name = format!("capture-{}", cfg.channel);
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || capture_thread(cfg, ring, thread_shared, ready_tx))
            .map_err(|e| audio_err(format!("spawn capture thread: {e}")))?;
        let mut me = Self {
            shared,
            thread: Some(thread),
        };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(me),
            Ok(Err(e)) => {
                me.join();
                Err(audio_err(e))
            }
            Err(_) => {
                me.join();
                Err(audio_err("capture did not start within 10 s"))
            }
        }
    }

    fn join(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl PacketSource for WasapiCapture {
    fn reopen(&self) {
        self.shared.reopen.store(true, Ordering::SeqCst);
    }

    fn status(&self) -> CaptureStatus {
        self.shared.status()
    }

    fn stop(mut self: Box<Self>) {
        self.join();
    }
}

impl Drop for WasapiCapture {
    fn drop(&mut self) {
        self.join();
    }
}

fn capture_thread(
    cfg: CaptureConfig,
    ring: rtrb::Producer<Packet>,
    shared: Arc<SharedCaptureState>,
    ready: mpsc::Sender<std::result::Result<(), String>>,
) {
    let _ = initialize_mta();
    let en = match DeviceEnumerator::new() {
        Ok(e) => e,
        Err(e) => {
            let _ = ready.send(Err(format!("device enumerator: {e}")));
            return;
        }
    };
    // Default-device changes matter only when following the default.
    let default_changed = Arc::new(AtomicBool::new(false));
    let _registration = if cfg.device.is_none() {
        let flag = Arc::clone(&default_changed);
        let (want_dir, want_role) = if cfg.channel == Channel::Loopback {
            (Direction::Render, Role::Console)
        } else {
            (Direction::Capture, Role::Communications)
        };
        let mut cb = DeviceEventCallbacks::new();
        cb.set_default_device_callback(move |dir, role, _id| {
            if dir == want_dir && role == want_role {
                flag.store(true, Ordering::SeqCst);
            }
        });
        en.register_notification_callback(cb).ok()
    } else {
        None
    };
    let mut em = Emitter {
        ring,
        shared: Arc::clone(&shared),
        last_end: qpc_now_100ns(),
        layout: if cfg.autoconvert {
            Layout::Pcm16
        } else {
            Layout::Mix {
                rate: 48_000,
                channels: 2,
                float: true,
                bits: 32,
            }
        },
    };
    let own_pid = std::process::id();
    let mut ready = Some(ready);
    let mut reason = ChangeReason::Initial;
    // A followed endpoint overrides the default until the default changes.
    let mut target: Option<String> = cfg.device.clone();
    let mut opens = 0u32;
    while !shared.stop.load(Ordering::SeqCst) {
        match open_stream(&en, &cfg, target.as_deref()) {
            Ok(mut stream) => {
                if let Some(r) = ready.take() {
                    let _ = r.send(Ok(()));
                }
                em.layout = stream.layout;
                em.control(Control::DeviceChanged {
                    device_id: stream.id.clone(),
                    name: stream.name.clone(),
                    reason,
                });
                shared.update(|s| {
                    s.device_id = Some(stream.id.clone());
                    s.device_name = Some(stream.name.clone());
                    s.reopens = opens;
                });
                opens += 1;
                match stream_loop(&cfg, &en, &mut stream, &mut em, &default_changed, own_pid) {
                    LoopEnd::Stop => break,
                    LoopEnd::Reopen(r) => {
                        if r == ChangeReason::DefaultChanged {
                            target = cfg.device.clone();
                        }
                        reason = r;
                    }
                    LoopEnd::Follow(id) => {
                        target = Some(id);
                        reason = ChangeReason::Followed;
                    }
                    LoopEnd::Lost(e) => {
                        drop(stream);
                        let msg = format!("{e}");
                        shared.update(|s| s.last_error = Some(msg.clone()));
                        em.control(Control::DeviceLost { error: msg });
                        reason = ChangeReason::Invalidated;
                        if !is_invalidated(&e) {
                            em.idle(Duration::from_millis(500));
                        }
                        // A followed or vanished endpoint: go back to the configured one.
                        target = cfg.device.clone();
                    }
                }
            }
            Err(e) => {
                let msg = format!("open {} capture: {e}", cfg.channel);
                if let Some(r) = ready.take() {
                    let _ = r.send(Err(msg));
                    return;
                }
                shared.update(|s| s.last_error = Some(msg.clone()));
                em.control(Control::DeviceLost { error: msg });
                reason = ChangeReason::Invalidated;
                target = cfg.device.clone();
                em.idle(Duration::from_secs(1));
            }
        }
    }
}

fn stream_loop(
    cfg: &CaptureConfig,
    en: &DeviceEnumerator,
    stream: &mut Stream,
    em: &mut Emitter,
    default_changed: &AtomicBool,
    own_pid: u32,
) -> LoopEnd {
    let mut follower = EndpointFollower::default();
    let mut last_follow = Instant::now();
    let block = stream.layout.block_align();
    loop {
        if em.shared.stop.load(Ordering::SeqCst) {
            return LoopEnd::Stop;
        }
        if em.shared.reopen.swap(false, Ordering::SeqCst) {
            return LoopEnd::Reopen(ChangeReason::Resumed);
        }
        if default_changed.swap(false, Ordering::SeqCst) {
            return LoopEnd::Reopen(ChangeReason::DefaultChanged);
        }
        if cfg.follow_foreign && last_follow.elapsed() >= Duration::from_millis(FOLLOW_INTERVAL_MS)
        {
            last_follow = Instant::now();
            let default = en
                .get_default_device_for_role(&Direction::Render, &Role::Console)
                .and_then(|d| d.get_id())
                .unwrap_or_default();
            let chosen =
                choose_loopback_endpoint(&stream.id, &default, &render_activity(en, own_pid));
            if let Some(id) = follower.check(&stream.id, &chosen) {
                return LoopEnd::Follow(id);
            }
        }
        loop {
            let n = match stream.capture.get_next_packet_size() {
                Ok(Some(n)) => n as usize,
                Ok(None) => 0,
                Err(e) => return LoopEnd::Lost(e),
            };
            if n == 0 {
                break;
            }
            if stream.buf.len() < n * block {
                stream.buf.resize(n * block, 0);
            }
            match stream.capture.read_from_device(&mut stream.buf) {
                Ok((frames, info)) => {
                    if frames == 0 {
                        break;
                    }
                    let flags = PacketFlags {
                        silent: info.flags.silent,
                        discontinuity: info.flags.data_discontinuity,
                        timestamp_error: info.flags.timestamp_error,
                        synthetic: false,
                    };
                    let qpc = if flags.timestamp_error || info.timestamp == 0 {
                        em.last_end
                    } else {
                        info.timestamp as i64
                    };
                    let data = stream.layout.data(&stream.buf[..frames as usize * block]);
                    em.audio(qpc, flags, data);
                }
                Err(e) => return LoopEnd::Lost(e),
            }
        }
        em.synthesize_if_idle();
        std::thread::sleep(POLL);
    }
}

// ---- playback helper ---------------------------------------------------------------

fn find_render_device(en: &DeviceEnumerator, query: Option<&str>) -> Result<Device> {
    let Some(q) = query else {
        return en
            .get_default_device_for_role(&Direction::Render, &Role::Console)
            .map_err(|e| werr("default render device", e));
    };
    if let Ok(d) = en.get_device(q) {
        return Ok(d);
    }
    let coll = en
        .get_device_collection(&Direction::Render)
        .map_err(|e| werr("device list", e))?;
    let mut infos = Vec::new();
    for i in 0..coll.get_nbr_devices().unwrap_or(0) {
        if let Ok(d) = coll.get_device_at_index(i) {
            infos.extend(device_info(&d, DeviceFlow::Render, "", ""));
        }
    }
    let hit = super::find_device(&infos, DeviceFlow::Render, q)
        .ok_or_else(|| audio_err(format!("no render device matches {q:?}")))?;
    en.get_device(&hit.id)
        .map_err(|e| werr("open render device", e))
}

pub(super) fn play_wav(device: Option<&str>, wav: &Path, stop: &AtomicBool) -> Result<()> {
    let mut reader = hound::WavReader::open(wav)
        .map_err(|e| audio_err(format!("open {}: {e}", wav.display())))?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(audio_err("play_wav needs a 16-bit PCM WAV"));
    }
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| audio_err(format!("read {}: {e}", wav.display())))?;
    let device = device.map(str::to_owned);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop_flag);
    let handle = std::thread::Builder::new()
        .name("play-wav".into())
        .spawn(move || -> Result<()> {
            let _ = initialize_mta();
            let en = DeviceEnumerator::new().map_err(|e| werr("device enumerator", e))?;
            let dev = find_render_device(&en, device.as_deref())?;
            let mut client = dev
                .get_iaudioclient()
                .map_err(|e| werr("audio client", e))?;
            let ch = usize::from(spec.channels);
            let format = WaveFormat::new(
                16,
                16,
                &SampleType::Int,
                spec.sample_rate as usize,
                ch,
                None,
            );
            client
                .initialize_client(
                    &format,
                    &Direction::Render,
                    &StreamMode::EventsShared {
                        autoconvert: true,
                        buffer_duration_hns: 1_000_000,
                    },
                )
                .map_err(|e| werr("initialize render", e))?;
            let event = client.set_get_eventhandle().map_err(|e| werr("event", e))?;
            let render = client
                .get_audiorenderclient()
                .map_err(|e| werr("render client", e))?;
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            let frame_bytes = 2 * ch;
            let total_frames = bytes.len() / frame_bytes;
            let mut pos = 0usize;
            let mut started = false;
            while pos < total_frames && !thread_stop.load(Ordering::SeqCst) {
                let space = client
                    .get_available_space_in_frames()
                    .map_err(|e| werr("buffer space", e))? as usize;
                let n = space.min(total_frames - pos);
                if n > 0 {
                    render
                        .write_to_device(
                            n,
                            &bytes[pos * frame_bytes..(pos + n) * frame_bytes],
                            None,
                        )
                        .map_err(|e| werr("write", e))?;
                    pos += n;
                }
                if !started {
                    client.start_stream().map_err(|e| werr("start", e))?;
                    started = true;
                }
                let _ = event.wait_for_event(1_000);
            }
            // Let the buffered tail play out.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && !thread_stop.load(Ordering::SeqCst) {
                match client.get_current_padding() {
                    Ok(0) | Err(_) => break,
                    Ok(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            let _ = client.stop_stream();
            Ok(())
        })
        .map_err(|e| audio_err(format!("spawn: {e}")))?;
    while !handle.is_finished() {
        if stop.load(Ordering::SeqCst) {
            stop_flag.store(true, Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    handle
        .join()
        .map_err(|_| audio_err("playback thread panicked"))?
}
