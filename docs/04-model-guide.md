# Model and hardware guide

Status: v0.3, 16 Oct 2026, for 0.1.0-beta.5; punctuation (sections 1, 5 and 8) added 26 Oct 2026. Source for the README section "Choosing your package and models" (Phase 5). Numbers marked **measured** come from docs/research/03, 08 and 12 and docs/eval on an RTX 3080 10 GB; numbers marked *published* come from model cards and leaderboards cited in docs/research/01, 02 and 04; anything else is an estimate and says so. The Vulkan and CPU packages were measured on 16 Oct 2026 on the development PC's NVIDIA card and CPU only (section 4).

## 1. What runs inside Diarist

The speech runtime with its models, and since 0.1.0-beta.5 a small punctuation model with its own runtime. Nothing else. No Python, no PyTorch, no CUDA toolkit on the user's machine.

| Component | What it does | Size on disk | Licence |
|---|---|---|---|
| NeMo-Speech.cpp 0.2.0 (NVIDIA) | GGUF inference runtime, one zip per GPU backend | 98 MB zip, 134 MB extracted (CUDA build) | Apache-2.0 |
| Nemotron 3 Diarization, q8_0 | Who spoke when, up to 8 speakers, streaming and file | 102 MB | OpenMDW-1.1 |
| Nemotron Speech Streaming EN 0.6B, q8_0 | Speech to text for live and file mode, default for English | 668 MB | NVIDIA Open Model Licence |
| Nemotron 3.5 ASR Streaming 0.6B, q8_0 | Recommended for non-English or mixed meetings: 40+ locales with automatic detection, 30 percent less VRAM | 742 MB | OpenMDW-1.1 |
| Parakeet TDT 0.6B v3, q8_0 | Experimental only, off by default: better punctuation but drops speech (see section 3) | 681 MB | CC-BY-4.0 |
| Horizon-Labs punctuation-restoration-small, its int8-embedding ONNX file and tokenizer | Adds `.` `,` `?` and sentence capitals to the finished words after Stop and on imports, on the CPU (section 8) | 256 MB plus a 33 MB tokenizer | Apache-2.0 (declared in its model card); base model MIT |
| ONNX Runtime 1.31.0 (Microsoft), CPU build, installed beside the punctuation model as `diarist-onnxruntime.dll` | Runs the punctuation model inside Diarist's own process | 16 MB | MIT |

First-run download with the defaults: engine plus diarizer plus the EN model plus the punctuation model (306 MB with its tokenizer, ONNX Runtime and licence files), about 1.2 GB. With every optional model: 2.6 GB.

## 2. Speaker diarization: why Nemotron 3

This is the headline feature and the one place where model choice changes the product. On the only independent benchmark that scores every system the same way (VoiceArena Diarization-Bench v1, 139 English conversations, collar 0, overlap scored, *published*):

| System | DER, lower is better | Licence | Local, no gating |
|---|---|---|---|
| **Nemotron 3 Diarization** | **14.7** | OpenMDW-1.1 | yes |
| DiariZen s80-MD-v2 | 19.3 | weights CC BY-NC | yes, non-commercial |
| pyannoteAI Precision-3 | 20.6 | commercial API | no |
| pyannoteAI Precision-2 | 23.4 | commercial API | no |
| Sortformer 4spk v2 (NVIDIA, previous generation) | 24.6 | CC-BY-4.0 | yes |
| pyannote community-1 (what most open tools ship) | 30.6 | CC-BY-4.0, gated | token required |

Independent check on AMI meetings by a noScribe contributor (*published*): 11.6 percent of words to the wrong speaker for Nemotron 3 versus 15.7 for pyannote community-1. Our own spike (**measured**): 99.6 percent of words correctly tagged on a 21-minute AMI meeting through the streaming path, 4 of 4 speakers found.

Limits you should know (*published* and **measured**):
- At most 8 speakers per recording. A ninth person is merged into an existing label. Long multi-meeting recordings should be split.
- Counting accuracy drops at very low latency; live uses the 1.04 s setting, file jobs the same streaming algorithm with 21-second chunks (below).
- The stateless `diarize --offline` pass scored worse than streaming on our test clip (DER 43 versus 24) and is not used; it is not the `v3-offline` preset below.
- Hard cases listed on the model card: heavy overlap, far-field microphones, noisy rooms.

Imports and the post-pass after Stop run the diarizer with its offline geometry, `--asr.diar.preset v3-offline`: the same cache-based streaming algorithm, in 21-second chunks instead of 1.04-second ones; live recording and the optional standalone diarization pass keep the streaming preset. Measured on the full evaluation set of seven sessions with the file lookahead of section 3 (**measured**, 16 Oct 2026, docs/eval/2026-10-16-shipped_n35.md and -shipped_en.md against -n35_rc_13.md and -en_rc_13.md; docs/research/12 section 2): the words are identical and only who said them changes. Words within a second of a speaker change that land on the wrong speaker go, with Nemotron 3.5, from 8.2 to 5.5 percent on a dense Q&A meeting (Aug 2), 6.6 to 5.7 (Aug 9), 6.6 to 6.2 on AMI and 41.2 to 18.0 on the 77-minute in-room recording; Sep 7 stays at 5.7, and Sep 5 goes from 7.1 to 7.8 (one word). The EN model follows the same pattern (8.2 to 5.6, 7.6 to 6.8, 7.3 to 6.3, 42.2 to 19.1; Sep 5 7.3 both times; Sep 7 11.8 to 12.7, one word). The cost: on Sep 5 a participant with only 7 words is merged into another label with both models (5 speakers found as 4, within docs/00's bar of one); the single-speaker control stays at one. `engine.file_diar_preset = streaming` (section 5) goes back to the streaming geometry. The speech pass runs about 30 percent faster (Nemotron 3.5 over the whole set: 363 to 249 s for 324 minutes of audio) with the same or lower VRAM. The preset is set per diarizer in the manifest (`file_preset`).

## 3. Speech to text: why the Nemotron streaming models, which one, and why not Parakeet

Open ASR leaderboard average WER, lower is better (*published*, October 2026):

| Model | Avg WER | Word timestamps | Runs in our runtime | Streaming |
|---|---|---|---|---|
| Canary-Qwen 2.5B | 5.63 | no | no | no |
| Qwen3-ASR 1.7B | 5.76 | no | no, vLLM only | no |
| Parakeet TDT 0.6B v2 (English) | 6.05 | yes | yes | no |
| **Parakeet TDT 0.6B v3** (25 languages) | 6.34 | yes | yes | no |
| Kyutai STT 2.6B | 6.4 | yes, 2.5 s delay | no | yes |
| Nemotron Speech Streaming EN 0.6B | 6.93 at 1.1 s latency | yes | yes | yes |
| **Nemotron 3.5 ASR Streaming 0.6B** (40+ locales) | not on the leaderboard yet; see measured comparison below | yes | yes | yes |
| Whisper large-v3 (for reference) | about 7.4 | approximate | no | no |

Why not the top two: they give no word timestamps, and speaker assignment needs the start and end of every word. They also run only inside a Python stack.

Both Nemotron streaming models are safe on long files (**measured**, docs/research/08 section 7): on the 77-minute recording each found all 8 labels, dropped nothing, and tagged speakers at 99.9 percent agreement with the standalone diarizer; live, both keep two channels in real time and show a word's first text about 0.5 s after it is spoken at the live lookahead of section 3 (0.14 s at the runtime default; docs/eval/2026-10-27-live-lookahead.md). The choice between them comes from the eval harness with the shipped file setting: lookahead 13 (below) and the diarizer's offline geometry (section 2) (**measured**, 16 Oct 2026, docs/eval/2026-10-16-shipped_en.md and -shipped_n35.md: the app's own defaults with nothing passed, word for word and tag for tag the same as the configurations measured in docs/research/12). cpWER on the Zoom meetings is against Zoom's own machine transcript, so it mixes Zoom's errors in; AMI ES2002a has a word-level manual reference. WDER is the share of words given to the wrong speaker, near = within a second of a speaker change.

| Session (minutes, speakers) | EN cpWER | EN WDER (near) | 3.5 cpWER | 3.5 WDER (near) |
|---|---|---|---|---|
| Aug 2 (44, 6) | 9.5 | 0.3 (5.6) | 10.3 | 0.2 (5.5) |
| Aug 9 (44, 3) | 8.1 | 0.3 (6.8) | 8.4 | 0.3 (5.7) |
| Sep 5 (45, 5) | 4.7 | 0.2 (7.3) | 5.1 | 0.2 (7.8) |
| Sep 7 (48, 5) | 7.2 | 0.3 (12.7) | 7.5 | 0.2 (5.7) |
| Sep Logistics (46, 1; single-speaker control) | 7.0 | 0.0 | 6.6 | 0.0 |
| AMI ES2002a (21, 4; far-field, word-level reference) | 18.0 | 2.3 (6.3) | 20.7 | 2.2 (6.2) |

| | EN 0.6B | 3.5 0.6B |
|---|---|---|
| Speech pass, all seven sessions (324 min) | 255 s, 76 times real time | 249 s, 78 times real time |
| Whole import incl. an extra diarization pass the app skips | 30 times real time | 31 times real time |
| Peak VRAM, 77-minute file (docs/research/08; an upper bound at the shipped setting) | 2.6 GB | 1.8 GB |
| Languages | English | 40+ locales, automatic detection |
| Filler words ("um", "uh") | kept | about 40 percent dropped |
| Quiet speech | dropped (below) | kept |

Two soft spots of the shipped setting: on Sep 5 a participant with only 7 words is merged into another speaker by the diarizer's offline geometry, with both models (section 2); and with the EN model, Sep 7's words near a speaker change go from 8.2 percent at the old setting to 12.7 (11.8 from the lookahead, one word more from the geometry; about five of the session's 110 such words), within the noise of one session, while every other session improves.

The EN model drops quiet speech (**measured**, docs/eval/2026-10-16-fleurs.md): on about 10 minutes of FLEURS English, where 36 of the 55 recordings peak below -30 dBFS, it scores 21.5 percent WER at the file setting (23.3 at the live one), nearly all of it words left out, against 8.8 for Nemotron 3.5, and returns nothing or a fraction for eight quiet recordings that 3.5 transcribes in full. For quiet recordings (a soft-spoken participant, a far microphone, a file recorded at a low level) use Nemotron 3.5 or a louder source.

- **Default for English: the EN model.** It keeps more of what was said, and the difference is real after fillers are discounted on both sides.
- **Recommended for other or mixed languages: Nemotron 3.5.** It is also the better choice on a 4 GB card.
- Neither model punctuates much (0.2 and 2.5 percent of words). Since 0.1.0-beta.5 a punctuation model adds marks and sentence capitals after Stop and on imports, on the CPU, without touching the words (section 8).

Imports and the post-pass after Stop run both models at their full decoder lookahead, 13 frames (1.12 s, `--asr.streaming.rnnt_right_context 13`) (**measured**, 16 Oct 2026, full set; docs/research/12 section 1); live recording runs at 6 since 0.1.0-beta.5 (below), at the runtime's default of 1 before. Against the 0.1.0-beta.1 setting (lookahead 1, streaming diarizer; docs/eval/2026-10-16-nemotron_en.md and -nemotron35.md) cpWER on AMI goes from 19.8 to 18.0 with the EN model and from 23.0 to 20.7 with Nemotron 3.5, and every Zoom meeting improves: EN 10.3, 8.5, 5.1 and 7.8 to 9.5, 8.1, 4.7 and 7.2; 3.5 12.0, 9.2, 5.4 and 7.9 to 10.3, 8.4, 5.1 and 7.5. The speech pass runs 2.6 times faster (Nemotron 3.5 over the whole set: 663 to 249 s) with peak VRAM unchanged within the run-to-run noise. The value is set per model in the manifest (`file_right_context`), because the runtime's `-1` (the value in the model file) means 13 for the EN model but only 3 for the pinned Nemotron 3.5. With `post_pass_mode = on_issues` a clean recording keeps its live transcript and so the live setting.

Live recording runs both models at lookahead 6 (560 ms) since 0.1.0-beta.5 (**measured**, 27 Oct 2026, docs/eval/2026-10-27-live-lookahead.md): the first words of an utterance show 0.48 to 0.52 s after they end at the median (p90 0.56 to 0.58 s), against 0.14 s at the runtime's default of 1, with no measurable VRAM cost. The same decoder setting on the file path, with the live diarizer geometry, scores cpWER on AMI 18.0 instead of 19.8 with the EN model and 21.6 instead of 23.0 with Nemotron 3.5, and every one of the six Zoom meetings improves with both (mean 13.9 to 13.6 and 14.4 to 13.9). Live words also come about 80 ms closer to their real times. The value is set per model in the manifest (`live_right_context`); `engine.live_lookahead = low` (section 5) goes back to 1.

Why Parakeet TDT v3 is experimental only (**measured**, docs/research/08 section 8):
- It is about 0.6 WER points better than the EN model and writes real punctuation and casing (12 percent of words punctuated).
- But it silently dropped a 46-second span and two 10-second spans in the 77-minute file with no error. Cutting the file into 10-minute pieces recovered them, yet a 2-minute cut around the gap lost the same 46 seconds again. The drop depends on the audio content, not the file length, so no chunk size is safe. The same behaviour is reported for this model in another runtime (transcribe.cpp issue #71).
- A transcript tool cannot ship a model that may skip a minute of a meeting without telling you. It stays reachable behind an experimental setting for short clips where punctuation matters, and a dual-pass mode that fills its gaps from Nemotron is possible later.

Realistic expectation: on clean meeting audio about 1 word in 15 to 20 is wrong or missing with any of these models; on AMI-style far-field recordings about 1 in 9 (*published* AMI WER 11.3 for Parakeet v3). The product's value is in who said what, stored and searchable, not in a perfect verbatim.

## 4. Which package for which PC

The runtime ships three Windows zips. The setup screen checks the GPU (NVML for NVIDIA, DXGI for any vendor), preselects the package and lets you override it: CUDA 98 MB, Vulkan 19 MB, CPU 6 MB.

| Your GPU | Package | Status |
|---|---|---|
| NVIDIA, driver new enough for CUDA 13 (R580 or newer is the estimate; 610.88 **measured** working) | `windows-x86_64-cuda` | **Measured** on RTX 3080 10 GB; kernels built for Turing (sm_75), Ampere (sm_86), Ada (sm_89) and Blackwell (sm_120) |
| AMD or Intel GPU, or older NVIDIA driver | `windows-x86_64-vulkan` | **Measured** on the RTX 3080 only, nothing about AMD or Intel (16 Oct 2026, through the app's own path, file setting): the 5-minute AMI clip in 10.7 to 12.0 s, 25 to 28 times real time (CUDA 67 to 69 times), the same word count as CUDA with 3.2 percent of words different and the same speakers; live through the app's adapter ready in 3.1 s with finals and speakers, first word 0.14 s after it was spoken, no abort in 60 s |
| No usable GPU | `windows-x86_64-cpu` | **Measured** on a Ryzen 9 5900X, 12 cores (16 Oct 2026, through the app's own path, file setting): the 5-minute AMI clip in 58 s, 5.2 times real time (an hour in about 12 minutes), 0.6 percent of words different from CUDA. The app says once that it is slow. Live not tested and not recommended |

VRAM needed (**measured** on the 3080, excluding the desktop's own 1.5 GB):

| Scenario | VRAM | Notes |
|---|---|---|
| Live recording, two channels (mic plus system audio), Nemotron 3.5 and diarizer resident | 1.05 GB idle, 1.26 GB with two sessions, transient spikes of 0.5 to 0.6 GB per session; budget 2 GB | The EN model sits at 1.18 GB with smaller spikes. At the live lookahead 6 (0.1.0-beta.5) the server with both sessions peaked at 1.16 GB with the EN model and 1.2 GB with Nemotron 3.5 over three 60-second runs each, within 3 MiB per session of the runtime default (docs/eval/2026-10-27-live-lookahead.md) |
| File transcription, default Nemotron 3.5 path | 1.82 GB peak | 77 min file; the EN model peaks at 2.6 GB |
| Standalone diarization pass only | 0.7 to 0.85 GB | |
| File transcription, experimental Parakeet | 4.9 GB peak | |
| Live recording plus a queued file job | not concurrent | The app runs file jobs after recording stops |

Estimated tiers (not measured; the app enforces them by reading free VRAM before starting):

| Card VRAM | What works |
|---|---|
| 4 GB | Default file mode (1.8 GB) should fit; live (2 GB budget with spikes) is borderline next to the desktop's own use; Parakeet will not fit. A user with a 4 GB card reported out-of-memory with the runtime's default 16 slots, which we do not use |
| 6 GB | Live and default file mode comfortably; Parakeet only with nothing else on the GPU |
| 8 GB and up | Everything, including Parakeet |
| 10 GB (RTX 3080, the development machine) | Everything, with room for a browser and the meeting app on the same GPU |

Other requirements:
- Windows 10 (1803 or newer) or Windows 11, 64-bit. WebView2 is already present on these; the installer fetches it if not.
- Windows 11 22H2 or newer for operating-system echo cancellation when recording without headphones; older builds fall back to our own echo gate (Phase 3).
- RAM: the engine process uses about 300 MB (**measured**); 8 GB total is an estimate for a comfortable machine; the CPU package needs RAM for the models themselves (about 1.5 GB) plus working memory.
- Disk: 1.6 GB for engine and models; recordings about 90 MB per hour as stereo FLAC with 30-day retention by default; the transcript database is small (a year of daily meetings is tens of MB).
- The engine binaries are unsigned by NVIDIA; Windows Smart App Control, when enabled, may block them until the app is signed (planned).

## 5. Adding or updating models and the engine yourself (ComfyUI-style)

- Open Settings, Models, "Open models folder". Drop any GGUF the runtime supports anywhere in that folder (any subfolder is scanned; the app's own downloads land in `models\<publisher>\<model>\<revision>\`). Press Refresh. The app reads the file's header, lists it under the right role with its size and a VRAM estimate, and you can select it. Nothing else to configure.
- If a file is listed as "unrecognised", the runtime does not know its architecture, or it is not a speech model. You can still force it with a `<file>.meet.json` sidecar (`{"role":"asr","streaming":true}`), and the runtime's own error is shown if it cannot load it.
- If a file is unrecognised but you know what it is, Settings offers "Use for speech to text" and "Use for speakers" on it, with a warning.
- New runtime versions: Settings, Models, "Check for newer runtime" lists the NeMo-Speech.cpp releases on GitHub and installs a package next to the others. Or put the zip (checked against the manifest and extracted on Refresh) or the extracted folder under `engine\`, press Refresh, select it. The app warns when a version is newer than the ones it has been tested against.
- "Download recommended" fetches the models in this guide into the same folders with hash verification. Every download shows its URL, SHA-256, size and folder with Retry and Copy link, so a browser and a file copy always work; a file placed by hand is hash-checked on Refresh. Offline machines copy the files in the same way.
- Models no list knows: "Add from URL" (optional SHA-256) and "Add from file" put them into `models\user\`; `models.user.json` in the data folder adds entries to the recommended list. Each entry needs `name`, `role`, `file`, an `http(s)` `url` (Diarist downloads every entry itself), `size_bytes` and `sha256`; it lands in `models\user\` unless it says `"direct": false` (then `models\<repo>\<revision>\`, with `repo` as `owner/name`) or names a `dir`. `file`, `repo`, `revision` and `dir` must stay inside the models folder (no `..`, no drive, no Windows device name such as `NUL`); an entry that breaks a rule stops the list from loading, and the message names the entry and the field.
- Selections are per role; the app remembers them and falls back to the recommended model with a notice if a selected file disappears.
- Decoder lookahead for imports and the post-pass (section 3): the recommended Nemotron models carry theirs in the manifest (`"file_right_context": 13`). A model you add gets none, so it runs at the runtime's default (1, the live setting), unless its `<file>.meet.json` gives one, e.g. `{"file_right_context": 13}`; use the largest value its model card lists, since a larger one may not be supported.
- Diarizer preset for imports and the post-pass (section 2): Nemotron 3 Diarization carries `"file_preset": "v3-offline"` in the manifest. A diarizer you add gets none, so it runs the runtime's streaming default, unless its `<file>.meet.json` names one, e.g. `{"file_preset": "v3-offline"}` (the runtime's names are `v3-streaming` and `v3-offline` for V3 models, `streaming` and `offline` for V2).
- `engine.file_diar_preset`, the same kind of setting for the diarizer: empty or `model` (the default) uses each diarizer's value; `streaming` passes no preset flag, so file jobs tag speakers like live; any other text is passed as the preset name for every diarizer, e.g. `v3-streaming`. It is set like `engine.file_lookahead` below.
- The escape hatch, a setting with no switch in the app, `engine.file_lookahead`: empty or `model` (the default) uses each model's value; `low` passes no lookahead flag, so file jobs run like live (for a 4 GB card that runs out of memory on imports, or to compare in a bug report); a number, e.g. `6`, uses that value for every model. Settings are stored in `diarist.db` in the data folder; with Diarist closed, `sqlite3 "<data folder>\diarist.db" "INSERT OR REPLACE INTO settings (key, value) VALUES ('engine.file_lookahead', 'low')"` sets it, and deleting the row restores the default.
- The live server's lookahead, `engine.live_lookahead`, set the same way: empty or `model` (the default) uses the ASR model's own `live_right_context` (6, 560 ms, for both recommended Nemotron models since 0.1.0-beta.5, section 3); `low` passes no flag, so live runs at the runtime default, 1 frame, 160 ms (the setting before 0.1.0-beta.5: words appear about 0.35 s sooner and are recognised less well); a number, e.g. `6`, passes `--asr.streaming.rnnt_right_context 6` to `nemo-speech serve` for a model trained for that value (the EN model: 0, 1, 6, 13; Nemotron 3.5: 0, 1, 3, 6, 13, from their model cards). A value the model was not trained for is not used: the log says why and the model's own value applies. A model you add is not checked; its `<file>.meet.json` can carry one, e.g. `{"live_right_context": 6}`. The live server restarts with the new value the next time Record or the start sheet needs it (never under a running recording), and every live run records the value it ran at (`engine_info.live_right_context`).

### Punctuation models (`.onnx`)

A punctuation model is a token classifier in ONNX format that Diarist runs itself on the CPU, through ONNX Runtime, after Stop and on imports; it only labels the words (a mark after a word, `¿` `¡` before one, capitals), and Diarist applies the labels with checks, so it can never change a word, a time or a speaker. The runner is generic: a description block tells it what the model's inputs, outputs and labels are. The recommended model carries its block in the manifest; a model you add needs a `<file>.meet.json` sidecar next to its `.onnx` file with a `punctuation` key. An `.onnx` file without a valid block is listed as unrecognised with the note "needs a `.meet.json` that describes its labels", and an unrecognised file can never be selected for punctuation.

To add one, for example a Hugging Face folder:

1. Copy the folder anywhere under `models\` (the Hugging Face layout works as it is: `onnx\model_quantized.onnx` with `tokenizer.json` at the top).
2. Put ONNX Runtime into the same folder as `diarist-onnxruntime.dll`: take `lib\onnxruntime.dll` from Microsoft's `onnxruntime-win-x64-<version>.zip` (the GitHub releases of `microsoft/onnxruntime`; a 1.31.x release, or the one the block's `runtime_version` names) and rename it. The new name keeps Windows from confusing it with another copy, such as the old `onnxruntime.dll` in `C:\Windows\System32`; a file named `onnxruntime.dll` is never loaded, and a block whose `runtime` names one is refused. One copy for several models can go under `engine\onnxruntime-<version>\` instead, also as `diarist-onnxruntime.dll` (the model's own folder is searched first; among several such folders, one whose version has the block's major and minor version, newest first). ONNX Runtime needs the Microsoft Visual C++ Redistributable (x64): when `MSVCP140.dll`, `MSVCP140_1.dll`, `VCRUNTIME140.dll` or `VCRUNTIME140_1.dll` is in neither `C:\Windows\System32` nor the library's folder, punctuation does not load and Diagnostics, "Punctuation runtime", names the missing file and links Microsoft's page. The library's own dependencies are looked for only in its folder and in System32, never in the current folder or `PATH`.
3. Write `<file>.meet.json` next to the `.onnx` file (`manifests/punctuation-sidecar.example.json` in the repository is a complete one), press Refresh, and select the file under Punctuation.

A dropped-in punctuation folder is picked up at the next scan (Refresh, or the next start) and, when no other punctuation model is selected, selected by itself; its `diarist-onnxruntime.dll` is then loaded into Diarist's own process after the next Stop or import. Its hash is not checked: only the recommended model's files are checked against the manifest (a library whose SHA-256 differs from its manifest entry is refused, `reason: "runtime"`), because a folder you drop in is yours. So drop in models, and above all their ONNX Runtime library, only from sources you trust.

The sidecar's keys: `role` (`"punctuation"`, optional), `display_name`, `languages` (primary subtags such as `["en", "de"]`, or `["*"]`; a model that lists none is not checked), and `punctuation`, the block:

| Key | Meaning |
|---|---|
| `tokenizer` | the tokenizer file, relative to the model's folder (required). The model's folder is the nearest one, from the `.onnx` file's own folder up to three folders above it (never above `models\`), that holds this file. No `..`, no drive |
| `tokenizer_kind` | `hf_json` (a Hugging Face `tokenizer.json`; the default and the only kind). Convert a SentencePiece `.model` into one first |
| `input_casing` | `keep` (default) or `lower` for models trained on lower-case text |
| `max_tokens` | tokens per window, the special tokens included (required; 16 to 4096) |
| `context_tokens` | tokens of context on each side of a window, whose labels are discarded (default an eighth of `max_tokens`, at most 64; at most half of `max_tokens`, and at least 8 tokens of a window must be left for labels) |
| `batch` | windows per inference call (default 8; 1 to 64). Windows of one stream are batched; the mic and loopback streams of a recording are punctuated one after the other |
| `bos_id`, `eos_id`, `pad_id` | the token put before and after every window (leave out when none) and the padding token (default 0) |
| `inputs` | `{"ids": "input_ids", "mask": "attention_mask"}`: the input tensors' names; leave `mask` out for a model without one |
| `heads` | the outputs to read, at least one `post` or `case` head: `output` (tensor name), `kind` (`post`: a mark after the word; `pre`: before it; `case`: capitals; `boundary`: read by nothing), `labels` (each class in the output's order: a mark such as `.` `,` `?` `!` `:` `-` `¿` `¡` or its script forms `。` `，` `、` `।` `،` `؟` `？`, a name such as `period`, `""`, `0`, `O` or any `<...>` token for nothing, `<ACRONYM>` (never applied); for a `case` head `lower`, `cap` or `upper`, none for per-character casing), `position` (`last_token`: the word's last subword carries its label; `first_token`; `per_subword`: every subword is labelled, a mark inside a word is dropped and counted) and `values` (`logits`: float scores per class; `decisions`: class ids, or `[batch, tokens, characters]` flags for per-character casing). A `;` label is refused as ambiguous: write `question` for the Greek question mark or `""` |
| `casing` | `none` (default: only the sentence-start rule), `first_letter` or `per_char` |
| `marks_by_language` | marks allowed per language, e.g. `{"*": ["period", "comma", "question"], "es": ["period", "comma", "question", "inverted_question"]}`; a `?` or `!` the list leaves out becomes the language's period, any other mark it leaves out is dropped and counted (`marks_excluded` in the run's `engine_info.punctuation`) |
| `unit`, `policy` | `stream` (default) or `turn` windows; `keep_asr` (default: a mark the speech model wrote stays) or `replace` |
| `runtime`, `runtime_version` | the ONNX Runtime file in the model's folder (default `diarist-onnxruntime.dll`; never `onnxruntime.dll`) and the ONNX Runtime release the model needs, by major and minor version: `1.31.0` and `1.31` both accept any 1.31.x (1.31.2, not 1.30 or 1.32), because the runtime's C API and ABI change with the minor version. Leave it out to accept any release Diarist can load (1.31 or newer) |

Every error in a block names its field (`heads[1].labels[3]: ...`); a sidecar that is not valid JSON, or misses a required key, shows its parse error first and then the note above.

ONNX Runtime is loaded once per run of Diarist. Another punctuation model whose folder has its own copy of the same minor release uses the copy already loaded; one whose `runtime_version` names another release takes effect after a restart, and Diagnostics says so. The punctuation model runs on the CPU only (4 threads; `transcript.punctuation_threads` changes it) and is unloaded after each job. `transcript.punctuation` set to `off` turns punctuation off.

A punctuation entry in `models.user.json` (or the recommended one) lists its other files under `files`, each with `path` (relative to the entry's folder, `dir`), `url`, `size_bytes` and `sha256`; every file needs its SHA-256, the primary one included, each path appears once, and paths may not leave the folder or use a Windows device name (`CON`, `NUL`, `COM1`, ...). An entry that breaks these rules is refused when the list is read, with the field named. The meeting's details record where the library was loaded from relative to the data folder (`models/...`, or `<models>/...` when the models folder is elsewhere), never as a full path.

## 6. Languages

- Diarization is language-independent (Nemotron 3 Diarization).
- The default EN model transcribes English only. Nemotron 3.5 covers the other languages. You do not need to select it: a recording or import in a language the selected model does not cover runs on an installed model that lists that language (by the language list in its GGUF header, so a model you added yourself counts too), the stored selection stays, and the app says so ("Transcribing with Nemotron 3.5: the selected model is English-only"). When no installed model covers the language, the start sheet and the import sheet offer the Nemotron 3.5 download and wait for it; the job never starts on the wrong model. `diarist-cli transcribe --language de` switches the same way, or stops naming the model to install; `--asr-model` turns the switch off.
- The lists offer the transcription-ready and broad-coverage languages of this table (Nemotron 3.5 model card, FLEURS word error rate at the 1.12 s setting with the language given; character error rate for Japanese, Korean and Chinese; *published*) that an installed model declares in its GGUF header (the selected model, and the models a switch can use): 30 entries with Nemotron 3.5. The declaration is the header's prompt dictionary, which has every tag the tokenizer knows (121 for Nemotron 3.5, Malayalam, Tamil and Swahili among them), not the languages the model was trained on; given Malayalam, Nemotron 3.5 returns nothing but unknown-token markers. So a declared language the table does not know, and the adaptation tier, are not listed; "Other tag…" still takes the tag, with the note "Not in this model's supported list; the output may be unusable". To offer one anyway (a fine-tuned model's language, or Greek for a model that handles it), add it to `languages.user.json` in the data folder: an entry without a tier, or with `ready` or `broad` (`{"code": "el", "name": "Greek", "tier": "broad"}`), is listed for any model that declares it. A file import or post-pass whose words come back more than half unknown-token markers (`<unk>`, which the runtime glues into runs, or `⁇`) stores nothing and ends with "The model could not transcribe this language (unknown tokens). Choose a supported language or another model." in the toast and on the meeting's page; the meeting stays, an import with its audio (import the file again in another language), a recording with its WAVs and live transcript (Retry runs the post-pass again, for example after selecting another model).

| Tier | Languages | FLEURS WER | In the app |
|---|---|---|---|
| Transcription-ready | Spanish (es-ES, es-US), Italian, Portuguese (pt-PT, pt-BR), Hindi, Korean, English, German, French, Russian, Turkish, Vietnamese (vi-VN), Dutch, Japanese (ja-JP), Arabic, Ukrainian | 4 to 13 | Listed |
| Broad coverage | Polish, Norwegian Bokmål, Finnish, Chinese (zh-CN), Czech, Bulgarian, Slovak, Swedish, Croatian, Romanian, Estonian, Danish, Hungarian | 15 to 29 | Listed as "reduced accuracy" |
| Adaptation-ready | Greek, Lithuanian, Latvian, Maltese, Slovenian, Hebrew, Thai, Norwegian Nynorsk | needs fine-tuning | Not listed ("Other tag…", or a `languages.user.json` entry) |

Measured in Diarist's own runtime (**measured**, 16 Oct 2026, docs/eval/2026-10-16-fleurs.md): about 10 minutes of the FLEURS test split per language (37 to 55 sentences joined with 1.5 s of silence), transcribed by Nemotron 3.5 at the file setting (lookahead 13, 1.12 s) without diarization. WER in percent, CER for Japanese. These are smoke tests on small slices, one or two points either way is noise, and they sit 1 to 9.5 points above the model card's numbers, least for English and most for Hindi and Japanese (full test split, NVIDIA's normaliser; FLEURS writes numbers as digits and the runtime writes them as words), so read them for direction, not as Diarist's published accuracy:

| Language (tag passed) | Words | Language given | Automatic detection | Model card, given / automatic |
|---|---|---|---|---|
| English (`en`) | 1,186 | 8.8 | 8.8 | 7.91 / 8.84 |
| German (`de`) | 878 | 10.6 | 10.3 | 8.31 / 8.22 |
| Spanish (`es-US`) | 1,178 | 6.8 | 9.5 (detects es-ES) | 4.11 / 4.13 |
| French (`fr`) | 1,360 | 11.8 | 11.8 | 9.03 / 9.02 |
| Italian (`it`) | 989 | 6.4 | 6.1 | 4.25 / 4.32 |
| Portuguese (`pt-BR`) | 1,046 | 9.1 | 8.7 (detects pt-PT) | 5.48 / 5.47 |
| Hindi (`hi`) | 1,168 | 14.2 | 14.9 | 6.81 / 8.23 |
| Japanese (`ja-JP`, CER) | 2,072 characters | 20.9 | 21.1 | 11.48 / 11.66 |

At the runtime's default lookahead (160 ms, the live setting before 0.1.0-beta.5) the same slices score 0.1 to 9.7 points worse with the language given, Spanish most (16.5). The EN model on the English slice: 21.5 at the file setting, nearly all of it dropped quiet speech (section 3).

- Language tags: the runtime's prompt dictionary has bare `de`, `fr`, `ko` and so on, but no bare `ja`, `zh`, `vi`, `th` or `he`, which it silently runs as automatic detection; bare `es` is `es-US` and bare `pt` is `pt-PT`. The app therefore offers `ja-JP`, `zh-CN` and `vi-VN`, lists Spanish and Portuguese per locale, and gives every job a tag the model's dictionary has, also for meetings stored with a bare tag by 0.1.0-beta.1 (whose Japanese and Chinese ran as automatic detection).
- A model that lists no languages (Parakeet, most GGUFs you add) is not checked: the table's ready and broad-coverage languages are offered for it with a "not checked for this model" note, and nothing switches.
- The table is data, like the model list: `manifests/languages.json` (code, name, tier, bare; compiled in), replaced by a copy in the data folder's `manifests\languages.json` and extended by `languages.user.json` in the data folder, whose entries are matched on `code` (a known code replaces the shipped entry, a new one is added; `tier` is optional, `bare` defaults to true for a code without a region). A file that cannot be read is skipped with a warning in the log, never an error. Every language list also has "Other tag…", where any BCP-47 tag can be typed (`sw-KE`, `zh-TW`): it is stored as typed (or as the table's spelling), the runtime gets the model's own locale for it, an installed model that lists it is switched to as for a chosen language (with the "not in this model's supported list" note when the table does not offer it), and a tag nothing installed lists is passed to the selected model unchecked, with a note.
- Set the meeting's language when you know it: automatic detection loses about a second of the first utterance and costs 0.1 to 3 WER points against a given language, most for Hindi, Russian, Ukrainian, Polish, Bulgarian, Croatian and Romanian. It is never the default. The first-run screen sets the default language once from the Windows display language.
- Live: the start sheet's language decides which model the live server runs; changing it while the sheet is open restarts an idle server with the right model (3 to 6 s), and a recording in progress keeps its language.
- Search ignores accents (Unicode tokenizer with diacritics removed) and does not stem, so it behaves the same in every language that separates words with spaces. Chinese and Japanese are not segmented yet (docs/KNOWN-ISSUES.md).
- Punctuation covers every language in these lists except Thai, with fewer marks for Hindi, Japanese and Chinese (section 8).

## 7. Honest caveats

- Speaker labels in the live view are provisional; the runtime never revises them. The post-pass after Stop produces the stored transcript.
- Word confidence is not available from the runtime (always 1.0), so the UI cannot flag uncertain words yet.
- Meetings with more than 8 speakers, strong overlap or far-field audio will degrade; the eval harness reports how much on real recordings.
- Punctuation is a second model's guess over the finished words: about a third of its question marks and periods are not where a person would put them (section 8). It never changes a word.
- Everything here is a snapshot of October 2026. The runtime is three months old and improving quickly; the manifests pin exact versions and hashes so an update is a deliberate step.

## 8. Punctuation

Since 0.1.0-beta.5 (**measured** in docs/eval/2026-10-26-punctuation.md, decided in docs/plans/16 section 3.13).

**What it does.** After Stop, on both channels, and on every import, a small token classifier reads the finished words and labels each one: a mark after it (`.` `,` `?`, written in the script's own forms: `।` for Hindi, `。` `？` for Japanese and Chinese, `،` `؟` for Arabic, `;` for the Greek question mark) or nothing. Diarist then raises the first letter of each turn and of every word after a sentence end, in scripts with capitals. Lines of the transcript end at a sentence end where the pause allows, so a meeting reads as sentences. A turn is a speaker's run of words up to a speaker change or a pause of more than 1.5 s; each is lowercased and labelled in windows of up to 512 tokens. A mark the speech model wrote itself stays, because the speech model heard the voice. A channel whose speech model already punctuates 12 or more words in 100 gets only the capitals.

**What it never does.** It never changes, adds, drops or moves a word, never changes a word's times or speaker, never lowers a capital the speech model wrote, and never touches the live view (no live punctuation in this release). Diarist applies the labels itself and checks every word; a word that would break any of this keeps the speech model's text. The speech model's text stays stored with each changed word, so "Remove punctuation" goes back to it exactly. Search, which ignores punctuation and case, finds the same lines either way. Word accuracy and speaker accuracy are therefore unchanged: on the evaluation set and the seven talks, through the app's own import and export, cpWER, tcpWER and WDER are identical with and without punctuation (**measured**, 14 sessions). What does change is the line breaks: transcript lines end at sentence ends, which is why exported RTTM spans are shorter (DER and JER against continuous references rise by 2 to 6 points of "missed" pauses; the eval harness runs with punctuation off for that reason).

**The model.** `Horizon-Labs/punctuation-restoration-small` at revision `ee31fdf`, its own int8-embedding file `onnx/model_quantized.onnx` (256 MB, used as published) with `tokenizer.json` (33 MB), from Hugging Face. It runs on ONNX Runtime 1.31.0, Microsoft's official CPU build, installed in the model's folder as `diarist-onnxruntime.dll` (16 MB) and loaded by its full path. The runtime, its licence and notices, and the model's licence and notice files come from the project's own models release on GitHub. The model is a required download (306 MB in all, `models\horizon-labs\punctuation-restoration-small\`); Import and Record never wait for it, and without it transcripts are simply unpunctuated. Of the three candidates measured, it finds sentence ends best on every one of 14 sessions (sentence-boundary F1 0.725 against 0.671 and 0.655 on the six Zoom meetings plus AMI), ties the others on punctuation F1 over `.` `,` `?` there (0.597, 0.600 and 0.598), leads on the hand-annotated AMI meeting (0.55 against 0.52), gives the best capitals with its sentence-start rule (upper-case F1 0.75 on AMI against 0.70 to 0.71 for the true-casing models), and covers 35 of Diarist's 36 languages; the others were a 47-language model, which Diarist would have had to quantise itself, and an English-only one.

**Licence and provenance.** The model card declares Apache-2.0; the repository has no licence file at this revision, so Diarist installs the Apache License 2.0 text beside it (`LICENSE-Apache-2.0.txt`), with `NOTICE-punctuation.txt` recording the revision, the files' SHA-256 and where the model came from. The base model is jhu-clsp/mmBERT-small (MIT). Per its card, the model was trained on text windows in 88 languages from FineWeb-2 and FineWeb (ODC-BY), plus 49,144 windows of synthetic transcripts of everyday speech (about 4 percent of the training text), written by `Qwen/Qwen3.8-27B` (Apache-2.0), which the publisher ran locally (`code/punct/gen_spoken.py` in the repository), so no API provider's terms are involved. The publisher's organisation was a few weeks old when the model was chosen (October 2026). The two alternatives measured and the base model declare their licences the same way, in the YAML header of their model cards; that is normal on Hugging Face. A dated copy of the card and of the repository's file list at this revision is kept with the project's models release (docs/plans/16 section 12.9). ONNX Runtime is MIT; its licence and third-party notices travel beside it. To switch the model off for every new install, set `required` to `false` in `manifests/models.json`; to switch punctuation off, set `transcript.punctuation` to `off`.

**Languages.** Every language Diarist offers except Thai, which writes no sentence marks and which the model does not cover (the meeting's details say punctuation is not available). Commas are left out for Hindi (precision 0.53 on a FLEURS slice), Japanese and Chinese (the speech model returns each utterance as one or a few word units, so a comma has nowhere to go; marks go only at the ends of those units). French and Italian keep commas although their 10-minute FLEURS slices measured just under Diarist's 0.75 precision bar (0.74 and 0.69), pending a larger sample. Colons and dashes are never written. Spanish gets `?` without `¿`.

**Cost.** About 5 to 6 seconds of CPU per hour of audio at 4 threads, 0.65 GB of RAM while it runs (the model is unloaded after each job), no VRAM (**measured**: 5.3 to 5.6 s per hour over the 28 hours of the evaluation set with Diarist's own build, 3.2 to 8.1 per recording, the short ones highest, docs/eval/2026-10-27-beta5-release.md; 0.5 GB for a 21-minute meeting). Each job also loads the model and its tokenizer first, about 2 s (1.9 s measured in the spike).

**How good it is.** Against Zoom's own transcripts of the author's meetings, about two in three of its periods and question marks are where Zoom put one (precision 0.65 and 0.70; on the far-field four-person AMI meeting its question marks are right 0.39 of the time), and about four in five of its commas (0.8), but it writes about one comma per sentence where Zoom writes about two, so it misses more than half of Zoom's commas. On read-speech imports (an audiobook, a talk read from a script) the speech model punctuates by itself (5 to 10 marks per 100 words) and its marks are kept; comma precision there is lower (en 0.70, es 0.75). docs/KNOWN-ISSUES.md lists what this means in use.

**Older meetings** are not changed automatically: "Add punctuation" on a meeting, Settings' "Add punctuation to N older meetings", or `diarist-cli punctuate <meeting>` / `--all-missing` builds a punctuated copy and keeps the earlier run. Adding your own punctuation model is in section 5.
