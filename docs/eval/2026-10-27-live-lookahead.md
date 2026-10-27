# Eval: live lookahead 6 (27 Oct 2026)

Chunk F part 2 of docs/plans/16 (section 5, D3): should the live server run both Nemotron models at decoder lookahead 6 (`--asr.streaming.rnnt_right_context 6`, 560 ms chunks) instead of the runtime default, 1 (160 ms)? Three measurements, each at R 1 and R 6 on the same machine, runtime and model files: (b) live first-word latency and VRAM through the real `nemo-speech serve`, (a) word accuracy of the live setting on the file path over the gate set, (c) one of the owner's speaker recordings replayed through the live server and the real post-pass, for carry-over and the echo gate. Numbers are percentages unless a column says otherwise.

**Verdict: the rule passes for both models, so both Nemotron entries in `manifests/models.json` carry `"live_right_context": 6`.** At R 6 the first words of an utterance show 0.48 to 0.52 s after they end at the median (0.14 s at R 1) and 0.56 to 0.58 s at p90; VRAM is unchanged within 3 MiB per session; cpWER improves on AMI (EN 19.75 to 18.03, Nemotron 3.5 23.05 to 21.55) and on every one of the six Zoom sessions for both models (Zoom mean EN 13.90 to 13.58, 3.5 14.36 to 13.85); on the replayed speaker recording the names land as at R 1, the carry-over shift goes from -80/-160 ms to 0, and the echo figures stay within 0.5 points except one that improves by 1.35. `engine.live_lookahead = low` goes back to the runtime default.

## The rule (docs/plans/16 section 5, also section 11)

> Adopt when cpWER at R 6 is better than at R 1 on AMI and on the Zoom mean for both models; live first-word latency median at most 1.0 s and p90 at most 1.5 s; VRAM per session at most 200 MB more and two sessions under the 2 GB budget; (c) within tolerance.

(c)'s tolerance, from the same section: names placed as at R 1, the carry-over shift within ±500 ms, and the echo numbers of the replayed speaker recording (residual duplicates, the owner's words kept) within 0.5 points of the R 1 figures. "Zoom mean" is the unweighted mean of the six Zoom sessions' cpWER, as in rule 4.4.

| Condition | EN | Nemotron 3.5 | Bar | Holds |
|---|---|---|---|---|
| AMI cpWER, R 1 to R 6 | 19.75 to 18.03 | 23.05 to 21.55 | better | both |
| Zoom mean cpWER (6 sessions), R 1 to R 6 | 13.90 to 13.58 | 14.36 to 13.85 | better | both |
| First-word latency at R 6, median (3 runs) | 0.52, 0.52, 0.52 s | 0.48, 0.50, 0.50 s | at most 1.0 s | both |
| First-word latency at R 6, p90 (3 runs) | 0.56, 0.56, 0.56 s | 0.56, 0.58, 0.58 s | at most 1.5 s | both |
| VRAM per session, R 6 minus R 1 (means of 3 runs) | -2 MiB | +3 MiB | at most +200 MB | both |
| Server and two sessions at R 6, largest peak | 1,161 MiB | 1,197 MiB | under 2 GB | both |
| (c) names, shift, echo (EN model, as the app runs live by default) | as at R 1; 0 ms; +0.43 / 0 residual, +0.41 / +1.35 owner's | not replayed (see the notes) | see above | yes, with the note in (c) |

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88. Other agents shared the PC; every GPU job went through the shared GPU queue (one job at a time), and every latency block and replay also held `quiet.lock` (no new heavy work by anyone meanwhile) |
| Runtime | nemo-speech 0.2.0, `nemo-speech-0.2.0-windows-x86_64-cuda`, the app's installed engine read in place (`nemo-speech.exe` SHA-256 `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272`) |
| Models | The app's installed files, read in place: Nemotron EN (`nemotron-speech-streaming-en-0.6b.q8_0`, rev `ebe59e5a8171`), Nemotron 3.5 (`nemotron-3.5-asr-streaming-0.6b.q8_0`, rev `1c8deaecc64b`), Nemotron 3 Diarization (rev `f667ed73aee5`); sizes and SHA-256 equal to `manifests/models.json` |
| Build | This branch at `29ad464` (the integration branch with chunk F part 1), release profile: `diarist-cli` SHA-256 `a776ac321894c009618febfc2c51ca2d7cd6275e16e408fff74c1069019cb54c` and the `live_ws_gpu`, `live_carry_repro` and `echo_sample` test binaries |
| Live command (from serve.log) | `nemo-speech --json serve --asr-model <asr> --diar-model <diar> --device cuda:0 --host 127.0.0.1 --port <p> --no-ui --asr.batching.state_arena_slots 2 --asr.endpointing.enable=true`, plus `--asr.streaming.rnnt_right_context 6` at R 6 (no flag at R 1, as 0.1.0-beta.4 ships) |
| Idle rule for the latency blocks | `nvidia-smi pmon -s u -d 1` ran for the whole block, besides snapshots before and after; a block in which any process other than the engine and the Claude desktop app exceeded 5 percent SM in any sample is invalid and was run again (one of the 13 blocks: the shell at 21 percent). Before each block a 10-second pmon sample had to be clean too |

## (b) Live first-word latency and VRAM

Method: `crates/core/tests/live_ws_gpu.rs` with `DIARIST_LIVE_SESSIONS=2 DIARIST_LIVE_SECONDS=60`, `DIARIST_LIVE_ASR` per model and `DIARIST_LIVE_RIGHT_CONTEXT=6` or unset. 60 s of AMI ES2002a (from 0:56) streamed in real time as 100 ms frames to two sessions on one server, as a recording runs them (the loopback session diarized, the mic session not), language `en`. Three runs per setting, alternating R 1 and R 6. First-word latency of an utterance: when its first revision carrying a word arrived, minus the end of the first word of its final revision (the runtime's word times; stream and wall time agree); both sessions pooled. VRAM is the whole GPU's `memory.used` (WDDM shows no per-process figure): before the server starts, idle once it is ready, and the peak while streaming (the test samples every 5 s; an outside sampler every 200 ms gives the second figure). Per session = (peak - idle) / 2.

| Model | Lookahead | Run | Utterances | Median s | p90 s | Max s | VRAM before / idle / peak MiB (test) | Peak, 200 ms sampler | Per session MiB, test / sampler | Server and two sessions (peak - before) MiB, test / sampler |
|---|---|---|---|---|---|---|---|---|---|---|
| EN | 1 (no flag) | 1 | 30 | 0.14 | 0.18 | 0.20 | 1,966 / 3,065 / 3,103 | 3,113 | 19 / 24 | 1,137 / 1,147 |
| EN | 1 (no flag) | 2 | 30 | 0.14 | 0.18 | 0.20 | 1,932 / 3,039 / 3,096 | 3,100 | 28 / 30 | 1,164 / 1,168 |
| EN | 1 (no flag) | 3 | 30 | 0.14 | 0.18 | 0.20 | 1,947 / 3,046 / 3,088 | 3,092 | 21 / 23 | 1,141 / 1,145 |
| EN | 6 | 1 | 26 | 0.52 | 0.56 | 0.56 | 1,971 / 3,083 / 3,109 | 3,113 | 13 / 15 | 1,138 / 1,142 |
| EN | 6 | 2 | 26 | 0.52 | 0.56 | 0.56 | 1,956 / 3,067 / 3,114 | 3,116 | 24 / 24 | 1,158 / 1,160 |
| EN | 6 | 3 | 26 | 0.52 | 0.56 | 0.56 | 1,957 / 3,065 / 3,118 | 3,122 | 26 / 28 | 1,161 / 1,165 |
| 3.5 | 1 (no flag) | 1 | 32 | 0.14 | 0.18 | 0.18 | 1,953 / 3,085 / 3,141 | 3,151 | 28 / 33 | 1,188 / 1,198 |
| 3.5 | 1 (no flag) | 2 | 32 | 0.14 | 0.18 | 0.18 | 1,947 / 3,078 / 3,118 | 3,120 | 20 / 21 | 1,171 / 1,173 |
| 3.5 | 1 (no flag) | 3 | 32 | 0.14 | 0.18 | 0.18 | 1,947 / 3,078 / 3,118 | 3,120 | 20 / 21 | 1,171 / 1,173 |
| 3.5 | 6 | 1 | 32 | 0.48 | 0.56 | 0.68 | 1,947 / 3,090 / 3,142 | 3,146 | 26 / 28 | 1,195 / 1,199 |
| 3.5 | 6 | 2 | 32 | 0.50 | 0.58 | 0.68 | 1,947 / 3,090 / 3,138 | 3,142 | 24 / 26 | 1,191 / 1,195 |
| 3.5 | 6 | 3 | 32 | 0.50 | 0.58 | 0.78 | 1,947 / 3,090 / 3,144 | 3,148 | 27 / 29 | 1,197 / 1,201 |

| Model | Lookahead | Median s | p90 s | Per session MiB, mean (test / sampler) | Server resident (idle - before) MiB, mean | Server and two sessions, largest (test / sampler) MiB |
|---|---|---|---|---|---|---|
| EN | 1 | 0.14 | 0.18 | 23 / 26 | 1,102 | 1,164 / 1,168 |
| EN | 6 | 0.52 | 0.56 | 21 / 23 | 1,110 | 1,161 / 1,165 |
| 3.5 | 1 | 0.14 | 0.18 | 23 / 25 | 1,131 | 1,188 / 1,198 |
| 3.5 | 6 | 0.48 to 0.50 | 0.56 to 0.58 | 26 / 28 | 1,143 | 1,197 / 1,201 |

R 6 adds 0.34 to 0.38 s, close to the 0.4 s that 5 more 80 ms frames predict, and stays far inside both bars. Per session it uses 2 MiB less with EN and 3 MiB more with 3.5 (bar +200), and the resident server 8 and 12 MiB more; the whole server with both sessions stays under 1.2 GB (budget 2 GB). With R 6 the EN sessions cut the minute into 13 utterances instead of 15 (3.5: 16 both times), with nearly the same word count (EN 143 per session both times, 3.5 132 and 133); in the replay of (c) the server sent about half as many revisions (larger chunks). Every server became ready in 2.6 s at R 6 and 3.1 s at R 1.

A first set of these runs earlier the same day, checked with pmon every 5 s, gave the same latencies to the hundredth (EN 0.14 / 0.18 and 0.52 / 0.56; 3.5 0.14 / 0.18 and 0.48 to 0.50 / 0.56 to 0.60) but whole-GPU VRAM readings that moved by up to 200 MiB between runs, because the owner was using the GPU at the time (a browser, a desktop app's GPU process, at one point a local model server). It was discarded and the set above run again on a free GPU with pmon every second.

## (a) Word accuracy at the live setting (file path)

Method: the live decoder setting through the file path, which runs the same cache-aware decoder (`--stream --endpointing --diarize`), on the gate set (Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14, AMI ES2002a; 324 min). New configs in `tools/eval/configs.toml`: `en_rc_1`, `n35_rc_1` (`--asr.streaming.rnnt_right_context 1`) and `en_rc_6` (`6`); `n35_rc_6` existed. All four pass `--asr.diar.preset v3-streaming`, the live server's diarizer geometry, because a rebuilt `diarist-cli` otherwise applies the manifest's file setting (lookahead 13, `v3-offline`); every run.json records lookahead 1 or 6, preset `v3-streaming` and the CLI above. `run.py --config <c> --sessions <id> --force`, one session per GPU queue call, then the metrics on the CPU; the per-config reports are docs/eval/2026-10-27-en_rc_1.md, -en_rc_6.md, -n35_rc_1.md and -n35_rc_6.md (their wall-time and VRAM columns were taken on a shared GPU and are not timing measurements). References: AMI's word-level manual annotation; on the Zoom sessions Zoom's own machine transcript, so Zoom cpWER mixes Zoom's errors in (docs/05). The runs are deterministic: the R 1 runs reproduce earlier R 1 measurements to the word (AMI 19.8 for EN and 23.0 for 3.5 as docs/eval/2026-10-16-nemotron_en.md and -nemotron35.md; AMI, Aug 2 and Sep 5 as research/19 section 5's live-setting runs), and `n35_rc_6` on AMI gives research/12's 21.6.

Nemotron EN, `en_rc_1` against `en_rc_6`:

| Session | Min | cpWER R 1 | cpWER R 6 | Change | del R 1 / R 6 | ins R 1 / R 6 | sub R 1 / R 6 | tcpWER R 1 / R 6 | WDER R 1 / R 6 | WDER near R 1 / R 6 | Speakers found R 1 / R 6 (reference) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | 44 | 10.27 | 9.93 | -0.34 | 275 / 281 | 228 / 228 | 193 / 164 | 10.28 / 9.99 | 0.55 / 0.58 | 8.75 / 8.40 | 6 / 6 (6) |
| Aug 9 | 44 | 8.49 | 8.11 | -0.38 | 180 / 186 | 233 / 212 | 141 / 131 | 8.51 / 8.14 | 0.28 / 0.27 | 6.93 / 6.17 | 3 / 3 (3) |
| Sep 7 | 48 | 7.78 | 7.44 | -0.34 | 183 / 188 | 228 / 216 | 133 / 116 | 7.78 / 7.44 | 0.26 / 0.32 | 8.18 / 9.17 | 4 / 4 (5) |
| Sep 5 | 45 | 5.12 | 4.82 | -0.30 | 100 / 102 | 203 / 187 | 135 / 123 | 5.13 / 4.83 | 0.14 / 0.11 | 6.90 / 5.52 | 5 / 5 (5) |
| Sep Logistics | 46 | 7.08 | 6.98 | -0.10 | 116 / 120 | 208 / 201 | 123 / 120 | 7.08 / 6.98 | 0.00 / 0.00 | – | 1 / 1 (1) |
| Aug 14 (stress) | 77 | 44.67 | 44.18 | -0.49 | 2,384 / 2,390 | 2,512 / 2,473 | 536 / 510 | 47.60 / 47.07 | 20.88 / 20.76 | 43.82 / 41.06 | 8 / 8 (6) |
| AMI ES2002a | 21 | 19.75 | 18.03 | -1.72 | 355 / 339 | 32 / 24 | 129 / 108 | 20.10 / 18.45 | 3.43 / 2.72 | 9.29 / 7.71 | 4 / 4 (4) |
| Zoom mean (6) | | 13.90 | 13.58 | -0.32 | | | | | | | |
| Zoom mean without Aug 14 (5) | | 7.75 | 7.46 | -0.29 | | | | | | | |
| Zoom pooled (6) | | 17.14 | 16.79 | -0.34 | | | | | | | |

Nemotron 3.5, `n35_rc_1` against `n35_rc_6`:

| Session | Min | cpWER R 1 | cpWER R 6 | Change | del R 1 / R 6 | ins R 1 / R 6 | sub R 1 / R 6 | tcpWER R 1 / R 6 | WDER R 1 / R 6 | WDER near R 1 / R 6 | Speakers found R 1 / R 6 (reference) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | 44 | 12.01 | 11.08 | -0.93 | 377 / 371 | 194 / 173 | 243 / 207 | 12.02 / 11.08 | 0.58 / 0.61 | 9.09 / 9.52 | 6 / 6 (6) |
| Aug 9 | 44 | 9.15 | 8.74 | -0.41 | 223 / 246 | 171 / 161 | 203 / 163 | 9.20 / 8.79 | 0.30 / 0.29 | 7.42 / 6.70 | 3 / 3 (3) |
| Sep 7 | 48 | 7.92 | 7.84 | -0.09 | 254 / 278 | 136 / 133 | 164 / 137 | 7.94 / 7.84 | 0.25 / 0.24 | 8.18 / 7.07 | 4 / 4 (5) |
| Sep 5 | 45 | 5.39 | 5.21 | -0.18 | 171 / 185 | 115 / 104 | 175 / 157 | 5.40 / 5.21 | 0.17 / 0.13 | 7.80 / 7.04 | 5 / 5 (5) |
| Sep Logistics | 46 | 7.33 | 6.33 | -1.00 | 160 / 135 | 154 / 142 | 149 / 123 | 7.33 / 6.33 | 0.00 / 0.00 | – | 1 / 1 (1) |
| Aug 14 (stress) | 77 | 44.34 | 43.90 | -0.44 | 2,458 / 2,446 | 2,337 / 2,321 | 597 / 572 | 47.18 / 46.68 | 20.67 / 20.68 | 40.42 / 40.51 | 8 / 8 (6) |
| AMI ES2002a | 21 | 23.05 | 21.55 | -1.49 | 434 / 408 | 28 / 32 | 140 / 123 | 23.35 / 21.86 | 3.15 / 2.44 | 8.94 / 6.78 | 4 / 4 (4) |
| Zoom mean (6) | | 14.36 | 13.85 | -0.51 | | | | | | | |
| Zoom mean without Aug 14 (5) | | 8.36 | 7.84 | -0.52 | | | | | | | |
| Zoom pooled (6) | | 17.50 | 17.02 | -0.48 | | | | | | | |

Every session improves with both models; most of the gain is substitutions and insertions. Deletions rise a little on several Zoom sessions with 3.5 (Aug 9 223 to 246, Sep 7 254 to 278, Sep 5 171 to 185) and by 2 to 6 words with EN, and fall on AMI with both; on the Zoom sessions that may be Zoom's reference as much as the model, but it is worth watching with the gap count of research/19 if live words ever become the stored transcript. EN at R 6 reaches the same AMI cpWER as at the file setting, lookahead 13 (18.0, docs/04 section 3). Speaker counts are unchanged everywhere. The transcript's DER (from the exported segments against Zoom's cue times) moves by -1.0 to +0.8 points on the Zoom sessions, up with EN on all six (by 0.2 to 0.8; Aug 2 6.4 to 7.2) and mixed with 3.5, and falls on AMI with both models (EN 20.4 to 19.7, 3.5 26.1 to 25.3): the R 6 words are stamped about 80 ms earlier (below), which moves segment edges against Zoom's cues; it is not part of the rule.

## (c) Replay of a speaker recording: carry-over and the echo gate

Method: meeting 16 (the owner on speakers, 7.5 minutes, echo coupled, the post-pass canceller engaged), copied with the database into a scratch folder and never touched in place; the copies were deleted afterwards. `crates/core/tests/live_carry_repro.rs` streams its stereo `playback.flac` in real time through `nemo-speech serve` (EN model, as the app runs live by default, with the meeting's clock anchors), stores every revision as the app does, renames the loopback label with the most final lines from the legend and the second one from its newest line halfway through, then runs the real post-pass (EN, lookahead 13, `v3-offline`). Once with no flag (R 1) and once with `DIARIST_LIVE_RIGHT_CONTEXT=6`. Then `crates/core/tests/echo_sample.rs` over each replay's database (CPU only): its post-pass path uses meeting 16's stored canceller words for both, so only the transfer gate, which compares against the live run's raw mic words, sees the setting; its live path replays the live run's every revision through the live gate. The R 1 replay reproduces the app's own live run of that meeting exactly (2,430 revisions, and every echo figure below equal to the stored live run's), so the harness is faithful.

Carry-over:

| | R 1 | R 6 |
|---|---|---|
| Live revisions | 2,430 | 1,313 |
| Renamed at the midpoint | `spk_0` from the legend, `spk_1` from its line at 171.3 to 175.0 s | `spk_0` from the legend, `spk_1` from its line at 171.0 to 175.0 s |
| Names after the post-pass (`CarryOverReport.named`) | legend name on post-pass `spk_0`, line name on `spk_1` | the same |
| Speakers matched / needing review / created | 3 / 0 / 0 | 3 / 0 / 0 |
| `shift_ms` (the live run's times moved to meet the post-pass's) | mic -80, loopback -160 | mic 0, loopback 0 |
| Unplaced names | 0 | 0 |
| Post-pass words | 1,240 | 1,241 (one more mic word kept by the transfer gate) |

Echo gate (echo_sample rows; "owner's words" are non-duplicates whose ±5 neighbours are at most 30 percent duplicates, classed on the words before the gate; residual duplicates by the measurement's matcher; chance is the matcher with the kept words moved 45 s later):

| Path | Figure | R 1 | R 6 | Change |
|---|---|---|---|---|
| Post-pass, transfer gate on the live run's mic words (the shipped row) | words before the gate / kept | 366 / 230 | 366 / 232 | |
| | residual duplicates | 0 (0.00 %, chance 2.17 %) | 1 (0.43 %, chance 2.16 %) | +0.43 points |
| | owner's words kept | 227 of 248 (91.53 %) | 228 of 248 (91.94 %) | +0.41 points |
| Live path, every revision through the live gate | mic words before the gate / kept | 1,196 / 231 | 1,195 / 231 | |
| | residual duplicates | 0 (0.00 %, chance 2.60 %) | 0 (0.00 %, chance 3.03 %) | 0 |
| | owner's words kept | 214 of 219 (97.72 %) | 213 of 215 (99.07 %) | +1.35 points (one word fewer kept, four fewer words classed as the owner's) |

The transfer gate's inputs: the live run's raw mic words 1,196 at R 1 and 1,195 at R 6, of which the raw gate calls 966 echo in both (text layer 889 and 901, correlation 74 and 60); candidates 136 and 127, removed by the shipped level guard 132 and 125, guarded 4 and 2, unmatched 6 and 14. The real post-pass of each replay (its `engine_info.echo`; the harness writes no `loopback.gain`, so its canceller runs without the gain track and sees 359 words) drops 129 and 128 of 359 (transfer 48 and 47), the rest identical. The text layer's lag spread on the live path: at 300 ms, 872 and 863 words in 3-word runs; at the shipped 500 ms, 886 and 908.

Word times: R 6 stamps words about 80 ms earlier than R 1 at the median (AMI 2,390 aligned words, Aug 2 6,598, Sep 5 8,567, EN; AMI with 3.5 the same), and against AMI's manual word times the median lateness goes from 0.29 to 0.21 s with the EN model and from 0.35 to 0.28 s with Nemotron 3.5 (means 0.56 to 0.29 s and 0.57 to 0.28 s; `levers.py offsets`). That is the carry-over shift above going to zero: at R 6 the live words line up with the post-pass's (lookahead 13) on this recording.

Against (c)'s tolerance: names placed as at R 1; shift 0 ms (within ±500); residual duplicates +0.43 points on the post-pass path and unchanged live (within 0.5); the owner's words kept +0.41 points on the post-pass path (within 0.5) and +1.35 points on the live path. The last moves by more than 0.5 points, in the direction the gate is tuned for (the owner's words outrank residual duplicates, plan 15), so it is no sign that the gate windows measured at R 1 have stopped fitting; the plan's remedy for a (c) failure, re-sweeping the windows, answers a loss, not a gain. This report reads (c) as passed; read strictly as a two-sided tolerance it would not be.

## Notes

- The replay ran the EN model only, the live default for English; Nemotron 3.5 live (other languages, low-VRAM mode) has the latency, VRAM and accuracy rows above but no echo replay. The echo layers work on word text and times, and 3.5's R 6 words move by the same 80 ms on AMI.
- What changes for users: the live text is replaced by the post-pass by default, so the gain reaches what is read during the meeting, `post_pass_mode = on_issues` meetings that keep their live transcript, and the transfer gate's raw live words. Older live runs stay as they are; each live run records the value it ran at (`engine_info.live_right_context`).
- Two-sided reading of "VRAM per session at most 200 MB more": both models are within 3 MiB either way.
- The live latency figure is measured against the runtime's own word times, which R 6 stamps earlier; against true word ends the R 1 figure would be larger by the 80 ms difference, so the 0.34 to 0.38 s that R 6 adds is, if anything, an overstatement.

## Commands

```
# Environment (read only): the app's engine and models
DIARIST_ENGINE_DIR=%LOCALAPPDATA%\app.diarist.desktop\engine\nemo-speech-0.2.0-windows-x86_64-cuda
DIARIST_MODELS_DIR=%LOCALAPPDATA%\app.diarist.desktop\models
cargo build --release -p diarist-cli
cargo test --release -p diarist-core --test live_ws_gpu --test live_carry_repro --test echo_sample --no-run

# (b) per model, 3 times each, alternating unset and 6; each run one GPU queue call holding quiet.lock,
#     with `nvidia-smi pmon -s u -d 1` and a 200 ms memory.used sampler running for the whole call
DIARIST_AMI_WAV=<ES2002a.Mix-Headset.wav> DIARIST_LIVE_SESSIONS=2 DIARIST_LIVE_SECONDS=60 \
DIARIST_LIVE_ASR=<nemotron-speech-streaming-en-0.6b.q8_0.gguf | nemotron-3.5-asr-streaming-0.6b.q8_0.gguf> \
[DIARIST_LIVE_RIGHT_CONTEXT=6] cargo test --release -p diarist-core --test live_ws_gpu -- --nocapture

# (a) per config and session, then scoring, reports and the tables
DIARIST_EVAL_ENGINE_DIR=... DIARIST_EVAL_MODELS_DIR=... DIARIST_EVAL_CLI=target/release/diarist-cli.exe
python -I run.py --config <en_rc_1|en_rc_6|n35_rc_1|n35_rc_6> --sessions <id> --force --transcribe-only
python -I run.py --config <c> --sessions all --metrics-only
python -I levers.py compare --configs en_rc_1,en_rc_6,n35_rc_1,n35_rc_6
python -I levers.py offsets --base en_rc_1 --configs en_rc_6 --sessions ami,aug2,sep5   # and n35_rc_1 / n35_rc_6 on ami

# (c) on copies of diarist.db (-wal, -shm) and meeting 16's folder
REPRO_FLAC=<copy>/playback.flac REPRO_SRC_DB=<copy>/diarist.db REPRO_MEETING=16 REPRO_OUT=<scratch>/r1 \
  cargo test --release -p diarist-core --test live_carry_repro -- --ignored --nocapture
# the same with DIARIST_LIVE_RIGHT_CONTEXT=6 and REPRO_OUT=<scratch>/r6; then per replay, without the engine variables,
# a folder named after the replay meeting's uid holding copies of playback.flac, loopback.words.json, mic-aec.words.json, loopback.gain:
DIARIST_ECHO_MEETINGS=<that folder> DIARIST_ECHO_DB=<scratch>/r1/repro.db DIARIST_ECHO_OUT=<scratch> \
  cargo test --release -p diarist-core --test echo_sample -- --nocapture
```

Nothing of the owner's recordings or transcripts is in this report beyond counts and shorthand labels.
