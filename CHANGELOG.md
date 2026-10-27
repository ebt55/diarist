# Changelog

All notable changes to Diarist. Versions follow semantic versioning.

## [0.1.0-beta.5] - 2026-10-27

The fifth beta pre-release. Transcripts get punctuation and sentence
capitals after Stop and on imports, from a small model on the CPU that
never changes a word. The EN model's bursts of one repeated word are
collapsed, the live view runs with more lookahead and makes fewer
mistakes, the echo gate removes far more of a loud echo's residue when
you say almost nothing, and a finished recording can be processed again
from its stored audio. Models now come through Diarist's own downloader.
On the evaluation set (36 recordings, 28 hours) word error falls from
11.01 to 10.85 percent with the EN model and stays at 11.30 with
Nemotron 3.5; no recording got worse. Measured on one PC.

- Added: punctuation and capitals. After Stop (on both channels) and on
  every import, a punctuation model reads the finished words and adds
  `.` `,` and `?` (in the script's own forms, such as `।` for Hindi and
  `。` `？` for Japanese and Chinese), and Diarist raises the first letter
  of each turn and of every word after a sentence end. Lines of the
  transcript now end at sentence ends where the pause allows. Words, their
  times and their speakers never change: on the evaluation set word and
  speaker error are identical with and without it, with both speech
  models, and Diarist's own checks rejected none of its 533,000 words. A
  mark the speech model wrote itself stays. The model is Horizon-Labs
  punctuation-restoration-small (35 languages; no commas for Hindi,
  Japanese and Chinese; Thai is not punctuated), a 306 MB download that
  the first-run screen fetches with the other models and that installed
  copies offer once ("New: punctuation", Download or Later). It runs in
  Diarist's own process through ONNX Runtime 1.31.0 (installed beside the
  model as `diarist-onnxruntime.dll`): about 5 to 6 seconds of CPU per
  hour of audio (5.3 to 5.6 on the evaluation set), 0.65 GB of RAM while
  it runs, no VRAM. The speech model's own text is kept with each changed
  word, so "Remove punctuation" on a meeting goes back to it exactly and
  "Add punctuation" adds it again. Settings, Models, Punctuation shows the
  model, "New transcripts: Add automatically" or Off, and "Add punctuation
  to N older meetings", which builds a punctuated copy of each and keeps
  the earlier transcripts; meetings from earlier versions stay as they are
  until you ask. Diagnostics gains a "Punctuation runtime" row, and
  `diarist-cli punctuate <meeting>` (or `--all-missing`, `--remove`) does
  the same from the command line. The JSON export gives each changed word
  its `raw_text`. The live view stays unpunctuated. About two in three of
  its periods and question marks, and four in five of its commas, are
  where Zoom's own transcripts put them, but it writes about half as many
  commas as Zoom does (docs/KNOWN-ISSUES.md).
- Added: Process again. A finished recording whose live transcript was
  kept (Settings, "Keep the live transcript") can be processed again from
  its stored `playback.flac`, with today's models and settings: the
  meeting details offer "Process again". The FLAC is only read, never
  rewritten; the two WAVs decoded from it are deleted when the job ends,
  whatever happens. The new transcript becomes the one shown and every
  earlier one stays; names given while recording carry over. On copies of
  three of the author's meetings it reproduced the stored words (1,191,
  2,192 and 11,596 of as many), the removed echo words and the echo
  summary exactly. Recordings made with the earlier echo handling get
  today's this way, one click each.
- Changed: the EN model sometimes writes one word many times in a burst
  ("the the the the ..."), several copies on one 80 ms frame. After Stop,
  on imports, on Restore and when punctuation is added or removed, each
  burst is now kept as one word spanning it. Repeats at speaking pace
  ("no no no no", a stutter, laughter), pairs, numbers and Chinese,
  Japanese and Thai words stay. On the evaluation set it removed 438 words
  with the EN model (10 with Nemotron 3.5), word error fell on 21 of the
  36 recordings and rose on none; on the author's own meetings it removed
  215 words, at least 89 percent of them insertions that Whisper and
  Nemotron 3.5 do not have. The meeting details say how many bursts were
  collapsed. On by default; `transcript.collapse_loops = off` keeps every
  word in transcripts built afterwards (no switch in the app, docs/DEV.md).
- Changed: live recording runs both Nemotron models with 560 ms of
  lookahead (6 frames) instead of the runtime's 160 ms. The live text has
  fewer errors (word error at the live setting on the hand-labelled AMI
  meeting from 19.75 to 18.03 percent with the EN model and from 23.05 to
  21.55 with Nemotron 3.5, and lower on all six Zoom meetings measured)
  and live words sit closer to their real times; the first words of an
  utterance show about half a second after they are spoken (median 0.48
  to 0.52 s, against 0.14 s). VRAM is unchanged. The setting
  `engine.live_lookahead = low` goes back to the runtime default.
- Changed: recording with speakers when you say almost nothing. After the
  echo canceller, the gate took your level from the words left over, and
  on a long talk where you hardly speak those are what is left of the
  echo, so its level check sat below the microphone's noise floor and the
  residue stayed in "me" (on one 52-minute talk, 288 words, about 250 of
  them echo). Your level now comes only from words the canceller left
  alone, and only when it sits at least 6 dB over the noise floor. When
  it cannot be read, a word that the gate on the raw microphone takes for
  echo and that the canceller cut by 3 dB or more goes when it is at most
  12 dB over the floor, or 18 dB when the recording shows a silent owner
  (no readable level, at most 10 of the words the canceller left alone 18
  dB or more over the floor, and at least 99.8 percent of the raw
  microphone's words judged echo). On that talk "me" now keeps 77 words,
  and the ones that look like echo went from 253 to 42; every other
  recording and test fixture comes out exactly as before. The run's details record where
  the level came from (`engine_info.echo.owner_source`, `owner_low`,
  `owner_high`, and `floor_rule` under `transfer`). Stored meetings change
  only with Process again.
- Changed: models are downloaded by Diarist itself, no longer by the
  runtime through Windows' `curl.exe`, which failed on networks that block
  certificate revocation checks (`CRYPT_E_REVOCATION_OFFLINE`). It is the
  downloader the engine install already used: https only, resumes an
  interrupted download where it stopped (only from the same address and
  for the same hash), checks size and SHA-256 against the manifest, and
  gives up on a connection that stays silent for two minutes. The
  firewall now sees `diarist.exe` for every download (docs/INSTALL.md).
  Models fetched by earlier versions stay where they are. A model of
  several files lists each file's address, hash and Copy link on the
  first-run screen and in Settings. `diarist-cli models pull --via-runtime`
  keeps the old path for this release.
- Changed: the first start upgrades the database (schema 3 to 5) and
  keeps a copy of the old one as `diarist.db.bak-3` next to it.
  0.1.0-beta.4 cannot open the upgraded database, so going back to it
  means restoring that copy.
- Fixed: imports and recordings failed in a data folder with a long path
  ("path not found", os error 3, once the runtime's path reached 260
  characters), and so did the database ("unable to open database file").
  Diarist now hands long paths to the engine, to SQLite and to ONNX
  Runtime in Windows' extended-length form, also for a folder on a
  network share; checked with a 264-character data folder.
- Fixed: Restore carries the speakers over by their label instead of by
  the times they speak, so a speaker with only a few words keeps their
  name and colour instead of coming back as "new label". Names given on
  a restored or punctuated copy of the transcript, or on a live
  transcript that was kept as the final one, are kept when the meeting is
  processed again.
- Fixed: a failed engine install no longer marks the queued model
  downloads as failed. Add from URL takes https links only, and says so.
  Engine errors show paths in their plain form instead of `\\?\C:\...`.
  A `.meet.json` sidecar that does not parse shows its error in Settings,
  and `models.json` and `models.user.json` entries with an unsafe path
  (`..`, an absolute path, a device name) or a file without a hash are
  refused, naming the entry and the field.
- Known: after the echo canceller, your own words spoken entirely over
  loud speech from the other side can still be removed (about one in four
  on a test fixture); headphones avoid it. docs/KNOWN-ISSUES.md has this
  and the other open rows.

## [0.1.0-beta.4] - 2026-10-22

The fourth beta pre-release. Diarist can now record a meeting played on
speakers: when the microphone hears the other side, the echoed words are
kept out of "me", live and in the final transcript, and the processing
after Stop cancels the echo before it transcribes you. Measured on one PC
with five recordings; headphones still give the cleanest "me". The
copyright holder is now named, and the Diarist name, logo and icons are
no longer under CC BY 4.0; the code stays MIT.

- Added: recording with speakers. Without headphones the microphone hears
  the meeting, so the other side was transcribed twice, once more as "me"
  a few milliseconds later. Diarist now notices when the microphone hears
  the system audio (the two compared on 2 s blocks) and removes the echoed
  words from "me", in the live view and in the final transcript; the other
  side's own channel is never touched. On a 111-minute meeting recorded
  with speakers it removed 99 percent of the duplicated words and kept
  99.4 percent of the owner's own. The live view says when it hears echo
  and offers "Turn off for this recording" (N). The meeting details say how
  many words were removed, with Restore to put them back as "me" (and
  "Remove echo words again"). Settings, Recording, "Recording with speakers
  (echo)": Detect and handle (default), Off or Always on, with the gate
  threshold under Advanced; Diagnostics says whether it is on and what it
  removed from the last recording. Words you say while the other side
  talks are still recognised from the mixture, so headphones remain the
  cleanest setup.
- Added: the processing after Stop cancels the echo before it transcribes
  "me". When the microphone hears the meeting, Diarist's own echo
  canceller takes the other side out of a temporary copy of the microphone
  audio (what is stored stays as recorded), that copy is transcribed, and
  the echo gate removes what is left. On the 111-minute meeting recorded
  with speakers it kept 98.3 percent of the owner's own words and left 0.6
  percent of "me" as copies of the other side; the owner's own words spoken
  over the meeting's real echo came out with 8.1 percent of words wrong
  instead of 17.1. It runs while the other side is transcribed, so the
  processing after Stop takes only seconds longer (about 45 s more for two
  hours when the live transcript has no "me" words and the microphone is
  transcribed once more). The live view still removes echoed
  words with the gate alone until the live canceller ships. New recordings
  keep the system audio's automatic gain beside the audio (`loopback.gain`,
  about 1.4 MB per hour, deleted with the audio) so the canceller can undo
  it.

- Changed: recording with speakers, and headphones that leak, leave far
  fewer copies of the other side in "me". Diarist now also checks "me"
  against what the other side said a moment before (three or more words
  repeated with one delay go; single words never), against the
  microphone's noise floor while the other side talks (a word that quiet
  cannot be you; never near your own level), and drops stray fragments
  inside a phrase that was echo; the sound comparison now looks at the
  speech band only. On the two 21 Oct test recordings the copies left in
  "me" went from 69 of 386 words to 2 of 292 (headphones) and from 22 of
  340 to none of 308 (speakers, with the change below), and in the live run
  from 158 of 476 to 1 of 310. A recording where the microphone does not hear the meeting comes
  out exactly as before. The live view can take a word back within 3 s
  of showing it, when the other side's words for the same moment arrive
  later. The meeting details' "Show them" list says why each word was
  removed.
- Changed: on speakers with a loud meeting, the processing after Stop
  left more copies of the other side in "me" than the live view (on one
  7.5-minute recording 21 of 293 words against 1 of 231): what the echo
  canceller leaves of a loud echo comes out as garbled fragments. It now
  also decides on the microphone as recorded, which the speech model hears
  cleanly (the live transcript's "me" words, or the microphone transcribed
  once more), and removes the cancelled words that sit where that decision
  found echo, when the canceller took sound out of them and they are well
  under your own level. On that recording none are left (the live view's
  result) and every word of yours the canceller left alone stays but one;
  on the 111-minute meeting the copies left went from 1.8 to 0.6 percent
  with exactly the same words of the owner kept. The meeting details list
  these as "Before echo cancellation, the microphone heard the meeting's
  words there."
- Fixed: the "live latency" log line counted the live view's echo
  corrections as latency (34 s on one recording).
- Changed: the copyright holder is named. `LICENSE` and the crate metadata
  say "Ebin Babu Thomas" instead of "Diarist contributors". The Diarist
  name, logo and icons (`branding/`, `apps/desktop/src-tauri/icons/`) are
  no longer offered under CC BY 4.0: they may be used unmodified to refer
  to Diarist or to package the official builds, not on forks or other
  products (`branding/README.md`). Copies taken under CC BY 4.0 keep that
  licence. The code stays MIT.

## [0.1.0-beta.3] - 2026-10-17

- Fixed: clicking a word after scrolling the transcript away from the
  cursor row did nothing. Pressing the mouse focused the row, and the
  transcript then scrolled back to the cursor before the button came up,
  so the click never reached the word. The transcript now scrolls to the
  cursor only when it gets focus from the keyboard. A click on a row whose
  words are still loading plays from the start of that row, and moving the
  pointer over the transcript keeps it from following the playback away
  from the row you are aiming at.
- Fixed: names given to speakers while recording are kept after Stop. The
  final transcript matched live and final speakers by the overlap of their
  word times, and the two passes place each word up to a few hundred
  milliseconds apart, so on real meetings nobody matched and a name given
  from the speaker legend fell back to "Speaker N". Speakers are now
  matched by their stretches of speech; a name that still finds no match
  goes to the final speaker who covers most of that speech, marked "new
  label" to check, and renaming from the legend no longer forgets where an
  earlier rename from a line placed the name.
- Changed: R in the live transcript with no line selected renames the
  newest finished line of a known speaker instead of the newest line,
  which was usually the provisional "Unknown speaker" whose name was lost
  at Stop. With no such line it says to select a line (j/k) or rename from
  the speaker legend.
- Changed: when a name given during a recording still reaches nobody in
  the final transcript, the live transcript is kept for that meeting
  instead of deleted (also after a crash during the processing), so the
  name is not gone for good; the app log records the matching counts
  after every Stop (no names or text).

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
  package (measured on an RTX 3080 and a Ryzen 9 5900X only: Vulkan 25 to 28
  times real time, CPU 5 times).
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
