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
//! - [`download`]: our downloader (resume, SHA-256, atomic rename) for the
//!   engine zip and its extraction, and for every manifest model
//!   ([`download_model`]); [`pull`]: the old `nemo-speech pull` path, kept
//!   for one release behind `diarist-cli models pull --via-runtime`;
//!   [`doctor`]: runtime version and GPU.
//! - [`add`]: "Add from URL" and "Add from file" for models no manifest lists.
//! - [`releases`]: newer runtimes from the NeMo-Speech.cpp GitHub releases.
//! - [`setup`]: the first-run check (engine, models, GPU, disk) and the
//!   download list with URLs and hashes.
//! - [`status`] assembles the Settings screen's [`EngineStatus`];
//!   [`resolve_selection`] gives the engine and model paths to run with and
//!   [`select_engine`] builds the file engine from them (scanning first when
//!   nothing usable is selected); [`select_by_id`] validates a pick.
//! - [`languages`]: the tier table of offered languages;
//!   [`select_for_language`] runs a job whose language the selected model
//!   does not cover on Nemotron 3.5, and [`select_engine_for`] builds the
//!   engine for a job in a language.
//! - [`device`]: the `--device` every job passes follows the selected
//!   runtime package's backend ([`device_for`]: `cuda:0`, `vulkan:0`,
//!   `cpu`), and so do the free-VRAM checks ([`VramCheck`]).

pub mod add;
pub mod classify;
pub mod device;
pub mod doctor;
pub mod download;
pub mod gguf;
pub mod languages;
pub mod legacy;
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
pub use device::{
    cpu_notice_once, device_for, device_uses_gpu, engine_backend, gate_vram, Backend, VramCheck,
    AUTO_DEVICE, CPU_SLOW_NOTICE,
};
pub use doctor::{doctor, DoctorReport};
pub use download::{
    download_file, download_model, install_engine, model_dest, sha256_file, verify_file,
};
pub use gguf::{read_header, GgufHeader, GgufValue};
pub use manifest::{EngineEntry, Manifest, ModelEntry, TESTED_ENGINE_VERSIONS};
pub use paths::{free_disk_bytes, DataPaths, DataRootSource};
pub use pull::{pull, pull_dir, pull_with_progress, Pulled};
pub use scan::{ensure_selections, recommended, scan, ScanReport};
pub use setup::{download_items, readiness, setup_status, MissingPart, Readiness, SetupStatus};

use crate::engine::{DiarizeRunner, Engine, NemoCascadeEngine, NemoCliEngine};
use crate::error::{CoreError, Result};
use crate::settings::{self, FileDiarPreset, FileLookahead};
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
    /// The runtime package's backend ([`engine_backend`]); `None` when
    /// nothing tells, and jobs then pass `--device auto`.
    pub backend: Option<Backend>,
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
        backend: engine_backend(&engine),
        asr: paths.absolute(&asr.path),
        asr_file: asr,
        diarizer: paths.absolute(&diar.path),
        diarizer_file: diar,
    })
}

impl ResolvedSelection {
    /// The `--device` the selected runtime package takes ([`device_for`]).
    pub fn device(&self) -> &'static str {
        device_for(self.backend)
    }
}

/// The file engine for the current selection.
pub struct SelectedEngine {
    pub engine: Arc<dyn Engine>,
    /// The standalone diarization pass with the same runtime and diarizer.
    pub diarize_runner: DiarizeRunner,
    pub selection: ResolvedSelection,
    /// The `--device` the engine, the diarization pass and a live server
    /// for this selection pass: [`EngineOverrides::device`], else the
    /// package's ([`ResolvedSelection::device`]).
    pub device: String,
    /// The ASR model is not a streaming one, so the experimental cascade
    /// (`NemoCascadeEngine`, may drop speech) runs instead of `NemoCliEngine`.
    pub experimental: bool,
    /// The decoder lookahead the file engine passes ([`file_right_context`];
    /// `None`: the runtime default, always for the cascade).
    pub right_context: Option<u32>,
    /// The diarizer preset diarized file jobs pass ([`file_diar_preset`];
    /// `None`: the runtime's streaming default, always for the cascade).
    pub diar_preset: Option<String>,
}

impl SelectedEngine {
    /// Jobs run on a GPU (not the CPU package or `--device cpu`), so the
    /// free-VRAM checks and low-VRAM mode apply.
    pub fn uses_gpu(&self) -> bool {
        device_uses_gpu(&self.device)
    }
}

/// Per-invocation changes to the stored selection (the CLI's `--asr-model`,
/// `--diar-model`, `--engine-arg`, `--device`). Models are a file path or
/// anything [`find_file`] accepts; nothing is persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineOverrides {
    pub asr: Option<String>,
    pub diarizer: Option<String>,
    /// Flags appended verbatim to `nemo-speech transcribe`.
    pub extra_args: Vec<String>,
    /// `--device` instead of the package's (`cuda:1`, `cpu`, `auto`).
    pub device: Option<String>,
}

/// Builds the engine for the selected runtime, ASR model and diarizer on
/// the runtime package's device ([`ResolvedSelection::device`]: `cuda:0`,
/// `vulkan:0`, `cpu`). When nothing usable is selected, scans the models
/// and engine folders first (which also picks the recommended files: the
/// EN model and Nemotron 3 Diarization).
pub fn select_engine(paths: &DataPaths, store: &dyn Store) -> Result<SelectedEngine> {
    select_engine_with(paths, store, &EngineOverrides::default())
}

/// [`select_engine`] with per-invocation overrides.
pub fn select_engine_with(
    paths: &DataPaths,
    store: &dyn Store,
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
    // The Vulkan and CPU packages refuse `--device cuda:0` (exit code 2).
    let device = overrides
        .device
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(sel.device())
        .to_owned();
    let models_dir = paths.models();
    // A file the header did not identify is tried as a streaming model; the
    // runtime's own error is shown if it is not one.
    let experimental =
        sel.asr_file.role != ModelRole::Unknown && !sel.asr_file.capabilities.streaming;
    // File jobs and the post-pass run at the model's full lookahead; live
    // keeps the runtime's low-latency default (`engine::live_ws`).
    let right_context = asr_right_context(paths, store, &sel.asr_file)?;
    // The diarizer runs its file preset there (`v3-offline`); live and the
    // standalone `diarize` pass keep the streaming one.
    let diar_preset = if experimental {
        None
    } else {
        diarizer_file_preset(paths, store, &sel.diarizer_file)?
    };
    let inner: Arc<dyn Engine> = if experimental {
        let mut e = NemoCascadeEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.clone();
        e.version = sel.engine_version.clone();
        e.extra_args = overrides.extra_args.clone();
        Arc::new(e)
    } else {
        let mut e = NemoCliEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.clone();
        e.version = sel.engine_version.clone();
        // `--language` only for multilingual models (Nemotron 3.5); the EN
        // model ignores it. The meeting keeps its language either way.
        e.pass_language = is_multilingual(&sel.asr_file);
        e.right_context = right_context;
        e.diar_preset = diar_preset.clone();
        e.extra_args = overrides.extra_args.clone();
        Arc::new(e)
    };
    // Every job gets a tag the model's prompt dictionary has (`ja` -> `ja-JP`).
    let engine: Arc<dyn Engine> = Arc::new(languages::TaggedEngine::new(
        inner,
        sel.asr_file.capabilities.languages.clone(),
    ));
    let mut diarize_runner = DiarizeRunner::new(&sel.engine_dir, &models_dir, &sel.diarizer);
    diarize_runner.device = device.clone();
    Ok(SelectedEngine {
        engine,
        diarize_runner,
        selection: sel,
        device,
        experimental,
        right_context,
        diar_preset,
    })
}

/// The preset diarized file jobs run diarizer `file` with, from the stored
/// `engine.file_diar_preset` and the data root's manifest
/// ([`file_diar_preset`]). Copy diagnostics shows it for the selection.
pub fn diarizer_file_preset(
    paths: &DataPaths,
    store: &dyn Store,
    file: &ModelFile,
) -> Result<Option<String>> {
    Ok(file_diar_preset(
        file,
        &settings::file_diar_preset(&store.get_settings()?),
        Manifest::load(&paths.root).ok().as_ref(),
    ))
}

/// The diarizer preset of file jobs and the post-pass for `file`: the
/// `engine.file_diar_preset` setting when it is `streaming` (none) or a
/// name, else the diarizer's own `file_preset` (its manifest entry or
/// `<file>.meet.json`; for a row scanned before the field existed, the
/// `manifest` entry it matched). A diarizer without one gets no flag.
pub fn file_diar_preset(
    file: &ModelFile,
    setting: &FileDiarPreset,
    manifest: Option<&Manifest>,
) -> Option<String> {
    let own = file.capabilities.file_preset.clone().or_else(|| {
        let name = file.manifest_name.as_deref()?;
        manifest?.model(name)?.file_preset.clone()
    });
    setting.resolve(own)
}

/// The decoder lookahead file jobs on ASR model `file` pass, with the
/// stored `engine.file_lookahead` and the data root's manifest
/// ([`file_right_context`]); `None` for a model the experimental cascade
/// runs (not streaming). Copy diagnostics shows it for the selection.
pub fn asr_right_context(
    paths: &DataPaths,
    store: &dyn Store,
    file: &ModelFile,
) -> Result<Option<u32>> {
    if file.role != ModelRole::Unknown && !file.capabilities.streaming {
        return Ok(None);
    }
    Ok(file_right_context(
        file,
        settings::file_lookahead(&store.get_settings()?),
        Manifest::load(&paths.root).ok().as_ref(),
    ))
}

/// The decoder lookahead file jobs and the post-pass run `file` at: the
/// `engine.file_lookahead` setting when it is `low` (none) or a number,
/// else the model's own `file_right_context` (its manifest entry or
/// `<file>.meet.json`; for a row scanned before the field existed, the
/// `manifest` entry it matched). A model without one gets no flag, so an
/// unknown model is never pushed past what it supports.
pub fn file_right_context(
    file: &ModelFile,
    setting: FileLookahead,
    manifest: Option<&Manifest>,
) -> Option<u32> {
    let own = file.capabilities.file_right_context.or_else(|| {
        let name = file.manifest_name.as_deref()?;
        manifest?.model(name)?.file_right_context
    });
    setting.resolve(own)
}

/// The manifest model low-VRAM mode switches file imports to: Nemotron 3.5,
/// whose file pass peaks at 1.8 GB against 2.6 GB for the EN model (docs/04
/// section 4).
pub const LOW_VRAM_MODEL: &str = "nemotron-3.5";

/// The manifest model that covers every offered language
/// ([`languages::LANGUAGES`]): the same Nemotron 3.5.
pub const MULTILINGUAL_MODEL: &str = LOW_VRAM_MODEL;

/// The installed [`LOW_VRAM_MODEL`] file, if any.
pub fn low_vram_asr(store: &dyn Store) -> Result<Option<ModelFile>> {
    Ok(installed_asr(&store.list_model_files()?, LOW_VRAM_MODEL).cloned())
}

/// The present ASR file of manifest model `name` among `files`.
fn installed_asr<'a>(files: &'a [ModelFile], name: &str) -> Option<&'a ModelFile> {
    files.iter().find(|f| {
        f.role == ModelRole::Asr && !f.missing && f.manifest_name.as_deref() == Some(name)
    })
}

/// Why a job runs on another ASR model than the stored selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    /// Low-VRAM mode (`engine.low_vram`, imports only).
    LowVram,
    /// The selected model does not cover the job's language.
    Language,
}

/// The ASR model a job runs on ([`select_for_language`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrChoice {
    /// The stored selection.
    Selected,
    /// An installed model that covers the language (Nemotron 3.5 for
    /// low-VRAM mode); the stored selection is unchanged.
    Switch {
        file: Box<ModelFile>,
        reason: SwitchReason,
    },
}

impl AsrChoice {
    /// Why it switched, if it did.
    pub fn reason(&self) -> Option<SwitchReason> {
        match self {
            Self::Selected => None,
            Self::Switch { reason, .. } => Some(*reason),
        }
    }

    /// The overrides that run a job on this choice.
    pub fn overrides(&self) -> EngineOverrides {
        EngineOverrides {
            // The stored relative path: `find_file` matches it exactly.
            asr: match self {
                Self::Selected => None,
                Self::Switch { file, .. } => Some(file.path.clone()),
            },
            ..Default::default()
        }
    }
}

/// Why `selected` does not cover `language`, as the end of a sentence:
/// "the selected model is English-only".
fn not_covered(selected: &ModelFile, language: &str) -> String {
    match languages::only_language(selected) {
        Some(only) => format!(
            "the selected model is {}-only",
            languages::find(&only).map_or(only.as_str(), |l| l.name.as_str())
        ),
        None if languages::is_auto(language) => {
            "the selected model cannot detect the language".into()
        }
        None => format!(
            "the selected model does not cover {}",
            languages::label(language)
        ),
    }
}

/// "Nemotron 3.5" for the manifest's file, else the file's own name.
pub fn model_name(file: &ModelFile) -> String {
    if file.manifest_name.as_deref() == Some(MULTILINGUAL_MODEL) {
        return "Nemotron 3.5".into();
    }
    let name = file.display_name.trim_start_matches('.');
    name.strip_suffix(".gguf").unwrap_or(name).to_owned()
}

/// The notice for a job that switched from `selected` to `to`:
/// "Transcribing with Nemotron 3.5: the selected model is English-only".
pub fn switch_notice(
    reason: SwitchReason,
    selected: &ModelFile,
    to: &ModelFile,
    language: &str,
) -> String {
    let why = match reason {
        SwitchReason::LowVram => "low-VRAM mode".to_owned(),
        SwitchReason::Language => not_covered(selected, language),
    };
    format!("Transcribing with {}: {why}", model_name(to))
}

/// A job's language that neither the selected ASR model nor any installed
/// one can transcribe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageModelMissing {
    /// The job's language as given (`de`, `auto`).
    pub language: String,
    /// Display name of the selected model.
    pub selected: String,
    /// Why the selected model does not do: "the selected model is English-only".
    pub reason: String,
    /// The manifest model to install ([`MULTILINGUAL_MODEL`]).
    pub install: &'static str,
}

impl LanguageModelMissing {
    /// "German (de)", or "Automatic language detection".
    pub fn what(&self) -> String {
        if languages::is_auto(&self.language) {
            "Automatic language detection".into()
        } else {
            languages::label(&self.language)
        }
    }
}

impl std::fmt::Display for LanguageModelMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} needs Nemotron 3.5, which is not installed ({}). Install it from Settings, \
             Models (CLI: `diarist-cli models pull {}`), or choose another language",
            self.what(),
            self.reason,
            self.install
        )
    }
}

impl std::error::Error for LanguageModelMissing {}

impl From<LanguageModelMissing> for CoreError {
    fn from(e: LanguageModelMissing) -> Self {
        CoreError::Models(e.to_string())
    }
}

/// The installed streaming ASR model, other than `selected`, that lists
/// `language` (by capability, so a hand-dropped or re-converted model
/// counts); the manifest's Nemotron 3.5 first, then the lowest id.
pub fn asr_for_language<'a>(
    selected: &ModelFile,
    files: &'a [ModelFile],
    language: &str,
) -> Option<&'a ModelFile> {
    let mut found: Vec<&ModelFile> = files
        .iter()
        .filter(|f| {
            f.role == ModelRole::Asr
                && !f.missing
                && f.id != selected.id
                && f.capabilities.streaming
                && languages::lists(f, language)
        })
        .collect();
    found.sort_by_key(|f| {
        (
            f.manifest_name.as_deref() != Some(MULTILINGUAL_MODEL),
            f.id.0,
        )
    });
    found.first().copied()
}

/// The ASR model a job in `language` runs on, given the stored selection
/// `selected` and the scanned `files`:
///
/// - the selection when it covers the language ([`languages::covers`]: the
///   same primary subtag, `auto` on a multilingual model, or a model that
///   lists no languages, which never switches), unless `low_vram` asks for
///   Nemotron 3.5;
/// - an installed streaming model that lists the language
///   ([`asr_for_language`]) when the selection does not cover it; or the
///   installed Nemotron 3.5 for low-VRAM mode. One switch whichever asks
///   (the language wins as the reason);
/// - [`LanguageModelMissing`], naming the model to install (Nemotron 3.5),
///   when no installed model covers a language `table` knows (or `auto`);
/// - the selection, not checked, for a tag nothing installed lists and the
///   table does not know (one the user typed): it is passed through.
///
/// The stored selection never changes.
pub fn select_for_language(
    selected: &ModelFile,
    files: &[ModelFile],
    language: &str,
    low_vram: bool,
    table: &languages::LanguageTable,
) -> std::result::Result<AsrChoice, LanguageModelMissing> {
    if !languages::covers(selected, language) {
        return match asr_for_language(selected, files, language) {
            Some(f) => Ok(AsrChoice::Switch {
                file: Box::new(f.clone()),
                reason: SwitchReason::Language,
            }),
            None if !table.knows(language) => Ok(AsrChoice::Selected),
            None => Err(LanguageModelMissing {
                language: language.trim().to_owned(),
                selected: selected.display_name.clone(),
                reason: not_covered(selected, language),
                install: MULTILINGUAL_MODEL,
            }),
        };
    }
    // Low-VRAM mode: Nemotron 3.5 if it is installed (and covers the
    // language), else the selection runs.
    let low = low_vram
        .then(|| installed_asr(files, LOW_VRAM_MODEL))
        .flatten()
        .filter(|f| f.id != selected.id && languages::covers(f, language));
    Ok(match low {
        Some(f) => AsrChoice::Switch {
            file: Box::new(f.clone()),
            reason: SwitchReason::LowVram,
        },
        None => AsrChoice::Selected,
    })
}

/// The engine for a job in `language` (imports, recordings, `diarist-cli
/// transcribe`): the selection, or the installed Nemotron 3.5 as
/// [`select_for_language`] decides (`low_vram`: also for low-VRAM mode,
/// which the CPU package ignores: it uses no GPU memory). An ASR model
/// given in `overrides` wins: nothing switches. Returns the engine and why
/// it switched; a language nothing installed covers is an error naming the
/// model to install.
pub fn select_engine_for(
    paths: &DataPaths,
    store: &dyn Store,
    language: &str,
    low_vram: bool,
    overrides: &EngineOverrides,
) -> Result<(SelectedEngine, Option<SwitchReason>)> {
    // Built first: it scans when nothing usable is selected.
    let built = select_engine_with(paths, store, overrides)?;
    if overrides.asr.is_some() {
        return Ok((built, None));
    }
    let files = store.list_model_files()?;
    let table = languages::LanguageTable::load(&paths.root);
    let choice = select_for_language(
        &built.selection.asr_file,
        &files,
        language,
        low_vram && built.uses_gpu(),
        &table,
    )?;
    let Some(reason) = choice.reason() else {
        return Ok((built, None));
    };
    let mut ov = choice.overrides();
    ov.diarizer = overrides.diarizer.clone();
    ov.extra_args = overrides.extra_args.clone();
    ov.device = overrides.device.clone();
    Ok((select_engine_with(paths, store, &ov)?, Some(reason)))
}

/// The engine for a file import in `language`: [`select_engine_for`] with
/// low-VRAM mode. Returns why it switched to Nemotron 3.5, if it did.
pub fn select_import_engine(
    paths: &DataPaths,
    store: &dyn Store,
    low_vram: bool,
    language: &str,
) -> Result<(SelectedEngine, Option<SwitchReason>)> {
    select_engine_for(
        paths,
        store,
        language,
        low_vram,
        &EngineOverrides::default(),
    )
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
    // A file outside the data root is stored by its absolute path, whose
    // separators and case `std::path::absolute` may not keep.
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    if let Some(f) = store
        .list_model_files()?
        .into_iter()
        .find(|f| f.path == rel || norm(&f.path) == norm(&rel))
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

    fn table() -> languages::LanguageTable {
        languages::LanguageTable::fallback()
    }

    fn speaks(mut f: ScannedModelFile, langs: &[&str]) -> ScannedModelFile {
        f.capabilities.languages = langs.iter().map(|s| (*s).to_owned()).collect();
        f
    }

    /// The EN model and, with `n35`, Nemotron 3.5 (the locales its header
    /// lists, in part), the EN model selected.
    fn language_store(n35: bool) -> MemoryStore {
        let store = MemoryStore::new();
        let mut files = vec![
            scanned("engine/nemo-speech-0.2.0", ModelRole::Engine, "e"),
            speaks(scanned("models/en.gguf", ModelRole::Asr, "en"), &["en"]),
            scanned("models/d.gguf", ModelRole::Diarizer, "diar"),
        ];
        if n35 {
            let mut f = speaks(
                scanned("models/n35.gguf", ModelRole::Asr, "n35"),
                &["en-US", "en", "de-DE", "de", "ja-JP", "auto"],
            );
            f.manifest_name = Some(MULTILINGUAL_MODEL.into());
            files.push(f);
        }
        store.sync_model_files(&files, 1).unwrap();
        select(&store, ModelRole::Engine, "e").unwrap();
        select(&store, ModelRole::Asr, "en").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        store
    }

    fn asr_files(store: &MemoryStore) -> (ModelFile, Vec<ModelFile>) {
        let files = store.list_model_files().unwrap();
        let sel = selected_file(store, ModelRole::Asr).unwrap().unwrap();
        (sel, files)
    }

    #[test]
    fn english_on_the_en_model_keeps_the_selection() {
        let (en, files) = asr_files(&language_store(true));
        assert_eq!(
            select_for_language(&en, &files, "en", false, &table()),
            Ok(AsrChoice::Selected)
        );
        assert_eq!(
            select_for_language(&en, &files, "en-GB", false, &table()),
            Ok(AsrChoice::Selected)
        );
        // Without Nemotron 3.5 too: nothing is missing for English.
        let (en, files) = asr_files(&language_store(false));
        assert_eq!(
            select_for_language(&en, &files, "en", false, &table()),
            Ok(AsrChoice::Selected)
        );
    }

    #[test]
    fn german_on_the_en_model_switches_to_installed_nemotron_35() {
        let (en, files) = asr_files(&language_store(true));
        for lang in ["de", "de-DE", "auto"] {
            let choice = select_for_language(&en, &files, lang, false, &table()).unwrap();
            let AsrChoice::Switch { file, reason } = &choice else {
                panic!("{lang}: {choice:?}");
            };
            assert_eq!(file.manifest_name.as_deref(), Some(MULTILINGUAL_MODEL));
            assert_eq!(*reason, SwitchReason::Language);
            assert_eq!(choice.overrides().asr.as_deref(), Some("models/n35.gguf"));
        }
        let n35 = files.iter().find(|f| f.display_name == "n35").unwrap();
        assert_eq!(
            switch_notice(SwitchReason::Language, &en, n35, "de"),
            "Transcribing with Nemotron 3.5: the selected model is English-only"
        );
        let mut dropped = n35.clone();
        dropped.manifest_name = None;
        dropped.display_name = ".my-asr.q8_0.gguf".into();
        assert_eq!(
            switch_notice(SwitchReason::LowVram, &en, &dropped, "en"),
            "Transcribing with my-asr.q8_0: low-VRAM mode"
        );
        // Nemotron 3.5 selected: it covers German and detection itself.
        for lang in ["de", "auto", "en"] {
            assert_eq!(
                select_for_language(n35, &files, lang, false, &table()),
                Ok(AsrChoice::Selected)
            );
        }
    }

    #[test]
    fn german_without_nemotron_35_names_the_model_to_install() {
        let (en, files) = asr_files(&language_store(false));
        let err = select_for_language(&en, &files, "de", false, &table()).unwrap_err();
        assert_eq!(err.install, "nemotron-3.5");
        assert_eq!(err.language, "de");
        assert_eq!(err.reason, "the selected model is English-only");
        let text = err.to_string();
        assert!(text.starts_with("German (de) needs Nemotron 3.5"), "{text}");
        assert!(text.contains("models pull nemotron-3.5"), "{text}");
        let err = select_for_language(&en, &files, "auto", true, &table()).unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Automatic language detection needs"));
        // A model whose languages are unknown is trusted with any language.
        let mut odd = en.clone();
        odd.capabilities.languages.clear();
        assert_eq!(
            select_for_language(&odd, &files, "de", false, &table()),
            Ok(AsrChoice::Selected)
        );
    }

    #[test]
    fn low_vram_and_language_switch_once() {
        let (en, files) = asr_files(&language_store(true));
        // Low-VRAM mode alone switches, as before.
        let choice = select_for_language(&en, &files, "en", true, &table()).unwrap();
        assert_eq!(choice.reason(), Some(SwitchReason::LowVram));
        // Both ask for Nemotron 3.5: one switch, the language as the reason.
        let choice = select_for_language(&en, &files, "de", true, &table()).unwrap();
        assert_eq!(choice.reason(), Some(SwitchReason::Language));
        assert_eq!(choice.overrides().asr.as_deref(), Some("models/n35.gguf"));
        // Without Nemotron 3.5 low-VRAM mode falls back to the selection,
        // and the language still says what to install.
        let (en, files) = asr_files(&language_store(false));
        assert_eq!(
            select_for_language(&en, &files, "en", true, &table()),
            Ok(AsrChoice::Selected)
        );
        assert!(select_for_language(&en, &files, "de", true, &table()).is_err());
    }

    #[test]
    fn the_switch_goes_by_capability_not_by_manifest_name() {
        let (en, mut files) = asr_files(&language_store(false));
        let mut dropped = en.clone();
        dropped.id = ModelFileId(77);
        dropped.path = "models/user/n35-reconverted.gguf".into();
        dropped.manifest_name = None;
        dropped.capabilities.languages = vec!["en".into(), "de-DE".into(), "auto".into()];
        let mut offline = dropped.clone();
        offline.id = ModelFileId(5);
        offline.capabilities.streaming = false;
        files.push(offline);
        files.push(dropped);
        let choice = select_for_language(&en, &files, "de", false, &table()).unwrap();
        assert_eq!(
            choice.overrides().asr.as_deref(),
            Some("models/user/n35-reconverted.gguf")
        );
        assert_eq!(choice.reason(), Some(SwitchReason::Language));
        // A tag nothing lists and the table does not know (typed by the
        // user) runs on the selection, unchecked.
        assert_eq!(
            select_for_language(&en, &files, "x-klingon", false, &table()),
            Ok(AsrChoice::Selected)
        );
        // Nothing lists French: the download offer names Nemotron 3.5.
        assert_eq!(
            select_for_language(&en, &files, "fr", false, &table())
                .unwrap_err()
                .install,
            MULTILINGUAL_MODEL
        );
    }

    #[test]
    fn imports_switch_to_nemotron_35_and_keep_the_selection() {
        let paths = DataPaths::at("D:/data");
        let store = language_store(false);
        // Not installed: low-VRAM mode runs the selection; German cannot run.
        let (b, switched) = select_import_engine(&paths, &store, true, "en").unwrap();
        assert!(switched.is_none() && b.selection.asr_file.display_name == "en");
        let err = select_import_engine(&paths, &store, false, "de")
            .err()
            .unwrap();
        assert!(err.to_string().contains("nemotron-3.5"), "{err}");

        let store = language_store(true);
        let (b, switched) = select_import_engine(&paths, &store, true, "en").unwrap();
        assert_eq!(switched, Some(SwitchReason::LowVram));
        assert_eq!(b.selection.asr_file.display_name, "n35");
        let (b, switched) = select_import_engine(&paths, &store, false, "en").unwrap();
        assert!(switched.is_none() && b.selection.asr_file.display_name == "en");
        let (b, switched) = select_import_engine(&paths, &store, true, "de").unwrap();
        assert_eq!(switched, Some(SwitchReason::Language));
        assert_eq!(b.selection.asr_file.display_name, "n35");
        // An explicit model (the CLI's --asr-model) is never switched.
        let ov = EngineOverrides {
            asr: Some("en".into()),
            ..Default::default()
        };
        let (b, switched) = select_engine_for(&paths, &store, "de", true, &ov).unwrap();
        assert!(switched.is_none() && b.selection.asr_file.display_name == "en");
        assert_eq!(
            selected_file(&store, ModelRole::Asr)
                .unwrap()
                .unwrap()
                .display_name,
            "en"
        );
    }

    /// The three 0.2.0 packages as the scan stores them (the manifest's
    /// backend in `quant`), the EN model, Nemotron 3.5 and the diarizer.
    fn package_store() -> MemoryStore {
        let store = MemoryStore::new();
        let mut files = Vec::new();
        for backend in ["cuda", "vulkan", "cpu"] {
            let name = format!("nemo-speech-0.2.0-windows-x86_64-{backend}");
            let mut e = scanned(&format!("engine/{name}"), ModelRole::Engine, &name);
            e.quant = Some(backend.into());
            e.manifest_name = Some(name);
            files.push(e);
        }
        files.push(speaks(
            scanned("models/en.gguf", ModelRole::Asr, "en"),
            &["en"],
        ));
        let mut n35 = speaks(
            scanned("models/n35.gguf", ModelRole::Asr, "n35"),
            &["en", "de", "auto"],
        );
        n35.manifest_name = Some(MULTILINGUAL_MODEL.into());
        files.push(n35);
        files.push(scanned("models/d.gguf", ModelRole::Diarizer, "diar"));
        store.sync_model_files(&files, 1).unwrap();
        select(&store, ModelRole::Asr, "en").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        store
    }

    #[test]
    fn jobs_take_the_device_of_the_selected_package() {
        let paths = DataPaths::at("D:/data");
        let store = package_store();
        for (backend, device) in [("cuda", "cuda:0"), ("vulkan", "vulkan:0"), ("cpu", "cpu")] {
            select(
                &store,
                ModelRole::Engine,
                &format!("nemo-speech-0.2.0-windows-x86_64-{backend}"),
            )
            .unwrap();
            let sel = resolve_selection(&paths, &store).unwrap();
            assert_eq!(sel.backend.map(Backend::as_str), Some(backend));
            assert_eq!(sel.device(), device);
            let built = select_engine(&paths, &store).unwrap();
            assert_eq!(built.device, device, "{backend}");
            // The file job, its EngineInfo (runs store it) and the
            // standalone diarization pass all pass it.
            assert_eq!(built.engine.info().device, device, "{backend}");
            let args: Vec<String> = built
                .diarize_runner
                .args(Path::new("a.wav"), Path::new("a.rttm"))
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let at = args.iter().position(|a| a == "--device").unwrap();
            assert_eq!(args[at + 1], device, "{args:?}");
            assert_eq!(built.uses_gpu(), backend != "cpu");
            // A recording or import in a language switched to Nemotron
            // 3.5 keeps the package's device.
            let (b, switched) =
                select_engine_for(&paths, &store, "de", false, &EngineOverrides::default())
                    .unwrap();
            assert_eq!(switched, Some(SwitchReason::Language));
            assert_eq!(b.device, device);
            // Low-VRAM mode switches imports on a GPU package only.
            let (b, switched) = select_import_engine(&paths, &store, true, "en").unwrap();
            if backend == "cpu" {
                assert!(switched.is_none() && b.selection.asr_file.display_name == "en");
            } else {
                assert_eq!(switched, Some(SwitchReason::LowVram));
                assert_eq!(b.selection.asr_file.display_name, "n35");
            }
            // The CLI's --device wins, through a language switch too.
            let ov = EngineOverrides {
                device: Some("cuda:1".into()),
                ..Default::default()
            };
            let (b, _) = select_engine_for(&paths, &store, "de", false, &ov).unwrap();
            assert_eq!(
                (b.device.as_str(), b.engine.info().device.as_str()),
                ("cuda:1", "cuda:1")
            );
        }
    }

    #[test]
    fn file_jobs_run_at_the_model_lookahead() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let store = MemoryStore::new();
        let mut en = scanned("models/en.gguf", ModelRole::Asr, "en");
        en.capabilities.file_right_context = Some(13);
        let mut tdt = scanned("models/tdt.gguf", ModelRole::Asr, "tdt");
        tdt.capabilities.streaming = false;
        tdt.capabilities.file_right_context = Some(13);
        store
            .sync_model_files(
                &[
                    scanned("engine/nemo-speech-0.2.0", ModelRole::Engine, "e"),
                    en,
                    scanned("models/mine.gguf", ModelRole::Asr, "mine"),
                    tdt,
                    scanned("models/d.gguf", ModelRole::Diarizer, "diar"),
                ],
                1,
            )
            .unwrap();
        select(&store, ModelRole::Engine, "e").unwrap();
        select(&store, ModelRole::Asr, "en").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        let rc = |asr: Option<&str>| {
            let ov = EngineOverrides {
                asr: asr.map(Into::into),
                ..Default::default()
            };
            select_engine_with(&paths, &store, &ov)
                .unwrap()
                .right_context
        };
        assert_eq!(rc(None), Some(13));
        // A hand-dropped model without a value gets no flag; the cascade never.
        assert_eq!(rc(Some("mine")), None);
        assert_eq!(rc(Some("tdt")), None);
        // `low` drops the flag; a number applies to every streaming model.
        store
            .set_setting(settings::ENGINE_FILE_LOOKAHEAD, "low")
            .unwrap();
        assert_eq!(rc(None), None);
        store
            .set_setting(settings::ENGINE_FILE_LOOKAHEAD, "6")
            .unwrap();
        assert_eq!((rc(None), rc(Some("mine"))), (Some(6), Some(6)));
        assert_eq!(rc(Some("tdt")), None);
        store
            .set_setting(settings::ENGINE_FILE_LOOKAHEAD, "model")
            .unwrap();
        assert_eq!(rc(None), Some(13));

        // A row scanned before the field existed: the manifest entry it matched.
        let mut old = selected_file(&store, ModelRole::Asr).unwrap().unwrap();
        old.capabilities.file_right_context = None;
        old.manifest_name = Some("nemotron-3.5".into());
        let builtin = Manifest::builtin();
        assert_eq!(
            file_right_context(&old, FileLookahead::Model, Some(&builtin)),
            Some(13)
        );
        assert_eq!(file_right_context(&old, FileLookahead::Model, None), None);
        assert_eq!(
            file_right_context(&old, FileLookahead::Low, Some(&builtin)),
            None
        );
    }

    #[test]
    fn file_jobs_run_the_diarizer_file_preset() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let store = MemoryStore::new();
        let mut diar = scanned("models/d.gguf", ModelRole::Diarizer, "diar");
        diar.capabilities.file_preset = Some("v3-offline".into());
        let mut tdt = scanned("models/tdt.gguf", ModelRole::Asr, "tdt");
        tdt.capabilities.streaming = false;
        store
            .sync_model_files(
                &[
                    scanned("engine/nemo-speech-0.2.0", ModelRole::Engine, "e"),
                    scanned("models/en.gguf", ModelRole::Asr, "en"),
                    tdt,
                    diar,
                    scanned("models/d2.gguf", ModelRole::Diarizer, "plain"),
                ],
                1,
            )
            .unwrap();
        select(&store, ModelRole::Engine, "e").unwrap();
        select(&store, ModelRole::Asr, "en").unwrap();
        select(&store, ModelRole::Diarizer, "diar").unwrap();
        let preset = |asr: Option<&str>, diarizer: Option<&str>| {
            let ov = EngineOverrides {
                asr: asr.map(Into::into),
                diarizer: diarizer.map(Into::into),
                ..Default::default()
            };
            select_engine_with(&paths, &store, &ov).unwrap().diar_preset
        };
        assert_eq!(preset(None, None).as_deref(), Some("v3-offline"));
        // A diarizer without a value gets no flag; the cascade never.
        assert_eq!(preset(None, Some("plain")), None);
        assert_eq!(preset(Some("tdt"), None), None);
        // `streaming` drops the flag; a name applies to every diarizer.
        store
            .set_setting(settings::ENGINE_FILE_DIAR_PRESET, "streaming")
            .unwrap();
        assert_eq!(preset(None, None), None);
        store
            .set_setting(settings::ENGINE_FILE_DIAR_PRESET, "v3-streaming")
            .unwrap();
        assert_eq!(preset(None, Some("plain")).as_deref(), Some("v3-streaming"));
        store
            .set_setting(settings::ENGINE_FILE_DIAR_PRESET, "model")
            .unwrap();
        assert_eq!(preset(None, None).as_deref(), Some("v3-offline"));

        // A row scanned before the field existed: the manifest entry it matched.
        let mut old = selected_file(&store, ModelRole::Diarizer).unwrap().unwrap();
        old.capabilities.file_preset = None;
        old.manifest_name = Some("nemotron-3-diarization".into());
        let builtin = Manifest::builtin();
        assert_eq!(
            file_diar_preset(&old, &FileDiarPreset::Model, Some(&builtin)).as_deref(),
            Some("v3-offline")
        );
        assert_eq!(file_diar_preset(&old, &FileDiarPreset::Model, None), None);
        assert_eq!(
            file_diar_preset(&old, &FileDiarPreset::Streaming, Some(&builtin)),
            None
        );
    }

    #[test]
    fn a_model_outside_the_data_root_resolves_to_its_row() {
        let store = MemoryStore::new();
        let paths = DataPaths::at("D:/data");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n35.gguf");
        std::fs::write(&file, b"x").unwrap();
        // Scanned with forward slashes (the CLI's --models-dir outside the root).
        let stored = std::path::absolute(&file)
            .unwrap()
            .display()
            .to_string()
            .replace('\\', "/");
        let mut f = scanned(&stored, ModelRole::Asr, "n35");
        f.manifest_name = Some(MULTILINGUAL_MODEL.into());
        store.sync_model_files(&[f], 1).unwrap();
        let (_, row) = resolve_model(&paths, &store, ModelRole::Asr, &stored).unwrap();
        assert_ne!(
            row.id,
            ModelFileId(0),
            "the scanned row, not a synthetic one"
        );
        assert_eq!(row.manifest_name.as_deref(), Some(MULTILINGUAL_MODEL));
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
        let built = select_engine(&paths, &store).unwrap();
        assert!(!built.experimental);
        // A runtime folder whose backend nothing tells: the runtime chooses.
        assert_eq!(built.selection.backend, None);
        assert_eq!(built.diarize_runner.device, "auto");
        assert_eq!(built.engine.info().device, "auto");
        assert!(built.engine.info().runtime.contains("0.2.0"));

        // Per-invocation overrides: by name, or a file that was never scanned.
        let dir = tempfile::tempdir().unwrap();
        let loose = dir.path().join("loose-asr.gguf");
        std::fs::write(&loose, b"not a gguf").unwrap();
        let ov = EngineOverrides {
            asr: Some(loose.display().to_string()),
            diarizer: Some("diar".into()),
            extra_args: vec!["--verbatim".into()],
            device: Some("cpu".into()),
        };
        let built = select_engine_with(&paths, &store, &ov).unwrap();
        assert_eq!(built.selection.asr, loose);
        assert_eq!(built.selection.asr_file.id, ModelFileId(0));
        assert_eq!(built.selection.asr_file.display_name, "loose-asr");
        assert_eq!(built.selection.diarizer_file.display_name, "diar");
        assert_eq!(
            (built.device.as_str(), built.diarize_runner.device.as_str()),
            ("cpu", "cpu")
        );
        assert!(
            built.experimental,
            "an unclassified file is not known to stream"
        );
        assert!(select_engine_with(
            &paths,
            &store,
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
