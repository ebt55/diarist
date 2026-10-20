# Plan 15: recording without headphones (echo)

Status: planned 20 Oct 2026 by Fable 5.1 from a measurement of a real 1 h 51 min recording made with speakers (section 2). Implementation by Opus 5.5 agents, one chunk at a time, each with the numbers in section 6 before it merges. Supersedes section 3f of `docs/plans/04-phase3-live-capture.md`.

## 1. The problem

Live recording transcribes two channels: the microphone (speaker `me`, no diarization unless `record.diarize_mic`) and the system-audio loopback (the other participants, diarized). With the meeting on speakers, the far end reaches the microphone acoustically. Two things go wrong:

1. Far-end speech is transcribed twice: once from the loopback with the right speaker, once from the mic as `me`, a few tens of milliseconds later.
2. The owner's own words spoken while the far end talks are recognised from a mixture, so they come out wrong or not at all.

`docs/KNOWN-ISSUES.md` says "use headphones". This plan makes a speaker recording usable without changing anything for a headphone recording.

## 2. What the measurement showed (meeting 12, 19 Oct 2026, 111 minutes)

Measured on the owner's PC (HyperX QuadCast USB mic, Realtek Digital Output to speakers) from the stored `playback.flac` (left mic, right loopback) and the two word files; scripts and raw outputs stay in the session scratchpad, nothing private is copied here.

| Quantity | Value |
|---|---|
| Mic words / loopback words (post-pass) | 14,627 / 13,551 |
| Mic words that are copies of a loopback word within ±0.8 s | 83.7 percent (chance level 4.0 percent) |
| Mic 3-word runs that repeat a loopback run | 72.6 percent (chance 0.01 percent) |
| Loopback words that reappear in the mic transcript | 90.6 percent |
| Owner's own words (non-duplicates whose neighbours are non-duplicates) | about 1,462 in 27 stretches, 10 percent of the mic transcript |
| Echo lag at signal level, on meeting time (through the clock anchors) | median 3.3 ms, p10 1.3, p90 5.5; flat over 111 minutes |
| The same on raw per-channel sample indices | 10 to 346 ms, growing linearly: the mic clock runs −51.6 ppm against the loopback (3.1 ms per minute) |
| Echo return loss (loopback level minus mic level, far end only) | median 10.3 dB (8.2 to 12.4); 9 dB early, 11 dB late |
| Owner's voice at the mic against the echo at the mic | only about 5 dB louder (−32.8 against −38.1 dBFS) |
| Normalised cross-correlation loopback→mic, 2 s windows where the loopback is active | median 0.72; 97.5 percent of windows above 0.2, 93.2 percent above 0.5 |
| Same with the mic shifted 7 s (null) | median 0.075, p99 0.21 |
| Owner's words that overlap far-end speech (double-talk) | 11.8 percent of the owner's words |

Word-level correlation (word span padded 160 ms, lags 0 to 20 ms):

| Word set | Median coefficient | Above 0.3 |
|---|---|---|
| Echo words (duplicates) | 0.74 | 99.6 percent |
| Owner's words during far-end speech | 0.06 | 7 percent |
| Null (mic shifted) | – | 0.9 percent |

Energy alone does not separate: echo sits 6 to 16 dB under the loopback while the owner's voice is only 5 dB over the echo.

The earlier live recordings (meetings 4 to 11) have a silent mic (−93 dBFS, 0 mic words), so they are a "no coupling" control for the detector but not a headphone-speech control; a headphone recording with speech is still owed (section 6).

What follows from the numbers:

- The echo is loud (return loss 10 dB) and the owner's voice barely louder than the echo, so cleaning the signal (echo cancellation) is what makes double-talk recognisable; a word gate alone can only remove duplicates.
- The lag is tiny and stable on meeting time, so a correlation gate needs a search window of only about −10 to +40 ms once the clock anchors are applied (or a slowly tracked lag on raw indices), which makes it cheap and selective.
- A correlation threshold of 0.35 drops 99 percent of echo words and about 7 percent of the owner's double-talk words (about 0.8 percent of the owner's words overall); nothing happens on a null.

## 3. Design

Three parts, each engaging only when acoustic coupling is actually detected, so a headphone recording is processed exactly as today.

### 3.1 Coupling monitor (`audio::echo`)

- Input: the 100 ms frames of both channels on meeting time (the recorder already receives per-channel frames and levels; the frames' `pts_us` are channel time, and the shared `ClockMap` maps them to meeting time). A frame pair is examined when the loopback frame is active (RMS above −45 dBFS after the AGC; the threshold comes from the measured level histogram).
- Per active pair: normalised cross-correlation of the loopback frame against the mic over lags −20 to +60 ms at first, then ±25 ms around the tracked peak lag (tracking absorbs any residual clock offset). About 1,600 × 80 multiply-adds per pair: under 0.1 percent of one core.
- State: the median coefficient over the last 20 active pairs and the tracked lag. `coupled` becomes true when the median is at least 0.35 (null baseline p99 is 0.21; echo median 0.72) and false again when it stays under 0.2 for 60 active pairs (a user who puts headphones on mid-meeting).
- Output: `EchoState { coupled, coupling, lag_ms }`, a per-frame history ring (frame index, coefficient, lag, loopback active) kept for the live gate, and one `LiveWarning::Echo` when coupling is first detected (UI: "Your speakers reach the microphone (echo). Windows echo cancellation is on" or, without it, "... Echoed words are removed after Stop; headphones would help").
- Offline: `EchoProfile::scan(mic.wav, loopback.wav, clock maps)` computes the same thing over a stored recording (2 s windows, coarse to fine) in a few seconds for two hours, so the post-pass does not depend on the live monitor having run (crash recovery, re-runs, imports of a stereo recording later).

### 3.2 Word gate (post-pass, then live)

- Post-pass: after the mic WAV is transcribed and before segmentation, if the profile says `coupled`, each mic word is scored: normalised cross-correlation between the mic and the loopback over the word's span padded by 160 ms, at the profile lag ±20 ms. Words at or above `echo.gate_threshold` (default 0.35) are dropped. The loopback transcript is never touched.
- Recorded in the run's `engine_info.echo`: `coupled`, `coupling`, `lag_ms`, `mic_words_before`, `dropped`, `threshold`, `aec` (section 3.3). One count-only log line (no text).
- Live: when `coupled` and OS echo cancellation is not active, a mic utterance's words whose frames (from the history ring) are echo frames at the tracked lag are dropped before they are stored and shown. Decision latency is zero: the frames covering a word have already been scored when its utterance arrives. The live run therefore matches what the post-pass will keep.
- Not gated: anything on the loopback; the mic when not coupled; words below the threshold (the owner talking over the far end stays, mixture or not).

### 3.3 Windows echo cancellation (OS AEC)

Windows 11 22H2 and later let a capture stream name a render endpoint as its echo reference (`IAcousticEchoCancellationControl`, wrapped by the `wasapi` crate as `is_aec_supported` / `set_echo_cancellation_render_endpoint`). The probe in `docs/research/14-echo-cancellation.md` establishes whether it is available on the owner's mic, how many dB it removes, whether it needs the Communications category (which can duck other audio) and whether it can be switched on a running stream. Decision rules, pending those numbers:

- Engage only when the monitor says `coupled` (or at once with `echo.mode = always`). With no coupling nothing changes for anyone.
- Reference endpoint: the loopback's current endpoint; follow it when the loopback follows the meeting app to another output.
- If engaging requires reopening the mic stream, do it once at detection (a `DeviceChanged { reason: Requested }` control packet and a gap anchor, as a reopen after resume does today).
- Prefer a stream category that does not duck other applications; if only Communications works, document the Sound settings switch and keep the gate as the fallback for users who turn it off.
- The gate stays on after the AEC engages: residual echo under double-talk still leaks words, and the gate's false-drop rate on the owner's words is low.
- If the probe shows no support or under 10 dB of cancellation on this machine, chunk B is replaced by an app-side canceller (partitioned frequency-domain adaptive filter on the mic with the loopback as reference; the lag is 3 ms and stable, so a 128 ms filter suffices) behind the same `coupled` switch. That is a separate plan.

### 3.4 Settings and UI

| Setting | Default | Meaning |
|---|---|---|
| `echo.mode` | `auto` | `auto`: everything above engages on detected coupling. `off`: nothing engages, nothing is gated (today's behaviour). `always`: AEC from the first frame and the gate regardless of detection, for someone who always records with speakers |
| `echo.gate_threshold` | `0.35` | Correlation at or above which a mic word is dropped; 0.2 to 0.6 accepted |

Meeting details show the echo line when coupling was detected: "Echo detected (coupling 0.72, 3 ms); N far-end words removed from "me"; Windows echo cancellation on/off". The Diagnostics "Why isn't it working" checklist gets a row: "Recording with speakers: echo handling is on/off/unsupported here".

## 4. Chunks

Each chunk is one branch and one PR with its measurement table.

### A. Monitor and post-pass gate (crates/core only)

Owns: new `crates/core/src/audio/echo.rs` (+ `mod echo` in `audio/mod.rs`); `crates/core/src/live/postpass.rs` (gate call after the mic transcription, `engine_info.echo`); `crates/core/src/live/mod.rs` (feed both channels' frames to the monitor, `LiveWarning::Echo`, store the live profile); settings defaults and the `echo.*` keys where settings live; `docs/DEV.md` settings table. Must not touch `audio/capture/*` (chunk B's territory) or anything under `apps/`.

Acceptance:
- Synthetic test: loopback = speech-like signal, mic = that signal delayed 3 ms and 10 dB down plus an independent speech-like signal with its own "words" → `coupled`, the gate drops every echo-only word and keeps every independent word with at least 0.3 margin on the coefficient. Uncorrelated channels → not coupled, 0 drops. Mic clock drift of −50 ppm injected → the tracked lag follows it.
- Replay of meeting 12 through the real post-pass (the env-gated harness `crates/core/tests/live_carry_repro.rs` already replays a stored FLAC): mic words 14,627 before; after the gate at most 2,500, duplicate rate (section 2 definition) under 3 percent, and at least 97 percent of the owner's words kept (the measurement left the word lists in the scratchpad for this comparison). Post-pass wall time within +10 s of before.
- Replay of meeting 5 (silent mic): not coupled, 0 drops, output identical to before.
- `cargo test -p diarist-core` green; `cargo clippy` clean on the new code.

### B. Windows echo cancellation (capture)

Owns: `crates/core/src/audio/capture/{mod,wasapi_impl}.rs` (AEC reference on the mic client, engage on request, follow the loopback endpoint), `crates/core/src/live/mod.rs` (engage on `coupled`, record `aec`), `docs/research/14-echo-cancellation.md` (probe numbers). Decided after the probe (section 3.3).

Acceptance: a 10-minute recording by the owner with speakers after the change (section 6, item 4): duplicate rate under 3 percent in the live run and the post-pass, the probe's attenuation figure, no ducking of the meeting unless documented.

### C. Live gate and UI

Owns: `crates/core/src/live/mod.rs` (drop echo words before store and sink when coupled and AEC is not active), `apps/desktop` (warning banner text, meeting details echo line, settings controls, diagnostics row), generated TypeScript types.

Acceptance: on the meeting 12 replay the live run's mic word count is within 10 percent of the post-pass's after-gate count; headphone replay unchanged; UI strings reviewed against docs/00's voice.

### D. Docs

`docs/KNOWN-ISSUES.md` row (from "use headphones" to what the app does and the measured residual), `CHANGELOG.md`, `docs/INSTALL.md` one sentence on recording with speakers, `docs/01-architecture.md` echo line, `docs/plans/04` section 3f pointer here.

## 5. Cost, in processing time and accuracy

| Part | Live cost | Post-pass cost | Accuracy effect |
|---|---|---|---|
| Monitor | under 0.1 percent of a core; no added word latency (runs beside the feed) | a few seconds for 2 h (offline scan) | none by itself |
| Word gate | none (decisions use frames already scored) | seconds (one correlation per mic word over ~0.5 s of audio) against the minutes the transcription takes | speaker recordings: "me" goes from 10 percent genuine to about 97 percent; about 0.8 percent of the owner's words lost under double-talk. Headphone recordings: zero change, the gate never engages |
| OS AEC | zero CPU in Diarist (runs in the audio engine); capture latency change measured by the probe, expected tens of milliseconds | none | double-talk words become recognisable to the extent of the cancellation; residual handled by the gate. Headphone recordings: not engaged |

Nothing in this plan touches file imports, so the `tools/eval` file table cannot move; it is still run once as the smoke test. The loopback channel is never modified, so the far end's accuracy is unchanged by construction.

## 6. Verification, in order

1. Unit and synthetic tests in chunk A.
2. Meeting 12 replay: the before/after table in section 4A, in the PR.
3. Meeting 5 replay and a synthetic headphone case: zero drops, identical output.
4. Owner: one 10-minute recording with speakers and one with headphones after chunk B, both with "Keep live runs" on; the duplicate rate and the owner-word retention for each go into `docs/research/14-echo-cancellation.md` section 3.
5. `tools/eval` smoke (one config, the fixed set) unchanged.
6. The live latency line the UI logs ("live latency (UI): ... lag avg") within noise of the 19 Oct figure (119 ms loopback, 147 ms mic).

## 7. Out of scope

Removing the owner's voice that comes back through the far end's microphone (the meeting app's own echo cancellation does that and the delay is hundreds of milliseconds, outside the gate's window, so it is also never confused with local echo); a room with several people on one mic (`record.diarize_mic`, unchanged); app-side echo cancellation unless the probe forces it.
