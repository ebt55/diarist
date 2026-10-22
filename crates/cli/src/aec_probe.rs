//! `diarist-cli aec-probe` (hidden developer subcommand): can Windows'
//! acoustic echo cancellation (`IAcousticEchoCancellationControl`, through
//! the wasapi crate) clean the mic channel when the meeting plays on
//! speakers, and what does it cost?
//!
//! The probe plays a test signal on a render endpoint and records the mic
//! the way the app opens it (shared mode, AUTOCONVERTPCM, 16 kHz mono
//! PCM16, 200 ms buffer, 10 ms polling), once without a stream category
//! (the app today) and once with a category and the render endpoint as the
//! AEC reference. Results and method: docs/research/14-echo-cancellation.md.
//! The app's capture path (`diarist_core::audio::capture`) is not touched;
//! only its public `play_wav`, `list_devices` and `qpc_now_100ns` are used.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use diarist_core::audio::capture::{self, DeviceFlow, DeviceInfo, PlayReport};
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use wasapi::{
    initialize_mta, AcousticEchoCancellationControl, AudioCaptureClient, AudioClient,
    AudioClientProperties, DeviceEnumerator, Direction, SampleType, StreamCategory, StreamMode,
    WaveFormat,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    IAudioEffectsManager, AUDIO_EFFECT, AUDIO_EFFECT_STATE_OFF, AUDIO_EFFECT_STATE_ON,
};
use windows::Win32::Media::KernelStreaming as ks;
use windows::Win32::System::Com::CLSCTX_ALL;

const RATE: u32 = 16_000;
const HNS_PER_SAMPLE: i64 = 625;
/// Same shared-mode buffer and poll interval as the app's capture.
const BUFFER_HNS: i64 = 2_000_000;
const POLL: Duration = Duration::from_millis(10);
/// Lag window searched for the mic-versus-reference cross-correlation.
const MIN_LAG_MS: i64 = -100;
const MAX_LAG_MS: i64 = 500;

/// Command-line options of `aec-probe`.
pub struct Args {
    pub out: PathBuf,
    pub seconds: u32,
    pub mic: Option<String>,
    pub render: Option<String>,
    pub category: String,
    pub wav: Option<PathBuf>,
    pub ducking: bool,
    pub double_talk: bool,
}

/// Stream category of a probe client. `App` sets no client properties at
/// all, which is what the app does (the category is then Other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cat {
    App,
    Other,
    Communications,
    Speech,
    GameChat,
    FarFieldSpeech,
    UniformSpeech,
    VoiceTyping,
}

impl Cat {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "communications" => Cat::Communications,
            "speech" => Cat::Speech,
            "other" => Cat::Other,
            "gamechat" => Cat::GameChat,
            _ => return None,
        })
    }

    fn stream(self) -> Option<StreamCategory> {
        Some(match self {
            Cat::App => return None,
            Cat::Other => StreamCategory::Other,
            Cat::Communications => StreamCategory::Communications,
            Cat::Speech => StreamCategory::Speech,
            Cat::GameChat => StreamCategory::GameChat,
            Cat::FarFieldSpeech => StreamCategory::FarFieldSpeech,
            Cat::UniformSpeech => StreamCategory::UniformSpeech,
            Cat::VoiceTyping => StreamCategory::VoiceTyping,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Cat::App => "none (app)",
            Cat::Other => "Other",
            Cat::Communications => "Communications",
            Cat::Speech => "Speech",
            Cat::GameChat => "GameChat",
            Cat::FarFieldSpeech => "FarFieldSpeech",
            Cat::UniformSpeech => "UniformSpeech",
            Cat::VoiceTyping => "VoiceTyping",
        }
    }
}

/// Prints and keeps every line for `report.txt`.
#[derive(Default)]
struct Out(String);

impl Out {
    fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

macro_rules! say {
    ($o:expr) => { $o.line("") };
    ($o:expr, $($t:tt)*) => { $o.line(format!($($t)*)) };
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub fn run(args: Args) -> Result<(), String> {
    let cat = Cat::parse(&args.category)
        .ok_or_else(|| format!("unknown --category {:?}", args.category))?;
    if args.seconds < 4 || args.seconds > 120 {
        return Err("--seconds must be 4 to 120".into());
    }
    std::fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let _ = initialize_mta();
    let mut o = Out::default();
    let devs = capture::list_devices().map_err(s)?;
    let mic = pick(&devs, DeviceFlow::Capture, args.mic.as_deref())?;
    let render = pick(&devs, DeviceFlow::Render, args.render.as_deref())?;
    let default_render = pick(&devs, DeviceFlow::Render, None)?;
    say!(o, "aec-probe {}", diarist_core::VERSION);
    say!(o, "mic:    {} ({})", mic.name, mic.id);
    say!(o, "render: {} ({})", render.name, render.id);
    say!(o, "AEC-on category: {}", cat.label());
    for d in [&mic, &render, &default_render] {
        say!(o, "endpoint volume, {}: {}", d.name, endpoint_volume(&d.id));
    }
    say!(o);

    // a. AEC support per capture endpoint and category.
    support_table(&mut o, &devs);

    // b. Reference signal.
    let ref_path = args.out.join("ref.wav");
    let reference = match &args.wav {
        Some(w) => load_ref(w)?,
        None => speech_like(args.seconds),
    };
    write_wav(&ref_path, &reference)?;
    say!(
        o,
        "reference: {} ({:.1} s, peak {:.1} dBFS, RMS {:.1} dBFS)",
        ref_path.display(),
        reference.len() as f64 / f64::from(RATE),
        peak_dbfs(&reference),
        rms_dbfs(&reference)
    );
    say!(o);

    // c. Run 1: the mic as the app opens it, no AEC.
    say!(
        o,
        "run 1: AEC off, playing the reference on {}",
        render.name
    );
    let spec_off = MicSpec {
        device_id: mic.id.clone(),
        cat: Cat::App,
        aec_ref: None,
    };
    let (take1, play1) = playback_run(&spec_off, &render.id, &ref_path, Vec::new())?;
    write_wav(&args.out.join("mic-aec-off.wav"), &take1.samples)?;
    let play_idx1 = take1.idx_of(play1.start_qpc_100ns);
    let lag1 = lag_of(&take1.samples, play_idx1, &reference, 0, reference.len());
    let lag_samples = match lag1 {
        Some((l, c)) if c.abs() >= 0.05 => l,
        _ => 0,
    };
    let st1 = PlayStats::of(&take1, play_idx1, lag_samples, reference.len());
    print_take(&mut o, "run 1", &take1, Some(&play1));
    let echo_heard = st1.during - st1.outside >= 3.0;
    if !echo_heard {
        say!(o);
        say!(
            o,
            "The mic during playback ({:.1} dBFS) is within 3 dB of the mic outside it ({:.1} dBFS): \
             the mic is probably muted (or the speakers are off). No echo to cancel, so runs 2 to 4 \
             are skipped; only the API support above is reported.",
            st1.during,
            st1.outside
        );
    }

    let mut runs: Vec<(String, Take, Option<PlayReport>)> =
        vec![("1 AEC off".into(), take1, Some(play1))];
    let mut stats = vec![Some(st1)];
    let mut controls: Option<(Take, PlayReport)> = None;
    if echo_heard {
        let spec_on = MicSpec {
            device_id: mic.id.clone(),
            cat,
            aec_ref: Some(render.id.clone()),
        };
        // d. Run 2: category plus AEC reference.
        say!(o);
        say!(o, "run 2: AEC on ({}), playing the reference", cat.label());
        let (take2, play2) = playback_run(&spec_on, &render.id, &ref_path, Vec::new())?;
        write_wav(&args.out.join("mic-aec-on.wav"), &take2.samples)?;
        print_take(&mut o, "run 2", &take2, Some(&play2));
        let idx2 = take2.idx_of(play2.start_qpc_100ns);
        let lag2 = match lag_of(&take2.samples, idx2, &reference, 0, reference.len()) {
            Some((l, c)) if c.abs() >= 0.3 => l,
            _ => lag_samples,
        };
        let aec_ctl = take2.setup.aec_control == "ok";
        stats.push(Some(PlayStats::of(&take2, idx2, lag2, reference.len())));
        runs.push((format!("2 AEC on, {}", cat.label()), take2, Some(play2)));

        // e. Run 3: AEC on, nothing playing.
        say!(o);
        say!(o, "run 3: AEC on ({}), 5 s of silence", cat.label());
        let take3 = silence_run(&spec_on, Duration::from_secs(5))?;
        write_wav(&args.out.join("mic-aec-on-silence.wav"), &take3.samples)?;
        print_take(&mut o, "run 3", &take3, None);
        stats.push(None);
        runs.push((format!("3 AEC on, {}, silence", cat.label()), take3, None));

        // Run 4: change the reference and the effect state on the running client.
        let other = devs
            .iter()
            .filter(|d| d.flow == DeviceFlow::Render && d.id != render.id)
            .max_by_key(|d| d.name.contains("CABLE Input"))
            .cloned();
        say!(o);
        say!(
            o,
            "run 4: AEC on ({}), playing the reference; at 1/4 the reference becomes {}, at 1/2 \
             it is set back to {}, at 3/4 the AEC effect is set OFF",
            cat.label(),
            other
                .as_ref()
                .map_or("(no other render endpoint)", |d| d.name.as_str()),
            render.name
        );
        let q = Duration::from_secs_f64(reference.len() as f64 / f64::from(RATE) / 4.0);
        let mut schedule = Vec::new();
        if let Some(d) = &other {
            schedule.push((q, Cmd::SetRef(Some(d.id.clone()))));
        }
        schedule.push((q * 2, Cmd::SetRef(Some(render.id.clone()))));
        schedule.push((q * 3, Cmd::AecEffect(false)));
        if aec_ctl {
            let (take4, play4) = playback_run(&spec_on, &render.id, &ref_path, schedule)?;
            write_wav(&args.out.join("mic-aec-controls.wav"), &take4.samples)?;
            print_take(&mut o, "run 4", &take4, Some(&play4));
            controls = Some((take4, play4));
        } else {
            say!(o, "  run 4: skipped, run 2 had no AEC control");
        }
    }
    let mut dt = None;
    if echo_heard && args.double_talk {
        if default_render.id == render.id {
            say!(
                o,
                "double talk: skipped (needs --render to be an endpoint that feeds the mic, \
                 such as CABLE Input, other than the default output)"
            );
        } else {
            say!(o);
            say!(
                o,
                "double talk: AEC on ({}); far end on {} (the AEC reference) and, {} ms later at \
                 -6 dB, on {} with a near-end signal added",
                cat.label(),
                default_render.name,
                DT_DELAY_MS,
                render.name
            );
            dt = Some(double_talk_run(
                &mic,
                &render,
                &default_render,
                cat,
                &reference,
                &args.out,
            )?);
        }
    }

    // f. Summary.
    say!(o);
    summary(&mut o, &runs, &stats, &reference, lag1, echo_heard);
    if let Some((take4, play4)) = &controls {
        say!(o);
        control_summary(&mut o, take4, play4, &reference, lag_samples);
    }

    if let Some(d) = &dt {
        say!(o);
        double_talk_summary(&mut o, d, lag_samples);
    }

    if args.ducking {
        say!(o);
        ducking_check(&mut o, &mic, &default_render, &args.out)?;
    }

    let report = args.out.join("report.txt");
    std::fs::write(&report, &o.0).map_err(|e| format!("{}: {e}", report.display()))?;
    println!();
    println!("report: {}", report.display());
    Ok(())
}

fn pick(devs: &[DeviceInfo], flow: DeviceFlow, query: Option<&str>) -> Result<DeviceInfo, String> {
    match query {
        Some(q) => capture::find_device(devs, flow, q)
            .ok_or_else(|| format!("no {flow:?} endpoint matches {q:?}")),
        // The app's defaults: the communications mic and the console output.
        None => devs
            .iter()
            .find(|d| {
                d.flow == flow
                    && if flow == DeviceFlow::Capture {
                        d.is_default_comms
                    } else {
                        d.is_default
                    }
            })
            .cloned()
            .ok_or_else(|| format!("no default {flow:?} endpoint")),
    }
}

// ---- WASAPI -------------------------------------------------------------------

/// Master volume and mute of an endpoint as Windows reports them (a
/// hardware mute button on the mic is not visible here).
fn endpoint_volume(id: &str) -> String {
    let run = || -> Result<String, String> {
        let en = DeviceEnumerator::new().map_err(s)?;
        let dev = en.get_device(id).map_err(s)?;
        // SAFETY: activating a documented endpoint service on a live IMMDevice
        // on this (MTA) thread, then plain getters.
        unsafe {
            let v: IAudioEndpointVolume =
                dev.as_immdevice().Activate(CLSCTX_ALL, None).map_err(s)?;
            let db = v.GetMasterVolumeLevel().map_err(s)?;
            let sc = v.GetMasterVolumeLevelScalar().map_err(s)?;
            let mute = v.GetMute().map_err(s)?.as_bool();
            Ok(format!(
                "{db:.1} dB (scalar {sc:.2}){}",
                if mute { ", MUTED" } else { "" }
            ))
        }
    };
    run().unwrap_or_else(|e| format!("unavailable: {e}"))
}

fn pcm16() -> WaveFormat {
    WaveFormat::new(16, 16, &SampleType::Int, RATE as usize, 1, None)
}

fn shared_mode() -> StreamMode {
    StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: BUFFER_HNS,
    }
}

/// Opens and initializes a capture (or, on a render endpoint, loopback)
/// client the way the app does, with `cat`'s client properties first.
fn open_client(en: &DeviceEnumerator, id: &str, cat: Cat) -> Result<(AudioClient, String), String> {
    let dev = en.get_device(id).map_err(|e| format!("open device: {e}"))?;
    let mut client = dev
        .get_iaudioclient()
        .map_err(|e| format!("IAudioClient: {e}"))?;
    let mix = client
        .get_mixformat()
        .map(|m| {
            format!(
                "{} Hz {} ch {}-bit {}",
                m.get_samplespersec(),
                m.get_nchannels(),
                m.get_bitspersample(),
                match m.get_subformat() {
                    Ok(SampleType::Float) => "float",
                    _ => "int",
                }
            )
        })
        .unwrap_or_else(|e| format!("mix format: {e}"));
    if let Some(c) = cat.stream() {
        // SetClientProperties has to come before Initialize.
        client
            .set_properties(AudioClientProperties::new().set_category(c))
            .map_err(|e| format!("SetClientProperties({}): {e}", cat.label()))?;
    }
    client
        .initialize_client(&pcm16(), &Direction::Capture, &shared_mode())
        .map_err(|e| format!("Initialize: {e}"))?;
    Ok((client, mix))
}

fn effect_name(id: &windows::core::GUID) -> String {
    let known = [
        (ks::AUDIO_EFFECT_TYPE_ACOUSTIC_ECHO_CANCELLATION, "AEC"),
        (ks::AUDIO_EFFECT_TYPE_NOISE_SUPPRESSION, "NS"),
        (ks::AUDIO_EFFECT_TYPE_AUTOMATIC_GAIN_CONTROL, "AGC"),
        (ks::AUDIO_EFFECT_TYPE_BEAMFORMING, "Beamforming"),
        (ks::AUDIO_EFFECT_TYPE_CONSTANT_TONE_REMOVAL, "ToneRemoval"),
        (ks::AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION, "DeepNS"),
        (ks::AUDIO_EFFECT_TYPE_DYNAMIC_RANGE_COMPRESSION, "DRC"),
        (ks::AUDIO_EFFECT_TYPE_EQUALIZER, "EQ"),
        (
            ks::AUDIO_EFFECT_TYPE_FAR_FIELD_BEAMFORMING,
            "FarFieldBeamforming",
        ),
        (ks::AUDIO_EFFECT_TYPE_LOUDNESS_EQUALIZER, "LoudnessEQ"),
        (ks::AUDIO_EFFECT_TYPE_ENVIRONMENTAL_EFFECTS, "Environmental"),
        (ks::AUDIO_EFFECT_TYPE_ROOM_CORRECTION, "RoomCorrection"),
        (ks::AUDIO_EFFECT_TYPE_BASS_BOOST, "BassBoost"),
        (ks::AUDIO_EFFECT_TYPE_BASS_MANAGEMENT, "BassManagement"),
        (
            ks::AUDIO_EFFECT_TYPE_SPEAKER_COMPENSATION,
            "SpeakerCompensation",
        ),
        (ks::AUDIO_EFFECT_TYPE_SPEAKER_FILL, "SpeakerFill"),
        (
            ks::AUDIO_EFFECT_TYPE_SPEAKER_PROTECTION,
            "SpeakerProtection",
        ),
        (
            ks::AUDIO_EFFECT_TYPE_VIRTUAL_HEADPHONES,
            "VirtualHeadphones",
        ),
        (ks::AUDIO_EFFECT_TYPE_VIRTUAL_SURROUND, "VirtualSurround"),
    ];
    known
        .iter()
        .find(|(g, _)| g == id)
        .map_or_else(|| format!("{id:?}"), |(_, n)| (*n).to_owned())
}

fn describe_effects(effects: &[AUDIO_EFFECT]) -> String {
    if effects.is_empty() {
        return "-".into();
    }
    effects
        .iter()
        .map(|e| {
            format!(
                "{}({}{})",
                effect_name(&e.id),
                if e.state == AUDIO_EFFECT_STATE_ON {
                    "on"
                } else {
                    "off"
                },
                if e.canSetState.as_bool() {
                    ",settable"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn effects_of(client: &AudioClient) -> String {
    match client
        .get_audio_effects_manager()
        .and_then(|m| m.get_audio_effects())
    {
        Ok(Some(v)) => describe_effects(&v),
        Ok(None) => "-".into(),
        Err(e) => format!("effects: {e}"),
    }
}

fn support_table(o: &mut Out, devs: &[DeviceInfo]) {
    say!(
        o,
        "a. capture endpoints: shared-mode client at 16 kHz mono PCM16 with AUTOCONVERTPCM (as the \
         app), per stream category: is_aec_supported, IAcousticEchoCancellationControl via \
         GetService, the stream's audio effects"
    );
    let cats = [
        Cat::App,
        Cat::Communications,
        Cat::Speech,
        Cat::GameChat,
        Cat::FarFieldSpeech,
        Cat::UniformSpeech,
        Cat::VoiceTyping,
    ];
    let en = match DeviceEnumerator::new() {
        Ok(e) => e,
        Err(e) => {
            say!(o, "  device enumerator: {e}");
            return;
        }
    };
    for d in devs.iter().filter(|d| d.flow == DeviceFlow::Capture) {
        let mut marks = Vec::new();
        if d.is_default {
            marks.push("default");
        }
        if d.is_default_comms {
            marks.push("default comms");
        }
        let mut mix_shown = false;
        let mut lines = Vec::new();
        for cat in cats {
            let cell = match open_client(&en, &d.id, cat) {
                Ok((client, mix)) => {
                    if !mix_shown {
                        lines.insert(0, format!("    mix format {mix}"));
                        mix_shown = true;
                    }
                    let aec = match client.is_aec_supported() {
                        Ok(true) => "yes".to_owned(),
                        Ok(false) => "no".to_owned(),
                        Err(e) => format!("error {e}"),
                    };
                    let ctl = match client.get_aec_control() {
                        Ok(_) => "ok".to_owned(),
                        Err(e) => format!("{e}"),
                    };
                    format!(
                        "is_aec_supported {aec}; AEC control {ctl}; effects {}",
                        effects_of(&client)
                    )
                }
                Err(e) => format!("error: {e}"),
            };
            lines.push(format!("    {:<16} {cell}", cat.label()));
        }
        say!(
            o,
            "  {}{}",
            d.name,
            if marks.is_empty() {
                String::new()
            } else {
                format!(" [{}]", marks.join(", "))
            }
        );
        for l in lines {
            say!(o, "{l}");
        }
    }
    say!(o);
}

/// What a mic run opens.
#[derive(Debug, Clone)]
struct MicSpec {
    device_id: String,
    cat: Cat,
    /// Render endpoint to set as the AEC reference (when the control exists).
    aec_ref: Option<String>,
}

enum Cmd {
    SetRef(Option<String>),
    AecEffect(bool),
    Stop,
}

/// AEC state of one run's client.
#[derive(Debug, Default)]
struct Setup {
    mix: String,
    effects: String,
    aec_supported: String,
    aec_control: String,
    set_ref: String,
    stream_latency_ms: Option<f64>,
    period_ms: Option<(f64, f64)>,
}

/// One recording.
#[derive(Debug, Default)]
struct Take {
    spec_label: String,
    setup: Setup,
    samples: Vec<i16>,
    /// QPC (100 ns) of the first frame of the first packet: sample 0.
    anchor_qpc: i64,
    /// QPC just before `Start`.
    start_qpc: i64,
    stop_qpc: i64,
    /// Wall time from `Start` returning to the first packet read.
    first_packet_wall_ms: Option<f64>,
    packets: u64,
    silent: u64,
    discontinuities: u64,
    timestamp_errors: u64,
    frames_per_packet: Vec<u32>,
    /// QPC at read minus packet timestamp, per packet.
    ages_ms: Vec<f64>,
    /// Last packet: timestamp and index of its first sample.
    last_ts: i64,
    last_idx: usize,
    events: Vec<(i64, String)>,
    effects_end: String,
}

impl Take {
    fn idx_of(&self, qpc: i64) -> isize {
        ((qpc - self.anchor_qpc) as f64 / HNS_PER_SAMPLE as f64).round() as isize
    }

    fn slice(&self, from: isize, to: isize) -> &[i16] {
        let n = self.samples.len() as isize;
        let a = from.clamp(0, n) as usize;
        let b = to.clamp(0, n) as usize;
        &self.samples[a..b.max(a)]
    }
}

struct MicHandle {
    tx: mpsc::Sender<Cmd>,
    join: JoinHandle<Result<Take, String>>,
}

impl MicHandle {
    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    fn stop(self) -> Result<Take, String> {
        let _ = self.tx.send(Cmd::Stop);
        self.join
            .join()
            .map_err(|_| "mic thread panicked".to_owned())?
    }
}

fn start_mic(spec: &MicSpec) -> Result<MicHandle, String> {
    let (tx, rx) = mpsc::channel();
    let (rtx, rrx) = mpsc::channel::<Result<(), String>>();
    let spec = spec.clone();
    let join = std::thread::Builder::new()
        .name("aec-probe-capture".into())
        .spawn(move || mic_thread(spec, rx, rtx))
        .map_err(s)?;
    match rrx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(MicHandle { tx, join }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => {
            let _ = tx.send(Cmd::Stop);
            Err("capture did not start within 10 s".into())
        }
    }
}

type Opened = (
    AudioClient,
    AudioCaptureClient,
    Setup,
    Option<AcousticEchoCancellationControl>,
    Option<IAudioEffectsManager>,
);

fn open_for_run(en: &DeviceEnumerator, spec: &MicSpec) -> Result<Opened, String> {
    let (client, mix) = open_client(en, &spec.device_id, spec.cat)?;
    let mut setup = Setup {
        mix,
        effects: effects_of(&client),
        ..Default::default()
    };
    let mut control = None;
    if spec.cat != Cat::App || spec.aec_ref.is_some() {
        setup.aec_supported = match client.is_aec_supported() {
            Ok(b) => b.to_string(),
            Err(e) => format!("error {e}"),
        };
        match client.get_aec_control() {
            Ok(c) => {
                setup.aec_control = "ok".into();
                setup.set_ref = match &spec.aec_ref {
                    Some(id) => match c.set_echo_cancellation_render_endpoint(Some(id.clone())) {
                        Ok(()) => "ok".into(),
                        Err(e) => format!("failed: {e}"),
                    },
                    None => "not asked".into(),
                };
                control = Some(c);
            }
            Err(e) => {
                setup.aec_control = format!("{e}");
                setup.set_ref = "not possible (no AEC control)".into();
            }
        }
    }
    // SAFETY: GetService on an initialized client; the interface is
    // released when dropped.
    let fx = unsafe {
        client
            .as_iaudioclient()
            .GetService::<IAudioEffectsManager>()
    }
    .ok();
    // SAFETY: plain query on an initialized client.
    setup.stream_latency_ms = unsafe { client.as_iaudioclient().GetStreamLatency() }
        .ok()
        .map(|h| h as f64 / 10_000.0);
    setup.period_ms = client
        .get_device_period()
        .ok()
        .map(|(d, m)| (d as f64 / 10_000.0, m as f64 / 10_000.0));
    let cap = client
        .get_audiocaptureclient()
        .map_err(|e| format!("IAudioCaptureClient: {e}"))?;
    Ok((client, cap, setup, control, fx))
}

fn mic_thread(
    spec: MicSpec,
    rx: mpsc::Receiver<Cmd>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<Take, String> {
    let _ = initialize_mta();
    let en = match DeviceEnumerator::new() {
        Ok(e) => e,
        Err(e) => {
            let m = format!("device enumerator: {e}");
            let _ = ready.send(Err(m.clone()));
            return Err(m);
        }
    };
    let (client, cap, setup, control, fx) = match open_for_run(&en, &spec) {
        Ok(v) => v,
        Err(m) => {
            let _ = ready.send(Err(m.clone()));
            return Err(m);
        }
    };
    let mut take = Take {
        spec_label: spec.cat.label().into(),
        setup,
        ..Default::default()
    };
    take.start_qpc = capture::qpc_now_100ns();
    if let Err(e) = client.start_stream() {
        let m = format!("Start: {e}");
        let _ = ready.send(Err(m.clone()));
        return Err(m);
    }
    let t0 = Instant::now();
    let _ = ready.send(Ok(()));
    let mut buf = vec![0u8; 32_000];
    'run: loop {
        loop {
            match rx.try_recv() {
                Ok(Cmd::Stop) | Err(TryRecvError::Disconnected) => break 'run,
                Err(TryRecvError::Empty) => break,
                Ok(Cmd::SetRef(id)) => {
                    let what = format!(
                        "SetEchoCancellationRenderEndpoint({})",
                        id.as_deref().unwrap_or("NULL")
                    );
                    let r = match &control {
                        Some(c) => match c.set_echo_cancellation_render_endpoint(id) {
                            Ok(()) => "ok".to_owned(),
                            Err(e) => format!("failed: {e}"),
                        },
                        None => "no AEC control".to_owned(),
                    };
                    take.events
                        .push((capture::qpc_now_100ns(), format!("{what}: {r}")));
                }
                Ok(Cmd::AecEffect(on)) => {
                    let state = if on {
                        AUDIO_EFFECT_STATE_ON
                    } else {
                        AUDIO_EFFECT_STATE_OFF
                    };
                    let r = match &fx {
                        // SAFETY: plain COM call on a live interface.
                        Some(m) => match unsafe {
                            m.SetAudioEffectState(
                                ks::AUDIO_EFFECT_TYPE_ACOUSTIC_ECHO_CANCELLATION,
                                state,
                            )
                        } {
                            Ok(()) => "ok".to_owned(),
                            Err(e) => format!("failed: {e}"),
                        },
                        None => "no IAudioEffectsManager".to_owned(),
                    };
                    take.events.push((
                        capture::qpc_now_100ns(),
                        format!(
                            "SetAudioEffectState(AEC, {}): {r}; effects now {}",
                            if on { "ON" } else { "OFF" },
                            effects_of(&client)
                        ),
                    ));
                }
            }
        }
        loop {
            let n = match cap.get_next_packet_size() {
                Ok(Some(n)) => n as usize,
                Ok(None) => 0,
                Err(e) => {
                    take.events
                        .push((capture::qpc_now_100ns(), format!("packet size: {e}")));
                    break 'run;
                }
            };
            if n == 0 {
                break;
            }
            if buf.len() < n * 2 {
                buf.resize(n * 2, 0);
            }
            let (frames, info) = match cap.read_from_device(&mut buf) {
                Ok(v) => v,
                Err(e) => {
                    take.events
                        .push((capture::qpc_now_100ns(), format!("read: {e}")));
                    break 'run;
                }
            };
            if frames == 0 {
                break;
            }
            let now = capture::qpc_now_100ns();
            let idx = take.samples.len();
            let ts = if info.flags.timestamp_error || info.timestamp == 0 {
                take.timestamp_errors += u64::from(info.flags.timestamp_error);
                take.anchor_qpc + idx as i64 * HNS_PER_SAMPLE
            } else {
                info.timestamp as i64
            };
            if take.packets == 0 {
                take.anchor_qpc = ts;
                take.first_packet_wall_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
            }
            take.packets += 1;
            take.frames_per_packet.push(frames);
            take.discontinuities += u64::from(info.flags.data_discontinuity);
            if info.flags.silent {
                take.silent += 1;
                take.samples
                    .extend(std::iter::repeat_n(0i16, frames as usize));
            } else {
                take.samples.extend(
                    buf[..frames as usize * 2]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b)),
                );
            }
            take.ages_ms.push((now - ts) as f64 / 10_000.0);
            take.last_ts = ts;
            take.last_idx = idx;
        }
        std::thread::sleep(POLL);
    }
    take.stop_qpc = capture::qpc_now_100ns();
    take.effects_end = effects_of(&client);
    let _ = client.stop_stream();
    Ok(take)
}

/// Records 1 s, plays `wav` on `render_id` (running `schedule` against the
/// running client meanwhile, offsets from the play request), records 1 s.
fn playback_run(
    spec: &MicSpec,
    render_id: &str,
    wav: &Path,
    schedule: Vec<(Duration, Cmd)>,
) -> Result<(Take, PlayReport), String> {
    let h = start_mic(spec)?;
    std::thread::sleep(Duration::from_secs(1));
    let render = render_id.to_owned();
    let wav = wav.to_owned();
    let t0 = Instant::now();
    let player = std::thread::Builder::new()
        .name("aec-probe-play".into())
        .spawn(move || {
            let stop = AtomicBool::new(false);
            capture::play_wav(Some(&render), &wav, &stop).map_err(s)
        })
        .map_err(s)?;
    for (at, cmd) in schedule {
        if let Some(wait) = at.checked_sub(t0.elapsed()) {
            std::thread::sleep(wait);
        }
        h.send(cmd);
    }
    let played = player
        .join()
        .map_err(|_| "playback thread panicked".to_owned())?;
    std::thread::sleep(Duration::from_secs(1));
    let take = h.stop()?;
    Ok((take, played?))
}

fn silence_run(spec: &MicSpec, d: Duration) -> Result<Take, String> {
    let h = start_mic(spec)?;
    std::thread::sleep(d);
    h.stop()
}

// ---- signals ------------------------------------------------------------------

/// Band-limited noise bursts, 300 ms on and 200 ms off, centre frequencies
/// cycling through the speech formant range, peak -12 dBFS.
fn speech_like(seconds: u32) -> Vec<i16> {
    bursts(
        seconds as usize * RATE as usize,
        0x9E37_79B9_7F4A_7C15,
        &[
            500.0, 1_400.0, 800.0, 2_400.0, 650.0, 1_800.0, 1_000.0, 3_000.0,
        ],
        &[1.0, 0.8, 0.9, 0.6, 1.0, 0.7, 0.85, 0.55],
        (0.3, 0.2),
    )
}

/// A second, different speech-like signal (the near end of the
/// double-talk check): other centres, 250 ms on and 250 ms off.
fn near_like(samples: usize) -> Vec<i16> {
    bursts(
        samples,
        0x1234_5678_9ABC_DEF1,
        &[700.0, 2_000.0, 450.0, 1_200.0, 2_800.0, 900.0],
        &[1.0, 0.7, 0.9, 0.8, 0.6, 1.0],
        (0.25, 0.25),
    )
}

/// Band-limited noise bursts (`on_off` seconds), peak -12 dBFS.
fn bursts(total: usize, seed: u64, centres: &[f64], gains: &[f64], on_off: (f64, f64)) -> Vec<i16> {
    let fs = f64::from(RATE);
    let on = (on_off.0 * fs) as usize;
    let off = (on_off.1 * fs) as usize;
    let ramp = (0.015 * fs) as usize;
    let mut rng = seed;
    let mut noise = move || {
        // xorshift64*
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        let v = rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (v as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    };
    let mut x = vec![0f64; total];
    let mut pos = 0usize;
    let mut k = 0usize;
    while pos < total {
        let fc: f64 = centres[k % centres.len()];
        let g = gains[k % gains.len()];
        // Two cascaded RBJ band-pass biquads (0 dB peak), Q 1.4.
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        let alpha = w0.sin() / (2.0 * 1.4);
        let (b0, b2, a0, a1, a2) = (alpha, -alpha, 1.0 + alpha, -2.0 * w0.cos(), 1.0 - alpha);
        let mut st = [[0f64; 4]; 2];
        let mut filt = |v: f64| {
            let mut y = v;
            for s in &mut st {
                let out = (b0 * y + b2 * s[1] - a1 * s[2] - a2 * s[3]) / a0;
                s[1] = s[0];
                s[0] = y;
                s[3] = s[2];
                s[2] = out;
                y = out;
            }
            y
        };
        // Settle the filter before the burst.
        for _ in 0..800 {
            filt(noise());
        }
        for i in 0..on {
            if pos + i >= total {
                break;
            }
            let env = if i < ramp {
                0.5 - 0.5 * (std::f64::consts::PI * i as f64 / ramp as f64).cos()
            } else if i >= on - ramp {
                0.5 - 0.5 * (std::f64::consts::PI * (on - i) as f64 / ramp as f64).cos()
            } else {
                1.0
            };
            x[pos + i] = filt(noise()) * env * g;
        }
        pos += on + off;
        k += 1;
    }
    let peak = x.iter().fold(0f64, |m, v| m.max(v.abs())).max(1e-9);
    let scale = 10f64.powf(-12.0 / 20.0) * 32_767.0 / peak;
    x.iter().map(|v| (v * scale).round() as i16).collect()
}

fn tone(seconds: f64, hz: f64, dbfs: f64) -> Vec<i16> {
    let n = (seconds * f64::from(RATE)) as usize;
    let a = 10f64.powf(dbfs / 20.0) * 32_767.0;
    let ramp = 800usize;
    (0..n)
        .map(|i| {
            let env = (i.min(n - 1 - i) as f64 / ramp as f64).min(1.0);
            let t = i as f64 / f64::from(RATE);
            (a * env * (2.0 * std::f64::consts::PI * hz * t).sin()).round() as i16
        })
        .collect()
}

fn load_ref(path: &Path) -> Result<Vec<i16>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = r.spec();
    if spec.sample_rate != RATE
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("--wav must be a 16 kHz 16-bit PCM WAV".into());
    }
    let ch = usize::from(spec.channels.max(1));
    let all: Vec<i16> = r
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(all
        .chunks(ch)
        .map(|f| (f.iter().map(|&v| i32::from(v)).sum::<i32>() / ch as i32) as i16)
        .collect())
}

fn write_wav(path: &Path, samples: &[i16]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w =
        hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    for &v in samples {
        w.write_sample(v).map_err(s)?;
    }
    w.finalize().map_err(s)
}

// ---- analysis -----------------------------------------------------------------

/// RMS in dBFS (full-scale square wave = 0 dBFS; full-scale sine = -3).
fn rms_dbfs(x: &[i16]) -> f64 {
    if x.is_empty() {
        return f64::NAN;
    }
    let p = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
    10.0 * (p.max(1e-3) / 32_768f64.powi(2)).log10()
}

fn power(x: &[i16]) -> f64 {
    if x.is_empty() {
        return f64::NAN;
    }
    x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64 / 32_768f64.powi(2)
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-12).log10()
}

fn peak_dbfs(x: &[i16]) -> f64 {
    let p = x
        .iter()
        .map(|&v| i32::from(v).unsigned_abs())
        .max()
        .unwrap_or(0);
    20.0 * (f64::from(p).max(1.0) / 32_768.0).log10()
}

/// Lag (mic samples after the playback position, `MIN_LAG_MS` to
/// `MAX_LAG_MS`) where `reference[from..to]` best matches the mic, and the
/// normalized correlation there.
fn lag_of(
    mic: &[i16],
    play_idx: isize,
    reference: &[i16],
    from: usize,
    to: usize,
) -> Option<(isize, f64)> {
    let min_lag = MIN_LAG_MS * i64::from(RATE) / 1000;
    let max_lag = MAX_LAG_MS * i64::from(RATE) / 1000;
    let y: Vec<f32> = reference
        .get(from..to)?
        .iter()
        .map(|&v| f32::from(v))
        .collect();
    let x0 = play_idx + from as isize + min_lag as isize;
    let x1 = play_idx + to as isize + max_lag as isize;
    if x0 < 0 || x1 as usize > mic.len() || y.is_empty() {
        return None;
    }
    let x: Vec<f32> = mic[x0 as usize..x1 as usize]
        .iter()
        .map(|&v| f32::from(v))
        .collect();
    let n = (x.len() + y.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let pad = |v: &[f32]| {
        let mut c: Vec<Complex32> = v.iter().map(|&a| Complex32::new(a, 0.0)).collect();
        c.resize(n, Complex32::new(0.0, 0.0));
        c
    };
    let mut a = pad(&x);
    let mut b = pad(&y);
    fwd.process(&mut a);
    fwd.process(&mut b);
    for (p, q) in a.iter_mut().zip(&b) {
        *p *= q.conj();
    }
    inv.process(&mut a);
    let ey: f64 = y.iter().map(|&v| f64::from(v).powi(2)).sum();
    let mut pre = vec![0f64; x.len() + 1];
    for (i, &v) in x.iter().enumerate() {
        pre[i + 1] = pre[i] + f64::from(v).powi(2);
    }
    let mut best: Option<(usize, f64)> = None;
    for k in 0..=(x.len() - y.len()) {
        let ex = pre[k + y.len()] - pre[k];
        if ex <= 0.0 || ey <= 0.0 {
            continue;
        }
        let r = f64::from(a[k].re) / n as f64 / (ex * ey).sqrt();
        if best.is_none_or(|(_, b)| r.abs() > b.abs()) {
            best = Some((k, r));
        }
    }
    best.map(|(k, r)| (k as isize + min_lag as isize, r))
}

struct PlayStats {
    /// Mic index of the playback start (from the render device clock).
    play_idx: isize,
    /// Mic index where the echo of reference sample 0 arrives.
    echo_idx: isize,
    during: f64,
    outside: f64,
    p_during: f64,
    p_outside: f64,
}

impl PlayStats {
    fn of(t: &Take, play_idx: isize, lag: isize, ref_len: usize) -> Self {
        let a = play_idx + lag;
        let b = a + ref_len as isize;
        let guard = (RATE / 20) as isize;
        let tail = (3 * RATE / 10) as isize;
        let mut out: Vec<i16> = t.slice(guard, play_idx - guard).to_vec();
        out.extend_from_slice(t.slice(b + tail, t.samples.len() as isize - guard));
        let dur = t.slice(a, b);
        Self {
            play_idx,
            echo_idx: a,
            during: rms_dbfs(dur),
            outside: rms_dbfs(&out),
            p_during: power(dur),
            p_outside: power(&out),
        }
    }
}

fn median(v: &[f64]) -> f64 {
    pct(v, 0.5)
}

fn pct(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * q).round() as usize]
}

fn print_take(o: &mut Out, name: &str, t: &Take, play: Option<&PlayReport>) {
    let st = &t.setup;
    say!(
        o,
        "  {name}: category {}; mic mix format {}",
        t.spec_label,
        st.mix
    );
    if !st.aec_supported.is_empty() {
        say!(
            o,
            "  {name}: is_aec_supported {}; AEC control {}; set reference {}",
            st.aec_supported,
            st.aec_control,
            st.set_ref
        );
    }
    say!(
        o,
        "  {name}: effects at Initialize: {}; at Stop: {}",
        st.effects,
        t.effects_end
    );
    if let Some(p) = play {
        say!(
            o,
            "  {name}: played {} frames on {} (start from {})",
            p.frames,
            p.device_name,
            if p.from_device_clock {
                "the device clock"
            } else {
                "Start"
            }
        );
    }
    for (q, e) in &t.events {
        say!(
            o,
            "  {name}: at {:.2} s: {e}",
            (q - t.anchor_qpc) as f64 / 1e7
        );
    }
}

fn series(t: &Take, from: isize, to: isize, block: usize) -> Vec<f64> {
    let mut v = Vec::new();
    let mut a = from;
    while a + block as isize <= to {
        v.push(rms_dbfs(t.slice(a, a + block as isize)));
        a += block as isize;
    }
    v
}

fn fmt_series(v: &[f64]) -> String {
    v.iter()
        .map(|x| format!("{x:.0}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn packet_line(t: &Take) -> String {
    let secs = (t.stop_qpc - t.start_qpc) as f64 / 1e7;
    let fpp: Vec<f64> = t.frames_per_packet.iter().map(|&f| f64::from(f)).collect();
    let span = (t.last_ts - t.anchor_qpc) as f64 / 1e4;
    let expected = t.last_idx as f64 * HNS_PER_SAMPLE as f64 / 1e4;
    format!(
        "first packet {} ms after Start (its timestamp {:+.1} ms from Start); {:.1} packets/s; \
         {} frames/packet (median); age at read median {:.1} ms, p95 {:.1} ms; \
         GetStreamLatency {}; device period {}; silent {} discont {} ts-err {}; \
         timestamp vs sample count drift {:+.2} ms",
        t.first_packet_wall_ms
            .map_or("-".into(), |m| format!("{m:.1}")),
        (t.anchor_qpc - t.start_qpc) as f64 / 1e4,
        t.packets as f64 / secs.max(1e-3),
        median(&fpp),
        median(&t.ages_ms),
        pct(&t.ages_ms, 0.95),
        t.setup
            .stream_latency_ms
            .map_or("-".into(), |m| format!("{m:.1} ms")),
        t.setup
            .period_ms
            .map_or("-".into(), |(d, m)| format!("{d:.1}/{m:.1} ms")),
        t.silent,
        t.discontinuities,
        t.timestamp_errors,
        span - expected
    )
}

fn summary(
    o: &mut Out,
    runs: &[(String, Take, Option<PlayReport>)],
    stats: &[Option<PlayStats>],
    reference: &[i16],
    lag1: Option<(isize, f64)>,
    echo_heard: bool,
) {
    say!(
        o,
        "f. summary (mic RMS, dBFS; playback span = reference shifted by the run-1 lag)"
    );
    say!(
        o,
        "  {:<34} {:>10} {:>10} {:>10}",
        "run",
        "during",
        "outside",
        "difference"
    );
    for ((name, t, _), st) in runs.iter().zip(stats) {
        match st {
            Some(st) => say!(
                o,
                "  {:<34} {:>10.1} {:>10.1} {:>10.1}",
                name,
                st.during,
                st.outside,
                st.during - st.outside
            ),
            None => say!(
                o,
                "  {:<34} {:>10} {:>10.1} {:>10}   (whole take; peak {:.1} dBFS; loudest 100 ms {:.1} dBFS)",
                name,
                "-",
                rms_dbfs(t.slice(800, t.samples.len() as isize - 800)),
                "-",
                peak_dbfs(t.slice(800, t.samples.len() as isize - 800)),
                series(t, 800, t.samples.len() as isize - 800, 1_600)
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max)
            ),
        }
    }
    say!(
        o,
        "  burst contrast (mic power in the reference's bursts over the late gaps between them, \
         same take, so independent of AGC gain; lag searched -20 to +400 ms):"
    );
    let mut contrasts = Vec::new();
    for ((name, t, _), st) in runs.iter().zip(stats) {
        if let Some(st) = st {
            match burst_contrast(t, st.play_idx, reference, 0, reference.len()) {
                Some((l, c, r)) => {
                    say!(
                        o,
                        "    {name}: contrast {c:.1} dB at lag {l:.0} ms; echo-to-background {r:.1} dB"
                    );
                    contrasts.push(Some(r));
                }
                None => {
                    say!(
                        o,
                        "    {name}: not measurable (reference has no clear bursts and gaps)"
                    );
                    contrasts.push(None);
                }
            }
        }
    }
    if let (Some(Some(r1)), Some(Some(r2))) = (contrasts.first(), contrasts.get(1)) {
        say!(
            o,
            "  echo reduction relative to background, run 1 minus run 2: {:.1} dB (a floor of \
             -40 dB echo-to-background means no echo found)",
            r1 - r2
        );
    }
    for ((name, t, _), st) in runs.iter().zip(stats) {
        if let Some(st) = st {
            let (g, rest) = coherent(t, st.echo_idx, reference);
            say!(
                o,
                "  {name}: mic = reference x {g:+.1} dB at the run's lag, plus a remainder of \
                 {rest:.1} dBFS (the part not explained by a scaled copy of the reference)"
            );
        }
    }
    match lag1 {
        Some((l, c)) => say!(
            o,
            "  capture-to-playback lag (run 1, mic vs reference cross-correlation): {:.1} ms, \
             normalized correlation {:.2}",
            l as f64 * 1000.0 / f64::from(RATE),
            c
        ),
        None => say!(o, "  capture-to-playback lag: not measurable"),
    }
    if let (Some(Some(s1)), Some(Some(s2))) = (stats.first(), stats.get(1)) {
        let raw = s1.during - s2.during;
        say!(
            o,
            "  echo return loss enhancement, run 1 minus run 2 during playback: {raw:.1} dB"
        );
        let e1 = s1.p_during - s1.p_outside;
        let e2 = s2.p_during - s2.p_outside;
        if e1 > 0.0 && e2 > 0.0 {
            say!(
                o,
                "  noise-floor corrected (echo power = during minus outside, per run): {:.1} dB",
                db(e1) - db(e2)
            );
        } else if e1 > 0.0 {
            say!(
                o,
                "  noise-floor corrected: run 2's playback span is not above its own floor; the \
                 residual echo is at or below the noise, so the enhancement is at least {:.1} dB",
                db(e1) - db(s2.p_outside)
            );
        }
        // Convergence: per 0.5 s difference across the playback span.
        let (t1, t2) = (&runs[0].1, &runs[1].1);
        let blk = (RATE / 2) as usize;
        let a1 = series(t1, s1.echo_idx, s1.echo_idx + reference.len() as isize, blk);
        let a2 = series(t2, s2.echo_idx, s2.echo_idx + reference.len() as isize, blk);
        let d: Vec<f64> = a1.iter().zip(&a2).map(|(x, y)| x - y).collect();
        say!(o, "  run 1 per 0.5 s during playback: {}", fmt_series(&a1));
        say!(o, "  run 2 per 0.5 s during playback: {}", fmt_series(&a2));
        say!(o, "  run 1 minus run 2 per 0.5 s:     {}", fmt_series(&d));
        if let Some((l, c)) = lag_of(&t2.samples, s2.play_idx, reference, 0, reference.len()) {
            say!(
                o,
                "  run 2 residual vs reference: lag {:.1} ms, normalized correlation {:.2}",
                l as f64 * 1000.0 / f64::from(RATE),
                c
            );
        }
    }
    if !echo_heard {
        say!(
            o,
            "  (no echo above the floor in run 1: no AEC measurement)"
        );
    }
    say!(o, "  packet timing per run:");
    for (name, t, _) in runs {
        say!(o, "    {name}: {}", packet_line(t));
    }
}

/// Echo against background inside one take, independent of any gain the
/// processing applies: mic power in the reference's bursts (edges trimmed)
/// over mic power in the late part of the gaps between bursts (from 100 ms
/// after a burst, past the room's reverberation), at the lag in -20 to
/// +400 ms that makes this contrast largest. Returns (lag ms, contrast dB,
/// echo-to-background dB = 10 log10(10^(contrast/10) - 1)).
fn burst_contrast(
    t: &Take,
    play_idx: isize,
    reference: &[i16],
    from: usize,
    to: usize,
) -> Option<(f64, f64, f64)> {
    const B: usize = 160;
    let to = to.min(reference.len());
    let blocks: Vec<f64> = reference[from..to]
        .as_chunks::<B>()
        .0
        .iter()
        .map(|c| rms_dbfs(c))
        .collect();
    let top = blocks.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let on: Vec<bool> = blocks.iter().map(|&r| r > top - 30.0).collect();
    let n = blocks.len();
    let on_trim: Vec<usize> = (1..n.saturating_sub(1))
        .filter(|&b| on[b] && on[b - 1] && on[b + 1])
        .collect();
    let off_trim: Vec<usize> = (10..n)
        .filter(|&b| blocks[b] < -80.0 && !on[b - 10..=b].iter().any(|&x| x))
        .collect();
    if on_trim.len() < 10 || off_trim.len() < 10 {
        return None;
    }
    let mean_power = |idx: &[usize], lag: isize| {
        let mut sum = 0.0;
        let mut cnt = 0usize;
        for &b in idx {
            let a = play_idx + (from + b * B) as isize + lag;
            let x = t.slice(a, a + B as isize);
            if x.len() == B {
                sum += power(x);
                cnt += 1;
            }
        }
        (cnt > 0).then(|| sum / cnt as f64)
    };
    let mut best: Option<(f64, f64)> = None;
    let step = (RATE / 200) as isize;
    let mut lag = -(RATE as isize) / 50;
    while lag <= (RATE as isize) * 2 / 5 {
        if let (Some(pon), Some(poff)) = (mean_power(&on_trim, lag), mean_power(&off_trim, lag)) {
            let c = db(pon) - db(poff);
            if best.is_none_or(|(_, b)| c > b) {
                best = Some((lag as f64 * 1000.0 / f64::from(RATE), c));
            }
        }
        lag += step;
    }
    best.map(|(l, c)| (l, c, db((10f64.powf(c / 10.0) - 1.0).max(1e-4))))
}

/// Least-squares gain of the mic on the reference aligned at `echo_idx`,
/// and the level of what is left.
fn coherent(t: &Take, echo_idx: isize, reference: &[i16]) -> (f64, f64) {
    let y = t.slice(echo_idx, echo_idx + reference.len() as isize);
    let r = &reference[..y.len()];
    let ry: f64 = r
        .iter()
        .zip(y)
        .map(|(&a, &b)| f64::from(a) * f64::from(b))
        .sum();
    let rr: f64 = r.iter().map(|&a| f64::from(a).powi(2)).sum();
    let g = if rr > 0.0 { ry / rr } else { 0.0 };
    let rest: f64 = r
        .iter()
        .zip(y)
        .map(|(&a, &b)| (f64::from(b) - g * f64::from(a)).powi(2))
        .sum::<f64>()
        / y.len().max(1) as f64;
    (
        20.0 * g.abs().max(1e-6).log10(),
        db(rest / 32_768f64.powi(2)),
    )
}

fn control_summary(o: &mut Out, t: &Take, play: &PlayReport, reference: &[i16], lag: isize) {
    say!(
        o,
        "run 4 segments (mic RMS dBFS over each segment from 0.5 s after the change):"
    );
    let play_idx = t.idx_of(play.start_qpc_100ns);
    let echo0 = play_idx + lag;
    let end = echo0 + reference.len() as isize;
    let mut bounds = vec![(
        echo0,
        "reference = render endpoint (set before Start)".to_owned(),
    )];
    for (q, e) in &t.events {
        bounds.push((t.idx_of(*q), e.clone()));
    }
    bounds.push((end, String::new()));
    let settle = (RATE / 2) as isize;
    let st = PlayStats::of(t, play_idx, lag, reference.len());
    for w in bounds.windows(2) {
        let (a, what) = (&w[0].0, &w[0].1);
        let b = w[1].0.min(end);
        let from = (*a + settle).max(echo0);
        if b - from < (RATE / 4) as isize {
            say!(
                o,
                "  {:>5.2} s {what}: segment too short",
                (*a - echo0) as f64 / f64::from(RATE)
            );
            continue;
        }
        let r = rms_dbfs(t.slice(from, b));
        let ref_from = (from - echo0).max(0) as usize;
        let ref_to = ((b - echo0) as usize).min(reference.len());
        let lagtxt = lag_of(&t.samples, play_idx, reference, ref_from, ref_to).map_or(
            "-".into(),
            |(l, c)| {
                format!(
                    "lag {:.1} ms corr {:.2}",
                    l as f64 * 1000.0 / f64::from(RATE),
                    c
                )
            },
        );
        let lagtxt = match burst_contrast(t, play_idx, reference, ref_from, ref_to) {
            Some((l, c, e)) => format!(
                "{lagtxt}; burst contrast {c:.1} dB at {l:.0} ms, echo-to-background {e:.1} dB"
            ),
            None => lagtxt,
        };
        say!(
            o,
            "  {:>5.2} s {what}: {r:.1} dBFS ({:+.1} dB over the run's outside {:.1}); {lagtxt}",
            (*a - echo0) as f64 / f64::from(RATE),
            r - st.outside,
            st.outside
        );
    }
}

// ---- double talk --------------------------------------------------------------

/// Delay of the far end in the mic-side track relative to the reference
/// endpoint (an echo always arrives after its reference).
const DT_DELAY_MS: usize = 60;

struct DoubleTalk {
    take: Take,
    cable_play: PlayReport,
    far_play: PlayReport,
    /// What went into the mic-side endpoint, by component.
    far_c: Vec<i16>,
    near_c: Vec<i16>,
    /// Segments in mic-side track samples: (label, from, to).
    segments: Vec<(&'static str, usize, usize)>,
}

fn spawn_play(id: &str, path: &Path) -> Result<JoinHandle<Result<PlayReport, String>>, String> {
    let id = id.to_owned();
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("aec-probe-play".into())
        .spawn(move || {
            let stop = AtomicBool::new(false);
            capture::play_wav(Some(&id), &path, &stop).map_err(s)
        })
        .map_err(s)
}

fn join_play(h: JoinHandle<Result<PlayReport, String>>) -> Result<PlayReport, String> {
    h.join()
        .map_err(|_| "playback thread panicked".to_owned())?
}

/// The mic hears far end plus near end through `cable` (a render endpoint
/// that feeds the mic, such as the VB-Audio cable) while the AEC reference
/// is `far_dev`, which plays the far end only.
fn double_talk_run(
    mic: &DeviceInfo,
    cable: &DeviceInfo,
    far_dev: &DeviceInfo,
    cat: Cat,
    far: &[i16],
    out: &Path,
) -> Result<DoubleTalk, String> {
    let fs = RATE as usize;
    let delay = fs * DT_DELAY_MS / 1000;
    let near_len = 4 * fs;
    let near = near_like(near_len);
    let dt_start = (far.len() / 3).max(2 * fs);
    let far_end = far.len() + delay;
    let solo_start = far_end + fs;
    let len = solo_start + near_len;
    let mut far_c = vec![0i16; len];
    for (i, &v) in far.iter().enumerate() {
        far_c[i + delay] = v / 2;
    }
    let mut near_c = vec![0i16; len];
    for start in [dt_start, solo_start] {
        for (i, &v) in near.iter().enumerate() {
            if let Some(x) = near_c.get_mut(start + i) {
                *x = v;
            }
        }
    }
    let mix: Vec<i16> = far_c
        .iter()
        .zip(&near_c)
        .map(|(&a, &b)| a.saturating_add(b))
        .collect();
    let far_path = out.join("dt-far.wav");
    let cable_path = out.join("dt-mic-side.wav");
    write_wav(&far_path, far)?;
    write_wav(&cable_path, &mix)?;
    write_wav(&out.join("dt-near.wav"), &near_c)?;
    let spec = MicSpec {
        device_id: mic.id.clone(),
        cat,
        aec_ref: Some(far_dev.id.clone()),
    };
    let h = start_mic(&spec)?;
    std::thread::sleep(Duration::from_secs(1));
    let pf = spawn_play(&far_dev.id, &far_path)?;
    let pc = spawn_play(&cable.id, &cable_path)?;
    let far_play = join_play(pf);
    let cable_play = join_play(pc);
    std::thread::sleep(Duration::from_secs(1));
    let take = h.stop()?;
    write_wav(&out.join("mic-double-talk.wav"), &take.samples)?;
    let half = fs / 2;
    let segments = vec![
        (
            "far only, before",
            dt_start.saturating_sub(2 * fs),
            dt_start,
        ),
        ("double talk", dt_start + half, dt_start + near_len),
        ("far only, after", dt_start + near_len + half, far_end),
        ("near only", solo_start + half, solo_start + near_len),
    ];
    Ok(DoubleTalk {
        take,
        cable_play: cable_play?,
        far_play: far_play?,
        far_c,
        near_c,
        segments,
    })
}

/// Least-squares gains of `y` on the far and near components.
fn two_gains(y: &[f64], f: &[f64], n: &[f64]) -> (f64, f64) {
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let (ff, nn, fnn) = (dot(f, f), dot(n, n), dot(f, n));
    let (fy, ny) = (dot(f, y), dot(n, y));
    if ff <= 0.0 && nn <= 0.0 {
        return (0.0, 0.0);
    }
    if nn <= 0.0 {
        return (fy / ff, 0.0);
    }
    if ff <= 0.0 {
        return (0.0, ny / nn);
    }
    let det = ff * nn - fnn * fnn;
    ((fy * nn - ny * fnn) / det, (ny * ff - fy * fnn) / det)
}

fn double_talk_summary(o: &mut Out, d: &DoubleTalk, cable_lag: isize) {
    let t = &d.take;
    print_take(o, "double talk", t, Some(&d.cable_play));
    say!(
        o,
        "  double talk: far-end playback on {} started {:+.1} ms from the mic-side playback",
        d.far_play.device_name,
        (d.far_play.start_qpc_100ns - d.cable_play.start_qpc_100ns) as f64 / 1e4
    );
    let start = t.idx_of(d.cable_play.start_qpc_100ns);
    // The processing can delay the mic: align by the near end alone.
    let (solo_from, solo_to) = d.segments.last().map_or((0, 0), |s| (s.1, s.2));
    let lag = match lag_of(&t.samples, start, &d.near_c, solo_from, solo_to) {
        Some((l, c)) => {
            say!(
                o,
                "  double talk: mic-side path lag {:.1} ms (near end alone, correlation {c:.2}; \
                 without processing {:.1} ms)",
                l as f64 * 1000.0 / f64::from(RATE),
                cable_lag as f64 * 1000.0 / f64::from(RATE)
            );
            l
        }
        None => cable_lag,
    };
    let base = start + lag;
    let gain_db = |g: f64| 20.0 * g.abs().max(1e-6).log10();
    say!(
        o,
        "  segment            mic out   far in  near in  far gain  near gain  out minus near part"
    );
    for (label, a, b) in &d.segments {
        let y: Vec<f64> = t
            .slice(base + *a as isize, base + *b as isize)
            .iter()
            .map(|&v| f64::from(v))
            .collect();
        let m = y.len();
        if m < (RATE / 4) as usize {
            say!(o, "  {label:<17} too short");
            continue;
        }
        let f: Vec<f64> = d.far_c[*a..*a + m].iter().map(|&v| f64::from(v)).collect();
        let n: Vec<f64> = d.near_c[*a..*a + m].iter().map(|&v| f64::from(v)).collect();
        let (gf, gn) = two_gains(&y, &f, &n);
        let lvl = |v: &[f64]| {
            10.0 * (v.iter().map(|x| x * x).sum::<f64>()
                / v.len().max(1) as f64
                / 32_768f64.powi(2))
            .max(1e-12)
            .log10()
        };
        let rest: Vec<f64> = y.iter().zip(&n).map(|(a, b)| a - gn * b).collect();
        say!(
            o,
            "  {label:<17} {:>8.1} {:>8.1} {:>8.1} {:>9.1} {:>10.1} {:>10.1}",
            lvl(&y),
            lvl(&f),
            lvl(&n),
            gain_db(gf),
            gain_db(gn),
            lvl(&rest)
        );
    }
    say!(
        o,
        "  (levels dBFS; gains in dB are least-squares fits of the mic output on the far and near \
         components as they entered the mic side, aligned by the run-1 lag)"
    );
    let blk = (RATE / 2) as usize;
    let end = base + d.near_c.len() as isize;
    say!(
        o,
        "  mic out per 0.5 s: {}",
        fmt_series(&series(t, base, end, blk))
    );
}

// ---- ducking ------------------------------------------------------------------

/// Loopback level of a tone a second process plays on `render` while mic
/// streams of each category run in this process.
fn ducking_check(
    o: &mut Out,
    mic: &DeviceInfo,
    render: &DeviceInfo,
    out: &Path,
) -> Result<(), String> {
    let phase = Duration::from_secs(3);
    let gap = Duration::from_secs(2);
    let cats = [Cat::App, Cat::Communications, Cat::Speech, Cat::GameChat];
    let total = 2.5 + cats.len() as f64 * 5.0 + 3.0;
    let tone_path = out.join("tone.wav");
    write_wav(&tone_path, &tone(total, 330.0, -24.0))?;
    say!(
        o,
        "ducking: a second process (diarist-cli play) plays a 330 Hz tone at -24 dBFS on {}; this \
         process loops that endpoint back and opens the mic in each category for 3 s with 2 s gaps",
        render.name
    );
    let lb = start_mic(&MicSpec {
        device_id: render.id.clone(),
        cat: Cat::App,
        aec_ref: None,
    })?;
    let exe = std::env::current_exe().map_err(s)?;
    let mut child = std::process::Command::new(exe)
        .args(["play", "--device", &render.id])
        .arg(&tone_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn tone player: {e}"))?;
    std::thread::sleep(Duration::from_millis(2_500));
    let mut marks = Vec::new();
    for cat in cats {
        let spec = MicSpec {
            device_id: mic.id.clone(),
            cat,
            aec_ref: (cat != Cat::App).then(|| render.id.clone()),
        };
        let a = capture::qpc_now_100ns();
        match start_mic(&spec) {
            Ok(h) => {
                std::thread::sleep(phase);
                let b = capture::qpc_now_100ns();
                let t = h.stop();
                marks.push((cat, a, b, t.map(|t| t.setup.set_ref).unwrap_or_else(|e| e)));
            }
            Err(e) => {
                marks.push((cat, a, a, format!("open failed: {e}")));
            }
        }
        std::thread::sleep(gap);
    }
    std::thread::sleep(Duration::from_secs(1));
    let lbt = lb.stop()?;
    let _ = child.kill();
    let _ = child.wait();
    write_wav(&out.join("loopback-ducking.wav"), &lbt.samples)?;
    let sec = |q: i64| lbt.idx_of(q);
    let settle = RATE as isize;
    // Baseline: from the tone's start (+0.5 s) to the first mic open.
    let on = lbt
        .samples
        .chunks(800)
        .position(|c| rms_dbfs(c) > -45.0)
        .map(|i| (i * 800) as isize)
        .unwrap_or(0);
    let first = marks.first().map_or(0, |m| sec(m.1));
    let base = rms_dbfs(lbt.slice(on + (RATE / 2) as isize, first));
    say!(o, "  loopback baseline (tone alone): {base:.1} dBFS");
    for (i, (cat, a, b, setref)) in marks.iter().enumerate() {
        let during = rms_dbfs(lbt.slice(sec(*a) + settle, sec(*b)));
        let next = marks
            .get(i + 1)
            .map_or(sec(*b) + (2 * RATE) as isize, |m| sec(m.1));
        let after = rms_dbfs(lbt.slice(sec(*b) + settle, next));
        say!(
            o,
            "  mic open, category {:<15} loopback {during:.1} dBFS ({:+.1} dB); after close {after:.1} dBFS ({:+.1} dB); AEC reference {setref}",
            cat.label(),
            during - base,
            after - base
        );
    }
    say!(
        o,
        "  loopback per 0.25 s from the tone start: {}",
        fmt_series(&series(
            &lbt,
            on,
            lbt.samples.len() as isize,
            (RATE / 4) as usize
        ))
    );
    Ok(())
}
