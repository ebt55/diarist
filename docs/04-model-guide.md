# Model and hardware guide

Status: v0.2, 15 Oct 2026. Source for the README section "Choosing your package and models" (Phase 5). Numbers marked **measured** come from docs/research/03 and 08 on an RTX 3080 10 GB; numbers marked *published* come from model cards and leaderboards cited in docs/research/01, 02 and 04; anything else is an estimate and says so. Remaining PENDING items are the Vulkan and CPU packages, untested by us.

## 1. What runs inside Diarist

Three neural models and one runtime. Nothing else. No Python, no PyTorch, no CUDA toolkit on the user's machine.

| Component | What it does | Size on disk | Licence |
|---|---|---|---|
| NeMo-Speech.cpp 0.2.0 (NVIDIA) | GGUF inference runtime, one zip per GPU backend | 98 MB zip, 134 MB extracted (CUDA build) | Apache-2.0 |
| Nemotron 3 Diarization, q8_0 | Who spoke when, up to 8 speakers, streaming and file | 102 MB | OpenMDW-1.1 |
| Nemotron Speech Streaming EN 0.6B, q8_0 | Speech to text for live and file mode, default for English | 668 MB | NVIDIA Open Model Licence |
| Nemotron 3.5 ASR Streaming 0.6B, q8_0 | Recommended for non-English or mixed meetings: 40+ locales with automatic detection, 30 percent less VRAM | 742 MB | OpenMDW-1.1 |
| Parakeet TDT 0.6B v3, q8_0 | Experimental only, off by default: better punctuation but drops speech (see section 3) | 681 MB | CC-BY-4.0 |

First-run download with the defaults: engine plus diarizer plus the EN model, about 870 MB. With every optional model: 2.3 GB.

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
- Counting accuracy drops at very low latency; we use the 1.04 s setting for live and the same streaming algorithm for files.
- The "offline" full-attention preset scored worse than streaming on our test clip (DER 43 versus 24) and is not used.
- Hard cases listed on the model card: heavy overlap, far-field microphones, noisy rooms.

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

Both Nemotron streaming models are safe on long files (**measured**, docs/research/08 section 7): on the 77-minute recording each found all 8 labels, dropped nothing, and tagged speakers at 99.9 percent agreement with the standalone diarizer; live, both reach first text in about 0.2 s and keep two channels in real time. The choice between them comes from the eval harness (docs/eval, 15 Oct 2026, **measured**):

| | EN 0.6B | 3.5 0.6B |
|---|---|---|
| cpWER on the four English Zoom meetings | 5.1, 7.8, 8.5, 10.3 | 5.4, 7.9, 9.2, 12.0 |
| cpWER on AMI ES2002a (word-level reference) | 19.8 | 23.0 |
| Words to wrong speaker | identical | identical |
| Peak VRAM, 77-minute file | 2.6 GB | 1.8 GB |
| Languages | English | 40+ locales, automatic detection |
| Filler words ("um", "uh") | kept | about 40 percent dropped |

- **Default for English: the EN model.** It keeps more of what was said, and the difference is real after fillers are discounted on both sides.
- **Recommended for other or mixed languages: Nemotron 3.5.** It is also the better choice on a 4 GB card.
- Neither model punctuates much (0.2 and 2.5 percent of words); the app's segmenting at pauses supplies the structure, and punctuation restoration is planned as a CPU post-step.

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
| AMD or Intel GPU, or older NVIDIA driver | `windows-x86_64-vulkan` | Shipped by the runtime; untested by us. Expect it to work for file mode; live is PENDING a test |
| No usable GPU | `windows-x86_64-cpu` | Shipped by the runtime; untested by us. Speed unknown; file mode will be much slower than real time |

VRAM needed (**measured** on the 3080, excluding the desktop's own 1.5 GB):

| Scenario | VRAM | Notes |
|---|---|---|
| Live recording, two channels (mic plus system audio), Nemotron 3.5 and diarizer resident | 1.05 GB idle, 1.26 GB with two sessions, transient spikes of 0.5 to 0.6 GB per session; budget 2 GB | The EN model sits at 1.18 GB with smaller spikes |
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

- Open Settings, Models, "Open models folder". Drop any GGUF the runtime supports anywhere in that folder (subfolders are fine; `asr\` and `diarization\` exist for tidiness). Press Refresh. The app reads the file's header, lists it under the right role with its size and a VRAM estimate, and you can select it. Nothing else to configure.
- If a file is listed as "unrecognised", the runtime does not know its architecture, or it is not a speech model. You can still force it with a `<file>.meet.json` sidecar (`{"role":"asr","streaming":true}`), and the runtime's own error is shown if it cannot load it.
- If a file is unrecognised but you know what it is, Settings offers "Use for speech to text" and "Use for speakers" on it, with a warning.
- New runtime versions: Settings, Models, "Check for newer runtime" lists the NeMo-Speech.cpp releases on GitHub and installs a package next to the others. Or put the zip (checked against the manifest and extracted on Refresh) or the extracted folder under `engine\`, press Refresh, select it. The app warns when a version is newer than the ones it has been tested against.
- "Download recommended" fetches the models in this guide into the same folders with hash verification. Every download shows its URL, SHA-256, size and folder with Retry and Copy link, so a browser and a file copy always work; a file placed by hand is hash-checked on Refresh. Offline machines copy the files in the same way.
- Models no list knows: "Add from URL" (optional SHA-256) and "Add from file" put them into `models\user\`; `models.user.json` in the data folder adds entries to the recommended list.
- Selections are per role; the app remembers them and falls back to the recommended model with a notice if a selected file disappears.

## 6. Languages

- Diarization is language-independent.
- Live and file mode: the default EN model is English only. Select Nemotron 3.5 in Settings for 40+ locales; set the meeting's language when you know it, since automatic detection loses about a second of the first utterance.
- Search ignores accents (Unicode tokenizer with diacritics removed) and does not stem, so it behaves the same in every language.

## 7. Honest caveats

- Speaker labels in the live view are provisional; the runtime never revises them. The post-pass after Stop produces the stored transcript.
- Word confidence is not available from the runtime (always 1.0), so the UI cannot flag uncertain words yet.
- Meetings with more than 8 speakers, strong overlap or far-field audio will degrade; the eval harness reports how much on real recordings.
- Everything here is a snapshot of October 2026. The runtime is three months old and improving quickly; the manifests pin exact versions and hashes so an update is a deliberate step.
