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
| Echo lag at signal level, on meeting time (through the clock anchors) | median 3.3 ms, p10 1.3, p90 5.5; flat in 5-minute medians over 111 minutes. The canceller spike's finer trackers (research 14 section 2) show the lag is a sawtooth under that: it drifts at 8.8 ppm and jumps back about 5 ms every 9.45 minutes, with 40 to 85 µs of second-to-second wobble; no clock anchor shows it, so it sits in the playback or capture hardware |
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

Chunk A's results (20 Oct 2026, `docs/eval/2026-10-20-echo.md`): on meeting 12 the gate at 350/160 ms with hysteresis and threshold 0.35 drops 99.37 percent of duplicates and 96.5 percent of misrecognised echo that has far-end audio in its span, keeps 98.5 percent of the owner's words (94.8 percent during far-end speech), couples at 20 s and never releases, never couples on the 7 s null, and leaves the silent-mic controls untouched; the monitor costs 635 µs per active 2 s block, the offline scan 1.1 s for 111 minutes, the gate 90 µs per word. The FLEURS double-talk fixture (public English utterances stood in for the owner over a meeting with the measured echo path) then showed the gate's limit: it removes 99.6 percent of echo words, but "me" WER on utterances spoken over continuous far-end speech rises from 36.7 to 59.3 percent at threshold 0.35. The reason is arithmetic: the coefficient is about the square root of the echo's share of the mic power, so an echo-only word sits near 0.76 while the owner at +5 dB over the echo sits near 0.47, and a threshold under 0.5 cannot tell them apart. Decision: the operating point moves between the two populations (band swept in chunk A's follow-up, fixture WER change at most +1 point), accepting that about 1 percent more duplicates survive; the canceller (chunk B) is what recovers double-talk, by lowering the echo's share before the gate and the ASR see the frame.

What follows from the numbers:

- The echo is loud (return loss 10 dB) and the owner's voice barely louder than the echo, so cleaning the signal (echo cancellation) is what makes double-talk recognisable; a word gate alone can only remove duplicates.
- The lag is tiny and stable on meeting time, so a correlation gate needs a search window of only a few tens of milliseconds once the clock anchors are applied, which makes it cheap and selective. On raw per-channel indices the lag drifts 3 ms per minute, so anything working on raw indices must track it per window.
- The echo words are stamped late, so the correlation window must reach back before the word's start (350 ms) more than after it (160 ms).
- Detection must be decided on 2 s blocks with a lag-consistency check: on 100 ms frames the null reaches 0.35 one frame in seven and a frame-level state machine engages falsely within minutes.

## 3. Design

Three parts. The monitor runs on every recording; the gate and the canceller engage only when acoustic coupling is actually detected, so a headphone recording is processed exactly as today.

### 3.1 Coupling monitor (`audio::echo`)

- Input: a non-blocking tap on each channel's pipeline before the engine feed (the recorder's event loop receives only levels today, and the feed drops frames when degraded, so neither can carry the PCM). The tap copies each 100 ms frame into a 60 s PCM ring per channel (f32, 3.84 MB per channel; 60 s so that a long echoed utterance can still be scored on its final revision), placed on meeting time through the shared `ClockMap` (channel `pts_us` mapped; both maps exist once each channel has its first anchor).
- Detection works on 2 s blocks (the unit the thresholds were measured on): when the loopback block RMS is at or above −45 dBFS after the AGC (from the measured level histogram), compute the normalised cross-correlation of the loopback block against the mic by FFT, at sample resolution, over causal lags −5 to +60 ms for the first 5 active blocks (negative lags are excluded because Windows "Listen to this device" or sidetone can put the owner's own voice on the loopback, where the mic would lead; a gate on those lags would drop the owner's words), then −5 to +500 ms until a lag-consistent lock (so a recording that starts on a Bluetooth or HDMI output couples too), then ±25 ms around the tracked lag. One FFT yields every lag, so the window only decides which peaks count.
- State: the median coefficient over the last 10 active blocks and a lag-consistency count (blocks whose peak lies within ±3 ms of the tracked lag). `coupled` becomes true when the median is at least 0.35 and at least 7 of the last 10 active blocks are lag-consistent (null: p99 0.21 and 6 percent consistency); it becomes false when the median stays under 0.2 for 30 active blocks (a user who puts headphones on mid-meeting). With no active block yet the state is `unknown`, shown as such, never as "no echo".
- Re-acquisition: a lock missed by 3 active blocks in a row, or a `DeviceChanged` on either channel, returns the search to the wide window (Bluetooth and HDMI outputs add 100 to 300 ms of device latency after the loopback tap); it re-locks when 3 wide peaks agree (a synthetic 3 ms → 200 ms switch re-locks in 8.4 s with or without the device-change hint). The lag is tracked per block, not fixed per meeting; offline, a new lock fills the track back to the switch. Meeting 12 shows 13 benign re-locks with the lag between 0.7 and 7.2 ms throughout.
- Also estimated while coupled, per coupled block and summarised as the median: the echo return loss `erl_db = (loopback_dB − loopback_gain_db) − (mic_dB − mic_gain_db) − 20·log10(coefficient)`, with the AGC gains taken from the frames (`Frame::gain_db`) so that a mic AGC turned on for a quiet headset does not make a 35 dB leak read as 15 dB, and a loopback AGC raising a quiet far end does not inflate the figure. The coefficient term isolates the echo's share of the mic power (about coefficient² of it), so no "blocks without mic words" condition is needed (only 59 of 1,762 far-end blocks in meeting 12 had none, because the echo itself produces mic words). It feeds the canceller's engagement rule (section 3.3).
- Output: `EchoState { status: unknown | clear | coupled, coupling, lag_ms, erl_db }`, one `LiveWarning::Echo { coupling, lag_ms, aec }` when coupling is first detected. UI banner: "Sound from the meeting reaches your microphone. Windows echo cancellation is on" or, without it, "... Echoed words are removed from "me"; headphones would help", with a "Turn off for this recording" action (chunk C).
- Offline: `EchoProfile::scan(mic.wav, loopback.wav, clock maps)` computes the same thing over a stored recording (2 s blocks, FFT correlation) in a few seconds for two hours, so the post-pass does not depend on the live monitor having run (crash recovery, re-runs, re-processing). It returns the per-block lag track, not one lag.
- Cost: an FFT correlation of a 2 s block (about 32k points at 16 kHz, done as a 4,096-point block convolution) is well under 1 percent of a core; direct correlation at sample resolution would be 1 to 2 percent and is not used.

### 3.2 Word gate (post-pass and live, one function)

- `gate_words`: for each mic word, the normalised cross-correlation (FFT) between the mic and the loopback over [start − 350 ms, end + 160 ms] at the tracked lag for that time ±20 ms, all at sample resolution. Decision with hysteresis (band chosen in chunk A's sweep, `docs/eval/2026-10-20-echo.md`): drop at or above 0.60; keep under 0.20; between, drop when the median of the two neighbouring words on each side is at or above `echo.gate_threshold` (0.50). The band sits between the two measured populations (echo-only words near 0.76, the owner at +5 dB over the echo near 0.47) because the owner's words outrank residual duplicates: this band drops 98.97 percent of duplicates and lowers the double-talk fixture's "me" WER by 0.68 points, where the first band (0.20/0.35/0.50) dropped 99.37 percent and raised it by 15.9. A word whose loopback span is all zeros (coefficient undefined) is kept. The loopback transcript is never touched.
- Post-pass: after the mic WAV is transcribed and before segmentation, when the profile says `coupled`, or when `engine_info.echo.aec` is set for the recording (the canceller's output reads as "not coupled" by construction, and its residual still leaks words), or when `echo.mode` is `always`. Dropped words are not lost: `engine_info.echo` keeps the counts and parameters `{ coupled, coupling, lag_ms, erl_db, mic_words_before, dropped, threshold, aec, mode }`, and the words themselves `[{start_ms, end_ms, text, coefficient}]` go to a new nullable JSON column `echo_dropped` on the runs table (one `ALTER TABLE ADD COLUMN` migration; `Store::echo_dropped(run)` / `set_echo_dropped`, both stores, conformance case). They are not in `engine_info` because `get_meeting` returns the current run's whole `engine_info` to the UI on every rename and edit, and 12,000 entries would roughly double a two-hour meeting's reload. Only the details line and the "Restore N removed words" action (chunk C, puts them back as `me`) read the column; search and export never see echo copies, whose text is already in the loopback transcript. One count-only log line.
- Live: every mic word is stored in `live_utterances` as today; the gate runs at the sink (what the live view shows) and in `materialise_live_run`, so the live run that carry-over and the `on_issues` promotion use is gated the same way. Each live mic word is scored once from the 60 s PCM ring with the same function and keeps its score across revisions (a word whose audio has left the ring keeps the score of the word it replaced); a word not yet scorable (its padded span has not arrived) is shown and scored at the next frame; nothing is scored before the first coupling, and at that moment the earlier words still in the ring are scored once. Live and post-pass therefore agree by construction, and the gate is one shared function so the replay harness can exercise it.
- Not gated: anything on the loopback; the mic when not coupled; words below the threshold (the owner talking over the far end stays, mixture or not).

### 3.3 Echo cancellation

**Windows' own canceller is out.** Windows 11 22H2 and later let a capture stream name a render endpoint as its echo reference (`IAcousticEchoCancellationControl`). The probe (`docs/research/14-echo-cancellation.md` section 1, 20 Oct 2026, this PC) found the control offered only under the Speech-family stream categories, never with no category, Communications or GameChat; under Speech it cancelled nothing, not even on an exact digital loop through the virtual cable (output still 0.93 correlated with the reference, 11.6 dB louder, no convergence in 12 s), while its processing added about 20 dB of automatic gain (room floor from −42 to −21 dBFS) and 110 to 130 ms of delay. Its effect state is read-only, so it cannot be switched off once requested. No ducking was measured for any category. So the canceller in this plan is Diarist's own, decided by the spike in section 2 of the same research note (a partitioned-block frequency-domain adaptive filter in Rust, reference = loopback, measured offline on meeting 12 for echo removed, near-end preserved and words recovered on re-transcription). The rules below apply to it; the two-client and reopen mechanics written for the OS path fall away, since the app's canceller runs on the frames it already has.

Decision rules:

- Engage only when the monitor says `coupled` and the estimated echo return loss is under 20 dB (meeting 12: 10 dB; a leaky headset at 30 to 40 dB engages the gate but never the canceller). `echo.mode = always` engages it from the first frame. With no coupling nothing changes for anyone: the canceller's category DSP (noise suppression, gain, nonlinear suppression) must never touch a headphone user's mic, and `tools/eval` cannot see that regression.
- Latched: once engaged it stays on for the recording. The monitor reads the canceller's output, so after convergence it would otherwise read "clear" and switch the canceller off, the echo would return, and every reopen is a gap. It is released only by a device change (then re-decided) or by the user's "Turn off for this recording".
- Shape: the archive stays raw. Live, the canceller runs on the tap's frames (mic input, pre-AGC loopback reference, aligned on meeting time with a small bulk delay so lags −5 to +60 ms sit inside the filter) and its output replaces the mic frames going to the engine feed only; `mic.wav` keeps the raw signal. In the post-pass, the same filter runs over `mic.wav` with `loopback.wav` as reference (deterministic, full-file, clock maps applied) into a temporary `mic-aec.wav` that is transcribed instead of the raw mic and deleted with the other WAVs; re-runs redo it. The monitor keeps reading the raw mic, so finding 3's feedback cannot occur, but the latch stays because engagement mid-recording is a step change the live ASR sees.
- The loopback reference is whatever the loopback captures, so following the meeting app to another output needs nothing extra beyond the wide lag re-acquisition after a `DeviceChanged`.
- The gate stays on after the canceller engages (section 3.2): a linear filter leaves residual echo under double-talk and during re-convergence.
- Go/no-go for wiring it (chunk B2) was set as: on meeting 12 the spike must remove at least 15 dB of echo on far-end-only stretches, change the owner's own stretches by at most 1 dB, not diverge over 111 minutes, cost at most 5 percent of one core per 100 ms frame, and the re-transcription of the cancelled mic must keep at least 98 percent of the owner's labelled words while cutting duplicates by at least 90 percent before the gate.
- **Spike result (20 Oct 2026, research 14 section 2): go, staged.** Echo removed 12.2 dB energy-weighted (15.1 dB median per word), 14.7 dB (19.9) with mild residual suppression, where the mic's noise floor caps the measurable figure near 20 dB; owner's stretches changed 0.88 dB (0.15 dB with the loopback AGC undone); no divergence in 111 minutes; 140 to 156 µs per 100 ms frame (0.15 percent of a core). Re-transcription of the cancelled mic: duplicates 12,264 → 3,687, and → 676 with suppression (94.5 percent cut); owner's words kept 1,417 of 1,462, of which 17 missing ones were a repeated-token loop and 10 spelling variants in the raw transcript, 18 (1.2 percent) really gone; double-talk words kept 165 of 172. The decisive number: on a spliced set with ground truth (the owner's own recorded words over real far-end echo) the owner's WER falls from 17.1 to 6.7 percent. Two bars were missed narrowly (15 dB as energy; 98 percent kept against 96.9 nominal, 98.8 counting the raw transcript's artefacts), and the WER gain is the user's complaint itself, so the canceller ships, in two steps:
  1. Post-pass first (deterministic, offline, with lookahead): the filter plus suppression over `mic.wav` with `loopback.wav` as reference into a temporary `mic-aec.wav`, transcribed instead of the raw mic, then the gate. The loopback AGC's gain must be undone for the reference, so from now on each recording stores its loopback gain track (one value per 100 ms frame, about 40 KB per hour, beside the WAVs and kept with the FLAC) and the post-pass replays it backwards; recordings without a track use the post-AGC reference (the spike's 12.2 dB case).
  2. Live second, causal: a bulk delay and the monitor's per-block lag, letting the filter itself follow the sawtooth and the 8.8 ppm drift (a 5 ms step is 80 taps of a 200 ms filter; re-convergence takes a second or two and the gate covers the residual meanwhile), measured on the replay harness against the offline figures before it is turned on. The spike's trackers look ahead and are an offline best case; the "clock anchors only" row (10.7 dB, 13 dB with suppression) is the causal floor.

### 3.4 Settings and UI

| Setting | Default | Meaning |
|---|---|---|
| `echo.mode` | `auto` | `auto`: everything above engages on detected coupling. `off`: nothing engages, nothing is gated (today's behaviour). `always`: canceller from the first frame and the gate regardless of detection, for someone who always records with speakers |
| `echo.gate_threshold` | `0.50` | Correlation at or above which a mic word in the hysteresis band (0.20 to 0.60) is dropped when its neighbours' median reaches it; 0.2 to 0.6 accepted |

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

### B. Diarist's own echo canceller

B1, the spike (done 20 Oct 2026, commits ef47f78 and 7191bc6): `crates/core/src/audio/aec.rs` (the filter, residual suppression, unit tests, benchmark), `crates/core/tests/aec_spike.rs` (meeting 12 offline evaluation and re-transcription), `docs/research/14-echo-cancellation.md` section 2. Verdict: go, staged (section 3.3).

B2a, post-pass canceller: `crates/core/src/live/postpass.rs` (`mic-aec.wav` from the raw WAVs with the gain track replayed backwards, transcribed instead of the mic, deleted with the other WAVs; `engine_info.echo.aec = true` and the measured ERLE), `crates/core/src/audio/pipeline.rs` (write the loopback gain track per 100 ms frame beside the WAVs), the audio-track registration so the gain track is kept with the FLAC, `rerun.rs` (re-runs redo the canceller). Engaged when the profile is `coupled` with return loss under 20 dB, or `echo.mode = always`.

B2b, live canceller: the tap consumer (cancelled mic frames replace the feed's mic frames once coupled and latched; the WAV stays raw), `crates/core/src/live/mod.rs` (engage with the ERL rule, latch, record `aec`), measured on the replay harness first. `audio/capture/*` is not touched at all any more.

Acceptance for B2a: the meeting 12 replay through the post-pass with the canceller plus the gate: owner's labelled words kept at least 98 percent (counting the raw transcript's repeated-token loops and spelling variants as kept), duplicates at the matcher's chance level, the spliced double-talk set's WER at most 8 percent, the FLEURS fixture's "me" WER lower than with the gate alone; a 10-minute recording by the owner with speakers after the change (section 6, item 4); the headphone recording unchanged. For B2b additionally: live ERLE within 3 dB of the offline figure on the replay, live latency within noise.

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
| Diarist's canceller (only on the spike's go) | a frequency-domain adaptive filter per 100 ms mic frame, budget 5 percent of one core, measured by the spike; the mic feed is delayed by the bulk delay (tens of milliseconds) so live words on the mic appear that much later | one pass over the two WAVs before the mic transcription, well under a minute for 2 h | double-talk words become recognisable to the extent of the cancellation (go/no-go asks for 15 dB on real audio); residual handled by the gate. Headphone recordings: not engaged |

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
