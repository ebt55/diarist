//! Adapter tests: the translator and time map on their own, and a whole
//! session against an in-process fake server that replays the spike's
//! recorded event stream (`tests/fixtures/ws_session_events.jsonl`).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::*;

fn fixture() -> Vec<Value> {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/ws_session_events.jsonl",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn time_map_handles_offsets_drops_and_filler() {
    let mut m = TimeMap::default();
    // A session opened 10 s into the recording.
    m.record(160_000, 1_600);
    m.record(161_600, 1_600);
    assert_eq!(m.pts_of(0), 160_000);
    assert_eq!(m.pts_of(2_000), 162_000);
    // The feed dropped 2 s, then resumed.
    m.record(195_200, 1_600);
    assert_eq!(m.pts_of(3_200), 195_200);
    assert_eq!(m.pts_us(0.15), 10_150_000);
    assert_eq!(m.pts_us(0.25), 12_250_000);
    // Keep-alive silence maps past the last real frame.
    m.record_filler(1_600);
    assert_eq!(m.pts_of(4_800), 196_800);
    m.record(240_000, 1_600);
    assert_eq!(m.pts_of(6_400), 240_000);
    assert_eq!(m.sent(), 8_000);
}

fn delta(text: &str, at: f64) -> Value {
    json!({"type": "conversation.item.input_audio_transcription.delta", "delta": text, "audio_processed": at})
}

#[test]
fn translator_partials_finals_and_rewrites() {
    let map = TimeMap::default();
    let mut seen = Seen::default();
    let mut t = Translator::new(JobId("j".into()), Channel::Loopback, true);
    let mut evs = Vec::new();
    for (d, at) in [("Hel", 1.0), ("lo", 1.1), ("", 1.2), (" world", 1.3)] {
        evs.extend(t.on_server_event(&delta(d, at), &map, &mut seen));
    }
    assert_eq!(evs.len(), 3);
    let EngineEvent::Utterance {
        utt, rev, words, ..
    } = &evs[2]
    else {
        panic!()
    };
    assert_eq!((*utt, *rev), (0, 2));
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(texts, ["Hello", "world"]);
    assert!(words.iter().all(|w| w.spk.is_none() && w.t1_us > w.t0_us));
    // A rewritten hypothesis replaces the partial.
    let evs = t.on_server_event(&delta("Hello world again", 1.5), &map, &mut seen);
    let EngineEvent::Utterance { words, .. } = &evs[0] else {
        panic!()
    };
    assert_eq!(words.len(), 3);
    let done = json!({"type": "conversation.item.input_audio_transcription.completed",
        "audio_processed": 2.0, "transcript": "Hello world again",
        "words": [{"word": "Hello", "start": 0.8, "end": 1.0, "speaker": 3},
                  {"word": "world", "start": 1.1, "end": 1.3, "speaker": 3},
                  {"word": "again", "start": 1.4, "end": 1.6, "speaker": 1}]});
    let evs = t.on_server_event(&done, &map, &mut seen);
    let EngineEvent::Utterance {
        utt, rev, words, ..
    } = &evs[0]
    else {
        panic!()
    };
    assert_eq!((*utt, *rev), (0, 4));
    assert_eq!(words[0].spk.as_deref(), Some("spk_0"));
    assert_eq!(words[2].spk.as_deref(), Some("spk_1"));
    assert_eq!((words[0].t0_us, words[0].t1_us), (800_000, 1_000_000));
    assert!(matches!(
        evs[1],
        EngineEvent::TextFinalUntil {
            t_us: 2_000_000,
            ..
        }
    ));
    // A partial that ends in an empty final is cleared, then the next utt starts.
    t.on_server_event(&delta("um", 2.5), &map, &mut seen);
    let empty = json!({"type": "conversation.item.input_audio_transcription.completed",
        "audio_processed": 3.0, "transcript": "", "words": []});
    let evs = t.on_server_event(&empty, &map, &mut seen);
    assert!(
        matches!(&evs[0], EngineEvent::Utterance { utt: 1, rev: 1, words, .. } if words.is_empty())
    );
    let evs = t.on_server_event(&delta("next", 3.5), &map, &mut seen);
    assert!(matches!(
        &evs[0],
        EngineEvent::Utterance { utt: 2, rev: 0, .. }
    ));
    // Without diarization, speaker ids are dropped.
    let mut mic = Translator::new(JobId("m".into()), Channel::Mic, false);
    let evs = mic.on_server_event(&done, &map, &mut seen);
    assert!(
        matches!(&evs[0], EngineEvent::Utterance { words, .. } if words.iter().all(|w| w.spk.is_none()))
    );
    // Server errors and the commit acknowledgement.
    let evs = t.on_server_event(
        &json!({"type": "error", "error": {"message": "boom"}}),
        &map,
        &mut seen,
    );
    assert!(matches!(&evs[0], EngineEvent::Error { message, .. } if message == "boom"));
    t.on_server_event(
        &json!({"type": "input_audio_buffer.committed"}),
        &map,
        &mut seen,
    );
    assert!(seen.committed);
}

#[test]
fn session_update_only_sends_a_real_language() {
    let opts = |lang: &str| LiveOpts {
        diarize: false,
        language: lang.into(),
    };
    let u = session_update(&opts("de"), true);
    assert_eq!(u["session"]["language"], "de");
    assert_eq!(u["session"]["speaker_diarization"], false);
    assert!(session_update(&opts("auto"), true)["session"]
        .get("language")
        .is_none());
    assert!(session_update(&opts("de"), false)["session"]
        .get("language")
        .is_none());
}

/// What the fake server saw.
#[derive(Default)]
struct ServerLog {
    auth: Mutex<Option<String>>,
    update: Mutex<Option<Value>>,
    audio_bytes: AtomicU64,
    committed: AtomicBool,
    closed_normally: AtomicBool,
}

/// Replays the fixture's server events: everything before the client's
/// commit at `speed` times real time, the rest once the commit arrives.
#[allow(clippy::result_large_err)] // the handshake callback's signature is tungstenite's
async fn fake_server(listener: TcpListener, events: Vec<Value>, speed: f64, log: Arc<ServerLog>) {
    let (tcp, _) = listener.accept().await.unwrap();
    let log_h = Arc::clone(&log);
    let ws = tokio_tungstenite::accept_hdr_async(tcp, move |req: &Request, resp: Response| {
        *log_h.auth.lock().unwrap() = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(resp)
    })
    .await
    .unwrap();
    let (mut sink, mut stream) = ws.split();
    let commit_at = events
        .iter()
        .position(|e| e["dir"] == "client" && e["event"]["type"] == "input_audio_buffer.commit")
        .unwrap();
    let server = |range: std::ops::Range<usize>| -> Vec<(f64, Value)> {
        events[range]
            .iter()
            .filter(|e| e["dir"] == "server")
            .map(|e| (e["t"].as_f64().unwrap(), e["event"].clone()))
            .collect()
    };
    let before = server(0..commit_at);
    let after = server(commit_at..events.len());
    // session.created, then wait for session.update.
    sink.send(Message::Text(before[0].1.to_string().into()))
        .await
        .unwrap();
    loop {
        if let Some(Ok(Message::Text(t))) = stream.next().await {
            *log.update.lock().unwrap() = serde_json::from_str(t.as_str()).ok();
            break;
        }
    }
    let log_r = Arc::clone(&log);
    let reader = tokio::spawn(async move {
        while let Some(Ok(m)) = stream.next().await {
            match m {
                Message::Binary(b) => {
                    log_r
                        .audio_bytes
                        .fetch_add(b.len() as u64, Ordering::SeqCst);
                }
                Message::Text(t) => {
                    if t.as_str().contains("input_audio_buffer.commit") {
                        log_r.committed.store(true, Ordering::SeqCst);
                    }
                }
                Message::Close(f) => {
                    let normal = f.is_some_and(|f| f.code == CloseCode::Normal);
                    log_r.closed_normally.store(normal, Ordering::SeqCst);
                    break;
                }
                _ => {}
            }
        }
    });
    let start = tokio::time::Instant::now();
    for (t, ev) in &before[1..] {
        tokio::time::sleep_until(start + Duration::from_secs_f64(t / speed)).await;
        sink.send(Message::Text(ev.to_string().into()))
            .await
            .unwrap();
    }
    while !log.committed.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for (_, ev) in &after {
        sink.send(Message::Text(ev.to_string().into()))
            .await
            .unwrap();
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), reader).await;
    let _ = sink.close().await;
}

#[test]
fn replays_the_recorded_session() {
    let events = fixture();
    let rt = runtime();
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(ServerLog::default());
    let server = rt.spawn(fake_server(
        listener,
        events.clone(),
        30.0,
        Arc::clone(&log),
    ));
    let engine = NemoWsLiveEngine::with_endpoint(
        format!("ws://127.0.0.1:{port}/v1/audio/transcriptions/realtime"),
        "0123456789abcdef0123456789abcdef",
    );
    let job = JobId("live-1".into());
    let mut session = engine
        .open(
            job.clone(),
            Channel::Loopback,
            LiveOpts {
                diarize: true,
                language: "en".into(),
            },
        )
        .unwrap();
    // 60 s of audio as 600 frames of 100 ms (contents do not matter here).
    for i in 0..600u64 {
        session.push(&[0i16; 1_600], i * 100_000).unwrap();
    }
    // Let the pre-commit part of the replay finish (60 s / 30).
    std::thread::sleep(Duration::from_millis(2_300));
    let (_keep, dummy) = std::sync::mpsc::channel();
    let rx = std::mem::replace(session.events(), dummy);
    session.close().unwrap();
    let got: Vec<EngineEvent> = rx.try_iter().collect();
    rt.block_on(server).unwrap();

    assert_eq!(
        log.auth.lock().unwrap().as_deref(),
        Some("Bearer 0123456789abcdef0123456789abcdef")
    );
    assert_eq!(
        log.update.lock().unwrap().clone().unwrap(),
        json!({"type": "session.update", "session": {"sample_rate": 16000, "word_timestamps": true, "speaker_diarization": true}})
    );
    assert_eq!(log.audio_bytes.load(Ordering::SeqCst), 600 * 3_200);
    assert!(log.closed_normally.load(Ordering::SeqCst));

    // Expected sequence, rebuilt from the fixture: one rev per non-empty
    // delta, the final rev per completed, then TextFinalUntil; Done last.
    let mut expected: Vec<(u32, u32, Option<String>)> = Vec::new();
    let (mut utt, mut rev) = (0u32, 0u32);
    let mut finals = 0;
    for e in events.iter().filter(|e| e["dir"] == "server") {
        let ty = e["event"]["type"].as_str().unwrap();
        if ty.ends_with(".delta") && !e["event"]["delta"].as_str().unwrap().is_empty() {
            expected.push((utt, rev, None));
            rev += 1;
        } else if ty.ends_with(".completed") {
            expected.push((
                utt,
                rev,
                Some(e["event"]["transcript"].as_str().unwrap().into()),
            ));
            utt += 1;
            rev = 0;
            finals += 1;
        }
    }
    let mut seq = Vec::new();
    let mut tfu = Vec::new();
    let mut speakers = BTreeSet::new();
    let mut first_final_words = None;
    for ev in &got {
        match ev {
            EngineEvent::Utterance {
                job_id,
                channel,
                utt,
                rev,
                words,
            } => {
                assert_eq!(job_id, &job);
                assert_eq!(*channel, Channel::Loopback);
                let is_final = words.iter().any(|w| w.spk.is_some());
                for w in words {
                    if let Some(s) = &w.spk {
                        speakers.insert(s.clone());
                    }
                }
                if is_final && first_final_words.is_none() {
                    first_final_words = Some(words.clone());
                }
                let text = is_final.then(|| {
                    words
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                seq.push((*utt, *rev, text));
            }
            EngineEvent::TextFinalUntil { t_us, .. } => tfu.push(*t_us),
            _ => {}
        }
    }
    assert_eq!(seq, expected);
    assert_eq!(tfu.len(), finals);
    assert_eq!(finals, 15);
    assert!(tfu.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(tfu[0], 4_100_000);
    assert_eq!(
        speakers.into_iter().collect::<Vec<_>>(),
        ["spk_0", "spk_1", "spk_2", "spk_3"]
    );
    let w = first_final_words.unwrap();
    assert_eq!(
        (w[0].text.as_str(), w[0].t0_us, w[0].t1_us),
        ("Um,", 640_000, 720_000)
    );
    assert!(matches!(
        got.last(),
        Some(EngineEvent::Done { stats, .. }) if stats.audio_ms == Some(60_000) && stats.speakers == Some(4)
    ));
    assert!(!got.iter().any(|e| matches!(e, EngineEvent::Error { .. })));
}

#[test]
fn a_dying_server_becomes_an_error() {
    let rt = runtime();
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = rt.spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let _ = ws.next().await; // session.update
        let _ = ws.next().await; // one frame
        drop(ws); // crash: no close handshake
    });
    let engine = NemoWsLiveEngine::with_endpoint(format!("ws://127.0.0.1:{port}/x"), "k");
    let mut s = engine
        .open(
            JobId("j".into()),
            Channel::Mic,
            LiveOpts {
                diarize: false,
                language: "en".into(),
            },
        )
        .unwrap();
    s.push(&[0i16; 1_600], 0).unwrap();
    rt.block_on(server).unwrap();
    let ev = s.events().recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(
            ev,
            EngineEvent::Error {
                code: ErrorCode::Crash,
                ..
            }
        ),
        "{ev:?}"
    );
    // Pushing into a dead session fails soon.
    let mut failed = false;
    for i in 0..200 {
        if s.push(&[0i16; 1_600], i * 100_000).is_err() {
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(failed);
    // Nothing listens: open fails.
    let dead = NemoWsLiveEngine::with_endpoint("ws://127.0.0.1:9/x", "k");
    assert!(dead
        .open(
            JobId("j".into()),
            Channel::Mic,
            LiveOpts {
                diarize: false,
                language: "en".into()
            }
        )
        .is_err());
}
