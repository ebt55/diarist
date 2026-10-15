//! Free GPU memory for the live start gate (`docs/01`, "Capture
//! robustness"): NVML through `nvml-wrapper` (the driver's `nvml.dll`,
//! loaded at run time, queried in microseconds), and `nemo-speech doctor`
//! when NVML is not available. The reading is used as it is: nothing is
//! added or subtracted for the live server.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nvml_wrapper::Nvml;
use serde::Serialize;
use ts_rs::TS;

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

/// The oldest CUDA driver API the runtime's CUDA package runs on (CUDA 13,
/// driver R580 or newer; docs/04 section 4).
pub const MIN_CUDA_DRIVER: i32 = 13_000;

/// One NVIDIA GPU as NVML describes it (first-run check, diagnostics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct GpuInfo {
    /// e.g. "NVIDIA GeForce RTX 3080".
    pub name: String,
    pub vram_total_mb: u32,
    pub vram_free_mb: u32,
    /// Display driver version, e.g. "610.88".
    pub driver_version: String,
    /// Highest CUDA version the driver supports, e.g. "13.1".
    pub cuda_version: Option<String>,
    /// The driver is new enough for the CUDA package.
    pub cuda_ok: bool,
}

/// GPU `index` from NVML (`None` without an NVIDIA driver).
pub fn gpu_info(index: u32) -> Option<GpuInfo> {
    let nvml = nvml()?;
    let dev = nvml.device_by_index(index).ok()?;
    let mem = dev.memory_info().ok()?;
    let cuda = nvml.sys_cuda_driver_version().ok();
    Some(GpuInfo {
        name: dev.name().unwrap_or_else(|_| "NVIDIA GPU".into()),
        vram_total_mb: (mem.total >> 20) as u32,
        vram_free_mb: (mem.free >> 20) as u32,
        driver_version: nvml.sys_driver_version().unwrap_or_default(),
        cuda_version: cuda.map(|v| {
            format!(
                "{}.{}",
                nvml_wrapper::cuda_driver_version_major(v),
                nvml_wrapper::cuda_driver_version_minor(v)
            )
        }),
        cuda_ok: cuda.is_some_and(|v| v >= MIN_CUDA_DRIVER),
    })
}

/// A display adapter as Windows lists it (DXGI), for choosing the runtime
/// package before any runtime is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct GpuAdapter {
    pub name: String,
    /// `nvidia`, `amd`, `intel` or `other`.
    pub vendor: String,
    pub vram_mb: u32,
}

/// Hardware display adapters, largest dedicated memory first (software
/// adapters such as the Basic Render Driver are skipped).
pub fn adapters() -> Vec<GpuAdapter> {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
        const SOFTWARE: u32 = 2; // DXGI_ADAPTER_FLAG_SOFTWARE
        let mut out = Vec::new();
        let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
            return out;
        };
        for i in 0..16 {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
                break;
            };
            let Ok(d) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            if d.Flags & SOFTWARE != 0 || d.VendorId == 0x1414 {
                continue; // Microsoft Basic Render Driver
            }
            let len = d.Description.iter().position(|&c| c == 0).unwrap_or(128);
            out.push(GpuAdapter {
                name: String::from_utf16_lossy(&d.Description[..len])
                    .trim()
                    .to_owned(),
                vendor: match d.VendorId {
                    0x10DE => "nvidia",
                    0x1002 | 0x1022 => "amd",
                    0x8086 => "intel",
                    _ => "other",
                }
                .to_owned(),
                vram_mb: (d.DedicatedVideoMemory >> 20) as u32,
            });
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.vram_mb));
        out
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The runtime package for this machine: `cuda` with an NVIDIA GPU whose
/// driver supports CUDA 13, `vulkan` with another GPU (or an older NVIDIA
/// driver), else `cpu`.
pub fn recommended_backend(gpu: Option<&GpuInfo>, adapters: &[GpuAdapter]) -> &'static str {
    if gpu.is_some_and(|g| g.cuda_ok) {
        "cuda"
    } else if !adapters.is_empty() || gpu.is_some() {
        "vulkan"
    } else {
        "cpu"
    }
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
            let g = gpu_info(0).unwrap();
            assert!(g.name.contains("NVIDIA") || !g.name.is_empty(), "{g:?}");
            assert!(!g.driver_version.is_empty(), "{g:?}");
            assert_eq!(g.vram_total_mb, total);
            let a = adapters();
            assert!(a.iter().any(|x| x.vendor == "nvidia"), "{a:?}");
            assert_eq!(
                recommended_backend(Some(&g), &a),
                if g.cuda_ok { "cuda" } else { "vulkan" }
            );
        }
        assert_eq!(recommended_backend(None, &[]), "cpu");
        let amd = GpuAdapter {
            name: "AMD Radeon".into(),
            vendor: "amd".into(),
            vram_mb: 8_000,
        };
        assert_eq!(recommended_backend(None, &[amd]), "vulkan");
    }
}
