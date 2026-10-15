//! Engines and models, ComfyUI-style (`docs/01`, "Model management" and
//! "Runtime management").
//!
//! - [`paths`]: the data root (portable marker, registry, default) and the
//!   `models\` and `engine\` folders under it.
//! - [`gguf`]: header-only GGUF reader; [`classify`]: role and capabilities
//!   from the header, with `<file>.meet.json` overrides.
//! - [`scan`]: walks `models\` and `engine\`, syncs `model_files` and keeps
//!   the per-role selections (settings `selected_model.*`, `selected_engine`)
//!   pointing at present files.
//! - [`manifest`]: the recommended lists in `manifests/`.
//! - [`download`]: engine zip download (resume, SHA-256, atomic rename) and
//!   extraction; [`pull`]: `nemo-speech pull` for models, verified against
//!   the manifest; [`doctor`]: runtime version and GPU.
//! - [`status`] assembles the Settings screen's [`EngineStatus`];
//!   [`resolve_selection`] gives the engine and model paths to run with.

pub mod classify;
pub mod doctor;
pub mod download;
pub mod gguf;
pub mod manifest;
pub mod paths;
pub mod pull;
pub mod scan;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub use classify::{classify, Classification, Sidecar};
pub use doctor::{doctor, DoctorReport};
pub use download::{download_file, install_engine, sha256_file, verify_file};
pub use gguf::{read_header, GgufHeader, GgufValue};
pub use manifest::{EngineEntry, Manifest, ModelEntry, TESTED_ENGINE_VERSIONS};
pub use paths::{free_disk_bytes, DataPaths, DataRootSource};
pub use pull::{pull, Pulled};
pub use scan::{ensure_selections, recommended, scan, ScanReport};

use crate::error::{CoreError, Result};
use crate::store::Store;
use crate::types::{ModelFile, ModelFileId, ModelRole};
use crate::views::{selection_key, EngineStatus};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The engine, ASR model and diarizer currently selected, as absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSelection {
    pub engine_dir: PathBuf,
    pub engine_version: Option<String>,
    pub asr: PathBuf,
    pub asr_file: ModelFile,
    pub diarizer: PathBuf,
    pub diarizer_file: ModelFile,
}

/// The selected file of `role`, if it is set and present.
pub fn selected_file(store: &dyn Store, role: ModelRole) -> Result<Option<ModelFile>> {
    let settings = store.get_settings()?;
    let Some(id) = settings
        .0
        .get(&selection_key(role))
        .and_then(|v| v.parse::<i64>().ok())
        .map(ModelFileId)
    else {
        return Ok(None);
    };
    Ok(store
        .list_model_files()?
        .into_iter()
        .find(|f| f.id == id && f.role == role && !f.missing))
}

/// Resolves the selected engine, ASR model and diarizer to paths. Errors
/// name what is missing and how to fix it.
pub fn resolve_selection(paths: &DataPaths, store: &dyn Store) -> Result<ResolvedSelection> {
    let need = |role: ModelRole| -> Result<ModelFile> {
        selected_file(store, role)?.ok_or_else(|| {
            CoreError::Models(format!(
                "no {role} selected: put one under {} and run `meet-cli models scan`{}",
                if role == ModelRole::Engine {
                    paths.engine().display().to_string()
                } else {
                    paths.models().display().to_string()
                },
                if role == ModelRole::Engine {
                    String::new()
                } else {
                    " (or `meet-cli models pull`)".to_owned()
                }
            ))
        })
    };
    let engine = need(ModelRole::Engine)?;
    let asr = need(ModelRole::Asr)?;
    let diar = need(ModelRole::Diarizer)?;
    Ok(ResolvedSelection {
        engine_dir: paths.absolute(&engine.path),
        engine_version: engine.architecture.clone(),
        asr: paths.absolute(&asr.path),
        asr_file: asr,
        diarizer: paths.absolute(&diar.path),
        diarizer_file: diar,
    })
}

/// Selects a file for `role` by id, path (or path suffix), manifest name,
/// display name or file name. Returns the selected file.
pub fn select(store: &dyn Store, role: ModelRole, query: &str) -> Result<ModelFile> {
    let q = query.trim();
    let files: Vec<ModelFile> = store
        .list_model_files()?
        .into_iter()
        .filter(|f| f.role == role && !f.missing)
        .collect();
    let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    let qn = norm(q);
    let hit = files
        .iter()
        .find(|f| q.parse::<i64>().ok() == Some(f.id.0))
        .or_else(|| files.iter().find(|f| norm(&f.path) == qn))
        .or_else(|| {
            files.iter().find(|f| {
                f.manifest_name
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(q))
            })
        })
        .or_else(|| {
            files
                .iter()
                .find(|f| f.display_name.eq_ignore_ascii_case(q))
        })
        .or_else(|| {
            files.iter().find(|f| {
                let p = norm(&f.path);
                p.ends_with(&format!("/{qn}")) || p.rsplit('/').next() == Some(qn.as_str())
            })
        })
        .or_else(|| {
            files
                .iter()
                .find(|f| f.architecture.as_deref() == Some(q) && role == ModelRole::Engine)
        })
        .cloned()
        .ok_or_else(|| {
            CoreError::Models(format!(
                "no {role} matches {q:?}; candidates: {}",
                files
                    .iter()
                    .map(|f| format!("{} ({})", f.display_name, f.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    store.set_setting(&selection_key(role), &hit.id.to_string())?;
    Ok(hit)
}

/// The Settings screen's state. `doctor` (optional, it spawns the runtime)
/// supplies GPU and free VRAM.
pub fn status(
    paths: &DataPaths,
    store: &dyn Store,
    doctor: Option<&DoctorReport>,
) -> Result<EngineStatus> {
    Ok(EngineStatus::from_parts(
        paths.root.display().to_string(),
        store.list_model_files()?,
        &store.get_settings()?,
        TESTED_ENGINE_VERSIONS,
        doctor.and_then(DoctorReport::free_vram_mb),
        doctor.and_then(DoctorReport::device_info),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, ScannedModelFile};
    use crate::types::{ModelCapabilities, ModelSource};

    fn scanned(path: &str, role: ModelRole, name: &str) -> ScannedModelFile {
        ScannedModelFile {
            path: path.into(),
            role,
            architecture: Some(
                if role == ModelRole::Engine {
                    "0.2.0"
                } else {
                    "asr"
                }
                .into(),
            ),
            display_name: name.into(),
            quant: None,
            size_bytes: 1,
            sha256: None,
            source: ModelSource::User,
            capabilities: ModelCapabilities {
                streaming: true,
                ..Default::default()
            },
            manifest_name: None,
        }
    }

    #[test]
    fn select_resolve_and_status() {
        let store = MemoryStore::new();
        let paths = DataPaths::at("D:/data");
        store
            .sync_model_files(
                &[
                    scanned(
                        "engine/nemo-speech-0.2.0",
                        ModelRole::Engine,
                        "nemo-speech-0.2.0",
                    ),
                    scanned("models/a/one.gguf", ModelRole::Asr, "one"),
                    scanned("models/b/two.gguf", ModelRole::Asr, "two"),
                    scanned("models/d.gguf", ModelRole::Diarizer, "diar"),
                ],
                1,
            )
            .unwrap();
        assert!(resolve_selection(&paths, &store).is_err());
        select(&store, ModelRole::Engine, "0.2.0").unwrap();
        select(&store, ModelRole::Asr, "two.gguf").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        assert!(select(&store, ModelRole::Asr, "diar").is_err());
        let r = resolve_selection(&paths, &store).unwrap();
        assert_eq!(
            r.asr,
            PathBuf::from("D:/data")
                .join("models")
                .join("b")
                .join("two.gguf")
        );
        assert_eq!(r.engine_version.as_deref(), Some("0.2.0"));
        let st = status(&paths, &store, None).unwrap();
        assert_eq!(st.engines.len(), 1);
        assert!(st.engines[0].tested);
        assert_eq!(st.selected.asr, Some(r.asr_file.id));
        assert_eq!(st.models_by_role[&ModelRole::Asr].len(), 2);
    }
}
