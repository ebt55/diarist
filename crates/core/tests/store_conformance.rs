//! Conformance suite for `diarist_core::store::Store` and `RunStore`
//! implementations.
//!
//! Every case is a function over `&dyn Store` (or `&dyn RunStore`) that
//! starts from an empty store and creates its own data. `MemoryStore` and
//! `SqliteStore` (an in-memory SQLite database) both run it below.
//!
//! Cases assert behaviour the UI relies on, not implementation details: no
//! assumptions about id values, search ranking or snippet window size.

use diarist_core::engine::EngineWord;
use diarist_core::store::{
    MemoryStore, NewAudioTrack, NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord,
    RunStore, ScannedModelFile, SqliteStore, Store, StoreError,
};
use diarist_core::transcript::SegmentOptions;
use diarist_core::types::{
    AudioFormat, Channel, ClockAnchor, EchoDroppedWord, MeetingId, MeetingStatus,
    ModelCapabilities, ModelRole, ModelSource, RunId, RunKind, RunStatus, SegmentId, Source,
    Speaker, SpeakerId, TrackChannel,
};
use diarist_core::views::{default_speaker_color, SegmentView};

type Case = fn(&dyn Store);
type RunCase = fn(&dyn RunStore);

/// All cases, by name.
const CASES: &[(&str, Case)] = &[
    ("empty_store", empty_store),
    ("meeting_lifecycle", meeting_lifecycle),
    (
        "transcript_requires_current_run",
        transcript_requires_current_run,
    ),
    ("segments_page_in_seq_order", segments_page_in_seq_order),
    ("words_in_time_window", words_in_time_window),
    ("search_current_runs_only", search_current_runs_only),
    ("rename_and_color", rename_and_color),
    (
        "merge_flattens_and_rejects_cycles",
        merge_flattens_and_rejects_cycles,
    ),
    ("reassign_segment", reassign_segment),
    ("split_segment", split_segment),
    ("insert_transcript_validation", insert_transcript_validation),
    ("list_order_title_and_delete", list_order_title_and_delete),
    ("settings_round_trip", settings_round_trip),
    ("audio_tracks", audio_tracks),
    ("model_files_sync", model_files_sync),
    ("remove_setting", remove_setting),
    ("playback_track_rules", playback_track_rules),
    ("mark_reviewed", mark_reviewed),
    ("raw_text_round_trip", raw_text_round_trip),
];

/// `RunStore` cases, by name.
const RUN_CASES: &[(&str, RunCase)] = &[
    ("runs_list_get_delete", runs_list_get_delete),
    ("live_latest_revisions", live_latest_revisions),
    ("anchors_uid_speakers_edits", anchors_uid_speakers_edits),
    (
        "carry_over_names_anchor_edits",
        carry_over_names_anchor_edits,
    ),
    ("carry_over_user_speaker", carry_over_user_speaker),
    (
        "rename_at_anchors_a_live_speaker",
        rename_at_anchors_a_live_speaker,
    ),
    ("carry_over_rejects_bad_runs", carry_over_rejects_bad_runs),
    (
        "carry_over_survives_earlier_post_pass_words",
        carry_over_survives_earlier_post_pass_words,
    ),
    (
        "carry_over_survives_engine_word_spikes",
        carry_over_survives_engine_word_spikes,
    ),
    (
        "carry_over_keeps_alternating_speakers_apart",
        carry_over_keeps_alternating_speakers_apart,
    ),
    (
        "carry_over_seeded_engine_like_meetings",
        carry_over_seeded_engine_like_meetings,
    ),
    (
        "rename_without_anchor_keeps_the_anchor",
        rename_without_anchor_keeps_the_anchor,
    ),
    (
        "live_speakers_survive_materialise",
        live_speakers_survive_materialise,
    ),
    ("delete_audio_track", delete_audio_track),
    (
        "echo_dropped_and_gated_materialise",
        echo_dropped_and_gated_materialise,
    ),
    ("carry_over_identity_by_label", carry_over_identity_by_label),
    ("run_segments_and_words", run_segments_and_words),
    ("set_current_run_if_compares", set_current_run_if_compares),
];

fn report(name: &str, e: Box<dyn std::any::Any + Send>) -> ! {
    let msg = e
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    panic!("conformance case `{name}` failed: {msg}");
}

/// Runs every case against a fresh store from `make`.
fn run_all(make: impl Fn() -> Box<dyn RunStore>) {
    for (name, case) in CASES {
        let store = make();
        let as_store: &dyn Store = &*store;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(as_store)));
        if let Err(e) = result {
            report(name, e);
        }
    }
    for (name, case) in RUN_CASES {
        let store = make();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(&*store)));
        if let Err(e) = result {
            report(name, e);
        }
    }
}

#[test]
fn memory_store_conforms() {
    run_all(|| Box::new(MemoryStore::new()));
}

#[test]
fn sqlite_store_conforms() {
    run_all(|| Box::new(SqliteStore::open_in_memory().unwrap()));
}

// ---- fixtures --------------------------------------------------------------

const T0: i64 = 1_791_000_000_000;

fn new_meeting(title: &str, started_at: i64) -> NewMeeting {
    NewMeeting {
        title: title.into(),
        source: Source::File,
        source_path: Some("C:/rec/a.m4a".into()),
        language: "en".into(),
        diarize_mic: false,
        started_at,
        status: MeetingStatus::Processing,
    }
}

/// Three segments: spk_0 "Hello budget team", me "Thanks everyone", spk_1
/// "Budget is approved today". Words are 500 ms each, segments start at 0,
/// 2000, 4000 ms.
fn sample_transcript() -> NewTranscript {
    let texts = [
        (0usize, "Hello budget team"),
        (1, "Thanks everyone"),
        (2, "Budget is approved today"),
    ];
    let mut t = NewTranscript {
        speakers: vec![
            speaker(Channel::Loopback, "spk_0"),
            speaker(Channel::Mic, "me"),
            speaker(Channel::Loopback, "spk_1"),
        ],
        ..Default::default()
    };
    for (i, (spk, text)) in texts.iter().enumerate() {
        let start = i as i64 * 2_000;
        let words: Vec<&str> = text.split(' ').collect();
        t.segments.push(NewSegment {
            speaker: *spk,
            start_ms: start,
            end_ms: start + words.len() as i64 * 500,
            text: (*text).into(),
        });
        for (k, w) in words.iter().enumerate() {
            t.words.push(NewWord {
                segment: i,
                start_ms: start + k as i64 * 500,
                end_ms: start + (k as i64 + 1) * 500,
                text: (*w).into(),
                confidence: None,
                raw_label: Some(format!("raw{spk}")),
                margin: None,
                overlap: false,
                raw_text: None,
            });
        }
    }
    t
}

fn speaker(channel: Channel, label: &str) -> NewSpeaker {
    NewSpeaker {
        channel,
        engine_label: label.into(),
        display_name: None,
        color: None,
        needs_review: false,
    }
}

/// Creates a meeting with the sample transcript as its current, finished run.
fn seeded_meeting(store: &dyn Store, title: &str, started_at: i64) -> (MeetingId, RunId) {
    let m = store
        .create_meeting(new_meeting(title, started_at))
        .unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({"runtime": "test"}))
        .unwrap();
    store
        .insert_transcript(run.id, &sample_transcript())
        .unwrap();
    store.finish_run(run.id, RunStatus::Done).unwrap();
    store.set_current_run(m.id, run.id).unwrap();
    store
        .finish_meeting(m.id, started_at + 6_000, 6_000)
        .unwrap();
    store.set_meeting_status(m.id, MeetingStatus::Done).unwrap();
    (m.id, run.id)
}

fn segments(store: &dyn Store, id: MeetingId) -> Vec<SegmentView> {
    store.get_segments(id, 0, 1_000).unwrap()
}

fn speaker_by_label(store: &dyn Store, id: MeetingId, label: &str) -> SpeakerId {
    store
        .get_meeting(id)
        .unwrap()
        .speakers
        .into_iter()
        .find(|s| s.engine_label == label)
        .unwrap_or_else(|| panic!("no speaker {label}"))
        .id
}

fn is_not_found<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::NotFound { .. }))
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

// ---- cases -----------------------------------------------------------------

fn empty_store(store: &dyn Store) {
    assert!(store.list_meetings().unwrap().is_empty());
    assert!(is_not_found(store.get_meeting(MeetingId(424_242))));
    assert!(is_not_found(store.get_segments(MeetingId(424_242), 0, 10)));
    assert!(is_not_found(store.delete_meeting(MeetingId(424_242))));
    assert!(is_not_found(store.rename_speaker(
        SpeakerId(424_242),
        "x",
        None
    )));
    assert!(store.search("anything", 10).unwrap().is_empty());
    assert!(store.get_settings().unwrap().0.is_empty());
}

fn meeting_lifecycle(store: &dyn Store) {
    let (id, run) = seeded_meeting(store, "Budget review", T0);
    let list = store.list_meetings().unwrap();
    assert_eq!(list.len(), 1);
    let s = &list[0];
    assert_eq!(s.id, id);
    assert_eq!(s.title, "Budget review");
    assert_eq!(s.status, MeetingStatus::Done);
    assert_eq!(s.duration_ms, Some(6_000));
    assert_eq!(s.speaker_count, 3);
    assert!(s.preview.starts_with("Hello budget team"));
    assert!(s.preview.chars().count() <= 200);
    assert_eq!(s.uid.len(), 26);

    let d = store.get_meeting(id).unwrap();
    assert_eq!(d.meeting.current_run_id, Some(run));
    let cur = d.current_run.expect("current run");
    assert_eq!(cur.id, run);
    assert_eq!(cur.status, RunStatus::Done);
    assert!(cur.finished_at.is_some());
    assert_eq!(d.meeting.ended_at, Some(T0 + 6_000));
    assert_eq!(d.speakers.len(), 3);
    let me = d.speakers.iter().find(|s| s.engine_label == "me").unwrap();
    assert_eq!(me.name, "Me");
    assert_eq!(me.segment_count, 1);
    assert_eq!(me.talk_ms, 1_000);
    let spk0 = d
        .speakers
        .iter()
        .find(|s| s.engine_label == "spk_0")
        .unwrap();
    assert_eq!(spk0.name, "Speaker 1");
    assert_eq!(spk0.color, default_speaker_color("spk_0"));
    assert!(d.speakers.iter().all(|s| s.merged_into.is_none()));
}

fn transcript_requires_current_run(store: &dyn Store) {
    let m = store.create_meeting(new_meeting("No run yet", T0)).unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({}))
        .unwrap();
    assert_eq!(run.status, RunStatus::Running);
    store
        .insert_transcript(run.id, &sample_transcript())
        .unwrap();
    // Not current yet: nothing is shown.
    assert!(segments(store, m.id).is_empty());
    assert!(store.get_meeting(m.id).unwrap().speakers.is_empty());
    assert_eq!(store.list_meetings().unwrap()[0].speaker_count, 0);
    store.set_current_run(m.id, run.id).unwrap();
    assert_eq!(segments(store, m.id).len(), 3);
    // A run of another meeting cannot become current.
    let other = store.create_meeting(new_meeting("Other", T0 + 1)).unwrap();
    assert!(is_invalid(store.set_current_run(other.id, run.id)));
}

fn segments_page_in_seq_order(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Paging", T0);
    let all = segments(store, id);
    assert_eq!(all.len(), 3);
    assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(all[0].text, "Hello budget team");
    assert_eq!(all[0].speaker_name, "Speaker 1");
    assert_eq!(all[1].speaker_name, "Me");
    assert_eq!(all[1].channel, Channel::Mic);
    let page = store.get_segments(id, 1, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, all[1].id);
    assert!(store.get_segments(id, 10, 5).unwrap().is_empty());
}

fn words_in_time_window(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Words", T0);
    let words = store.get_words(id, 0, 10_000).unwrap();
    assert_eq!(words.len(), 9);
    assert!(words.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
    assert_eq!(words[0].raw_label.as_deref(), Some("raw0"));
    // [1000, 2500) overlaps "team" (1000..1500) and "Thanks" (2000..2500).
    let texts: Vec<String> = store
        .get_words(id, 1_000, 2_500)
        .unwrap()
        .into_iter()
        .map(|w| w.text)
        .collect();
    assert_eq!(texts, ["team", "Thanks"]);
    assert!(store.get_words(id, 50_000, 60_000).unwrap().is_empty());
}

fn search_current_runs_only(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Searchable", T0);
    let hits = store.search("BUDGET", 10).unwrap();
    assert_eq!(hits.len(), 2);
    for h in &hits {
        assert_eq!(h.meeting_id, id);
        assert_eq!(h.meeting_title, "Searchable");
        let marked: String = h
            .snippet
            .iter()
            .filter(|p| p.hit)
            .map(|p| p.text.to_lowercase())
            .collect();
        assert!(
            marked.contains("budget"),
            "no highlighted hit in {:?}",
            h.snippet
        );
    }
    // All terms must match.
    let both = store.search("budget approved", 10).unwrap();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].speaker_name, "Speaker 2");
    assert_eq!(both[0].start_ms, 4_000);
    assert!(store
        .search("budget nonexistentword", 10)
        .unwrap()
        .is_empty());
    assert!(store.search("   ", 10).unwrap().is_empty());
    assert_eq!(store.search("budget", 1).unwrap().len(), 1);

    // A newer run that is not current is not searched.
    let run2 = store
        .create_run(id, RunKind::Rediarize, serde_json::json!({}))
        .unwrap();
    let mut t = sample_transcript();
    t.segments[0].text = "Zebra crossing".into();
    store.insert_transcript(run2.id, &t).unwrap();
    assert!(store.search("zebra", 10).unwrap().is_empty());
    store.set_current_run(id, run2.id).unwrap();
    assert_eq!(store.search("zebra", 10).unwrap().len(), 1);
}

fn rename_and_color(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Names", T0);
    let spk0 = speaker_by_label(store, id, "spk_0");
    let first_seg = segments(store, id)[0].id;
    store
        .rename_speaker(spk0, "  Priya  ", Some(first_seg))
        .unwrap();
    assert_eq!(segments(store, id)[0].speaker_name, "Priya");
    let view = store
        .get_meeting(id)
        .unwrap()
        .speakers
        .into_iter()
        .find(|s| s.id == spk0)
        .unwrap();
    assert_eq!(view.display_name.as_deref(), Some("Priya"));
    // Blank restores the default.
    store.rename_speaker(spk0, " ", None).unwrap();
    assert_eq!(segments(store, id)[0].speaker_name, "Speaker 1");

    store.set_speaker_color(spk0, "#A1B2C3").unwrap();
    assert_eq!(segments(store, id)[0].speaker_color, "#a1b2c3");
    assert!(is_invalid(store.set_speaker_color(spk0, "red")));
    assert!(is_invalid(store.set_speaker_color(spk0, "#12345")));

    // An anchor from another meeting is rejected.
    let (other, _) = seeded_meeting(store, "Other", T0 + 1);
    let foreign_seg = segments(store, other)[0].id;
    assert!(is_invalid(store.rename_speaker(
        spk0,
        "X",
        Some(foreign_seg)
    )));
}

fn merge_flattens_and_rejects_cycles(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Merges", T0);
    let a = speaker_by_label(store, id, "spk_0");
    let b = speaker_by_label(store, id, "spk_1");
    let me = speaker_by_label(store, id, "me");
    store.rename_speaker(b, "Marcus", None).unwrap();

    store.merge_speakers(a, b).unwrap();
    let segs = segments(store, id);
    assert_eq!(segs[0].speaker_name, "Marcus");
    assert_eq!(segs[0].speaker_id, b);
    let d = store.get_meeting(id).unwrap();
    assert_eq!(
        d.speakers.iter().find(|s| s.id == a).unwrap().merged_into,
        Some(b)
    );
    let root = d.speakers.iter().find(|s| s.id == b).unwrap();
    assert_eq!(root.segment_count, 2, "root counts merged segments");
    assert_eq!(store.list_meetings().unwrap()[0].speaker_count, 2);

    // Merging b (which has a child) into me repoints the child: chains stay one level deep.
    store.merge_speakers(b, me).unwrap();
    let d = store.get_meeting(id).unwrap();
    assert_eq!(
        d.speakers.iter().find(|s| s.id == a).unwrap().merged_into,
        Some(me)
    );
    assert_eq!(
        d.speakers.iter().find(|s| s.id == b).unwrap().merged_into,
        Some(me)
    );
    assert!(segments(store, id).iter().all(|s| s.speaker_name == "Me"));

    // Cycles and self-merges are rejected.
    assert!(is_invalid(store.merge_speakers(me, a)));
    assert!(is_invalid(store.merge_speakers(me, me)));

    // Unmerge restores the speaker's own name; unmerging a root is a no-op.
    store.unmerge_speaker(b).unwrap();
    store.unmerge_speaker(me).unwrap();
    assert_eq!(segments(store, id)[2].speaker_name, "Marcus");

    // Speakers of different meetings cannot be merged.
    let (other, _) = seeded_meeting(store, "Other", T0 + 1);
    let foreign = speaker_by_label(store, other, "spk_0");
    assert!(is_invalid(store.merge_speakers(a, foreign)));
}

fn reassign_segment(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Reassign", T0);
    let me = speaker_by_label(store, id, "me");
    let seg = segments(store, id)[0].id;
    store.reassign_segment(seg, me).unwrap();
    let segs = segments(store, id);
    assert_eq!(segs[0].speaker_id, me);
    assert_eq!(segs[0].speaker_name, "Me");

    let (other, _) = seeded_meeting(store, "Other", T0 + 1);
    let foreign = speaker_by_label(store, other, "me");
    assert!(is_invalid(store.reassign_segment(seg, foreign)));
    assert!(is_not_found(store.reassign_segment(SegmentId(424_242), me)));
}

fn split_segment(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Split", T0);
    let before = segments(store, id);
    let words = store.get_words(id, 4_000, 6_000).unwrap(); // "Budget is approved today"
    assert_eq!(words.len(), 4);
    let new_id = store.split_segment(before[2].id, words[2].id).unwrap();
    let after = segments(store, id);
    assert_eq!(after.len(), 4);
    assert_eq!(after[2].text, "Budget is");
    assert_eq!(after[3].id, new_id);
    assert_eq!(after[3].text, "approved today");
    assert_eq!(after[3].start_ms, words[2].start_ms);
    assert_eq!(after[2].end_ms, words[1].end_ms);
    assert_eq!(after[3].speaker_id, after[2].speaker_id);
    assert!(after.windows(2).all(|w| w[0].seq < w[1].seq));
    let moved = store.get_words(id, words[2].start_ms, 6_000).unwrap();
    assert!(moved.iter().all(|w| w.segment_id == new_id));

    // Splitting before the first word, or at a foreign word, is invalid.
    assert!(is_invalid(store.split_segment(before[0].id, words[0].id)));
    let first_words = store.get_words(id, 0, 500).unwrap();
    assert!(is_invalid(
        store.split_segment(before[0].id, first_words[0].id)
    ));
}

fn insert_transcript_validation(store: &dyn Store) {
    let m = store.create_meeting(new_meeting("Validation", T0)).unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({}))
        .unwrap();
    let mut bad = sample_transcript();
    bad.segments[0].speaker = 99;
    assert!(is_invalid(store.insert_transcript(run.id, &bad)));
    let mut bad = sample_transcript();
    bad.words[0].segment = 99;
    assert!(is_invalid(store.insert_transcript(run.id, &bad)));
    let mut bad = sample_transcript();
    bad.speakers[2].engine_label = "spk_0".into();
    assert!(is_invalid(store.insert_transcript(run.id, &bad)));
    // Failed validation wrote nothing, so a good transcript still goes in, once.
    store
        .insert_transcript(run.id, &sample_transcript())
        .unwrap();
    assert!(matches!(
        store.insert_transcript(run.id, &sample_transcript()),
        Err(StoreError::Conflict(_))
    ));
    assert!(is_not_found(
        store.insert_transcript(RunId(424_242), &sample_transcript())
    ));
    assert!(is_not_found(store.create_run(
        MeetingId(424_242),
        RunKind::File,
        serde_json::json!({})
    )));
}

fn list_order_title_and_delete(store: &dyn Store) {
    let (old, _) = seeded_meeting(store, "Old", T0);
    let (new, _) = seeded_meeting(store, "New", T0 + 86_400_000);
    let (mid, _) = seeded_meeting(store, "Mid", T0 + 3_600_000);
    let ids: Vec<MeetingId> = store
        .list_meetings()
        .unwrap()
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, [new, mid, old]);

    store.set_meeting_title(mid, "  Renamed  ").unwrap();
    assert_eq!(store.get_meeting(mid).unwrap().meeting.title, "Renamed");
    store.set_audio_keep(mid, true).unwrap();
    assert!(store.get_meeting(mid).unwrap().meeting.audio_keep);

    store.delete_meeting(mid).unwrap();
    assert!(is_not_found(store.get_meeting(mid)));
    assert!(is_not_found(store.get_segments(mid, 0, 10)));
    assert_eq!(store.list_meetings().unwrap().len(), 2);
    assert_eq!(store.search("budget", 100).unwrap().len(), 4);
    assert!(store
        .search("budget", 100)
        .unwrap()
        .iter()
        .all(|h| h.meeting_id != mid));
    assert!(is_not_found(store.delete_meeting(mid)));
}

fn settings_round_trip(store: &dyn Store) {
    store.set_setting("theme", "dark").unwrap();
    store.set_setting("post_pass_mode", "auto").unwrap();
    store.set_setting("theme", "light").unwrap();
    let s = store.get_settings().unwrap();
    assert_eq!(s.0.len(), 2);
    assert_eq!(s.0["theme"], "light");
    assert_eq!(s.0["post_pass_mode"], "auto");
    assert!(is_invalid(store.set_setting(" ", "x")));
}

fn audio_tracks(store: &dyn Store) {
    let (id, _) = seeded_meeting(store, "Audio", T0);
    assert!(store.list_audio_tracks(id).unwrap().is_empty());
    let track = store
        .add_audio_track(NewAudioTrack {
            meeting_id: id,
            channel: TrackChannel::Playback,
            path: "C:/rec/a.m4a".into(),
            format: AudioFormat::Original,
            sample_rate: 48_000,
            channels: 2,
            duration_ms: Some(6_000),
            size_bytes: Some(96_000),
            channel_frames: None,
        })
        .unwrap();
    assert_eq!(track.channel_frames, None);
    // The playback FLAC of a live meeting keeps each channel's own length.
    let flac = store
        .add_audio_track(NewAudioTrack {
            meeting_id: id,
            channel: TrackChannel::Playback,
            path: "audio/U/playback.flac".into(),
            format: AudioFormat::Flac,
            sample_rate: 16_000,
            channels: 2,
            duration_ms: Some(6_000),
            size_bytes: Some(50_000),
            channel_frames: Some(vec![96_000, 95_123]),
        })
        .unwrap();
    assert_eq!(flac.channel_frames, Some(vec![96_000, 95_123]));
    let tracks = store.list_audio_tracks(id).unwrap();
    assert_eq!(tracks, vec![track, flac]);
    assert!(is_not_found(store.list_audio_tracks(MeetingId(424_242))));
    store.delete_meeting(id).unwrap();
    assert!(is_not_found(store.list_audio_tracks(id)));
}

fn scanned(path: &str, role: ModelRole, name: &str) -> ScannedModelFile {
    ScannedModelFile {
        path: path.into(),
        role,
        architecture: Some("arch".into()),
        display_name: name.into(),
        quant: Some("Q8_0".into()),
        size_bytes: 1_000,
        sha256: None,
        source: ModelSource::User,
        capabilities: ModelCapabilities::default(),
        manifest_name: None,
    }
}

fn model_files_sync(store: &dyn Store) {
    assert!(store.list_model_files().unwrap().is_empty());
    let first = store
        .sync_model_files(
            &[
                scanned("models/b.gguf", ModelRole::Diarizer, "B diarizer"),
                scanned("models/a.gguf", ModelRole::Asr, "A asr"),
            ],
            1_000,
        )
        .unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].role, ModelRole::Asr, "ordered by role");
    assert!(first.iter().all(|f| !f.missing && f.first_seen_at == 1_000));
    let a_id = first[0].id;

    // Second scan: a.gguf changed size, b.gguf vanished, c.gguf is new.
    let mut a = scanned("models/a.gguf", ModelRole::Asr, "A asr");
    a.size_bytes = 2_000;
    let second = store
        .sync_model_files(
            &[a, scanned("models/c.gguf", ModelRole::Asr, "C asr")],
            2_000,
        )
        .unwrap();
    assert_eq!(second, store.list_model_files().unwrap());
    assert_eq!(second.len(), 3);
    let a = second.iter().find(|f| f.path == "models/a.gguf").unwrap();
    assert_eq!(a.id, a_id, "id survives a rescan");
    assert_eq!(
        (a.first_seen_at, a.last_seen_at, a.size_bytes),
        (1_000, 2_000, 2_000)
    );
    let b = second.iter().find(|f| f.path == "models/b.gguf").unwrap();
    assert!(b.missing);
    assert_eq!(b.last_seen_at, 1_000);
    // A file that comes back is no longer missing.
    let third = store
        .sync_model_files(
            &[scanned("models/b.gguf", ModelRole::Diarizer, "B diarizer")],
            3_000,
        )
        .unwrap();
    assert!(
        !third
            .iter()
            .find(|f| f.path == "models/b.gguf")
            .unwrap()
            .missing
    );
    assert!(is_invalid(store.sync_model_files(
        &[
            scanned("models/x.gguf", ModelRole::Asr, "X"),
            scanned("models/x.gguf", ModelRole::Asr, "X")
        ],
        4_000
    )));
}

fn track(meeting_id: MeetingId, channel: TrackChannel, format: AudioFormat) -> NewAudioTrack {
    NewAudioTrack {
        meeting_id,
        channel,
        path: format!("audio/x/{channel}.{format}"),
        format,
        sample_rate: 16_000,
        channels: 1,
        duration_ms: Some(6_000),
        size_bytes: None,
        channel_frames: None,
    }
}

fn remove_setting(store: &dyn Store) {
    store.set_setting("theme", "dark").unwrap();
    store.set_setting("default_language", "de").unwrap();
    store.remove_setting("theme").unwrap();
    store.remove_setting("never_set").unwrap();
    let s = store.get_settings().unwrap();
    assert_eq!(s.0.len(), 1);
    assert_eq!(s.0["default_language"], "de");
    assert!(is_invalid(store.remove_setting(" ")));
}

fn playback_track_rules(store: &dyn Store) {
    // Import: the original wins over a newer intermediate WAV.
    let (file, _) = seeded_meeting(store, "Import", T0);
    assert_eq!(store.playback_track(file).unwrap(), None);
    let original = store
        .add_audio_track(track(file, TrackChannel::Playback, AudioFormat::Original))
        .unwrap();
    store
        .add_audio_track(track(file, TrackChannel::Source, AudioFormat::Wav))
        .unwrap();
    assert_eq!(store.playback_track(file).unwrap(), Some(original));

    // Live: the newest playback track; before it exists, the newest track.
    let live = store
        .create_meeting(NewMeeting {
            source: Source::Live,
            source_path: None,
            ..new_meeting("Live", T0 + 1)
        })
        .unwrap()
        .id;
    store
        .add_audio_track(track(live, TrackChannel::Mic, AudioFormat::Wav))
        .unwrap();
    let loopback = store
        .add_audio_track(track(live, TrackChannel::Loopback, AudioFormat::Wav))
        .unwrap();
    assert_eq!(store.playback_track(live).unwrap(), Some(loopback));
    let flac = store
        .add_audio_track(track(live, TrackChannel::Playback, AudioFormat::Flac))
        .unwrap();
    store
        .add_audio_track(track(live, TrackChannel::Mic, AudioFormat::Wav))
        .unwrap();
    assert_eq!(store.playback_track(live).unwrap(), Some(flac));
    assert!(is_not_found(store.playback_track(MeetingId(424_242))));
}

fn mark_reviewed(store: &dyn Store) {
    let m = store.create_meeting(new_meeting("Review", T0)).unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({}))
        .unwrap();
    let mut t = sample_transcript();
    t.speakers[2].needs_review = true;
    store.insert_transcript(run.id, &t).unwrap();
    store.set_current_run(m.id, run.id).unwrap();
    let spk = speaker_by_label(store, m.id, "spk_1");
    let flagged = |store: &dyn Store| {
        store
            .get_meeting(m.id)
            .unwrap()
            .speakers
            .iter()
            .find(|s| s.id == spk)
            .unwrap()
            .needs_review
    };
    assert!(flagged(store));
    assert!(segments(store, m.id)[2].needs_review);
    store.mark_reviewed(spk).unwrap();
    assert!(!flagged(store));
    assert!(!segments(store, m.id)[2].needs_review);
    store.mark_reviewed(spk).unwrap();
    assert!(is_not_found(store.mark_reviewed(SpeakerId(424_242))));
}

// ---- RunStore cases ----------------------------------------------------------

fn runs_list_get_delete(store: &dyn RunStore) {
    let (id, current) = seeded_meeting(store, "Runs", T0);
    let other = store
        .create_run(id, RunKind::Rediarize, serde_json::json!({"n": 2}))
        .unwrap();
    store
        .insert_transcript(other.id, &sample_transcript())
        .unwrap();
    let runs = store.list_runs(id).unwrap();
    assert_eq!(
        runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![current, other.id]
    );
    assert_eq!(store.get_run(other.id).unwrap().engine_info["n"], 2);
    store
        .set_run_engine_info(other.id, serde_json::json!({"n": 3, "note": "x"}))
        .unwrap();
    let info = store.get_run(other.id).unwrap().engine_info;
    assert_eq!(
        (info["n"].as_i64(), info["note"].as_str()),
        (Some(3), Some("x"))
    );
    assert!(is_not_found(
        store.set_run_engine_info(RunId(424_242), serde_json::json!({}))
    ));
    assert_eq!(store.run_speakers(other.id).unwrap().len(), 3);
    // Deleting a non-current run leaves the transcript alone.
    store.delete_run(other.id).unwrap();
    assert!(is_not_found(store.get_run(other.id)));
    assert!(is_not_found(store.run_speakers(other.id)));
    assert_eq!(segments(store, id).len(), 3);
    assert_eq!(store.search("approved", 10).unwrap().len(), 1);
    // Deleting the current run clears it.
    store.delete_run(current).unwrap();
    let d = store.get_meeting(id).unwrap();
    assert_eq!(d.meeting.current_run_id, None);
    assert!(d.current_run.is_none());
    assert!(segments(store, id).is_empty());
    assert!(store.search("approved", 10).unwrap().is_empty());
    assert!(store.list_runs(id).unwrap().is_empty());
    assert!(is_not_found(store.list_runs(MeetingId(424_242))));
    assert!(is_not_found(store.delete_run(RunId(424_242))));
}

fn ew(t0_ms: u64, t1_ms: u64, text: &str, spk: Option<&str>) -> EngineWord {
    EngineWord {
        t0_us: t0_ms * 1_000,
        t1_us: t1_ms * 1_000,
        text: text.into(),
        conf: None,
        spk: spk.map(str::to_owned),
    }
}

fn live_meeting(store: &dyn RunStore) -> MeetingId {
    store
        .create_meeting(NewMeeting {
            source: Source::Live,
            source_path: None,
            status: MeetingStatus::Recording,
            ..new_meeting("Live", T0)
        })
        .unwrap()
        .id
}

fn live_latest_revisions(store: &dyn RunStore) {
    let m = live_meeting(store);
    let run = store
        .create_run(m, RunKind::Live, serde_json::json!({}))
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
    assert!(is_not_found(store.append_live_utterance(
        RunId(424_242),
        Channel::Mic,
        0,
        0,
        &[]
    )));
    let rows = store.list_live_utterances(run).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[1].utt, rows[1].rev), (0, 1));
    assert_eq!(rows[1].raw_label.as_deref(), Some("spk_0"));
    assert_eq!((rows[1].start_ms, rows[1].end_ms), (0, 700));
    assert_eq!(rows[1].words[1].t1_us, 700_000);
    assert_eq!(rows[2].raw_label, None);

    store
        .materialise_live_run(run, &SegmentOptions::default())
        .unwrap();
    store.set_current_run(m, run).unwrap();
    let texts: Vec<(String, String)> = segments(store, m)
        .into_iter()
        .map(|s| (s.text, s.speaker_name))
        .collect();
    // Each channel is segmented on its own, then segments go by start time:
    // the mic's "hi" inside the loopback utterance does not split it.
    assert_eq!(
        texts,
        [
            ("hello all".to_owned(), "Speaker 1".to_owned()),
            ("hi".to_owned(), "Me".to_owned()),
        ]
    );
    assert!(matches!(
        store.materialise_live_run(run, &SegmentOptions::default()),
        Err(StoreError::Conflict(_))
    ));
    // Live rows go with the run.
    store.delete_run(run).unwrap();
    assert!(is_not_found(store.list_live_utterances(run)));
}

/// The echo gate's list of removed words round-trips per run and goes with
/// it; a gated materialisation leaves the skipped live words out while they
/// stay stored.
fn echo_dropped_and_gated_materialise(store: &dyn RunStore) {
    let m = live_meeting(store);
    let run = store
        .create_run(m, RunKind::Live, serde_json::json!({}))
        .unwrap()
        .id;
    assert_eq!(store.echo_dropped(run).unwrap(), None);
    let words = vec![
        EchoDroppedWord {
            start_ms: 1_000,
            end_ms: 1_200,
            text: "copied".into(),
            coefficient: 0.75,
            reason: Some(diarist_core::types::EchoReason::Correlation),
        },
        EchoDroppedWord {
            start_ms: 2_000,
            end_ms: 2_300,
            text: "twice".into(),
            coefficient: 0.42,
            reason: None,
        },
    ];
    store.set_echo_dropped(run, &words).unwrap();
    assert_eq!(store.echo_dropped(run).unwrap(), Some(words.clone()));
    store.set_echo_dropped(run, &words[1..]).unwrap();
    assert_eq!(store.echo_dropped(run).unwrap(), Some(words[1..].to_vec()));
    store.set_echo_dropped(run, &[]).unwrap();
    assert_eq!(store.echo_dropped(run).unwrap(), Some(Vec::new()));
    assert!(is_not_found(store.echo_dropped(RunId(424_242))));
    assert!(is_not_found(store.set_echo_dropped(RunId(424_242), &words)));
    // Its engine_info does not carry the list.
    assert_eq!(
        store.get_run(run).unwrap().engine_info,
        serde_json::json!({})
    );

    store
        .append_live_utterance(
            run,
            Channel::Mic,
            0,
            0,
            &[ew(0, 300, "echo", None), ew(400, 700, "mine", None)],
        )
        .unwrap();
    store
        .append_live_utterance(
            run,
            Channel::Mic,
            0,
            1,
            &[
                ew(0, 300, "echo", None),
                ew(400, 700, "mine", None),
                ew(800, 900, "too", None),
            ],
        )
        .unwrap();
    // Only the latest revision is asked: word 0 of rev 1 goes.
    store
        .materialise_live_run_except(run, &SegmentOptions::default(), &|u, i| {
            assert_eq!(u.rev, 1);
            i == 0
        })
        .unwrap();
    store.set_current_run(m, run).unwrap();
    let texts: Vec<String> = segments(store, m).into_iter().map(|s| s.text).collect();
    assert_eq!(texts, ["mine too"]);
    assert_eq!(store.list_live_utterances(run).unwrap()[1].words.len(), 3);
    store.delete_run(run).unwrap();
    assert!(is_not_found(store.echo_dropped(run)));
}

fn live_speakers_survive_materialise(store: &dyn RunStore) {
    let m = live_meeting(store);
    let run = store
        .create_run(m, RunKind::Live, serde_json::json!({}))
        .unwrap()
        .id;
    // While recording: the provisional speaker gets a row on first use...
    let a = store.live_speaker(run, Channel::Loopback, "spk_0").unwrap();
    assert_eq!(a.engine_label, "spk_0");
    assert_eq!(a.channel, Channel::Loopback);
    assert_eq!(a.run_id, run);
    assert_eq!(
        store
            .live_speaker(run, Channel::Loopback, "spk_0")
            .unwrap()
            .id,
        a.id
    );
    let b = store.live_speaker(run, Channel::Loopback, "spk_1").unwrap();
    assert_ne!(a.id, b.id);
    // ...and can be renamed, recoloured and merged before Stop.
    store.rename_speaker(a.id, "Alice", None).unwrap();
    store.set_speaker_color(a.id, "#123456").unwrap();
    store.merge_speakers(b.id, a.id).unwrap();
    store
        .append_live_utterance(
            run,
            Channel::Loopback,
            0,
            0,
            &[
                ew(0, 300, "hello", Some("spk_0")),
                ew(400, 700, "there", Some("spk_1")),
            ],
        )
        .unwrap();
    store
        .append_live_utterance(run, Channel::Mic, 0, 0, &[ew(900, 1_000, "hi", None)])
        .unwrap();
    store
        .materialise_live_run(run, &SegmentOptions::default())
        .unwrap();
    store.set_current_run(m, run).unwrap();
    let speakers = store.run_speakers(run).unwrap();
    assert_eq!(speakers.len(), 3, "{speakers:?}");
    let alice = speakers.iter().find(|s| s.id == a.id).unwrap();
    assert_eq!(alice.display_name.as_deref(), Some("Alice"));
    assert_eq!(alice.color.as_deref(), Some("#123456"));
    assert_eq!(
        speakers.iter().find(|s| s.id == b.id).unwrap().merged_into,
        Some(a.id)
    );
    let segs = segments(store, m);
    assert_eq!(segs[0].speaker_name, "Alice");
    assert!(segs
        .iter()
        .any(|s| s.text == "hi" && s.speaker_name == "Me"));
    // After materialising, no new live speakers.
    assert!(matches!(
        store.live_speaker(run, Channel::Loopback, "spk_9"),
        Err(StoreError::Conflict(_))
    ));
    // Only live runs have live speakers.
    let (_, file_run) = seeded_meeting(store, "File", T0);
    assert!(is_invalid(store.live_speaker(
        file_run,
        Channel::Source,
        "spk_0"
    )));
    assert!(is_not_found(store.live_speaker(
        RunId(424_242),
        Channel::Mic,
        "me"
    )));
}

fn delete_audio_track(store: &dyn RunStore) {
    let m = live_meeting(store);
    let mic = store
        .add_audio_track(track(m, TrackChannel::Mic, AudioFormat::Wav))
        .unwrap();
    let flac = store
        .add_audio_track(track(m, TrackChannel::Playback, AudioFormat::Flac))
        .unwrap();
    store.delete_audio_track(mic.id).unwrap();
    let left = store.list_audio_tracks(m).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, flac.id);
    assert!(is_not_found(store.delete_audio_track(mic.id)));
}

fn anchors_uid_speakers_edits(store: &dyn RunStore) {
    let (m, run) = seeded_meeting(store, "Helpers", T0);
    let uid = store.get_meeting(m).unwrap().meeting.uid;
    assert_eq!(store.meeting_id_by_uid(&uid).unwrap(), Some(m));
    assert_eq!(store.meeting_id_by_uid("nope").unwrap(), None);
    let anchor = |sample_index: i64, qpc_100ns: i64| ClockAnchor {
        meeting_id: m,
        channel: Channel::Mic,
        sample_index,
        qpc_100ns,
    };
    store
        .add_clock_anchors(&[anchor(16_000, 2), anchor(0, 1)])
        .unwrap();
    store.add_clock_anchors(&[anchor(16_000, 3)]).unwrap();
    let anchors = store.clock_anchors(m, Channel::Mic).unwrap();
    assert_eq!(anchors, vec![anchor(0, 1), anchor(16_000, 3)]);
    assert!(store
        .clock_anchors(m, Channel::Loopback)
        .unwrap()
        .is_empty());
    assert!(is_not_found(store.add_clock_anchors(&[ClockAnchor {
        meeting_id: MeetingId(424_242),
        ..anchor(0, 0)
    }])));

    let speakers = store.run_speakers(run).unwrap();
    assert_eq!(speakers.len(), 3);
    assert!(speakers.windows(2).all(|p| p[0].id < p[1].id));
    let seg = segments(store, m)[0].id;
    let target = speakers.iter().find(|s| s.engine_label == "spk_1").unwrap();
    store.rename_speaker(target.id, "Dana", None).unwrap();
    store.reassign_segment(seg, target.id).unwrap();
    let edits = store.list_edits(m).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].payload.to_engine_label.as_deref(), Some("spk_1"));
    assert_eq!(edits[0].payload.to_display_name.as_deref(), Some("Dana"));
    assert_eq!(edits[0].channel, Channel::Loopback);
    assert_eq!((edits[0].start_ms, edits[0].end_ms), (0, 1_500));
    store.delete_meeting(m).unwrap();
    assert!(is_not_found(store.list_edits(m)));
    assert!(is_not_found(store.clock_anchors(m, Channel::Mic)));
}

/// A transcript on the loopback channel from `(label, [(first_s, last_s)])`
/// segments with one 800 ms word per second.
fn carry_transcript(spec: &[(&str, &[(i64, i64)])]) -> NewTranscript {
    let mut t = NewTranscript::default();
    let mut segs: Vec<(usize, i64, i64)> = Vec::new();
    for (label, ranges) in spec {
        t.speakers.push(speaker(Channel::Loopback, label));
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
                raw_label: Some(spec[spk].0.to_owned()),
                margin: None,
                overlap: false,
                raw_text: None,
            });
        }
    }
    t
}

fn run_with(store: &dyn RunStore, m: MeetingId, kind: RunKind, t: &NewTranscript) -> RunId {
    let run = store.create_run(m, kind, serde_json::json!({})).unwrap().id;
    store.insert_transcript(run, t).unwrap();
    run
}

fn by_label(store: &dyn RunStore, run: RunId, label: &str) -> Speaker {
    store
        .run_speakers(run)
        .unwrap()
        .into_iter()
        .find(|s| s.engine_label == label)
        .unwrap_or_else(|| panic!("no speaker {label}"))
}

fn segment_at(store: &dyn RunStore, m: MeetingId, start_ms: i64) -> SegmentId {
    segments(store, m)
        .into_iter()
        .find(|s| s.start_ms == start_ms)
        .unwrap_or_else(|| panic!("no segment at {start_ms}"))
        .id
}

fn carry_over_names_anchor_edits(store: &dyn RunStore) {
    let m = live_meeting(store);
    // Old (live) run: A 0-20 s, B 20-30 + 30-40 s, C 40-43 s (too short),
    // D 43-50 + 50-60 s.
    let old = run_with(
        store,
        m,
        RunKind::Live,
        &carry_transcript(&[
            ("spk_0", &[(0, 20)]),
            ("spk_1", &[(20, 30), (30, 40)]),
            ("spk_2", &[(40, 43)]),
            ("spk_3", &[(43, 50), (50, 60)]),
        ]),
    );
    store.set_current_run(m, old).unwrap();
    let a = by_label(store, old, "spk_0");
    let b = by_label(store, old, "spk_1");
    let c = by_label(store, old, "spk_2");
    store.rename_speaker(a.id, "Priya", None).unwrap();
    store.set_speaker_color(a.id, "#123456").unwrap();
    // Marcus was renamed from his 30-40 s segment: that is the anchor.
    store
        .rename_speaker(b.id, "Marcus", Some(segment_at(store, m, 30_000)))
        .unwrap();
    store.rename_speaker(c.id, "Ghost", None).unwrap();
    // The user gave D's 50-60 s segment to Priya.
    store
        .reassign_segment(segment_at(store, m, 50_000), a.id)
        .unwrap();

    // New (post-pass) run: X 0-20, Y 20-31, Z 31-43, W 43-60 (one segment).
    let new = run_with(
        store,
        m,
        RunKind::PostPass,
        &carry_transcript(&[
            ("spk_0", &[(0, 20)]),
            ("spk_1", &[(20, 31)]),
            ("spk_2", &[(31, 43)]),
            ("spk_3", &[(43, 60)]),
        ]),
    );
    let report = store.carry_over(old, new).unwrap();
    let x = by_label(store, new, "spk_0");
    let y = by_label(store, new, "spk_1");
    let z = by_label(store, new, "spk_2");
    let w = by_label(store, new, "spk_3");

    // Overlap of speech regions: A-X, B-Y (11 s beats B-Z 9 s); C (3 s) and
    // D (43-50 s once its 50-60 s went to Priya: 7 s) are under the 10 s
    // region minimum. (With the earlier word-time rule, 5 s of 800 ms words,
    // D's 5.6 s matched W.)
    assert!(report.matched.contains(&(a.id, x.id)));
    assert!(report.matched.contains(&(b.id, y.id)));
    assert_eq!(report.matched.len(), 2);
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
    // W matched nobody (D is under the minimum): a new label to check.
    assert_eq!(report.needs_review, {
        let mut v = vec![y.id, z.id, w.id];
        v.sort();
        v
    });
    // Ghost (under the minimum) names nobody; W has no name.
    assert_eq!(w.display_name, None);
    assert!(w.needs_review);
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
    let segs = segments(store, m);
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

/// A live run while recording: `spk_0` says three words (2.4 s, under the
/// 5 s overlap minimum) at 10-13 s, `spk_1` talks from 20 to 40 s. The user
/// renames `spk_0` from its utterance (`anchored`) or from the legend.
/// Returns the meeting, the live run and the two live speakers.
fn recorded_live_run(
    store: &dyn RunStore,
    anchored: bool,
) -> (MeetingId, RunId, SpeakerId, SpeakerId) {
    let m = live_meeting(store);
    let live = store
        .create_run(m, RunKind::Live, serde_json::json!({}))
        .unwrap()
        .id;
    let words = |from: u64, to: u64, spk: &str| -> Vec<EngineWord> {
        (from..to)
            .map(|s| ew(s * 1_000, s * 1_000 + 800, &format!("w{s}"), Some(spk)))
            .collect()
    };
    store
        .append_live_utterance(live, Channel::Loopback, 0, 0, &words(10, 13, "spk_0"))
        .unwrap();
    store
        .append_live_utterance(live, Channel::Loopback, 1, 0, &words(20, 40, "spk_1"))
        .unwrap();
    let a = store
        .live_speaker(live, Channel::Loopback, "spk_0")
        .unwrap();
    let b = store
        .live_speaker(live, Channel::Loopback, "spk_1")
        .unwrap();
    if anchored {
        store
            .rename_speaker_at(a.id, "  Dana ", 10_000, 12_800)
            .unwrap();
    } else {
        store.rename_speaker(a.id, "Dana", None).unwrap();
    }
    store.rename_speaker(b.id, "Eve", None).unwrap();
    store
        .materialise_live_run(live, &SegmentOptions::default())
        .unwrap();
    store.finish_run(live, RunStatus::Done).unwrap();
    store.set_current_run(m, live).unwrap();
    (m, live, a.id, b.id)
}

fn rename_at_anchors_a_live_speaker(store: &dyn RunStore) {
    let (m, live, a, b) = recorded_live_run(store, true);
    // The anchor is stored and survives the materialisation.
    let row = store
        .run_speakers(live)
        .unwrap()
        .into_iter()
        .find(|s| s.id == a)
        .unwrap();
    assert_eq!(row.display_name.as_deref(), Some("Dana"));
    assert_eq!(
        (row.name_anchor_start_ms, row.name_anchor_end_ms),
        (Some(10_000), Some(12_800))
    );
    assert_eq!(segments(store, m)[0].speaker_name, "Dana");
    assert!(is_invalid(store.rename_speaker_at(b, "X", 5_000, 4_999)));
    assert!(is_not_found(store.rename_speaker_at(
        SpeakerId(424_242),
        "X",
        0,
        1
    )));

    // The post-pass labels the three words spk_1 and the long talker spk_0.
    let new = run_with(
        store,
        m,
        RunKind::PostPass,
        &carry_transcript(&[("spk_0", &[(20, 40)]), ("spk_1", &[(10, 13)])]),
    );
    let report = store.carry_over(live, new).unwrap();
    let dana = by_label(store, new, "spk_1");
    let eve = by_label(store, new, "spk_0");
    // The anchored name goes to the owner of the anchor range although the
    // old speaker had under 5 s of speech; the unanchored name follows the
    // overlap match.
    assert_eq!(dana.display_name.as_deref(), Some("Dana"));
    assert_eq!(dana.name_anchor_start_ms, Some(10_000));
    assert_eq!(eve.display_name.as_deref(), Some("Eve"));
    assert_eq!(report.matched, [(b, eve.id)]);
    // Unmatched by overlap: the new label is flagged for review.
    assert!(dana.needs_review && !eve.needs_review);

    // The same rename from the legend (no anchor): too short to match, the
    // name reaches spk_1 through the fallback (it covers all of Dana's speech
    // region), flagged for review. (Before the fallback it was lost.)
    let (m2, live2, _, _) = recorded_live_run(store, false);
    let new2 = run_with(
        store,
        m2,
        RunKind::PostPass,
        &carry_transcript(&[("spk_0", &[(20, 40)]), ("spk_1", &[(10, 13)])]),
    );
    store.carry_over(live2, new2).unwrap();
    let dana2 = by_label(store, new2, "spk_1");
    assert_eq!(dana2.display_name.as_deref(), Some("Dana"));
    assert!(dana2.needs_review);
    assert_eq!(
        by_label(store, new2, "spk_0").display_name.as_deref(),
        Some("Eve")
    );
}

fn carry_over_user_speaker(store: &dyn RunStore) {
    let m = live_meeting(store);
    let old = run_with(
        store,
        m,
        RunKind::Live,
        &carry_transcript(&[("spk_0", &[(0, 10), (10, 20)]), ("spk_1", &[(20, 30)])]),
    );
    store.set_current_run(m, old).unwrap();
    let b = by_label(store, old, "spk_1");
    store.rename_speaker(b.id, "Temp", None).unwrap();
    store
        .reassign_segment(segment_at(store, m, 10_000), b.id)
        .unwrap();
    // Renamed afterwards: the edit still says "Temp".
    store.rename_speaker(b.id, "Final", None).unwrap();

    let new = run_with(
        store,
        m,
        RunKind::PostPass,
        &carry_transcript(&[("spk_0", &[(0, 20)]), ("spk_1", &[(20, 30)])]),
    );
    let report = store.carry_over(old, new).unwrap();
    assert_eq!(report.created_speakers.len(), 1);
    let user = by_label(store, new, "user_1");
    assert_eq!(user.display_name.as_deref(), Some("Temp"));
    assert!(user.needs_review);
    // Final (old B, 10-30 s after the reassign) matched the new spk_1.
    assert_eq!(
        by_label(store, new, "spk_1").display_name.as_deref(),
        Some("Final")
    );
    store.set_current_run(m, new).unwrap();
    let names: Vec<(i64, String)> = segments(store, m)
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

/// One block of a meeting with turn-taking as it comes out of the engine:
/// `(speaker, word durations in ms, gap between its words, pause after)`.
/// Short words (150 to 300 ms) and quick turns (240 ms) included.
const TURNS: &[(usize, &[i64], i64, i64)] = &[
    (0, &[240, 300, 400, 200, 560, 320], 80, 400),
    (1, &[200, 160, 480, 300], 0, 240),
    (2, &[300, 240, 200, 400, 360, 240, 280], 80, 600),
    (0, &[160, 200], 0, 240),
    (1, &[400, 320, 240, 560, 200, 300], 80, 320),
    (2, &[150, 300], 0, 240),
    (0, &[300, 400, 240, 200, 480], 160, 800),
    (1, &[200], 0, 240),
];
const BLOCKS: usize = 8;

/// Per-utterance variation of the shift, in units of the jitter.
const JITTER: &[i64] = &[0, 1, -1, 1, 0, -1, -1, 1, 0];

/// The block-repeated meeting as one segment per utterance, words named
/// `b<block>t<turn>w<word>`, every time moved by `shift_ms` (plus or minus
/// `jitter_ms` per utterance), speaker `i` labelled `labels[i]`.
fn turns_transcript(labels: [&str; 3], shift_ms: i64, jitter_ms: i64) -> NewTranscript {
    let mut t = NewTranscript::default();
    for l in labels {
        t.speakers.push(speaker(Channel::Loopback, l));
    }
    let mut at = 1_000;
    for b in 0..BLOCKS {
        for (u, (spk, durs, gap, pause)) in TURNS.iter().enumerate() {
            let seg = t.segments.len();
            let start = at;
            let shift_ms = shift_ms + JITTER[seg % JITTER.len()] * jitter_ms;
            let mut texts = Vec::new();
            for (k, d) in durs.iter().enumerate() {
                if k > 0 {
                    at += gap;
                }
                let text = format!("b{b}t{u}w{k}");
                t.words.push(NewWord {
                    segment: seg,
                    start_ms: at + shift_ms,
                    end_ms: at + d + shift_ms,
                    text: text.clone(),
                    confidence: None,
                    raw_label: Some(labels[*spk].to_owned()),
                    margin: None,
                    overlap: false,
                    raw_text: None,
                });
                texts.push(text);
                at += d;
            }
            t.segments.push(NewSegment {
                speaker: *spk,
                start_ms: start + shift_ms,
                end_ms: at + shift_ms,
                text: texts.join(" "),
            });
            at += pause;
        }
    }
    t
}

/// The post-pass after Stop runs the file lookahead, whose word starts come
/// 80 to 190 ms earlier than the live run's (docs/research/12, "Word times").
/// Names given during the meeting (from a one-word segment, from a live
/// utterance range, from the legend) and reassigned utterances (one and two
/// short words) must land on the same words.
fn carry_over_survives_earlier_post_pass_words(store: &dyn RunStore) {
    for (shift, jitter) in [(0, 0), (-160, 0), (-200, 0), (-160, 40), (-200, 40)] {
        let m = live_meeting(store);
        let live = run_with(
            store,
            m,
            RunKind::Live,
            &turns_transcript(["spk_0", "spk_1", "spk_2"], 0, 0),
        );
        store.set_current_run(m, live).unwrap();
        let seg_of = |b: usize, u: usize| -> SegmentId {
            let first = format!("b{b}t{u}w0");
            let w = store
                .get_words(m, 0, i64::MAX)
                .unwrap()
                .into_iter()
                .find(|w| w.text == first)
                .unwrap();
            w.segment_id
        };
        let (a, b, c) = (
            by_label(store, live, "spk_0").id,
            by_label(store, live, "spk_1").id,
            by_label(store, live, "spk_2").id,
        );
        store.rename_speaker(a, "Priya", None).unwrap();
        // Marcus from his one-word (200 ms) segment.
        store
            .rename_speaker(b, "Marcus", Some(seg_of(3, 7)))
            .unwrap();
        // Lee from a live utterance range: 150 + 300 ms.
        let lee = segments(store, m)
            .into_iter()
            .find(|s| s.id == seg_of(5, 5))
            .unwrap();
        store
            .rename_speaker_at(c, "Lee", lee.start_ms, lee.end_ms)
            .unwrap();
        // Short utterances given to someone else.
        store.reassign_segment(seg_of(6, 3), b).unwrap(); // A's 160 + 200 ms -> Marcus
        store.reassign_segment(seg_of(1, 7), c).unwrap(); // B's 200 ms -> Lee
        store.reassign_segment(seg_of(2, 5), a).unwrap(); // C's 150 + 300 ms -> Priya

        let new = run_with(
            store,
            m,
            RunKind::PostPass,
            &turns_transcript(["spk_2", "spk_0", "spk_1"], shift, jitter),
        );
        let report = store.carry_over(live, new).unwrap();
        store.set_current_run(m, new).unwrap();
        let names: std::collections::HashMap<SegmentId, String> = segments(store, m)
            .into_iter()
            .map(|s| (s.id, s.speaker_name))
            .collect();
        let mut wrong = Vec::new();
        let words = store.get_words(m, i64::MIN, i64::MAX).unwrap();
        for w in &words {
            let (bl, rest) = w.text[1..].split_once('t').unwrap();
            let turn: usize = rest.split_once('w').unwrap().0.parse().unwrap();
            let bl: usize = bl.parse().unwrap();
            let want = match (bl, turn) {
                (6, 3) => "Marcus",
                (1, 7) => "Lee",
                (2, 5) => "Priya",
                _ => ["Priya", "Marcus", "Lee"][TURNS[turn].0],
            };
            if names[&w.segment_id] != want {
                wrong.push(format!("{} {} (want {want})", w.text, names[&w.segment_id]));
            }
        }
        assert_eq!(words.len(), BLOCKS * 33, "shift {shift} jitter {jitter}");
        assert!(
            wrong.is_empty(),
            "shift {shift} jitter {jitter} ms: {} of {} words on the wrong speaker: {wrong:?}; {report:?}",
            wrong.len(),
            words.len()
        );
        assert_eq!(
            report.matched.len(),
            3,
            "shift {shift} jitter {jitter}: {report:?}"
        );
        assert_eq!(
            report.edits_replayed, 3,
            "shift {shift} jitter {jitter}: {report:?}"
        );
        assert_eq!(
            report.segments_split, 0,
            "shift {shift} jitter {jitter}: {report:?}"
        );
        assert!(
            report.needs_review.is_empty(),
            "shift {shift} jitter {jitter}: {report:?}"
        );
        assert!(
            report.created_speakers.is_empty(),
            "shift {shift} jitter {jitter}"
        );
    }
}

/// Deterministic xorshift for the engine-like fixtures.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

/// Utterances as the engines emit them: `(speaker index, [(start, end)])`,
/// mostly 80 ms words (one encoder frame) with 80 to 320 ms gaps, 240 ms
/// between turns.
fn spike_turns(order: &[usize], seed: u64) -> Vec<(usize, Vec<(i64, i64)>)> {
    let mut rng = Rng(seed);
    let mut at = 1_000;
    let mut out = Vec::new();
    for &spk in order {
        let n = 3 + rng.next() % 10;
        let mut words = Vec::new();
        for k in 0..n {
            if k > 0 {
                at += rng.pick(&[80, 160, 240, 320]);
            }
            let d = rng.pick(&[80, 80, 80, 80, 160, 240]);
            words.push((at, at + d));
            at += d;
        }
        out.push((spk, words));
        at += 240;
    }
    out
}

/// One segment per utterance on the loopback; speaker `i` is `labels[i]`.
/// Each word moves by `shift_ms` plus a per-word offset drawn from
/// `spread` (sorted again within the utterance).
fn spike_transcript(
    turns: &[(usize, Vec<(i64, i64)>)],
    labels: &[&str],
    shift_ms: i64,
    spread: &[i64],
    seed: u64,
) -> NewTranscript {
    let mut rng = Rng(seed);
    let mut t = NewTranscript::default();
    for l in labels {
        t.speakers.push(speaker(Channel::Loopback, l));
    }
    for (u, (spk, words)) in turns.iter().enumerate() {
        let mut ws: Vec<(i64, i64)> = words
            .iter()
            .map(|&(a, b)| {
                let off = shift_ms
                    + if spread.is_empty() {
                        0
                    } else {
                        rng.pick(spread)
                    };
                ((a + off).max(0), (b + off).max(0))
            })
            .collect();
        ws.sort();
        let seg = t.segments.len();
        t.segments.push(NewSegment {
            speaker: *spk,
            start_ms: ws[0].0,
            end_ms: ws.iter().map(|w| w.1).max().unwrap(),
            text: format!("u{u}"),
        });
        for (k, (a, b)) in ws.into_iter().enumerate() {
            t.words.push(NewWord {
                segment: seg,
                start_ms: a,
                end_ms: b,
                text: format!("u{u}w{k}"),
                confidence: None,
                raw_label: Some(labels[*spk].to_owned()),
                margin: None,
                overlap: false,
                raw_text: None,
            });
        }
    }
    t
}

/// Post-pass word offsets against the live run as measured on real
/// recordings (right context 13 against 1): a median 160 ms earlier, spread
/// 400 ms earlier to 0 (p10 / p90), about two thirds within one 80 ms frame
/// of the median. As offsets around the -160 ms shift.
const SPIKE_SPREAD: &[i64] = &[-240, -160, -80, -80, 0, 0, 0, 0, 80, 80, 160, 240];

/// Live and post-pass words of three speakers are 80 ms spikes and the
/// post-pass places each one a little differently (`SPIKE_SPREAD`): the
/// spikes overlap too little for word overlap to match anyone, so names
/// given from the legend used to be lost. Speech regions match.
fn carry_over_survives_engine_word_spikes(store: &dyn RunStore) {
    let order: Vec<usize> = (0..60).map(|i| [0, 1, 2, 0, 2, 1][i % 6]).collect();
    let turns = spike_turns(&order, 0x5EED);
    let m = live_meeting(store);
    let live = run_with(
        store,
        m,
        RunKind::Live,
        &spike_transcript(&turns, &["spk_0", "spk_1", "spk_2"], 0, &[], 1),
    );
    store.set_current_run(m, live).unwrap();
    let (a, b) = (
        by_label(store, live, "spk_0").id,
        by_label(store, live, "spk_1").id,
    );
    // Priya from the legend, Marcus from one of his live lines.
    store.rename_speaker(a, "Priya", None).unwrap();
    let line = &turns.iter().filter(|t| t.0 == 1).nth(4).unwrap().1;
    store
        .rename_speaker_at(b, "Marcus", line[0].0, line[line.len() - 1].1)
        .unwrap();
    // The post-pass labels them differently.
    let new = run_with(
        store,
        m,
        RunKind::PostPass,
        &spike_transcript(&turns, &["spk_2", "spk_0", "spk_1"], -160, SPIKE_SPREAD, 2),
    );
    let report = store.carry_over(live, new).unwrap();
    let name = |label: &str| by_label(store, new, label).display_name;
    assert_eq!(
        report.shift_ms.get(&Channel::Loopback),
        Some(&-160),
        "{report:?}"
    );
    assert_eq!(name("spk_2").as_deref(), Some("Priya"), "{report:?}");
    assert_eq!(name("spk_0").as_deref(), Some("Marcus"), "{report:?}");
    assert_eq!(name("spk_1"), None, "{report:?}");
    assert_eq!(report.matched.len(), 3, "{report:?}");
    assert!(report.needs_review.is_empty(), "{report:?}");
}

/// Two speakers alternating in quick turns (240 ms apart, some one-word
/// interjections), post-pass labels swapped and word times spread as in
/// `SPIKE_SPREAD`: each live speaker matches its own post-pass speaker,
/// never the neighbour whose regions touch its own.
fn carry_over_keeps_alternating_speakers_apart(store: &dyn RunStore) {
    let order: Vec<usize> = (0..80).map(|i| i % 2).collect();
    let mut turns = spike_turns(&order, 0xA17E);
    // Every fifth turn of B is a one-word interjection.
    for (i, t) in turns.iter_mut().enumerate() {
        if t.0 == 1 && i % 10 == 1 {
            t.1.truncate(1);
        }
    }
    let m = live_meeting(store);
    let live = run_with(
        store,
        m,
        RunKind::Live,
        &spike_transcript(&turns, &["spk_0", "spk_1"], 0, &[], 3),
    );
    store.set_current_run(m, live).unwrap();
    let (a, b) = (
        by_label(store, live, "spk_0").id,
        by_label(store, live, "spk_1").id,
    );
    store.rename_speaker(a, "Ana", None).unwrap();
    store.rename_speaker(b, "Ben", None).unwrap();
    let new = run_with(
        store,
        m,
        RunKind::PostPass,
        &spike_transcript(&turns, &["spk_1", "spk_0"], -160, SPIKE_SPREAD, 4),
    );
    let report = store.carry_over(live, new).unwrap();
    let (na, nb) = (by_label(store, new, "spk_1"), by_label(store, new, "spk_0"));
    let mut matched = report.matched.clone();
    matched.sort();
    let mut want = vec![(a, na.id), (b, nb.id)];
    want.sort();
    assert_eq!(matched, want, "{report:?}");
    assert_eq!(na.display_name.as_deref(), Some("Ana"));
    assert_eq!(nb.display_name.as_deref(), Some("Ben"));
    assert!(report.needs_review.is_empty(), "{report:?}");
}

/// Post-pass word offsets against the live run, absolute, as measured on two
/// recorded meetings: median -160 ms, p10 -400 ms, p90 0, about 60 percent
/// within one 80 ms frame of the median.
const MEASURED_OFFSETS: &[i64] = &[
    -480, -400, -400, -320, -320, -240, -240, -240, -160, -160, -160, -160, -160, -160, -160, -160,
    -80, -80, -80, 0, 0, 80,
];

/// A transcript on the loopback from utterances (`turns`, live times), each
/// word labelled by `label(turn, word)`; segments are runs of one label
/// within an utterance. With `offsets`, every word moves by a value drawn
/// from it (then the utterance is sorted again).
fn labelled_transcript(
    turns: &[Vec<(i64, i64)>],
    label: &dyn Fn(usize, usize) -> String,
    offsets: Option<(&[i64], u64)>,
) -> NewTranscript {
    let mut rng = Rng(offsets.map_or(1, |o| o.1));
    let mut t = NewTranscript::default();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (u, words) in turns.iter().enumerate() {
        let mut ws: Vec<(i64, i64, String)> = words
            .iter()
            .enumerate()
            .map(|(k, &(a, b))| {
                let off = offsets.map_or(0, |o| rng.pick(o.0));
                ((a + off).max(0), (b + off).max(0), label(u, k))
            })
            .collect();
        ws.sort();
        for (k, (a, b, l)) in ws.iter().enumerate() {
            let spk = *index.entry(l.clone()).or_insert_with(|| {
                t.speakers.push(speaker(Channel::Loopback, l));
                t.speakers.len() - 1
            });
            if k == 0 || ws[k - 1].2 != *l {
                t.segments.push(NewSegment {
                    speaker: spk,
                    start_ms: *a,
                    end_ms: *b,
                    text: format!("u{u}"),
                });
            }
            let seg = t.segments.len() - 1;
            t.segments[seg].end_ms = t.segments[seg].end_ms.max(*b);
            t.words.push(NewWord {
                segment: seg,
                start_ms: *a,
                end_ms: *b,
                text: format!("u{u}w{k}"),
                confidence: None,
                raw_label: Some(l.clone()),
                margin: None,
                overlap: false,
                raw_text: None,
            });
        }
    }
    t
}

/// 200 seeded meetings with engine-like words (80 ms spikes, 80 to 320 ms
/// gaps, 240 ms between turns, post-pass offsets drawn from
/// `MEASURED_OFFSETS`): live A, B (with one-word backchannels), C, E (two
/// short turns, under the minimum) and `spk_unknown` words at some turn
/// ends; the post-pass splits C into C1 (two thirds) and C2, and labels one
/// word in the middle of A's longest turn as a new speaker T. Ann (A) and
/// Eve (E) are named from the legend, Bob (B) from a live line. Every seed:
/// exactly A-A', B-B', C-C1 match; Ann, Bob and Eve (through the fallback)
/// land on A', B', E' and no name anywhere else. A control meeting of two
/// balanced speakers whose post-pass labels swap halfway matches nobody.
fn carry_over_seeded_engine_like_meetings(store: &dyn RunStore) {
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let base = [0usize, 1, 0, 2, 1, 0, 2, 1];
        let mut order: Vec<usize> = (0..56).map(|i| base[i % base.len()]).collect();
        order[11] = 3;
        order[37] = 3;
        let mut turns: Vec<Vec<(i64, i64)>> = Vec::new();
        let mut at = 1_000;
        for &spk in &order {
            let backchannel = spk == 1 && rng.next().is_multiple_of(4);
            let n = if backchannel { 1 } else { 3 + rng.next() % 10 };
            let mut ws = Vec::new();
            for k in 0..n {
                if k > 0 {
                    at += rng.pick(&[80, 160, 240, 320]);
                }
                let d = rng.pick(&[80, 80, 80, 80, 160, 240]);
                ws.push((at, at + d));
                at += d;
            }
            turns.push(ws);
            at += 240;
        }
        // C1: C's turns up to two thirds of C's talk time.
        let span = |t: usize| turns[t].last().unwrap().1 - turns[t][0].0;
        let c_turns: Vec<usize> = (0..order.len()).filter(|&t| order[t] == 2).collect();
        let c_total: i64 = c_turns.iter().map(|&t| span(t)).sum();
        let mut c1 = std::collections::HashSet::new();
        let mut c_sum = 0;
        for &t in &c_turns {
            if 3 * c_sum >= 2 * c_total {
                break;
            }
            c_sum += span(t);
            c1.insert(t);
        }
        let a_long = (0..order.len())
            .filter(|&t| order[t] == 0)
            .max_by_key(|&t| (turns[t].len(), std::cmp::Reverse(t)))
            .unwrap();
        let live_labels = ["spk_0", "spk_1", "spk_2", "spk_3"];
        let live_label = |t: usize, k: usize| -> String {
            let n = turns[t].len();
            if order[t] != 3 && t.is_multiple_of(7) && n > 3 && k == n - 1 {
                "spk_unknown".into()
            } else {
                live_labels[order[t]].into()
            }
        };
        // Post-pass labels: A' spk_0, B' spk_1, C1 spk_2, C2 spk_4, E' spk_3, T spk_5.
        let pp_label = |t: usize, k: usize| -> String {
            match order[t] {
                0 if t == a_long && k == turns[t].len() / 2 => "spk_5".into(),
                2 if !c1.contains(&t) => "spk_4".into(),
                s => live_labels[s].into(),
            }
        };
        let m = live_meeting(store);
        let live = run_with(
            store,
            m,
            RunKind::Live,
            &labelled_transcript(&turns, &live_label, None),
        );
        store.set_current_run(m, live).unwrap();
        let old = |l: &str| by_label(store, live, l).id;
        store.rename_speaker(old("spk_0"), "Ann", None).unwrap();
        store.rename_speaker(old("spk_3"), "Eve", None).unwrap();
        let line = (0..order.len())
            .filter(|&t| order[t] == 1 && turns[t].len() > 2)
            .nth(2)
            .unwrap();
        let (l0, l1) = (turns[line][0].0, turns[line].last().unwrap().1);
        store
            .rename_speaker_at(old("spk_1"), "Bob", l0, l1)
            .unwrap();
        let new = run_with(
            store,
            m,
            RunKind::PostPass,
            &labelled_transcript(&turns, &pp_label, Some((MEASURED_OFFSETS, seed + 1_000))),
        );
        let report = store.carry_over(live, new).unwrap();
        let newl = |l: &str| by_label(store, new, l);
        let mut matched = report.matched.clone();
        matched.sort();
        let mut want = vec![
            (old("spk_0"), newl("spk_0").id),
            (old("spk_1"), newl("spk_1").id),
            (old("spk_2"), newl("spk_2").id),
        ];
        want.sort();
        assert_eq!(matched, want, "seed {seed}: {report:?}");
        let names: Vec<(String, Option<String>, bool)> = store
            .run_speakers(new)
            .unwrap()
            .into_iter()
            .map(|s| (s.engine_label, s.display_name, s.needs_review))
            .collect();
        for (label, name, review) in &names {
            let want = match label.as_str() {
                "spk_0" => Some("Ann"),
                "spk_1" => Some("Bob"),
                "spk_3" => Some("Eve"),
                _ => None,
            };
            assert_eq!(name.as_deref(), want, "seed {seed} {label}: {names:?}");
            // Matched speakers are settled; the fallback's receiver and the
            // unmatched C2 and T are flagged.
            let flagged = matches!(label.as_str(), "spk_3" | "spk_4" | "spk_5");
            assert_eq!(*review, flagged, "seed {seed} {label}: {names:?}");
        }

        // Control: two balanced speakers, post-pass labels swapped halfway.
        let mut cturns: Vec<Vec<(i64, i64)>> = Vec::new();
        let mut at = 1_000;
        for _ in 0..40 {
            let n = 3 + rng.next() % 10;
            let mut ws = Vec::new();
            for k in 0..n {
                if k > 0 {
                    at += rng.pick(&[80, 160, 240, 320]);
                }
                let d = rng.pick(&[80, 80, 80, 80, 160, 240]);
                ws.push((at, at + d));
                at += d;
            }
            cturns.push(ws);
            at += 240;
        }
        let half = cturns.len() / 2;
        let cm = live_meeting(store);
        let clive = run_with(
            store,
            cm,
            RunKind::Live,
            &labelled_transcript(&cturns, &|t, _| format!("spk_{}", t % 2), None),
        );
        let cnew = run_with(
            store,
            cm,
            RunKind::PostPass,
            &labelled_transcript(
                &cturns,
                &|t, _| format!("spk_{}", (t + usize::from(t >= half)) % 2),
                Some((MEASURED_OFFSETS, seed + 2_000)),
            ),
        );
        let creport = store.carry_over(clive, cnew).unwrap();
        assert!(
            creport.matched.is_empty(),
            "control seed {seed}: {creport:?}"
        );
    }
}

/// A rename without an anchor keeps the anchor an earlier rename set; a
/// blank name clears both.
fn rename_without_anchor_keeps_the_anchor(store: &dyn RunStore) {
    let m = live_meeting(store);
    let live = store
        .create_run(m, RunKind::Live, serde_json::json!({}))
        .unwrap()
        .id;
    let s = store
        .live_speaker(live, Channel::Loopback, "spk_0")
        .unwrap();
    let anchor = |id| {
        let s = store
            .run_speakers(live)
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        (s.display_name, s.name_anchor_start_ms, s.name_anchor_end_ms)
    };
    store.rename_speaker_at(s.id, "Dana", 1_000, 2_500).unwrap();
    store.rename_speaker(s.id, "Dana K.", None).unwrap();
    assert_eq!(
        anchor(s.id),
        (Some("Dana K.".into()), Some(1_000), Some(2_500))
    );
    store
        .rename_speaker_at(s.id, "Dana K.", 4_000, 4_400)
        .unwrap();
    assert_eq!(
        anchor(s.id),
        (Some("Dana K.".into()), Some(4_000), Some(4_400))
    );
    store.rename_speaker(s.id, "  ", None).unwrap();
    assert_eq!(anchor(s.id), (None, None, None));
    store.rename_speaker(s.id, "Dana", None).unwrap();
    assert_eq!(anchor(s.id), (Some("Dana".into()), None, None));
}

fn carry_over_rejects_bad_runs(store: &dyn RunStore) {
    let m1 = live_meeting(store);
    let m2 = live_meeting(store);
    let t = carry_transcript(&[("spk_0", &[(0, 10)])]);
    let r1 = run_with(store, m1, RunKind::Live, &t);
    let r2 = run_with(store, m2, RunKind::PostPass, &t);
    assert!(is_invalid(store.carry_over(r1, r2)));
    assert!(is_invalid(store.carry_over(r1, r1)));
    assert!(is_not_found(store.carry_over(r1, RunId(424_242))));
}

/// `words.raw_text` (plan 16 section 3.5): stored where set, `None`
/// elsewhere, kept through a segment split.
fn raw_text_round_trip(store: &dyn Store) {
    let m = store.create_meeting(new_meeting("Raw", T0)).unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({}))
        .unwrap();
    let mut t = sample_transcript();
    // "Hello" was "hello", "team" was "team" (unchanged), "today." was "today".
    t.words[0].raw_text = Some("hello".into());
    t.words[8].text = "today.".into();
    t.words[8].raw_text = Some("today".into());
    store.insert_transcript(run.id, &t).unwrap();
    store.set_current_run(m.id, run.id).unwrap();
    let words = store.get_words(m.id, i64::MIN, i64::MAX).unwrap();
    assert_eq!(words.len(), 9);
    assert_eq!(words[0].raw_text.as_deref(), Some("hello"));
    assert_eq!(words[0].asr_text(), "hello");
    assert_eq!(words[1].raw_text, None);
    assert_eq!(words[1].asr_text(), "budget");
    assert_eq!(
        (words[8].text.as_str(), words[8].raw_text.as_deref()),
        ("today.", Some("today"))
    );
    // A split moves words, never their raw text.
    let seg = segments(store, m.id)[2].id;
    store.split_segment(seg, words[7].id).unwrap();
    let after = store.get_words(m.id, i64::MIN, i64::MAX).unwrap();
    assert_eq!(after[8].raw_text.as_deref(), Some("today"));
    assert_eq!(after[0].raw_text.as_deref(), Some("hello"));
    // Serialized only where set.
    let j = serde_json::to_value(&after[1]).unwrap();
    assert!(j.get("raw_text").is_none(), "{j}");
    assert_eq!(
        serde_json::to_value(&after[0]).unwrap()["raw_text"],
        "hello"
    );
}

/// `carry_over_identity` (plan 16 section 3.9 step 6): a run rebuilt from
/// another's words, each under its own speaker row and with every row
/// present, gets every speaker's name, colour, global id, merge, review flag
/// and anchor by `(channel, engine_label)`, however little the speaker said.
/// No `reassign` edit is replayed: the rebuild already put every word where
/// the edits had (a replay by display name would move them again after a
/// rename).
fn carry_over_identity_by_label(store: &dyn RunStore) {
    let m = live_meeting(store);
    let spec: &[(&str, &[(i64, i64)])] = &[
        ("spk_0", &[(0, 20)]),
        ("spk_1", &[(20, 30), (30, 40)]),
        ("spk_2", &[(40, 43)]),
        ("spk_3", &[(43, 50), (50, 60)]),
    ];
    let mut t = carry_transcript(spec);
    t.speakers[3].needs_review = true;
    let old = run_with(store, m, RunKind::PostPass, &t);
    store.set_current_run(m, old).unwrap();
    let a = by_label(store, old, "spk_0");
    let b = by_label(store, old, "spk_1");
    let c = by_label(store, old, "spk_2");
    store.rename_speaker(a.id, "Priya", None).unwrap();
    store.set_speaker_color(a.id, "#123456").unwrap();
    store
        .rename_speaker(b.id, "Marcus", Some(segment_at(store, m, 30_000)))
        .unwrap();
    // A three-second speaker keeps its name (the time-overlap carry would
    // not match it), and is merged into Marcus.
    store.rename_speaker(c.id, "Ghost", None).unwrap();
    store.merge_speakers(c.id, b.id).unwrap();
    store
        .reassign_segment(segment_at(store, m, 50_000), a.id)
        .unwrap();

    let new = run_with(store, m, RunKind::PostPass, &carry_transcript(spec));
    let report = store.carry_over_identity(old, new).unwrap();
    let x = by_label(store, new, "spk_0");
    let y = by_label(store, new, "spk_1");
    let z = by_label(store, new, "spk_2");
    let w = by_label(store, new, "spk_3");
    assert_eq!(report.matched.len(), 4);
    assert!(report.matched.contains(&(c.id, z.id)));
    assert_eq!(
        (x.display_name.as_deref(), x.color.as_deref()),
        (Some("Priya"), Some("#123456"))
    );
    assert_eq!(y.display_name.as_deref(), Some("Marcus"));
    assert_eq!(
        (y.name_anchor_start_ms, y.name_anchor_end_ms),
        (Some(30_000), Some(39_800))
    );
    assert_eq!(z.display_name.as_deref(), Some("Ghost"));
    assert_eq!(z.merged_into, Some(y.id));
    assert!(!x.needs_review && !y.needs_review && !z.needs_review);
    assert!(w.needs_review, "the review flag comes along");
    assert_eq!(report.needs_review, vec![w.id]);
    assert!(report.shift_ms.values().all(|&s| s == 0));
    // Every field of every row, global id included, ids mapped.
    let old_rows = store.run_speakers(old).unwrap();
    let new_rows = store.run_speakers(new).unwrap();
    assert_eq!(old_rows.len(), new_rows.len());
    let label_of = |rows: &[Speaker], id: Option<SpeakerId>| {
        id.map(|id| {
            rows.iter()
                .find(|s| s.id == id)
                .unwrap()
                .engine_label
                .clone()
        })
    };
    for o in &old_rows {
        let n = new_rows
            .iter()
            .find(|n| (n.channel, &n.engine_label) == (o.channel, &o.engine_label))
            .unwrap();
        assert_eq!(
            (
                &n.display_name,
                &n.color,
                n.global_id,
                n.needs_review,
                n.name_anchor_start_ms,
                n.name_anchor_end_ms,
                label_of(&new_rows, n.merged_into)
            ),
            (
                &o.display_name,
                &o.color,
                o.global_id,
                o.needs_review,
                o.name_anchor_start_ms,
                o.name_anchor_end_ms,
                label_of(&old_rows, o.merged_into)
            ),
            "{}",
            o.engine_label
        );
    }
    // No edit replays: the new run's words stay where they were built.
    assert_eq!((report.edits_replayed, report.segments_split), (0, 0));
    assert!(report.created_speakers.is_empty());
    store.set_current_run(m, new).unwrap();
    let tail: Vec<(i64, String)> = segments(store, m)
        .into_iter()
        .filter(|s| s.start_ms >= 43_000)
        .map(|s| (s.start_ms, s.speaker_name))
        .collect();
    assert_eq!(
        tail,
        [
            (43_000, "Speaker 4".to_owned()),
            (50_000, "Speaker 4".to_owned())
        ]
    );
    // Bad runs.
    assert!(is_invalid(store.carry_over_identity(new, new)));
    assert!(is_not_found(store.carry_over_identity(old, RunId(424_242))));
    let other = live_meeting(store);
    let elsewhere = run_with(store, other, RunKind::PostPass, &carry_transcript(spec));
    assert!(is_invalid(store.carry_over_identity(old, elsewhere)));
}

/// `run_segments` and `run_words` read any run, current or not, with each
/// segment's own speaker row (not the merge root the views show).
fn run_segments_and_words(store: &dyn RunStore) {
    let (m, first) = seeded_meeting(store, "Runs", T0);
    let mut t = sample_transcript();
    t.words[2].margin = Some(0.5);
    t.words[3].overlap = true;
    t.words[0].raw_text = Some("hello".into());
    let second = run_with(store, m, RunKind::PostPass, &t);
    // A merge on the first run: the views show the root, run_segments the row.
    let spk0 = by_label(store, first, "spk_0");
    let spk1 = by_label(store, first, "spk_1");
    store.merge_speakers(spk1.id, spk0.id).unwrap();
    let shown = segments(store, m);
    assert_eq!(shown[2].speaker_id, spk0.id);
    let segs = store.run_segments(first).unwrap();
    assert_eq!(segs.len(), 3);
    assert!(segs.iter().all(|s| s.run_id == first && s.meeting_id == m));
    assert_eq!(
        segs.iter().map(|s| s.seq).collect::<Vec<_>>(),
        [0, 1, 2],
        "seq order"
    );
    assert_eq!(segs[2].speaker_id, spk1.id, "its own row");
    assert_eq!(segs[0].text, "Hello budget team");
    let words = store.run_words(first).unwrap();
    assert_eq!(words.len(), 9);
    assert!(words.windows(2).all(|p| p[0].start_ms <= p[1].start_ms));
    let own: std::collections::HashSet<SegmentId> = segs.iter().map(|s| s.id).collect();
    assert!(words.iter().all(|w| own.contains(&w.segment_id)));
    // The run that is not current, with its fusion fields and raw text.
    let other = store.run_words(second).unwrap();
    assert_eq!(other.len(), 9);
    assert!(other.iter().all(|w| !own.contains(&w.segment_id)));
    assert_eq!(other[2].margin, Some(0.5));
    assert!(other[3].overlap);
    assert_eq!(other[0].raw_text.as_deref(), Some("hello"));
    assert_eq!(store.run_segments(second).unwrap().len(), 3);
    // An empty run, and one that does not exist.
    let empty = store
        .create_run(m, RunKind::File, serde_json::json!({}))
        .unwrap()
        .id;
    assert!(store.run_segments(empty).unwrap().is_empty());
    assert!(store.run_words(empty).unwrap().is_empty());
    assert!(is_not_found(store.run_segments(RunId(424_242))));
    assert!(is_not_found(store.run_words(RunId(424_242))));
}

/// `set_current_run_if`: the compare-and-set a rebuild ends with.
fn set_current_run_if_compares(store: &dyn RunStore) {
    let (m, first) = seeded_meeting(store, "CAS", T0);
    let second = run_with(store, m, RunKind::PostPass, &sample_transcript());
    let third = run_with(store, m, RunKind::PostPass, &sample_transcript());
    let current = || store.get_meeting(m).unwrap().current_run.unwrap().id;
    // Still the run expected: switched.
    assert!(store.set_current_run_if(m, first, second).unwrap());
    assert_eq!(current(), second);
    // Not any more: nothing changes.
    assert!(!store.set_current_run_if(m, first, third).unwrap());
    assert_eq!(current(), second);
    // A meeting that records or is being finalized is never switched.
    for status in [MeetingStatus::Processing, MeetingStatus::Recording] {
        store.set_meeting_status(m, status).unwrap();
        assert!(!store.set_current_run_if(m, second, third).unwrap());
        assert_eq!(current(), second);
    }
    store.set_meeting_status(m, MeetingStatus::Failed).unwrap();
    assert!(store.set_current_run_if(m, second, third).unwrap());
    assert_eq!(current(), third);
    // A meeting without a current run expects nothing.
    store.delete_run(third).unwrap();
    assert_eq!(store.get_meeting(m).unwrap().current_run, None);
    assert!(!store.set_current_run_if(m, second, first).unwrap());
    // Bad arguments.
    let other = store.create_meeting(new_meeting("Other", T0)).unwrap().id;
    assert!(is_invalid(store.set_current_run_if(other, first, second)));
    assert!(is_not_found(store.set_current_run_if(
        m,
        first,
        RunId(424_242)
    )));
}
