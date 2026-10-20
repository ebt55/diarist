# Plan 15: recording without headphones (echo)

Status: planned 20 Oct 2026 by Fable 5.1 from a measurement of a real 1 h 51 min recording made with speakers (section 2); revised the same day after an Opus 5.5 review that re-ran the gate over every word (section 8 holds the dispositions). Implementation by Opus 5.5 agents, one chunk at a time, each with the numbers in section 6 before it merges. Supersedes section 3f of `docs/plans/04-phase3-live-capture.md`.

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
| Mic words that are copies of a loopback word within ±0.8 s ("duplicates") | 83.7 percent (chance level of the matcher 4.0 percent) |
| Mic 3-word runs that repeat a loopback run | 72.6 percent (chance 0.01 percent) |
| Loopback words that reappear in the mic transcript | 90.6 percent |
| Misrecognised echo (mic words that are not text duplicates but sit among duplicates and correlate like echo) | about 905 |
| Owner's own words (non-duplicates whose neighbours are non-duplicates) | about 1,462 in 27 stretches, 10 percent of the mic transcript; 172 of them during far-end speech |
| Echo lag at signal level, on meeting time (through the clock anchors) | median 3.3 ms, p10 1.3, p90 5.5; flat over 111 minutes |
| The same on raw per-channel sample indices | 10 to 346 ms, growing linearly: the mic clock runs −51.6 ppm against the loopback (3.1 ms per minute) |
| Mic ASR stamps echo words late against the loopback word | +75 ms median, +275 ms at p90 |
| Echo return loss (loopback level minus mic level, far end only) | median 10.3 dB (8.2 to 12.4); 9 dB early, 11 dB late |
| Owner's voice at the mic against the echo at the mic | only about 5 dB louder (−32.8 against −38.1 dBFS) |
| Normalised cross-correlation loopback→mic, 2 s windows where the loopback is active | median 0.72; 97.5 percent of windows above 0.2, 93.2 percent above 0.5 |
| Same with the mic shifted 7 s (null), 2 s windows | median 0.075, p99 0.21 |
| Same statistic on 100 ms frames (review) | echo median 0.76; null median 0.195, p95 0.46, 13.5 percent at or above 0.35 |
| Lag consistency on 100 ms frames (peak within ±3 ms of the tracked lag) | 80 percent of echo frames, 6.1 percent of null frames |
| Owner's words that overlap far-end speech (double-talk) | 11.8 percent of the owner's words |

Word-level correlation (word span padded, lags at the tracked lag ±20 ms), over all 14,627 mic words:

| Padding before / after | Duplicates dropped | Duplicate rate among kept words | Owner's words kept | Owner's words kept during far-end speech |
|---|---|---|---|---|
| 160 / 160 ms, threshold 0.35 | 97.85 percent | 14.1 percent | 99.2 percent | 93.6 percent |
| 350 / 160 ms, threshold 0.35 | – | 6.6 percent | 98.6 percent | 95.3 percent |
| 500 / 200 ms, threshold 0.35 | – | 4.6 percent | 98.2 percent | – |

Neighbour hysteresis (drop at or above 0.5, keep under 0.2, otherwise the median of the two neighbours on each side at or above 0.35) takes duplicates dropped from 97.85 to 98.35 percent and false drops during far-end speech from 6.4 to 4.65 percent at 160/160. An energy veto (keep when mic minus loopback exceeds −3 dB) loses duplicates without helping the owner's words.

The earlier live recordings (meetings 4 to 11) have a silent mic (−93 dBFS, 0 mic words), so they are a "no coupling" control for the detector but not a headphone-speech control; a headphone recording with speech is owed before chunk A merges (section 6).

What follows from the numbers:

- The echo is loud (return loss 10 dB) and the owner's voice barely louder than the echo, so cleaning the signal (echo cancellation) is what makes double-talk recognisable; a word gate alone can only remove duplicates.
- The lag is tiny and stable on meeting time, so a correlation gate needs a search window of only a few tens of milliseconds once the clock anchors are applied, which makes it cheap and selective. On raw per-channel indices the lag drifts 3 ms per minute, so anything working on raw indices must track it per window.
- The echo words are stamped late, so the correlation window must reach back before the word's start (350 ms) more than after it (160 ms).
- Detection must be decided on 2 s blocks with a lag-consistency check: on 100 ms frames the null reaches 0.35 one frame in seven and a frame-level state machine engages falsely within minutes.

## 3. Design

Three parts. The monitor runs on every recording; the gate and the canceller engage only when acoustic coupling is actually detected, so a headphone recording is processed exactly as today.

### 3.1 Coupling monitor (`audio::echo`)

- Input: a non-blocking tap on each channel's pipeline before the engine feed (the recorder's event loop receives only levels today, and the feed drops frames when degraded, so neither can carry the PCM). The tap copies each 100 ms frame into a 20 s PCM ring per channel, placed on meeting time through the shared `ClockMap` (channel `pts_us` mapped; both maps exist once each channel has its first anchor).
- Detection works on 2 s blocks (the unit the thresholds were measured on): when the loopback block RMS is at or above −45 dBFS after the AGC (from the measured level histogram), compute the normalised cross-correlation of the loopback block against the mic by FFT, at sample resolution, over causal lags −5 to +60 ms at first (negative lags are excluded because Windows "Listen to this device" or sidetone can put the owner's own voice on the loopback, where the mic would lead; a gate on those lags would drop the owner's words), then ±25 ms around the tracked lag.
- State: the median coefficient over the last 10 active blocks and a lag-consistency count (blocks whose peak lies within ±3 ms of the tracked lag). `coupled` becomes true when the median is at least 0.35 and at least 7 of the last 10 active blocks are lag-consistent (null: p99 0.21 and 6 percent consistency); it becomes false when the median stays under 0.2 for 30 active blocks (a user who puts headphones on mid-meeting). With no active block yet the state is `unknown`, shown as such, never as "no echo".
- Re-acquisition: on a `DeviceChanged` on either channel the lag search widens to −5 to +500 ms for the next blocks (Bluetooth and HDMI outputs add 100 to 300 ms of device latency after the loopback tap) and the lag track restarts. The lag is tracked per block, not fixed per meeting.
- Also estimated while coupled, per coupled block and summarised as the median: the echo return loss `erl_db = (loopback_dB − loopback_gain_db) − (mic_dB − mic_gain_db) − 20·log10(coefficient)`, with the AGC gains taken from the frames (`Frame::gain_db`) so that a mic AGC turned on for a quiet headset does not make a 35 dB leak read as 15 dB, and a loopback AGC raising a quiet far end does not inflate the figure. The coefficient term isolates the echo's share of the mic power (about coefficient² of it), so no "blocks without mic words" condition is needed (only 59 of 1,762 far-end blocks in meeting 12 had none, because the echo itself produces mic words). It feeds the canceller's engagement rule (section 3.3).
- Output: `EchoState { status: unknown | clear | coupled, coupling, lag_ms, erl_db }`, one `LiveWarning::Echo { coupling, lag_ms, aec }` when coupling is first detected. UI banner: "Sound from the meeting reaches your microphone. Windows echo cancellation is on" or, without it, "... Echoed words are removed from "me"; headphones would help", with a "Turn off for this recording" action (chunk C).
- Offline: `EchoProfile::scan(mic.wav, loopback.wav, clock maps)` computes the same thing over a stored recording (2 s blocks, FFT correlation) in a few seconds for two hours, so the post-pass does not depend on the live monitor having run (crash recovery, re-runs, re-processing). It returns the per-block lag track, not one lag.
- Cost: an FFT correlation of a 2 s block (about 32k points at 16 kHz, done as a 4,096-point block convolution) is well under 1 percent of a core; direct correlation at sample resolution would be 1 to 2 percent and is not used.

### 3.2 Word gate (post-pass and live, one function)

- `gate_words`: for each mic word, the normalised cross-correlation (FFT) between the mic and the loopback over [start − 350 ms, end + 160 ms] at the tracked lag for that time ±20 ms, all at sample resolution. Decision with hysteresis: drop at or above 0.5; keep under 0.2; between, drop when the median of the two neighbouring words on each side is at or above `echo.gate_threshold` (0.35). A word whose loopback span is all zeros (coefficient undefined) is kept. The loopback transcript is never touched.
- Post-pass: after the mic WAV is transcribed and before segmentation, when the profile says `coupled`, or when `engine_info.echo.aec` is set for the recording (the canceller's output reads as "not coupled" by construction, and its residual still leaks words), or when `echo.mode` is `always`. Dropped words are not lost: `engine_info.echo` keeps the counts and parameters `{ coupled, coupling, lag_ms, erl_db, mic_words_before, dropped, threshold, aec, mode }`, and the words themselves `[{start_ms, end_ms, text, coefficient}]` go to a new nullable JSON column `echo_dropped` on the runs table (one `ALTER TABLE ADD COLUMN` migration; `Store::echo_dropped(run)` / `set_echo_dropped`, both stores, conformance case). They are not in `engine_info` because `get_meeting` returns the current run's whole `engine_info` to the UI on every rename and edit, and 12,000 entries would roughly double a two-hour meeting's reload. Only the details line and the "Restore N removed words" action (chunk C, puts them back as `me`) read the column; search and export never see echo copies, whose text is already in the loopback transcript. One count-only log line.
- Live: every mic word is stored in `live_utterances` as today; the gate runs at the sink (what the live view shows) and in `materialise_live_run`, so the live run that carry-over and the `on_issues` promotion use is gated the same way. Each live mic word is scored from the 20 s PCM ring with the same function once per revision; a word not yet scorable (its padded span has not arrived) is shown and re-scored on the next revision or at the next frame. Live and post-pass therefore agree by construction, and the gate is one shared function so the replay harness can exercise it.
- Not gated: anything on the loopback; the mic when not coupled; words below the threshold (the owner talking over the far end stays, mixture or not).

### 3.3 Windows echo cancellation (OS AEC)

Windows 11 22H2 and later let a capture stream name a render endpoint as its echo reference (`IAcousticEchoCancellationControl`, wrapped by the `wasapi` crate as `is_aec_supported` / `set_echo_cancellation_render_endpoint`). The probe in `docs/research/14-echo-cancellation.md` establishes whether it is available on the owner's mic, how many dB it removes, whether it needs the Communications category (which can duck other audio) and whether it can be switched on a running stream. Decision rules, pending those numbers:

- Engage only when the monitor says `coupled` and the estimated echo return loss is under 20 dB (meeting 12: 10 dB; a leaky headset at 30 to 40 dB engages the gate but never the canceller). `echo.mode = always` engages it from the first frame. With no coupling nothing changes for anyone: the canceller's category DSP (noise suppression, gain, nonlinear suppression) must never touch a headphone user's mic, and `tools/eval` cannot see that regression.
- Latched: once engaged it stays on for the recording. The monitor reads the canceller's output, so after convergence it would otherwise read "clear" and switch the canceller off, the echo would return, and every reopen is a gap. It is released only by a device change (then re-decided) or by the user's "Turn off for this recording".
- Preferred shape, if the probe shows a second client on the same endpoint works: keep the raw mic client for the archive and the monitor, open a second client on the mic with the reference set for the engine feed, and also write its output as `mic-aec.wav`, which the post-pass transcribes when present (deleted with the other WAVs after the post-pass; re-runs fall back to the raw mic plus the gate). If only one client can exist, reopen the mic with the reference once at engagement (a `DeviceChanged { reason: Requested }` control packet and a gap anchor, as a reopen after resume does today) and accept that the archive is the cancelled signal.
- Reference endpoint: the loopback's current endpoint; follow it when the loopback follows the meeting app to another output.
- Never auto-engage a stream category that ducks other applications. If the probe shows only Communications works, `auto` uses the gate only and the canceller is opt-in through `echo.mode = always`, documented with the Sound settings switch.
- The gate stays on after the canceller engages (section 3.2).
- If the probe shows no support or under 10 dB of cancellation on this machine, chunk B is replaced by an app-side canceller (partitioned frequency-domain adaptive filter on the mic with the loopback as reference; the lag is 3 ms and stable, so a 128 ms filter suffices) behind the same `coupled` switch. That is a separate plan.

### 3.4 Settings and UI

| Setting | Default | Meaning |
|---|---|---|
| `echo.mode` | `auto` | `auto`: everything above engages on detected coupling. `off`: nothing engages, nothing is gated (today's behaviour). `always`: canceller from the first frame and the gate regardless of detection, for someone who always records with speakers |
| `echo.gate_threshold` | `0.35` | Correlation at or above which a mic word in the hysteresis band is dropped; 0.2 to 0.6 accepted |

Meeting details show the echo line when coupling was detected: "Echo detected (coupling 0.72, 3 ms); N far-end words removed from "me" (Restore); Windows echo cancellation on/off". The Diagnostics "Why isn't it working" checklist gets a row: "Recording with speakers: echo handling is on/off/unsupported here". The live warning text is added in the same chunk as the warning kind (the banner falls through to raw JSON for an unknown kind).

## 4. Chunks

Each chunk is one branch and one PR with its measurement table.

### A. Monitor, gate and post-pass integration (crates/core, one UI string)

Owns: new `crates/core/src/audio/echo.rs` (+ `mod echo`); the PCM tap in `crates/core/src/audio/pipeline.rs` (non-blocking, before the feed); `crates/core/src/live/postpass.rs` (gate after the mic transcription, `engine_info.echo`); `crates/core/src/live/mod.rs` (monitor fed from both taps, `LiveWarning::Echo`, `materialise_live_run` gated, the live sink gated); settings defaults and the `echo.*` keys; `docs/DEV.md` settings table; the one warning string in the desktop app's warning text function; crates/core/tests for the new tests. Must not touch `audio/capture/*` (chunk B) or the rest of `apps/`.

Acceptance (meeting 12 replay through the real post-pass path or, equivalently, the stored word files plus the FLAC; classes as labelled by the measurement):
- Duplicates dropped at least 99 percent; misrecognised echo dropped at least 90 percent; owner's words kept at least 98 percent overall and at least 93 percent during far-end speech.
- Silent-mic control (meeting 5): status `clear` after the first active blocks, 0 drops, output identical to before.
- Synthetic double-talk fixture with ground truth (dev only, GPU): the owner's speech is stood in by public FLEURS English utterances (already fetched by `tools/eval`, transcripts known) placed at random times over a loopback meeting whose echo path is the one estimated from meeting 12 (delay, 10 dB return loss, the room's short impulse response); run the post-pass path with the gate off and on and report "me" WER against the FLEURS transcripts, the duplicate count and the double-talk words kept. The gate must not raise "me" WER on the stood-in speech by more than 1 point absolute while removing at least 99 percent of the echo words. Method and numbers go to `docs/eval/<date>-echo.md`.
- Null false engagement: on the mic shifted 7 s, the detector never reports `coupled` over 111 minutes; on the real recording it stays `coupled` without releasing.
- Drift: with −50 ppm injected on raw indices and no clock maps, the gate still drops echo words at minute 100.
- Post-pass wall time within +10 s of before on meeting 12; the monitor's measured cost per 2 s block stated in the PR.
- `cargo test -p diarist-core` green; `cargo clippy` clean on the new code.
- Owner's headphone recording with speech (section 6, item 4) processed: status `clear`, 0 drops.

### B. Windows echo cancellation (capture)

Owns: `crates/core/src/audio/capture/{mod,wasapi_impl}.rs` (reference on the mic, second client or reopen, follow the loopback endpoint), `crates/core/src/live/mod.rs` (engage on `coupled` with the ERL rule, latch, record `aec`), `crates/core/src/live/postpass.rs` (`mic-aec.wav` when present), `docs/research/14-echo-cancellation.md` (probe numbers). Decided after the probe (section 3.3).

Acceptance: a 10-minute recording by the owner with speakers after the change: duplicate rate at chance level in the live run and the post-pass, the probe's attenuation figure, no ducking of the meeting unless documented; the headphone recording unchanged.

### C. Live view and UI

Owns: `apps/desktop` (banner with its "Turn off for this recording" action, meeting details echo line with Restore, settings controls, diagnostics row), generated TypeScript types, the replay harness extended to the shared gate.

Acceptance: on the meeting 12 replay the live run's mic word count is within 10 percent of the post-pass's after-gate count; headphone replay unchanged; UI strings reviewed against docs/00's voice.

### D. Docs

`docs/KNOWN-ISSUES.md` row (from "use headphones" to what the app does and the measured residual), `CHANGELOG.md`, `docs/INSTALL.md` one sentence on recording with speakers, `docs/01-architecture.md` echo line, `docs/plans/04` section 3f pointer here.

## 5. Cost, in processing time and accuracy

| Part | Live cost | Post-pass cost | Accuracy effect |
|---|---|---|---|
| Monitor | one FFT correlation per 2 s block, under 1 percent of a core, off the capture and feed threads; no added word latency | a few seconds for 2 h (offline scan) | none by itself |
| Word gate | one FFT correlation per mic word revision over about 0.5 s of audio, milliseconds each; a word may be shown unscored for up to one frame before its score arrives | seconds for a 2 h recording (14,627 words) against the minutes the transcription takes | speaker recordings: "me" goes from 10 percent genuine to about 94 to 96 percent (chance-level duplicates remain); about 1.4 percent of the owner's words lost overall, under 7 percent of those spoken over the far end. Headphone recordings: zero change, the gate never engages |
| OS canceller | zero CPU in Diarist (runs in the audio engine); capture latency change measured by the probe | none | double-talk words become recognisable to the extent of the cancellation; residual handled by the gate. Headphone recordings: not engaged |

Nothing in this plan touches file imports, so the `tools/eval` file table cannot move; it is still run once as the smoke test. The loopback channel is never modified, so the far end's accuracy is unchanged by construction. The echo fixture in section 4A is the quality gate plan 11 asks for on echo changes.

## 6. Verification, in order

1. Unit and synthetic tests in chunk A, including the null-engagement and drift cases.
2. Meeting 12 replay: the per-class table in section 4A, in the PR.
3. The FLEURS double-talk fixture: "me" WER gate off against on, in `docs/eval`.
4. Owner, before chunk A merges: one 5-minute recording with headphones in which they talk, with "Keep live runs" on. After chunk B: one 10-minute recording with speakers and the same with headphones. Status, drops and the duplicate rate for each go into `docs/research/14-echo-cancellation.md` section 3.
5. `tools/eval` smoke (one config, the fixed set) unchanged.
6. The live latency line the UI logs ("live latency (UI): ... lag avg") within noise of the 19 Oct figure (119 ms loopback, 147 ms mic).

## 7. Out of scope

Removing the owner's voice that comes back through the far end's microphone (the meeting app's own echo cancellation does that and the delay is hundreds of milliseconds, outside the gate's window, so it is also never confused with local echo); a room with several people on one mic (`record.diarize_mic`, unchanged); app-side echo cancellation unless the probe forces it.

## 8. Review dispositions (Opus 5.5 second opinion, 20 Oct 2026)

| Finding | Disposition |
|---|---|
| 1. ±160 ms padding leaves 14 percent duplicates; echo words are stamped late | Accepted: padding 350/160 ms, hysteresis added, acceptance restated per class |
| 2. Detector thresholds measured on 2 s windows but applied to 100 ms frames; null engages within minutes; sidetone makes the mic lead; Bluetooth/HDMI latency after a device change; drift on raw indices | Accepted in full: 2 s blocks, lag consistency 7 of 10, causal lags only, wide re-acquire on device change, per-block lag track, drift test asserts a drop at minute 100, `unknown` state shown |
| 3. The canceller's output feeds its own detector and would oscillate; the offline profile reads "clear" after cancellation | Accepted: latched for the recording, released only by a device change or the user; the post-pass gates whenever `aec` is set; the live gate runs with the canceller on |
| 4. The recorder receives no frames; "zero decision latency" unfounded; per-frame history undefined; the replay harness bypasses on_feed | Accepted: a PCM tap before the feed, a 20 s ring, live words scored by the same word-level function per revision, one shared gate function |
| 5. Drops irreversible (WAVs deleted, live words deleted) | Accepted: store every live word, gate at the sink and at materialisation, keep dropped words with a Restore action; no hidden speaker. Rebuttal round: the list moves out of `engine_info` (fetched whole on every edit) into its own nullable column, counts stay in `engine_info.echo` |
| 6. Canceller on detection rather than always; add an ERL bound and a duplicate-words condition; never auto-engage a ducking category; test a second client | Accepted with one change: the ERL bound (under 20 dB) and lag consistency replace the duplicate-words condition, which would make engagement depend on live ASR timing. Rebuttal round: the ERL must be computed on pre-AGC levels with the coefficient isolating the echo power (a mic AGC at +15 dB would otherwise make a 35 dB headset leak pass the bound), which also removes the "blocks without mic words" condition. Ducking rule and second-client probe accepted |
| 7. Acceptance numbers below the matcher's chance level; no double-talk ground truth; headphone recording scheduled too late | Accepted: per-class targets; FLEURS double-talk fixture with "me" WER; the owner's headphone recording moves before chunk A merges |
| 8. Lag count and cost figures wrong; the post-pass gate done directly breaks the time budget | Accepted: FFT correlation at sample resolution everywhere; section 5 restated |
| 9. Neighbour hysteresis (small gain) and energy veto (loses duplicates) | Hysteresis accepted; energy veto rejected, as the reviewer also concluded |
| 10. Unknown warning kind renders JSON; banner wording and action; NaN guard | Accepted: the string ships with the kind in chunk A; wording and action in chunk C; undefined coefficient keeps the word |
