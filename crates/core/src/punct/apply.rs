//! Applying a model's labels to the ASR's words (plan 16 sections 3.2 to
//! 3.4). Pure code, no model: the [`Punctuator`] labels whole streams and
//! this module decides what each word's display text becomes, checking the
//! invariants of section 3.3 on every word.
//!
//! Definitions (section 3.3): `old` is the ASR's text of a word; `base` is
//! `old` under [`MarkPolicy::KeepAsr`] and `old` without its trailing
//! allow-list marks under [`MarkPolicy::Replace`]; `new` is the display text.
//! The model is given `old` without its trailing allow-list marks under both
//! policies (the input it was trained on, research/18 section 4.3).
//!
//! What can change, per word: one mark appended from the allow-list, one
//! pre-mark prepended (`¿` `¡`, Spanish), letters raised from lower to upper
//! case. Times, speaker, labels, confidence and the word count never change,
//! so the normalised word sequence of a run is the same before and after.

use std::time::Instant;

use super::{
    Case, LangHint, Mark, MarkPolicy, PunctOptions, PunctReason, PunctReport, PunctUnit,
    Punctuator, Script, WordLabel,
};
use crate::audio::echo::text::norm;
use crate::error::CoreError;
use crate::models::languages::{is_auto, primary_subtag};
use crate::transcript::{ends_sentence, TranscriptWord};
use crate::types::Channel;

/// A channel whose ASR already marks this many words per 100 or more is not
/// given to the model (`reason: "asr_punctuates"`; Parakeet marks 12.4).
pub const ASR_PUNCTUATES_PER_100: f64 = 8.0;

/// The language a run is punctuated in (plan 16 section 3.8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunLanguage {
    /// The meeting's language: a BCP-47 tag or `auto`.
    pub meeting: String,
    /// For `auto`: the language the runtime detected most often, if any.
    pub detected: Option<String>,
}

impl RunLanguage {
    pub fn new(meeting: impl Into<String>) -> Self {
        Self {
            meeting: meeting.into(),
            detected: None,
        }
    }

    pub fn with_detected(mut self, detected: Option<String>) -> Self {
        self.detected = detected.filter(|d| !d.trim().is_empty() && !is_auto(d));
        self
    }

    /// The primary subtag to punctuate in and where it came from
    /// (`meeting` or `detected`); `None` when only the script can decide.
    fn resolve(&self) -> Option<(String, &'static str)> {
        let m = self.meeting.trim();
        if !m.is_empty() && !is_auto(m) {
            return Some((primary_subtag(m), "meeting"));
        }
        self.detected
            .as_deref()
            .map(|d| (primary_subtag(d), "detected"))
    }
}

/// Punctuates every channel of a run in place and returns the report for
/// `engine_info.punctuation`.
///
/// Each word starts from the ASR's text (`raw_text`, else `text`), so the
/// call can be repeated on words that were punctuated before. Per channel,
/// the words in time order form streams, broken by pauses longer than
/// `opts.max_gap_ms` (and speaker changes for [`PunctUnit::Turn`]); each
/// stream is labelled in one [`Punctuator::label_stream`] call. A word whose
/// display text differs from the ASR's gets `raw_text = Some(old)`; every
/// other word `raw_text = None`. Without a punctuator nothing changes but
/// the ASR's text is restored (`reason: "no_model"`). A model error or a
/// cancellation leaves the whole run with the ASR's text (`reason:
/// "error"`). Never panics on the model's output.
pub fn punctuate_channels(
    channels: &mut [(Channel, Vec<TranscriptWord>)],
    p: Option<&mut dyn Punctuator>,
    opts: &PunctOptions,
    lang: &RunLanguage,
    cancel: &dyn Fn() -> bool,
) -> PunctReport {
    let started = Instant::now();
    for (_, words) in channels.iter_mut() {
        for w in words.iter_mut() {
            if let Some(raw) = w.raw_text.take() {
                w.text = raw;
            }
        }
    }
    let Some(p) = p else {
        return PunctReport::not_applied(PunctReason::NoModel);
    };
    let info = p.descriptor().clone();
    let mut report = PunctReport {
        model: Some(info.name.clone()),
        revision: info.revision.clone(),
        sha256: info.sha256.clone(),
        runtime: (!info.runtime.name.is_empty()).then(|| info.runtime.clone()),
        unit: Some(opts.unit),
        policy: Some(opts.policy),
        threads: Some(opts.threads),
        ..PunctReport::default()
    };
    let language = lang.resolve();
    if let Some((l, source)) = &language {
        report.language = Some(l.clone());
        report.language_source = Some((*source).to_owned());
        if !info.covers(l) || l == "th" {
            report.reason = PunctReason::Language;
            return report;
        }
    }

    let mut totals = Totals::default();
    let mut first_failure: Option<PunctReason> = None;
    let mut applied = false;
    let mut any_words = false;
    let mut script_used: Option<Script> = None;
    for (channel, words) in channels.iter_mut() {
        if words.is_empty() {
            continue;
        }
        any_words = true;
        let rate = asr_marks_per_100(words);
        report
            .asr_marks_per_100
            .insert(channel.as_str().to_owned(), round2(rate));
        totals.words += words.len() as u64;
        let order = time_order(words);
        let streams = split_streams(words, &order, opts);
        let channel_reason = if rate >= ASR_PUNCTUATES_PER_100 {
            for s in &streams {
                let hint = stream_hint(words, s, language.as_ref().map(|l| l.0.clone()));
                let mut texts: Vec<String> = s.iter().map(|&i| words[i].text.clone()).collect();
                sentence_starts(&mut texts, &hint);
                totals.commit(words, s, texts);
            }
            PunctReason::AsrPunctuates
        } else {
            let mut channel_ok = false;
            let mut channel_failure: Option<PunctReason> = None;
            for s in &streams {
                if cancel() {
                    return failed(channels, report, "cancelled");
                }
                let hint = stream_hint(words, s, language.as_ref().map(|l| l.0.clone()));
                if language.is_none() && !script_covered(&info, hint.script) {
                    channel_failure.get_or_insert(PunctReason::Language);
                    continue;
                }
                if hint.script == Script::Thai {
                    channel_failure.get_or_insert(PunctReason::Language);
                    continue;
                }
                let olds: Vec<&str> = s.iter().map(|&i| words[i].text.as_str()).collect();
                let inputs: Vec<String> = olds.iter().map(|o| strip_marks(o)).collect();
                let input_refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
                let labels = match p.label_stream(&input_refs, &hint, cancel) {
                    Ok(l) => l,
                    Err(CoreError::Cancelled) => return failed(channels, report, "cancelled"),
                    Err(e) => return failed(channels, report, &e.to_string()),
                };
                totals.inner_marks_dropped += p.take_stats().inner_marks_dropped;
                if labels.len() != s.len() {
                    log::warn!(
                        "punctuation: {} labels for a stream of {} words; the stream keeps the ASR's text",
                        labels.len(),
                        s.len()
                    );
                    totals.streams_refused += 1;
                    channel_failure.get_or_insert(PunctReason::Invariant);
                    continue;
                }
                if language.is_none() {
                    script_used.get_or_insert(hint.script);
                }
                let bases: Vec<String> = match opts.policy {
                    MarkPolicy::KeepAsr => olds.iter().map(|o| (*o).to_owned()).collect(),
                    MarkPolicy::Replace => inputs.clone(),
                };
                let mut texts: Vec<String> = labels
                    .iter()
                    .enumerate()
                    .map(|(k, label)| {
                        let script = Script::of_word(olds[k]).unwrap_or(hint.script);
                        labelled(&bases[k], &inputs[k], label, script, &hint, opts)
                    })
                    .collect();
                sentence_starts(&mut texts, &hint);
                // Rules 2 and 3, checked on every word at run time: a word
                // that fails keeps the ASR's text.
                for (k, t) in texts.iter_mut().enumerate() {
                    if t != olds[k] && !valid(olds[k], &bases[k], t, &hint) {
                        totals.rejected_words += 1;
                        *t = olds[k].to_owned();
                    }
                }
                totals.commit(words, s, texts);
                channel_ok = true;
            }
            if channel_ok {
                PunctReason::Ok
            } else {
                channel_failure.unwrap_or(PunctReason::Ok)
            }
        };
        if channel_reason == PunctReason::Ok {
            applied = true;
        } else {
            first_failure.get_or_insert(channel_reason);
        }
        report
            .channels
            .insert(channel.as_str().to_owned(), channel_reason);
    }
    if language.is_none() {
        if let Some(s) = script_used {
            report.language = Some(s.as_str().to_owned());
            report.language_source = Some("script".to_owned());
        }
    }
    report.applied = applied || !any_words;
    report.reason = if report.applied {
        PunctReason::Ok
    } else {
        first_failure.unwrap_or(PunctReason::Ok)
    };
    report.words = Some(totals.words);
    report.marks_added = Some(totals.marks_added);
    report.case_upgrades = Some(totals.case_upgrades);
    report.rejected_words = Some(totals.rejected_words);
    report.inner_marks_dropped = Some(totals.inner_marks_dropped);
    report.streams_refused = Some(totals.streams_refused);
    report.wall_s = Some(round2(started.elapsed().as_secs_f64()));
    report
}

/// The ASR's text back on every word, and an error report.
fn failed(
    channels: &mut [(Channel, Vec<TranscriptWord>)],
    mut report: PunctReport,
    message: &str,
) -> PunctReport {
    for (_, words) in channels.iter_mut() {
        for w in words.iter_mut() {
            if let Some(raw) = w.raw_text.take() {
                w.text = raw;
            }
        }
    }
    log::warn!("punctuation failed: {message}");
    report.applied = false;
    report.reason = PunctReason::Error;
    report.message = Some(message.to_owned());
    report.asr_marks_per_100.clear();
    report.channels.clear();
    report
}

/// Counters over a run, and the place where a stream's texts are written.
#[derive(Default)]
struct Totals {
    words: u64,
    marks_added: u64,
    case_upgrades: u64,
    rejected_words: u64,
    inner_marks_dropped: u64,
    streams_refused: u64,
}

impl Totals {
    /// Writes `texts` (one per index of `stream`) into the words, sets
    /// `raw_text` where the text changed and counts marks and capitals.
    fn commit(&mut self, words: &mut [TranscriptWord], stream: &[usize], texts: Vec<String>) {
        for (&i, new) in stream.iter().zip(texts) {
            let w = &mut words[i];
            if new == w.text {
                continue;
            }
            let (marks, raised) = changes(&w.text, &new);
            self.marks_added += marks;
            if raised {
                self.case_upgrades += 1;
            }
            let old = std::mem::replace(&mut w.text, new);
            w.raw_text = Some(old);
        }
    }
}

/// Marks added and whether a letter was raised, from `old` to `new`.
fn changes(old: &str, new: &str) -> (u64, bool) {
    let marks = |s: &str| s.chars().filter(|c| is_mark_char(*c)).count() as i64;
    let added = (marks(new) - marks(old)).max(0) as u64;
    let upper = |s: &str| s.chars().filter(|c| c.is_uppercase()).count();
    (added, upper(new) > upper(old))
}

/// Indices of `words` in time order (stable, so equal start times keep
/// their order).
fn time_order(words: &[TranscriptWord]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| words[i].start_ms);
    order
}

/// Streams of one channel: runs of `order` broken by pauses longer than
/// `max_gap_ms` and, for [`PunctUnit::Turn`], speaker changes.
fn split_streams(
    words: &[TranscriptWord],
    order: &[usize],
    opts: &PunctOptions,
) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    for &i in order {
        if let Some(&prev) = cur.last() {
            let (a, b) = (&words[prev], &words[i]);
            let gap = b.start_ms - a.end_ms;
            let turn = opts.unit == PunctUnit::Turn && a.speaker != b.speaker;
            if gap > opts.max_gap_ms || turn {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(i);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn stream_hint(words: &[TranscriptWord], stream: &[usize], language: Option<String>) -> LangHint {
    LangHint {
        language,
        script: Script::dominant(stream.iter().map(|&i| words[i].text.as_str())),
    }
}

/// For a meeting in `auto` with no detected language: whether the model
/// lists `*` or a language written in `script`.
fn script_covered(info: &super::PunctModelInfo, script: Script) -> bool {
    if script == Script::Thai {
        return false;
    }
    info.any_language() || script.languages().iter().any(|l| info.covers(l))
}

/// Words of a channel whose ASR text ends in a mark, per 100 words.
fn asr_marks_per_100(words: &[TranscriptWord]) -> f64 {
    if words.is_empty() {
        return 0.0;
    }
    let marked = words
        .iter()
        .filter(|w| w.text.trim_end().chars().last().is_some_and(is_mark_char))
        .count();
    marked as f64 * 100.0 / words.len() as f64
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The sentence and clause marks of the allow-list in every script, and
/// the ASR's own (`;` `…`).
fn is_mark_char(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | '?'
            | '!'
            | ':'
            | ';'
            | '…'
            | '。'
            | '，'
            | '、'
            | '？'
            | '！'
            | '：'
            | '।'
            | '॥'
            | '،'
            | '؟'
            | '۔'
            | '\u{037E}'
            | '¿'
            | '¡'
    )
}

/// The allow-list marks every script renders to (what `Replace` strips).
fn is_allow_list_char(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | '?'
            | '!'
            | ':'
            | '-'
            | '。'
            | '，'
            | '、'
            | '？'
            | '！'
            | '：'
            | '।'
            | '،'
            | '؟'
            | '\u{037E}'
    )
}

/// `old` without its trailing allow-list marks; `old` itself when nothing
/// but marks would be left.
fn strip_marks(old: &str) -> String {
    let stripped = old.trim_end_matches(is_allow_list_char);
    if norm(stripped).is_empty() {
        old.to_owned()
    } else {
        stripped.to_owned()
    }
}

/// Whether `mark` may be applied in `language` (plan 16 section 3.4).
fn allowed(opts: &PunctOptions, language: Option<&str>, mark: Mark) -> bool {
    let listed = language
        .and_then(|l| opts.marks_by_language.get(l))
        .or_else(|| opts.marks_by_language.get("*"));
    if let Some(list) = listed {
        return list.contains(&mark);
    }
    match mark {
        Mark::Period | Mark::Comma | Mark::Question | Mark::Exclamation => true,
        Mark::InvertedQuestion | Mark::InvertedExclamation => language == Some("es"),
        Mark::Colon | Mark::Dash => false,
    }
}

/// How `mark` is written in `script` (plan 16 section 3.4); `None` where the
/// script has no such mark (Thai has none).
pub fn render(mark: Mark, script: Script, language: Option<&str>) -> Option<&'static str> {
    use Mark::*;
    let latin = |m: Mark| match m {
        Period => ".",
        Comma => ",",
        Question => "?",
        Exclamation => "!",
        Colon => ":",
        Dash => "-",
        InvertedQuestion => "¿",
        InvertedExclamation => "¡",
    };
    match script {
        Script::Thai => None,
        Script::Latin => Some(latin(mark)),
        _ if mark.is_pre() => None,
        Script::Cyrillic | Script::Armenian | Script::Hebrew | Script::Hangul | Script::Other => {
            Some(latin(mark))
        }
        Script::Greek => Some(match mark {
            Question => "\u{037E}",
            m => latin(m),
        }),
        Script::Devanagari => Some(match mark {
            Period => "।",
            m => latin(m),
        }),
        Script::Arabic => Some(match mark {
            Comma => "،",
            Question => "؟",
            m => latin(m),
        }),
        Script::Han | Script::Hiragana | Script::Katakana => match mark {
            Period => Some("。"),
            Comma => Some(if language == Some("ja") || script != Script::Han {
                "、"
            } else {
                "，"
            }),
            Question => Some("？"),
            Exclamation => Some("！"),
            Colon => Some("："),
            _ => None,
        },
    }
}

/// Whether a lower-case letter may be raised, and to what: only when
/// `to_uppercase` gives exactly one character whose lower case is the
/// original (so ß, ŉ and the Greek final sigma stay), and never i or ı in
/// Turkish or Azerbaijani.
fn raise(c: char, language: Option<&str>) -> Option<char> {
    if !c.is_lowercase() {
        return None;
    }
    if matches!(language, Some("tr" | "az")) && matches!(c, 'i' | 'ı') {
        return None;
    }
    let mut up = c.to_uppercase();
    let u = up.next()?;
    if up.next().is_some() || u == c {
        return None;
    }
    let mut low = u.to_lowercase();
    let l = low.next()?;
    (low.next().is_none() && l == c).then_some(u)
}

/// Raises the first cased letter of `text`, if it is lower case and may be
/// raised. A word that starts with an upper-case letter is left alone.
fn raise_first(text: &str, language: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut done = false;
    for c in text.chars() {
        if !done && (c.is_lowercase() || c.is_uppercase()) {
            done = true;
            if let Some(u) = raise(c, language) {
                out.push(u);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// The display text of one word from its `base`, the `input` the model was
/// given (a prefix of `base`) and its label: case upgrades first, then the
/// marks. Validated by [`valid`] afterwards.
fn labelled(
    base: &str,
    input: &str,
    label: &WordLabel,
    script: Script,
    hint: &LangHint,
    opts: &PunctOptions,
) -> String {
    let language = hint.language.as_deref();
    let chars: Vec<char> = base.chars().collect();
    let n_input = input.chars().count();
    let mut cased: Vec<char> = chars.clone();
    match &label.case {
        Case::Keep => {}
        Case::First => {
            cased = raise_first(base, language).chars().collect();
        }
        Case::Upper => {
            for c in cased.iter_mut() {
                if let Some(u) = raise(*c, language) {
                    *c = u;
                }
            }
        }
        Case::Chars(flags) => {
            if flags.len() == n_input && n_input <= cased.len() {
                for (c, &f) in cased.iter_mut().zip(flags) {
                    if f {
                        if let Some(u) = raise(*c, language) {
                            *c = u;
                        }
                    }
                }
            } else if flags.first() == Some(&true) {
                cased = raise_first(base, language).chars().collect();
            }
        }
    }
    let mut new: String = cased.into_iter().collect();
    let has_text = !norm(base).is_empty();
    if let Some(m) = label.post.filter(|m| !m.is_pre()) {
        let ends_in_punct = base
            .chars()
            .last()
            .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace());
        if has_text && !ends_in_punct && allowed(opts, language, m) {
            if let Some(r) = render(m, script, language) {
                new.push_str(r);
            }
        }
    }
    if let Some(m) = label.pre.filter(|m| m.is_pre()) {
        let starts_with_punct = base
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace());
        if has_text && !starts_with_punct && allowed(opts, language, m) {
            if let Some(r) = render(m, script, language) {
                new.insert_str(0, r);
            }
        }
    }
    new
}

/// Raises the first letter of a stream's first word and of every word
/// after a sentence end, in scripts with case (plan 16 section 3.3 rule 6).
fn sentence_starts(texts: &mut [String], hint: &LangHint) {
    let language = hint.language.as_deref();
    let mut start = true;
    for t in texts.iter_mut() {
        let script = Script::of_word(t).unwrap_or(hint.script);
        if start && script.has_case() {
            *t = raise_first(t, language);
        }
        start = ends_sentence(t);
    }
}

/// The invariants of plan 16 section 3.3 rules 2 and 3 for one word: the
/// echo gate's `norm` is unchanged, and `new` is `base` with only letters
/// raised (never lowered, never changed otherwise) and at most one
/// allow-list mark before and one after.
fn valid(old: &str, base: &str, new: &str, hint: &LangHint) -> bool {
    if norm(new) != norm(old) {
        return false;
    }
    let base: Vec<char> = base.chars().collect();
    let new: Vec<char> = new.chars().collect();
    if new.len() < base.len() || new.len() > base.len() + 2 {
        return false;
    }
    let language = hint.language.as_deref();
    // Find where base starts inside new: after at most one pre-mark.
    for offset in 0..=(new.len() - base.len()).min(1) {
        if offset == 1 && !matches!(new[0], '¿' | '¡') {
            continue;
        }
        let tail = new.len() - offset - base.len();
        if tail > 1 {
            continue;
        }
        let body = &new[offset..offset + base.len()];
        let same = base
            .iter()
            .zip(body)
            .all(|(&b, &n)| n == b || raise(b, language) == Some(n));
        let tail_ok = tail == 0 || is_allow_list_char(new[new.len() - 1]);
        if same && tail_ok {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::punct::fixture::FixturePunctuator;
    use std::collections::BTreeMap;

    fn tw(channel: Channel, start: i64, text: &str, spk: &str) -> TranscriptWord {
        TranscriptWord {
            channel,
            start_ms: start,
            end_ms: start + 300,
            text: text.into(),
            confidence: Some(0.8),
            speaker: spk.into(),
            raw_label: Some(spk.into()),
            raw_text: None,
        }
    }

    /// Words of one channel 400 ms apart.
    fn stream(channel: Channel, text: &str) -> Vec<TranscriptWord> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, t)| tw(channel, i as i64 * 400, t, "spk_0"))
            .collect()
    }

    fn texts(words: &[TranscriptWord]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    fn en() -> RunLanguage {
        RunLanguage::new("en")
    }

    fn run(
        channels: &mut [(Channel, Vec<TranscriptWord>)],
        p: &mut FixturePunctuator,
        opts: &PunctOptions,
        lang: &RunLanguage,
    ) -> PunctReport {
        punctuate_channels(channels, Some(p), opts, lang, &|| false)
    }

    #[test]
    fn periods_commas_and_sentence_starts() {
        let mut ch = vec![(
            Channel::Source,
            stream(
                Channel::Source,
                "so we met today i think it went well right",
            ),
        )];
        let mut p = FixturePunctuator::marks(&[
            ("so", Mark::Comma),
            ("today", Mark::Period),
            ("well", Mark::Comma),
            ("right", Mark::Question),
        ]);
        let before: Vec<TranscriptWord> = ch[0].1.clone();
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(
            texts(&ch[0].1),
            ["So,", "we", "met", "today.", "I", "think", "it", "went", "well,", "right?"]
        );
        assert!(r.applied);
        assert_eq!(r.reason, PunctReason::Ok);
        assert_eq!(r.marks_added, Some(4));
        assert_eq!(r.case_upgrades, Some(2));
        assert_eq!(r.rejected_words, Some(0));
        assert_eq!(r.words, Some(10));
        assert_eq!(r.language.as_deref(), Some("en"));
        assert_eq!(r.channels.get("source"), Some(&PunctReason::Ok));
        // Rule 1: everything but the text is untouched; rule 4: raw_text
        // only where the text changed.
        for (a, b) in ch[0].1.iter().zip(&before) {
            assert_eq!(
                (a.start_ms, a.end_ms, &a.speaker, &a.raw_label, a.confidence),
                (b.start_ms, b.end_ms, &b.speaker, &b.raw_label, b.confidence)
            );
            assert_eq!(norm(&a.text), norm(&b.text));
            if a.text == b.text {
                assert_eq!(a.raw_text, None, "{}", a.text);
            } else {
                assert_eq!(a.raw_text.as_deref(), Some(b.text.as_str()));
            }
        }
        // Run again on the punctuated words: same result, from the ASR's text.
        let r2 = run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(texts(&ch[0].1)[0], "So,");
        assert_eq!(ch[0].1[0].raw_text.as_deref(), Some("so"));
        assert_eq!(r2.marks_added, Some(4));
    }

    #[test]
    fn labels_that_would_break_a_word_are_rejected_and_counted() {
        // Upper on "ßtraße"-like words cannot raise ß; Chars with a wrong
        // length falls back; a label on a word with no letters adds nothing.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "straße 42 ok"))];
        let mut p = FixturePunctuator::per_word(|w| match w {
            "straße" => WordLabel {
                case: Case::Upper,
                ..WordLabel::default()
            },
            "42" => WordLabel::post(Mark::Comma),
            _ => WordLabel::post(Mark::Period),
        });
        let lang = RunLanguage::new("de");
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &lang);
        assert_eq!(texts(&ch[0].1), ["STRAßE", "42,", "ok."]);
        assert_eq!(r.rejected_words, Some(0));
        // The validator refuses changed letters, dropped characters,
        // lowered capitals and disallowed marks.
        let hint = LangHint {
            language: Some("en".into()),
            script: Script::Latin,
        };
        assert!(valid("hello", "hello", "Hello.", &hint));
        assert!(valid("hello", "hello", "¿hello?", &hint));
        assert!(!valid("hello", "hello", "hallo", &hint), "letter changed");
        assert!(!valid("hello", "hello", "hell", &hint), "character dropped");
        assert!(!valid("Paris", "Paris", "paris", &hint), "capital lowered");
        assert!(
            !valid("hello", "hello", "hello;", &hint),
            "mark not allowed"
        );
        assert!(!valid("hello", "hello", "hello..", &hint), "two marks");
        assert!(!valid("hello", "hello", "hello world", &hint), "word added");
        assert!(!valid("ok", "ok", "ok\u{2764}", &hint), "symbol");
    }

    #[test]
    fn a_word_that_fails_the_norm_check_keeps_the_asr_text_and_is_counted() {
        // Upper-casing "οδοσ" gives "ΟΔΟΣ", whose lower case ends in the
        // final sigma: `norm` would change, so the word is refused at run
        // time and keeps the ASR's text; the other words are labelled.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "οδοσ και"))];
        let mut p = FixturePunctuator::per_word(|w| match w {
            "οδοσ" => WordLabel {
                case: Case::Upper,
                post: Some(Mark::Comma),
                ..WordLabel::default()
            },
            _ => WordLabel::post(Mark::Period),
        });
        let r = run(
            &mut ch,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("el"),
        );
        assert_eq!(texts(&ch[0].1), ["οδοσ", "και."]);
        assert_eq!(ch[0].1[0].raw_text, None);
        assert_eq!(r.rejected_words, Some(1));
        assert!(r.applied);
        // Per-character flags: Turkish ı and i are never raised; a flag list
        // of the wrong length falls back to the first letter.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "ılık iyi su"))];
        let mut p = FixturePunctuator::per_word(|w| WordLabel {
            case: Case::Chars(match w {
                "iyi" => vec![true, false, true],
                "ılık" => vec![true, false, false, false],
                _ => vec![true; 4],
            }),
            ..WordLabel::default()
        });
        let r = run(
            &mut ch,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("tr"),
        );
        assert_eq!(texts(&ch[0].1), ["ılık", "iyi", "Su"]);
        assert_eq!(r.rejected_words, Some(0));
        assert_eq!(r.case_upgrades, Some(1));
        // Per-character flags of the right length raise exactly those letters.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "the iphone"))];
        let mut p = FixturePunctuator::per_word(|w| WordLabel {
            case: Case::Chars(if w == "iphone" {
                vec![false, true, false, false, false, false]
            } else {
                vec![false; 3]
            }),
            ..WordLabel::default()
        });
        run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(texts(&ch[0].1), ["The", "iPhone"]);
    }

    #[test]
    fn a_wrong_length_result_refuses_the_stream_without_panicking() {
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "one two three"))];
        let mut p = FixturePunctuator::new(|words, _| {
            Ok(vec![WordLabel::post(Mark::Period); words.len() + 1])
        });
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(texts(&ch[0].1), ["one", "two", "three"]);
        assert!(ch[0].1.iter().all(|w| w.raw_text.is_none()));
        assert!(!r.applied);
        assert_eq!(r.reason, PunctReason::Invariant);
        assert_eq!(r.streams_refused, Some(1));
        // Empty results too.
        let mut empty = FixturePunctuator::new(|_, _| Ok(Vec::new()));
        let r = run(&mut ch, &mut empty, &PunctOptions::default(), &en());
        assert_eq!(r.reason, PunctReason::Invariant);
    }

    /// `text` followed by `n` filler words (`f0` ...), so the ASR's own
    /// marks stay under the 8-per-100 rate.
    fn padded(text: &str, n: usize) -> String {
        let filler: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
        format!("{text} {}", filler.join(" "))
    }

    #[test]
    fn keep_asr_against_replace() {
        // The ASR wrote "done." and "right," (2 of 30 words); the model says
        // "done?" and nothing after "right".
        let words = || stream(Channel::Source, &padded("done. right, so", 27));
        let label = |w: &str| match w {
            "done" => WordLabel::post(Mark::Question),
            "so" => WordLabel::post(Mark::Period),
            _ => WordLabel::KEEP,
        };
        let mut keep = vec![(Channel::Source, words())];
        let mut p = FixturePunctuator::per_word(label);
        let r = run(&mut keep, &mut p, &PunctOptions::default(), &en());
        assert_eq!(r.channels.get("source"), Some(&PunctReason::Ok));
        // KeepAsr: the ASR's marks stay; nothing is appended after them.
        assert_eq!(texts(&keep[0].1)[..3], ["Done.", "Right,", "so."]);
        // The model saw the words without the ASR's marks.
        assert_eq!(p.calls()[0][..3], ["done", "right", "so"]);
        let mut replace = vec![(Channel::Source, words())];
        let opts = PunctOptions {
            policy: MarkPolicy::Replace,
            ..PunctOptions::default()
        };
        let r = run(&mut replace, &mut p, &opts, &en());
        // Replace: the ASR's marks go, the model's are used.
        assert_eq!(texts(&replace[0].1)[..3], ["Done?", "Right", "so."]);
        assert_eq!(replace[0].1[1].raw_text.as_deref(), Some("right,"));
        assert_eq!(r.policy, Some(MarkPolicy::Replace));
        assert_eq!(r.rejected_words, Some(0));
    }

    #[test]
    fn no_mark_after_existing_punctuation_or_on_empty_words() {
        let mut ch = vec![(
            Channel::Source,
            stream(
                Channel::Source,
                &padded("(aside) okay... \"quoted\" - fine", 20),
            ),
        )];
        let mut p = FixturePunctuator::per_word(|w| {
            if w.starts_with('f') && w != "fine" {
                WordLabel::KEEP
            } else {
                WordLabel::post(Mark::Comma)
            }
        });
        run(&mut ch, &mut p, &PunctOptions::default(), &en());
        // The stream's first word starts with a capital; a mark only on the
        // word without punctuation; the ASR's "..." ends a sentence.
        assert_eq!(
            texts(&ch[0].1)[..5],
            ["(Aside)", "okay...", "\"Quoted\"", "-", "fine,"]
        );
    }

    #[test]
    fn sentence_start_casing_by_script() {
        let lang = RunLanguage::new("auto");
        let mut p = FixturePunctuator::marks(&[("один", Mark::Period), ("hello", Mark::Period)]);
        // Latin and Cyrillic get capitals; ASR capitals are kept.
        let mut ch = vec![
            (Channel::Mic, stream(Channel::Mic, "hello there NASA")),
            (Channel::Loopback, stream(Channel::Loopback, "один два")),
        ];
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &lang);
        assert_eq!(texts(&ch[0].1), ["Hello.", "There", "NASA"]);
        assert_eq!(texts(&ch[1].1), ["Один.", "Два"]);
        assert_eq!(r.language_source.as_deref(), Some("script"));
        // Han has no case: only the marks.
        let mut zh = vec![(Channel::Source, stream(Channel::Source, "你好 世界"))];
        let mut pz = FixturePunctuator::marks(&[("你好", Mark::Comma), ("世界", Mark::Period)]);
        run(
            &mut zh,
            &mut pz,
            &PunctOptions::default(),
            &RunLanguage::new("zh-CN"),
        );
        assert_eq!(texts(&zh[0].1), ["你好，", "世界。"]);
        let mut ja = vec![(Channel::Source, stream(Channel::Source, "これ は"))];
        let mut pj = FixturePunctuator::marks(&[("これ", Mark::Comma), ("は", Mark::Question)]);
        run(
            &mut ja,
            &mut pj,
            &PunctOptions::default(),
            &RunLanguage::new("ja-JP"),
        );
        assert_eq!(texts(&ja[0].1), ["これ、", "は？"]);
    }

    #[test]
    fn turkish_i_and_eszett_are_never_raised() {
        let mut p = FixturePunctuator::marks(&[("evet", Mark::Period)]);
        let mut tr = vec![(Channel::Source, stream(Channel::Source, "evet iyi ılık"))];
        // A capital after every word: the stream start and after "evet.".
        run(
            &mut tr,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("tr"),
        );
        assert_eq!(texts(&tr[0].1), ["Evet.", "iyi", "ılık"]);
        let mut de = vec![(Channel::Source, stream(Channel::Source, "ßo gut"))];
        let mut pd = FixturePunctuator::per_word(|_| WordLabel {
            case: Case::Upper,
            ..WordLabel::default()
        });
        run(
            &mut de,
            &mut pd,
            &PunctOptions::default(),
            &RunLanguage::new("de"),
        );
        assert_eq!(texts(&de[0].1), ["ßO", "GUT"]);
        // The raise rule itself.
        assert_eq!(raise('a', None), Some('A'));
        assert_eq!(raise('ß', None), None);
        assert_eq!(raise('ŉ', None), None);
        assert_eq!(raise('ς', None), None, "Greek final sigma");
        assert_eq!(raise('σ', None), Some('Σ'));
        assert_eq!(raise('i', Some("tr")), None);
        assert_eq!(raise('ı', Some("tr")), None);
        assert_eq!(raise('i', Some("en")), Some('I'));
        assert_eq!(raise('A', None), None);
        assert_eq!(raise('1', None), None);
    }

    #[test]
    fn hindi_gets_the_danda_and_arabic_its_marks() {
        let mut hi = vec![(
            Channel::Source,
            stream(Channel::Source, "नमस्ते दोस्तों आप कैसे हैं"),
        )];
        let mut p = FixturePunctuator::marks(&[("दोस्तों", Mark::Period), ("हैं", Mark::Question)]);
        run(
            &mut hi,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("hi"),
        );
        assert_eq!(texts(&hi[0].1), ["नमस्ते", "दोस्तों।", "आप", "कैसे", "हैं?"]);
        let mut ar = vec![(Channel::Source, stream(Channel::Source, "نعم كيف حالك"))];
        let mut pa = FixturePunctuator::marks(&[("نعم", Mark::Comma), ("حالك", Mark::Question)]);
        run(
            &mut ar,
            &mut pa,
            &PunctOptions::default(),
            &RunLanguage::new("ar"),
        );
        assert_eq!(texts(&ar[0].1), ["نعم،", "كيف", "حالك؟"]);
        // Greek question mark.
        let mut el = vec![(Channel::Source, stream(Channel::Source, "τι κάνεις"))];
        let mut pe = FixturePunctuator::marks(&[("κάνεις", Mark::Question)]);
        run(
            &mut el,
            &mut pe,
            &PunctOptions::default(),
            &RunLanguage::new("el"),
        );
        assert_eq!(texts(&el[0].1), ["Τι", "κάνεις\u{037E}"]);
        assert!(ends_sentence(&el[0].1[1].text));
    }

    #[test]
    fn spanish_pre_marks_and_marks_by_language() {
        let mut es = vec![(Channel::Source, stream(Channel::Source, "qué tal"))];
        let mut p = FixturePunctuator::per_word(|w| match w {
            "qué" => WordLabel {
                pre: Some(Mark::InvertedQuestion),
                ..WordLabel::default()
            },
            _ => WordLabel::post(Mark::Question),
        });
        run(
            &mut es,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("es-US"),
        );
        assert_eq!(texts(&es[0].1), ["¿Qué", "tal?"]);
        // Not in English.
        let mut en_ch = vec![(Channel::Source, stream(Channel::Source, "qué tal"))];
        run(&mut en_ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(texts(&en_ch[0].1), ["Qué", "tal?"]);
        // Colon only where listed; a listed language replaces the defaults.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "note this"))];
        let mut pc = FixturePunctuator::marks(&[("note", Mark::Colon), ("this", Mark::Period)]);
        run(&mut ch, &mut pc, &PunctOptions::default(), &en());
        assert_eq!(texts(&ch[0].1), ["Note", "this."]);
        let opts = PunctOptions {
            marks_by_language: BTreeMap::from([("en".to_owned(), vec![Mark::Colon])]),
            ..PunctOptions::default()
        };
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "note this"))];
        run(&mut ch, &mut pc, &opts, &en());
        assert_eq!(texts(&ch[0].1), ["Note:", "this"]);
    }

    #[test]
    fn a_channel_the_asr_punctuates_is_left_to_the_casing_rule() {
        // Mic: 2 of 20 words marked by the ASR (10 per 100); loopback: none.
        let mut mic_text: Vec<String> = (0..20).map(|i| format!("w{i}")).collect();
        mic_text[4] = "stop.".into();
        mic_text[9] = "and,".into();
        let mic = stream(Channel::Mic, &mic_text.join(" "));
        let lb = stream(Channel::Loopback, "yes we can");
        let mut ch = vec![(Channel::Mic, mic), (Channel::Loopback, lb)];
        let mut p = FixturePunctuator::marks(&[("can", Mark::Period)]);
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(r.channels.get("mic"), Some(&PunctReason::AsrPunctuates));
        assert_eq!(r.channels.get("loopback"), Some(&PunctReason::Ok));
        assert_eq!(r.asr_marks_per_100.get("mic"), Some(&10.0));
        assert_eq!(r.asr_marks_per_100.get("loopback"), Some(&0.0));
        assert!(r.applied);
        // The model only saw the loopback.
        assert_eq!(p.calls().len(), 1);
        assert_eq!(texts(&ch[1].1), ["Yes", "we", "can."]);
        // The mic got capitals at its start and after "stop." only.
        assert_eq!(ch[0].1[0].text, "W0");
        assert_eq!(ch[0].1[5].text, "W5");
        assert_eq!(ch[0].1[10].text, "w10");
        assert_eq!(ch[0].1[4].text, "stop.");
        // Both channels punctuated by their ASR: not applied.
        let mut both = vec![(Channel::Mic, ch[0].1.clone())];
        let r = run(&mut both, &mut p, &PunctOptions::default(), &en());
        assert!(!r.applied);
        assert_eq!(r.reason, PunctReason::AsrPunctuates);
    }

    #[test]
    fn gaps_break_streams_and_turns_are_optional() {
        let mut words = stream(Channel::Source, "one two three four");
        // A 2 s pause before "three"; "four" is another speaker.
        for w in &mut words[2..] {
            w.start_ms += 2_000;
            w.end_ms += 2_000;
        }
        words[3].speaker = "spk_1".into();
        let mut p = FixturePunctuator::per_word(|_| WordLabel::KEEP);
        let mut ch = vec![(Channel::Source, words.clone())];
        run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(p.calls(), [vec!["one", "two"], vec!["three", "four"]]);
        // Every stream starts with a capital, so a segment that starts after
        // a pause starts with one.
        assert_eq!(texts(&ch[0].1), ["One", "two", "Three", "four"]);
        let mut p = FixturePunctuator::per_word(|_| WordLabel::KEEP);
        let opts = PunctOptions {
            unit: PunctUnit::Turn,
            ..PunctOptions::default()
        };
        let mut ch = vec![(Channel::Source, words)];
        run(&mut ch, &mut p, &opts, &en());
        assert_eq!(p.calls(), [vec!["one", "two"], vec!["three"], vec!["four"]]);
    }

    #[test]
    fn words_out_of_time_order_keep_their_positions() {
        let mut words = stream(Channel::Source, "b a");
        words[0].start_ms = 500;
        words[0].end_ms = 800;
        words[1].start_ms = 0;
        words[1].end_ms = 300;
        let mut p = FixturePunctuator::marks(&[("b", Mark::Period)]);
        let mut ch = vec![(Channel::Source, words)];
        run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert_eq!(p.calls(), [vec!["a", "b"]]);
        assert_eq!(texts(&ch[0].1), ["b.", "A"]);
    }

    #[test]
    fn languages_the_model_does_not_cover() {
        let mut p = FixturePunctuator::marks(&[("x", Mark::Period)]).languages(&["en", "de"]);
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "chào bạn"))];
        let r = run(
            &mut ch,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("vi-VN"),
        );
        assert!(!r.applied);
        assert_eq!(r.reason, PunctReason::Language);
        assert_eq!(r.language.as_deref(), Some("vi"));
        assert_eq!(r.model.as_deref(), Some("fixture"));
        assert_eq!(texts(&ch[0].1), ["chào", "bạn"]);
        assert!(p.calls().is_empty());
        // Thai is never punctuated, even by a model that lists `*`.
        let mut any = FixturePunctuator::marks(&[]);
        let mut th = vec![(Channel::Source, stream(Channel::Source, "สวัสดี ครับ"))];
        let r = run(
            &mut th,
            &mut any,
            &PunctOptions::default(),
            &RunLanguage::new("th"),
        );
        assert_eq!(r.reason, PunctReason::Language);
        let r = run(
            &mut th,
            &mut any,
            &PunctOptions::default(),
            &RunLanguage::new("auto"),
        );
        assert_eq!(r.reason, PunctReason::Language);
        // Auto with a detected language uses it.
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "hallo welt"))];
        let lang = RunLanguage::new("auto").with_detected(Some("de-DE".into()));
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &lang);
        assert!(r.applied);
        assert_eq!(
            (r.language.as_deref(), r.language_source.as_deref()),
            (Some("de"), Some("detected"))
        );
        // Auto without one: a Cyrillic stream is not covered by en + de.
        let mut ru = vec![(Channel::Source, stream(Channel::Source, "привет мир"))];
        let r = run(
            &mut ru,
            &mut p,
            &PunctOptions::default(),
            &RunLanguage::new("auto"),
        );
        assert_eq!(r.reason, PunctReason::Language);
    }

    #[test]
    fn without_a_punctuator_or_on_errors_the_asr_text_stays() {
        let mut ch = vec![(Channel::Source, stream(Channel::Source, "hi there"))];
        ch[0].1[0].text = "Hi,".into();
        ch[0].1[0].raw_text = Some("hi".into());
        let r = punctuate_channels(&mut ch, None, &PunctOptions::default(), &en(), &|| false);
        assert_eq!(r, PunctReport::not_applied(PunctReason::NoModel));
        assert_eq!(texts(&ch[0].1), ["hi", "there"]);
        assert!(ch[0].1.iter().all(|w| w.raw_text.is_none()));
        let mut bad = FixturePunctuator::new(|_, _| Err(CoreError::Models("boom".into())));
        let r = run(&mut ch, &mut bad, &PunctOptions::default(), &en());
        assert_eq!(r.reason, PunctReason::Error);
        assert!(r.message.as_deref().unwrap().contains("boom"));
        assert_eq!(texts(&ch[0].1), ["hi", "there"]);
        // Cancelled between streams.
        let mut p = FixturePunctuator::marks(&[("hi", Mark::Period)]);
        let r = punctuate_channels(
            &mut ch,
            Some(&mut p),
            &PunctOptions::default(),
            &en(),
            &|| true,
        );
        assert_eq!(r.reason, PunctReason::Error);
        assert_eq!(r.message.as_deref(), Some("cancelled"));
        assert_eq!(texts(&ch[0].1), ["hi", "there"]);
    }

    #[test]
    fn an_empty_run_counts_as_applied() {
        let mut ch: Vec<(Channel, Vec<TranscriptWord>)> = vec![(Channel::Mic, Vec::new())];
        let mut p = FixturePunctuator::marks(&[]);
        let r = run(&mut ch, &mut p, &PunctOptions::default(), &en());
        assert!(r.applied);
        assert_eq!(r.words, Some(0));
        assert!(r.channels.is_empty());
    }

    #[test]
    fn the_normalised_word_sequence_never_changes() {
        // Every label on every word of a mixed stream: norm per word holds.
        let text = "hello wORLD i'm 3.5 ok... ¿qué? straße ılık नमस्ते 你好 x";
        let mut ch = vec![(Channel::Source, stream(Channel::Source, text))];
        let before: Vec<String> = ch[0].1.iter().map(|w| norm(&w.text)).collect();
        for mark in Mark::ALL {
            for case in [
                Case::Keep,
                Case::First,
                Case::Upper,
                Case::Chars(vec![true; 3]),
            ] {
                for lang in ["en", "es", "tr", "hi", "zh", "auto"] {
                    let opts = PunctOptions {
                        marks_by_language: BTreeMap::from([("*".to_owned(), Mark::ALL.to_vec())]),
                        ..PunctOptions::default()
                    };
                    let case = case.clone();
                    let mut p = FixturePunctuator::per_word(move |_| WordLabel {
                        pre: Some(mark),
                        post: Some(mark),
                        case: case.clone(),
                    });
                    let r = run(&mut ch, &mut p, &opts, &RunLanguage::new(lang));
                    let after: Vec<String> = ch[0].1.iter().map(|w| norm(&w.text)).collect();
                    assert_eq!(after, before, "{mark:?} {lang}");
                    assert_eq!(r.rejected_words, Some(0), "{mark:?} {lang}");
                    for w in &ch[0].1 {
                        let old = w.raw_text.as_deref().unwrap_or(&w.text);
                        assert!(w.text.chars().count() <= old.chars().count() + 2);
                    }
                }
            }
        }
    }
}
