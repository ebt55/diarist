//! Conformance suite for `meet_core::store::Store` implementations.
//!
//! Every case is a function over `&dyn Store` that starts from an empty store
//! and creates its own data. `MemoryStore` and `SqliteStore` (an in-memory
//! SQLite database) both run it below.
//!
//! Cases assert behaviour the UI relies on, not implementation details: no
//! assumptions about id values, search ranking or snippet window size.

use meet_core::store::{
    MemoryStore, NewAudioTrack, NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord,
    ScannedModelFile, SqliteStore, Store, StoreError,
};
use meet_core::types::{
    AudioFormat, Channel, MeetingId, MeetingStatus, ModelCapabilities, ModelRole, ModelSource,
    RunId, RunKind, RunStatus, SegmentId, Source, SpeakerId, TrackChannel,
};
use meet_core::views::{default_speaker_color, SegmentView};

type Case = fn(&dyn Store);

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
];

/// Runs every case against a fresh store from `make`.
fn run_all(make: impl Fn() -> Box<dyn Store>) {
    for (name, case) in CASES {
        let store = make();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(&*store)));
        if let Err(e) = result {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panic!("conformance case `{name}` failed: {msg}");
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
        })
        .unwrap();
    let tracks = store.list_audio_tracks(id).unwrap();
    assert_eq!(tracks, vec![track]);
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
