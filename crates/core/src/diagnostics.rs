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
    /// `open_engine`, `settings`, `sound_settings`, `start_engine`, `rescan`.
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
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut buf = [0u16; 128];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
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

fn source_name(s: DataRootSource) -> &'static str {
    match s {
        DataRootSource::Explicit => "explicit (DIARIST_DATA_DIR or --data-dir)",
        DataRootSource::Portable => "portable (portable.txt next to the exe)",
        DataRootSource::Registry => r"relocated (HKCU\Software\Diarist\DataRoot)",
        DataRootSource::Default => "default",
    }
}

/// The report text.
pub fn report(paths: &DataPaths, store: &dyn Store, input: &DiagnosticsInput) -> String {
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
        if cfg!(windows) {
            assert!(os_version().starts_with("Windows"), "{}", os_version());
        }
    }
}
