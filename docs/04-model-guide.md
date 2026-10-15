# Model and hardware guide

Status: v0.1, 15 Oct 2026. Source for the README section "Choosing your package and models" (Phase 5). Numbers marked **measured** come from docs/research/03 and 08 on an RTX 3080 10 GB; numbers marked *published* come from model cards and leaderboards cited in docs/research/01, 02 and 04; anything else is an estimate and says so. PENDING items have a test running or planned.

## 1. What runs inside meet-assist

Three neural models and one runtime. Nothing else. No Python, no PyTorch, no CUDA toolkit on the user's machine.

| Component | What it does | Size on disk | Licence |
|---|---|---|---|
| NeMo-Speech.cpp 0.2.0 (NVIDIA) | GGUF inference runtime, one zip per GPU backend | 98 MB zip, 134 MB extracted (CUDA build) | Apache-2.0 |
| Nemotron 3 Diarization, q8_0 | Who spoke when, up to 8 speakers, streaming and file | 102 MB | OpenMDW-1.1 |
| Nemotron Speech Streaming EN 0.6B, q8_0 | Speech to text for live and file mode (default) | 668 MB | NVIDIA Open Model Licence |
| Parakeet TDT 0.6B v3, q8_0 | Optional higher-accuracy speech to text for files, 25 languages | 681 MB | CC-BY-4.0 |
| Nemotron 3.5 ASR Streaming 0.6B, q8_0 | PENDING test: multilingual streaming model that may replace the EN model for both modes | 742 MB | OpenMDW-1.1 |

First-run download with the defaults: engine plus diarizer plus one ASR model, about 870 MB. Everything: 1.6 GB.

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

## 3. Speech to text: why Nemotron streaming by default and Parakeet as an option

Open ASR leaderboard average WER, lower is better (*published*, October 2026):

| Model | Avg WER | Word timestamps | Runs in our runtime | Streaming |
|---|---|---|---|---|
| Canary-Qwen 2.5B | 5.63 | no | no | no |
| Qwen3-ASR 1.7B | 5.76 | no | no, vLLM only | no |
| Parakeet TDT 0.6B v2 (English) | 6.05 | yes | yes | no |
| **Parakeet TDT 0.6B v3** (25 languages) | 6.34 | yes | yes | no |
| Kyutai STT 2.6B | 6.4 | yes, 2.5 s delay | no | yes |
| **Nemotron Speech Streaming EN 0.6B** | 6.93 at 1.1 s latency | yes | yes | yes |
| Whisper large-v3 (for reference) | about 7.4 | approximate | no | no |

Why not the top two: they give no word timestamps, and speaker assignment needs the start and end of every word. They also run only inside a Python stack.

Why Nemotron streaming is the default for both live and file mode:
- Live needs a cache-aware streaming model; this is the one in the runtime. **Measured** 0.83 s from audio to first text.
- For files, the same model fed through the runtime's streaming recognizer with endpointing and diarization covered a 77-minute recording completely (12,266 words, 8 labels, 27x real time, 2.6 GB VRAM) and its speaker tags agreed 99.9 percent with the standalone diarizer. **Measured.**
- Parakeet TDT v3 is about 0.6 WER points better and adds punctuation and casing, but on the same 77-minute file it silently dropped a 45-second span and two 10-second spans with no error. **Measured.** PENDING: a chunked test will show whether that is the runtime's long-file splitter (then Parakeet returns as the file-mode default with our own chunking) or the model.
- PENDING: Nemotron 3.5 streaming is multilingual (40+ locales, `language: auto`) and is the runtime's own default. If it matches the EN model on coverage and adds punctuation, it becomes the single ASR model.

Realistic expectation: on clean meeting audio about 1 word in 15 to 20 is wrong or missing with any of these models; on AMI-style far-field recordings about 1 in 9 (*published* AMI WER 11.3 for Parakeet v3). The product's value is in who said what, stored and searchable, not in a perfect verbatim.

## 4. Which package for which PC

The runtime ships three Windows zips. The app downloads the right one after checking the GPU.

| Your GPU | Package | Status |
|---|---|---|
| NVIDIA, driver new enough for CUDA 13 (R580 or newer is the estimate; 610.88 **measured** working) | `windows-x86_64-cuda` | **Measured** on RTX 3080 10 GB; kernels built for Turing (sm_75), Ampere (sm_86), Ada (sm_89) and Blackwell (sm_120) |
| AMD or Intel GPU, or older NVIDIA driver | `windows-x86_64-vulkan` | Shipped by the runtime; untested by us. Expect it to work for file mode; live is PENDING a test |
| No usable GPU | `windows-x86_64-cpu` | Shipped by the runtime; untested by us. Speed unknown; file mode will be much slower than real time |

VRAM needed (**measured** on the 3080, excluding the desktop's own 1.5 GB):

| Scenario | VRAM | Notes |
|---|---|---|
| Live recording, two channels (mic plus system audio), ASR and diarizer resident | about 1.3 GB | Server idle 1.18 GB, plus 50 to 100 MB per session |
| File transcription, default Nemotron path | 2.2 to 2.6 GB peak | 21 min and 77 min files |
| Standalone diarization pass only | 0.7 to 0.85 GB | |
| File transcription, Parakeet option | 4.9 GB peak | |
| Live recording plus a queued file job | not concurrent | The app runs file jobs after recording stops |

Estimated tiers (not measured; the app enforces them by reading free VRAM before starting):

| Card VRAM | What works |
|---|---|
| 4 GB | Live and default file mode should fit with the server limited to 2 sessions; Parakeet will not. A user with a 4 GB card reported out-of-memory with the runtime's default 16 slots, which we do not use |
| 6 GB | Live and default file mode comfortably; Parakeet only with nothing else on the GPU |
| 8 GB and up | Everything, including Parakeet |
| 10 GB (RTX 3080, the development machine) | Everything, with room for a browser and the meeting app on the same GPU |

Other requirements:
- Windows 10 (1803 or newer) or Windows 11, 64-bit. WebView2 is already present on these; the installer fetches it if not.
- Windows 11 22H2 or newer for operating-system echo cancellation when recording without headphones; older builds fall back to our own echo gate (Phase 3).
- RAM: the engine process uses about 300 MB (**measured**); 8 GB total is an estimate for a comfortable machine; the CPU package needs RAM for the models themselves (about 1.5 GB) plus working memory.
- Disk: 1.6 GB for engine and models; recordings about 90 MB per hour as stereo FLAC with 30-day retention by default; the transcript database is small (a year of daily meetings is tens of MB).
- The engine binaries are unsigned by NVIDIA; Windows Smart App Control, when enabled, may block them until the app is signed (planned).

## 5. Languages

- Diarization is language-independent.
- Live: English only with the EN streaming model. PENDING: Nemotron 3.5 would add 40+ locales with automatic detection.
- File mode: Parakeet v3 covers 25 European languages; the EN streaming model is English only.
- Search ignores accents (Unicode tokenizer with diacritics removed) and does not stem, so it behaves the same in every language.

## 6. Honest caveats

- Speaker labels in the live view are provisional; the runtime never revises them. The post-pass after Stop produces the stored transcript.
- Word confidence is not available from the runtime (always 1.0), so the UI cannot flag uncertain words yet.
- Meetings with more than 8 speakers, strong overlap or far-field audio will degrade; the eval harness reports how much on real recordings.
- Everything here is a snapshot of October 2026. The runtime is three months old and improving quickly; the manifests pin exact versions and hashes so an update is a deliberate step.
