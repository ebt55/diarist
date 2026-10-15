# Research: GGUF speech runtimes on Windows (verified 15 Oct 2026)

Opus 5.5 research agent, read via GitHub API and Hugging Face. C = confirmed in a primary source. U = unconfirmed.

## NeMo-Speech.cpp (NVIDIA, Apache-2.0)

**Windows + CUDA**
- Prebuilt (C): releases v0.1.0 (19 Aug 2026), v0.2.0 (2 Oct 2026) and nightlies ship `nemo-speech-<ver>-windows-x86_64-{cpu,cuda,vulkan}.zip`.
- CUDA zip built with CUDA 13.0, arch list `75-real;80-virtual;86-real;89-real;120a-real` (sm_86 = RTX 3080), cuBLAS shim, server profile. Source: `.github/workflows/release.yml`.
- Self-contained (C): `package-release.ps1` requires every imported DLL to be bundled or come from Windows or the driver (`nvcuda`, `nvml`). No CUDA toolkit needed at runtime. Driver R580+ implied by CUDA 13 (U). Ours is 610.88.
- `install.ps1` (C): `-Backend auto` picks cuda when `nvidia-smi` works. Downloads release zip, checks SHA-256, installs to `%LOCALAPPDATA%\Programs\NeMoSpeech`. Builds from source only if no zip exists.
- Build from source (C): VS 2022 Build Tools, CMake >= 3.26, Ninja, CUDA 12.x/13.x, vcpkg (auto-provisioned). `scripts\windows\build.ps1 -Backend cuda -CudaArch 86`.
- ITN/TN not supported on Windows (C).

**Streaming PCM input**
- CLI (C): no stdin, pipe or socket input. `app/transcribe.cpp` reads WAV files only (PCM16 or f32).
- `--live` (C): `microphone_capture.cpp` opens the default microphone via miniaudio. No device selection, no loopback.
- Server (C): `nemo-speech serve` exposes WebSocket `/v1/audio/transcriptions/realtime` taking binary LE PCM16 frames (8 to 96 kHz) after optional `session.update`.
- C ABI streaming (C): `nemo_speech_asr_streaming_recognize`, `nemo_speech_asr_stream_push_f32`, `nemo_speech_asr_stream_next`.

**Output formats**
- CLI `--json` (C): `{file, text, confidence, duration, languages[], words:[{word, start, end, confidence, speaker?}]}`. No segments. `speaker` is 1-based, omitted when 0. Seconds.
- Confidence always 1.0 (C, issue #55).
- Live CLI: human preview on stderr; final transcript on Ctrl-C; labels revisable up to 10 s (V2).
- WebSocket events (C): `transcription.delta` (partial text + audio_processed), `transcription.completed` (transcript + words[] with speaker when requested), `input_audio_buffer.committed/cleared`, `error`.
- **No label revisions over WebSocket or C ABI (C):** neither exposes the C++ `stable_speaker_time()` / `refresh_speaker_tags()` that the CLI uses. Speaker tags on finals are provisional and never revised.
- HTTP (C): `POST /v1/audio/transcriptions` with `response_format=verbose_json&diarization=true` returns words[] with speaker. `POST /v1/audio/diarizations` returns `{segments:[{start,end,speaker}]}`.

**Nemotron 3 Diarization**
- In v0.2.0 (C): added PR #50 (24 Sep 2026), default diarizer since #52.
- GGUF files (C), q8_0 only, NVIDIA HF repos: `Nemotron-3-Diarization.q8_0.gguf` 107 MB; `nemotron-3.5-asr-streaming-0.6b.q8_0.gguf` 742 MB; `nemotron-speech-streaming-en-0.6b.q8_0.gguf` 700 MB.
- VRAM: undocumented. Default `asr.batching.state_arena_slots=16` OOM'd on a 4 GB card; reporter set it to 1 (issue #49). Expect fit on 10 GB with 2 to 4 slots (U).
- No RTX 3080 / Ampere performance reports found.

**Library, activity, limits**
- Stable C ABI headers `include/nemo_speech/{asr,diar,nmt,tts}.h` (v1). Release archives include headers, shared libs, CMake package (C). No Rust bindings.
- Created 15 Jul 2026, 25 squash commits, last push 13 Oct 2026, 179 stars, 20 open issues+PRs (C).
- Open live-path bugs (C): #48 streaming wedges after sustained zero-PCM frames; #49 WebSocket `commit` never returns `.completed` for utterances >= 1 s (C ABI works).
- 6.6 min cap only for full-attention diarization (`--offline`). Streaming diarization unlimited (C). Long ASR files auto-switch to streaming runner. Diar C ABI compacts after about 20 min. WebSocket cumulative input capped at `--max-upload-mb` (512 MiB default, about 4.6 h PCM16 at 16 kHz).
- Max speakers: 4 with Sortformer V2, 8 with Nemotron 3 (C). Parakeet TDT v3 rejects `--stream` and `--live` (C).

## transcribe.cpp (MIT)
- Diarization is Sortformer 4spk v2.1 only (F32/F16). Nemotron 3 Diarization not on main (issue #170, PR #175 open since 26 Sep 2026) (C).
- Streaming ASR for the Nemotron streaming models (C).
- Rust crate `transcribe-cpp` 0.4.0: streaming via `session.stream()/feed()`, diarization batch only (`DiarizeSession::run`) (C). CUDA on Windows builds from source or links prebuilt tarball; issue #79 open (static CUDA link manifest missing libs).
- ASR and diarization are separate outputs to merge ourselves, except the `multitalker-parakeet-streaming-0.6b-v1` bundle (4 speakers, English, offline-API only) (C).

## sherpa-onnx
- Prebuilt `win-x64-cuda` (CUDA 12/13, cuDNN 9, ORT 1.28.2) (C). Offline diarization = pyannote segmentation + embedding + clustering, in C API and Rust crate 1.13.8, provider defaults to cpu (C). No streaming diarization (C). Nemotron 3 Diarization PR #4032 open since 10 Oct 2026 (C).

## Not in prior research
- **audio.cpp** (0xShug0/audio.cpp, Apache-2.0, v0.9.1 on 7 Oct 2026): Windows CUDA 12.4 and 13.3 zips; `nemotron_3_diar` BF16 batch and stream; `nemotron_asr` streaming; "speaker-tagged ASR" since v0.9.0 (C from README). Quality unverified (U).
- **CrispASR** (CrispStrobe/CrispASR, MIT): Windows CUDA zip; Nemotron-3-Diarization as sortformer backend (C). Streaming unknown.

## Agent recommendation
Use NeMo-Speech.cpp v0.2.0 Windows CUDA zip. Live via `serve` WebSocket or C ABI; post-hoc via CLI or HTTP. Treat live labels as preview and run a canonical file pass when the meeting ends. Add dither so silent loopback is never exactly zero (bug #48). Do not depend on WebSocket `commit` (bug #49). Fallback: FFI to the C ABI DLLs.

## Sources
NVIDIA/NeMo-Speech.cpp (README, docs/cli.md, server.md, api.md, development/windows-build.md, scripts/install.ps1, .github/workflows/release.yml, app/microphone_capture.cpp, app/transcribe.cpp, server/http/http_server.cpp, include/nemo_speech/asr.h, diar.h, models/index.json, issues #48 #49 #54 #55); huggingface.co/nvidia/Nemotron-3-Diarization; handy-computer/transcribe.cpp; crates.io/crates/transcribe-cpp; k2-fsa/sherpa-onnx; 0xShug0/audio.cpp; huggingface.co/audio-cpp/Nemotron-3-Diarization-GGUF; CrispStrobe/CrispASR.
