//! Live capture (`docs/01`, "Audio capture"; plan 04 chunk 3a).
//!
//! Two capture threads per recording, one per [`Channel`]: the microphone
//! ([`Channel::Mic`], default communications capture endpoint) and the
//! render-endpoint loopback ([`Channel::Loopback`], default render endpoint).
//! Both open WASAPI in shared mode at 16 kHz mono PCM16 with
//! `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, so the published format never
//! changes across endpoint switches; with `capture.autoconvert = false` they
//! open at the mix format instead and the pipeline resamples
//! ([`super::pipeline`]). They poll every 10 ms; when no packet has arrived
//! for 200 ms (loopback while nothing renders, a lost device) they
//! synthesize a silent packet so the timeline keeps moving.
//!
//! The threads do nothing but copy: every packet (QPC timestamp in 100 ns,
//! SILENT / discontinuity / timestamp-error flags, samples) goes into an
//! `rtrb` ring that the channel's pipeline drains. Device events become
//! [`Control`] packets in the same ring, so the pipeline records a gap
//! anchor at the right place: the first open, a reopen after
//! `AUDCLNT_E_DEVICE_INVALIDATED`, a default-device change, a resume after
//! suspend, and (loopback) a switch to the endpoint a foreign process is
//! actually rendering to ([`choose_loopback_endpoint`], re-checked every 2 s,
//! switched after two confirmations).
//!
//! [`PacketSource`] abstracts a running capture so the recorder can be
//! tested with synthetic sources.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::types::Channel;

#[cfg(windows)]
mod wasapi_impl;

/// An audio device error (reported as I/O: the device is the "file").
pub(crate) fn audio_err(msg: impl Into<String>) -> CoreError {
    CoreError::Io(std::io::Error::other(msg.into()))
}

/// Sample rate of everything after capture.
pub const CAPTURE_RATE: u32 = 16_000;
/// 100 ns units per 16 kHz sample.
pub const HNS_PER_SAMPLE: i64 = 625;
/// How often the loopback capture re-checks which endpoint foreign
/// processes render to.
pub const FOLLOW_INTERVAL_MS: u64 = 2_000;
/// Consecutive checks that must agree before the loopback switches.
pub const FOLLOW_CONFIRMATIONS: u32 = 2;

/// Direction of an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFlow {
    /// Speakers, headsets, virtual cables' inputs: loopback sources.
    Render,
    /// Microphones.
    Capture,
}

/// An active audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Endpoint id (stable across reboots), used by `--mic` / `--loopback`.
    pub id: String,
    /// Friendly name, e.g. `CABLE Output (VB-Audio Virtual Cable)`.
    pub name: String,
    pub flow: DeviceFlow,
    /// Default endpoint for the console role.
    pub is_default: bool,
    /// Default endpoint for the communications role.
    pub is_default_comms: bool,
}

/// Active endpoints of both directions (render first).
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    #[cfg(windows)]
    {
        wasapi_impl::list_devices()
    }
    #[cfg(not(windows))]
    {
        Err(audio_err("audio capture is only implemented on Windows"))
    }
}

/// Finds an endpoint of `flow` by exact id, then exact name, then a
/// case-insensitive name prefix or substring (`"CABLE Output"` matches
/// `"CABLE Output (VB-Audio Virtual Cable)"`).
pub fn find_device(devices: &[DeviceInfo], flow: DeviceFlow, query: &str) -> Option<DeviceInfo> {
    let q = query.trim();
    let ql = q.to_lowercase();
    let of_flow = || devices.iter().filter(|d| d.flow == flow);
    of_flow()
        .find(|d| d.id == q)
        .or_else(|| of_flow().find(|d| d.name.eq_ignore_ascii_case(q)))
        .or_else(|| of_flow().find(|d| d.name.to_lowercase().starts_with(&ql)))
        .or_else(|| of_flow().find(|d| d.name.to_lowercase().contains(&ql)))
        .cloned()
}

/// Flags of one capture packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PacketFlags {
    /// `AUDCLNT_BUFFERFLAGS_SILENT`: treat the samples as zeros.
    pub silent: bool,
    /// `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`: a glitch before this packet.
    pub discontinuity: bool,
    /// `AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR`: the timestamp is unreliable.
    pub timestamp_error: bool,
    /// Made up by the capture thread because nothing arrived (always silent).
    pub synthetic: bool,
}

/// Samples of one packet.
#[derive(Debug, Clone, PartialEq)]
pub enum PacketData {
    /// 16 kHz mono PCM16 (autoconvert on, the normal case).
    Pcm16(Vec<i16>),
    /// The endpoint's mix format (autoconvert off): interleaved f32.
    Float {
        rate: u32,
        channels: u16,
        samples: Vec<f32>,
    },
}

impl PacketData {
    /// Frames (samples per channel) in the packet.
    pub fn frames(&self) -> usize {
        match self {
            PacketData::Pcm16(s) => s.len(),
            PacketData::Float {
                channels, samples, ..
            } => samples.len() / usize::from((*channels).max(1)),
        }
    }

    /// Sample rate of the data.
    pub fn rate(&self) -> u32 {
        match self {
            PacketData::Pcm16(_) => CAPTURE_RATE,
            PacketData::Float { rate, .. } => *rate,
        }
    }
}

/// Why a capture (re)opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    /// First open of the recording.
    Initial,
    /// `AUDCLNT_E_DEVICE_INVALIDATED` or another stream error.
    Invalidated,
    /// The system default endpoint changed.
    DefaultChanged,
    /// A foreign process renders to another endpoint (loopback following).
    Followed,
    /// Reopened after the system resumed from suspend.
    Resumed,
    /// Reopened on request.
    Requested,
}

/// A device event in the packet stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// The capture opened on `device_id`; the next audio packet may start
    /// after a gap (the pipeline records a gap anchor).
    DeviceChanged {
        device_id: String,
        name: String,
        reason: ChangeReason,
    },
    /// The device could not be opened or read; the capture retries and
    /// synthesizes silence meanwhile.
    DeviceLost { error: String },
    /// The capture thread did not run for `gap_hns` (the system slept). The
    /// span is not zero-filled: the pipeline records a gap anchor instead.
    Resumed { gap_hns: i64 },
}

/// One item in a capture ring.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Audio {
        /// QPC time of the first frame in 100 ns units.
        qpc_100ns: i64,
        flags: PacketFlags,
        data: PacketData,
    },
    Control {
        qpc_100ns: i64,
        control: Control,
    },
}

/// Which endpoint a capture uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    pub channel: Channel,
    /// Endpoint id; `None` follows the default (mic: communications capture
    /// endpoint; loopback: console render endpoint).
    pub device: Option<String>,
    /// 16 kHz mono PCM16 via `AUTOCONVERTPCM` (setting `capture.autoconvert`).
    pub autoconvert: bool,
    /// Loopback only, and only with `device: None`: follow the endpoint a
    /// foreign process renders to.
    pub follow_foreign: bool,
    /// With `autoconvert`: 32-bit float at 16 kHz mono instead of PCM16, so
    /// quiet audio keeps its resolution until the AGC (falls back to PCM16
    /// when the endpoint refuses).
    pub float: bool,
}

impl CaptureConfig {
    pub fn new(channel: Channel, device: Option<String>, autoconvert: bool) -> Self {
        Self {
            follow_foreign: channel == Channel::Loopback && device.is_none(),
            channel,
            device,
            autoconvert,
            float: false,
        }
    }
}

/// What a running capture reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureStatus {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    /// Opens after the first one.
    pub reopens: u32,
    /// Packets dropped because the ring was full.
    pub overflow_packets: u64,
    pub last_error: Option<String>,
}

/// A running capture.
pub trait PacketSource: Send {
    /// Reopens the device (after resume). The capture emits a
    /// `DeviceChanged { reason: Resumed }` control packet.
    fn reopen(&self);
    fn status(&self) -> CaptureStatus;
    /// Stops the capture thread; packets already in the ring stay there.
    fn stop(self: Box<Self>);
}

/// Starts a capture writing into `ring`.
pub type SourceFactory =
    dyn Fn(CaptureConfig, rtrb::Producer<Packet>) -> Result<Box<dyn PacketSource>> + Send + Sync;

/// The WASAPI capture as a [`SourceFactory`].
pub fn wasapi_source_factory() -> Arc<SourceFactory> {
    Arc::new(start_capture)
}

/// Starts a WASAPI capture thread.
pub fn start_capture(
    cfg: CaptureConfig,
    ring: rtrb::Producer<Packet>,
) -> Result<Box<dyn PacketSource>> {
    #[cfg(windows)]
    {
        Ok(Box::new(wasapi_impl::WasapiCapture::start(cfg, ring)?))
    }
    #[cfg(not(windows))]
    {
        let _ = (cfg, ring);
        Err(audio_err("audio capture is only implemented on Windows"))
    }
}

/// What [`play_wav`] played.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayReport {
    pub device_name: String,
    /// Frames written.
    pub frames: u64,
    /// QPC (100 ns) at which the first frame was played: from the device
    /// clock when it reported a position, else when the stream started.
    pub start_qpc_100ns: i64,
    pub from_device_clock: bool,
}

/// Plays a 16-bit PCM WAV to the render endpoint matching `device` (id or
/// name; `None` = default) in shared mode, blocking until it has played or
/// `stop` is set. A developer helper for deterministic tests (e.g. a WAV
/// into the VB-Audio cable's render side is captured from `CABLE Output`).
pub fn play_wav(
    device: Option<&str>,
    wav: &std::path::Path,
    stop: &AtomicBool,
) -> Result<PlayReport> {
    #[cfg(windows)]
    {
        wasapi_impl::play_wav(device, wav, stop)
    }
    #[cfg(not(windows))]
    {
        let _ = (device, wav, stop);
        Err(audio_err("playback is only implemented on Windows"))
    }
}

/// QueryPerformanceCounter now, in 100 ns units (the timebase of WASAPI
/// packet timestamps).
pub fn qpc_now_100ns() -> i64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::Performance::{
            QueryPerformanceCounter, QueryPerformanceFrequency,
        };
        let mut count = 0i64;
        let mut freq = 0i64;
        // SAFETY: both calls only write the given i64s; they cannot fail on XP and later.
        unsafe {
            let _ = QueryPerformanceCounter(&mut count);
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        if freq <= 0 {
            return 0;
        }
        (i128::from(count) * 10_000_000 / i128::from(freq)) as i64
    }
    #[cfg(not(windows))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        let start = START.get_or_init(Instant::now);
        (start.elapsed().as_nanos() / 100) as i64
    }
}

/// Foreign playback seen on one render endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointActivity {
    pub device_id: String,
    /// Active sessions of other processes (not ours, not the system's pid 0).
    pub foreign_sessions: u32,
    /// Highest session peak (0..1) among them.
    pub peak: f32,
}

/// The render endpoint the loopback should capture (anarlog's approach):
/// stay on `current` while a foreign process plays there; otherwise prefer
/// the default endpoint if a foreign process plays there; otherwise the
/// endpoint with the loudest foreign session; with no foreign playback
/// anywhere, stay where we are.
pub fn choose_loopback_endpoint(
    current: &str,
    default: &str,
    activity: &[EndpointActivity],
) -> String {
    let playing = |id: &str| {
        activity
            .iter()
            .any(|a| a.device_id == id && a.foreign_sessions > 0)
    };
    if playing(current) {
        return current.to_owned();
    }
    if playing(default) {
        return default.to_owned();
    }
    activity
        .iter()
        .filter(|a| a.foreign_sessions > 0)
        .fold(None::<&EndpointActivity>, |best, a| match best {
            Some(b) if b.peak >= a.peak => Some(b),
            _ => Some(a),
        })
        .map(|a| a.device_id.clone())
        .unwrap_or_else(|| current.to_owned())
}

/// Debounces [`choose_loopback_endpoint`]: a switch is reported only after
/// [`FOLLOW_CONFIRMATIONS`] consecutive checks chose the same other endpoint.
#[derive(Debug, Default, Clone)]
pub struct EndpointFollower {
    candidate: Option<String>,
    confirmations: u32,
}

impl EndpointFollower {
    /// Feeds one check; returns the endpoint to switch to, if any.
    pub fn check(&mut self, current: &str, chosen: &str) -> Option<String> {
        if chosen == current {
            self.candidate = None;
            self.confirmations = 0;
            return None;
        }
        if self.candidate.as_deref() == Some(chosen) {
            self.confirmations += 1;
        } else {
            self.candidate = Some(chosen.to_owned());
            self.confirmations = 1;
        }
        if self.confirmations >= FOLLOW_CONFIRMATIONS {
            self.candidate = None;
            self.confirmations = 0;
            Some(chosen.to_owned())
        } else {
            None
        }
    }
}

/// State shared between a capture thread and its handle.
#[derive(Debug, Default)]
pub(crate) struct SharedCaptureState {
    pub stop: AtomicBool,
    pub reopen: AtomicBool,
    pub overflow: AtomicU64,
    pub status: Mutex<CaptureStatus>,
}

impl SharedCaptureState {
    pub fn status(&self) -> CaptureStatus {
        let mut s = self
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        s.overflow_packets = self.overflow.load(Ordering::Relaxed);
        s
    }

    pub fn update(&self, f: impl FnOnce(&mut CaptureStatus)) {
        f(&mut self.status.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(id: &str, sessions: u32, peak: f32) -> EndpointActivity {
        EndpointActivity {
            device_id: id.into(),
            foreign_sessions: sessions,
            peak,
        }
    }

    #[test]
    fn loopback_endpoint_choice() {
        // Teams plays on the headset, nothing on the default speakers: follow it.
        let a = [act("speakers", 0, 0.0), act("headset", 1, 0.2)];
        assert_eq!(
            choose_loopback_endpoint("speakers", "speakers", &a),
            "headset"
        );
        // Once there, stay while it keeps playing, even if the default plays too.
        let b = [act("speakers", 1, 0.9), act("headset", 1, 0.1)];
        assert_eq!(
            choose_loopback_endpoint("headset", "speakers", &b),
            "headset"
        );
        // Something plays on the default and on a virtual cable: the default wins.
        let c = [act("speakers", 1, 0.0), act("cable", 1, 0.8)];
        assert_eq!(choose_loopback_endpoint("old", "speakers", &c), "speakers");
        // Two other endpoints: the louder one.
        let d = [act("a", 1, 0.1), act("b", 2, 0.5)];
        assert_eq!(choose_loopback_endpoint("speakers", "speakers", &d), "b");
        // Nobody plays anywhere: stay.
        assert_eq!(
            choose_loopback_endpoint("headset", "speakers", &[]),
            "headset"
        );
    }

    #[test]
    fn follower_needs_two_confirmations() {
        let mut f = EndpointFollower::default();
        assert_eq!(f.check("speakers", "headset"), None);
        assert_eq!(f.check("speakers", "headset"), Some("headset".into()));
        // A flap resets the count.
        assert_eq!(f.check("headset", "speakers"), None);
        assert_eq!(f.check("headset", "headset"), None);
        assert_eq!(f.check("headset", "speakers"), None);
        assert_eq!(f.check("headset", "cable"), None);
        assert_eq!(f.check("headset", "cable"), Some("cable".into()));
    }

    #[test]
    fn device_lookup_by_id_and_name() {
        let devs = vec![
            DeviceInfo {
                id: "{0.0.0}.{a}".into(),
                name: "CABLE Input (VB-Audio Virtual Cable)".into(),
                flow: DeviceFlow::Render,
                is_default: false,
                is_default_comms: false,
            },
            DeviceInfo {
                id: "{0.0.1}.{b}".into(),
                name: "CABLE Output (VB-Audio Virtual Cable)".into(),
                flow: DeviceFlow::Capture,
                is_default: false,
                is_default_comms: false,
            },
        ];
        assert_eq!(
            find_device(&devs, DeviceFlow::Capture, "CABLE Output")
                .unwrap()
                .id,
            "{0.0.1}.{b}"
        );
        assert_eq!(
            find_device(&devs, DeviceFlow::Render, "{0.0.0}.{a}")
                .unwrap()
                .name,
            "CABLE Input (VB-Audio Virtual Cable)"
        );
        assert!(find_device(&devs, DeviceFlow::Render, "CABLE Output").is_none());
        assert!(find_device(&devs, DeviceFlow::Capture, "virtual cable").is_some());
    }

    #[test]
    fn qpc_moves_forward() {
        let a = qpc_now_100ns();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = qpc_now_100ns();
        assert!(b > a);
    }
}
