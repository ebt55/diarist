# Phase 8: hardening, performance and local crash capture

Status: proposed 15 Oct 2026, awaiting Ebin's go. Planner: Fable 5.1. Implementation: Opus 5.5 agents, one chunk at a time, each behind the eval harness and the soak tests described here.

## Why

0.1.0-beta.1 works end to end on one machine. "Mature harness" means it keeps working on machines and recordings we have never seen, tells the user what happened when it does not, and gets faster only where a measurement says it is slow. Nothing in this phase changes transcript quality on purpose; anything that could is gated by `tools/eval` on the fixed regression set (docs/05-eval-data.md) and ships only when no session gets worse.

## Rule for every chunk

Measure, change, measure again, keep the table in the chunk's PR. A change without a before and after number does not merge. Quality-affecting changes (anything in fusion, segmentation, gain, echo) run the full eval table and must not regress any session's WDER or cpWER by more than the noise band established in the first run.

## 8a. Crash capture that never phones home

The privacy promise (PRIVACY.md: "no crash reporting") stays. What changes is that a crash leaves a readable report on the user's own disk, and the app points at it next time. Nothing is sent anywhere, ever; "Copy report" puts text on the clipboard for the user to paste into an issue if they choose.

- **Rust panics** (app and CLI): a panic hook writes `logs\crash-<utc-timestamp>.txt` with app version, Windows build, GPU name and driver, the panic message, a backtrace captured with `Backtrace::force_capture`, and the last 200 lines of the app log. The hook must not allocate much or take locks that the panicking thread may hold; write to a pre-opened file handle.
- **Native crashes** (access violation, stack overflow, abort): the `crash-handler` and `minidumper` crates (Embark, MIT or Apache-2.0) write `logs\crash-<utc>.dmp` locally. Keep the last five reports; delete older ones at launch.
- **Engine exits**: `ServeSupervisor` and the CLI engine already keep the exit code and a stderr tail; persist them as `logs\engine-crash-<utc>.txt` and link them from Diagnostics.
- **WebView errors**: `window.onerror` and `unhandledrejection` forward to the app log through a Tauri command; no user content in the message, only the error text and stack.
- **Surfacing**: at next launch, if a report newer than the last acknowledged one exists, a quiet notice says "Diarist closed unexpectedly at <time>. A report is on your disk." with Open folder, Copy report, Dismiss. Diagnostics lists reports. Copy report replaces the user's profile path with `~` so a pasted report does not leak the Windows user name.
- **Docs**: PRIVACY.md gains a paragraph: crash reports are files in the data folder, never transmitted, deleted with the data folder. README's privacy section says the same in one line.
- **Verification**: a hidden `diarist-cli crash --panic|--segv|--engine` reproduces each path; the report appears, the notice appears once, Copy report has no profile path, the app log is intact.

## 8b. Performance, measured first

Build `diarist-cli bench` (or a `tools/bench` script) that reports a table, then fix only what the table says. Targets are from docs/00-product-spec.md.

| Measurement | How | Target |
|---|---|---|
| Cold start to visible window | Process start to first paint, 5 runs, warm disk | under 1 s |
| Import real-time factor | 77-minute and 2-hour eval files, GPU warm | under 0.1, no regression from 0.037 |
| Post-pass after Stop | per recorded minute, two channels | under 1 s per minute |
| Live first-word latency | integration test with synthetic playback | under 1.5 s, both channels |
| Search latency | seeded library of 2,000 meetings, 20 common queries | under 50 ms per query |
| Memory | RSS of app and engine over a 4-hour recording | flat after the first 10 minutes |
| VRAM | idle, live two sessions, import | within the budgets in docs/04 |
| Database size | per meeting-hour, with FTS | documented, with vacuum behaviour |
| Installer | size and install time | under 10 MB, under 20 s |

Likely fixes, only if measured: do nothing blocking on the main thread at launch (NVML, model scan, retention all deferred); FTS prefix queries and `rank` ordering; batch inserts in one transaction during materialisation; waveform cache on disk; FLAC encoding off the critical path after Stop; keep the file-mode engine process warm between imports in the same session.

## 8c. Soak and fault tests

Automated where possible (integration tests behind a `soak` feature or a nightly script), manual where not, each with a written result in `docs/eval/soak-<date>.md`:

- 4-hour live recording with synthetic playback: no gap growth, anchors within 10 ppm, memory flat, WAV and FLAC lengths match.
- Device storms: default device changes every 30 s for 10 minutes; headphone plug and unplug; the recording never stops and every change leaves an anchor.
- Suspend and resume mid-recording: audio resumes, the gap is recorded, the transcript is intact.
- Disk full during recording and during import: the app says so, nothing already written is lost, the meeting is recoverable.
- GPU loss: kill the engine process mid-job and simulate a driver reset; the job fails cleanly, the queue continues, the user sees the cause.
- Bad inputs: truncated and corrupted GGUF headers, a zero-byte model, a runtime zip with a missing DLL, media files that symphonia rejects and Media Foundation accepts, a 3-hour recording, a file with no speech.
- Library scale: 5,000 seeded meetings; list, search and open stay responsive.
- Two instances, a second launch while recording, launch during the post-pass: single-instance and recovery behave.
- Crash recovery matrix: kill the app during recording, during the post-pass, during import, during Move data folder; each recovers as documented.

Property tests (proptest) for the GGUF header parser, the WAV header repair, the clock-anchor mapping and the segment interleaver. No cargo-fuzz on Windows; corrupted-file fixtures instead.

## 8d. Quality gate as a habit

- `tools/eval` runs on the fixed set before every release; the table goes into `docs/eval/<date>-<model>.md` and the release notes link it.
- CONTRIBUTING.md states the rule: no quality-affecting change without the eval table in the PR.
- Hand-correct the Zoom references for at least two sessions so the baseline stops depending on Zoom's own labels.

## 8e. Ship quality (decisions for Ebin, not code)

- Code signing: an OV certificate removes the SmartScreen prompt over time (an EV one immediately). Cost is a few hundred dollars a year; without it the beta keeps the "Run anyway" step. Not urgent for a beta.
- Winget manifest once a stable 0.1.0 exists, so `winget install Diarist` works.
- A release checklist in docs/DEV.md: version bump, changelog, eval table, build, draft, publish, verify the updater manifest.

## 8f. Ship `diarist-cli` in the installer

Coding agents and scripts set Diarist up for people (docs/SETUP-WITH-AN-AGENT.md). Today the installer carries only the app, so an agent can place files but cannot select a package or model, turn on low-VRAM mode or run `doctor`; it has to tell the user what to click. Bundling `diarist-cli.exe` next to `diarist.exe` (Tauri `bundle.resources` or `externalBin`, a few MB) turns the whole setup into commands with checkable output: `diarist-cli engine install`, `models pull`, `models select`, `doctor`. Settings that live in the database get CLI verbs for the few an agent needs (`settings set engine.low_vram true`). The setup guide is then updated to prefer the CLI and keep the file-placement path as the fallback.

## Order

8a first (small, self-contained, closes the open question about crash logs). Then 8b measurement only, so the numbers exist before anything is optimised. Then 8c in two or three chunks. 8b fixes and 8d interleave with whatever the measurements and soaks turn up.

## Out of scope

Fusion (docs/plans/06), echo handling (docs/plans/04 section 3f), speaker identity, punctuation restoration. Each is a feature phase with its own plan and its own eval gate.

## Small things noticed along the way

- While recording, the sidebar brand truncates to "Di..." because the Record button grows into "0:59 Stop" (docs/screenshots/recording-light.png, 15 Oct 2026). Give the timer its own space or let the brand collapse to the mark.
- Stop asks "Stop recording?" even for a recording under a minute; keep the confirmation (the user chose it) but make sure the Ctrl+Alt+R path and the tray path show the same dialog.
- `%APPDATA%\app.diarist.desktop` is created empty by the WebView or Tauri on first launch, next to the real data folder under `%LOCALAPPDATA%`; harmless, but the uninstaller's "Delete the application data" should remove it too.
