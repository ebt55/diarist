//! The "recommended" lists shipped in `manifests/`: models (`models.json`)
//! and runtime zips (`engine.json`). They are compiled in; a data root may
//! override them with its own `manifests\*.json` copies.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::punct::spec::PunctSpec;
use crate::types::ModelRole;

const MODELS_JSON: &str = include_str!("../../../../manifests/models.json");
const ENGINE_JSON: &str = include_str!("../../../../manifests/engine.json");

/// User-level model list in the data root, merged over the shipped one.
pub const USER_MODELS: &str = "models.user.json";

/// Runtime versions whose CLI contract matches our fixtures (research/08).
pub const TESTED_ENGINE_VERSIONS: &[&str] = &["0.2.0"];

/// Decoder lookaheads (`--asr.streaming.rnnt_right_context`, 80 ms frames)
/// the shipped streaming models were trained for, from their model cards'
/// `att_context_size` lists (docs/research/17, lever 3): the EN model
/// {0, 1, 6, 13}, Nemotron 3.5 {0, 1, 3, 6, 13}. The runtime takes any
/// value without an error (docs/research/12 section 1), so the live
/// lookahead is checked against these ([`crate::models::live_right_context`]).
const TRAINED_LIVE_RIGHT_CONTEXTS: &[(&str, &[u32])] = &[
    ("nemotron-en", &[0, 1, 6, 13]),
    ("nemotron-3.5", &[0, 1, 3, 6, 13]),
];

/// The lookaheads manifest model `name` was trained for; `None` for a model
/// whose values are not known (any other entry), which is not checked.
pub fn trained_live_right_contexts(name: &str) -> Option<&'static [u32]> {
    TRAINED_LIVE_RIGHT_CONTEXTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
}

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
    /// Decoder lookahead of the live server (`nemo-speech serve
    /// --asr.streaming.rnnt_right_context`, 80 ms frames), copied into the
    /// matching file's capabilities. Must be one the model was trained for
    /// ([`trained_live_right_contexts`]). Absent: no flag, the runtime
    /// default (1 frame, 160 ms; docs/plans/16 section 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_right_context: Option<u32>,
    /// Diarizer preset for file jobs and the post-pass (`--asr.diar.preset`,
    /// `v3-offline` for Nemotron 3 Diarization: the streaming algorithm with
    /// 21 s chunks), copied into the matching file's capabilities. Absent: no
    /// flag, the runtime's streaming default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_preset: Option<String>,
    /// More files of a multi-file model (a punctuation model: its tokenizer,
    /// licence files and the ONNX Runtime library), fetched before the
    /// primary `file`, so the scanner (keyed on the primary file) never sees
    /// a selectable model with a part missing. `size_bytes` is the total of
    /// all files; `file`, `url` and `sha256` describe the primary file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ModelExtraFile>,
    /// The folder under `models\` the entry lands in, instead of `user`
    /// (direct entries) or `<repo>\<revision>` (pulled ones). Relative, no
    /// `..`: `models.user.json` is user-writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The description block of a punctuation model (docs/plans/16 section
    /// 3.6), copied into the matched file's capabilities by the scanner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub punctuation: Option<PunctSpec>,
}

/// One more file of a [`ModelEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelExtraFile {
    /// Relative to the entry's folder; Hugging Face names may be nested
    /// (`onnx/model_quantized.onnx`, `tokenizer.json` at the root).
    pub path: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl ModelEntry {
    /// The folder the model lands in: `models\<dir>` when the entry names
    /// one, else `models\user` for direct downloads and where `nemo-speech
    /// pull` puts a pulled model (`<repo>\<revision>`).
    pub fn dest_dir(&self, models_dir: &Path) -> std::path::PathBuf {
        if let Some(dir) = self.dir.as_deref().filter(|d| !d.trim().is_empty()) {
            crate::punct::spec::rel_path(models_dir, dir)
        } else if self.direct {
            models_dir.join("user")
        } else {
            self.repo
                .split('/')
                .fold(models_dir.to_path_buf(), |p, part| p.join(part))
                .join(&self.revision)
        }
    }

    /// The primary file's name, without a folder (`file` may be nested).
    pub fn file_name(&self) -> &str {
        self.file.rsplit(['/', '\\']).next().unwrap_or(&self.file)
    }

    /// The primary file's size: `size_bytes` less the other files.
    pub fn primary_size_bytes(&self) -> u64 {
        self.size_bytes
            .saturating_sub(self.files.iter().map(|f| f.size_bytes).sum())
    }

    /// Checks the fields a user can get wrong in `models.user.json`: the
    /// paths (relative, no `..`) and the description block. Errors name the
    /// field.
    pub fn validate(&self) -> std::result::Result<(), String> {
        check_relative_path("file", &self.file)?;
        if let Some(dir) = &self.dir {
            check_relative_path("dir", dir)?;
        }
        for (i, f) in self.files.iter().enumerate() {
            check_relative_path(&format!("files[{i}].path"), &f.path)?;
            if f.path
                .replace('\\', "/")
                .eq_ignore_ascii_case(&self.file.replace('\\', "/"))
            {
                return Err(format!("files[{i}].path: the primary file again"));
            }
        }
        if self.files.iter().map(|f| f.size_bytes).sum::<u64>() > self.size_bytes {
            return Err("size_bytes: smaller than the other files together".into());
        }
        if let Some(spec) = &self.punctuation {
            spec.validate().map_err(|e| format!("punctuation.{e}"))?;
        }
        Ok(())
    }
}

/// A path from a manifest or description block: relative, inside the
/// folder it is relative to (no `..`, no drive, no root, no `\\?\`).
/// `Err` starts with `field`.
pub fn check_relative_path(field: &str, path: &str) -> std::result::Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err(format!("{field}: empty"));
    }
    let bad = p.starts_with(['/', '\\'])
        || p.contains(':')
        || p.split(['/', '\\']).any(|part| part == "..");
    if bad || Path::new(p).is_absolute() {
        return Err(format!(
            "{field}: {path:?} must be a relative path inside the model's folder (no `..`, no drive)"
        ));
    }
    Ok(())
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
                e.validate().map_err(|err| {
                    CoreError::Models(format!("{USER_MODELS}: {}: {err}", e.name))
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
        for e in &m.models {
            e.validate().map_err(|err| {
                CoreError::Models(format!("manifests/models.json: {}: {err}", e.name))
            })?;
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

    /// The entry a file on disk corresponds to: same file name and size (of
    /// the primary file, for a multi-file entry).
    pub fn match_file(&self, file_name: &str, size_bytes: u64) -> Option<&ModelEntry> {
        self.models.iter().find(|m| {
            m.file_name().eq_ignore_ascii_case(file_name) && m.primary_size_bytes() == size_bytes
        })
    }

    /// Models fetched on first run.
    pub fn required(&self) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(|m| m.required)
    }

    /// The newest tested engine, else the newest; among packages of the same
    /// version the first listed (CUDA in the shipped manifest). `max_by`
    /// keeps the last of equal elements, hence the `rev()`.
    pub fn default_engine(&self) -> Option<&EngineEntry> {
        self.engines
            .iter()
            .rev()
            .filter(|e| e.tested)
            .max_by(|a, b| version_cmp(&a.version, &b.version))
            .or_else(|| {
                self.engines
                    .iter()
                    .rev()
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
    fn live_lookahead_is_unset_until_measured_and_trained_when_set() {
        let m = Manifest::builtin();
        // docs/plans/16 section 5: no live flag until the measurement passes
        // (chunk F part 2 sets 6 for both Nemotron models here).
        for name in ["nemotron-en", "nemotron-3.5", "nemotron-3-diarization"] {
            assert_eq!(m.model(name).unwrap().live_right_context, None, "{name}");
        }
        // Whatever the manifest sets is a value the model was trained for.
        for e in &m.models {
            if let Some(n) = e.live_right_context {
                let trained = trained_live_right_contexts(&e.name);
                assert!(trained.is_some_and(|v| v.contains(&n)), "{}: {n}", e.name);
            }
        }
        assert_eq!(
            trained_live_right_contexts("nemotron-en"),
            Some(&[0, 1, 6, 13][..])
        );
        assert_eq!(
            trained_live_right_contexts("Nemotron-3.5"),
            Some(&[0, 1, 3, 6, 13][..])
        );
        assert_eq!(trained_live_right_contexts("parakeet-tdt"), None);
        // Round trip: left out where unset, read back where set.
        let json = serde_json::to_string(&m.models).unwrap();
        assert!(!json.contains("live_right_context"), "{json}");
        let mut en = m.model("nemotron-en").unwrap().clone();
        en.live_right_context = Some(6);
        let text = serde_json::to_string(&en).unwrap();
        assert!(text.contains(r#""live_right_context":6"#), "{text}");
        assert_eq!(serde_json::from_str::<ModelEntry>(&text).unwrap(), en);
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

    /// A punctuation entry in `models.user.json`: nested primary file, the
    /// tokenizer and the runtime as extra files, its own folder and the
    /// description block (the shape chunk D ships in `models.json`).
    const PUNCT_ENTRY: &str = r#"[{
        "name": "punct-small", "role": "punctuation",
        "repo": "Horizon-Labs/punctuation-restoration-small",
        "revision": "ee31fdfc5304f876ea3b0dd72d30eda7e940b2b1",
        "file": "onnx/model_quantized.onnx",
        "url": "https://example.invalid/onnx/model_quantized.onnx",
        "size_bytes": 1600,
        "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
        "files": [
            {"path": "tokenizer.json", "url": "https://example.invalid/tokenizer.json",
             "size_bytes": 500, "sha256": "2222222222222222222222222222222222222222222222222222222222222222"},
            {"path": "diarist-onnxruntime.dll", "url": "https://example.invalid/onnxruntime.dll",
             "size_bytes": 100, "sha256": "3333333333333333333333333333333333333333333333333333333333333333"}
        ],
        "dir": "Horizon-Labs/punctuation-restoration-small/ee31fdf",
        "languages": ["*"],
        "punctuation": {
            "tokenizer": "tokenizer.json", "max_tokens": 512, "context_tokens": 48,
            "bos_id": 2, "eos_id": 1,
            "inputs": {"ids": "input_ids", "mask": "attention_mask"},
            "heads": [{"output": "logits", "kind": "post", "labels": ["", ".", ",", "?", "-", ":"],
                       "position": "last_token", "values": "logits"}],
            "runtime_version": "1.31.0"
        }
    }]"#;

    #[test]
    fn multi_file_entries_round_trip_and_land_in_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(USER_MODELS), PUNCT_ENTRY).unwrap();
        let m = Manifest::load(dir.path()).unwrap();
        let e = m.model("punct-small").unwrap();
        assert_eq!(e.role, ModelRole::Punctuation);
        assert!(e.direct, "user entries are direct");
        assert_eq!(e.files.len(), 2);
        assert_eq!(e.file_name(), "model_quantized.onnx");
        assert_eq!(e.primary_size_bytes(), 1000, "size_bytes is the total");
        assert_eq!(e.punctuation.as_ref().unwrap().max_tokens, 512);
        // `dir` wins over `user` and over `<repo>\<revision>`.
        let models = Path::new("M");
        assert_eq!(
            e.dest_dir(models),
            models
                .join("Horizon-Labs")
                .join("punctuation-restoration-small")
                .join("ee31fdf")
        );
        // The scanner matches the primary file by its own size.
        assert!(m.match_file("model_quantized.onnx", 1000).is_some());
        assert!(m.match_file("model_quantized.onnx", 1600).is_none());
        // Round trip, and the new fields stay out of single-file entries.
        let back: Vec<ModelEntry> =
            serde_json::from_str(&serde_json::to_string(&m.models).unwrap()).unwrap();
        assert_eq!(back, m.models);
        let shipped = serde_json::to_string(&Manifest::builtin().models).unwrap();
        for key in ["\"files\"", "\"dir\"", "\"punctuation\""] {
            assert!(!shipped.contains(key), "{key}");
        }

        // A path out of the folder, an absolute one, a broken block: refused,
        // naming the field.
        for (from, to, field) in [
            (
                r#""path": "tokenizer.json""#,
                r#""path": "../../tokenizer.json""#,
                "files[0].path",
            ),
            (
                r#""dir": "Horizon-Labs"#,
                r#""dir": "C:\\Windows\\Horizon-Labs"#,
                "dir",
            ),
            (r#""file": "onnx/"#, r#""file": "/onnx/"#, "file"),
            (
                r#""heads": [{"output""#,
                r#""heads": [], "x": [{"output""#,
                "punctuation.heads",
            ),
        ] {
            assert!(PUNCT_ENTRY.contains(from), "{from}");
            std::fs::write(dir.path().join(USER_MODELS), PUNCT_ENTRY.replace(from, to)).unwrap();
            let err = Manifest::load(dir.path()).unwrap_err().to_string();
            assert!(
                err.contains(&format!("punct-small: {field}")),
                "{field}: {err}"
            );
        }
        for e in &Manifest::builtin().models {
            e.validate().unwrap();
        }
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
