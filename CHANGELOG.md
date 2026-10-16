# Changelog

All notable changes to Diarist. Versions follow semantic versioning.

## [0.1.0-beta.2] - 2026-10-16

The second beta pre-release, and the first one that installed copies of
0.1.0-beta.1 can reach through Check for updates. Imports and the
processing after Stop are more accurate, languages follow the installed
speech models, and a crash now leaves a local report you can paste into a
bug report.

- Added: local crash reports. A crash of the app or the CLI (a panic or a
  native fault) leaves `logs\crash-<time>.txt` in the data folder, a native
  fault also a minidump `crash-<time>.dmp`, and an engine that exits on its
  own `logs\engine-crash-<time>.txt`. The next launch says "Diarist closed
  unexpectedly" with Open folder, Copy report (your user folder shown as
  `~`) and Dismiss; Diagnostics lists the reports. The newest five of each
  kind are kept. Nothing is sent anywhere. Errors in the window's scripts
  now reach the app log.
- Added: languages follow the model. The language lists show what the
  installed speech-to-text models cover (broad-coverage languages marked
  "reduced accuracy"); a recording or import in a language the selected
  model does not cover runs on an installed model that does (Nemotron 3.5),
  with a notice, or offers the Nemotron 3.5 download first. Imports ask for
  the language; the first-run screen asks for the meetings' language and
  picks the model for it. `diarist-cli transcribe --language` switches the
  same way.
- Changed: `diarist-cli record` switches the model by language like
  `transcribe` and the app (the given `--language`, else the default
  language), with the same notice.
- Fixed: Japanese and Chinese were transcribed with automatic language
  detection; the language is now passed to the model.
- Fixed: Install and restart ended an import or a post-pass in progress.
  It now waits, and says which job is running.
- Changed: imports and the post-pass after Stop use the speech model's full
  lookahead; live recording keeps the low-latency setting. On the full
  evaluation set, word error on the hand-labelled meeting falls from 19.8 to
  18.3 percent (EN model) and from 23.0 to 20.8 (Nemotron 3.5), every other
  meeting improves too, and the speech pass runs about twice as fast.
- Fixed: names given during a recording and edits made on the live
  transcript carry over to the final transcript even though the final pass
  now places words up to 0.2 s earlier.
- Changed: a new app icon in a light and a dark variant; the installer icon is
  the dark-background one, and the running app follows the taskbar theme.
- Fixed: the taskbar button showed the icon blurred (a 32 px image
  enlarged). The window, its taskbar button and the tray now take the
  icon's frame for the display's scale, including the 16, 20 and 24 px
  frames drawn on the pixel grid, and follow the taskbar: the paper page on
  a dark taskbar, the ink page on a light one.
- Added: Diagnostics' checklist has a "Microphone privacy" row. When
  Windows' "Microphone access" or "Let desktop apps access your microphone"
  is off, Windows hands Diarist silence instead of the microphone; the row
  says so and opens the privacy page.
- Fixed: a deleted meeting's id (or a transcript run's) could be given to the
  next one, so anything that remembered the old one by id saw the new one.
  Ids now come from a counter and are never reused. Before a database
  upgrade the app copies `diarist.db` to `diarist.db.bak-<old version>` next
  to it, keeping only the newest copy. The window also forgets what it kept
  for a deleted meeting (its post-pass error, a search hit's scroll, an
  Open button in a notice, the last recording's live text), and a meeting
  that is still recording or being finalized cannot be deleted.
- Fixed: the live view did not open for the second and later recordings of
  a session until Stop. A recording made after deleting the previous one
  gets that meeting id again, and the app took it for the recording it had
  already opened.
- Changed: imports and the post-pass after Stop run the speaker model with
  its offline geometry: fewer words land on the wrong speaker around a
  change (full set with Nemotron 3.5: 5.5 to 7.8 percent of the words near
  a change, from 5.7 to 8.2 with the lookahead alone; 41 to 18 on an in-room
  recording) and the speech pass runs another 30 percent faster; live
  recording keeps the streaming setting. On one meeting a participant with
  only 7 words is merged into another speaker.
- Fixed: playback was much quieter than the meeting itself (a video played
  on the PC and recorded came back faint, and the mix halved it again when
  the microphone channel was silent). The player now raises a quiet meeting
  so its loud speech peaks near full scale (worked out from the waveform,
  at most +24 dB, with a limiter on the loudest moments), has a volume
  slider kept across meetings, and an Auto level button that plays the
  audio as stored (settings `playback.normalize` and `playback.volume`).
  The stored audio and the waveform do not change.
- Added: a language a model names but cannot transcribe no longer ends in
  a transcript of unknown tokens. The language lists offer the 30
  transcription-ready and broad-coverage languages that an installed model
  lists, not every tag in its header (121 for Nemotron 3.5, Malayalam among
  them); any other tag can still be typed under "Other tag…", with a
  warning, and `languages.user.json` can add a language to the lists. An
  import or post-pass whose words come back mostly unknown tokens stops
  with "The model could not transcribe this language (unknown tokens)" and
  keeps the meeting.
- Fixed: the Vulkan and CPU runtime packages did not run because the app
  always asked for the CUDA device; the device now follows the installed
  package (Vulkan measured at 25 times real time on an RTX 3080, CPU at 3
  times).
- Changed: Nemotron 3.5 drops far fewer phrases on public language clips at
  the file setting (measured on FLEURS slices).

## [0.1.0-beta.1] - 2026-10-15

The first public release, as a beta pre-release.

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
