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
cargo run -p diarist-cli -- version                     # prints "diarist-cli 0.1.0-beta.4"
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
| `DIARIST_ECHO_MEETINGS` (live meeting folders, `;`-separated, read only; `DIARIST_ECHO_SAMPLE` for one) + `DIARIST_ECHO_DB` (a copy of the database) | `-p diarist-core --release --test echo_sample -- --nocapture`: per meeting the echo profile and its 7 s null (must never couple; `DIARIST_ECHO_NULL=1` replays the live monitor over the null too), then the layered gate on both paths: the post-pass path (the canceller where the post-pass runs it, its output's words from `mic-aec.words.json` or transcribed once with `DIARIST_ENGINE_DIR` + `DIARIST_MODELS_DIR` and cached in `DIARIST_ECHO_OUT`; where it runs, the transfer gate with every guard (`transfer` lines) on the live run's mic words and on the raw mic transcribed once with the engine (timed, cached as `<uid>.raw-words.json`; `mic.words.json` without an engine), the shipped one's row in the table) and the live path (the live run's every revision through the live gate in arrival order, the monitor fed in 100 ms frames; a meeting without a live run stands in its stored transcripts). Prints the acceptance row per meeting and path (dropped per layer, residual duplicates with the chance level, owner's words kept; `DIARIST_ECHO_CLASSES` adds the meeting 12 labels), and on both paths the text layer at the 300 and 500 ms lag spreads (`spread` lines). `DIARIST_ECHO_SWEEP=1` adds the sweeps (text window, floor percentile, correlation look-back and bands, after-canceller variants), `DIARIST_ECHO_DUMP` a per-word feature TSV with text, and per raw source each cancelled word's raw neighbours and guard decisions (local analysis; delete it), `DIARIST_ECHO_KEEP=1` keeps the cached words (docs/eval/2026-10-21-echo-round2.md, "Transfer gate") |
| `DIARIST_ECHO_FLEURS` + `DIARIST_ECHO_SAMPLE` + engine + models | `-p diarist-core --release --test echo_fleurs -- --ignored --nocapture`: FLEURS English utterances (`fleurs_fetch.py --langs en`) as the owner over a speaker meeting's far end with its echo path, the real post-pass with `echo.mode` off and auto (the canceller, then the gate) and the gate alone over the off run's words; prints "me" WER, echo words removed and double-talk words kept, then the headphone control (no echo: `auto` must give the words `off` gives; `DIARIST_ECHO_CONTROL=0` skips it); `DIARIST_ECHO_BANDS` sweeps the gate alone per band and look-back, `DIARIST_ECHO_AEC_SWEEP=1` the after-canceller variants; the transfer gate per guard over the auto run's words before the gate, the off run's words standing in for the raw mic (`DIARIST_ECHO_TRANSFER=0` skips it). The fixture of the eval docs is `DIARIST_ECHO_MINUTES=20` over meeting 12's far end (docs/eval/2026-10-20-echo.md, docs/eval/2026-10-21-echo-round2.md) |
| `DIARIST_ECHO_SAMPLE` + `DIARIST_ECHO_DB` (a copy) + `DIARIST_ECHO_CLASSES` + `DIARIST_ECHO_OUT` + engine + models | `-p diarist-core --release --test aec_postpass -- --ignored --nocapture` (about 10 minutes for 111 minutes of audio): the real post-pass over a stored speaker recording with `echo.mode` off and auto, the gate alone over the off run's words, scored per class (owner's words kept, with the fair count; duplicates against a 7 s null); the canceller's wall time; the spliced double-talk set of research 14 rebuilt and transcribed (owner's WER raw, cancelled, cancelled and gated, per after-canceller variant of the layered gate, and per guard of the transfer gate on the raw window's words). The auto post-pass's live run has no mic words, so it transcribes the raw mic once more for the transfer gate (`engine_info.echo.transfer.raw_wall_s`; 44 to 49 s more for 111 minutes). `DIARIST_ECHO_AGC_REPLAY=1` adds a post-pass with a gain track made by replaying the AGC backwards and the signal-level table (no track, 10 ms values, one per frame). Everything it writes under `DIARIST_ECHO_OUT` is deleted at the end (`DIARIST_ECHO_KEEP=1` keeps it; `DIARIST_ECHO_REUSE=1` reuses the post-pass results of the last kept run) (docs/eval/2026-10-20-echo.md, "Post-pass canceller (B2a)") |
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
- Echo (plan 15). The first detected coupling shows the echo warning with
  "Turn off for this recording" (**N** while it shows;
  `turn_off_echo_for_recording`, `Recorder::turn_off_echo`): the rest of the
  recording and its post-pass are not gated, the live run's
  `engine_info.echo` gets `user_off: true` and mode `off_for_recording` at
  once (a crash keeps it), and `LiveStateView.echo_off` turns the banner
  into "Echo handling is off for this recording". `live_snapshot` applies
  the recorder's current gate decisions (`Recorder::echo_hidden`), so a
  reloaded view shows what `live:words` showed.
- The meeting details read the current run's `engine_info.echo`: "Echo
  detected (coupling 0.72, 3 ms): N far-end words removed from "me"" with
  Restore, and "Echo cancellation on" when `aec` is set.
  `restore_echo_dropped(run_id)` (the meeting's current run) builds a new
  run of the same kind from the run's words plus its `echo_dropped` as
  `me` (engine labels kept, each channel re-segmented with the run's
  `engine_info.segmentation` and interleaved as the post-pass does), carries
  names, colours and reassignments over and makes the old run's merges
  again, writes `engine_info.echo.restored: true` (`restored_from`,
  `restored_words`, `dropped: 0`) and an empty `echo_dropped`, and makes it
  current; the old run stays, and restoring it again replaces the earlier
  restored run. A restored run offers "Remove echo words again"
  (`remove_restored_echo(run_id)`: the old run becomes current, nothing is
  deleted). `echo_dropped(run_id)` lists the removed words on demand ("Show
  them", each word's `reason`, the layer that removed it, as its tooltip;
  runs from before plan 15 section 9 have none); `get_meeting` never
  carries them. `engine_info.echo.layers` counts the removed words per
  layer (`text`, `attenuation`, `level`, `correlation`, `utterance`, and
  `transfer` when the post-pass transfer gate removed any), with
  `band`, `band_calibrated`, `text_matched`, `floor_db` and `owner_db`;
  after the post-pass canceller `engine_info.echo.transfer` has the raw
  mic's `source` (`live` or `transcribed`), `raw_words`, `raw_dropped`,
  `raw_layers`, `raw_band`, `raw_wall_s`, `gate_s`, the `guard` and its
  counts (`candidates`, `removed`, `text`, `level`, `plain`, `guarded`,
  `unmatched`), or `{"error": ...}` when the raw transcription failed. Both refuse while the meeting
  records or is finalized. When the post-pass echo step fails, the mic
  stays ungated and `engine_info.echo` is `{"error": "<message>"}`.
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
| `echo.mode` | `auto` | Recording with speakers (plan 15; Settings, "Recording with speakers (echo)": Detect and handle / Off / Always on): `auto` runs the echo monitor (2 s blocks, FFT correlation of the loopback against the mic, both band-passed to 300 to 3,400 Hz) and the layered echo gate on the mic words, live (what the view shows, and the live run at Stop) and in the post-pass (`audio::echo::layers`, plan 15 section 9): text matching against the loopback's words and the near-end level once the text matches show echo (8 or more words in delay-consistent runs of three, and at least 1 percent of the mic words), the correlation from the first detected coupling to the end of the recording, and an utterance rule; a recording that neither couples nor shows text matches (headphones without a leak) is left exactly as it was; `off` runs nothing and gates nothing; `always` runs the correlation regardless of detection. In the post-pass, a coupled recording whose echo return loss is under 20 dB (every recording with `always`) first goes through Diarist's echo canceller, whose output (`mic-aec.wav`) is transcribed instead of the raw mic; the transfer gate then decides on the raw mic's words (the live run's, else `mic.wav` transcribed once more) with the raw band and carries the echo decisions over to the cancelled words the canceller cut by 3 dB or more and that sit 12 dB or more under the owner's level (`audio::echo::transfer`, plan 15 section 9.2). "Turn off for this recording" overrides it for one recording |
| `echo.gate_threshold` | `0.5` | The neighbours' median correlation at or above which a mic word in the correlation layer's middle band (0.30 to 0.70) is dropped; words at 0.70 or more always go, under 0.30 always stay. With 20 or more text-matched words the band is calibrated per recording instead (drop bound at their 10th percentile within 0.55 to 0.80, keep bound at the 90th percentile of the owner-level words; this setting moves the neighbours' bound from the middle). After the post-pass echo canceller has run, the band over the cancelled transcript's words is 0.60 / threshold + 0.20 (0.70 by default) / 0.95, not calibrated (`band_after_canceller`; docs/eval/2026-10-21-echo-round2.md); the transfer gate's decision on the raw mic's words uses the raw band with this threshold (calibrated as live). Values outside 0.2 to 0.6 are clamped (Settings, under Advanced; the range comes from `ECHO_GATE` in `generated/settingDefaults.ts`). Removed words are listed in the run's `echo_dropped` column, the counts in `engine_info.echo`; Restore puts them back (above) |

The level meters show the gain the AGC applied; their tooltip shows the
peak before it.

### Punctuation settings

Plan 16 section 3.8; also in `settings::DEFAULTS`:

| Key | Default | Effect |
|---|---|---|
| `transcript.punctuation` | `auto` | `auto`: the post-pass, imports, Restore and "Add punctuation" punctuate the words when a punctuation model is selected, installed and covers the run's language (`punct::load`); `off`: never (`engine_info.punctuation.reason` is `off`). Nothing live is punctuated in beta.5 |
| `transcript.punctuation_threads` | `4` | ONNX Runtime intra-op threads, clamped to 1 to the machine's cores (`punct::threads`); no UI |
| `selected_model.punctuation` | `""` | The selected punctuation model file, like the other roles; the scan picks the recommended one |

Punctuation never changes a word, its times or its speaker: the display
text goes to `words.text`, the speech model's text to `words.raw_text`
where they differ (docs/02). `diarist-cli punctuate <meeting>` (or
`--all-missing`) adds it to finished meetings as a new run, `--remove`
takes it out; without a model it prints the reason and exits 0. The eval
data roots keep it `off` (plan 16 section 3.5).

### Crash recovery

At launch, before a recorder can start, `live::recover` finishes live
meetings a crash left behind:

- `recording`: WAV headers are repaired from the file lengths, the live run
  is materialised and made current, and the post-pass is queued.
- `processing` (the post-pass was queued or running) with both WAVs: the
  runs the interrupted post-pass wrote are deleted and it is queued again.
  A copy of the live run (Restore, a punctuation rebuild) that was current
  stays current, so the post-pass carries its names over; such copies are
  never taken for the live run.
- `processing` without WAVs: a finished `post_pass` run that is current is
  kept (the live run is deleted unless `keep_live_runs` or the post-pass
  recorded `engine_info.unplaced_names`); otherwise the live
  run becomes current with `engine_info.recovery` explaining why, and the
  meeting is done.

Before the first import can be queued, every launch also marks imports
left in `processing` as failed and deletes the runs an interrupted
punctuation rebuild or Restore left half-built (still `running`, not
current, `punctuation.rebuilt_from` or `echo.restored_from` set, meeting
`done` or `failed`): `pipeline::recover_interrupted_imports`.

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
- Engine zips and models: `download_file` (ureq, platform verifier,
  redirects followed, User-Agent `diarist/<version>`) resumes `<file>.part`
  with Range requests, each from the original URL so a resumed download
  gets a fresh redirect; each request runs at most two minutes, three
  requests in a row without data end it as stalled. Since 0.1.0-beta.5
  `download_model` fetches every manifest model from its `url` into
  `ModelEntry::dest_dir` (`<repo>\<revision>\` for shipped models, the
  layout `nemo-speech pull` used, so earlier pulls are found; `models\user\`
  for direct entries) and writes `<file>.sha256`. `nemo-speech pull`
  (System32 `curl.exe`, which fails where revocation checks are blocked) is
  kept for one release behind `diarist-cli models pull --via-runtime`.
- Manifest models without the runtime's `.verified` sidecar (hand-placed or
  downloaded by us) are hashed once on scan and cached in `<file>.sha256`
  (a download writes it directly); a mismatch is reported and the file is
  not treated as the manifest model. Linked folders under `models\` are
  scanned.
- Observed connections: the engine zip from `diarist.exe` (github.com,
  then GitHub's download CDN on 185.199.108-111.133); up to 0.1.0-beta.4,
  model pulls only from `C:\Windows\System32\curl.exe` (huggingface.co,
  us.aws.cdn.hf.co). Since 0.1.0-beta.5 models come from `diarist.exe`
  (`diarist-cli.exe` for the CLI) too.
- Long paths: `process::command` passes the runtime as `\\?\C:\...` from
  240 characters (`process::exe_arg`); `CreateProcessW` refuses the plain
  form at 260 or more with os error 3. Model, audio and output paths get the
  same form as arguments (`process::path_arg`), and so does the database
  (`DataPaths::db`: SQLite opens the plain form only below 260). Rust's own
  file calls handle long paths themselves.
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
