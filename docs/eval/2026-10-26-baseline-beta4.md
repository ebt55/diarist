# Eval: baseline for 0.1.0-beta.4 (26 Oct 2026)

The reference every later change compares against: the shipped configuration of Diarist 0.1.0-beta.4 on all 35 of the owner's Zoom recordings plus AMI ES2002a. Shipped configuration only: NVIDIA's published q8_0 GGUFs at the manifest revisions, nemo-speech 0.2.0, the app's file-pass settings (lookahead 13, `v3-offline` diarizer preset, `--stream --endpointing --diarize`). Both ASR models: Nemotron EN (the default) and Nemotron 3.5. Generated from the runs of `docs/research/19-q8-pointwise-respin.md` (configs `rs_en_shipped` and `rs_n35_shipped`); no extra GPU passes. Numbers are percentages unless a column says otherwise.

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88 |
| diarist-cli | 0.1.0-beta.4 built from `fc597ba` (the beta.4 tree), SHA-256 `d736a0ae99eb074f086c9ca338089da6ef1a8d7033561fbe93c89a9e168657fe` |
| Engine | nemo-speech 0.2.0 (`engine_version` in every run.json), package `nemo-speech-0.2.0-windows-x86_64-cuda`, a byte copy of the app's installed engine; `nemo-speech.exe` SHA-256 `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272` |
| ASR, EN | `asr_file` `nemotron-speech-streaming-en-0.6b.q8_0`, `nvidia/nemotron-speech-streaming-en-0.6b` @ `ebe59e5a8171`, SHA-256 `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d` (equal to `manifests/models.json`) |
| ASR, 3.5 | `asr_file` `nemotron-3.5-asr-streaming-0.6b.q8_0`, `nvidia/nemotron-3.5-asr-streaming-0.6b` @ `1c8deaecc64b`, SHA-256 `a5c435f294eea8f88ce68dd27b8c3bfea7f777cb2fbba04fcd30eaa555f429ae` |
| Diarizer | `diarizer_file` `Nemotron-3-Diarization.q8_0`, `nvidia/Nemotron-3-Diarization` @ `f667ed73aee5`, SHA-256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` |
| Settings recorded in run.json | `right_context` 13, `diar_preset` v3-offline, `device` cuda:0, `language` en, `engine_args` empty (the app's own defaults from the manifest) |
| Engine command (from engine.log) | `nemo-speech transcribe <wav> --model <asr.gguf> [--language en, 3.5 only] --stream --endpointing --asr.streaming.rnnt_right_context 13 --diarize --diar-model <Nemotron-3-Diarization.q8_0.gguf> --asr.diar.preset v3-offline --device cuda:0 --json --output <out> --force` |
| Text normaliser | whisper EnglishTextNormalizer per token, filler removal, ok to okay (the harness's) |
| Determinism | Re-running the same configuration gives identical words, times and speaker tags (checked on AMI, Aug 2, Sep 5 and Talk 1 for both models; research/19 section 2) |

## Data

| Group | Sessions | Minutes | What it is |
|---|---|---|---|
| Gate set | Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14, AMI ES2002a | 324 | docs/05's established set; earlier reports use it, so it stays first and unchanged |
| Broad | Aug 1, 3 to 8, 10 to 13, 15; Sep 1 to 4, 6; Jul 1 to 5 | 1,017 | every other recording of the owner's three meeting folders (22 recordings) |
| Talks | Talk 1 to Talk 7 | 356 | seven Zoom webinars, 2 to 5 speakers |

Labels follow docs/05's shorthand: month plus the number that prefixes the recording's folder (not the recording date; several recordings share a date). Duplicate check (run by the coordinating agent, 26 Oct 2026): SHA-256 of all 35 m4a files, the Zoom recording IDs, and an 8-word overlap test between transcripts found no duplicates.

References: Zoom's own `transcript.vtt` (Zoom's speech recognition with per-participant speaker labels), converted by `zoom_vtt_to_ref.py`; AMI's word-level manual annotation. On the Zoom sessions cpWER, tcpWER and WER therefore mix in Zoom's errors (docs/05); word accuracy proper is judged on AMI only. Speaker identity per Zoom cue is near ground truth, which is what WDER and the speaker count use. Aug 14 is the in-room stress session (room devices under one label).

## Metrics

- cpWER, tcpWER (collar 5 s), WDER and DER at collar 0.25 as in every docs/eval report (`tools/eval/metrics.py`).
- WER: speaker-agnostic, time-ordered normalised words, Levenshtein; deletions, insertions and substitutions are its counts.
- Count err: transcript speakers minus reference speakers.
- Gaps: stretches of at least 2 s (and of at least 5 s) without a hypothesis word in which the reference has at least 3 words, counted with a 0.5 s margin at both ends (research/19 section 0).
- × real time: audio duration over the `nemo-speech transcribe` step's wall time (`asr_wall_s`, model load included), taken under the shared quiet.lock (no other heavy work on the PC).

## Results

Speed: × real time comes from the run itself when that run held quiet.lock and no other heavy job overlapped it (the gate set, Talk 2 to 7, Aug 1, 3 and 5, and 3.5 on Aug 4). Otherwise it comes from a timing-only re-run of the same configuration in an exclusive window, checked with `nvidia-smi pmon` (no other process above 5 % SM use; the idle desktop app excepted). Talk 1 and Aug 4 first ran while another agent's build overlapped; those were re-timed. "pending" marks sessions whose timing re-run is still owed: the window was interrupted at 02:50 on 27 Oct by another GPU job. The words do not depend on load, so every accuracy column is final. During the window ComfyUI held about 2.5 GB of VRAM idle. Pooled × real time counts only the timed sessions and says how many.

### Gate set (six user meetings plus AMI)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | EN | 44 | 9.5 | 9.5 | 9.1 | 266 | 189 | 164 | 0.3 | 7.4 | +0 | 21 | 1 | 71 |
| Aug 2 | 3.5 | 44 | 10.3 | 10.4 | 10.1 | 361 | 125 | 199 | 0.2 | 7.5 | +0 | 28 | 5 | 82 |
| Aug 9 | EN | 44 | 8.1 | 8.2 | 7.6 | 162 | 205 | 128 | 0.3 | 9.0 | +0 | 25 | 4 | 78 |
| Aug 9 | 3.5 | 44 | 8.4 | 8.4 | 8.0 | 236 | 144 | 145 | 0.3 | 8.9 | +0 | 31 | 6 | 82 |
| Sep 7 | EN | 48 | 7.2 | 7.2 | 6.9 | 161 | 194 | 124 | 0.3 | 11.4 | -1 | 50 | 2 | 67 |
| Sep 7 | 3.5 | 48 | 7.5 | 7.5 | 7.2 | 251 | 110 | 141 | 0.2 | 11.9 | -1 | 60 | 4 | 83 |
| Sep 5 | EN | 45 | 4.7 | 4.7 | 4.5 | 92 | 171 | 122 | 0.2 | 3.9 | -1 | 14 | 2 | 81 |
| Sep 5 | 3.5 | 45 | 5.1 | 5.1 | 4.9 | 176 | 89 | 158 | 0.2 | 3.6 | -1 | 9 | 1 | 83 |
| Sep Logistics | EN | 46 | 7.0 | 7.0 | 7.0 | 116 | 198 | 130 | 0.0 | 6.5 | +0 | 10 | 1 | 83 |
| Sep Logistics | 3.5 | 46 | 6.6 | 6.6 | 6.6 | 141 | 147 | 129 | 0.0 | 5.2 | +0 | 14 | 2 | 84 |
| Aug 14 | EN | 77 | 43.9 | 44.5 | 7.2 | 260 | 329 | 291 | 19.4 | 25.7 | +2 | 22 | 9 | 82 |
| Aug 14 | 3.5 | 77 | 42.9 | 43.6 | 7.0 | 318 | 178 | 353 | 19.3 | 25.0 | +2 | 22 | 9 | 84 |
| AMI ES2002a | EN | 21 | 18.0 | 18.4 | 16.6 | 339 | 16 | 78 | 2.3 | 19.9 | +0 | 10 | 4 | 83 |
| AMI ES2002a | 3.5 | 21 | 20.7 | 21.0 | 19.1 | 396 | 19 | 84 | 2.2 | 24.7 | +0 | 12 | 3 | 81 |

### Broad set (folders A, B and C)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 1 | EN | 47 | 4.1 | 4.1 | 3.8 | 122 | 95 | 90 | 0.2 | 5.4 | +0 | 14 | 1 | 81 |
| Aug 1 | 3.5 | 47 | 4.5 | 4.5 | 4.4 | 184 | 62 | 114 | 0.1 | 4.8 | +0 | 14 | 3 | 85 |
| Aug 3 | EN | 52 | 10.0 | 10.0 | 9.8 | 241 | 215 | 274 | 0.1 | 7.2 | +0 | 15 | 3 | 80 |
| Aug 3 | 3.5 | 52 | 10.0 | 10.0 | 9.9 | 306 | 149 | 279 | 0.1 | 6.2 | +0 | 15 | 4 | 84 |
| Aug 4 | EN | 46 | 7.7 | 7.8 | 7.0 | 178 | 165 | 96 | 0.4 | 13.4 | +0 | 45 | 2 | 74 |
| Aug 4 | 3.5 | 46 | 8.3 | 8.3 | 8.2 | 322 | 64 | 128 | 0.1 | 15.0 | +0 | 51 | 5 | 79 |
| Aug 5 | EN | 43 | 5.2 | 5.2 | 5.1 | 52 | 239 | 141 | 0.1 | 2.4 | +0 | 2 | 1 | 85 |
| Aug 5 | 3.5 | 43 | 5.0 | 5.1 | 5.0 | 111 | 137 | 174 | 0.1 | 2.3 | +0 | 2 | 1 | 86 |
| Aug 6 | EN | 65 | 5.2 | 5.2 | 5.1 | 223 | 144 | 252 | 0.1 | 4.9 | +0 | 24 | 3 | 77 |
| Aug 6 | 3.5 | 65 | 6.6 | 6.6 | 6.6 | 373 | 122 | 312 | 0.0 | 5.5 | +0 | 36 | 3 | pending |
| Aug 7 | EN | 55 | 35.3 | 35.5 | 8.4 | 202 | 256 | 205 | 14.2 | 21.5 | +2 | 35 | 1 | 72 |
| Aug 7 | 3.5 | 55 | 35.5 | 35.9 | 8.2 | 269 | 155 | 229 | 14.7 | 22.3 | +2 | 31 | 2 | pending |
| Aug 8 | EN | 46 | 6.8 | 6.8 | 6.7 | 128 | 120 | 210 | 0.0 | 7.4 | +0 | 14 | 1 | 78 |
| Aug 8 | 3.5 | 46 | 7.5 | 7.5 | 7.4 | 183 | 74 | 250 | 0.0 | 6.6 | +0 | 11 | 1 | pending |
| Aug 10 | EN | 47 | 6.4 | 6.4 | 6.2 | 138 | 153 | 126 | 0.1 | 7.4 | +0 | 25 | 1 | 77 |
| Aug 10 | 3.5 | 47 | 6.6 | 6.6 | 6.6 | 179 | 127 | 133 | 0.1 | 7.4 | +0 | 22 | 3 | pending |
| Aug 11 | EN | 43 | 5.7 | 5.7 | 5.7 | 77 | 315 | 70 | 0.0 | 4.2 | +0 | 14 | 0 | 82 |
| Aug 11 | 3.5 | 43 | 4.7 | 4.7 | 4.7 | 137 | 143 | 103 | 0.0 | 4.7 | +0 | 20 | 0 | pending |
| Aug 12 | EN | 46 | 4.4 | 4.4 | 4.3 | 44 | 204 | 59 | 0.1 | 3.0 | +0 | 1 | 0 | 73 |
| Aug 12 | 3.5 | 46 | 3.0 | 3.0 | 3.0 | 70 | 71 | 73 | 0.0 | 2.4 | +0 | 1 | 0 | pending |
| Aug 13 | EN | 47 | 5.7 | 5.8 | 5.6 | 142 | 96 | 123 | 0.1 | 10.7 | +0 | 26 | 0 | 69 |
| Aug 13 | 3.5 | 47 | 6.0 | 6.0 | 5.9 | 175 | 64 | 142 | 0.0 | 7.7 | +0 | 18 | 1 | pending |
| Aug 15 | EN | 44 | 8.4 | 8.5 | 7.3 | 161 | 159 | 124 | 0.6 | 7.8 | +1 | 7 | 2 | 79 |
| Aug 15 | 3.5 | 44 | 7.4 | 7.4 | 6.6 | 169 | 96 | 135 | 0.4 | 6.6 | +1 | 10 | 2 | pending |
| Sep 1 | EN | 35 | 8.8 | 8.8 | 8.7 | 195 | 137 | 119 | 0.1 | 7.2 | +0 | 20 | 1 | 77 |
| Sep 1 | 3.5 | 35 | 9.6 | 9.6 | 9.5 | 257 | 93 | 143 | 0.1 | 7.3 | +0 | 23 | 2 | pending |
| Sep 2 | EN | 42 | 6.6 | 6.6 | 6.4 | 139 | 120 | 199 | 0.1 | 5.4 | +0 | 14 | 0 | pending |
| Sep 2 | 3.5 | 42 | 7.1 | 7.1 | 6.9 | 163 | 93 | 241 | 0.1 | 5.4 | +0 | 17 | 1 | pending |
| Sep 3 | EN | 43 | 6.9 | 6.9 | 6.8 | 230 | 123 | 198 | 0.1 | 5.1 | +0 | 32 | 3 | pending |
| Sep 3 | 3.5 | 43 | 8.3 | 8.3 | 8.3 | 300 | 95 | 275 | 0.1 | 6.3 | +0 | 29 | 2 | pending |
| Sep 4 | EN | 43 | 8.4 | 8.4 | 8.2 | 225 | 188 | 159 | 0.2 | 7.0 | +0 | 21 | 0 | pending |
| Sep 4 | 3.5 | 43 | 9.0 | 9.0 | 8.8 | 297 | 131 | 185 | 0.1 | 7.7 | +0 | 27 | 2 | pending |
| Sep 6 | EN | 47 | 4.8 | 4.9 | 4.6 | 181 | 118 | 113 | 0.2 | 6.4 | +0 | 40 | 2 | pending |
| Sep 6 | 3.5 | 47 | 5.9 | 6.0 | 5.7 | 305 | 79 | 127 | 0.2 | 8.4 | +0 | 43 | 2 | pending |
| Jul 1 | EN | 28 | 9.7 | 9.7 | 9.5 | 174 | 93 | 112 | 0.2 | 8.1 | +0 | 23 | 3 | pending |
| Jul 1 | 3.5 | 28 | 9.5 | 9.5 | 9.4 | 217 | 48 | 110 | 0.2 | 10.0 | +0 | 22 | 1 | pending |
| Jul 2 | EN | 40 | 6.7 | 6.7 | 6.7 | 157 | 191 | 108 | 0.0 | 7.8 | +0 | 43 | 3 | pending |
| Jul 2 | 3.5 | 40 | 6.2 | 6.3 | 6.2 | 209 | 89 | 127 | 0.0 | 9.6 | +0 | 41 | 4 | pending |
| Jul 3 | EN | 63 | 9.1 | 9.1 | 8.9 | 225 | 587 | 273 | 0.1 | 4.5 | +0 | 34 | 0 | pending |
| Jul 3 | 3.5 | 63 | 10.0 | 10.0 | 9.8 | 425 | 309 | 467 | 0.2 | 5.8 | +0 | 44 | 1 | pending |
| Jul 4 | EN | 40 | 5.4 | 5.4 | 5.1 | 104 | 111 | 106 | 0.2 | 7.5 | +0 | 21 | 1 | pending |
| Jul 4 | 3.5 | 40 | 5.9 | 5.9 | 5.8 | 142 | 107 | 120 | 0.0 | 7.0 | +0 | 25 | 1 | pending |
| Jul 5 | EN | 52 | 12.1 | 12.2 | 12.1 | 195 | 413 | 291 | 0.1 | 7.2 | +0 | 32 | 2 | pending |
| Jul 5 | 3.5 | 52 | 11.8 | 11.9 | 11.8 | 283 | 265 | 328 | 0.0 | 7.7 | +0 | 40 | 5 | pending |

### Talks (folder D)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Talk 1 | EN | 53 | 6.6 | 6.6 | 6.3 | 140 | 143 | 179 | 0.2 | 6.9 | +0 | 15 | 3 | 72 |
| Talk 1 | 3.5 | 53 | 6.7 | 6.7 | 6.5 | 153 | 111 | 218 | 0.1 | 5.4 | +0 | 13 | 3 | pending |
| Talk 2 | EN | 45 | 7.2 | 7.2 | 7.0 | 138 | 180 | 205 | 0.1 | 4.2 | -1 | 23 | 0 | 82 |
| Talk 2 | 3.5 | 45 | 7.4 | 7.4 | 7.3 | 184 | 116 | 241 | 0.1 | 4.2 | -1 | 22 | 0 | 83 |
| Talk 3 | EN | 52 | 12.8 | 12.8 | 12.3 | 343 | 337 | 236 | 0.4 | 9.1 | -1 | 49 | 4 | 66 |
| Talk 3 | 3.5 | 52 | 12.7 | 12.7 | 12.5 | 402 | 213 | 314 | 0.2 | 9.8 | +0 | 46 | 4 | 81 |
| Talk 4 | EN | 67 | 30.2 | 30.3 | 5.7 | 163 | 171 | 212 | 12.7 | 19.0 | +1 | 33 | 2 | 63 |
| Talk 4 | 3.5 | 67 | 30.6 | 30.6 | 6.2 | 230 | 87 | 285 | 12.8 | 18.1 | +1 | 25 | 1 | 82 |
| Talk 5 | EN | 37 | 5.3 | 5.3 | 5.1 | 79 | 82 | 157 | 0.1 | 3.7 | -1 | 4 | 0 | 80 |
| Talk 5 | 3.5 | 37 | 6.8 | 6.8 | 6.6 | 125 | 62 | 225 | 0.1 | 3.4 | -1 | 5 | 0 | 83 |
| Talk 6 | EN | 45 | 14.8 | 14.8 | 14.7 | 166 | 230 | 493 | 0.1 | 4.7 | +0 | 10 | 0 | 82 |
| Talk 6 | 3.5 | 45 | 15.5 | 15.5 | 15.4 | 224 | 185 | 523 | 0.1 | 6.0 | +0 | 15 | 2 | 83 |
| Talk 7 | EN | 58 | 14.8 | 14.8 | 10.5 | 295 | 565 | 209 | 2.4 | 10.1 | +3 | 38 | 3 | 81 |
| Talk 7 | 3.5 | 58 | 15.2 | 15.3 | 10.7 | 464 | 333 | 298 | 2.6 | 12.0 | +4 | 45 | 2 | 81 |

### Pooled

| Set | Model | Sessions | Min | cpWER | WER | del | ins | sub | Gaps ≥ 2 s | Gaps ≥ 5 s | Sum of abs count err | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | EN | 7 | 324 | 16.7 | 7.5 | 1396 | 1302 | 1037 | 152 | 23 | 4 | 77 |
| gate | 3.5 | 7 | 324 | 16.8 | 7.8 | 1879 | 812 | 1209 | 176 | 30 | 4 | 83 |
| broad | EN | 22 | 1017 | 8.3 | 6.8 | 3533 | 4242 | 3448 | 502 | 30 | 3 | 77 (13 timed) |
| broad | 3.5 | 22 | 1017 | 8.6 | 7.2 | 5076 | 2573 | 4195 | 542 | 46 | 3 | 84 (4 timed) |
| talks | EN | 7 | 356 | 14.0 | 8.7 | 1324 | 1708 | 1691 | 172 | 12 | 7 | 74 |
| talks | 3.5 | 7 | 356 | 14.4 | 9.2 | 1782 | 1107 | 2104 | 171 | 12 | 7 | 82 (6 timed) |
| all Zoom | EN | 35 | 1676 | 10.9 | 7.2 | 5914 | 7236 | 6098 | 816 | 61 | 14 | 76 (26 timed) |
| all Zoom | 3.5 | 35 | 1676 | 11.2 | 7.6 | 8341 | 4473 | 7424 | 877 | 85 | 14 | 83 (16 timed) |
| all | EN | 36 | 1697 | 11.0 | 7.3 | 6253 | 7252 | 6176 | 826 | 65 | 14 | 76 (27 timed) |
| all | 3.5 | 36 | 1697 | 11.3 | 7.7 | 8737 | 4492 | 7508 | 889 | 88 | 14 | 83 (17 timed) |


## Reproducing

From `tools/eval` with the README setup, `sessions.toml` holding the 36 sessions (private paths), and these environment variables:

```sh
export DIARIST_EVAL_ENGINE_DIR=<copy of the app's engine>/nemo-speech-0.2.0-windows-x86_64-cuda
export DIARIST_EVAL_MODELS_DIR=<models folder with the three manifest GGUFs>
export DIARIST_EVAL_CLI=<diarist-cli.exe built from the beta.4 tree>
# configs rs_en_shipped and rs_n35_shipped are copies of configs.toml's shipped_en and shipped_n35
for c in rs_en_shipped rs_n35_shipped; do
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions <ids> --transcribe-only
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions <ids> --metrics-only --no-report
done
# WER, gaps, pooled tables: the scratch analyze.py and tables.py of research/19 section 8
```

Most runs used a copy of run.py without the standalone `--rttm` diarizer pass (`--rttm` only adds a second, separate diarizer pass after the transcript is built, `crates/core/src/pipeline/import.rs` step 3), so this report has no standalone-diarizer table. The transcript, its words and tags, and the transcribe-step time are the same with and without it.
