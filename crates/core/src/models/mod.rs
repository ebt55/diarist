//! Engines and models, ComfyUI-style (`docs/01`, "Model management" and
//! "Runtime management").
//!
//! - [`paths`]: the data root (portable marker, registry, default) and the
//!   `models\` and `engine\` folders under it; [`relocate`]: "Move data
//!   folder" (copy, verify, clean the old root up after the restart).
//! - [`gguf`]: header-only GGUF reader; [`classify`]: role and capabilities
//!   from the header, with `<file>.meet.json` overrides.
//! - [`scan`]: walks `models\` and `engine\`, syncs `model_files` and keeps
//!   the per-role selections (settings `selected_model.*`, `selected_engine`)
//!   pointing at present files.
//! - [`manifest`]: the recommended lists in `manifests/`.
//! - [`download`]: engine zip download (resume, SHA-256, atomic rename) and
//!   extraction; [`pull`]: `nemo-speech pull` for models, verified against
//!   the manifest; [`doctor`]: runtime version and GPU.
//! - [`add`]: "Add from URL" and "Add from file" for models no manifest lists.
//! - [`releases`]: newer runtimes from the NeMo-Speech.cpp GitHub releases.
//! - [`setup`]: the first-run check (engine, models, GPU, disk) and the
//!   download list with URLs and hashes.
//! - [`status`] assembles the Settings screen's [`EngineStatus`];
//!   [`resolve_selection`] gives the engine and model paths to run with and
//!   [`select_engine`] builds the file engine from them (scanning first when
//!   nothing usable is selected); [`select_by_id`] validates a pick.

pub mod add;
pub mod classify;
pub mod doctor;
pub mod download;
pub mod gguf;
pub mod manifest;
pub mod paths;
pub mod pull;
pub mod releases;
pub mod relocate;
pub mod scan;
pub mod setup;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub use classify::{classify, Classification, Sidecar};
pub use doctor::{doctor, DoctorReport};
pub use download::{download_file, install_engine, sha256_file, verify_file};
pub use gguf::{read_header, GgufHeader, GgufValue};
pub use manifest::{EngineEntry, Manifest, ModelEntry, TESTED_ENGINE_VERSIONS};
pub use paths::{free_disk_bytes, DataPaths, DataRootSource};
pub use pull::{pull, pull_dir, pull_with_progress, Pulled};
pub use scan::{ensure_selections, recommended, scan, ScanReport};
pub use setup::{download_items, readiness, setup_status, MissingPart, Readiness, SetupStatus};

use crate::engine::{DiarizeRunner, Engine, NemoCascadeEngine, NemoCliEngine};
use crate::error::{CoreError, Result};
use crate::store::{Store, StoreError};
use crate::types::{ModelFile, ModelFileId, ModelRole, ModelSource};
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
    // An unrecognised file the user chose for this role counts too.
    Ok(store
        .list_model_files()?
        .into_iter()
        .find(|f| f.id == id && (f.role == role || f.role == ModelRole::Unknown) && !f.missing))
}

/// Resolves the selected engine, ASR model and diarizer to paths. Errors
/// name what is missing and how to fix it.
pub fn resolve_selection(paths: &DataPaths, store: &dyn Store) -> Result<ResolvedSelection> {
    let need = |role: ModelRole| -> Result<ModelFile> {
        selected_file(store, role)?.ok_or_else(|| {
            CoreError::Models(format!(
                "no {role} selected: install it from the setup screen, or put one under {} and press Refresh in Settings (CLI: `diarist-cli {}`)",
                if role == ModelRole::Engine {
                    paths.engine().display().to_string()
                } else {
                    paths.models().display().to_string()
                },
                if role == ModelRole::Engine {
                    "engine install"
                } else {
                    "models pull"
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

/// The file engine for the current selection.
pub struct SelectedEngine {
    pub engine: Arc<dyn Engine>,
    /// The standalone diarization pass with the same runtime and diarizer.
    pub diarize_runner: DiarizeRunner,
    pub selection: ResolvedSelection,
    /// The ASR model is not a streaming one, so the experimental cascade
    /// (`NemoCascadeEngine`, may drop speech) runs instead of `NemoCliEngine`.
    pub experimental: bool,
}

/// Per-invocation changes to the stored selection (the CLI's `--asr-model`,
/// `--diar-model`, `--engine-arg`). Models are a file path or anything
/// [`find_file`] accepts; nothing is persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineOverrides {
    pub asr: Option<String>,
    pub diarizer: Option<String>,
    /// Flags appended verbatim to `nemo-speech transcribe`.
    pub extra_args: Vec<String>,
}

/// Builds the engine for the selected runtime, ASR model and diarizer on
/// `device` (`cuda:0`, `cpu`). When nothing usable is selected, scans the
/// models and engine folders first (which also picks the recommended
/// files: the EN model and Nemotron 3 Diarization).
pub fn select_engine(paths: &DataPaths, store: &dyn Store, device: &str) -> Result<SelectedEngine> {
    select_engine_with(paths, store, device, &EngineOverrides::default())
}

/// [`select_engine`] with per-invocation overrides.
pub fn select_engine_with(
    paths: &DataPaths,
    store: &dyn Store,
    device: &str,
    overrides: &EngineOverrides,
) -> Result<SelectedEngine> {
    let mut sel = match resolve_selection(paths, store) {
        Ok(s) => s,
        Err(_) => {
            let manifest = Manifest::load(&paths.root)?;
            scan(paths, store, &manifest)?;
            resolve_selection(paths, store)?
        }
    };
    if let Some(q) = &overrides.asr {
        (sel.asr, sel.asr_file) = resolve_model(paths, store, ModelRole::Asr, q)?;
    }
    if let Some(q) = &overrides.diarizer {
        (sel.diarizer, sel.diarizer_file) = resolve_model(paths, store, ModelRole::Diarizer, q)?;
    }
    let models_dir = paths.models();
    // A file the header did not identify is tried as a streaming model; the
    // runtime's own error is shown if it is not one.
    let experimental =
        sel.asr_file.role != ModelRole::Unknown && !sel.asr_file.capabilities.streaming;
    let engine: Arc<dyn Engine> = if experimental {
        let mut e = NemoCascadeEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.to_owned();
        e.version = sel.engine_version.clone();
        e.extra_args = overrides.extra_args.clone();
        Arc::new(e)
    } else {
        let mut e = NemoCliEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.to_owned();
        e.version = sel.engine_version.clone();
        // `--language` only for multilingual models (Nemotron 3.5); the EN
        // model ignores it. The meeting keeps its language either way.
        e.pass_language = is_multilingual(&sel.asr_file);
        e.extra_args = overrides.extra_args.clone();
        Arc::new(e)
    };
    let mut diarize_runner = DiarizeRunner::new(&sel.engine_dir, &models_dir, &sel.diarizer);
    diarize_runner.device = device.to_owned();
    Ok(SelectedEngine {
        engine,
        diarize_runner,
        selection: sel,
        experimental,
    })
}

/// The manifest model low-VRAM mode switches file imports to: Nemotron 3.5,
/// whose file pass peaks at 1.8 GB against 2.6 GB for the EN model (docs/04
/// section 4).
pub const LOW_VRAM_MODEL: &str = "nemotron-3.5";

/// The installed [`LOW_VRAM_MODEL`] file, if any.
pub fn low_vram_asr(store: &dyn Store) -> Result<Option<ModelFile>> {
    Ok(store.list_model_files()?.into_iter().find(|f| {
        f.role == ModelRole::Asr && !f.missing && f.manifest_name.as_deref() == Some(LOW_VRAM_MODEL)
    }))
}

/// The engine for a file import: the selection, except that with
/// `low_vram` set and Nemotron 3.5 installed, the import runs on Nemotron
/// 3.5 (the stored selection is unchanged). Returns whether it switched.
pub fn select_import_engine(
    paths: &DataPaths,
    store: &dyn Store,
    device: &str,
    low_vram: bool,
) -> Result<(SelectedEngine, bool)> {
    let switch = if low_vram {
        low_vram_asr(store)?.filter(|f| {
            selected_file(store, ModelRole::Asr)
                .ok()
                .flatten()
                .is_none_or(|sel| sel.id != f.id)
        })
    } else {
        None
    };
    let overrides = EngineOverrides {
        // The stored relative path: `find_file` matches it exactly.
        asr: switch.as_ref().map(|f| f.path.clone()),
        ..Default::default()
    };
    let built = select_engine_with(paths, store, device, &overrides)?;
    Ok((built, switch.is_some()))
}

/// A model given as a path (scanned or not) or a [`find_file`] query, as an
/// absolute path and its `model_files` row (a synthetic one, id 0, for an
/// unscanned file, classified from its GGUF header).
pub fn resolve_model(
    paths: &DataPaths,
    store: &dyn Store,
    role: ModelRole,
    query: &str,
) -> Result<(PathBuf, ModelFile)> {
    let p = Path::new(query.trim());
    if !p.is_file() {
        let f = find_file(store, role, query)?;
        return Ok((paths.absolute(&f.path), f));
    }
    let abs = std::path::absolute(p)?;
    let rel = paths.relative(&abs);
    if let Some(f) = store
        .list_model_files()?
        .into_iter()
        .find(|f| f.path == rel)
    {
        if f.role != role {
            return Err(CoreError::Models(format!(
                "{query} is a {} model, not {role}",
                f.role
            )));
        }
        return Ok((abs, f));
    }
    let size = std::fs::metadata(&abs)?.len();
    let c = read_header(&abs).map(|h| classify(&h, size)).ok();
    let file_role = c.as_ref().map_or(ModelRole::Unknown, |c| c.role);
    if file_role != role && file_role != ModelRole::Unknown {
        return Err(CoreError::Models(format!(
            "{query} is a {file_role} model, not {role}"
        )));
    }
    let stem = abs
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = ModelFile {
        id: ModelFileId(0),
        path: rel,
        role,
        architecture: c.as_ref().and_then(|c| c.architecture.clone()),
        display_name: c
            .as_ref()
            .and_then(|c| c.display_name.clone())
            .unwrap_or(stem),
        quant: c.as_ref().and_then(|c| c.quant.clone()),
        size_bytes: size as i64,
        sha256: None,
        source: ModelSource::User,
        capabilities: c.map(|c| c.capabilities).unwrap_or_default(),
        manifest_name: None,
        first_seen_at: 0,
        last_seen_at: 0,
        missing: false,
    };
    Ok((abs, file))
}

/// A model that understands more than one language (its header or sidecar
/// lists several), so the meeting language is worth passing to it.
pub fn is_multilingual(file: &ModelFile) -> bool {
    file.capabilities.languages.len() > 1
}

/// Selects file `id` for `role` after checking it can be: a known role,
/// a file of that role (or an unrecognised one the user forces, which the
/// runtime may refuse), present on disk. Returns the file.
pub fn select_by_id(store: &dyn Store, role: ModelRole, id: ModelFileId) -> Result<ModelFile> {
    if role == ModelRole::Unknown {
        return Err(StoreError::Invalid(
            "choose the role to use the file for (speech to text or speaker diarization)".into(),
        )
        .into());
    }
    let file = store
        .list_model_files()?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| StoreError::not_found("model file", id))?;
    if file.role != role && !(file.role == ModelRole::Unknown && role != ModelRole::Engine) {
        return Err(StoreError::Invalid(format!(
            "{} is a {} file, not {role}; pick it under its own role",
            file.display_name, file.role
        ))
        .into());
    }
    if file.missing {
        return Err(
            StoreError::Invalid(format!("{} is missing from disk", file.display_name)).into(),
        );
    }
    store.set_setting(&selection_key(role), &id.to_string())?;
    Ok(file)
}

/// Selects a file for `role` by id, path (or path suffix), manifest name,
/// display name or file name (see [`find_file`]). Returns the selected file.
pub fn select(store: &dyn Store, role: ModelRole, query: &str) -> Result<ModelFile> {
    let hit = find_file(store, role, query)?;
    store.set_setting(&selection_key(role), &hit.id.to_string())?;
    Ok(hit)
}

/// The present file of `role` matching `query`: its id, path (or path
/// suffix), manifest name, display name, file name, or (engines) version.
pub fn find_file(store: &dyn Store, role: ModelRole, query: &str) -> Result<ModelFile> {
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
    fn low_vram_mode_switches_imports_to_nemotron_35() {
        let store = MemoryStore::new();
        let paths = DataPaths::at("D:/data");
        let mut n35 = scanned("models/n35.gguf", ModelRole::Asr, "n35");
        n35.manifest_name = Some(LOW_VRAM_MODEL.into());
        store
            .sync_model_files(
                &[
                    scanned("engine/nemo-speech-0.2.0", ModelRole::Engine, "e"),
                    scanned("models/en.gguf", ModelRole::Asr, "en"),
                    scanned("models/d.gguf", ModelRole::Diarizer, "diar"),
                ],
                1,
            )
            .unwrap();
        select(&store, ModelRole::Engine, "e").unwrap();
        select(&store, ModelRole::Asr, "en").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        // Not installed: the selection runs.
        let (b, switched) = select_import_engine(&paths, &store, "cpu", true).unwrap();
        assert!(!switched && b.selection.asr_file.display_name == "en");
        store
            .sync_model_files(
                &[
                    scanned("engine/nemo-speech-0.2.0", ModelRole::Engine, "e"),
                    scanned("models/en.gguf", ModelRole::Asr, "en"),
                    scanned("models/d.gguf", ModelRole::Diarizer, "diar"),
                    n35,
                ],
                2,
            )
            .unwrap();
        let (b, switched) = select_import_engine(&paths, &store, "cpu", true).unwrap();
        assert!(switched);
        assert_eq!(b.selection.asr_file.display_name, "n35");
        let (b, switched) = select_import_engine(&paths, &store, "cpu", false).unwrap();
        assert!(!switched && b.selection.asr_file.display_name == "en");
        assert_eq!(
            selected_file(&store, ModelRole::Asr)
                .unwrap()
                .unwrap()
                .display_name,
            "en"
        );
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
        assert_eq!(st.engines[0].display_name, "nemo-speech-0.2.0");
        assert!(st.engines[0].tested);
        assert_eq!(st.selected.asr, Some(r.asr_file.id));
        assert_eq!(st.models_by_role[&ModelRole::Asr].len(), 2);

        // By id, with the checks the Settings screen relies on.
        let one = st.models_by_role[&ModelRole::Asr]
            .iter()
            .find(|f| f.display_name == "one")
            .unwrap()
            .id;
        assert_eq!(
            select_by_id(&store, ModelRole::Asr, one)
                .unwrap()
                .display_name,
            "one"
        );
        assert_eq!(
            selected_file(&store, ModelRole::Asr).unwrap().unwrap().id,
            one
        );
        assert!(select_by_id(&store, ModelRole::Diarizer, one).is_err());
        assert!(select_by_id(&store, ModelRole::Unknown, one).is_err());
        assert!(select_by_id(&store, ModelRole::Asr, ModelFileId(9_999)).is_err());
        // Engines that cannot run are reported, not built: the paths do not exist.
        let built = select_engine(&paths, &store, "cpu").unwrap();
        assert!(!built.experimental);
        assert_eq!(built.diarize_runner.device, "cpu");
        assert!(built.engine.info().runtime.contains("0.2.0"));

        // Per-invocation overrides: by name, or a file that was never scanned.
        let dir = tempfile::tempdir().unwrap();
        let loose = dir.path().join("loose-asr.gguf");
        std::fs::write(&loose, b"not a gguf").unwrap();
        let ov = EngineOverrides {
            asr: Some(loose.display().to_string()),
            diarizer: Some("diar".into()),
            extra_args: vec!["--verbatim".into()],
        };
        let built = select_engine_with(&paths, &store, "cpu", &ov).unwrap();
        assert_eq!(built.selection.asr, loose);
        assert_eq!(built.selection.asr_file.id, ModelFileId(0));
        assert_eq!(built.selection.asr_file.display_name, "loose-asr");
        assert_eq!(built.selection.diarizer_file.display_name, "diar");
        assert!(
            built.experimental,
            "an unclassified file is not known to stream"
        );
        assert!(select_engine_with(
            &paths,
            &store,
            "cpu",
            &EngineOverrides {
                asr: Some("diar".into()),
                ..Default::default()
            }
        )
        .is_err());
        // The stored selection did not change.
        assert_eq!(
            selected_file(&store, ModelRole::Asr).unwrap().unwrap().id,
            one
        );
    }
}
