//! The languages Diarist offers (docs/04 section 6): a static tier table
//! from the Nemotron 3.5 model card (FLEURS word error rate at the 1.12 s
//! setting with the language given; character error rate for ja, ko, zh),
//! matched against a model's `capabilities.languages` on the primary
//! subtag, since the GGUF header lists locales (`en-US`, `de-DE`, `ja-JP`).
//!
//! Tags. Nemotron 3.5's prompt dictionary has bare `de`, `fr`, `ko` and so
//! on, but no bare `ja`, `zh`, `vi`, `th` or `he`: the runtime runs those as
//! automatic detection, so the table lists `ja-JP`, `zh-CN` and `vi-VN`, and
//! [`TaggedEngine`] / [`TaggedLiveEngine`] give every job a tag the model's
//! dictionary has ([`runtime_tag`]), also for meetings stored with a bare
//! tag. Bare `es` is `es-US` and bare `pt` is `pt-PT` in the dictionary, so
//! Spanish and Portuguese are listed per locale ([`canonical`] maps the
//! bare tags the same way).
//!
//! The table is data: `manifests/languages.json` (compiled in, equal to
//! [`LANGUAGES`], the fallback), replaced by a data-root copy in
//! `manifests\languages.json` and extended by `languages.user.json`
//! ([`LanguageTable::load`], like `models.user.json`). The app gets the
//! effective table from the `language_table` command; the TypeScript module
//! `generated/languageTiers.ts` ([`typescript_module`]) is its fallback, and
//! `apps/desktop/src/lib/languages.ts` builds the language lists from it.
//! A language a model lists that the table does not know is still offered,
//! named by its tag.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::{Engine, LiveEngine, LiveSession};
use crate::error::Result;
use crate::protocol::{EngineEvent, EngineInfo, FileJob, LiveOpts};
use crate::types::{Channel, JobId, ModelFile};

/// How well Nemotron 3.5 transcribes a language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LanguageTier {
    /// FLEURS WER 4 to 13: listed plainly.
    Ready,
    /// FLEURS WER 15 to 29: listed with "(reduced accuracy)".
    Broad,
    /// The model knows the language but needs fine-tuning for it: offered
    /// only for a model that lists it, marked.
    Adaptation,
}

impl LanguageTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Broad => "broad",
            Self::Adaptation => "adaptation",
        }
    }

    /// The tier's name (Settings, docs/04 section 6).
    pub fn title(self) -> &'static str {
        match self {
            Self::Ready => "Transcription-ready",
            Self::Broad => "Broad coverage",
            Self::Adaptation => "Adaptation-ready",
        }
    }

    /// The one-line expectation shown next to the tier's name.
    pub fn expectation(self) -> &'static str {
        match self {
            Self::Ready => "4 to 13 percent word error rate (FLEURS)",
            Self::Broad => "15 to 29 percent word error rate (FLEURS), reduced accuracy",
            Self::Adaptation => "needs fine-tuning; not offered",
        }
    }
}

/// One language of the tier table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// The tag offered and stored: the primary subtag, or a locale where the
    /// runtime needs one (`ja-JP`) or the language has several (`es-ES`).
    pub code: &'static str,
    /// English name.
    pub name: &'static str,
    pub tier: LanguageTier,
    /// The entry a bare primary subtag stands for (`es` is `es-US` in the
    /// runtime's dictionary, `ja` means `ja-JP`).
    pub bare: bool,
}

const fn lang(code: &'static str, name: &'static str, tier: LanguageTier) -> Language {
    Language {
        code,
        name,
        tier,
        bare: true,
    }
}

const fn locale(code: &'static str, name: &'static str, tier: LanguageTier) -> Language {
    Language {
        code,
        name,
        tier,
        bare: false,
    }
}

/// Every language Nemotron 3.5 was evaluated on, best first within a tier
/// (docs/plans/12 section 1).
pub const LANGUAGES: &[Language] = {
    use LanguageTier::{Adaptation as A, Broad as B, Ready as R};
    &[
        locale("es-ES", "Spanish, Spain", R),
        lang("es-US", "Spanish, Americas", R),
        lang("it", "Italian", R),
        lang("pt-PT", "Portuguese, Portugal", R),
        locale("pt-BR", "Portuguese, Brazil", R),
        lang("hi", "Hindi", R),
        lang("ko", "Korean", R),
        lang("en", "English", R),
        lang("de", "German", R),
        lang("fr", "French", R),
        lang("ru", "Russian", R),
        lang("tr", "Turkish", R),
        lang("vi-VN", "Vietnamese", R),
        lang("nl", "Dutch", R),
        lang("ja-JP", "Japanese", R),
        lang("ar", "Arabic", R),
        lang("uk", "Ukrainian", R),
        lang("pl", "Polish", B),
        lang("nb", "Norwegian Bokmål", B),
        lang("fi", "Finnish", B),
        lang("zh-CN", "Chinese", B),
        lang("cs", "Czech", B),
        lang("bg", "Bulgarian", B),
        lang("sk", "Slovak", B),
        lang("sv", "Swedish", B),
        lang("hr", "Croatian", B),
        lang("ro", "Romanian", B),
        lang("et", "Estonian", B),
        lang("da", "Danish", B),
        lang("hu", "Hungarian", B),
        lang("el", "Greek", A),
        lang("lt", "Lithuanian", A),
        lang("lv", "Latvian", A),
        lang("mt", "Maltese", A),
        lang("sl", "Slovenian", A),
        lang("he", "Hebrew", A),
        lang("th", "Thai", A),
        lang("nn", "Norwegian Nynorsk", A),
    ]
};

/// One language of the effective table: [`LANGUAGES`] as shipped in
/// `manifests/languages.json`, with the data root's overrides
/// ([`LanguageTable::load`]). What the `language_table` command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LanguageEntry {
    /// The tag offered and stored (`de`, `ja-JP`, `es-ES`).
    pub code: String,
    /// English name.
    pub name: String,
    /// Expected quality on Nemotron 3.5; `None` for a language added
    /// without one (no "reduced accuracy" or other mark).
    #[serde(default)]
    pub tier: Option<LanguageTier>,
    /// The entry a bare primary subtag stands for (`ja` means `ja-JP`).
    pub bare: bool,
}

impl From<&Language> for LanguageEntry {
    fn from(l: &Language) -> Self {
        Self {
            code: l.code.to_owned(),
            name: l.name.to_owned(),
            tier: Some(l.tier),
            bare: l.bare,
        }
    }
}

/// An entry as `languages.user.json` (or a data-root copy of
/// `languages.json`) may write it: `bare` defaults to true for a bare code,
/// `tier` to none.
#[derive(Debug, Deserialize)]
struct RawEntry {
    code: String,
    name: String,
    #[serde(default)]
    tier: Option<LanguageTier>,
    #[serde(default)]
    bare: Option<bool>,
}

impl From<RawEntry> for LanguageEntry {
    fn from(r: RawEntry) -> Self {
        let code = r.code.trim().to_owned();
        Self {
            bare: r.bare.unwrap_or(!code.contains(['-', '_'])),
            code,
            name: r.name.trim().to_owned(),
            tier: r.tier,
        }
    }
}

/// The shipped table, compiled in.
const LANGUAGES_JSON: &str = include_str!("../../../../manifests/languages.json");

/// Languages added or changed by the user, in the data root (like
/// `models.user.json`): entries matched on `code` replace the shipped one,
/// new ones are added.
pub const USER_LANGUAGES: &str = "languages.user.json";

/// The effective language table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageTable {
    pub entries: Vec<LanguageEntry>,
    /// Files that could not be read and were skipped, for the log.
    pub notes: Vec<String>,
}

static BUILTIN: LazyLock<LanguageTable> = LazyLock::new(LanguageTable::builtin);

fn parse_entries(text: &str) -> std::result::Result<Vec<LanguageEntry>, String> {
    let raw: Vec<RawEntry> =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let mut out: Vec<LanguageEntry> = Vec::new();
    for r in raw {
        let e = LanguageEntry::from(r);
        if e.code.is_empty() || e.name.is_empty() {
            return Err(format!("an entry needs a code and a name ({e:?})"));
        }
        merge(&mut out, e);
    }
    Ok(out)
}

fn merge(into: &mut Vec<LanguageEntry>, e: LanguageEntry) {
    match into
        .iter_mut()
        .find(|x| x.code.eq_ignore_ascii_case(&e.code))
    {
        Some(x) => *x = e,
        None => into.push(e),
    }
}

impl LanguageTable {
    /// [`LANGUAGES`], the compiled-in fallback.
    pub fn fallback() -> Self {
        Self {
            entries: LANGUAGES.iter().map(LanguageEntry::from).collect(),
            notes: Vec::new(),
        }
    }

    /// The shipped `manifests/languages.json`, else [`Self::fallback`].
    pub fn builtin() -> Self {
        match parse_entries(LANGUAGES_JSON) {
            Ok(entries) if !entries.is_empty() => Self {
                entries,
                notes: Vec::new(),
            },
            Ok(_) => Self::fallback(),
            Err(e) => {
                let mut t = Self::fallback();
                t.notes.push(format!(
                    "manifests/languages.json (built in): {e}; using the table compiled in"
                ));
                t
            }
        }
    }

    /// The built-in table, replaced by `<root>/manifests/languages.json`
    /// when present, then `<root>/languages.user.json` merged over it
    /// (matched on `code`). A file that cannot be read or parsed is skipped
    /// with a note; this never fails.
    pub fn load(data_root: &Path) -> Self {
        let mut t = Self::builtin();
        let copy = data_root.join("manifests").join("languages.json");
        if let Ok(text) = std::fs::read_to_string(&copy) {
            match parse_entries(&text) {
                Ok(entries) if !entries.is_empty() => t.entries = entries,
                Ok(_) => t.notes.push(format!(
                    "{} is empty; using the built-in table",
                    copy.display()
                )),
                Err(e) => t.notes.push(format!(
                    "{}: {e}; using the built-in table (fix or remove the file)",
                    copy.display()
                )),
            }
        }
        let user = data_root.join(USER_LANGUAGES);
        if let Ok(text) = std::fs::read_to_string(&user) {
            match parse_entries(&text) {
                Ok(entries) => {
                    for e in entries {
                        merge(&mut t.entries, e);
                    }
                }
                Err(e) => t.notes.push(format!(
                    "{}: {e}; ignored (fix or remove the file)",
                    user.display()
                )),
            }
        }
        t
    }

    /// The entry `tag` stands for: its own, or for a bare subtag the one
    /// marked [`LanguageEntry::bare`] (`ja` is Japanese, `es` Spanish of the
    /// Americas). Another region (`zh-TW`) has no entry.
    pub fn find(&self, tag: &str) -> Option<&LanguageEntry> {
        let tag = tag.trim();
        if let Some(l) = self
            .entries
            .iter()
            .find(|l| l.code.eq_ignore_ascii_case(tag))
        {
            return Some(l);
        }
        if tag.contains(['-', '_']) {
            return None;
        }
        let p = primary_subtag(tag);
        self.entries
            .iter()
            .find(|l| l.bare && primary_subtag(&l.code) == p)
    }

    /// Whether the table has `tag` or its language (any locale of it), or
    /// `tag` is `auto`: a language Nemotron 3.5 is the answer for when no
    /// installed model lists it.
    pub fn knows(&self, tag: &str) -> bool {
        let p = primary_subtag(tag);
        is_auto(tag) || self.entries.iter().any(|l| primary_subtag(&l.code) == p)
    }

    /// The tag to store and offer for `tag`: the table's code (`ja` and
    /// beta.1's stored `ja` give `ja-JP`), `auto`, or `tag` unchanged (a tag
    /// typed by the user that the table does not know).
    pub fn canonical(&self, tag: &str) -> String {
        let tag = tag.trim();
        if is_auto(tag) {
            return AUTO.into();
        }
        self.find(tag)
            .map_or_else(|| tag.to_owned(), |l| l.code.clone())
    }

    /// `German (de)`, `automatic detection`, or the tag itself.
    pub fn label(&self, tag: &str) -> String {
        let tag = tag.trim();
        if is_auto(tag) {
            return "automatic detection".into();
        }
        match self.find(tag) {
            Some(l) => format!("{} ({tag})", l.name),
            None => tag.to_owned(),
        }
    }
}

/// The job language that asks the model to detect the language (costs
/// about a second of the first utterance, docs/04 section 6). Never the
/// default.
pub const AUTO: &str = "auto";

/// Whether `language` is [`AUTO`].
pub fn is_auto(language: &str) -> bool {
    language.trim().eq_ignore_ascii_case(AUTO)
}

/// The primary subtag of a BCP-47 tag, lower case (`de-DE` and `de_DE` give `de`).
pub fn primary_subtag(tag: &str) -> String {
    tag.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// [`LanguageTable::find`] on the built-in table.
pub fn find(tag: &str) -> Option<&'static LanguageEntry> {
    BUILTIN.find(tag)
}

/// [`LanguageTable::canonical`] on the built-in table.
pub fn canonical(tag: &str) -> String {
    BUILTIN.canonical(tag)
}

/// [`LanguageTable::label`] on the built-in table.
pub fn label(tag: &str) -> String {
    BUILTIN.label(tag)
}

/// The distinct primary subtags `file` lists (`auto` excluded).
fn subtags(file: &ModelFile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in &file.capabilities.languages {
        let p = primary_subtag(t);
        if !p.is_empty() && p != AUTO && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Whether `file` lists its languages at all (Nemotron models do; Parakeet
/// and most hand-added GGUFs do not).
pub fn lists_languages(file: &ModelFile) -> bool {
    !subtags(file).is_empty()
}

/// Whether `file` lists `language` (same primary subtag; `auto` when it
/// lists more than one language). False for a model with no list.
pub fn lists(file: &ModelFile, language: &str) -> bool {
    let tags = subtags(file);
    if is_auto(language) {
        return tags.len() > 1;
    }
    tags.contains(&primary_subtag(language))
}

/// Whether `file` can be given a job in `language`: it lists the language,
/// or it lists no languages at all (not checked: the user chose it and the
/// runtime decides, so nothing switches).
pub fn covers(file: &ModelFile, language: &str) -> bool {
    language.trim().is_empty() || !lists_languages(file) || lists(file, language)
}

/// The one language a single-language model covers (the EN model: `en`).
pub fn only_language(file: &ModelFile) -> Option<String> {
    match subtags(file).as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The tag to give the runtime for `language` on a model listing
/// `model_languages`: unchanged when the list has it, when the model is not
/// multilingual (it is not given a language) or when no listed locale
/// shares its primary subtag; else the first locale that does (Nemotron 3.5
/// lists `ja-JP`, `zh-CN` and `vi-VN` but no bare `ja`, `zh` or `vi`).
pub fn runtime_tag(model_languages: &[String], language: &str) -> String {
    let language = language.trim();
    let p = primary_subtag(language);
    let mut distinct: Vec<String> = model_languages.iter().map(|t| primary_subtag(t)).collect();
    distinct.retain(|t| !t.is_empty() && t != AUTO);
    distinct.sort();
    distinct.dedup();
    if is_auto(language)
        || distinct.len() < 2
        || model_languages
            .iter()
            .any(|t| t.eq_ignore_ascii_case(language))
    {
        return language.to_owned();
    }
    model_languages
        .iter()
        .find(|t| primary_subtag(t) == p)
        .cloned()
        .unwrap_or_else(|| language.to_owned())
}

/// A file engine that gives every job a language tag the model's prompt
/// dictionary has ([`runtime_tag`]); everything else is the inner engine's.
pub struct TaggedEngine {
    inner: Arc<dyn Engine>,
    languages: Vec<String>,
}

impl TaggedEngine {
    /// `languages`: the ASR model's `capabilities.languages`.
    pub fn new(inner: Arc<dyn Engine>, languages: Vec<String>) -> Self {
        Self { inner, languages }
    }
}

impl Engine for TaggedEngine {
    fn transcribe_file(&self, mut job: FileJob) -> Result<Receiver<EngineEvent>> {
        job.language = runtime_tag(&self.languages, &job.language);
        self.inner.transcribe_file(job)
    }

    fn cancel(&self, job_id: JobId) {
        self.inner.cancel(job_id);
    }

    fn info(&self) -> EngineInfo {
        self.inner.info()
    }
}

/// [`TaggedEngine`] for live sessions.
pub struct TaggedLiveEngine {
    inner: Arc<dyn LiveEngine>,
    languages: Vec<String>,
}

impl TaggedLiveEngine {
    /// `languages`: the ASR model's `capabilities.languages`.
    pub fn new(inner: Arc<dyn LiveEngine>, languages: Vec<String>) -> Self {
        Self { inner, languages }
    }
}

impl LiveEngine for TaggedLiveEngine {
    fn open(
        &self,
        job_id: JobId,
        channel: Channel,
        mut opts: LiveOpts,
    ) -> Result<Box<dyn LiveSession>> {
        opts.language = runtime_tag(&self.languages, &opts.language);
        self.inner.open(job_id, channel, opts)
    }
}

/// The TypeScript module with [`LANGUAGES`]
/// (`apps/desktop/src/lib/generated/languageTiers.ts`).
pub fn typescript_module() -> String {
    let entries = BUILTIN
        .entries
        .iter()
        .map(|l| {
            format!(
                "  {{ code: {:?}, name: {:?}, tier: {}, bare: {} }},",
                l.code,
                l.name,
                l.tier
                    .map_or("null".to_owned(), |t| format!("{:?}", t.as_str())),
                l.bare
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tiers = [
        LanguageTier::Ready,
        LanguageTier::Broad,
        LanguageTier::Adaptation,
    ];
    let per_tier = |f: fn(LanguageTier) -> &'static str| {
        tiers
            .iter()
            .map(|t| format!("  {}: {:?},", t.as_str(), f(*t)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (titles, notes) = (
        per_tier(LanguageTier::title),
        per_tier(LanguageTier::expectation),
    );
    format!(
        "// This file was generated from diarist_core::models::languages. Do not edit this file manually.\n\n\
         import type {{ LanguageEntry }} from \"./LanguageEntry\";\n\
         import type {{ LanguageTier }} from \"./LanguageTier\";\n\n\
         /**\n * The shipped table (`manifests/languages.json`), best first within a tier: the fallback for\n \
         * the `language_table` command. `code` is the tag offered and stored; `bare` marks the entry a\n \
         * bare primary subtag stands for.\n */\n\
         export const LANGUAGE_TIERS: readonly LanguageEntry[] = [\n{entries}\n];\n\n\
         /** The name of each tier. */\n\
         export const TIER_TITLES: Record<LanguageTier, string> = {{\n{titles}\n}};\n\n\
         /** The one-line expectation per tier. */\n\
         export const TIER_EXPECTATIONS: Record<LanguageTier, string> = {{\n{notes}\n}};\n\n\
         /** Manifest name of the model that covers every offered language. */\n\
         export const MULTILINGUAL_MODEL = {:?};\n",
        super::MULTILINGUAL_MODEL
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelCapabilities, ModelFileId, ModelRole, ModelSource};

    fn asr(langs: &[&str]) -> ModelFile {
        ModelFile {
            id: ModelFileId(1),
            path: "models/x.gguf".into(),
            role: ModelRole::Asr,
            architecture: Some("asr".into()),
            display_name: "x".into(),
            quant: None,
            size_bytes: 1,
            sha256: None,
            source: ModelSource::User,
            capabilities: ModelCapabilities {
                streaming: true,
                languages: langs.iter().map(|s| (*s).to_owned()).collect(),
                ..Default::default()
            },
            manifest_name: None,
            first_seen_at: 0,
            last_seen_at: 0,
            missing: false,
        }
    }

    /// Part of Nemotron 3.5's prompt dictionary, as `classify` lists it.
    const N35: &[&str] = &[
        "en-US", "en", "es-ES", "es-US", "es", "zh-CN", "zh-TW", "de-DE", "de", "ja-JP", "pt-BR",
        "pt-PT", "pt", "ko-KR", "ko", "vi-VN", "th-TH", "he-IL", "auto",
    ];

    #[test]
    fn tier_table() {
        let count = |t| LANGUAGES.iter().filter(|l| l.tier == t).count();
        // 15 languages, Spanish and Portuguese in two locales each.
        assert_eq!(count(LanguageTier::Ready), 17);
        assert_eq!(count(LanguageTier::Broad), 13);
        assert_eq!(count(LanguageTier::Adaptation), 8);
        for (i, l) in LANGUAGES.iter().enumerate() {
            assert!(
                !LANGUAGES[..i].iter().any(|o| o.code == l.code),
                "{} twice",
                l.code
            );
            let p = primary_subtag(l.code);
            let bare = LANGUAGES
                .iter()
                .filter(|o| o.bare && primary_subtag(o.code) == p)
                .count();
            assert_eq!(bare, 1, "{p}: one entry for the bare subtag");
        }
        assert_eq!(
            find("de-DE").map(|l| l.name.as_str()),
            None,
            "no de-DE entry"
        );
        assert_eq!(find("de").map(|l| l.name.as_str()), Some("German"));
        assert_eq!(find("zh").map(|l| l.code.as_str()), Some("zh-CN"));
        assert_eq!(find("zh-TW"), None);
        assert_eq!(find("xx"), None);
        assert_eq!(label("de"), "German (de)");
        assert_eq!(label("ja"), "Japanese (ja)");
        assert_eq!(label("auto"), "automatic detection");
        assert_eq!(primary_subtag(" EN-gb "), "en");
    }

    #[test]
    fn the_shipped_json_is_the_compiled_table() {
        let t = LanguageTable::builtin();
        assert!(t.notes.is_empty(), "{:?}", t.notes);
        assert_eq!(t.entries, LanguageTable::fallback().entries);
    }

    #[test]
    fn the_data_root_extends_and_overrides_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Nothing there: the shipped table.
        assert_eq!(LanguageTable::load(root).entries, BUILTIN.entries);
        std::fs::write(
            root.join(USER_LANGUAGES),
            "\u{feff}[{\"code\": \"ta\", \"name\": \"Tamil\"},
              {\"code\": \"DE\", \"name\": \"Deutsch\", \"tier\": \"ready\"},
              {\"code\": \"es-MX\", \"name\": \"Spanish, Mexico\"}]",
        )
        .unwrap();
        let t = LanguageTable::load(root);
        assert!(t.notes.is_empty(), "{:?}", t.notes);
        assert_eq!(t.entries.len(), LANGUAGES.len() + 2);
        let ta = t.find("ta").unwrap();
        assert_eq!((ta.name.as_str(), ta.tier, ta.bare), ("Tamil", None, true));
        assert_eq!(t.label("de"), "Deutsch (de)");
        assert_eq!(t.find("es-MX").map(|l| l.bare), Some(false));
        assert_eq!(
            t.canonical("es"),
            "es-US",
            "the bare subtag still means es-US"
        );
        assert!(t.knows("ta-IN") && !t.knows("xx"));
        // A data-root copy of the manifest replaces the shipped list; a
        // broken user file is skipped with a note, never an error.
        std::fs::create_dir_all(root.join("manifests")).unwrap();
        std::fs::write(
            root.join("manifests").join("languages.json"),
            r#"[{"code": "en", "name": "English", "tier": "ready", "bare": true}]"#,
        )
        .unwrap();
        std::fs::write(root.join(USER_LANGUAGES), "{not json").unwrap();
        let t = LanguageTable::load(root);
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.notes.len(), 1);
        assert!(t.notes[0].contains(USER_LANGUAGES), "{:?}", t.notes);
        std::fs::write(root.join("manifests").join("languages.json"), "[").unwrap();
        let t = LanguageTable::load(root);
        assert_eq!(t.entries, BUILTIN.entries, "a broken copy falls back");
        assert_eq!(t.notes.len(), 2);
    }

    #[test]
    fn a_typed_tag_round_trips() {
        // Unknown to the table: stored and given to the runtime as typed,
        // or as the model's locale for it.
        assert_eq!(canonical(" sw-KE "), "sw-KE");
        assert_eq!(canonical("ta"), "ta");
        let n35: Vec<String> = ["en", "de", "sw-KE", "ta-IN", "auto"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(runtime_tag(&n35, &canonical("sw-KE")), "sw-KE");
        assert_eq!(runtime_tag(&n35, &canonical("ta")), "ta-IN");
        assert_eq!(runtime_tag(&n35, &canonical("x-klingon")), "x-klingon");
    }

    #[test]
    fn stored_tags_map_to_the_table() {
        // beta.1 stored bare tags; the runtime ran ja and zh as auto.
        for (stored, code) in [
            ("ja", "ja-JP"),
            ("zh", "zh-CN"),
            ("vi", "vi-VN"),
            ("es", "es-US"),
            ("pt", "pt-PT"),
            ("de", "de"),
            ("ES-es", "es-ES"),
            ("AUTO", "auto"),
            ("zh-TW", "zh-TW"),
            ("xx", "xx"),
        ] {
            assert_eq!(canonical(stored), code, "{stored}");
        }
    }

    #[test]
    fn coverage_on_the_primary_subtag() {
        let en = asr(&["en"]);
        let n35 = asr(N35);
        let unknown = asr(&[]);
        assert!(covers(&en, "en") && covers(&en, "en-GB"));
        assert!(!covers(&en, "de") && !covers(&en, "auto"));
        assert!(covers(&n35, "de") && covers(&n35, "ja-JP") && covers(&n35, "auto"));
        assert!(!covers(&n35, "fr"));
        assert!(covers(&unknown, "de") && covers(&unknown, "auto"));
        assert!(!lists(&unknown, "de") && !lists_languages(&unknown));
        assert_eq!(only_language(&en).as_deref(), Some("en"));
        assert_eq!(only_language(&n35), None);
    }

    #[test]
    fn the_runtime_gets_a_tag_its_dictionary_has() {
        let n35: Vec<String> = N35.iter().map(|s| (*s).to_owned()).collect();
        for (given, sent) in [
            ("ja", "ja-JP"),
            ("zh", "zh-CN"),
            ("vi", "vi-VN"),
            ("th", "th-TH"),
            ("he", "he-IL"),
            ("ja-JP", "ja-JP"),
            ("es", "es"),
            ("es-ES", "es-ES"),
            ("pt", "pt"),
            ("ko", "ko"),
            ("de", "de"),
            ("de-DE", "de-DE"),
            ("auto", "auto"),
            ("fr", "fr"),
        ] {
            assert_eq!(runtime_tag(&n35, given), sent, "{given}");
        }
        // A single-language model is not given a language: unchanged.
        assert_eq!(runtime_tag(&["en".to_owned()], "en-GB"), "en-GB");
        assert_eq!(runtime_tag(&[], "ja"), "ja");
    }

    /// Writes `generated/languageTiers.ts` (name matches the `export_bindings`
    /// filter in docs/DEV.md).
    #[test]
    fn export_bindings_language_tiers() {
        let module = typescript_module();
        assert!(
            module.contains("  { code: \"de\", name: \"German\", tier: \"ready\", bare: true },")
        );
        assert!(module.contains("export const MULTILINGUAL_MODEL = \"nemotron-3.5\";"));
        assert!(module.contains("import type { LanguageEntry } from \"./LanguageEntry\";"));
        if let Some(dir) = std::env::var_os("TS_RS_EXPORT_DIR") {
            let path = std::path::PathBuf::from(dir).join("languageTiers.ts");
            if std::fs::read_to_string(&path).ok().as_deref() != Some(module.as_str()) {
                std::fs::write(path, module).unwrap();
            }
        }
    }
}
