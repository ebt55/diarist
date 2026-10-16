//! "Copy diagnostics" (Diagnostics page): one plain-text block to paste
//! into a bug report. It names versions, the GPU, the data root and the
//! selected engine and models with their hashes; it holds no transcript
//! text, meeting titles or audio.

use std::fmt::Write as _;

use crate::gpu::GpuInfo;
use crate::models::{selected_file, DataPaths, DataRootSource};
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
    /// A UI action that fixes it: `setup`, `download:<name>`, `open_models`,
    /// `open_engine`, `settings`, `sound_settings`, `mic_privacy`, `start_engine`,
    /// `rescan`.
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
                    " (older than the CUDA 13 the runtime needs)"
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
                format!(
                    "{}{version} | {} | {} bytes{}{}",
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
            "asr: en.gguf | models/en.gguf | 42 bytes | sha256 abc123",
            "diarizer: none selected",
            "live server: stopped",
            "checks:\n  [pass] Free disk: 80 GB free",
            "last engine stderr (C:/x/engine.log, last 2 lines):\n  line 2\n  line 3",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
        assert!(!text.contains("line 1"));
        assert!(text.contains("crash reports: none"), "{text}");
        if cfg!(windows) {
            assert!(os_version().starts_with("Windows"), "{}", os_version());
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
}
