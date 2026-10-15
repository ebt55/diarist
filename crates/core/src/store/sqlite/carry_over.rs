//! Carry-over of names, colours and edits from one run to the next
//! (`docs/02-data-model.md`, "Carry-over from the previous current run").
//!
//! 1. Overlap matrix per channel: seconds where a word of old speaker A and a
//!    word of new speaker X overlap. Old speakers with under 5 s of speech are
//!    ignored.
//! 2. Greedy one-to-one matching, highest overlap first, accepted at 60
//!    percent of the smaller speaker's speech time; colour and global id are
//!    copied to the match.
//! 3. Names follow the anchor: an old speaker with a name anchor gives its
//!    name to the new speaker owning most words inside the anchor range; when
//!    that differs from the overlap winner both get `needs_review`. Names
//!    without an anchor (including names inherited through a merge) go to the
//!    overlap winner.
//! 4. Unmatched new speakers get `needs_review`.
//! 5. Merges are not carried structurally.
//! 6. `reassign` edits are replayed in order at word level by word midpoint,
//!    splitting segments where an edit covers only part of one. The target is
//!    resolved by display name; a `user_N` speaker is created when no speaker
//!    of the new run has that name.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, Transaction};

use super::{
    get_run, get_seg_row, list_edits, regroup_segment, run_speakers, segment_words, SqliteStore,
    WordRow,
};
use crate::store::{StoreError, StoreResult};
use crate::types::{Channel, RunId, Speaker, SpeakerId};

/// Old speakers with less speech than this cannot pass names on.
const MIN_SPEECH_MS: i64 = 5_000;
/// A match must cover this share of the smaller speaker's speech.
const MATCH_SHARE: f64 = 0.6;

/// What [`SqliteStore::carry_over`] changed.
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
}

/// One word with the speaker owning its segment.
#[derive(Debug, Clone, Copy)]
struct OwnedWord {
    start: i64,
    end: i64,
    speaker: SpeakerId,
}

fn run_words(tx: &Transaction<'_>, run: RunId) -> StoreResult<BTreeMap<Channel, Vec<OwnedWord>>> {
    let mut stmt = tx.prepare(
        "SELECT sp.channel, w.start_ms, w.end_ms, s.speaker_id
         FROM words w JOIN segments s ON s.id = w.segment_id
         JOIN speakers sp ON sp.id = s.speaker_id
         WHERE s.run_id = ?1 ORDER BY w.start_ms, w.id",
    )?;
    let mut out: BTreeMap<Channel, Vec<OwnedWord>> = BTreeMap::new();
    let rows = stmt.query_map([run.0], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (channel, start, end, speaker) = row?;
        let channel: Channel = channel
            .parse()
            .map_err(|e| StoreError::Backend(format!("speakers.channel: {e}")))?;
        out.entry(channel).or_default().push(OwnedWord {
            start,
            end: end.max(start),
            speaker: SpeakerId(speaker),
        });
    }
    Ok(out)
}

/// Overlap in ms between every (old, new) speaker pair of one channel.
/// Both lists are sorted by start; words of one run do not overlap much, so a
/// sweep with a moving lower bound is enough.
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

impl SqliteStore {
    /// Carries display names, colours, global ids and `reassign` edits from
    /// `from_run` (normally the meeting's current run) to `to_run` (a newer
    /// run of the same meeting). Does not change the current run. One
    /// transaction; see the module docs for the rules.
    pub fn carry_over(&self, from_run: RunId, to_run: RunId) -> StoreResult<CarryOverReport> {
        if from_run == to_run {
            return Err(StoreError::Invalid(
                "carry-over needs two different runs".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let old_run = get_run(&tx, from_run)?;
        let new_run = get_run(&tx, to_run)?;
        if old_run.meeting_id != new_run.meeting_id {
            return Err(StoreError::Invalid(
                "carry-over runs belong to different meetings".into(),
            ));
        }
        let mut report = CarryOverReport::default();

        let old_speakers = run_speakers(&tx, from_run)?;
        let new_speakers = run_speakers(&tx, to_run)?;
        let old_by_id: HashMap<SpeakerId, &Speaker> =
            old_speakers.iter().map(|s| (s.id, s)).collect();
        let old_words = run_words(&tx, from_run)?;
        let new_words = run_words(&tx, to_run)?;

        let mut speech: HashMap<SpeakerId, i64> = HashMap::new();
        for w in old_words.values().chain(new_words.values()).flatten() {
            *speech.entry(w.speaker).or_default() += w.end - w.start;
        }
        let speech_of = |id: SpeakerId| speech.get(&id).copied().unwrap_or(0);

        // 1-2. Overlap matrix per channel and greedy one-to-one matching.
        let mut pairs: Vec<(i64, SpeakerId, SpeakerId)> = Vec::new();
        for (channel, old) in &old_words {
            let Some(new) = new_words.get(channel) else {
                continue;
            };
            for ((a, x), ov) in overlap_matrix(old, new) {
                if speech_of(a) >= MIN_SPEECH_MS {
                    pairs.push((ov, a, x));
                }
            }
        }
        pairs.sort_by(|p, q| q.0.cmp(&p.0).then(p.1.cmp(&q.1)).then(p.2.cmp(&q.2)));
        let mut old_match: HashMap<SpeakerId, SpeakerId> = HashMap::new();
        let mut new_matched: HashSet<SpeakerId> = HashSet::new();
        for (ov, a, x) in pairs {
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

        #[derive(Default, Clone)]
        struct Update {
            name: Option<String>,
            anchor: Option<(i64, i64)>,
            color: Option<String>,
            global_id: Option<i64>,
            review: bool,
        }
        let mut updates: BTreeMap<SpeakerId, Update> = new_speakers
            .iter()
            .map(|s| {
                (
                    s.id,
                    Update {
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

        // 4. Unmatched new speakers need review.
        for s in &new_speakers {
            if !new_matched.contains(&s.id) {
                updates.entry(s.id).or_default().review = true;
            }
        }

        {
            let mut stmt = tx.prepare(
                "UPDATE speakers SET display_name = COALESCE(?2, display_name),
                    name_anchor_start_ms = COALESCE(?3, name_anchor_start_ms),
                    name_anchor_end_ms = COALESCE(?4, name_anchor_end_ms),
                    color = COALESCE(?5, color), global_id = COALESCE(?6, global_id),
                    needs_review = ?7
                 WHERE id = ?1",
            )?;
            for (id, u) in &updates {
                stmt.execute(params![
                    id.0,
                    u.name,
                    u.anchor.map(|a| a.0),
                    u.anchor.map(|a| a.1),
                    u.color,
                    u.global_id,
                    u.review
                ])?;
                if let Some(name) = &u.name {
                    report.named.push((*id, name.clone()));
                }
                if u.review {
                    report.needs_review.push(*id);
                }
            }
        }

        // 6. Replay reassign edits.
        for edit in list_edits(&tx, old_run.meeting_id)? {
            let target = resolve_target(
                &tx,
                to_run,
                edit.channel,
                &edit.payload,
                &old_speakers,
                &old_match,
                &mut report,
            )?;
            let covered = covered_words(&tx, to_run, edit.channel, edit.start_ms, edit.end_ms)?;
            if covered.is_empty() {
                continue;
            }
            report.edits_replayed += 1;
            let mut by_segment: BTreeMap<i64, HashSet<i64>> = BTreeMap::new();
            for (word, seg) in covered {
                by_segment.entry(seg).or_default().insert(word);
            }
            for (seg_id, covered) in by_segment {
                let seg = get_seg_row(&tx, seg_id)?;
                let words = segment_words(&tx, seg_id)?;
                let mut groups: Vec<(&[WordRow], i64)> = Vec::new();
                let mut start = 0;
                for i in 1..=words.len() {
                    let boundary = i == words.len()
                        || covered.contains(&words[i].id) != covered.contains(&words[i - 1].id);
                    if boundary {
                        let speaker = if covered.contains(&words[start].id) {
                            target.0
                        } else {
                            seg.speaker_id
                        };
                        groups.push((&words[start..i], speaker));
                        start = i;
                    }
                }
                if groups.is_empty() {
                    continue;
                }
                if groups.len() > 1 {
                    report.segments_split += 1;
                }
                regroup_segment(&tx, seg, &groups)?;
            }
        }
        report.needs_review.sort();
        report.needs_review.dedup();
        tx.commit()?;
        Ok(report)
    }
}

/// `(word id, segment id)` of words of `run` on `channel` whose midpoint is in `[start, end)`.
fn covered_words(
    tx: &Transaction<'_>,
    run: RunId,
    channel: Channel,
    start: i64,
    end: i64,
) -> StoreResult<Vec<(i64, i64)>> {
    let mut stmt = tx.prepare(
        "SELECT w.id, w.segment_id FROM words w
         JOIN segments s ON s.id = w.segment_id JOIN speakers sp ON sp.id = s.speaker_id
         WHERE s.run_id = ?1 AND sp.channel = ?2
           AND w.start_ms + w.end_ms >= 2 * ?3 AND w.start_ms + w.end_ms < 2 * ?4",
    )?;
    let rows = stmt
        .query_map(params![run.0, channel.as_str(), start, end], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The new-run speaker an edit's words go to: the speaker with the target's
/// display name, else (for an unnamed target) the match of the old speaker
/// with the target's engine label, else a new `user_N` speaker.
fn resolve_target(
    tx: &Transaction<'_>,
    run: RunId,
    channel: Channel,
    payload: &crate::types::ReassignTarget,
    old_speakers: &[Speaker],
    old_match: &HashMap<SpeakerId, SpeakerId>,
    report: &mut CarryOverReport,
) -> StoreResult<SpeakerId> {
    let current = run_speakers(tx, run)?;
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
    let meeting = get_run(tx, run)?.meeting_id;
    tx.execute(
        "INSERT INTO speakers (run_id, meeting_id, channel, engine_label, display_name, needs_review)
         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
        params![
            run.0,
            meeting.0,
            channel.as_str(),
            format!("user_{n}"),
            payload.to_display_name
        ],
    )?;
    let id = SpeakerId(tx.last_insert_rowid());
    report.created_speakers.push(id);
    report.needs_review.push(id);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord, Store};
    use crate::types::{MeetingId, MeetingStatus, RunKind, Source};

    /// A transcript on the loopback channel from `(label, [(first_s, last_s)])`
    /// segments with one 800 ms word per second.
    fn transcript(spec: &[(&str, &[(i64, i64)])]) -> NewTranscript {
        let mut t = NewTranscript::default();
        let mut segs: Vec<(usize, i64, i64)> = Vec::new();
        for (label, ranges) in spec {
            t.speakers.push(NewSpeaker {
                channel: Channel::Loopback,
                engine_label: (*label).into(),
                display_name: None,
                color: None,
                needs_review: false,
            });
            for &(a, b) in *ranges {
                segs.push((t.speakers.len() - 1, a, b));
            }
        }
        segs.sort_by_key(|s| s.1);
        for (i, (spk, a, b)) in segs.into_iter().enumerate() {
            let words: Vec<i64> = (a..b).collect();
            t.segments.push(NewSegment {
                speaker: spk,
                start_ms: a * 1_000,
                end_ms: (b - 1) * 1_000 + 800,
                text: words
                    .iter()
                    .map(|s| format!("w{s}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            });
            for s in words {
                t.words.push(NewWord {
                    segment: i,
                    start_ms: s * 1_000,
                    end_ms: s * 1_000 + 800,
                    text: format!("w{s}"),
                    confidence: None,
                    raw_label: Some((*spec[spk].0).to_owned()),
                    margin: None,
                    overlap: false,
                });
            }
        }
        t
    }

    fn setup(store: &SqliteStore) -> MeetingId {
        store
            .create_meeting(NewMeeting {
                title: "Carry".into(),
                source: Source::Live,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 1,
                status: MeetingStatus::Processing,
            })
            .unwrap()
            .id
    }

    fn run_with(store: &SqliteStore, m: MeetingId, kind: RunKind, t: &NewTranscript) -> RunId {
        let run = store.create_run(m, kind, serde_json::json!({})).unwrap().id;
        store.insert_transcript(run, t).unwrap();
        run
    }

    fn by_label(store: &SqliteStore, run: RunId, label: &str) -> Speaker {
        store
            .run_speakers(run)
            .unwrap()
            .into_iter()
            .find(|s| s.engine_label == label)
            .unwrap()
    }

    fn segment_at(store: &SqliteStore, m: MeetingId, start_ms: i64) -> crate::types::SegmentId {
        store
            .get_segments(m, 0, 1_000)
            .unwrap()
            .into_iter()
            .find(|s| s.start_ms == start_ms)
            .unwrap()
            .id
    }

    #[test]
    fn names_follow_overlap_and_anchor_and_edits_replay() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = setup(&store);
        // Old (live) run: A 0-20 s, B 20-30 + 30-40 s, C 40-43 s (too short),
        // D 43-50 + 50-60 s.
        let old = run_with(
            &store,
            m,
            RunKind::Live,
            &transcript(&[
                ("spk_0", &[(0, 20)]),
                ("spk_1", &[(20, 30), (30, 40)]),
                ("spk_2", &[(40, 43)]),
                ("spk_3", &[(43, 50), (50, 60)]),
            ]),
        );
        store.set_current_run(m, old).unwrap();
        let a = by_label(&store, old, "spk_0");
        let b = by_label(&store, old, "spk_1");
        let c = by_label(&store, old, "spk_2");
        store.rename_speaker(a.id, "Priya", None).unwrap();
        store.set_speaker_color(a.id, "#123456").unwrap();
        // Marcus was renamed from his 30-40 s segment: that is the anchor.
        store
            .rename_speaker(b.id, "Marcus", Some(segment_at(&store, m, 30_000)))
            .unwrap();
        store.rename_speaker(c.id, "Ghost", None).unwrap();
        // The user gave D's 50-60 s segment to Priya.
        store
            .reassign_segment(segment_at(&store, m, 50_000), a.id)
            .unwrap();

        // New (post-pass) run: X 0-20, Y 20-31, Z 31-43, W 43-60 (one segment).
        let new = run_with(
            &store,
            m,
            RunKind::PostPass,
            &transcript(&[
                ("spk_0", &[(0, 20)]),
                ("spk_1", &[(20, 31)]),
                ("spk_2", &[(31, 43)]),
                ("spk_3", &[(43, 60)]),
            ]),
        );
        let report = store.carry_over(old, new).unwrap();
        let x = by_label(&store, new, "spk_0");
        let y = by_label(&store, new, "spk_1");
        let z = by_label(&store, new, "spk_2");
        let w = by_label(&store, new, "spk_3");

        // Overlap: A-X, B-Y (8.8 s beats B-Z 7.2 s), D-W; C is ignored (2.4 s).
        assert!(report.matched.contains(&(a.id, x.id)));
        assert!(report.matched.contains(&(b.id, y.id)));
        assert_eq!(report.matched.len(), 3);
        assert_eq!(x.display_name.as_deref(), Some("Priya"));
        assert_eq!(x.color.as_deref(), Some("#123456"));
        assert!(!x.needs_review);
        // The anchor (30-39.8 s) is Z's: Marcus goes to Z, not the overlap winner Y.
        assert_eq!(z.display_name.as_deref(), Some("Marcus"));
        assert_eq!(z.name_anchor_start_ms, Some(30_000));
        assert_eq!(y.display_name, None);
        assert!(
            y.needs_review && z.needs_review,
            "anchor disagreement flags both"
        );
        // Ghost (under 5 s) names nobody; W matched D (unnamed).
        assert_eq!(w.display_name, None);
        assert!(!w.needs_review);
        assert!(store
            .run_speakers(new)
            .unwrap()
            .iter()
            .all(|s| s.display_name.as_deref() != Some("Ghost")));

        // The reassign of 50-59.8 s replays onto W's 43-60 s segment, split at 50 s.
        assert_eq!(report.edits_replayed, 1);
        assert_eq!(report.segments_split, 1);
        assert!(report.created_speakers.is_empty());
        store.set_current_run(m, new).unwrap();
        let segs = store.get_segments(m, 0, 100).unwrap();
        let tail: Vec<(i64, i64, &str)> = segs
            .iter()
            .filter(|s| s.start_ms >= 43_000)
            .map(|s| (s.start_ms, s.end_ms, s.speaker_name.as_str()))
            .collect();
        assert_eq!(
            tail,
            [(43_000, 49_800, "Speaker 4"), (50_000, 59_800, "Priya")]
        );
        assert!(segs.windows(2).all(|p| p[0].seq + 1 == p[1].seq));
        let words = store.get_words(m, 50_000, 60_000).unwrap();
        let moved = segs.iter().find(|s| s.start_ms == 50_000).unwrap().id;
        assert!(words.iter().all(|w| w.segment_id == moved));
        assert_eq!(store.search("w55", 5).unwrap()[0].speaker_name, "Priya");
    }

    #[test]
    fn edit_to_a_vanished_name_creates_a_user_speaker() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = setup(&store);
        let old = run_with(
            &store,
            m,
            RunKind::Live,
            &transcript(&[("spk_0", &[(0, 10), (10, 20)]), ("spk_1", &[(20, 30)])]),
        );
        store.set_current_run(m, old).unwrap();
        let b = by_label(&store, old, "spk_1");
        store.rename_speaker(b.id, "Temp", None).unwrap();
        store
            .reassign_segment(segment_at(&store, m, 10_000), b.id)
            .unwrap();
        // Renamed afterwards: the edit still says "Temp".
        store.rename_speaker(b.id, "Final", None).unwrap();

        let new = run_with(
            &store,
            m,
            RunKind::PostPass,
            &transcript(&[("spk_0", &[(0, 20)]), ("spk_1", &[(20, 30)])]),
        );
        let report = store.carry_over(old, new).unwrap();
        assert_eq!(report.created_speakers.len(), 1);
        let user = by_label(&store, new, "user_1");
        assert_eq!(user.display_name.as_deref(), Some("Temp"));
        assert!(user.needs_review);
        // Final (old B, 10-30 s after the reassign) matched the new spk_1.
        assert_eq!(
            by_label(&store, new, "spk_1").display_name.as_deref(),
            Some("Final")
        );
        store.set_current_run(m, new).unwrap();
        let names: Vec<(i64, String)> = store
            .get_segments(m, 0, 100)
            .unwrap()
            .into_iter()
            .map(|s| (s.start_ms, s.speaker_name))
            .collect();
        assert_eq!(
            names,
            [
                (0, "Speaker 1".to_owned()),
                (10_000, "Temp".to_owned()),
                (20_000, "Final".to_owned())
            ]
        );
    }

    #[test]
    fn rejects_runs_of_different_meetings() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m1 = setup(&store);
        let m2 = setup(&store);
        let t = transcript(&[("spk_0", &[(0, 10)])]);
        let r1 = run_with(&store, m1, RunKind::Live, &t);
        let r2 = run_with(&store, m2, RunKind::PostPass, &t);
        assert!(matches!(
            store.carry_over(r1, r2),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.carry_over(r1, r1),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.carry_over(r1, RunId(9_999)),
            Err(StoreError::NotFound { .. })
        ));
    }
}
