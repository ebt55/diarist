# Model and runtime landscape, October 2026: is our stack still the best choice? (25 Oct 2026)

Research only. No code changed, nothing downloaded except web pages and API listings. Opus 5.5 research agent. Every claim carries a source and the date it was read or published. **Inference** marks a statement that is our reading or estimate, not something a source says. **Measured** marks something checked on the development PC during this research.

**Summary.** No newer runtime release exists: NeMo-Speech.cpp v0.2.0 (2 Oct 2026) is still the latest tag. But `main` got the most important accuracy fix since we pinned it. PR #46, merged 24 Oct 2026, fixes two bugs that drop words: a wrong first-chunk geometry in cache-aware streaming, and Q8 quantization of the Conformer pointwise convolutions. The second one lives in the GGUF files, not in the runtime. Both of our pinned Nemotron GGUFs carry all 48 pointwise tensors as Q8_0 (measured from their headers). NVIDIA has not republished them. The cheapest real lever for "many word errors sometimes" is therefore to re-convert the two Nemotron checkpoints with the fixed converter and measure them on the shipped runtime. On models: no NVIDIA ASR model has appeared since June 2026 except what we know. On the current Open ASR Leaderboard (version 21 Oct 2026), Nemotron EN is within about one WER point of the best open models on conversational speech, and the models that beat it either give no word timestamps or need a second runtime. Parakeet Unified EN 0.6B (April 2026) is the one in-family candidate worth a conversion attempt. The diarizer has no update and no better open rival. Section 7 ranks six spikes.

## 0. What we ship and what was checked

| Component | Pin | Source of the pin |
|---|---|---|
| Runtime | NeMo-Speech.cpp 0.2.0, Windows CUDA, Vulkan and CPU zips | `manifests/engine.json` |
| ASR, English (default) | `nvidia/nemotron-speech-streaming-en-0.6b` @ `ebe59e5`, q8_0 | `manifests/models.json` |
| ASR, multilingual | `nvidia/nemotron-3.5-asr-streaming-0.6b` @ `1c8deae`, q8_0 | `manifests/models.json` |
| ASR, experimental | `nvidia/parakeet-tdt-0.6b-v3` @ `541d1f9`, q8_0 | `manifests/models.json` |
| Diarizer | `nvidia/Nemotron-3-Diarization` @ `f667ed7`, q8_0 | `manifests/models.json` |
| File setting | lookahead 13 (1.12 s), `--asr.diar.preset v3-offline`, `--stream --endpointing` | docs/04 sections 2 and 3, docs/research/12 |
| Live setting | lookahead 1 (160 ms), streaming diarizer preset | docs/04 section 3 |

Method (25 Oct 2026): GitHub API for releases, commits, PRs, issues and Actions runs of NVIDIA/NeMo-Speech.cpp and handy-computer/transcribe.cpp; Hugging Face API for commit histories, file lists and discussions of the pinned repos; the Open ASR Leaderboard space's own Gradio config (the result CSVs behind it are private); model cards. GGUF tensor types were read from the headers of the files the app had already installed (a 100-line stdlib reader in the scratchpad, nothing downloaded).

## 1. NeMo-Speech.cpp after v0.2.0

### Releases

| Tag | Date | Notes |
|---|---|---|
| v0.1.0 | 19 Aug 2026 | |
| v0.2.0 | 2 Oct 2026 | what we ship |
| `nightly` (moving tag) | asset built 17 Oct 2026 from `caa3f47` | Windows CUDA zip 102,186,191 B, SHA-256 `4c7e4b5f…b164` (from the release's `SHA256SUMS`) |

Source: https://github.com/NVIDIA/NeMo-Speech.cpp/releases (read 25 Oct 2026). The scheduled nightly builds of 21, 23 and 24 Oct failed or were cancelled, and the run of 25 Oct was still pending when checked, so the nightly asset does not contain anything merged after 16 Oct (https://github.com/NVIDIA/NeMo-Speech.cpp/actions, Release workflow, read 25 Oct 2026).

**Is there a release to bump to? No.** Inference: wait for the next tag, but measure `main` now (spike 2), because the fixes below are in no binary we can pin yet.

### Commits on `main` since v0.2.0 (9, read 25 Oct 2026)

Source: https://github.com/NVIDIA/NeMo-Speech.cpp/compare/v0.2.0...main

| Commit | Date | PR | What it changes | Matters for Diarist? |
|---|---|---|---|---|
| `b809bbb` | 3 Oct | #43 | TTS: reject non-finite NanoCodec audio | no |
| `8642eaa` | 7 Oct | #62 | Diarization front end: power spectrum once per frame instead of per mel band, lazy buffer compaction. Whole-file `diarize` 1.74 times faster on an A100 and 1.31 times on an A5000, live streaming about 4 percent; features bit-identical on x86-64, DER unchanged | speed of the standalone `diarize` pass (which the app skips by default) and possibly the fused pass (inference) |
| `e59dfb0` | 11 Oct | #65 | TTS on integrated GPUs | no |
| `083861c` | 13 Oct | #66 | TTS speaker ids | no |
| `e37a54a` | 16 Oct | #69 | OpenFST and Sparrowhawk URLs (ITN build) | no; ITN still builds on Linux and macOS only (docs/build.md on `main`) |
| `caa3f47` | 16 Oct | #70 | Diarization: keep frames until their words are tagged. Fixes our issue #67: plain `transcribe --diarize` on AMI ES2002a 59.2 to 99.7 percent of words on the right speaker | yes: it makes `--stream` without `--endpointing`, and the plain offline path for non-streaming models, safe on long files |
| `2bdfb03` | 20 Oct | #47 | mmap-backed weight loading; peak memory 1.70 GB to about 520 MB in the PR's test. CUDA reports no `buffer_from_host_ptr` and "is unaffected" | the CPU package's RAM (inference); nothing for CUDA |
| `1a7e945` | 24 Oct | #46 | **Streaming startup geometry and Q8 pointwise precision** (below) | **yes, word accuracy** |
| `e680fc3` | 24 Oct | #76 | licence notices | no |

Sources: the PR pages https://github.com/NVIDIA/NeMo-Speech.cpp/pull/62 (merged 7 Oct 2026), /pull/70 (16 Oct 2026), /pull/47 (20 Oct 2026), /pull/46 (24 Oct 2026).

### PR #46 in detail: two causes of dropped words

Source: https://github.com/NVIDIA/NeMo-Speech.cpp/pull/46 (opened 14 Sep, merged 24 Oct 2026) and `docs/model-conversion.md` on `main` (24 Oct 2026).

1. **Startup alignment (runtime).** The first chunk of a stream had no previous audio, but the runner prepended zero mel history and the encoder applied the overlap crop meant for later chunks. At lookahead 13 that encodes 9 zero frames plus 112 real frames and drops two encoder frames. The PR author also saw a later blank-only interval disappear once startup was fixed. Fix in `src/asr/runner.cpp` and `src/asr/encoder/fastconformer.cpp`: the first chunk is `1 + subsampling × R` real mel frames with no crop.
2. **Q8 pointwise convolutions (converter).** NeMo-Speech.cpp quantized the Conformer pointwise convolutions like linear layers; transcribe.cpp keeps them in F16. On one recording, 120 s windows at 17 starting points 20 ms apart lost a speaker's introduction in 1 of 17 runs with NVIDIA's published Q8 GGUF (the pinned 3.5 file, `1c8deae`) and in 0 of 17 with only those weights restored to F16. The PR says plainly that this is "not an independent corpus or a WER/CER improvement claim". Fix in `conversion/asr.py`: cache-aware RNNT Q8_0 exports keep `encoder.layers.N.conv.pointwise_conv{1,2}.weight` in F16; the model grows by 67.5 MiB.

The project's conversion doc now says (24 Oct 2026): quantizing these kernels "can cause speech to be decoded as long blank runs", and "Existing GGUFs are not repaired by updating the runtime: regenerate them from the original `.nemo` checkpoint with `--outtype q8_0`." A maintainer's review on 23 Oct 2026: "This is a critical fix, both issues directly lose words in the default format we use." Another contributor reported on 23 Oct 2026 that trimming initial silence halved the words lost at stream start on a small pt-BR and English set (PR #46 thread).

**Measured, 25 Oct 2026:** the pinned GGUFs carry exactly the tensors the fix targets.

| File (pinned revision) | Tensors | Q8_0 | F16 | F32 | Pointwise conv weights |
|---|---|---|---|---|---|
| Nemotron EN `ebe59e5` | 653 | 272 | 30 | 351 | 48 of 48 in Q8_0 (e.g. `encoder.layers.0.conv.pointwise_conv1.weight`, 1024 × 2048) |
| Nemotron 3.5 `1c8deae` | 657 | 272 | 30 | 355 | 48 of 48 in Q8_0 |
| Nemotron 3 Diarization `f667ed7` | 360 | 126 | 1 | 233 | (not a cache-aware RNNT; out of scope of the fix) |

Supporting evidence that the Q8 export costs accuracy at our file setting:

| Engine and weights | Lookahead | LibriSpeech test-clean WER | Source |
|---|---|---|---|
| NeMo (FP32) | 1.12 s | 2.32 | NeMo-Speech.cpp `BENCHMARK.md`, RTX 4090 (read 25 Oct 2026) |
| NeMo-Speech.cpp, NVIDIA Q8_0 | 1.12 s | 2.51 | same |
| NeMo (FP32) | 160 ms | 2.69 | same |
| NeMo-Speech.cpp, NVIDIA Q8_0 | 160 ms | 2.66 | same |
| transcribe.cpp, Q8_0 with F16 pointwise | 1.04 s (R 13) streaming | 2.31 | transcribe.cpp `docs/models/nemotron-speech-streaming-en-0.6b.md` (read 25 Oct 2026) |
| transcribe.cpp, F16 | R 13 streaming | 2.29 | same |

Inference: the 0.19-point gap at 1.12 s on short LibriSpeech utterances is probably mostly the startup bug (every utterance is a new stream, and R 13 crops more), while the pointwise bug shows as rare mid-stream blank runs on long audio. For a 45-minute meeting the startup bug touches only the first second; the pointwise bug can strike anywhere. That pattern, a mostly fine transcript with a sentence or a speaker's turn sometimes missing, matches "many word errors sometimes" better than a uniform model-quality gap does. Our 77-minute check in research/08 section 7 looked for empty 60 s buckets, which would miss a lost sentence.

Inference: a re-converted GGUF should load in the shipped 0.2.0 runtime, because F16 pointwise tensors are an existing path (`--outtype fp16` existed in 0.2.0) and PR #46's runtime changes touch only first-chunk geometry. Spike 1 checks this first.

### Open issues and PRs that touch our paths (read 25 Oct 2026)

| Item | State | Path | What it says | Effect on us |
|---|---|---|---|---|
| [#40](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/40) token-silence EOU misfires mid-sentence | open since 10 Sep | `--stream --endpointing` (our file jobs and live) | The endpointer compares decode-clock token emission against the 800 ms threshold, so decode latency looks like silence; the report also showed a hard state reset corrupting words | Our reading of the v0.2.0 source (`src/asr/runner.cpp`): EOU now "publishes a hypothesis; it does not end the acoustic stream" and keeps encoder caches and predictor state, so the reset part predates 0.2.0. The false-fire part remains: extra finals, not changed words (inference). A maintainer wrote on 11 Sep that false endpoints "happens frequently" and cause "severe degradation" in the pre-0.2.0 code (PR #41 thread) |
| [#41](https://github.com/NVIDIA/NeMo-Speech.cpp/pull/41) soft EOU | open, last updated 11 Sep | same | Targets the pre-0.2.0 runner | stale (inference) |
| [#71](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/71) Arabic streaming RNNT drops words with Q4 and FP16 | open since 18 Oct | Nemotron 3.5, CPU, R 0 | Drops one verse fragment in one repetition; FP16 conversion also drops it; unresolved whether model, converter or decoder | a second, unexplained drop report on 3.5; not Q8-pointwise-specific |
| [#72](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/72) / [#73](https://github.com/NVIDIA/NeMo-Speech.cpp/pull/73) realtime WebSocket drops under load | issue and fix PR open since 22 Oct | live WebSocket | cpp-httplib 0.50.1 rejects frames whose header straddles its 4 KiB buffer when frames queue up; 50 of 648 alignments dropped; v0.63.0 fixes it | live sessions on a busy GPU could lose the connection without a close frame; the post-pass covers the stored transcript |
| [#48](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/48) wedge after zero PCM, Vulkan `GGML_ASSERT` | open | live | unchanged | our dither stays |
| [#49](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/49) `commit` without `.completed` | open | live | unchanged | we do not rely on it |
| [#55](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/55) confidence always 1.0 | open | all | unchanged | |
| [#56](https://github.com/NVIDIA/NeMo-Speech.cpp/issues/56) multitalker Parakeet support | open | — | maintainer, 1 Oct: "we have been planning to add support for it" | watch item for overlap |
| [#78](https://github.com/NVIDIA/NeMo-Speech.cpp/pull/78) timestamp audit after #46 | closed 25 Oct, not merged | timestamps | No further #46-induced timing defect found; EOF word times can exceed the audio duration, pre-existing; a maintainer says word times "should always fall within the actual audio" (PR #46 thread, 24 Oct) | we already clamp word ends (research/08 pitfall 11) |

Unchanged on `main` (read 25 Oct 2026): the punctuation gate (`vocab_self_punctuates` in `src/asr/recognizer.cpp`), so `--pnc-model` stays ignored for both Nemotron models (research/12 section 3); `docs/asr/configuration.md` (no new flags, no RNNT beam search, boosting unchanged, so research/12 section 4 stands); `models/index.json` (still points at `1c8deae` for 3.5 and `ebe59e5` for EN). Nothing on `main` targets Vulkan or Windows beyond #47's CPU mmap path.

## 2. The pinned model repos on Hugging Face

Read 25 Oct 2026 from `https://huggingface.co/api/models/<repo>/commits/main` and `/discussions`.

| Repo | Our revision | Newer commits | Notes |
|---|---|---|---|
| nvidia/nemotron-speech-streaming-en-0.6b | `ebe59e5` (5 Aug 2026, "Add NeMo-Speech.cpp GGUF") | **none** | The `.nemo` is the March 2026 checkpoint (`2ea504f`, 13 Mar 2026). Discussion #18 (25 Jun 2026): a word repeated in speech can run away ("way way way …"); the runtime caps it with `max_symbols_per_step` (v0.2.0 `src/asr/decoders/rnnt_greedy_decoder.cpp`) |
| nvidia/nemotron-3.5-asr-streaming-0.6b | `1c8deae` (5 Aug 2026) | `ea30d66` (10 Sep 2026, re-converted GGUF with embedded SentencePiece tokenizer) | Known; research/12 section 4 measured it (boosting harmful, 10 edit operations of drift). Its pointwise tensors were quantized by the old converter too (inference from the date: the fix landed 24 Oct). No model-card WER change since June |
| nvidia/parakeet-tdt-0.6b-v3 | `541d1f9` (5 Aug 2026) | **none** | Discussion #42 (8 May 2026, still open, a second user on 31 Jul): "The model sometimes drops full sentences", in NeMo's own pipeline and ONNX. Confirms research/08 section 8: the drop is model-side |
| nvidia/Nemotron-3-Diarization | `f667ed7` (24 Sep 2026) | **none** | See section 5 |

No repo has a GGUF built with the 24 Oct converter yet. Inference: NVIDIA will likely republish, since its own doc now tells users to regenerate; check the commit pages before converting ourselves.

## 3. New ASR models since mid-2026, and the current leaderboard

### NVIDIA

No new NVIDIA ASR model since Nemotron 3.5 (June 2026). Sources: NVIDIA-NeMo/Speech README "Updates" (read 25 Oct 2026: newest ASR entries are 2026-06 Nemotron 3.5 and 2026-04 Parakeet Unified EN; NeMo Speech 3.0 released 7 Aug 2026); Hugging Face listing of `nvidia` speech models sorted by creation (read 25 Oct 2026).

| Model | Released | Type | Runs in NeMo-Speech.cpp? | Notes |
|---|---|---|---|---|
| parakeet-unified-en-0.6b | 7 Apr 2026 | FastConformer + RNNT, one set of weights for offline and buffered streaming (min. 160 ms), English, punctuation and capitalization | **Not listed** (README supports Nemotron 3.5, Nemotron EN, Parakeet TDT v3, Parakeet CTC 1.1B). The converter handles FastConformer CTC, RNNT and TDT heads; whether it converts this model is untested (inference) | Card, average WER without PnC: offline **5.91**, 1.12 s 6.29, 160 ms 8.44, against Nemotron EN 6.92 at 1.12 s in the same table; AMI offline **10.14** (Nemotron EN card, same old AMI split: 11.73 at 1.12 s). Licence NVIDIA Open Model License. transcribe.cpp ports it (Q8_0 1.60 percent on LibriSpeech test-clean) |
| multitalker-parakeet-streaming-0.6b-v1 | Oct 2025 | speaker-conditioned ASR, English, 4 speakers (Sortformer v2.1) | no (planned, issue #56) | research/04 section 5 |
| parakeet_realtime_eou_120m-v1 | 2025 | end-of-utterance model | no | not a transcription upgrade |

Sources: https://huggingface.co/nvidia/parakeet-unified-en-0.6b (card read 25 Oct 2026), https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b (card read 25 Oct 2026), https://github.com/NVIDIA/NeMo-Speech.cpp README (25 Oct 2026), https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/parakeet-unified-en-0.6b.md (25 Oct 2026).

### Open ASR Leaderboard, version 21 Oct 2026

Source: https://huggingface.co/spaces/hf-audio/open_asr_leaderboard, version "21-10-2026" ("Add ibm-granite/granite-speech-5.0-2b"), values read from the space's Gradio config on 25 Oct 2026. There is no streaming track: the tabs are Leaderboard, Multilingual, Long-form, Private data, Pareto, Benchmark fitting and About. The English average now covers AMI-Cleaned, Earnings22-Cleaned-AA, Gigaspeech-Cleaned, LibriSpeech clean and other, SPGISpeech, URGENT24, Voice Arena Monsoon, VoxPopuli-AA-Cleaned and two private sets, so it is not comparable with older snapshots. "Private (conversational)" is Appen and DataoceanAI conversational speech nobody trained on; with AMI it is the closest column to our meetings.

Open-weight models only, ranked by the meeting-like columns. WER in percent.

| Model | Avg | AMI-Cleaned | Earnings22 | Private conversational | Word timestamps | Streaming | Licence | Runtime on Windows |
|---|---|---|---|---|---|---|---|---|
| Qwen3-ASR-1.7B | 5.40 | 8.31 | 5.84 | **12.14** | via a separate ForcedAligner model, 11 languages, 5 min | vLLM only | Apache-2.0 | transcribe.cpp (no timestamps) |
| Edge0 ARK-ASR-0.6B | 5.93 | 8.61 | 6.75 | 12.57 | not stated | not stated | Apache-2.0 | Transformers, remote code |
| Qwen3-ASR-0.6B | 6.24 | 9.33 | 7.83 | 12.74 | as 1.7B | vLLM only | Apache-2.0 | transcribe.cpp (no timestamps) |
| **Nemotron EN 0.6B (ours)** | 6.60 | 8.84 | 9.45 | **13.28** | yes | yes | NVIDIA Open Model License | **NeMo-Speech.cpp** |
| MOSS-Transcribe-Diarize | 6.17 | 8.33 | 6.09 | 13.40 | segments | no | Apache-2.0 | transcribe.cpp |
| Canary-Qwen 2.5B | 5.94 | 7.91 | 6.41 | 13.41 | no | no | CC-BY-4.0 | transcribe.cpp |
| Parakeet TDT 0.6B v3 | 6.18 | 9.42 | 5.85 | 13.44 | yes | no | CC-BY-4.0 | NeMo-Speech.cpp (drops spans) |
| Cohere Transcribe 03-2026 (2B) | 5.77 | 7.02 | 7.88 | 13.46 | no ("does not feature" timestamps) | no | Apache-2.0 | transcribe.cpp |
| Parakeet TDT 0.6B v2 | 6.02 | 9.09 | 6.76 | 13.58 | yes | no | CC-BY-4.0 | transcribe.cpp; NeMo-Speech.cpp's converter handles TDT, so likely convertible (inference) |
| Granite Speech 5.0 2B | 5.68 | **6.96** | 6.22 | 13.79 | not stated | no | Apache-2.0 | llama.cpp GGUF (card); segments of about 2 min |
| TheStage whisper-large-v3-turbo | 5.91 | 7.32 | 7.18 | 13.98 | not checked | no | CC-BY-4.0 | not checked (ONNX weights) |
| Granite Speech 5.0 TurboCTC 470M | 6.38 | 7.71 | 6.75 | 14.03 | no ("timestamp requests are rejected") | no | Apache-2.0 | transcribe.cpp |
| VibeVoice-ASR | 8.16 | 12.02 | 7.97 | 14.62 | speaker-attributed; timestamps not checked | no (a streaming 1.5B variant appeared 2 Sep 2026, no numbers on the card) | MIT | Transformers |
| Parakeet CTC 1.1B | 7.40 | 12.23 | 9.66 | 14.70 | yes | buffered | CC-BY-4.0 | NeMo-Speech.cpp |
| Whisper large-v3 | 7.16 | 13.63 | 7.99 | 15.13 | approximate | no | MIT | whisper.cpp |
| canary-1b-v2 | 7.27 | 13.03 | 7.28 | 15.16 | not checked | no | CC-BY-4.0 | transcribe.cpp |
| **Nemotron 3.5 0.6B (ours, multilingual)** | 9.75 | 13.43 | 15.30 | 16.30 | yes | yes | OpenMDW-1.1 | **NeMo-Speech.cpp** |
| Voxtral Mini 4B Realtime 2602 | 8.44 | 13.34 | 9.31 | 17.45 | no | yes | Apache-2.0 | transcribe.cpp |

Model-card facts in the timestamp, streaming, licence and runtime columns come from: https://huggingface.co/Qwen/Qwen3-ASR-1.7B, https://huggingface.co/CohereLabs/cohere-transcribe-03-2026, https://huggingface.co/ibm-granite/granite-speech-5.0-2b (released 20 Oct 2026), https://huggingface.co/ibm-granite/granite-speech-5.0-470m-turboctc (multilingual release 21 Oct 2026), https://huggingface.co/Edge0/ARK-ASR-0.6B, https://huggingface.co/microsoft/VibeVoice-ASR-Streaming-1.5B, and transcribe.cpp's README and `docs/models/*.md` (v0.4.1, 22 Oct 2026), all read 25 Oct 2026. Not on the current board: Kyutai STT (no new STT model since 2025 on `huggingface.co/kyutai`), Moonshine Streaming Medium (card: average 6.65, AMI 10.68, Earnings22 11.90 on the old split; MIT; no timestamps), Parakeet Unified EN.

What the table says:

- **Our English model is near the open frontier on conversational speech.** The best open model on private conversational data, Qwen3-ASR-1.7B, is 1.14 points (9 percent relative) better, and it needs a separate forced aligner for word times and vLLM for streaming. Everything better on AMI-Cleaned gives no word timestamps (Cohere, Granite 5.0, Canary-Qwen, TurboCTC) or runs only offline. Inference: swapping the ASR model buys at most about a point on our kind of audio and costs the fused diarization path; the runtime and conversion fixes of section 1 are the better first bet.
- **Nemotron 3.5 is clearly weaker in English**: 9.75 against 6.60 average, 16.30 against 13.28 on conversational data. Its own card "recommends" the EN model "for English-only transcription use cases" (https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b, read 25 Oct 2026). Part of the gap may be the setting: the leaderboard runs it through Transformers with `en-US` (`transformers/run_eval.py`, `--language` default `en-US`, https://github.com/huggingface/open_asr_leaderboard, read 25 Oct 2026) at the checkpoint's default context, which for 3.5 is the 320 ms entry (our GGUF header says `train_right_ctx` 3; research/12 section 1) while EN's default is 1.12 s (inference). Either way docs/04's choice of EN as the English default stands.
- **Parakeet TDT v3 does not beat EN on meeting-like columns** (AMI-Cleaned 9.42 against 8.84; conversational 13.44 against 13.28). Its lower average comes from Earnings22, SPGISpeech and VoxPopuli.

### Multilingual (leaderboard Multilingual tab, same version)

| Model | de | fr | it | es | pt | hi | nl | In our runtime |
|---|---|---|---|---|---|---|---|---|
| Nemotron 3.5 (ours) | 8.96 | 9.40 | 8.66 | 5.43 | 8.25 | 14.18 | 14.50 | yes, streaming |
| Parakeet TDT 0.6B v3 | 4.12 | 5.38 | 4.66 | 3.71 | 6.08 | – | 7.53 | yes, offline, drops spans |
| canary-1b-v2 | 4.06 | 4.79 | 4.75 | 3.23 | 6.26 | – | 6.73 | no (AED) |
| Qwen3-ASR-1.7B | 3.97 | 5.68 | 5.44 | 3.80 | 6.31 | 12.74 | 7.95 | no |
| Whisper large-v3 | 4.00 | 6.24 | 4.45 | 3.35 | 4.94 | 29.86 | 4.42 | no |

Nemotron 3.5 ranks 22nd of the 28 models with all seven languages. Inside our runtime, Parakeet v3 roughly halves the European-language WER, which is why a gap-filling dual pass (spike 6) is the multilingual lever, not a new model.

### Other runtimes

transcribe.cpp (MIT) is now a serious second runtime: v0.4.1 (22 Oct 2026) ships a prebuilt `windows-x86_64-cuda` package and 16 model families, including Qwen3-ASR, Cohere, Canary-Qwen, Granite TurboCTC, Parakeet Unified and both Nemotron models. Its diarizer is still Sortformer 4spk v2.1: the Nemotron 3 Diarization port is open as issue #170 and PR #175 (since 24 and 26 Sep 2026), and the Parakeet v3 drop issue #71 is open since 7 Jul 2026. Source: https://github.com/handy-computer/transcribe.cpp (README, releases, issues; read 25 Oct 2026). Inference: moving to it would trade our 8-speaker fused diarization for models that mostly lack word timestamps; not worth it today.

## 4. Diarizer

| Question | Answer | Source |
|---|---|---|
| Updates to Nemotron 3 Diarization? | None after `f667ed7` (24 Sep 2026) | HF commits, read 25 Oct 2026 |
| Known speaker count mode? | "not implemented" | NVIDIA reply, 12 Oct 2026, https://huggingface.co/nvidia/Nemotron-3-Diarization/discussions/7 |
| More than 8 speakers? | NVIDIA recommends EEND-VC: diarize windows of up to 8 speakers, then cluster per-window speaker embeddings globally with a cannot-link constraint | same thread, 12 and 19 Oct 2026 |
| Independent ranking | Unchanged: VoiceArena Diarization-Bench v1, Nemotron 3 DER 14.72, DiariZen 19.34, pyannoteAI Precision-3 20.56, Sortformer 4spk v2 24.61, pyannote community-1 30.61 | https://voicearena.com/diarization-bench, read 25 Oct 2026 |
| New open rivals | None released. pyannote-audio's latest is 4.0.7 (30 Jun 2026). BUT's "Pandemonium" (21 Oct 2026), a diarization-conditioned Parakeet multi-talker ASR for long meetings, says "The model will be released soon". Multitalker Parakeet support in NeMo-Speech.cpp is planned (issue #56) | GitHub pyannote/pyannote-audio releases; https://huggingface.co/BUT-FIT/Pandemonium; read 25 Oct 2026 |
| Precision of our GGUF | NVIDIA's GGUF is q8_0 (126 Q8_0 tensors, measured). The diarization converter's default is F32, described as "bit-tight parity with NeMo"; F16 and Q8_0 are for smaller artifacts | `conversion/diarization.py` and `docs/asr/models.md` on `main`, read 25 Oct 2026 |

Verdict: keep Nemotron 3 Diarization. The one diarizer lever left inside the runtime is precision (spike 5), with no published evidence either way.

## 5. Constraints applied

RTX 3080 10 GB for development; live within 2 GB of VRAM; fully local; permissive licences preferred; changing runtime is expensive; the owner wants measured accuracy and speed first. So the ranking below favours, in order: fixes that keep NeMo-Speech.cpp and the current models, then settings, then new models that convert into the same runtime, and last anything needing a second runtime.

## 6. What contradicts our docs

| Doc | It says | What is true on 25 Oct 2026 |
|---|---|---|
| docs/04 section 3, leaderboard table "(published, October 2026)" | Canary-Qwen 5.63, Qwen3-ASR 5.76, Parakeet v2 6.05, v3 6.34, Kyutai 6.4, Nemotron EN 6.93, Nemotron 3.5 "not on the leaderboard yet", Whisper large-v3 about 7.4 | Version 21 Oct 2026, new dataset mix: Canary-Qwen 5.94, Qwen3-ASR-1.7B 5.40, Parakeet v2 6.02, v3 6.18, Nemotron EN 6.60 (6.93 is its model-card number), Whisper large-v3 7.16. Nemotron 3.5 **is** on it at 9.75, 52nd of 57. Kyutai is not on the current board |
| docs/04 section 3 | The top two "run only inside a Python stack" | Canary-Qwen and Qwen3-ASR run in transcribe.cpp (GGUF, Windows CUDA prebuilt). Still no word timestamps, so the conclusion holds |
| docs/04 section 3 | Parakeet v3 is "about 0.6 WER points better than the EN model" | Only on the overall average (6.18 against 6.60). On AMI-Cleaned (9.42 against 8.84) and private conversational data (13.44 against 13.28) EN is better |
| docs/04 section 3, last paragraph | "published AMI WER 11.3 for Parakeet v3" | Current AMI-Cleaned split: 9.42 (Parakeet v3), 8.84 (Nemotron EN). Different split; the old number is stale rather than wrong |
| docs/04 sections 1 and 3 (implicitly), research/08 section 7 | The q8_0 GGUFs behave like the model; "dropped nothing" on the 77-minute file | NeMo-Speech.cpp's own doc since 24 Oct: Q8 pointwise quantization "can cause speech to be decoded as long blank runs" and existing GGUFs must be regenerated. Both pinned Nemotron GGUFs have all 48 pointwise tensors in Q8_0 (measured). Our 60 s bucket test would not see a lost sentence |
| docs/KNOWN-ISSUES, runtime row 1 | The #67 fix reaches the shipped runtime "only with the next release" | Still true for tagged releases; a prebuilt nightly with the fix (`caa3f47`) exists since 17 Oct 2026 |
| docs/research/08 section 7 | "Recommendation: one model, Nemotron 3.5, for both live and file mode" | Superseded by docs/04 (EN default). The leaderboard (9.75 against 6.60) and the 3.5 card itself support docs/04 |

## 7. Ranked spike candidates

All measurement uses `tools/eval` (README "Running", "Configs"). Two harness pitfalls apply to every spike:

- **run.py's cache key ignores the engine directory and the GGUF bytes.** A run is reused when `asr`, `diarizer`, `language` and `engine_args` match (`run.py`, `cached()`). A runtime or re-converted-model A/B must use a new config name (or `--force`), never the same name with a different `DIARIST_EVAL_ENGINE_DIR`.
- **A GGUF that is not in the manifest gets no file lookahead.** Without a `<file>.meet.json` of `{"file_right_context": 13}` it runs at lookahead 1 (docs/04 section 5), and a diarizer without `{"file_preset": "v3-offline"}` runs the streaming preset. Put new files under the eval models folder's `user\` subfolder with a distinct file name, plus the sidecar; select them in `configs.toml` by file name (`diarist-cli models select` accepts a file name or path suffix).

A small harness addition helps spikes 1, 2 and 4: a `levers.py gaps --a <config> --b <config>` that lists 30 s windows where one config has under 60 percent of the other's words (the ad-hoc check of research/12 section 5), so rare blank runs show up as a count, not a feeling.

| Rank | Spike | Exact pin and flags | Expected gain and evidence | Cost | How to measure |
|---|---|---|---|---|---|
| **1** | **Re-convert both Nemotron GGUFs with F16 pointwise convolutions, run them on the shipped 0.2.0 runtime** | `convert_model.py` from NeMo-Speech.cpp `main` at `1a7e945` or later; sources `nvidia/nemotron-speech-streaming-en-0.6b --revision ebe59e5a817142986528bbbee5dba8db7b38ed50` and `nvidia/nemotron-3.5-asr-streaming-0.6b --revision 1c8deaecc64b91f034d73e08dd8b64625eb3395d`, `--outtype q8_0` (the converter now keeps the 48 pointwise tensors F16). Control: the same two with `--outtype fp16`. File setting unchanged (lookahead 13, `v3-offline`) via the sidecar | Fewer deletions and no rare multi-second blank runs. Evidence: PR #46 (1 of 17 windows lost a passage with NVIDIA's Q8, 0 of 17 with F16 pointwise; maintainer: "directly lose words"); `docs/model-conversion.md` ("long blank runs"); transcribe.cpp's Q8 with F16 pointwise equals F16 on LibriSpeech (2.31 against 2.29). No meeting WER published; expected average cpWER change under a point, with the value in the outliers (inference) | Low. Two `.nemo` downloads (2.47 and 2.37 GB, owner approves), the converter venv of research/12 updated to `main`'s `requirements.txt`, minutes of CPU. +67.5 MiB per GGUF, about +70 MB VRAM (inference). Shipping needs a hosted file until NVIDIA republishes; check the NVIDIA Open Model License and OpenMDW-1.1 terms for redistribution first | First a load check with `nemo_direct.py <AMI clip> --asr <new gguf>` on 0.2.0. Then configs `en_pwf16`, `n35_pwf16` (and `_fp16`): `uv run python -I run.py --config <c> --sessions all`; `levers.py diff --a shipped_en --b en_pwf16 --sessions all`; deletions in the AMI report; the new `gaps` check on every session; `run.py --config <c> --fleurs en` for EN's quiet-speech deletions (21.5 percent today). Gate: AMI cpWER and every Zoom cpWER equal or better, fewer deletions, speed and VRAM within noise |
| **2** | **Runtime from `main` (≥ `1a7e945`): startup fix, #70, #62; and file jobs without `--endpointing`** | The nightly once it is rebuilt past `1a7e945` (today's asset is `caa3f47` and lacks #46), pinned by the SHA-256 in that day's `SHA256SUMS`; or a local build, `scripts\windows\build.ps1 -Backend cuda -CudaArch 86` (VS 2022 and CUDA 12.8 are on the machine; Ninja and vcpkg are fetched or must be added). Run as a 2 × 2 with spike 1's GGUFs. Variant: drop `--endpointing` from file jobs (`engine_args` cannot remove a flag; needs a diarist-cli switch or `nemo_direct.py`) | Fewer lost words at each stream start (every import, post-pass and live session). Evidence: PR #46 (R 13 first chunk had 9 zero frames and lost 2 frames); a contributor's "halves the number of lost words at the start of the stream" (23 Oct); NVIDIA's own 2.51 against 2.32 gap at 1.12 s. On 45-minute meetings the startup effect is small (inference). Without endpointing: likely identical words, since 0.2.0 keeps decoder state across EOU (source reading), but fewer spurious finals from issue #40; #70 makes it safe on long files. #62 may speed the diarizer front end (1.31 to 1.74 times on whole files, PR #62) | Medium. No tagged release; the nightly tag moves daily, so eval only until a release; the app warns on an untested version | `DIARIST_EVAL_ENGINE_DIR=<main build>` with new config names (`shipped_en_main`, `en_pwf16_main`, …), `--sessions all`; `levers.py diff` against the 0.2.0 runs; words in each session's first 10 s against AMI's manual reference; transcribe step seconds from `run.json`; repeat research/08 section 7's live check (first text, finals, VRAM) through `serve`. Gate as spike 1, plus tag agreement and speaker counts unchanged |
| **3** | **Live lookahead 6 (560 ms) instead of 1** | `serve … --asr.streaming.rnnt_right_context 6` for both models. Valid values: EN {0, 1, 6, 13}, 3.5 {0, 1, 3, 6, 13} (model cards) | Better live text, and better stored text for `post_pass_mode = on_issues` users. Evidence: EN card AMI 14.71 at 160 ms, 11.88 at 560 ms, 11.73 at 1.12 s (average 7.67, 7.07, 6.93); ours, research/12 section 1: Nemotron 3.5 AMI cpWER 23.0 at R 1, 21.6 at R 6, 20.8 at R 13, and larger R was faster, not slower | Low for the measurement; a product change for the live server's arguments. Cost to users: about 0.4 s more delay before live words appear; VRAM per live session unmeasured | Word accuracy: configs `en_rc_6` (new) and the existing `n35_rc_6`, `run.py --sessions ami,aug2,sep5` (the `--stream` file path is the same cache-aware decoder at the same R; inference). Latency and VRAM: research/08 section 7's live test at R 6, two sessions, 60 s from AMI |
| **4** | **Parakeet Unified EN 0.6B as the English file-pass model, if it converts** | `convert_model.py nvidia/parakeet-unified-en-0.6b --outtype q8_0` (one `.nemo`, 2.47 GB; HF revision to pin at download; transcribe.cpp ported `d4ac992`). Offline, no `--stream`; sidecar `{"role": "asr", "streaming": false}`. Needs spike 2's runtime for correct tags on long files (#70) | Card: offline average 5.91 against EN's 6.92 at 1.12 s in the same NVIDIA table (−1.0); AMI 10.14 against 11.73 on the EN card (−1.6, about 14 percent relative). It also writes punctuation and capitals, which neither Nemotron model does. Inference: the leaderboard's AMI-Cleaned gap would be smaller | Medium to high. NeMo-Speech.cpp does not list this model; the converter may reject it or mis-handle its chunked convolutions in offline mode (inference). Parakeet-family drops are a known risk (v3 drops in NeMo itself). VRAM probably like Parakeet TDT on long files (4.6 GB peak on 77 minutes, research/08; inference), so file jobs only | `nemo_direct.py` on the AMI 5-minute clip, `levers.py clip-score`; then a config over `--sessions all`; the 77-minute session through `gaps` against EN (the research/08 section 8 drop test); punctuation share with `levers.py punct` |
| 5 | F32 diarizer GGUF | `convert_model.py nvidia/Nemotron-3-Diarization --revision f667ed73aee57d40cc39428eb768b4fd87a0a29e` with the default (`--outtype f32`); about 400 MB (inference from 100 M parameters); sidecar `{"file_preset": "v3-offline"}` | Unknown. The converter calls F32 "bit-tight parity with NeMo"; NVIDIA shipped Q8_0. No published comparison. Could move near-change WDER or the 7-word speaker merge on Sep 5 (inference) | Low: a 199 MB `.nemo`. About +300 MB VRAM (inference), which matters for the 2 GB live budget, so file jobs first | Config `diar_f32` with `diarizer = "<file>"`, `--sessions all`; DER, WDER, WDER near, speaker counts against `shipped_en`; peak VRAM from `run.json` |
| 6 | Parakeet TDT v3 with Nemotron 3.5 filling its gaps, for European languages | Pinned v3 `541d1f9` and 3.5; a fusion step in diarist-core (not built) | Leaderboard multilingual WER: de 4.12 against 8.96, fr 5.38 against 9.40, it 4.66 against 8.66, nl 7.53 against 14.50. The drops (research/08 section 8; HF discussion #42) are what the fill must catch; transcribe.cpp also reports v3 drifting into English on accented speech under full attention | High: fusion logic and its own eval; double the compute per file job | FLEURS slices (`run.py --fleurs all` for both models), then multilingual recordings when the owner has some; the `gaps` check for drops |

Not worth a spike now, with the reason:

- A second runtime for Qwen3-ASR, Cohere Transcribe, Granite Speech 5.0 or Canary-Qwen: at most about one point better on conversational data (leaderboard, section 3), no word timestamps in transcribe.cpp for any of them, and no Nemotron 3 Diarization there yet (PR #175 open).
- Word boosting and the runtime PnC hook: nothing on `main` changes them (section 1); research/12 sections 3 and 4 stand.
- Moonshine Streaming for live: better AMI at low latency than EN at 160 ms on the old split (10.68 against 14.71), but MIT-licensed weights in a different runtime with no word timestamps; spike 3 gets most of that gain inside our stack.

Watch list (check before each release): a NeMo-Speech.cpp tag after v0.2.0; new GGUF commits on the two Nemotron repos (NVIDIA republishing with F16 pointwise would make spike 1 a manifest bump); PR #73 (live WebSocket drops); issue #56 (multitalker Parakeet); transcribe.cpp PR #175; BUT Pandemonium's release.

## Sources

NVIDIA/NeMo-Speech.cpp: releases, `compare/v0.2.0...main`, PRs #41, #46, #47, #62, #70, #73, #78, issues #40, #48, #49, #55, #56, #71, #72, Actions runs of the Release workflow, `README.md`, `BENCHMARK.md`, `docs/model-conversion.md`, `docs/asr/models.md`, `docs/asr/configuration.md`, `docs/build.md`, `conversion/asr.py`, `conversion/diarization.py`, `src/asr/runner.cpp` (v0.2.0 and `main`), `src/asr/recognizer.cpp`, `src/asr/decoders/rnnt_greedy_decoder.cpp`, `models/index.json` (all read 25 Oct 2026). Hugging Face: commit histories, file trees and discussions of the four pinned repos and of nvidia/parakeet-unified-en-0.6b; model cards of nemotron-speech-streaming-en-0.6b, nemotron-3.5-asr-streaming-0.6b, parakeet-unified-en-0.6b, ibm-granite/granite-speech-5.0-2b, ibm-granite/granite-speech-5.0-470m-turboctc, CohereLabs/cohere-transcribe-03-2026, Qwen/Qwen3-ASR-1.7B, Edge0/ARK-ASR-0.6B, moonshine-ai/moonshine-streaming-medium, microsoft/VibeVoice-ASR-Streaming-1.5B, BUT-FIT/Pandemonium; the Open ASR Leaderboard space (version 21 Oct 2026) and huggingface/open_asr_leaderboard `transformers/run_eval.py` (all read 25 Oct 2026). NVIDIA-NeMo/Speech README and releases (25 Oct 2026). handy-computer/transcribe.cpp README, v0.4.1 release, `docs/models/` pages, issues #71 and #170, PR #175 (25 Oct 2026). VoiceArena Diarization-Bench (25 Oct 2026). pyannote/pyannote-audio releases (25 Oct 2026).
