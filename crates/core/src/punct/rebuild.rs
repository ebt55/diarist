//! Adding or removing punctuation on a finished meeting (plan 16 section
//! 3.9): "Add punctuation", "Remove punctuation", the bulk action and
//! `diarist-cli punctuate`; and the parts every rebuild shares with Restore
//! and the promoted live run.
//!
//! A rebuild never changes a word's speaker. [`source_run`] reads the run's
//! words with the speaker row of each word's own segment (not the merge
//! root the views show), so the engine's defaults, `user_N` rows made by a
//! reassign and the reassignments themselves all come along as stored;
//! [`rebuild_transcript`] segments the words again (sentence ends now split
//! segments) without ever joining two source speakers in one segment, and
//! adds every source speaker row that got no word; then
//! [`RunStore::carry_over_identity`] copies each row's name, colour, global
//! id, merge, review flag and anchor by `(channel, engine_label)`. The new
//! run becomes current through [`RunStore::set_current_run_if`] only while
//! the run it was built from is still current; otherwise it is deleted
//! (nobody saw it, it holds nothing found nowhere else) and the caller gets
//! [`MEETING_CHANGED`].
//!
//! [`rebuild_run`] works on the current run, back to the ASR's text
//! (`raw_text`, else `text`), punctuated (or not, for [`RebuildWhy::Remove`]
//! and the `off` setting) and segmented with the run's own segment options,
//! as a new run of the same kind: `engine_info` copied with `punctuation`
//! replaced (and `punctuation.rebuilt_from` naming the root run), the
//! current run's `echo_dropped` copied. Nothing is deleted. No run is made
//! when nothing would change, and "Add" makes none when the model did not
//! run (no model, a language it does not cover, a failure): an earlier
//! punctuation is never stripped by it. Before punctuation the words'
//! repeated-token loops are collapsed with the current
//! `transcript.collapse_loops` (plan 16 section 12.3), so a rebuild of an
//! older meeting collapses its loops too; `engine_info.loops` is replaced
//! (its counts include the current run's).
//!
//! A crash between creating such a run and switching to it leaves a
//! non-current run in `running`; [`sweep_interrupted_rebuilds`] deletes
//! those at launch.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::{punctuate_loaded, Loaded, PunctReason, PunctReport, PunctSource, RunLanguage};
use crate::error::{CoreError, Result};
use crate::live::{check_idle, segment_options};
use crate::store::{CarryOverReport, NewSpeaker, NewTranscript, RunStore, StoreError};
use crate::transcript::loops::{self, collapse_channels};
use crate::transcript::{build_transcript_channels, SegmentOptions, TranscriptWord};
use crate::types::{
    Channel, EchoDroppedWord, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, SegmentId,
    Speaker, TranscriptRun,
};

/// The message of the `Store(Conflict)` that [`rebuild_run`] and Restore
/// return when the meeting's current run changed while they ran (another
/// rebuild, a Restore, a re-run of the post-pass) or the meeting started
/// processing: nothing was switched and the run they built is gone.
pub const MEETING_CHANGED: &str = "This meeting's transcript changed while this ran (another \
     change to it finished first); nothing was switched. Try again.";

/// The error for [`MEETING_CHANGED`].
pub fn meeting_changed() -> CoreError {
    StoreError::Conflict(MEETING_CHANGED.into()).into()
}

/// Whether `e` is [`meeting_changed`] (worth one more try).
pub fn is_meeting_changed(e: &CoreError) -> bool {
    matches!(e, CoreError::Store(StoreError::Conflict(m)) if m == MEETING_CHANGED)
}

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

/// Whether a run is a copy made by a rebuild or a Restore.
pub fn is_copy(engine_info: &serde_json::Value) -> bool {
    !engine_info["punctuation"]["rebuilt_from"].is_null()
        || !engine_info["echo"]["restored_from"].is_null()
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

/// A run's transcript as a rebuild reads it (see the module docs).
#[derive(Debug, Clone)]
pub struct SourceRun {
    pub run: TranscriptRun,
    /// Every speaker row of the run, `user_N` rows and rows without words
    /// included.
    pub speakers: Vec<Speaker>,
    /// The words by channel (the channel of their segment's own speaker
    /// row; mic first), in `(start_ms, id)` order. `speaker` is that row's
    /// engine label, which is unique per channel within a run, so
    /// segmenting the words again keeps every word with its row. `text`,
    /// `raw_text`, `raw_label`, `confidence`, `margin` and `overlap` are as
    /// stored; [`super::punctuate_channels`] starts from the ASR's text.
    pub channels: Vec<(Channel, Vec<TranscriptWord>)>,
}

/// Every word's text, by channel.
pub(crate) fn texts_of(channels: &[(Channel, Vec<TranscriptWord>)]) -> Vec<Vec<String>> {
    channels
        .iter()
        .map(|(_, ws)| ws.iter().map(|w| w.text.clone()).collect())
        .collect()
}

/// Reads `run` (current or not) for a rebuild: [`SourceRun`]. A word or
/// segment that belongs to no speaker of the run is an error (rather than a
/// word left out).
pub fn source_run(store: &dyn RunStore, run: RunId) -> Result<SourceRun> {
    let run = store.get_run(run)?;
    let speakers = store.run_speakers(run.id)?;
    let rows: HashMap<_, (Channel, String)> = speakers
        .iter()
        .map(|s| (s.id, (s.channel, s.engine_label.clone())))
        .collect();
    let mut owner: HashMap<SegmentId, (Channel, String)> = HashMap::new();
    for seg in store.run_segments(run.id)? {
        let row = rows.get(&seg.speaker_id).ok_or_else(|| {
            invalid(&format!(
                "segment {} of run {} belongs to no speaker of that run",
                seg.id, run.id
            ))
        })?;
        owner.insert(seg.id, row.clone());
    }
    let mut by_channel: BTreeMap<Channel, Vec<TranscriptWord>> = BTreeMap::new();
    for w in store.run_words(run.id)? {
        let (channel, label) = owner.get(&w.segment_id).cloned().ok_or_else(|| {
            invalid(&format!(
                "word {} of run {} belongs to no segment of that run",
                w.id, run.id
            ))
        })?;
        by_channel.entry(channel).or_default().push(TranscriptWord {
            channel,
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            text: w.text,
            confidence: w.confidence,
            speaker: label,
            raw_label: w.raw_label,
            raw_text: w.raw_text,
            margin: w.margin,
            overlap: w.overlap,
        });
    }
    Ok(SourceRun {
        run,
        speakers,
        // BTreeMap order puts the mic first (equal start times: mic first,
        // as the post-pass).
        channels: by_channel.into_iter().collect(),
    })
}

/// The transcript of a rebuild of `source`: `channels` (its words,
/// punctuated again, plus any words Restore puts back) segmented with
/// `segment`, each word under its `speaker` label, so a segment never spans
/// two source speakers; then every speaker row of `source` that got no word
/// (`user_N` rows, rows whose words the echo gate took), so
/// [`RunStore::carry_over_identity`] finds every row by `(channel,
/// engine_label)`.
pub fn rebuild_transcript(
    source: &SourceRun,
    channels: Vec<(Channel, Vec<TranscriptWord>)>,
    segment: &SegmentOptions,
) -> NewTranscript {
    let mut t = build_transcript_channels(channels, segment);
    let have: HashSet<(Channel, String)> = t
        .speakers
        .iter()
        .map(|s| (s.channel, s.engine_label.clone()))
        .collect();
    for s in &source.speakers {
        if !have.contains(&(s.channel, s.engine_label.clone())) {
            t.speakers.push(NewSpeaker {
                channel: s.channel,
                engine_label: s.engine_label.clone(),
                display_name: None,
                color: None,
                needs_review: false,
            });
        }
    }
    t
}

/// Stores a rebuild of `from` (a run of `meeting`) as a new run and makes it
/// current: `create_run`, `echo_dropped` (when given), the transcript, the
/// identity carry, `finish_run`, then [`RunStore::set_current_run_if`] with
/// `from` expected. A failure before the switch deletes the new run; a
/// meeting that changed meanwhile deletes it too and returns
/// [`meeting_changed`]. Returns the new run and the carry report.
pub(crate) fn store_rebuild(
    store: &dyn RunStore,
    meeting: MeetingId,
    from: RunId,
    kind: RunKind,
    engine_info: serde_json::Value,
    echo_dropped: Option<&[EchoDroppedWord]>,
    transcript: &NewTranscript,
) -> Result<(RunId, CarryOverReport)> {
    let new = store.create_run(meeting, kind, engine_info)?;
    let built = (|| -> Result<CarryOverReport> {
        if let Some(dropped) = echo_dropped {
            store.set_echo_dropped(new.id, dropped)?;
        }
        store.insert_transcript(new.id, transcript)?;
        let carry = store.carry_over_identity(from, new.id)?;
        store.finish_run(new.id, RunStatus::Done)?;
        Ok(carry)
    })();
    let carry = match built {
        Ok(c) => c,
        Err(e) => {
            let _ = store.delete_run(new.id);
            return Err(e);
        }
    };
    // A store error here leaves the run as it is: it may have become
    // current, and deleting a current run would empty the meeting.
    if store.set_current_run_if(meeting, from, new.id)? {
        Ok((new.id, carry))
    } else {
        log::warn!(
            "meeting {meeting}: run {from} stopped being current (or the meeting started processing) while run {} was built from it; that run is deleted and nothing switched",
            new.id
        );
        let _ = store.delete_run(new.id);
        Err(meeting_changed())
    }
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
/// and (`Store(Invalid)`) one without a transcript; [`meeting_changed`]
/// when the current run changed while it ran.
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
/// action keeps one session for all its meetings; a punctuator that
/// panicked is dropped and `loaded` holds its error report for the rest of
/// the batch). For [`RebuildWhy::Remove`] the punctuator is not used.
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
    let source = source_run(store, current.id)?;
    let mut channels = source.channels.clone();
    let before = texts_of(&channels);
    // Loops collapse at rebuild time with the current setting (plan 16
    // section 12.3), so Add punctuation on an older meeting collapses its
    // loops too; the counts include the current run's.
    let loops = collapse_channels(&mut channels, loops::enabled(store));
    let collapsed = loops.words_removed;
    let loops = loops.on_top_of(&current.engine_info);
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
    let after = texts_of(&channels);
    let was_applied = PunctReport::of_engine_info(&current.engine_info).is_some_and(|r| r.applied);
    report.rebuilt_from = Some(root);
    // A collapsed loop is a change too (the word list is shorter).
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
    let transcript = rebuild_transcript(&source, channels, &segment);
    let mut info = current.engine_info.clone();
    if !info.is_object() {
        info = serde_json::json!({});
    }
    info["punctuation"] = report.to_json();
    info["loops"] = loops.to_json();
    let dropped = store.echo_dropped(current.id)?;
    let (new, carry) = store_rebuild(
        store,
        meeting,
        current.id,
        current.kind,
        info,
        dropped.as_deref(),
        &transcript,
    )?;
    log::info!(
        "meeting {meeting}: punctuation {why:?} as run {new} from run {} (root {root}, kept): {} ({} marks, {} capitals; {collapsed} words of repeated-token loops collapsed)",
        current.id,
        report.reason.as_str(),
        report.marks_added.unwrap_or(0),
        report.case_upgrades.unwrap_or(0)
    );
    Ok(RebuildReport {
        meeting,
        run: Some(new),
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

/// Deletes what a crash in the middle of a rebuild or a Restore left: runs
/// still `running` that are copies (`punctuation.rebuilt_from` or
/// `echo.restored_from`) and not current, of meetings that are `done` or
/// `failed` (nothing builds such a run for a meeting that records or is
/// being finalized). Such a run was never shown and holds nothing found
/// nowhere else. Call it at launch before any job is queued
/// (`pipeline::recover_interrupted_imports` does); returns the runs deleted.
pub fn sweep_interrupted_rebuilds(store: &dyn RunStore) -> Result<Vec<RunId>> {
    let mut out = Vec::new();
    for m in store.list_meetings()? {
        if !matches!(m.status, MeetingStatus::Done | MeetingStatus::Failed) {
            continue;
        }
        let current = store.get_meeting(m.id)?.current_run.map(|r| r.id);
        for r in store.list_runs(m.id)? {
            if r.status == RunStatus::Running && Some(r.id) != current && is_copy(&r.engine_info) {
                store.delete_run(r.id)?;
                log::warn!(
                    "meeting {}: run {} was left half-built by an interrupted rebuild or Restore; deleted",
                    m.id,
                    r.id
                );
                out.push(r.id);
            }
        }
    }
    Ok(out)
}

/// Shared by the rebuild, Restore and post-pass tests: what a run's words
/// and speaker rows look like to the user and to carry-over.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::{BTreeMap, HashMap};

    use crate::store::RunStore;
    use crate::types::{Channel, RunId, SegmentId, SpeakerId};
    use crate::views::{default_speaker_color, default_speaker_name};

    /// One word: its times, channel, the ASR's text, the engine's label, the
    /// engine label of its own speaker row, and the name and colour shown
    /// for it (through the merge root).
    #[derive(Debug, Clone, PartialEq)]
    pub struct Seen {
        pub start: i64,
        pub end: i64,
        pub channel: Channel,
        pub asr: String,
        pub raw_label: Option<String>,
        pub own: String,
        pub name: String,
        pub color: String,
        pub margin: Option<f32>,
        pub overlap: bool,
    }

    /// Every word of `run`, by start, channel and text.
    pub fn seen(store: &dyn RunStore, run: RunId) -> Vec<Seen> {
        let speakers = store.run_speakers(run).unwrap();
        let by_id: HashMap<SpeakerId, _> = speakers.iter().map(|s| (s.id, s)).collect();
        let owner: HashMap<SegmentId, SpeakerId> = store
            .run_segments(run)
            .unwrap()
            .into_iter()
            .map(|s| (s.id, s.speaker_id))
            .collect();
        let mut out: Vec<Seen> = store
            .run_words(run)
            .unwrap()
            .into_iter()
            .map(|w| {
                let own = by_id[&owner[&w.segment_id]];
                let root = own.merged_into.map_or(own, |r| by_id[&r]);
                Seen {
                    start: w.start_ms,
                    end: w.end_ms,
                    channel: own.channel,
                    asr: w.asr_text().to_owned(),
                    raw_label: w.raw_label.clone(),
                    own: own.engine_label.clone(),
                    name: root
                        .display_name
                        .clone()
                        .unwrap_or_else(|| default_speaker_name(&root.engine_label)),
                    color: root
                        .color
                        .clone()
                        .unwrap_or_else(|| default_speaker_color(&root.engine_label).to_owned()),
                    margin: w.margin,
                    overlap: w.overlap,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            (a.start, a.channel, a.end, &a.asr).cmp(&(b.start, b.channel, b.end, &b.asr))
        });
        out
    }

    /// A speaker row without its ids: name, colour, global id, merge root
    /// (by channel and label), review flag, anchor.
    pub type Row = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<(Channel, String)>,
        bool,
        Option<i64>,
        Option<i64>,
    );

    /// Every speaker row of `run` by `(channel, engine_label)`.
    pub fn rows(store: &dyn RunStore, run: RunId) -> BTreeMap<(Channel, String), Row> {
        let speakers = store.run_speakers(run).unwrap();
        let key = |id: SpeakerId| {
            let s = speakers.iter().find(|s| s.id == id).unwrap();
            (s.channel, s.engine_label.clone())
        };
        speakers
            .iter()
            .map(|s| {
                (
                    (s.channel, s.engine_label.clone()),
                    (
                        s.display_name.clone(),
                        s.color.clone(),
                        s.global_id,
                        s.merged_into.map(key),
                        s.needs_review,
                        s.name_anchor_start_ms,
                        s.name_anchor_end_ms,
                    ),
                )
            })
            .collect()
    }

    /// `new` was rebuilt from `old`: the same speaker rows, and every word
    /// with the same channel, labels, row, name and colour.
    pub fn assert_identity(store: &dyn RunStore, old: RunId, new: RunId) {
        assert_eq!(rows(store, old), rows(store, new), "speaker rows");
        assert_eq!(seen(store, old), seen(store, new), "words");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::testing::{assert_identity, rows, seen};
    use super::*;
    use crate::punct::fixture::FixturePunctuator;
    use crate::punct::{Mark, WordLabel};
    use crate::store::{MemoryStore, NewMeeting, SqliteStore, Store};
    use crate::transcript::{SegmentOptions, ME_LABEL};
    use crate::types::{EchoDroppedWord, RunKind, Source, SpeakerId};

    fn word(channel: Channel, start: i64, text: &str, spk: Option<&str>) -> TranscriptWord {
        tw(channel, start, text, spk.unwrap_or(ME_LABEL), spk)
    }

    /// A word segmented under `speaker`, with the engine's `raw_label`.
    fn tw(
        channel: Channel,
        start: i64,
        text: &str,
        speaker: &str,
        raw_label: Option<&str>,
    ) -> TranscriptWord {
        TranscriptWord {
            channel,
            start_ms: start,
            end_ms: start + 300,
            text: text.into(),
            confidence: Some(0.9),
            speaker: speaker.into(),
            raw_label: raw_label.map(str::to_owned),
            raw_text: None,
            margin: None,
            overlap: false,
        }
    }

    fn new_meeting(store: &dyn RunStore, source: Source, diarize_mic: bool) -> MeetingId {
        store
            .create_meeting(NewMeeting {
                title: "Identity".into(),
                source,
                source_path: None,
                language: "en".into(),
                diarize_mic,
                started_at: 1_791_000_000_000,
                status: MeetingStatus::Done,
            })
            .unwrap()
            .id
    }

    /// A finished current run of `m` from `t`.
    fn add_run(store: &dyn RunStore, m: MeetingId, kind: RunKind, t: &NewTranscript) -> RunId {
        let run = store
            .create_run(
                m,
                kind,
                serde_json::json!({"runtime": "fake 1", "punctuation": {"applied": false, "reason": "no_model"}}),
            )
            .unwrap();
        store.insert_transcript(run.id, t).unwrap();
        store.finish_run(run.id, RunStatus::Done).unwrap();
        store.set_current_run(m, run.id).unwrap();
        run.id
    }

    fn speaker(store: &dyn RunStore, run: RunId, channel: Channel, label: &str) -> SpeakerId {
        store
            .run_speakers(run)
            .unwrap()
            .into_iter()
            .find(|s| s.channel == channel && s.engine_label == label)
            .unwrap()
            .id
    }

    fn segment_at(store: &dyn RunStore, m: MeetingId, start: i64) -> SegmentId {
        store
            .get_segments(m, 0, u32::MAX)
            .unwrap()
            .into_iter()
            .find(|s| s.start_ms == start)
            .unwrap()
            .id
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

    /// A period after every word.
    fn periods() -> PunctSource {
        PunctSource::factory(|| FixturePunctuator::per_word(|_| WordLabel::post(Mark::Period)))
    }

    fn add(store: &dyn RunStore, m: MeetingId) -> RunId {
        let r = rebuild_run(store, &periods(), m, RebuildWhy::Add, &|| false).unwrap();
        let run = r.run.expect("a rebuilt run");
        assert_eq!(
            store.get_meeting(m).unwrap().current_run.unwrap().id,
            run,
            "current"
        );
        run
    }

    fn both(case: fn(&dyn RunStore)) {
        case(&SqliteStore::open_in_memory().unwrap());
        case(&MemoryStore::new());
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
        assert_eq!(run.status, RunStatus::Done);
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
        assert_identity(store, first, added);
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
        assert_identity(store, added, removed_run);

        // Add again: from the root too; no run is ever deleted.
        let readd = rebuild_run(store, &source(), m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(readd.root, first);
        assert_eq!(texts(store, m)[1], "there.");
        let runs = store.list_runs(m).unwrap();
        assert_eq!(runs.len(), 4);
        for r in [first, added, removed_run, readd.run.unwrap()] {
            assert!(store.get_run(r).is_ok());
        }
        assert_identity(store, first, readd.run.unwrap());
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
        both(add_remove_add);
    }

    /// Finding 1's repro: `transcribe --no-diarize` stores words without an
    /// engine label under `spk_0` (import.rs); renaming that speaker and
    /// punctuating must keep the name on every word.
    fn undiarized_import(store: &dyn RunStore) {
        let m = new_meeting(store, Source::File, false);
        let words: Vec<TranscriptWord> = ["so", "we", "met", "today"]
            .iter()
            .enumerate()
            .map(|(i, t)| tw(Channel::Source, i as i64 * 400, t, "spk_0", None))
            .collect();
        let first = add_run(
            store,
            m,
            RunKind::File,
            &build_transcript_channels(vec![(Channel::Source, words)], &SegmentOptions::default()),
        );
        let spk = speaker(store, first, Channel::Source, "spk_0");
        store.rename_speaker(spk, "Alice", None).unwrap();
        store.set_speaker_color(spk, "#123456").unwrap();
        let added = add(store, m);
        assert_eq!(texts(store, m), ["So.", "We.", "Met.", "Today."]);
        assert_identity(store, first, added);
        assert!(seen(store, added).iter().all(|w| w.name == "Alice"
            && w.color == "#123456"
            && w.own == "spk_0"
            && w.raw_label.is_none()));
        assert_eq!(rows(store, added).len(), 1);
    }

    #[test]
    fn an_undiarized_import_keeps_its_speaker_through_a_rebuild() {
        both(undiarized_import);
    }

    /// A post-pass with a diarized mic stores the mic's untagged words under
    /// `spk_unknown` (postpass.rs), not `me`.
    fn diarized_mic(store: &dyn RunStore) {
        let m = new_meeting(store, Source::Live, true);
        let mic = vec![
            tw(Channel::Mic, 0, "hello", "spk_0", Some("spk_0")),
            tw(Channel::Mic, 400, "there", "spk_unknown", None),
            tw(Channel::Mic, 800, "friend", "spk_0", Some("spk_0")),
            tw(Channel::Mic, 5_000, "right", "spk_unknown", None),
        ];
        let lb = vec![tw(Channel::Loopback, 2_000, "yes", "spk_0", Some("spk_0"))];
        let first = add_run(
            store,
            m,
            RunKind::PostPass,
            &build_transcript_channels(
                vec![(Channel::Mic, mic), (Channel::Loopback, lb)],
                &SegmentOptions::default(),
            ),
        );
        store
            .rename_speaker(
                speaker(store, first, Channel::Mic, "spk_unknown"),
                "Room",
                None,
            )
            .unwrap();
        store
            .rename_speaker(speaker(store, first, Channel::Mic, "spk_0"), "Carol", None)
            .unwrap();
        store
            .rename_speaker(
                speaker(store, first, Channel::Loopback, "spk_0"),
                "Dev",
                None,
            )
            .unwrap();
        let added = add(store, m);
        assert_identity(store, first, added);
        let names: Vec<(String, String)> = seen(store, added)
            .into_iter()
            .map(|w| (w.asr, w.name))
            .collect();
        assert_eq!(
            names,
            [
                ("hello".to_owned(), "Carol".to_owned()),
                ("there".into(), "Room".into()),
                ("friend".into(), "Carol".into()),
                ("yes".into(), "Dev".into()),
                ("right".into(), "Room".into()),
            ]
        );
        assert!(!rows(store, added).contains_key(&(Channel::Mic, ME_LABEL.to_owned())));
    }

    #[test]
    fn a_diarized_mic_keeps_its_untagged_words_through_a_rebuild() {
        both(diarized_mic);
    }

    /// A `user_N` row (made by an earlier re-run's reassign replay) with a
    /// merge into it, a row without words, a review flag, and the fusion
    /// fields of the words.
    fn user_speaker(store: &dyn RunStore) {
        let m = new_meeting(store, Source::Live, false);
        let lb = vec![
            tw(Channel::Loopback, 0, "one", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 400, "two", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 3_000, "three", "user_1", Some("spk_0")),
            tw(Channel::Loopback, 3_400, "four", "user_1", Some("spk_0")),
            tw(Channel::Loopback, 6_000, "five", "spk_1", Some("spk_1")),
        ];
        let mut t =
            build_transcript_channels(vec![(Channel::Loopback, lb)], &SegmentOptions::default());
        let user = t
            .speakers
            .iter()
            .position(|s| s.engine_label == "user_1")
            .unwrap();
        t.speakers[user].needs_review = true;
        // A speaker whose words all went (the echo gate, a reassign).
        t.speakers.push(NewSpeaker {
            channel: Channel::Loopback,
            engine_label: "spk_2".into(),
            display_name: None,
            color: None,
            needs_review: false,
        });
        t.words[1].margin = Some(0.25);
        t.words[3].overlap = true;
        let first = add_run(store, m, RunKind::PostPass, &t);
        let id = |label: &str| speaker(store, first, Channel::Loopback, label);
        store.rename_speaker(id("user_1"), "Dana", None).unwrap();
        store.merge_speakers(id("spk_1"), id("user_1")).unwrap();
        store
            .rename_speaker(id("spk_2"), "Eve", Some(segment_at(store, m, 0)))
            .unwrap();
        store.set_speaker_color(id("spk_2"), "#abcdef").unwrap();

        let added = add(store, m);
        assert_identity(store, first, added);
        let r = rows(store, added);
        assert_eq!(r.len(), 4, "{r:?}");
        let user_row = &r[&(Channel::Loopback, "user_1".to_owned())];
        assert_eq!(user_row.0.as_deref(), Some("Dana"));
        assert!(user_row.4, "the review flag comes along");
        assert_eq!(
            r[&(Channel::Loopback, "spk_1".to_owned())].3,
            Some((Channel::Loopback, "user_1".to_owned())),
            "the merge into user_1"
        );
        let eve = &r[&(Channel::Loopback, "spk_2".to_owned())];
        assert_eq!(
            (eve.0.as_deref(), eve.1.as_deref(), eve.5),
            (Some("Eve"), Some("#abcdef"), Some(0))
        );
        let w = seen(store, added);
        assert_eq!(w[4].name, "Dana", "five: spk_1 shows as Dana");
        assert_eq!((w[1].margin, w[3].overlap), (Some(0.25), true));
        // And once more from the copy.
        let removed = rebuild_run(store, &periods(), m, RebuildWhy::Remove, &|| false)
            .unwrap()
            .run
            .unwrap();
        assert_identity(store, first, removed);
    }

    #[test]
    fn user_speakers_merges_and_rows_without_words_survive_a_rebuild() {
        both(user_speaker);
    }

    /// Finding 3b: reassign to Bob, rename Bob to Robert, rebuild. The
    /// reassigned words stay with Robert; no `user_N` "Bob" appears.
    fn reassign_then_rename(store: &dyn RunStore) {
        let m = new_meeting(store, Source::Live, false);
        let lb = vec![
            tw(Channel::Loopback, 0, "one", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 400, "two", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 3_000, "three", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 3_400, "four", "spk_0", Some("spk_0")),
            tw(Channel::Loopback, 6_000, "five", "spk_1", Some("spk_1")),
        ];
        let first = add_run(
            store,
            m,
            RunKind::PostPass,
            &build_transcript_channels(vec![(Channel::Loopback, lb)], &SegmentOptions::default()),
        );
        let alice = speaker(store, first, Channel::Loopback, "spk_0");
        let bob = speaker(store, first, Channel::Loopback, "spk_1");
        store.rename_speaker(alice, "Alice", None).unwrap();
        store.rename_speaker(bob, "Bob", None).unwrap();
        store
            .reassign_segment(segment_at(store, m, 3_000), bob)
            .unwrap();
        store.rename_speaker(bob, "Robert", None).unwrap();

        let added = add(store, m);
        assert_identity(store, first, added);
        let names: Vec<String> = seen(store, added).into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["Alice", "Alice", "Robert", "Robert", "Robert"]);
        assert_eq!(rows(store, added).len(), 2, "no user_N row");
        let r = rebuild_run(store, &periods(), m, RebuildWhy::Remove, &|| false).unwrap();
        assert_eq!(r.carry_over.unwrap().created_speakers, []);
        assert_identity(store, first, r.run.unwrap());
    }

    #[test]
    fn a_reassignment_survives_a_rename_and_a_rebuild() {
        both(reassign_then_rename);
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

    /// A punctuator that runs `during` while it labels (another change to
    /// the meeting finishing first), then puts a period after every word.
    fn racing(during: impl Fn() + Send + Sync + 'static) -> PunctSource {
        let during = Arc::new(during);
        PunctSource::factory(move || {
            let during = Arc::clone(&during);
            FixturePunctuator::new(move |words, _| {
                during();
                Ok(vec![WordLabel::post(Mark::Period); words.len()])
            })
        })
    }

    fn lost_race(store: Arc<dyn RunStore>) {
        let (m, first) = meeting(&*store);
        // Another copy, made current while the rebuild labels.
        let other = add(&*store, m);
        store.set_current_run(m, first).unwrap();
        let runs = store.list_runs(m).unwrap().len();
        let s = Arc::clone(&store);
        let switch = racing(move || s.set_current_run(m, other).unwrap());
        let err = rebuild_run(&*store, &switch, m, RebuildWhy::Add, &|| false).unwrap_err();
        assert!(is_meeting_changed(&err), "{err}");
        assert_eq!(err.to_string(), format!("conflict: {MEETING_CHANGED}"));
        // The newer current run stays; the run built from the old one is gone.
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, other);
        assert_eq!(store.list_runs(m).unwrap().len(), runs);
        // A meeting that started processing meanwhile is not switched either.
        store.set_current_run(m, first).unwrap();
        let s = Arc::clone(&store);
        let busy = racing(move || s.set_meeting_status(m, MeetingStatus::Processing).unwrap());
        let err = rebuild_run(&*store, &busy, m, RebuildWhy::Add, &|| false).unwrap_err();
        assert!(is_meeting_changed(&err));
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, first);
        assert_eq!(store.list_runs(m).unwrap().len(), runs);
    }

    #[test]
    fn a_rebuild_that_loses_a_race_switches_nothing_and_leaves_no_run() {
        lost_race(Arc::new(SqliteStore::open_in_memory().unwrap()));
        lost_race(Arc::new(MemoryStore::new()));
    }

    fn panics(store: &dyn RunStore) {
        let (m1, first1) = meeting(store);
        let (m2, first2) = meeting(store);
        let p = FixturePunctuator::new(|_, _| panic!("the model blew up"));
        let calls = p.calls_handle();
        // The bulk action's one session.
        let mut loaded: Loaded = Ok(Box::new(p));
        let r1 = rebuild_run_with(store, &mut loaded, m1, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(r1.run, None);
        assert_eq!(r1.punctuation.reason, PunctReason::Error);
        let message = r1.punctuation.message.clone().unwrap();
        assert!(message.contains("the model blew up"), "{message}");
        // The session is gone; the rest of the batch gets its report.
        let report = loaded.as_ref().err().expect("the punctuator was dropped");
        assert_eq!(report.reason, PunctReason::Error);
        let r2 = rebuild_run_with(store, &mut loaded, m2, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!((r2.run, r2.punctuation.reason), (None, PunctReason::Error));
        assert_eq!(calls.lock().unwrap().len(), 1, "never called again");
        // Nothing changed on either meeting.
        assert_eq!(
            store.get_meeting(m1).unwrap().current_run.unwrap().id,
            first1
        );
        assert_eq!(
            store.get_meeting(m2).unwrap().current_run.unwrap().id,
            first2
        );
        assert_eq!(texts(store, m1)[0], "hello");
    }

    #[test]
    fn a_panicking_punctuator_is_dropped_for_the_rest_of_a_bulk_run() {
        both(panics);
    }

    fn sweep(store: &dyn RunStore) {
        let (m, first) = meeting(store);
        let run = |info: serde_json::Value, finish: bool| {
            let r = store.create_run(m, RunKind::PostPass, info).unwrap();
            if finish {
                store.finish_run(r.id, RunStatus::Done).unwrap();
            }
            r.id
        };
        let rebuilt = serde_json::json!({"punctuation": {"rebuilt_from": first.0}});
        let restored = serde_json::json!({"echo": {"restored_from": first.0}});
        let half_rebuild = run(rebuilt.clone(), false);
        let half_restore = run(restored.clone(), false);
        let finished_copy = run(rebuilt.clone(), true);
        let running_post_pass = run(serde_json::json!({"runtime": "fake"}), false);
        // A processing meeting's copies are the post-pass's business.
        let (busy, busy_first) = meeting(store);
        let busy_copy = store
            .create_run(busy, RunKind::PostPass, rebuilt.clone())
            .unwrap()
            .id;
        store
            .set_meeting_status(busy, MeetingStatus::Processing)
            .unwrap();
        // A running copy that is current is kept.
        let (cur, _) = meeting(store);
        let current_copy = store
            .create_run(cur, RunKind::PostPass, restored)
            .unwrap()
            .id;
        store.set_current_run(cur, current_copy).unwrap();

        // At launch, inside the import recovery.
        crate::pipeline::recover_interrupted_imports(store).unwrap();
        for r in [half_rebuild, half_restore] {
            assert!(store.get_run(r).is_err(), "run {r} deleted");
        }
        for r in [
            first,
            finished_copy,
            running_post_pass,
            busy_copy,
            busy_first,
            current_copy,
        ] {
            assert!(store.get_run(r).is_ok(), "run {r} kept");
        }
        // Directly, on a fresh half-built copy.
        let again = run(rebuilt, false);
        assert_eq!(sweep_interrupted_rebuilds(store).unwrap(), [again]);
        assert!(sweep_interrupted_rebuilds(store).unwrap().is_empty());
        assert_eq!(store.get_meeting(m).unwrap().current_run.unwrap().id, first);
    }

    #[test]
    fn the_startup_sweep_deletes_only_half_built_copies() {
        both(sweep);
    }

    /// Plan 16 section 12.3: a rebuild collapses the repeated-token loops of
    /// a run made before the collapse ("Add punctuation" on an older
    /// meeting) with the current setting, and reports them.
    fn rebuild_collapses_loops(store: &dyn RunStore) {
        let older = |store: &dyn RunStore| {
            let m = new_meeting(store, Source::Live, false);
            let mut lb = vec![tw(Channel::Loopback, 0, "so", "spk_0", Some("spk_0"))];
            lb.extend((0..5).map(|_| tw(Channel::Loopback, 400, "we", "spk_0", Some("spk_0"))));
            lb.push(tw(Channel::Loopback, 900, "start", "spk_0", Some("spk_0")));
            let mic = vec![tw(Channel::Mic, 5_000, "ok", ME_LABEL, None)];
            let t = build_transcript_channels(
                vec![(Channel::Mic, mic), (Channel::Loopback, lb)],
                &SegmentOptions::default(),
            );
            (m, add_run(store, m, RunKind::PostPass, &t))
        };
        let count = |m: MeetingId| store.get_words(m, i64::MIN, i64::MAX).unwrap().len();
        let (m, first) = older(store);
        store
            .rename_speaker(
                speaker(store, first, Channel::Loopback, "spk_0"),
                "Alice",
                None,
            )
            .unwrap();
        // Without a model "Add" makes no run, so nothing is collapsed either
        // (and no earlier punctuation is stripped).
        let none = rebuild_run(store, &PunctSource::None, m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!((none.run, count(m)), (None, 8));
        let added = add(store, m);
        assert_eq!(count(m), 4, "one \"we\" of five");
        let we = store
            .get_words(m, i64::MIN, i64::MAX)
            .unwrap()
            .into_iter()
            .find(|w| w.asr_text() == "we")
            .unwrap();
        assert_eq!(
            (we.start_ms, we.end_ms, we.text.as_str()),
            (400, 700, "We.")
        );
        let info = store.get_run(added).unwrap().engine_info;
        assert_eq!(info["loops"]["applied"], true);
        assert_eq!(
            (
                info["loops"]["runs"].as_u64(),
                info["loops"]["words_removed"].as_u64()
            ),
            (Some(1), Some(4))
        );
        assert_eq!(info["loops"]["per_channel"]["loopback"]["words_removed"], 4);
        assert_eq!(rows(store, first), rows(store, added), "speaker rows");
        // Again: nothing changes, no run.
        let again = rebuild_run(store, &periods(), m, RebuildWhy::Add, &|| false).unwrap();
        assert_eq!(again.run, None);
        // Remove keeps the collapse (the words are the collapsed ones) and
        // its counts.
        let removed = rebuild_run(store, &periods(), m, RebuildWhy::Remove, &|| false)
            .unwrap()
            .run
            .unwrap();
        assert_eq!(count(m), 4);
        let info = store.get_run(removed).unwrap().engine_info;
        assert_eq!(
            (
                info["loops"]["runs"].as_u64(),
                info["loops"]["words_removed"].as_u64()
            ),
            (Some(1), Some(4))
        );
        // With the setting off a rebuild keeps every word.
        store
            .set_setting(crate::settings::TRANSCRIPT_COLLAPSE_LOOPS, "off")
            .unwrap();
        let (m2, _) = older(store);
        let off = add(store, m2);
        assert_eq!(count(m2), 8);
        let info = store.get_run(off).unwrap().engine_info;
        assert_eq!(
            (
                info["loops"]["applied"].as_bool(),
                info["loops"]["runs"].as_u64()
            ),
            (Some(false), Some(0))
        );
        store
            .remove_setting(crate::settings::TRANSCRIPT_COLLAPSE_LOOPS)
            .unwrap();
    }

    #[test]
    fn a_rebuild_collapses_the_loops_of_an_older_run() {
        both(rebuild_collapses_loops);
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
        assert!(is_copy(&rebuilt.engine_info) && is_copy(&restored.engine_info));
        assert!(!is_copy(
            &serde_json::json!({"punctuation": {"applied": true}})
        ));
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
