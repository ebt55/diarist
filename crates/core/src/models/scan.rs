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
/// Whether a directory entry is a folder, following a junction or symlink
/// (a model folder linked in from elsewhere is scanned like a real one; the
/// walk depth bounds link cycles).
fn entry_is_dir(e: &std::fs::DirEntry) -> bool {
    match e.file_type() {
        Ok(t) if t.is_symlink() => std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()),
        Ok(t) => t.is_dir(),
        Err(_) => false,
    }
}

fn walk(dir: &Path, depth: u32, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if entry_is_dir(&e) {
            if depth > 0 {
                walk(&p, depth - 1, f);
            }
        } else if std::fs::metadata(&p).is_ok_and(|m| m.is_file()) {
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
                .filter(entry_is_dir)
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for d in dirs {
                visit(&d, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if process::is_engine_dir(engine_root) {
        return vec![engine_root.to_path_buf()];
    }
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

/// `<file>.sha256`: our cached SHA-256 of a file the runtime did not verify
/// (a hand-placed download). Trusted while it is not older than the file.
fn sidecar_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

/// The file's SHA-256 from the `.sha256` cache, else computed (seconds for
/// a model) and cached. `None` when the file cannot be read.
fn own_sha(path: &Path) -> Option<String> {
    let side = sidecar_path(path);
    let fresh = match (std::fs::metadata(path), std::fs::metadata(&side)) {
        (Ok(f), Ok(s)) => match (f.modified(), s.modified()) {
            (Ok(fm), Ok(sm)) => sm >= fm,
            _ => false,
        },
        _ => false,
    };
    if fresh {
        if let Some(h) = std::fs::read_to_string(&side)
            .ok()
            .and_then(|t| t.split_whitespace().next().map(str::to_ascii_lowercase))
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Some(h);
        }
    }
    let h = super::download::sha256_file(path).ok()?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = std::fs::write(
        &side,
        format!(
            "{h}  {name}
"
        ),
    );
    Some(h)
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
    let candidate = manifest.match_file(&file_name, size);
    // A recommended file the runtime did not fetch (put there by hand): its
    // hash must match the manifest before it counts as that model.
    let own = match (&verified, candidate) {
        (None, Some(_)) => own_sha(path),
        _ => None,
    };
    let entry = candidate.filter(|m| {
        let got = verified.as_deref().or(own.as_deref());
        got.is_some_and(|h| h.eq_ignore_ascii_case(&m.sha256))
    });
    if let (Some(m), None) = (candidate, entry) {
        problem = Some(format!(
            "{}: same name and size as {} but SHA-256 {} instead of {}; not used as {}",
            path.display(),
            m.name,
            verified
                .as_deref()
                .or(own.as_deref())
                .unwrap_or("unreadable"),
            m.sha256,
            m.name
        ));
    }
    let sha256 = verified
        .clone()
        .or(own)
        .or_else(|| entry.map(|m| m.sha256.clone()));
    if let Some(m) = entry {
        if c.capabilities.vram_mb_measured.is_none() {
            c.capabilities.vram_mb_measured = m.vram_mb;
        }
        // The sidecar's values win, as for VRAM.
        if c.capabilities.file_right_context.is_none() {
            c.capabilities.file_right_context = m.file_right_context;
        }
        if c.capabilities.file_preset.is_none() {
            c.capabilities.file_preset = m.file_preset.clone();
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
            files.iter().any(|f| {
                f.id == id
                    && (f.role == role
                        || (f.role == ModelRole::Unknown && role != ModelRole::Engine))
                    && !f.missing
            })
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
    fn hand_placed_files_are_hash_checked_and_linked_folders_followed() {
        use crate::models::ModelEntry;
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        let store = MemoryStore::new();
        // A tiny "recommended" model: a valid GGUF header is enough.
        let good = dir.path().join("dl").join("tiny-asr.q8_0.gguf");
        write_gguf(&good, &asr_kv());
        let bytes = std::fs::read(&good).unwrap();
        let entry = ModelEntry {
            name: "tiny".into(),
            label: None,
            pull_name: "tiny".into(),
            role: ModelRole::Asr,
            required: true,
            direct: false,
            experimental: false,
            recommended_for: None,
            repo: "nvidia/tiny".into(),
            revision: "abc".into(),
            file: "tiny-asr.q8_0.gguf".into(),
            url: "https://example.invalid/tiny".into(),
            size_bytes: bytes.len() as u64,
            sha256: crate::models::download::sha256_file(&good).unwrap(),
            licence: "x".into(),
            streaming: true,
            languages: vec![],
            vram_mb: Some(100),
            vram_note: None,
            file_right_context: None,
            file_preset: None,
        };
        let manifest = Manifest {
            models: vec![entry],
            engines: vec![],
        };
        // Put by hand into the models folder: recognised after a hash check.
        let placed = paths.models().join("tiny-asr.q8_0.gguf");
        std::fs::create_dir_all(paths.models()).unwrap();
        std::fs::copy(&good, &placed).unwrap();
        let r = scan(&paths, &store, &manifest).unwrap();
        let f = r
            .files
            .iter()
            .find(|f| f.path.ends_with("tiny-asr.q8_0.gguf"))
            .unwrap();
        assert_eq!(f.manifest_name.as_deref(), Some("tiny"), "{r:?}");
        assert_eq!(f.source, ModelSource::Manifest);
        assert!(sidecar_path(&placed).is_file(), "the hash is cached");

        // Same name and size, other bytes: not the recommended model.
        let mut evil = bytes.clone();
        let last = evil.len() - 1;
        evil[last] ^= 0x55;
        std::fs::write(&placed, &evil).unwrap();
        // Newer than the cached hash, so it is hashed again.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&placed)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let r = scan(&paths, &store, &manifest).unwrap();
        let f = r
            .files
            .iter()
            .find(|f| f.path.ends_with("tiny-asr.q8_0.gguf"))
            .unwrap();
        assert_eq!(f.manifest_name, None);
        assert!(
            r.errors.iter().any(|e| e.contains("instead of")),
            "{:?}",
            r.errors
        );

        // A folder linked into models\ is scanned like a real one.
        std::fs::remove_file(&placed).unwrap();
        let _ = std::fs::remove_file(sidecar_path(&placed));
        let link = paths.models().join("linked");
        let made = if cfg!(windows) {
            std::process::Command::new("cmd")
                .arg("/c")
                .arg("mklink")
                .arg("/J")
                .arg(&link)
                .arg(good.parent().unwrap())
                .output()
                .is_ok_and(|o| o.status.success())
        } else {
            false
        };
        if made {
            let r = scan(&paths, &store, &manifest).unwrap();
            assert!(
                r.files.iter().any(|f| !f.missing
                    && f.path.ends_with("linked/tiny-asr.q8_0.gguf")
                    && f.manifest_name.as_deref() == Some("tiny")),
                "{:?}",
                r.files
            );
            std::fs::remove_dir(&link).unwrap(); // the link only
            assert!(good.is_file());
        }
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
        manifest.models[0].file_preset = Some("v3-offline".into());
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
        // The manifest's file lookahead and preset reach the capabilities; a
        // sidecar wins.
        assert_eq!(f.capabilities.file_right_context, Some(13));
        assert_eq!(f.capabilities.file_preset.as_deref(), Some("v3-offline"));
        std::fs::write(
            Sidecar::path_for(&p),
            r#"{"file_right_context": 6, "file_preset": "v3-streaming"}"#,
        )
        .unwrap();
        let r = scan(&paths, &store, &manifest).unwrap();
        assert_eq!(r.files[0].capabilities.file_right_context, Some(6));
        assert_eq!(
            r.files[0].capabilities.file_preset.as_deref(),
            Some("v3-streaming")
        );
        assert_eq!(r.files[0].manifest_name.as_deref(), Some("nemotron-3.5"));
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

    /// The real data layout when `DIARIST_MODELS_DIR` and
    /// `DIARIST_ENGINE_DIR` are set (read-only: files are only listed).
    #[test]
    fn scans_the_real_models_when_available() {
        let Some(models) = std::env::var_os("DIARIST_MODELS_DIR") else {
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
        for want in ["nemotron-en", "nemotron-3-diarization"] {
            assert!(names.contains(&want), "{want} not found in {names:?}");
        }
        // The default ASR selection is the EN model.
        let asr = r
            .files
            .iter()
            .find(|f| f.manifest_name.as_deref() == Some("nemotron-en"))
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
