//! Repeated-token loop collapse (docs/plans/16 section 12.3).
//!
//! The streaming decoder sometimes emits one token many times over: bursts
//! of up to nine copies stamped on the same 80 ms encoder frame, a few
//! frames apart ("the the the the the ..."). People repeat words too (a
//! stutter, laughter, "no no no no"), but at human pace, several frames
//! apart. The collapse uses that timing signature, not the count alone.
//!
//! The rule, over one channel's words in time order ([`collapse_channel`]):
//! a run is a maximal sequence of consecutive words with the same
//! normalised text (the echo gate's [`norm`] of the ASR's text) and the same
//! speaker label, split into pieces wherever two consecutive starts are more
//! than [`LOOP_SPLIT_MS`] apart. A piece is a loop when it has
//! [`LOOP_MIN_RUN`] or more words at a median start-to-start gap of at most
//! [`LOOP_FRAME_MS`] (one frame), or exactly three words that start in the
//! same frame (within half a frame of each other: equal on the runtime's
//! 80 ms grid, whatever the clock map's rounding; three words one frame
//! apart are within the base rate of distinct words and stay). That is the
//! rule of plan 16 section 12.3, and it decides only which pieces are
//! loops. What goes is narrower: within a loop, the bursts, the maximal
//! sub-runs whose consecutive starts are at most [`LOOP_FRAME_MS`] apart,
//! and of those only the ones that meet the rule on their own (four or more
//! words, or exactly three in one frame). Each such burst becomes its first
//! word, ending where the burst's last word ends and carrying the last
//! word's trailing ASR mark; its start, speaker, engine label, confidence
//! and fusion fields are the first word's, and its text is the ASR's
//! (`raw_text` cleared: punctuation runs after the collapse). The words of
//! a loop at human pace around a burst stay as they are, so five copies in
//! one frame followed by the same word at 300, 600 and 900 ms keep the three
//! real ones (the piece's median alone would take all eight), and a train
//! of bursts a few frames apart keeps one word per burst. This is strictly
//! more conservative than the plan's rule: it never removes a word that the
//! plan's rule keeps. Never collapsed: pairs, pieces at human pace, words
//! whose `norm` is empty or digits only, and words with Han, Kana or Thai
//! characters (continuous scripts, where grammatical triples are common and
//! the runtime's word units are large). Every other word keeps its count,
//! order and times.
//!
//! Where it runs: every build that makes words, after the echo gate (which
//! keeps seeing the raw words) and the unknown-token check, before
//! punctuation and segmentation: the post-pass (both channels), the
//! promoted live run, imports, Restore (the words in `echo_dropped` are raw,
//! so a loop removed as echo comes back whole) and punctuation rebuilds,
//! Remove punctuation included. Behind `transcript.collapse_loops` (default
//! on; [`enabled`]). Counts only reach the database ([`LoopReport`],
//! `engine_info.loops`); the removed words are not stored with the run.
//! Where the originals survive depends on the path: the file engine's words
//! JSON in the meeting folder (`<wav stem>.words.json`) for the post-pass
//! and imports; the earlier run, which stays, for Restore and rebuilds; the
//! live run for a promoted one, which the post-pass keeps whenever the
//! collapse removed a word (the live engine writes no words JSON). Turning
//! the setting off later does not bring collapsed words back into a run;
//! only that earlier run or a new transcription has them. No live
//! collapse: the live view's words are replaced by the post-pass.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::TranscriptWord;
use crate::audio::echo::text::norm;
use crate::punct::Script;
use crate::settings;
use crate::store::Store;
use crate::types::Channel;

/// A piece of this many identical words or more is a loop when its median
/// start-to-start gap is at most [`LOOP_FRAME_MS`]; a burst inside it of
/// this many words or more goes.
pub const LOOP_MIN_RUN: usize = 4;
/// One encoder frame: Nemotron stamps word starts on an 80 ms grid. The
/// median gap of a loop, and the largest gap inside a burst.
pub const LOOP_FRAME_MS: i64 = 80;
/// A run is split where two consecutive starts are more than this apart
/// (a pause inside a run of identical words is a new piece).
pub const LOOP_SPLIT_MS: i64 = 400;

/// What the collapse removed from one channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChannelLoops {
    /// Bursts collapsed (one word kept of each).
    pub runs: u64,
    /// Words removed (each burst's words but the one kept).
    pub words_removed: u64,
}

impl ChannelLoops {
    fn add(&mut self, other: ChannelLoops) {
        self.runs += other.runs;
        self.words_removed += other.words_removed;
    }
}

/// The rule's parameters, recorded with the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoopRule {
    /// [`LOOP_MIN_RUN`].
    pub min_run: u32,
    /// [`LOOP_FRAME_MS`].
    pub frame_ms: u32,
    /// [`LOOP_SPLIT_MS`].
    pub split_ms: u32,
}

impl Default for LoopRule {
    fn default() -> Self {
        Self {
            min_run: LOOP_MIN_RUN as u32,
            frame_ms: LOOP_FRAME_MS as u32,
            split_ms: LOOP_SPLIT_MS as u32,
        }
    }
}

/// `engine_info.loops` of a run (plan 16 section 12.3): counts only,
/// replaced wholesale on every build. The counts are the run's words
/// against the engine's: what this build removed plus, for a run rebuilt
/// from another (Restore, a punctuation rebuild), what that run's own
/// `engine_info.loops` counted ([`LoopReport::on_top_of`]), because its
/// words are already collapsed and the earlier removals are not seen again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoopReport {
    /// The collapse ran on this build (`transcript.collapse_loops` on).
    pub applied: bool,
    /// Bursts collapsed (the details call them repeated-token loops).
    pub runs: u64,
    /// Words removed.
    pub words_removed: u64,
    /// Per channel (`mic`, `loopback`, `source`).
    #[serde(default)]
    pub per_channel: BTreeMap<String, ChannelLoops>,
    #[serde(default)]
    pub rule: LoopRule,
}

impl LoopReport {
    /// As JSON for `engine_info.loops`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"applied": false}))
    }

    /// The report stored in a run's `engine_info`, if it has one.
    pub fn of_engine_info(engine_info: &serde_json::Value) -> Option<LoopReport> {
        serde_json::from_value(engine_info.get("loops")?.clone()).ok()
    }

    /// This build's report with the counts of the run it was built from
    /// (`earlier`, that run's `engine_info`) added: the rebuilt run's words
    /// are that run's, already collapsed.
    pub fn on_top_of(mut self, earlier: &serde_json::Value) -> Self {
        if let Some(e) = Self::of_engine_info(earlier) {
            self.runs += e.runs;
            self.words_removed += e.words_removed;
            for (ch, c) in e.per_channel {
                self.per_channel.entry(ch).or_default().add(c);
            }
        }
        self
    }
}

/// `transcript.collapse_loops` of the store (on unless set off; an
/// unreadable store reads as the default, on).
pub fn enabled(store: &dyn Store) -> bool {
    settings::effective(store)
        .map(|s| settings::collapse_loops(&s))
        .unwrap_or(true)
}

/// Collapses the loops of every channel when `on`; the report for
/// `engine_info.loops` (with `applied: false` and nothing touched when off).
pub fn collapse_channels(channels: &mut [(Channel, Vec<TranscriptWord>)], on: bool) -> LoopReport {
    collapse_channels_marked(channels, on, &mut []).0
}

/// [`collapse_channels`] with a mark per word: `marks[i]` for the words of
/// `channels[i]` ([`collapse_channel_marked`]; a channel past the end of
/// `marks` has none marked). Also returns how many marked words the
/// collapse removed (0 when off, which touches neither words nor marks).
pub fn collapse_channels_marked(
    channels: &mut [(Channel, Vec<TranscriptWord>)],
    on: bool,
    marks: &mut [Vec<bool>],
) -> (LoopReport, u64) {
    let mut report = LoopReport {
        applied: on,
        ..LoopReport::default()
    };
    if !on {
        return (report, 0);
    }
    let mut marked = 0;
    for (i, (channel, words)) in channels.iter_mut().enumerate() {
        let c = match marks.get_mut(i) {
            Some(m) => {
                let (c, removed) = collapse_channel_marked(words, m);
                marked += removed;
                c
            }
            None => collapse_channel(words),
        };
        report.runs += c.runs;
        report.words_removed += c.words_removed;
        report
            .per_channel
            .entry(channel.as_str().to_owned())
            .or_default()
            .add(c);
    }
    (report, marked)
}

/// Collapses the loops of one channel's words in place (see the module
/// docs). The words are put in time order first (a stable sort, so words
/// with the same start keep their order).
pub fn collapse_channel(words: &mut Vec<TranscriptWord>) -> ChannelLoops {
    words.sort_by_key(|w| w.start_ms);
    apply(words, None).0
}

/// [`collapse_channel`] with a mark per word (`marks[i]` belongs to
/// `words[i]` and moves with it through the sort and the collapse): also
/// returns how many marked words the collapse removed. A marked word that
/// is a burst's first, the one kept, is not counted. Restore marks the words
/// it puts back, so it can say how many of them merged into a loop.
pub fn collapse_channel_marked(
    words: &mut Vec<TranscriptWord>,
    marks: &mut Vec<bool>,
) -> (ChannelLoops, u64) {
    marks.resize(words.len(), false);
    let mut paired: Vec<(TranscriptWord, bool)> = std::mem::take(words)
        .into_iter()
        .zip(marks.drain(..))
        .collect();
    paired.sort_by_key(|(w, _)| w.start_ms);
    for (w, m) in paired {
        words.push(w);
        marks.push(m);
    }
    apply(words, Some(marks))
}

/// Collapses the bursts of `words` (in time order), keeping `marks` in step;
/// the counts and the marked words removed.
fn apply(words: &mut Vec<TranscriptWord>, marks: Option<&mut Vec<bool>>) -> (ChannelLoops, u64) {
    let bursts = bursts(words);
    let mut stats = ChannelLoops::default();
    if bursts.is_empty() {
        return (stats, 0);
    }
    let old = std::mem::take(words);
    let old_marks = marks.as_ref().map(|m| m.to_vec());
    let mut new_marks = Vec::new();
    let mut marked_removed = 0;
    let mut next = bursts.iter().peekable();
    let mut k = 0;
    while k < old.len() {
        match next.peek() {
            Some(&&(a, b)) if a == k => {
                words.push(collapsed(&old[a..=b]));
                if let Some(m) = &old_marks {
                    new_marks.push(m[a]);
                    marked_removed += m[a + 1..=b].iter().filter(|&&x| x).count() as u64;
                }
                stats.runs += 1;
                stats.words_removed += (b - a) as u64;
                next.next();
                k = b + 1;
            }
            _ => {
                words.push(old[k].clone());
                if let Some(m) = &old_marks {
                    new_marks.push(m[k]);
                }
                k += 1;
            }
        }
    }
    if let Some(m) = marks {
        *m = new_marks;
    }
    (stats, marked_removed)
}

/// The bursts to collapse in `words` (in time order) as (first, last)
/// indices, in order: within each piece that the plan's rule takes for a
/// loop ([`is_loop`]), the frame-level sub-runs that are loops on their own
/// ([`is_burst`]).
fn bursts(words: &[TranscriptWord]) -> Vec<(usize, usize)> {
    let norms: Vec<String> = words.iter().map(|w| norm(w.asr_text())).collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut j = i;
        while j + 1 < words.len()
            && norms[j + 1] == norms[i]
            && words[j + 1].speaker == words[i].speaker
        {
            j += 1;
        }
        let mut a = i;
        for k in i + 1..=j + 1 {
            if k > j || words[k].start_ms - words[k - 1].start_ms > LOOP_SPLIT_MS {
                if is_loop(&words[a..k], &norms[a]) {
                    let mut s = a;
                    for t in a + 1..=k {
                        if t == k || words[t].start_ms - words[t - 1].start_ms > LOOP_FRAME_MS {
                            if is_burst(&words[s..t]) {
                                out.push((s, t - 1));
                            }
                            s = t;
                        }
                    }
                }
                a = k;
            }
        }
        i = j + 1;
    }
    out
}

/// Whether a piece (identical normalised text `n`, one speaker, starts in
/// order, no gap over [`LOOP_SPLIT_MS`]) is a loop by the plan's rule:
/// [`LOOP_MIN_RUN`] or more words at a median start gap of at most
/// [`LOOP_FRAME_MS`], or exactly three in one frame.
fn is_loop(piece: &[TranscriptWord], n: &str) -> bool {
    if !eligible(n) {
        return false;
    }
    match piece.len() {
        len if len >= LOOP_MIN_RUN => median_gap_at_most(piece, LOOP_FRAME_MS),
        3 => in_one_frame(piece),
        _ => false,
    }
}

/// Whether a sub-run of a loop (consecutive starts at most
/// [`LOOP_FRAME_MS`] apart) is a burst that meets the rule on its own:
/// [`LOOP_MIN_RUN`] or more words (their median gap is at most one frame
/// by construction), or exactly three in one frame.
fn is_burst(sub: &[TranscriptWord]) -> bool {
    match sub.len() {
        len if len >= LOOP_MIN_RUN => true,
        3 => in_one_frame(sub),
        _ => false,
    }
}

/// Three words that start in the same frame: within half a frame of the
/// first (equal on the runtime's 80 ms grid, whatever the clock map's
/// rounding).
fn in_one_frame(words: &[TranscriptWord]) -> bool {
    words[words.len() - 1].start_ms - words[0].start_ms < LOOP_FRAME_MS / 2
}

/// A token the collapse may touch: a non-empty `norm` that is not digits
/// only and has no Han, Kana or Thai character.
fn eligible(n: &str) -> bool {
    !n.is_empty()
        && !n.chars().all(|c| c.is_numeric() || c.is_whitespace())
        && !n.chars().any(|c| Script::of_char(c).is_continuous())
}

/// The median start-to-start gap of `piece` (two or more words) is at most
/// `limit` (for an even number of gaps, the mean of the middle two).
fn median_gap_at_most(piece: &[TranscriptWord], limit: i64) -> bool {
    let mut gaps: Vec<i64> = piece
        .windows(2)
        .map(|p| p[1].start_ms - p[0].start_ms)
        .collect();
    gaps.sort_unstable();
    let m = gaps.len() / 2;
    if gaps.len() % 2 == 1 {
        gaps[m] <= limit
    } else {
        gaps[m - 1] + gaps[m] <= 2 * limit
    }
}

/// The one word a loop becomes: the first, spanning the piece, with the
/// last word's trailing ASR mark.
fn collapsed(piece: &[TranscriptWord]) -> TranscriptWord {
    let first = &piece[0];
    let last = &piece[piece.len() - 1];
    let (head, _) = split_tail(first.asr_text());
    let (_, tail) = split_tail(last.asr_text());
    let mut w = first.clone();
    w.end_ms = piece
        .iter()
        .map(|w| w.end_ms)
        .max()
        .unwrap_or(first.end_ms)
        .max(first.start_ms);
    w.text = format!("{head}{tail}");
    w.raw_text = None;
    w
}

/// A word's text before and after its trailing marks (everything after the
/// last letter or digit, trailing whitespace dropped).
fn split_tail(text: &str) -> (&str, &str) {
    let t = text.trim_end();
    let end = t
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(0, |(i, c)| i + c.len_utf8());
    t.split_at(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    fn w(start: i64, text: &str, spk: &str) -> TranscriptWord {
        TranscriptWord {
            channel: Channel::Loopback,
            start_ms: start,
            end_ms: start + 80,
            text: text.into(),
            confidence: Some(0.5),
            speaker: spk.into(),
            raw_label: Some(spk.into()),
            raw_text: None,
            margin: None,
            overlap: false,
        }
    }

    /// `n` copies of `text` from `start`, `gap` ms apart.
    fn run(start: i64, gap: i64, n: usize, text: &str) -> Vec<TranscriptWord> {
        (0..n as i64)
            .map(|k| w(start + k * gap, text, "spk_0"))
            .collect()
    }

    fn texts(words: &[TranscriptWord]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    /// Collapses a copy and returns it with the counts.
    fn collapse(words: &[TranscriptWord]) -> (Vec<TranscriptWord>, ChannelLoops) {
        let mut out = words.to_vec();
        let c = collapse_channel(&mut out);
        (out, c)
    }

    #[test]
    fn a_burst_of_24_becomes_one_word_spanning_it_with_the_last_mark() {
        let mut words = vec![w(0, "so", "spk_0"), w(400, "it's", "spk_0")];
        // 24 copies in one frame.
        let mut burst: Vec<TranscriptWord> = (0..24)
            .map(|k| {
                let mut x = w(1_000, "the", "spk_0");
                x.confidence = Some(k as f32 / 100.0);
                x.raw_label = Some(format!("raw{k}"));
                x
            })
            .collect();
        burst[0].text = "The".into();
        burst[5].text = "the,".into();
        burst[23].text = "the.".into();
        burst[23].end_ms = 1_500;
        words.extend(burst);
        words.push(w(2_000, "end", "spk_0"));
        let (out, c) = collapse(&words);
        assert_eq!(
            c,
            ChannelLoops {
                runs: 1,
                words_removed: 23
            }
        );
        assert_eq!(texts(&out), ["so", "it's", "The.", "end"]);
        let kept = &out[2];
        assert_eq!((kept.start_ms, kept.end_ms), (1_000, 1_500));
        assert_eq!(kept.confidence, Some(0.0));
        assert_eq!(kept.raw_label.as_deref(), Some("raw0"));
        assert_eq!(
            (kept.speaker.as_str(), kept.raw_text.as_deref()),
            ("spk_0", None)
        );
        // Everything else as it was.
        assert_eq!(out[0], words[0]);
        assert_eq!(out[1], words[1]);
        assert_eq!(out[3], words[words.len() - 1]);
    }

    #[test]
    fn a_train_of_bursts_keeps_one_word_per_burst() {
        // 24 copies: bursts of 8 in one frame, 160 ms between bursts. The
        // plan's rule takes the piece for a loop (median gap 0) and would
        // keep one word; only the frame-level bursts go, one word each.
        let mut words: Vec<TranscriptWord> = (0..24)
            .map(|k| w(1_000 + (k / 8) * 160, "the", "spk_0"))
            .collect();
        words[7].text = "the,".into();
        words[23].text = "the.".into();
        let (out, c) = collapse(&words);
        assert_eq!(
            c,
            ChannelLoops {
                runs: 3,
                words_removed: 21
            }
        );
        assert_eq!(texts(&out), ["the,", "the", "the."]);
        assert_eq!(
            out.iter()
                .map(|x| (x.start_ms, x.end_ms))
                .collect::<Vec<_>>(),
            [(1_000, 1_080), (1_160, 1_240), (1_320, 1_400)]
        );
    }

    #[test]
    fn real_repeats_after_a_burst_stay() {
        // Five in one frame, then the same word at human pace: the burst
        // goes, the three real ones stay (the piece's median, 0 ms, would
        // have taken all eight).
        let mut words = run(0, 0, 5, "no");
        words.extend([300, 600, 900].map(|t| w(t, "no", "spk_0")));
        let (out, c) = collapse(&words);
        assert_eq!((c.runs, c.words_removed), (1, 4));
        assert_eq!(
            out.iter().map(|x| x.start_ms).collect::<Vec<_>>(),
            [0, 300, 600, 900]
        );
        assert_eq!(out[0].end_ms, 80, "the burst's end, not the piece's");
        assert_eq!(&out[1..], &words[5..]);
        // Three in one frame and a real one 250 ms later: two words.
        let mut words = run(0, 0, 3, "I");
        words.push(w(250, "I", "spk_0"));
        let (out, c) = collapse(&words);
        assert_eq!((texts(&out), c.words_removed), (vec!["I", "I"], 2));
        assert_eq!(out[1], words[3]);
        // Four one frame apart with one gap jittered to 81 ms: the plan's
        // median (80) takes the piece for a loop, but its frame-level
        // sub-runs are pairs, so every word stays; at 81 ms throughout the
        // plan's rule keeps them too.
        for starts in [[0, 80, 161, 241], [0, 81, 162, 243]] {
            let words: Vec<TranscriptWord> = starts.iter().map(|&t| w(t, "and", "spk_0")).collect();
            let (out, c) = collapse(&words);
            assert_eq!((out, c), (words, ChannelLoops::default()), "{starts:?}");
        }
    }

    #[test]
    fn the_marked_words_removed_are_counted() {
        // A burst of four whose first and third are marked, put in time
        // order with the rest: the first is kept, so one marked word went.
        let mut words = vec![w(500, "end", "spk_0"), w(0, "go", "spk_0")];
        words.extend(run(100, 0, 4, "go"));
        let mut marks = vec![true, false, true, false, true, false];
        let (c, removed) = collapse_channel_marked(&mut words, &mut marks);
        assert_eq!((c.words_removed, removed), (3, 1));
        assert_eq!(texts(&words), ["go", "go", "end"]);
        assert_eq!(
            marks,
            [false, true, true],
            "the marks moved with their words"
        );
        // Without a loop only the order changes.
        let mut words = vec![w(300, "b", "spk_0"), w(100, "a", "spk_0")];
        let mut marks = vec![true, false];
        let (c, removed) = collapse_channel_marked(&mut words, &mut marks);
        assert_eq!((c, removed), (ChannelLoops::default(), 0));
        assert_eq!((texts(&words), marks), (vec!["a", "b"], vec![false, true]));
    }

    #[test]
    fn four_at_human_pace_and_pairs_stay() {
        for words in [
            run(0, 240, 4, "no"),
            run(0, 160, 6, "ha"),
            run(0, 0, 2, "the"),
            run(0, 80, 2, "the"),
        ] {
            let (out, c) = collapse(&words);
            assert_eq!(out, words);
            assert_eq!(c, ChannelLoops::default());
        }
        // One frame apart: four or more is a loop (median 80 ms), three is not.
        let (out, c) = collapse(&run(0, 80, 4, "and"));
        assert_eq!((out.len(), c.words_removed), (1, 3));
        assert_eq!(out[0].end_ms, 320);
        let (out, _) = collapse(&run(0, 80, 3, "and"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn three_in_one_frame_collapse_three_one_frame_apart_do_not() {
        let (out, c) = collapse(&run(1_000, 0, 3, "I"));
        assert_eq!((texts(&out), c.runs, c.words_removed), (vec!["I"], 1, 2));
        // Within half a frame (the clock map's rounding) is still one frame.
        let mut jitter = run(1_000, 0, 3, "I");
        jitter[2].start_ms += 1;
        assert_eq!(collapse(&jitter).0.len(), 1);
        for gaps in [[80, 80], [0, 80], [80, 0], [40, 0]] {
            let words = vec![
                w(1_000, "I", "spk_0"),
                w(1_000 + gaps[0], "I", "spk_0"),
                w(1_000 + gaps[0] + gaps[1], "I", "spk_0"),
            ];
            assert_eq!(collapse(&words).0, words, "{gaps:?}");
        }
    }

    #[test]
    fn the_median_decides_the_loop_and_the_bursts_what_goes() {
        let at = |starts: &[i64]| -> Vec<TranscriptWord> {
            starts.iter().map(|&t| w(t, "we", "spk_0")).collect()
        };
        // Gaps 0, 0, 160, 160: the median is 80, a loop; of it the three in
        // one frame go to one word, the two at 160 ms steps stay.
        let words = at(&[0, 0, 0, 160, 320]);
        let (out, c) = collapse(&words);
        assert_eq!(c.words_removed, 2);
        assert_eq!(&out[1..], &words[3..]);
        // Gaps 0, 80, 160, 160: the median is 120, not a loop, so its burst
        // of three one frame apart stays too.
        let words = at(&[0, 0, 80, 240, 400]);
        assert_eq!(collapse(&words).0, words);
        // Gaps 0, 0, 0, 160: a loop; a burst of four one frame wide.
        let words = at(&[0, 0, 0, 80, 240]);
        let (out, c) = collapse(&words);
        assert_eq!((c.words_removed, out.len()), (3, 2));
        assert_eq!(out[0].end_ms, 160);
        // Gaps 240, 0, 0: the median is 0, a loop despite the first gap; the
        // first word stays, the three in one frame become one.
        let words = at(&[0, 240, 240, 240]);
        let (out, _) = collapse(&words);
        assert_eq!(out.iter().map(|x| x.start_ms).collect::<Vec<_>>(), [0, 240]);
        // A burst inside a piece the plan's rule keeps never goes: four in
        // one frame, then five at 300 ms steps (median gap 300), all stay.
        let words = at(&[0, 0, 0, 0, 300, 600, 900, 1_200, 1_500]);
        assert_eq!(collapse(&words).0, words);
    }

    #[test]
    fn text_speaker_and_a_400_ms_pause_split_a_run() {
        // Different normalised texts: two loops, each its own.
        let mut words = run(0, 0, 4, "go");
        words.extend(run(80, 0, 4, "no"));
        let (out, c) = collapse(&words);
        assert_eq!((texts(&out), c.runs), (vec!["go", "no"], 2));
        // Case and punctuation do not: "The", "the," and "THE" are one text.
        let mixed: Vec<TranscriptWord> = ["The", "the,", "THE", "the"]
            .into_iter()
            .map(|t| w(0, t, "spk_0"))
            .collect();
        assert_eq!(texts(&collapse(&mixed).0), ["The"]);
        // Different speaker labels: two pairs, nothing collapsed.
        let words = vec![
            w(0, "yes", "spk_0"),
            w(0, "yes", "spk_0"),
            w(0, "yes", "spk_1"),
            w(0, "yes", "spk_1"),
        ];
        assert_eq!(collapse(&words).0, words);
        // A pause of more than 400 ms ends the piece: the first four collapse,
        // the one after the pause stays.
        let mut words = run(0, 0, 4, "you");
        words.push(w(401, "you", "spk_0"));
        let (out, c) = collapse(&words);
        assert_eq!(c.words_removed, 3);
        assert_eq!(out.iter().map(|w| w.start_ms).collect::<Vec<_>>(), [0, 401]);
        assert_eq!(out[0].end_ms, 80, "the piece's end, not the next word's");
        // Exactly 400 ms is not a pause: one piece of five, median 0, a
        // loop; its burst of four goes, the word 400 ms later stays.
        let mut words = run(0, 0, 4, "you");
        words.push(w(400, "you", "spk_0"));
        let (out, c) = collapse(&words);
        assert_eq!((out.len(), out[0].end_ms, c.words_removed), (2, 80, 3));
        // A pause splits a slow run into pieces that are judged alone:
        // three in one frame, a pause, three in one frame.
        let mut words = run(0, 0, 3, "uh");
        words.extend(run(1_000, 0, 3, "uh"));
        assert_eq!(collapse(&words).0.len(), 2);
    }

    /// The echo gate's `norm` on combining vowel signs, tone marks, control
    /// characters and odd spaces; `tools/eval/test_metrics.py`
    /// (`test_loop_norm_is_the_rust_norm`) checks `levers.py`'s
    /// `loop_norm` against the same pairs, so the measurement and the app
    /// see the same tokens.
    const NORM_PAIRS: [(&str, &str); 9] = [
        ("नहीं", "नहीं"),
        ("ที่", "ที"),
        ("தமிழ்", "தமிழ"),
        ("Ко-ко,", "коко"),
        ("a\u{1c}b", "ab"),
        ("İ", "i"),
        ("Ⅻ", "ⅻ"),
        (" ok\u{a0}", "ok"),
        ("đâu?", "đâu"),
    ];

    #[test]
    fn norm_matches_the_measurement_script() {
        for (text, want) in NORM_PAIRS {
            assert_eq!(norm(text), want, "{text:?}");
        }
        // Devanagari collapses (a script with spaces), Thai never.
        assert_eq!(collapse(&run(0, 0, 4, "नहीं")).0.len(), 1);
        assert_eq!(collapse(&run(0, 0, 4, "ที่")).0.len(), 4);
    }

    #[test]
    fn digits_empty_text_and_continuous_scripts_stay() {
        for t in ["2", "1990", "...", "-", "", "的", "の", "カ", "ที่", "東京"] {
            let words = run(0, 0, 6, t);
            assert_eq!(collapse(&words).0, words, "{t:?}");
        }
        // Letters with digits are words.
        assert_eq!(collapse(&run(0, 0, 4, "2nd")).0.len(), 1);
        // Another script with spaces between words collapses.
        assert_eq!(collapse(&run(0, 0, 4, "да")).0.len(), 1);
    }

    #[test]
    fn every_other_word_keeps_its_count_order_and_times() {
        // A conversation with a loop in the middle, a real stutter and a
        // pair, the words not in time order.
        let mut words = vec![
            w(5_000, "end", "spk_1"),
            w(0, "I", "spk_0"),
            w(240, "I", "spk_0"),
            w(480, "I", "spk_0"),
            w(720, "I", "spk_0"),
            w(900, "think", "spk_0"),
            w(1_200, "that", "spk_1"),
            w(1_200, "that", "spk_1"),
        ];
        words.extend(run(2_000, 0, 5, "is"));
        words.push(w(2_400, "fine", "spk_0"));
        let (out, c) = collapse(&words);
        assert_eq!(c.words_removed, 4);
        let mut expected: Vec<TranscriptWord> =
            words.iter().filter(|x| x.text != "is").cloned().collect();
        let mut is = w(2_000, "is", "spk_0");
        is.end_ms = 2_080;
        expected.push(is);
        expected.sort_by_key(|x| x.start_ms);
        assert_eq!(out, expected);
        // Without a loop nothing changes but the order.
        let plain = vec![w(300, "b", "spk_0"), w(100, "a", "spk_0")];
        let (out, c) = collapse(&plain);
        assert_eq!(texts(&out), ["a", "b"]);
        assert_eq!(c, ChannelLoops::default());
    }

    #[test]
    fn a_punctuated_word_collapses_from_the_asr_text() {
        // A rebuild reads display text with the ASR's in `raw_text`.
        let mut words = run(0, 0, 4, "so");
        words[0].text = "So".into();
        words[0].raw_text = Some("so".into());
        words[3].text = "so.".into();
        words[3].raw_text = None;
        let (out, _) = collapse(&words);
        assert_eq!(
            (out[0].text.as_str(), out[0].raw_text.as_deref()),
            ("so.", None)
        );
        assert_eq!(split_tail("well?! "), ("well", "?!"));
        assert_eq!(split_tail("¿qué"), ("¿qué", ""));
        assert_eq!(split_tail("..."), ("", "..."));
    }

    #[test]
    fn the_report_counts_per_channel_and_adds_the_earlier_runs() {
        let mut channels = vec![
            (Channel::Mic, run(0, 0, 4, "me")),
            (Channel::Loopback, {
                let mut v = run(0, 0, 3, "a");
                v.extend(run(5_000, 0, 6, "b"));
                v
            }),
        ];
        let before = channels.clone();
        let off = collapse_channels(&mut channels, false);
        assert_eq!(channels, before, "off touches nothing");
        assert_eq!(
            off.to_json(),
            serde_json::json!({"applied": false, "runs": 0, "words_removed": 0, "per_channel": {},
                               "rule": {"min_run": 4, "frame_ms": 80, "split_ms": 400}})
        );
        let on = collapse_channels(&mut channels, true);
        assert_eq!((on.applied, on.runs, on.words_removed), (true, 3, 10));
        assert_eq!(
            on.per_channel["mic"],
            ChannelLoops {
                runs: 1,
                words_removed: 3
            }
        );
        assert_eq!(
            on.per_channel["loopback"],
            ChannelLoops {
                runs: 2,
                words_removed: 7
            }
        );
        let info = serde_json::json!({"loops": on.to_json()});
        assert_eq!(LoopReport::of_engine_info(&info), Some(on.clone()));
        assert_eq!(LoopReport::of_engine_info(&serde_json::json!({})), None);
        // Collapsing collapsed words again finds nothing; the earlier counts
        // are added from the run the words came from.
        let again = collapse_channels(&mut channels, true);
        assert_eq!(again.words_removed, 0);
        let total = again.on_top_of(&info);
        assert_eq!(
            (total.applied, total.runs, total.words_removed),
            (true, 3, 10)
        );
        assert_eq!(total.per_channel, on.per_channel);
        // A run from before the collapse has no block: nothing added.
        let fresh = collapse_channels(&mut [(Channel::Source, run(0, 0, 4, "x"))], true);
        assert_eq!(
            fresh
                .clone()
                .on_top_of(&serde_json::json!({"runtime": "x"})),
            fresh
        );
    }

    #[test]
    fn the_setting_is_on_unless_set_off() {
        let store = MemoryStore::new();
        assert!(enabled(&store));
        for (v, on) in [
            ("off", false),
            ("false", false),
            ("0", false),
            ("on", true),
            ("maybe", true),
        ] {
            store
                .set_setting(settings::TRANSCRIPT_COLLAPSE_LOOPS, v)
                .unwrap();
            assert_eq!(enabled(&store), on, "{v}");
        }
    }
}
