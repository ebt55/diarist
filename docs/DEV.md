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
cargo run -p diarist-cli -- version                     # prints "diarist-cli 0.1.0-beta.2"
cargo run -p diarist-cli -- doctor                      # data root, selections, GPU, free VRAM
cargo run -p diarist-cli -- models scan                 # find GGUFs under models\ and runtimes under engine\
cargo run -p diarist-cli -- models pull                 # required models via `nemo-speech pull`, SHA-checked
cargo run -p diarist-cli -- engine install              # runtime zip from manifests/engine.json
cargo run -p diarist-cli -- retention [--apply]         # audio past audio_retention_days (dry run by default)
cargo run --release -p diarist-cli -- transcribe some.m4a [--language auto] [--rttm] [--out t.md]
cargo run -p diarist-cli -- list | show <id> | search <words> | export <id> --format srt
cargo run -p diarist-cli -- devices                     # audio endpoints with ids, default and comms marks
cargo run -p diarist-cli -- play --device "Speakers (2- VB-Audio" x.wav   # render a WAV to an output endpoint
cargo run --release -p diarist-cli -- record [--seconds 60] [--title t] [--language en] [--diarize-mic] [--mic <id|name>] [--loopback <id|name>]
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

Global flags: `--data-dir` (default: `DIARIST_DATA_DIR`, then
`portable.txt` next to the exe, then `HKCU\Software\Diarist\DataRoot`,
then `%LOCALAPPDATA%\app.diarist.desktop`), `--db`, and `--models-dir` /
`--engine-dir` (relative paths are taken from the current directory) to use
models and a runtime from elsewhere, e.g. the repo's local copies:

```sh
cargo run --release -p diarist-cli -- --data-dir %TEMP%\diarist-scratch --models-dir models --engine-dir engine transcribe data\samples\ES2002a.Mix-Headset.wav
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
id or by (a prefix of) the name `diarist-cli devices` prints. It never changes
the system's default devices or volumes.

`play --device <render id|name> <16-bit wav>` renders a WAV to an output
endpoint and prints the QPC time its first frame played (for capture
tests). On the reference machine the VB-Audio cable's render side is called
`Speakers (2- VB-Audio Virtual Cable)`, not `CABLE Input`; whatever is played
there comes out of the `CABLE Output (2- VB-Audio Virtual Cable)` capture
endpoint, which makes a deterministic mic:

```sh
diarist-cli play --device "Speakers (2- VB-Audio" talk.wav &
diarist-cli record --seconds 30 --mic "CABLE Output"
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
| `DIARIST_SAMPLES` (folders, `;`-separated) | `--test decode_samples`: every `*.m4a` decodes within 50 ms of ffprobe (run with `--release`) |
| `DIARIST_MODELS_DIR` | real GGUF headers, scan, classification |
| `DIARIST_ENGINE_DIR` + `DIARIST_MODELS_DIR` | `--test engine_gpu` (30 s AMI cut, cancel, error mapping, diarize, cascade), `doctor`, cached `pull` |
| `DIARIST_AMI_WAV` | AMI WAV for `engine_gpu` (default `<models>\..\data\samples\ES2002a.Mix-Headset.wav`) |
| `DIARIST_LONGFILE_WAV` + engine + models | `--test acceptance_gpu` (run with `--release`, about 2.5 min): 77-minute import within 2 percent of the reference word count, 8 speakers, no empty minute |
| engine + models | `-p diarist-core --test live_ws_gpu`: 30 s of AMI streamed in real time to `nemo-speech serve`; words, `spk_N` tags, first-word latency, VRAM |
| `DIARIST_AUDIO_TEST=1` (VB-Audio Cable) | `-p diarist-core --test capture_device`: a tone played into the cable is captured from `CABLE Output` at 16 kHz |
| `DIARIST_LIVE_TEST=1` + engine + models (VB-Audio Cable) | `-p diarist-cli --test live_record` (about 80 s, one minute audible on the loopback output): cable as the mic, a second AMI cut on the default output (`DIARIST_LOOPBACK_RENDER` picks another output; `DIARIST_CABLE_RENDER` / `DIARIST_CABLE_CAPTURE` override the cable); WAV lengths, cross-correlation offsets, live latency, post-pass run, FLAC, rename carry-over |
| `REPRO_FLAC` + engine + models (no audio device) | `-p diarist-core --test live_carry_repro -- --ignored --nocapture`: replays a stereo 16 kHz FLAC (left mic, right loopback, e.g. a live meeting's `playback.flac`) in real time through `nemo-speech serve` as the recorder would, renames the top loopback label from the legend and the second from a live line half way, then runs the real post-pass with `keep_live_runs` on; prints the carry-over report, both runs' speakers and a checksum of the post-pass words, and fails when a name reaches nobody. `REPRO_OUT` (default `%TEMP%\diarist-replay`) holds `repro.db` and the logs; `REPRO_SRC_DB` + `REPRO_MEETING` take the clock anchors from the recording's database (read-only); `REPRO_SECONDS`, `REPRO_SPEED`, `REPRO_RENAME_AT`; `REPRO_RESUME=1` runs the post-pass and carry-over again on `REPRO_OUT`'s live run |

`UPDATE_GOLDEN=1 cargo test -p diarist-core golden` rewrites the export golden
files in `crates/core/tests/golden/`.

`.cargo/config.toml` turns on the MSRV-aware resolver
(`incompatible-rust-versions = "fallback"`), so `cargo update` picks dependency
versions that support the workspace `rust-version` (1.90) when they exist.

### Core/UI seam

- **TypeScript types** for `diarist_core::views`, the domain types and the
  live payloads (`diarist_core::live`, `live::views`, `audio::capture`'s
  `DeviceInfo` as `AudioDevice`, `engine::ServeStatus` / `ServeSnapshot`)
  are generated by ts-rs into `apps/desktop/src/lib/generated/` whenever the
  `diarist-core` tests run (`cargo test -p diarist-core`, or the workspace test
  above), together with three constant modules: `palette.ts`
  (`SPEAKER_PALETTE`, `ME_COLOR`), `settingDefaults.ts`
  (`diarist_core::settings::DEFAULTS`) and `languageTiers.ts`
  (`LANGUAGE_TIERS`, `MULTILINGUAL_MODEL`, from
  `diarist_core::models::languages`). They are committed; never edit them by
  hand. ts-rs does not delete
  files for removed types, so after renaming or removing one, regenerate from
  scratch:

  ```sh
  rm -rf apps/desktop/src/lib/generated && cargo test -p diarist-core --lib export_bindings
  ```

  `.cargo/config.toml` sets `TS_RS_EXPORT_DIR` and `TS_RS_LARGE_INT=number`
  (ids and timestamps are `i64` but stay below 2^53).
- **Store conformance:** `crates/core/tests/store_conformance.rs` runs every
  `Store` and `RunStore` case (runs, live utterances and live speakers, anchors, carry-over, track deletion)
  against `MemoryStore` and `SqliteStore` (in-memory); a new implementation
  adds one test that calls `run_all` with its own factory.
- **FFI sidecar wire format** (not on the main path):
  `cargo test -p diarist-core --features ffi-wire`.
- **Demo data:** `diarist.exe --demo` runs on `MemoryStore::demo()`
  (500 meetings, three with 2,000 segments, model files and selections) with
  a scratch data root under `%TEMP%\diarist-demo`. Without `--demo` the
  app opens `SqliteStore` on the resolved data root.

## Desktop frontend

From `apps/desktop`:

```sh
bun install
bun run build     # vite build into apps/desktop/dist
bun run check     # svelte-check (TypeScript + Svelte)
bun run test      # bun test: plain TypeScript logic under tests/
bun run dev       # Vite dev server on http://127.0.0.1:1420 (no Tauri APIs in a plain browser)
```

If `bun install` fails with `ConnectionRefused downloading package manifest`,
re-run it; the registry connection is occasionally refused on this network.

## Desktop app (Tauri)

From `apps/desktop`:

```sh
bun run tauri dev                       # dev window with hot reload (not yet run)
bun run tauri build --debug --no-bundle # target/debug/diarist.exe, frontend embedded
bun run tauri build                     # release exe + NSIS installer + updater signature
```

A plain `cargo build -p diarist` builds an exe that loads the frontend
from the Vite dev server (`bun run dev`); use the `tauri build` commands for
an exe with the frontend inside.

The release build signs the updater artifact (`bundle.createUpdaterArtifacts`)
and needs the updater private key:

```sh
export TAURI_SIGNING_PRIVATE_KEY="$USERPROFILE/.tauri/diarist.key"   # a path or the key itself
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""                              # this key has no password
bun run tauri build
```

Without the key, build with `bun run tauri build -c '{"bundle":{"createUpdaterArtifacts":false}}'`.
The key was generated once with `bunx tauri signer generate --ci -w
%USERPROFILE%\.tauri\diarist.key` (a key file kept under an earlier name
works the same: point the variable at it or rename it); it is not in the
repository. Its public
half is `plugins.updater.pubkey` in `tauri.conf.json`. Losing the private key
means installed copies can no longer verify updates: back it up, and give it
to CI as the `TAURI_SIGNING_PRIVATE_KEY` secret (`.github/workflows/release.yml`,
not run yet). The repository's social preview (Settings, General, Social
preview) is `branding/social.png`.

**Update endpoint.** `plugins.updater.endpoints` in `tauri.conf.json` is
`https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json`,
not GitHub's `releases/latest/download/latest.json`: "latest" skips
pre-releases, so it would never serve a beta. `updater/latest.json` names the
version, the notes, the publication time and, for `windows-x86_64`, the
contents of the installer's `.sig` and the release asset URL
(`https://github.com/ebt55/diarist/releases/download/v<version>/Diarist_<version>_x64-setup.exe`).
It must match the published asset: a tag push `v*` runs the release workflow,
which builds the installer and `.sig`, attaches them to a draft pre-release
and opens a pull request that updates `updater/latest.json`; merge it only
after the draft is published. A release built by hand skips the build: create
the draft pre-release with the installer and its `.sig` attached, publish it,
then commit `updater/latest.json` on main with that `.sig`'s contents and the
asset URL. In that order because draft assets are not public: a manifest that
points at a draft makes every installed copy's check fail until the draft is
published, while a published release that the manifest does not name yet is
simply not offered. Publishing pushes the tag; the workflow sees the installer
already on the release and builds nothing. The steps for 0.1.0-beta.2, the
first update ever delivered, are in
[docs/plans/13-release-beta2-checklist.md](plans/13-release-beta2-checklist.md).

**Versioning.** Releases are `0.1.0-beta.N` pre-releases while the app is
tried on other machines, then `0.x.y` while the database schema, the data
layout and the updater format may still change; 1.0 comes after a few
releases have upgraded installed copies without loss. The version lives in
the workspace `Cargo.toml`, `tauri.conf.json` and `apps/desktop/package.json`
(Cargo.lock follows through cargo). NSIS accepts pre-release versions; MSI
would not, and is not built.

**Release PDB.** The release profile (workspace `Cargo.toml`) builds with
`debug = "line-tables-only"` and `strip = "none"`, so `bun run tauri build`
leaves `target\release\diarist.pdb` next to the exe (and `cargo build
--release -p diarist-cli` leaves `diarist_cli.pdb`). On windows-msvc the debug
information lives only in the PDB; the exe names it by file name and GUID, so
its size does not change and the installer does not grow. Measured on
`diarist-cli` (16 Oct 2026): exe 10,636,288 bytes with the old `strip = true`
and 10,633,216 with line tables; PDB 3.3 MB, which resolved only the standard
library's frames (ours showed as `__ImageBase`), against 29.8 MB that resolves
every frame to a function, file and line. A PDB matches only the build that
produced it, so archive `diarist.exe` and `diarist.pdb` of every release
together, either as a private artifact the owner keeps (for example
`diarist-<version>-symbols.zip` next to the signing key's backup) or as an
asset on the GitHub release; the owner decides per release. A published PDB
holds no code, but it does hold function names and the build machine's
absolute source paths, user folder name included. To symbolicate a crash
report: its header gives `exe base`, a native report gives `address: ... in
diarist.exe + 0x<offset>`, and for a frame of a panic backtrace the offset is
the frame address minus the exe base. With the archived exe and PDB in one
folder, `cdb -z <folder>\diarist.exe -y <folder> -c ".lines -e; ln
diarist+0x<offset>; q"` (cdb from the Windows SDK's Debugging Tools; the CLI's
module is `diarist_cli`) prints the function and the source line, and `cdb -z
crash-<time>.dmp -y <folder> -i <folder> -c ".lines -e; .ecxr; kn; q"` prints
the whole stack of a minidump the user chose to send. Keep the symbol path to
that folder: `srv*` would fetch Windows' own symbols from Microsoft.

The Tauri CLI downloads NSIS into its cache on the first bundle. If that
download is blocked, add `--no-bundle` to get just the exe.

Release build on the reference machine (15 Oct 2026, 5 min 47 s from a warm
cache): installer `Diarist_0.1.0_x64-setup.exe` 6.67 MiB, exe 24.9 MiB.
Startup and import timings: "Release measurements" below.

### Installer and uninstaller

`apps/desktop/src-tauri/windows/hooks.nsh` hooks Tauri's NSIS template
(per-user install, `downloadBootstrapper` for WebView2). The POSTUNINSTALL
hook reads `HKCU\Software\Diarist\DataRoot`: with "Delete the application
data" ticked it also removes Diarist's items from a moved data root
(with the pre-upgrade `diarist.db.bak-*`), the key and the empty roaming
folder `%APPDATA%\app.diarist.desktop` the WebView leaves (the template
removes that one too; the hook makes sure); unticked it asks whether to
remove `engine\` and `models\` and leaves the rest. For
tests, `uninstall.exe /S /DELETEAPPDATA` acts as if the box were ticked and
`/S /REMOVEMODELS` answers Yes to the question. **The ticked box (and
`/DELETEAPPDATA`) always removes `%LOCALAPPDATA%\app.diarist.desktop`**:
move a real data folder out of the way before testing it.

Smoke test (PowerShell), as used for Phase 0 acceptance:

```powershell
$p = Start-Process -FilePath .\target\debug\diarist.exe -PassThru
Start-Sleep -Seconds 5
$p.Refresh(); "alive: $(-not $p.HasExited)"
Stop-Process -Id $p.Id; $p.WaitForExit(5000) | Out-Null; "exited: $($p.HasExited)"
```

Runtime locations, all under the resolved data root (default
`%LOCALAPPDATA%\app.diarist.desktop`): the app log `logs\diarist.log`
and the live server's `logs\serve.log`, the WebView2 profile `EBWebView\`,
the window state `.window-state.json`. A scratch data root
(`DIARIST_DATA_DIR`, `portable.txt` or the registry key) therefore never
touches the real one. Only one instance runs at a time (single-instance
plugin): close the app before starting another build.
To inspect the page from outside, start the exe with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` and
attach any Chrome DevTools client to `http://127.0.0.1:9223`; commands can be
called from the page with `window.__TAURI_INTERNALS__.invoke(...)` (e.g.
`import_file`, `export_meeting_to` writes an export without the dialog).

Debug builds compile dependencies with `opt-level = 2` (workspace
`Cargo.toml`), but `diarist-core` itself stays unoptimised: decoding a 45-minute
m4a takes about 90 s in a debug build against 3 s in release.

### End-to-end on the GPU against a scratch data root

The Phase 1.5 check (run 15 Oct 2026): a scratch data root whose `models`
and `engine` folders are junctions to the repo's local copies, so nothing is
copied or downloaded.

```powershell
$root = "$env:TEMP\diarist-e2e"
New-Item -ItemType Directory -Force $root | Out-Null
cmd /c mklink /J "$root\models" "$PWD\models"
cmd /c mklink /J "$root\engine" "$PWD\engine"
$env:DIARIST_DATA_DIR = $root
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9223"
.\target\debug\diarist.exe
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
diarist-cli --data-dir %TEMP%\diarist-cli-data devices        # ids and names, [default] / [comms] marks
diarist-cli --data-dir %TEMP%\diarist-cli-data play --device "Speakers (2- VB-Audio" talk.wav
diarist-cli --data-dir %TEMP%\diarist-cli-data play --device "Speakers (HyperX QuadCast)" other.wav
```

`diarist-cli doctor` prints the NVML reading (`nvml  8409 MB free of 10240
MB`) next to the runtime's.

### From the CLI

```sh
diarist-cli play --device "Speakers (2- VB-Audio" talk.wav &
diarist-cli play --device "Speakers (HyperX QuadCast)" other.wav &
diarist-cli record --seconds 60 --mic "CABLE Output" --loopback "Speakers (HyperX QuadCast)"
```

### From the app

- **R** opens the start sheet (title, language, room mode, microphone and
  system-audio pickers preselected from `record.mic_device` /
  `record.loopback_device`); **Enter** starts. R never stops a recording.
- **Ctrl+.** or the Stop button asks "Stop recording?" every time;
  **Enter** confirms, Esc keeps recording. **Ctrl+Alt+R** (global) and the
  tray's Stop ask too: with the window on screen they bring it forward and
  show the same question (`live:toggled` `confirm_stop`), and a second
  press while it is open answers it; with the window hidden or minimized
  the first press arms (a Windows notification and the tray item ask "Stop
  recording?", `stop_armed`) and a second press within 3 s stops. The
  wording lives in `lib/stopConfirm.ts` (and `live.rs` for the
  notification).
- The live server starts when the start sheet opens or Ctrl+Alt+R starts a
  recording, not at launch (`engine.preload` = true starts it at launch), and
  stops after `engine.idle_minutes` (default 10, 0 = never) without a
  recording; the log line `live server starting: ... | idle stop after N min`
  shows the value it was launched with.
- Before the sheet opens, `check_ready("record")` runs; when something is
  missing (engine, models, a streaming model, a usable GPU for the CUDA
  build) an inline panel lists it with Install/Download, Copy link and Open
  folder instead. Import does the same with `check_ready("import")`.
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
  kept (the live run is deleted unless `keep_live_runs` or the post-pass
  recorded `engine_info.unplaced_names`); otherwise the live
  run becomes current with `engine_info.recovery` explaining why, and the
  meeting is done.

The app log says what it did (`recovered meeting 3 (was processing):
PostPassQueued; ...`). To try it, end the app from Task Manager while it
records (or while it shows "Finalizing") and start it again.

### Smoke run over DevTools

A scratch data root whose `models` and `engine` are junctions to the repo
copies (see "End-to-end on the GPU" above), the debug app started with
`DIARIST_DATA_DIR` set to it and
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

The app log goes to `<data root>\logs\diarist.log`.

## First run, downloads and the data folder (Phase 5)

Checked on the reference machine on 15 Oct 2026 with scratch data roots set
through `HKCU\Software\Diarist\DataRoot` (and removed afterwards).

- `setup_status` scans, installs runtime zips dropped into `engine\`
  (`download::install_dropped_engines`, SHA-checked), and reports the GPU
  (NVML: name, VRAM, driver, CUDA version; DXGI for other vendors), the
  recommended runtime package (`cuda`, `vulkan`, `cpu`), free disk and every
  download with URL, SHA-256, size and folder. The first launch opens the
  setup screen until the user picks "Set up later" (`setup.later`).
- Engine zips: `download_file` (ureq, platform verifier) resumes
  `<zip>.part` with Range requests; each request runs at most two minutes,
  three requests in a row without data end it as stalled.
- Models: `nemo-speech pull` downloads with System32 `curl.exe` into
  `<file>.partial` in the model's folder; `pull_with_progress` polls that
  folder (opening each file, since a directory listing keeps the old size of
  a file being written) and stops a pull after two minutes without new bytes.
  `models.user.json` entries (`"direct": true` by default) are fetched by our
  own downloader into `models\user\`.
- Hand-placed manifest models without the runtime's `.verified` sidecar are
  hashed once on scan and cached in `<file>.sha256`; a mismatch is reported
  and the file is not treated as the manifest model. Linked folders under
  `models\` are scanned.
- Observed connections: the engine zip from `diarist.exe` (github.com,
  then GitHub's download CDN on 185.199.108-111.133); model pulls only from
  `C:\Windows\System32\curl.exe` (huggingface.co, us.aws.cdn.hf.co).
- Move data folder: `move_data_root` copies (database through the backup
  API, then audio, engine, models, manifests; logs, window state and the
  pre-upgrade `diarist.db.bak-*` as they are), verifies sizes, writes the registry key (cleared when the target is
  the default root) and leaves `moved-from.json`; the UI restarts the app and
  the next launch deletes the copied items from the old root, retrying for up
  to 10 s while the exiting instance still holds files.
- Rename migration (`models::legacy`): when the data root has no
  `diarist.db` and a data root from before the rename to Diarist exists
  (the old registry value, else the old default folder), the app runs on an
  empty `MemoryStore` and asks once whether to move it in (`migrate_legacy_data`,
  the same copy, verify, restart and cleanup as Move data folder, plus
  removing the old registry value) or start fresh (`start_fresh` creates
  `diarist.db`). Test it only against a scratch copy:
  `DIARIST_LEGACY_ROOT=<copy of an old root>` with `DIARIST_DATA_DIR=<empty
  folder>` (without `DIARIST_LEGACY_ROOT`, a `DIARIST_DATA_DIR` run never
  offers the move).
- Retention runs at launch and every 24 h (`maintenance.rs`);
  `diarist-cli retention --dry-run|--apply [--days N]` does the same from the CLI.
- `health_check` is the Diagnostics checklist; `diagnostics_report` (Copy
  diagnostics) includes it.

A quick way to drive the app is the DevTools protocol (see "Smoke run over
DevTools"): `window.__TAURI_INTERNALS__.invoke('setup_status')`,
`invoke('check_ready', {action: 'record'})`, `invoke('health_check')`,
`invoke('move_data_root', {to})`.

## Release measurements

Release build of 15 Oct 2026 on the reference machine:

| What | Value |
|---|---|
| Installer | 6.67 MiB, 6,990,679 bytes (`Diarist_0.1.0_x64-setup.exe`, plus a 440-byte `.sig`) |
| Exe | 24.9 MiB, 26,150,400 bytes |
| Release build time | 5 min 47 s (LTO, codegen-units 1, warm dependency cache) |
| Start, second and later launches | 756 to 779 ms from process start to the "interactive" log line (3 runs); 113 to 136 ms after navigation start (`performance.now()`); library with 0 meetings, scratch data root |
| Start, first launch of a new exe | 6.8 s (cold file cache and the first antivirus scan of the new binary) |
| Import, AMI ES2002a WAV (21 min 13 s) | 45.4 s wall (about 28x real time), EN model and Nemotron 3 Diarization, 4 speakers |
| Live server ready after the sheet opened | 3.2 s, holds 1099 MB of VRAM |
| Downloads on this network | engine zip about 9.5 MB/s after about 15 s of connection setup (a stuck first CDN address); diarizer pull 102 MB in about 15 s |

## Install and uninstall test (15 Oct 2026)

On the reference machine, with the real data root protected: a byte copy and
a SHA-256 manifest of `%LOCALAPPDATA%\app.diarist.desktop` first (232
files), `HKCU\Software\Diarist\DataRoot` pointed at an empty scratch
folder, and the real root renamed aside only for the ticked uninstall (the
template removes `%LOCALAPPDATA%\app.diarist.desktop` whenever the box is
ticked).

| Step | Result |
|---|---|
| `Diarist_0.1.0_x64-setup.exe /S` | 13 s; `%LOCALAPPDATA%\Diarist\{diarist.exe, uninstall.exe}`, Start menu entry, uninstall key |
| Launch the installed app | setup screen for the empty scratch root (GPU, driver 610.88, CUDA 13.3; Install engine (98 MB), Install recommended models (770 MB)); log, WebView2 profile and window state written under the scratch root |
| Uninstall, box unticked (driven with BM_CLICK on the NSIS controls) | question "Remove the downloaded engine and models (903 MB)? Transcripts and recordings are kept in ...", answered Yes: `engine\` and `models\` removed; database (the AMI transcript), `audio\`, logs and the DataRoot key kept; program, Start menu entry and uninstall key removed |
| Reinstall `/S`, uninstall with the box ticked | no question; the scratch root's items and then the folder removed, `HKCU\Software\Diarist` removed, `%APPDATA%\app.diarist.desktop` removed |
| Real data root afterwards | renamed back; 232 files with the same sizes and SHA-256 as before the test |

UI Automation exposes the NSIS controls only as panes, so the test script
sends `BM_GETCHECK` / `BM_CLICK` to the checkbox and buttons by window handle.
