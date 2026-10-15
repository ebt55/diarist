//! Live preview rows (`live_utterances`) and their materialisation into the
//! live run's segments and words (`docs/02`, "Run lifecycle"). The row format
//! and the transcript rules are shared with `MemoryStore` (`store::live`).

use rusqlite::{params, Connection};

use super::{get_run, run_has_transcript, validate_transcript, write_transcript, SqliteStore};
use crate::protocol::EngineWord;
use crate::store::live::{latest_revisions, live_row, transcript_from_live, LiveWordJson};
use crate::store::{StoreError, StoreResult};
use crate::transcript::SegmentOptions;
use crate::types::{Channel, LiveUtterance, RunId};

fn live_rows(conn: &Connection, run: RunId) -> StoreResult<Vec<LiveUtterance>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel, utt, rev, start_ms, end_ms, raw_label, words_json
         FROM live_utterances WHERE run_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([run.0], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, u32>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(id, channel, utt, rev, start_ms, end_ms, raw_label, json)| {
                let channel: Channel = channel
                    .parse()
                    .map_err(|e| StoreError::Backend(format!("live_utterances.channel: {e}")))?;
                let words: Vec<LiveWordJson> = serde_json::from_str(&json)
                    .map_err(|e| StoreError::Backend(format!("live_utterances.words_json: {e}")))?;
                Ok(LiveUtterance {
                    id,
                    run_id: run,
                    channel,
                    utt,
                    rev,
                    start_ms,
                    end_ms,
                    raw_label,
                    words: words.iter().map(LiveWordJson::to_engine).collect(),
                })
            },
        )
        .collect()
}

impl SqliteStore {
    pub(super) fn append_live_utterance_impl(
        &self,
        run: RunId,
        channel: Channel,
        utt: u32,
        rev: u32,
        words: &[EngineWord],
    ) -> StoreResult<i64> {
        let row = live_row(words);
        let words_json = serde_json::to_string(&row.words)
            .map_err(|e| StoreError::Backend(format!("json: {e}")))?;
        let conn = self.lock()?;
        get_run(&conn, run)?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO live_utterances
                (run_id, channel, utt, rev, start_ms, end_ms, raw_label, words_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run.0,
                channel.as_str(),
                utt,
                rev,
                row.start_ms,
                row.end_ms,
                row.raw_label,
                words_json
            ],
        )?;
        if n == 0 {
            return Err(StoreError::Conflict(format!(
                "utterance {utt} rev {rev} on {channel} already stored for run {run}"
            )));
        }
        Ok(conn.last_insert_rowid())
    }

    pub(super) fn list_live_utterances_impl(&self, run: RunId) -> StoreResult<Vec<LiveUtterance>> {
        let conn = self.lock()?;
        get_run(&conn, run)?;
        live_rows(&conn, run)
    }

    pub(super) fn materialise_live_run_impl(
        &self,
        run: RunId,
        opts: &SegmentOptions,
    ) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let meeting = get_run(&tx, run)?.meeting_id;
        if run_has_transcript(&tx, run)? {
            return Err(StoreError::Conflict(format!(
                "run {run} already has a transcript"
            )));
        }
        let rows = live_rows(&tx, run)?;
        let transcript = transcript_from_live(&latest_revisions(&rows), opts);
        validate_transcript(&transcript)?;
        write_transcript(&tx, run, meeting, &transcript)?;
        tx.commit()?;
        Ok(())
    }
}
