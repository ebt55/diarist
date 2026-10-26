//! The description block of a punctuation model (docs/plans/16 section 3.6):
//! what the generic ONNX runner needs to know about a token classifier it
//! has never seen. The same [`PunctSpec`] comes from two places: the
//! manifest entry of a shipped model (`ModelEntry.punctuation`), which the
//! scanner copies into the matched file's capabilities, and the
//! `<file>.meet.json` sidecar of a model a user drops in (its `punctuation`
//! key). docs/04-model-guide.md section 5 documents the schema, and
//! `manifests/punctuation-sidecar.example.json` is a complete example.
//!
//! Paths in the block are relative to the model's folder: the nearest folder,
//! from the `.onnx` file's own folder upwards (at most
//! [`MAX_ROOT_LEVELS`] levels, never above `models\`), that holds the
//! tokenizer. That covers both a flat folder and the Hugging Face layout
//! (`onnx/model_quantized.onnx` with `tokenizer.json` at the root) without
//! a `..` in any path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Case, CasingKind, Mark, MarkPolicy, PunctUnit};
use crate::models::manifest::check_relative_path;

/// The ONNX Runtime library's file name inside a model folder: renamed from
/// `onnxruntime.dll`, so the Windows loader never resolves an import of
/// that name to this copy, nor this load to another copy (plan 16 section 3.7).
pub const RUNTIME_DLL: &str = "diarist-onnxruntime.dll";

/// How far above the `.onnx` file the model's folder may be.
pub const MAX_ROOT_LEVELS: usize = 3;

/// Windows a stream is cut into when the block names no batch size.
pub const DEFAULT_BATCH: u32 = 8;

/// What a punctuation model is, for the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PunctSpec {
    /// The tokenizer file, relative to the model's folder.
    pub tokenizer: String,
    #[serde(default)]
    pub tokenizer_kind: TokenizerKind,
    /// Whether the words are lower-cased before tokenizing (models trained
    /// on lower-case text).
    #[serde(default)]
    pub input_casing: InputCasing,
    /// Tokens per window, the special tokens included.
    pub max_tokens: u32,
    /// Tokens of context on each side of a window, whose labels are
    /// discarded. Default: an eighth of `max_tokens`, at most 64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context_tokens: Option<u32>,
    /// Windows per inference call (short streams are batched with padding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub batch: Option<u32>,
    /// Token put before every window (`<bos>`, `<s>`, `[CLS]`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bos_id: Option<i64>,
    /// Token put after every window (`<eos>`, `</s>`, `[SEP]`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub eos_id: Option<i64>,
    /// Token that pads the shorter windows of a batch.
    #[serde(default)]
    pub pad_id: i64,
    /// Names of the model's input tensors.
    #[serde(default)]
    pub inputs: SpecInputs,
    /// The output tensors the runner reads.
    pub heads: Vec<PunctHead>,
    /// What casing the model predicts (`none`: the sentence-start rule only).
    #[serde(default)]
    pub casing: CasingKind,
    /// Marks allowed per language (primary subtag, or `*`), replacing the
    /// default allow-list (plan 16 section 3.4).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marks_by_language: BTreeMap<String, Vec<Mark>>,
    /// Stream (default) or turn windows (plan 16 section 3.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unit: Option<PunctUnit>,
    /// What happens to the ASR's own marks (plan 16 section 3.3 rule 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policy: Option<MarkPolicy>,
    /// The ONNX Runtime library, relative to the model's folder (default
    /// [`RUNTIME_DLL`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runtime: Option<String>,
    /// The loaded library's version must start with this (`1.31.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runtime_version: Option<String>,
}

/// The tokenizer's file format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "PunctTokenizerKind")]
pub enum TokenizerKind {
    /// A Hugging Face `tokenizer.json` (a SentencePiece `sp.model` is
    /// converted into one first).
    #[default]
    HfJson,
}

/// Whether the words are lower-cased for the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "PunctInputCasing")]
pub enum InputCasing {
    /// The ASR's casing, as is.
    #[default]
    Keep,
    Lower,
}

/// Input tensor names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "PunctInputs")]
pub struct SpecInputs {
    /// Token ids, `int64 [batch, tokens]`.
    pub ids: String,
    /// Attention mask (`int64`, 1 for a token, 0 for padding), if the model
    /// takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mask: Option<String>,
}

impl Default for SpecInputs {
    fn default() -> Self {
        Self {
            ids: "input_ids".into(),
            mask: None,
        }
    }
}

/// One output tensor of the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PunctHead {
    /// Output tensor name.
    pub output: String,
    pub kind: HeadKind,
    /// The label of each class, in the output's order: a mark (`.` `,` `?`
    /// `!` `:` `-` `¿` `¡`, their script forms `。` `，` `、` `।` `،` `؟`
    /// `？` `！`, or a name: `period`, `comma`, `question`, ...), nothing
    /// (`""`, `0`, `O`, `<NULL>` or any other `<...>` token), `<ACRONYM>`
    /// (never applied); for a `case` head `lower`, `cap` or `upper`. Empty
    /// for a per-character `case` head and a `boundary` head.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Which subword of a word carries the word's label.
    pub position: HeadPosition,
    pub values: HeadValues,
}

/// What a head predicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "PunctHeadKind")]
pub enum HeadKind {
    /// A mark after the word.
    Post,
    /// A mark before the word (`¿` `¡`).
    Pre,
    /// Casing: per word (labels) or per character (`[batch, tokens, chars]`).
    Case,
    /// Sentence boundaries; read by nothing (the post head's marks are what
    /// segmentation uses), allowed so a model's full description is valid.
    Boundary,
}

/// Which subword of a word carries the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "PunctHeadPosition")]
pub enum HeadPosition {
    /// The word's last token (A: one label per word).
    LastToken,
    /// The word's first token (pre-marks).
    FirstToken,
    /// Every subword has a label (B, C): the word's mark is its last
    /// subword's, a mark predicted on an inner subword is dropped and
    /// counted; per-character casing covers every subword.
    PerSubword,
}

/// What the output holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "PunctHeadValues")]
pub enum HeadValues {
    /// Scores per class, `float [batch, tokens, labels]`: the highest wins.
    Logits,
    /// Class ids, `int64`, `int32` or `bool` `[batch, tokens]` (or
    /// `[batch, tokens, chars]` for per-character casing).
    Decisions,
}

/// What one class of a `post` or `pre` head means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkLabel {
    None,
    Mark(Mark),
    /// `<ACRONYM>` (a period after every character): never applied.
    Acronym,
}

impl MarkLabel {
    /// Reads one label; `Err` for one that is neither a known mark nor
    /// "nothing".
    pub fn parse(label: &str) -> Result<MarkLabel, String> {
        let l = label.trim();
        if l.eq_ignore_ascii_case("<acronym>") || l.eq_ignore_ascii_case("acronym") {
            return Ok(MarkLabel::Acronym);
        }
        if is_nothing(l) {
            return Ok(MarkLabel::None);
        }
        if l == ";" {
            // The Greek question mark looks like a semicolon, and some
            // models label it so (B); others mean a semicolon. Say which.
            return Err(format!(
                "{l:?} is ambiguous: write \"question\" for the Greek question mark, or \"\" to ignore it"
            ));
        }
        Mark::from_label(l)
            .map(MarkLabel::Mark)
            .ok_or_else(|| format!("unknown label {l:?} (write \"\" to ignore it)"))
    }
}

/// What one class of a per-word `case` head means.
pub fn parse_case_label(label: &str) -> Result<Case, String> {
    let l = label.trim().to_ascii_lowercase();
    match l.as_str() {
        "lower" | "keep" | "none" | "o" | "0" | "" | "<null>" => Ok(Case::Keep),
        "cap" | "first" | "title" | "capitalize" | "capitalise" => Ok(Case::First),
        "upper" | "all_caps" | "allcaps" => Ok(Case::Upper),
        _ => Err(format!(
            "unknown casing label {label:?} (lower, cap or upper)"
        )),
    }
}

/// `""`, `0`, `O`, `none`, `null`, `<NULL>`, `<PAD>` and the like.
fn is_nothing(l: &str) -> bool {
    l.is_empty()
        || matches!(
            l.to_ascii_lowercase().as_str(),
            "0" | "o" | "none" | "null" | "nothing"
        )
        || (l.len() > 2 && l.starts_with('<') && l.ends_with('>'))
}

impl PunctSpec {
    /// Parses and validates a description block (JSON text).
    pub fn from_json(text: &str) -> Result<PunctSpec, String> {
        let spec: PunctSpec = serde_json::from_str(text).map_err(|e| e.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    /// Context tokens on each side of a window.
    pub fn context(&self) -> u32 {
        self.context_tokens
            .unwrap_or_else(|| (self.max_tokens / 8).min(64))
    }

    /// Windows per inference call.
    pub fn batch_size(&self) -> u32 {
        self.batch.unwrap_or(DEFAULT_BATCH).max(1)
    }

    /// Special tokens per window (BOS and EOS).
    pub fn specials(&self) -> u32 {
        u32::from(self.bos_id.is_some()) + u32::from(self.eos_id.is_some())
    }

    /// The ONNX Runtime library's relative path.
    pub fn runtime_file(&self) -> &str {
        self.runtime.as_deref().unwrap_or(RUNTIME_DLL)
    }

    /// The first head of `kind`.
    pub fn head(&self, kind: HeadKind) -> Option<&PunctHead> {
        self.heads.iter().find(|h| h.kind == kind)
    }

    /// Checks everything that does not need the files: every error names
    /// its field (`heads[0].labels[3]: ...`).
    pub fn validate(&self) -> Result<(), String> {
        check_relative_path("tokenizer", &self.tokenizer)?;
        if let Some(r) = &self.runtime {
            check_relative_path("runtime", r)?;
        }
        if self.max_tokens < 16 {
            return Err(format!(
                "max_tokens: {} is too small (at least 16)",
                self.max_tokens
            ));
        }
        let room =
            i64::from(self.max_tokens) - i64::from(self.specials()) - 2 * i64::from(self.context());
        if room < 8 {
            return Err(format!(
                "context_tokens: {} on each side leaves {room} tokens of {} for labels (at least 8)",
                self.context(),
                self.max_tokens
            ));
        }
        if self.inputs.ids.trim().is_empty() {
            return Err("inputs.ids: empty".into());
        }
        if self.heads.is_empty() {
            return Err("heads: no head (at least a `post` head)".into());
        }
        if !self
            .heads
            .iter()
            .any(|h| matches!(h.kind, HeadKind::Post | HeadKind::Case))
        {
            return Err("heads: neither a `post` nor a `case` head".into());
        }
        for (i, h) in self.heads.iter().enumerate() {
            let field = format!("heads[{i}]");
            if h.output.trim().is_empty() {
                return Err(format!("{field}.output: empty"));
            }
            if self.heads[..i].iter().any(|o| o.kind == h.kind) {
                return Err(format!("{field}.kind: a second {:?} head", h.kind));
            }
            match h.kind {
                HeadKind::Post | HeadKind::Pre => {
                    if h.labels.is_empty() {
                        return Err(format!("{field}.labels: empty"));
                    }
                    for (j, l) in h.labels.iter().enumerate() {
                        let m =
                            MarkLabel::parse(l).map_err(|e| format!("{field}.labels[{j}]: {e}"))?;
                        if let MarkLabel::Mark(m) = m {
                            if m.is_pre() != (h.kind == HeadKind::Pre) {
                                return Err(format!(
                                    "{field}.labels[{j}]: {l:?} cannot go {} a word",
                                    if m.is_pre() { "after" } else { "before" }
                                ));
                            }
                        }
                    }
                    if h.values == HeadValues::Logits && h.labels.len() < 2 {
                        return Err(format!("{field}.labels: logits need two classes or more"));
                    }
                }
                HeadKind::Case => {
                    for (j, l) in h.labels.iter().enumerate() {
                        parse_case_label(l).map_err(|e| format!("{field}.labels[{j}]: {e}"))?;
                    }
                    if h.values == HeadValues::Logits && h.labels.len() < 2 {
                        return Err(format!(
                            "{field}.labels: a casing head with logits needs its classes"
                        ));
                    }
                }
                HeadKind::Boundary => {}
            }
        }
        for lang in self.marks_by_language.keys() {
            if lang.trim().is_empty() {
                return Err("marks_by_language: an empty language".into());
            }
        }
        Ok(())
    }

    /// The model's folder for the `.onnx` file at `model`: the nearest
    /// folder from the file's own upwards (at most [`MAX_ROOT_LEVELS`],
    /// never above `models_dir`) that holds the tokenizer. `Err` names what
    /// is missing.
    pub fn model_root(&self, model: &Path, models_dir: &Path) -> Result<PathBuf, String> {
        let mut dir = model.parent();
        for _ in 0..=MAX_ROOT_LEVELS {
            let Some(d) = dir else { break };
            if rel_path(d, &self.tokenizer).is_file() {
                return Ok(d.to_path_buf());
            }
            if same_dir(d, models_dir) {
                break;
            }
            dir = d.parent();
        }
        Err(format!(
            "the tokenizer {} is not in the model's folder (next to {} or up to {MAX_ROOT_LEVELS} folders above it)",
            self.tokenizer,
            model
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ))
    }
}

/// `root` joined with a validated relative path (`/` or `\` separated).
pub fn rel_path(root: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .fold(root.to_path_buf(), |acc, p| acc.join(p))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Candidate A's block (Horizon-Labs/punctuation-restoration-small).
    pub(crate) const A_SPEC: &str = r#"{
        "tokenizer": "tokenizer.json",
        "tokenizer_kind": "hf_json",
        "input_casing": "keep",
        "max_tokens": 512,
        "context_tokens": 48,
        "batch": 8,
        "bos_id": 2,
        "eos_id": 1,
        "pad_id": 0,
        "inputs": {"ids": "input_ids", "mask": "attention_mask"},
        "heads": [
            {"output": "logits", "kind": "post", "labels": ["", ".", ",", "?", "-", ":"],
             "position": "last_token", "values": "logits"}
        ],
        "casing": "none",
        "marks_by_language": {"*": ["period", "comma", "question"]},
        "unit": "stream",
        "policy": "keep_asr",
        "runtime": "diarist-onnxruntime.dll",
        "runtime_version": "1.31.0"
    }"#;

    /// B's shape: four decision heads, per-character casing.
    const B_SPEC: &str = r#"{
        "tokenizer": "tokenizer.json",
        "input_casing": "lower",
        "max_tokens": 256,
        "bos_id": 0, "eos_id": 2, "pad_id": 1,
        "inputs": {"ids": "input_ids"},
        "heads": [
            {"output": "pre_preds", "kind": "pre", "labels": ["<NULL>", "¿"], "position": "first_token", "values": "decisions"},
            {"output": "post_preds", "kind": "post",
             "labels": ["<NULL>", "<ACRONYM>", ".", ",", "?", "？", "，", "。", "、", "", "।", "؟", "،", "question", "", "", ""],
             "position": "per_subword", "values": "decisions"},
            {"output": "cap_preds", "kind": "case", "position": "per_subword", "values": "decisions"},
            {"output": "seg_preds", "kind": "boundary", "position": "per_subword", "values": "decisions"}
        ],
        "casing": "per_char"
    }"#;

    #[test]
    fn parses_every_field() {
        let a = PunctSpec::from_json(A_SPEC).unwrap();
        assert_eq!(a.tokenizer, "tokenizer.json");
        assert_eq!(a.tokenizer_kind, TokenizerKind::HfJson);
        assert_eq!(a.input_casing, InputCasing::Keep);
        assert_eq!((a.max_tokens, a.context(), a.batch_size()), (512, 48, 8));
        assert_eq!(
            (a.bos_id, a.eos_id, a.pad_id, a.specials()),
            (Some(2), Some(1), 0, 2)
        );
        assert_eq!(a.inputs.ids, "input_ids");
        assert_eq!(a.inputs.mask.as_deref(), Some("attention_mask"));
        let post = a.head(HeadKind::Post).unwrap();
        assert_eq!(
            (post.output.as_str(), post.position, post.values),
            ("logits", HeadPosition::LastToken, HeadValues::Logits)
        );
        let marks: Vec<MarkLabel> = post
            .labels
            .iter()
            .map(|l| MarkLabel::parse(l).unwrap())
            .collect();
        assert_eq!(
            marks,
            [
                MarkLabel::None,
                MarkLabel::Mark(Mark::Period),
                MarkLabel::Mark(Mark::Comma),
                MarkLabel::Mark(Mark::Question),
                MarkLabel::Mark(Mark::Dash),
                MarkLabel::Mark(Mark::Colon),
            ]
        );
        assert_eq!(a.casing, CasingKind::None);
        assert_eq!(
            a.marks_by_language["*"],
            [Mark::Period, Mark::Comma, Mark::Question]
        );
        assert_eq!(a.unit, Some(PunctUnit::Stream));
        assert_eq!(a.policy, Some(MarkPolicy::KeepAsr));
        assert_eq!(a.runtime_file(), RUNTIME_DLL);
        assert_eq!(a.runtime_version.as_deref(), Some("1.31.0"));
        // Round trip.
        let back = PunctSpec::from_json(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);

        let b = PunctSpec::from_json(B_SPEC).unwrap();
        assert_eq!(b.input_casing, InputCasing::Lower);
        assert_eq!(b.context(), 32, "an eighth of 256");
        assert_eq!(b.batch_size(), DEFAULT_BATCH);
        assert_eq!(b.inputs.mask, None);
        assert_eq!(b.casing, CasingKind::PerChar);
        assert_eq!(b.head(HeadKind::Case).unwrap().labels.len(), 0);
        let post = b.head(HeadKind::Post).unwrap();
        assert_eq!(MarkLabel::parse(&post.labels[1]), Ok(MarkLabel::Acronym));
        assert_eq!(
            MarkLabel::parse(&post.labels[7]),
            Ok(MarkLabel::Mark(Mark::Period))
        );
        assert_eq!(
            MarkLabel::parse(&post.labels[13]),
            Ok(MarkLabel::Mark(Mark::Question))
        );
        assert_eq!(b.runtime_file(), RUNTIME_DLL);
        // Defaults stay out of the JSON.
        let json = serde_json::to_string(&b).unwrap();
        assert!(
            !json.contains("context_tokens") && !json.contains("runtime"),
            "{json}"
        );
    }

    fn err(json: &str) -> String {
        PunctSpec::from_json(json).unwrap_err()
    }

    #[test]
    fn errors_name_the_field() {
        let without_heads = r#"{"tokenizer": "tokenizer.json", "max_tokens": 512}"#;
        assert!(
            err(without_heads).contains("heads"),
            "{}",
            err(without_heads)
        );
        let empty_heads = r#"{"tokenizer": "tokenizer.json", "max_tokens": 512, "heads": []}"#;
        assert!(
            err(empty_heads).starts_with("heads:"),
            "{}",
            err(empty_heads)
        );
        let up = A_SPEC.replace(
            r#""tokenizer": "tokenizer.json""#,
            r#""tokenizer": "../tokenizer.json""#,
        );
        assert!(err(&up).starts_with("tokenizer:"), "{}", err(&up));
        let abs = A_SPEC.replace(
            r#""runtime": "diarist-onnxruntime.dll""#,
            r#""runtime": "C:\\Windows\\System32\\onnxruntime.dll""#,
        );
        assert!(err(&abs).starts_with("runtime:"), "{}", err(&abs));
        let bad_label = A_SPEC.replace(r#""-", ":""#, r#""-", "~""#);
        assert!(
            err(&bad_label).starts_with("heads[0].labels[5]:"),
            "{}",
            err(&bad_label)
        );
        let semicolon = A_SPEC.replace(r#""-", ":""#, r#""-", ";""#);
        assert!(err(&semicolon).contains("ambiguous"), "{}", err(&semicolon));
        let pre_after = A_SPEC.replace(r#""-", ":""#, r#""-", "¿""#);
        assert!(
            err(&pre_after).contains("cannot go after"),
            "{}",
            err(&pre_after)
        );
        let tiny = A_SPEC.replace(r#""context_tokens": 48"#, r#""context_tokens": 252"#);
        assert!(err(&tiny).starts_with("context_tokens:"), "{}", err(&tiny));
        let two = A_SPEC.replace(
            r#""casing": "none""#,
            r#""casing": "none", "inputs": {"ids": ""}"#,
        );
        assert!(err(&two).contains("inputs"), "{}", err(&two));
        let boundary_only = r#"{"tokenizer": "t.json", "max_tokens": 64,
            "heads": [{"output": "seg", "kind": "boundary", "position": "per_subword", "values": "decisions"}]}"#;
        assert!(err(boundary_only).starts_with("heads:"));
        let twice = B_SPEC.replace(r#""kind": "boundary""#, r#""kind": "pre""#);
        assert!(err(&twice).starts_with("heads[3].kind:"), "{}", err(&twice));
        let case_label = B_SPEC.replace(
            r#""kind": "case", "position""#,
            r#""kind": "case", "labels": ["lower", "shout"], "position""#,
        );
        assert!(
            err(&case_label).starts_with("heads[2].labels[1]:"),
            "{}",
            err(&case_label)
        );
    }

    #[test]
    fn case_labels() {
        assert_eq!(parse_case_label("LOWER"), Ok(Case::Keep));
        assert_eq!(parse_case_label("Cap"), Ok(Case::First));
        assert_eq!(parse_case_label("UPPER"), Ok(Case::Upper));
        assert!(parse_case_label("mixed").is_err());
    }

    #[test]
    fn the_model_folder_is_where_the_tokenizer_is() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        let root = models.join("Horizon-Labs").join("small").join("rev");
        std::fs::create_dir_all(root.join("onnx")).unwrap();
        let model = root.join("onnx").join("model_quantized.onnx");
        std::fs::write(&model, b"x").unwrap();
        let spec = PunctSpec::from_json(A_SPEC).unwrap();
        let e = spec.model_root(&model, &models).unwrap_err();
        assert!(e.contains("tokenizer.json"), "{e}");
        std::fs::write(root.join("tokenizer.json"), b"{}").unwrap();
        assert_eq!(spec.model_root(&model, &models).unwrap(), root);
        // A flat folder works as well.
        let flat = models.join("flat");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join("tokenizer.json"), b"{}").unwrap();
        assert_eq!(
            spec.model_root(&flat.join("m.onnx"), &models).unwrap(),
            flat
        );
        // Never above models\.
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();
        let lone = models.join("lone");
        std::fs::create_dir_all(&lone).unwrap();
        assert!(spec.model_root(&lone.join("m.onnx"), &models).is_err());
        assert_eq!(
            rel_path(&root, "onnx/model_quantized.onnx"),
            root.join("onnx").join("model_quantized.onnx")
        );
    }
}
