# Phase 3: live capture and live transcription

Status: draft. Chunk 3c (live engine adapter) waits on the runtime spike. Everything else rests on docs/research/02-ui-audio-stack.md.

## Goal

Press Record: the app captures the microphone and the system render loopback, keeps them aligned on one clock, writes each as a 16 kHz mono WAV, streams both to the engine, shows words within 1.5 s with provisional speaker labels, and on Stop runs the canonical post-pass so the stored transcript is the full-context offline result with display names carried over.

## Owns

`crates/core/src/audio/{capture,pipeline,wav}.rs`, `crates/core/src/engine/live*.rs`, `crates/core/src/live/`, `crates/meet-cli` (`record`, `devices`). UI wiring is a separate chunk under `apps/desktop`.

## Chunks

### 3a Capture (`audio::capture`)
- `wasapi` crate. `MicCapture`: default communications capture endpoint, shared mode, event-driven, mix format. `LoopbackCapture`: default render endpoint opened with `Direction::Capture` (loopback), shared mode, event-driven, `wait_for_event(timeout)` of 200 ms so silence produces synthesized zero packets.
- Both emit `Packet { channel, qpc_100ns, flags { silent, discontinuity, timestamp_error }, frames: Vec<f32> interleaved, channels: u16, rate: u32 }` into an `rtrb` ring. Callback threads do nothing else.
- `register_notification_callback` for default-device changes; on change, the capture restarts on the new endpoint and emits a `DeviceChanged` control packet so the pipeline can log the discontinuity.
- Never process loopback (new Teams returns zeros).
- `list_devices() -> Vec<DeviceInfo { id, name, flow, is_default, is_default_comms }>`.

### 3b Pipeline (`audio::pipeline`, `audio::wav`, `audio::feed`)
- One `ChannelPipeline` per channel. Input: packets. Steps: place on a QPC timeline relative to `session_start_qpc`; zero-fill any gap (missing frames = gap duration times rate); treat `silent`-flagged packets as zeros; downmix to mono (average); bring to 16 kHz (CONTESTED point A: rubato in our thread vs WASAPI `AUTOCONVERTPCM`; implement whichever research/06 decides); chunk into 100 ms PCM16 frames with `pts_us` from the QPC timeline.
- Clock anchors: once per second record `(sample_index, qpc_100ns)` per channel into `clock_anchors`. Drift is reported in ppm for diagnostics and corrected offline by piecewise-linear mapping when interleaving; the resampler ratio is never changed live.
- WAV writer (`hound`): append PCM16, `flush()` every second, header finalized on close; on crash the header is repaired at next launch from file length. The writer runs on the pipeline thread and never waits on the engine.
- Engine feed (`audio::feed`): a bounded queue (10 s per channel) between the pipeline and the live sessions on its own thread. When full, it drops to record-only, emits a `Degraded` event for the UI banner, and resumes feeding when the engine catches up. Frames that are exactly zero get 1-LSB dither before the engine (runtime bug #48); the WAV gets the clean signal.
- Keep the system awake with `SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_CONTINUOUS)` while recording; on `WM_POWERBROADCAST` suspend, flush and anchor; on resume, reopen captures and record a gap anchor.
- Tests with synthetic packets: a 300 ms gap yields exactly 4800 zero samples at 16 kHz; a SILENT packet is zeroed; 48 kHz stereo sine comes out at the right length and frequency; anchors reproduce an injected 100 ppm drift within 10 percent; a stalled consumer makes the feed degrade and recover without the WAV losing a sample.

### 3c Live engine adapter (`engine::live_ws`), decided by the spike
- Implements `LiveEngine` / `LiveSession` from docs/03 v0.2 over `nemo-speech serve` (docs/03 "v2 live adapter", command lines and WebSocket schema in docs/research/08-runtime-cli-contract.md).
- `ServeSupervisor`: starts the server lazily at app launch with endpointing on and 2 slots, picks a free port, generates a per-launch API key passed by environment, polls `/ready`, restarts on exit with backoff, exposes `status()`.
- `WsSession`: `tokio-tungstenite` client; `session.update`, binary PCM16 frames, event parsing into `Utterance` (`utt`/`rev`), `TextFinalUntil` on `.completed`; speaker ids normalized to `spk_N`; clean close.
- Tests: an in-process fake WebSocket server replays `docs/research/fixtures/ws_session_events.jsonl` and the adapter produces the expected `EngineEvent` sequence; a GPU-gated test runs against the real server with a 30 s WAV.
- Later upgrade (not this phase): Rust FFI to the C ABI (`diar_frame_probs`) for our own live label refresh.

### 3d Live orchestrator (`live::Recorder`)
- Start: NVML free-VRAM check (refuse under 2 GB) and free-disk check; create meeting (status recording) and a `live` run; start captures and pipelines; open sessions `mic` (diarize off unless `diarize_mic`, speaker `me`) and `loopback` (diarize on); route frames through the feed.
- Preview: `Utterance` events are appended to `live_utterances` (one row per `utt`/`rev`) and forwarded through a `LiveSink` trait (`transcript:words` with `{utt, rev}` replacement semantics); the CLI prints, the desktop app forwards to the WebView. Text older than `TextFinalUntil` (or 10 s) is shown as final text; all labels are shown as provisional.
- Loopback endpoint: every 2 s check which render endpoint foreign processes are playing to (anarlog's approach); after two confirmations, reopen loopback on it and record a gap anchor. Also reopen on `AUDCLNT_E_DEVICE_INVALIDATED` and on default-device notifications.
- Stop: close sessions, finish WAVs, materialise the live run (latest `rev` per `utt` into segments and words), status processing. Post-pass through `JobQueue`: `NemoCliEngine` on `loopback.wav` (diarize) and `mic.wav` (no diarize, speaker `me`, or diarize when `diarize_mic`), map times through `clock_anchors`, `transcript::interleave`, `transcript::segment`, write a `post_pass` run, `carry_over(live_run, post_pass_run)`, set `current_run_id`, write the playback track (format per research/06 point B), delete the live run unless `keep_live_runs`. Status done.
- Crash recovery at launch: for meetings in `recording`, repair WAV headers, materialise the live run, queue the post-pass.

### 3e CLI
- `meet-cli devices`; `meet-cli record [--seconds N] [--title s]` prints live words with channel and label, Ctrl-C stops and runs the post-pass, prints a summary.

### 3f Echo (v2.1, after 3a to 3e are verified)
- If `is_aec_supported` on the mic client, set the loopback endpoint as the echo-cancellation reference and category Communications.
- Otherwise correlation gate: for each mic word, if loopback energy in [t0 - 300 ms, t1] is high and normalized cross-correlation of the two 16 kHz signals exceeds a threshold, drop the mic word (it is the far end leaking into the mic). Threshold tuned on a recording made with speakers.

## Verification

- Unit tests above. Manual: play a YouTube talk on speakers, speak into the mic for 2 minutes with `meet-cli record`; both WAVs exist, their lengths differ by under 50 ms, cross-correlation of a clap heard by both peaks at the same timeline position; live words for the mic appear within 1.5 s; post-pass transcript has `me` plus the video's speakers; renaming a preview speaker before Stop survives the post-pass.
- Record 60 minutes in the background while using the PC; no gap growth, memory flat, WAV sizes 115 MB each.

## Out of scope

UI controls (next chunk), speaker identity, our own fusion.
