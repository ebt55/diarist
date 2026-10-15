# Changelog

All notable changes to Diarist. Versions follow semantic versioning.

## [0.1.0] - unreleased

The first public release.

### Renamed to Diarist

- The project was called meet-assist until 15 Oct 2026 and is now Diarist:
  app `diarist.exe` (installer `Diarist_<version>_x64-setup.exe`), data
  folder `%LOCALAPPDATA%\app.diarist.desktop`, database `diarist.db`,
  registry key `HKCU\Software\Diarist\DataRoot`, CLI `diarist-cli`, crates
  `diarist-core` and `diarist-cli`, environment variables `DIARIST_*`
  (`MEET_ASSIST_CIMC_WAV` is now `DIARIST_LONGFILE_WAV`), repository
  `ebt55/diarist`.
- The first start under the new name offers once to move existing
  meet-assist data (`%LOCALAPPDATA%\app.meetassist.desktop`, or the folder
  named by `HKCU\Software\meet-assist\DataRoot`) into Diarist's data folder,
  with the same copy, check and cleanup as Move data folder, or to start
  fresh.

### Transcription

- Import audio and video files (AAC, MP3, FLAC, WAV, Ogg/Vorbis directly;
  other formats through Windows Media Foundation or ffmpeg) and transcribe
  them with speaker labels on the GPU, about 27 times faster than real time
  on an RTX 3080.
- Live recording of the microphone ("me") and the system audio (diarized into
  up to 8 speakers), with text in about a second, level meters, automatic gain
  for quiet system audio, and a post-pass over the recorded channels when you
  stop. Names given during the meeting carry over.
- Crash recovery: an interrupted recording or post-pass is finished at the
  next start; an interrupted import is marked failed; a notice says what was
  recovered.

### Library

- Full-text search across all transcripts, speaker rename, merge, recolour,
  reassign and split, playback with a waveform, export to Markdown, text,
  JSON, SRT and VTT.
- Recordings kept as lossless FLAC; audio retention (30 days by default,
  pinned meetings exempt, transcripts never deleted), at launch and daily,
  with the reclaimable space in Settings; `diarist-cli retention`.

### Setup and models

- First-run screen: GPU (name, VRAM, driver, CUDA version), disk, the runtime
  package for this GPU (CUDA, Vulkan or CPU) and the recommended models, each
  with progress, Retry (resumes), Copy link, SHA-256, size and folder. "Set
  up later" is always possible; Import and Record say what they miss.
- Files put into `models\` or `engine\` by hand are recognised on Refresh
  (hash-checked; runtime zips extracted). Add from URL and Add from file for
  models the app does not list; `models.user.json` for extra recommended
  entries; unrecognised files can be used with a warning.
- Check for newer runtime lists NeMo-Speech.cpp releases from GitHub and
  installs a package next to the others.

### Desktop

- Per-user NSIS installer (no administrator rights); the uninstaller keeps
  transcripts unless "Delete the application data" is ticked, and otherwise
  offers to remove the engine and models.
- Data folder relocatable from Settings (copy, verify, restart, clean up);
  portable mode with `portable.txt`.
- Tray icon with a recording indicator; Ctrl+Alt+R records from any app and
  asks before stopping; one instance at a time; start with Windows; window
  size and position remembered.
- The live engine loads on the first recording and unloads after 10 idle
  minutes; low-VRAM mode for imports.
- Diagnostics: "Why isn't it working?" checklist with fixes, Copy diagnostics,
  the app log and the engine's last errors.
- Check for updates (manual), signed with the project's updater key.

### Known limits

- Not code-signed yet (SmartScreen warns once); the NVIDIA runtime is
  unsigned too.
- Vulkan and CPU runtime packages are untested; at most 8 speakers per
  recording; the Nemotron models punctuate little.
