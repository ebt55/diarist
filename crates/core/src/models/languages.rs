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
//! The table is exported to TypeScript as `generated/languageTiers.ts`
//! ([`typescript_module`]); `apps/desktop/src/lib/languages.ts` builds the
//! language lists from it.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use crate::engine::{Engine, LiveEngine, LiveSession};
use crate::error::Result;
use crate::protocol::{EngineEvent, EngineInfo, FileJob, LiveOpts};
use crate::types::{Channel, JobId, ModelFile};

/// How well Nemotron 3.5 transcribes a language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageTier {
    /// FLEURS WER 4 to 13: listed plainly.
    Ready,
    /// FLEURS WER 15 to 29: listed with "(reduced accuracy)".
    Broad,
    /// The model knows the language but needs fine-tuning for it: never
    /// offered.
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

/// The tier table entry `tag` stands for: its own entry, or for a bare
/// subtag the entry marked [`Language::bare`] (`ja` is Japanese, `es`
/// Spanish of the Americas). Another region (`zh-TW`) has no entry.
pub fn find(tag: &str) -> Option<&'static Language> {
    let tag = tag.trim();
    if let Some(l) = LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(tag)) {
        return Some(l);
    }
    if tag.contains(['-', '_']) {
        return None;
    }
    let p = primary_subtag(tag);
    LANGUAGES
        .iter()
        .find(|l| l.bare && primary_subtag(l.code) == p)
}

/// The tag to store and offer for `tag`: the tier table's code (`ja` and
/// beta.1's stored `ja` give `ja-JP`), `auto`, or `tag` unchanged.
pub fn canonical(tag: &str) -> String {
    let tag = tag.trim();
    if is_auto(tag) {
        return AUTO.into();
    }
    find(tag).map_or_else(|| tag.to_owned(), |l| l.code.to_owned())
}

/// `German (de)`, `automatic detection`, or the tag itself.
pub fn label(tag: &str) -> String {
    let tag = tag.trim();
    if is_auto(tag) {
        return "automatic detection".into();
    }
    match find(tag) {
        Some(l) => format!("{} ({tag})", l.name),
        None => tag.to_owned(),
    }
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
    let entries = LANGUAGES
        .iter()
        .map(|l| {
            format!(
                "  {{ code: {:?}, name: {:?}, tier: {:?}, bare: {} }},",
                l.code,
                l.name,
                l.tier.as_str(),
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
         /** How well Nemotron 3.5 transcribes a language (FLEURS, docs/04 section 6). */\n\
         export type LanguageTier = \"ready\" | \"broad\" | \"adaptation\";\n\n\
         /**\n * Every evaluated language, best first within a tier; `adaptation` ones are never offered.\n \
         * `code` is the tag offered and stored; `bare` marks the entry a bare primary subtag stands for.\n */\n\
         export const LANGUAGE_TIERS: readonly {{ code: string; name: string; tier: LanguageTier; bare: boolean }}[] = [\n{entries}\n];\n\n\
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
        assert_eq!(find("de-DE").map(|l| l.name), None, "no de-DE entry");
        assert_eq!(find("de").map(|l| l.name), Some("German"));
        assert_eq!(find("zh").map(|l| l.code), Some("zh-CN"));
        assert_eq!(find("zh-TW"), None);
        assert_eq!(find("xx"), None);
        assert_eq!(label("de"), "German (de)");
        assert_eq!(label("ja"), "Japanese (ja)");
        assert_eq!(label("auto"), "automatic detection");
        assert_eq!(primary_subtag(" EN-gb "), "en");
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
        if let Some(dir) = std::env::var_os("TS_RS_EXPORT_DIR") {
            let path = std::path::PathBuf::from(dir).join("languageTiers.ts");
            if std::fs::read_to_string(&path).ok().as_deref() != Some(module.as_str()) {
                std::fs::write(path, module).unwrap();
            }
        }
    }
}
