//! "Copy diagnostics" (Diagnostics page): one plain-text block to paste
//! into a bug report. It names versions, the GPU, the data root and the
//! selected engine and models with their hashes; it holds no transcript
//! text, meeting titles or audio.

use std::fmt::Write as _;

use crate::audio::echo::EchoMode;
use crate::gpu::{GpuAdapter, GpuInfo};
use crate::models::{
    device_for, engine_backend, selected_file, Backend, DataPaths, DataRootSource,
};
use crate::store::Store;
use crate::types::ModelRole;

/// One row of "Why isn't it working?" (Diagnostics), also in the copied report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct HealthCheck {
    pub id: String,
    pub label: String,
    /// `pass`, `warn`, `fail` or `skip` (not needed right now).
    pub status: String,
    /// What was found, and the next step when it is not a pass.
    pub detail: String,
    /// A UI action that fixes it (or, on a passing row, changes it):
    /// `setup`, `download:<name>`, `open_models`, `open_engine`, `settings`,
    /// `settings:<field>` (Settings scrolled to that field), `sound_settings`,
    /// `mic_privacy`, `start_engine`, `rescan`.
    pub fix: Option<String>,
    pub fix_label: Option<String>,
}

impl HealthCheck {
    pub fn new(id: &str, label: &str, status: &str, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: status.into(),
            detail: detail.into(),
            fix: None,
            fix_label: None,
        }
    }

    pub fn fix(mut self, action: impl Into<String>, label: &str) -> Self {
        self.fix = Some(action.into());
        self.fix_label = Some(label.into());
        self
    }
}

/// The "GPU and driver" row for the runtime package of `backend` (the
/// selected one, else the one this PC should install), with `gpu` from
/// NVML and `adapters` from DXGI. Only the CUDA package needs an NVIDIA GPU
/// with CUDA 13; the Vulkan one runs on any GPU, the CPU one on none (a
/// warning: it is slow).
pub fn gpu_check(backend: Backend, gpu: Option<&GpuInfo>, adapters: &[GpuAdapter]) -> HealthCheck {
    let (id, label) = ("gpu", "GPU and driver");
    let nvidia = |g: &GpuInfo| {
        format!(
            "{}, {} MB, driver {}, CUDA {}",
            g.name,
            g.vram_total_mb,
            g.driver_version,
            g.cuda_version.as_deref().unwrap_or("?")
        )
    };
    let adapter = |a: &GpuAdapter| format!("{}, {} MB", a.name, a.vram_mb);
    let found = gpu.map(nvidia).or_else(|| adapters.first().map(adapter));
    match backend {
        Backend::Cuda => match gpu {
            Some(g) if g.cuda_ok => HealthCheck::new(id, label, "pass", nvidia(g)),
            Some(g) => HealthCheck::new(
                id,
                label,
                "fail",
                format!(
                    "{} with driver {} supports CUDA {}; the CUDA engine needs CUDA 13 (R580 or \
                     newer). Update the driver from nvidia.com, or install the Vulkan runtime.",
                    g.name,
                    g.driver_version,
                    g.cuda_version.as_deref().unwrap_or("?")
                ),
            )
            .fix("setup", "Choose a runtime"),
            None => HealthCheck::new(
                id,
                label,
                "fail",
                match adapters.first() {
                    Some(a) => format!(
                        "No NVIDIA GPU; found {}. The CUDA runtime needs one: use the Vulkan \
                         runtime (untested by us) or the CPU runtime (slow).",
                        a.name
                    ),
                    None => "No GPU found. The CUDA runtime needs an NVIDIA GPU: use the CPU \
                             runtime (slow)."
                        .into(),
                },
            )
            .fix("setup", "Choose a runtime"),
        },
        Backend::Vulkan => match found {
            Some(f) => HealthCheck::new(
                id,
                label,
                "pass",
                format!("{f}; the Vulkan runtime runs on it ({})", backend.device()),
            ),
            None => HealthCheck::new(
                id,
                label,
                "fail",
                "No GPU found. The Vulkan runtime needs one: use the CPU runtime (slow).",
            )
            .fix("setup", "Choose a runtime"),
        },
        Backend::Cpu => HealthCheck::new(
            id,
            label,
            "warn",
            format!(
                "{}{}",
                cpu_sentence(),
                match (&found, gpu.is_some_and(|g| g.cuda_ok)) {
                    (Some(f), true) =>
                        format!(" This PC has {f}: the CUDA runtime is much faster."),
                    (Some(f), false) => {
                        format!(" This PC has {f}: the Vulkan runtime is much faster.")
                    }
                    (None, _) => String::new(),
                }
            ),
        )
        .fix("setup", "Choose a runtime"),
    }
}

/// [`crate::models::CPU_SLOW_NOTICE`] as a sentence.
fn cpu_sentence() -> String {
    let n = crate::models::CPU_SLOW_NOTICE;
    let mut c = n.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The "GPU memory" rows for jobs on `backend`, with `free_mb` from NVML
/// and `model_need_mb` the selected speech model's peak: the live start
/// gate's 2.5 GB and the file model's need. The CPU package uses no GPU
/// memory (skipped); without NVML (an AMD or Intel GPU) the free memory is
/// unknown and recording starts without the check (skipped).
pub fn vram_checks(
    backend: Backend,
    free_mb: Option<u32>,
    model_need_mb: Option<u32>,
) -> Vec<HealthCheck> {
    let need_live = crate::live::MIN_FREE_VRAM_MB;
    let free = match (backend, free_mb) {
        (Backend::Cpu, _) => {
            return vec![HealthCheck::new(
                "vram_live",
                "GPU memory",
                "skip",
                "The CPU runtime uses system memory, not GPU memory.",
            )]
        }
        (_, Some(f)) => f,
        (Backend::Vulkan, None) => {
            return vec![HealthCheck::new(
                "vram_live",
                "GPU memory",
                "skip",
                "Free GPU memory is only known on NVIDIA GPUs; recording on the Vulkan runtime \
                 starts without the check.",
            )]
        }
        (Backend::Cuda, None) => {
            return vec![HealthCheck::new(
                "vram_live",
                "GPU memory",
                "skip",
                "Free GPU memory is only known on NVIDIA GPUs.",
            )]
        }
    };
    let live = if free >= need_live {
        HealthCheck::new(
            "vram_live",
            "GPU memory for live",
            "pass",
            format!("{free} MB free, live needs {need_live} MB"),
        )
    } else {
        HealthCheck::new(
            "vram_live",
            "GPU memory for live",
            "warn",
            format!(
                "{free} MB free, live needs {need_live} MB. Close GPU-heavy apps (games, video \
                 editors, other AI tools) before recording."
            ),
        )
    };
    let file = match model_need_mb {
        Some(n) if free >= n => HealthCheck::new(
            "vram_file",
            "GPU memory for files",
            "pass",
            format!("{free} MB free, the selected model peaks at about {n} MB"),
        ),
        Some(n) => HealthCheck::new(
            "vram_file",
            "GPU memory for files",
            "warn",
            format!(
                "{free} MB free, the selected model peaks at about {n} MB. Close GPU-heavy apps \
                 or turn on low-VRAM mode."
            ),
        )
        .fix("settings", "Low-VRAM mode"),
        None => HealthCheck::new(
            "vram_file",
            "GPU memory for files",
            "skip",
            format!("{free} MB free; the selected model's need is unknown"),
        ),
    };
    vec![live, file]
}

/// Everything the report needs that `diarist-core` cannot look up itself.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsInput {
    /// e.g. "Diarist 0.1.0".
    pub app: String,
    /// e.g. "Windows 11 (26200)".
    pub os: String,
    pub gpu: Option<GpuInfo>,
    /// Extra `label: value` lines (live server state, autostart, ...).
    pub extra: Vec<(String, String)>,
    /// The last engine stderr (path line first), if any.
    pub engine_stderr: Option<String>,
    /// Lines of `engine_stderr` to keep from its end.
    pub stderr_lines: usize,
    /// The "Why isn't it working?" checklist.
    pub checks: Vec<HealthCheck>,
}

/// "Windows 11 (build 26200, 25H2)" from the registry; "" elsewhere.
pub fn os_version() -> String {
    #[cfg(windows)]
    {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let build = hklm_string(key, "CurrentBuildNumber").unwrap_or_default();
        let display = hklm_string(key, "DisplayVersion").unwrap_or_default();
        let name = match build.parse::<u32>() {
            Ok(b) if b >= 22_000 => "Windows 11",
            Ok(_) => "Windows 10",
            Err(_) => "Windows",
        };
        let mut s = format!("{name} (build {build}");
        if !display.is_empty() {
            s.push_str(&format!(", {display}"));
        }
        s.push(')');
        s
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

#[cfg(windows)]
fn hklm_string(key: &str, value: &str) -> Option<String> {
    reg_string(
        windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        key,
        value,
    )
}

#[cfg(windows)]
fn reg_string(
    root: windows::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut buf = [0u16; 128];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            &k,
            &v,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Where Windows keeps this user's microphone consent (Settings, Privacy &
/// security, Microphone). Its `Value` is "Microphone access"; the
/// `NonPackaged` subkey's `Value` is "Let desktop apps access your
/// microphone", the switch that covers Diarist.
pub const MIC_CONSENT_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// The two consent values as read (`Allow`, `Deny`), `None` where the key
/// or the value is missing, which Windows treats as allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MicConsent {
    /// "Microphone access", for all apps.
    pub all_apps: Option<String>,
    /// "Let desktop apps access your microphone".
    pub desktop_apps: Option<String>,
}

/// Reads [`MIC_CONSENT_KEY`] from HKCU; all `None` off Windows.
pub fn mic_consent() -> MicConsent {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        MicConsent {
            all_apps: reg_string(HKEY_CURRENT_USER, MIC_CONSENT_KEY, "Value"),
            desktop_apps: reg_string(
                HKEY_CURRENT_USER,
                &format!(r"{MIC_CONSENT_KEY}\NonPackaged"),
                "Value",
            ),
        }
    }
    #[cfg(not(windows))]
    {
        MicConsent::default()
    }
}

/// The checklist row for the consent: a fail with a button for
/// `ms-settings:privacy-microphone` (`mic_privacy`) when either switch is
/// off, because Windows then hands desktop apps silence from every
/// microphone instead of an error.
pub fn mic_privacy_check(c: &MicConsent) -> HealthCheck {
    let denied = |v: &Option<String>| {
        v.as_deref()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("deny"))
    };
    let (id, label) = ("mic_privacy", "Microphone privacy");
    let detail = match (denied(&c.all_apps), denied(&c.desktop_apps)) {
        (false, false) => {
            let detail = if c.all_apps.is_none() && c.desktop_apps.is_none() {
                "No Windows privacy setting blocks desktop apps from the microphone"
            } else {
                "Windows lets desktop apps use the microphone"
            };
            return HealthCheck::new(id, label, "pass", detail);
        }
        (true, _) => {
            "Windows does not allow desktop apps to use the microphone: Microphone access is off, \
             so Diarist records silence from it. Turn on Microphone access and Let desktop apps \
             access your microphone, then check again."
        }
        (false, true) => {
            "Windows does not allow desktop apps to use the microphone: Let desktop apps access \
             your microphone is off, so Diarist records silence from it. Turn it on, then check \
             again."
        }
    };
    HealthCheck::new(id, label, "fail", detail).fix("mic_privacy", "Privacy settings")
}

/// The "Recording with speakers" row (plan 15): whether echo handling is on
/// (`echo.mode`) and, from `last` (`engine_info.echo` of the newest finished
/// live recording's current run, when it has one), what it did there. It
/// names no meeting, since the row is also in the copied report. Always
/// offers Settings, where the switch is.
pub fn echo_check(mode: EchoMode, last: Option<&serde_json::Value>) -> HealthCheck {
    let (id, label) = ("echo", "Recording with speakers");
    let note = last.map(last_echo_note).unwrap_or_default();
    let (status, state) = match mode {
        EchoMode::Off => (
            "skip",
            "Echo handling is off: what the microphone picks up from the speakers stays in \
             \"me\". Turn it on in Settings, Recording, if you record without headphones.",
        ),
        EchoMode::Auto => (
            "pass",
            "Echo handling is on: when the microphone hears the meeting from the speakers, \
             the echoed words are removed from \"me\".",
        ),
        EchoMode::Always => (
            "pass",
            "Echo handling is always on: in every recording, the words the microphone picks \
             up from the speakers are removed from \"me\".",
        ),
    };
    HealthCheck::new(id, label, status, format!("{state}{note}")).fix("settings:echo", "Settings")
}

/// What the last recording's echo step did, as a sentence after a space.
fn last_echo_note(e: &serde_json::Value) -> String {
    let coupled = e["coupled"].as_bool().unwrap_or(false);
    let coupling = e["coupling"].as_f64().unwrap_or(0.0);
    let lag = e["lag_ms"]
        .as_f64()
        .map(|l| format!(", {l:.0} ms"))
        .unwrap_or_default();
    let heard = format!(
        " In the last recording the microphone heard the speakers (coupling {coupling:.2}{lag})"
    );
    let n = |key: &str| e[key].as_u64().unwrap_or(0);
    let words = |n: u64| if n == 1 { "word" } else { "words" };
    if e["restored"].as_bool() == Some(true) {
        let r = n("restored_words");
        let were = if r == 1 { "was" } else { "were" };
        format!(
            "{heard}; its {r} removed {} {were} put back with Restore.",
            words(r)
        )
    } else if e["user_off"].as_bool() == Some(true) {
        if coupled {
            format!("{heard}; echo handling was turned off for it, so nothing was removed.")
        } else {
            " Echo handling was turned off during the last recording.".into()
        }
    } else if coupled || n("dropped") > 0 {
        let d = n("dropped");
        format!(
            "{heard}: {d} far-end {} removed from \"me\" (Restore in that meeting's details \
             puts them back).",
            words(d)
        )
    } else if e.get("coupled").is_some() {
        " The last recording had no echo.".into()
    } else {
        String::new()
    }
}

/// The user's profile folder (`%USERPROFILE%`, `$HOME` elsewhere).
pub fn profile_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|p| !p.trim().is_empty())
}

/// `text` with the user's profile folder replaced by `~`, so a pasted
/// report does not carry the Windows user name ("Copy report", "Copy
/// diagnostics"). See [`scrub_path`].
pub fn scrub_profile(text: &str) -> String {
    match profile_dir() {
        Some(p) => scrub_path(text, &p),
        None => text.to_owned(),
    }
}

/// `text` with every occurrence of the folder `dir` replaced by `~`. A match
/// ignores ASCII case (Windows paths do), takes any run of `\` and `/` as
/// one separator (so `C:/Users/x` and the doubled `C:\\Users\\x` of a
/// Debug-printed path match too) and must end at a name boundary
/// (`C:\Users\ann` does not match inside `C:\Users\anna`). A `dir` of fewer
/// than two parts (a bare drive) is left alone.
pub fn scrub_path(text: &str, dir: &str) -> String {
    let parts: Vec<&[u8]> = dir
        .split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .map(str::as_bytes)
        .collect();
    if parts.len() < 2 {
        return text.to_owned();
    }
    let sep = |b: u8| b == b'\\' || b == b'/';
    let name_byte =
        |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') || b >= 0x80;
    let t = text.as_bytes();
    // The end of a match starting at `i`, if one does.
    let match_at = |i: usize| -> Option<usize> {
        let mut j = i;
        for (n, part) in parts.iter().enumerate() {
            if n > 0 {
                let from = j;
                while j < t.len() && sep(t[j]) {
                    j += 1;
                }
                if j == from {
                    return None;
                }
            }
            let end = j.checked_add(part.len()).filter(|&e| e <= t.len())?;
            if !t[j..end].eq_ignore_ascii_case(part) {
                return None;
            }
            j = end;
        }
        (j == t.len() || !name_byte(t[j])).then_some(j)
    };
    let mut out = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let starts = i == 0 || !name_byte(t[i - 1]);
        match match_at(i).filter(|_| starts) {
            Some(end) => {
                out.push(b'~');
                i = end;
            }
            None => {
                out.push(t[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn source_name(s: DataRootSource) -> &'static str {
    match s {
        DataRootSource::Explicit => "explicit (DIARIST_DATA_DIR or --data-dir)",
        DataRootSource::Portable => "portable (portable.txt next to the exe)",
        DataRootSource::Registry => r"relocated (HKCU\Software\Diarist\DataRoot)",
        DataRootSource::Default => "default",
    }
}

/// The report text, with the profile folder as `~` ([`scrub_profile`]).
pub fn report(paths: &DataPaths, store: &dyn Store, input: &DiagnosticsInput) -> String {
    scrub_profile(&report_text(paths, store, input))
}

fn report_text(paths: &DataPaths, store: &dyn Store, input: &DiagnosticsInput) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", input.app);
    if !input.os.is_empty() {
        let _ = writeln!(out, "os: {}", input.os);
    }
    match &input.gpu {
        Some(g) => {
            let _ = writeln!(
                out,
                "gpu: {} | {} MB VRAM, {} MB free | driver {} | CUDA {}{}",
                g.name,
                g.vram_total_mb,
                g.vram_free_mb,
                if g.driver_version.is_empty() {
                    "?"
                } else {
                    &g.driver_version
                },
                g.cuda_version.as_deref().unwrap_or("?"),
                if g.cuda_ok {
                    ""
                } else {
                    " (older than the CUDA 13 the CUDA runtime needs)"
                }
            );
        }
        None => {
            let _ = writeln!(out, "gpu: no NVIDIA GPU found (NVML not available)");
        }
    }
    let _ = writeln!(
        out,
        "data root: {} [{}]",
        paths.root.display(),
        source_name(paths.source)
    );
    // The copy SqliteStore makes before a schema upgrade (`diarist.db.bak-<v>`).
    let db = paths.db();
    let prefix = format!(
        "{}.bak-",
        db.file_name()
            .map_or("diarist.db".into(), |n| n.to_string_lossy())
    );
    let backups: Vec<String> = db
        .parent()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&prefix) && !n.ends_with(".tmp"))
        .collect();
    if !backups.is_empty() {
        let _ = writeln!(
            out,
            "database backup: {} (copy made before the last schema upgrade)",
            backups.join(", ")
        );
    }
    for (role, label) in [
        (ModelRole::Engine, "engine"),
        (ModelRole::Asr, "asr"),
        (ModelRole::Diarizer, "diarizer"),
    ] {
        let line = match selected_file(store, role) {
            Ok(Some(f)) => {
                let version = if role == ModelRole::Engine {
                    format!(" {}", f.architecture.as_deref().unwrap_or("?"))
                } else {
                    String::new()
                };
                let hash = match &f.sha256 {
                    Some(h) => format!("sha256 {h}"),
                    None if role == ModelRole::Engine => String::new(),
                    None => "sha256 not recorded".to_owned(),
                };
                // What imports and post-passes pass, as their runs store it.
                let lookahead = match role {
                    ModelRole::Asr => crate::models::asr_right_context(paths, store, &f)
                        .map(|rc| {
                            format!(
                                " | file lookahead {}",
                                crate::protocol::lookahead_text(rc.map(i64::from))
                            )
                        })
                        .unwrap_or_default(),
                    ModelRole::Diarizer => crate::models::diarizer_file_preset(paths, store, &f)
                        .map(|p| {
                            format!(
                                " | file preset {}",
                                crate::protocol::diar_preset_text(p.as_deref())
                            )
                        })
                        .unwrap_or_default(),
                    // The `--device` every job passes follows the package.
                    ModelRole::Engine => format!(
                        " | device {}{}",
                        device_for(engine_backend(&f)),
                        engine_backend(&f).map_or(String::new(), |b| format!(" ({b} package)"))
                    ),
                    _ => String::new(),
                };
                format!(
                    "{}{version} | {} | {} bytes{}{}{lookahead}",
                    f.display_name,
                    f.path,
                    f.size_bytes,
                    if hash.is_empty() { "" } else { " | " },
                    hash
                )
            }
            Ok(None) => "none selected".to_owned(),
            Err(e) => format!("error: {e}"),
        };
        let _ = writeln!(out, "{label}: {line}");
    }
    for (k, v) in &input.extra {
        let _ = writeln!(out, "{k}: {v}");
    }
    let crashes = crate::crash::list_reports(&paths.logs());
    if crashes.is_empty() {
        let _ = writeln!(out, "crash reports: none");
    } else {
        let names: Vec<String> = crashes
            .iter()
            .map(|r| {
                let kind = match r.kind {
                    crate::crash::CrashKind::Panic => "panic",
                    crate::crash::CrashKind::Native => "native crash",
                    crate::crash::CrashKind::Engine => "engine exit",
                };
                format!("{} ({kind})", r.name)
            })
            .collect();
        let _ = writeln!(
            out,
            "crash reports (in the log folder): {}",
            names.join(", ")
        );
    }
    if !input.checks.is_empty() {
        let _ = writeln!(out, "checks:");
        for c in &input.checks {
            let _ = writeln!(out, "  [{}] {}: {}", c.status, c.label, c.detail);
        }
    }
    match &input.engine_stderr {
        Some(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let keep = input.stderr_lines.max(1);
            let (head, body) = lines
                .split_first()
                .map_or((None, &[][..]), |(h, b)| (Some(*h), b));
            let start = body.len().saturating_sub(keep);
            let _ = writeln!(
                out,
                "last engine stderr ({}, last {} lines):",
                head.unwrap_or("?"),
                body.len() - start
            );
            for l in &body[start..] {
                let _ = writeln!(out, "  {l}");
            }
        }
        None => {
            let _ = writeln!(
                out,
                "last engine stderr: no engine job has run on this data root"
            );
        }
    }
    out
}

/// The "Punctuation runtime" row (docs/plans/16 section 3.7): whether the
/// selected punctuation model's ONNX Runtime library is there, has its
/// Visual C++ runtime, and loaded and passed its checks in this process.
/// `skip` when punctuation is off or no model is installed (nothing gates
/// on it); a missing redistributable names the DLL and Microsoft's page.
pub fn punctuation_check(paths: &DataPaths, store: &dyn Store) -> HealthCheck {
    use crate::punct::onnx::{self, RuntimeStatus};
    let (id, label) = ("punctuation", "Punctuation runtime");
    if crate::punct::setting(store) == crate::punct::PunctSetting::Off {
        return HealthCheck::new(
            id,
            label,
            "skip",
            "Punctuation is off (transcript.punctuation): transcripts keep the speech model's \
             own punctuation.",
        );
    }
    let file = match selected_file(store, ModelRole::Punctuation) {
        Ok(Some(f)) => f,
        _ => {
            let row = HealthCheck::new(
                id,
                label,
                "skip",
                "No punctuation model is installed: transcripts keep the speech model's own \
                 punctuation.",
            );
            let entry = crate::models::Manifest::load(&paths.root)
                .ok()
                .and_then(|m| {
                    m.models
                        .iter()
                        .find(|e| e.role == ModelRole::Punctuation && !e.experimental)
                        .map(|e| e.name.clone())
                });
            return match entry {
                Some(name) => row.fix(format!("download:{name}"), "Download"),
                None => row,
            };
        }
    };
    let fail = |detail: String| HealthCheck::new(id, label, "fail", detail);
    let Some(spec) = file.capabilities.punctuation.clone() else {
        return fail(format!(
            "{} has no punctuation description. Press Refresh in Settings, Models.",
            file.display_name
        ))
        .fix("rescan", "Refresh");
    };
    let model = paths.absolute(&file.path);
    let dll = match spec
        .model_root(&model, &paths.models())
        .map_err(|e| e.to_string())
        .and_then(|root| onnx::find_runtime(&root, &spec, paths).map_err(|u| u.message))
    {
        Ok(p) => p,
        Err(e) => {
            return fail(format!(
                "{e}. Download the punctuation model again, or put the library into its folder."
            ))
            .fix("open_models", "Open models folder")
        }
    };
    if let Some(d) = onnx::missing_dependency() {
        return fail(format!(
            "{d} is missing: ONNX Runtime needs the Microsoft Visual C++ Redistributable (x64). \
             Install it from {}, then restart Diarist.",
            onnx::VC_REDIST_URL
        ));
    }
    let rel = paths.relative(&dll);
    match onnx::runtime_status() {
        RuntimeStatus::NotLoaded => HealthCheck::new(
            id,
            label,
            "pass",
            format!("ONNX Runtime at {rel}; it loads at the first punctuated transcript."),
        ),
        RuntimeStatus::Ready(rt) if same_path(&rt.path, &dll) => HealthCheck::new(
            id,
            label,
            "pass",
            format!(
                "ONNX Runtime {} loaded from {} (CPU).",
                rt.version,
                paths.relative(&rt.path)
            ),
        ),
        RuntimeStatus::Ready(rt) => HealthCheck::new(
            id,
            label,
            "warn",
            format!(
                "ONNX Runtime {} is loaded from {}; restart Diarist to use {rel}.",
                rt.version,
                paths.relative(&rt.path)
            ),
        ),
        RuntimeStatus::Broken { path, message } => fail(format!(
            "{} ({}). Restart Diarist after fixing it.",
            message,
            paths.relative(&path)
        )),
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::select;
    use crate::store::{MemoryStore, ScannedModelFile};
    use crate::types::{ModelCapabilities, ModelSource};

    #[test]
    fn report_lists_versions_gpu_root_models_and_stderr() {
        let store = MemoryStore::new();
        let row = |path: &str, role, arch: &str, sha: Option<&str>| ScannedModelFile {
            path: path.into(),
            role,
            architecture: Some(arch.into()),
            display_name: path.rsplit('/').next().unwrap().into(),
            quant: None,
            size_bytes: 42,
            sha256: sha.map(Into::into),
            source: ModelSource::Manifest,
            capabilities: ModelCapabilities::default(),
            manifest_name: None,
        };
        store
            .sync_model_files(
                &[
                    row("engine/nemo-speech-0.2.0", ModelRole::Engine, "0.2.0", None),
                    row("models/en.gguf", ModelRole::Asr, "x", Some("abc123")),
                ],
                1,
            )
            .unwrap();
        select(&store, ModelRole::Engine, "0.2.0").unwrap();
        select(&store, ModelRole::Asr, "en.gguf").unwrap();
        let input = DiagnosticsInput {
            app: "Diarist 0.1.0".into(),
            os: "Windows 11".into(),
            gpu: Some(GpuInfo {
                name: "NVIDIA GeForce RTX 3080".into(),
                vram_total_mb: 10_240,
                vram_free_mb: 8_000,
                driver_version: "610.88".into(),
                cuda_version: Some("13.1".into()),
                cuda_ok: true,
            }),
            extra: vec![("live server".into(), "stopped".into())],
            engine_stderr: Some("C:/x/engine.log\nline 1\nline 2\nline 3".into()),
            stderr_lines: 2,
            checks: vec![HealthCheck::new("disk", "Free disk", "pass", "80 GB free")],
        };
        let text = report(&DataPaths::at("D:/data"), &store, &input);
        for want in [
            "Diarist 0.1.0",
            "driver 610.88 | CUDA 13.1",
            "data root: D:/data [explicit",
            "engine: nemo-speech-0.2.0 0.2.0 | engine/nemo-speech-0.2.0",
            "asr: en.gguf | models/en.gguf | 42 bytes | sha256 abc123 | file lookahead runtime default",
            "diarizer: none selected",
            "live server: stopped",
            "checks:\n  [pass] Free disk: 80 GB free",
            "last engine stderr (C:/x/engine.log, last 2 lines):\n  line 2\n  line 3",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
        assert!(!text.contains("line 1"));
        assert!(text.contains("crash reports: none"), "{text}");
        // A streaming model with a file lookahead shows the value imports pass.
        let mut asr = row("models/en.gguf", ModelRole::Asr, "x", Some("abc123"));
        asr.capabilities.streaming = true;
        asr.capabilities.file_right_context = Some(13);
        let engine = row("engine/nemo-speech-0.2.0", ModelRole::Engine, "0.2.0", None);
        // And a diarizer with a file preset, the one imports pass.
        let mut diar = row(
            "models/d.gguf",
            ModelRole::Diarizer,
            "sortformer",
            Some("d1"),
        );
        diar.capabilities.file_preset = Some("v3-offline".into());
        store.sync_model_files(&[engine, asr, diar], 2).unwrap();
        select(&store, ModelRole::Diarizer, "d.gguf").unwrap();
        let text = report(&DataPaths::at("D:/data"), &store, &input);
        assert!(
            text.contains("sha256 abc123 | file lookahead 13\n"),
            "{text}"
        );
        assert!(
            text.contains("diarizer: d.gguf | models/d.gguf | 42 bytes | sha256 d1 | file preset v3-offline\n"),
            "{text}"
        );
        // The device jobs pass, from the package; `auto` when nothing tells.
        assert!(
            text.contains("engine/nemo-speech-0.2.0 | 42 bytes | device auto\n"),
            "{text}"
        );
        let mut vulkan = row(
            "engine/nemo-speech-0.2.0-windows-x86_64-vulkan",
            ModelRole::Engine,
            "0.2.0",
            None,
        );
        vulkan.quant = Some("vulkan".into());
        store.sync_model_files(&[vulkan], 3).unwrap();
        select(
            &store,
            ModelRole::Engine,
            "nemo-speech-0.2.0-windows-x86_64-vulkan",
        )
        .unwrap();
        let text = report(&DataPaths::at("D:/data"), &store, &input);
        assert!(
            text.contains("| 42 bytes | device vulkan:0 (vulkan package)\n"),
            "{text}"
        );
        if cfg!(windows) {
            assert!(os_version().starts_with("Windows"), "{}", os_version());
        }
    }

    fn rtx(cuda_ok: bool) -> GpuInfo {
        GpuInfo {
            name: "NVIDIA GeForce RTX 3080".into(),
            vram_total_mb: 10_240,
            vram_free_mb: 8_000,
            driver_version: "610.88".into(),
            cuda_version: Some(if cuda_ok { "13.1" } else { "12.4" }.into()),
            cuda_ok,
        }
    }

    #[test]
    fn the_gpu_row_follows_the_selected_package() {
        let amd = [GpuAdapter {
            name: "AMD Radeon RX 7800 XT".into(),
            vendor: "amd".into(),
            vram_mb: 16_368,
        }];
        let status = |b, g: Option<&GpuInfo>, a: &[GpuAdapter]| gpu_check(b, g, a).status;
        let (new, old) = (rtx(true), rtx(false));
        // CUDA: an NVIDIA GPU with CUDA 13, else a failure naming the fix.
        assert_eq!(status(Backend::Cuda, Some(&new), &[]), "pass");
        assert_eq!(status(Backend::Cuda, Some(&old), &[]), "fail");
        assert_eq!(status(Backend::Cuda, None, &amd), "fail");
        assert_eq!(status(Backend::Cuda, None, &[]), "fail");
        // Vulkan: any GPU passes, the old NVIDIA driver and AMD included;
        // "no CUDA" is not a failure.
        for (g, a) in [
            (Some(&new), &[][..]),
            (Some(&old), &[][..]),
            (None, &amd[..]),
        ] {
            let c = gpu_check(Backend::Vulkan, g, a);
            assert_eq!(c.status, "pass", "{c:?}");
            assert!(c.detail.contains("vulkan:0"), "{c:?}");
            assert!(!c.detail.contains("needs CUDA 13"), "{c:?}");
        }
        assert!(gpu_check(Backend::Vulkan, None, &amd)
            .detail
            .starts_with("AMD Radeon RX 7800 XT, 16368 MB"));
        assert_eq!(status(Backend::Vulkan, None, &[]), "fail");
        // CPU: a warning that it is slow, never a CUDA failure.
        for (g, a) in [(None, &[][..]), (Some(&old), &[][..]), (None, &amd[..])] {
            let c = gpu_check(Backend::Cpu, g, a);
            assert_eq!(c.status, "warn", "{c:?}");
            assert!(c.detail.starts_with("The CPU runtime is slow"), "{c:?}");
            assert!(!c.detail.contains("needs CUDA 13"), "{c:?}");
        }
        assert!(gpu_check(Backend::Cpu, Some(&new), &[])
            .detail
            .ends_with("the CUDA runtime is much faster."));
        assert!(gpu_check(Backend::Cpu, None, &amd)
            .detail
            .ends_with("the Vulkan runtime is much faster."));
    }

    #[test]
    fn the_vram_rows_skip_for_cpu_and_without_nvml() {
        let ids = |v: Vec<HealthCheck>| -> Vec<(String, String)> {
            v.into_iter().map(|c| (c.id, c.status)).collect()
        };
        let pair = |a: &str, b: &str, c: &str, d: &str| {
            vec![(a.to_owned(), b.to_owned()), (c.to_owned(), d.to_owned())]
        };
        // A GPU package with NVML: the live gate and the file model's need.
        for b in [Backend::Cuda, Backend::Vulkan] {
            assert_eq!(
                ids(vram_checks(b, Some(9_000), Some(2_600))),
                pair("vram_live", "pass", "vram_file", "pass")
            );
            assert_eq!(
                ids(vram_checks(b, Some(2_000), Some(2_600))),
                pair("vram_live", "warn", "vram_file", "warn")
            );
        }
        // Without NVML (AMD, Intel): unknown, skipped; never a block.
        let v = vram_checks(Backend::Vulkan, None, Some(2_600));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].status, "skip");
        assert!(v[0].detail.contains("without the check"), "{v:?}");
        // CPU: no GPU memory, whatever NVML says.
        for free in [None, Some(500), Some(9_000)] {
            let v = vram_checks(Backend::Cpu, free, Some(2_600));
            assert_eq!(v.len(), 1);
            assert_eq!(
                (v[0].id.as_str(), v[0].status.as_str()),
                ("vram_live", "skip")
            );
            assert!(v[0].detail.contains("system memory"), "{v:?}");
        }
    }

    #[test]
    fn mic_privacy_fails_when_either_switch_is_deny() {
        let consent = |a: Option<&str>, d: Option<&str>| MicConsent {
            all_apps: a.map(Into::into),
            desktop_apps: d.map(Into::into),
        };
        let allowed = mic_privacy_check(&consent(Some("Allow"), Some("Allow")));
        assert_eq!(
            (allowed.id.as_str(), allowed.status.as_str()),
            ("mic_privacy", "pass")
        );
        assert_eq!(allowed.fix, None);
        let missing = mic_privacy_check(&consent(None, None));
        assert_eq!(missing.status, "pass", "{missing:?}");
        assert!(
            missing.detail.contains("No Windows privacy setting"),
            "{missing:?}"
        );
        // One of the two missing: the other decides.
        assert_eq!(
            mic_privacy_check(&consent(None, Some("Allow"))).status,
            "pass"
        );
        assert_eq!(
            mic_privacy_check(&consent(None, Some("Deny"))).status,
            "fail"
        );
        for (a, d, names) in [
            (Some("Deny"), Some("Allow"), "Microphone access is off"),
            (
                Some("Allow"),
                Some("Deny"),
                "Let desktop apps access your microphone is off",
            ),
            (Some("Deny"), Some("Deny"), "Microphone access is off"),
            (Some(" deny "), None, "Microphone access is off"),
        ] {
            let c = mic_privacy_check(&consent(a, d));
            assert_eq!(c.status, "fail", "{a:?}/{d:?}");
            assert!(
                c.detail
                    .contains("does not allow desktop apps to use the microphone"),
                "{c:?}"
            );
            assert!(c.detail.contains(names), "{a:?}/{d:?}: {c:?}");
            assert_eq!(c.fix.as_deref(), Some("mic_privacy"));
            assert_eq!(c.fix_label.as_deref(), Some("Privacy settings"));
        }
        // An unexpected value is not a block.
        assert_eq!(
            mic_privacy_check(&consent(Some("Prompt"), Some(""))).status,
            "pass"
        );
        // This PC's own values read without failing, whatever they are.
        let _ = mic_privacy_check(&mic_consent());
    }

    #[test]
    fn the_echo_row_says_whether_it_is_on_and_what_the_last_recording_did() {
        let on = echo_check(EchoMode::Auto, None);
        assert_eq!((on.id.as_str(), on.status.as_str()), ("echo", "pass"));
        assert_eq!(on.label, "Recording with speakers");
        assert!(
            on.detail.starts_with("Echo handling is on"),
            "{}",
            on.detail
        );
        assert_eq!(on.fix.as_deref(), Some("settings:echo"));
        let off = echo_check(EchoMode::Off, None);
        assert_eq!(off.status, "skip");
        assert!(
            off.detail.starts_with("Echo handling is off"),
            "{}",
            off.detail
        );
        assert_eq!(off.fix.as_deref(), Some("settings:echo"));
        assert!(echo_check(EchoMode::Always, None)
            .detail
            .starts_with("Echo handling is always on"));
        let note = |e: serde_json::Value| echo_check(EchoMode::Auto, Some(&e)).detail;
        let removed = note(serde_json::json!({
            "coupled": true, "coupling": 0.7213, "lag_ms": 3.25, "dropped": 312
        }));
        assert!(
            removed.ends_with(
                "In the last recording the microphone heard the speakers (coupling 0.72, 3 ms): \
                 312 far-end words removed from \"me\" (Restore in that meeting's details puts \
                 them back)."
            ),
            "{removed}"
        );
        let restored = note(serde_json::json!({
            "coupled": true, "coupling": 0.7, "restored": true, "restored_words": 1, "dropped": 0
        }));
        assert!(
            restored.ends_with("(coupling 0.70); its 1 removed word was put back with Restore."),
            "{restored}"
        );
        let user_off = note(serde_json::json!({
            "coupled": true, "coupling": 0.7, "user_off": true, "dropped": 0
        }));
        assert!(
            user_off.ends_with("echo handling was turned off for it, so nothing was removed."),
            "{user_off}"
        );
        let clear = note(serde_json::json!({"coupled": false, "coupling": 0.04, "dropped": 0}));
        assert!(
            clear.ends_with(" The last recording had no echo."),
            "{clear}"
        );
        assert!(note(serde_json::json!({})).ends_with("from \"me\"."));
    }

    #[test]
    fn scrub_replaces_the_profile_folder_in_every_spelling() {
        let p = r"C:\Users\Ann Lee";
        let cases = [
            (
                r"log C:\Users\Ann Lee\AppData\x.log",
                r"log ~\AppData\x.log",
            ),
            ("json \"c:/users/ann lee/a\"", "json \"~/a\""),
            (
                r#"$ "C:\\Users\\Ann Lee\\engine\\bin""#,
                r#"$ "~\\engine\\bin""#,
            ),
            (r"\\?\C:\Users\Ann Lee\models", r"\\?\~\models"),
            ("end C:/Users/Ann Lee", "end ~"),
            // Not the profile: another user, or a longer name.
            (r"C:\Users\Ann Leez\x", r"C:\Users\Ann Leez\x"),
            (r"C:\Users\Bob\x", r"C:\Users\Bob\x"),
            (r"XC:\Users\Ann Lee", r"XC:\Users\Ann Lee"),
            ("Jürgen C:\\Users\\Ann Lee", "Jürgen ~"),
        ];
        for (input, want) in cases {
            assert_eq!(scrub_path(input, p), want, "{input}");
        }
        assert_eq!(scrub_path(r"C:\Users\Jürgen\x", r"C:\Users\Jürgen"), r"~\x");
        assert_eq!(scrub_path(r"C:\x", "C:"), r"C:\x");
        assert_eq!(scrub_path("", p), "");
    }

    #[test]
    fn the_punctuation_row_skips_without_a_model_and_names_a_missing_library() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let store = MemoryStore::new();
        let c = punctuation_check(&paths, &store);
        assert_eq!((c.id.as_str(), c.status.as_str()), ("punctuation", "skip"));
        assert!(c.detail.contains("No punctuation model"), "{}", c.detail);
        store
            .set_setting(crate::settings::TRANSCRIPT_PUNCTUATION, "off")
            .unwrap();
        assert!(punctuation_check(&paths, &store).detail.contains("is off"));
        store
            .set_setting(crate::settings::TRANSCRIPT_PUNCTUATION, "auto")
            .unwrap();

        // A dropped-in model without its ONNX Runtime library.
        let root = paths.models().join("punct");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("model.onnx"), b"x").unwrap();
        std::fs::write(root.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(
            root.join("model.onnx.meet.json"),
            r#"{"punctuation": {"tokenizer": "tokenizer.json", "max_tokens": 64,
                "heads": [{"output": "logits", "kind": "post", "labels": ["", "."],
                           "position": "last_token", "values": "logits"}]}}"#,
        )
        .unwrap();
        crate::models::scan(&paths, &store, &crate::models::Manifest::builtin()).unwrap();
        let c = punctuation_check(&paths, &store);
        assert_eq!(c.status, "fail", "{}", c.detail);
        assert!(c.detail.contains("diarist-onnxruntime.dll"), "{}", c.detail);
        assert_eq!(c.fix.as_deref(), Some("open_models"));
        // With a library file there (not loaded in this test process).
        std::fs::write(root.join("diarist-onnxruntime.dll"), b"MZ").unwrap();
        let c = punctuation_check(&paths, &store);
        if crate::punct::onnx::missing_dependency().is_none()
            && crate::punct::onnx::runtime_status() == crate::punct::onnx::RuntimeStatus::NotLoaded
        {
            assert_eq!(c.status, "pass", "{}", c.detail);
            assert!(
                c.detail.contains("models/punct/diarist-onnxruntime.dll"),
                "{}",
                c.detail
            );
        }
    }
}
