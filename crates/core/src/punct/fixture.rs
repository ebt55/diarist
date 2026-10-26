//! [`FixturePunctuator`]: scripted labels for tests (no model). Used by the
//! unit tests of `apply.rs` and `rebuild.rs`, the post-pass, import and
//! Restore tests, and anything else that needs punctuation without ONNX.

use std::sync::{Arc, Mutex};

use super::{LangHint, Mark, PunctModelInfo, PunctRuntime, Punctuator, WordLabel};
use crate::error::{CoreError, Result};

type Rule = Box<dyn FnMut(&[&str], &LangHint) -> Result<Vec<WordLabel>> + Send>;

/// A [`Punctuator`] whose labels come from a closure. Every stream it was
/// given is recorded ([`FixturePunctuator::calls`]), also through a handle
/// that outlives the punctuator ([`FixturePunctuator::calls_handle`]).
pub struct FixturePunctuator {
    info: PunctModelInfo,
    rule: Rule,
    calls: Arc<Mutex<Vec<Vec<String>>>>,
}

impl FixturePunctuator {
    /// Labels from `rule(stream, hint)`; it may return a wrong length or an
    /// error on purpose.
    pub fn new(
        rule: impl FnMut(&[&str], &LangHint) -> Result<Vec<WordLabel>> + Send + 'static,
    ) -> Self {
        Self {
            info: PunctModelInfo {
                name: "fixture".to_owned(),
                revision: Some("test".to_owned()),
                sha256: None,
                languages: vec!["*".to_owned()],
                casing: super::CasingKind::None,
                runtime: PunctRuntime {
                    name: "fixture".to_owned(),
                    version: "0".to_owned(),
                    path: String::new(),
                },
            },
            rule: Box::new(rule),
            calls: Arc::default(),
        }
    }

    /// One label per word from `f(word)` (the word as the model sees it:
    /// the ASR's text without its trailing marks).
    pub fn per_word(f: impl Fn(&str) -> WordLabel + Send + 'static) -> Self {
        Self::new(move |words, _| Ok(words.iter().map(|w| f(w)).collect()))
    }

    /// A mark after every word listed (compared without case), nothing
    /// elsewhere.
    pub fn marks(marks: &[(&str, Mark)]) -> Self {
        let marks: Vec<(String, Mark)> =
            marks.iter().map(|(w, m)| (w.to_lowercase(), *m)).collect();
        Self::per_word(move |w| {
            let w = w.to_lowercase();
            marks
                .iter()
                .find(|(k, _)| *k == w)
                .map_or(WordLabel::KEEP, |(_, m)| WordLabel::post(*m))
        })
    }

    /// The languages the model claims (default `*`).
    pub fn languages(mut self, languages: &[&str]) -> Self {
        self.info.languages = languages.iter().map(|l| (*l).to_owned()).collect();
        self
    }

    /// The streams labelled so far, as given.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// A handle on the recorded streams that stays readable after the
    /// punctuator moved into a job.
    pub fn calls_handle(&self) -> Arc<Mutex<Vec<Vec<String>>>> {
        Arc::clone(&self.calls)
    }
}

impl Punctuator for FixturePunctuator {
    fn descriptor(&self) -> &PunctModelInfo {
        &self.info
    }

    fn label_stream(
        &mut self,
        words: &[&str],
        hint: &LangHint,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<WordLabel>> {
        if cancel() {
            return Err(CoreError::Cancelled);
        }
        if let Ok(mut c) = self.calls.lock() {
            c.push(words.iter().map(|w| (*w).to_owned()).collect());
        }
        (self.rule)(words, hint)
    }
}
