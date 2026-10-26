//! The punctuation model's tokenizer (a Hugging Face `tokenizer.json`, plan
//! 16 section 3.7) as a [`TokenCounter`]: token ids for the words of one
//! stream, each token mapped to its word by its byte offsets in the joined
//! text.

use std::path::Path;

use super::windows::{Encoded, TokenCounter};
use crate::error::{CoreError, Result};

/// A loaded `tokenizer.json`.
pub struct HfTokenizer {
    inner: tokenizers::Tokenizer,
}

impl std::fmt::Debug for HfTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HfTokenizer")
    }
}

fn tok_err(path: &Path, e: impl std::fmt::Display) -> CoreError {
    CoreError::Models(format!("tokenizer {}: {e}", path.display()))
}

impl HfTokenizer {
    /// Loads `path` with truncation and padding off (windows are cut by
    /// [`super::windows`]; A's file truncates at 512 tokens, which would drop
    /// the rest of a long stream silently) and parallel encoding off (the
    /// post-pass runs beside the ASR and the echo canceller).
    pub fn load(path: &Path) -> Result<Self> {
        tokenizers::utils::parallelism::set_parallelism(false);
        let mut inner = tokenizers::Tokenizer::from_file(path).map_err(|e| tok_err(path, e))?;
        inner.with_truncation(None).map_err(|e| tok_err(path, e))?;
        inner.with_padding(None);
        Ok(Self { inner })
    }
}

impl TokenCounter for HfTokenizer {
    fn encode(&self, words: &[&str], sep: &str) -> Result<Encoded> {
        let (text, ranges) = join(words, sep);
        let enc = self
            .inner
            .encode(text.as_str(), false)
            .map_err(|e| CoreError::Models(format!("tokenizer: {e}")))?;
        let mut out = Encoded::default();
        let mut prev = 0usize;
        for ((&id, &(s, e)), piece) in enc
            .get_ids()
            .iter()
            .zip(enc.get_offsets())
            .zip(enc.get_tokens())
        {
            let w = word_at(&ranges, s, e).max(prev);
            prev = w;
            out.ids.push(i64::from(id));
            out.word_of.push(w);
            out.ends_word
                .push(ranges.get(w).is_some_and(|r| e == r.1 && e > r.0));
            out.pieces
                .push(piece.trim_start_matches('\u{2581}').to_owned());
        }
        Ok(out)
    }
}

/// `words` joined by `sep`, with each word's byte range in the text.
pub fn join(words: &[&str], sep: &str) -> (String, Vec<(usize, usize)>) {
    let mut text = String::with_capacity(words.iter().map(|w| w.len() + sep.len()).sum());
    let mut ranges = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            text.push_str(sep);
        }
        let start = text.len();
        text.push_str(w);
        ranges.push((start, text.len()));
    }
    (text, ranges)
}

/// The word a token with byte offsets `s..e` belongs to: the word holding
/// its last byte; a token that is only a separator (a lone `▁`) belongs to
/// the next word.
pub fn word_at(ranges: &[(usize, usize)], s: usize, e: usize) -> usize {
    if ranges.is_empty() {
        return 0;
    }
    let c = if e > s { e - 1 } else { s };
    // The last word starting at or before `c`.
    let i = ranges.partition_point(|r| r.0 <= c).saturating_sub(1);
    let in_word = c >= ranges[i].0 && c < ranges[i].1;
    if in_word || i + 1 >= ranges.len() || c < ranges[i].0 {
        i
    } else {
        i + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_map_to_their_words() {
        let (text, r) = join(&["hello", "big", "world"], " ");
        assert_eq!(text, "hello big world");
        assert_eq!(r, [(0, 5), (6, 9), (10, 15)]);
        // "hel" "lo" " big" (space attached in front) "▁" alone, "world".
        assert_eq!(word_at(&r, 0, 3), 0);
        assert_eq!(word_at(&r, 3, 5), 0);
        assert_eq!(word_at(&r, 5, 9), 1);
        assert_eq!(
            word_at(&r, 9, 10),
            2,
            "a lone separator goes with the next word"
        );
        assert_eq!(word_at(&r, 10, 15), 2);
        assert_eq!(word_at(&r, 15, 15), 2);
        let (text, r) = join(&["東京", "に"], "");
        assert_eq!(
            (text.as_str(), r.as_slice()),
            ("東京に", &[(0, 6), (6, 9)][..])
        );
        assert_eq!(word_at(&r, 3, 6), 0);
        assert_eq!(word_at(&r, 6, 9), 1);
    }

    /// A tokenizer.json built in the test (WordLevel over whitespace): the
    /// real crate path, no model download.
    #[test]
    fn loads_a_tokenizer_json_and_maps_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokenizer.json");
        std::fs::write(
            &path,
            r#"{
              "version": "1.0", "truncation": {"direction": "Right", "max_length": 2, "strategy": "LongestFirst", "stride": 0},
              "padding": null, "added_tokens": [],
              "normalizer": {"type": "Lowercase"},
              "pre_tokenizer": {"type": "Whitespace"},
              "post_processor": null, "decoder": null,
              "model": {"type": "WordLevel", "vocab": {"[UNK]": 0, "hello": 1, "world": 2, ",": 3}, "unk_token": "[UNK]"}
            }"#,
        )
        .unwrap();
        let t = HfTokenizer::load(&path).unwrap();
        let e = t.encode(&["Hello", "big", "world,"], " ").unwrap();
        // Truncation at 2 is off: all four tokens are there.
        assert_eq!(e.ids, [1, 0, 2, 3]);
        assert_eq!(e.word_of, [0, 1, 2, 2]);
        assert_eq!(e.ends_word, [true, true, false, true]);
        assert_eq!(e.word_ranges(3), [0..1, 1..2, 2..4]);
        assert!(HfTokenizer::load(&dir.path().join("missing.json")).is_err());
    }
}
