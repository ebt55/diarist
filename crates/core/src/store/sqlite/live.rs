//! Live preview rows (`live_utterances`) and their materialisation into the
//! live run's segments and words (`docs/02`, "Run lifecycle").

use std::collections::BTreeMap;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{get_run, run_has_transcript, validate_transcript, write_transcript, SqliteStore};
use crate::protocol::EngineWord;
use crate::store::{StoreError, StoreResult};
use crate::transcript::{
    build_transcript, interleave, us_to_ms, SegmentOptions, TranscriptWord, ME_LABEL, UNKNOWN_LABEL,
};
use crate::types::{Channel, RunId};

/// One word in `live_utterances.words_json` (`[{t0,t1,text,conf,spk}]`, ms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct LiveWordJson {
    t0: i64,
    t1: i64,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conf: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spk: Option<String>,
}

impl SqliteStore {
    /// Appends one revision of a live utterance (append-only; a later `rev`
    /// of the same `(channel, utt)` supersedes earlier ones). Returns the row id.
    /// A repeated `(run, channel, utt, rev)` is a `Conflict`.
    pub fn append_live_utterance(
        &self,
        run: RunId,
        channel: Channel,
        utt: u32,
        rev: u32,
        words: &[EngineWord],
    ) -> StoreResult<i64> {
        let json: Vec<LiveWordJson> = words
            .iter()
            .map(|w| LiveWordJson {
                t0: us_to_ms(w.t0_us),
                t1: us_to_ms(w.t1_us.max(w.t0_us)),
                text: w.text.clone(),
                conf: w.conf,
                spk: w.spk.clone(),
            })
            .collect();
        let start = json.first().map(|w| w.t0).unwrap_or(0);
        let end = json.iter().map(|w| w.t1).max().unwrap_or(start);
        // The utterance's label: the most frequent word label (first on ties).
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for w in &json {
            if let Some(s) = w.spk.as_deref() {
                match counts.iter_mut().find(|(l, _)| *l == s) {
                    Some(e) => e.1 += 1,
                    None => counts.push((s, 1)),
                }
            }
        }
        let raw_label = counts
            .iter()
            .fold(None::<(&str, usize)>, |best, &(l, n)| match best {
                Some((_, bn)) if bn >= n => best,
                _ => Some((l, n)),
            })
            .map(|(l, _)| l.to_owned());
        let words_json =
            serde_json::to_string(&json).map_err(|e| StoreError::Backend(format!("json: {e}")))?;
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
                start,
                end,
                raw_label,
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

    /// Turns the latest revision of every live utterance of `run` into the
    /// run's speakers, segments and words (one transaction). Mic words without
    /// a speaker become `me`; other untagged words `spk_unknown`. `Conflict`
    /// if the run already has a transcript.
    pub fn materialise_live_run(&self, run: RunId, opts: &SegmentOptions) -> StoreResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let meeting = get_run(&tx, run)?.meeting_id;
        if run_has_transcript(&tx, run)? {
            return Err(StoreError::Conflict(format!(
                "run {run} already has a transcript"
            )));
        }
        let mut per_channel: BTreeMap<Channel, Vec<TranscriptWord>> = BTreeMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT channel, words_json FROM live_utterances lu
                 WHERE run_id = ?1 AND rev = (SELECT MAX(rev) FROM live_utterances
                     WHERE run_id = lu.run_id AND channel = lu.channel AND utt = lu.utt)
                 ORDER BY start_ms, utt",
            )?;
            let rows = stmt.query_map([run.0], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (channel, json) = row?;
                let channel: Channel = channel
                    .parse()
                    .map_err(|e| StoreError::Backend(format!("live_utterances.channel: {e}")))?;
                let words: Vec<LiveWordJson> = serde_json::from_str(&json)
                    .map_err(|e| StoreError::Backend(format!("live_utterances.words_json: {e}")))?;
                let default = if channel == Channel::Mic {
                    ME_LABEL
                } else {
                    UNKNOWN_LABEL
                };
                per_channel
                    .entry(channel)
                    .or_default()
                    .extend(words.into_iter().map(|w| TranscriptWord {
                        channel,
                        start_ms: w.t0,
                        end_ms: w.t1,
                        text: w.text,
                        confidence: w.conf,
                        speaker: w.spk.clone().unwrap_or_else(|| default.to_owned()),
                        raw_label: w.spk,
                    }));
            }
        }
        let channels: Vec<(Channel, Vec<TranscriptWord>)> = per_channel
            .into_iter()
            .map(|(c, mut words)| {
                words.sort_by_key(|w| w.start_ms);
                (c, words)
            })
            .collect();
        let words = interleave(channels);
        let transcript = build_transcript(&words, opts);
        validate_transcript(&transcript)?;
        write_transcript(&tx, run, meeting, &transcript)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewMeeting, Store};
    use crate::types::{MeetingStatus, RunKind, Source};

    fn ew(t0_ms: u64, t1_ms: u64, text: &str, spk: Option<&str>) -> EngineWord {
        EngineWord {
            t0_us: t0_ms * 1_000,
            t1_us: t1_ms * 1_000,
            text: text.into(),
            conf: None,
            spk: spk.map(str::to_owned),
        }
    }

    #[test]
    fn latest_revisions_are_materialised() {
        let store = SqliteStore::open_in_memory().unwrap();
        let m = store
            .create_meeting(NewMeeting {
                title: "Live".into(),
                source: Source::Live,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 1,
                status: MeetingStatus::Recording,
            })
            .unwrap();
        let run = store
            .create_run(m.id, RunKind::Live, serde_json::json!({}))
            .unwrap()
            .id;
        store
            .append_live_utterance(
                run,
                Channel::Loopback,
                0,
                0,
                &[ew(0, 300, "helo", Some("spk_0"))],
            )
            .unwrap();
        store
            .append_live_utterance(
                run,
                Channel::Loopback,
                0,
                1,
                &[
                    ew(0, 300, "hello", Some("spk_0")),
                    ew(400, 700, "all", Some("spk_0")),
                ],
            )
            .unwrap();
        store
            .append_live_utterance(run, Channel::Mic, 0, 0, &[ew(350, 600, "hi", None)])
            .unwrap();
        assert!(matches!(
            store.append_live_utterance(run, Channel::Mic, 0, 0, &[]),
            Err(StoreError::Conflict(_))
        ));
        store
            .materialise_live_run(run, &SegmentOptions::default())
            .unwrap();
        store.set_current_run(m.id, run).unwrap();
        let segs = store.get_segments(m.id, 0, 10).unwrap();
        let texts: Vec<(&str, &str)> = segs
            .iter()
            .map(|s| (s.text.as_str(), s.speaker_name.as_str()))
            .collect();
        assert_eq!(
            texts,
            [("hello", "Speaker 1"), ("hi", "Me"), ("all", "Speaker 1")]
        );
        assert!(matches!(
            store.materialise_live_run(run, &SegmentOptions::default()),
            Err(StoreError::Conflict(_))
        ));
    }
}
