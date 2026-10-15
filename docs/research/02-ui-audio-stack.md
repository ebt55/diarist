# Research: Windows UI stack and audio capture from Rust (verified 15 Oct 2026)

Opus 5.5 research agent. [C] = confirmed in a primary source. [U] = unconfirmed or design inference.

## Tauri 2 on Windows
- Latest stable 2.12.4 (14 Oct 2026); 3.0.0-alpha.5 the same day, so plan a v3 migration later. [C] crates.io
- WebView2 install mode: `downloadBootstrapper` is default and adds 0 MB; `embedBootstrapper` about 1.8 MB; `offlineInstaller` about 127 MB; `fixedVersion` about 180 MB. Windows 10 (Apr 2018+) and 11 ship WebView2. Keep default. [C] v2.tauri.app/distribute/windows-installer
- Installer: NSIS (`-setup.exe`, per-user default, hooks, multi-language). MSI uses WiX v3, Windows-only build, needs VBSCRIPT. [C]
- Size and cold start: third-party 3 to 15 MB installers, 190 to 380 ms cold start. [U] Create the window hidden and show after first paint to hide the startup flash. CUDA runtime DLLs bundled with the sidecar (cuBLAS etc, often >100 MB) will break a 20 MB installer target; download them alongside the models instead. [U]
- Sidecar: `bundle.externalBin` (file needs `-<target-triple>` suffix); `app.shell().sidecar("name").spawn()` returns `(Receiver<CommandEvent>, CommandChild)`. `CommandEvent::Stdout(Vec<u8>)` is one line per event unless `set_raw_out`. `child.write()` for stdin. [C] tauri-plugin-shell 2.5.0. For binary PCM into the engine, plain `tokio::process` or a named pipe is simpler. [U]
- Tray built into core (`tray-icon` feature); global-shortcut and autostart plugins support Windows. Vibe ships all three. [C]
- Past WebView2 rendering/resize bugs (tauri#9740, fixed in wry 0.39.4). wry hides white flash only on WebView2 134+. WebView2 is multi-process, costs RAM. [U]

## Alternatives
- Slint 1.18.1: GPLv3, or royalty-free desktop licence with attribution, or commercial. `ListView` virtualizes. Rich inline highlighting weaker than HTML. [C/U]
- egui/eframe 0.36.2: tiny, instant start; `ScrollArea::show_rows` needs one row height, so variable-height rows need manual `show_viewport`. No hot reload. [C]
- Iced 0.14.0 (Dec 2025): hot reload, time-travel debugging, table/grid widgets; no list virtualization mentioned; pre-1.0 churn. [C]
- Dioxus 0.7.10: desktop uses the system WebView (wry), so same WebView2 as Tauri; subsecond Rust hot-patching; native Blitz renderer is WIP. [C]
- Flutter Windows: needs Dart-Rust FFI bridge, probably 20 MB+. [U]
- WinUI 3 / MAUI: C#/C++, not Rust; self-contained deployment large; weak for Linux later. [C]
- Electron: 244 MiB vs 8.6 MiB Tauri in one measurement. [U]
- PySide6/Flet: Python runtime plus Qt/Flutter, likely >50 MB, slow cold start. [U]

## Recommendation: Tauri 2
A long variable-height transcript with search highlighting, text selection, colour pickers and accessibility is cheapest in HTML/CSS. Audio, DB and sidecar live in Rust; the WebView only renders. Pure-Rust GUIs win on size and start but need custom virtualization (egui), have weaker rich text (Slint), or are pre-1.0 (Iced). Fallback if WebView2 proves unacceptable: Slint under GPLv3.

## Frontend: Svelte 5
Svelte 5.57.2; runes give fine-grained updates so a label revision re-renders one row. SolidJS 1.9.17 equally fine-grained, smaller ecosystem. React 19.3.1 needs disciplined memoization (Vibe and anarlog use React). Virtualization: virtua (Svelte and Solid, dynamic sizing, reverse scroll, about 3 kB gzipped) [C]; TanStack Virtual also has Svelte/Solid adapters [C].

## cpal 0.18.2 (16 Aug 2026)
- Loopback: `build_input_stream` on an output Device; cpal adds `AUDCLNT_STREAMFLAGS_LOOPBACK` when data flow is eRender. Undocumented on docs.rs. [C] src/host/wasapi/device.rs
- Silence: capture thread waits `INFINITE`; no `AUDCLNT_BUFFERFLAGS_SILENT` handling found. During silence no callbacks fire, so zero-fill yourself. [C]
- Device events: no public hotplug API. Default streams follow system default since 0.18.0; `DeviceChanged` removed in 0.18.2, restored on unreleased main. [C] Whether a loopback stream on the default device follows default changes: [U].
- Process loopback: not supported. [C]

## wasapi crate 0.26.0 (11 Oct 2026)
- Loopback: `initialize_client` on a Render device with `Direction::Capture` in shared mode. Exclusive returns error. [C]
- Event-driven (EventsShared) or polling (PollingShared); `Handle::wait_for_event(timeout_ms)` lets you time out and synthesize silence. [C]
- Process loopback: `AudioClient::new_application_loopback_client(pid, include_tree)`; `get_device_period` and `get_audioclock` unavailable in that mode. [C]
- Device notifications: `register_notification_callback` covers added/removed and default-device changes. [C]
- windows-rs exposes `ActivateAudioInterfaceAsync`, process-loopback params and `IAcousticEchoCancellationControl`. [C]
- **Warning:** process loopback (include or exclude) returns all-zero buffers for new Teams (`ms-teams.exe`); Microsoft issue open; anarlog reverted to endpoint loopback. [C] microsoft/windows-classic-samples#414, fastrepl/anarlog#7590

## Resampler: rubato 5.0.1 (1 Oct 2026)
- `Fft` synchronous, fixed ratio; 48 to 16 kHz is exactly 3:1. `Async::new_sinc` same quality, most CPU, adjustable ratio. `Async::new_poly` has no anti-alias filter: avoid for downsampling. `Slip` corrects drift with no added delay. Real-time safe via `process_into_buffer`. [C]
- Major version went 1 to 5 during 2026: pin it. [C]
- Pick: `Fft` per stream plus `Slip` or small `Async` ratio adjustment for drift.

## Timestamps for alignment
- `pu64QPCPosition` = QPC when the first frame was recorded, 100 ns units. [C] MS Learn IAudioCaptureClient::GetBuffer
- cpal: `InputCallbackInfo.timestamp().capture` from qpc_position; shared timebase; includes pipeline latency since 0.18. [C]
- wasapi-rs: `BufferInfo { flags{silent, data_discontinuity, timestamp_error}, index, timestamp }`. [C]
- Design: place each packet on a QPC timeline; zero-fill gaps and SILENT packets; fit frames against QPC to estimate drift; feed correction to resampler. [U]

## Echo when using speakers
- `IAcousticEchoCancellationControl` needs build 22621+; `GetService` after `Initialize`; `E_NOINTERFACE` means unsupported. [C]
- wasapi-rs wraps it: `is_aec_supported`, `get_aec_control().set_echo_cancellation_render_endpoint`; its `aec` example sets `StreamCategory::Communications`. [C] Effectiveness depends on OEM processing. [U]
- Cheaper: gate each mic voice segment against time-aligned loopback (cross-correlation at 0 to 300 ms lag plus energy ratio); drop or attribute segments loopback explains. [U]
- anarlog ships an ONNX neural AEC (`crates/aec`) [C]; pure-Rust `aec3` port of unknown quality [U].

## How Vibe and anarlog capture system audio
- Vibe: forked cpal, loopback via `build_input_stream` on output device; separate WAVs via hound, merged with ffmpeg. Issue #1592 (open, Sep 2026): "System audio recording loses silent gaps, causing progressive desynchronization with microphone". Issue #1309: averaging mix lost 9 dB (fixed). [C]
- anarlog: mic via cpal; Windows speaker via wasapi crate endpoint loopback, autoconverted to 44.1 kHz f32, first channel only. Every 2 s re-checks which endpoint a foreign process plays to and switches after two confirmations (follows Teams onto a headset). Joiner pads lagging side with zeros; drift tracker in ppm; rubato with audioadapter. [C] crates/audio-actual/src/speaker/windows.rs

## SQLite
- rusqlite 0.40.2 `bundled`: compiles SQLite with FTS5, FTS3, JSON1, RTREE, LOAD_EXTENSION; works with MSVC. [C]
- sqlite-vec: compiles statically via `cc`, registers with `sqlite3_auto_extension`; pre-1.0 (0.1.9). MSVC build not verified. [C/U]
- turso 0.8.3: FTS is Tantivy-based, not FTS5-compatible. Reject. [C]
- sqlx: async overhead not needed.

## Final recommendation
- UI: Tauri 2.12.x, NSIS, WebView2 downloadBootstrapper, window hidden until first paint, plan v3 migration.
- Frontend: Svelte 5 with virtua (TanStack Virtual as alternative).
- Audio: wasapi crate for both streams (QPC timestamps, SILENT flag, event waits with timeout, device notifications, AEC hook). Endpoint loopback following the active endpoint like anarlog. No process loopback (Teams returns zeros). Zero-fill gaps against QPC time. cpal only for cross-platform mic later.
- Resampler: rubato 5 `Fft` 48 to 16 kHz with `Slip`/`Async` drift adjustment.
- Database: rusqlite bundled (FTS5 included); sqlite-vec statically later.
