//! Cross-channel text matching, the echo gate's first layer (plan 15
//! section 9, item 1, as revised in section 9.1; research 16 section 2).
//!
//! The far end's transcript is known: an echo copy in "me" repeats loopback
//! words that started a moment earlier, all with about the same delay. A
//! mic word matches a loopback token of the same normalised text ([`norm`])
//! whose start lies inside the causal window ([`TextWindow`]: the mic copy
//! from [`TextWindow::lo_ms`] before to 1.0 s after the loopback word; the
//! mic copy is stamped 0.3 to 0.4 s late at the median in the post-pass, up
//! to +0.65 s at p90, and the loopback's own stamps can be late too). Within
//! any [`RUN_WORDS`] consecutive mic words spanning at most [`RUN_SPAN_MS`],
//! the words that match distinct loopback tokens with lags no more than
//! [`TextWindow::spread_ms`] apart ([`LAG_SPREAD_MS`]) form a run
//! ([`runs`]):
//!
//! - three or more such words are echo on text alone ([`Runs::run3`]): a
//!   mis-transcribed middle word or a second far-end speaker's word in
//!   between does not break the run, the delay agreement keeps chance low;
//! - two, when one of them is a content word ([`is_content`]), are echo only
//!   with an acoustic tie-breaker ([`Runs::run2`]; [`super::layers`]);
//! - a single word never counts (chance is 2 to 4 percent: the owner's
//!   "yeah" and "okay").

use std::collections::HashMap;

/// The causal window of a match: the mic word's start minus the loopback
/// word's start, from `lo_ms` (negative: the mic copy first) to `hi_ms`;
/// a run's lags lie within `spread_ms` of each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextWindow {
    pub lo_ms: i64,
    pub hi_ms: i64,
    pub spread_ms: i64,
}

impl TextWindow {
    /// −0.15 to +1.0 s, lags within [`LAG_SPREAD_MS`]. The lower edge was
    /// measured at −0.5 s (research 16) and −0.15 s (the external review):
    /// neither takes any of the owner's words on meetings 12, 13 and 14,
    /// live or post-pass, and −0.15 s halves the chance matches on meeting
    /// 12's live words (6 words of 14,627 with the mic moved 45 s, against
    /// 12) for 2 percent fewer duplicates found there
    /// (docs/eval/2026-10-21-echo-round2.md).
    pub const DEFAULT: Self = Self {
        lo_ms: -150,
        hi_ms: 1_000,
        spread_ms: LAG_SPREAD_MS,
    };
}

impl Default for TextWindow {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Consecutive mic words a run is looked for in ...
pub const RUN_WORDS: usize = 4;
/// ... spanning at most this (first to last start).
pub const RUN_SPAN_MS: i64 = 2_000;
/// The lags of a run's matches lie within this of each other: 500 ms
/// (plan 15 section 9.2; round two's 300 ms missed the canceller's
/// residue, whose garbled copies are stamped up to 0.7 s late).
pub const LAG_SPREAD_MS: i64 = 500;
/// A content word has at least this many characters ...
pub const CONTENT_CHARS: usize = 5;
/// ... and is not one of these.
const STOP: &[&str] = &[
    "about",
    "above",
    "after",
    "again",
    "against",
    "always",
    "among",
    "another",
    "anything",
    "around",
    "because",
    "before",
    "being",
    "below",
    "between",
    "could",
    "doing",
    "during",
    "every",
    "everything",
    "gonna",
    "having",
    "maybe",
    "might",
    "never",
    "nothing",
    "other",
    "others",
    "really",
    "right",
    "should",
    "since",
    "something",
    "still",
    "their",
    "there",
    "these",
    "thing",
    "things",
    "think",
    "those",
    "though",
    "through",
    "under",
    "until",
    "where",
    "which",
    "while",
    "would",
    "yeahs",
];

/// Lower case, punctuation stripped (letters, digits and spaces kept):
/// numbers and contractions stay as the runtime writes them, the same on
/// both channels.
pub fn norm(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A content word: [`CONTENT_CHARS`] characters or more and not a common
/// function word.
pub fn is_content(n: &str) -> bool {
    n.chars().count() >= CONTENT_CHARS && !STOP.contains(&n)
}

/// The loopback words in time order, indexed by text.
#[derive(Debug, Clone, Default)]
pub struct FarIndex {
    /// Start (ms) of every word, in time order.
    starts: Vec<i64>,
    /// Positions in `starts` per text, in time order.
    by_text: HashMap<String, Vec<usize>>,
}

impl FarIndex {
    /// From `(start_ms, normalised text)` in any order.
    pub fn new(words: impl IntoIterator<Item = (i64, String)>) -> Self {
        let mut words: Vec<(i64, String)> = words.into_iter().collect();
        words.sort_by_key(|w| w.0);
        let mut by_text: HashMap<String, Vec<usize>> = HashMap::new();
        for (j, w) in words.iter().enumerate() {
            if !w.1.is_empty() {
                by_text.entry(w.1.clone()).or_default().push(j);
            }
        }
        Self {
            starts: words.iter().map(|w| w.0).collect(),
            by_text,
        }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// `(lag, token)` of every loopback token with text `n` whose start lies
    /// in `window` for a mic word starting at `t` (ms).
    fn matches(&self, n: &str, t: i64, window: TextWindow) -> Vec<(i64, usize)> {
        let Some(list) = self.by_text.get(n) else {
            return Vec::new();
        };
        let (lo, hi) = (t - window.hi_ms, t - window.lo_ms);
        let k = list.partition_point(|&j| self.starts[j] < lo);
        list[k..]
            .iter()
            .take_while(|&&j| self.starts[j] <= hi)
            .map(|&j| (t - self.starts[j], j))
            .collect()
    }
}

/// Which mic words belong to a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runs {
    /// In a run of three or more matches (echo on text alone).
    pub run3: Vec<bool>,
    /// In a run of two with a content word, and in no longer one: echo only
    /// with an acoustic tie-breaker.
    pub run2: Vec<bool>,
}

impl Runs {
    /// In any run.
    pub fn any(&self, i: usize) -> bool {
        self.run3[i] || self.run2[i]
    }

    pub fn count3(&self) -> usize {
        self.run3.iter().filter(|v| **v).count()
    }

    pub fn count2(&self) -> usize {
        self.run2.iter().filter(|v| **v).count()
    }
}

/// The runs among mic words given in time order as `(start_ms, normalised
/// text)` (see the module docs). Empty texts never match.
pub fn runs(mic: &[(i64, &str)], far: &FarIndex, window: TextWindow) -> Runs {
    let n = mic.len();
    let cands: Vec<Vec<(i64, usize)>> = mic
        .iter()
        .map(|&(t, s)| {
            if s.is_empty() {
                Vec::new()
            } else {
                far.matches(s, t, window)
            }
        })
        .collect();
    let mut out = Runs {
        run3: vec![false; n],
        run2: vec![false; n],
    };
    let mut chosen: Vec<usize> = Vec::with_capacity(RUN_WORDS);
    let mut used: Vec<usize> = Vec::with_capacity(RUN_WORDS);
    for i in 0..n {
        if cands[i].is_empty() {
            continue;
        }
        let mut end = i;
        while end + 1 < n && end + 1 < i + RUN_WORDS && mic[end + 1].0 - mic[i].0 <= RUN_SPAN_MS {
            end += 1;
        }
        if end == i {
            continue;
        }
        // Every lag in the window as the run's smallest.
        let span = &cands[i..=end];
        for &(base, _) in span.iter().flatten() {
            chosen.clear();
            used.clear();
            for (k, c) in (i..=end).zip(span) {
                if let Some(&(_, j)) = c
                    .iter()
                    .filter(|(lag, j)| {
                        *lag >= base && *lag - base <= window.spread_ms && !used.contains(j)
                    })
                    .min_by_key(|(lag, _)| *lag - base)
                {
                    chosen.push(k);
                    used.push(j);
                }
            }
            if chosen.len() >= 3 {
                for &k in &chosen {
                    out.run3[k] = true;
                }
            } else if chosen.len() == 2 && chosen.iter().any(|&k| is_content(mic[k].1)) {
                for &k in &chosen {
                    out.run2[k] = true;
                }
            }
        }
    }
    for i in 0..n {
        out.run2[i] &= !out.run3[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn far(words: &[(i64, &str)]) -> FarIndex {
        FarIndex::new(words.iter().map(|(t, w)| (*t, norm(w))))
    }

    fn mic<'a>(words: &[(i64, &'a str)]) -> Vec<(i64, &'a str)> {
        words.to_vec()
    }

    const W: TextWindow = TextWindow::DEFAULT;

    #[test]
    fn norm_strips_punctuation_and_case() {
        assert_eq!(norm("Don't,"), "dont");
        assert_eq!(norm(" Okay. "), "okay");
        assert_eq!(norm("42%"), "42");
        assert_eq!(norm("..."), "");
        assert!(is_content("starlink") && !is_content("there") && !is_content("yeah"));
    }

    #[test]
    fn a_late_copy_of_three_words_matches_and_a_mistimed_one_does_not() {
        let f = far(&[
            (1_000, "we"),
            (1_200, "need"),
            (1_400, "more"),
            (1_600, "time"),
        ]);
        let r = runs(
            &mic(&[
                (1_400, "we"),
                (1_600, "need"),
                (1_800, "more"),
                (2_000, "time"),
            ]),
            &f,
            W,
        );
        assert_eq!(r.run3, [true; 4]);
        // 1.2 s late: outside the window.
        let r = runs(
            &mic(&[(2_200, "we"), (2_400, "need"), (2_600, "more")]),
            &f,
            W,
        );
        assert_eq!(r.run3, [false; 3]);
        // The mic first by 0.1 s: inside; by 0.3 s: outside.
        let r = runs(
            &mic(&[(900, "we"), (1_100, "need"), (1_300, "more")]),
            &f,
            W,
        );
        assert_eq!(r.run3, [true; 3]);
        let r = runs(&mic(&[(700, "we"), (900, "need"), (1_100, "more")]), &f, W);
        assert_eq!(r.run3, [false; 3]);
        // The lags disagree by more than 0.5 s.
        let r = runs(
            &mic(&[(1_100, "we"), (1_800, "need"), (2_300, "more")]),
            &f,
            W,
        );
        assert_eq!(r.run3, [false; 3]);
        // By 0.4 s: a run with the 500 ms spread, none with round two's 300.
        let late = mic(&[(1_100, "we"), (1_500, "need"), (1_900, "more")]);
        assert_eq!(runs(&late, &f, W).run3, [true; 3]);
        let narrow = TextWindow {
            spread_ms: 300,
            ..W
        };
        assert_eq!(runs(&late, &f, narrow).run3, [false; 3]);
    }

    #[test]
    fn a_misheard_middle_word_does_not_break_the_run() {
        let f = far(&[
            (1_000, "we"),
            (1_200, "need"),
            (1_400, "more"),
            (1_600, "time"),
        ]);
        let r = runs(
            &mic(&[
                (1_300, "we"),
                (1_500, "kneed"),
                (1_700, "more"),
                (1_900, "time"),
            ]),
            &f,
            W,
        );
        assert_eq!(r.run3, [true, false, true, true]);
    }

    #[test]
    fn two_word_runs_need_a_content_word_and_single_words_never_match() {
        let f = far(&[
            (1_000, "yeah"),
            (1_200, "right"),
            (5_000, "starlink"),
            (5_300, "coverage"),
            (9_000, "okay"),
        ]);
        let r = runs(
            &mic(&[
                (1_300, "yeah"),
                (1_500, "right"),
                (5_300, "starlink"),
                (5_600, "coverage"),
                (9_200, "okay"),
            ]),
            &f,
            W,
        );
        assert_eq!(r.run3, [false; 5]);
        assert_eq!(r.run2, [false, false, true, true, false]);
    }

    #[test]
    fn one_loopback_token_serves_one_mic_word() {
        let f = far(&[(1_000, "the"), (1_200, "plan")]);
        let r = runs(
            &mic(&[(1_300, "the"), (1_350, "the"), (1_500, "plan")]),
            &f,
            W,
        );
        assert_eq!(r.run3, [false; 3]);
    }
}
