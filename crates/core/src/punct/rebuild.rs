//! Adding or removing punctuation on a finished meeting (plan 16 section
//! 3.9): "Add punctuation", "Remove punctuation", the bulk action and
//! `diarist-cli punctuate`.
//!
//! [`rebuild_run`] reads the current run's words by channel with the
//! engine's labels, back to the ASR's text (`raw_text`, else `text`),
//! punctuates them (or not, for [`RebuildWhy::Remove`] and the `off`
//! setting), segments them with the run's own segment options and stores
//! the result as a new run of the same kind: `engine_info` copied from the
//! current run with `punctuation` replaced (and `punctuation.rebuilt_from`
//! naming the root run), the current run's `echo_dropped` copied, speakers
//! carried by identity ([`RunStore::carry_over_identity`]). The new run
//! becomes current. Nothing is deleted. No run is made when nothing would
//! change, and "Add" makes none when the model did not run (no model, a
//! language it does not cover, a failure): an earlier punctuation is never
//! stripped by it.

use std::collections::{BTreeMap, HashMap};

use super::{punctuate_loaded, Loaded, PunctReason, PunctReport, PunctSource, RunLanguage};
use crate::error::{CoreError, Result};
use crate::live::{check_idle, segment_options};
use crate::store::{CarryOverReport, RunStore, StoreError};
use crate::transcript::{build_transcript_channels, TranscriptWord, ME_LABEL, UNKNOWN_LABEL};
use crate::types::{Channel, MeetingId, MeetingStatus, RunId, RunStatus, SegmentId, TranscriptRun};

/// Why a run is rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildWhy {
    /// Punctuate with the selected model ("Add punctuation", the bulk
    /// action, a model or setting change).
    Add,
    /// Back to the ASR's text ("Remove punctuation"; `reason: "off"`).
    Remove,
}

/// What [`rebuild_run`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct RebuildReport {
    pub meeting: MeetingId,
    /// The new current run; `None` when nothing would have changed (no run
    /// was made).
    pub run: Option<RunId>,
    /// The run that was current (kept).
    pub from_run: RunId,
    /// The run the current one was ultimately built from
    /// (`punctuation.rebuilt_from` of the new run).
    pub root: RunId,
    /// `engine_info.punctuation` of the new run (or what it would have been).
    pub punctuation: PunctReport,
    pub carry_over: Option<CarryOverReport>,
}

fn invalid(message: &str) -> CoreError {
    StoreError::Invalid(message.into()).into()
}

/// The run `run` was built from by rebuilds and restores, followed to the
/// end (`punctuation.rebuilt_from`, `echo.restored_from`); `run` itself when
/// it was built from audio. A pointer to a run that is gone ends the walk.
pub fn root_run(store: &dyn RunStore, run: &TranscriptRun) -> RunId {
    let mut cur = run.clone();
    for _ in 0..64 {
        let next = parent_of(&cur.engine_info);
        match next.and_then(|id| store.get_run(id).ok()) {
            Some(r) if r.meeting_id == cur.meeting_id && r.id != cur.id => cur = r,
            _ => break,
        }
    }
    cur.id
}

/// The run a rebuilt or restored run was built from.
pub fn parent_of(engine_info: &serde_json::Value) -> Option<RunId> {
    engine_info["punctuation"]["rebuilt_from"]
        .as_i64()
        .or_else(|| engine_info["echo"]["restored_from"].as_i64())
        .map(RunId)
}

/// Whether `ancestor` is `run` or one of the runs it was rebuilt or
/// restored from.
pub fn descends_from(store: &dyn RunStore, run: &TranscriptRun, ancestor: RunId) -> bool {
    let mut cur = run.clone();
    for _ in 0..64 {
        if cur.id == ancestor {
            return true;
        }
        match parent_of(&cur.engine_info).and_then(|id| store.get_run(id).ok()) {
            Some(r) if r.meeting_id == cur.meeting_id && r.id != cur.id => cur = r,
            _ => return false,
        }
    }
    false
}

/// The words of the meeting's current run by channel (mic first), with the
/// speaker each will be segmented under: the engine's label (`raw_label`;
/// an undiarized mic is `me`, an untagged word `spk_unknown`). `text` and
/// `raw_text` are as stored; [`super::punctuate_channels`] starts from the
/// ASR's text.
pub fn current_channels(
    store: &dyn RunStore,
    meeting: MeetingId,
) -> Result<Vec<(Channel, Vec<TranscriptWord>)>> {
    let detail = store.get_meeting(meeting)?;
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
            raw_text: w.raw_text,
        });
    }
    // BTreeMap order puts the mic first (equal start times: mic first, as
    // the post-pass).
    Ok(by_channel.into_iter().collect())
}

/// The language to punctuate a stored run in: the meeting's, and for `auto`
/// the language an earlier build of this run detected, if any.
pub fn run_language(meeting_language: &str, engine_info: &serde_json::Value) -> RunLanguage {
    let p = &engine_info["punctuation"];
    let detected = (p["language_source"].as_str() == Some("detected"))
        .then(|| p["language"].as_str().map(str::to_owned))
        .flatten();
    RunLanguage::new(meeting_language).with_detected(detected)
}

/// Adds (with the selected model of `source`) or removes punctuation on
/// the meeting's current run as a new current run (see the module docs).
/// Refuses (`Store(Conflict)`) a meeting that records or is processing,
/// and (`Store(Invalid)`) one without a transcript.
pub fn rebuild_run(
    store: &dyn RunStore,
    source: &PunctSource,
    meeting: MeetingId,
    why: RebuildWhy,
    cancel: &dyn Fn() -> bool,
) -> Result<RebuildReport> {
    let mut loaded: Loaded = match why {
        RebuildWhy::Add => source.load(store),
        RebuildWhy::Remove => Err(Box::new(PunctReport::not_applied(PunctReason::Off))),
    };
    rebuild_run_with(store, &mut loaded, meeting, why, cancel)
}

/// [`rebuild_run`] with a punctuator loaded once by the caller (the bulk
/// action keeps one session for all its meetings). For
/// [`RebuildWhy::Remove`] the punctuator is not used.
pub fn rebuild_run_with(
    store: &dyn RunStore,
    loaded: &mut Loaded,
    meeting: MeetingId,
    why: RebuildWhy,
    cancel: &dyn Fn() -> bool,
) -> Result<RebuildReport> {
    let detail = store.get_meeting(meeting)?;
    check_idle(detail.meeting.status)?;
    let current = detail
        .current_run
        .ok_or_else(|| invalid("This meeting has no transcript to punctuate."))?;
    let root = root_run(store, &current);
    let mut channels = current_channels(store, meeting)?;
    let before: Vec<Vec<String>> = channels
        .iter()
        .map(|(_, ws)| ws.iter().map(|w| w.text.clone()).collect())
        .collect();
    let segment = segment_options(&current.engine_info);
    let lang = run_language(&detail.meeting.language, &current.engine_info);
    let mut report = match why {
        RebuildWhy::Add => punctuate_loaded(loaded, &mut channels, &segment, &lang, cancel),
        RebuildWhy::Remove => {
            let mut off: Loaded = Err(Box::new(PunctReport::not_applied(PunctReason::Off)));
            punctuate_loaded(&mut off, &mut channels, &segment, &lang, cancel)
        }
    };
    if cancel() {
        return Err(CoreError::Cancelled);
    }
    let after: Vec<Vec<String>> = channels
        .iter()
        .map(|(_, ws)| ws.iter().map(|w| w.text.clone()).collect())
        .collect();
    let was_applied = PunctReport::of_engine_info(&current.engine_info).is_some_and(|r| r.applied);
    report.rebuilt_from = Some(root);
    let changed = before != after;
    // Add makes a run only when the model ran (no model, a language it
    // does not cover or a failure never strips an earlier punctuation);
    // Remove only when there is something to remove.
    let worth_it = match why {
        RebuildWhy::Add => report.applied && (changed || !was_applied),
        RebuildWhy::Remove => changed || was_applied,
    };
    if !worth_it {
        log::info!(
            "meeting {meeting}: punctuation {why:?} changes nothing ({}); run {} stays",
            report.reason.as_str(),
            current.id
        );
        return Ok(RebuildReport {
            meeting,
            run: None,
            from_run: current.id,
            root,
            punctuation: report,
            carry_over: None,
        });
    }
    let transcript = build_transcript_channels(channels, &segment);
    let mut info = current.engine_info.clone();
    if !info.is_object() {
        info = serde_json::json!({});
    }
    info["punctuation"] = report.to_json();
    let new = store.create_run(meeting, current.kind, info)?;
    let built = (|| -> Result<CarryOverReport> {
        if let Some(dropped) = store.echo_dropped(current.id)? {
            store.set_echo_dropped(new.id, &dropped)?;
        }
        store.insert_transcript(new.id, &transcript)?;
        store.finish_run(new.id, RunStatus::Done)?;
        Ok(store.carry_over_identity(current.id, new.id)?)
    })();
    let carry = match built {
        Ok(r) => r,
        Err(e) => {
            let _ = store.delete_run(new.id);
            return Err(e);
        }
    };
    store.set_current_run(meeting, new.id)?;
    log::info!(
        "meeting {meeting}: punctuation {why:?} as run {} from run {} (root {root}, kept): {} ({} marks, {} capitals)",
        new.id,
        current.id,
        report.reason.as_str(),
        report.marks_added.unwrap_or(0),
        report.case_upgrades.unwrap_or(0)
    );
    Ok(RebuildReport {
        meeting,
        run: Some(new.id),
        from_run: current.id,
        root,
        punctuation: report,
        carry_over: Some(carry),
    })
}

/// Meetings whose current run has no applied punctuation and that are
/// finished (`done` or `failed`), newest first: what the bulk action and
/// `diarist-cli punctuate --all-missing` work on.
pub fn meetings_missing_punctuation(store: &dyn RunStore) -> Result<Vec<MeetingId>> {
    let mut out = Vec::new();
    for m in store.list_meetings()? {
        if !matches!(m.status, MeetingStatus::Done | MeetingStatus::Failed) {
            continue;
        }
        let detail = store.get_meeting(m.id)?;
        let Some(run) = detail.current_run else {
            continue;
        };
        if !PunctReport::of_engine_info(&run.engine_info).is_some_and(|r| r.applied) {
            out.push(m.id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::punct::fixture::FixturePunctuator;
    use crate::punct::Mark;
    use crate::store::{MemoryStore, NewMeeting, SqliteStore, Store};
    use crate::transcript::SegmentOptions;
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
            raw_text: None,
        }
    }

    /// A finished post-pass meeting, current run with an echo summary and
    /// one dropped word.
    fn meeting(store: &dyn RunStore) -> (MeetingId, RunId) {
        let m = store
            .create_meeting(NewMeeting {
                title: "Rebuild".into(),
                source: Source::Live,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 1_791_000_000_000,
                status: MeetingStatus::Done,
            })
            .unwrap();
        let run = store
            .create_run(
                m.id,
                RunKind::PostPass,
                serde_json::json!({
                    "runtime": "fake 1",
                    "segmentation": {"max_gap_ms": 1500, "sentence_gap_ms": 500, "max_words": 80},
                    "echo": {"coupled": true, "dropped": 1, "aec": false, "mode": "auto"},
                    "punctuation": {"applied": false, "reason": "no_model"},
                }),
            )
            .unwrap();
        let mic = vec![
            word(Channel::Mic, 1_000, "hello", None),
            word(Channel::Mic, 1_400, "there", None),
        ];
        let lb = vec![
            word(Channel::Loopback, 3_000, "so", Some("spk_0")),
            word(Channel::Loopback, 3_400, "we", Some("spk_0")),
            word(Channel::Loopback, 3_800, "start", Some("spk_0")),
            word(Channel::Loopback, 6_000, "ok", Some("spk_1")),
        ];
        let t = build_transcript_channels(
            vec![(Channel::Mic, mic), (Channel::Loopback, lb)],
            &SegmentOptions::default(),
        );
        store.insert_transcript(run.id, &t).unwrap();
        store
            .set_echo_dropped(
                run.id,
                &[EchoDroppedWord {
                    start_ms: 2_000,
                    end_ms: 2_300,
                    text: "echo".into(),
                    coefficient: 0.8,
                    reason: None,
                }],
            )
            .unwrap();
        store.finish_run(run.id, RunStatus::Done).unwrap();
        store.set_current_run(m.id, run.id).unwrap();
        (m.id, run.id)
    }

    fn texts(store: &dyn RunStore, m: MeetingId) -> Vec<String> {
        store
            .get_words(m, i64::MIN, i64::MAX)
            .unwrap()
            .into_iter()
            .map(|w| w.text)
            .collect()
    }

    fn source() -> PunctSource {
        PunctSource::factory(|| {
            FixturePunctuator::marks(&[
                ("there", Mark::Period),
                ("start", Mark::Question),
                ("ok", Mark::Period),
            ])
        })
    }

    fn add_remove_add(store: &dyn RunStore) {
        let (m, first) = meeting(store);
        // Names, a colour, a merge and a review flag on the first run.
        let speakers = store.run_speakers(first).unwrap();
        let id = |label: &str| {
            speakers
                .iter()
                .find(|s| s.engine_label == label)
                .unwrap()
                .id
        };
        store.rename_speaker(id("spk_0"), "Alice", None).unwrap();
        store.set_speaker_color(id("spk_0"), "#123456").unwrap();
        store.merge_speakers(id("spk_1"), id("spk_0")).unwrap();
        store.rename_speaker(id("me"), "Bob", None).unwrap();

        let r = rebuild_run(store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        let added = r.run.unwrap();
        assert_eq!((r.from_run, r.root), (first, first));
        assert!(r.punctuation.applied);
        assert_eq!(
            texts(store, m),
            ["Hello", "there.", "So", "we", "start?", "Ok."]
        );
        let words = store.get_words(m, i64::MIN, i64::MAX).unwrap();
        assert_eq!(words[0].raw_text.as_deref(), Some("hello"));
        assert_eq!(words[2].raw_text.as_deref(), Some("so"));
        assert_eq!(words[3].raw_text, None);
        let run = store.get_run(added).unwrap();
        assert_eq!(run.kind, RunKind::PostPass);
        assert_eq!(
            run.engine_info["punctuation"]["rebuilt_from"].as_i64(),
            Some(first.0)
        );
        assert_eq!(run.engine_info["punctuation"]["applied"], true);
        // The echo summary, segmentation and echo_dropped come along.
        assert_eq!(run.engine_info["echo"]["dropped"], 1);
        assert_eq!(run.engine_info["segmentation"]["max_gap_ms"], 1500);
        assert_eq!(store.echo_dropped(added).unwrap().unwrap().len(), 1);
        // Speakers by identity: names, colour, merge.
        let new = store.run_speakers(added).unwrap();
        let by = |label: &str| new.iter().find(|s| s.engine_label == label).unwrap();
        assert_eq!(by("spk_0").display_name.as_deref(), Some("Alice"));
        assert_eq!(by("spk_0").color.as_deref(), Some("#123456"));
        assert_eq!(by("spk_1").merged_into, Some(by("spk_0").id));
        assert_eq!(by("me").display_name.as_deref(), Some("Bob"));
        assert!(new.iter().all(|s| !s.needs_review));
        // Add again: nothing changes, no run.
        let again = rebuild_run(store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(again.run, None);
        assert_eq!(store.list_runs(m).unwrap().len(), 2);
        // Add without a model (or one that does not cover the language)
        // never strips the punctuation that is there.
        let none = rebuild_run(store, &PunctSource::None, m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(
            (none.run, none.punctuation.reason),
            (None, PunctReason::NoModel)
        );
        let german = PunctSource::factory(|| {
            FixturePunctuator::marks(&[("there", Mark::Period)]).languages(&["de"])
        });
        let lang = rebuild_run(store, &german, m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(
            (lang.run, lang.punctuation.reason),
            (None, PunctReason::Language)
        );
        assert_eq!(texts(store, m)[1], "there.");
        assert_eq!(store.list_runs(m).unwrap().len(), 2);

        // Remove: back to the ASR's text, rebuilt from the root.
        let removed = rebuild_run(store, &source(), m, RebuildWhy::Remove, &|| false).unwrap();
        let removed_run = removed.run.unwrap();
        assert_eq!((removed.from_run, removed.root), (added, first));
        assert_eq!(removed.punctuation.reason, PunctReason::Off);
        assert_eq!(
            texts(store, m),
            ["hello", "there", "so", "we", "start", "ok"]
        );
        assert!(store
            .get_words(m, i64::MIN, i64::MAX)
            .unwrap()
            .iter()
            .all(|w| w.raw_text.is_none()));
        let info = store.get_run(removed_run).unwrap().engine_info;
        assert_eq!(info["punctuation"]["reason"], "off");
        assert_eq!(info["punctuation"]["rebuilt_from"].as_i64(), Some(first.0));

        // Add again: from the root too; no run is ever deleted.
        let readd = rebuild_run(store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(readd.root, first);
        assert_eq!(texts(store, m)[1], "there.");
        let runs = store.list_runs(m).unwrap();
        assert_eq!(runs.len(), 4);
        for r in [first, added, removed_run, readd.run.unwrap()] {
            assert!(store.get_run(r).is_ok());
        }
        let cur = store.get_meeting(m).unwrap().current_run.unwrap();
        assert_eq!(
            store
                .run_speakers(cur.id)
                .unwrap()
                .iter()
                .find(|s| s.engine_label == "spk_0")
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alice")
        );
        // A meeting being finalized is refused.
        store
            .set_meeting_status(m, MeetingStatus::Processing)
            .unwrap();
        assert!(matches!(
            rebuild_run(store, &source(), m, RebuildWhy::Add, &|| false),
            Err(CoreError::Store(StoreError::Conflict(_)))
        ));
    }

    #[test]
    fn add_remove_add_rebuilds_from_the_root_and_keeps_every_run() {
        add_remove_add(&SqliteStore::open_in_memory().unwrap());
        add_remove_add(&MemoryStore::new());
    }

    #[test]
    fn without_a_model_nothing_is_rebuilt() {
        let store = MemoryStore::new();
        let (m, first) = meeting(&store);
        let r = rebuild_run(&store, &PunctSource::None, m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(r.run, None);
        assert_eq!(r.punctuation.reason, PunctReason::NoModel);
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, first);
        assert_eq!(meetings_missing_punctuation(&store).unwrap(), [m]);
        // The setting `off` reads as off for every source.
        store
            .set_setting(crate::settings::TRANSCRIPT_PUNCTUATION, "off")
            .unwrap();
        let r = rebuild_run(&store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!((r.run, r.punctuation.reason), (None, PunctReason::Off));
        store
            .remove_setting(crate::settings::TRANSCRIPT_PUNCTUATION)
            .unwrap();
        rebuild_run(&store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        assert!(meetings_missing_punctuation(&store).unwrap().is_empty());
        // Cancelled: nothing made.
        let runs = store.list_runs(m).unwrap().len();
        assert!(matches!(
            rebuild_run(&store, &source(), m, RebuildWhy::Remove, &|| true),
            Err(CoreError::Cancelled)
        ));
        assert_eq!(store.list_runs(m).unwrap().len(), runs);
    }

    #[test]
    fn roots_and_descent_follow_rebuilds_and_restores() {
        let store = MemoryStore::new();
        let (m, first) = meeting(&store);
        let mk = |info: serde_json::Value| store.create_run(m, RunKind::PostPass, info).unwrap();
        let restored = mk(serde_json::json!({"echo": {"restored_from": first.0}}));
        let rebuilt = mk(serde_json::json!({"punctuation": {"rebuilt_from": restored.id.0}}));
        assert_eq!(root_run(&store, &rebuilt), first);
        assert!(descends_from(&store, &rebuilt, first));
        assert!(descends_from(&store, &rebuilt, rebuilt.id));
        assert!(!descends_from(&store, &restored, rebuilt.id));
        // A pointer to a run that is gone ends the walk.
        let orphan = mk(serde_json::json!({"punctuation": {"rebuilt_from": 99_999}}));
        assert_eq!(root_run(&store, &orphan), orphan.id);
        assert_eq!(
            run_language(
                "auto",
                &serde_json::json!({"punctuation": {"language": "de", "language_source": "detected"}})
            ),
            RunLanguage::new("auto").with_detected(Some("de".into()))
        );
        assert_eq!(
            run_language(
                "auto",
                &serde_json::json!({"punctuation": {"language": "latin", "language_source": "script"}})
            ),
            RunLanguage::new("auto")
        );
    }
}
