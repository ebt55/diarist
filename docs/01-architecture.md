# Architecture decision record

Status: v0.7, 15 Oct 2026 (Phase 5: install model, layout, runtime and model management updated). Decided by Fable 5.1 from research in `docs/research/`, revised after the Opus review (research/05), the GPU spike (research/03) and the adjudication (research/06). No open contested points; the Opus reviewer's concurrence round is recorded in research/05.

## Decisions at a glance

| Area | Decision | Why (short) |
|---|---|---|
| Inference runtime | NVIDIA NeMo-Speech.cpp v0.2.0, prebuilt Windows CUDA zip, portable folder | Only shipped runtime with Nemotron 3 Diarization (8 spk) plus word-level speaker tags; self-contained DLLs, no CUDA toolkit for users; NVIDIA-maintained |
| Diarizer | Nemotron 3 Diarization q8_0 (streaming Sortformer v3) | Tops the only independent cross-system benchmark (VoiceArena DER 14.7 vs 19 to 31); streaming and offline in one checkpoint |
| Diarizer config | Streaming preset (`v3-streaming`, the runtime default) for live and the standalone `diarize` pass; file mode and the post-pass run `v3-offline` (the same streaming algorithm with 21 s chunks, the manifest's `file_preset`). The stateless `diarize --offline` pass is not used: on the spike it scored worse (DER 43 vs 24 on an AMI clip) and relabelled speakers mid-clip. Retune onset/offset/min-duration on our eval set (defaults are V2 CALLHOME-tuned) | Measured, not assumed (research/03 D). Re-test offline on each runtime release |
| ASR, both modes | One streaming model resident for live and file, selected per role in Settings. Default for English: Nemotron Speech Streaming EN 0.6B q8_0. Recommended for other or mixed languages: Nemotron 3.5 ASR Streaming 0.6B q8_0 (40+ locales, auto-detect, 30 percent less VRAM, no drops on long files). File mode runs `transcribe --stream --endpointing --diarize`, with `--language` when the selected model is 3.5. File jobs and the post-pass run at the model's maximum decoder lookahead (`--asr.streaming.rnnt_right_context 13`, 1.12 s, per model in the manifest), live at the runtime's low-latency setting | Measured (docs/eval, 15 Oct 2026): EN is 0.1 to 1.7 cpWER points better than 3.5 on the four English Zoom meetings and 3.2 points better on AMI, with identical speaker attribution. Both models cover the 77-minute recording without drops and tag speakers at 99.9 percent agreement with the standalone diarizer |
| ASR, experimental | Parakeet TDT 0.6B v3: not downloaded by default, offered only behind an "experimental" setting with a warning | The silent drops are in the model, not the runtime: a 2-minute cut around the gap still loses the same 46 s (research/08 section 8; transcribe.cpp issue #71). No chunk length is safe. Its punctuation and casing are the only reason to keep it reachable; a gap-filled dual pass is v1.5 work at the earliest |
| Punctuation | Segments split at endpoints and pauses give structure; Nemotron 3.5 punctuates about 2.5 percent of words. Punctuation restoration as a CPU post-step (candidate: sherpa-onnx's CT-Transformer punctuation model, to verify) is planned for v1.5 | Readability without trusting a model that drops speech |
| Engine integration | Child process behind Rust traits `Engine` / `LiveEngine` (docs/03); no wire format unless an FFI sidecar is built | Crash isolation; runtime churn stays behind one adapter |
| v1 file adapter | `NemoCliEngine`: one `nemo-speech.exe transcribe --stream --endpointing --diarize --json` with explicit model paths (research/08 section 1). Optional standalone `diarize` pass (RTTM and JSON kept in the meeting folder) when the eval harness or a setting asks for it | Never plain `transcribe --diarize` without `--stream --endpointing` (first ten minutes mislabelled); never rely on implicit model names (they trigger downloads) |
| Long files | Verified: 77 min and 2 h files run without truncation or positional errors on the Nemotron path and on `diarize`. Known limit: at most 8 labels per stream for the whole recording; past that, labels are reused for different people. The UI warns when a recording exceeds 3 h or when all 8 labels are in use | research/03 follow-up |
| v2 live adapter | `nemo-speech serve` WebSocket, launched with `--asr.endpointing.enable=true` and `state_arena_slots 2` (4.6 to 5.9 s to ready; 2.6 to 3.2 s measured in Phase 3.5 and 5); two sessions (mic, loopback). Started when the record sheet opens or Ctrl+Alt+R starts a recording (setting `engine.preload` starts it at launch), stopped after `engine.idle_minutes` (10) without a recording. C ABI via FFI is the later upgrade because `diar_frame_probs` exposes 10 ms speaker probabilities | Spike: bugs #48/#49 did not reproduce, two sessions keep up in real time, live tags 99.5 percent correct over 90 s |
| Live labels | Live text is a complete transcript, saved as the live run. Labels are provisional (never revised by the runtime). The post-pass re-runs the same model over the clean recorded channels when recording stops; display names carried over by anchor and overlap matching. Setting `post_pass_mode`: `always` (default) or `on_issues` (only when the live session reported degraded feed, engine restarts or gaps) | Live and file mode share one model, so the post-pass is a safety net and the slot for better fusion or models, not a requirement; default stays `always` until the eval harness shows live and post-pass agree |
| Transcript runs | Every transcript is a `transcript_run`; `meetings.current_run_id` selects the shown one; runs are never overwritten | Live, post-pass, re-diarization and fusion experiments coexist; edits survive |
| Audio capture | `wasapi` crate for mic and render-endpoint loopback, both opened at 16 kHz mono PCM16 with `AUTOCONVERTPCM \| SRC_DEFAULT_QUALITY`; polling with a timeout (loopback events are unreliable when nothing renders); QPC anchors per second; zero-fill; follow the endpoint foreign apps render to (anarlog's trick) from v2.0; reopen on `DEVICE_INVALIDATED`; no process loopback | Process loopback returns zeros for new Teams; the default render endpoint is often not where Teams plays; a fixed 16 kHz published rate survives endpoint and format switches (research/06 A) |
| Capture resampling | None in the capture path: Windows converts to 16 kHz. Setting `capture.autoconvert = false` falls back to mix format plus rubato for a bad OEM converter. rubato is used only for file import | Simplicity and robustness; loopback audio has already been through meeting-app codecs, so converter filter shape does not decide WER |
| Alignment | QPC anchors `(sample_index, qpc)` per channel per second stored in the DB; word times mapped piecewise-linearly at interleave time; drift fixed offline, never by steering the resampler live | Testable, deterministic, no real-time ratio changes. Measured (research/03 follow-up, Phase 3a): channels within 3 ms, correlation 0.998 |
| Levels | Slow automatic gain on the loopback channel (peaks toward -20 dBFS, ceiling +40 dB, limiter, no gain on silence; gain falls up to 40 dB/s, rises up to 8 dB/s) applied before both the WAV and the engine feed; AGC channels capture 32-bit float so quiet audio keeps its resolution; mic AGC off by default (`capture.mic_agc`); `LoopbackQuiet` warning names the device when even the ceiling is not enough | Some outputs (the development machine's Realtek Digital Output) apply the Windows volume in software, so loopback at low volume arrives about 40 dB down. Measured: first live word 31 s late without AGC, 0.93 s with it; source correlation 0.68 to 1.000 |
| Channels | Mic = "me" (ASR only) unless `diarize_mic` is set for in-room meetings. Loopback = "them" (ASR + diarization). Two engine sessions | Speaker separation for the user is exact and free; diarizer sees fewer speakers |
| Capture robustness | WAV writer never waits on inference; engine feed has a bounded queue and degrades to record-only with a UI banner on stalls and on hard `Oom` errors alike; NVML free-VRAM check before opening sessions; `JobQueue` defers Parakeet file jobs while a recording is active; `SetThreadExecutionState(ES_SYSTEM_REQUIRED)` while recording; suspend flushes and resume reopens with a logged gap; single-instance plugin | An engine stall or VRAM contention must never lose audio; NVIDIA's sysmem fallback turns some OOMs into slowdowns but is bounded, so both paths are handled |
| VRAM budget (measured) | Live with 3.5: about 1.05 GB idle, 1.26 GB with two sessions, transient spikes of 0.5 to 0.6 GB per session, so budget 2 GB. File pass: 1.82 GB peak with 3.5 (2.6 GB EN, 4.9 GB Parakeet). The queue rule defers file jobs while recording, so steady state stays under 2.5 GB | research/03 and research/08 section 7 |
| UI shell | Tauri 2.12.x, NSIS installer, WebView2 download bootstrapper, hidden window until first paint | Smallest installer with a rich transcript view; sidecar and tray support |
| Frontend | Svelte 5 + TypeScript + virtua; TypeScript types generated from Rust with ts-rs | Fine-grained reactivity for row-level relabels; one source of truth for types |
| Core/UI seam | `core::views`, `trait Store` (with `MemoryStore` and `SqliteStore`), `core::pipeline::import`, `JobQueue` (one GPU job at a time) frozen in Phase 0.5 before Phases 1 and 2 | Lets core and UI proceed in parallel without touching the same files |
| Database | rusqlite `bundled` (FTS5 on), WAL, INTEGER primary keys, FTS `content_rowid` on the integer id, tokenizer `unicode61 remove_diacritics 2`, backups via the backup API or `VACUUM INTO` a temp file then rename | Stable across VACUUM; fast at tens of millions of words; language-neutral search |
| Recording storage | Per-channel 16 kHz mono WAV during recording with periodic `flush()`. After the post-pass: one stereo FLAC (left mic, right loopback) encoded with the `flac-codec` crate, registered as the `playback` track with each channel's length, WAVs deleted (lossless). "Process again" decodes it back into the two WAVs for a new post-pass run and deletes them after; nothing ever writes a registered FLAC. Imports keep the original file as the playback source and delete the 16 kHz intermediate. Retention default 30 days, pin to keep. Free-space check before recording or import | FLAC is 30 to 40 percent of PCM on this material (about 90 MB per stereo hour); pure-Rust encoders are mature; symphonia already decodes it; no Media Foundation (research/06 B) |
| Audio decode | symphonia (AAC-LC, MP3, FLAC, WAV, Ogg/Vorbis), then Windows Media Foundation `IMFSourceReader`, then `ffmpeg` if on PATH, then an error that shows the conversion command. Phase 1 test decodes all 23 eval files | All eval files are AAC-LC 48 kHz stereo (symphonia rates AAC-LC "Great"); HE-AAC and Opus need the fallbacks |
| Playback in UI | Tauri asset protocol with Range requests (206, 1000 kB chunks); asset scope set at runtime from the data root; `<audio crossorigin="anonymous">` routed through a Web Audio gain matrix (mono by default, solo-channel toggle) so hard-panned me/them is not forced on headphones, then a level gain: the waveform's 99th-percentile bucket peak raised to -1 dBFS (at most +24 dB, `playback.normalize`, default on) times the player's volume (`playback.volume`), with a limiter that works only when that gain pushes the loudest bucket past -1 dBFS; waveform peaks precomputed in Rust, never by fetching the file | Seeking works; the page and the asset are different local origins, so Web Audio needs CORS; fetching a file whole would load 690 MB of floats |
| Install layout | Install dir stays Tauri's NSIS default (`%LOCALAPPDATA%\Diarist`, user-changeable on the directory page). Data root defaults to Tauri's `app_local_data_dir()` = `%LOCALAPPDATA%\app.diarist.desktop` (db, audio, engine, models, logs, the WebView2 profile and the window state, all resolved before the app starts so they follow a moved root; nothing in Roaming except an empty folder the window-state plugin creates), relocatable from Settings (copy, verify, registry, restart, old copy deleted at the next launch), with the chosen path in `HKCU\Software\Diarist\DataRoot`; portable mode `<exe dir>\data`. The uninstaller's "delete app data" checkbox (off by default, hidden during updates) wipes everything; a POSTUNINSTALL hook otherwise offers to remove engine and models while keeping transcripts and recordings | No forked NSIS template; Tauri's path APIs and the uninstaller agree on one folder (research/06 C) |
| Install model | 7 MB installer (6.66 MiB measured); first run downloads the runtime package for the GPU (CUDA 98 MB, Vulkan 19 MB or CPU 6 MB; preselected from NVML and DXGI, the user can override) with our downloader (SHA-256 from the manifest, rustls with `rustls-platform-verifier`, Range resume, requests of at most two minutes so a dead connection ends as stalled), then the two default models with the runtime's own `nemo-speech pull` (System32 `curl.exe`) into `NEMO_SPEECH_MODEL_DIR` under the data root, SHA-checked by the runtime and by us: Nemotron 3 Diarization (102 MB) and the EN model (668 MB), 770 MB. Nemotron 3.5 (742 MB) and experimental Parakeet (681 MB) are opt-in from Settings. Every item shows URL, hash, size and folder with Retry and Copy link; files placed by hand are hash-checked on Refresh. Nothing is a gate (docs/00, "nobody fights the app"): Import and Record check at click time and offer the fix | ComfyUI-style: no Python, no torch, no CUDA toolkit; one ASR model serves live and file; 870 MB default download, 2.3 GB with every option |
| Model management (ComfyUI-style) | One `models\` tree under the data root. The app scans it recursively at launch and on Refresh, reads each GGUF header (`general.architecture`, name, quantization, file size) and classifies the file by role (ASR, diarizer, later embedding, punctuation, LLM) from a small architecture table; unknown architectures are listed as "unrecognised" with the header's name. Settings shows every discovered model per role with size, measured or estimated VRAM, and a fit mark against free VRAM; the user picks one per role. Downloads (first run, `nemo-speech pull`, "Download recommended", "Add from URL" into `models\user\`) land in the same tree; `models.user.json` in the data root adds recommended entries. A `<file>.meet.json` sidecar can override role and capabilities when detection is wrong; an unrecognised file can still be selected for a role, with a warning | Drop a new NVIDIA GGUF in the folder and use it the same minute, no app release. The runtime decides at load time whether it can run the file; its error is shown verbatim. Role subfolders are a convention for humans, not a requirement, because the header already says what the file is |
| Runtime management | `engine\<name>-<version>\` folders are scanned the same way; the app runs `nemo-speech --json doctor` on each, lists versions in Settings, defaults to the newest it has fixtures for, and lets the user pick or drop in a newer zip or extracted folder. "Check for newer runtime" lists the NeMo-Speech.cpp GitHub releases (unauthenticated API, on demand only) and installs a package into `engine\<name>\` with the release's `.sha256`. Versions outside the tested range work with a warning | Runtime updates are a folder drop, like a ComfyUI custom node; the adapter contract is re-verified against fixtures per tested version |
| Code signing | Plan SignPath's free OSS programme before public release; document Smart App Control behaviour for the unsigned runtime | nemo-speech.exe and its DLLs are unsigned |
| Licence | MIT for our code | Runtime Apache-2.0; models OpenMDW / NVIDIA Open Model Licence / CC-BY-4.0, downloaded by the user at first run |
| Word-to-speaker fusion | v1: the runtime's tags from the streaming fused pass (99.6 to 99.9 percent agreement with the standalone diarizer). File jobs and the post-pass tag with the diarizer's offline geometry (`--asr.diar.preset v3-offline`), which cut wrong-speaker words near a change from 8.2 to 5.5 percent on a dense meeting and from 41.2 to 18.0 on an in-room recording (docs/research/12 section 2); live keeps the streaming geometry. Our `fusion::assign_words` (start-anchored window [t0 - 40 ms, min(t1, t0 + 160 ms)] over diarize segments, nearest segment otherwise) exists for the experimental Parakeet path and for re-fusion. v1.5: Viterbi smoothing with cheaper switches at pauses and punctuation, and 10 ms probabilities from the C ABI `diar_frame_probs`, as a new run kind measured against v1 | Baseline (docs/eval): WDER 0.2 to 0.6 percent overall on the user's meetings, but 7 to 9 percent within 1 s of a speaker change, and one 11-word speaker folded into another by the runtime's tagging although the standalone diarizer found it. Those two numbers are the v1.5 targets |
| Cross-meeting identity | v3. TitaNet-L (CC-BY-4.0, EER 0.66) via sherpa-onnx CPU; A/B WeSpeaker ResNet293-LM. Voiceprint = mean of embeddings from 2 to 6 s single-speaker crops; cosine match; user confirms | Not needed for the transcript itself; CPU inference keeps VRAM for ASR |
| Research upgrade path | Diarization-conditioned ASR (NVIDIA SSA-v2, cpWER 15.9 vs cascade 42.3) when released or ported; DiCoW v3.3 as offline ceiling | Cascades lose about 2x cpWER to conditioned ASR on overlap and boundaries; nothing shippable today on a GGUF runtime |
| Toolchain | Rust stable; agents may run `rustup update stable` when a dependency requires it and must report it | 2026 crates may exceed the installed 1.88 |

## Process model

```
+---------------------------------------------------------------+
| diarist.exe (Tauri 2)                                     |
|  WebView2: Svelte UI  <-- Tauri events/commands -->           |
|  Rust backend (crates/core)                                   |
|    audio::capture   wasapi mic + loopback -> rtrb rings       |
|    audio::pipeline  QPC anchors, zero-fill, (resample), WAV   |
|    audio::feed      bounded queue -> live engine sessions     |
|    engine::*        Engine / LiveEngine adapters, JobQueue    |
|    store::Sqlite    rusqlite, migrations, FTS, runs, queries  |
|    pipeline         import, post-pass, carry-over             |
|    models           manifest, download, verify, registry      |
+----------------------------+----------------------------------+
                             | child processes
              +--------------+----------------+
              | nemo-speech.exe transcribe     |  (per file job, one at a time)
              | nemo-speech.exe serve          |  (long-lived, live) PENDING
              +--------------------------------+
```

- One Tauri process. All heavy lifting is Rust; the WebView only renders.
- Audio callbacks are real-time threads: they only copy into lock-free rings (`rtrb`). The pipeline thread drains rings, records a QPC anchor per second, zero-fills gaps and SILENT packets, writes WAV, and hands frames to the feed. The feed thread owns the engine sessions and a bounded queue; when the queue is full it drops to record-only and raises a UI banner. The writer never blocks on the engine.
- `JobQueue` runs one GPU job at a time (imports, post-passes, re-runs) and exposes cancel. Live sessions are not jobs; they hold the GPU while recording, and the post-pass is queued at Stop.
- The engine supervisor owns child processes. If one dies, in-flight jobs become `Error`, the UI is told, and the next job relaunches. A dead live session leaves recording running; the post-pass makes the transcript whole.

## Data layout on disk

```
%LOCALAPPDATA%\Diarist\              Tauri app install dir (NSIS, per-user, about 10 MB)

%LOCALAPPDATA%\app.diarist.desktop\   data root (relocatable; portable mode: <exe dir>\data)
  engine\
    nemo-speech-0.2.0-windows-x86_64-cuda\   one folder per runtime version; drop in a newer one to try it
  models\                                 scanned recursively; drop any GGUF here
    asr\                                  convention only: the app reads the GGUF header to find the role
    diarization\
    nvidia\<repo>\<rev>\...               where `nemo-speech pull` puts files (NEMO_SPEECH_MODEL_DIR points here)
  diarist.db (+ -wal)                 SQLite
  audio\<meeting_uid>\                    mic.wav, loopback.wav while recording; playback.flac after
  logs\
```

The repo ships `manifests/engine.json` and `manifests/models.json` (URLs, sha256, sizes, licences, measured VRAM) as the "recommended" list the downloader uses; they are a convenience, not a gate. Anything the scanner finds is selectable.

## File-mode flow (v1)

1. User drops a file. The decoder chain (symphonia, Media Foundation, ffmpeg) produces `audio\<uid>\source.wav` at 16 kHz mono PCM16; the original path is stored as the `original` playback track.
2. `NemoCliEngine` runs `nemo-speech.exe transcribe <wav> --model <nemotron-3.5.gguf> --language <lang> --stream --endpointing --diarize --diar-model <diar.gguf> --device cuda:0 --json --output <words.json> --force` through the `JobQueue` (the `--language` flag is omitted for `auto`, which costs about a second of the first utterance), with `NEMO_SPEECH_MODEL_DIR` set and `PATH` reduced to the engine folder plus System32. The CLI prints no progress; the UI shows elapsed time and the expected duration from the measured 27x real-time factor. The JSON is kept in the meeting folder. When the eval harness or a setting asks, a standalone `diarize --format rttm` pass is run too and kept.
3. The adapter normalizes 1-based `speaker` ints to `spk_N`, emits `Utterance` events per contiguous same-speaker stretch, then `TextFinalUntil`, `SpeakersFinalUntil`, `Done`. A JSON error object on stderr or exit codes 1 to 4 become `Error` (research/08 section 0).
4. `transcript::segment` builds segments; `SqliteStore` writes a `file` run with speakers, segments and words in one transaction and sets `current_run_id`. FTS triggers index the text. The intermediate WAV is deleted unless a setting keeps it.

## Live flow (v2)

1. Start: NVML check (refuse under 2 GB free with a clear message), free-space check, create meeting (status recording), start captures and pipelines, open sessions `mic` (diarize off unless `diarize_mic`) and `loopback` (diarize on). Keep the system awake.
2. Steady state: 100 ms frames per channel go through the bounded feed to the engine. `Utterance` events are appended to `live_utterances` and forwarded to the UI; mic words get speaker `me`; loopback labels are provisional. Level meters per channel; a warning when loopback has been silent for 60 s while the mic is active. If the default or active render endpoint changes, the loopback capture reopens on the new endpoint and logs a discontinuity.
3. Echo (plan 15, `echo.mode`, default `auto`): each pipeline also copies its frames to a non-blocking tap; a monitor places both channels on meeting time and correlates the loopback with the mic on 2 s blocks (FFT, causal lags; coupled at a median of 0.35 over 10 active blocks with a consistent lag). From the first coupling the word gate scores each mic word once against the loopback (span start − 350 ms to end + 160 ms, at the tracked lag ± 20 ms, with hysteresis), hides the echoed words in the live view and leaves them out of the live run at Stop; the post-pass rescans the WAVs and, when coupled with a return loss under 20 dB, runs Diarist's own echo canceller (`audio::aec`, a partitioned frequency-domain adaptive filter with residual suppression; the loopback's AGC gain track `<channel>.gain` written beside the WAVs is replayed backwards for the reference) over the raw mic into a temporary `mic-aec.wav`, transcribes that instead of the mic, then gates the words against the raw mic with a higher band. Removed words are kept in the run's `echo_dropped` (Restore puts them back as a new run); the loopback is never touched. "Turn off for this recording" ungates the rest of the recording and the post-pass. Windows' echo cancellation is not used: on this PC it cancelled nothing (research/14, section 1). The live view is gate-only until the causal canceller ships (plan 15 chunk B2b).
4. Stop: close sessions, finish WAVs, materialise the live run, status processing. Post-pass (submitted ahead of any imports queued during the recording): file-mode flow on `loopback.wav` (diarize) and `mic.wav` (no diarize, speaker `me`), map times through the clock anchors, segment each channel on its own, then `transcript::interleave_segments` by start time (overlapping segments stay whole; interleaving words would chop simultaneous speech into one-word rows), write a `post_pass` run, carry over names and edits (docs/02), set `current_run_id`, encode the stereo FLAC playback track and delete the per-channel WAVs, delete the live run unless kept. Status done. `post_pass_mode = on_issues` skips the post-pass and promotes the live run when nothing went wrong.
5. Suspend: flush and anchor; resume: reopen captures and mark the gap. Crash: on next launch, meetings left in `recording` get WAV headers repaired, the live run materialised, and the post-pass queued.

## Repository layout

```
diarist/
  Cargo.toml                workspace
  crates/core/              lib: types, views, store (trait, memory, sqlite), engine, audio, pipeline, models, transcript, export
  crates/cli/          dev CLI: transcribe, record, export, search, models, doctor (no UI)
  crates/meet-engine/       v2 optional: Rust sidecar linking the C ABI (only if FFI wins)
  apps/desktop/             Tauri 2 app: src-tauri/ (thin commands) + src/ (Svelte 5) + src/lib/generated (ts-rs)
  manifests/                engine.json, models.json
  tools/eval/               Python (uv), dev-only: DER / WDER / cpWER against references
  docs/
```

Rule: `apps/desktop/src-tauri` holds only Tauri command wrappers and event plumbing over `diarist_core::store::Store`. All logic is in `crates/core` and is testable without a GUI.

## Where the "implement a research paper" ambition lands

The streaming Sortformer paper (arXiv 2507.18446) is already implemented in the runtime as Nemotron 3 Diarization, and it tops the only independent cross-system benchmark. Re-implementing it would not improve anything. The places where our own work moves quality are, in order:

1. **Fusion (v1.5).** Replace argmax-per-word with a sequence model over words (Viterbi with pause- and punctuation-aware switch costs, overlap flags), stored as a new run so it is compared, not assumed. Needs speaker activity at better than segment granularity; the spike checks whether the C ABI or a CLI flag exposes 10 ms probabilities, otherwise we work from RTTM segments.
2. **Eval (Phase 4).** Hand-labelled meetings of the user's own; WDER near speaker changes and the per-meeting maximum as headline numbers.
3. **Conditioned ASR (later).** When NVIDIA releases SSA-v2 or a GGUF port exists, swap the loopback ASR for it: a new adapter, not a rewrite.

## Testing strategy

- `crates/core` unit tests use fixtures: real runtime JSON outputs captured in the spike, synthetic PCM for the pipeline (gap insertion, drift, anchors), SQL migration round-trips, the `Store` conformance suite run against both `MemoryStore` and `SqliteStore`.
- Integration tests that need the GPU are gated on `DIARIST_ENGINE_DIR` and `DIARIST_MODELS_DIR`; decode tests over the user's files are gated on `DIARIST_SAMPLES`.
- The eval harness (`tools/eval`) computes DER, JER, WDER and cpWER on the eval set; results are committed as a regression table.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Runtime is three months old; live path has open bugs | Live is preview; post-pass is canonical; dither silent loopback for the engine feed only; never rely on WS `commit` |
| VRAM unknown; sysmem fallback hides OOM as a slowdown | Spike measures; `state_arena_slots` 2 to 4; NVML check; document per-app "Prefer No Sysmem Fallback"; CPU retry for file jobs |
| Long files on a full-utterance ASR | Acceptance test on 77 min and 2 h; chunked fallback |
| Loopback silence gaps or endpoint switches desync channels | QPC anchors with zero-fill; reopen on invalidation; piecewise-linear mapping offline |
| Echo without headphones | Correlation monitor and word gate (plan 15): echoed mic words removed live and in the post-pass, kept for Restore; measured on one speaker recording (docs/eval/2026-10-20-echo.md). OS AEC measured useless here (research/14); the app's own canceller runs in the post-pass (owner's double-talk WER 17.1 to 7.6 percent on a spliced set with ground truth), live canceller next. Headphones still give the cleanest "me" |
| Label permutation between preview and post-pass | Overlap matching carries display names; `needs_review` badge for the rest |
| Disk fills up | Free-space checks; retention; pins |
| Unsigned runtime blocked by Smart App Control | Document; plan signing |
| Runtime or Tauri API changes | Trait boundary; manifests pin versions and hashes |
| WebView2 cold start | Hidden window until first paint; measure; Slint fallback documented |
