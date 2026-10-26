//! Windows over a stream of tokens (docs/plans/16 section 3.2), generic over
//! the tokenizer ([`TokenCounter`]) and the model (a closure), so the
//! windowing is unit-tested without either.
//!
//! A stream that fits the model's token limit is one window. A longer one
//! is cut into cores of at most `max_tokens - specials - 2 * context`
//! tokens, ending at word ends where possible, each with up to `context`
//! tokens on both sides whose outputs are discarded. The cores partition
//! the stream, so every token's output comes from exactly one window, and
//! a word's label from the window whose core holds its last token. Windows
//! are sorted by length and run `batch` at a time, so padding stays small.

use crate::error::{CoreError, Result};

/// A stream after tokenization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Encoded {
    /// Token ids, special tokens excluded.
    pub ids: Vec<i64>,
    /// The word each token belongs to (non-decreasing).
    pub word_of: Vec<usize>,
    /// The token ends exactly where its word ends.
    pub ends_word: Vec<bool>,
    /// Each token's characters as the tokenizer normalised them (the
    /// SentencePiece word marker `▁` removed), for per-character casing.
    pub pieces: Vec<String>,
}

impl Encoded {
    /// The token range of every word (`start..end`, empty for a word that
    /// got no token), for `words` words.
    pub fn word_ranges(&self, words: usize) -> Vec<std::ops::Range<usize>> {
        let mut out = vec![0..0; words];
        for (k, &w) in self.word_of.iter().enumerate() {
            if let Some(r) = out.get_mut(w) {
                if r.start == r.end {
                    *r = k..k + 1;
                } else {
                    r.end = k + 1;
                }
            }
        }
        out
    }
}

/// Turns the words of one stream into tokens.
pub trait TokenCounter {
    /// Tokens of `words` joined by `sep` (`" "`, or `""` for scripts
    /// written without spaces), each mapped to its word.
    fn encode(&self, words: &[&str], sep: &str) -> Result<Encoded>;
}

/// How streams are cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowOpts {
    /// Tokens per window, special tokens included.
    pub max_tokens: usize,
    /// Special tokens the model adds to every window (BOS, EOS).
    pub specials: usize,
    /// Context tokens on each side.
    pub context: usize,
    /// Windows per model call.
    pub batch: usize,
}

impl WindowOpts {
    /// Tokens a window holds besides its special tokens.
    pub fn room(&self) -> usize {
        self.max_tokens.saturating_sub(self.specials).max(1)
    }

    /// Tokens whose outputs a window keeps.
    pub fn core(&self) -> usize {
        self.room().saturating_sub(2 * self.context).max(1)
    }
}

/// One window over a stream's tokens: `left..right` goes to the model, the
/// outputs of `start..end` are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub left: usize,
    pub start: usize,
    pub end: usize,
    pub right: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.right - self.left
    }

    pub fn is_empty(&self) -> bool {
        self.right == self.left
    }
}

/// The windows over a stream whose tokens belong to the words `word_of`.
pub fn spans(word_of: &[usize], opts: &WindowOpts) -> Vec<Span> {
    let n = word_of.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= opts.room() {
        return vec![Span {
            left: 0,
            start: 0,
            end: n,
            right: n,
        }];
    }
    let core = opts.core();
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = (start + core).min(n);
        if end < n {
            // Back off to the last word end inside the core; a single word
            // longer than the core is cut where the core ends.
            let mut j = end;
            while j > start && word_of[j - 1] == word_of[j] {
                j -= 1;
            }
            if j > start {
                end = j;
            }
        }
        out.push(Span {
            left: start.saturating_sub(opts.context),
            start,
            end,
            right: (end + opts.context).min(n),
        });
        start = end;
    }
    out
}

/// Runs `infer` over every window of every stream and returns each token's
/// output, taken from the window whose core holds it. `infer` gets a batch
/// of token slices (special tokens not added) and returns one output per
/// token of each. `cancel` is polled before every batch.
pub fn run_windows<R: Clone + Default>(
    streams: &[&Encoded],
    opts: &WindowOpts,
    cancel: &dyn Fn() -> bool,
    infer: &mut dyn FnMut(&[&[i64]]) -> Result<Vec<Vec<R>>>,
) -> Result<Vec<Vec<R>>> {
    let mut windows: Vec<(usize, Span)> = Vec::new();
    for (s, e) in streams.iter().enumerate() {
        windows.extend(spans(&e.word_of, opts).into_iter().map(|w| (s, w)));
    }
    windows.sort_by_key(|(_, w)| w.len());
    let mut out: Vec<Vec<R>> = streams
        .iter()
        .map(|e| vec![R::default(); e.ids.len()])
        .collect();
    for chunk in windows.chunks(opts.batch.max(1)) {
        if cancel() {
            return Err(CoreError::Cancelled);
        }
        let inputs: Vec<&[i64]> = chunk
            .iter()
            .map(|(s, w)| &streams[*s].ids[w.left..w.right])
            .collect();
        let results = infer(&inputs)?;
        if results.len() != chunk.len() {
            return Err(CoreError::Models(format!(
                "punctuation: the model returned {} windows for {}",
                results.len(),
                chunk.len()
            )));
        }
        for ((s, w), r) in chunk.iter().zip(results) {
            if r.len() != w.len() {
                return Err(CoreError::Models(format!(
                    "punctuation: the model returned {} positions for a window of {} tokens",
                    r.len(),
                    w.len()
                )));
            }
            for k in w.start..w.end {
                out[*s][k] = r[k - w.left].clone();
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tokenizer that cuts every word into pieces of three characters,
    /// with ids that say which word and piece a token is.
    pub(crate) struct Fake;

    impl TokenCounter for Fake {
        fn encode(&self, words: &[&str], _sep: &str) -> Result<Encoded> {
            let mut e = Encoded::default();
            for (w, word) in words.iter().enumerate() {
                let chars: Vec<char> = word.chars().collect();
                let pieces: Vec<String> = chars.chunks(3).map(|c| c.iter().collect()).collect();
                let n = pieces.len();
                for (p, piece) in pieces.into_iter().enumerate() {
                    e.ids.push((w * 1_000 + p) as i64);
                    e.word_of.push(w);
                    e.ends_word.push(p + 1 == n);
                    e.pieces.push(piece);
                }
            }
            Ok(e)
        }
    }

    fn words(n: usize) -> Vec<String> {
        // 1 to 4 pieces per word.
        (0..n)
            .map(|i| "abcdefghijkl"[..1 + (i * 7) % 12].to_owned())
            .collect()
    }

    #[test]
    fn a_long_stream_labels_every_word_exactly_once() {
        let owned = words(1_200);
        let ws: Vec<&str> = owned.iter().map(String::as_str).collect();
        let e = Fake.encode(&ws, " ").unwrap();
        assert!(e.ids.len() > 2_000, "{}", e.ids.len());
        let opts = WindowOpts {
            max_tokens: 64,
            specials: 2,
            context: 8,
            batch: 4,
        };
        let sp = spans(&e.word_of, &opts);
        // The cores partition the stream, at word ends, within the limit.
        assert_eq!(sp[0].start, 0);
        assert_eq!(sp.last().unwrap().end, e.ids.len());
        for pair in sp.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
            assert_ne!(e.word_of[pair[0].end - 1], e.word_of[pair[0].end]);
        }
        assert!(sp
            .iter()
            .all(|w| w.len() + opts.specials <= opts.max_tokens));
        assert!(sp.iter().all(|w| w.end - w.start <= opts.core()));

        // Each word's label comes from exactly one window: count, per word,
        // the windows whose kept outputs include its last token.
        let mut calls = 0;
        let mut seen = vec![0u32; ws.len()];
        let out = run_windows(&[&e], &opts, &|| false, &mut |batch| {
            calls += 1;
            assert!(batch.len() <= opts.batch);
            Ok(batch.iter().map(|ids| ids.to_vec()).collect())
        })
        .unwrap();
        for w in &sp {
            for k in w.start..w.end {
                if e.ends_word[k] {
                    seen[e.word_of[k]] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&c| c == 1), "{seen:?}");
        // Every token got the output of its own position.
        assert_eq!(out[0], e.ids);
        assert_eq!(calls, sp.len().div_ceil(opts.batch));
        // Word ranges cover every word.
        let ranges = e.word_ranges(ws.len());
        assert!(ranges.iter().all(|r| r.start < r.end));
        assert_eq!(ranges.last().unwrap().end, e.ids.len());
    }

    #[test]
    fn short_streams_are_one_window_and_batched() {
        let a = Fake.encode(&["hello", "there"], " ").unwrap();
        let b = Fake.encode(&["hi"], " ").unwrap();
        let empty = Fake.encode(&[], " ").unwrap();
        let opts = WindowOpts {
            max_tokens: 512,
            specials: 2,
            context: 48,
            batch: 8,
        };
        assert_eq!(spans(&a.word_of, &opts).len(), 1);
        assert!(spans(&empty.word_of, &opts).is_empty());
        let mut calls = 0;
        let out = run_windows(&[&a, &b, &empty], &opts, &|| false, &mut |batch| {
            calls += 1;
            assert_eq!(batch.len(), 2, "both streams in one call");
            assert!(batch[0].len() <= batch[1].len(), "sorted by length");
            Ok(batch.iter().map(|ids| ids.to_vec()).collect())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            (out[0].clone(), out[1].clone()),
            (a.ids.clone(), b.ids.clone())
        );
        assert!(out[2].is_empty());
    }

    #[test]
    fn a_word_longer_than_a_window_is_cut_and_cancel_stops() {
        let long = "x".repeat(3 * 40);
        let e = Fake.encode(&[long.as_str(), "end"], " ").unwrap();
        let opts = WindowOpts {
            max_tokens: 16,
            specials: 2,
            context: 2,
            batch: 1,
        };
        let sp = spans(&e.word_of, &opts);
        assert!(sp.len() > 1);
        assert_eq!(sp.last().unwrap().end, e.ids.len());
        let r = run_windows(&[&e], &opts, &|| true, &mut |b| {
            Ok(b.iter().map(|ids| ids.to_vec()).collect())
        });
        assert!(matches!(r, Err(CoreError::Cancelled)));
        // A model that returns the wrong shape is an error, not a panic.
        let r = run_windows(&[&e], &opts, &|| false, &mut |b| {
            Ok(b.iter().map(|_| vec![0i64]).collect())
        });
        assert!(r.is_err());
    }
}
