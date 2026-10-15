# Research: diarization state of the art, fusion, identity, eval (verified 15 Oct 2026)

Opus 5.5 research agent. CONFIRMED = read in a primary source. UNCONFIRMED = inference or secondary.

## 1. Nemotron 3 Diarization is still the best open option
- CONFIRMED. VoiceArena Diarization-Bench v1 (139 English conversations, collar 0, overlap scored), DER: Nemotron 3 **14.72** (JER 25.5, correct count 83.5%); DiariZen s80-MD-v2 19.34; pyannoteAI Precision-3 20.56 (commercial, 17 Sep 2026); Precision-2 23.41; Sortformer 4spk v2 24.61; pyannote community-1 30.61. voicearena.com/diarization-bench, pyannote.ai/blog/precision-3.
- CONFIRMED: licence `openmdw-1.1`, not gated, q8_0 GGUF in repo. Pull `nvidia/Nemotron-3-Diarization`, not a "-preview" repo (preview evaluation-only claim UNCONFIRMED).
- CONFIRMED no new open challengers: pyannote-audio 4.0.7 (30 Jun 2026) latest; Precision-3 closed. DiariZen last model 31 Jan 2026, CC BY-NC. FASTDIAR (Microsoft/KTH, arXiv 2610.02941, 2 Oct 2026) worse than Sortformer v2.1 on DIHARD3 (about 32.6 vs 20.4). MOSS-Transcribe-Diarize 0.9B (Jul 2026, Apache-2.0) offline, Chinese cpCER only.
- Caveat: Nemotron's AMI/AliMeeting/NOTSOFAR numbers use forced-alignment references; not comparable with DiariZen AMI 13.9. VoxConverse is in Nemotron training data.

## 2. Nemotron 3 details (card + NeMo-Speech.cpp `src/asr/diar/sortformer_model.h`)
- Architecture CONFIRMED, not FastConformer: 10 ms mel stacked 8x to 80 ms, 31-layer pre-LN RoPE Transformer initialised from NEST SSL, 8x subpixel Conv1D upsample to 10 ms sigmoid outputs [T, 8]. About 100M params. Channels ordered by first appearance. (V2 was 17-layer FastConformer + 18-layer Transformer.)
- Input 16 kHz mono. Speaker count: 8 fixed sigmoid slots, no counting head. Count accuracy drops at low latency (NOTSOFAR-SC 78% to 55%; DIHARD 5 to 9 speakers DER about 27 to 29%). More than 8 speakers: missed or merged.
- Latency knobs in 80 ms frames (cache / FIFO / chunk / right context / update period): offline 264/40/340/40/300 (30.4 s); low latency 264/264/9/4/222 (1.04 s); ultra-low 264/264/3/1/222 (0.32 s). NeMo-Speech.cpp exposes `--diar-lc` and presets `v3-offline` / `v3-streaming`.
- Weaknesses (card): overlap, noise, reverberation, far-field, very long recordings, more than 8 speakers, misplaced boundaries. No permutation step needed (arrival-ordered cache). Live labels provisional until chunk + right context commits; V3 has no birth gate so labels commit after right context.
- Post-processing defaults in NeMo-Speech.cpp: onset 0.641, offset 0.561, pad 0.229/0.079 s, min on/off 0.511/0.296 s. Source says these are V2 CALLHOME-tuned and should be retuned.
- ASR combination: card links ASR_INTEGRATION_GUIDE.md using NeMo `SpeakerTaggedASR` (`multispk_transcribe_utils.py`) with multitalker-parakeet or Nemotron 3.5 ASR (`masked_asr=true`). Offline cascade: `nemo-speech transcribe x.wav --diarize --json` pairs Parakeet TDT v3 with Nemotron 3.

## 3. Word-to-speaker assignment
- NeMo Python CONFIRMED: window from one 80 ms frame before word start to word end; mean sigmoid per speaker, clamp at 0.01, argmax; `fix_prev_words_count=5` re-renders recent words' text only.
- NeMo-Speech.cpp CONFIRMED (`diar_pipeline.cpp/.h`): mean probability over [start, min(end, start + 160 ms)], argmax. Start-anchored on purpose: "late punctuation emissions can extend transducer word spans into the next speaker's turn". Main cause of edge-word errors with TDT.
- NVIDIA blog: word midpoint; "ambiguous" when two speakers overlap. WhisperX: overlap duration per speaker, largest wins; `fill_nearest` for no-overlap words.
- Vibe #633 was pyannote segment reordering duplicating end-of-file audio; closed 10 Jul 2026 after move to Sortformer.
- Literature: NVIDIA study (arXiv 2609.10265) blames cascade errors on overlap and inaccurate word timestamps. DiarizationLM Gemma-4-E4B-v1 (Apache-2.0, 5.3 GB GGUF) helps 2-speaker phone audio a lot (Fisher WDER 5.32 to 2.99) but little on meetings (AMI 15.68 to 14.89). Also 2409.00151 (NAR LM corrector), 2309.05248 (LLM contextual beam search).
- **Recommended algorithm (agent design, UNCONFIRMED as published):**
  1. On 10 ms probabilities, per word average per-speaker probability over [t0 - 40 ms, min(t1, t0 + 160 ms)]; keep the whole score vector.
  2. Margin = top minus runner-up. Flag overlap when runner-up > 0.5. If top < 0.3 use nearest active speaker.
  3. Viterbi over the word sequence: states = speakers, emissions = log p, switch cost lambda, cut to about lambda/4 after sentence-final punctuation or a gap > 250 ms; sub-word continuations cannot switch. Removes 1 to 2 word islands, snaps turns to pauses. Tune lambda on labelled meetings.
  4. Live: re-score last about 5 words plus open sentence as frames commit. Saved transcript: re-run offline (30.4 s config) over the whole file.

## 4. Speaker embeddings (EER on VoxCeleb1-O cleaned)
| Model | Params | EER | Licence | ONNX in sherpa-onnx |
|---|---|---|---|---|
| TitaNet-L | 23M | 0.66% | CC-BY-4.0 | yes |
| TitaNet-S | ~6.4M | 1.08% (partly confirmed) | NeMo/NGC | yes |
| WeSpeaker ResNet34-LM | 6.6M | 0.797 (0.723 AS-norm) | CC-BY-4.0 | yes |
| WeSpeaker ResNet293-LM | 28.6M | 0.532 (0.447 AS-norm) | CC-BY-4.0 | yes |
| WeSpeaker / 3D-Speaker CAM++ | 7.2M | 0.65 to 0.71 | CC-BY-4.0 / Apache-2.0 | yes |
| 3D-Speaker ERes2NetV2 | 17.8M | 0.61% | Apache-2.0 | zh-cn only |
| SpeechBrain ECAPA | - | 0.80% | Apache-2.0 | no |

Pick TitaNet-L (trained on VoxCeleb + Fisher + Switchboard + SRE, closer to conversational audio); A/B against ResNet293-LM. Run via sherpa-onnx `SpeakerEmbeddingExtractor` on CPU (v1.13.8, official C API, rust-api-examples; handles fbank and normalisation). `ort` directly (2.0.0-rc.14) would mean re-implementing front-ends. Voiceprints: crops of 2 to 6 s where the channel's probability > 0.8 and all others < 0.2; average L2-normalised embeddings; cosine match with threshold calibrated on own meetings; user confirms before updating a stored voiceprint.

## 5. Joint model or cascade
NVIDIA study (arXiv 2609.10265, Interspeech 2026), average cpWER with real diarization: cascade (Sortformer v2.1 + Nemotron streaming, argmax at word timestamp) 42.27 (AMI-IHM 43.54); masked input 34.77; WL-SOT 33.46; SSA-v1 23.36 (22.10); **SSA-v2 conditioned on Nemotron 3 15.91 (13.38)**; DiCoW v3.3 (BUT, Whisper-large-v3 about 2B, CC-BY-4.0, offline) 14.57. Gap is mostly overlap deletions plus boundary errors. That cascade was 1.04 s streaming, not offline Parakeet + offline Nemotron 3. multitalker-parakeet-0.6b-v1: nvidia-open-model-license, English only, one ASR instance per speaker, conditioned on Sortformer v2.1 (4 spk), AMI-IHM cpWER 21.26, SDM 37.44; not in NeMo-Speech.cpp (no multitalker code as of 13 Oct 2026). SSA-v2 unreleased. **Verdict: ship the cascade now; Nemotron-3-conditioned SSA is the research upgrade when released or ported. DiCoW is the offline ceiling but needs a heavy port away from Python.**

## 6. Measuring on own meetings
- Metrics: DER (collar 0.25 and 0, overlap included), JER (catches quiet speakers swallowed), speaker-count error, WDER (words to wrong speaker), cpWER and tcpWER, cpWER minus DI-cpWER (attribution error estimate).
- Tools CONFIRMED: `pyannote.metrics` DiarizationErrorRate and JaccardErrorRate; `meeteval` (MIT): cpwer, tcpwer, orcwer, greedy_dicpwer, `meeteval-der md_eval_22` (Windows UNCONFIRMED; WSL fallback); `diarizationlm` package for WDER; NeMo `e2e_diarize_speech.py` for diarizer-only DER and count accuracy.
- Harness: app exports per-word JSON (start, end, speaker, margin) plus RTTM and raw probability arrays; references made by correcting the app's own output into RTTM plus SegLST; Python script converts formats and runs all metrics per meeting and config; report WDER separately for words within +/- 1 s of a reference speaker change; tune lambda, onset/offset, window on 2 meetings, report on the other 2 to 3.

## Verdict
- Diarizer: Nemotron 3 Diarization (OpenMDW, q8_0 via NeMo-Speech.cpp). Offline 30.4 s config for final transcripts; 0.64 to 1.04 s for live.
- Fusion: start-anchored per-word mean of 10 ms probabilities, margin and overlap flags, Viterbi smoothing with cheaper switches at pauses and punctuation, offline re-pass.
- Embedding: TitaNet-L ONNX via sherpa-onnx CPU; A/B ResNet293-LM.
- Eval: meeteval + pyannote.metrics JER + diarizationlm WDER on 3 to 5 hand-corrected meetings.
- Biggest risk: overlap and boundary words. Cascades lose about 2x cpWER to diarization-conditioned ASR; more than 8 speakers or far-field degrade sharply. Plan an SSA/DiCoW-style upgrade path.
