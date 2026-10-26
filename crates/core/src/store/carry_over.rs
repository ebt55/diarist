//! Carry-over of names, colours and edits from one run to the next
//! (`docs/02-data-model.md`, "Carry-over from the previous current run"),
//! written once over [`CarryBackend`] so `SqliteStore` (inside a
//! transaction) and `MemoryStore` (under its lock) share the rules.
//!
//! 0. Alignment per channel: the new run's word times may sit a constant
//!    amount off the old run's (the post-pass's file lookahead puts word
//!    starts 80 to 190 ms earlier than live, docs/research/12, "Word
//!    times"). The shift within 500 ms at which the old words overlap the
//!    new ones most (ties: the smallest) is applied to the old run's word
//!    times, name anchors and edit ranges before everything below. Runs whose
//!    words line up keep shift 0, so nothing changes for them.
//! 1. Overlap matrix per channel: milliseconds where a speech region of old
//!    speaker A and one of new speaker X overlap. Regions are built in
//!    channel order ([`regions`]): consecutive words of one speaker at most
//!    [`JOIN_GAP_MS`] apart, ended by any word of another speaker (so a
//!    region never swallows a backchannel), then widened by
//!    [`REGION_PAD_MS`] on both sides, never past half the gap to the next
//!    word of another speaker, and not below 0. Word times alone do not work:
//!    both engines emit words as short spikes (median 80 ms, one encoder
//!    frame) and the post-pass places each word a little differently from
//!    the live run (median 160 ms earlier, 400 ms earlier to 0 at p10 / p90,
//!    docs/research/12, "Word times"), so after step 0 only about half of the
//!    spike time overlaps even when both runs agree on every speaker.
//!    `spk_unknown` rows take no part on either side. Speakers with under
//!    [`MIN_SPEECH_MS`] of region time, old or new, are not matched (a tiny
//!    new label inside a long live speaker would otherwise cover nearly all
//!    of its own time and take the name).
//! 2. Greedy one-to-one matching, highest overlap first, accepted at 60
//!    percent of the smaller speaker's region time; colour and global id are
//!    copied to the match.
//! 3. Names follow the anchor: an old speaker with a name anchor (the
//!    segment renamed from, or the utterance range of a rename made while
//!    recording, `Store::rename_speaker_at`) gives its name to the new
//!    speaker owning most words inside the anchor range, however little the
//!    old speaker said (the region-time minimum applies only to step 1);
//!    when that differs from the overlap winner both get `needs_review`.
//!    Names without an anchor (including names inherited through a merge) go
//!    to the overlap winner.
//!
//!    Fallback for names that neither an anchor nor a match placed (a
//!    legend rename of a short talker, or of a speaker the post-pass split):
//!    larger old speakers first, the name goes to the new speaker with the
//!    most region overlap with the old speaker, when that overlap is at least
//!    [`FALLBACK_SHARE`] of the old speaker's own region time; never onto a
//!    speaker that already received a name, never a name already given, and
//!    the receiver gets `needs_review`.
//! 4. Unmatched new speakers get `needs_review`.
//! 5. Merges are not carried structurally.
//! 6. `reassign` edits are replayed in order at word level by word midpoint,
//!    splitting segments where an edit covers only part of one. The target is
//!    resolved by display name; a `user_N` speaker is created when no speaker
//!    of the new run has that name.
//!
//! [`carry_over_identity`] is the carry for a run rebuilt from another
//! one's words and labels (punctuation added or removed, echo words
//! restored, plan 16 section 3.9): speakers are matched by `(channel,
//! engine_label)`, with no alignment and no region minimum, and keep their
//! name, colour, global id, merge, `needs_review` and name anchor; then the
//! `reassign` edits are replayed as in step 6.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use super::{StoreError, StoreResult};
use crate::transcript::UNKNOWN_LABEL;
use crate::types::{
    Channel, Edit, MeetingId, ReassignTarget, RunId, Speaker, SpeakerId, TranscriptRun,
};

/// Speakers (old or new) with less region time than this are not matched
/// (steps 1-2). Region time is about 2.1 times word time on recorded speech
/// (two replayed meetings, 12 and 34 minutes, at the constants below), so
/// this keeps roughly the earlier minimum of 5 s of word time: about 20 s of
/// talk.
const MIN_SPEECH_MS: i64 = 10_000;
/// A match must cover this share of the smaller speaker's region time.
const MATCH_SHARE: f64 = 0.6;
/// Consecutive words of one speaker at most this far apart are one speech
/// region. Chosen with [`REGION_PAD_MS`] from join gaps 150 / 300 / 500 and
/// pads 0 / 100 / 200 ms: the smallest pair that matched every speaker of
/// the two replayed meetings at 75 percent or more (78 to 84) and kept every
/// wrong pair under 40 percent (7 at most) on 200 seeded adversarial
/// meetings, whose halfway label swap stays at 58 percent at most.
const JOIN_GAP_MS: i64 = 150;
/// Each speech region is widened by this on both sides (at most half the
/// gap to the next word of another speaker).
const REGION_PAD_MS: i64 = 100;
/// The fallback of step 3 needs this share of the old speaker's own region
/// time.
const FALLBACK_SHARE: f64 = 0.5;
/// The largest shift between the runs' word times that alignment looks for.
const MAX_SHIFT_MS: i64 = 500;
/// The alignment's step.
const SHIFT_STEP_MS: i64 = 10;

/// What [`super::RunStore::carry_over`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarryOverReport {
    /// (old speaker, new speaker) pairs accepted by the overlap matching.
    pub matched: Vec<(SpeakerId, SpeakerId)>,
    /// New speakers that received a display name.
    pub named: Vec<(SpeakerId, String)>,
    /// New speakers flagged `needs_review`, ascending.
    pub needs_review: Vec<SpeakerId>,
    /// `reassign` edits replayed.
    pub edits_replayed: usize,
    /// Segments that had to be split by an edit.
    pub segments_split: usize,
    /// `user_N` speakers created for edit targets nobody in the new run is named.
    pub created_speakers: Vec<SpeakerId>,
    /// Per channel, the shift (ms) applied to the old run's times to line
    /// them up with the new run's (negative: the new words come earlier).
    pub shift_ms: BTreeMap<Channel, i64>,
}

/// One word with the speaker owning its segment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OwnedWord {
    pub start: i64,
    pub end: i64,
    pub speaker: SpeakerId,
}

/// Values copied onto a new speaker; `None` keeps the current value.
#[derive(Debug, Default, Clone)]
pub(crate) struct SpeakerUpdate {
    pub name: Option<String>,
    pub anchor: Option<(i64, i64)>,
    pub color: Option<String>,
    pub global_id: Option<i64>,
    /// The merge root (a speaker of the same run).
    pub merged_into: Option<SpeakerId>,
    /// Always written.
    pub review: bool,
}

/// The primitives carry-over needs from a store. Segment and word ids are raw
/// `i64`s; word lists are in `(start_ms, id)` order.
pub(crate) trait CarryBackend {
    fn run(&mut self, run: RunId) -> StoreResult<TranscriptRun>;
    fn speakers(&mut self, run: RunId) -> StoreResult<Vec<Speaker>>;
    /// Words of the run per channel (the channel of the segment's speaker),
    /// sorted by start.
    fn words(&mut self, run: RunId) -> StoreResult<BTreeMap<Channel, Vec<OwnedWord>>>;
    fn update_speaker(&mut self, id: SpeakerId, u: &SpeakerUpdate) -> StoreResult<()>;
    fn edits(&mut self, meeting: MeetingId) -> StoreResult<Vec<Edit>>;
    /// `(word id, segment id)` of words of `run` on `channel` whose midpoint
    /// is in `[start, end)`.
    fn covered_words(
        &mut self,
        run: RunId,
        channel: Channel,
        start: i64,
        end: i64,
    ) -> StoreResult<Vec<(i64, i64)>>;
    /// The segment's speaker and its word ids in order.
    fn segment_words(&mut self, segment: i64) -> StoreResult<(SpeakerId, Vec<i64>)>;
    /// Rewrites one segment as consecutive groups of its words (`groups`
    /// covers all of them, in order, with a speaker each): the first group
    /// keeps the row, the others become new segments right after it.
    fn regroup(&mut self, segment: i64, groups: &[(Range<usize>, SpeakerId)]) -> StoreResult<()>;
    /// A new speaker row of `run` with `needs_review = true`.
    fn create_speaker(
        &mut self,
        run: RunId,
        channel: Channel,
        engine_label: &str,
        display_name: Option<&str>,
    ) -> StoreResult<SpeakerId>;
}

/// Speech regions of one channel's words (sorted by start, every speaker of
/// the channel, `spk_unknown` included). In channel order, consecutive words
/// of one speaker at most [`JOIN_GAP_MS`] apart form a region; the next word
/// of another speaker ends it. Each region is widened by [`REGION_PAD_MS`]
/// on both sides, but never by more than half the gap to the nearest word of
/// another speaker (none when they overlap) and never below 0; regions of
/// one speaker that then touch are merged. Sorted by start.
fn regions(words: &[OwnedWord]) -> Vec<OwnedWord> {
    let n = words.len();
    // The nearest word of another speaker before / after each word.
    let mut prev_other: Vec<Option<usize>> = vec![None; n];
    for i in 1..n {
        prev_other[i] = if words[i - 1].speaker != words[i].speaker {
            Some(i - 1)
        } else {
            prev_other[i - 1]
        };
    }
    let mut next_other: Vec<Option<usize>> = vec![None; n];
    for i in (0..n.saturating_sub(1)).rev() {
        next_other[i] = if words[i + 1].speaker != words[i].speaker {
            Some(i + 1)
        } else {
            next_other[i + 1]
        };
    }
    let mut by: BTreeMap<SpeakerId, Vec<(i64, i64)>> = BTreeMap::new();
    let mut i = 0;
    while i < n {
        let speaker = words[i].speaker;
        let (start, mut end, mut j) = (words[i].start, words[i].end, i);
        while j + 1 < n
            && words[j + 1].speaker == speaker
            && words[j + 1].start - end <= JOIN_GAP_MS
        {
            j += 1;
            end = end.max(words[j].end);
        }
        let left = prev_other[i].map_or(REGION_PAD_MS, |k| {
            ((start - words[k].end).max(0) / 2).min(REGION_PAD_MS)
        });
        let right = next_other[j].map_or(REGION_PAD_MS, |k| {
            ((words[k].start - end).max(0) / 2).min(REGION_PAD_MS)
        });
        let (start, end) = ((start - left).max(0), end + right);
        let own = by.entry(speaker).or_default();
        match own.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => own.push((start, end)),
        }
        i = j + 1;
    }
    let mut out: Vec<OwnedWord> = by
        .into_iter()
        .flat_map(|(speaker, rs)| {
            rs.into_iter().map(move |(start, end)| OwnedWord {
                start,
                end,
                speaker,
            })
        })
        .collect();
    // `overlap_matrix` sweeps in start order.
    out.sort_by_key(|r| (r.start, r.end, r.speaker));
    out
}

/// Overlap in ms between every (old, new) speaker pair of one channel. Both
/// lists are sorted by start; an item is skipped only once it ends before
/// the current old item starts, so overlapping items in one list are fine.
fn overlap_matrix(old: &[OwnedWord], new: &[OwnedWord]) -> HashMap<(SpeakerId, SpeakerId), i64> {
    let mut m: HashMap<(SpeakerId, SpeakerId), i64> = HashMap::new();
    let mut lo = 0usize;
    for a in old {
        while lo < new.len() && new[lo].end <= a.start {
            lo += 1;
        }
        let mut k = lo;
        while k < new.len() && new[k].start < a.end {
            let b = &new[k];
            let ov = a.end.min(b.end) - a.start.max(b.start);
            if ov > 0 {
                *m.entry((a.speaker, b.speaker)).or_default() += ov;
            }
            k += 1;
        }
    }
    m
}

/// Milliseconds where the words of `old`, moved by `shift`, overlap words of
/// `new` (speakers ignored). Both sorted by start, as in [`overlap_matrix`].
fn total_overlap(old: &[OwnedWord], new: &[OwnedWord], shift: i64) -> i64 {
    let mut sum = 0;
    let mut lo = 0usize;
    for a in old {
        let (start, end) = (a.start + shift, a.end + shift);
        while lo < new.len() && new[lo].end <= start {
            lo += 1;
        }
        let mut k = lo;
        while k < new.len() && new[k].start < end {
            sum += (end.min(new[k].end) - start.max(new[k].start)).max(0);
            k += 1;
        }
    }
    sum
}

/// The shift (multiple of [`SHIFT_STEP_MS`], at most [`MAX_SHIFT_MS`]) that
/// lines `old` up with `new` best: most overlap, the smallest shift on a
/// tie (0 when the words already line up), the earlier one between equals.
fn alignment_shift(old: &[OwnedWord], new: &[OwnedWord]) -> i64 {
    let mut best = (total_overlap(old, new, 0), 0);
    for step in 1..=MAX_SHIFT_MS / SHIFT_STEP_MS {
        for shift in [-step * SHIFT_STEP_MS, step * SHIFT_STEP_MS] {
            let ov = total_overlap(old, new, shift);
            if ov > best.0 {
                best = (ov, shift);
            }
        }
    }
    best.1
}

/// The carry-over rules (module docs) over any backend. The caller provides
/// atomicity (a transaction, a held lock).
pub(crate) fn carry_over(
    b: &mut dyn CarryBackend,
    from_run: RunId,
    to_run: RunId,
) -> StoreResult<CarryOverReport> {
    if from_run == to_run {
        return Err(StoreError::Invalid(
            "carry-over needs two different runs".into(),
        ));
    }
    let old_run = b.run(from_run)?;
    let new_run = b.run(to_run)?;
    if old_run.meeting_id != new_run.meeting_id {
        return Err(StoreError::Invalid(
            "carry-over runs belong to different meetings".into(),
        ));
    }
    let mut report = CarryOverReport::default();

    let old_speakers = b.speakers(from_run)?;
    let new_speakers = b.speakers(to_run)?;
    let old_by_id: HashMap<SpeakerId, &Speaker> = old_speakers.iter().map(|s| (s.id, s)).collect();
    let mut old_words = b.words(from_run)?;
    let new_words = b.words(to_run)?;

    // 0. Line the old run's times up with the new run's, per channel.
    let mut shifts: BTreeMap<Channel, i64> = BTreeMap::new();
    for (channel, old) in old_words.iter_mut() {
        let Some(new) = new_words.get(channel) else {
            continue;
        };
        let shift = alignment_shift(old, new);
        for w in old.iter_mut() {
            w.start += shift;
            w.end += shift;
        }
        shifts.insert(*channel, shift);
    }
    report.shift_ms = shifts.clone();
    let shift_of = |c: Channel| shifts.get(&c).copied().unwrap_or(0);

    // 1-2. Speech regions (`spk_unknown` breaks and bounds them but takes no
    // part), overlap matrix per channel and greedy one-to-one matching, all
    // on region time.
    let unknown: HashSet<SpeakerId> = old_speakers
        .iter()
        .chain(&new_speakers)
        .filter(|s| s.engine_label == UNKNOWN_LABEL)
        .map(|s| s.id)
        .collect();
    let regions_of =
        |words: &BTreeMap<Channel, Vec<OwnedWord>>| -> BTreeMap<Channel, Vec<OwnedWord>> {
            words
                .iter()
                .map(|(c, w)| {
                    let mut r = regions(w);
                    r.retain(|r| !unknown.contains(&r.speaker));
                    (*c, r)
                })
                .collect()
        };
    let (old_regions, new_regions) = (regions_of(&old_words), regions_of(&new_words));
    let mut speech: HashMap<SpeakerId, i64> = HashMap::new();
    for r in old_regions.values().chain(new_regions.values()).flatten() {
        *speech.entry(r.speaker).or_default() += r.end - r.start;
    }
    let speech_of = |id: SpeakerId| speech.get(&id).copied().unwrap_or(0);

    // Every overlapping (old, new) pair; the matching takes those where both
    // speakers have the minimum, the name fallback (step 3) any of them.
    let mut overlaps: Vec<(i64, SpeakerId, SpeakerId)> = Vec::new();
    for (channel, old) in &old_regions {
        let Some(new) = new_regions.get(channel) else {
            continue;
        };
        overlaps.extend(
            overlap_matrix(old, new)
                .into_iter()
                .map(|((a, x), ov)| (ov, a, x)),
        );
    }
    overlaps.sort_by(|p, q| q.0.cmp(&p.0).then(p.1.cmp(&q.1)).then(p.2.cmp(&q.2)));
    let mut old_match: HashMap<SpeakerId, SpeakerId> = HashMap::new();
    let mut new_matched: HashSet<SpeakerId> = HashSet::new();
    for &(ov, a, x) in &overlaps {
        if speech_of(a) < MIN_SPEECH_MS || speech_of(x) < MIN_SPEECH_MS {
            continue;
        }
        if old_match.contains_key(&a) || new_matched.contains(&x) {
            continue;
        }
        let smaller = speech_of(a).min(speech_of(x));
        if smaller > 0 && ov as f64 >= MATCH_SHARE * smaller as f64 {
            old_match.insert(a, x);
            new_matched.insert(x);
            report.matched.push((a, x));
        }
    }

    // What the user sees for an old speaker: its merge root's name, colour, identity.
    let root_of = |s: &Speaker| -> Speaker {
        s.merged_into
            .and_then(|r| old_by_id.get(&r))
            .map(|r| (*r).clone())
            .unwrap_or_else(|| s.clone())
    };

    let mut updates: BTreeMap<SpeakerId, SpeakerUpdate> = new_speakers
        .iter()
        .map(|s| {
            (
                s.id,
                SpeakerUpdate {
                    review: s.needs_review,
                    ..Default::default()
                },
            )
        })
        .collect();

    for &(a, x) in &report.matched {
        let root = root_of(old_by_id[&a]);
        let u = updates.entry(x).or_default();
        u.color = root.color.clone();
        u.global_id = root.global_id;
    }

    // 3. Names: anchors first, then overlap winners.
    let mut named_by_anchor: HashSet<SpeakerId> = HashSet::new();
    let mut handled: HashSet<SpeakerId> = HashSet::new();
    for s in &old_speakers {
        let (Some(name), None, Some(a0), Some(a1)) = (
            s.display_name.clone(),
            s.merged_into,
            s.name_anchor_start_ms,
            s.name_anchor_end_ms,
        ) else {
            continue;
        };
        // In the new run's time, which the new speaker keeps.
        let (a0, a1) = (a0 + shift_of(s.channel), a1 + shift_of(s.channel));
        let words = new_words.get(&s.channel).map(Vec::as_slice).unwrap_or(&[]);
        let mut counts: BTreeMap<SpeakerId, usize> = BTreeMap::new();
        for w in words {
            let mid2 = w.start + w.end;
            if mid2 >= 2 * a0 && mid2 <= 2 * a1 {
                *counts.entry(w.speaker).or_default() += 1;
            }
        }
        let owner = counts
            .iter()
            .max_by(|p, q| p.1.cmp(q.1).then(q.0.cmp(p.0)))
            .map(|(id, _)| *id);
        let Some(owner) = owner else {
            continue; // nobody speaks there now: fall back to the overlap winner
        };
        handled.insert(s.id);
        named_by_anchor.insert(owner);
        let u = updates.entry(owner).or_default();
        u.name = Some(name);
        u.anchor = Some((a0, a1));
        if let Some(&winner) = old_match.get(&s.id) {
            if winner != owner {
                u.review = true;
                updates.entry(winner).or_default().review = true;
            }
        }
    }
    for &(a, x) in &report.matched {
        if handled.contains(&a) || named_by_anchor.contains(&x) {
            continue;
        }
        let root = root_of(old_by_id[&a]);
        if let Some(name) = root.display_name {
            let u = updates.entry(x).or_default();
            if u.name.is_none() {
                u.name = Some(name);
            }
        }
    }
    // Fallback: names neither an anchor nor a match placed, larger old
    // speakers first, to the new speaker with the most region overlap when
    // it covers FALLBACK_SHARE of the old speaker's own region time.
    let mut given: HashSet<String> = updates.values().filter_map(|u| u.name.clone()).collect();
    let mut unplaced: Vec<(&Speaker, String)> = old_speakers
        .iter()
        .filter(|s| !handled.contains(&s.id) && !old_match.contains_key(&s.id))
        .filter_map(|s| root_of(s).display_name.map(|n| (s, n)))
        .collect();
    unplaced.sort_by(|p, q| {
        speech_of(q.0.id)
            .cmp(&speech_of(p.0.id))
            .then(p.0.id.cmp(&q.0.id))
    });
    for (s, name) in unplaced {
        if given.contains(&name) {
            continue;
        }
        // `overlaps` is sorted by overlap, largest first.
        let Some(&(ov, _, x)) = overlaps.iter().find(|p| p.1 == s.id) else {
            continue;
        };
        let own = speech_of(s.id);
        let u = updates.entry(x).or_default();
        if own > 0 && ov as f64 >= FALLBACK_SHARE * own as f64 && u.name.is_none() {
            u.name = Some(name.clone());
            u.review = true;
            given.insert(name);
        }
    }

    // 4. Unmatched new speakers need review.
    for s in &new_speakers {
        if !new_matched.contains(&s.id) {
            updates.entry(s.id).or_default().review = true;
        }
    }

    for (id, u) in &updates {
        b.update_speaker(*id, u)?;
        if let Some(name) = &u.name {
            report.named.push((*id, name.clone()));
        }
        if u.review {
            report.needs_review.push(*id);
        }
    }

    // 6. Replay reassign edits.
    replay_edits(
        b,
        old_run.meeting_id,
        to_run,
        &old_speakers,
        &old_match,
        &shift_of,
        &mut report,
    )?;
    report.needs_review.sort();
    report.needs_review.dedup();
    Ok(report)
}

/// Step 6: replays the meeting's `reassign` edits onto `to_run` at word
/// level (old run times moved by `shift_of(channel)`).
fn replay_edits(
    b: &mut dyn CarryBackend,
    meeting: MeetingId,
    to_run: RunId,
    old_speakers: &[Speaker],
    old_match: &HashMap<SpeakerId, SpeakerId>,
    shift_of: &dyn Fn(Channel) -> i64,
    report: &mut CarryOverReport,
) -> StoreResult<()> {
    for edit in b.edits(meeting)? {
        let target = resolve_target(
            b,
            to_run,
            edit.channel,
            &edit.payload,
            old_speakers,
            old_match,
            report,
        )?;
        let shift = shift_of(edit.channel);
        let covered = b.covered_words(
            to_run,
            edit.channel,
            edit.start_ms + shift,
            edit.end_ms + shift,
        )?;
        if covered.is_empty() {
            continue;
        }
        report.edits_replayed += 1;
        let mut by_segment: BTreeMap<i64, HashSet<i64>> = BTreeMap::new();
        for (word, seg) in covered {
            by_segment.entry(seg).or_default().insert(word);
        }
        for (seg_id, covered) in by_segment {
            let (seg_speaker, words) = b.segment_words(seg_id)?;
            let mut groups: Vec<(Range<usize>, SpeakerId)> = Vec::new();
            let mut start = 0;
            for i in 1..=words.len() {
                let boundary = i == words.len()
                    || covered.contains(&words[i]) != covered.contains(&words[i - 1]);
                if boundary {
                    let speaker = if covered.contains(&words[start]) {
                        target
                    } else {
                        seg_speaker
                    };
                    groups.push((start..i, speaker));
                    start = i;
                }
            }
            if groups.is_empty() {
                continue;
            }
            if groups.len() > 1 {
                report.segments_split += 1;
            }
            b.regroup(seg_id, &groups)?;
        }
    }
    Ok(())
}

/// The identity carry (module docs) over any backend: `to_run` was built
/// from `from_run`'s words with the same labels. The caller provides
/// atomicity.
pub(crate) fn carry_over_identity(
    b: &mut dyn CarryBackend,
    from_run: RunId,
    to_run: RunId,
) -> StoreResult<CarryOverReport> {
    if from_run == to_run {
        return Err(StoreError::Invalid(
            "carry-over needs two different runs".into(),
        ));
    }
    let old_run = b.run(from_run)?;
    let new_run = b.run(to_run)?;
    if old_run.meeting_id != new_run.meeting_id {
        return Err(StoreError::Invalid(
            "carry-over runs belong to different meetings".into(),
        ));
    }
    let mut report = CarryOverReport::default();
    let old_speakers = b.speakers(from_run)?;
    let new_speakers = b.speakers(to_run)?;
    let new_by_key: HashMap<(Channel, &str), SpeakerId> = new_speakers
        .iter()
        .map(|s| ((s.channel, s.engine_label.as_str()), s.id))
        .collect();
    let mut old_match: HashMap<SpeakerId, SpeakerId> = HashMap::new();
    for s in &old_speakers {
        if let Some(&x) = new_by_key.get(&(s.channel, s.engine_label.as_str())) {
            old_match.insert(s.id, x);
            report.matched.push((s.id, x));
        }
    }
    for s in &old_speakers {
        let Some(&x) = old_match.get(&s.id) else {
            continue;
        };
        let anchor = match (s.name_anchor_start_ms, s.name_anchor_end_ms) {
            (Some(a0), Some(a1)) => Some((a0, a1)),
            _ => None,
        };
        let u = SpeakerUpdate {
            name: s.display_name.clone(),
            anchor,
            color: s.color.clone(),
            global_id: s.global_id,
            merged_into: s
                .merged_into
                .and_then(|r| old_match.get(&r).copied())
                .filter(|&r| r != x),
            review: s.needs_review,
        };
        b.update_speaker(x, &u)?;
        if let Some(name) = &u.name {
            report.named.push((x, name.clone()));
        }
        if u.review {
            report.needs_review.push(x);
        }
    }
    for c in [Channel::Mic, Channel::Loopback, Channel::Source] {
        if new_speakers.iter().any(|s| s.channel == c) {
            report.shift_ms.insert(c, 0);
        }
    }
    replay_edits(
        b,
        old_run.meeting_id,
        to_run,
        &old_speakers,
        &old_match,
        &|_| 0,
        &mut report,
    )?;
    report.needs_review.sort();
    report.needs_review.dedup();
    Ok(report)
}

/// The new-run speaker an edit's words go to: the speaker with the target's
/// display name, else (for an unnamed target) the match of the old speaker
/// with the target's engine label, else a new `user_N` speaker.
fn resolve_target(
    b: &mut dyn CarryBackend,
    run: RunId,
    channel: Channel,
    payload: &ReassignTarget,
    old_speakers: &[Speaker],
    old_match: &HashMap<SpeakerId, SpeakerId>,
    report: &mut CarryOverReport,
) -> StoreResult<SpeakerId> {
    let current = b.speakers(run)?;
    if let Some(name) = &payload.to_display_name {
        if let Some(s) = current
            .iter()
            .filter(|s| s.channel == channel && s.display_name.as_deref() == Some(name.as_str()))
            .min_by_key(|s| (s.merged_into.is_some(), s.id))
        {
            return Ok(s.merged_into.unwrap_or(s.id));
        }
    } else if let Some(label) = &payload.to_engine_label {
        let matched = old_speakers
            .iter()
            .find(|s| s.channel == channel && &s.engine_label == label)
            .and_then(|s| old_match.get(&s.id));
        if let Some(&x) = matched {
            return Ok(x);
        }
    }
    // Nobody fits: create user_N rather than dropping the edit.
    let used: HashSet<u32> = current
        .iter()
        .filter(|s| s.channel == channel)
        .filter_map(|s| s.engine_label.strip_prefix("user_")?.parse().ok())
        .collect();
    let n = (1..).find(|n| !used.contains(n)).unwrap_or(1);
    let id = b.create_speaker(
        run,
        channel,
        &format!("user_{n}"),
        payload.to_display_name.as_deref(),
    )?;
    report.created_speakers.push(id);
    report.needs_review.push(id);
    Ok(id)
}

/// Segment bounds of group `i` of `n` when a segment `[seg_start, seg_end]`
/// is regrouped: the first group keeps the segment's start, the last its end;
/// inner boundaries come from the words.
pub(crate) fn group_bounds(
    i: usize,
    n: usize,
    seg_start: i64,
    seg_end: i64,
    words: &[(i64, i64)],
) -> (i64, i64) {
    let start = if i == 0 {
        seg_start
    } else {
        words.first().map(|w| w.0).unwrap_or(seg_start)
    };
    let end = if i + 1 == n {
        seg_end
    } else {
        words.iter().map(|w| w.1).max().unwrap_or(start)
    };
    (start, end.max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: i64, end: i64, speaker: i64) -> OwnedWord {
        OwnedWord {
            start,
            end,
            speaker: SpeakerId(speaker),
        }
    }

    fn spans(words: &[OwnedWord]) -> Vec<(i64, i64, i64)> {
        regions(words)
            .into_iter()
            .map(|r| (r.start, r.end, r.speaker.0))
            .collect()
    }

    #[test]
    fn another_speaker_ends_a_region_and_caps_the_padding() {
        let words = [
            w(0, 80, 1),
            w(150, 230, 1), // 70 ms on: same region
            w(300, 380, 2), // a backchannel 70 ms after
            w(400, 480, 1), // 20 ms after it: a new region of 1
        ];
        // Padding: 100 ms where nobody else is near, half the gap otherwise
        // (35 ms either side of the 70 ms gaps, 10 ms of the 20 ms one).
        assert_eq!(spans(&words), [(0, 265, 1), (265, 390, 2), (390, 580, 1)]);
    }

    #[test]
    fn padded_regions_of_one_speaker_merge() {
        // 160 ms apart: two regions (over 150 ms), touching once padded.
        assert_eq!(
            spans(&[w(1_000, 1_080, 7), w(1_240, 1_320, 7)]),
            [(900, 1_420, 7)]
        );
    }

    #[test]
    fn overlapping_speakers_get_no_padding_between_them() {
        assert_eq!(
            spans(&[w(0, 500, 1), w(400, 600, 2)]),
            [(0, 500, 1), (400, 700, 2)]
        );
        assert!(regions(&[]).is_empty());
    }
}
