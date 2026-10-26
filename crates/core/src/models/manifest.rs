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
    /// Index name for `nemo-speech pull`, used only by `diarist-cli models
    /// pull --via-runtime` (the old path, kept for one release); "" for
    /// [`ModelEntry::direct`] entries.
    #[serde(default)]
    pub pull_name: String,
    pub role: ModelRole,
    /// Downloaded on first run.
    #[serde(default)]
    pub required: bool,
    /// Lands in `models\user` instead of `models\<repo>\<revision>` (entries
    /// of `models.user.json` default to it). Every entry, direct or not, is
    /// fetched from `url` by Diarist's own downloader.
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
    /// (direct entries) or `<repo>\<revision>` (the others). Relative, no
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
    /// one, else `models\user` for direct entries, else `<repo>\<revision>`
    /// (the layout `nemo-speech pull` used, so files earlier versions pulled
    /// are found). [`ModelEntry::validate`] keeps every part a plain
    /// relative name, so this never leaves `models_dir`.
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

    /// Checks the fields a user can get wrong in `models.user.json` (or a
    /// data root's `manifests\models.json`): the download address, the
    /// paths the entry lands at (`repo` as `owner/name`, `revision`, `file`,
    /// `dir`, `files`: relative, no `..`, no Windows device names, each
    /// file once), the hashes of a multi-file entry and the description
    /// block. Errors name the field.
    pub fn validate(&self) -> std::result::Result<(), String> {
        check_url("url", &self.url)?;
        check_relative_path("file", &self.file)?;
        if !self.repo.trim().is_empty() {
            check_relative_path("repo", &self.repo)?;
            let parts: Vec<&str> = self.repo.split(['/', '\\']).collect();
            if parts.len() != 2 || parts.iter().any(|p| p.is_empty() || *p == ".") {
                return Err(format!(
                    "repo: {:?} must be owner/name (two parts)",
                    self.repo
                ));
            }
        }
        if !self.revision.trim().is_empty() {
            check_relative_path("revision", &self.revision)?;
        }
        if let Some(dir) = &self.dir {
            check_relative_path("dir", dir)?;
        }
        let lands_in_repo = !self.direct && self.dir.as_deref().is_none_or(|d| d.trim().is_empty());
        if lands_in_repo && (self.repo.trim().is_empty() || self.revision.trim().is_empty()) {
            return Err(
                "repo: an entry that is not `direct` and has no `dir` lands in \
                 <repo>\\<revision>, so it needs both"
                    .into(),
            );
        }
        let primary = normalized_relative(&self.file);
        let mut seen: Vec<String> = Vec::with_capacity(self.files.len());
        for (i, f) in self.files.iter().enumerate() {
            check_relative_path(&format!("files[{i}].path"), &f.path)?;
            check_url(&format!("files[{i}].url"), &f.url)?;
            let n = normalized_relative(&f.path);
            if n == primary {
                return Err(format!("files[{i}].path: the primary file again"));
            }
            if seen.contains(&n) {
                return Err(format!("files[{i}].path: {:?} is listed twice", f.path));
            }
            seen.push(n);
            // A file without a hash could never be told complete, so the
            // entry would never count as installed (setup.rs).
            if !is_sha256(&f.sha256) {
                return Err(format!(
                    "files[{i}].sha256: {:?} is not a SHA-256 (64 hex digits); every file of a \
                     multi-file entry needs one",
                    f.sha256
                ));
            }
        }
        if !self.files.is_empty() && !is_sha256(&self.sha256) {
            return Err(format!(
                "sha256: {:?} is not a SHA-256 (64 hex digits); a multi-file entry needs the \
                 primary file's too",
                self.sha256
            ));
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
/// folder it is relative to (no `..`, no drive, no root, no `\\?\`), and a
/// name Windows keeps as written: no device name (`CON`, `NUL`, `AUX`,
/// `PRN`, `COM0` to `COM9`, `LPT0` to `LPT9`, also with an extension), no
/// part ending in `.` or a space, none of `< > " | ? *` or a control
/// character (each part as [`check_engine_name`] checks a whole engine
/// name). `Err` starts with `field`.
pub fn check_relative_path(field: &str, path: &str) -> std::result::Result<(), String> {
    if path.trim().is_empty() {
        return Err(format!("{field}: empty"));
    }
    let bad = path.trim_start().starts_with(['/', '\\'])
        || path.contains(':')
        || path.split(['/', '\\']).any(|part| part == "..");
    if bad || Path::new(path).is_absolute() {
        return Err(format!(
            "{field}: {path:?} must be a relative path inside the model's folder (no `..`, no drive)"
        ));
    }
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() {
        return Err(format!("{field}: {path:?} names no file"));
    }
    for part in parts {
        if let Some(why) = plain_name_problem(part) {
            return Err(format!("{field}: {path:?}: {part:?} {why}"));
        }
    }
    Ok(())
}

/// Why `name` is not one file or folder name that Windows keeps as
/// written, or `None` when it is: not empty, `.` or `..`; no separator,
/// drive or `:`; none of `< > " | ? *` or a control character; no trailing
/// dot or space; not a device name ([`is_dos_device`]). The one rule behind
/// each part of a manifest path ([`check_relative_path`]) and an engine
/// folder name ([`check_engine_name`]).
fn plain_name_problem(name: &str) -> Option<String> {
    if name.is_empty() || name == "." || name == ".." {
        return Some("is not a file or folder name".into());
    }
    if let Some(c) = name.chars().find(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
    }) {
        return Some(format!(
            "has {c:?}, a character Windows does not allow in a name"
        ));
    }
    if name.ends_with(['.', ' ']) {
        return Some("ends in a dot or a space, which Windows drops".into());
    }
    if is_dos_device(name) {
        return Some("is a device name on Windows, not a file".into());
    }
    None
}

/// A download address: `http://` or `https://` and something after it.
/// Every entry is fetched from its `url` by Diarist's own downloader, so an
/// entry without one is refused when the list is read, not at download
/// time. `Err` starts with `field`.
pub fn check_url(field: &str, url: &str) -> std::result::Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err(format!(
            "{field}: empty; Diarist downloads every file from its url"
        ));
    }
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .filter(|r| !r.is_empty() && !r.starts_with('/'));
    if rest.is_none() || u.chars().any(char::is_whitespace) {
        return Err(format!("{field}: {url:?} is not an http(s) address"));
    }
    Ok(())
}

/// Whether `part` is a name Windows maps to a device: `CON`, `PRN`, `AUX`,
/// `NUL`, `COM0` to `COM9`, `LPT0` to `LPT9` (and `COM¹` to `COM³`, `LPT¹`
/// to `LPT³`), in any case, also before an extension (`nul.json`) or
/// trailing spaces.
pub fn is_dos_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end();
    let up = stem.to_ascii_uppercase();
    if matches!(
        up.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    ["COM", "LPT"].iter().any(|p| {
        up.strip_prefix(p).is_some_and(|rest| {
            let mut c = rest.chars();
            matches!((c.next(), c.next()), (Some(d), None) if d.is_ascii_digit() || matches!(d, '¹' | '²' | '³'))
        })
    })
}

/// A relative path as compared for sameness: `/` and `\` alike, empty and
/// `.` parts dropped, case folded (NTFS names ignore case).
pub fn normalized_relative(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("/")
}

/// 64 hex digits.
pub fn is_sha256(s: &str) -> bool {
    let s = s.trim();
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
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

/// Checks that an engine `name` is one plain folder name, since it becomes
/// `engine\<name>` (and `engine\downloads\<name>.zip`) and
/// `manifests\engine.json` in the data root can be edited: the rules of
/// one part of a manifest path (`plain_name_problem`: not empty, `.` or
/// `..`, no separator, drive or `:`, none of `<>"|?*` or control
/// characters, no trailing dot or space, not a device name such as `CON`,
/// `NUL`, `COM0` to `COM9`, also with an extension).
pub fn check_engine_name(name: &str) -> Result<()> {
    match plain_name_problem(name) {
        None => Ok(()),
        Some(why) => Err(CoreError::Models(format!(
            "manifests/engine.json: the engine name {name:?} {why}; it becomes a folder name \
             under engine\\, so it must be one plain name such as \
             nemo-speech-0.2.0-windows-x86_64-cuda"
        ))),
    }
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
    /// an entry with a known name replaces it, a new one is added. Every
    /// entry is fetched from its `url` by Diarist's own downloader; user
    /// entries are `direct` (they land in `models\user`) unless they say
    /// `"direct": false` (then `models\<repo>\<revision>`). Every entry is
    /// checked ([`ModelEntry::validate`]); one that fails refuses the list,
    /// naming the entry and the field.
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
            for e in &m.engines {
                check_engine_name(&e.name)?;
            }
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
        // optional pick for other or mixed languages. The punctuation model
        // is fetched on first run too (plan 16 section 3.13.7; the owner may
        // veto `required` here without a code change).
        assert_eq!(
            required,
            ["nemotron-3-diarization", "nemotron-en", PUNCTUATION_MODEL]
        );
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
        for e in Manifest::builtin()
            .models
            .iter()
            .filter(|e| e.role != ModelRole::Punctuation)
        {
            let json = serde_json::to_string(e).unwrap();
            for key in ["\"files\":", "\"dir\":", "\"punctuation\":"] {
                assert!(!json.contains(key), "{}: {key}", e.name);
            }
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
            // The same file twice, the primary again after "./" and "//",
            // a device name, an extra file without its hash.
            (
                r#""path": "diarist-onnxruntime.dll""#,
                r#""path": "Tokenizer.JSON""#,
                "files[1].path",
            ),
            (
                r#""path": "tokenizer.json""#,
                r#""path": "./onnx//model_quantized.onnx""#,
                "files[0].path",
            ),
            (
                r#""path": "tokenizer.json""#,
                r#""path": "onnx/NUL.json""#,
                "files[0].path",
            ),
            (
                r#""sha256": "2222222222222222222222222222222222222222222222222222222222222222""#,
                r#""sha256": """#,
                "files[0].sha256",
            ),
            (
                r#""sha256": "1111111111111111111111111111111111111111111111111111111111111111""#,
                r#""sha256": "1111""#,
                "sha256",
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

    /// The shipped punctuation model's manifest name (plan 16 section 3.13.7).
    const PUNCTUATION_MODEL: &str = "punctuation-horizon-small";

    /// Candidate A as plan 16 section 3.13.7 fixes it: the upstream int8
    /// file and tokenizer from Hugging Face at the pinned revision, the
    /// licence texts and the renamed ONNX Runtime 1.31.0 DLL from the
    /// project's models release, and the description block.
    #[test]
    fn the_punctuation_entry_is_candidate_a_with_every_file() {
        use crate::punct::spec::{
            HeadKind, HeadPosition, HeadValues, InputCasing, MarkLabel, TokenizerKind, RUNTIME_DLL,
        };
        use crate::punct::{CasingKind, Mark, MarkPolicy, PunctUnit};

        let m = Manifest::builtin();
        let e = m.model(PUNCTUATION_MODEL).unwrap();
        assert_eq!(e.role, ModelRole::Punctuation);
        assert!(e.required && e.direct && !e.experimental && !e.streaming);
        assert_eq!(e.pull_name, "");
        assert_eq!(e.repo, "Horizon-Labs/punctuation-restoration-small");
        let rev = "ee31fdfc5304f876ea3b0dd72d30eda7e940b2b1";
        assert_eq!(e.revision, rev);
        let hf = format!("https://huggingface.co/{}/resolve/{rev}/", e.repo);
        assert_eq!(e.file, "onnx/model_quantized.onnx");
        assert_eq!(e.url, format!("{hf}onnx/model_quantized.onnx"));
        assert_eq!(
            e.sha256,
            "5679870be92c7fa47c9be9b2872fac389ef28d43d2c33f35e1315b93a82f4ace"
        );
        assert_eq!(e.primary_size_bytes(), 268_414_381);
        assert!(e.licence.starts_with("Apache-2.0") && e.licence.ends_with("base model MIT"));
        assert_eq!(
            e.dir.as_deref(),
            Some("horizon-labs/punctuation-restoration-small")
        );
        assert_eq!(
            e.dest_dir(Path::new("M")),
            Path::new("M")
                .join("horizon-labs")
                .join("punctuation-restoration-small")
        );

        // Every other file, in download order, with its size and SHA-256.
        let release = "https://github.com/ebt55/diarist/releases/download/models-2026-10/";
        let files: Vec<(&str, u64, &str)> = e
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.size_bytes, f.sha256.as_str()))
            .collect();
        assert_eq!(
            files,
            [
                (
                    "tokenizer.json",
                    34_363_286,
                    "f34b44a6dc8a0eef651bfa10e8325057530152606a69d9c3b6ae02efb1558f0a"
                ),
                (
                    "LICENSE-Apache-2.0.txt",
                    11_358,
                    "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"
                ),
                (
                    "NOTICE-punctuation.txt",
                    2_179,
                    "f9d70a3fe979fefacbb384d3bdec12fbdf14cfa11f0adbe6f7ac3d9a615ded92"
                ),
                (
                    "onnxruntime-LICENSE.txt",
                    1_094,
                    "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674"
                ),
                (
                    "onnxruntime-ThirdPartyNotices.txt",
                    344_457,
                    "c53a76501ef60db6f865f20599f220761201ac4057683acefdab37d861b86622"
                ),
                (
                    RUNTIME_DLL,
                    17_253_176,
                    "0c42dc4e278f0968fdf9ebf1b1e54a74c657a9942e70803e453c91993451cf4c"
                ),
            ]
        );
        for f in &e.files {
            let host = if f.path == "tokenizer.json" {
                hf.as_str()
            } else {
                release
            };
            assert_eq!(f.url, format!("{host}{}", f.path), "{}", f.path);
        }
        assert_eq!(
            e.size_bytes,
            268_414_381 + e.files.iter().map(|f| f.size_bytes).sum::<u64>()
        );
        assert_eq!(e.size_bytes, 320_389_931);
        // The scanner matches the primary file by its own size.
        let matched = m.match_file("model_quantized.onnx", 268_414_381).unwrap();
        assert_eq!(matched.name, PUNCTUATION_MODEL);

        // Every offered language but Thai, by primary subtag (nb and nn are
        // the card's `no`).
        let mut offered: Vec<String> = crate::models::languages::LanguageTable::builtin()
            .entries
            .iter()
            .map(|l| crate::models::languages::primary_subtag(&l.code))
            .filter(|l| l != "th")
            .collect();
        offered.sort();
        offered.dedup();
        let mut langs = e.languages.clone();
        langs.sort();
        assert_eq!(langs, offered);
        assert_eq!(e.languages.len(), 35);

        // The description block.
        let s = e.punctuation.as_ref().unwrap();
        s.validate().unwrap();
        assert_eq!(s.tokenizer, "tokenizer.json");
        assert_eq!(s.tokenizer_kind, TokenizerKind::HfJson);
        assert_eq!(s.input_casing, InputCasing::Lower);
        assert_eq!((s.max_tokens, s.context(), s.batch_size()), (512, 48, 8));
        assert_eq!((s.bos_id, s.eos_id, s.pad_id), (Some(2), Some(1), 0));
        assert_eq!(s.inputs.ids, "input_ids");
        assert_eq!(s.inputs.mask.as_deref(), Some("attention_mask"));
        assert_eq!(s.heads.len(), 1);
        let h = &s.heads[0];
        assert_eq!(
            (h.output.as_str(), h.kind, h.position, h.values),
            (
                "logits",
                HeadKind::Post,
                HeadPosition::LastToken,
                HeadValues::Logits
            )
        );
        assert_eq!(h.labels, ["none", ".", ",", "?", "-", ":"]);
        let labels: Vec<MarkLabel> = h
            .labels
            .iter()
            .map(|l| MarkLabel::parse(l).unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                MarkLabel::None,
                MarkLabel::Mark(Mark::Period),
                MarkLabel::Mark(Mark::Comma),
                MarkLabel::Mark(Mark::Question),
                MarkLabel::Mark(Mark::Dash),
                MarkLabel::Mark(Mark::Colon),
            ]
        );
        // No casing head: the sentence-start rule only.
        assert_eq!(s.casing, CasingKind::None);
        assert_eq!(s.unit, Some(PunctUnit::Turn));
        assert_eq!(s.policy, Some(MarkPolicy::KeepAsr));
        assert_eq!(s.runtime_file(), RUNTIME_DLL);
        assert_eq!(s.runtime_version.as_deref(), Some("1.31.0"));
        // `.` `,` `?` everywhere; no comma for hi, ja and zh; `:` and `-` never.
        let pcq = vec![Mark::Period, Mark::Comma, Mark::Question];
        let pq = vec![Mark::Period, Mark::Question];
        assert_eq!(
            s.marks_by_language,
            std::collections::BTreeMap::from([
                ("*".to_owned(), pcq),
                ("hi".to_owned(), pq.clone()),
                ("ja".to_owned(), pq.clone()),
                ("zh".to_owned(), pq),
            ])
        );
    }

    /// `repo`, `revision`, `file` and `url` are checked when the list is
    /// read: the folder `dest_dir` builds from them never leaves `models\`,
    /// and an entry without an address fails naming the entry.
    #[test]
    fn entries_land_inside_the_models_folder_and_have_an_address() {
        const PULLED: &str = r#"[{"name": "mine", "role": "asr", "direct": false,
            "repo": "owner/model", "revision": "abc123", "file": "m.gguf",
            "url": "https://example.invalid/m.gguf", "size_bytes": 10,
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}]"#;
        let dir = tempfile::tempdir().unwrap();
        let load = |json: &str| {
            std::fs::write(dir.path().join(USER_MODELS), json).unwrap();
            Manifest::load(dir.path())
        };
        let m = load(PULLED).unwrap();
        let e = m.model("mine").unwrap();
        assert_eq!(
            e.dest_dir(Path::new("M")),
            Path::new("M").join("owner").join("model").join("abc123")
        );
        for (from, to, field) in [
            (r#""repo": "owner/model""#, r#""repo": "../model""#, "repo"),
            (
                r#""repo": "owner/model""#,
                r#""repo": "owner/../x""#,
                "repo",
            ),
            (r#""repo": "owner/model""#, r#""repo": "C:\\owner""#, "repo"),
            (
                r#""repo": "owner/model""#,
                r#""repo": "\\\\server\\share""#,
                "repo",
            ),
            (r#""repo": "owner/model""#, r#""repo": "owner""#, "repo"),
            (r#""repo": "owner/model""#, r#""repo": "a/b/c""#, "repo"),
            (r#""repo": "owner/model""#, r#""repo": "owner/CON""#, "repo"),
            (
                r#""repo": "owner/model""#,
                r#""repo": "owner./model""#,
                "repo",
            ),
            (r#""repo": "owner/model""#, r#""repo": "", "x": """#, "repo"),
            (r#""revision": "abc123""#, r#""revision": "..""#, "revision"),
            (
                r#""revision": "abc123""#,
                r#""revision": "/abs""#,
                "revision",
            ),
            (
                r#""revision": "abc123""#,
                r#""revision": "a:b""#,
                "revision",
            ),
            (
                r#""revision": "abc123""#,
                r#""revision": "nul""#,
                "revision",
            ),
            (
                r#""revision": "abc123""#,
                r#""revision": "abc ""#,
                "revision",
            ),
            (
                r#""file": "m.gguf""#,
                r#""file": "../../evil.gguf""#,
                "file",
            ),
            (r#""file": "m.gguf""#, r#""file": "C:\\evil.gguf""#, "file"),
            (r#""file": "m.gguf""#, r#""file": "aux.gguf""#, "file"),
            (r#""file": "m.gguf""#, r#""file": "m.gguf.""#, "file"),
            (
                r#""url": "https://example.invalid/m.gguf""#,
                r#""url": """#,
                "url",
            ),
            (
                r#""url": "https://example.invalid/m.gguf""#,
                r#""url": "file:///C:/m.gguf""#,
                "url",
            ),
            (
                r#""url": "https://example.invalid/m.gguf""#,
                r#""url": "https://""#,
                "url",
            ),
        ] {
            assert!(PULLED.contains(from), "{from}");
            let err = load(&PULLED.replace(from, to)).unwrap_err().to_string();
            assert!(
                err.contains(&format!("{USER_MODELS}: mine: {field}")),
                "{to}: {err}"
            );
        }
        // Nested revisions and files are fine; a direct entry needs no repo.
        load(&PULLED.replace(r#""revision": "abc123""#, r#""revision": "refs/pr/3""#)).unwrap();
        load(&PULLED.replace(r#""file": "m.gguf""#, r#""file": "onnx/m.gguf""#)).unwrap();
        let mut direct: Vec<serde_json::Value> = serde_json::from_str(PULLED).unwrap();
        direct[0]["direct"] = serde_json::Value::Bool(true);
        let o = direct[0].as_object_mut().unwrap();
        o.remove("repo");
        o.remove("revision");
        let m = load(&serde_json::to_string(&direct).unwrap()).unwrap();
        assert_eq!(
            m.model("mine").unwrap().dest_dir(Path::new("M")),
            Path::new("M").join("user")
        );
        // An extra file without an address fails at load, naming it.
        let mut multi: Vec<serde_json::Value> = serde_json::from_str(PUNCT_ENTRY).unwrap();
        multi[0]["files"][1]["url"] = serde_json::Value::String(String::new());
        let err = load(&serde_json::to_string(&multi).unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("punct-small: files[1].url: empty"), "{err}");
        // The shipped list passes.
        for e in &Manifest::builtin().models {
            e.validate().unwrap();
        }
        assert!(check_url("u", "http://127.0.0.1:8080/x").is_ok());
    }

    #[test]
    fn relative_paths_refuse_what_windows_would_not_keep() {
        for ok in [
            "tokenizer.json",
            "onnx/model_quantized.onnx",
            r"onnx\model.onnx",
            "./tokenizer.json",
            "console.json",
            "com10.dll",
            "lpt.txt",
            "nulls/x.json",
            ".hidden",
        ] {
            check_relative_path("f", ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "",
            "  ",
            ".",
            "./",
            "../x",
            "a/../../x",
            "/x",
            r"\x",
            "C:x",
            r"C:\x",
            r"\\?\C:\x",
            r"\\server\share\x",
            "CON",
            "nul.json",
            "Aux",
            "prn.tar.gz",
            "com1",
            "LPT9.dll",
            "com0",
            "COM¹",
            "nul .txt",
            "onnx/con/model.onnx",
            "model.",
            "model ",
            "onnx./model.onnx",
            "a?b",
            "a*b",
            "a|b",
            "a<b",
            "a\"b",
            "a\tb",
            // Control characters past ASCII 31 (DEL, C1), as for engine names.
            "a\u{7f}b",
            "onnx/a\u{9b}b.onnx",
        ] {
            let e = check_relative_path("files[0].path", bad).unwrap_err();
            assert!(e.starts_with("files[0].path:"), "{bad:?}: {e}");
        }
        assert!(is_dos_device("CONOUT$"));
        assert!(!is_dos_device("COM"));
        assert_eq!(
            normalized_relative(r"./Onnx//\model.onnx"),
            "onnx/model.onnx"
        );
        assert!(is_sha256(&"a".repeat(64)) && !is_sha256(&"g".repeat(64)) && !is_sha256(""));
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

    #[test]
    fn engine_names_are_one_plain_folder_name() {
        for e in Manifest::builtin().engines {
            check_engine_name(&e.name).unwrap();
        }
        check_engine_name("nemo-speech-0.3.0-rc.1-windows-x86_64-cuda").unwrap();
        check_engine_name("console").unwrap();
        for bad in [
            "",
            ".",
            "..",
            r"..\..\Windows",
            "../x",
            "a/b",
            r"a\b",
            "C:x",
            r"C:\engine",
            r"\\server\share",
            "x<y",
            "x|y",
            "x?",
            "x*",
            "x\"y",
            "x\ty",
            "x.",
            "x ",
            "CON",
            "nul",
            "Aux.zip",
            "com1",
            "LPT9.txt",
            "CONOUT$",
            // The rules of a manifest path part apply too (one helper).
            "com0",
            "LPT0.zip",
            "COM¹",
            "nul .zip",
            "x\u{7f}y",
        ] {
            assert!(check_engine_name(bad).is_err(), "{bad:?}");
        }
        // Each rule gives the same reason for a path part and an engine name.
        for (bad, why) in [
            ("CON", "device name"),
            ("x.", "dot or a space"),
            ("x?", "does not allow"),
        ] {
            let path = check_relative_path("f", bad).unwrap_err();
            let engine = check_engine_name(bad).unwrap_err().to_string();
            assert!(
                path.contains(why) && engine.contains(why),
                "{path} / {engine}"
            );
        }
        // An edited `manifests\engine.json` with such a name does not load.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("manifests")).unwrap();
        let mut entry = serde_json::to_value(&Manifest::builtin().engines[0]).unwrap();
        entry["name"] = r"..\..\escape".into();
        std::fs::write(
            dir.path().join("manifests/engine.json"),
            serde_json::to_string(&[entry]).unwrap(),
        )
        .unwrap();
        let err = Manifest::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("one plain name"), "{err}");
    }
}
