//! The first-run check (`docs/plans/06`, deliverable 2): is there an
//! engine, a speech-to-text model and a diarizer; what would the
//! recommended downloads fetch; does the GPU and the disk fit.

use serde::Serialize;
use ts_rs::TS;

use super::manifest::Manifest;
use super::paths::{free_disk_bytes, DataPaths};
use super::pull::pull_dir;
use crate::gpu::GpuInfo;
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
    /// Free bytes on the data root's volume, when known.
    pub disk_free_bytes: Option<u64>,
    /// Bytes the missing required downloads need on disk at their peak
    /// (the engine zip and its extraction, the models).
    pub disk_needed_bytes: u64,
    /// The default engine, then the models: required ones first.
    pub items: Vec<DownloadItem>,
}

/// The extracted runtime is about 1.4 times its zip (98 MB to 134 MB).
const EXTRACTION_FACTOR: f64 = 1.4;

/// Every manifest download with whether a matching file is present: the
/// default engine first, then the required models, then the optional ones
/// (experimental last).
pub fn download_items(
    paths: &DataPaths,
    manifest: &Manifest,
    files: &[ModelFile],
) -> Vec<DownloadItem> {
    let mut items = Vec::new();
    if let Some(e) = manifest.default_engine() {
        items.push(DownloadItem {
            kind: DownloadKind::Engine,
            name: e.name.clone(),
            label: format!("NeMo-Speech.cpp {} runtime ({})", e.version, e.backend),
            size_bytes: e.size_bytes,
            installed: files.iter().any(|f| {
                f.role == ModelRole::Engine
                    && !f.missing
                    && f.architecture.as_deref() == Some(e.version.as_str())
            }),
            required: true,
            experimental: false,
            url: e.url.clone(),
            sha256: e.sha256.clone(),
            dest: paths.engine().join(&e.name).display().to_string(),
            licence: e.licence.clone(),
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
            dest: pull_dir(&paths.models(), m).display().to_string(),
            licence: m.licence.clone(),
        });
    }
    items
}

/// The first-run state from the scanned `files` (scan first), with `gpu`
/// from [`crate::gpu::gpu_info`].
pub fn setup_status(
    paths: &DataPaths,
    manifest: &Manifest,
    files: &[ModelFile],
    gpu: Option<GpuInfo>,
) -> SetupStatus {
    let present = |role: ModelRole| files.iter().any(|f| f.role == role && !f.missing);
    let (engine_ready, asr_ready, diarizer_ready) = (
        present(ModelRole::Engine),
        present(ModelRole::Asr),
        present(ModelRole::Diarizer),
    );
    let items = download_items(paths, manifest, files);
    let disk_needed_bytes = items
        .iter()
        .filter(|i| i.required && !i.installed)
        .map(|i| match i.kind {
            DownloadKind::Engine => (i.size_bytes as f64 * (1.0 + EXTRACTION_FACTOR)) as u64,
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
        disk_free_bytes: free_disk_bytes(&paths.root),
        disk_needed_bytes,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, ScannedModelFile, Store};
    use crate::types::{ModelCapabilities, ModelSource};

    fn row(path: &str, role: ModelRole, arch: &str, manifest: Option<&str>) -> ScannedModelFile {
        ScannedModelFile {
            path: path.into(),
            role,
            architecture: Some(arch.into()),
            display_name: path.into(),
            quant: None,
            size_bytes: 1,
            sha256: None,
            source: ModelSource::Manifest,
            capabilities: ModelCapabilities::default(),
            manifest_name: manifest.map(Into::into),
        }
    }

    #[test]
    fn first_run_until_engine_asr_and_diarizer_are_present() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let manifest = Manifest::builtin();
        let store = MemoryStore::new();

        let s = setup_status(&paths, &manifest, &store.list_model_files().unwrap(), None);
        assert!(s.needs_setup && !s.engine_ready && !s.asr_ready && !s.diarizer_ready);
        assert_eq!(s.items[0].kind, DownloadKind::Engine);
        assert!(s.items[0]
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
        // Engine zip and extraction plus 102 MB + 668 MB of models.
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

        store
            .sync_model_files(
                &[
                    row(
                        "engine/nemo-speech-0.2.0-windows-x86_64-cuda",
                        ModelRole::Engine,
                        "0.2.0",
                        None,
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
        let s = setup_status(&paths, &manifest, &store.list_model_files().unwrap(), None);
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
                        None,
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
        let s = setup_status(&paths, &manifest, &store.list_model_files().unwrap(), None);
        assert!(!s.needs_setup);
    }
}
