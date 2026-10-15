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

### 3b Pipeline (`audio::pipeline`, `audio::wav`)
- One `ChannelPipeline` per channel. Input: packets. Steps: place on a QPC timeline relative to `session_start_qpc`; zero-fill any gap (missing frames = gap duration times rate); treat `silent`-flagged packets as zeros; downmix to mono (average); resample to 16 kHz with rubato `Fft` when the ratio is an integer (48 kHz), else `Async` sinc; chunk into 100 ms PCM16 frames with `pts_us` from the QPC timeline.
- Drift tracker: compares frames produced with QPC elapsed, reports ppm; if |ppm| > 50, adjust with `Slip`. Log once per minute.
- WAV writer (`hound`): append PCM16, flush every second, header finalized on close; on crash the header is repaired at next launch from file length.
- Engine feed gets 1-LSB dither on frames that are exactly zero (runtime bug #48). The WAV gets the clean signal.
- Tests with synthetic packets: a 300 ms gap yields exactly 4800 zero samples at 16 kHz; a SILENT packet is zeroed; 48 kHz stereo sine resamples to the right length and frequency; drift tracker reports an injected 100 ppm offset within 10 percent.

### 3c Live engine adapter (`engine::live`) PENDING spike
- Interface fixed now: `trait LiveEngine { fn open(&self, channel: Channel, diarize: bool) -> Result<Box<dyn LiveSession>> }`, `trait LiveSession { fn push(&mut self, pcm16: &[i16], pts_us: u64); fn events(&mut self) -> &mut Receiver<EngineEvent>; fn close(self) }`.
- Implementation chosen from the spike: WebSocket to `nemo-speech serve` (one long-lived process, two sessions) or Rust FFI to the C ABI in a `meet-engine` sidecar. Either way the adapter emits protocol `words` (final false), `relabel` if available, and `finalized` when the runtime commits (or after a fixed horizon of 10 s if it never does).

### 3d Live orchestrator (`live::Recorder`)
- Start: create meeting (status recording), start captures and pipelines, open sessions `mic` (diarize off, speaker `me`) and `loopback` (diarize on), route frames.
- Preview buffer: words keyed by (channel, t0); `words` events upsert; `relabel` remaps labels in the window; `finalized` moves words older than `up_to_t` into a "committed preview" list. Every 2 s the buffer is written to `live_checkpoints`. A `LiveSink` trait receives UI events (`transcript:words`, `transcript:relabel`); the CLI prints, the desktop app forwards to the WebView.
- Stop: close sessions, finish WAVs, status processing, then post-pass: `NemoCliEngine` on `loopback.wav` (diarize) and `mic.wav` (no diarize, speaker `me`), `transcript::interleave`, `transcript::segment`, replace in one transaction. Speaker carry-over: build the overlap matrix between preview labels and post-pass labels on the loopback channel (seconds of speech both assign to the same time), greedy match highest first, copy `display_name` and `color`. Clear the checkpoint. Status done.
- Crash recovery at launch: for meetings in `recording`, repair WAV headers, replay checkpoint into segments/words marked recovered, then run the post-pass.

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
