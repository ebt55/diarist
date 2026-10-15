//! The ComfyUI-style scanner: every GGUF/ONNX under `models\` and every
//! runtime folder under `engine\`, upserted into `model_files` through
//! [`Store::sync_model_files`], with per-role selections kept valid.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::classify::{classify, Classification, Sidecar};
use super::doctor::doctor;
use super::gguf::read_header;
use super::manifest::{version_cmp, Manifest, TESTED_ENGINE_VERSIONS};
use super::paths::DataPaths;
use crate::engine::process;
use crate::error::Result;
use crate::store::{ScannedModelFile, Store};
use crate::types::{ModelCapabilities, ModelFile, ModelFileId, ModelRole, ModelSource};
use crate::views::selection_key;

/// What a scan found and changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// All rows after the scan (missing ones included).
    pub files: Vec<ModelFile>,
    /// Selection changes, for the user.
    pub notes: Vec<String>,
    /// Files that could not be read (still listed, as `unknown`).
    pub errors: Vec<String>,
}

/// Scans `models\` and `engine\` under the data root, syncs the store and
/// repairs selections.
pub fn scan(paths: &DataPaths, store: &dyn Store, manifest: &Manifest) -> Result<ScanReport> {
    let previous: HashMap<String, ModelFile> = store
        .list_model_files()?
        .into_iter()
        .map(|f| (f.path.clone(), f))
        .collect();
    let mut found = Vec::new();
    let mut errors = Vec::new();

    let mut model_files = Vec::new();
    walk(&paths.models(), 12, &mut |p| {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if matches!(ext.as_deref(), Some("gguf" | "onnx")) {
            model_files.push(p.to_path_buf());
        }
    });
    model_files.sort();
    for p in model_files {
        match scan_model(paths, &p, manifest) {
            Ok((f, err)) => {
                if let Some(e) = err {
                    errors.push(e);
                }
                found.push(f);
            }
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }

    for dir in engine_dirs(&paths.engine()) {
        let (f, err) = scan_engine(paths, &dir, manifest, &previous);
        if let Some(e) = err {
            errors.push(e);
        }
        found.push(f);
    }

    let files = store.sync_model_files(&found, super::now_ms())?;
    let notes = ensure_selections(store, &files, manifest)?;
    Ok(ScanReport {
        files: store.list_model_files()?,
        notes,
        errors,
    })
}

/// Depth-limited recursive walk calling `f` for every file.
fn walk(dir: &Path, depth: u32, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                walk(&p, depth - 1, f);
            }
        } else if ft.is_file() {
            f(&p);
        }
    }
}

/// Folders under `engine\` (up to three levels) holding `bin\nemo-speech.exe`.
pub fn engine_dirs(engine_root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if process::is_engine_dir(dir) {
            out.push(dir.to_path_buf());
            return;
        }
        if depth == 0 {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut dirs: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for d in dirs {
                visit(&d, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(engine_root) {
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        for d in dirs {
            visit(&d, 2, &mut out);
        }
    }
    out
}

/// SHA-256 and size from the runtime's `<file>.verified` sidecar.
fn verified_sha(path: &Path, size: u64) -> Option<String> {
    let mut s = path.as_os_str().to_owned();
    s.push(".verified");
    let text = std::fs::read_to_string(PathBuf::from(s)).ok()?;
    let mut sha = None;
    let mut vsize = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("sha256=") {
            sha = Some(v.trim().to_ascii_lowercase());
        } else if let Some(v) = line.strip_prefix("size=") {
            vsize = v.trim().parse::<u64>().ok();
        }
    }
    (vsize == Some(size)).then_some(sha).flatten()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One model file; the second value is a non-fatal problem to report.
fn scan_model(
    paths: &DataPaths,
    path: &Path,
    manifest: &Manifest,
) -> Result<(ScannedModelFile, Option<String>)> {
    let size = std::fs::metadata(path)?.len();
    let is_gguf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
    let mut problem = None;
    let mut c = if is_gguf {
        match read_header(path) {
            Ok(h) => classify(&h, size),
            Err(e) => {
                problem = Some(e.to_string());
                unknown()
            }
        }
    } else {
        unknown()
    };
    match Sidecar::read(path) {
        Ok(Some(sc)) => sc.apply(&mut c),
        Ok(None) => {}
        Err(e) => problem = Some(e.to_string()),
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let verified = verified_sha(path, size);
    let entry = manifest
        .match_file(&file_name, size)
        .filter(|m| verified.as_deref().is_none_or(|v| v == m.sha256));
    let sha256 = verified.clone().or_else(|| entry.map(|m| m.sha256.clone()));
    if let Some(m) = entry {
        if c.capabilities.vram_mb_measured.is_none() {
            c.capabilities.vram_mb_measured = m.vram_mb;
        }
        if c.role == ModelRole::Unknown {
            c.role = m.role;
        }
    }
    let source = if verified.is_some() {
        ModelSource::Pull
    } else if entry.is_some() {
        ModelSource::Manifest
    } else {
        ModelSource::User
    };
    Ok((
        ScannedModelFile {
            path: paths.relative(path),
            role: c.role,
            architecture: c.architecture,
            display_name: c.display_name.unwrap_or_else(|| file_stem(path)),
            quant: c.quant,
            size_bytes: size as i64,
            sha256,
            source,
            capabilities: c.capabilities,
            manifest_name: entry.map(|m| m.name.clone()),
        },
        problem,
    ))
}

fn unknown() -> Classification {
    Classification {
        role: ModelRole::Unknown,
        family: None,
        architecture: None,
        display_name: None,
        quant: None,
        capabilities: ModelCapabilities::default(),
    }
}

/// Version from a folder name like `nemo-speech-0.2.0-windows-x86_64-cuda`.
fn version_from_name(name: &str) -> Option<String> {
    name.split('-')
        .find(|p| {
            let parts: Vec<&str> = p.split('.').collect();
            parts.len() >= 2
                && parts
                    .iter()
                    .all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_owned)
}

fn scan_engine(
    paths: &DataPaths,
    dir: &Path,
    manifest: &Manifest,
    previous: &HashMap<String, ModelFile>,
) -> (ScannedModelFile, Option<String>) {
    let rel = paths.relative(dir);
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(process::exe_path(dir))
        .map(|m| m.len())
        .unwrap_or(0);
    let mut problem = None;
    // Re-use the version from the last scan when the binary did not change:
    // `doctor` initialises CUDA and takes a second or two.
    let cached = previous
        .get(&rel)
        .filter(|f| f.size_bytes == size as i64 && !f.missing)
        .and_then(|f| f.architecture.clone());
    let version = cached.or_else(|| match doctor(dir) {
        Ok(r) => Some(r.version),
        Err(e) => {
            problem = Some(format!("{}: {e}", dir.display()));
            version_from_name(&name)
        }
    });
    let entry = manifest.engines.iter().find(|e| e.name == name);
    (
        ScannedModelFile {
            path: rel,
            role: ModelRole::Engine,
            architecture: version,
            display_name: name,
            quant: entry.map(|e| e.backend.clone()),
            size_bytes: size as i64,
            sha256: None,
            source: if entry.is_some() {
                ModelSource::Manifest
            } else {
                ModelSource::User
            },
            capabilities: ModelCapabilities::default(),
            manifest_name: entry.map(|e| e.name.clone()),
        },
        problem,
    )
}

/// The file auto-selected for `role`: the manifest's required model, then
/// other non-experimental manifest models, then any streaming file of the
/// role (ASR), any file (diarizer); for engines the newest tested version.
pub fn recommended<'a>(
    role: ModelRole,
    files: &'a [ModelFile],
    manifest: &Manifest,
) -> Option<&'a ModelFile> {
    let present = files.iter().filter(|f| f.role == role && !f.missing);
    match role {
        ModelRole::Engine => present.max_by(|a, b| {
            let tested = |f: &ModelFile| {
                TESTED_ENGINE_VERSIONS.contains(&f.architecture.as_deref().unwrap_or(""))
            };
            tested(a).cmp(&tested(b)).then_with(|| {
                version_cmp(
                    a.architecture.as_deref().unwrap_or("0"),
                    b.architecture.as_deref().unwrap_or("0"),
                )
            })
        }),
        _ => {
            let rank = |f: &ModelFile| -> Option<u8> {
                let entry = f.manifest_name.as_deref().and_then(|n| manifest.model(n));
                match entry {
                    Some(e) if e.required => Some(0),
                    Some(e) if !e.experimental => Some(1),
                    Some(_) => None,
                    None if role == ModelRole::Asr && !f.capabilities.streaming => None,
                    None => Some(2),
                }
            };
            present
                .filter_map(|f| rank(f).map(|r| (r, f)))
                .min_by(|a, b| a.0.cmp(&b.0).then(a.1.display_name.cmp(&b.1.display_name)))
                .map(|(_, f)| f)
        }
    }
}

/// Makes sure the engine, ASR and diarizer selections point at present files
/// of the right role, falling back to [`recommended`]. Returns notes.
pub fn ensure_selections(
    store: &dyn Store,
    files: &[ModelFile],
    manifest: &Manifest,
) -> Result<Vec<String>> {
    let settings = store.get_settings()?;
    let mut notes = Vec::new();
    for role in [ModelRole::Engine, ModelRole::Asr, ModelRole::Diarizer] {
        let key = selection_key(role);
        let current = settings
            .0
            .get(&key)
            .and_then(|v| v.parse::<i64>().ok())
            .map(ModelFileId);
        let valid = current.is_some_and(|id| {
            files
                .iter()
                .any(|f| f.id == id && f.role == role && !f.missing)
        });
        if valid {
            continue;
        }
        match recommended(role, files, manifest) {
            Some(f) => {
                store.set_setting(&key, &f.id.to_string())?;
                notes.push(if current.is_some() {
                    format!(
                        "{role}: the selected file is gone; using {} instead",
                        f.display_name
                    )
                } else {
                    format!("{role}: selected {}", f.display_name)
                });
            }
            None => {
                if current.is_some() {
                    store.set_setting(&key, "")?;
                    notes.push(format!(
                        "{role}: the selected file is gone and nothing replaces it"
                    ));
                }
            }
        }
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::super::gguf::{build, GgufValue};
    use super::*;
    use crate::store::MemoryStore;

    fn s(v: &str) -> GgufValue {
        GgufValue::Str(v.into())
    }

    fn write_gguf(path: &Path, kv: &[(&str, GgufValue)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, build::header(3, 0, kv)).unwrap();
    }

    fn asr_kv() -> Vec<(&'static str, GgufValue)> {
        vec![
            ("general.architecture", s("asr")),
            ("general.name", s("my-streaming-asr")),
            ("asr.head_type", s("rnnt")),
            ("asr.encoder.cache_supported", GgufValue::Bool(true)),
        ]
    }

    #[test]
    fn picks_up_dropped_files_and_marks_removed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let store = MemoryStore::new();
        let manifest = Manifest::builtin();
        let asr = paths.models().join("asr").join("mine.gguf");
        write_gguf(&asr, &asr_kv());
        let diar = paths.models().join("diarization").join("d.gguf");
        write_gguf(
            &diar,
            &[
                ("general.architecture", s("sortformer")),
                ("sortformer.version", s("v3")),
            ],
        );
        std::fs::write(paths.models().join("notes.txt"), "ignored").unwrap();
        std::fs::write(paths.models().join("broken.gguf"), "GGUF-not-really").unwrap();

        let r = scan(&paths, &store, &manifest).unwrap();
        assert_eq!(r.files.len(), 3);
        let by_path: HashMap<&str, &ModelFile> =
            r.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let a = by_path["models/asr/mine.gguf"];
        assert_eq!(a.role, ModelRole::Asr);
        assert_eq!(a.display_name, "my-streaming-asr");
        assert_eq!(a.source, ModelSource::User);
        assert!(a.capabilities.streaming);
        assert_eq!(
            by_path["models/diarization/d.gguf"].role,
            ModelRole::Diarizer
        );
        assert_eq!(by_path["models/broken.gguf"].role, ModelRole::Unknown);
        assert_eq!(r.errors.len(), 1);
        // First scan selects what it can.
        let settings = store.get_settings().unwrap();
        assert_eq!(settings.0["selected_model.asr"], a.id.to_string());
        assert!(r.notes.iter().any(|n| n.starts_with("asr: selected")));

        // Remove the ASR model: it is marked missing and the selection is cleared.
        std::fs::remove_file(&asr).unwrap();
        let r = scan(&paths, &store, &manifest).unwrap();
        let a = r
            .files
            .iter()
            .find(|f| f.path == "models/asr/mine.gguf")
            .unwrap();
        assert!(a.missing);
        assert_eq!(store.get_settings().unwrap().0["selected_model.asr"], "");
        assert!(r.notes.iter().any(|n| n.contains("gone")));
    }

    #[test]
    fn manifest_files_get_measured_vram_and_pull_source() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let store = MemoryStore::new();
        // A fake manifest whose entry matches a tiny file.
        let mut manifest = Manifest::builtin();
        let bytes = build::header(3, 0, &asr_kv());
        manifest.models[0].size_bytes = bytes.len() as u64;
        manifest.models[0].file = "tiny.gguf".into();
        let p = paths
            .models()
            .join("nvidia")
            .join("x")
            .join("rev")
            .join("tiny.gguf");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, &bytes).unwrap();
        let mut verified = p.as_os_str().to_owned();
        verified.push(".verified");
        std::fs::write(
            PathBuf::from(verified),
            format!(
                "sha256={}\nsize={}\nmtime=1\n",
                manifest.models[0].sha256,
                bytes.len()
            ),
        )
        .unwrap();
        let r = scan(&paths, &store, &manifest).unwrap();
        let f = &r.files[0];
        assert_eq!(f.manifest_name.as_deref(), Some("nemotron-3.5"));
        assert_eq!(f.source, ModelSource::Pull);
        assert_eq!(f.capabilities.vram_mb_measured, Some(1_820));
        assert_eq!(
            f.sha256.as_deref(),
            Some(manifest.models[0].sha256.as_str())
        );
    }

    #[test]
    fn finds_engine_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("engine");
        let e1 = root
            .join("nemo-speech")
            .join("nemo-speech-0.2.0-windows-x86_64-cuda");
        std::fs::create_dir_all(e1.join("bin")).unwrap();
        std::fs::write(process::exe_path(&e1), b"MZ").unwrap();
        std::fs::create_dir_all(root.join("empty").join("bin")).unwrap();
        assert_eq!(engine_dirs(&root), vec![e1]);
        assert_eq!(
            version_from_name("nemo-speech-0.2.0-windows-x86_64-cuda").as_deref(),
            Some("0.2.0")
        );
        assert_eq!(version_from_name("custom"), None);
    }

    #[test]
    fn engine_selection_prefers_tested_then_newest() {
        let mk = |id: i64, v: &str| ModelFile {
            id: ModelFileId(id),
            path: format!("engine/{v}"),
            role: ModelRole::Engine,
            architecture: Some(v.into()),
            display_name: v.into(),
            quant: None,
            size_bytes: 1,
            sha256: None,
            source: ModelSource::User,
            capabilities: ModelCapabilities::default(),
            manifest_name: None,
            first_seen_at: 0,
            last_seen_at: 0,
            missing: false,
        };
        let files = vec![mk(1, "0.1.0"), mk(2, "0.2.0"), mk(3, "0.9.0")];
        let pick = recommended(ModelRole::Engine, &files, &Manifest::builtin()).unwrap();
        assert_eq!(pick.id, ModelFileId(2));
    }

    /// The real data layout when `MEET_ASSIST_MODELS_DIR` and
    /// `MEET_ASSIST_ENGINE_DIR` are set (read-only: files are only listed).
    #[test]
    fn scans_the_real_models_when_available() {
        let Some(models) = std::env::var_os("MEET_ASSIST_MODELS_DIR") else {
            return;
        };
        let models = PathBuf::from(models);
        let root = models.parent().unwrap().to_path_buf();
        let paths = DataPaths::at(&root);
        let store = MemoryStore::new();
        let r = scan(&paths, &store, &Manifest::builtin()).unwrap();
        let names: Vec<&str> = r
            .files
            .iter()
            .filter_map(|f| f.manifest_name.as_deref())
            .collect();
        for want in ["nemotron-3.5", "nemotron-3-diarization"] {
            assert!(names.contains(&want), "{want} not found in {names:?}");
        }
        let asr = r
            .files
            .iter()
            .find(|f| f.manifest_name.as_deref() == Some("nemotron-3.5"))
            .unwrap();
        let sel = store.get_settings().unwrap().0["selected_model.asr"].clone();
        assert_eq!(sel, asr.id.to_string());
        let parakeet = r
            .files
            .iter()
            .find(|f| f.manifest_name.as_deref() == Some("parakeet-tdt"));
        if let Some(p) = parakeet {
            assert!(!p.capabilities.streaming);
        }
    }
}
