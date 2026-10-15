# Architecture decision record

Status: v0.3, 15 Oct 2026. Decided by Fable 5.1 from research in `docs/research/`. Items marked PENDING wait on the runtime spike (research/03).

## Decisions at a glance

| Area | Decision | Why (short) |
|---|---|---|
| Inference runtime | NVIDIA NeMo-Speech.cpp v0.2.0, prebuilt Windows CUDA zip, portable folder | Only shipped runtime with Nemotron 3 Diarization (8 spk) plus word-level speaker tags; self-contained DLLs, no CUDA toolkit for users; NVIDIA-maintained |
| Diarizer | Nemotron 3 Diarization q8_0 (streaming Sortformer v3) | Best open non-gated DER; beat pyannote in an independent AMI test; streaming and offline in one checkpoint |
| ASR, file mode | Parakeet TDT 0.6B v3 (fallback: Nemotron streaming EN 0.6B) | Best accuracy with timestamps in the runtime; PENDING spike comparison |
| ASR, live mode | Nemotron Speech Streaming EN 0.6B q8_0 | Only true cache-aware streaming model in the runtime |
| Engine integration | Separate engine process behind a Rust `Engine` trait speaking our protocol (docs/03) | Crash isolation; runtime churn stays behind one adapter |
| v1 file adapter | Spawn `nemo-speech.exe transcribe --diarize --json` | Known-good path; includes the CLI's speaker-label refresh; zero C++ for us |
| v2 live adapter | PENDING spike: `nemo-speech serve` WebSocket (default) vs Rust FFI to the C ABI | WS needs no build step and shares weights across sessions; FFI gives control if WS bugs #48/#49 block |
| Live labels | Preview only. Canonical post-pass over the recorded channels when recording stops | Streaming labels are never revised over WS or C ABI; post-pass with full context is strictly better and costs under a minute |
| Audio capture | `wasapi` crate for mic and render-endpoint loopback; QPC timeline; zero-fill; no process loopback | Process loopback returns zeros for new Teams; wasapi exposes SILENT flag, QPC stamps, device notifications, AEC hook |
| Channels | Mic = "me" (ASR only). Loopback = "them" (ASR + diarization). Two engine sessions | Speaker separation for the user is free and exact; diarizer sees fewer speakers |
| Resampling | rubato 5 `Fft` 48 to 16 kHz with drift slip | Exact 3:1 ratio, real-time safe |
| UI shell | Tauri 2.12.x, NSIS installer, WebView2 download bootstrapper, hidden window until first paint | Smallest installer with a rich transcript view; sidecar and tray support |
| Frontend | Svelte 5 + TypeScript + virtua | Fine-grained reactivity for row-level relabels; tiny runtime; virtualization with dynamic heights |
| Database | rusqlite `bundled` (FTS5 on), WAL | Zero-install, FTS5 proven; sqlite-vec statically later for embeddings |
| Recording storage | 16 kHz mono WAV per channel during recording; optional FLAC afterwards | Crash-safe append; needed for the post-pass and the eval set |
| Install model | 10 MB installer; first run downloads engine zip and models into `%LOCALAPPDATA%\meet-assist` with SHA-256 checks; portable mode via marker file | ComfyUI-style: no Python, no torch, no CUDA toolkit; engine and models update independently |
| Licence | MIT for our code | Runtime Apache-2.0; models OpenMDW / NVIDIA Open Model Licence / CC-BY-4.0, downloaded by the user at first run |
| Word-to-speaker fusion | v1: runtime's start-anchored argmax. v1.5: our own fusion (per-word speaker score vector, margin and overlap flags, Viterbi smoothing with cheaper switches at pauses and punctuation), measured against v1 on the eval set | NeMo-Speech.cpp uses mean probability over [start, start+160 ms] argmax; boundary words and 1 to 2 word islands are the known failure; smoothing is cheap and testable |
| Diarizer config | Post-pass and file mode: `v3-offline` preset (30.4 s context). Live: 1.04 s low-latency preset. Retune onset/offset/min-duration on our eval set (defaults are V2 CALLHOME-tuned) | Offline context is strictly more accurate; live only needs to be readable |
| Cross-meeting identity | v3. TitaNet-L (CC-BY-4.0, EER 0.66) via sherpa-onnx CPU; A/B WeSpeaker ResNet293-LM. Voiceprint = mean of embeddings from 2 to 6 s single-speaker crops; cosine match; user confirms | Not needed for the transcript itself; CPU inference keeps VRAM for ASR |
| Research upgrade path | Diarization-conditioned ASR (NVIDIA SSA-v2, cpWER 15.9 vs cascade 42.3) when released or ported; DiCoW v3.3 as offline ceiling | Cascades lose about 2x cpWER to conditioned ASR on overlap and boundaries; nothing shippable today on a GGUF runtime |

## Process model

```
+---------------------------------------------------------------+
| meet-assist.exe (Tauri 2)                                     |
|  WebView2: Svelte UI  <-- Tauri events/commands -->           |
|  Rust backend (crates/core)                                   |
|    audio::capture   wasapi mic + loopback -> rtrb rings       |
|    audio::pipeline  QPC align, zero-fill, resample, WAV write |
|    engine::supervisor  spawn/restart, protocol, job queue     |
|    engine::nemo_cli    v1 file jobs                           |
|    engine::nemo_live   v2 live sessions (WS or FFI)           |
|    db                  rusqlite, migrations, FTS, queries     |
|    models              manifest, download, verify, registry   |
+----------------------------+----------------------------------+
                             | child processes
              +--------------+----------------+
              | nemo-speech.exe transcribe     |  (per file job)
              | nemo-speech.exe serve          |  (long-lived, live) PENDING
              +--------------------------------+
```

- One Tauri process. All heavy lifting is Rust; the WebView only renders.
- Audio callbacks are real-time threads: they only copy into lock-free rings (`rtrb`). A worker thread drains rings, aligns on the QPC timeline, zero-fills gaps and SILENT packets, resamples, appends to WAV and feeds the engine.
- The engine supervisor owns child processes. If one dies, in-flight jobs are marked failed, the UI is told, and the supervisor relaunches on the next job. Live sessions reconnect and continue; the post-pass makes the final transcript whole.

## Install layout

```
%LOCALAPPDATA%\meet-assist\
  app\                       Tauri app (NSIS, per-user, about 10 MB)
  engine\nemo-speech-0.2.0\  runtime zip contents incl. CUDA DLLs (first run)
  models\                    *.gguf (first run; about 1 GB)
  models.json                installed manifest with sha256 and versions
  meet-assist.db (+ -wal)    SQLite
  audio\<meeting_id>\        mic.wav, loopback.wav or source.wav
  logs\
```

Portable mode: if `portable.txt` sits next to the exe, all of the above live in that folder. The repo ships `manifests/engine.json` and `manifests/models.json` (URLs, sha256, sizes, licences); the app can refresh them from GitHub releases.

## File-mode flow (v1)

1. User drops a file. Rust decodes any container via `symphonia` to 16 kHz mono PCM16 WAV in `audio\<id>\source.wav`.
2. `NemoCliEngine` spawns `nemo-speech.exe transcribe source.wav --diarize --json --device cuda:0 --output out.json`, streams progress from stderr if available.
3. Adapter maps CLI JSON (`words[{word,start,end,speaker}]`) to protocol `words` events with `final:true`, then `finalized`, then `done`.
4. `core::transcript` builds segments (contiguous same-speaker words, split on gaps over 1.5 s or sentence punctuation) and writes speakers, segments, words in one transaction. FTS triggers index the text.

## Live flow (v2)

1. Start: create meeting (status recording), open mic capture and loopback capture, start WAV writers, open two engine sessions: `mic` (diarize off) and `loopback` (diarize on).
2. Steady state: 100 ms PCM frames per channel go to the engine with QPC-derived timestamps. `words` events feed an in-memory preview buffer and the UI. Mic words get speaker `me`. Loopback words carry `spk_N` provisional labels. The preview buffer is checkpointed to `live_checkpoints` every 2 s.
3. Echo: when the OS reports AEC support, request it on the mic client with the loopback endpoint as reference. Otherwise (v2.1) gate mic words whose span overlaps loopback speech with high cross-correlation.
4. Stop: close sessions, finish WAVs, status processing. Run the canonical post-pass: file-mode flow on `loopback.wav` (diarize) and `mic.wav` (no diarize, speaker `me`), interleave by time, replace the preview. Match preview speakers to post-pass speakers by time-overlap (greedy on the overlap matrix) so display names set during the meeting carry over. Status done.
5. Crash: on next launch, any meeting left in `recording` has its checkpoint replayed and its WAVs post-passed.

Loopback endpoint selection: v2.0 uses the default render endpoint and follows default-device changes via wasapi notifications. v2.1 adopts anarlog's trick of following the endpoint a foreign process is actually rendering to (so a headset used by Teams is captured).

## Repository layout

```
meet-assist/
  Cargo.toml                workspace
  crates/core/              lib: db, audio, engine, models, transcript, types
  crates/meet-cli/          dev CLI: transcribe a file, dump JSON, run post-pass (no UI)
  crates/meet-engine/       v2 optional: Rust sidecar linking the C ABI (only if FFI wins)
  apps/desktop/             Tauri 2 app: src-tauri/ (thin commands) + src/ (Svelte 5)
  manifests/                engine.json, models.json
  tools/eval/               Python (uv), dev-only: DER / cpWER against hand labels
  docs/
```

Rule: `apps/desktop/src-tauri` holds only Tauri command wrappers and event plumbing. All logic is in `crates/core` and is testable without a GUI.

## Where the "implement a research paper" ambition lands

The streaming Sortformer paper (arXiv 2507.18446) is already implemented in the runtime as Nemotron 3 Diarization, and it tops the only independent cross-system benchmark (VoiceArena, DER 14.7 vs 19 to 31 for everything else open). Re-implementing it would not improve anything. The places where our own work moves quality are, in order:

1. **Fusion (v1.5).** Replace argmax-per-word with a sequence model over words (Viterbi with pause- and punctuation-aware switch costs, overlap flags). Needs speaker activity at better than segment granularity; the spike checks whether the C ABI or a CLI flag exposes 10 ms probabilities, otherwise we work from RTTM segments.
2. **Eval (Phase 4).** Hand-labelled meetings of the user's own, WDER near speaker changes as the headline number. Everything above is tuned on this.
3. **Conditioned ASR (later).** When NVIDIA releases SSA-v2 or a GGUF port exists, swap the loopback ASR for it. The engine trait makes that a new adapter, not a rewrite.

## Testing strategy

- `crates/core` unit tests use fixtures: real runtime JSON outputs captured in the spike, synthetic PCM for the pipeline (gap insertion, drift), SQL migration round-trips.
- Integration tests that need the GPU are gated on `MEET_ASSIST_ENGINE_DIR` being set and are run manually.
- The eval harness (`tools/eval`) computes DER and words-to-wrong-speaker on the user's hand-labelled meetings plus AMI ES2002a; results are committed as a regression table.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Runtime is three months old; live path has open bugs | Live is preview; post-pass is canonical; dither silent loopback (never exact zeros); never rely on WS `commit` |
| VRAM unknown | Spike measures; `state_arena_slots` 2 to 4; budget 4 GB resident |
| Loopback silence gaps desync channels (Vibe #1592) | QPC timeline with zero-fill; drift tracker |
| Echo without headphones | OS AEC when available; correlation gate; document "use headphones" |
| Label permutation between preview and post-pass | Overlap matching carries display names |
| Runtime API changes (v3 of Tauri, new engine releases) | Engine trait boundary; manifests pin versions and hashes |
| WebView2 cold start | Hidden window until first paint; measure; Slint fallback documented |
