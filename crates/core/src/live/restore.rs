//! Restore (plan 15, the meeting details' echo line): the mic words the echo
//! gate removed from a run, put back as `me`.
//!
//! [`restore_echo_dropped`] rebuilds the meeting's current run from its
//! stored words plus the run's `echo_dropped` list (on the mic, as `me`).
//! Every stored word keeps the label the engine gave it (`raw_label`; an
//! undiarized mic is `me`), each channel is segmented on its own with the
//! run's segment options (`engine_info.segmentation`, else the defaults)
//! and the segments are ordered by start time as the post-pass does
//! ([`build_transcript_channels`]). The result is a new run of the same kind
//! with `engine_info.echo.restored: true` (with `restored_from`,
//! `restored_words` and `dropped: 0`) and an empty `echo_dropped`; names,
//! colours and reassignments carry over from the old run as after a
//! post-pass ([`RunStore::carry_over`]), the old run's merges are made again
//! (both runs have the same speaker labels), and the new run becomes
//! current. The old run stays: [`remove_restored_echo`] ("Remove echo words
//! again") makes it current again, and restoring it once more replaces the
//! earlier restored run.

use std::collections::{BTreeMap, HashMap};

use crate::error::{CoreError, Result};
use crate::store::{CarryOverReport, RunStore, StoreError};
use crate::transcript::{
    build_transcript_channels, SegmentOptions, TranscriptWord, ME_LABEL, UNKNOWN_LABEL,
};
use crate::types::{Channel, MeetingId, MeetingStatus, RunId, RunStatus, SegmentId, SpeakerId};

/// What [`restore_echo_dropped`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct EchoRestore {
    pub meeting: MeetingId,
    /// The new current run.
    pub run: RunId,
    /// The run the words had been removed from (kept).
    pub from_run: RunId,
    /// Words put back.
    pub restored: usize,
    pub carry_over: CarryOverReport,
}

fn invalid(message: &str) -> CoreError {
    StoreError::Invalid(message.into()).into()
}

/// Nothing may rebuild a meeting that records or is being finalized.
fn check_idle(status: MeetingStatus) -> Result<()> {
    match status {
        MeetingStatus::Recording => {
            Err(StoreError::Conflict("This meeting is still recording.".into()).into())
        }
        MeetingStatus::Processing => Err(StoreError::Conflict(
            "This meeting's transcript is still being finalized; try again when it is done.".into(),
        )
        .into()),
        MeetingStatus::Done | MeetingStatus::Failed => Ok(()),
    }
}

/// The segment options a run was built with (`engine_info.segmentation`,
/// written by the post-pass), else the defaults (live runs).
fn segment_options(info: &serde_json::Value) -> SegmentOptions {
    let d = SegmentOptions::default();
    let s = &info["segmentation"];
    SegmentOptions {
        max_gap_ms: s["max_gap_ms"].as_i64().unwrap_or(d.max_gap_ms),
        sentence_gap_ms: s["sentence_gap_ms"].as_i64().unwrap_or(d.sentence_gap_ms),
        max_words: s["max_words"].as_u64().map_or(d.max_words, |v| v as usize),
    }
}

/// The merges of `from` made again on `to` (the same speakers by channel
/// and engine label).
fn remerge(store: &dyn RunStore, from: RunId, to: RunId) -> Result<()> {
    let old = store.run_speakers(from)?;
    let new: HashMap<(Channel, String), SpeakerId> = store
        .run_speakers(to)?
        .into_iter()
        .map(|s| ((s.channel, s.engine_label), s.id))
        .collect();
    let key: HashMap<SpeakerId, (Channel, String)> = old
        .iter()
        .map(|s| (s.id, (s.channel, s.engine_label.clone())))
        .collect();
    for s in &old {
        let Some(root) = s.merged_into else {
            continue;
        };
        let a = new.get(&(s.channel, s.engine_label.clone()));
        let b = key.get(&root).and_then(|k| new.get(k));
        if let (Some(&a), Some(&b)) = (a, b) {
            if a != b {
                store.merge_speakers(a, b)?;
            }
        }
    }
    Ok(())
}

/// Puts the words the echo gate removed from `run` back into "me" as a new
/// current run (see the module docs). `run` must be its meeting's current
/// run, with words in `echo_dropped`, and the meeting neither recording nor
/// being finalized.
pub fn restore_echo_dropped(store: &dyn RunStore, run: RunId) -> Result<EchoRestore> {
    let old = store.get_run(run)?;
    let meeting = old.meeting_id;
    let detail = store.get_meeting(meeting)?;
    check_idle(detail.meeting.status)?;
    if detail.current_run.as_ref().map(|r| r.id) != Some(run) {
        return Err(invalid(
            "Restore works on the transcript the meeting shows now; reopen the meeting and use \
             its Restore.",
        ));
    }
    if old.engine_info["echo"]["restored"].as_bool() == Some(true) {
        return Err(invalid(
            "The words removed as echo are already back in this transcript.",
        ));
    }
    let dropped = store.echo_dropped(run)?.unwrap_or_default();
    if dropped.is_empty() {
        return Err(invalid(
            "No words were removed from \"me\" as echo in this transcript.",
        ));
    }
    // An earlier restore of this run is replaced.
    for r in store.list_runs(meeting)? {
        if r.id != run && r.engine_info["echo"]["restored_from"].as_i64() == Some(run.0) {
            store.delete_run(r.id)?;
        }
    }

    // The stored words by channel, with the engine's labels.
    let channel_of: HashMap<SegmentId, Channel> = store
        .get_segments(meeting, 0, u32::MAX)?
        .into_iter()
        .map(|s| (s.id, s.channel))
        .collect();
    let undiarized_mic = !detail.meeting.diarize_mic;
    let mut by_channel: BTreeMap<Channel, Vec<TranscriptWord>> = BTreeMap::new();
    for w in store.get_words(meeting, i64::MIN, i64::MAX)? {
        let Some(&channel) = channel_of.get(&w.segment_id) else {
            continue;
        };
        let speaker = match (channel, &w.raw_label) {
            (Channel::Mic, _) if undiarized_mic => ME_LABEL.to_owned(),
            (_, Some(label)) => label.clone(),
            (Channel::Mic, None) => ME_LABEL.to_owned(),
            (_, None) => UNKNOWN_LABEL.to_owned(),
        };
        by_channel.entry(channel).or_default().push(TranscriptWord {
            channel,
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            text: w.text,
            confidence: w.confidence,
            speaker,
            raw_label: w.raw_label,
        });
    }
    let restored = dropped.len();
    by_channel
        .entry(Channel::Mic)
        .or_default()
        .extend(dropped.into_iter().map(|d| TranscriptWord {
            channel: Channel::Mic,
            start_ms: d.start_ms,
            end_ms: d.end_ms.max(d.start_ms),
            text: d.text,
            confidence: None,
            speaker: ME_LABEL.to_owned(),
            raw_label: None,
        }));
    // Mic first on equal start times, as the post-pass.
    let mut channels: Vec<(Channel, Vec<TranscriptWord>)> = Vec::new();
    if let Some(mic) = by_channel.remove(&Channel::Mic) {
        channels.push((Channel::Mic, mic));
    }
    channels.extend(by_channel);
    let transcript = build_transcript_channels(channels, &segment_options(&old.engine_info));

    let mut info = old.engine_info.clone();
    if !info.is_object() {
        info = serde_json::json!({});
    }
    if !info["echo"].is_object() {
        info["echo"] = serde_json::json!({});
    }
    info["echo"]["restored"] = serde_json::json!(true);
    info["echo"]["restored_from"] = serde_json::json!(run.0);
    info["echo"]["restored_words"] = serde_json::json!(restored);
    info["echo"]["dropped"] = serde_json::json!(0);
    let new = store.create_run(meeting, old.kind, info)?;
    let built = (|| -> Result<CarryOverReport> {
        store.set_echo_dropped(new.id, &[])?;
        store.insert_transcript(new.id, &transcript)?;
        store.finish_run(new.id, RunStatus::Done)?;
        let report = store.carry_over(run, new.id)?;
        remerge(store, run, new.id)?;
        Ok(report)
    })();
    let report = match built {
        Ok(r) => r,
        Err(e) => {
            let _ = store.delete_run(new.id);
            return Err(e);
        }
    };
    store.set_current_run(meeting, new.id)?;
    log::info!(
        "meeting {meeting}: {restored} echo words restored into \"me\" as run {} (from run {run}, kept); carry-over {} matched, {} named, {} need review",
        new.id,
        report.matched.len(),
        report.named.len(),
        report.needs_review.len()
    );
    Ok(EchoRestore {
        meeting,
        run: new.id,
        from_run: run,
        restored,
        carry_over: report,
    })
}

/// "Remove echo words again": makes the run that the restored run `run`
/// was built from current again and returns it. The restored run stays (a
/// later Restore replaces it).
pub fn remove_restored_echo(store: &dyn RunStore, run: RunId) -> Result<RunId> {
    let r = store.get_run(run)?;
    let detail = store.get_meeting(r.meeting_id)?;
    check_idle(detail.meeting.status)?;
    if detail.current_run.as_ref().map(|c| c.id) != Some(run) {
        return Err(invalid(
            "This works on the transcript the meeting shows now; reopen the meeting and try again.",
        ));
    }
    let Some(from) = r.engine_info["echo"]["restored_from"].as_i64().map(RunId) else {
        return Err(invalid(
            "This transcript has no restored echo words to remove.",
        ));
    };
    match store.get_run(from) {
        Ok(prev) if prev.meeting_id == r.meeting_id => {}
        _ => {
            return Err(invalid(
                "The transcript without the echo words is gone, so they cannot be removed again.",
            ))
        }
    }
    store.set_current_run(r.meeting_id, from)?;
    log::info!(
        "meeting {}: echo words removed again (run {from} current, run {run} kept)",
        r.meeting_id
    );
    Ok(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, NewMeeting, SqliteStore, Store};
    use crate::types::{EchoDroppedWord, RunKind, Source};

    fn word(channel: Channel, start: i64, text: &str, spk: Option<&str>) -> TranscriptWord {
        TranscriptWord {
            channel,
            start_ms: start,
            end_ms: start + 300,
            text: text.into(),
            confidence: Some(0.9),
            speaker: spk.unwrap_or(ME_LABEL).into(),
            raw_label: spk.map(str::to_owned),
        }
    }

    /// A finished live meeting whose post-pass run removed two echo words
    /// ("one", "two") from the mic; returns it and the run.
    fn meeting(store: &dyn RunStore) -> (MeetingId, RunId) {
        let m = store
            .create_meeting(NewMeeting {
                title: "Echo".into(),
                source: Source::Live,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 1_791_000_000_000,
                status: MeetingStatus::Done,
            })
            .unwrap();
        store
            .finish_meeting(m.id, m.started_at + 60_000, 60_000)
            .unwrap();
        let run = store
            .create_run(
                m.id,
                RunKind::PostPass,
                serde_json::json!({
                    "runtime": "fake 1",
                    "segmentation": {"max_gap_ms": 1500, "sentence_gap_ms": 500, "max_words": 80},
                    "echo": {"coupled": true, "coupling": 0.72, "lag_ms": 3.25,
                             "mic_words_before": 4, "dropped": 2, "aec": false, "mode": "auto"},
                }),
            )
            .unwrap();
        let mic = vec![
            word(Channel::Mic, 1_000, "hello", None),
            word(Channel::Mic, 9_000, "bye", None),
        ];
        let lb = vec![
            word(Channel::Loopback, 2_000, "one", Some("spk_0")),
            word(Channel::Loopback, 2_400, "two", Some("spk_0")),
            word(Channel::Loopback, 6_000, "three", Some("spk_1")),
        ];
        let t = build_transcript_channels(
            vec![(Channel::Mic, mic), (Channel::Loopback, lb)],
            &SegmentOptions::default(),
        );
        store.insert_transcript(run.id, &t).unwrap();
        store
            .set_echo_dropped(
                run.id,
                &[
                    EchoDroppedWord {
                        start_ms: 2_080,
                        end_ms: 2_350,
                        text: "one".into(),
                        coefficient: 0.81,
                    },
                    EchoDroppedWord {
                        start_ms: 2_470,
                        end_ms: 2_700,
                        text: "two".into(),
                        coefficient: 0.77,
                    },
                ],
            )
            .unwrap();
        store.finish_run(run.id, RunStatus::Done).unwrap();
        store.set_current_run(m.id, run.id).unwrap();
        (m.id, run.id)
    }

    fn mic_texts(store: &dyn RunStore, m: MeetingId) -> Vec<String> {
        let mic: Vec<SegmentId> = store
            .get_segments(m, 0, u32::MAX)
            .unwrap()
            .into_iter()
            .filter(|s| s.channel == Channel::Mic)
            .map(|s| s.id)
            .collect();
        store
            .get_words(m, i64::MIN, i64::MAX)
            .unwrap()
            .into_iter()
            .filter(|w| mic.contains(&w.segment_id))
            .map(|w| w.text)
            .collect()
    }

    fn restore_and_back(store: &dyn RunStore) {
        let (m, run) = meeting(store);
        // Names and a merge on the run the words were removed from.
        let speakers = store.run_speakers(run).unwrap();
        let id = |label: &str| {
            speakers
                .iter()
                .find(|s| s.engine_label == label)
                .unwrap()
                .id
        };
        store.rename_speaker(id("spk_0"), "Alice", None).unwrap();
        store.rename_speaker(id("me"), "Ebin", None).unwrap();
        store.merge_speakers(id("spk_1"), id("spk_0")).unwrap();

        let r = restore_echo_dropped(store, run).unwrap();
        assert_eq!((r.meeting, r.from_run, r.restored), (m, run, 2));
        let d = store.get_meeting(m).unwrap();
        let cur = d.current_run.unwrap();
        assert_eq!((cur.id, cur.kind), (r.run, RunKind::PostPass));
        let e = &cur.engine_info["echo"];
        assert_eq!(e["restored"], true);
        assert_eq!(e["restored_from"].as_i64(), Some(run.0));
        assert_eq!(
            (e["restored_words"].as_u64(), e["dropped"].as_u64()),
            (Some(2), Some(0))
        );
        assert_eq!(e["coupling"].as_f64(), Some(0.72));
        assert_eq!(store.echo_dropped(r.run).unwrap(), Some(Vec::new()));
        // The echo words are back on the mic as "me", in time order; the
        // loopback is as it was.
        assert_eq!(mic_texts(store, m), ["hello", "one", "two", "bye"]);
        assert_eq!(
            store.get_words(m, i64::MIN, i64::MAX).unwrap().len(),
            7,
            "every word once"
        );
        let names: Vec<(String, String)> = d
            .speakers
            .iter()
            .filter(|s| s.merged_into.is_none())
            .map(|s| (s.engine_label.clone(), s.name.clone()))
            .collect();
        assert!(
            names.contains(&("spk_0".into(), "Alice".into())),
            "{names:?}"
        );
        assert!(names.contains(&("me".into(), "Ebin".into())), "{names:?}");
        let new = store.run_speakers(r.run).unwrap();
        let spk1 = new.iter().find(|s| s.engine_label == "spk_1").unwrap();
        let spk0 = new.iter().find(|s| s.engine_label == "spk_0").unwrap();
        assert_eq!(spk1.merged_into, Some(spk0.id), "the merge is made again");
        // The old run is kept; a second Restore on the restored run is refused.
        assert!(store.get_run(run).is_ok());
        assert!(matches!(
            restore_echo_dropped(store, r.run),
            Err(CoreError::Store(StoreError::Invalid(_)))
        ));
        // Remove echo words again: the old run is current, nothing deleted.
        assert_eq!(remove_restored_echo(store, r.run).unwrap(), run);
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, run);
        assert_eq!(mic_texts(store, m), ["hello", "bye"]);
        assert!(store.get_run(r.run).is_ok());
        // Not current any more: both refuse.
        assert!(remove_restored_echo(store, r.run).is_err());
        // Restore again replaces the earlier restored run.
        let again = restore_echo_dropped(store, run).unwrap();
        assert!(store.get_run(r.run).is_err());
        assert_eq!(store.list_runs(m).unwrap().len(), 2);
        assert_eq!(
            store.get_meeting(m).unwrap().current_run.unwrap().id,
            again.run
        );
        // A run that is not current is refused, and so is a meeting being finalized.
        assert!(matches!(
            restore_echo_dropped(store, run),
            Err(CoreError::Store(StoreError::Invalid(_)))
        ));
        store.set_current_run(m, run).unwrap();
        store
            .set_meeting_status(m, MeetingStatus::Processing)
            .unwrap();
        assert!(matches!(
            restore_echo_dropped(store, run),
            Err(CoreError::Store(StoreError::Conflict(_)))
        ));
    }

    #[test]
    fn restore_puts_echo_words_back_and_remove_switches_back() {
        restore_and_back(&SqliteStore::open_in_memory().unwrap());
        restore_and_back(&MemoryStore::new());
    }

    #[test]
    fn nothing_to_restore_is_refused() {
        let store = MemoryStore::new();
        let (m, run) = meeting(&store);
        store.set_echo_dropped(run, &[]).unwrap();
        assert!(matches!(
            restore_echo_dropped(&store, run),
            Err(CoreError::Store(StoreError::Invalid(_)))
        ));
        assert!(matches!(
            remove_restored_echo(&store, run),
            Err(CoreError::Store(StoreError::Invalid(_)))
        ));
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, run);
    }
}
