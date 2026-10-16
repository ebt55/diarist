//! The "recommended" lists shipped in `manifests/`: models (`models.json`)
//! and runtime zips (`engine.json`). They are compiled in; a data root may
//! override them with its own `manifests\*.json` copies.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::types::ModelRole;

const MODELS_JSON: &str = include_str!("../../../../manifests/models.json");
const ENGINE_JSON: &str = include_str!("../../../../manifests/engine.json");

/// User-level model list in the data root, merged over the shipped one.
pub const USER_MODELS: &str = "models.user.json";

/// Runtime versions whose CLI contract matches our fixtures (research/08).
pub const TESTED_ENGINE_VERSIONS: &[&str] = &["0.2.0"];

/// One recommended model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Our name, e.g. `nemotron-3.5`.
    pub name: String,
    /// What the first-run screen and Settings show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Index name for `nemo-speech pull` ("" for [`ModelEntry::direct`] entries).
    #[serde(default)]
    pub pull_name: String,
    pub role: ModelRole,
    /// Downloaded on first run.
    #[serde(default)]
    pub required: bool,
    /// Fetched from `url` by our own downloader into `modelsSer` instead of
    /// with `nemo-speech pull` (entries of `models.user.json`).
    #[serde(default)]
    pub direct: bool,
    /// Offered only behind the experimental setting.
    #[serde(default)]
    pub experimental: bool,
    /// Why an optional model is worth installing, e.g. `multilingual`
    /// (Nemotron 3.5: other or mixed languages; the EN model is the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_for: Option<String>,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub revision: String,
    pub file: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub licence: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub languages: Vec<String>,
    /// Measured peak VRAM delta on the reference GPU.
    #[serde(default)]
    pub vram_mb: Option<u32>,
    #[serde(default)]
    pub vram_note: Option<String>,
    /// Decoder lookahead for file jobs and the post-pass
    /// (`--asr.streaming.rnnt_right_context`, 80 ms frames), copied into the
    /// matching file's capabilities. Explicit, because `-1` (the GGUF
    /// header's value) is 13 for the EN model but 3 for the pinned Nemotron
    /// 3.5. Absent: no flag, the runtime default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_right_context: Option<u32>,
    /// Diarizer preset for file jobs and the post-pass (`--asr.diar.preset`,
    /// `v3-offline` for Nemotron 3 Diarization: the streaming algorithm with
    /// 21 s chunks), copied into the matching file's capabilities. Absent: no
    /// flag, the runtime's streaming default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_preset: Option<String>,
}

impl ModelEntry {
    /// The folder the model lands in: `modelsSer` for direct downloads,
    /// else where `nemo-speech pull` puts it (`<repo><revision>`).
    pub fn dest_dir(&self, models_dir: &Path) -> std::path::PathBuf {
        if self.direct {
            models_dir.join("user")
        } else {
            self.repo
                .split('/')
                .fold(models_dir.to_path_buf(), |p, part| p.join(part))
                .join(&self.revision)
        }
    }
}

/// One runtime zip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineEntry {
    /// Folder name after extraction, e.g. `nemo-speech-0.2.0-windows-x86_64-cuda`.
    pub name: String,
    pub version: String,
    pub backend: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub licence: String,
    #[serde(default)]
    pub tested: bool,
}

/// Both lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub models: Vec<ModelEntry>,
    pub engines: Vec<EngineEntry>,
}

impl Manifest {
    /// The lists compiled into this build.
    pub fn builtin() -> Self {
        Self {
            models: serde_json::from_str(MODELS_JSON).expect("manifests/models.json is valid"),
            engines: serde_json::from_str(ENGINE_JSON).expect("manifests/engine.json is valid"),
        }
    }

    /// The built-in lists, replaced by `<root>/manifests/{models,engine}.json`
    /// when present, then `<root>/models.user.json` merged over the models:
    /// an entry with a known name replaces it, a new one is added. User
    /// entries are fetched from their `url` directly unless they set
    /// `"direct": false` and a `pull_name`.
    pub fn load(data_root: &Path) -> Result<Self> {
        let mut m = Self::builtin();
        let dir = data_root.join("manifests");
        if let Ok(text) = std::fs::read_to_string(dir.join("models.json")) {
            m.models = serde_json::from_str(strip_bom(&text))
                .map_err(|e| CoreError::Models(format!("manifests/models.json: {e}")))?;
        }
        if let Ok(text) = std::fs::read_to_string(dir.join("engine.json")) {
            m.engines = serde_json::from_str(strip_bom(&text))
                .map_err(|e| CoreError::Models(format!("manifests/engine.json: {e}")))?;
        }
        if let Ok(text) = std::fs::read_to_string(data_root.join(USER_MODELS)) {
            let raw: Vec<serde_json::Value> =
                serde_json::from_str(strip_bom(&text)).map_err(|e| {
                    CoreError::Models(format!("{USER_MODELS}: {e} (fix or remove the file)"))
                })?;
            for mut v in raw {
                if let Some(o) = v.as_object_mut() {
                    o.entry("direct").or_insert(serde_json::Value::Bool(true));
                }
                let e: ModelEntry = serde_json::from_value(v).map_err(|e| {
                    CoreError::Models(format!(
                        "{USER_MODELS}: {e} (each entry needs name, role, file, url, size_bytes, sha256)"
                    ))
                })?;
                match m
                    .models
                    .iter_mut()
                    .find(|x| x.name.eq_ignore_ascii_case(&e.name))
                {
                    Some(x) => *x = e,
                    None => m.models.push(e),
                }
            }
        }
        Ok(m)
    }

    /// The newest package for `backend` (`cuda`, `vulkan`, `cpu`), tested first.
    pub fn engine_for_backend(&self, backend: &str) -> Option<&EngineEntry> {
        let of = || {
            self.engines
                .iter()
                .filter(|e| e.backend.eq_ignore_ascii_case(backend))
        };
        of().filter(|e| e.tested)
            .max_by(|a, b| version_cmp(&a.version, &b.version))
            .or_else(|| of().max_by(|a, b| version_cmp(&a.version, &b.version)))
    }

    /// A model by our name or its pull name (case-insensitive).
    pub fn model(&self, name: &str) -> Option<&ModelEntry> {
        self.models
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name) || m.pull_name.eq_ignore_ascii_case(name))
    }

    /// The entry a file on disk corresponds to: same file name and size.
    pub fn match_file(&self, file_name: &str, size_bytes: u64) -> Option<&ModelEntry> {
        self.models
            .iter()
            .find(|m| m.file.eq_ignore_ascii_case(file_name) && m.size_bytes == size_bytes)
    }

    /// Models fetched on first run.
    pub fn required(&self) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(|m| m.required)
    }

    /// The newest tested engine, else the newest.
    pub fn default_engine(&self) -> Option<&EngineEntry> {
        self.engines
            .iter()
            .filter(|e| e.tested)
            .max_by(|a, b| version_cmp(&a.version, &b.version))
            .or_else(|| {
                self.engines
                    .iter()
                    .max_by(|a, b| version_cmp(&a.version, &b.version))
            })
    }
}

/// Text without a leading UTF-8 byte-order mark (Windows editors and
/// PowerShell 5 write one).
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Compares dotted versions numerically (`0.10.0` > `0.9.1`).
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split(['.', '-'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    parse(a).cmp(&parse(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_manifest_has_the_documented_models() {
        let m = Manifest::builtin();
        let required: Vec<&str> = m.required().map(|e| e.name.as_str()).collect();
        // The EN model is the default ASR (docs/eval, 15 Oct 2026); 3.5 is the
        // optional pick for other or mixed languages.
        assert_eq!(required, ["nemotron-3-diarization", "nemotron-en"]);
        let parakeet = m.model("parakeet-tdt").unwrap();
        assert!(parakeet.experimental && !parakeet.required);
        let multi = m.model("nemotron-3.5").unwrap();
        assert!(!multi.required && !multi.experimental);
        assert_eq!(multi.recommended_for.as_deref(), Some("multilingual"));
        for e in &m.models {
            assert_eq!(e.sha256.len(), 64, "{}", e.name);
            assert_eq!(e.revision.len(), 40, "{}", e.name);
            assert!(e.size_bytes > 100_000_000, "{}", e.name);
            assert!(e.url.contains(&e.revision) && e.url.ends_with(&e.file));
        }
        let asr = m.match_file("nemotron-3.5-asr-streaming-0.6b.q8_0.gguf", 741_548_352);
        assert_eq!(asr.unwrap().role, ModelRole::Asr);
        assert!(m
            .match_file("nemotron-3.5-asr-streaming-0.6b.q8_0.gguf", 1)
            .is_none());
        let engine = m.default_engine().unwrap();
        assert_eq!(engine.version, "0.2.0");
        assert_eq!(engine.sha256.len(), 64);
        assert!(TESTED_ENGINE_VERSIONS.contains(&engine.version.as_str()));
    }

    #[test]
    fn file_lookahead_is_explicit_for_both_nemotron_models() {
        let m = Manifest::builtin();
        let rc = |n: &str| m.model(n).unwrap().file_right_context;
        // 13 frames (1.12 s) for both, never `-1`: the pinned 3.5 GGUF's
        // header says 3 (docs/plans/12, section 1).
        assert_eq!(rc("nemotron-en"), Some(13));
        assert_eq!(rc("nemotron-3.5"), Some(13));
        assert_eq!(rc("nemotron-3-diarization"), None);
        assert_eq!(rc("parakeet-tdt"), None, "offline TDT: no such key");
        // Round trip: written where set, left out elsewhere.
        let json = serde_json::to_string(&m.models).unwrap();
        assert_eq!(json.matches(r#""file_right_context":13"#).count(), 2);
        assert_eq!(json.matches("file_right_context").count(), 2);
        let back: Vec<ModelEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m.models);
    }

    #[test]
    fn file_preset_is_offline_for_the_diarizer() {
        let m = Manifest::builtin();
        let preset = |n: &str| m.model(n).unwrap().file_preset.clone();
        assert_eq!(
            preset("nemotron-3-diarization").as_deref(),
            Some("v3-offline")
        );
        for asr in ["nemotron-en", "nemotron-3.5", "parakeet-tdt"] {
            assert_eq!(preset(asr), None, "{asr}");
        }
        // Round trip: written where set, left out elsewhere.
        let json = serde_json::to_string(&m.models).unwrap();
        assert_eq!(json.matches(r#""file_preset":"v3-offline""#).count(), 1);
        assert_eq!(json.matches("file_preset").count(), 1);
        let back: Vec<ModelEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m.models);
    }

    #[test]
    fn versions_compare_numerically() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("0.10.0", "0.9.1"), Ordering::Greater);
        assert_eq!(version_cmp("0.2.0", "0.2.0"), Ordering::Equal);
        assert_eq!(version_cmp("0.2", "0.2.1"), Ordering::Less);
    }

    #[test]
    fn user_models_merge_over_the_shipped_list_and_backends_resolve() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(USER_MODELS),
            r#"[
              {"name": "my-asr", "role": "asr", "file": "my-asr.gguf",
               "url": "https://example.com/my-asr.gguf", "size_bytes": 1234,
               "sha256": "0000000000000000000000000000000000000000000000000000000000000000"},
              {"name": "nemotron-en", "role": "asr", "file": "nemotron-speech-streaming-en-0.6b.q8_0.gguf",
               "url": "https://mirror.example/en.gguf", "size_bytes": 699872960, "required": true,
               "sha256": "d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d"}
            ]"#,
        )
        .unwrap();
        let m = Manifest::load(dir.path()).unwrap();
        let mine = m.model("my-asr").unwrap();
        assert!(mine.direct && !mine.required);
        assert_eq!(mine.dest_dir(Path::new("M")), Path::new("M").join("user"));
        let en = m.model("nemotron-en").unwrap();
        assert_eq!(en.url, "https://mirror.example/en.gguf", "replaced by name");
        assert_eq!(m.models.len(), Manifest::builtin().models.len() + 1);
        let shipped = Manifest::builtin();
        let diar = shipped.model("nemotron-3-diarization").unwrap();
        assert!(!diar.direct);
        assert!(diar.dest_dir(Path::new("M")).ends_with(&diar.revision));

        assert_eq!(m.engine_for_backend("cuda").unwrap().backend, "cuda");
        assert_eq!(
            m.engine_for_backend("VULKAN").unwrap().size_bytes,
            19_302_464
        );
        assert_eq!(m.engine_for_backend("cpu").unwrap().version, "0.2.0");
        assert!(m.engine_for_backend("metal").is_none());
        assert_eq!(m.default_engine().unwrap().backend, "cuda");

        std::fs::write(dir.path().join(USER_MODELS), r#"[{"name": "broken"}]"#).unwrap();
        let err = Manifest::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("each entry needs"), "{err}");
    }

    #[test]
    fn data_root_copy_overrides() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()).unwrap(), Manifest::builtin());
        std::fs::create_dir_all(dir.path().join("manifests")).unwrap();
        // With the BOM PowerShell 5 writes.
        std::fs::write(dir.path().join("manifests/engine.json"), "\u{feff}[]").unwrap();
        assert!(Manifest::load(dir.path()).unwrap().engines.is_empty());
        std::fs::write(dir.path().join("manifests/models.json"), "{").unwrap();
        assert!(Manifest::load(dir.path()).is_err());
    }
}
