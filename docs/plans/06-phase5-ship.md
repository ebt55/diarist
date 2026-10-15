# Phase 5: ship

Status: draft. Depends on Phases 2 and 3. Decisions from docs/01 and research/06 C.

## Goal

A stranger with an NVIDIA GPU installs meet-assist from a 10 MB setup file, the first run fetches the engine and models with progress and verification, and the app is usable for file import within five minutes and for live recording after one more download. Upgrades keep data. Uninstall never silently deletes transcripts.

## Owns

`apps/desktop/**` (first-run flow, settings, tray, shortcuts, autostart, NSIS hooks), `crates/core/src/models` (installer orchestration), `crates/core/src/paths`, `README.md`, `docs/INSTALL.md`.

## Deliverables

1. **Paths.** `core::paths`: data root resolution order: `portable.txt` next to the exe (`<exe dir>\data`), then `HKCU\Software\meet-assist\DataRoot`, then Tauri `app_local_data_dir()`. "Move data folder" in Settings copies, verifies, updates the registry key, deletes the old copy, and asks for a restart. Asset-protocol scope and `NEMO_SPEECH_MODEL_DIR` follow the resolved root.
2. **First run.** A setup screen: GPU check (NVML; name, VRAM, driver), free-space check, then "Install engine (98 MB)" and "Install models (780 MB)" with per-item progress, resume and SHA verification; the streaming ASR (670 MB) is offered on the first Record. "Import from folder" accepts a folder containing the zip and GGUFs for offline machines. Errors show the exact URL and hash so the user can fetch manually.
3. **Engine lifecycle.** `ServeSupervisor` starts `nemo-speech serve` lazily on first Record (not at app launch unless Settings says so), stops it after 10 minutes idle, and restarts on crash with backoff. Settings: low-VRAM mode (Nemotron for files), "Prefer No Sysmem Fallback" how-to.
4. **Installer.** Tauri NSIS, per-user, default install dir, `downloadBootstrapper`; POSTUNINSTALL hook reads `HKCU\Software\meet-assist\DataRoot` first (this key is ours, not under the identifier keys the checkbox removes, so it is still present when the hook runs). If the delete-app-data checkbox is on and not updating: wipe the resolved data root too, so "delete app data" is true even after the user moved the folder, then delete the key. If the checkbox is off: prompt "Remove downloaded engine and models (1.6 GB)? Transcripts and recordings are kept." Updater: Tauri updater plugin against GitHub releases, signed with the Tauri updater key; app signing via SignPath OSS when accepted, documented as unsigned until then.
5. **Desktop integration.** Tray icon with Record/Stop and recording indicator; global shortcuts (default Ctrl+Alt+R record toggle, Ctrl+Alt+M open window); autostart toggle; single-instance plugin; window state remembered.
6. **Retention.** Daily job: delete `playback.flac` and WAVs of unpinned meetings older than the retention setting; never touches transcripts; shows reclaimable space in Settings, and states that re-diarization or fusion re-runs only reach meetings whose audio still exists. `meet-cli` gets `pin`/`unpin`; the eval harness pins its sessions.
7. **Diagnostics.** Log folder opener, "copy diagnostics" (versions, GPU, data root, last engine stderr), crash recovery report on launch.
8. **Docs.** README with screenshots, INSTALL.md, PRIVACY.md (nothing leaves the machine), LICENSES.md listing runtime and model licences, CONTRIBUTING.md.

## Acceptance

- Fresh Windows 11 VM or a second user account: install, first run, import a file, record 2 minutes, uninstall with the checkbox off keeps the DB and FLAC; with it on, removes everything.
- Upgrade from the previous build keeps data and settings.
- Release exe cold start to interactive under 1 s on this machine (measure; the debug build was 1.4 to 1.9 s).
- Installer under 12 MB.
