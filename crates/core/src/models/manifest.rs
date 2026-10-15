//! The "recommended" lists shipped in `manifests/`: models (`models.json`)
//! and runtime zips (`engine.json`). They are compiled in; a data root may
//! override them with its own `manifests\*.json` copies.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::types::ModelRole;

const MODELS_JSON: &str = include_str!("../../../../manifests/models.json");
const ENGINE_JSON: &str = include_str!("../../../../manifests/engine.json");

/// Runtime versions whose CLI contract matches our fixtures (research/08).
pub const TESTED_ENGINE_VERSIONS: &[&str] = &["0.2.0"];

/// One recommended model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Our name, e.g. `nemotron-3.5`.
    pub name: String,
    /// Index name for `nemo-speech pull`.
    pub pull_name: String,
    pub role: ModelRole,
    /// Downloaded on first run.
    pub required: bool,
    /// Offered only behind the experimental setting.
    #[serde(default)]
    pub experimental: bool,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
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

    /// `<root>/manifests/{models,engine}.json` when present, else the built-in lists.
    pub fn load(data_root: &Path) -> Result<Self> {
        let mut m = Self::builtin();
        let dir = data_root.join("manifests");
        if let Ok(text) = std::fs::read_to_string(dir.join("models.json")) {
            m.models = serde_json::from_str(&text)
                .map_err(|e| CoreError::Models(format!("manifests/models.json: {e}")))?;
        }
        if let Ok(text) = std::fs::read_to_string(dir.join("engine.json")) {
            m.engines = serde_json::from_str(&text)
                .map_err(|e| CoreError::Models(format!("manifests/engine.json: {e}")))?;
        }
        Ok(m)
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
        assert_eq!(required, ["nemotron-3.5", "nemotron-3-diarization"]);
        let parakeet = m.model("parakeet-tdt").unwrap();
        assert!(parakeet.experimental && !parakeet.required);
        assert!(!m.model("nemotron-en").unwrap().required);
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
    fn versions_compare_numerically() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("0.10.0", "0.9.1"), Ordering::Greater);
        assert_eq!(version_cmp("0.2.0", "0.2.0"), Ordering::Equal);
        assert_eq!(version_cmp("0.2", "0.2.1"), Ordering::Less);
    }

    #[test]
    fn data_root_copy_overrides() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()).unwrap(), Manifest::builtin());
        std::fs::create_dir_all(dir.path().join("manifests")).unwrap();
        std::fs::write(dir.path().join("manifests/engine.json"), "[]").unwrap();
        assert!(Manifest::load(dir.path()).unwrap().engines.is_empty());
        std::fs::write(dir.path().join("manifests/models.json"), "{").unwrap();
        assert!(Manifest::load(dir.path()).is_err());
    }
}
