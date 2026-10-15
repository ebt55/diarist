//! The first-run check (`docs/plans/06`, deliverable 2) and the per-action
//! readiness check (docs/00, "nobody fights the app"): is there an engine,
//! a speech-to-text model and a diarizer; which runtime package fits this
//! GPU; what the downloads are (URL, hash, folder); does the disk fit. No
//! check is a gate: the UI shows what is missing next to a one-click fix.

use serde::Serialize;
use ts_rs::TS;

use super::manifest::Manifest;
use super::paths::{free_disk_bytes, DataPaths};
use super::selected_file;
use crate::error::Result;
use crate::gpu::{recommended_backend, GpuAdapter, GpuInfo};
use crate::store::Store;
use crate::types::{ModelFile, ModelRole};
use crate::views::{DownloadItem, DownloadKind};

/// What the first-run screen shows (`setup_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    /// No engine, speech-to-text model or diarizer: show the setup screen.
    pub needs_setup: bool,
    pub engine_ready: bool,
    pub asr_ready: bool,
    pub diarizer_ready: bool,
    pub data_root: String,
    pub engine_dir: String,
    pub models_dir: String,
    /// The first NVIDIA GPU (NVML); `None` without an NVIDIA driver.
    pub gpu: Option<GpuInfo>,
    /// Display adapters of any vendor (DXGI).
    pub adapters: Vec<GpuAdapter>,
    /// The runtime package this machine should use: `cuda`, `vulkan`, `cpu`.
    pub recommended_backend: String,
    /// Free bytes on the data root's volume, when known.
    pub disk_free_bytes: Option<u64>,
    /// Bytes the missing required downloads need on disk at their peak
    /// (the engine zip and its extraction, the models).
    pub disk_needed_bytes: u64,
    /// Every runtime package, then the models: required ones first.
    pub items: Vec<DownloadItem>,
}

/// The extracted runtime is about 1.4 times its zip (98 MB to 134 MB).
const EXTRACTION_FACTOR: f64 = 1.4;

/// Every manifest download with whether it is present: the runtime
/// packages (the one for `backend` marked required), then the required
/// models, then the optional ones (experimental last).
pub fn download_items(
    paths: &DataPaths,
    manifest: &Manifest,
    files: &[ModelFile],
    backend: &str,
) -> Vec<DownloadItem> {
    let mut items = Vec::new();
    let recommended = manifest.engine_for_backend(backend).map(|e| e.name.clone());
    for e in &manifest.engines {
        items.push(DownloadItem {
            kind: DownloadKind::Engine,
            name: e.name.clone(),
            label: format!(
                "NeMo-Speech.cpp {} runtime, {}",
                e.version,
                match e.backend.as_str() {
                    "cuda" => "CUDA (NVIDIA)",
                    "vulkan" => "Vulkan (AMD, Intel, NVIDIA)",
                    "cpu" => "CPU only",
                    other => other,
                }
            ),
            size_bytes: e.size_bytes,
            installed: files.iter().any(|f| {
                f.role == ModelRole::Engine
                    && !f.missing
                    && (f.manifest_name.as_deref() == Some(e.name.as_str())
                        || f.display_name == e.name)
            }),
            required: recommended.as_deref() == Some(e.name.as_str()),
            experimental: false,
            url: e.url.clone(),
            sha256: e.sha256.clone(),
            dest: paths.engine().join(&e.name).display().to_string(),
            licence: e.licence.clone(),
            backend: Some(e.backend.clone()),
            tested: e.tested,
        });
    }
    let mut models: Vec<_> = manifest.models.iter().collect();
    models.sort_by_key(|m| (!m.required, m.experimental));
    for m in models {
        items.push(DownloadItem {
            kind: DownloadKind::Model,
            name: m.name.clone(),
            label: m
                .label
                .clone()
                .unwrap_or_else(|| format!("{} ({})", m.name, m.role)),
            size_bytes: m.size_bytes,
            installed: files
                .iter()
                .any(|f| !f.missing && f.manifest_name.as_deref() == Some(m.name.as_str())),
            required: m.required,
            experimental: m.experimental,
            url: m.url.clone(),
            sha256: m.sha256.clone(),
            dest: m.dest_dir(&paths.models()).display().to_string(),
            licence: m.licence.clone(),
            backend: None,
            tested: true,
        });
    }
    items
}

/// The first-run state from the scanned `files` (scan first), with `gpu`
/// from [`crate::gpu::gpu_info`] and `adapters` from [`crate::gpu::adapters`].
pub fn setup_status(
    paths: &DataPaths,
    manifest: &Manifest,
    files: &[ModelFile],
    gpu: Option<GpuInfo>,
    adapters: Vec<GpuAdapter>,
) -> SetupStatus {
    let present = |role: ModelRole| files.iter().any(|f| f.role == role && !f.missing);
    let (engine_ready, asr_ready, diarizer_ready) = (
        present(ModelRole::Engine),
        present(ModelRole::Asr),
        present(ModelRole::Diarizer),
    );
    let backend = recommended_backend(gpu.as_ref(), &adapters);
    let items = download_items(paths, manifest, files, backend);
    let engine_missing = !engine_ready;
    let disk_needed_bytes = items
        .iter()
        .filter(|i| i.required && !i.installed)
        .map(|i| match i.kind {
            DownloadKind::Engine if engine_missing => {
                (i.size_bytes as f64 * (1.0 + EXTRACTION_FACTOR)) as u64
            }
            DownloadKind::Engine => 0,
            DownloadKind::Model => i.size_bytes,
        })
        .sum();
    SetupStatus {
        needs_setup: !(engine_ready && asr_ready && diarizer_ready),
        engine_ready,
        asr_ready,
        diarizer_ready,
        data_root: paths.root.display().to_string(),
        engine_dir: paths.engine().display().to_string(),
        models_dir: paths.models().display().to_string(),
        gpu,
        adapters,
        recommended_backend: backend.to_owned(),
        disk_free_bytes: free_disk_bytes(&paths.root),
        disk_needed_bytes,
        items,
    }
}

/// Something an action needs that is not there, with the fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MissingPart {
    /// `engine`, `asr`, `diarizer`, `streaming` or `gpu`.
    pub what: String,
    /// What is wrong and what to do, in the user's words.
    pub message: String,
    /// The download that fixes it, if one does.
    pub item: Option<DownloadItem>,
}

/// Whether Import or Record can run now (`check_ready`), and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct Readiness {
    /// `import` or `record`.
    pub action: String,
    pub ok: bool,
    pub missing: Vec<MissingPart>,
}

/// What `action` (`import` or `record`) needs and lacks. Scan first so the
/// selections are current. Nothing here blocks the action for good: each
/// missing part names its fix.
pub fn readiness(
    paths: &DataPaths,
    store: &dyn Store,
    manifest: &Manifest,
    gpu: Option<&GpuInfo>,
    adapters: &[GpuAdapter],
    action: &str,
) -> Result<Readiness> {
    let files = store.list_model_files()?;
    let backend = recommended_backend(gpu, adapters);
    let items = download_items(paths, manifest, &files, backend);
    let item = |name: Option<&str>| {
        name.and_then(|n| items.iter().find(|i| i.name == n && !i.installed).cloned())
    };
    let required_model = |role: ModelRole| {
        manifest
            .models
            .iter()
            .find(|m| m.required && m.role == role)
            .map(|m| m.name.as_str())
    };
    let mut missing = Vec::new();

    let engine = selected_file(store, ModelRole::Engine)?;
    match &engine {
        None => missing.push(MissingPart {
            what: "engine".into(),
            message: "The speech engine is not installed. Install it here, or put an extracted \
                      runtime into the engine folder and press Refresh."
                .into(),
            item: item(
                manifest
                    .engine_for_backend(backend)
                    .map(|e| e.name.as_str()),
            ),
        }),
        Some(e) => {
            // `quant` holds the package backend for manifest runtimes.
            let cuda = e.quant.as_deref() == Some("cuda") || e.display_name.ends_with("-cuda");
            if cuda && !gpu.is_some_and(|g| g.cuda_ok) {
                missing.push(MissingPart {
                    what: "gpu".into(),
                    message: match gpu {
                        Some(g) => format!(
                            "The CUDA engine needs an NVIDIA driver with CUDA 13 (R580 or newer); {} \
                             has {}. Update the driver from nvidia.com, or install the Vulkan runtime.",
                            g.name,
                            g.cuda_version.as_deref().unwrap_or("an older one")
                        ),
                        None => "The installed engine is the CUDA build, which needs an NVIDIA GPU. \
                                 Install the Vulkan runtime (AMD, Intel) or the CPU runtime."
                            .into(),
                    },
                    item: item(manifest.engine_for_backend(backend).map(|e| e.name.as_str()))
                        .filter(|i| i.backend.as_deref() != Some("cuda")),
                });
            }
        }
    }
    let asr = selected_file(store, ModelRole::Asr)?;
    match &asr {
        None => missing.push(MissingPart {
            what: "asr".into(),
            message: "No speech-to-text model. Download the recommended one, or put a .gguf into \
                      the models folder and press Refresh."
                .into(),
            item: item(required_model(ModelRole::Asr)),
        }),
        Some(f) if action == "record" && f.role == ModelRole::Asr && !f.capabilities.streaming => {
            missing.push(MissingPart {
                what: "streaming".into(),
                message: format!(
                    "{} cannot transcribe live. Select a streaming model (Nemotron EN or 3.5) in \
                     Settings, Models.",
                    f.display_name
                ),
                item: item(required_model(ModelRole::Asr)),
            })
        }
        Some(_) => {}
    }
    if selected_file(store, ModelRole::Diarizer)?.is_none() {
        missing.push(MissingPart {
            what: "diarizer".into(),
            message: "No speaker model (diarizer). Download the recommended one, or put a .gguf \
                      into the models folder and press Refresh."
                .into(),
            item: item(required_model(ModelRole::Diarizer)),
        });
    }
    Ok(Readiness {
        action: action.to_owned(),
        ok: missing.is_empty(),
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::select;
    use crate::store::{MemoryStore, ScannedModelFile, Store};
    use crate::types::{ModelCapabilities, ModelSource};

    fn row(path: &str, role: ModelRole, arch: &str, manifest: Option<&str>) -> ScannedModelFile {
        ScannedModelFile {
            path: path.into(),
            role,
            architecture: Some(arch.into()),
            display_name: path.rsplit('/').next().unwrap().into(),
            quant: (role == ModelRole::Engine).then(|| "cuda".into()),
            size_bytes: 1,
            sha256: None,
            source: ModelSource::Manifest,
            capabilities: ModelCapabilities {
                streaming: true,
                ..Default::default()
            },
            manifest_name: manifest.map(Into::into),
        }
    }

    fn nvidia(cuda_ok: bool) -> GpuInfo {
        GpuInfo {
            name: "NVIDIA GeForce RTX 3080".into(),
            vram_total_mb: 10_240,
            vram_free_mb: 9_000,
            driver_version: "610.88".into(),
            cuda_version: Some(if cuda_ok { "13.1" } else { "12.4" }.into()),
            cuda_ok,
        }
    }

    #[test]
    fn first_run_until_engine_asr_and_diarizer_are_present() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let manifest = Manifest::builtin();
        let store = MemoryStore::new();

        let s = setup_status(
            &paths,
            &manifest,
            &store.list_model_files().unwrap(),
            Some(nvidia(true)),
            vec![],
        );
        assert!(s.needs_setup && !s.engine_ready && !s.asr_ready && !s.diarizer_ready);
        assert_eq!(s.recommended_backend, "cuda");
        let engines: Vec<_> = s
            .items
            .iter()
            .filter(|i| i.kind == DownloadKind::Engine)
            .collect();
        assert_eq!(engines.len(), 3, "cuda, vulkan, cpu");
        let rec: Vec<_> = engines.iter().filter(|i| i.required).collect();
        assert_eq!(rec.len(), 1);
        assert!(rec[0]
            .dest
            .ends_with("nemo-speech-0.2.0-windows-x86_64-cuda"));
        let required: Vec<&str> = s
            .items
            .iter()
            .filter(|i| i.required)
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(required.len(), 3, "{required:?}");
        assert!(required.contains(&"nemotron-en") && required.contains(&"nemotron-3-diarization"));
        assert!(s.items.last().unwrap().experimental, "Parakeet last");
        let models: u64 = 107_012_128 + 699_872_960;
        assert!(
            s.disk_needed_bytes > models + 102_183_465 * 2,
            "{}",
            s.disk_needed_bytes
        );
        assert!(s
            .items
            .iter()
            .all(|i| !i.url.is_empty() && i.sha256.len() == 64));
        // An AMD machine gets the Vulkan package preselected.
        let amd = GpuAdapter {
            name: "AMD Radeon RX 7600".into(),
            vendor: "amd".into(),
            vram_mb: 8_000,
        };
        let s2 = setup_status(&paths, &manifest, &[], None, vec![amd]);
        assert_eq!(s2.recommended_backend, "vulkan");
        assert!(s2
            .items
            .iter()
            .any(|i| i.required && i.backend.as_deref() == Some("vulkan")));

        store
            .sync_model_files(
                &[
                    row(
                        "engine/nemo-speech-0.2.0-windows-x86_64-cuda",
                        ModelRole::Engine,
                        "0.2.0",
                        Some("nemo-speech-0.2.0-windows-x86_64-cuda"),
                    ),
                    row(
                        "models/d.gguf",
                        ModelRole::Diarizer,
                        "sortformer",
                        Some("nemotron-3-diarization"),
                    ),
                ],
                1,
            )
            .unwrap();
        let s = setup_status(
            &paths,
            &manifest,
            &store.list_model_files().unwrap(),
            Some(nvidia(true)),
            vec![],
        );
        assert!(s.needs_setup && s.engine_ready && s.diarizer_ready && !s.asr_ready);
        assert!(s.items[0].installed);
        assert_eq!(s.disk_needed_bytes, 699_872_960);

        // Any ASR file will do, not only the recommended one.
        store
            .sync_model_files(
                &[
                    row(
                        "engine/nemo-speech-0.2.0-windows-x86_64-cuda",
                        ModelRole::Engine,
                        "0.2.0",
                        Some("nemo-speech-0.2.0-windows-x86_64-cuda"),
                    ),
                    row(
                        "models/d.gguf",
                        ModelRole::Diarizer,
                        "sortformer",
                        Some("nemotron-3-diarization"),
                    ),
                    row("models/mine.gguf", ModelRole::Asr, "fastconformer", None),
                ],
                2,
            )
            .unwrap();
        let s = setup_status(
            &paths,
            &manifest,
            &store.list_model_files().unwrap(),
            Some(nvidia(true)),
            vec![],
        );
        assert!(!s.needs_setup);
    }

    #[test]
    fn readiness_names_what_each_action_lacks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let manifest = Manifest::builtin();
        let store = MemoryStore::new();
        let g = nvidia(true);
        let r = readiness(&paths, &store, &manifest, Some(&g), &[], "import").unwrap();
        assert!(!r.ok);
        let what: Vec<&str> = r.missing.iter().map(|m| m.what.as_str()).collect();
        assert_eq!(what, ["engine", "asr", "diarizer"]);
        assert_eq!(
            r.missing[0].item.as_ref().unwrap().backend.as_deref(),
            Some("cuda")
        );
        assert_eq!(r.missing[1].item.as_ref().unwrap().name, "nemotron-en");

        let mut asr = row("models/p.gguf", ModelRole::Asr, "tdt", None);
        asr.capabilities.streaming = false;
        store
            .sync_model_files(
                &[
                    row(
                        "engine/nemo-speech-0.2.0-windows-x86_64-cuda",
                        ModelRole::Engine,
                        "0.2.0",
                        None,
                    ),
                    asr,
                    row("models/d.gguf", ModelRole::Diarizer, "sortformer", None),
                ],
                1,
            )
            .unwrap();
        select(&store, ModelRole::Engine, "0.2.0").unwrap();
        select(&store, ModelRole::Asr, "p.gguf").unwrap();
        select(&store, ModelRole::Diarizer, "d.gguf").unwrap();
        assert!(
            readiness(&paths, &store, &manifest, Some(&g), &[], "import")
                .unwrap()
                .ok
        );
        let rec = readiness(&paths, &store, &manifest, Some(&g), &[], "record").unwrap();
        assert_eq!(rec.missing.len(), 1);
        assert_eq!(rec.missing[0].what, "streaming");
        // A CUDA engine on an old driver: the Vulkan package is offered.
        let old = nvidia(false);
        let r = readiness(&paths, &store, &manifest, Some(&old), &[], "import").unwrap();
        assert_eq!(r.missing[0].what, "gpu");
        assert!(
            r.missing[0].message.contains("12.4"),
            "{}",
            r.missing[0].message
        );
        assert_eq!(
            r.missing[0].item.as_ref().unwrap().backend.as_deref(),
            Some("vulkan")
        );
    }
}
