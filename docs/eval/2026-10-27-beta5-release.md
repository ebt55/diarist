# Eval: release check for 0.1.0-beta.5 (27 Oct 2026)

The release check of the beta.5 integration branch against the beta.4 baseline (`docs/eval/2026-10-26-baseline-beta4.md`): the same 36 sessions (the owner's 35 Zoom recordings plus AMI ES2002a), the same models, runtime and file-pass settings, scored by the same definitions, with diarist-cli rebuilt from the integration tree at `d9a0666` (chunks A to F, D, I, J and K, and the public-data harness). Shipped configuration only, now with the two beta.5 transcript steps at their defaults: the repeated-token loop collapse (`transcript.collapse_loops = on`, plan 16 section 12.3) and punctuation (`transcript.punctuation = auto` with the manifest's punctuation model, plan 16 section 3). Both ASR models: Nemotron EN (the default) and Nemotron 3.5. Numbers are percentages unless a column says otherwise.

On these recordings (single-track files, the import path) the beta.5 changes that can act are the loop collapse, which removes words, and punctuation, which never changes words, times or speakers but splits segments at sentence ends. The echo gate (chunk I), Process again (J) and the live lookahead (F) act on live recordings only, and the model files are unchanged (the Q8 pointwise respin was not shipped, research/19).

Result: the three accuracy bars pass. Pooled cpWER EN 11.01 → 10.85, Nemotron 3.5 11.30 → 11.30; no session is worse at all; no gap is new and none is gone. The speed bar passes for both models. Over all 14 gate runs, pooled × real time against beta.4 is +5.0 percent with EN (faster) and, as measured, -5.4 percent with Nemotron 3.5 (slower, 0.4 points beyond the 5 percent bar). It passes for 3.5 because its shortfall sits in the three late runs, which overlapped claude.exe at up to 34 percent SM; the same build timed those sessions at -1.5 to +1.3 percent of beta.4 at 13:44 to 13:48, and the timed step is the same executable, models and command line as beta.4 (speed, below). Accepted by the owner on 27 Oct 2026; re-timed in beta.6.

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88 |
| diarist-cli | built in release from `d9a0666` (`claude/beta5-integration`), SHA-256 `ce5b757b031734406992728f32332bec23d0be121f64659556442291eb23401a` |
| Engine | nemo-speech 0.2.0 (`engine_version` in every run.json), package `nemo-speech-0.2.0-windows-x86_64-cuda`, a byte copy of the app's installed engine; `nemo-speech.exe` SHA-256 `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272` (the baseline's) |
| ASR, EN | `nemotron-speech-streaming-en-0.6b.q8_0`, SHA-256 `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d` (the baseline's) |
| ASR, 3.5 | `nemotron-3.5-asr-streaming-0.6b.q8_0`, SHA-256 `a5c435f294eea8f88ce68dd27b8c3bfea7f777cb2fbba04fcd30eaa555f429ae` (the baseline's) |
| Diarizer | `Nemotron-3-Diarization.q8_0`, SHA-256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` (the baseline's) |
| Punctuation | `punctuation-horizon-small` (`Horizon-Labs/punctuation-restoration-small` @ `ee31fdfc5304`): `onnx/model_quantized.onnx` SHA-256 `5679870be92c7fa47c9be9b2872fac389ef28d43d2c33f35e1315b93a82f4ace`, `tokenizer.json` `f34b44a6dc8a0eef651bfa10e8325057530152606a69d9c3b6ae02efb1558f0a`, `diarist-onnxruntime.dll` (ONNX Runtime 1.31.0, CPU) `0c42dc4e278f0968fdf9ebf1b1e54a74c657a9942e70803e453c91993451cf4c`, and the four licence and notice files. The release asset is not published yet, so the spike's candidate A copy and the prepared release files were used; all seven files and the total size match `manifests/models.json`. They sit in the eval models folder under the entry's `dir`, where the scan matched the entry (`[punctuation-horizon-small]`), and the harness selects it in each data root (`prepare_data_root`) |
| Settings recorded in run.json | `right_context` 13, `diar_preset` v3-offline, `device` cuda:0, `language` en, `engine_args` empty, `punctuation` true, `collapse_loops` true, `diar_pass` false |
| Punctuation as run (`engine_info.punctuation`, all 72 runs) | `applied` true, `reason` ok, unit `turn`, policy `keep_asr`, 4 intra-op threads, runtime version 1.31.0, language en |
| Engine command (from engine.log) | `nemo-speech transcribe <wav> --model <asr.gguf> [--language en, 3.5 only] --stream --endpointing --asr.streaming.rnnt_right_context 13 --diarize --diar-model <Nemotron-3-Diarization.q8_0.gguf> --asr.diar.preset v3-offline --device cuda:0 --json --output <out> --force`, the baseline's |
| Text normaliser | whisper EnglishTextNormalizer per token, filler removal, ok to okay (the harness's) |

## Data

The baseline's sessions file, unchanged:

| Group | Sessions | Minutes | What it is |
|---|---|---|---|
| Gate set | Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14, AMI ES2002a | 324 | docs/05's established set |
| Broad | Aug 1, 3 to 8, 10 to 13, 15; Sep 1 to 4, 6; Jul 1 to 5 | 1,017 | every other recording of the owner's three meeting folders (22 recordings) |
| Talks | Talk 1 to Talk 7 | 356 | seven Zoom webinars, 2 to 5 speakers |

References as in the baseline: Zoom's own `transcript.vtt` converted by `zoom_vtt_to_ref.py`, AMI's word-level manual annotation. On the Zoom sessions cpWER, tcpWER and WER mix in Zoom's errors; word accuracy proper is judged on AMI only.

## Metrics

The baseline's, computed by the same code: research/19's `analyze.py` (plain WER, its deletions, insertions and substitutions, and the gaps) over `run.py`'s metrics.json (cpWER, tcpWER, WDER, DER, count error). Run over the baseline's own runs it reproduces every number of the baseline's 72 rows exactly, so the two columns of every table below are scored alike.

- cpWER, tcpWER (collar 5 s), WDER and DER at collar 0.25 (`tools/eval/metrics.py`).
- WER: speaker-agnostic, time-ordered normalised words, Levenshtein; del, ins and sub are its counts.
- Count err: transcript speakers minus reference speakers.
- Gaps: stretches of at least 2 s (and of at least 5 s) without a hypothesis word in which the reference has at least 3 words, with a 0.5 s margin at both ends (research/19 section 0). "New" and "gone" compare each run's gaps with the baseline run's gaps of the same session and model by overlap in time.
- × real time: audio duration over the `nemo-speech transcribe` step's wall time (`asr_wall_s`, model load included), taken under quiet.lock, as in the baseline.
- Punctuation and case do not enter any word score: the harness normalises every token (marks removed, casefolded) before alignment. Checked on Aug 2 (EN): the run scored as exported (1,220 words ending in a mark, 1,494 words whose display text differs from the ASR's) and scored again with every word's text replaced by its `raw_text` give identical cpWER, tcpWER, DI-cpWER, ORC-WER, WER with its del, ins and sub, WDER, gaps and word counts.
- DER does move, through punctuation alone: the export RTTM has one span per segment, segments now end at sentence ends where the pause allows, and the pause after a sentence end reads as missed speech against Zoom's cues and AMI's word-run RTTM. The gate set's EN DER here equals chunk D's app-path replay with punctuation on exactly (`docs/eval/2026-10-26-punctuation.md` section 12.5: Aug 2 7.4 → 13.2, AMI 19.9 → 25.4, +4.4 points on average). WDER, which scores every word's speaker, does not move with punctuation (it moves only where the collapse removed words). The DER columns below are therefore not comparable with the baseline's and are not a release bar.

## Results

Speed: × real time is shown only for timed runs that held quiet.lock with no non-Diarist process above 5 % SM in any `nvidia-smi pmon -s u -d 1` sample of the run (claude.exe excepted). Every gate-set run was timed, in two blocks under quiet.lock on 27 Oct 2026 (13:34 to 13:39 EN, 13:43 to 13:48 3.5, IST). Surfshark.exe, a desktop application of the owner's, drew 7 to 22 percent SM in single one-second samples about every 30 s, so 8 of the 14 runs were invalid and are left out. They were re-timed with configs `rel_shipped_en_t` and `rel_shipped_n35_t` (copies of the release configs, the same build `ce5b757b`, `--transcribe-only --no-diar-pass`) in one exclusive block on 27 Oct 2026, 17:16:48 to 17:23:01 IST. Two earlier 60 s pmon checks had failed (a browser at 9 to 47 percent SM, then another desktop application at up to 44 percent); the third passed with only claude.exe drawing the GPU (up to 31 percent SM). All 8 runs are valid (`check_pmon.py`: in 367 sample times, no sample of a non-Diarist process other than claude.exe above 5 percent SM). claude.exe, which the rule allows because it draws the Claude desktop app the agents run in, was busier during the block's second half, the three Nemotron 3.5 runs: 7 to 22 samples above 5 percent SM per run (up to 34 percent), against 2 to 4 per EN run. nemo-speech's own SM averaged 54.6 to 58.1 percent there, against 56.7 to 62.0 for the EN runs, at the same total CPU load (9 against 8 percent). Accuracy is unaffected: decoding is deterministic, and every run's words are scored above. The broad and talks sets were run without quiet.lock (accuracy only; decoding is deterministic, research/19 section 2).

### Gate set (six user meetings plus AMI)

Δ: beta.5 minus beta.4, in points, from the rounded values of the two reports.

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | Δ WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | EN | 44 | 9.5 | 0.0 | 9.5 | 9.1 | 0.0 | 266 | 189 | 164 | 0.3 | 13.2 | +0 | 21 | 1 | 81 |
| Aug 2 | 3.5 | 44 | 10.3 | 0.0 | 10.4 | 10.1 | 0.0 | 361 | 125 | 199 | 0.2 | 12.2 | +0 | 28 | 5 | 79 |
| Aug 9 | EN | 44 | 8.0 | -0.1 | 8.0 | 7.4 | -0.2 | 162 | 195 | 128 | 0.3 | 12.7 | +0 | 25 | 4 | 79 |
| Aug 9 | 3.5 | 44 | 8.4 | 0.0 | 8.4 | 8.0 | 0.0 | 236 | 144 | 145 | 0.3 | 11.2 | +0 | 31 | 6 | 72 |
| Sep 7 | EN | 48 | 7.1 | -0.1 | 7.1 | 6.8 | -0.1 | 161 | 188 | 124 | 0.3 | 15.0 | -1 | 50 | 2 | 79 |
| Sep 7 | 3.5 | 48 | 7.5 | 0.0 | 7.5 | 7.2 | 0.0 | 251 | 110 | 141 | 0.2 | 14.6 | -1 | 60 | 4 | 83 |
| Sep 5 | EN | 45 | 4.7 | 0.0 | 4.7 | 4.5 | 0.0 | 92 | 171 | 122 | 0.2 | 7.0 | -1 | 14 | 2 | 82 |
| Sep 5 | 3.5 | 45 | 5.1 | 0.0 | 5.1 | 4.9 | 0.0 | 176 | 89 | 158 | 0.2 | 5.6 | -1 | 9 | 1 | 83 |
| Sep Logistics | EN | 46 | 7.0 | 0.0 | 7.0 | 7.0 | 0.0 | 116 | 198 | 130 | 0.0 | 12.2 | +0 | 10 | 1 | 83 |
| Sep Logistics | 3.5 | 46 | 6.6 | 0.0 | 6.6 | 6.6 | 0.0 | 141 | 147 | 129 | 0.0 | 9.3 | +0 | 14 | 2 | 77 |
| Aug 14 | EN | 77 | 43.8 | -0.1 | 44.4 | 7.1 | -0.1 | 260 | 311 | 291 | 19.4 | 29.3 | +2 | 22 | 9 | 83 |
| Aug 14 | 3.5 | 77 | 42.9 | 0.0 | 43.6 | 7.0 | 0.0 | 318 | 178 | 353 | 19.3 | 27.6 | +2 | 22 | 9 | 78 |
| AMI ES2002a | EN | 21 | 18.0 | 0.0 | 18.4 | 16.6 | 0.0 | 341 | 16 | 77 | 2.3 | 25.4 | +0 | 10 | 4 | 82 |
| AMI ES2002a | 3.5 | 21 | 20.7 | 0.0 | 21.0 | 19.1 | 0.0 | 396 | 19 | 84 | 2.2 | 28.2 | +0 | 12 | 3 | 82 |

### Broad set (folders A, B and C)

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | Δ WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 1 | EN | 47 | 4.1 | 0.0 | 4.1 | 3.8 | 0.0 | 122 | 95 | 90 | 0.2 | 9.0 | +0 | 14 | 1 | – |
| Aug 1 | 3.5 | 47 | 4.5 | 0.0 | 4.5 | 4.4 | 0.0 | 184 | 62 | 114 | 0.1 | 6.9 | +0 | 14 | 3 | – |
| Aug 3 | EN | 52 | 10.0 | 0.0 | 10.0 | 9.8 | 0.0 | 241 | 212 | 274 | 0.1 | 11.5 | +0 | 15 | 3 | – |
| Aug 3 | 3.5 | 52 | 9.9 | -0.1 | 10.0 | 9.8 | -0.1 | 306 | 146 | 279 | 0.1 | 9.7 | +0 | 15 | 4 | – |
| Aug 4 | EN | 46 | 7.6 | -0.1 | 7.7 | 6.9 | -0.1 | 179 | 159 | 95 | 0.4 | 16.9 | +0 | 45 | 2 | – |
| Aug 4 | 3.5 | 46 | 8.3 | 0.0 | 8.3 | 8.2 | 0.0 | 322 | 64 | 128 | 0.1 | 17.3 | +0 | 51 | 5 | – |
| Aug 5 | EN | 43 | 5.0 | -0.2 | 5.0 | 4.9 | -0.2 | 52 | 221 | 141 | 0.1 | 6.0 | +0 | 2 | 1 | – |
| Aug 5 | 3.5 | 43 | 5.0 | 0.0 | 5.1 | 5.0 | 0.0 | 111 | 137 | 174 | 0.1 | 4.3 | +0 | 2 | 1 | – |
| Aug 6 | EN | 65 | 5.2 | 0.0 | 5.2 | 5.1 | 0.0 | 223 | 144 | 252 | 0.1 | 9.0 | +0 | 24 | 3 | – |
| Aug 6 | 3.5 | 65 | 6.6 | 0.0 | 6.6 | 6.6 | 0.0 | 373 | 122 | 312 | 0.0 | 8.3 | +0 | 36 | 3 | – |
| Aug 7 | EN | 55 | 35.0 | -0.3 | 35.2 | 8.0 | -0.4 | 204 | 231 | 203 | 14.2 | 25.6 | +2 | 35 | 1 | – |
| Aug 7 | 3.5 | 55 | 35.5 | 0.0 | 35.9 | 8.2 | 0.0 | 269 | 155 | 229 | 14.7 | 25.6 | +2 | 31 | 2 | – |
| Aug 8 | EN | 46 | 6.8 | 0.0 | 6.8 | 6.7 | 0.0 | 128 | 120 | 210 | 0.0 | 10.4 | +0 | 14 | 1 | – |
| Aug 8 | 3.5 | 46 | 7.5 | 0.0 | 7.5 | 7.4 | 0.0 | 183 | 74 | 250 | 0.0 | 9.0 | +0 | 11 | 1 | – |
| Aug 10 | EN | 47 | 6.4 | 0.0 | 6.4 | 6.2 | 0.0 | 138 | 153 | 126 | 0.1 | 11.2 | +0 | 25 | 1 | – |
| Aug 10 | 3.5 | 47 | 6.6 | 0.0 | 6.6 | 6.6 | 0.0 | 179 | 127 | 133 | 0.1 | 10.0 | +0 | 22 | 3 | – |
| Aug 11 | EN | 43 | 5.3 | -0.4 | 5.3 | 5.3 | -0.4 | 79 | 279 | 68 | 0.0 | 7.9 | +0 | 14 | 0 | – |
| Aug 11 | 3.5 | 43 | 4.7 | 0.0 | 4.7 | 4.7 | 0.0 | 137 | 143 | 103 | 0.0 | 7.5 | +0 | 20 | 0 | – |
| Aug 12 | EN | 46 | 4.3 | -0.1 | 4.4 | 4.3 | 0.0 | 44 | 202 | 59 | 0.1 | 6.7 | +0 | 1 | 0 | – |
| Aug 12 | 3.5 | 46 | 3.0 | 0.0 | 3.0 | 3.0 | 0.0 | 70 | 71 | 73 | 0.0 | 4.8 | +0 | 1 | 0 | – |
| Aug 13 | EN | 47 | 5.7 | 0.0 | 5.8 | 5.6 | 0.0 | 142 | 96 | 123 | 0.1 | 12.5 | +0 | 26 | 0 | – |
| Aug 13 | 3.5 | 47 | 6.0 | 0.0 | 6.0 | 5.9 | 0.0 | 175 | 64 | 142 | 0.0 | 8.8 | +0 | 18 | 1 | – |
| Aug 15 | EN | 44 | 8.0 | -0.4 | 8.1 | 6.9 | -0.4 | 163 | 134 | 123 | 0.6 | 14.2 | +1 | 7 | 2 | – |
| Aug 15 | 3.5 | 44 | 7.4 | 0.0 | 7.4 | 6.6 | 0.0 | 169 | 96 | 135 | 0.4 | 12.0 | +1 | 10 | 2 | – |
| Sep 1 | EN | 35 | 8.8 | 0.0 | 8.8 | 8.7 | 0.0 | 195 | 137 | 119 | 0.1 | 12.7 | +0 | 20 | 1 | – |
| Sep 1 | 3.5 | 35 | 9.6 | 0.0 | 9.6 | 9.5 | 0.0 | 257 | 93 | 143 | 0.1 | 11.3 | +0 | 23 | 2 | – |
| Sep 2 | EN | 42 | 6.5 | -0.1 | 6.5 | 6.3 | -0.1 | 139 | 112 | 199 | 0.1 | 10.3 | +0 | 14 | 0 | – |
| Sep 2 | 3.5 | 42 | 7.1 | 0.0 | 7.1 | 6.9 | 0.0 | 163 | 93 | 241 | 0.1 | 8.7 | +0 | 17 | 1 | – |
| Sep 3 | EN | 43 | 6.9 | 0.0 | 6.9 | 6.8 | 0.0 | 230 | 120 | 198 | 0.1 | 10.0 | +0 | 32 | 3 | – |
| Sep 3 | 3.5 | 43 | 8.3 | 0.0 | 8.3 | 8.3 | 0.0 | 300 | 95 | 275 | 0.1 | 10.3 | +0 | 29 | 2 | – |
| Sep 4 | EN | 43 | 8.2 | -0.2 | 8.2 | 8.0 | -0.2 | 226 | 173 | 158 | 0.2 | 10.8 | +0 | 21 | 0 | – |
| Sep 4 | 3.5 | 43 | 9.0 | 0.0 | 9.0 | 8.8 | 0.0 | 297 | 131 | 185 | 0.1 | 10.4 | +0 | 27 | 2 | – |
| Sep 6 | EN | 47 | 4.8 | 0.0 | 4.9 | 4.6 | 0.0 | 181 | 118 | 113 | 0.2 | 12.3 | +0 | 40 | 2 | – |
| Sep 6 | 3.5 | 47 | 5.9 | 0.0 | 6.0 | 5.7 | 0.0 | 305 | 79 | 127 | 0.2 | 12.8 | +0 | 43 | 2 | – |
| Jul 1 | EN | 28 | 9.6 | -0.1 | 9.6 | 9.4 | -0.1 | 174 | 91 | 112 | 0.2 | 13.1 | +0 | 23 | 3 | – |
| Jul 1 | 3.5 | 28 | 9.5 | 0.0 | 9.5 | 9.4 | 0.0 | 217 | 48 | 110 | 0.2 | 14.4 | +0 | 22 | 1 | – |
| Jul 2 | EN | 40 | 6.3 | -0.4 | 6.3 | 6.2 | -0.5 | 157 | 161 | 108 | 0.0 | 11.4 | +0 | 43 | 3 | – |
| Jul 2 | 3.5 | 40 | 6.2 | 0.0 | 6.3 | 6.2 | 0.0 | 209 | 89 | 127 | 0.0 | 12.3 | +0 | 41 | 4 | – |
| Jul 3 | EN | 63 | 7.9 | -1.2 | 7.9 | 7.7 | -1.2 | 225 | 442 | 273 | 0.1 | 9.1 | +0 | 34 | 0 | – |
| Jul 3 | 3.5 | 63 | 10.0 | 0.0 | 10.0 | 9.8 | 0.0 | 425 | 309 | 467 | 0.2 | 9.5 | +0 | 44 | 1 | – |
| Jul 4 | EN | 40 | 5.4 | 0.0 | 5.4 | 5.1 | 0.0 | 104 | 111 | 106 | 0.2 | 12.2 | +0 | 21 | 1 | – |
| Jul 4 | 3.5 | 40 | 5.9 | 0.0 | 5.9 | 5.8 | 0.0 | 142 | 107 | 120 | 0.0 | 9.6 | +0 | 25 | 1 | – |
| Jul 5 | EN | 52 | 11.7 | -0.4 | 11.8 | 11.7 | -0.4 | 195 | 380 | 291 | 0.1 | 10.1 | +0 | 32 | 2 | – |
| Jul 5 | 3.5 | 52 | 11.8 | 0.0 | 11.8 | 11.8 | 0.0 | 284 | 259 | 328 | 0.0 | 9.9 | +0 | 40 | 5 | – |

### Talks (folder D)

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | Δ WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Talk 1 | EN | 53 | 6.6 | 0.0 | 6.6 | 6.3 | 0.0 | 140 | 143 | 179 | 0.2 | 9.9 | +0 | 15 | 3 | – |
| Talk 1 | 3.5 | 53 | 6.7 | 0.0 | 6.7 | 6.5 | 0.0 | 153 | 111 | 218 | 0.1 | 7.8 | +0 | 13 | 3 | – |
| Talk 2 | EN | 45 | 7.1 | -0.1 | 7.1 | 6.9 | -0.1 | 138 | 171 | 205 | 0.1 | 8.4 | -1 | 23 | 0 | – |
| Talk 2 | 3.5 | 45 | 7.4 | 0.0 | 7.4 | 7.3 | 0.0 | 184 | 116 | 241 | 0.1 | 7.7 | -1 | 22 | 0 | – |
| Talk 3 | EN | 52 | 12.6 | -0.2 | 12.6 | 12.1 | -0.2 | 343 | 321 | 236 | 0.4 | 11.9 | -1 | 49 | 4 | – |
| Talk 3 | 3.5 | 52 | 12.7 | 0.0 | 12.7 | 12.5 | 0.0 | 402 | 213 | 314 | 0.2 | 12.0 | +0 | 46 | 4 | – |
| Talk 4 | EN | 67 | 30.1 | -0.1 | 30.1 | 5.5 | -0.2 | 163 | 159 | 212 | 12.7 | 21.6 | +1 | 33 | 2 | – |
| Talk 4 | 3.5 | 67 | 30.6 | 0.0 | 30.6 | 6.2 | 0.0 | 230 | 87 | 285 | 12.8 | 20.0 | +1 | 25 | 1 | – |
| Talk 5 | EN | 37 | 5.3 | 0.0 | 5.3 | 5.1 | 0.0 | 79 | 82 | 157 | 0.1 | 9.0 | -1 | 4 | 0 | – |
| Talk 5 | 3.5 | 37 | 6.8 | 0.0 | 6.8 | 6.6 | 0.0 | 125 | 62 | 225 | 0.1 | 6.5 | -1 | 5 | 0 | – |
| Talk 6 | EN | 45 | 14.6 | -0.2 | 14.6 | 14.5 | -0.2 | 167 | 221 | 491 | 0.1 | 9.7 | +0 | 10 | 0 | – |
| Talk 6 | 3.5 | 45 | 15.5 | 0.0 | 15.5 | 15.4 | 0.0 | 224 | 185 | 523 | 0.1 | 9.2 | +0 | 15 | 2 | – |
| Talk 7 | EN | 58 | 14.8 | 0.0 | 14.8 | 10.5 | 0.0 | 295 | 565 | 209 | 2.4 | 15.2 | +3 | 38 | 3 | – |
| Talk 7 | 3.5 | 58 | 15.2 | 0.0 | 15.3 | 10.7 | 0.0 | 464 | 333 | 298 | 2.6 | 15.7 | +4 | 45 | 2 | – |

### Pooled

| Set | Model | Sessions | Min | cpWER beta.4 | cpWER beta.5 | Change | WER beta.4 | WER beta.5 | del | ins | sub | Gaps ≥ 2 s beta.4 / beta.5 | Gaps ≥ 5 s beta.4 / beta.5 | Sum of abs count err beta.4 / beta.5 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | EN | 7 | 324 | 16.7 | 16.6 | -0.1 | 7.5 | 7.4 | 1398 | 1268 | 1036 | 152 / 152 | 23 / 23 | 4 / 4 |
| gate | 3.5 | 7 | 324 | 16.8 | 16.8 | 0.0 | 7.8 | 7.8 | 1879 | 812 | 1209 | 176 / 176 | 30 / 30 | 4 / 4 |
| broad | EN | 22 | 1017 | 8.3 | 8.1 | -0.2 | 6.8 | 6.6 | 3541 | 3891 | 3441 | 502 / 502 | 30 / 30 | 3 / 3 |
| broad | 3.5 | 22 | 1017 | 8.6 | 8.6 | 0.0 | 7.2 | 7.2 | 5077 | 2564 | 4195 | 542 / 542 | 46 / 46 | 3 / 3 |
| talks | EN | 7 | 356 | 14.0 | 13.9 | -0.1 | 8.7 | 8.6 | 1325 | 1662 | 1689 | 172 / 172 | 12 / 12 | 7 / 7 |
| talks | 3.5 | 7 | 356 | 14.4 | 14.4 | 0.0 | 9.2 | 9.2 | 1782 | 1107 | 2104 | 171 / 171 | 12 / 12 | 7 / 7 |
| all Zoom | EN | 35 | 1676 | 10.9 | 10.8 | -0.1 | 7.2 | 7.1 | 5923 | 6805 | 6089 | 816 / 816 | 61 / 61 | 14 / 14 |
| all Zoom | 3.5 | 35 | 1676 | 11.2 | 11.2 | 0.0 | 7.6 | 7.6 | 8342 | 4464 | 7424 | 877 / 877 | 85 / 85 | 14 / 14 |
| all | EN | 36 | 1697 | 11.0 | 10.9 | -0.1 | 7.3 | 7.1 | 6264 | 6821 | 6166 | 826 / 826 | 65 / 65 | 14 / 14 |
| all | 3.5 | 36 | 1697 | 11.3 | 11.3 | 0.0 | 7.7 | 7.7 | 8738 | 4483 | 7508 | 889 / 889 | 88 / 88 | 14 / 14 |

### Where the words changed

Every change in words comes from the loop collapse. With Nemotron EN it collapsed 61 bursts, 438 words, on 23 of the 36 sessions; cpWER falls on 21 of them and is unchanged on the other two and on the 13 sessions without a burst. With Nemotron 3.5 it collapsed 3 bursts, 10 words, on 2 sessions, and cpWER falls on both. No session's cpWER or tcpWER rises with either model. The one score that rises anywhere is AMI's plain WER with EN, by one edit (433 to 434: the collapse took 2 words there; deletions go 339 to 341 and substitutions 78 to 77; cpWER and tcpWER are unchanged, WER rounds to 16.6 both times). On every session without a collapsed word every word score (cpWER, tcpWER, WER with its del, ins and sub, WDER) equals the baseline's. These are the loop-collapse note's figures (`docs/eval/2026-10-26-loop-collapse.md`: 438 words, cpWER lower on 21 of 36, mean 10.38 → 10.24 with EN), now from fresh GPU runs through the app's own pipeline. Speaker counts are unchanged on all 72 runs.

### Punctuation

Punctuation ran on all 72 runs with 0 rejected words. Its cost is the step's wall time at the default 4 intra-op threads (`engine_info.punctuation.wall_s`); the gate set's runs held quiet.lock, the others did not.

| Set | Model | Sessions | Hours | Words | Marks added | Capitals raised | Rejected words | Punctuation s | s per hour |
|---|---|---|---|---|---|---|---|---|---|
| gate (timed) | EN | 7 | 5.4 | 49,832 | 7,372 | 3,327 | 0 | 29.1 | 5.4 |
| gate (timed) | 3.5 | 7 | 5.4 | 48,499 | 6,424 | 3,210 | 0 | 29.2 | 5.4 |
| broad | EN | 22 | 16.9 | 165,170 | 23,311 | 9,380 | 0 | 88.7 | 5.2 |
| broad | 3.5 | 22 | 16.9 | 160,469 | 19,750 | 8,957 | 0 | 99.5 | 5.9 |
| talks | EN | 7 | 5.9 | 55,263 | 7,716 | 3,142 | 0 | 32.6 | 5.5 |
| talks | 3.5 | 7 | 5.9 | 53,654 | 6,697 | 3,063 | 0 | 30.9 | 5.2 |
| all | EN | 36 | 28.3 | 270,265 | 38,399 | 15,849 | 0 | 150.4 | 5.3 |
| all | 3.5 | 36 | 28.3 | 262,622 | 32,871 | 15,230 | 0 | 159.6 | 5.6 |

Per session the step takes 3.2 to 8.1 s per hour of audio, the short sessions highest (loading the 34 MB tokenizer and the model is a fixed cost of about a second). Plan 16's bar is 15 s per hour (section 3.12 rule 2).

### Speed, gate set

The transcribe step's wall time, model load included, against the baseline's valid timings. Every gate run now has a valid timing: the 6 of the first blocks (13:34 to 13:48, "first") and the 8 of the timed block (17:16 to 17:23, "block").

| Session | Model | Min | beta.4 s | beta.4 × real time | beta.5 s | beta.5 × real time | Change in × real time | Timing |
|---|---|---|---|---|---|---|---|---|
| Aug 2 | EN | 44 | 36.7 | 71 | 32.4 | 81 | +13.3 % | block |
| Aug 9 | EN | 44 | 34.1 | 78 | 33.9 | 79 | +0.6 % | block |
| Sep 7 | EN | 48 | 42.7 | 67 | 35.9 | 79 | +18.9 % | block |
| Sep 5 | EN | 45 | 33.2 | 81 | 32.9 | 82 | +0.9 % | block |
| Sep Logistics | EN | 46 | 33.0 | 83 | 33.1 | 83 | -0.3 % | first |
| Aug 14 | EN | 77 | 56.1 | 82 | 55.4 | 83 | +1.3 % | block |
| AMI ES2002a | EN | 21 | 15.4 | 83 | 15.6 | 82 | -1.3 % | first |
| Aug 2 | 3.5 | 44 | 31.8 | 82 | 32.9 | 79 | -3.3 % | first |
| Aug 9 | 3.5 | 44 | 32.4 | 82 | 36.9 | 72 | -12.2 % | block |
| Sep 7 | 3.5 | 48 | 34.2 | 83 | 34.5 | 83 | -0.9 % | first |
| Sep 5 | 3.5 | 45 | 32.3 | 83 | 32.6 | 83 | -0.9 % | first |
| Sep Logistics | 3.5 | 46 | 32.7 | 84 | 35.6 | 77 | -8.1 % | block |
| Aug 14 | 3.5 | 77 | 54.7 | 84 | 59.3 | 78 | -7.8 % | block |
| AMI ES2002a | 3.5 | 21 | 15.8 | 81 | 15.5 | 82 | +1.9 % | first |
| gate pooled | EN | 324 | 251.2 | 77.4 | 239.2 | 81.3 | +5.0 % | |
| gate pooled | 3.5 | 324 | 233.9 | 83.1 | 247.3 | 78.6 | -5.4 % | |

Reading it:

- EN is faster than beta.4 on the pooled set, +5.0 percent. Most of that comes from two beta.4 runs that were slow for no reason visible in the words: Aug 2 at 71 and Sep 7 at 67 times real time, against 78 to 83 for every other EN run of either release. Without them the other five sessions pool at +0.5 percent.
- Nemotron 3.5 is 5.4 percent slower pooled as measured, 0.4 points beyond the bar; the bar is recorded as passing for the reasons below.
  - The four runs of the first block are within -3.3 to +1.9 percent of beta.4; the three of the timed block are 7.8 to 12.2 percent slower.
  - The same build timed those three sessions at 13:44 to 13:48 in 32.0, 33.2 and 55.2 s, within -1.5 to +1.3 percent of beta.4's 32.4, 32.7 and 54.7. Those runs were left out only for Surfshark's single samples.
  - The block's second half is where claude.exe drew the GPU most (above).
  - Nothing in the measured step changed between the releases: `nemo-speech transcribe` is the same executable, with the same models and the same command line (Setup), so a code cause is not expected.
- Accepted by the owner on 27 Oct 2026; re-timed in beta.6: the three 3.5 runs (Aug 9, Sep Logistics, Aug 14) with the Claude desktop app idle (minimised, no agent output streaming), about 3 minutes of GPU under quiet.lock.

### Release bars

| Bar | Result | |
|---|---|---|
| Pooled cpWER per model not worse than beta.4 | EN 11.01 → 10.85 (all 36), 3.5 11.30 → 11.30; every set lower or equal (table above) | met |
| No session worse by more than 0.5 cpWER points (unless the words show why) | no session worse at all, with either model | met |
| No new gap of 5 s or more | 0 new gaps of any length with either model, and none gone | met |
| Speed within 5 % of beta.4 for the same measurement | all 14 gate runs timed and valid: pooled × real time +5.0 percent EN (faster), -5.4 percent Nemotron 3.5. The 3.5 shortfall is the three runs of the timed block, which overlapped the Claude desktop app's GPU use; the same build did those sessions within -1.5 to +1.3 percent of beta.4 earlier the same day (speed table) | met (both models). 3.5's -5.4 percent as measured comes from the three late runs, which overlapped claude.exe at up to 34 percent SM; the same build timed those sessions at -1.5 to +1.3 percent at 13:44 to 13:48, and the timed step is the same executable, models and command line as beta.4. Accepted by the owner on 27 Oct 2026; re-timed in beta.6 |

## Integration checks on the same tree

The ignored GPU test suites, release build of `d9a0666` (the exact re-run test at `63b0bc2`), each through the shared GPU queue: `acceptance_gpu` (the 77-minute file, word count within 0.27 percent, 160 s), `live_ws_gpu` at the shipped live lookahead 6 (first-word latency median 0.52 s, 33 s), `live_carry_repro` at lookahead 6 (180 s of meeting 16 replayed, every name reached a post-pass speaker, 188 s), `echo_sample` on meetings 4, 5 and 12 to 21 (every post-pass and live row equals `docs/eval/2026-10-26-echo-owner-level.md` section 6; meeting 21 at the silent tier: 77 kept, 42 far-active cut, `owner_source` implausible, raw echo share 99.93 percent), `aec_postpass` (meeting 12's real post-pass: 1,578 kept, owner's words 96.65 percent by the labels, 98.29 fair; the spliced set 8.1 percent, forced unknown 7.9) and `echo_fleurs` (20 minutes: 12.69 percent, 0 echo words; the headphone control identical), all with the loop collapse off in their test stores as before. All pass. After chunk I's talking-owner guard was merged (`6645724`: the silent tier also needs the owner's words to be missing, `owner_high`, N = 10, plan 16 section 12.8), `echo_sample` on the same meetings, `aec_postpass`, `echo_fleurs` and the meeting 17 exact re-run ran again on a release build of that tip: every count and table cell equals the `d9a0666` run (880 cells of `aec_postpass`, 440 of `echo_sample`, 94 of `echo_fleurs`); only the new `owner_high` values and wall times differ. Meeting 21 stays at the silent tier with `owner_high` 5: 77 kept, 42 far-active cut.

The Process again exact test (`live::rerun::tests::a_stored_meeting_runs_again_from_its_flac_exactly`, collapse off, no punctuation) on copies of meetings 17, 18 and 20 (17 again on `6645724`): words identical (1,191, 2,192 and 11,596 of as many), `echo_dropped` identical (3, 12 and 1,381 words), every key of the stored `engine_info.echo` identical, `playback.flac` and `loopback.gain` byte-identical, the decoded WAVs gone, the new run current and the earlier and live runs kept. The stored runs predate chunk I, so the test now leaves out of the new run's summary only these fields, and only when the stored run lacks them: `owner_low`, `owner_source`, `owner_words`, `owner_high`, and under `transfer` the keys `floor`, `floor_rule`, `raw_echo_share`, `raw_floor_db` and `raw_owner_db` (plan 16 12.5: identical apart from the new fields). Any other new key or changed value still fails it.

## Reproducing

From `tools/eval` with the README setup, `sessions.toml` holding the 36 sessions (private paths), and these environment variables:

```sh
export DIARIST_EVAL_ENGINE_DIR=<copy of the app's engine>/nemo-speech-0.2.0-windows-x86_64-cuda
export DIARIST_EVAL_MODELS_DIR=<models folder: the three manifest GGUFs and the punctuation entry's seven files under its dir>
export DIARIST_EVAL_CLI=<diarist-cli.exe built from the beta.5 tree>
# configs rel_shipped_en and rel_shipped_n35: copies of configs.toml's shipped_en and
# shipped_n35 with `punctuation = true` (the app's default); the loop collapse is on by default
for c in rel_shipped_en rel_shipped_n35; do
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions <ids> --transcribe-only --no-diar-pass
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions <ids> --metrics-only --no-report
done
# the timed re-runs: configs rel_shipped_en_t and rel_shipped_n35_t (the same two, renamed so
# they transcribe again) with --transcribe-only --no-diar-pass under quiet.lock, with
# nvidia-smi pmon -s u -d 1 running; validity per run from that log (no non-Diarist
# process other than claude.exe above 5 % SM)
# WER, gaps and the tables: research/19's analyze.py over these runs and the baseline's
```

Keep the engine and models folders on a short path: from a folder of about 180 characters the runtime's DLL paths pass 260 and `nemo-speech` exits with 0xc0000106 (STATUS_NAME_TOO_LONG) before it loads anything.
