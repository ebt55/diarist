# Spike: NeMo-Speech.cpp v0.2.0 on this machine (15 Oct 2026)

Test machine: RTX 3080 10 GB, driver 610.88, Ryzen 9 5900X, Windows 11 26200. All binaries ran with a stripped PATH (engine `bin`, System32, Windows), which hides the CUDA toolkits installed on this PC. VRAM is the `nvidia-smi` total sampled every 250 ms, minus the desktop baseline (~1.5 GB). Windows does not report per-process VRAM. Scripts and raw outputs are in the session scratchpad.

## Setup

| Item | Result |
|---|---|
| Zip | `nemo-speech-0.2.0-windows-x86_64-cuda.zip`, 97.5 MiB. SHA-256 `9ffe5e84…f98d` matches the `.sha256` file, `SHA256SUMS` and the GitHub digest |
| Extracted | 134.1 MiB: `bin`, `include`, `lib`, `share` |
| CUDA DLLs | `ggml-cuda.dll` 101.2 MiB (CUDA runtime statically linked) and `cublas64_13.dll` 1.5 MiB (shim). No cudart or cublasLt; only the driver's `nvcuda.dll` is needed |
| Other DLLs | tts 13.7 MiB, asr 4.0 MiB, llama 2.5 MiB, ggml 1.6 MiB, MSVC runtime 1.6 MiB. `nemo_speech_asr_c.dll` (55 KB) exports 27 ASR + 13 diar C functions; a `.lib` is shipped |
| GPU reported | `doctor`: `[0] gpu NVIDIA GeForce RTX 3080 (10.0 GiB)`; log `backend=CUDA0` |
| Models | `nemo-speech pull <name>`. Directory comes from env `NEMO_SPEECH_MODEL_DIR` (no flag; default is the user cache). Layout `<dir>\nvidia\<repo>\<rev>\<file>.gguf`. SHA-checked, ~1 min each |
| GGUFs (all Q8_0) | Nemotron-3-Diarization 102.1 MiB · nemotron-speech-streaming-en-0.6b 667.5 MiB · parakeet-tdt-0.6b-v3 680.9 MiB |
| Sample | AMI ES2002a Mix-Headset, CC-BY-4.0: 21:12.6, 16 kHz mono, 4 speakers. Reference RTTM: pyannote/AMI-diarization-setup `only_words/rttms/train/ES2002a.rttm`. The AMI server needed `curl --ssl-revoke-best-effort` |

## A. Cold start (CLI on a 3 s clip: load, warmup and decode)

| Run | Wall (run 1 / 2) | VRAM delta |
|---|---|---|
| `transcribe`, nemotron-en | 847 / 759 ms | ~1.65 GB |
| plus `--diarize` | 1285 / 1244 ms | ~1.9 GB |
| `serve` (ASR + diar), launch to `/ready` | 4.6 s (5.9 s with endpointing on) | 1.18 GB idle |

## B/C. File mode, full 21:12 meeting

| Run | Wall | × real time | Peak VRAM | Words | Speakers | Word speaker accuracy* |
|---|---|---|---|---|---|---|
| nemotron-en, ASR only | 28.8 s | 44 | 1.79 GB | 2448 | – | – |
| nemotron-en `--diarize` (B) | 48.3 s | 26 | 2.20 GB | 2448 | 4 | **63.3%** |
| nemotron-en `--stream --endpointing --diarize` | 47.6 s | 27 | ~2.2 GB | 2448 | 4 | **99.6%** |
| Parakeet TDT v3 `--diarize` (C) | 35.5 s | 36 | **4.86 GB** | 2513 | 4 | 63.2% |
| Parakeet words re-tagged from a `diarize` RTTM | +19.2 s | – | 0.85 GB | 2513 | 4 | 99.2% |
| `diarize` (streaming) | 19.2 s | 66 | 0.85 GB | – | 4 | DER 27.2% (miss 11.0, FA 15.6, confusion 0.6)** |

\*Words spoken while exactly one reference speaker is active, scored after the best one-to-one label mapping. \*\*pyannote.metrics at collar 0; collar 0.25 gives 23.7%. The `only_words` reference probably inflates false alarms.

**New bug.** Plain `transcribe --diarize` on the 21-minute file mislabels early words:

| Window (s) | 0–300 | 300–600 | 600–900 | 900–end |
|---|---|---|---|---|
| Accuracy | 62.2% | 63.5% | 99.8% | 99.1% |

Files cut at 350, 420 and 600 s score 99.5–99.7%, and the diarizer itself is fine. So the fault appears only when one end-of-file final covers a long file. One hypothesis is the ~20 min compaction horizon (U).

There is no max-speakers flag, and the API's `max_speaker_count` is ignored. The default diarizer is Nemotron-3-Diarization (from the help text and the `[model] cached` log line).

Ten consecutive words:
```json
[{"word":"lots","start":1185.76,"end":1185.92,"confidence":1,"speaker":2},{"word":"of","start":1185.92,"end":1186,"confidence":1,"speaker":2},{"word":"unplugging","start":1186.08,"end":1186.4,"confidence":1,"speaker":2},{"word":"next","start":1215.84,"end":1215.92,"confidence":1,"speaker":1},{"word":"semester","start":1216,"end":1216.24,"confidence":1,"speaker":1},{"word":"this","start":1216.64,"end":1216.72,"confidence":1,"speaker":1},{"word":"is","start":1216.72,"end":1216.8,"confidence":1,"speaker":1},{"word":"quite","start":1216.8,"end":1216.88,"confidence":1,"speaker":1},{"word":"good","start":1216.96,"end":1217.04,"confidence":1,"speaker":1},{"word":"trick","start":1217.2,"end":1217.28,"confidence":1,"speaker":1}]
```

Excerpt 125–145 s (eyeballed only, no WER measured):

| Nemotron Speech Streaming EN | Parakeet TDT v3 |
|---|---|
| to the design um I'm not really sure what what you guys have already received um in your emails what did you get um I just got the project announcement but designing a remote control yeah that's is that what everybody got okay um so we're gonna | the design. Um I'm not really sure what what you guys have already received. Um in your emails. What did you get? Um I just got the project announcement by the project is uh designing remote control. That's about it. Yeah, that's that what everybody got. Okay. Um so we're gonna have |

Parakeet adds punctuation and casing and catches more words. Nemotron-en often leaves long stretches unpunctuated.

## D. Offline vs streaming diarization (clip 50–350 s)

| Mode | Wall | DER (collar 0) | Miss | FA | Confusion | DER (collar 0.25) |
|---|---|---|---|---|---|---|
| streaming (default) | 4.2 s | **24.4%** | 18.1 | 5.6 | 0.7 | 20.0% |
| `diarize --offline` | 1.3 s | 43.0% | 27.2 | 5.4 | **10.5** | 38.8% |

| t (s) | 240 | 270 | 300 | 330 | 345 |
|---|---|---|---|---|---|
| Reference | MEE008 | MEE007 | FEE005+MEE008 | MEE006 | FEE005 |
| Streaming | S2 | S4 | S1 | S3 | S1 |
| Offline | S2 | S2 ✗ | S4 ✗ | S4 ✗ | S4 ✗ |

Offline relabels speakers partway through the clip. Its VRAM could not be sampled (the run is too short).

## E. Realtime WebSocket (`serve`, slots 2, API key in env)

The client streamed 100 ms PCM16 frames in real time from 0:56.

| Check | Result |
|---|---|
| Time to first text delta | 0.82–0.85 s after the first frame (first word starts at 0.64 s) |
| Finals without commit, endpointing off (default) | None. Committing after 80 s returned `.completed` + `committed` in 0.13 s, so **#49 did not reproduce** |
| Finals without commit, `--asr.endpointing.enable=true` | 20 finals in 80 s, median 0.84 s after the last word |
| `speaker` on words | Every word, ids 1–4. Live accuracy over 56–146 s: **99.5%** |
| 5 s of zeros, then speech | Recovered (119 events after). 20 s of zeros also recovered. **#48 did not reproduce** |
| Lag (sent − processed) | 1 session: median −0.08 s, max 0.09 s. 2 concurrent sessions: max 0.16 s. Both keep up |
| VRAM | Idle with models loaded +1.18 GB; +47–73 MiB with 1 session; +100 MiB with 2. Process RAM ~300 MB |

## F. C ABI (`asr.h`, `diar.h` from the zip)

| Topic | Finding |
|---|---|
| Load | `nemo_speech_asr_create(&cfg)` with size-prefixed, append-only optional sub-configs: backend (`int32 gpu`, -1 = CPU; no device string), model `path`, endpointing, **diar `model_path`**, batching `state_arena_slots` |
| Stream | `streaming_recognize(opts{enable_speaker_diarization, enable_word_time_offsets, interim_results})`, then `push_f32` (8–96 kHz), `next` (NULL = needs more audio), `force_endpoint`, `finish`, `close` |
| Result | `is_final`, `audio_processed`, transcript, and per word: text, int32 start/end, confidence, `speaker_tag` (0 = untagged). No revision API |
| `diar.h` | Separate model (`preset` `v3-streaming`/`v3-offline`) and stream. **Per-frame probabilities are readable live** (`diar_frame_probs`, 8 speakers at 10 ms), plus `diar_segments` with thresholds. Enough to build label refresh ourselves |
| Threads, errors | Status enum; thread-local `last_error`. Each diar stream is single-threaded and compute serializes internally; ASR batches concurrent callers |

## What this means

1. **Does the CUDA zip work out of the box on this 3080? Yes.** No toolkit, no PATH setup, and sm_86 kernels are native.
2. **VRAM budget:** ~1.3 GB for live (ASR and diar resident, plus sessions). File mode adds ~2.2 GB with Nemotron or ~4.9 GB with Parakeet. The worst case, ~6.5 GB, fits in 10 GB.
3. **Models:** live uses Nemotron Speech Streaming EN. Files use Parakeet TDT v3 with words re-tagged from a standalone `diarize` pass; fallback is Nemotron-en with `--stream --endpointing --diarize`. Never use plain `transcribe --diarize` on long files.
4. **Use the WebSocket for v2 live.** There is no blocker: both bugs were absent, two sessions keep up in real time, and tags were accurate. Endpointing has to be enabled server-wide at launch. Labels are never revised, so a post-meeting file pass becomes the canonical transcript. The C ABI is a later upgrade path, since `diar_frame_probs` enables label refresh.
5. **Corrections to my earlier report:** #48 and #49 did not reproduce. Live tags held up in practice (only 90 s tested). Long-file `transcribe --diarize` has a tagging bug. `--offline` exists only on `diarize`, and it scored worse here. The only bundled CUDA DLL is the 1.5 MiB cuBLAS shim.
