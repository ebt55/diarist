# Accuracy levers in nemo-speech 0.2.0: measurements (16 Oct 2026)

Chunk A of `docs/plans/12-release-beta2.md`: the four runtime levers Diarist does not use yet (decoder lookahead, diarizer geometry, punctuation model, word boosting) and a per-language smoke test, measured on the fixed eval set (`docs/05-eval-data.md`). No product code changed. Opus 5.5 implementation agent.

## 0. Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88; idle desktop about 1,230 MiB on the GPU |
| diarist-cli | `tools/eval/.bin/diarist-cli.exe` of the main checkout, sha256 `e733adce07afd4ce…` (has `--engine-arg`) |
| Runtime | nemo-speech 0.2.0, `nemo-speech-0.2.0-windows-x86_64-cuda` |
| ASR | Nemotron 3.5 `nvidia/nemotron-3.5-asr-streaming-0.6b` rev `1c8deae` (pinned); Nemotron EN `nvidia/nemotron-speech-streaming-en-0.6b` rev `ebe59e5` |
| Diarizer | Nemotron 3 Diarization rev `f667ed7`, runtime preset `v3-streaming` unless a row says otherwise |
| Harness | `tools/eval/run.py` (per-config `engine_args`, passed as `diarist-cli transcribe --engine-arg=<flag>`), `levers.py` (offsets, tag agreement, diffs, summary table), `nemo_direct.py` (direct runtime runs for the spikes), `fleurs_fetch.py` |
| Sets | Fast subset: AMI ES2002a (21 min), Aug 2 (44 min), Sep 5 (45 min). Long file: Aug 14 (77 min; the same recording as the CIMC file of research/08 section 7, identical duration 4621.717 s). Full set: the seven sessions of docs/05 |
| VRAM | Peak of total `memory.used` (nvidia-smi every 200 ms) during the transcribe step minus the reading just before the run (research/08 pitfall 10). run.py waits until the GPU is at the idle-desktop level first. Other processes on the desktop move the baseline by up to 350 MiB between runs (1,196 to 1,580 MiB were seen), so differences under about 200 MiB are noise |
| Speed | "Transcribe s" is the `nemo-speech transcribe` step alone (from engine.log creation to the runtime's words JSON). It excludes the standalone `--rttm` diarize pass, which engine args do not reach and which is the same in every config |
| Metrics | As in every docs/eval report: cpWER on AMI (word-level manual reference) is the word-accuracy number; Zoom cpWER is against Zoom's own machine transcript, so it mixes in Zoom's errors; WDER and "near" (words within 1 s of a reference speaker change) judge attribution |

Base command for every eval row (from run.py): `diarist-cli --data-dir .work/data/<config> --models-dir <models> --engine-dir <engine> transcribe <16 kHz wav> --language en --title <session> --rttm --out <run>/transcript.json --format json [--engine-arg=<flag> ...]`, which diarist-core turns into `nemo-speech transcribe <wav> --model <asr.gguf> [--language en] --stream --endpointing --diarize --diar-model <Nemotron-3-Diarization.q8_0.gguf> --device cuda:0 --json --output <out> --force <engine args>`. Each table names its config; configs are in `tools/eval/configs.toml` and run with `uv run python -I run.py --config <config> --sessions ami,aug2,sep5` (fast subset), `--sessions aug14`, or `--sessions all`.

## 1. Decoder lookahead (`--asr.streaming.rnnt_right_context R`)

### What R means in this runtime

The cache-aware encoder sees R frames of 80 ms ahead of each chunk, so the chunk is (R + 1) × 80 ms: R = 1 (the runtime default) is the model card's 160 ms column, 3 is 320 ms, 6 is 560 ms and 13 is 1.12 s.

- `-1` means the GGUF's trained maximum, read from its header (`asr.encoder.train_right_ctx`). That is **13 for Nemotron EN but 3 for the pinned Nemotron 3.5 GGUF** (the re-converted revision `ea30d66` says 3 as well). Checked: `-1` gives word-for-word, time-for-time identical output to `13` on EN and to `3` on 3.5, on all three fast-subset sessions (`levers.py diff --a en_rc_max --b en_rc_13`, `--a n35_rc_max --b n35_rc_3`: 0 edit operations). So the explicit values are what matter, and the configs below use them.
- The runtime does not clamp R: 3.5 accepts 13 (and even 20) with exit 0. R = 13 on 3.5 is a real, different decode, not a silent fallback to 3: against R = 3 it changes 226 of 2,334 words on AMI, 396 of 6,595 on Aug 2 and 387 of 8,387 on Sep 5 (`levers.py diff --a n35_rc_3 --b n35_rc_13`), and it is the model card's own 1.12 s setting for 3.5. Its numbers are reported below on that basis.

### Quality and speed, fast subset

Configs `nemotron35` (R = 1), `n35_rc_3`, `n35_rc_6`, `n35_rc_13`, `nemotron_en` (R = 1), `en_rc_13`; command `uv run python -I run.py --config <config> --sessions ami,aug2,sep5`, table from `uv run python -I levers.py table --configs nemotron35,n35_rc_3,n35_rc_6,n35_rc_13,nemotron_en,en_rc_13`. Percentages.

| Config | R (chunk) | AMI cpWER | AMI WDER near | Aug 2 cpWER | Aug 2 WDER | Aug 2 near | Sep 5 cpWER | Sep 5 WDER | Sep 5 near | Transcribe s (110 min) | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|
| nemotron35 | 1 (160 ms) | 23.0 | 8.9 | 12.0 | 0.6 | 9.1 | 5.4 | 0.2 | 7.8 | 228 | 29 |
| n35_rc_3 | 3 (320 ms) | 21.9 | 7.4 | 11.2 | 0.6 | 7.8 | 5.0 | 0.2 | 7.5 | 157 | 42 |
| n35_rc_6 | 6 (560 ms) | 21.6 | 6.8 | 11.1 | 0.6 | 9.5 | 5.2 | 0.1 | 7.0 | 132 | 50 |
| n35_rc_13 | 13 (1.12 s) | **20.8** | **6.6** | **10.9** | 0.5 | 8.2 | 5.1 | 0.1 | 7.1 | **123** | **53** |
| nemotron_en | 1 (160 ms) | 19.8 | 9.3 | 10.3 | 0.6 | 8.8 | 5.1 | 0.1 | 6.9 | 222 | 30 |
| en_rc_13 | 13 (1.12 s) | **18.3** | **7.3** | 10.2 | 0.7 | 8.2 | **4.7** | 0.2 | 7.3 | **120** | **55** |

- AMI cpWER, the one word-accuracy number with a human reference: 3.5 improves 23.0 to 20.8 (−2.2 points, −10 % relative) monotonically with R; EN improves 19.8 to 18.3 (−1.5). That matches the model card's direction (FLEURS English 8.88 at 160 ms to 7.91 at 1.12 s for 3.5).
- The gain is mostly fewer substitutions and deletions (AMI cp errors for 3.5: 140 substitutions and 434 deletions at R = 1, 117 and 402 at R = 13).
- Speaker attribution does not get worse: WDER is unchanged (0.1 to 0.7 % on the Zoom sessions), and WDER near speaker changes falls on AMI (8.9 to 6.6 % for 3.5, 9.3 to 7.3 % for EN), the session with the densest turn-taking. On the Zoom sessions "near" moves by ±1.3 points in both directions with no trend (Aug 2: 9.1, 7.8, 9.5, 8.2 across R).
- **Larger R is faster, not slower**: fewer, larger chunks. The transcribe step takes 54 % of the R = 1 time for both models at R = 13 (×53 to ×55 real time instead of ×29 to ×30). So nothing argues for a middle value; R = 6 sits between 3 and 13 on every column.

### 77-minute file (Aug 14): VRAM, tag agreement, speed

Command `uv run python -I run.py --config <config> --sessions aug14`; tag agreement from `uv run python -I levers.py tags --configs <...> --session aug14` (each runtime word tag against the standalone `diarize` RTTM of the same run at the word's span, labels matched one to one; research/08 section 7 reported 99.9 %).

| Config | Transcribe s | × real time | Peak VRAM Δ, MiB | Tag agreement with `diarize` | Words | Speakers (tags / diarizer) | Transcript WDER near (stress session) |
|---|---|---|---|---|---|---|---|
| nemotron35 | 154.5 | 30 | 1,894 | 99.9 % | 11,928 | 8 / 8 | 40.4 |
| n35_rc_3 | 121.6 | 38 | 1,752 | 99.9 % | 11,910 | 8 / 8 | 39.5 |
| n35_rc_6 | 102.7 | 45 | 1,659 | 99.9 % | 11,908 | 8 / 8 | 40.5 |
| n35_rc_13 | **89.5** | **52** | 1,747 | 99.9 % | 11,887 | 8 / 8 | 41.2 |
| nemotron_en | 148.0 | 31 | 2,056 | 99.8 % | 12,266 | 8 / 8 | 43.8 |
| en_rc_13 | **81.7** | **57** | 1,881 | 99.7 % | 12,193 | 8 / 8 | 42.2 |

- VRAM: no cost. The peak delta of the transcribe step stays within 1.66 to 1.89 GB for 3.5 and 1.88 to 2.06 GB for EN, and the differences are inside the baseline noise above. The research/08 figures (1.82 GB for 3.5, 2.58 GB for EN on this file) hold as an upper bound; the lower EN number here is the same file on a quieter desktop.
- Tag agreement stays at 99.7 to 99.9 %: more lookahead does not break the fused pass's word tags on a long file.
- The Aug 14 transcript WDER is the stress session (room devices under one Zoom label; docs/05) and moves within ±2 points.

### Word times

`uv run python -I levers.py offsets --base nemotron35 --configs n35_rc_3,n35_rc_6,n35_rc_13 --sessions ami,aug2,sep5,aug14` (and `--base nemotron_en --configs en_rc_13`): normalised words of each config aligned with the same model's R = 1 run, start-time difference on the equal pairs (negative = earlier than R = 1). Against AMI's manual word times: the same alignment against the reference.

| Config | Δ start vs R = 1, median (mean), AMI / Aug 2 / Sep 5 / Aug 14, ms | Lateness against AMI manual times, median (mean), ms |
|---|---|---|
| nemotron35 (R = 1) | 0 | 350 (567) |
| n35_rc_3 | 0 (−28) / 0 (−33) / 0 (−37) / 0 (−34) | 320 (509) |
| n35_rc_6 | −80 (−50) / −80 (−72) / −80 (−75) / −80 (−72) | 280 (277) |
| n35_rc_13 | −80 (−111) / −80 (−124) / −80 (−125) / −80 (−124) | 240 (234) |
| nemotron_en (R = 1) | 0 | 290 (563) |
| en_rc_13 | −160 (−147) / −160 (−178) / −160 (−188) / −160 (−186) | 160 (223) |

- Words start earlier with more lookahead: one 80 ms frame for 3.5 at R ≥ 6, two frames for EN at R = 13. The EN shift is above the 150 ms flag on Aug 2, Sep 5 and Aug 14 (mean −178 to −188 ms).
- The shift is toward the truth: against AMI's manual times the known lateness (research/08 pitfall 11, about 0.3 s) falls from 350 to 240 ms (3.5) and from 290 to 160 ms (EN). Nothing becomes early.
- Consequence for live-to-post-pass carry-over (time-based matching of live words to post-pass words): the post-pass words of a file job with R = 13 sit 80 ms (3.5) to 160 ms (EN) earlier than the live words, which run at R = 1. A matching tolerance of at least 250 ms, or matching on text plus nearest time, absorbs it; an exact-frame match would not.

## 2. Diarizer geometry in the fused pass (`--asr.diar.preset v3-offline`)

Per the plan review, the `--diar-rc` 2× and 4× sweep (80 to 320 ms) was replaced by the diarizer's offline-geometry streaming preset inside the fused `transcribe` pass. The runtime's presets (NeMo-Speech.cpp v0.2.0 `src/asr/diar/aosc_state.h`, in 80 ms frames: speaker cache, FIFO, chunk, cache update period, left and right context):

| Preset | Cache | FIFO | Chunk | Update period | Left ctx | Right ctx |
|---|---|---|---|---|---|---|
| `v3-streaming` (default) | 264 | 80 | 13 (1.04 s) | 40 | 0 | 1 |
| `v3-offline` | 264 | 0 | 264 (21.1 s) | 188 | 1 | 1 |

This is still the cache-based streaming algorithm (not the stateless `diarize --offline` pass that scored DER 43 % in research/03 section D), with 21-second chunks. It is not the model card's 30.4 s geometry quoted in research/04 (cache 264, FIFO 40, chunk 340, right context 40, update 300): the runtime ships its own values. Engine args reach only the fused pass; the standalone `--rttm` diarizer pass (the "Standalone diarizer" tables of the reports) stays on `v3-streaming`.

Config `n35_rc_13_diar_off` (`--asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline`) against `n35_rc_13`; command `uv run python -I run.py --config n35_rc_13_diar_off --sessions ami,aug2,sep5,aug14`. Percentages; speakers are reference / transcript.

| Session | Config | Speakers | DER c0.25 | DER c0 | JER | WDER | WDER near | cpWER | Transcribe s | Peak VRAM Δ, MiB |
|---|---|---|---|---|---|---|---|---|---|---|
| AMI | n35_rc_13 | 4 / 4 | 24.8 | 34.3 | 38.9 | 2.3 | 6.6 | 20.8 | 23.5 | 1,625 |
| AMI | + v3-offline | 4 / 4 | 24.7 | 33.9 | 38.5 | 2.2 | 6.2 | 20.7 | 15.7 | 1,437 |
| Aug 2 | n35_rc_13 | 6 / 6 | 7.6 | 12.1 | 12.3 | 0.5 | 8.2 | 10.9 | 46.0 | 1,625 |
| Aug 2 | + v3-offline | 6 / 6 | 7.5 | 12.0 | 11.5 | **0.2** | **5.5** | 10.3 | 36.7 | 1,488 |
| Sep 5 | n35_rc_13 | 5 / 5 | 3.6 | 6.2 | 29.2 | 0.1 | 7.1 | 5.1 | 53.7 | 1,645 |
| Sep 5 | + v3-offline | 5 / **4** | 3.6 | 6.2 | 29.3 | 0.2 | 7.8 | 5.1 | 34.0 | 1,416 |
| Aug 14 (stress, 77 min) | n35_rc_13 | 6 / 8 | 26.1 | 30.0 | 70.1 | 20.7 | 41.2 | 43.5 | 89.5 | 1,747 |
| Aug 14 (stress, 77 min) | + v3-offline | 6 / 8 | 25.0 | 28.8 | **58.5** | 19.3 | **18.0** | 42.9 | 62.5 | 1,637 |

- The words are identical (the preset changes only the speaker tags; Aug 14 has 11,887 words in both), so every change above is attribution.
- WDER near speaker changes, the headline weakness, falls from 8.2 to 5.5 % on Aug 2 and from 41.2 to 18.0 % on the in-room stress session; AMI moves 6.6 to 6.2 %. Sep 5 goes the other way, 7.1 to 7.8 %.
- Sep 5 loses one speaker: the reference's smallest speaker has 7 words; streaming tagged 2 words to it, the offline geometry merges them into another label. Speaker count is the docs/00 bar ("within one" on user meetings); this is a regression on that session, though a 7-word one.
- The transcribe step is 20 to 37 % faster again and peak VRAM is the same or lower.
- Tag agreement with the (streaming) standalone diarizer on Aug 14 drops from 99.9 to 99.1 %, as expected when the two passes no longer use the same geometry.

FULLSET_DIAR

## 3. Punctuation and casing (`--pnc-model`)

### The runtime never runs a PnC model on our two ASR models

`nemo-speech` 0.2.0 decides once, when the recognizer is built, whether the ASR model punctuates by itself, and if it does it drops `--pnc-model` with a warning before even opening the file (NeMo-Speech.cpp v0.2.0, `src/asr/recognizer.cpp` `vocab_self_punctuates()` and `src/asr/postproc/pipeline.cpp` `Postprocessor::Postprocessor`):

- The test is "the ASR vocabulary contains a sentence terminator token": `.`, `?`, `!`, `।` or `॥`, with or without the SentencePiece word marker.
- The warning text is `[pnc] WARNING: model emits its own punctuation/capitalization; ignoring --pnc-model (the PnC BERT targets plain-text CTC models; stacking it here would double/garble punctuation).`
- Both pinned GGUFs pass the test, so both are "self-punctuating": Nemotron 3.5's vocabulary (13,087 pieces) has `.`, `?`, `▁.`, `।` and `!`; Nemotron EN's (1,024 pieces) has `.`, `?` and `!` (read from `asr.tokenizer.vocab` in the GGUF headers).

That is what the customization doc's sentence "preserves punctuation from self-punctuating models or runs a loaded PnC model for plain-text models" means in code: for Diarist's models it is always the first branch. Empirical check (PnC GGUF not needed, because the gate runs before the file is opened): `uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr <model> [--language en] --out .work/spikes/pnc/<n>_gate -- --pnc-model .work/pnc/gate-probe-not-a-model.gguf`, where the probe is a 41-byte text file with a `.gguf` name. Result on both models: exit 0, the warning above on stderr, and `words.json` identical to the run without `--pnc-model` (every word, time and speaker tag). A file that is not a model at all loads "fine", because it is never opened.

| AMI 5-minute clip (50 to 350 s) | Words | End in punctuation | Capitalised | Wall s | Peak VRAM Δ, MiB |
|---|---|---|---|---|---|
| Nemotron 3.5, no `--pnc-model` | 479 | 4 (0.8 %) | 29 (6.1 %) | 11.9 | 1,639 |
| Nemotron 3.5, `--pnc-model <probe>` | 479 | 4 (0.8 %) | 29 (6.1 %) | 12.7 | 1,599 |
| Nemotron EN, no `--pnc-model` | 531 | 1 (0.2 %) | 31 (5.8 %) | 12.1 | 1,550 |
| Nemotron EN, `--pnc-model <probe>` | 531 | 1 (0.2 %) | 31 (5.8 %) | 12.2 | 1,728 |

Excerpt, AMI 125 s, 40 words, Nemotron 3.5 before and after (identical): "to the design I'm not really sure what what you guys have already received in your emails what did you get um I just got the project announcement designing remote control yeah that's that where everybody got okay so we're". The model's own punctuation on this clip is four commas and periods in 479 words, so "self-punctuating" is a statement about the vocabulary, not about the output.

So the four questions of the plan have one answer: with nemo-speech 0.2.0 and either Diarist ASR model, `--pnc-model` changes nothing (no words, no casing, no cpWER change, no time or VRAM cost), and it cannot double-punctuate because it never runs. Plan F's "runtime hook as plumbing" would ship a code path that is dead for every model in the manifest.

### Converting the Apache-2.0 checkpoint (1-800-BAD-CODE)

The English repository `1-800-BAD-CODE/punctuation_fullstop_truecase_english` has no `.nemo` (only `punct_cap_seg_en.onnx`, `spe_32k_lc_en.model`, `config.yaml`). The 47-language repository `1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase` (licence `apache-2.0`, not gated, commit `d1769a5`) has `pcs47_1jun.nemo`: 1,119,815,680 bytes, SHA-256 `d94d08b868a35d01b0fa089775e593098b0cf233c25646b0e803a2b823248d00`, kept in `tools/eval/.work/pnc/src/`. Its archive holds only a SentencePiece model (`…_sp_xlmroberta.model`), `model_config.yaml` (XLM-RoBERTa base encoder, decoder `nemo.collections.nlp.modules.token_classification.ConditionedPCSDecoder` with pre-punctuation, post-punctuation, capitalisation and segmentation heads) and `model_weights.ckpt`.

The converter is NeMo-Speech.cpp's own `convert_model.py` at tag `v0.2.0` (commit `6a3ca36`, shallow clone in `tools/eval/.work/nemo-speech-src`), in a separate venv with its `requirements.txt` (`tools/eval/.work/pnc-venv`, Python 3.12, torch 2.11 CPU, transformers 5.5.1). It rejects the checkpoint both ways:

| Command (from the venv) | Exit | Error |
|---|---|---|
| `python convert_model.py pcs47_1jun.nemo --outfile pcs47_1jun.q8_0.gguf --outtype q8_0` | 1 | `convert_model.py: error: cannot determine checkpoint architecture; pass --architecture asr\|diarization\|pnc\|vad\|tts\|codec\|nmt\|s2s` |
| `python convert_model.py pcs47_1jun.nemo --architecture pnc --outfile pcs47_1jun.q8_0.gguf --outtype q8_0` | 1 | `convert_model.py: error: missing expected PnC artifacts in .nemo` |

The PnC converter (`conversion/pnc.py`) only knows NeMo's `PunctuationCapitalizationModel`: a WordPiece BERT (`vocab.txt`, `encoder_config.json`, `punct_label_ids.csv`, `capit_label_ids.csv`) with two one-layer heads renamed from `bert_model.*`, `punct_classifier.mlp.layer0.*` and `capit_classifier.mlp.layer0.*`. The 1-800-BAD-CODE model has none of those files, a different tokenizer and four multi-layer heads, and the runtime's PnC graph (`src/asr/pnc/`) has no XLM-RoBERTa or pre-punctuation support either, so this is not a converter flag away. As planned, that is where the conversion stops.

### The NGC checkpoints

Not downloaded. The NGC catalog entry for `nvidia/nemo/punctuation_en_bert` (version 1.0.0rc1, one file `punctuation_en_bert.nemo`, 405,906,986 bytes, "canGuestDownload": true) carries this licence text, verbatim from its description:

> License to use this model is covered by the NGC [TERMS OF USE](https://ngc.nvidia.com/legal/terms) unless another License/Terms Of Use/EULA is clearly specified. By downloading the public and release version of the model, you accept the terms and conditions of the NGC [TERMS OF USE](https://ngc.nvidia.com/legal/terms).

Downloading it accepts the NGC Terms of Use on the owner's behalf, so it is left to the owner. Given the gate above it would not show anything new in Diarist's pipeline either: a converted BERT would be ignored on both models exactly like the probe. If the owner still wants the functional check, the command is in section 7.

## 4. Word boosting (`--speech-context`, `--speech-context-boost`)

### Phrases

From the AMI baselines (`levers.py ami-misses` and a substitution list over `engine.words.json` against the manual reference), the 5-minute clip (AMI 50 to 350 s, `ffmpeg -ss 50 -t 300`) has three terms both models get wrong: the name **Craig** (heard as "greg" at 87 s), **beagle** (dropped at 204 s) and the title **Planet of the Apes** ("plant at the amps" at 271 s). The other names in the clip (David, Andrew, Laura) are already right.

### The pinned GGUFs cannot boost

`nemo-speech` loads the boosting tokenizer from `asr.tokenizer.spm_model` (a base64 SentencePiece model) in the GGUF (`src/asr/model.cpp`). Both pinned files carry only `asr.tokenizer.type` and `asr.tokenizer.vocab`, so every phrase fails to tokenize and the runtime says so once per run:

`[asr] RNNT word boosting requested but no phrases could be tokenized (no embedded SentencePiece tokenizer in the GGUF?); boosting disabled.`

With either pinned model at boost 2, 3 or 5 the output is identical to the unboosted run (0 edit operations; table below). The EN repository has no newer revision (its last commit is `ebe59e5`, 5 Aug 2026), so only 3.5 can boost at all.

### The re-converted 3.5 GGUF boosts, and over-boosts

Revision `ea30d66debe3740a08b573244286791d423d6b3e` of `nvidia/nemotron-3.5-asr-streaming-0.6b` ("Re-convert GGUF with embedded SentencePiece tokenizer", 10 Sep 2026): `nemotron-3.5-asr-streaming-0.6b.q8_0.gguf`, **742,090,464 bytes, SHA-256 `3fc991d3badad7277c11030a7519832cddaf2057aafed6d4b25147e953a070b1`** (matches the Hugging Face LFS id), in `tools/eval/.work/models-new/nvidia/nemotron-3.5-asr-streaming-0.6b/<revision>/`. Its header adds `asr.tokenizer.spm_model`; `train_right_ctx` is still 3. It is not bit-identical in behaviour to the pinned file: without boosting it differs from the pinned file by 10 edit operations in 479 words on the clip (among them it drops "well this is the kickoff" at the start), at the same clip WER, so a manifest bump needs its own eval run.

Command per row: `uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr <model> [--language en] --out .work/spikes/boost/<run> -- --speech-context Craig --speech-context beagle --speech-context "Planet of the Apes" --speech-context-boost <N>`; table from `levers.py boost`. Hits count normalised occurrences in the clip (reference: Craig 2, beagle 3, Planet of the Apes 1). Clip WER is speaker-agnostic, against the manual reference words of the clip.

| Model | Boost | Craig | beagle | Planet of the Apes | Words changed vs unboosted | Clip WER % | Runtime message |
|---|---|---|---|---|---|---|---|
| 3.5 pinned `1c8deae` | none / 2 / 3 / 5 | 1 | 2 | 0 | 0 | 23.0 | "no phrases could be tokenized … boosting disabled" |
| EN pinned `ebe59e5` | none / 2 / 3 / 5 | 1 | 3 | 0 | 0 | 15.1 | same |
| 3.5 `ea30d66` | none | 1 | 2 | 0 | 0 | 23.0 | – |
| 3.5 `ea30d66` | 2 | 1 | 2 | 0 | 19 | 24.5 | none |
| 3.5 `ea30d66` | 3 | 1 | 2 | **11** | 111 | **36.5** | none |
| 3.5 `ea30d66` | 5 | 0 | 1 | **52** | 414 | **83.4** | none |
| 3.5 `ea30d66`, 20 names not in the audio | 3 | 1 | 0 | 0 | 170 | **45.7** | none |
LOWBOOST

What it looks like at boost 3 (AMI 265 s): "a small cute in Planet of the Apes at the Planet of the Apes becomes real I Planet of the Apes with them"; with the 20 absent names: "Svensson uh when Petrov Svensson Ramire I Ramirez with them you can take as Lindqvist as you like". The greedy RNNT boosting in 0.2.0 fixes neither Craig nor beagle at any level; "Planet of the Apes" only shows up once it is inserted all over the clip. It inserts boosted phrases wherever their first tokens are plausible, and the more phrases and the higher the boost, the more it inserts. The plan's G default (boost 3, speaker names from the library) would wreck transcripts: a list of 20 names the meeting never mentions takes the clip from 23.0 to 45.7 % WER.

## 6. Other findings

- **`--verbatim` changes nothing here.** In nemo-speech 0.2.0 it sets `verbatim_transcripts`, which only skips inverse text normalisation (`src/asr/postproc/pipeline.cpp`, the ITN stage); fillers are dropped by the model itself, not by a post-processor. With no ITN grammars configured, `nemotron35_verbatim` (now runnable: `engine_args = ["--verbatim"]`) gives word-for-word and time-for-time identical output to `nemotron35` on AMI, Aug 2 and Sep 5 (`levers.py diff --a nemotron35 --b nemotron35_verbatim`: 0 edit operations). docs/05's hope that it brings fillers back does not hold.
- **The 3.5 prompt dictionary** (`asr.rnnt.prompt_dictionary` in the GGUF, 121 entries) maps bare `en`, `es` (to es-US), `fr`, `de`, `hi`, `pt` (to **pt-PT**), `it`, `nl`, `pl`, `ru`, `ko`, `tr`, `uk`, `ro` and others, but has no bare `ja`, `zh`, `vi`, `th` or `he`; those need the locale form (`ja-JP`, `zh-CN`, ...). LANGTAG_RESULT
- **GPU sharing during evals.** Another agent's debug build of the desktop app with a live `nemo-speech serve` held 2 GB and 20 to 40 % utilisation for part of the afternoon. run.py's idle check (`tools/eval/gpu.py`) now waits only while another job keeps the GPU busy (median utilisation of 25 % or more over 5 s) or holds more than 1.2 GB above the idle level; before, it waited on memory alone and a desktop at 1.7 to 1.9 GB of browser and app windows blocked it.

## 8. Reproducing (all from `tools/eval`, after the README setup)

```sh
# environment: the main checkout's engine, models and diarist-cli (this worktree has none)
export DIARIST_EVAL_ENGINE_DIR='<main>\engine\nemo-speech\nemo-speech-0.2.0-windows-x86_64-cuda'
export DIARIST_EVAL_MODELS_DIR='<main>\models'
export DIARIST_EVAL_CLI='<main>\tools\eval\.bin\diarist-cli.exe'

# 1. lookahead
for c in nemotron35 n35_rc_3 n35_rc_6 n35_rc_13 nemotron_en en_rc_13 n35_rc_max en_rc_max; do
  uv run python -I run.py --config $c --sessions ami,aug2,sep5 --out .work/reports/$c-fast.md
done
for c in nemotron35 n35_rc_3 n35_rc_6 n35_rc_13 nemotron_en en_rc_13; do uv run python -I run.py --config $c --sessions aug14; done
uv run python -I levers.py table --configs nemotron35,n35_rc_3,n35_rc_6,n35_rc_13,nemotron_en,en_rc_13
uv run python -I levers.py diff --a n35_rc_3 --b n35_rc_13          # 13 differs from 3
uv run python -I levers.py diff --a n35_rc_max --b n35_rc_3         # -1 == 3 on 3.5
uv run python -I levers.py diff --a en_rc_max --b en_rc_13          # -1 == 13 on EN
uv run python -I levers.py offsets --base nemotron35 --configs n35_rc_3,n35_rc_6,n35_rc_13 --sessions ami,aug2,sep5,aug14
uv run python -I levers.py offsets --base nemotron_en --configs en_rc_13 --sessions ami,aug2,sep5,aug14
uv run python -I levers.py tags --configs nemotron35,n35_rc_3,n35_rc_6,n35_rc_13,nemotron_en,en_rc_13 --session aug14

# 2. diarizer geometry
uv run python -I run.py --config n35_rc_13_diar_off --sessions all
uv run python -I run.py --config en_rc_13_diar_off --sessions all

# 3. punctuation: the gate probe, then the converter (venv per section 3)
uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr nemotron-3.5 --language en --out .work/spikes/pnc/n35_gate -- --pnc-model .work/pnc/gate-probe-not-a-model.gguf
uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr nemotron-en --out .work/spikes/pnc/en_gate -- --pnc-model .work/pnc/gate-probe-not-a-model.gguf
.work/pnc-venv/Scripts/python.exe convert_model.py .work/pnc/src/pcs47_1jun.nemo --outfile .work/pnc/pcs47_1jun.q8_0.gguf --outtype q8_0
.work/pnc-venv/Scripts/python.exe convert_model.py .work/pnc/src/pcs47_1jun.nemo --architecture pnc --outfile .work/pnc/pcs47_1jun.q8_0.gguf --outtype q8_0

# 4. boosting (the clip is AMI 50 to 350 s: ffmpeg -ss 50 -t 300 -i ES2002a.Mix-Headset.wav -ac 1 -ar 16000 .work/clips/ami_050-350.wav)
uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr <model> [--language en] --out .work/spikes/boost/<run> -- \
    --speech-context Craig --speech-context beagle --speech-context "Planet of the Apes" --speech-context-boost <2|3|5>
uv run python -I levers.py boost --base .work/spikes/boost/n35_base --runs .work/spikes/boost/n35_b2 ... --phrases Craig beagle "Planet of the Apes"

# 5. languages
uv run python -I fleurs_fetch.py
for c in nemotron35 n35_rc_13; do
  uv run python -I run.py --config $c --fleurs all --fleurs-language given
  uv run python -I run.py --config $c --fleurs all --fleurs-language auto
done
uv run python -I run.py --config nemotron35 --fleurs ja,pt --fleurs-language short
uv run python -I run.py --config nemotron_en --fleurs en
uv run python -I run.py --config en_rc_13 --fleurs en
uv run python -I run.py --fleurs-report

# full-set reports (docs/eval/2026-10-16-<config>.md)
for c in nemotron35 nemotron_en n35_rc_13 en_rc_13 n35_rc_13_diar_off en_rc_13_diar_off; do uv run python -I run.py --config $c --sessions all; done
```

`convert_model.py` runs from the v0.2.0 checkout with its directory on `sys.path` (`python -I` drops the script's directory, so the harness used a three-line `runpy` wrapper that prepends it).

Owner-only, not run (downloading accepts the NGC Terms of Use; see section 3):

```sh
curl -L --ssl-revoke-best-effort -o tools/eval/.work/pnc/src/punctuation_en_bert.nemo "https://api.ngc.nvidia.com/v2/models/nvidia/nemo/punctuation_en_bert/versions/1.0.0rc1/files/punctuation_en_bert.nemo"
```
