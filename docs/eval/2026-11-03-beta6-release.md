# Eval: release check for 0.1.0-beta.6 (3 Nov 2026)

The release check of the beta.6 integration branch against 0.1.0-beta.5 (`docs/eval/2026-10-27-beta5-release.md`). It uses the same 36 sessions (the owner's 35 Zoom recordings plus AMI ES2002a), the same models, runtime and file-pass settings, and the same scoring definitions. diarist-cli was rebuilt from the integration tree at `592b355` (plan 18's chunks V, Q, U, N1, N2 with spike L and R, and chunk H). It runs the app's shipped defaults: the loop collapse on and punctuation on (`transcript.punctuation = auto` with the manifest's punctuation model, now pinned at v1.1.1). Both ASR models are scored: Nemotron EN (the default) and Nemotron 3.5. The accuracy runs were untimed. The speed bar was measured afterwards, in a separate timed gate-set block under quiet.lock (Speed, gate set). Numbers are percentages unless a column says otherwise.

Nothing in beta.6 touches the file transcription path:

- The runtime stays 0.2.0, with the same executable, the same models and the same command line.
- The punctuation model's files are the bytes beta.5 used. v1.1.1 added only the authors' `LICENSE` and `NOTICE`.
- In `crates/`, only test modules changed: four files under `crates/core/src/models/` (`fetch.rs`, `manifest.rs`, `scan.rs`, `setup.rs`).
- The desktop changes (chunk V) are the window title, the taskbar overlay, the tray frames and the in-app dot.

So the words should be identical to beta.5's. The bars below are those of the beta.5 note's "Release bars", judged against beta.5's numbers.

Result: all four bars pass, and the words are the same as beta.5's.

- **Words.** Every one of the 72 runs (36 sessions, two models) has the same runtime words as the cached v0.2.0 run of that session: 270,703 with EN and 262,632 with Nemotron 3.5. After the loop collapse and punctuation, every run has the same transcript words (text, the speech model's own text, times and speaker) and the same segments as beta.5's import path: 270,265 and 262,622 words, 13,093 and 11,972 segments.
- **Scores.** Every score equals beta.5's on every session: cpWER, tcpWER, WER with its deletions, insertions and substitutions, WDER, DER, count error and both gap counts. cpWER, tcpWER, WDER, DER, count error and the gaps also equal the beta.5 note's printed tables. Its WER, del, ins and sub columns came from another script, which counts a few words differently (Data and metrics), so beta.5 is rescored here with `metrics.py`.
- **Pooled cpWER.** 10.85 with EN and 11.30 with Nemotron 3.5, as in beta.5. No session is worse, and no gap is new or gone.
- **Punctuation.** It ran on all 72 runs at the v1.1.1 pin (`0cea764`) and added the same marks and capitals as in beta.5.
- **Speed.** Timed in a separate block on an idle machine. Pooled over the gate set: 78.0 times real time with EN (beta.5 81.3, 4.1 percent slower) and 81.4 with Nemotron 3.5 (beta.5 78.6, 3.6 percent faster). Both are within the 5 percent bar.

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88 |
| diarist-cli | built in release from `592b355` (`claude/beta6-integration`, version 0.1.0-beta.6; the later merge `6e219c7` changes only the desktop front end), SHA-256 `72b2ff9d6c006d2b1ee5e00a4a4a4fa9a3a6926897f0a185a1ef16ff98e38c9f` |
| Engine | nemo-speech 0.2.0 (`engine_version` in every run.json), package `nemo-speech-0.2.0-windows-x86_64-cuda`, a byte copy of the app's installed engine; `nemo-speech.exe` SHA-256 `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272` (beta.5's) |
| ASR, EN | `nemotron-speech-streaming-en-0.6b.q8_0`, SHA-256 `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d` (beta.5's) |
| ASR, 3.5 | `nemotron-3.5-asr-streaming-0.6b.q8_0`, SHA-256 `a5c435f294eea8f88ce68dd27b8c3bfea7f777cb2fbba04fcd30eaa555f429ae` (beta.5's) |
| Diarizer | `Nemotron-3-Diarization.q8_0`, SHA-256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` (beta.5's) |
| Punctuation | `punctuation-horizon-small` at `0cea764` (v1.1.1), the eight files of beta.6's manifest entry, each matching its SHA-256 in `manifests/models.json`. `onnx/model_quantized.onnx` `5679870b…1f4ace` and `tokenizer.json` `f34b44a6…b1558f0a` are beta.5's, and `diarist-onnxruntime.dll` (ONNX Runtime 1.31.0, CPU) is the same file. `LICENSE` and `NOTICE` are the authors' own (docs/eval/2026-10-30-punctuation-pin.md). The harness selects the entry in each data root, and every run records `engine_info.punctuation.revision` `0cea764…` |
| Configs | `rel6_shipped_en` and `rel6_shipped_n35`: copies of `configs.toml`'s `shipped_en` and `shipped_n35` with `punctuation = true` (the app's default; the loop collapse is on by default), under new names so nothing came from a cache. `--transcribe-only --no-diar-pass`, then `--metrics-only` |
| Settings recorded in run.json | `right_context` 13, `diar_preset` v3-offline, `device` cuda:0, `language` en, `engine_args` empty, `punctuation` true, `collapse_loops` true, `diar_pass` false |
| Engine command | the beta.5 note's, unchanged |
| GPU | every run through the shared GPU queue. The accuracy runs were untimed, without quiet.lock (2 and 3 Nov 2026, about 23:13 to 00:54 IST); the timed block ran under quiet.lock (3 Nov 2026, 03:17 to 03:31 IST, Speed below) |

## Data and metrics

The sessions are those of the beta.5 note:

- the gate set: Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14 and AMI ES2002a, 324 minutes;
- the broad set: 22 recordings, 1,017 minutes;
- the talks: Talk 1 to Talk 7, 356 minutes.

The references are Zoom's own `transcript.vtt` for the Zoom sessions and AMI's word-level manual annotation. The metric definitions are those of `tools/eval/metrics.py`, as in the beta.5 note, with one difference in where four columns come from:

- **WER and its deletions, insertions and substitutions** come from `metrics.py`. The beta.5 note took these four columns from research/19's scratch `analyze.py`, which orders words differently: a few words on most sessions, the same number of deletions and insertions each time (docs/eval/2026-10-30-gapfill.md section 3). Every other column is computed as in the beta.5 note.
- **The beta.5 baseline for the like-for-like comparison** is N1's `en_replay` and `n35_replay` (docs/eval/2026-10-30-gapfill.md section 3), scored by today's `metrics.py`. Each is the cached v0.2.0 runtime words of every session (research/19's `rs_*_shipped`, the same runtime, models and command line) replayed through beta.5's own file import, with the loop collapse and punctuation. They reproduce the beta.5 note's cpWER, tcpWER, WDER, DER, count error and both gap counts on all 36 sessions with both models.
- **"New" and "gone" gaps** compare each beta.6 run's gaps with that baseline run's gaps of the same session and model, by overlap in time, as the beta.5 note did against beta.4.

## The bars (the beta.5 note's "Release bars", against beta.5's numbers)

Quoted from plan 18 section 12, which restates the beta.5 note's release bars for this release: "pooled cpWER per model not worse, no session worse by more than 0.5 unless the words show why, no new gap of 5 s or more, speed within 5 percent for the same measurement".

- **Pooled cpWER per model not worse than beta.5.** Beta.5: 10.85 (EN), 11.30 (Nemotron 3.5), all 36 sessions.
- **No session worse by more than 0.5 cpWER points**, unless the words show why.
- **No new gap of 5 s or more** against the beta.5 run of the same session and model.
- **Speed within 5 % of beta.5 for the same measurement.** Pooled × real time of the `nemo-speech transcribe` step over the gate set, per model, against beta.5's 81.3 (EN) and 78.6 (Nemotron 3.5). It is timed under quiet.lock with `nvidia-smi pmon -s u -d 1`, as in the beta.5 note and the 31 Oct re-time (`docs/eval/2026-10-31-retime-n35.md`). A run counts only when it is valid.

## Results

Δ cpWER is beta.6 minus the beta.5 note's printed value, in points, from the rounded values of the two reports. In the pooled table "cpWER beta.5" is the beta.5 import path scored by today's `metrics.py` (Data and metrics, above). It equals the beta.5 note's pooled figures: EN 16.6, 8.1, 13.9, 10.8 and 10.85; Nemotron 3.5 16.8, 8.6, 14.4, 11.2 and 11.30.

### Gate set (six user meetings plus AMI)

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | EN | 44 | 9.5 | 0.0 | 9.5 | 9.2 | 268 | 191 | 164 | 0.3 | 13.2 | +0 | 21 | 1 |
| Aug 2 | 3.5 | 44 | 10.3 | 0.0 | 10.4 | 10.1 | 361 | 125 | 199 | 0.2 | 12.2 | +0 | 28 | 5 |
| Aug 9 | EN | 44 | 8.0 | 0.0 | 8.0 | 7.4 | 162 | 195 | 128 | 0.3 | 12.7 | +0 | 25 | 4 |
| Aug 9 | 3.5 | 44 | 8.4 | 0.0 | 8.4 | 8.0 | 236 | 144 | 145 | 0.3 | 11.2 | +0 | 31 | 6 |
| Sep 7 | EN | 48 | 7.1 | 0.0 | 7.1 | 6.8 | 164 | 191 | 123 | 0.3 | 15.0 | -1 | 50 | 2 |
| Sep 7 | 3.5 | 48 | 7.5 | 0.0 | 7.5 | 7.2 | 251 | 110 | 141 | 0.2 | 14.6 | -1 | 60 | 4 |
| Sep 5 | EN | 45 | 4.7 | 0.0 | 4.7 | 4.5 | 93 | 172 | 122 | 0.2 | 7.0 | -1 | 14 | 2 |
| Sep 5 | 3.5 | 45 | 5.1 | 0.0 | 5.1 | 4.9 | 176 | 89 | 158 | 0.2 | 5.6 | -1 | 9 | 1 |
| Sep Logistics | EN | 46 | 7.0 | 0.0 | 7.0 | 7.1 | 119 | 201 | 130 | 0.0 | 12.2 | +0 | 10 | 1 |
| Sep Logistics | 3.5 | 46 | 6.6 | 0.0 | 6.6 | 6.6 | 141 | 147 | 129 | 0.0 | 9.3 | +0 | 14 | 2 |
| Aug 14 | EN | 77 | 43.8 | 0.0 | 44.4 | 7.2 | 264 | 315 | 291 | 19.4 | 29.3 | +2 | 22 | 9 |
| Aug 14 | 3.5 | 77 | 42.9 | 0.0 | 43.6 | 7.0 | 318 | 178 | 353 | 19.3 | 27.6 | +2 | 22 | 9 |
| AMI ES2002a | EN | 21 | 18.0 | 0.0 | 18.4 | 16.6 | 341 | 16 | 77 | 2.3 | 25.4 | +0 | 10 | 4 |
| AMI ES2002a | 3.5 | 21 | 20.7 | 0.0 | 21.0 | 19.1 | 396 | 19 | 84 | 2.2 | 28.2 | +0 | 12 | 3 |

### Broad set (folders A, B and C)

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 1 | EN | 47 | 4.1 | 0.0 | 4.1 | 3.9 | 125 | 98 | 90 | 0.2 | 9.0 | +0 | 14 | 1 |
| Aug 1 | 3.5 | 47 | 4.5 | 0.0 | 4.5 | 4.4 | 184 | 62 | 114 | 0.1 | 6.9 | +0 | 14 | 3 |
| Aug 3 | EN | 52 | 10.0 | 0.0 | 10.0 | 9.8 | 241 | 212 | 274 | 0.1 | 11.5 | +0 | 15 | 3 |
| Aug 3 | 3.5 | 52 | 9.9 | 0.0 | 10.0 | 9.8 | 306 | 146 | 279 | 0.1 | 9.7 | +0 | 15 | 4 |
| Aug 4 | EN | 46 | 7.6 | 0.0 | 7.7 | 6.9 | 180 | 160 | 95 | 0.4 | 16.9 | +0 | 45 | 2 |
| Aug 4 | 3.5 | 46 | 8.3 | 0.0 | 8.3 | 8.2 | 322 | 64 | 128 | 0.1 | 17.3 | +0 | 51 | 5 |
| Aug 5 | EN | 43 | 5.0 | 0.0 | 5.0 | 5.0 | 56 | 225 | 141 | 0.1 | 6.0 | +0 | 2 | 1 |
| Aug 5 | 3.5 | 43 | 5.0 | 0.0 | 5.1 | 5.0 | 111 | 137 | 174 | 0.1 | 4.3 | +0 | 2 | 1 |
| Aug 6 | EN | 65 | 5.2 | 0.0 | 5.2 | 5.1 | 224 | 145 | 252 | 0.1 | 9.0 | +0 | 24 | 3 |
| Aug 6 | 3.5 | 65 | 6.6 | 0.0 | 6.6 | 6.6 | 373 | 122 | 312 | 0.0 | 8.3 | +0 | 36 | 3 |
| Aug 7 | EN | 55 | 35.0 | 0.0 | 35.2 | 8.1 | 206 | 233 | 203 | 14.2 | 25.6 | +2 | 35 | 1 |
| Aug 7 | 3.5 | 55 | 35.5 | 0.0 | 35.9 | 8.2 | 269 | 155 | 229 | 14.7 | 25.6 | +2 | 31 | 2 |
| Aug 8 | EN | 46 | 6.8 | 0.0 | 6.8 | 6.7 | 128 | 120 | 210 | 0.0 | 10.4 | +0 | 14 | 1 |
| Aug 8 | 3.5 | 46 | 7.5 | 0.0 | 7.5 | 7.4 | 183 | 74 | 250 | 0.0 | 9.0 | +0 | 11 | 1 |
| Aug 10 | EN | 47 | 6.4 | 0.0 | 6.4 | 6.3 | 142 | 157 | 126 | 0.1 | 11.2 | +0 | 25 | 1 |
| Aug 10 | 3.5 | 47 | 6.6 | 0.0 | 6.6 | 6.6 | 180 | 128 | 133 | 0.1 | 10.0 | +0 | 22 | 3 |
| Aug 11 | EN | 43 | 5.3 | 0.0 | 5.3 | 5.3 | 81 | 281 | 68 | 0.0 | 7.9 | +0 | 14 | 0 |
| Aug 11 | 3.5 | 43 | 4.7 | 0.0 | 4.7 | 4.7 | 137 | 143 | 103 | 0.0 | 7.5 | +0 | 20 | 0 |
| Aug 12 | EN | 46 | 4.3 | 0.0 | 4.4 | 4.3 | 44 | 202 | 59 | 0.1 | 6.7 | +0 | 1 | 0 |
| Aug 12 | 3.5 | 46 | 3.0 | 0.0 | 3.0 | 3.0 | 70 | 71 | 73 | 0.0 | 4.8 | +0 | 1 | 0 |
| Aug 13 | EN | 47 | 5.7 | 0.0 | 5.8 | 5.7 | 145 | 99 | 123 | 0.1 | 12.5 | +0 | 26 | 0 |
| Aug 13 | 3.5 | 47 | 6.0 | 0.0 | 6.0 | 5.9 | 175 | 64 | 142 | 0.0 | 8.8 | +0 | 18 | 1 |
| Aug 15 | EN | 44 | 8.0 | 0.0 | 8.1 | 7.0 | 164 | 135 | 123 | 0.6 | 14.2 | +1 | 7 | 2 |
| Aug 15 | 3.5 | 44 | 7.4 | 0.0 | 7.4 | 6.6 | 169 | 96 | 135 | 0.4 | 12.0 | +1 | 10 | 2 |
| Sep 1 | EN | 35 | 8.8 | 0.0 | 8.8 | 8.7 | 195 | 137 | 119 | 0.1 | 12.7 | +0 | 20 | 1 |
| Sep 1 | 3.5 | 35 | 9.6 | 0.0 | 9.6 | 9.5 | 257 | 93 | 143 | 0.1 | 11.3 | +0 | 23 | 2 |
| Sep 2 | EN | 42 | 6.5 | 0.0 | 6.5 | 6.4 | 143 | 116 | 199 | 0.1 | 10.3 | +0 | 14 | 0 |
| Sep 2 | 3.5 | 42 | 7.1 | 0.0 | 7.1 | 6.9 | 163 | 93 | 241 | 0.1 | 8.7 | +0 | 17 | 1 |
| Sep 3 | EN | 43 | 6.9 | 0.0 | 6.9 | 6.8 | 233 | 123 | 198 | 0.1 | 10.0 | +0 | 32 | 3 |
| Sep 3 | 3.5 | 43 | 8.3 | 0.0 | 8.3 | 8.3 | 303 | 98 | 275 | 0.1 | 10.3 | +0 | 29 | 2 |
| Sep 4 | EN | 43 | 8.2 | 0.0 | 8.2 | 8.1 | 230 | 177 | 158 | 0.2 | 10.8 | +0 | 21 | 0 |
| Sep 4 | 3.5 | 43 | 9.0 | 0.0 | 9.0 | 8.8 | 298 | 132 | 185 | 0.1 | 10.4 | +0 | 27 | 2 |
| Sep 6 | EN | 47 | 4.8 | 0.0 | 4.9 | 4.7 | 189 | 126 | 113 | 0.2 | 12.3 | +0 | 40 | 2 |
| Sep 6 | 3.5 | 47 | 5.9 | 0.0 | 6.0 | 5.7 | 306 | 80 | 127 | 0.2 | 12.8 | +0 | 43 | 2 |
| Jul 1 | EN | 28 | 9.6 | 0.0 | 9.6 | 9.5 | 175 | 92 | 112 | 0.2 | 13.1 | +0 | 23 | 3 |
| Jul 1 | 3.5 | 28 | 9.5 | 0.0 | 9.5 | 9.4 | 217 | 48 | 110 | 0.2 | 14.4 | +0 | 22 | 1 |
| Jul 2 | EN | 40 | 6.3 | 0.0 | 6.3 | 6.3 | 160 | 164 | 108 | 0.0 | 11.4 | +0 | 43 | 3 |
| Jul 2 | 3.5 | 40 | 6.2 | 0.0 | 6.3 | 6.2 | 209 | 89 | 127 | 0.0 | 12.3 | +0 | 41 | 4 |
| Jul 3 | EN | 63 | 7.9 | 0.0 | 7.9 | 7.8 | 229 | 446 | 273 | 0.1 | 9.1 | +0 | 34 | 0 |
| Jul 3 | 3.5 | 63 | 10.0 | 0.0 | 10.0 | 9.8 | 425 | 309 | 467 | 0.2 | 9.5 | +0 | 44 | 1 |
| Jul 4 | EN | 40 | 5.4 | 0.0 | 5.4 | 5.3 | 110 | 117 | 107 | 0.2 | 12.2 | +0 | 21 | 1 |
| Jul 4 | 3.5 | 40 | 5.9 | 0.0 | 5.9 | 5.8 | 142 | 107 | 120 | 0.0 | 9.6 | +0 | 25 | 1 |
| Jul 5 | EN | 52 | 11.7 | 0.0 | 11.8 | 11.7 | 196 | 381 | 291 | 0.1 | 10.1 | +0 | 32 | 2 |
| Jul 5 | 3.5 | 52 | 11.8 | 0.0 | 11.8 | 11.8 | 285 | 260 | 328 | 0.0 | 9.9 | +0 | 40 | 5 |

### Talks (folder D)

| Session | Model | Min | cpWER | Δ cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Talk 1 | EN | 53 | 6.6 | 0.0 | 6.6 | 6.4 | 144 | 147 | 179 | 0.2 | 9.9 | +0 | 15 | 3 |
| Talk 1 | 3.5 | 53 | 6.7 | 0.0 | 6.7 | 6.5 | 153 | 111 | 218 | 0.1 | 7.8 | +0 | 13 | 3 |
| Talk 2 | EN | 45 | 7.1 | 0.0 | 7.1 | 6.9 | 140 | 173 | 205 | 0.1 | 8.4 | -1 | 23 | 0 |
| Talk 2 | 3.5 | 45 | 7.4 | 0.0 | 7.4 | 7.3 | 184 | 116 | 241 | 0.1 | 7.7 | -1 | 22 | 0 |
| Talk 3 | EN | 52 | 12.6 | 0.0 | 12.6 | 12.1 | 345 | 323 | 236 | 0.4 | 11.9 | -1 | 49 | 4 |
| Talk 3 | 3.5 | 52 | 12.7 | 0.0 | 12.7 | 12.5 | 402 | 213 | 314 | 0.2 | 12.0 | +0 | 46 | 4 |
| Talk 4 | EN | 67 | 30.1 | 0.0 | 30.1 | 5.6 | 165 | 161 | 212 | 12.7 | 21.6 | +1 | 33 | 2 |
| Talk 4 | 3.5 | 67 | 30.6 | 0.0 | 30.6 | 6.2 | 230 | 87 | 285 | 12.8 | 20.0 | +1 | 25 | 1 |
| Talk 5 | EN | 37 | 5.3 | 0.0 | 5.3 | 5.2 | 80 | 83 | 157 | 0.1 | 9.0 | -1 | 4 | 0 |
| Talk 5 | 3.5 | 37 | 6.8 | 0.0 | 6.8 | 6.6 | 125 | 62 | 225 | 0.1 | 6.5 | -1 | 5 | 0 |
| Talk 6 | EN | 45 | 14.6 | 0.0 | 14.6 | 14.6 | 168 | 222 | 491 | 0.1 | 9.7 | +0 | 10 | 0 |
| Talk 6 | 3.5 | 45 | 15.5 | 0.0 | 15.5 | 15.4 | 224 | 185 | 523 | 0.1 | 9.2 | +0 | 15 | 2 |
| Talk 7 | EN | 58 | 14.8 | 0.0 | 14.8 | 10.6 | 301 | 571 | 209 | 2.4 | 15.2 | +3 | 38 | 3 |
| Talk 7 | 3.5 | 58 | 15.2 | 0.0 | 15.3 | 10.7 | 464 | 333 | 298 | 2.6 | 15.7 | +4 | 45 | 2 |

### Pooled

"Sessions with any scored difference" counts sessions where any key of metrics.json other than wall times and VRAM differs from the beta.5 run of the same session and model.

| Set | Model | Sessions | Min | cpWER beta.5 | cpWER beta.6 | Change | WER | del | ins | sub | Gaps ≥ 2 s | Gaps ≥ 5 s | New / gone ≥ 5 s | New / gone ≥ 2 s | Sum of abs count err | Sessions with any scored difference |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | EN | 7 | 324 | 16.63 | 16.63 | +0.00 | 7.5 | 1411 | 1281 | 1035 | 152 | 23 | 0 / 0 | 0 / 0 | 4 | 0 |
| gate | 3.5 | 7 | 324 | 16.79 | 16.79 | +0.00 | 7.8 | 1879 | 812 | 1209 | 176 | 30 | 0 / 0 | 0 / 0 | 4 | 0 |
| broad | EN | 22 | 1017 | 8.09 | 8.09 | +0.00 | 6.7 | 3596 | 3946 | 3442 | 502 | 30 | 0 / 0 | 0 / 0 | 3 | 0 |
| broad | 3.5 | 22 | 1017 | 8.60 | 8.60 | +0.00 | 7.2 | 5084 | 2571 | 4195 | 542 | 46 | 0 / 0 | 0 / 0 | 3 | 0 |
| talks | EN | 7 | 356 | 13.94 | 13.94 | +0.00 | 8.7 | 1343 | 1680 | 1689 | 172 | 12 | 0 / 0 | 0 / 0 | 7 | 0 |
| talks | 3.5 | 7 | 356 | 14.45 | 14.45 | +0.00 | 9.2 | 1782 | 1107 | 2104 | 171 | 12 | 0 / 0 | 0 / 0 | 7 | 0 |
| all Zoom | EN | 35 | 1676 | 10.78 | 10.78 | +0.00 | 7.1 | 6009 | 6891 | 6089 | 816 | 61 | 0 / 0 | 0 / 0 | 14 | 0 |
| all Zoom | 3.5 | 35 | 1676 | 11.20 | 11.20 | +0.00 | 7.6 | 8349 | 4471 | 7424 | 877 | 85 | 0 / 0 | 0 / 0 | 14 | 0 |
| all | EN | 36 | 1697 | 10.85 | 10.85 | +0.00 | 7.2 | 6350 | 6907 | 6166 | 826 | 65 | 0 / 0 | 0 / 0 | 14 | 0 |
| all | 3.5 | 36 | 1697 | 11.30 | 11.30 | +0.00 | 7.7 | 8745 | 4490 | 7508 | 889 | 88 | 0 / 0 | 0 / 0 | 14 | 0 |

### The words

The words did not change, as expected.

- The runtime, its command line, the speech models and the diarizer are beta.5's, byte for byte (Setup).
- The import pipeline, the loop collapse and the punctuation step are the same code: no non-test Rust changed in `crates/` (Integration checks, below).
- The punctuation model and tokenizer are the same bytes.

The comparisons, both by exact equality of every field:

- **Runtime words** (`engine.words.json`) against research/19's cached runs of the same sessions: equal on 72 of 72 runs.
- **Exported words** (`transcript.json`: text, `raw_text`, start, end, speaker) and **segments** against the beta.5 import path: equal on 72 of 72 runs.

The loop collapse removed the same bursts as in beta.5:

- EN: 61 bursts and 438 words on 23 sessions;
- Nemotron 3.5: 3 bursts and 10 words on 2 sessions.

### Punctuation and the loop collapse

Punctuation ran on all 72 runs: `applied` true, `reason` ok, unit `turn`, policy `keep_asr`, 4 intra-op threads, revision `0cea764aa6fc55c8bdfc20a5f0bb4dc27b64753d`. It rejected 0 words. Its marks and capitals are the beta.5 note's on every set.

Its wall time (`engine_info.punctuation.wall_s`) is untimed here: no quiet.lock, with the desktop and other agents on the machine. The table gives it for the record only. Beta.5's bar is 15 s per hour (plan 16 section 3.12 rule 2).

| Set | Model | Sessions | Hours | Words | Marks added | Capitals raised | Rejected words | Punctuation s | s per hour | Applied | Loop collapse: sessions, bursts, words |
|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | EN | 7 | 5.4 | 49,832 | 7,372 | 3,327 | 0 | 28.7 | 5.3 | 7 of 7 | 4, 10, 34 |
| gate | 3.5 | 7 | 5.4 | 48,499 | 6,424 | 3,210 | 0 | 30.2 | 5.6 | 7 of 7 | 0, 0, 0 |
| broad | EN | 22 | 16.9 | 165,170 | 23,311 | 9,380 | 0 | 87.3 | 5.2 | 22 of 22 | 14, 42, 358 |
| broad | 3.5 | 22 | 16.9 | 160,469 | 19,750 | 8,957 | 0 | 90.0 | 5.3 | 22 of 22 | 2, 3, 10 |
| talks | EN | 7 | 5.9 | 55,263 | 7,716 | 3,142 | 0 | 29.1 | 4.9 | 7 of 7 | 5, 9, 46 |
| talks | 3.5 | 7 | 5.9 | 53,654 | 6,697 | 3,063 | 0 | 22.3 | 3.8 | 7 of 7 | 0, 0, 0 |
| all | EN | 36 | 28.3 | 270,265 | 38,399 | 15,849 | 0 | 145.2 | 5.1 | 36 of 36 | 23, 61, 438 |
| all | 3.5 | 36 | 28.3 | 262,622 | 32,871 | 15,230 | 0 | 142.6 | 5.0 | 36 of 36 | 2, 3, 10 |

### Speed, gate set

**The block.** One block on 3 Nov 2026, 03:17:41 to 03:27:06 IST.

- **Locks.** quiet.lock throughout, and gpu.lock through the shared queue.
- **pmon.** `nvidia-smi pmon -s u -d 1` ran for the whole block.
- **Configs.** `rel6_shipped_en_t` and `rel6_shipped_n35_t`: the release configs, renamed so they transcribe again. The same diarist-cli build, with `--transcribe-only --no-diar-pass`.
- **The machine.**
  - The owner was away, and the browser, the VPN client and another desktop app had been closed.
  - A 60 s pmon check just before the block was clean: claude.exe peaked at 12 percent SM and nothing else rose above 0.
  - nvidia-smi before the block: 0 %, 894 MiB, 44 °C, P8.
  - CPU load: 3.8 percent (ten samples of 2 s, 0.6 to 7.9).
- **No cold start.** The WAVs were converted, and the engine, model and WAV files read once, before the block, so no conversion or cold read fell inside it.
- **Validity, as in the beta.5 note.** A run is valid when no process other than nemo-speech is above 5 percent SM in any sample of its window, claude.exe excepted. claude.exe's peak was 0 percent SM in every run.

**Two EN runs were re-timed.** Sep 7 and Aug 14 were invalid: explorer.exe drew 6 percent SM in one sample of each.

- After a fresh 60 s pmon check (no process above 0 percent), both were re-timed once, 03:28:48 to 03:30:38, under quiet.lock (config `rel6_shipped_en_t2`).
- Both re-times are valid: 38.1 and 58.3 s, against the invalid runs' 38.0 and 58.2 s.
- The table uses the re-times. Every other run is from the block, and all 14 rows are valid.

| Session | Model | Min | beta.5 s | beta.5 × real time | beta.6 s | beta.6 × real time | Change in × real time | Timing |
|---|---|---|---|---|---|---|---|---|
| Aug 2 | EN | 44 | 32.4 | 81 | 33.4 | 78 | -3.0 % | block |
| Aug 9 | EN | 44 | 33.9 | 79 | 34.5 | 77 | -1.7 % | block |
| Sep 7 | EN | 48 | 35.9 | 79 | 38.1 | 75 | -5.8 % | re-time |
| Sep 5 | EN | 45 | 32.9 | 82 | 33.8 | 80 | -2.7 % | block |
| Sep Logistics | EN | 46 | 33.1 | 83 | 34.5 | 79 | -4.1 % | block |
| Aug 14 | EN | 77 | 55.4 | 83 | 58.3 | 79 | -5.0 % | re-time |
| AMI ES2002a | EN | 21 | 15.6 | 82 | 16.7 | 76 | -6.6 % | block |
| Aug 2 | 3.5 | 44 | 32.9 | 79 | 32.4 | 81 | +1.5 % | block |
| Aug 9 | 3.5 | 44 | 36.9 | 72 | 32.3 | 82 | +14.2 % | block |
| Sep 7 | 3.5 | 48 | 34.5 | 83 | 35.0 | 82 | -1.4 % | block |
| Sep 5 | 3.5 | 45 | 32.6 | 83 | 32.8 | 82 | -0.6 % | block |
| Sep Logistics | 3.5 | 46 | 35.6 | 77 | 33.7 | 81 | +5.6 % | block |
| Aug 14 | 3.5 | 77 | 59.3 | 78 | 56.5 | 82 | +5.0 % | block |
| AMI ES2002a | 3.5 | 21 | 15.5 | 82 | 16.1 | 79 | -3.7 % | block |
| gate pooled | EN | 324 | 239.2 | 81.3 | 249.3 | 78.0 | -4.1 % | |
| gate pooled | 3.5 | 324 | 247.3 | 78.6 | 238.8 | 81.4 | +3.6 % | |

Reading it:

- **EN** pools at 78.0 times real time against beta.5's 81.3: 4.1 percent slower, inside the 5 percent bar.
  - Every EN session timed 1.7 to 6.6 percent slower than in beta.5's block; the 21-minute AMI file, where loading the model weighs most, is the slowest.
  - Beta.5's EN figure was itself 5.0 percent above beta.4's 77.4, which the beta.5 note puts down to two slow beta.4 runs. Against beta.4, beta.6's EN is 0.8 percent faster.
  - The timed step is the same executable, models and command line in all three releases (Setup), so no code cause is expected. The spread is that of the machine between sessions.
- **Nemotron 3.5** pools at 81.4 against beta.5's 78.6: 3.6 percent faster. Against beta.4's 83.1 it is 2.0 percent slower.
- **The three sessions of the 31 Oct re-time** (Aug 9, Sep Logistics, Aug 14):
  - beta.6 takes 32.3, 33.7 and 56.5 s, 122.5 s in all;
  - the re-time took 118.6 s, so 3.3 percent more seconds now;
  - beta.4 took 119.8 s, so 2.3 percent more;
  - that re-time's bar allowed 125.8 s.
  - The beta.5 block's 131.8 s for these sessions came from the environment, as that re-time found.

## Verdict on the bars

| Bar | Result | |
|---|---|---|
| Pooled cpWER per model not worse than beta.5 | EN 10.85 → 10.85, Nemotron 3.5 11.30 → 11.30 (all 36); every set equal | met |
| No session worse by more than 0.5 cpWER points | no session changed at all, with either model; every word is the same | met |
| No new gap of 5 s or more | 0 new and 0 gone, of 5 s or more and of 2 s or more, with either model | met |
| Speed within 5 % of beta.5 | all 14 gate runs timed and valid (two EN runs re-timed once): pooled × real time EN 78.0 against 81.3 (-4.1 %), Nemotron 3.5 81.4 against 78.6 (+3.6 %) | met |

## Integration checks on the same tree

Release builds of `592b355`, each run through the shared GPU queue.

- **`acceptance_gpu`** (the 77-minute file): 11,928 words, 0.27 percent over the reference count, 8 speakers, 163 s. Beta.5's figures.
- **`live_ws_gpu`** at the shipped live lookahead 6 (30 s of AMI, one session): first-word latency median 0.52 s, p90 0.56 s, ready in 2.6 s, 33 s. Beta.5: median 0.52 s, 33 s.
- **`echo_fleurs`** (20 minutes over meeting 12's far end, from a scratch copy of the meeting folder):
  - "me" WER 12.69 percent, with 0 echo words left, as in beta.5;
  - the headphone control is identical (1,178 "me" words off and auto).
- All pass.

**Not run for this release:** `echo_sample` (meetings 4, 5 and 12 to 21), `aec_postpass` (meeting 12's real post-pass), the Process again exact test (meetings 17, 18 and 20) and `live_carry_repro` (meeting 16).

- **Why.** These suites need private copies of the owner's database or of a meeting's recording. Those copies were not made for this check.
- **The evidence that they would pass.** `git diff --stat origin/main...HEAD -- crates/` touches only four files: `crates/core/src/models/fetch.rs`, `manifest.rs`, `scan.rs` and `setup.rs`. Every changed hunk lies inside their test modules (chunk Q's tests for the v1.1.1 pin). So the live, echo, post-pass and rerun code is byte-identical to beta.5's, which passed these suites on 27 Oct 2026 (the beta.5 note, "Integration checks"). The desktop changes (chunk V and the compact download row) are in `apps/desktop` and run in none of these tests.

## Reproducing

From `tools/eval` with the README setup, `sessions.toml` holding the 36 sessions (private paths), and these environment variables:

```sh
export DIARIST_EVAL_ENGINE_DIR=<copy of the app's engine>/nemo-speech-0.2.0-windows-x86_64-cuda
export DIARIST_EVAL_MODELS_DIR=<models folder: the three manifest GGUFs and the punctuation entry's eight files under its dir>
export DIARIST_EVAL_CLI=<diarist-cli.exe built in release from the beta.6 tree>
# configs rel6_shipped_en and rel6_shipped_n35: copies of configs.toml's shipped_en and
# shipped_n35 with `punctuation = true`, in a scratch configs file
for c in rel6_shipped_en rel6_shipped_n35; do
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions <ids> --transcribe-only --no-diar-pass
  uv run python -I run.py --config $c --configs-file <scratch configs.toml> --sessions all --metrics-only --no-report
done
# the beta.5 baseline: en_replay and n35_replay of docs/eval/2026-10-30-gapfill.md (gapfill.py run, then --metrics-only);
# the word and gap comparisons read transcript.json, engine.words.json and metrics.json of both
```

Keep the engine and models folders on a short path (the beta.5 note's last paragraph).
