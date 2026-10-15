# Developer setup

Commands below were run on the reference machine (Windows 11, Rust 1.99.0
MSVC, Node 22.19, bun 1.2.1, VS Build Tools 2022 with the C++ workload,
WebView2 142) on 15 Oct 2026, except those marked *(not yet run)*. Run them
from Git Bash or PowerShell.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust (MSVC) | 1.90+ (workspace `rust-version`; Tauri 2.12 needs it) | `rustup update stable` |
| VS Build Tools 2022 | C++ workload | linker and Windows SDK |
| bun | 1.2 | package manager and script runner for `apps/desktop` |
| Node | 22.12+ | Vite 8 runs on Node |
| WebView2 runtime | evergreen | preinstalled on Windows 11 |

No global installs: the Tauri CLI is a devDependency (`@tauri-apps/cli`) and is
run through `bun run tauri`.

## Rust workspace

From the repo root:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Dev CLI (exit codes: 0 ok, 1 error, 2 usage error):

```sh
cargo run -p meet-cli -- version                     # prints "meet-cli 0.1.0"
cargo run -p meet-cli -- doctor                      # data root, selections, GPU, free VRAM
cargo run -p meet-cli -- models scan                 # find GGUFs under models\ and runtimes under engine\
cargo run -p meet-cli -- models pull                 # required models via `nemo-speech pull`, SHA-checked
cargo run -p meet-cli -- engine install              # runtime zip from manifests/engine.json
cargo run --release -p meet-cli -- transcribe some.m4a [--language auto] [--rttm] [--out t.md]
cargo run -p meet-cli -- list | show <id> | search <words> | export <id> --format srt
cargo run -p meet-cli -- devices                     # audio endpoints with ids, default and comms marks
cargo run -p meet-cli -- play --device "Speakers (2- VB-Audio" x.wav   # render a WAV to an output endpoint
cargo run --release -p meet-cli -- record [--seconds 60] [--title t] [--language en] [--diarize-mic] [--mic <id|name>] [--loopback <id|name>]
```

`transcribe` extras: `--json` prints one machine-readable result object on
stdout (meeting id and uid, run id, duration, speakers, wall time, model
paths, the words JSON and, with `--rttm`, the RTTM path; the summary goes to
stderr); `--asr-model` / `--diar-model` take a `.gguf` path or a name, id or
path from `models list` for that run only; `--engine-arg=<flag>` (repeatable)
is passed verbatim to `nemo-speech transcribe`, e.g. `--engine-arg=--verbatim`.

The default ASR model is the EN model (`nemotron-en`, measured best on
English, docs/eval); Nemotron 3.5 is the optional pick for other or mixed
languages, and only it receives `--language`.

Global flags: `--data-dir` (default: `MEET_ASSIST_DATA_DIR`, then
`portable.txt` next to the exe, then `HKCU\Software\meet-assist\DataRoot`,
then `%LOCALAPPDATA%\app.meetassist.desktop`), `--db`, and `--models-dir` /
`--engine-dir` (relative paths are taken from the current directory) to use
models and a runtime from elsewhere, e.g. the repo's local copies:

```sh
cargo run --release -p meet-cli -- --data-dir %TEMP%\ma --models-dir models --engine-dir engine transcribe data\samples\ES2002a.Mix-Headset.wav
```

### Live recording (`record`, Phase 3a)

See also [Live recording](#live-recording) below for the app, the test
devices, the capture settings and crash recovery.

`record` checks free VRAM (2.5 GB, from NVML; `doctor` when NVML is not
available), starts `nemo-speech
serve` with the selected ASR model and diarizer (log: `<data
root>\logs\serve.log`), captures the microphone (default: the default
communications capture endpoint) and the system loopback (default: the
default output, following the endpoint other apps actually play to), and
prints live words per channel with provisional labels and the levels once a
second. Ctrl-C or `--seconds` stops it; it then waits for the post-pass and
prints the run ids, speakers, FLAC size and wall time. Devices are given by
id or by (a prefix of) the name `meet-cli devices` prints. It never changes
the system's default devices or volumes.

`play --device <render id|name> <16-bit wav>` renders a WAV to an output
endpoint and prints the QPC time its first frame played (for capture
tests). On the reference machine the VB-Audio cable's render side is called
`Speakers (2- VB-Audio Virtual Cable)`, not `CABLE Input`; whatever is played
there comes out of the `CABLE Output (2- VB-Audio Virtual Cable)` capture
endpoint, which makes a deterministic mic:

```sh
meet-cli play --device "Speakers (2- VB-Audio" talk.wav &
meet-cli record --seconds 30 --mic "CABLE Output"
```

Setting `capture.autoconvert` (default true) set to `false` opens the
captures at the mix format and resamples with rubato instead of Windows'
16 kHz converter.

Loopback audio comes after the output's software volume: on an endpoint
without hardware volume (S/PDIF, "Realtek Digital Output" here) at a few
percent, speech arrives 40 dB down (peaks around -70 dBFS, about 3 bits of
PCM16) and is barely transcribed. Record from an output at a normal volume.

### Gated tests

Skipped unless the variables are set:

| Variable | Tests |
|---|---|
| `MEET_ASSIST_SAMPLES` (folders, `;`-separated) | `--test decode_samples`: every `*.m4a` decodes within 50 ms of ffprobe (run with `--release`) |
| `MEET_ASSIST_MODELS_DIR` | real GGUF headers, scan, classification |
| `MEET_ASSIST_ENGINE_DIR` + `MEET_ASSIST_MODELS_DIR` | `--test engine_gpu` (30 s AMI cut, cancel, error mapping, diarize, cascade), `doctor`, cached `pull` |
| `MEET_ASSIST_AMI_WAV` | AMI WAV for `engine_gpu` (default `<models>\..\data\samples\ES2002a.Mix-Headset.wav`) |
| `MEET_ASSIST_CIMC_WAV` + engine + models | `--test acceptance_gpu` (run with `--release`, about 2.5 min): 77-minute import within 2 percent of the reference word count, 8 speakers, no empty minute |
| engine + models | `-p meet-core --test live_ws_gpu`: 30 s of AMI streamed in real time to `nemo-speech serve`; words, `spk_N` tags, first-word latency, VRAM |
| `MEET_ASSIST_AUDIO_TEST=1` (VB-Audio Cable) | `-p meet-core --test capture_device`: a tone played into the cable is captured from `CABLE Output` at 16 kHz |
| `MEET_ASSIST_LIVE_TEST=1` + engine + models (VB-Audio Cable) | `-p meet-cli --test live_record` (about 80 s, one minute audible on the loopback output): cable as the mic, a second AMI cut on the default output (`MEET_ASSIST_LOOPBACK_RENDER` picks another output; `MEET_ASSIST_CABLE_RENDER` / `MEET_ASSIST_CABLE_CAPTURE` override the cable); WAV lengths, cross-correlation offsets, live latency, post-pass run, FLAC, rename carry-over |

`UPDATE_GOLDEN=1 cargo test -p meet-core golden` rewrites the export golden
files in `crates/core/tests/golden/`.

`.cargo/config.toml` turns on the MSRV-aware resolver
(`incompatible-rust-versions = "fallback"`), so `cargo update` picks dependency
versions that support the workspace `rust-version` (1.90) when they exist.

### Core/UI seam

- **TypeScript types** for `meet_core::views`, the domain types and the
  live payloads (`meet_core::live`, `live::views`, `audio::capture`'s
  `DeviceInfo` as `AudioDevice`, `engine::ServeStatus` / `ServeSnapshot`)
  are generated by ts-rs into `apps/desktop/src/lib/generated/` whenever the
  `meet-core` tests run (`cargo test -p meet-core`, or the workspace test
  above), together with two constant modules: `palette.ts`
  (`SPEAKER_PALETTE`, `ME_COLOR`) and `settingDefaults.ts`
  (`meet_core::settings::DEFAULTS`). They are committed; never edit them by
  hand. ts-rs does not delete
  files for removed types, so after renaming or removing one, regenerate from
  scratch:

  ```sh
  rm -rf apps/desktop/src/lib/generated && cargo test -p meet-core --lib export_bindings
  ```

  `.cargo/config.toml` sets `TS_RS_EXPORT_DIR` and `TS_RS_LARGE_INT=number`
  (ids and timestamps are `i64` but stay below 2^53).
- **Store conformance:** `crates/core/tests/store_conformance.rs` runs every
  `Store` and `RunStore` case (runs, live utterances and live speakers, anchors, carry-over, track deletion)
  against `MemoryStore` and `SqliteStore` (in-memory); a new implementation
  adds one test that calls `run_all` with its own factory.
- **FFI sidecar wire format** (not on the main path):
  `cargo test -p meet-core --features ffi-wire`.
- **Demo data:** `meet-assist.exe --demo` runs on `MemoryStore::demo()`
  (500 meetings, three with 2,000 segments, model files and selections) with
  a scratch data root under `%TEMP%\meet-assist-demo`. Without `--demo` the
  app opens `SqliteStore` on the resolved data root.

## Desktop frontend

From `apps/desktop`:

```sh
bun install
bun run build     # vite build into apps/desktop/dist
bun run check     # svelte-check (TypeScript + Svelte)
bun run dev       # Vite dev server on http://127.0.0.1:1420 (no Tauri APIs in a plain browser)
```

If `bun install` fails with `ConnectionRefused downloading package manifest`,
re-run it; the registry connection is occasionally refused on this network.

## Desktop app (Tauri)

From `apps/desktop`:

```sh
bun run tauri dev            # dev window with hot reload (not yet run)
bun run tauri build --debug  # target/debug/meet-assist.exe + target/debug/bundle/nsis/*-setup.exe
bun run tauri build          # release build and NSIS installer (not yet run)
```

The Tauri CLI downloads NSIS into its cache on the first bundle. If that
download is blocked, add `--no-bundle` to get just the exe.

Smoke test (PowerShell), as used for Phase 0 acceptance:

```powershell
$p = Start-Process -FilePath .\target\debug\meet-assist.exe -PassThru
Start-Sleep -Seconds 5
$p.Refresh(); "alive: $(-not $p.HasExited)"
Stop-Process -Id $p.Id; $p.WaitForExit(5000) | Out-Null; "exited: $($p.HasExited)"
```

Runtime locations: app log under `%LOCALAPPDATA%\app.meetassist.desktop\logs\`,
WebView2 profile under `%LOCALAPPDATA%\app.meetassist.desktop\EBWebView\`.
To inspect the page from outside, start the exe with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` and
attach any Chrome DevTools client to `http://127.0.0.1:9223`; commands can be
called from the page with `window.__TAURI_INTERNALS__.invoke(...)` (e.g.
`import_file`, `export_meeting_to` writes an export without the dialog).

Debug builds compile dependencies with `opt-level = 2` (workspace
`Cargo.toml`), but `meet-core` itself stays unoptimised: decoding a 45-minute
m4a takes about 90 s in a debug build against 3 s in release.

### End-to-end on the GPU against a scratch data root

The Phase 1.5 check (run 15 Oct 2026): a scratch data root whose `models`
and `engine` folders are junctions to the repo's local copies, so nothing is
copied or downloaded.

```powershell
$root = "$env:TEMP\ma-e2e"
New-Item -ItemType Directory -Force $root | Out-Null
cmd /c mklink /J "$root\models" "$PWD\models"
cmd /c mklink /J "$root\engine" "$PWD\engine"
$env:MEET_ASSIST_DATA_DIR = $root
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9223"
.\target\debug\meet-assist.exe
```

The first import scans the folders and selects the EN model and Nemotron 3
Diarization. `nemo-speech` 0.2.0 cannot open paths of 260 characters or
more; the adapter passes long paths in the `\\?\` form, so deep data roots
work.

### Tauri versions

The `tauri` crate (2.12) and the npm `@tauri-apps/api` / `@tauri-apps/cli`
packages (~2.12) must stay on the same major.minor: the CLI refuses to build
otherwise. Same for any `@tauri-apps/plugin-*` package added later and its
crate.

## Live recording

Run on the reference machine on 15 Oct 2026 (Phase 3.5).

### Test devices

The VB-Audio Virtual Cable makes a deterministic microphone: whatever is
played to its render side `Speakers (2- VB-Audio Virtual Cable)` comes out
of the capture endpoint `CABLE Output (2- VB-Audio Virtual Cable)`. For the
system audio, play a second file to an output with hardware volume, here
`Speakers (HyperX QuadCast)`, and record that output's loopback. Do not use
`Realtek Digital Output` (the default output on this machine) for tests: it
applies the Windows volume in software, so at a low volume the loopback
arrives 40 dB down (the AGC makes up for most of it; see `LoopbackQuiet`).
Nothing here changes the system's default devices or volumes.

```sh
meet-cli --data-dir %TEMP%\ma-cli devices        # ids and names, [default] / [comms] marks
meet-cli --data-dir %TEMP%\ma-cli play --device "Speakers (2- VB-Audio" talk.wav
meet-cli --data-dir %TEMP%\ma-cli play --device "Speakers (HyperX QuadCast)" other.wav
```

`meet-cli doctor` prints the NVML reading (`nvml  8409 MB free of 10240
MB`) next to the runtime's.

### From the CLI

```sh
meet-cli play --device "Speakers (2- VB-Audio" talk.wav &
meet-cli play --device "Speakers (HyperX QuadCast)" other.wav &
meet-cli record --seconds 60 --mic "CABLE Output" --loopback "Speakers (HyperX QuadCast)"
```

### From the app

- **R** opens the start sheet (title, language, room mode, microphone and
  system-audio pickers preselected from `record.mic_device` /
  `record.loopback_device`); **Enter** starts. R never stops a recording.
- **Ctrl+.** or the Stop button asks "Stop recording?" every time;
  **Enter** confirms, Esc keeps recording. **Ctrl+Alt+R** (global) and the
  tray toggle without asking.
- **R** (or a double-click) on a live line renames its speaker with
  `rename_speaker_at`: the line's meeting-time range becomes the name
  anchor, so the post-pass gives the name to whoever speaks there, even a
  speaker with under 5 s of speech.
- While recording the GPU queue is paused (`LiveStateView.queue_paused`;
  queued imports say "Waiting for the recording to stop"); at Stop the
  post-pass goes to the front of the queue, then the queue resumes. A failed
  or cancelled post-pass can be retried (`live::rerun_post_pass`) while the
  WAVs exist.
- Start refuses under 2560 MB of free VRAM, read from NVML (`core::gpu`;
  `nemo-speech doctor` when NVML is missing) as it is, with the live server
  already loaded; the message says how much is free and how much is needed.
  The app log line `recording start: ... free VRAM 7104 MB (NVML; needs
  2560 MB)` shows the reading.

### Capture settings

All are in `settings::DEFAULTS` (and `generated/settingDefaults.ts`):

| Key | Default | Effect |
|---|---|---|
| `capture.loopback_agc` | `true` | Slow automatic gain on the system audio before the WAV and the engine feed (peaks toward -20 dBFS, at most +40 dB, never on silence); the loopback captures 32-bit float |
| `capture.mic_agc` | `false` | The same on the microphone |
| `capture.autoconvert` | `true` | Windows converts to 16 kHz; `false` captures at the mix format and resamples with rubato |
| `record.mic_device`, `record.loopback_device` | `""` | Endpoint ids for new recordings (`""`: the default communications mic; the default output, following the app that plays) |
| `record.diarize_mic` | `false` | Diarize the mic too (a room with several people) |

The level meters show the gain the AGC applied; their tooltip shows the
peak before it.

### Crash recovery

At launch, before a recorder can start, `live::recover` finishes live
meetings a crash left behind:

- `recording`: WAV headers are repaired from the file lengths, the live run
  is materialised and made current, and the post-pass is queued.
- `processing` (the post-pass was queued or running) with both WAVs: the
  runs the interrupted post-pass wrote are deleted and it is queued again.
- `processing` without WAVs: a finished `post_pass` run that is current is
  kept (the live run is deleted unless `keep_live_runs`); otherwise the live
  run becomes current with `engine_info.recovery` explaining why, and the
  meeting is done.

The app log says what it did (`recovered meeting 3 (was processing):
PostPassQueued; ...`). To try it, end the app from Task Manager while it
records (or while it shows "Finalizing") and start it again.

### Smoke run over DevTools

A scratch data root whose `models` and `engine` are junctions to the repo
copies (see "End-to-end on the GPU" above), the debug app started with
`MEET_ASSIST_DATA_DIR` set to it and
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223`, and a
Node script talking to `http://127.0.0.1:9223/json`: `Runtime.evaluate`
for `window.__TAURI_INTERNALS__.invoke(...)` and DOM checks,
`Input.dispatchKeyEvent` / `Input.insertText` for R, Enter and Ctrl+.
(Node 22 has `WebSocket` built in). The run on 15 Oct 2026: mic = CABLE
Output playing AMI ES2002a 120-150 s from 2 s, system audio = HyperX
QuadCast output playing 300-330 s from 17 s (15 s of overlap), about 52 s
recorded:

| What | Observed |
|---|---|
| Live server ready after the sheet opened | 2.6 s (holds 1087 MB) |
| Free VRAM at Start (NVML) | 7104 MB, gate 2560 MB |
| First live words after playback started | mic 2.8 s, system 1.1 s; UI lag avg 205 ms, max 1.08 s |
| Live words | mic 68, system 53 (3 provisional labels) |
| Stop to idle, to post-pass done | 0.26 s, 6.4 s |
| Overlap 17-32 s in the post-pass | 8 segments, 4 channel switches; 2 one-word segments, both one-word speaker turns within the system channel (interleaving words first: 14 runs, 6 of one word) |
| R rename of a system line at 29.8 s ("Dana") | the post-pass `spk_1` is "Dana", anchor 29760-30960 ms |
| Ctrl+. | "Stop recording?" while still recording; Enter stopped |
| R while recording | still recording, a hint toast, no sheet |
| Live legend | no chip for speakers without words |

The app log goes to `%LOCALAPPDATA%\app.meetassist.desktop\logs\`
(the folder tauri-plugin-log uses) whatever the data root.
