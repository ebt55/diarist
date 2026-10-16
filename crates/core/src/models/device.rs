//! The compute device the runtime is asked for (`--device`), which follows
//! the selected runtime package's backend (docs/04 section 4): `cuda:0`
//! for the CUDA package, `vulkan:0` for Vulkan, `cpu` for CPU. The Vulkan
//! and CPU packages refuse `--device cuda:0` with exit code 2 ("this build
//! has no CUDA backend"). A runtime folder nothing tells the backend of gets
//! `auto`, the runtime's own choice.
//!
//! The backend of an engine row is its `quant` (the manifest entry's
//! `backend`, or for a folder the manifest does not know, the folder
//! name's suffix or `nemo-speech --json doctor`'s `features.backend_*`,
//! filled by the scan), else its folder name ([`engine_backend`]).
//!
//! The free-VRAM checks follow the backend too ([`VramCheck`]): CUDA reads
//! NVML, else the runtime's doctor report, as before; Vulkan reads NVML (an
//! NVIDIA card on the Vulkan package) and is skipped without it rather than
//! blocking; CPU uses no GPU memory and is never checked.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::gpu::{self, VramReading};
use crate::types::ModelFile;

/// A runtime package's compute backend (the engine manifest's `backend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Cuda,
    Vulkan,
    Cpu,
}

/// `--device auto`: the runtime chooses (a runtime whose backend is unknown).
pub const AUTO_DEVICE: &str = "auto";

/// What `diarist-cli` and the app's log say once when the CPU package runs
/// (5-minute AMI clip on a Ryzen 9 5900X, 16 Oct 2026: 3.1 times real time
/// with the streaming diarizer preset, 5.9 with the `v3-offline` file preset).
pub const CPU_SLOW_NOTICE: &str = "the CPU runtime is slow: 3 to 6 times real time \
     (measured on a Ryzen 9 5900X; an hour of audio takes 10 to 20 minutes). Imports work; \
     live recording is not recommended. With a GPU, install the CUDA (NVIDIA) or Vulkan \
     (AMD, Intel) runtime instead.";

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Cuda, Backend::Vulkan, Backend::Cpu];

    /// `cuda`, `vulkan` or `cpu` in any case; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|b| s.eq_ignore_ascii_case(b.as_str()))
    }

    /// `cuda`, `vulkan` or `cpu`, as the manifest spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Vulkan => "vulkan",
            Backend::Cpu => "cpu",
        }
    }

    /// The `--device` value for this backend's first device.
    pub fn device(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda:0",
            Backend::Vulkan => "vulkan:0",
            Backend::Cpu => "cpu",
        }
    }

    /// The backend a `--device` value asks for (`cuda[:N]`, `vulkan[:N]`,
    /// `cpu`); `None` for `auto`, `metal` or anything else.
    pub fn of_device(device: &str) -> Option<Self> {
        Self::parse(device.trim().split(':').next().unwrap_or(""))
    }

    /// The backend a runtime folder's name ends with, as the release zips
    /// name it (`nemo-speech-0.2.0-windows-x86_64-vulkan`).
    pub fn of_name(name: &str) -> Option<Self> {
        Self::parse(name.trim().rsplit(['-', '_']).next()?)
    }

    /// The backend runtime folder `dir`'s name ends with ([`Self::of_name`]).
    pub fn of_dir(dir: &Path) -> Option<Self> {
        Self::of_name(&dir.file_name()?.to_string_lossy())
    }

    /// Runs on a GPU, so the free-VRAM checks and low-VRAM mode apply.
    pub fn uses_gpu(self) -> bool {
        self != Backend::Cpu
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The backend of engine row `file`: its `quant` (set by the scan from the
/// manifest entry, the folder name or the doctor report), else the folder
/// name of its display name or path. `None` when nothing tells.
pub fn engine_backend(file: &ModelFile) -> Option<Backend> {
    file.quant
        .as_deref()
        .and_then(Backend::parse)
        .or_else(|| Backend::of_name(&file.display_name))
        .or_else(|| {
            let mut parts = file.path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']);
            Backend::of_name(parts.next()?)
        })
}

/// The `--device` value for a runtime package of `backend`: `cuda:0`,
/// `vulkan:0` or `cpu`; [`AUTO_DEVICE`] when the backend is unknown.
pub fn device_for(backend: Option<Backend>) -> &'static str {
    backend.map_or(AUTO_DEVICE, Backend::device)
}

/// The `--device` value for runtime folder `engine_dir` by its name alone
/// (no process is started): the default of the engine adapters before
/// [`crate::models::select_engine`] sets the selection's device.
pub fn device_for_dir(engine_dir: &Path) -> &'static str {
    device_for(Backend::of_dir(engine_dir))
}

/// Whether a job on `device` runs on a GPU (`auto` is assumed to).
pub fn device_uses_gpu(device: &str) -> bool {
    Backend::of_device(device).is_none_or(Backend::uses_gpu)
}

/// How the free-VRAM checks (the live start gate, Diagnostics) read free
/// GPU memory for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VramCheck {
    /// CUDA and `auto`: NVML, else `nemo-speech doctor` (seconds).
    NvmlOrDoctor,
    /// Vulkan: NVML when the GPU is an NVIDIA one; otherwise skipped, so an
    /// AMD or Intel GPU starts without the check instead of being blocked.
    NvmlOnly,
    /// CPU: no GPU memory is used; never checked.
    Skip,
}

impl VramCheck {
    /// The check for jobs on `device`.
    pub fn for_device(device: &str) -> Self {
        match Backend::of_device(device) {
            Some(Backend::Cpu) => VramCheck::Skip,
            Some(Backend::Vulkan) => VramCheck::NvmlOnly,
            Some(Backend::Cuda) | None => VramCheck::NvmlOrDoctor,
        }
    }

    /// Free VRAM for jobs on `device` (`None`: unknown or not checked; the
    /// gate then lets the job start), with `nvml` for NVML and `doctor`
    /// for the runtime's report, each called only when this check uses it.
    pub fn read_with(
        self,
        nvml: impl FnOnce() -> Option<VramReading>,
        doctor: impl FnOnce() -> Option<VramReading>,
    ) -> Option<VramReading> {
        match self {
            VramCheck::Skip => None,
            VramCheck::NvmlOnly => nvml(),
            VramCheck::NvmlOrDoctor => nvml().or_else(doctor),
        }
    }
}

/// Free VRAM for the live start gate on `device` ([`VramCheck`]), the
/// doctor fallback run from `engine_dir`.
pub fn gate_vram(device: &str, engine_dir: Option<&Path>) -> Option<VramReading> {
    let index = gpu::device_index(device);
    VramCheck::for_device(device).read_with(
        || gpu::nvml_vram(index),
        || gpu::free_vram(index, engine_dir),
    )
}

/// [`CPU_SLOW_NOTICE`] the first time per process a job runs on `device`
/// `cpu`; `None` afterwards and for every other device.
pub fn cpu_notice_once(device: &str) -> Option<&'static str> {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    (Backend::of_device(device) == Some(Backend::Cpu) && !SHOWN.swap(true, Ordering::SeqCst))
        .then_some(CPU_SLOW_NOTICE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::VramSource;
    use crate::types::{ModelFileId, ModelRole, ModelSource};

    fn engine_row(path: &str, quant: Option<&str>) -> ModelFile {
        ModelFile {
            id: ModelFileId(1),
            path: path.into(),
            role: ModelRole::Engine,
            architecture: Some("0.2.0".into()),
            display_name: path.rsplit('/').next().unwrap().into(),
            quant: quant.map(Into::into),
            size_bytes: 1,
            sha256: None,
            source: ModelSource::Manifest,
            capabilities: Default::default(),
            manifest_name: None,
            first_seen_at: 0,
            last_seen_at: 0,
            missing: false,
        }
    }

    #[test]
    fn each_backend_maps_to_its_device() {
        assert_eq!(device_for(Some(Backend::Cuda)), "cuda:0");
        assert_eq!(device_for(Some(Backend::Vulkan)), "vulkan:0");
        assert_eq!(device_for(Some(Backend::Cpu)), "cpu");
        assert_eq!(device_for(None), "auto");
        for b in Backend::ALL {
            assert_eq!(Backend::parse(b.as_str()), Some(b));
            assert_eq!(Backend::of_device(b.device()), Some(b));
        }
        assert_eq!(Backend::parse(" VULKAN "), Some(Backend::Vulkan));
        assert_eq!(Backend::parse("metal"), None);
        assert_eq!(Backend::of_device("cuda:1"), Some(Backend::Cuda));
        assert_eq!(Backend::of_device("vulkan"), Some(Backend::Vulkan));
        assert_eq!(Backend::of_device("auto"), None);
        assert!(!Backend::Cpu.uses_gpu() && Backend::Vulkan.uses_gpu());
        assert!(device_uses_gpu("auto") && device_uses_gpu("vulkan:0") && !device_uses_gpu("cpu"));
    }

    #[test]
    fn the_engine_row_says_its_backend() {
        // The manifest's backend, as the scan stores it.
        let row = engine_row("engine/nemo-speech", Some("vulkan"));
        assert_eq!(engine_backend(&row), Some(Backend::Vulkan));
        // A folder the manifest does not know: its name.
        for (name, want) in [
            ("nemo-speech-0.2.0-windows-x86_64-cuda", Some(Backend::Cuda)),
            (
                "nemo-speech-0.2.0-windows-x86_64-vulkan",
                Some(Backend::Vulkan),
            ),
            ("nemo-speech-0.3.0-windows-x86_64-CPU", Some(Backend::Cpu)),
            ("my-runtime", None),
            ("nemo-speech-0.2.0", None),
        ] {
            let row = engine_row(&format!("engine/{name}"), None);
            assert_eq!(engine_backend(&row), want, "{name}");
            assert_eq!(
                device_for(engine_backend(&row)),
                want.map_or("auto", Backend::device)
            );
        }
        // An absolute path outside the data root (the CLI's --engine-dir).
        let mut row = engine_row(
            r"D:\eval\engines\nemo-speech-0.2.0-windows-x86_64-cpu\",
            None,
        );
        row.display_name = "renamed".into();
        assert_eq!(engine_backend(&row), Some(Backend::Cpu));
        assert_eq!(
            device_for_dir(Path::new(
                "E:/engine/nemo-speech-0.2.0-windows-x86_64-vulkan"
            )),
            "vulkan:0"
        );
        assert_eq!(device_for_dir(Path::new("E:/engine")), "auto");
    }

    #[test]
    fn the_vram_gate_skips_for_cpu_and_needs_nvml_for_vulkan() {
        let reading = |source| VramReading {
            free_mb: 1_000,
            total_mb: None,
            source,
        };
        let nvml = || Some(reading(VramSource::Nvml));
        let doctor = || Some(reading(VramSource::Doctor));
        let read = |device: &str, nvml_ok: bool| {
            VramCheck::for_device(device)
                .read_with(|| nvml_ok.then(nvml).flatten(), doctor)
                .map(|r| r.source)
        };
        // CUDA (and auto): NVML, else doctor, as before.
        assert_eq!(read("cuda:0", true), Some(VramSource::Nvml));
        assert_eq!(read("cuda:0", false), Some(VramSource::Doctor));
        assert_eq!(read("auto", false), Some(VramSource::Doctor));
        // Vulkan: NVML on an NVIDIA card, else no reading (no gate).
        assert_eq!(read("vulkan:0", true), Some(VramSource::Nvml));
        assert_eq!(read("vulkan:0", false), None);
        // CPU: never read, so never gated.
        assert_eq!(VramCheck::for_device("cpu"), VramCheck::Skip);
        assert_eq!(read("cpu", true), None);
        assert!(gate_vram("cpu", None).is_none());
    }

    #[test]
    fn the_cpu_notice_shows_once() {
        assert_eq!(cpu_notice_once("cuda:0"), None);
        assert_eq!(cpu_notice_once("vulkan:0"), None);
        assert_eq!(cpu_notice_once("cpu"), Some(CPU_SLOW_NOTICE));
        assert_eq!(cpu_notice_once("cpu"), None);
        assert!(CPU_SLOW_NOTICE.contains("3 to 6 times real time"));
        assert!(CPU_SLOW_NOTICE.contains("live recording is not recommended"));
    }
}
