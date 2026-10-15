//! `NemoWsLiveEngine`: the v2 live adapter (`docs/03`, "v2 live adapter";
//! research/08 section 4) over `nemo-speech serve`'s realtime WebSocket.
//!
//! One [`WsSession`] per channel at `/v1/audio/transcriptions/realtime`
//! with `Authorization: Bearer <key>`: one `session.update` (`sample_rate`,
//! `word_timestamps`, `speaker_diarization`, and `language` only for
//! multilingual models and never `auto`), then binary PCM16 frames.
//! [`Translator`] turns server events into [`EngineEvent`]s:
//! `…transcription.delta` grows the current utterance (`Utterance` with the
//! same `utt` and the next `rev`; words get times spread over the partial's
//! span and no speaker), `…transcription.completed` is that utterance's
//! final `rev` with the runtime's word times and speakers (normalized to
//! `spk_N` by first appearance) followed by `TextFinalUntil` at its end.
//!
//! Engine times count the samples the server received; [`TimeMap`] maps
//! them back to channel-sample time (`pts`), so frames dropped by a
//! degraded feed or keep-alive silence never shift word times. When no
//! frame has been sent for 5 s the writer sends 100 ms of 1-LSB noise, so
//! the server's 30 s read timeout never fires. `close` sends
//! `input_audio_buffer.commit`, waits for `input_audio_buffer.committed`
//! (the last `.completed` comes before it), closes with code 1000 and ends
//! with `Done`.

mod supervisor;

pub use supervisor::{
    ServeConfig, ServeEndpoint, ServeSnapshot, ServeStatus, ServeSupervisor, SessionLease,
};

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc as tmpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use super::{EngineEvent, EngineWord, ErrorCode, JobStats, LiveEngine, LiveOpts, LiveSession};
use crate::error::{CoreError, Result};
use crate::transcript::LabelMap;
use crate::types::{Channel, JobId};

/// Samples per second on the wire.
const RATE: u64 = 16_000;
/// Frames queued between `push` and the socket (5 s of 100 ms frames).
const SEND_QUEUE: usize = 50;
/// Silence is sent after this long without frames.
const KEEPALIVE: Duration = Duration::from_secs(5);
/// How long `push` waits for room in the send queue.
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `close` waits for `input_audio_buffer.committed`.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The process-wide runtime for WebSocket I/O.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("live-ws")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn engine_err(msg: impl Into<String>) -> CoreError {
    CoreError::Engine(msg.into())
}

// ---- time mapping -------------------------------------------------------------------

/// Maps engine time (samples the server received) to channel-sample time.
#[derive(Debug, Clone, Default)]
pub struct TimeMap {
    /// `(sent_sample, pts_sample)` at each discontinuity, increasing.
    segments: Vec<(u64, u64)>,
    sent: u64,
    /// pts just after the last real frame.
    pts_end: u64,
}

impl TimeMap {
    /// Records `n` samples sent whose first one is at channel sample `pts`.
    pub fn record(&mut self, pts: u64, n: u64) {
        let continues = self
            .segments
            .last()
            .is_some_and(|&(s, p)| p + (self.sent - s) == pts);
        if !continues {
            self.segments.push((self.sent, pts));
        }
        self.sent += n;
        self.pts_end = pts + n;
    }

    /// Records `n` samples of keep-alive silence (they map to the end of
    /// the last real frame onward).
    pub fn record_filler(&mut self, n: u64) {
        self.segments.push((self.sent, self.pts_end));
        self.sent += n;
    }

    /// Samples sent so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Channel sample of engine sample `t`.
    pub fn pts_of(&self, t: u64) -> u64 {
        let i = self.segments.partition_point(|&(s, _)| s <= t);
        match i {
            0 => t,
            _ => {
                let (s, p) = self.segments[i - 1];
                p + (t - s)
            }
        }
    }

    /// Channel time (µs) of engine time `secs`.
    pub fn pts_us(&self, secs: f64) -> u64 {
        let t = (secs.max(0.0) * RATE as f64).round() as u64;
        self.pts_of(t) * 1_000_000 / RATE
    }
}

// ---- event translation --------------------------------------------------------------

/// Turns realtime server events into engine events (see the module docs).
pub struct Translator {
    job_id: JobId,
    channel: Channel,
    diarize: bool,
    labels: LabelMap,
    utt: u32,
    rev: u32,
    partial: String,
    /// Engine seconds where the current partial starts.
    utt_start: Option<f64>,
    /// Engine seconds where the previous utterance ended.
    prev_end: f64,
    /// Utterance revisions emitted (for `Done` stats).
    pub utterances: u32,
}

/// What [`Translator::on_server_event`] saw besides engine events.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub committed: bool,
    pub updated: bool,
}

impl Translator {
    pub fn new(job_id: JobId, channel: Channel, diarize: bool) -> Self {
        Self {
            job_id,
            channel,
            diarize,
            labels: LabelMap::new(),
            utt: 0,
            rev: 0,
            partial: String::new(),
            utt_start: None,
            prev_end: 0.0,
            utterances: 0,
        }
    }

    /// Distinct speakers seen.
    pub fn speakers(&self) -> u32 {
        self.labels.len() as u32
    }

    fn utterance(&mut self, words: Vec<EngineWord>) -> EngineEvent {
        let ev = EngineEvent::Utterance {
            job_id: self.job_id.clone(),
            channel: self.channel,
            utt: self.utt,
            rev: self.rev,
            words,
        };
        self.rev += 1;
        self.utterances += 1;
        ev
    }

    /// Words of the partial, spread evenly over `[start, end]` engine seconds.
    fn partial_words(&self, start: f64, end: f64, map: &TimeMap) -> Vec<EngineWord> {
        let toks: Vec<&str> = self.partial.split_whitespace().collect();
        let n = toks.len().max(1) as f64;
        let step = ((end - start).max(0.08 * n)) / n;
        toks.iter()
            .enumerate()
            .map(|(i, w)| {
                let t0 = start + i as f64 * step;
                EngineWord {
                    t0_us: map.pts_us(t0),
                    t1_us: map.pts_us(t0 + step),
                    text: (*w).to_owned(),
                    conf: None,
                    spk: None,
                }
            })
            .collect()
    }

    /// Handles one server event (parsed JSON).
    pub fn on_server_event(
        &mut self,
        ev: &Value,
        map: &TimeMap,
        seen: &mut Seen,
    ) -> Vec<EngineEvent> {
        let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
        let processed = ev.get("audio_processed").and_then(Value::as_f64);
        match kind {
            "session.updated" => {
                seen.updated = true;
                vec![]
            }
            "input_audio_buffer.committed" => {
                seen.committed = true;
                vec![]
            }
            "error" => {
                let message = ev
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("server error")
                    .to_owned();
                vec![EngineEvent::Error {
                    job_id: self.job_id.clone(),
                    code: ErrorCode::Other,
                    message,
                }]
            }
            k if k.ends_with("input_audio_transcription.delta") => {
                let delta = ev.get("delta").and_then(Value::as_str).unwrap_or("");
                if delta.is_empty() {
                    return vec![];
                }
                let first_word = self.partial.split_whitespace().next().unwrap_or("");
                let rewrite = !self.partial.is_empty()
                    && delta.len() >= self.partial.len()
                    && !first_word.is_empty()
                    && delta.trim_start().starts_with(first_word);
                if rewrite {
                    self.partial = delta.trim_start().to_owned();
                } else {
                    self.partial.push_str(delta);
                }
                let now = processed.unwrap_or(self.prev_end);
                let start = *self
                    .utt_start
                    .get_or_insert_with(|| (now - 0.5).max(self.prev_end));
                let words = self.partial_words(start, now, map);
                vec![self.utterance(words)]
            }
            k if k.ends_with("input_audio_transcription.completed") => {
                let words: Vec<EngineWord> = ev
                    .get("words")
                    .and_then(Value::as_array)
                    .map(|ws| {
                        ws.iter()
                            .filter_map(|w| {
                                let text = w.get("word")?.as_str()?.to_owned();
                                let start = w.get("start")?.as_f64()?;
                                let end = w.get("end").and_then(Value::as_f64).unwrap_or(start);
                                let spk = if self.diarize {
                                    w.get("speaker")
                                        .and_then(Value::as_u64)
                                        .map(|s| self.labels.label(Some(s as u32)))
                                } else {
                                    None
                                };
                                Some(EngineWord {
                                    t0_us: map.pts_us(start),
                                    t1_us: map.pts_us(end.max(start)),
                                    text,
                                    conf: None,
                                    spk,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let transcript = ev.get("transcript").and_then(Value::as_str).unwrap_or("");
                let end_s = ev
                    .get("words")
                    .and_then(Value::as_array)
                    .and_then(|ws| ws.last())
                    .and_then(|w| w.get("end"))
                    .and_then(Value::as_f64)
                    .or(processed)
                    .unwrap_or(self.prev_end);
                let mut out = Vec::new();
                if !words.is_empty() {
                    out.push(self.utterance(words));
                } else if !transcript.trim().is_empty() {
                    // Text without word times: keep the partial's estimate.
                    self.partial = transcript.trim().to_owned();
                    let now = processed.unwrap_or(end_s);
                    let start = self.utt_start.unwrap_or((now - 1.0).max(self.prev_end));
                    let words = self.partial_words(start, now, map);
                    out.push(self.utterance(words));
                } else if self.rev > 0 {
                    // The partial came to nothing: replace it with an empty utterance.
                    out.push(self.utterance(Vec::new()));
                }
                if self.rev > 0 {
                    out.push(EngineEvent::TextFinalUntil {
                        job_id: self.job_id.clone(),
                        channel: self.channel,
                        t_us: map.pts_us(processed.unwrap_or(end_s).max(end_s)),
                    });
                    self.utt += 1;
                }
                self.rev = 0;
                self.partial.clear();
                self.utt_start = None;
                self.prev_end = end_s;
                out
            }
            _ => vec![],
        }
    }
}

// ---- the session ----------------------------------------------------------------------

enum Outgoing {
    Audio { pcm: Vec<i16>, pts_us: u64 },
    Commit,
}

/// One live session over the realtime WebSocket.
pub struct WsSession {
    job_id: JobId,
    channel: Channel,
    tx: Option<tmpsc::Sender<Outgoing>>,
    events: Receiver<EngineEvent>,
    finished: Option<oneshot::Receiver<()>>,
    _lease: Option<SessionLease>,
}

/// `session.update` for these options (`language` only when passed and not `auto`).
pub fn session_update(opts: &LiveOpts, pass_language: bool) -> Value {
    let mut session = json!({
        "sample_rate": RATE,
        "word_timestamps": true,
        "speaker_diarization": opts.diarize,
    });
    let lang = opts.language.trim();
    if pass_language && !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
        session["language"] = json!(lang);
    }
    json!({"type": "session.update", "session": session})
}

impl WsSession {
    /// Connects to `url`, sends `session.update` and starts the I/O tasks.
    pub fn connect(
        url: &str,
        api_key: &str,
        job_id: JobId,
        channel: Channel,
        opts: &LiveOpts,
        pass_language: bool,
        lease: Option<SessionLease>,
    ) -> Result<Self> {
        let mut req = url
            .into_client_request()
            .map_err(|e| engine_err(format!("websocket url {url}: {e}")))?;
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|e| engine_err(format!("api key: {e}")))?,
        );
        let update = session_update(opts, pass_language).to_string();
        let rt = runtime();
        let ws = rt.block_on(async {
            let (mut ws, _) = tokio::time::timeout(
                Duration::from_secs(10),
                tokio_tungstenite::connect_async(req),
            )
            .await
            .map_err(|_| engine_err("websocket connect timed out"))?
            .map_err(|e| engine_err(format!("websocket connect {url}: {e}")))?;
            ws.send(Message::Text(update.into()))
                .await
                .map_err(|e| engine_err(format!("session.update: {e}")))?;
            Ok::<_, CoreError>(ws)
        })?;
        let (sink, stream) = ws.split();
        let (tx, rx) = tmpsc::channel::<Outgoing>(SEND_QUEUE);
        let (ev_tx, ev_rx) = mpsc::channel::<EngineEvent>();
        let (committed_tx, committed_rx) = oneshot::channel::<()>();
        let (finished_tx, finished_rx) = oneshot::channel::<()>();
        let map = Arc::new(Mutex::new(TimeMap::default()));
        let started = Instant::now();
        let writer = rt.spawn(writer_task(sink, rx, Arc::clone(&map), committed_rx));
        rt.spawn(reader_task(
            stream,
            Translator::new(job_id.clone(), channel, opts.diarize),
            map,
            ev_tx,
            committed_tx,
            finished_tx,
            writer,
            started,
        ));
        Ok(Self {
            job_id,
            channel,
            tx: Some(tx),
            events: ev_rx,
            finished: Some(finished_rx),
            _lease: lease,
        })
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    pub fn job_id(&self) -> &JobId {
        &self.job_id
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

fn pcm_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
}

async fn writer_task(
    mut sink: WsSink,
    mut rx: tmpsc::Receiver<Outgoing>,
    map: Arc<Mutex<TimeMap>>,
    committed: oneshot::Receiver<()>,
) {
    let mut dither = 0x2545_F491u32;
    loop {
        match tokio::time::timeout(KEEPALIVE, rx.recv()).await {
            Ok(Some(Outgoing::Audio { pcm, pts_us })) => {
                let pts = pts_us * RATE / 1_000_000;
                map.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record(pts, pcm.len() as u64);
                if sink
                    .send(Message::Binary(pcm_bytes(&pcm).into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Ok(Some(Outgoing::Commit)) => {
                let commit = json!({"type": "input_audio_buffer.commit"}).to_string();
                if sink.send(Message::Text(commit.into())).await.is_err() {
                    return;
                }
                let _ = tokio::time::timeout(COMMIT_TIMEOUT, committed).await;
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::Normal,
                        reason: "".into(),
                    })))
                    .await;
                let _ = sink.flush().await;
                return;
            }
            Ok(None) => {
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
            Err(_) => {
                let mut pcm = vec![0i16; 1_600];
                crate::audio::feed::dither_silence(&mut pcm, &mut dither);
                map.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record_filler(pcm.len() as u64);
                if sink
                    .send(Message::Binary(pcm_bytes(&pcm).into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reader_task(
    mut stream: WsStream,
    mut tr: Translator,
    map: Arc<Mutex<TimeMap>>,
    ev_tx: mpsc::Sender<EngineEvent>,
    committed_tx: oneshot::Sender<()>,
    finished_tx: oneshot::Sender<()>,
    writer: tokio::task::JoinHandle<()>,
    started: Instant,
) {
    let mut committed_tx = Some(committed_tx);
    let mut seen = Seen::default();
    let mut failure: Option<String> = None;
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else {
                    continue;
                };
                let events = {
                    let m = map.lock().unwrap_or_else(|e| e.into_inner());
                    tr.on_server_event(&v, &m, &mut seen)
                };
                for ev in events {
                    let is_err = matches!(ev, EngineEvent::Error { .. });
                    let _ = ev_tx.send(ev);
                    if is_err {
                        failure = Some(String::new());
                    }
                }
                if seen.committed {
                    if let Some(c) = committed_tx.take() {
                        let _ = c.send(());
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                if !seen.committed {
                    failure = Some(format!("websocket: {e}"));
                }
                break;
            }
        }
    }
    let job_id = tr.job_id.clone();
    match failure {
        // An `error` event was already forwarded.
        Some(m) if m.is_empty() => {}
        Some(message) => {
            let _ = ev_tx.send(EngineEvent::Error {
                job_id,
                code: ErrorCode::Crash,
                message,
            });
        }
        None if !seen.committed && !writer.is_finished() => {
            let _ = ev_tx.send(EngineEvent::Error {
                job_id,
                code: ErrorCode::Crash,
                message: "the server closed the session".into(),
            });
        }
        None => {
            let audio_ms = map.lock().unwrap_or_else(|e| e.into_inner()).sent() * 1_000 / RATE;
            let wall_ms = started.elapsed().as_millis() as u64;
            let _ = ev_tx.send(EngineEvent::Done {
                job_id,
                stats: JobStats {
                    rtf: (audio_ms > 0).then(|| wall_ms as f64 / audio_ms as f64),
                    speakers: Some(tr.speakers()),
                    audio_ms: Some(audio_ms),
                    wall_ms: Some(wall_ms),
                },
            });
        }
    }
    writer.abort();
    let _ = finished_tx.send(());
}

impl LiveSession for WsSession {
    fn push(&mut self, pcm: &[i16], pts_us: u64) -> Result<()> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| engine_err("session is closing"))?;
        let mut item = Outgoing::Audio {
            pcm: pcm.to_vec(),
            pts_us,
        };
        let deadline = Instant::now() + PUSH_TIMEOUT;
        loop {
            match tx.try_send(item) {
                Ok(()) => return Ok(()),
                Err(tmpsc::error::TrySendError::Closed(_)) => {
                    return Err(engine_err("live session connection closed"))
                }
                Err(tmpsc::error::TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        return Err(engine_err("live session stalled (send queue full)"));
                    }
                    item = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn events(&mut self) -> &mut Receiver<EngineEvent> {
        &mut self.events
    }

    fn close(mut self: Box<Self>) -> Result<()> {
        if let Some(tx) = self.tx.take() {
            let rt = runtime();
            let _ = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), tx.send(Outgoing::Commit)).await
            });
        }
        if let Some(fin) = self.finished.take() {
            let _ = runtime().block_on(async {
                tokio::time::timeout(COMMIT_TIMEOUT + Duration::from_secs(5), fin).await
            });
        }
        Ok(())
    }
}

// ---- the engine -----------------------------------------------------------------------

enum Target {
    Supervised(ServeSupervisor),
    Fixed(ServeEndpoint, String),
}

/// The live engine over `nemo-speech serve`.
pub struct NemoWsLiveEngine {
    target: Target,
    /// Send the meeting language (multilingual models such as Nemotron 3.5).
    pub pass_language: bool,
    /// How long `open` waits for the server to become ready.
    pub ready_timeout: Duration,
}

impl NemoWsLiveEngine {
    /// Sessions on the server `supervisor` runs (started on first open).
    pub fn new(supervisor: ServeSupervisor, pass_language: bool) -> Self {
        Self {
            target: Target::Supervised(supervisor),
            pass_language,
            ready_timeout: Duration::from_secs(120),
        }
    }

    /// Sessions on a server that is already running at `url`.
    pub fn with_endpoint(url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            target: Target::Fixed(
                ServeEndpoint {
                    port: 0,
                    api_key: api_key.into(),
                },
                url.into(),
            ),
            pass_language: false,
            ready_timeout: Duration::from_secs(10),
        }
    }

    pub fn supervisor(&self) -> Option<&ServeSupervisor> {
        match &self.target {
            Target::Supervised(s) => Some(s),
            Target::Fixed(..) => None,
        }
    }
}

impl LiveEngine for NemoWsLiveEngine {
    fn open(
        &self,
        job_id: JobId,
        channel: Channel,
        opts: LiveOpts,
    ) -> Result<Box<dyn LiveSession>> {
        let (url, key, lease) = match &self.target {
            Target::Supervised(s) => {
                let ep = s.ensure_ready(self.ready_timeout)?;
                (ep.realtime_url(), ep.api_key, Some(s.lease()))
            }
            Target::Fixed(ep, url) => (url.clone(), ep.api_key.clone(), None),
        };
        Ok(Box::new(WsSession::connect(
            &url,
            &key,
            job_id,
            channel,
            &opts,
            self.pass_language,
            lease,
        )?))
    }
}

#[cfg(test)]
mod tests;
