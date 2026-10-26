//! Punctuation restoration (docs/plans/16 section 3): an add-on token
//! classifier over the finished words of a run, run in-process on the CPU
//! after the echo gate and before segmentation.
//!
//! The seam is the [`Punctuator`] trait: a model labels a whole stream of
//! words ([`Punctuator::label_stream`]) with one [`WordLabel`] per word, and
//! Diarist applies the labels itself, with the invariants of plan 16
//! section 3.3 checked per word, so punctuation can never change a word,
//! its times or its speaker. What ran, or why nothing ran, is recorded on
//! the run as `engine_info.punctuation` ([`PunctReport`]).
//!
//! [`current`] builds the punctuator for the selected model (the ONNX
//! runner, `onnx.rs`); [`load`] wraps it with the `transcript.punctuation`
//! setting and turns every way of not having one into a report.
//! [`apply::punctuate_channels`] applies a punctuator to a run's channels;
//! the post-pass, imports and Restore call it through [`punctuate_loaded`],
//! with the punctuator from a [`PunctSource`]. [`rebuild::rebuild_run`]
//! adds or removes punctuation on a finished meeting as a new run.

// Chunk A: label application, the test fixture and rebuilds.
pub mod apply;
mod chars;
pub mod fixture;
pub mod rebuild;

pub use apply::{punctuate_channels, RunLanguage};
pub use rebuild::{rebuild_run, rebuild_run_with, RebuildReport, RebuildWhy};

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{CoreError, Result};
use crate::models::DataPaths;
use crate::settings;
use crate::store::{Store, StoreError};
use crate::transcript::{SegmentOptions, TranscriptWord};
use crate::types::{Channel, RunId};

// The ONNX runner (chunk B): the description block, the tokenizer, generic
// windowing and the `ort` session.
pub mod onnx;
pub mod spec;
pub mod tokenize;
pub mod windows;

/// A punctuation model: labels whole streams of words.
///
/// `Send` so a job can keep one session for a batch of meetings (the bulk
/// action of plan 16 section 3.9).
pub trait Punctuator: Send {
    /// What the model is, for `engine_info.punctuation`.
    fn descriptor(&self) -> &PunctModelInfo;

    /// One label per word of `words` (one stream: a channel's words in time
    /// order up to a gap break), in order. The punctuator does its own
    /// windowing (at most its token limit per window, context on each side,
    /// short streams batched). A result of another length is refused by the
    /// caller: the stream keeps the ASR's text (`reason: "invariant"`).
    /// `cancel` is polled between windows; when it returns true, return
    /// [`CoreError::Cancelled`].
    fn label_stream(
        &mut self,
        words: &[&str],
        hint: &LangHint,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<WordLabel>>;

    /// The options this model runs with: its description block's `unit`,
    /// `policy` and `marks_by_language`, and the session's thread count.
    /// The caller still sets [`PunctOptions::max_gap_ms`] from the run's
    /// segment options.
    fn options(&self) -> PunctOptions {
        PunctOptions::default()
    }

    /// Counters the runner kept since the last call (marks it predicted
    /// inside a word and dropped), reset by the call. Read once per stream.
    fn take_stats(&mut self) -> LabelStats {
        LabelStats::default()
    }
}

/// Counters a [`Punctuator`] keeps while labelling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LabelStats {
    /// Marks predicted inside a word (B and C label subwords), dropped
    /// (plan 16 section 3.3 rule 7).
    pub inner_marks_dropped: u64,
}

/// The model's decision for one word.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WordLabel {
    /// A mark before the word (`¿` `¡`, Spanish only).
    pub pre: Option<Mark>,
    /// A mark after the word.
    pub post: Option<Mark>,
    /// Case upgrades (never applied as a downgrade).
    pub case: Case,
}

impl WordLabel {
    /// A label that changes nothing.
    pub const KEEP: WordLabel = WordLabel {
        pre: None,
        post: None,
        case: Case::Keep,
    };

    /// Only a mark after the word.
    pub fn post(mark: Mark) -> Self {
        Self {
            post: Some(mark),
            ..Self::default()
        }
    }
}

/// A punctuation mark, independent of script; rendered per script by the
/// allow-list of plan 16 section 3.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Mark {
    Period,
    Comma,
    Question,
    Exclamation,
    Colon,
    Dash,
    /// `¿`, before a word.
    InvertedQuestion,
    /// `¡`, before a word.
    InvertedExclamation,
}

impl Mark {
    pub const ALL: [Mark; 8] = [
        Mark::Period,
        Mark::Comma,
        Mark::Question,
        Mark::Exclamation,
        Mark::Colon,
        Mark::Dash,
        Mark::InvertedQuestion,
        Mark::InvertedExclamation,
    ];

    /// Whether the mark goes before a word.
    pub const fn is_pre(self) -> bool {
        matches!(self, Mark::InvertedQuestion | Mark::InvertedExclamation)
    }

    /// Whether the mark ends a sentence.
    pub const fn ends_sentence(self) -> bool {
        matches!(self, Mark::Period | Mark::Question | Mark::Exclamation)
    }

    /// The snake_case name used in JSON (`marks_by_language`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Mark::Period => "period",
            Mark::Comma => "comma",
            Mark::Question => "question",
            Mark::Exclamation => "exclamation",
            Mark::Colon => "colon",
            Mark::Dash => "dash",
            Mark::InvertedQuestion => "inverted_question",
            Mark::InvertedExclamation => "inverted_exclamation",
        }
    }

    /// The mark a model's label stands for: the mark itself in any of the
    /// scripts of the allow-list (`.` `。` `।`, `,` `，` `、` `،`, `?` `？`
    /// `؟` U+037E, `!` `！`, `:`, `-`, `¿`, `¡`) or its name in any case
    /// (`PERIOD`, `comma`, `inverted_question`). `None` for anything else
    /// (`O`, `<ACRONYM>`, the empty label).
    pub fn from_label(label: &str) -> Option<Mark> {
        let l = label.trim();
        let by_char = match l {
            "." | "。" | "।" => Some(Mark::Period),
            "," | "，" | "、" | "،" => Some(Mark::Comma),
            "?" | "？" | "؟" | "\u{037E}" => Some(Mark::Question),
            "!" | "！" => Some(Mark::Exclamation),
            ":" => Some(Mark::Colon),
            "-" => Some(Mark::Dash),
            "¿" => Some(Mark::InvertedQuestion),
            "¡" => Some(Mark::InvertedExclamation),
            _ => None,
        };
        by_char.or_else(|| {
            Mark::ALL
                .into_iter()
                .find(|m| m.as_str().eq_ignore_ascii_case(l))
        })
    }
}

/// Case upgrades for one word. Lower-casing is not expressible: a letter the
/// ASR capitalised is never lowered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Case {
    /// The ASR's casing.
    #[default]
    Keep,
    /// Raise the first cased letter.
    First,
    /// Raise every letter (an acronym).
    Upper,
    /// Raise the letters whose flag is true, one flag per character of the
    /// word as given to the model (per-character true-casing, B and C). A
    /// flag list of another length is applied as [`Case::First`] when its
    /// first flag is set, else ignored.
    Chars(Vec<bool>),
}

/// What kind of casing a model predicts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CasingKind {
    /// No casing head (A): the sentence-start rule only.
    #[default]
    None,
    /// First-letter casing.
    FirstLetter,
    /// Per-character true-casing (B, C).
    PerChar,
}

/// Where the ONNX Runtime library came from, for `engine_info.punctuation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PunctRuntime {
    /// e.g. `onnxruntime`.
    pub name: String,
    /// The loaded library's version string, e.g. `1.31.0`.
    pub version: String,
    /// The DLL, relative to the data root (absolute only when it lives
    /// outside it, as `model_files.path` does).
    pub path: String,
}

/// What a punctuation model is (`engine_info.punctuation`: `model`,
/// `revision`, `sha256`, `runtime`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PunctModelInfo {
    /// The manifest entry's name, or the file's display name for a drop-in.
    pub name: String,
    pub revision: Option<String>,
    pub sha256: Option<String>,
    /// Primary subtags the model covers; `*` for any language.
    pub languages: Vec<String>,
    pub casing: CasingKind,
    pub runtime: PunctRuntime,
}

impl PunctModelInfo {
    /// Whether the model lists `*`.
    pub fn any_language(&self) -> bool {
        self.languages.iter().any(|l| l.trim() == "*")
    }

    /// Whether the model covers `language` (compared by primary subtag).
    pub fn covers(&self, language: &str) -> bool {
        let p = crate::models::languages::primary_subtag(language);
        self.any_language()
            || self
                .languages
                .iter()
                .any(|l| crate::models::languages::primary_subtag(l) == p)
    }
}

/// The run's language as the punctuator sees it, per stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangHint {
    /// Primary subtag (`es`, not `es-US`); `None` when the meeting is `auto`
    /// and the runtime reported nothing.
    pub language: Option<String>,
    /// The stream's dominant script.
    pub script: Script,
}

/// A writing system, as far as punctuation and casing care (plan 16
/// section 3.4). A word's script is that of its first letter.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Script {
    Latin,
    Cyrillic,
    Greek,
    Armenian,
    Hebrew,
    Arabic,
    Devanagari,
    Hangul,
    Han,
    Hiragana,
    Katakana,
    Thai,
    /// Letters of any other script, or no letters at all.
    #[default]
    Other,
}

impl Script {
    /// The script of one character; [`Script::Other`] for non-letters.
    pub fn of_char(c: char) -> Script {
        if !c.is_alphabetic() {
            return Script::Other;
        }
        match c as u32 {
            0x0041..=0x024F | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF => Script::Latin,
            0xFF21..=0xFF3A | 0xFF41..=0xFF5A => Script::Latin,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
            0x0400..=0x052F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => {
                Script::Cyrillic
            }
            0x0530..=0x058F | 0xFB13..=0xFB17 => Script::Armenian,
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => Script::Hebrew,
            0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF => Script::Arabic,
            0xFE70..=0xFEFF => Script::Arabic,
            0x0900..=0x097F | 0xA8E0..=0xA8FF => Script::Devanagari,
            0x0E00..=0x0E7F => Script::Thai,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Script::Hangul,
            0x3040..=0x309F => Script::Hiragana,
            0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Katakana,
            0x3005 | 0x3007 | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han,
            0x20000..=0x3134F => Script::Han,
            _ => Script::Other,
        }
    }

    /// The script of a word: that of its first letter; `None` for a word
    /// without letters (digits, marks), which takes the stream's dominant
    /// script.
    pub fn of_word(word: &str) -> Option<Script> {
        word.chars()
            .find(|c| c.is_alphabetic())
            .map(Script::of_char)
    }

    /// The most frequent script among the words with letters (ties go to
    /// the script seen first); [`Script::Other`] when no word has letters.
    pub fn dominant<'a>(words: impl IntoIterator<Item = &'a str>) -> Script {
        let mut counts: Vec<(Script, usize)> = Vec::new();
        for w in words {
            if let Some(s) = Script::of_word(w) {
                match counts.iter_mut().find(|(k, _)| *k == s) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((s, 1)),
                }
            }
        }
        let mut best: Option<(Script, usize)> = None;
        for (s, n) in counts {
            if best.is_none_or(|(_, b)| n > b) {
                best = Some((s, n));
            }
        }
        best.map_or(Script::Other, |(s, _)| s)
    }

    /// Scripts with upper and lower case, where the sentence-start rule
    /// applies.
    pub const fn has_case(self) -> bool {
        matches!(
            self,
            Script::Latin | Script::Cyrillic | Script::Greek | Script::Armenian
        )
    }

    /// Scripts written without spaces between words (marks go only at the
    /// ends of the runtime's word units).
    pub const fn is_continuous(self) -> bool {
        matches!(
            self,
            Script::Han | Script::Hiragana | Script::Katakana | Script::Thai
        )
    }

    /// Languages Diarist offers in this script (primary subtags), for a
    /// meeting in `auto` without a detected language: a model punctuates
    /// such a stream when it lists `*` or one of these.
    pub const fn languages(self) -> &'static [&'static str] {
        match self {
            Script::Latin => &[
                "en", "de", "fr", "es", "it", "pt", "nl", "tr", "vi", "pl", "nb", "nn", "no", "fi",
                "cs", "sk", "sv", "hr", "ro", "et", "da", "hu", "lt", "lv", "mt", "sl",
            ],
            Script::Cyrillic => &["ru", "uk", "bg"],
            Script::Greek => &["el"],
            Script::Armenian => &["hy"],
            Script::Hebrew => &["he"],
            Script::Arabic => &["ar"],
            Script::Devanagari => &["hi"],
            Script::Hangul => &["ko"],
            Script::Han => &["zh", "ja"],
            Script::Hiragana | Script::Katakana => &["ja"],
            Script::Thai => &["th"],
            Script::Other => &[],
        }
    }

    /// The name recorded in `engine_info.punctuation.language` when only
    /// the script decided.
    pub const fn as_str(self) -> &'static str {
        match self {
            Script::Latin => "latin",
            Script::Cyrillic => "cyrillic",
            Script::Greek => "greek",
            Script::Armenian => "armenian",
            Script::Hebrew => "hebrew",
            Script::Arabic => "arabic",
            Script::Devanagari => "devanagari",
            Script::Hangul => "hangul",
            Script::Han => "han",
            Script::Hiragana => "hiragana",
            Script::Katakana => "katakana",
            Script::Thai => "thai",
            Script::Other => "other",
        }
    }
}

/// How a stream is cut for the model (plan 16 section 3.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PunctUnit {
    /// A channel's words in time order; only gaps of more than
    /// [`PunctOptions::max_gap_ms`] break it.
    #[default]
    Stream,
    /// Also broken where the speaker label changes.
    Turn,
}

/// What happens to marks the ASR wrote (plan 16 section 3.3 rule 5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MarkPolicy {
    /// Keep the ASR's marks; the model fills the rest.
    #[default]
    KeepAsr,
    /// Strip the ASR's trailing allow-list marks and use the model's only.
    Replace,
}

/// How labels are applied to a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PunctOptions {
    pub unit: PunctUnit,
    pub policy: MarkPolicy,
    /// ORT intra-op threads (recorded in the report).
    pub threads: u32,
    /// Marks allowed per language (primary subtag, or `*` for every language
    /// not listed), replacing the default allow-list of plan 16 section 3.4
    /// (period, comma, question, exclamation; `¿` `¡` for `es`; colon and
    /// dash only where listed here).
    pub marks_by_language: BTreeMap<String, Vec<Mark>>,
    /// A pause longer than this breaks a stream (the segmenter's own split,
    /// [`crate::transcript::SegmentOptions::max_gap_ms`]).
    pub max_gap_ms: i64,
}

impl Default for PunctOptions {
    fn default() -> Self {
        Self {
            unit: PunctUnit::Stream,
            policy: MarkPolicy::KeepAsr,
            threads: DEFAULT_THREADS,
            marks_by_language: BTreeMap::new(),
            max_gap_ms: crate::transcript::SegmentOptions::default().max_gap_ms,
        }
    }
}

/// `transcript.punctuation_threads` when unset.
pub const DEFAULT_THREADS: u32 = 4;

/// Why a run has the punctuation it has (`engine_info.punctuation.reason`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PunctReason {
    /// Applied.
    Ok,
    /// `transcript.punctuation = off`, or "Remove punctuation".
    Off,
    /// No punctuation model is selected and installed.
    #[default]
    NoModel,
    /// The model does not cover the run's language (or the script has no
    /// marks, Thai).
    Language,
    /// The ASR already punctuates this channel
    /// ([`apply::ASR_PUNCTUATES_PER_100`]: 12 or more marks per 100 words):
    /// only the sentence-start casing rule ran.
    AsrPunctuates,
    /// ONNX Runtime is missing, the wrong module loaded, or a dependency of
    /// it is missing.
    Runtime,
    /// The model failed (`message`).
    Error,
    /// The model's labels did not fit the words (wrong length).
    Invariant,
}

impl PunctReason {
    pub const ALL: [PunctReason; 8] = [
        PunctReason::Ok,
        PunctReason::Off,
        PunctReason::NoModel,
        PunctReason::Language,
        PunctReason::AsrPunctuates,
        PunctReason::Runtime,
        PunctReason::Error,
        PunctReason::Invariant,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            PunctReason::Ok => "ok",
            PunctReason::Off => "off",
            PunctReason::NoModel => "no_model",
            PunctReason::Language => "language",
            PunctReason::AsrPunctuates => "asr_punctuates",
            PunctReason::Runtime => "runtime",
            PunctReason::Error => "error",
            PunctReason::Invariant => "invariant",
        }
    }
}

/// `engine_info.punctuation` of a run (plan 16 section 3.5): replaced
/// wholesale on every build. A run that was not punctuated carries only
/// `applied`, `reason` and, where useful, `message`, `model` and `language`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PunctReport {
    /// At least one channel got the model's labels.
    pub applied: bool,
    /// `ok` when applied; else why not (the first channel's reason).
    pub reason: PunctReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runtime: Option<PunctRuntime>,
    /// The language the run was punctuated as (primary subtag), or the
    /// script's name when only the script decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Where `language` came from: `meeting`, `detected` or `script`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unit: Option<PunctUnit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policy: Option<MarkPolicy>,
    /// Per channel (`mic`, `loopback`, `source`): words with a mark from the
    /// ASR, per 100 words.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, f64>>", optional)]
    pub asr_marks_per_100: BTreeMap<String, f64>,
    /// Per channel: `ok`, or why that channel was left alone.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(as = "Option<BTreeMap<String, PunctReason>>", optional)]
    pub channels: BTreeMap<String, PunctReason>,
    /// Words seen over every channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub words: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub marks_added: Option<u64>,
    /// Words with at least one letter raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub case_upgrades: Option<u64>,
    /// Words whose labelled text failed an invariant and kept the ASR's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rejected_words: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub inner_marks_dropped: Option<u64>,
    /// Streams whose labels were refused (wrong length).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub streams_refused: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wall_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub threads: Option<u32>,
    /// Set on a run built by a rebuild (Add or Remove punctuation): the
    /// root run its words came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rebuilt_from: Option<RunId>,
}

impl PunctReport {
    /// A report for a run that was not punctuated.
    pub fn not_applied(reason: PunctReason) -> Self {
        Self {
            applied: false,
            reason,
            ..Self::default()
        }
    }

    /// [`PunctReport::not_applied`] with a message.
    pub fn not_applied_with(reason: PunctReason, message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::not_applied(reason)
        }
    }

    /// As JSON for `engine_info.punctuation`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"applied": false}))
    }

    /// The report stored in a run's `engine_info`, if it has one.
    pub fn of_engine_info(engine_info: &serde_json::Value) -> Option<PunctReport> {
        serde_json::from_value(engine_info.get("punctuation")?.clone()).ok()
    }
}

/// Why [`current`] could not give a punctuator although one is selected:
/// `reason` is [`PunctReason::Runtime`] (the DLL is missing, the wrong module
/// loaded, a dependency is missing) or [`PunctReason::Error`] (anything
/// else, e.g. a broken model file). Other errors convert to `Error`, so `?`
/// works inside [`current`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: PunctReason,
    pub message: String,
}

impl Unavailable {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            reason: PunctReason::Runtime,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            reason: PunctReason::Error,
            message: message.into(),
        }
    }
}

impl From<CoreError> for Unavailable {
    fn from(e: CoreError) -> Self {
        Unavailable::error(e.to_string())
    }
}

impl From<StoreError> for Unavailable {
    fn from(e: StoreError) -> Self {
        Unavailable::error(e.to_string())
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason.as_str(), self.message)
    }
}

/// The punctuator for the selected punctuation model of the data root at
/// `paths` (`selected_model.punctuation`, else the recommended file), built
/// from the caller's `paths` (the CLI's and the eval harness's
/// `--models-dir` and `--engine-dir` overrides), never from
/// `DataPaths::at(root)`. `Ok(None)`: no punctuation model is installed.
/// Does not read `transcript.punctuation` ([`load`] does).
///
/// The session is created here and dropped with the punctuator, so RAM
/// returns to the baseline after a job.
pub fn current(
    paths: &DataPaths,
    store: &dyn Store,
) -> std::result::Result<Option<Box<dyn Punctuator>>, Unavailable> {
    // The ONNX runner (onnx.rs) for the selected file, with the runtime
    // checks of plan 16 section 3.7 and `threads(store)` intra-op threads.
    onnx::current(paths, store)
}

/// The punctuator to use for a build, or the report to write when there is
/// none: `off` when `transcript.punctuation` is `off`, `no_model` when
/// [`current`] finds nothing, `runtime` or `error` (with the message) when
/// it fails.
pub fn load(
    paths: &DataPaths,
    store: &dyn Store,
) -> std::result::Result<Box<dyn Punctuator>, Box<PunctReport>> {
    if setting(store) == PunctSetting::Off {
        return Err(Box::new(PunctReport::not_applied(PunctReason::Off)));
    }
    match current(paths, store) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(Box::new(PunctReport::not_applied(PunctReason::NoModel))),
        Err(u) => {
            log::warn!("punctuation unavailable: {u}");
            Err(Box::new(PunctReport::not_applied_with(u.reason, u.message)))
        }
    }
}

/// A punctuator ready for a build, or the report to write without one
/// ([`load`], [`PunctSource::load`]).
pub type Loaded = std::result::Result<Box<dyn Punctuator>, Box<PunctReport>>;

/// Builds a punctuator for a store (tests, a session shared by a batch).
pub type PunctFactory = dyn Fn(&dyn Store) -> Loaded + Send + Sync;

/// Where a build (the post-pass, an import, Restore, a rebuild) gets its
/// punctuator: normally the selected model of the caller's data paths (the
/// CLI's and the eval harness's `--models-dir` and `--engine-dir` overrides
/// included), never a `DataPaths` rebuilt from a bare root.
#[derive(Clone, Default)]
pub enum PunctSource {
    /// Nothing punctuates (`reason: "no_model"`): tools and tests that do not.
    #[default]
    None,
    /// The selected punctuation model of these data paths ([`load`]).
    Data(DataPaths),
    /// Anything else, e.g. a [`fixture::FixturePunctuator`] in tests.
    Factory(Arc<PunctFactory>),
}

impl PunctSource {
    /// A source that builds a new punctuator with `f` for every build.
    pub fn factory<P: Punctuator + 'static>(f: impl Fn() -> P + Send + Sync + 'static) -> Self {
        PunctSource::Factory(Arc::new(move |_| Ok(Box::new(f()) as Box<dyn Punctuator>)))
    }

    /// The punctuator, or the report without one. `transcript.punctuation =
    /// off` is honoured for every source.
    pub fn load(&self, store: &dyn Store) -> Loaded {
        match self {
            PunctSource::None => Err(Box::new(PunctReport::not_applied(PunctReason::NoModel))),
            PunctSource::Data(paths) => load(paths, store),
            PunctSource::Factory(f) => {
                if setting(store) == PunctSetting::Off {
                    return Err(Box::new(PunctReport::not_applied(PunctReason::Off)));
                }
                f(store)
            }
        }
    }
}

impl std::fmt::Debug for PunctSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PunctSource::None => f.write_str("PunctSource::None"),
            PunctSource::Data(p) => f.debug_tuple("PunctSource::Data").field(&p.root).finish(),
            PunctSource::Factory(_) => f.write_str("PunctSource::Factory"),
        }
    }
}

/// Punctuates the channels of a build with `loaded` (the model's options,
/// streams broken at the run's `max_gap_ms`) and returns the report for
/// `engine_info.punctuation`. Without a punctuator every word gets the
/// ASR's text back and the report is the one `loaded` carries.
///
/// A panic in the punctuator (or in applying its labels) never reaches the
/// caller: the words keep the ASR's text, the report says `error` with the
/// panic's message, and the punctuator is dropped, `loaded` keeping that
/// report, so the rest of a bulk run does not use the session again.
pub fn punctuate_loaded(
    loaded: &mut Loaded,
    channels: &mut [(Channel, Vec<TranscriptWord>)],
    segment: &SegmentOptions,
    lang: &RunLanguage,
    cancel: &dyn Fn() -> bool,
) -> PunctReport {
    let outcome = match loaded {
        Ok(p) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut opts = p.options();
            opts.max_gap_ms = segment.max_gap_ms;
            apply::punctuate(channels, Some(p.as_mut()), &opts, lang, cancel)
        }))
        .unwrap_or_else(|payload| {
            let message = format!(
                "punctuation panicked: {}",
                apply::panic_message(payload.as_ref())
            );
            log::warn!("{message}");
            apply::Outcome {
                report: PunctReport::not_applied_with(PunctReason::Error, message),
                panicked: true,
            }
        }),
        Err(report) => {
            punctuate_channels(channels, None, &PunctOptions::default(), lang, cancel);
            return (**report).clone();
        }
    };
    if outcome.panicked {
        apply::restore_asr_text(channels);
        let report = PunctReport::not_applied_with(
            PunctReason::Error,
            outcome
                .report
                .message
                .clone()
                .unwrap_or_else(|| "the punctuation model panicked".to_owned()),
        );
        // The session is not used again; its drop may panic too.
        let gone = std::mem::replace(loaded, Err(Box::new(report)));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(gone)));
    }
    outcome.report
}

/// The `transcript.punctuation` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PunctSetting {
    /// Punctuate when a model is installed and covers the language.
    Auto,
    Off,
}

/// `transcript.punctuation`: `off` (or any false-like value) is off;
/// anything else, an unreadable store included, is `auto`.
pub fn setting(store: &dyn Store) -> PunctSetting {
    let Ok(s) = settings::effective(store) else {
        return PunctSetting::Auto;
    };
    match settings::get(&s, settings::TRANSCRIPT_PUNCTUATION)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "off" | "false" | "0" | "no" | "none" => PunctSetting::Off,
        _ => PunctSetting::Auto,
    }
}

/// `transcript.punctuation_threads`, clamped to 1 to the machine's
/// available parallelism; unreadable: [`DEFAULT_THREADS`] (clamped).
pub fn threads(store: &dyn Store) -> u32 {
    let max = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(DEFAULT_THREADS);
    let v = settings::effective(store)
        .ok()
        .and_then(|s| {
            settings::get(&s, settings::TRANSCRIPT_PUNCTUATION_THREADS)
                .trim()
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(DEFAULT_THREADS);
    v.clamp(1, max.max(1))
}

/// Labels nothing: every word keeps the ASR's text. For tests and as a
/// stand-in where a [`Punctuator`] is required but none should run.
#[derive(Debug, Clone, Default)]
pub struct NoopPunctuator {
    info: PunctModelInfo,
}

impl NoopPunctuator {
    pub fn new() -> Self {
        Self {
            info: PunctModelInfo {
                name: "noop".to_owned(),
                languages: vec!["*".to_owned()],
                ..PunctModelInfo::default()
            },
        }
    }
}

impl Punctuator for NoopPunctuator {
    fn descriptor(&self) -> &PunctModelInfo {
        &self.info
    }

    fn label_stream(
        &mut self,
        words: &[&str],
        _hint: &LangHint,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<WordLabel>> {
        if cancel() {
            return Err(CoreError::Cancelled);
        }
        Ok(vec![WordLabel::KEEP; words.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    #[test]
    fn marks_parse_from_labels_and_names() {
        assert_eq!(Mark::from_label("."), Some(Mark::Period));
        assert_eq!(Mark::from_label("PERIOD"), Some(Mark::Period));
        assert_eq!(Mark::from_label("。"), Some(Mark::Period));
        assert_eq!(Mark::from_label("، "), Some(Mark::Comma));
        assert_eq!(Mark::from_label("؟"), Some(Mark::Question));
        assert_eq!(Mark::from_label("¿"), Some(Mark::InvertedQuestion));
        assert_eq!(
            Mark::from_label("inverted_exclamation"),
            Some(Mark::InvertedExclamation)
        );
        assert_eq!(Mark::from_label("O"), None);
        assert_eq!(Mark::from_label("<ACRONYM>"), None);
        assert_eq!(Mark::from_label(""), None);
        for m in Mark::ALL {
            assert_eq!(Mark::from_label(m.as_str()), Some(m));
            assert_eq!(
                serde_json::to_value(m).unwrap(),
                serde_json::json!(m.as_str())
            );
        }
        assert!(Mark::InvertedQuestion.is_pre() && !Mark::Question.is_pre());
        assert!(Mark::Question.ends_sentence() && !Mark::Comma.ends_sentence());
    }

    #[test]
    fn scripts_of_words() {
        assert_eq!(Script::of_word("hello"), Some(Script::Latin));
        assert_eq!(Script::of_word("Ärger"), Some(Script::Latin));
        assert_eq!(Script::of_word("việt"), Some(Script::Latin));
        assert_eq!(Script::of_word("привет"), Some(Script::Cyrillic));
        assert_eq!(Script::of_word("γεια"), Some(Script::Greek));
        assert_eq!(Script::of_word("שלום"), Some(Script::Hebrew));
        assert_eq!(Script::of_word("مرحبا"), Some(Script::Arabic));
        assert_eq!(Script::of_word("नमस्ते"), Some(Script::Devanagari));
        assert_eq!(Script::of_word("안녕"), Some(Script::Hangul));
        assert_eq!(Script::of_word("你好"), Some(Script::Han));
        assert_eq!(Script::of_word("ありがとう"), Some(Script::Hiragana));
        assert_eq!(Script::of_word("カタカナ"), Some(Script::Katakana));
        assert_eq!(Script::of_word("สวัสดี"), Some(Script::Thai));
        assert_eq!(Script::of_word("Բարեւ"), Some(Script::Armenian));
        // The first letter decides; digits have none.
        assert_eq!(Script::of_word("3d"), Some(Script::Latin));
        assert_eq!(Script::of_word("2026"), None);
        assert_eq!(Script::of_word("..."), None);
        assert_eq!(
            Script::dominant(["это", "test", "очень", "42"]),
            Script::Cyrillic
        );
        assert_eq!(Script::dominant(["42", "!"]), Script::Other);
        assert_eq!(Script::dominant(["a", "б"]), Script::Latin);
        assert!(Script::Latin.has_case() && !Script::Han.has_case());
        assert!(Script::Han.languages().contains(&"ja"));
    }

    #[test]
    fn model_info_covers_by_primary_subtag() {
        let info = PunctModelInfo {
            languages: vec!["en".into(), "es-ES".into()],
            ..PunctModelInfo::default()
        };
        assert!(info.covers("en-GB") && info.covers("es-US") && info.covers("ES"));
        assert!(!info.covers("de") && !info.any_language());
        let any = PunctModelInfo {
            languages: vec!["*".into()],
            ..PunctModelInfo::default()
        };
        assert!(any.covers("th") && any.any_language());
    }

    #[test]
    fn report_without_a_model_is_minimal_json() {
        let r = PunctReport::not_applied(PunctReason::NoModel);
        assert_eq!(
            r.to_json(),
            serde_json::json!({"applied": false, "reason": "no_model"})
        );
        let e = PunctReport::not_applied_with(PunctReason::Runtime, "wrong module");
        assert_eq!(
            e.to_json(),
            serde_json::json!({"applied": false, "reason": "runtime", "message": "wrong module"})
        );
        let back: PunctReport = serde_json::from_value(e.to_json()).unwrap();
        assert_eq!(back, e);
        let info = serde_json::json!({"segmentation": {}, "punctuation": e.to_json()});
        assert_eq!(PunctReport::of_engine_info(&info), Some(e));
        assert_eq!(PunctReport::of_engine_info(&serde_json::json!({})), None);
        for r in PunctReason::ALL {
            assert_eq!(
                serde_json::to_value(r).unwrap(),
                serde_json::json!(r.as_str())
            );
        }
    }

    #[test]
    fn load_without_a_model_reports_no_model_or_off() {
        let store = MemoryStore::new();
        let paths = DataPaths::at(std::env::temp_dir().join("diarist-punct-none"));
        assert!(current(&paths, &store).unwrap().is_none());
        let err = load(&paths, &store).err().unwrap();
        assert_eq!(err.reason, PunctReason::NoModel);
        assert!(!err.applied);
        store
            .set_setting(settings::TRANSCRIPT_PUNCTUATION, "off")
            .unwrap();
        assert_eq!(setting(&store), PunctSetting::Off);
        assert_eq!(load(&paths, &store).err().unwrap().reason, PunctReason::Off);
        store
            .set_setting(settings::TRANSCRIPT_PUNCTUATION, "auto")
            .unwrap();
        assert_eq!(setting(&store), PunctSetting::Auto);
    }

    #[test]
    fn threads_are_clamped() {
        let store = MemoryStore::new();
        let max = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);
        assert_eq!(threads(&store), DEFAULT_THREADS.min(max));
        store
            .set_setting(settings::TRANSCRIPT_PUNCTUATION_THREADS, "0")
            .unwrap();
        assert_eq!(threads(&store), 1);
        store
            .set_setting(settings::TRANSCRIPT_PUNCTUATION_THREADS, "100000")
            .unwrap();
        assert_eq!(threads(&store), max);
        store
            .set_setting(settings::TRANSCRIPT_PUNCTUATION_THREADS, "many")
            .unwrap();
        assert_eq!(threads(&store), DEFAULT_THREADS.min(max));
    }

    #[test]
    fn noop_labels_every_word_with_keep() {
        let mut p = NoopPunctuator::new();
        let hint = LangHint {
            language: Some("en".into()),
            script: Script::Latin,
        };
        let labels = p.label_stream(&["a", "b", "c"], &hint, &|| false).unwrap();
        assert_eq!(labels, vec![WordLabel::KEEP; 3]);
        assert!(matches!(
            p.label_stream(&["a"], &hint, &|| true),
            Err(CoreError::Cancelled)
        ));
        assert_eq!(p.descriptor().name, "noop");
        assert_eq!(p.options(), PunctOptions::default());
        assert_eq!(p.take_stats(), LabelStats::default());
        assert_eq!(PunctOptions::default().max_gap_ms, 1_500);
    }
}
