# Accuracy levers in nemo-speech 0.2.0: measurements (16 Oct 2026)

Chunk A of `docs/plans/12-release-beta2.md`: the four runtime levers Diarist does not use yet (decoder lookahead, diarizer geometry, punctuation model, word boosting) and a per-language smoke test, measured on the fixed eval set (`docs/05-eval-data.md`). No product code changed. Opus 5.5 implementation agent.

**Summary** (section 7 has the table): adopt lookahead 13 and the `v3-offline` diarizer preset for file jobs and the post-pass. Together they take AMI cpWER from 23.0 to 20.7 % (Nemotron 3.5) and from 19.8 to 18.0 % (EN), make the transcribe step 2.6 times faster at the same VRAM, and lower near-change WDER on five of six multi-speaker sessions; the one cost is a 7-word speaker merged on Sep 5. Drop the runtime punctuation hook (nemo-speech ignores it for both of Diarist's models) and word boosting (impossible on the pinned GGUFs, destructive on the re-converted one). Pass locale tags: bare `ja` runs as `auto`, bare `pt` as pt-PT. Eval reports: `docs/eval/2026-10-16-{nemotron35,n35_rc_13,n35_rc_13_diar_off,nemotron_en,en_rc_13,en_rc_13_diar_off,fleurs}.md`.

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
- The offset is not constant per word. Live (the realtime server, R = 1) against the post-pass (R = 13, `v3-offline`) on two recorded meetings (12 and 34 minutes, EN, replayed through both paths), words matched by text: post-pass starts a median 160 ms earlier, mean 184 to 194 ms, p10 400 ms earlier, p90 0; only 58 to 64 percent sit within one 80 ms frame of the median. The median is the same in every tenth of either meeting (no drift). With words that are 80 ms spikes (the median word duration of both engines), word-on-word overlap after the best constant shift is 51 to 57 percent of the live word time, under carry-over's 60 percent bar, so carry-over matches speech regions instead (docs/02, "Carry-over").

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

### Full set (seven sessions)

`uv run python -I run.py --config <config> --sessions all` for `nemotron35`, `n35_rc_13`, `n35_rc_13_diar_off`, `nemotron_en`, `en_rc_13`, `en_rc_13_diar_off`; table from `uv run python -I levers.py compare --configs <...>`; reports in `docs/eval/2026-10-16-<config>.md`. Percentages; speakers are reference / transcript. In every session the preset leaves the words identical and retags 0 to 40 words (Aug 14: about 2,500, mostly a different numbering of the same 8 labels).

| Session | Config | Ref / hyp speakers | DER c0.25 | WDER | WDER near | cpWER |
|---|---|---|---|---|---|---|
| Aug 2 | nemotron35 | 6 / 6 | 8.5 | 0.6 | 9.1 | 12.0 |
| Aug 2 | n35_rc_13 | 6 / 6 | 7.6 | 0.5 | 8.2 | 10.9 |
| Aug 2 | n35_rc_13_diar_off | 6 / 6 | 7.5 | 0.2 | 5.5 | 10.3 |
| Aug 2 | nemotron_en | 6 / 6 | 6.4 | 0.6 | 8.8 | 10.3 |
| Aug 2 | en_rc_13 | 6 / 6 | 7.7 | 0.7 | 8.2 | 10.2 |
| Aug 2 | en_rc_13_diar_off | 6 / 6 | 7.4 | 0.3 | 5.6 | 9.5 |
| Aug 9 | nemotron35 | 3 / 3 | 9.0 | 0.3 | 7.4 | 9.2 |
| Aug 9 | n35_rc_13 | 3 / 3 | 9.0 | 0.3 | 6.6 | 8.5 |
| Aug 9 | n35_rc_13_diar_off | 3 / 3 | 8.9 | 0.3 | 5.7 | 8.4 |
| Aug 9 | nemotron_en | 3 / 3 | 8.4 | 0.3 | 6.9 | 8.5 |
| Aug 9 | en_rc_13 | 3 / 3 | 9.0 | 0.4 | 7.6 | 8.1 |
| Aug 9 | en_rc_13_diar_off | 3 / 3 | 9.0 | 0.3 | 6.8 | 8.1 |
| Sep 7 | nemotron35 | 5 / 4 | 12.6 | 0.3 | 8.2 | 7.9 |
| Sep 7 | n35_rc_13 | 5 / 4 | 11.9 | 0.2 | 5.7 | 7.5 |
| Sep 7 | n35_rc_13_diar_off | 5 / 4 | 11.9 | 0.2 | 5.7 | 7.5 |
| Sep 7 | nemotron_en | 5 / 4 | 10.6 | 0.3 | 8.2 | 7.8 |
| Sep 7 | en_rc_13 | 5 / 4 | 11.4 | 0.4 | 11.8 | 7.3 |
| Sep 7 | en_rc_13_diar_off | 5 / 4 | 11.4 | 0.3 | 12.7 | 7.2 |
| Sep 5 | nemotron35 | 5 / 5 | 3.6 | 0.2 | 7.8 | 5.4 |
| Sep 5 | n35_rc_13 | 5 / 5 | 3.6 | 0.1 | 7.1 | 5.1 |
| Sep 5 | n35_rc_13_diar_off | 5 / 4 | 3.6 | 0.2 | 7.8 | 5.1 |
| Sep 5 | nemotron_en | 5 / 5 | 3.0 | 0.1 | 6.9 | 5.1 |
| Sep 5 | en_rc_13 | 5 / 5 | 3.9 | 0.2 | 7.3 | 4.7 |
| Sep 5 | en_rc_13_diar_off | 5 / 4 | 3.9 | 0.2 | 7.3 | 4.7 |
| Sep Logistics | nemotron35 | 1 / 1 | 6.7 | 0.0 | – | 7.3 |
| Sep Logistics | n35_rc_13 | 1 / 1 | 5.2 | 0.0 | – | 6.6 |
| Sep Logistics | n35_rc_13_diar_off | 1 / 1 | 5.2 | 0.0 | – | 6.6 |
| Sep Logistics | nemotron_en | 1 / 1 | 5.9 | 0.0 | – | 7.1 |
| Sep Logistics | en_rc_13 | 1 / 1 | 6.5 | 0.0 | – | 7.0 |
| Sep Logistics | en_rc_13_diar_off | 1 / 1 | 6.5 | 0.0 | – | 7.0 |
| Aug 14 | nemotron35 | 6 / 8 | 26.8 | 20.7 | 40.4 | 44.3 |
| Aug 14 | n35_rc_13 | 6 / 8 | 26.1 | 20.7 | 41.2 | 43.5 |
| Aug 14 | n35_rc_13_diar_off | 6 / 8 | 25.0 | 19.3 | 18.0 | 42.9 |
| Aug 14 | nemotron_en | 6 / 8 | 26.3 | 20.9 | 43.8 | 44.7 |
| Aug 14 | en_rc_13 | 6 / 8 | 27.1 | 20.9 | 42.2 | 44.3 |
| Aug 14 | en_rc_13_diar_off | 6 / 8 | 25.7 | 19.4 | 19.1 | 43.9 |
| AMI ES2002a | nemotron35 | 4 / 4 | 26.1 | 3.2 | 8.9 | 23.0 |
| AMI ES2002a | n35_rc_13 | 4 / 4 | 24.8 | 2.3 | 6.6 | 20.8 |
| AMI ES2002a | n35_rc_13_diar_off | 4 / 4 | 24.7 | 2.2 | 6.2 | 20.7 |
| AMI ES2002a | nemotron_en | 4 / 4 | 20.4 | 3.4 | 9.3 | 19.8 |
| AMI ES2002a | en_rc_13 | 4 / 4 | 19.8 | 2.7 | 7.3 | 18.3 |
| AMI ES2002a | en_rc_13_diar_off | 4 / 4 | 19.9 | 2.3 | 6.3 | 18.0 |

Noise band: WDER near counts the 110 to 240 reference words within 1 s of a speaker change in a Zoom session, so one word moves it by 0.4 to 0.9 points; AMI has 716 such words (0.14 points per word).

- **Nemotron 3.5 + preset against 3.5 at lookahead 13**: near-change WDER improves on Aug 2 (8.2 to 5.5), Aug 9 (6.6 to 5.7), AMI (6.6 to 6.2) and the stress session Aug 14 (41.2 to 18.0); Sep 7 and Sep Logistics are unchanged; Sep 5 goes from 7.1 to 7.8, one word, inside the band. Overall WDER never rises by more than 0.1. cpWER is equal or better everywhere (Aug 2 10.9 to 10.3).
- **Speaker count**: the preset regresses one session, Sep 5, from 5 to 4 speakers: the reference's smallest speaker (7 words) is merged into another label, on both models. Sep 7 is one short with and without it (as it was at the default lookahead in the 15 Oct report). The single-speaker control stays at 1. This is within docs/00's bar ("within one"), but it is a real loss of a speaker who said one sentence.
- **Nemotron EN + preset against EN at lookahead 13**: the same pattern (Aug 2 8.2 to 5.6, Aug 9 7.6 to 6.8, AMI 7.3 to 6.3, Aug 14 42.2 to 19.1, Sep 5 7.3 to 7.3, Sep 7 11.8 to 12.7, one word), and the same Sep 5 speaker merge.
- **Lookahead and near-change WDER on EN**: lookahead 13 alone does not help the EN model's near-change attribution on every Zoom session the way it does on 3.5: Aug 2 8.8 to 8.2, Sep 5 6.9 to 7.3, Aug 9 6.9 to 7.6 and **Sep 7 8.2 to 11.8** (about four words), while AMI improves 9.3 to 7.3; its transcript DER also rises by 0.6 to 1.3 points on the Zoom sessions (the segments move with the 160 ms earlier word starts). On 3.5 lookahead 13 improves near-change WDER on every session but the stress session (Sep 7 8.2 to 5.7, Aug 9 7.4 to 6.6, Sep 5 7.8 to 7.1; Aug 14 40.4 to 41.2). EN's Sep 7 is the one session in the whole table that regresses beyond the band, and the preset does not repair it (12.7).

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
| 3.5 `ea30d66` | 0.5 | 1 | 2 | 0 | 2 | 23.0 | none |
| 3.5 `ea30d66` | 1 | 1 | 2 | 0 | 7 | 23.5 | none |
| 3.5 `ea30d66`, 20 names not in the audio | 1 | 1 | 2 | 0 | 9 | 23.5 | none |

The boost 0.5 and 1 rows were added after the plan's 2, 3 and 5 to look for a harmless level: at 1 the output still says "I'm Greg" and "plant at the amps", and both lists cost half a point of WER. No level on this clip fixes a target word without damage elsewhere.

What it looks like at boost 3 (AMI 265 s): "a small cute in Planet of the Apes at the Planet of the Apes becomes real I Planet of the Apes with them"; with the 20 absent names: "Svensson uh when Petrov Svensson Ramire I Ramirez with them you can take as Lindqvist as you like". The greedy RNNT boosting in 0.2.0 fixes neither Craig nor beagle at any level; "Planet of the Apes" only shows up once it is inserted all over the clip. It inserts boosted phrases wherever their first tokens are plausible, and the more phrases and the higher the boost, the more it inserts. The plan's G default (boost 3, speaker names from the library) would wreck transcripts: a list of 20 names the meeting never mentions takes the clip from 23.0 to 45.7 % WER.

## 5. Languages (FLEURS slices, WER only)

### Data and method

`fleurs_fetch.py` takes about 10 minutes of the FLEURS test split per language (`google/fleurs`, CC-BY-4.0): one recording per sentence in archive order, 37 to 55 utterances, joined with 1.5 s of silence into one WAV, so each language is one 10-minute "meeting" with a new speaker every 10 to 15 s. It streams the dataset's own `data/<config>/audio/test.tar.gz` and stops early (26 to 30 MB per language). The parquet conversion the plan named cannot be read in part: each language is one row group whose audio column is a single 0.3 to 0.7 GB dictionary page. `run.py --fleurs` runs `diarist-cli transcribe --no-diarize --language <tag>` on it and scores the whole slice: for en the harness normaliser; for the others NFKC, casefold and punctuation and symbols removed with combining marks kept (whisper's BasicTextNormalizer would split Hindi words at every vowel sign); CER for ja. Tags are the ones the runtime honours (section 6): `de`, `fr`, `it`, `hi`, `en`, `ja-JP`, `es-US` (FLEURS es_419 is Latin American), `pt-BR`. zh was not in the plan's list and was not fetched.

Commands: `uv run python -I run.py --config <nemotron35|n35_rc_13> --fleurs all --fleurs-language <given|auto>`, the EN control with `--config <nemotron_en|en_rc_13> --fleurs en`, the report with `uv run python -I run.py --fleurs-report` (written to `docs/eval/2026-10-16-fleurs.md`, which also has substitutions, deletions and insertions per run).

### Results

WER in percent (CER for ja). Card: the Nemotron 3.5 model card's FLEURS numbers (full test split, NVIDIA's normaliser) at the same lookahead, language given / auto-detect.

| Lang | Units | 160 ms given | 160 ms auto | **1.12 s given** | **1.12 s auto** | Card 160 ms given / auto | Card 1.12 s given / auto |
|---|---|---|---|---|---|---|---|
| de | 878 | 12.3 | 12.4 | 10.6 | 10.3 | 9.21 / 9.37 | 8.31 / 8.22 |
| es (`es-US`) | 1,178 | 16.5 | 25.4 | **6.8** | 9.5 | 4.64 / 4.82 | 4.11 / 4.13 |
| fr | 1,360 | 14.0 | 14.2 | 11.8 | 11.8 | 10.60 / 10.60 | 9.03 / 9.02 |
| it | 989 | 7.7 | 7.7 | 6.4 | 6.1 | 4.85 / 4.89 | 4.25 / 4.32 |
| pt (`pt-BR`) | 1,046 | 9.2 | 9.2 | 9.1 | 8.7 | 6.10 / 6.19 | 5.48 / 5.47 |
| hi | 1,168 | 15.7 | 15.1 | 14.2 | 14.9 | 7.97 / 10.83 | 6.81 / 8.23 |
| ja (`ja-JP`, CER) | 2,072 | 23.5 | 24.7 | 20.9 | 21.1 | 12.90 / 13.85 | 11.48 / 11.66 |
| en | 1,186 | 12.9 | 14.6 | **8.8** | 8.8 | 8.88 / 9.34 | 7.91 / 8.84 |
| en, **Nemotron EN** (control) | 1,186 | 23.3 | – | 21.5 | – | – | – |

- **Lookahead 13 helps every language**, by 0.1 (pt) to 9.7 (es) points with the language given, and it is three times faster (4.4 to 6.0 s per 10-minute slice against 13.9 to 16.6 s). This is the same lever as section 1, measured on eight languages.
- **At the default lookahead the runtime drops speech on these slices.** Deletions dominate the 160 ms errors (es: 114 deletions given, 241 auto, against 12 and 58 at 1.12 s). `levers.py fleurs-utts --langs es --configs nemotron35,n35_rc_13 --mode auto` lists 12 Spanish utterances that lose half or more of their words at 160 ms, at normal levels (peaks −15 to 0 dBFS), and 2 at 1.12 s. Running the same Spanish WAV directly at 160 ms reproduces it (one utterance gone, the opening phrase of another missing), while a 40 s cut around the same utterance is transcribed completely: it depends on the long stream, not on the audio of the utterance. The meeting recordings do not show it (no 30 s window in AMI, Aug 2, Sep 5 or Aug 14 where 160 ms has under 60 % of the 1.12 s words), so it seems tied to these slices' rapid speaker changes and digital silence; but it is one more reason not to run file jobs at 160 ms.
- **Auto-detect against a given language**: at 1.12 s they are within 0.7 points except Spanish (9.5 against 6.8; detection picks es-ES for Latin American audio). At 160 ms auto costs more (en 14.6 against 12.9, es 25.4 against 16.5). Detection itself was right for every language at the language level; for Spanish and Portuguese it picks the European locale (es-ES, pt-PT) on Latin American and Brazilian audio.
- **Our numbers sit 1 to 9.5 points above the card's.** The slices are small (one or two points of noise), FLEURS writes numbers as digits while the runtime, with no ITN grammars, writes them as words (15 to 20 % of the English and German edit operations involve a digit), and the normalisers differ. Two of the 42 Japanese recordings are near-silent in the source (peak −41 and −43 dBFS) and cost about 2.5 CER points by themselves; Hindi has the largest gap to the card and was not investigated further. Read the table for the direction of each lever, not as Diarist's published WER.
- **The EN model is not a usable control on FLEURS English**: 23.3 % at 160 ms and 21.5 % at 1.12 s, nearly all deletions (195 and 189). 36 of the 55 English recordings peak below −30 dBFS, and the EN model returns nothing, or a fraction, for eight of them (`fleurs-utts --langs en --configs nemotron_en,en_rc_13`), where Nemotron 3.5 transcribes all 55. So the EN model drops quiet speech that 3.5 hears. Diarist's Zoom recordings are louder, which is why EN still wins on AMI and Zoom cpWER (section 1); a quiet participant on a far microphone would not.

## 6. Other findings

- **`--verbatim` changes nothing here.** In nemo-speech 0.2.0 it sets `verbatim_transcripts`, which only skips inverse text normalisation (`src/asr/postproc/pipeline.cpp`, the ITN stage); fillers are dropped by the model itself, not by a post-processor. With no ITN grammars configured, `nemotron35_verbatim` (now runnable: `engine_args = ["--verbatim"]`) gives word-for-word and time-for-time identical output to `nemotron35` on AMI, Aug 2 and Sep 5 (`levers.py diff --a nemotron35 --b nemotron35_verbatim`: 0 edit operations). docs/05's hope that it brings fillers back does not hold.
- **The 3.5 prompt dictionary** (`asr.rnnt.prompt_dictionary` in the GGUF, 121 entries) maps bare `en`, `es` (to es-US), `fr`, `de`, `hi`, `pt` (to **pt-PT**), `it`, `nl`, `pl`, `ru`, `ko`, `tr`, `uk`, `ro` and others, but has no bare `ja`, `zh`, `vi`, `th` or `he`; those need the locale form (`ja-JP`, `zh-CN`, ...). Checked on the FLEURS slices with Nemotron 3.5 at the default lookahead (`run.py --config nemotron35 --fleurs ja,pt --fleurs-language short`): bare `ja`, which the app's language list offers today, gives output identical to `auto` (0 edit operations; CER 24.7 % against 23.5 % with `ja-JP`), so a user who picks Japanese silently gets detection. Bare `pt` runs the pt-PT prompt on Brazilian audio (the runtime reports `pt-PT`): 150 words differ from the `pt-BR` run, at the same WER (9.2 %) on this slice. The app needs the locale mapping that chunk C adds (ja to ja-JP, zh to zh-CN, separate pt-BR and pt-PT, es-ES and es-US).
- **GPU sharing during evals.** Another agent's debug build of the desktop app with a live `nemo-speech serve` held 2 GB and 20 to 40 % utilisation for part of the afternoon. run.py's idle check (`tools/eval/gpu.py`) now waits only while another job keeps the GPU busy (median utilisation of 25 % or more over 5 s) or holds more than 1.2 GB above the idle level; before, it waited on memory alone and a desktop at 1.7 to 1.9 GB of browser and app windows blocked it.

## 7. Recommendations

| Lever | Numbers (sections above) | Verdict |
|---|---|---|
| **Decoder lookahead** for file jobs and the post-pass: `--asr.streaming.rnnt_right_context 13` on both models (the explicit value; `-1` means 3 on the pinned 3.5) | AMI cpWER 23.0 → 20.8 (3.5) and 19.8 → 18.3 (EN). Zoom cpWER −0.3 to −1.1 (3.5) and −0.1 to −0.5 (EN) on all five user meetings. Near-change WDER on 3.5 better on every session but the stress one; on EN mixed (AMI 9.3 → 7.3, Sep 7 8.2 → 11.8). FLEURS: better in all eight languages, −0.1 to −9.7 points with the language given. Transcribe step 663 → 363 s for the 324-minute full set (×29 → ×54 real time). Peak VRAM unchanged (1.66 to 1.89 GB on the 77-minute file). Tag agreement with `diarize` 99.9 % (3.5), 99.7 % (EN). Word starts 80 ms (3.5) and 160 ms (EN) earlier, toward the true times | **Adopt** as the default for file jobs and the post-pass, both models (chunk E). Live stays at R = 1. Live-to-post-pass carry-over must match within at least 250 ms (or on text). A hidden `engine.file_lookahead` key is harmless, but no VRAM or speed cost needs an escape hatch. Watch EN's Sep 7 near-change number when EN is the selected model |
| **Diarizer geometry** `--asr.diar.preset v3-offline` in the fused pass, with lookahead 13 | Near-change WDER, 3.5: Aug 2 8.2 → 5.5, Aug 9 6.6 → 5.7, AMI 6.6 → 6.2, Aug 14 (stress) 41.2 → 18.0; Sep 7 and Sep Logistics unchanged; Sep 5 7.1 → 7.8 (one word). EN the same pattern. Words identical; cpWER equal or better everywhere. Transcribe step another 31 % faster (363 → 251 s, ×77 real time), VRAM the same or lower. One regression: Sep 5 loses its 7-word speaker (5 → 4) on both models; the single-speaker control stays at 1 | **Adopt** for file jobs and the post-pass, together with the lookahead. The speaker merge is within docs/00's "within one" and costs one sentence's label; keep a stored key to fall back to `v3-streaming` if users report merged one-sentence speakers. Live keeps the streaming geometry. The standalone `diarize` pass (if the app keeps one) should get the same preset so the two passes agree |
| **Punctuation through the runtime** (`--pnc-model`) | nemo-speech 0.2.0 ignores `--pnc-model` for any ASR model whose vocabulary has a sentence terminator, and both of ours do: the probe gives the warning, identical words, no time or VRAM cost. The Apache-2.0 47-language checkpoint does not convert ("cannot determine checkpoint architecture", then "missing expected PnC artifacts in .nemo"). The NGC checkpoints are under the NGC Terms of Use | **Drop** for beta.2, including F's "plumbing" (dead code for every model in the manifest). Punctuation goes to the app-side route (the 47-language ONNX model applied to the existing `words[]`, case upgrades only), beta.3 |
| **Word boosting** (`--speech-context`) | Pinned GGUFs: "no embedded SentencePiece tokenizer … boosting disabled", output unchanged. Re-converted 3.5 (`ea30d66`, 742,090,464 B, SHA-256 `3fc991d3…70b1`): no target word fixed at any boost from 0.5 to 5; clip WER 23.0 → 23.5 (boost 1), 24.5 (2), 36.5 (3), 83.4 (5); 20 names absent from the audio: 23.5 at boost 1, 45.7 at 3 | **Drop** for beta.2 and do not build G on runtime 0.2.0. No manifest bump for it (the re-converted GGUF also decodes slightly differently without boosting). Revisit with a runtime release that changes RNNT boosting; repeat the 20-absent-names control first |
| **Language tags** | Bare `ja` runs as `auto` (identical output); bare `pt` runs pt-PT on Brazilian audio; `es` means es-US | **Adopt** chunk C's locale mapping (ja-JP, zh-CN, pt-BR and pt-PT, es-ES and es-US) |
| **Auto-detect** | At 1.12 s within 0.7 points of a given tag except Spanish (9.5 against 6.8); at 160 ms up to 8.9 points worse (es) | Keep `auto` as a choice, not the default. With lookahead 13 its cost is small for most languages |
| **`--verbatim`** | Identical output (it only skips ITN; no grammars are configured) | **Drop**; docs/05 corrected |

What the website can say from this: on AMI ES2002a (far-field mix, word-level manual reference), Diarist's file pass goes from 23.0 to 20.7 % cpWER with Nemotron 3.5 and from 19.8 to 18.0 % with Nemotron EN with both levers, and on Diarist's own meetings it puts 5.5 to 7.8 % of the words near speaker changes on the wrong speaker (the in-room stress session aside), at about 77 times real time for the transcribe step on an RTX 3080. The FLEURS slices are smoke tests, not publishable WER; per-language claims should quote the model card.

### Not done, and why

- The NGC `punctuation_en_bert` functional check: not downloaded, because downloading it accepts the NGC Terms of Use, which is the owner's call. The gate in section 3 makes it moot for Diarist's models. The command is in section 8.
- zh was not fetched for FLEURS (not in the plan's language list); the tag finding for it (`zh-CN`) comes from the prompt dictionary and the plan review's test.
- The EN model's Sep 7 regression at lookahead 13 (near-change WDER 8.2 → 11.8) was not investigated further.

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
