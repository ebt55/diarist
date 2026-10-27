//! Transcript assembly: engine words to the stored model.
//!
//! Engine adapters produce [`EngineWord`]s with an adapter-normalized speaker
//! label (`spk_N`, or none). This module turns them into [`TranscriptWord`]s
//! (milliseconds, a label per word), groups them into segments
//! ([`segment`]: contiguous same-speaker words, split on long pauses, on
//! sentence punctuation followed by a pause, and on length) and builds the
//! [`NewTranscript`] that `Store::insert_transcript` writes
//! ([`build_transcript`]). The mic and loopback channels of a live meeting
//! are segmented one channel at a time and their segments ordered by start
//! time ([`interleave_segments`], [`build_transcript_channels`]), so speech
//! that overlaps in time stays in readable per-speaker segments instead of
//! alternating word by word. Before segmentation every build collapses the
//! decoder's repeated-token loops ([`loops`], plan 16 section 12.3).

pub mod loops;

use std::collections::HashMap;
use std::ops::Range;

use crate::protocol::EngineWord;
use crate::store::{NewSegment, NewSpeaker, NewTranscript, NewWord};
use crate::types::Channel;
use crate::views::SPEAKER_PALETTE;

/// Label for words the engine did not tag with a speaker.
pub const UNKNOWN_LABEL: &str = "spk_unknown";
/// Label of the undiarized mic channel.
pub const ME_LABEL: &str = "me";

/// `spk_N` for a zero-based speaker index, [`UNKNOWN_LABEL`] for none.
pub fn label_for(speaker: Option<u32>) -> String {
    match speaker {
        Some(n) => format!("spk_{n}"),
        None => UNKNOWN_LABEL.to_owned(),
    }
}

/// Deterministic colour for the speaker with zero-based index `index`
/// (the same palette the views fall back to, so stored and default colours agree).
pub fn speaker_color(index: usize) -> &'static str {
    SPEAKER_PALETTE[index % SPEAKER_PALETTE.len()]
}

/// Maps runtime speaker ids (any integers, e.g. 1-based) to `spk_N` labels
/// numbered by first appearance.
#[derive(Debug, Clone, Default)]
pub struct LabelMap {
    map: HashMap<u32, u32>,
}

impl LabelMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Zero-based index of `raw`, assigning the next one on first sight.
    pub fn index(&mut self, raw: u32) -> u32 {
        let next = self.map.len() as u32;
        *self.map.entry(raw).or_insert(next)
    }

    /// `spk_N` for `raw`, [`UNKNOWN_LABEL`] for `None`.
    pub fn label(&mut self, raw: Option<u32>) -> String {
        label_for(raw.map(|r| self.index(r)))
    }

    /// Distinct speakers seen so far.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A word ready for segmentation. Times are milliseconds on the meeting clock.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptWord {
    pub channel: Channel,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub confidence: Option<f32>,
    /// Label of the speaker the word's segment will belong to.
    pub speaker: String,
    /// Engine label before smoothing or edits (`words.raw_label`).
    pub raw_label: Option<String>,
    /// The ASR's text where `text` holds display text that differs from it
    /// (punctuation, `words.raw_text`); `None` when `text` is the ASR's.
    pub raw_text: Option<String>,
    /// v1.5 fusion (`words.margin`): `None` from the engines today; a
    /// rebuild copies the stored value.
    pub margin: Option<f32>,
    /// v1.5 fusion (`words.overlap`); a rebuild copies the stored value.
    pub overlap: bool,
}

impl TranscriptWord {
    /// The ASR's text of the word: `raw_text`, else `text`.
    pub fn asr_text(&self) -> &str {
        self.raw_text.as_deref().unwrap_or(&self.text)
    }
}

/// Converts engine words (microseconds) of one channel. Words without a
/// speaker get `default_label` (`me` for an undiarized mic, else
/// [`UNKNOWN_LABEL`]); `raw_label` keeps what the engine said.
pub fn words_from_engine(
    channel: Channel,
    words: &[EngineWord],
    default_label: &str,
) -> Vec<TranscriptWord> {
    words
        .iter()
        .map(|w| TranscriptWord {
            channel,
            start_ms: us_to_ms(w.t0_us),
            end_ms: us_to_ms(w.t1_us.max(w.t0_us)),
            text: w.text.clone(),
            confidence: w.conf,
            speaker: w.spk.clone().unwrap_or_else(|| default_label.to_owned()),
            raw_label: w.spk.clone(),
            raw_text: None,
            margin: None,
            overlap: false,
        })
        .collect()
}

/// Microseconds to milliseconds, rounded to nearest.
pub fn us_to_ms(us: u64) -> i64 {
    ((us + 500) / 1_000) as i64
}

/// Segmentation thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentOptions {
    /// Split when the pause between two words exceeds this.
    pub max_gap_ms: i64,
    /// Split after sentence-final punctuation when the pause exceeds this.
    pub sentence_gap_ms: i64,
    /// Split when a segment reaches this many words.
    pub max_words: usize,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            max_gap_ms: 1_500,
            sentence_gap_ms: 500,
            max_words: 80,
        }
    }
}

/// A segment before it has an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentDraft {
    pub channel: Channel,
    pub speaker: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Indices into the word slice passed to [`segment`].
    pub words: Range<usize>,
}

/// True when `text` ends a sentence (`.`, `?`, `!`, `…`, `。` `？` `！`, the
/// Devanagari `।` `॥`, the Arabic `؟` `۔`, the Ethiopic `።` `፧` and the Greek
/// question mark U+037E; also before closing quotes and trailing whitespace).
pub fn ends_sentence(text: &str) -> bool {
    let trimmed = text
        .trim_end()
        .trim_end_matches(['"', '\'', ')', '”', '’', ']']);
    matches!(
        trimmed.chars().last(),
        Some(
            '.' | '?'
                | '!'
                | '…'
                | '。'
                | '？'
                | '！'
                | '।'
                | '॥'
                | '؟'
                | '۔'
                | '።'
                | '፧'
                | '\u{037E}'
        )
    )
}

/// Groups words (in time order) into segments: contiguous words of the same
/// channel and speaker, split when the pause exceeds `max_gap_ms`, when the
/// previous word ends a sentence and the pause exceeds `sentence_gap_ms`, or
/// when the segment already has `max_words` words.
pub fn segment(words: &[TranscriptWord], opts: &SegmentOptions) -> Vec<SegmentDraft> {
    let mut out: Vec<SegmentDraft> = Vec::new();
    let mut start = 0usize;
    for i in 1..=words.len() {
        let split = match words.get(i) {
            None => true,
            Some(w) => {
                let prev = &words[i - 1];
                let gap = w.start_ms - prev.end_ms;
                w.speaker != prev.speaker
                    || w.channel != prev.channel
                    || gap > opts.max_gap_ms
                    || (ends_sentence(&prev.text) && gap > opts.sentence_gap_ms)
                    || i - start >= opts.max_words.max(1)
            }
        };
        if split && i > start {
            let group = &words[start..i];
            out.push(SegmentDraft {
                channel: group[0].channel,
                speaker: group[0].speaker.clone(),
                start_ms: group[0].start_ms,
                end_ms: group
                    .iter()
                    .map(|w| w.end_ms)
                    .max()
                    .unwrap_or(group[0].end_ms),
                text: join_text(group.iter().map(|w| w.text.as_str())),
                words: start..i,
            });
            start = i;
        }
    }
    out
}

/// Joins word texts with single spaces, skipping empty words.
pub fn join_text<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut s = String::new();
    for w in words {
        let w = w.trim();
        if w.is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(w);
    }
    s
}

/// Merges channels into one time-ordered word list (stable: equal start
/// times keep the order of `channels`, then of the words within a channel).
/// Segmenting the result splits wherever the channels alternate; transcripts
/// of several channels use [`build_transcript_channels`] instead.
pub fn interleave(channels: Vec<(Channel, Vec<TranscriptWord>)>) -> Vec<TranscriptWord> {
    let mut all: Vec<(i64, usize, usize, TranscriptWord)> = Vec::new();
    for (ci, (channel, words)) in channels.into_iter().enumerate() {
        for (wi, mut w) in words.into_iter().enumerate() {
            w.channel = channel;
            all.push((w.start_ms, ci, wi, w));
        }
    }
    all.sort_by_key(|(start, ci, wi, _)| (*start, *ci, *wi));
    all.into_iter().map(|(_, _, _, w)| w).collect()
}

/// Orders the segments of several channels by start time: `channels[i]` are
/// the segments of channel `i` (from [`segment`], in time order). Returns
/// `(channel index, segment index)` pairs. Segments of different channels
/// may overlap in time; they are never split or merged here. Equal start
/// times keep the order of `channels`, then of the segments within one.
pub fn interleave_segments(channels: &[Vec<SegmentDraft>]) -> Vec<(usize, usize)> {
    let mut order: Vec<(i64, usize, usize)> = channels
        .iter()
        .enumerate()
        .flat_map(|(ci, segs)| {
            segs.iter()
                .enumerate()
                .map(move |(si, s)| (s.start_ms, ci, si))
        })
        .collect();
    order.sort_unstable();
    order.into_iter().map(|(_, ci, si)| (ci, si)).collect()
}

/// The rows of one transcript, filled segment by segment.
#[derive(Default)]
struct TranscriptBuilder {
    t: NewTranscript,
    speaker_index: HashMap<(Channel, String), usize>,
}

impl TranscriptBuilder {
    /// Appends a segment and its words; speakers are created per (channel,
    /// label) in order of first appearance.
    fn push(&mut self, d: &SegmentDraft, words: &[TranscriptWord]) {
        let t = &mut self.t;
        let key = (d.channel, d.speaker.clone());
        let speaker = *self.speaker_index.entry(key).or_insert_with(|| {
            t.speakers.push(NewSpeaker {
                channel: d.channel,
                engine_label: d.speaker.clone(),
                display_name: None,
                color: None,
                needs_review: false,
            });
            t.speakers.len() - 1
        });
        let si = t.segments.len();
        t.segments.push(NewSegment {
            speaker,
            start_ms: d.start_ms,
            end_ms: d.end_ms,
            text: d.text.clone(),
        });
        t.words.extend(words.iter().map(|w| NewWord {
            segment: si,
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            text: w.text.clone(),
            confidence: w.confidence,
            raw_label: w.raw_label.clone(),
            margin: w.margin,
            overlap: w.overlap,
            raw_text: w.raw_text.clone(),
        }));
    }
}

/// Segments `words` (one stream in time order) and builds the rows for
/// `Store::insert_transcript`. Speakers are created per (channel, label) in
/// order of first appearance.
pub fn build_transcript(words: &[TranscriptWord], opts: &SegmentOptions) -> NewTranscript {
    let mut b = TranscriptBuilder::default();
    for d in segment(words, opts) {
        b.push(&d, &words[d.words.clone()]);
    }
    b.t
}

/// The transcript of several channels (a live meeting's mic and loopback):
/// each channel's words are sorted and segmented on their own, then the
/// segments are ordered by start time ([`interleave_segments`]), so `seq`
/// follows the start times and overlapping speech keeps whole segments.
/// Equal start times keep the order of `channels`.
pub fn build_transcript_channels(
    channels: Vec<(Channel, Vec<TranscriptWord>)>,
    opts: &SegmentOptions,
) -> NewTranscript {
    let words: Vec<Vec<TranscriptWord>> = channels
        .into_iter()
        .map(|(channel, mut words)| {
            for w in &mut words {
                w.channel = channel;
            }
            words.sort_by_key(|w| w.start_ms);
            words
        })
        .collect();
    let drafts: Vec<Vec<SegmentDraft>> = words.iter().map(|w| segment(w, opts)).collect();
    let mut b = TranscriptBuilder::default();
    for (ci, si) in interleave_segments(&drafts) {
        let d = &drafts[ci][si];
        b.push(d, &words[ci][d.words.clone()]);
    }
    b.t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: i64, end: i64, text: &str, spk: &str) -> TranscriptWord {
        TranscriptWord {
            channel: Channel::Source,
            start_ms: start,
            end_ms: end,
            text: text.into(),
            confidence: None,
            speaker: spk.into(),
            raw_label: Some(spk.into()),
            raw_text: None,
            margin: None,
            overlap: false,
        }
    }

    #[test]
    fn labels_and_label_map() {
        assert_eq!(label_for(Some(0)), "spk_0");
        assert_eq!(label_for(None), "spk_unknown");
        let mut m = LabelMap::new();
        assert_eq!(m.label(Some(3)), "spk_0");
        assert_eq!(m.label(Some(1)), "spk_1");
        assert_eq!(m.label(Some(3)), "spk_0");
        assert_eq!(m.label(None), "spk_unknown");
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn palette_is_deterministic() {
        assert_eq!(speaker_color(0), SPEAKER_PALETTE[0]);
        assert_eq!(speaker_color(SPEAKER_PALETTE.len() + 2), SPEAKER_PALETTE[2]);
        assert_eq!(
            speaker_color(3),
            crate::views::default_speaker_color("spk_3")
        );
    }

    #[test]
    fn splits_on_speaker_change_and_long_gap() {
        let words = vec![
            w(0, 300, "hello", "spk_0"),
            w(400, 700, "there", "spk_0"),
            w(800, 1_000, "hi", "spk_1"),
            // 1600 ms gap: split even though the speaker is the same.
            w(2_600, 2_900, "so", "spk_1"),
            // exactly 1500 ms gap: no split.
            w(4_400, 4_600, "yes", "spk_1"),
        ];
        let segs = segment(&words, &SegmentOptions::default());
        let texts: Vec<&str> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["hello there", "hi", "so yes"]);
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[0].end_ms, 700);
        assert_eq!(segs[2].words, 3..5);
        assert_eq!(segs[1].speaker, "spk_1");
    }

    #[test]
    fn splits_after_sentence_punctuation_only_with_a_pause() {
        let words = vec![
            w(0, 300, "Done.", "spk_0"),
            // 200 ms after a full stop: same segment.
            w(500, 800, "Next", "spk_0"),
            w(900, 1_200, "item?", "spk_0"),
            // 600 ms after a question mark: split.
            w(1_800, 2_000, "Yes", "spk_0"),
            // 600 ms after a word without punctuation: same segment.
            w(2_600, 2_800, "okay", "spk_0"),
        ];
        let segs = segment(&words, &SegmentOptions::default());
        let texts: Vec<&str> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Done. Next item?", "Yes okay"]);
        assert!(ends_sentence("end.\""));
        assert!(!ends_sentence("Dr"));
        // Script sentence ends (plan 16 section 3.4).
        for s in ["है।", "॥", "ماذا؟", "ہے۔", "ነው።", "፧", "τι\u{037E}", "好。"]
        {
            assert!(ends_sentence(s), "{s}");
        }
        assert!(!ends_sentence("،") && !ends_sentence("hello,"));
        assert!(ends_sentence("done. ") && ends_sentence("ok?\" "));
    }

    #[test]
    fn splits_long_monologues() {
        let words: Vec<TranscriptWord> = (0..170)
            .map(|i| w(i * 100, i * 100 + 80, "word", "spk_0"))
            .collect();
        let segs = segment(&words, &SegmentOptions::default());
        assert_eq!(
            segs.iter().map(|s| s.words.len()).collect::<Vec<_>>(),
            [80, 80, 10]
        );
        assert!(segment(&[], &SegmentOptions::default()).is_empty());
    }

    #[test]
    fn interleave_orders_by_time_and_sets_channels() {
        let mic = vec![w(100, 200, "a", "me"), w(900, 1_000, "c", "me")];
        let lb = vec![w(100, 300, "b", "spk_0"), w(500, 600, "d", "spk_0")];
        let all = interleave(vec![(Channel::Mic, mic), (Channel::Loopback, lb)]);
        let order: Vec<(&str, Channel)> =
            all.iter().map(|w| (w.text.as_str(), w.channel)).collect();
        assert_eq!(
            order,
            [
                ("a", Channel::Mic),
                ("b", Channel::Loopback),
                ("d", Channel::Loopback),
                ("c", Channel::Mic)
            ]
        );
    }

    /// Two channels that talk over each other for 10 s: the mic from 0 to
    /// 20 s, the loopback from 10 to 30 s, a word every 400 ms each. The
    /// source's only one-word segment is a lone "Yes." on the mic at 40 s.
    fn overlapping_channels() -> Vec<(Channel, Vec<TranscriptWord>)> {
        let mut mic: Vec<TranscriptWord> = (0..50)
            .map(|i| w(i * 400, i * 400 + 300, &format!("m{i}"), "me"))
            .collect();
        mic.push(w(40_000, 40_300, "Yes.", "me"));
        let lb: Vec<TranscriptWord> = (0..50)
            .map(|i| {
                let t = 10_200 + i * 400;
                w(t, t + 300, &format!("l{i}"), "spk_0")
            })
            .collect();
        vec![(Channel::Mic, mic), (Channel::Loopback, lb)]
    }

    #[test]
    fn overlapping_channels_keep_whole_segments() {
        let opts = SegmentOptions::default();
        let t = build_transcript_channels(overlapping_channels(), &opts);
        let words_in = |si: usize| t.words.iter().filter(|w| w.segment == si).count();
        let texts: Vec<(&str, usize)> = t
            .segments
            .iter()
            .enumerate()
            .map(|(si, s)| (t.speakers[s.speaker].engine_label.as_str(), words_in(si)))
            .collect();
        assert_eq!(texts, [("me", 50), ("spk_0", 50), ("me", 1)]);
        // Ordered by start; the first two overlap from 10.2 to 19.9 s.
        assert!(t
            .segments
            .windows(2)
            .all(|p| p[0].start_ms <= p[1].start_ms));
        assert_eq!((t.segments[0].start_ms, t.segments[0].end_ms), (0, 19_900));
        assert_eq!(
            (t.segments[1].start_ms, t.segments[1].end_ms),
            (10_200, 30_100)
        );
        assert_eq!(t.segments[2].text, "Yes.");
        assert_eq!(t.words.len(), 101);
        assert!(t.words.windows(2).all(|p| p[0].segment <= p[1].segment));
        assert_eq!(t.speakers[0].channel, Channel::Mic);
        assert_eq!(t.speakers[1].channel, Channel::Loopback);
        // Interleaving words first alternates speakers word by word in the overlap.
        let old = build_transcript(&interleave(overlapping_channels()), &opts);
        let one_word = (0..old.segments.len())
            .filter(|si| old.words.iter().filter(|w| w.segment == *si).count() == 1)
            .count();
        assert!(one_word > 40, "{one_word} one-word segments");
    }

    #[test]
    fn interleave_segments_orders_by_start_then_channel() {
        let seg = |start: i64| SegmentDraft {
            channel: Channel::Source,
            speaker: "x".into(),
            start_ms: start,
            end_ms: start + 5_000,
            text: String::new(),
            words: 0..0,
        };
        let order = interleave_segments(&[vec![seg(0), seg(3_000)], vec![seg(1_000), seg(3_000)]]);
        assert_eq!(order, [(0, 0), (1, 0), (0, 1), (1, 1)]);
        assert!(interleave_segments(&[]).is_empty());
    }

    #[test]
    fn build_transcript_creates_speakers_by_first_appearance() {
        let words = vec![
            w(0, 300, "one", "spk_1"),
            w(400, 600, "two", "spk_0"),
            w(700, 900, "three", "spk_1"),
        ];
        let t = build_transcript(&words, &SegmentOptions::default());
        let labels: Vec<&str> = t.speakers.iter().map(|s| s.engine_label.as_str()).collect();
        assert_eq!(labels, ["spk_1", "spk_0"]);
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[2].speaker, 0);
        assert_eq!(t.words.len(), 3);
        assert_eq!(t.words[2].segment, 2);
        assert_eq!(t.words[0].raw_label.as_deref(), Some("spk_1"));
    }

    #[test]
    fn engine_words_convert_to_ms_with_default_labels() {
        let ew = vec![
            EngineWord {
                t0_us: 240_000,
                t1_us: 320_000,
                text: "Um,".into(),
                conf: None,
                spk: Some("spk_0".into()),
            },
            EngineWord {
                t0_us: 400_400,
                t1_us: 480_000,
                text: "ok".into(),
                conf: None,
                spk: None,
            },
        ];
        let words = words_from_engine(Channel::Mic, &ew, ME_LABEL);
        assert_eq!((words[0].start_ms, words[0].end_ms), (240, 320));
        assert_eq!(words[1].start_ms, 400);
        assert_eq!(words[1].speaker, "me");
        assert_eq!(words[1].raw_label, None);
        assert_eq!(words[0].speaker, "spk_0");
    }
}
