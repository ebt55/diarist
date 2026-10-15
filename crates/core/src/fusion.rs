//! Word-to-speaker fusion: tags ASR words with speakers from diarization
//! segments (`docs/03`, experimental cascade and re-fusion).
//!
//! Rule (v1): a word belongs to the diarized segment that overlaps most of
//! its start-anchored window `[t0 - 40 ms, min(t1, t0 + 160 ms)]` (ties go to
//! the segment that started later); a word no segment overlaps goes to the
//! nearest segment. Speakers are renamed
//! `spk_N` in order of first appearance among the tagged words; each word
//! keeps its own `raw_label` untouched and records the diarizer's speaker.
//! The v1.5 Viterbi smoothing replaces this behind the same seam.

use crate::transcript::LabelMap;

/// Window before a word's start that still counts as "at the start".
pub const WINDOW_BEFORE_MS: i64 = 40;
/// Longest stretch after a word's start that the window covers.
pub const WINDOW_AFTER_MS: i64 = 160;

/// An ASR word without (or with an untrusted) speaker.
#[derive(Debug, Clone, PartialEq)]
pub struct AsrWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Whatever label the ASR attached, kept as is.
    pub raw_label: Option<String>,
}

/// One diarization segment; `speaker` is the diarizer's id (1-based for nemo-speech).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiarSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: u32,
}

/// A word with its fused speaker.
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// `spk_N` by first appearance, or `spk_unknown` when there are no segments.
    pub speaker: String,
    /// The diarizer's speaker id the word was given.
    pub diar_speaker: Option<u32>,
    /// The input word's own label.
    pub raw_label: Option<String>,
}

/// Tags `words` with speakers from `segments` (any order; overlapping
/// segments allowed). See the module docs for the rule.
pub fn assign_words(words: &[AsrWord], segments: &[DiarSegment]) -> Vec<TaggedWord> {
    let mut segs: Vec<DiarSegment> = segments
        .iter()
        .copied()
        .filter(|s| s.end_ms >= s.start_ms)
        .collect();
    segs.sort_by_key(|s| (s.start_ms, s.end_ms, s.speaker));
    // prefix_max_end[i] = max end of segs[..=i]: lets a binary search skip
    // every segment that ends before a window even when segments overlap.
    let mut prefix_max_end = Vec::with_capacity(segs.len());
    let mut running = i64::MIN;
    for s in &segs {
        running = running.max(s.end_ms);
        prefix_max_end.push(running);
    }
    let mut labels = LabelMap::new();
    words
        .iter()
        .map(|w| {
            let diar = speaker_for(w, &segs, &prefix_max_end);
            TaggedWord {
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                text: w.text.clone(),
                speaker: labels.label(diar),
                diar_speaker: diar,
                raw_label: w.raw_label.clone(),
            }
        })
        .collect()
}

fn speaker_for(w: &AsrWord, segs: &[DiarSegment], prefix_max_end: &[i64]) -> Option<u32> {
    if segs.is_empty() {
        return None;
    }
    let t0 = w.start_ms;
    let t1 = w.end_ms.max(t0);
    let w0 = t0 - WINDOW_BEFORE_MS;
    let w1 = t1.min(t0 + WINDOW_AFTER_MS);
    // First segment that could reach past w0.
    let first = prefix_max_end.partition_point(|&e| e <= w0);
    let mut best: Option<(i64, usize)> = None;
    for (i, s) in segs.iter().enumerate().skip(first) {
        if s.start_ms >= w1 {
            break;
        }
        let ov = s.end_ms.min(w1) - s.start_ms.max(w0);
        // Ties go to the later-starting segment (sorted by start), so an
        // interjection inside a long segment wins the words it starts with.
        if ov > 0 && best.is_none_or(|(b, _)| ov >= b) {
            best = Some((ov, i));
        }
    }
    if let Some((_, i)) = best {
        return Some(segs[i].speaker);
    }
    // Nearest segment to the word (gap to either side); earlier wins ties.
    let gap = |s: &DiarSegment| {
        if s.end_ms <= t0 {
            t0 - s.end_ms
        } else if s.start_ms >= t1 {
            s.start_ms - t1
        } else {
            0
        }
    };
    segs.iter()
        .enumerate()
        .min_by_key(|(i, s)| (gap(s), *i))
        .map(|(_, s)| s.speaker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: i64, end: i64, text: &str) -> AsrWord {
        AsrWord {
            start_ms: start,
            end_ms: end,
            text: text.into(),
            raw_label: None,
        }
    }

    fn s(start: i64, end: i64, speaker: u32) -> DiarSegment {
        DiarSegment {
            start_ms: start,
            end_ms: end,
            speaker,
        }
    }

    #[test]
    fn straddling_word_goes_to_the_speaker_at_its_start() {
        // "boundary" runs 900..1500; speaker 2 takes over at 1000 and owns
        // most of the word, but the start window [860, 1060] is mostly speaker 1's.
        let segs = [s(0, 1_000, 2), s(1_000, 3_000, 5)];
        let tagged = assign_words(&[w(100, 400, "hi"), w(900, 1_500, "boundary")], &segs);
        assert_eq!(tagged[1].diar_speaker, Some(2));
        assert_eq!(tagged[1].speaker, "spk_0");
        // A word starting just after the change belongs to the new speaker.
        let tagged = assign_words(&[w(1_030, 1_400, "next")], &segs);
        assert_eq!(tagged[0].diar_speaker, Some(5));
    }

    #[test]
    fn word_in_a_gap_goes_to_the_nearest_segment() {
        let segs = [s(0, 1_000, 1), s(5_000, 6_000, 2)];
        let tagged = assign_words(
            &[
                w(1_300, 1_500, "near-first"),
                w(4_600, 4_800, "near-second"),
            ],
            &segs,
        );
        assert_eq!(tagged[0].diar_speaker, Some(1));
        assert_eq!(tagged[1].diar_speaker, Some(2));
    }

    #[test]
    fn labels_follow_first_appearance_and_raw_labels_are_kept() {
        let segs = [s(0, 1_000, 3), s(1_000, 2_000, 1), s(2_000, 3_000, 3)];
        let mut words = vec![w(100, 300, "a"), w(1_100, 1_300, "b"), w(2_100, 2_300, "c")];
        words[1].raw_label = Some("asr-said".into());
        let tagged = assign_words(&words, &segs);
        let labels: Vec<&str> = tagged.iter().map(|t| t.speaker.as_str()).collect();
        assert_eq!(labels, ["spk_0", "spk_1", "spk_0"]);
        assert_eq!(tagged[1].raw_label.as_deref(), Some("asr-said"));
        // Deterministic: the same input gives the same labels.
        assert_eq!(assign_words(&words, &segs), tagged);
    }

    #[test]
    fn overlapping_segments_pick_the_larger_window_overlap() {
        // A long segment of speaker 1 covers a short interjection by speaker 2.
        let segs = [s(0, 10_000, 1), s(4_000, 4_600, 2)];
        let tagged = assign_words(&[w(4_050, 4_400, "yes"), w(7_000, 7_300, "so")], &segs);
        // Window [4010, 4210] lies inside both: the tie goes to the interjection.
        assert_eq!(tagged[0].diar_speaker, Some(2));
        assert_eq!(tagged[1].diar_speaker, Some(1));
        let tagged = assign_words(
            &[w(3_990, 4_400, "yes")],
            &[s(0, 4_000, 1), s(4_000, 4_600, 2)],
        );
        // Window [3950, 4150]: 50 ms of speaker 1, 150 ms of speaker 2.
        assert_eq!(tagged[0].diar_speaker, Some(2));
    }

    #[test]
    fn no_segments_means_unknown() {
        let tagged = assign_words(&[w(0, 100, "x")], &[]);
        assert_eq!(tagged[0].speaker, "spk_unknown");
        assert_eq!(tagged[0].diar_speaker, None);
    }
}
