# Eval: repeated-token loop collapse (2026-10-26, rule refined 2026-10-27)

Plan 16 section 12.3 (D8) and chunk K of section 12.5, with the reviewer's should-fix items of 12.6 (S7 to S10), and the review of the chunk itself (27 Oct 2026), whose findings refined the rule (below) and added sections 5 and 6. CPU only: nothing here ran on the GPU. No transcript text is reproduced; the owner's meetings appear by meeting number, the eval sessions by their docs/05 label, the public sessions by their set's label.

What shipped (`crates/core/src/transcript/loops.rs`):

- **The rule.** Over one channel's words in time order, a run is a maximal sequence of consecutive words with the same normalised text (the echo gate's `norm` of the ASR's text) and the same speaker label, split into pieces wherever two consecutive starts are more than 400 ms apart (`LOOP_SPLIT_MS`). A piece is a loop when it has 4 or more words (`LOOP_MIN_RUN`) at a median start-to-start gap of at most 80 ms (`LOOP_FRAME_MS`, one encoder frame), or exactly 3 words that start in the same frame: that is the plan's rule, and it now only decides which pieces are loops. **What goes is narrower (the rule as built):** within a loop, each burst, a maximal sub-run whose consecutive starts are at most 80 ms apart, that meets the rule on its own (4 or more words, or exactly 3 in one frame) becomes its first word (start, speaker, engine label, confidence, margin, overlap) ending where the burst ends, with the burst's last trailing ASR mark. The loop's words at human pace around a burst stay. The plan's rule took the piece's median over the whole piece, so five copies in one frame followed by the real word at 300, 600 and 900 ms became one word; the rule as built keeps the three real ones. It is strictly more conservative than the plan's rule: it removes a subset of the words the plan's rule removes, never a word the plan's rule keeps. Never collapsed: pairs, pieces at human pace, an empty `norm`, digits only, and Han, Kana and Thai words.
- **Where.** Every build that makes words, after the echo gate and the transfer gate (which keep seeing the raw words) and the unknown-token check, before punctuation and segmentation: the post-pass (both channels), the promoted live run (its inline `post_pass` run is built when the collapse removed a word, with or without a punctuator, and the live run then stays), imports, Restore (the words in `echo_dropped` are raw, so a loop removed as echo comes back whole and is collapsed again; `engine_info.echo.restored_merged` counts the restored words merged into a burst, and the details say so) and every punctuation rebuild, Add and Remove alike (with the setting at rebuild time). No live collapse.
- **Setting and storage.** `transcript.collapse_loops`, default `on`, no switch in the app (docs/DEV.md, "Punctuation settings", says how to set it). Counts only, in `engine_info.loops` = `{applied, runs, words_removed, per_channel: {<channel>: {runs, words_removed}}, rule: {min_run, frame_ms, split_ms}}`, `runs` counting the bursts collapsed, replaced wholesale on every build; on a run rebuilt from another the counts add that run's own (its words are already collapsed). The removed words are not stored with the run. Where they survive depends on the path: the file engine's words JSON in the meeting folder for the post-pass and imports; the earlier run, which stays, for Restore and rebuilds; the live run for a promoted live run, which the post-pass now keeps (and crash recovery too) whenever the collapse removed a word, because the live engine writes no words JSON. Turning the setting off, or Remove punctuation, does not bring collapsed words back into a transcript. The meeting details say "N repeated-token loops collapsed (M words)"; the JSON export is unchanged.

Two readings of the plan, noted here because they are choices: "the same frame" for three words is "within half a frame (40 ms) of the first word", which is equality on the runtime's 80 ms grid and robust to the clock map's ±1 ms rounding (on the stored meetings every gap under 400 ms between two identical consecutive words is 0, 80, 159, 160, 240, 241, 319, 320 or 399 ms, and no piece of 4 or more has a median between 81 and 119 ms, so the literal "at most 80 ms" for the median and this reading of "one frame" decide the same pieces as any tolerance would); and the kept word ends at the latest end in its burst. The burst's 80 ms is literal too: an 81 ms gap (a one-frame step jittered by the clock map) ends a burst, so four words at 0, 80, 161 and 241 ms are kept although the plan's median (80) takes them for a loop; no gap of 81 ms between identical words occurs on the stored meetings.

Measured with `levers.py loops`, which applies both rules side by side and, since the review, normalises tokens exactly as the Rust `norm` does (section 6).

## 1. The owner's meetings 12 to 21 (stored transcripts)

Source: a copy of the data root's database with its `-wal` and `-shm` (read only at the source, deleted after), each meeting's current run, each channel's words in `(start, id)` order with the engine label of its own segment's speaker row, as a rebuild reads them (`levers.py loops --db`). Meeting 19 is a file import. Columns (the same in sections 2 and 5): pairs of identical consecutive words (in brackets, those one frame apart or less); runs of exactly 3 (in one frame / one-frame spacing); pieces of 4 or more by median start gap; runs with a gap of more than 400 ms inside (in brackets, those with a loop); the loops by the plan's rule (4 or more / 3 in a frame) and the words that rule removes; the bursts the rule as built collapses (4 or more / 3 in a frame) and the words it removes; and "4+ kept", pieces of 4 or more at human pace, with their words.

| Meeting | Channel | Words | Pairs (one frame) | Runs of 3 (same frame / one-frame spacing) | Pieces of 4+ (median ≤ 80 / 81–160 / > 160 ms) | Runs split at 400 ms (with a loop) | Loops, plan's rule (4+ / 3 in a frame) | Removed, plan's rule | Bursts as built (4+ / 3 in a frame) | Removed as built | 4+ kept (words) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 12 | loopback | 13,551 | 146 (34) | 10 (1 / 0) | 10 (8 / 0 / 2) | 42 (1) | 8 / 1 | 72 | 6 / 3 | 63 | 2 (8) |
| 12 | mic | 14,627 | 170 (38) | 20 (3 / 0) | 6 (6 / 0 / 0) | 54 (1) | 6 / 3 | 72 | 4 / 3 | 62 | 0 |
| 13 | loopback | 928 | 2 (0) | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 13 | mic | 386 | 4 (0) | 1 (0 / 0) | 0 | 3 (0) | 0 | 0 | 0 | 0 | 0 |
| 14 | loopback | 1,055 | 25 (7) | 2 (0 / 0) | 1 (1 / 0 / 0) | 11 (1) | 1 / 0 | 22 | 1 / 0 | 19 | 0 |
| 14 | mic | 340 | 3 (0) | 0 | 0 | 2 (0) | 0 | 0 | 0 | 0 | 0 |
| 15 | loopback | 852 | 1 (0) | 0 | 0 | 3 (0) | 0 | 0 | 0 | 0 | 0 |
| 15 | mic | 187 | 7 (0) | 0 | 0 | 5 (0) | 0 | 0 | 0 | 0 | 0 |
| 16 | loopback | 1,010 | 6 (0) | 0 | 1 (1 / 0 / 0) | 3 (0) | 1 / 0 | 3 | 0 / 1 | 2 | 0 |
| 16 | mic | 293 | 4 (0) | 0 | 0 | 2 (0) | 0 | 0 | 0 | 0 | 0 |
| 17 | loopback | 1,031 | 6 (0) | 0 | 0 | 1 (0) | 0 | 0 | 0 | 0 | 0 |
| 17 | mic | 160 | 2 (0) | 0 | 0 | 1 (0) | 0 | 0 | 0 | 0 | 0 |
| 18 | loopback | 1,435 | 17 (2) | 1 (0 / 0) | 2 (0 / 1 / 1) | 15 (0) | 0 | 0 | 0 | 0 | 2 (8) |
| 18 | mic | 757 | 17 (2) | 2 (0 / 0) | 4 (4 / 0 / 0) | 26 (2) | 4 / 0 | 24 | 3 / 1 | 23 | 0 |
| 19 | source | 258 | 3 (0) | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 20 | loopback | 10,351 | 93 (11) | 7 (0 / 0) | 7 (2 / 1 / 4) | 38 (0) | 2 / 0 | 29 | 2 / 0 | 27 | 5 (21) |
| 20 | mic | 1,245 | 15 (0) | 3 (1 / 0) | 2 (1 / 0 / 1) | 9 (1) | 1 / 1 | 5 | 0 / 2 | 4 | 1 (5) |
| 21 | loopback | 7,666 | 164 (25) | 32 (0 / 1) | 12 (3 / 2 / 7) | 50 (0) | 3 / 0 | 16 | 3 / 0 | 15 | 9 (38) |
| 21 | mic | 288 | 0 | 0 | 0 | 2 (0) | 0 | 0 | 0 | 0 | 0 |
| all | | 56,420 | 685 (119) | 78 (5 / 1) | 45 (26 / 4 / 15) | 267 (6) | 26 / 5 | 243 | 19 / 10 | **215** | 19 (80) |

Pieces by length, median start gap p10 / p50 / p90: 3 words 80 / 240 / 360 ms (78 pieces); 4 or 5 words 0 / 240 / 320 ms (28); 6 or more 0 / 0 / 0 ms (17). Of the 31 loops, 19 keep words at human pace under the rule as built, 28 words in all; no loop is a train of adjacent bursts with nothing between them (merging such trains as well would remove the same 215). Collapsed bursts that cross a segment boundary: 1; split by a speaker label: none; followed by a word that starts with the loop's token (a false start of the next word): 5.

**Removal counts against the plan's expectation.** The plan expected about 250 words (12: 145, 14: 22, 16: 3, 18: 27, 20: 34, 21: 20, per segment): the draft rule before the review (no 400 ms split, three words at a median of one frame), which `levers.py` reproduces exactly, and the reviewer's 254 over the channel stream too.

| Count | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | Total |
|---|---|---|---|---|---|---|---|---|---|---|---|
| the rule as built (frame-level bursts), channel stream | 125 | 0 | 19 | 0 | 2 | 0 | 23 | 0 | 31 | 15 | **215** |
| the rule as built, within one segment | 124 | 0 | 19 | 0 | 2 | 0 | 23 | 0 | 31 | 15 | 214 |
| the plan's rule (a loop goes whole), channel stream | 144 | 0 | 22 | 0 | 3 | 0 | 24 | 0 | 34 | 16 | 243 |
| the plan's rule, within one segment | 143 | 0 | 22 | 0 | 3 | 0 | 24 | 0 | 34 | 16 | 242 |
| draft rule, channel stream (the reviewer's count) | 146 | 0 | 23 | 0 | 3 | 0 | 28 | 0 | 34 | 20 | 254 |
| draft rule, within one segment (the plan's expectation) | 145 | 0 | 22 | 0 | 3 | 0 | 27 | 0 | 34 | 20 | 251 |

From the draft to the plan's rule, the review's two should-fix items (on meeting 21 two triples not in one frame stay; in meeting 18's "me" two runs have a pause over 400 ms inside); from the plan's rule to the rule as built, the 28 human-pace words of 19 loops.

**EN-only-insertion share** (target at least 90 percent). The word-error analysis of these meetings (measurement 3, private) records every run of 4 or more identical tokens that Whisper distil-large-v3 has fewer than 3 copies of, for the stored post-pass loopback and final "me" and for Nemotron 3.5's transcripts of the same channels. A removed token counts as confirmed when its piece overlaps such a record and Nemotron 3.5 has no loop there (EN only). For the plan's rule (measured on 26 Oct with the records):

| Channel | Words removed | In a loop the analysis recorded | EN only (no Nemotron 3.5 loop there) | Not labelled |
|---|---|---|---|---|
| loopback | 142 | 131 (92.3 %) | 131 (92.3 %) | 11 (2 in 3-in-a-frame pieces) |
| mic ("me") | 101 | 89 (88.1 %) | 89 (88.1 %) | 12 (6 in 3-in-a-frame pieces) |
| both | 243 | 220 (**90.5 %**) | 220 (**90.5 %**) | 23 |

The analysis's per-loop records went with the private analysis and are no longer on this machine, so the share of the rule as built cannot be recounted token by token; it is bounded instead. The rule as built removes a subset of these 243 words: the same words from every 3-in-a-frame piece (8 of the 23 unlabelled tokens sit there) and 28 fewer from pieces of 4 or more, which hold the other 15 unlabelled tokens. So at least 220 − 28 = 192 and at most 220 − (28 − 15) = 207 of its 215 words are confirmed EN-only insertions:

| Channel | Removed as built | EN only, at least | EN only, at most |
|---|---|---|---|
| loopback | 126 | 115 (91.3 %) | 124 (98.4 %) |
| mic ("me") | 89 | 77 (86.5 %) | 83 (93.3 %) |
| both | 215 | 192 (**89.3 %**) | 207 (**96.3 %**) |

The target holds unless nearly all of the 28 words the refinement keeps sat in loops the analysis confirmed; section 2 suggests most of them are indeed insertions on Zoom audio (the cost of the refinement), and section 5 that on human-checked references most are real. Caveat as before: 23 of the 220 labelled tokens sit in records where Whisper has 2 copies of the token, so up to about one word per such record may be a real repeat.

## 2. The evaluation set (36 sessions)

The cached runtime words JSON of every session of the shipped EN configuration (`rs_en_shipped`, the respin measurement's copy of `shipped_en`), converted as the file import stores it (microseconds, then milliseconds rounded, clamped to the audio, labels by first appearance; `levers.py loops --configs rs_en_shipped`). Per group:

| Group | Words | Pairs (one frame) | Runs of 3 (same frame / one-frame spacing) | Pieces of 4+ (median ≤ 80 / 81–160 / > 160 ms) | Runs split at 400 ms (with a loop) | Loops, plan's rule (4+ / 3 in a frame) | Removed, plan's rule | Bursts as built (4+ / 3 in a frame) | Removed as built | 4+ kept (words) |
|---|---|---|---|---|---|---|---|---|---|---|
| tuning (2) | 13,256 | 138 (16) | 13 (0 / 1) | 4 (3 / 0 / 1) | 83 (1) | 3 / 0 | 14 | 2 / 1 | 10 | 1 (4) |
| held-out (2) | 15,617 | 136 (32) | 9 (0 / 0) | 2 (2 / 0 / 0) | 35 (1) | 2 / 0 | 7 | 0 / 2 | 4 | 0 |
| control (1) | 6,353 | 63 (9) | 7 (0 / 1) | 0 | 17 (0) | 0 | 0 | 0 | 0 | 0 |
| stress (1) | 12,193 | 121 (23) | 12 (0 / 0) | 6 (4 / 0 / 2) | 25 (0) | 4 / 0 | 20 | 3 / 1 | 18 | 2 (8) |
| overlap, AMI (1) | 2,447 | 15 (2) | 1 (1 / 0) | 0 | 12 (0) | 0 / 1 | 2 | 0 / 1 | 2 | 0 |
| talks (7) | 55,309 | 643 (88) | 69 (3 / 2) | 19 (7 / 4 / 8) | 168 (0) | 7 / 3 | 55 | 6 / 3 | 46 | 12 (52) |
| broad (22) | 165,528 | 1,491 (218) | 119 (4 / 10) | 59 (42 / 6 / 11) | 563 (11) | 42 / 4 | 423 | 35 / 7 | 358 | 17 (72) |
| all 36 | 270,703 | 2,607 (388) | 230 (8 / 14) | 90 (58 / 10 / 22) | 903 (13) | 58 / 8 | 521 | 46 / 15 | **438** | 32 (136) |

Pieces by length, median start gap p10 / p50 / p90: 3 words 80 / 200 / 320 ms (230 pieces); 4 or 5 words 0 / 160 / 240 ms (50); 6 or more 0 / 0 / 0 ms (40). The plan's rule removes 521 words in 66 loops; the rule as built 438 in 61 bursts, keeping 83 human-pace words in 41 of the loops (one loop is a train of two adjacent bursts; merging trains as well would remove 439). Jul 3 has 142 of the 438 (165 by the plan's rule). Followed by a word that starts with the token: 10. Nemotron 3.5's cached words of the same sessions (`rs_n35_shipped`): 3 loops and 13 words by the plan's rule, 3 bursts and 10 words as built.

**Does the reference have the repeat?** The reference's copies of the loop's token (the harness's normaliser; Zoom cue times interpolated by word length) within 1 s of each piece; for the loops whose human-pace words the rule as built keeps, against the copies it keeps:

| Pieces | Count | Filler (no reference token) | Reference has at most 1 | 2 to n − 1 | n or more |
|---|---|---|---|---|---|
| bursts collapsed as built | 61 | 2 | 52 | 7 | **0** |
| 4 or more, kept by the timing rule | 32 | 0 | 21 | 4 | 7 |
| loops whose human-pace words the rule as built keeps (n: the copies kept) | 41 | 0 | 30 | 5 | 6 |

No collapsed burst is a repeat the reference has in full. Of the 41 loops where the rule as built keeps more than the plan's rule, the Zoom reference has every kept copy in 6 and at most one copy in 30: on these sessions most of the 83 words the refinement keeps look like insertions, which is its cost (section 3). Zoom's own transcript is machine output and drops repeats readily, so "the reference has it" is evidence, not proof; section 5 checks the same against human-checked references.

## 3. The eval gate: the full set re-scored through the file import

Plan 16 section 12.3, measurement 2, without the GPU. Every session's cached runtime words JSON of `rs_en_shipped` went through the app's own file import (`pipeline::import_file`) with a replay engine that returns the cached words (chunk B's ignored `cli_check_root` driver) into scratch data roots without a punctuation model: one with `transcript.collapse_loops = off`, one with the plan's rule (the chunk's first build, 26 Oct) and one with the rule as built (27 Oct), both `on`. Then `diarist-cli export --format json` and `--format rttm` of each meeting (a debug build), the source run's diarizer RTTM beside it, and `run.py --metrics-only` over the exports (configs `shipped_en_replay_off`, `shipped_en_replay_on` and `shipped_en_replay_refined` in a scratch copy of `configs.toml`; each `run.json` names the replayed source and has no speed or VRAM fields).

**The replay is faithful:** with the collapse off it equals the GPU run on all 432 values compared (cpWER, tcpWER, DI-cpWER, ORC-WER, WDER, WDER near changes, DER at both collars, JER, speaker-count error, hypothesis speakers and words, on all 36 sessions).

| Session | Group | Removed, plan's rule | Removed as built | cpWER off | Δ plan's rule | cpWER as built | Δ as built | tcpWER Δ plan's rule | tcpWER Δ as built | DER c0.25 Δ as built | JER Δ as built |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | tuning | 0 | 0 | 9.516 | +0.000 | 9.516 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 9 | tuning | 14 | 10 | 8.111 | −0.184 | 7.958 | −0.153 | −0.184 | −0.153 | −0.004 | −0.001 |
| Sep 7 | held-out | 7 | 4 | 7.223 | −0.157 | 7.137 | −0.086 | −0.157 | −0.086 | +0.000 | +0.000 |
| Sep 5 | held-out | 0 | 0 | 4.698 | +0.000 | 4.698 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Sep Logistics | control | 0 | 0 | 7.031 | +0.000 | 7.031 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 14 | stress | 20 | 18 | 43.936 | −0.164 | 43.788 | −0.148 | −0.164 | −0.148 | +0.000 | +0.000 |
| AMI ES2002a | overlap | 2 | 2 | 18.032 | +0.000 | 18.032 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Talk 1 | talks | 0 | 0 | 6.601 | +0.000 | 6.601 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Talk 2 | talks | 11 | 9 | 7.200 | −0.147 | 7.080 | −0.121 | −0.147 | −0.121 | +0.007 | +0.003 |
| Talk 3 | talks | 18 | 13 | 12.790 | −0.268 | 12.575 | −0.215 | −0.268 | −0.215 | +0.009 | +0.002 |
| Talk 4 | talks | 14 | 12 | 30.230 | −0.145 | 30.106 | −0.124 | −0.145 | −0.124 | +0.000 | +0.000 |
| Talk 5 | talks | 0 | 0 | 5.329 | +0.000 | 5.329 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Talk 6 | talks | 10 | 10 | 14.774 | −0.165 | 14.609 | −0.165 | −0.165 | −0.165 | +0.000 | +0.000 |
| Talk 7 | talks | 2 | 2 | 14.817 | +0.000 | 14.817 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 1 | broad | 0 | 0 | 4.090 | +0.000 | 4.090 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 3 | broad | 8 | 3 | 10.034 | −0.108 | 9.993 | −0.040 | −0.108 | −0.040 | +0.000 | +0.000 |
| Aug 4 | broad | 8 | 7 | 7.712 | −0.079 | 7.617 | −0.095 | −0.079 | −0.095 | +0.000 | +0.000 |
| Aug 5 | broad | 23 | 20 | 5.170 | −0.247 | 4.958 | −0.211 | −0.247 | −0.211 | +0.000 | +0.000 |
| Aug 6 | broad | 0 | 0 | 5.179 | +0.000 | 5.179 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 7 | broad | 28 | 27 | 35.320 | −0.302 | 35.005 | −0.315 | −0.302 | −0.315 | +0.002 | +0.008 |
| Aug 8 | broad | 0 | 0 | 6.786 | +0.000 | 6.786 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 10 | broad | 0 | 0 | 6.435 | +0.000 | 6.435 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 11 | broad | 40 | 38 | 5.713 | −0.420 | 5.268 | −0.445 | −0.420 | −0.445 | −0.020 | −0.007 |
| Aug 12 | broad | 5 | 2 | 4.373 | −0.070 | 4.345 | −0.028 | −0.070 | −0.028 | −0.009 | −0.002 |
| Aug 13 | broad | 0 | 0 | 5.742 | +0.000 | 5.742 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Aug 15 | broad | 30 | 27 | 8.435 | −0.412 | 8.040 | −0.395 | −0.412 | −0.395 | +0.000 | +0.000 |
| Sep 1 | broad | 0 | 0 | 8.819 | +0.000 | 8.819 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Sep 2 | broad | 8 | 8 | 6.582 | −0.112 | 6.470 | −0.112 | −0.112 | −0.112 | +0.000 | +0.000 |
| Sep 3 | broad | 3 | 3 | 6.916 | −0.037 | 6.879 | −0.037 | −0.037 | −0.037 | +0.000 | +0.000 |
| Sep 4 | broad | 17 | 16 | 8.387 | −0.229 | 8.172 | −0.215 | −0.229 | −0.215 | +0.008 | +0.003 |
| Sep 6 | broad | 0 | 0 | 4.846 | +0.000 | 4.846 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Jul 1 | broad | 3 | 2 | 9.651 | −0.050 | 9.601 | −0.050 | −0.050 | −0.050 | +0.000 | +0.000 |
| Jul 2 | broad | 34 | 30 | 6.729 | −0.468 | 6.290 | −0.439 | −0.468 | −0.439 | −0.016 | −0.008 |
| Jul 3 | broad | 165 | 142 | 9.083 | −1.390 | 7.898 | −1.186 | −1.390 | −1.186 | −0.021 | −0.006 |
| Jul 4 | broad | 0 | 0 | 5.369 | +0.000 | 5.369 | +0.000 | +0.000 | +0.000 | +0.000 | +0.000 |
| Jul 5 | broad | 51 | 33 | 12.144 | −0.621 | 11.699 | −0.445 | −0.621 | −0.445 | −0.007 | −0.005 |
| **mean, all 36** | | 521 | 438 | **10.383** | **−0.160** | **10.244** | **−0.140** | −0.160 | −0.140 | −0.001 | −0.000 |
| mean, tuning (2) | | 14 | 10 | 8.814 | −0.092 | 8.737 | −0.077 | −0.092 | −0.077 | −0.002 | −0.001 |
| mean, held-out (2) | | 7 | 4 | 5.960 | −0.079 | 5.918 | −0.043 | −0.079 | −0.043 | +0.000 | +0.000 |
| mean, talks (7) | | 55 | 46 | 13.106 | −0.104 | 13.017 | −0.089 | −0.104 | −0.089 | +0.002 | +0.001 |
| mean, broad (22) | | 423 | 358 | 8.342 | −0.207 | 8.159 | −0.182 | −0.207 | −0.182 | −0.003 | −0.001 |

Percent, at the full precision of `metrics.json`; Δ is the rule's run minus off, in points.

| Bar (plan 16 section 12.3) | Plan's rule | Rule as built | |
|---|---|---|---|
| cpWER on no session worse by more than 0.05 | no session worse at all: 21 better (Jul 3 the most, −1.390), 15 unchanged | no session worse at all: 21 better (Jul 3 the most, −1.186), 15 unchanged | met by both |
| cpWER mean equal or better | 10.383 → 10.223 (−0.160) | 10.383 → 10.244 (−0.140) | met by both |
| tcpWER the same | no session worse; mean −0.160 | no session worse; mean −0.140 | met by both |
| DER and JER unchanged within noise (a collapsed burst keeps its span) | DER c0.25 −0.020 to +0.016 points, DER c0 −0.019 to +0.023, JER −0.008 to +0.010; speaker counts unchanged | DER c0.25 −0.021 to +0.009, DER c0 −0.020 to +0.018, JER −0.008 to +0.008; speaker counts unchanged | met by both |

**What the refinement costs here.** Against the plan's rule, the rule as built removes 83 fewer words and gives back 0.021 points of the mean cpWER gain (13 percent of it): 14 sessions are a little worse than under the plan's rule (Jul 3 +0.204, Jul 5 +0.176, the rest +0.071 or less), 3 a little better (Aug 11 −0.025, Aug 4 −0.016, Aug 7 −0.013: there the reference has words the plan's rule took), and the other 19 score the same under both (on Jul 1 the rule as built removes one word fewer at no cost). That fits section 2: on the Zoom references most of the human-pace words the refinement keeps are insertions. Section 5 shows the other side, on human-checked references. Two sessions lose words with cpWER unchanged under both rules: on AMI the manual reference has the triple's word twice at that point, so one insertion became one deletion (WDER −0.042); Talk 7's triple is a filler, which the harness's normaliser drops. DI-cpWER and ORC-WER move with cpWER (−1.186 to +0.038 as built; the +0.038 is one session's re-alignment, as under the plan's rule). The small DER and JER moves fit segment boundaries moving (the hypothesis RTTM is per segment, and with fewer words the 80-word segment split falls elsewhere in a long stretch); the kept word spans its burst, so no speech time is lost.

## 4. The Rust path against the measurement

The import's `engine_info.loops` removes exactly what `levers.py` predicts on every session, for both rules: as built 438 words in 61 bursts, the plan's rule (the first replay) 521 in 66 loops, session by session (36 of 36 equal for each). With the collapse off every run records `applied: false` and removes nothing.

## 5. Other languages and the public sets (review note 3)

Every cached run with a reference outside the Zoom set, read only: the FLEURS runs of the echo worktree's `.work/runs` (Nemotron 3.5 in `pn_shipped_n35` on all eight languages there, German, English, Spanish, French, Hindi, Italian, Japanese and Portuguese, about 10 minutes each with FLEURS's own transcription as the reference; the EN model and the respin variants on English) and the public-set runs of the data worktree's `.work/runs` (`shipped_en` and `shipped_n35` on AMI EN2002a to d, Mix-Headset and Array1-01, ICSI Bed004, Bed016, Bmr019 and Bro018, and NOTSOFAR MTG_32000, all English, with their human word-level references). `levers.py loops --sessions present[:fleurs-] --runs-dir <runs> --fleurs-root <fleurs>` or `--extra-sessions <the set's sessions.toml> --extra-root <worktree>`; the tokens through the Rust norm (section 6).

**FLEURS: nothing to lose in any language.** No session of any configuration has a run of 3 or more identical words; there are 2 pairs in all (Spanish and French, Nemotron 3.5), neither one frame apart. Either rule removes 0 words in every language:

| Language (FLEURS, Nemotron 3.5) | de | en | es | fr | hi | it | ja | pt |
|---|---|---|---|---|---|---|---|---|
| Words | 877 | 1,178 | 1,182 | 1,286 | 1,149 | 967 | 51 | 1,048 |
| Removed, plan's rule / as built | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

FLEURS is read sentences, so it holds neither engine loops nor reduplication; it shows the collapse does nothing to clean speech in these languages, not how it treats a real reduplication. No cached run covers Vietnamese, Russian or another language where reduplication is common, nor the public non-English sets (AliMeeting, SUMM-RE, Indic DiarBench have no runs yet). By construction the rule needs the decoder's timing, consecutive starts at most one 80 ms frame apart (4 or more) or 3 on one frame, which a speaker cannot produce for whole words, and Han, Kana and Thai are excluded outright; so a reduplication at speaking pace ("да да да", a Vietnamese reduplicative) stays whatever its language.

**The public English sets: the refinement is right where the plan's rule was not.** By the plan's rule / as built, the EN model loses 192 / 167 words on the 13 public sessions, Nemotron 3.5 93 / 83 on 11:

| Pieces (EN model, 13 sessions) | Count | Reference has at most 1 | 2 to n − 1 | n or more |
|---|---|---|---|---|
| bursts collapsed as built | 28 | 12 | 16 | **0** |
| 4 or more, kept by the timing rule | 11 | 0 | 7 | 4 |
| loops whose human-pace words the rule as built keeps (n: the copies kept) | 12 | 1 | 4 | **7** |

Every piece was read against its reference with a private inspection script (no text here). The long bursts are engine loops: 20 copies of one word in two frames where the reference has 2, 32 copies of another where it has 3, ten copies of a spoken digit in one frame. In 7 of the 12 loops where the rule as built keeps human-pace words, the reference has every word it keeps: a loop of six where the reference has three and the rule as built keeps three, two loops of four where the reference has two and it keeps two, and so on; the plan's rule would have left one word in each. What both rules still take from real speech: in 16 of the 28 collapsed bursts the reference has the word two or more times nearby (the window is ±1 s, so real repeats next to a burst count too), and where a person said a word two or three times and the decoder stamped those copies on one frame with its insertions, the burst becomes one word; the clearest case is the 3-in-a-frame rule, where in 2 of the 5 such pieces the reference has the word twice, so one real copy goes with the inserted one. That is the plan's rule for bursts, which the refinement does not change.

**Decision: no language exclusion.** The only real speech lost in any covered language is these English copies stamped on one frame with an engine loop, which a per-language switch could not separate from the loops it is there to remove (English is the language the EN model loops in); every other covered language loses nothing, and the timing rule, not the language, is what protects a real repeat. Nothing is switched off per script or per language beyond the existing Han, Kana and Thai exclusion; `transcript.collapse_loops = off` remains the switch for someone who sees a loss.

## 6. levers.py's norm against Rust's (review note 8)

`loop_norm` used Python's `str.isalnum`, which is false for the combining vowel signs (categories Mn and Mc) that Rust's `char::is_alphabetic` counts as letters (the Unicode Other_Alphabetic property: the matras of Devanagari, Bengali, Tamil and the other Indic scripts, Thai and Lao vowels, and more), so on Hindi it cut words apart at every vowel sign and could take distinct words for one token; and `str.isspace` takes U+001C to U+001F, which Rust's `is_whitespace` does not. `levers.py` now carries the ranges where Rust's std tables (Unicode 17.0, rustc 1.99) and Python 3.12's `unicodedata` (Unicode 15.0) differ, generated by running both over every scalar value: 290 ranges where Rust is alphabetic and Python has no letter (Other_Alphabetic and the letters added after Unicode 15.0), 9 ranges of digits added after 15.0, none the other way; and Rust's White_Space list. Han, Kana and Thai now need a letter too, as `Script::of_char` does. `test_metrics.py` (`test_loop_norm_is_the_rust_norm`) and `loops.rs` (`norm_matches_the_measurement_script`) check the same nine pairs (a Hindi word with a vowel sign and an anusvara, Thai with a tone mark, Tamil with a virama, Cyrillic, a control character, a dotted capital I, a Roman numeral, a no-break space, Vietnamese), and `test_loop_rule_matches_loops_rs` the burst cases. Every count in sections 1 and 2 by the plan's rule is the same as on 26 Oct with the old norm (the English tokens do not change), and FLEURS Hindi has no loop with either.

## 7. Echo rows

Unchanged by construction: the collapse runs in `run_post_pass` after `echo_step` and the unknown-token check and touches no file under `crates/core/src/audio/` (the gate, the transfer gate and the canceller) or `live/echo_gate.rs`; `crates/core/tests/echo_sample.rs` calls the gate's functions directly and no code this chunk changed, so its rows for meetings 12 to 21 cannot move. `engine_info.echo` and `echo_dropped` are written from the gate's raw words, before the collapse (Restore adds `restored_merged` to `engine_info.echo`, a count of its own). Two GPU tests not run here, `aec_postpass.rs` and `echo_fleurs.rs`, go through `run_post_pass` and count the final stored words of "me", so their kept-word and word-error figures see the collapse after this chunk (their `engine_info.echo` rows do not); to compare with their earlier tables, run them with `transcript.collapse_loops = off` in the test store.

## 8. Reproduce

```
# the owner's meetings, on a copy of the database with its -wal and -shm (numbers only)
uv run python -I levers.py loops --db <copy of diarist.db> --meetings 12-21 [--analysis <the analysis's loops JSON>]
# the eval set
uv run python -I levers.py --sessions-file <sessions.toml> loops --configs rs_en_shipped,rs_n35_shipped --runs-dir <.work/runs>
# FLEURS, every language cached
uv run python -I levers.py --sessions-file <sessions.toml> loops --configs pn_shipped_n35,pn_shipped_en --sessions present:fleurs- --runs-dir <.work/runs> --fleurs-root <.work/fleurs>
# the public sets
uv run python -I levers.py loops --configs shipped_en,shipped_n35 --sessions present --runs-dir <data worktree>/tools/eval/.work/runs --extra-sessions <data worktree>/tools/eval/.work/public/{ami,icsi,notsofar}/sessions.toml --extra-root <data worktree>
```

The replay and comparison scripts (`replay.sh <off|on|refined>`, `runinfo.py`, `compare.py`, `compare3.py`, `rust_vs_levers.py`, `configs.replay.toml`) lived in the gitignored `tools/eval/.work/chunk-k/` of the chunk's worktree and were deleted with the replay roots afterwards, since they held eval-session transcripts; `replay.sh` imports each session's cached words through chunk B's ignored `cli_check_root` test into a scratch data root with the setting under test and exports JSON and RTTM with a debug `diarist-cli`.
