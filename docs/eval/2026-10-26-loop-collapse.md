# Eval: repeated-token loop collapse (2026-10-26)

Plan 16 section 12.3 (D8) and chunk K of section 12.5, with the reviewer's should-fix items of 12.6 (S7 to S10). CPU only: nothing here ran on the GPU. No transcript text is reproduced; the owner's meetings appear by meeting number, the eval sessions by their docs/05 label.

What shipped (`crates/core/src/transcript/loops.rs`):

- **The rule.** Over one channel's words in time order, a run is a maximal sequence of consecutive words with the same normalised text (the echo gate's `norm` of the ASR's text) and the same speaker label, split into pieces wherever two consecutive starts are more than 400 ms apart (`LOOP_SPLIT_MS`). A piece is a loop when it has 4 or more words (`LOOP_MIN_RUN`) at a median start-to-start gap of at most 80 ms (`LOOP_FRAME_MS`, one encoder frame), or exactly 3 words that start in the same frame. A loop becomes its first word (start, speaker, engine label, confidence, margin, overlap) ending where the piece ends, with the last word's trailing ASR mark. Never collapsed: pairs, pieces at human pace, an empty `norm`, digits only, and Han, Kana and Thai words.
- **Where.** Every build that makes words, after the echo gate and the transfer gate (which keep seeing the raw words) and the unknown-token check, before punctuation and segmentation: the post-pass (both channels), the promoted live run (its inline `post_pass` run is now built when the collapse removed a word, with or without a punctuator), imports, Restore (the words in `echo_dropped` are raw, so a loop removed as echo comes back whole and is collapsed again) and punctuation rebuilds (with the setting at rebuild time). No live collapse.
- **Setting and storage.** `transcript.collapse_loops`, default `on`, no UI. Counts only, in `engine_info.loops` = `{applied, runs, words_removed, per_channel: {<channel>: {runs, words_removed}}, rule: {min_run, frame_ms, split_ms}}`, replaced wholesale on every build; on a run rebuilt from another the counts add that run's own (its words are already collapsed). The removed words are not stored in the database; the engine's words JSON in the meeting folder keeps them. The meeting details say "N repeated-token loops collapsed (M words)"; the JSON export is unchanged.

Two readings of the plan, noted here because they are choices: "the same frame" for three words is "within half a frame (40 ms) of the first word", which is equality on the runtime's 80 ms grid and robust to the clock map's ±1 ms rounding (on the stored meetings every gap under 400 ms between two identical consecutive words is 0, 80, 159, 160, 240, 241, 319, 320 or 399 ms, and no piece of 4 or more has a median between 81 and 119 ms, so the literal "at most 80 ms" for the median and this reading of "one frame" decide the same pieces as any tolerance would); and the kept word ends at the latest end in the piece, which is the last word's end on every recording here.

## 1. The owner's meetings 12 to 21 (stored transcripts)

Source: a copy of the data root's database (read only at the source, deleted after), each meeting's current run, each channel's words in `(start, id)` order with the engine label of its own segment's speaker row, as a rebuild reads them (`levers.py loops --db`). Meeting 19 is a file import. Columns (the same in section 2): pairs of identical consecutive words (in brackets, those one frame apart or less); runs of exactly 3 (in one frame / one frame apart); pieces of 4 or more by median start gap; runs with a gap of more than 400 ms inside (in brackets, those with a collapsed piece); the collapsed pieces (4 or more / 3 in a frame) and the words they lose; and "4+ kept", pieces of 4 or more at human pace, with their words.

| Meeting | Channel | Words | Pairs (one frame) | Runs of 3 (same frame / one-frame spacing) | Pieces of 4+ (median ≤ 80 / 81–160 / > 160 ms) | Runs split at 400 ms (with a collapse) | Collapsed (4+ / 3 in a frame) | Words removed | 4+ kept (words) |
|---|---|---|---|---|---|---|---|---|---|
| 12 | loopback | 13,551 | 146 (34) | 10 (1 / 0) | 10 (8 / 0 / 2) | 42 (1) | 8 / 1 | 72 | 2 (8) |
| 12 | mic | 14,627 | 170 (38) | 20 (3 / 0) | 6 (6 / 0 / 0) | 54 (1) | 6 / 3 | 72 | 0 |
| 13 | loopback | 928 | 2 (0) | 0 | 0 | 0 | 0 | 0 | 0 |
| 13 | mic | 386 | 4 (0) | 1 (0 / 0) | 0 | 3 (0) | 0 | 0 | 0 |
| 14 | loopback | 1,055 | 25 (7) | 2 (0 / 0) | 1 (1 / 0 / 0) | 11 (1) | 1 / 0 | 22 | 0 |
| 14 | mic | 340 | 3 (0) | 0 | 0 | 2 (0) | 0 | 0 | 0 |
| 15 | loopback | 852 | 1 (0) | 0 | 0 | 3 (0) | 0 | 0 | 0 |
| 15 | mic | 187 | 7 (0) | 0 | 0 | 5 (0) | 0 | 0 | 0 |
| 16 | loopback | 1,010 | 6 (0) | 0 | 1 (1 / 0 / 0) | 3 (0) | 1 / 0 | 3 | 0 |
| 16 | mic | 293 | 4 (0) | 0 | 0 | 2 (0) | 0 | 0 | 0 |
| 17 | loopback | 1,031 | 6 (0) | 0 | 0 | 1 (0) | 0 | 0 | 0 |
| 17 | mic | 160 | 2 (0) | 0 | 0 | 1 (0) | 0 | 0 | 0 |
| 18 | loopback | 1,435 | 17 (2) | 1 (0 / 0) | 2 (0 / 1 / 1) | 15 (0) | 0 | 0 | 2 (8) |
| 18 | mic | 757 | 17 (2) | 2 (0 / 0) | 4 (4 / 0 / 0) | 26 (2) | 4 / 0 | 24 | 0 |
| 19 | source | 258 | 3 (0) | 0 | 0 | 0 | 0 | 0 | 0 |
| 20 | loopback | 10,351 | 93 (11) | 7 (0 / 0) | 7 (2 / 1 / 4) | 38 (0) | 2 / 0 | 29 | 5 (21) |
| 20 | mic | 1,245 | 15 (0) | 3 (1 / 0) | 2 (1 / 0 / 1) | 9 (1) | 1 / 1 | 5 | 1 (5) |
| 21 | loopback | 7,666 | 164 (25) | 32 (0 / 1) | 12 (3 / 2 / 7) | 50 (0) | 3 / 0 | 16 | 9 (38) |
| 21 | mic | 288 | 0 | 0 | 0 | 2 (0) | 0 | 0 | 0 |
| all | | 56,420 | 685 (119) | 78 (5 / 1) | 45 (26 / 4 / 15) | 267 (6) | 26 / 5 | **243** | 19 (80) |

Pieces by length, median start gap p10 / p50 / p90: 3 words 80 / 240 / 360 ms (78 pieces); 4 or 5 words 0 / 240 / 320 ms (28); 6 or more 0 / 0 / 0 ms (17). Collapsed pieces that cross a segment boundary: 1; split by a speaker label: none (the rule without the label condition removes the same 243); followed by a word that starts with the loop's token (a false start of the next word): 5.

**Removal counts against the plan's expectation.** The plan expected about 250 words (12: 145, 14: 22, 16: 3, 18: 27, 20: 34, 21: 20, per segment). Those numbers are the draft rule before the review (no 400 ms split, three words at a median of one frame): `levers.py` reproduces them exactly, and the reviewer's 254 over the channel stream too. The rule as built, with the review's one-frame rule for three words and the 400 ms split, removes 243:

| Count | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | Total |
|---|---|---|---|---|---|---|---|---|---|---|---|
| the rule, channel stream (as built) | 144 | 0 | 22 | 0 | 3 | 0 | 24 | 0 | 34 | 16 | **243** |
| the rule, within one segment | 143 | 0 | 22 | 0 | 3 | 0 | 24 | 0 | 34 | 16 | 242 |
| draft rule, channel stream (the reviewer's count) | 146 | 0 | 23 | 0 | 3 | 0 | 28 | 0 | 34 | 20 | 254 |
| draft rule, within one segment (the plan's expectation) | 145 | 0 | 22 | 0 | 3 | 0 | 27 | 0 | 34 | 20 | 251 |

The differences are the two should-fix items working as intended: on meeting 21 two triples that are not in one frame (4 words) stay, and in meeting 18's "me" two runs have a pause of more than 400 ms inside, so the words after the pause stay (28 words over the stream under the draft, 24 now; plan 16 section 12.2.4 counts 36 words in that channel's five canceller-induced loops by the analysis's definition, which ignores timing).

**EN-only-insertion share** (target at least 90 percent). The word-error analysis of these meetings (measurement 3, private; only its per-loop records are used: start time, run length and how many copies Whisper distil-large-v3 has of the token within ±0.5 s, for the stored post-pass loopback and final "me" and for Nemotron 3.5's transcripts of the same channels) records every run of 4 or more identical tokens that Whisper has fewer than 3 copies of. A removed token counts as confirmed when its piece overlaps such a record and Nemotron 3.5 has no loop there (EN only).

| Channel | Words removed | In a loop the analysis recorded | EN only (no Nemotron 3.5 loop there) | Not labelled |
|---|---|---|---|---|
| loopback | 142 | 131 (92.3 %) | 131 (92.3 %) | 11 (2 in 3-in-a-frame pieces) |
| mic ("me") | 101 | 89 (88.1 %) | 89 (88.1 %) | 12 (6 in 3-in-a-frame pieces) |
| both | 243 | 220 (**90.5 %**) | 220 (**90.5 %**) | 23 |

The target is met over both channels. Of the 23 unlabelled tokens, 8 are in 3-in-a-frame pieces, which the analysis's definition (4 or more) cannot label; the other 15 are in pieces of 4 or more that no analysis record overlaps (the analysis normalises tokens its own way and was not built to label every loop). Caveat: 23 of the 220 labelled tokens sit in records where Whisper has 2 copies of the token, so up to about one word per such record may be a real repeat that the collapse removed.

## 2. The evaluation set (36 sessions)

The cached runtime words JSON of every session of the shipped EN configuration (`rs_en_shipped`, the respin measurement's copy of `shipped_en`), converted as the file import stores it (microseconds, then milliseconds rounded, clamped to the audio, labels by first appearance; `levers.py loops --configs rs_en_shipped`). Per group:

| Group | Words | Pairs (one frame) | Runs of 3 (same frame / one-frame spacing) | Pieces of 4+ (median ≤ 80 / 81–160 / > 160 ms) | Runs split at 400 ms (with a collapse) | Collapsed (4+ / 3 in a frame) | Words removed | 4+ kept (words) |
|---|---|---|---|---|---|---|---|---|
| tuning (2) | 13,256 | 138 (16) | 13 (0 / 1) | 4 (3 / 0 / 1) | 83 (1) | 3 / 0 | 14 | 1 (4) |
| held-out (2) | 15,617 | 136 (32) | 9 (0 / 0) | 2 (2 / 0 / 0) | 35 (1) | 2 / 0 | 7 | 0 |
| control (1) | 6,353 | 63 (9) | 7 (0 / 1) | 0 | 17 (0) | 0 | 0 | 0 |
| stress (1) | 12,193 | 121 (23) | 12 (0 / 0) | 6 (4 / 0 / 2) | 25 (0) | 4 / 0 | 20 | 2 (8) |
| overlap, AMI (1) | 2,447 | 15 (2) | 1 (1 / 0) | 0 | 12 (0) | 0 / 1 | 2 | 0 |
| talks (7) | 55,309 | 643 (88) | 69 (3 / 2) | 19 (7 / 4 / 8) | 168 (0) | 7 / 3 | 55 | 12 (52) |
| broad (22) | 165,528 | 1,491 (218) | 119 (4 / 10) | 59 (42 / 6 / 11) | 563 (11) | 42 / 4 | 423 | 17 (72) |
| all 36 | 270,703 | 2,607 (388) | 230 (8 / 14) | 90 (58 / 10 / 22) | 903 (13) | 58 / 8 | **521** | 32 (136) |

Pieces by length, median start gap p10 / p50 / p90: 3 words 80 / 200 / 320 ms (230 pieces); 4 or 5 words 0 / 160 / 240 ms (50); 6 or more 0 / 0 / 0 ms (40). The 521 words sit in 23 of the 36 sessions; Jul 3 has 165 of them (13 loops). Followed by a word that starts with the token: 11. Nemotron 3.5's cached words of the same sessions (`rs_n35_shipped`) give 3 loops and 13 words.

**Does the reference have the repeat?** The reference's copies of the loop's token (the harness's normaliser; Zoom cue times interpolated by word length) within 1 s of each piece:

| Pieces | Count | Filler (no reference token) | Reference has at most 1 | 2 to n − 1 | n or more |
|---|---|---|---|---|---|
| collapsed | 66 | 2 | 52 | 12 | **0** |
| 4 or more, kept by the timing rule | 32 | 0 | 21 | 4 | 7 |

No collapsed piece is a repeat the reference has in full (in 12 the reference has the token two or more times nearby, so a real word may go with the insertions); 7 of the 32 pieces the timing rule keeps are (real repeats at human pace, which is what the rule is for), and 21 more look like insertions at human pace that the rule leaves alone by design. Zoom's own transcript is machine output, so "the reference has it" is evidence, not proof.

## 3. The eval gate: the full set re-scored through the file import

Plan 16 section 12.3, measurement 2, without the GPU. Every session's cached runtime words JSON of `rs_en_shipped` went through the app's own file import (`pipeline::import_file`) with a replay engine that returns the cached words (chunk B's ignored `cli_check_root` driver) into two scratch data roots without a punctuation model: one with `transcript.collapse_loops = off`, one with the default, `on`. Then `diarist-cli export --format json` and `--format rttm` of each meeting (a debug build of this branch), the source run's diarizer RTTM beside it, and `run.py --metrics-only` over the exports (configs `shipped_en_replay_off` and `shipped_en_replay_on` in a scratch copy of `configs.toml`; each `run.json` names the replayed source and has no speed or VRAM fields).

**The replay is faithful:** with the collapse off it equals the GPU run on all 432 values compared (cpWER, tcpWER, DI-cpWER, ORC-WER, WDER, WDER near changes, DER at both collars, JER, speaker-count error, hypothesis speakers and words, on all 36 sessions).

| Session | Group | Words removed | cpWER off | cpWER on | Δ | tcpWER off | tcpWER on | Δ | DER c0.25 Δ | JER Δ | WDER Δ |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | tuning | 0 | 9.516 | 9.516 | +0.000 | 9.546 | 9.546 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 9 | tuning | 14 | 8.111 | 7.927 | −0.184 | 8.157 | 7.973 | −0.184 | −0.004 | −0.001 | +0.000 |
| Sep 7 | held-out | 7 | 7.223 | 7.065 | −0.157 | 7.223 | 7.065 | −0.157 | +0.000 | +0.000 | +0.000 |
| Sep 5 | held-out | 0 | 4.698 | 4.698 | +0.000 | 4.698 | 4.698 | +0.000 | +0.000 | +0.000 | +0.000 |
| Sep Logistics | control | 0 | 7.031 | 7.031 | +0.000 | 7.031 | 7.031 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 14 | stress | 20 | 43.936 | 43.771 | −0.164 | 44.528 | 44.363 | −0.164 | +0.000 | +0.000 | +0.000 |
| AMI ES2002a | overlap | 2 | 18.032 | 18.032 | +0.000 | 18.415 | 18.415 | +0.000 | +0.000 | +0.000 | −0.042 |
| Talk 1 | talks | 0 | 6.601 | 6.601 | +0.000 | 6.601 | 6.601 | +0.000 | +0.000 | +0.000 | +0.000 |
| Talk 2 | talks | 11 | 7.200 | 7.053 | −0.147 | 7.200 | 7.053 | −0.147 | +0.003 | +0.001 | +0.000 |
| Talk 3 | talks | 18 | 12.790 | 12.522 | −0.268 | 12.830 | 12.562 | −0.268 | −0.000 | +0.000 | +0.000 |
| Talk 4 | talks | 14 | 30.230 | 30.085 | −0.145 | 30.251 | 30.106 | −0.145 | +0.000 | +0.000 | +0.000 |
| Talk 5 | talks | 0 | 5.329 | 5.329 | +0.000 | 5.329 | 5.329 | +0.000 | +0.000 | +0.000 | +0.000 |
| Talk 6 | talks | 10 | 14.774 | 14.609 | −0.165 | 14.774 | 14.609 | −0.165 | +0.000 | +0.000 | +0.000 |
| Talk 7 | talks | 2 | 14.817 | 14.817 | +0.000 | 14.846 | 14.846 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 1 | broad | 0 | 4.090 | 4.090 | +0.000 | 4.139 | 4.139 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 3 | broad | 8 | 10.034 | 9.926 | −0.108 | 10.034 | 9.926 | −0.108 | +0.003 | +0.001 | +0.000 |
| Aug 4 | broad | 8 | 7.712 | 7.632 | −0.079 | 7.823 | 7.744 | −0.079 | +0.000 | +0.000 | +0.000 |
| Aug 5 | broad | 23 | 5.170 | 4.923 | −0.247 | 5.193 | 4.947 | −0.247 | +0.000 | +0.000 | +0.000 |
| Aug 6 | broad | 0 | 5.179 | 5.179 | +0.000 | 5.187 | 5.187 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 7 | broad | 28 | 35.320 | 35.018 | −0.302 | 35.496 | 35.194 | −0.302 | +0.007 | +0.010 | +0.006 |
| Aug 8 | broad | 0 | 6.786 | 6.786 | +0.000 | 6.786 | 6.786 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 10 | broad | 0 | 6.435 | 6.435 | +0.000 | 6.435 | 6.435 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 11 | broad | 40 | 5.713 | 5.292 | −0.420 | 5.713 | 5.292 | −0.420 | −0.020 | −0.007 | +0.000 |
| Aug 12 | broad | 5 | 4.373 | 4.303 | −0.070 | 4.387 | 4.317 | −0.070 | +0.000 | +0.000 | +0.000 |
| Aug 13 | broad | 0 | 5.742 | 5.742 | +0.000 | 5.758 | 5.758 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 15 | broad | 30 | 8.435 | 8.023 | −0.412 | 8.451 | 8.040 | −0.412 | +0.000 | +0.000 | +0.000 |
| Sep 1 | broad | 0 | 8.819 | 8.819 | +0.000 | 8.819 | 8.819 | +0.000 | +0.000 | +0.000 | +0.000 |
| Sep 2 | broad | 8 | 6.582 | 6.470 | −0.112 | 6.638 | 6.526 | −0.112 | +0.000 | +0.000 | +0.000 |
| Sep 3 | broad | 3 | 6.916 | 6.879 | −0.037 | 6.916 | 6.879 | −0.037 | +0.000 | +0.000 | +0.000 |
| Sep 4 | broad | 17 | 8.387 | 8.158 | −0.229 | 8.401 | 8.172 | −0.229 | +0.008 | +0.003 | +0.000 |
| Sep 6 | broad | 0 | 4.846 | 4.846 | +0.000 | 4.879 | 4.879 | +0.000 | +0.000 | +0.000 | +0.000 |
| Jul 1 | broad | 3 | 9.651 | 9.601 | −0.050 | 9.676 | 9.626 | −0.050 | +0.000 | +0.000 | +0.000 |
| Jul 2 | broad | 34 | 6.729 | 6.261 | −0.468 | 6.744 | 6.276 | −0.468 | −0.016 | −0.008 | +0.000 |
| Jul 3 | broad | 165 | 9.083 | 7.694 | −1.390 | 9.083 | 7.694 | −1.390 | −0.014 | −0.005 | +0.000 |
| Jul 4 | broad | 0 | 5.369 | 5.369 | +0.000 | 5.369 | 5.369 | +0.000 | +0.000 | +0.000 | +0.000 |
| Jul 5 | broad | 51 | 12.144 | 11.523 | −0.621 | 12.198 | 11.577 | −0.621 | +0.016 | +0.007 | +0.000 |
| **mean, all 36** | | 521 | **10.383** | **10.223** | **−0.160** | 10.432 | 10.272 | −0.160 | −0.000 | +0.000 | −0.001 |
| mean, tuning (2) | | 14 | 8.814 | 8.722 | −0.092 | 8.851 | 8.759 | −0.092 | −0.002 | −0.001 | +0.000 |
| mean, held-out (2) | | 7 | 5.960 | 5.882 | −0.079 | 5.960 | 5.882 | −0.079 | +0.000 | +0.000 | +0.000 |
| mean, talks (7) | | 55 | 13.106 | 13.002 | −0.104 | 13.119 | 13.015 | −0.104 | +0.000 | +0.000 | +0.000 |
| mean, broad (22) | | 423 | 8.342 | 8.135 | −0.207 | 8.369 | 8.163 | −0.207 | −0.001 | +0.000 | +0.000 |

Percent, at the full precision of `metrics.json`; Δ is on minus off, in points.

| Bar (plan 16 section 12.3) | Result | |
|---|---|---|
| cpWER on no session worse by more than 0.05 | no session worse at all: 21 better (Jul 3 the most, −1.390), 15 unchanged | met |
| cpWER mean equal or better | 10.383 → 10.223 (−0.160) | met |
| tcpWER the same | no session worse; mean 10.432 → 10.272 (−0.160) | met |
| DER and JER unchanged within noise (a collapsed run keeps its span) | DER c0.25 −0.020 to +0.016 points, DER c0 −0.019 to +0.023, JER −0.008 to +0.010; speaker counts unchanged on all 36 | met |

Two sessions lose words with cpWER unchanged: on AMI the manual reference has the triple's word twice at that point, so one insertion became one deletion (WDER −0.042, WDER near changes −0.114); Talk 7's triple is a filler, which the harness's normaliser drops. DI-cpWER and ORC-WER move with cpWER (−1.398 to +0.038; the +0.038 is one session's re-alignment). WDER moves by −0.042 to +0.006 (its aligned word pairs change). The small DER and JER moves fit segment boundaries moving (the hypothesis RTTM is per segment, and with fewer words the 80-word segment split falls elsewhere in a long stretch); the kept word spans its burst, so no speech time is lost.

## 4. The Rust path against the measurement

The import's `engine_info.loops` (the replay with the collapse on) removes exactly what `levers.py` predicts on every session: 521 words in 66 loops, session by session (Jul 3: 165 in 13, Aug 11: 40 in 2, AMI: 2 in 1, and so on). With the collapse off every run records `applied: false` and removes nothing.

## 5. Echo rows

Unchanged by construction: the collapse runs in `run_post_pass` after `echo_step` and the unknown-token check and touches no file under `crates/core/src/audio/` (the gate, the transfer gate and the canceller) or `live/echo_gate.rs`; `crates/core/tests/echo_sample.rs` calls the gate's functions directly and no code this chunk changed, so its rows for meetings 12 to 21 cannot move. `engine_info.echo` and `echo_dropped` are written from the gate's raw words, before the collapse. Two GPU tests not run here, `aec_postpass.rs` and `echo_fleurs.rs`, go through `run_post_pass` and count the final stored words of "me", so their kept-word and word-error figures see the collapse after this chunk (their `engine_info.echo` rows do not); to compare with their earlier tables, run them with `transcript.collapse_loops = off` in the test store.

## 6. Reproduce

```
# the owner's meetings, on a copy of the database (numbers only)
uv run python -I levers.py loops --db <copy of diarist.db> --meetings 12-21 --analysis <the analysis's loops JSON>
# the eval set
uv run python -I levers.py --sessions-file <sessions.toml> loops --configs rs_en_shipped,rs_n35_shipped --runs-dir <.work/runs>
```

The replay and comparison scripts are in the gitignored `tools/eval/.work/chunk-k/` of the chunk's worktree (`replay.sh`, `runinfo.py`, `compare.py`, `configs.replay.toml`).
