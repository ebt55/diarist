//! Free GPU memory for the live start gate (`docs/01`, "Capture
//! robustness"): NVML through `nvml-wrapper` (the driver's `nvml.dll`,
//! loaded at run time, queried in microseconds), and `nemo-speech doctor`
//! when NVML is not available. The reading is used as it is: nothing is
//! added or subtracted for the live server.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nvml_wrapper::Nvml;

use crate::error::{CoreError, Result};

/// Where a reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VramSource {
    Nvml,
    /// `nemo-speech --json doctor` (seconds, CUDA init).
    Doctor,
}

impl VramSource {
    pub fn as_str(self) -> &'static str {
        match self {
            VramSource::Nvml => "NVML",
            VramSource::Doctor => "nemo-speech doctor",
        }
    }
}

/// Free (and total) memory of one GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VramReading {
    pub free_mb: u32,
    pub total_mb: Option<u32>,
    pub source: VramSource,
}

/// Where the driver puts `nvml.dll`, most specific first (the bare name
/// last, so a stray copy elsewhere on the search path is not preferred).
fn nvml_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            v.push(PathBuf::from(root).join("System32").join("nvml.dll"));
        }
        if let Some(pf) =
            std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles"))
        {
            v.push(
                PathBuf::from(pf)
                    .join("NVIDIA Corporation")
                    .join("NVSMI")
                    .join("nvml.dll"),
            );
        }
    }
    v
}

/// The process-wide NVML handle (`None` without an NVIDIA driver).
fn nvml() -> Option<&'static Nvml> {
    static NVML: OnceLock<Option<Nvml>> = OnceLock::new();
    NVML.get_or_init(|| {
        for p in nvml_candidates() {
            if p.is_file() {
                if let Ok(n) = Nvml::builder().lib_path(p.as_os_str()).init() {
                    return Some(n);
                }
            }
        }
        Nvml::init().ok()
    })
    .as_ref()
}

/// The GPU index of a device string: `cuda:N` is `N`, anything else 0.
/// (NVML numbers GPUs by PCI bus, CUDA fastest first; on a one-GPU machine
/// they agree.)
pub fn device_index(device: &str) -> u32 {
    device
        .strip_prefix("cuda:")
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// Free and total memory of GPU `index` from NVML.
pub fn nvml_vram(index: u32) -> Option<VramReading> {
    let info = nvml()?.device_by_index(index).ok()?.memory_info().ok()?;
    Some(VramReading {
        free_mb: (info.free >> 20) as u32,
        total_mb: Some((info.total >> 20) as u32),
        source: VramSource::Nvml,
    })
}

/// Free memory from NVML, else from `nemo-speech doctor` in `engine_dir`
/// (`None` when neither can tell).
pub fn free_vram(index: u32, engine_dir: Option<&Path>) -> Option<VramReading> {
    nvml_vram(index).or_else(|| {
        let report = crate::models::doctor(engine_dir?).ok()?;
        let gpu = report.gpu()?;
        Some(VramReading {
            free_mb: (gpu.memory_free >> 20) as u32,
            total_mb: Some((gpu.memory_total >> 20) as u32),
            source: VramSource::Doctor,
        })
    })
}

/// The message of the start gate when `free_mb` is under `needed_mb`.
pub fn gate_message(free_mb: u32, needed_mb: u32) -> String {
    format!(
        "only {free_mb} MB of GPU memory is free; live transcription needs {needed_mb} MB \
         (the speech and speaker models plus room for two live sessions). Close other GPU \
         applications and try again."
    )
}

/// The start gate: an error saying how much is free and how much is needed
/// when `free_mb` is under `needed_mb`.
pub fn check_free_vram(free_mb: u32, needed_mb: u32) -> Result<()> {
    if free_mb < needed_mb {
        return Err(CoreError::Engine(gate_message(free_mb, needed_mb)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_indices_and_gate() {
        assert_eq!(device_index("cuda:0"), 0);
        assert_eq!(device_index("cuda:1"), 1);
        assert_eq!(device_index("cpu"), 0);
        assert!(check_free_vram(2_560, 2_560).is_ok());
        let e = check_free_vram(1_000, 2_560).unwrap_err().to_string();
        assert!(
            e.contains("only 1000 MB") && e.contains("needs 2560 MB"),
            "{e}"
        );
    }

    /// On a machine with an NVIDIA driver, NVML reports the GPU (the
    /// reference machine's RTX 3080); elsewhere it reports nothing.
    #[test]
    fn nvml_reading_is_plausible_when_present() {
        if let Some(r) = nvml_vram(0) {
            let total = r.total_mb.unwrap();
            assert!(r.free_mb <= total && total >= 1_024, "{r:?}");
            assert_eq!(r.source, VramSource::Nvml);
        }
    }
}
