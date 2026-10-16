//! `nemo-speech --json doctor`: runtime version, GPU and free VRAM
//! (research/08 section 5).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::device::Backend;
use crate::engine::process;
use crate::error::{CoreError, Result};
use crate::views::DeviceInfo;

/// One device as the runtime sees it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DoctorDevice {
    pub index: u32,
    /// `CUDA0`, `CPU`, ...
    pub name: String,
    /// e.g. `NVIDIA GeForce RTX 3080`.
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub memory_free: u64,
    #[serde(default)]
    pub memory_total: u64,
}

/// The parts of the doctor report we use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DoctorReport {
    pub version: String,
    #[serde(default)]
    pub accelerator_available: bool,
    /// A GPU backend is compiled in (`false` for the CPU package).
    #[serde(default)]
    pub accelerator_compiled: Option<bool>,
    #[serde(default)]
    pub driver_runtime_compatible: bool,
    #[serde(default)]
    pub devices: Vec<DoctorDevice>,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    pub runtime_warnings: Vec<String>,
}

impl DoctorReport {
    /// Parses the JSON object printed on stdout.
    pub fn parse(json: &str) -> Result<Self> {
        let start = json.find('{').unwrap_or(0);
        let end = json.rfind('}').map(|i| i + 1).unwrap_or(json.len());
        serde_json::from_str(&json[start..end])
            .map_err(|e| CoreError::Engine(format!("doctor output: {e}")))
    }

    /// The first GPU.
    pub fn gpu(&self) -> Option<&DoctorDevice> {
        self.devices.iter().find(|d| d.kind == "gpu")
    }

    /// The build's backend from `features.backend_*`: CUDA, Vulkan, or CPU
    /// when no GPU backend is compiled in (every `backend_*` flag false, or
    /// `accelerator_compiled` false). `None` for Metal or a report without
    /// those fields.
    pub fn backend(&self) -> Option<Backend> {
        let flag = |k: &str| self.features.get(k).copied();
        if flag("backend_cuda") == Some(true) {
            return Some(Backend::Cuda);
        }
        if flag("backend_vulkan") == Some(true) {
            return Some(Backend::Vulkan);
        }
        if flag("backend_metal") == Some(true) {
            return None;
        }
        let flags = self.features.keys().any(|k| k.starts_with("backend_"));
        (flags || self.accelerator_compiled == Some(false)).then_some(Backend::Cpu)
    }

    /// The runtime can run its package: the driver and runtime are
    /// compatible and, for a GPU build (CUDA, Vulkan), a GPU is present.
    /// The CPU build needs no GPU.
    pub fn healthy(&self) -> bool {
        self.driver_runtime_compatible
            && match self.backend() {
                Some(Backend::Cpu) => true,
                Some(_) => self.gpu().is_some(),
                None => false,
            }
    }

    pub fn free_vram_mb(&self) -> Option<u32> {
        self.gpu().map(|g| (g.memory_free >> 20) as u32)
    }

    /// The GPU as a view type.
    pub fn device_info(&self) -> Option<DeviceInfo> {
        self.gpu().map(|g| DeviceInfo {
            name: g.description.trim().to_owned(),
            vram_total_mb: Some((g.memory_total >> 20) as u32),
            vram_free_mb: Some((g.memory_free >> 20) as u32),
            driver_version: None,
        })
    }
}

/// Runs `nemo-speech --json doctor` from `engine_dir` (30 s timeout).
pub fn doctor(engine_dir: &Path) -> Result<DoctorReport> {
    let mut cmd = process::command(engine_dir, None);
    cmd.args(["--json", "doctor"]);
    let out = process::run_with_timeout(cmd, Duration::from_secs(30))?;
    if !out.status.success() {
        return Err(CoreError::Engine(format!(
            "doctor failed ({}): {}",
            out.status,
            process::tail(&String::from_utf8_lossy(&out.stderr), 2_000)
        )));
    }
    DoctorReport::parse(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "accelerator_available": true, "accelerator_compiled": true,
      "devices": [
        {"async": true, "description": "NVIDIA GeForce RTX 3080", "events": true, "index": 0,
         "memory_free": 9536798720, "memory_total": 10736893952, "name": "CUDA0", "type": "gpu"},
        {"async": false, "description": "AMD Ryzen 9 5900X 12-Core Processor            ", "events": false,
         "index": 1, "memory_free": 21714796544, "memory_total": 34280767488, "name": "CPU", "type": "cpu"}
      ],
      "driver_runtime_compatible": true,
      "features": {"asr": true, "backend_cuda": true, "diarization": true},
      "model_download": {"available": true, "executable": "C:\\Windows\\SYSTEM32\\curl.exe"},
      "runtime_warnings": [], "version": "0.2.0"
    }"#;

    #[test]
    fn parses_the_observed_report() {
        let r = DoctorReport::parse(SAMPLE).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert!(r.healthy());
        assert_eq!(r.free_vram_mb(), Some(9_095));
        let d = r.device_info().unwrap();
        assert_eq!(d.name, "NVIDIA GeForce RTX 3080");
        assert_eq!(d.vram_total_mb, Some(10_239));
        let cpu_only = DoctorReport::parse(&SAMPLE.replace("\"gpu\"", "\"cpu\"")).unwrap();
        assert!(!cpu_only.healthy());
        assert!(DoctorReport::parse("nope").is_err());
        assert_eq!(r.backend(), Some(Backend::Cuda));
    }

    /// The 0.2.0 Vulkan and CPU packages' reports on the reference PC (16
    /// Oct 2026), shortened: each says its backend and is healthy.
    #[test]
    fn vulkan_and_cpu_packages_say_their_backend() {
        let vulkan = DoctorReport::parse(
            r#"{"accelerator_available": true, "accelerator_compiled": true,
              "devices": [{"description": "NVIDIA GeForce RTX 3080", "index": 0,
                "memory_free": 9733931008, "memory_total": 10539237376, "name": "Vulkan0", "type": "gpu"}],
              "driver_runtime_compatible": true,
              "features": {"asr": true, "backend_cuda": false, "backend_metal": false, "backend_vulkan": true},
              "version": "0.2.0"}"#,
        )
        .unwrap();
        assert_eq!(vulkan.backend(), Some(Backend::Vulkan));
        assert!(vulkan.healthy());
        let cpu = DoctorReport::parse(
            r#"{"accelerator_available": false, "accelerator_compiled": false,
              "devices": [{"description": "AMD Ryzen 9 5900X", "index": 0,
                "memory_free": 17834729472, "memory_total": 34280767488, "name": "CPU", "type": "cpu"}],
              "driver_runtime_compatible": true,
              "features": {"asr": true, "backend_cuda": false, "backend_metal": false, "backend_vulkan": false},
              "version": "0.2.0"}"#,
        )
        .unwrap();
        assert_eq!(cpu.backend(), Some(Backend::Cpu));
        assert!(cpu.healthy(), "the CPU package needs no GPU");
        assert_eq!(cpu.free_vram_mb(), None);
        // A Vulkan build without a GPU cannot run.
        let no_gpu = DoctorReport {
            devices: cpu.devices.clone(),
            ..vulkan.clone()
        };
        assert!(!no_gpu.healthy());
        // An old report without the flags: unknown.
        let bare = DoctorReport::parse(r#"{"version": "0.1.0"}"#).unwrap();
        assert_eq!(bare.backend(), None);
        assert!(!bare.healthy());
    }

    /// The real runtime when `DIARIST_ENGINE_DIR` is set.
    #[test]
    fn real_doctor_when_available() {
        let Some(dir) = std::env::var_os("DIARIST_ENGINE_DIR") else {
            return;
        };
        let r = doctor(Path::new(&dir)).unwrap();
        assert!(!r.version.is_empty());
    }
}
