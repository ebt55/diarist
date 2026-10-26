//! [`CarryBackend`] over one SQLite transaction; the rules live in
//! `store::carry_over`.

use std::collections::BTreeMap;
use std::ops::Range;

use rusqlite::{params, Transaction};

use super::{
    get_run, get_seg_row, list_edits, regroup_segment, run_speakers, segment_words, WordRow,
};
use crate::store::carry_over::{CarryBackend, OwnedWord, SpeakerUpdate};
use crate::store::{StoreError, StoreResult};
use crate::types::{Channel, Edit, MeetingId, RunId, Speaker, SpeakerId, TranscriptRun};

pub(super) struct TxBackend<'a, 'c>(pub &'a Transaction<'c>);

impl CarryBackend for TxBackend<'_, '_> {
    fn run(&mut self, run: RunId) -> StoreResult<TranscriptRun> {
        get_run(self.0, run)
    }

    fn speakers(&mut self, run: RunId) -> StoreResult<Vec<Speaker>> {
        run_speakers(self.0, run)
    }

    fn words(&mut self, run: RunId) -> StoreResult<BTreeMap<Channel, Vec<OwnedWord>>> {
        let mut stmt = self.0.prepare(
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

    fn update_speaker(&mut self, id: SpeakerId, u: &SpeakerUpdate) -> StoreResult<()> {
        self.0.execute(
            "UPDATE speakers SET display_name = COALESCE(?2, display_name),
                name_anchor_start_ms = COALESCE(?3, name_anchor_start_ms),
                name_anchor_end_ms = COALESCE(?4, name_anchor_end_ms),
                color = COALESCE(?5, color), global_id = COALESCE(?6, global_id),
                needs_review = ?7, merged_into = COALESCE(?8, merged_into)
             WHERE id = ?1",
            params![
                id.0,
                u.name,
                u.anchor.map(|a| a.0),
                u.anchor.map(|a| a.1),
                u.color,
                u.global_id,
                u.review,
                u.merged_into.map(|m| m.0)
            ],
        )?;
        Ok(())
    }

    fn edits(&mut self, meeting: MeetingId) -> StoreResult<Vec<Edit>> {
        list_edits(self.0, meeting)
    }

    fn covered_words(
        &mut self,
        run: RunId,
        channel: Channel,
        start: i64,
        end: i64,
    ) -> StoreResult<Vec<(i64, i64)>> {
        let mut stmt = self.0.prepare(
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

    fn segment_words(&mut self, segment: i64) -> StoreResult<(SpeakerId, Vec<i64>)> {
        let seg = get_seg_row(self.0, segment)?;
        let words = segment_words(self.0, segment)?;
        Ok((
            SpeakerId(seg.speaker_id),
            words.iter().map(|w| w.id).collect(),
        ))
    }

    fn regroup(&mut self, segment: i64, groups: &[(Range<usize>, SpeakerId)]) -> StoreResult<()> {
        let seg = get_seg_row(self.0, segment)?;
        let words = segment_words(self.0, segment)?;
        let slices: Vec<(&[WordRow], i64)> = groups
            .iter()
            .map(|(r, s)| (&words[r.clone()], s.0))
            .collect();
        regroup_segment(self.0, seg, &slices)?;
        Ok(())
    }

    fn create_speaker(
        &mut self,
        run: RunId,
        channel: Channel,
        engine_label: &str,
        display_name: Option<&str>,
    ) -> StoreResult<SpeakerId> {
        let meeting = get_run(self.0, run)?.meeting_id;
        self.0.execute(
            "INSERT INTO speakers (run_id, meeting_id, channel, engine_label, display_name, needs_review)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![run.0, meeting.0, channel.as_str(), engine_label, display_name],
        )?;
        Ok(SpeakerId(self.0.last_insert_rowid()))
    }
}
