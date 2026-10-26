//! Replays a recorded meeting through the real live and post-pass paths to
//! check what carry-over makes of names given while recording. `#[ignore]`d
//! and gated on `REPRO_FLAC`, `DIARIST_ENGINE_DIR` (an extracted runtime)
//! and `DIARIST_MODELS_DIR` (EN model and Nemotron 3 Diarization); see
//! docs/DEV.md, "Gated tests".
//!
//! The stereo 16 kHz FLAC (left mic, right loopback: a live meeting's
//! `playback.flac` holds what the engines saw) is streamed in real time
//! through `nemo-speech serve` (the app's `ServeConfig`, one session per
//! channel, the feed's dither on silent frames). Every utterance revision
//! is stored as `live::EventLoop::on_feed` stores it (word times through
//! the clock map, `live_speaker` rows), and the live lines are split as the
//! live view splits them. At `REPRO_RENAME_AT` of the audio the loopback
//! label with the most final lines is renamed from the legend
//! (`rename_speaker`, no anchor) and the second one from its newest final
//! line (`rename_speaker_at`). Then the live run is materialised and the real
//! `run_post_pass` runs (EN, right context 13, `v3-offline`,
//! `keep_live_runs = true`, so the scratch database keeps both runs). The
//! test prints the `CarryOverReport`, both runs' speakers and the post-pass
//! words' count and checksum of `(start, end, speaker label)`, and asserts
//! that every name given during the replay reached a post-pass speaker.
//!
//! Env:
//! - `REPRO_FLAC`: the recording.
//! - `REPRO_OUT`: the output folder (`repro.db`, `replay.log`, `serve.log`);
//!   default `diarist-replay` in the temp folder.
//! - `REPRO_SRC_DB` and `REPRO_MEETING` (optional): the database and meeting
//!   the FLAC came from, opened read-only for the meeting's clock anchors.
//!   Without them the channels use the nominal clock.
//! - `REPRO_SPEED` (default 1: real time), `REPRO_SECONDS` (only the first
//!   seconds), `REPRO_RENAME_AT` (fraction of the audio, default 0.5).
//! - `REPRO_RESUME=1`: reuse `REPRO_OUT`'s database: finish a replay whose
//!   stream stopped early, or run the post-pass and carry-over again on the
//!   stored live run (earlier post-pass runs are deleted after their word
//!   checksum is printed).
//!
//! `cargo test -p diarist-core --test live_carry_repro -- --ignored --nocapture`

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use diarist_core::audio::clock::{channel_anchors, origin_from_anchors, ClockMap};
use diarist_core::audio::feed::dither_silence;
use diarist_core::audio::flac::decode_i16;
use diarist_core::engine::{
    EngineEvent, EngineWord, LiveEngine, LiveOpts, LiveSession, NemoCliEngine, NemoWsLiveEngine,
    ServeConfig, ServeSupervisor,
};
use diarist_core::live::{run_post_pass, PostPassJob};
use diarist_core::pipeline::CancelToken;
use diarist_core::store::{NewMeeting, RunStore, SqliteStore, Store, StoreError};
use diarist_core::transcript::SegmentOptions;
use diarist_core::types::{
    Channel, ClockAnchor, JobId, MeetingId, MeetingStatus, RunId, RunKind, RunStatus, Source,
    SpeakerId,
};

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

fn read_only(db: &Path) -> rusqlite::Connection {
    let uri = format!(
        "file:{}?mode=ro",
        db.display().to_string().replace('\\', "/")
    );
    rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

/// `(channel, sample index, qpc)` of a meeting's clock anchors.
fn source_anchors(db: &Path, meeting: i64) -> Vec<(String, i64, i64)> {
    let c = read_only(db);
    let mut st = c
        .prepare("SELECT channel, sample_index, qpc_100ns FROM clock_anchors WHERE meeting_id = ?1")
        .unwrap();
    let rows = st
        .query_map([meeting], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

/// Word count and FNV-1a checksum of `(start, end, speaker label)` of a
/// run's words in time order.
fn words_checksum(db: &Path, run: RunId) -> (usize, u64) {
    let c = read_only(db);
    let mut st = c
        .prepare(
            "SELECT w.start_ms, w.end_ms, sp.engine_label FROM words w
             JOIN segments s ON s.id = w.segment_id JOIN speakers sp ON sp.id = s.speaker_id
             WHERE s.run_id = ?1 ORDER BY w.start_ms, w.id",
        )
        .unwrap();
    let rows: Vec<(i64, i64, String)> = st
        .query_map([run.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (s, e, l) in &rows {
        for b in format!("{s},{e},{l};").bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    (rows.len(), h)
}

/// One live line as the live view (`live.svelte.ts` `splitRows`) builds it.
#[derive(Debug, Clone)]
struct Row {
    start_ms: i64,
    end_ms: i64,
    label: String,
    speaker: Option<SpeakerId>,
    final_: bool,
}

struct Live<'a> {
    store: &'a SqliteStore,
    run: RunId,
    maps: HashMap<Channel, ClockMap>,
    speakers: HashMap<(Channel, String), SpeakerId>,
    /// The newest revision's lines per (channel, utt).
    rows: BTreeMap<(Channel, u32), Vec<Row>>,
    revisions: usize,
}

impl Live<'_> {
    fn speaker(&mut self, ch: Channel, label: &str) -> Option<SpeakerId> {
        if let Some(id) = self.speakers.get(&(ch, label.to_owned())) {
            return Some(*id);
        }
        let id = self.store.live_speaker(self.run, ch, label).ok()?.id;
        self.speakers.insert((ch, label.to_owned()), id);
        Some(id)
    }

    fn on_event(&mut self, ch: Channel, ev: EngineEvent) {
        let EngineEvent::Utterance {
            utt, rev, words, ..
        } = ev
        else {
            return;
        };
        let map = &self.maps[&ch];
        let words: Vec<EngineWord> = words
            .into_iter()
            .map(|w| EngineWord {
                t0_us: map.meeting_us(w.t0_us),
                t1_us: map.meeting_us(w.t1_us),
                spk: if ch == Channel::Mic { None } else { w.spk },
                ..w
            })
            .collect();
        self.store
            .append_live_utterance(self.run, ch, utt, rev, &words)
            .unwrap();
        self.revisions += 1;
        let default = if ch == Channel::Mic {
            "me"
        } else {
            "spk_unknown"
        };
        let final_ = ch == Channel::Mic || words.iter().any(|w| w.spk.is_some());
        let mut rows: Vec<Row> = Vec::new();
        for w in &words {
            let label = w.spk.clone().unwrap_or_else(|| default.to_owned());
            let id = self.speaker(ch, &label);
            let (s, e) = ((w.t0_us / 1_000) as i64, (w.t1_us / 1_000) as i64);
            match rows.last_mut() {
                Some(r) if r.label == label => r.end_ms = e,
                _ => rows.push(Row {
                    start_ms: s,
                    end_ms: e,
                    label,
                    speaker: id,
                    final_,
                }),
            }
        }
        self.rows.insert((ch, utt), rows);
    }
}

struct Ctx {
    out: PathBuf,
    db: PathBuf,
    engine_dir: PathBuf,
    models: PathBuf,
    asr: PathBuf,
    diar: PathBuf,
    log: std::fs::File,
}

impl Ctx {
    fn say(&mut self, s: String) {
        eprintln!("{s}");
        let _ = writeln!(self.log, "{s}");
        let _ = self.log.flush();
    }
}

#[test]
#[ignore]
fn replay_meeting_through_live_server_and_post_pass() {
    let (Some(flac), Some(engine_dir), Some(models)) = (
        std::env::var_os("REPRO_FLAC").map(PathBuf::from),
        std::env::var_os("DIARIST_ENGINE_DIR").map(PathBuf::from),
        std::env::var_os("DIARIST_MODELS_DIR").map(PathBuf::from),
    ) else {
        eprintln!("REPRO_FLAC, DIARIST_ENGINE_DIR and DIARIST_MODELS_DIR are needed");
        return;
    };
    let env_num = |k: &str, d: f64| -> f64 {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(d)
    };
    let out = std::env::var_os("REPRO_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("diarist-replay"));
    std::fs::create_dir_all(&out).unwrap();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join("replay.log"))
        .unwrap();
    let mut cx = Ctx {
        db: out.join("repro.db"),
        out: out.clone(),
        asr: find(&models, "nemotron-speech-streaming-en-0.6b.q8_0.gguf").expect("EN model"),
        diar: find(&models, "Nemotron-3-Diarization.q8_0.gguf").expect("diarizer"),
        engine_dir,
        models,
        log,
    };

    // Audio: left mic, right loopback.
    let (pcm, channels) = decode_i16(&flac).unwrap();
    assert_eq!(channels, 2, "a stereo FLAC: left mic, right loopback");
    let mut mic: Vec<i16> = pcm.iter().step_by(2).copied().collect();
    let mut lb: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
    if let Some(s) = std::env::var("REPRO_SECONDS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        mic.truncate(s * 16_000);
        lb.truncate(s * 16_000);
    }
    let n = mic.len().min(lb.len());
    cx.say(format!("audio: {n} samples ({:.1} s)", n as f64 / 16_000.0));

    if std::env::var_os("REPRO_RESUME").is_some() {
        let store = SqliteStore::open(&cx.db).unwrap();
        let meeting = store.list_meetings().unwrap()[0].id;
        let runs = store.list_runs(meeting).unwrap();
        let live = runs
            .iter()
            .rev()
            .find(|r| r.kind == RunKind::Live)
            .unwrap()
            .id;
        for r in runs.iter().filter(|r| r.kind == RunKind::PostPass) {
            let (count, sum) = words_checksum(&cx.db, r.id);
            cx.say(format!(
                "earlier post-pass run {}: {count} words, checksum {sum:016x} (deleted)",
                r.id
            ));
            store.delete_run(r.id).unwrap();
        }
        cx.say(format!("resuming meeting {meeting}, live run {live}"));
        finish(&mut cx, &store, meeting, live, &mic, &lb, n);
        return;
    }

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", cx.db.display()));
    }
    let store = SqliteStore::open(&cx.db).unwrap();
    let meeting = store
        .create_meeting(NewMeeting {
            title: "replay".into(),
            source: Source::Live,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 0,
            status: MeetingStatus::Recording,
        })
        .unwrap();
    let run = store
        .create_run(
            meeting.id,
            RunKind::Live,
            serde_json::json!({"replay": true}),
        )
        .unwrap();
    store.set_current_run(meeting.id, run.id).unwrap();
    let mut maps = HashMap::new();
    let source = std::env::var_os("REPRO_SRC_DB").map(PathBuf::from).zip(
        std::env::var("REPRO_MEETING")
            .ok()
            .and_then(|m| m.parse::<i64>().ok()),
    );
    match source {
        Some((db, m)) => {
            let anchors: Vec<ClockAnchor> = source_anchors(&db, m)
                .into_iter()
                .map(|(ch, s, q)| ClockAnchor {
                    meeting_id: meeting.id,
                    channel: ch.parse().unwrap(),
                    sample_index: s,
                    qpc_100ns: q,
                })
                .collect();
            assert!(!anchors.is_empty(), "no clock anchors for meeting {m}");
            store.add_clock_anchors(&anchors).unwrap();
            let origin = origin_from_anchors(&anchors).unwrap();
            for ch in [Channel::Mic, Channel::Loopback] {
                maps.insert(
                    ch,
                    ClockMap::from_anchors(origin, &channel_anchors(&anchors, ch)),
                );
            }
            cx.say(format!("clock: {} anchors of meeting {m}", anchors.len()));
        }
        None => {
            for ch in [Channel::Mic, Channel::Loopback] {
                maps.insert(ch, ClockMap::new(0));
            }
            cx.say("clock: nominal (no REPRO_SRC_DB / REPRO_MEETING)".into());
        }
    }

    // The live server as the app runs it.
    let mut cfg = ServeConfig::new(&cx.engine_dir, &cx.models, &cx.asr, &cx.diar);
    cfg.log_path = Some(cx.out.join("serve.log"));
    cfg.idle_timeout = None;
    let sup = ServeSupervisor::new(cfg);
    sup.ensure_ready(Duration::from_secs(120)).unwrap();
    let engine = NemoWsLiveEngine::new(sup.clone(), false);
    let mut sessions: Vec<(Channel, Box<dyn LiveSession>, u32)> = Vec::new();
    for (ch, diarize) in [(Channel::Mic, false), (Channel::Loopback, true)] {
        let s = engine
            .open(
                JobId(format!("replay-{ch}")),
                ch,
                LiveOpts {
                    diarize,
                    language: "en".into(),
                },
            )
            .unwrap();
        sessions.push((
            ch,
            s,
            0x9E37_79B9u32 ^ (ch as u32).wrapping_mul(0x85EB_CA6B),
        ));
    }
    let mut live = Live {
        store: &store,
        run: run.id,
        maps,
        speakers: HashMap::new(),
        rows: BTreeMap::new(),
        revisions: 0,
    };
    let speed = env_num("REPRO_SPEED", 1.0);
    let frames = n / 1_600;
    let rename_frame = (frames as f64 * env_num("REPRO_RENAME_AT", 0.5)) as usize;
    let start = Instant::now();
    for i in 0..frames {
        let due = start + Duration::from_secs_f64(0.1 * i as f64 / speed);
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        let mut stalled = None;
        for (ch, s, dither) in sessions.iter_mut() {
            let src = if *ch == Channel::Mic { &mic } else { &lb };
            let mut frame = src[i * 1_600..(i + 1) * 1_600].to_vec();
            dither_silence(&mut frame, dither);
            if let Err(e) = s.push(&frame, i as u64 * 100_000) {
                stalled = Some(format!("{ch} push failed at {:.1} s: {e}", i as f64 / 10.0));
            }
        }
        if let Some(why) = stalled {
            cx.say(format!(
                "stream stopped early: {why} (the live run covers the audio up to here)"
            ));
            break;
        }
        for (ch, s, _) in sessions.iter_mut() {
            while let Ok(ev) = s.events().try_recv() {
                live.on_event(*ch, ev);
            }
        }
        if i % 600 == 0 {
            cx.say(format!(
                "t {:.0} s: {} revisions, labels {:?}",
                i as f64 / 10.0,
                live.revisions,
                live.speakers.keys().collect::<Vec<_>>()
            ));
        }
        if i == rename_frame {
            rename_two(&mut cx, &store, &mut live, i);
        }
    }
    // Close: commit, last finals.
    for (ch, mut s, _) in sessions {
        let (_k, dummy) = std::sync::mpsc::channel();
        let rx = std::mem::replace(s.events(), dummy);
        s.close().unwrap();
        for ev in rx.try_iter() {
            live.on_event(ch, ev);
        }
    }
    drop(engine);
    sup.stop();
    cx.say(format!(
        "streamed in {:.1} s, {} revisions",
        start.elapsed().as_secs_f64(),
        live.revisions
    ));
    finish(&mut cx, &store, meeting.id, run.id, &mic, &lb, n);
}

/// The loopback label with the most final lines is renamed from the legend,
/// the second one from its newest final line.
fn rename_two(cx: &mut Ctx, store: &SqliteStore, live: &mut Live<'_>, frame: usize) {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for ((ch, _), rows) in &live.rows {
        for r in rows.iter().filter(|r| *ch == Channel::Loopback && r.final_) {
            if r.label != "spk_unknown" {
                *count.entry(r.label.clone()).or_default() += 1;
            }
        }
    }
    let mut by: Vec<(usize, String)> = count.into_iter().map(|(l, c)| (c, l)).collect();
    by.sort_by(|a, b| b.cmp(a));
    cx.say(format!(
        "rename at {:.0} s: final loopback lines per label {by:?}",
        frame as f64 / 10.0
    ));
    if let Some((_, top)) = by.first() {
        let id = live.speaker(Channel::Loopback, top).unwrap();
        store.rename_speaker(id, "LegendName", None).unwrap();
        cx.say(format!(
            "{top} (speaker {id}) -> LegendName, from the legend"
        ));
    }
    if let Some((_, second)) = by.get(1) {
        let row = live
            .rows
            .iter()
            .filter(|((ch, _), _)| *ch == Channel::Loopback)
            .flat_map(|(_, rs)| rs.iter())
            .rfind(|r| r.final_ && &r.label == second)
            .cloned()
            .unwrap();
        let id = row.speaker.unwrap();
        store
            .rename_speaker_at(id, "LineName", row.start_ms, row.end_ms)
            .unwrap();
        cx.say(format!(
            "{second} (speaker {id}) -> LineName, from its line {}..{} ms",
            row.start_ms, row.end_ms
        ));
    }
}

/// Stop (materialise the live run) and the post-pass as the app runs them.
fn finish(
    cx: &mut Ctx,
    store: &SqliteStore,
    meeting: MeetingId,
    run: RunId,
    mic: &[i16],
    lb: &[i16],
    n: usize,
) {
    let duration_ms = (n as i64) * 1_000 / 16_000;
    store
        .finish_meeting(meeting, duration_ms, duration_ms)
        .unwrap();
    store
        .set_meeting_status(meeting, MeetingStatus::Processing)
        .unwrap();
    match store.materialise_live_run(run, &SegmentOptions::default()) {
        Ok(()) | Err(StoreError::Conflict(_)) => {}
        Err(e) => panic!("materialise: {e}"),
    }
    if store.get_run(run).unwrap().status == RunStatus::Running {
        store.finish_run(run, RunStatus::Done).unwrap();
    }
    store.set_current_run(meeting, run).unwrap();

    let uid = store.get_meeting(meeting).unwrap().meeting.uid;
    let folder = cx.out.join("audio").join(&uid);
    std::fs::create_dir_all(&folder).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    for (name, src) in [("mic.wav", mic), ("loopback.wav", lb)] {
        let mut w = hound::WavWriter::create(folder.join(name), spec).unwrap();
        for &v in &src[..n] {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
    }
    let mut file_engine = NemoCliEngine::new(&cx.engine_dir, &cx.models, &cx.asr, &cx.diar);
    file_engine.pass_language = false;
    file_engine.right_context = Some(13);
    file_engine.diar_preset = Some("v3-offline".into());
    let job = PostPassJob {
        job_id: JobId::new(),
        meeting,
        live_run: run,
        data_root: cx.out.clone(),
        folder: folder.clone(),
        language: "en".into(),
        diarize_mic: false,
        transcribe: true,
        keep_live_runs: true,
        segment: SegmentOptions::default(),
        punct: diarist_core::punct::PunctSource::None,
        from_flac: false,
    };
    let t = Instant::now();
    let report =
        run_post_pass(store, &file_engine, &job, &CancelToken::default(), &|_| {}).unwrap();
    // The recording's copies are not needed after the post-pass.
    let _ = std::fs::remove_dir_all(&folder);
    cx.say(format!(
        "post-pass {:.1} s; current run {}",
        t.elapsed().as_secs_f64(),
        report.current_run
    ));
    cx.say(format!("carry-over: {:?}", report.carry_over));
    for (label, r) in [("live", run), ("post-pass", report.current_run)] {
        for s in store.run_speakers(r).unwrap() {
            cx.say(format!(
                "{label} speaker {} {} {} name {:?} anchor {:?}..{:?} review {}",
                s.id,
                s.channel,
                s.engine_label,
                s.display_name,
                s.name_anchor_start_ms,
                s.name_anchor_end_ms,
                s.needs_review
            ));
        }
    }
    let (count, sum) = words_checksum(&cx.db, report.current_run);
    cx.say(format!(
        "post-pass words: {count}, checksum {sum:016x}; unplaced names {}; scratch DB {} (meeting {meeting}, live run {run}, post-pass run {})",
        report.unplaced_names,
        cx.db.display(),
        report.current_run
    ));
    assert_eq!(
        report.unplaced_names, 0,
        "a name given during the replay reached nobody: {:?}",
        report.carry_over
    );
}
