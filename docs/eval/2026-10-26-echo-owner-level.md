# Eval: echo gate, the owner's level after the canceller and the floor rule (2026-10-26)

Plan 16 section 12.2 (D7, fix 1) as chunk I of section 12.5: the design in 12.2.2 to 12.2.4, the reviewer's findings B1 to B3 in 12.6. Release build, AMD Ryzen 9 5900X; the GPU runs (EN model, lookahead 13, `v3-offline`) went through the shared queue, one batch each. A copy of the database and of the meeting folders, read only; numbers per meeting number only, no titles, names or transcript text.

What changed (`crates/core/src/audio/echo/{layers,transfer}.rs`, `crates/core/src/live/postpass.rs` inside `gate_mic`), as fixed after the review ruling of plan 16 section 12.7 (27 Oct; the first build's P = 15 and single F = 18 are in this note's history):

- **The owner's level after the canceller** (`CalibrationStats::owner`): the median level of the words in no text run that the canceller cut by under 3 dB, spoken while the far end was silent (`quiet`), else of all such words (`clean`), from 20 or more. A set whose median sits under the floor plus `OWNER_PLAUSIBLE_DB` (P = 6 dB, the presence margin: a level that close to the floor is not near-end speech) does not stop the search, the next set is tried; the level is unknown when every set of 20 or more words is that close (`implausible`) or none has 20 (`too_few`). An estimate from 6 to 15 dB over the floor is trusted and marked `owner_low`. Before the canceller (the raw gate of the transfer, the live view, Stop) the estimator is the earlier one: quiet words, else every word in no run (`unmatched`), else `too_few`; no plausibility check.
- **The transfer guard without the owner's level, in two tiers** (`transfer::Rule::floor_rule`): a candidate goes when the canceller cut it by 3 dB or more and its level after the canceller is at most F over the cancelled mic's floor, F = `F_u` (`FLOOR_MARGIN_DB`) in general and `F_s` = 18 dB (`FLOOR_MARGIN_SILENT_DB`) when two signatures of a silent owner agree: the owner's level is `implausible` and the raw gate flagged at least 99.5 percent of the raw mic's words (`SILENT_SHARE`). With a known level the guard is the earlier one (12 dB under the owner).
- **Recorded**: `engine_info.echo.owner_source`, `owner_words` and `owner_low`; in `engine_info.echo.transfer` the raw gate's `raw_owner_db`, `raw_floor_db` and `raw_echo_share`, the counts with `floor`, and `floor_rule: {tier, f_db, removed}` (`tier` `unknown` or `silent`, `tier` and `f_db` null with the owner's level known).

Method. `echo_sample.rs` replays both paths of every stored meeting on the CPU from `playback.flac`, the stored words and the database copy (live revisions in arrival order); "today" is the same decision with today's estimator forced in (`gate_with`), which reproduces the stored `engine_info.echo` of meetings 17, 18, 20 and 21 exactly (meeting 21: 1,993 candidates, 1,989 guarded, 288 kept). Meeting 12's cancelled mic was transcribed once on the GPU (48 s), meeting 18's raw mic once (6 s, for the 12.2.4 variant). `echo_fleurs.rs` and `aec_postpass.rs` ran the real post-pass on the GPU (FLEURS 20 minutes over meeting 12's far end; meeting 12 off and auto and the spliced set), the F sweep in-test over the same words. The meeting 12 measurement's per-word classes file is gone; it was rebuilt from the stored transcript by the same matcher (`DIARIST_ECHO_CLASSES_OUT`): 14,627 words, 12,260 duplicates, 1,462 owner's words, as in docs/eval/2026-10-20-echo.md. "Far-active cut kept" (12.2.3's bar): kept "me" words spoken while the far end was active that the canceller cut by 3 dB or more. "Owner's" is the matcher's class (non-duplicates whose ±5 neighbours are at most 30 percent duplicates); "clean owner's" those of them the canceller cut by under 3 dB.

## 1. First step: where the owner's level comes from

Before any threshold was set (12.2.3, B3). The sets are the words in no text run: quiet (the far end silent), unmatched (all), clean quiet and clean (of them, cut by under 3 dB; only after a canceller). "Today" is the estimator before this change, "shipped" the new one; the level is the median in dBFS, then the words it came from and its distance over the floor of the mic that was transcribed.

| Recording | Path | Words | Quiet | Unmatched | Clean quiet | Clean | Today | Shipped | Floor |
|---|---|---|---|---|---|---|---|---|---|
| 4, 5 (silent mic) | post-pass, Stop | 0 | 0 | 0 | 0 | 0 | too few | too few | – |
| 12 | post-pass (cancelled) | 2,544 | 1,186 | 1,965 | 1,186 | 1,559 | quiet −33.4 (1,186; +30.0) | quiet −33.4 (1,186; +30.0) | −63.4 |
| 12 | raw gate (stored raw transcript) | 14,627 | 1,228 | 2,791 | – | – | quiet −33.6 (1,228; +29.1) | the same | −62.6 |
| 12 | Stop (stand-in) | 14,627 | 1,228 | 2,791 | – | – | quiet −33.6 (1,228; +29.1) | the same | −62.6 |
| 13 | post-pass (no canceller) | 386 | 281 | 321 | – | – | quiet −35.7 (281; +27.0) | the same | −62.6 |
| 13 | Stop | 375 | 288 | 334 | – | – | quiet −35.2 (288; +27.4) | the same | −62.6 |
| 14 | post-pass (cancelled) | 369 | 294 | 330 | 294 | 307 | quiet −31.5 (294; +30.1) | quiet −31.5 (294; +30.1) | −61.6 |
| 14 | raw gate (live words) | 1,336 | 301 | 392 | – | – | quiet −31.4 (301; +28.8) | the same | −60.1 |
| 14 | Stop | 1,336 | 301 | 375 | – | – | quiet −31.4 (301; +28.8) | the same | −60.1 |
| 15 | post-pass (no canceller) | 187 | 181 | 187 | – | – | quiet −26.3 (181; +21.0) | the same | −47.4 |
| 15 | Stop | 187 | 182 | 187 | – | – | quiet −26.5 (182; +20.9) | the same | −47.4 |
| 16 | post-pass (cancelled) | 366 | 225 | 298 | 225 | 230 | quiet −20.1 (225; +27.3) | quiet −20.1 (225; +27.3) | −47.4 |
| 16 | raw gate (live words) | 1,196 | 227 | 307 | – | – | quiet −20.2 (227; +26.5) | the same | −46.6 |
| 16 | Stop | 1,196 | 227 | 308 | – | – | quiet −20.2 (227; +26.5) | the same | −46.6 |
| 17 | post-pass (cancelled) | 163 | 157 | 163 | 157 | 160 | quiet −24.3 (157; +27.9) | quiet −24.3 (157; +27.9) | −52.1 |
| 17 | raw gate (live words) | 1,168 | 162 | 217 | – | – | quiet −24.2 (162; +26.9) | the same | −51.1 |
| 17 | Stop | 1,168 | 162 | 213 | – | – | quiet −24.2 (162; +26.9) | the same | −51.1 |
| 18 | post-pass (cancelled) | 769 | 215 | 769 | 215 | 730 | quiet −22.7 (215; +29.7) | quiet −22.7 (215; +29.7) | −52.4 |
| 18 | raw gate (live words; raw transcribed) | 1,618; 1,653 | 205; 214 | 902; 868 | – | – | quiet −23.0 (205; +28.3); −22.9 (214; +28.5) | the same | −51.4 |
| 18 | Stop | 1,618 | 205 | 902 | – | – | quiet −23.0 (205; +28.3) | the same | −51.4 |
| 20 | post-pass (cancelled) | 2,626 | 1,053 | 1,671 | 1,053 | 1,243 | quiet −32.6 (1,053; +28.3) | quiet −32.6 (1,053; +28.3) | −60.9 |
| 20 | raw gate (live words) | 10,981 | 1,062 | 2,012 | – | – | quiet −32.5 (1,062; +27.1) | the same | −59.6 |
| 20 | Stop | 10,981 | 1,062 | 2,021 | – | – | quiet −32.5 (1,062; +27.1) | the same | −59.6 |
| **21** | **post-pass (cancelled)** | 2,063 | **16** | 422 | **16** | **35** | **unmatched −50.2 (422; +11.2)** | **implausible (35 clean words, median +3.3)** | −61.4 |
| 21 | raw gate (live words) | 7,611 | 4 | 431 | – | – | unmatched −29.0 (431; +31.1) | the same | −60.1 |
| 21 | Stop | 7,611 | 5 | 384 | – | – | unmatched −29.1 (384; +31.0) | the same | −60.1 |
| FLEURS, 55 utterances | post-pass (cancelled) | 1,182 | 378 | 1,182 | 378 | 1,079 | quiet −31.1 (378; +32.3) | quiet −31.1 (378; +32.3) | −63.4 |
| FLEURS | raw gate (the off run's words) | 2,413 | 386 | 1,182 | – | – | quiet −31.3 (386; +31.9) | the same | −63.1 |
| FLEURS (a): 10 utterances over the far end, −33 dBFS | post-pass (cancelled) | 214 | 5 | 214 | 5 | 190 | unmatched −31.4 (214; +32.2) | clean −31.2 (190; +32.5) | −63.6 |
| FLEURS (a) | raw gate | 2,284 | 8 | 317 | – | – | unmatched −32.7 (317; +30.7) | the same | −63.4 |
| FLEURS (b): the same, −44.5 dBFS | post-pass (cancelled) | 212 | 5 | 212 | 5 | 51 | unmatched −43.1 (212; +20.5) | clean −40.4 (51; +23.2) | −63.6 |
| FLEURS (b) | raw gate | 2,266 | 7 | 143 | – | – | unmatched −39.8 (143; +23.6) | the same | −63.4 |
| FLEURS (b′): the same, −38 dBFS | post-pass (cancelled) | 213 | 5 | 213 | 5 | 140 | unmatched −36.4 (213; +27.2) | clean −35.6 (140; +28.1) | −63.6 |
| FLEURS (b′) | raw gate | 2,272 | 7 | 275 | – | – | unmatched −36.2 (275; +27.2) | the same | −63.4 |
| Spliced set (meeting 12, 34 to 44 min) | post-pass (cancelled) | 659 | 11 | 652 | 11 | 547 | unmatched −33.9 (652; +29.5) | clean −32.9 (547; +30.4) | −63.4 |
| Spliced set | raw gate (the raw window transcribed) | 1,173 | 18 | 666 | – | – | unmatched −33.3 (666; +29.3) | the same | −62.6 |
| Spliced set (c): the 11 quiet owner's words out | post-pass (cancelled) | 648 | 0 | 641 | 0 | 536 | unmatched −33.9 (641; +29.5) | clean −32.9 (536; +30.5) | −63.4 |

Read: meetings 12 to 20 keep their estimate to the hundredth: after the canceller every quiet word in no run is also one it left alone (clean quiet = quiet), and no raw path moved. Meeting 21 is the case of 12.2.2: 16 quiet words (under 20) sent today's estimator to all 422 words in no run, the residue, 11.2 dB over the floor; 35 of them were left alone by the canceller, and their median sits 3.3 dB over the floor, under the presence margin of 6 dB, so the level is unknown (`implausible`; the first build's P = 15 gave the same here, and no recording has an estimate between 6 and 15 dB over the floor, so P = 6 moves no row of this note). The FLEURS fixture (a), whose ten owner utterances all lie over far-end speech, has only 5 quiet words, as the reviewer expected of such a fixture: today's estimator falls back to all words in no run (here the owner's own, +32.2), the shipped one to the 190 clean words (+32.5), so the owner is known and the fixture's `owner_source` is `clean`, not unknown. No fixture with ten owner utterances reaches the unknown regime by itself, because the owner's words over echo are mostly cut by under 3 dB (190 of 212 on (a)); the natural-unknown runs of section 3 (three and one utterances) do. Fixtures (a), (b) and (c) are therefore run **forced unknown**: the post-canceller owner's level set to unknown in the in-test decision (`gate_with`), every other feature as the post-pass computes it. The spliced set confirms the reviewer's B3: its owner's clips sit over the window with the most far-end speech, only 11 of its words in no run were spoken while the far end was silent, and today's estimator passed through the `unmatched` fallback (−33.9 dBFS, which happened to be the owner's own level because the owner's words dominate the window); the shipped one takes the 547 clean words (−32.9) and keeps the case right, also with the 11 quiet owner's words removed (536 clean words, −32.9): so variant (c), too, needs the owner's level forced unknown.

## 2. Histograms of the level over the cancelled mic's floor

Counts of words per 3 dB bin of level after the canceller over its floor; "candidates cut" are transfer candidates (overlapping flagged raw words only) the canceller cut by 3 dB or more, the words the floor rule acts on; decisions are today's.

| Words | n | < 0 | 0–3 | 3–6 | 6–9 | 9–12 | 12–15 | 15–18 | 18–21 | 21–24 | 24–27 | 27–30 | ≥ 30 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 21: kept, candidates cut | 239 | 3 | 21 | 35 | 33 | 41 | 51 | 27 | 17 | 6 | 3 | 2 | 0 |
| 21: kept, far-active cut | 253 | 3 | 23 | 37 | 34 | 46 | 53 | 27 | 17 | 7 | 3 | 3 | 0 |
| 12: clean owner's | 1,524 | 0 | 3 | 19 | 10 | 9 | 6 | 18 | 41 | 99 | 194 | 315 | 810 |
| 14: clean owner's | 301 | 0 | 2 | 4 | 0 | 1 | 2 | 3 | 3 | 15 | 41 | 74 | 156 |
| 16: clean owner's | 228 | 0 | 0 | 4 | 3 | 1 | 0 | 5 | 21 | 25 | 51 | 69 | 49 |
| 17: clean owner's | 160 | 0 | 0 | 3 | 0 | 0 | 0 | 2 | 3 | 18 | 45 | 46 | 43 |
| 18: clean owner's | 706 | 0 | 4 | 3 | 5 | 4 | 7 | 13 | 24 | 63 | 115 | 161 | 307 |
| 20: clean owner's | 1,193 | 0 | 6 | 9 | 10 | 3 | 15 | 16 | 60 | 119 | 214 | 277 | 464 |
| 12: owner's, candidates cut | 55 | 1 | 15 | 12 | 3 | 6 | 1 | 7 | 3 | 0 | 4 | 1 | 2 |
| 14: owner's, candidates cut | 6 | 3 | 2 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 16: owner's, candidates cut | 20 | 0 | 14 | 2 | 3 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 17: owner's, candidates cut | 3 | 0 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 18: owner's, candidates cut | 13 | 0 | 1 | 2 | 1 | 0 | 1 | 6 | 2 | 0 | 0 | 0 | 0 |
| 20: owner's, candidates cut | 21 | 0 | 14 | 3 | 2 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 21: owner's, candidates cut | 6 | 0 | 3 | 2 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 |
| FLEURS: owner's words, candidates cut | 18 | 0 | 2 | 0 | 0 | 0 | 0 | 1 | 1 | 5 | 4 | 5 | 0 |
| FLEURS (a): owner's words, candidates cut | 16 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 4 | 7 | 3 |
| FLEURS (b): owner's words, candidates cut | 99 | 0 | 1 | 1 | 3 | 0 | 8 | 33 | 32 | 19 | 2 | 0 | 0 |
| FLEURS (b′): owner's words, candidates cut | 49 | 0 | 0 | 0 | 0 | 0 | 1 | 2 | 6 | 15 | 11 | 12 | 2 |
| Spliced: owner's words (in the clips), candidates cut | 6 | 0 | 1 | 0 | 1 | 0 | 0 | 1 | 2 | 0 | 1 | 0 | 0 |
| FLEURS: owner's words cut under 3 dB (median +32.2) | 1,079 | 0 | 9 | 1 | 2 | 0 | 2 | 6 | 4 | 13 | 53 | 164 | 825 |
| FLEURS (a): the same (median +32.5) | 190 | 0 | 1 | 0 | 0 | 1 | 1 | 0 | 2 | 2 | 10 | 32 | 141 |
| FLEURS (b): the same (median +23.2) | 51 | 0 | 0 | 1 | 1 | 1 | 0 | 2 | 8 | 15 | 19 | 4 | 0 |
| FLEURS (b′): the same (median +28.1) | 140 | 0 | 0 | 0 | 1 | 1 | 1 | 1 | 1 | 12 | 32 | 56 | 35 |
| Spliced: owner's words cut under 3 dB | 541 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 32 | 83 | 120 | 297 |

Read: the residue the floor rule is for (meeting 21's kept candidates) sits 0 to 21 dB over the floor, 184 of its 239 under 15 and 211 under 18. The owner's words the canceller left alone sit 18 dB or more over the floor: 94 to 100 percent of them on every meeting and fixture, 90 percent on (b). The owner's words among the cut candidates on the real meetings sit mostly low (0 to 12 dB: floor-level fragments under far-end speech, which the round-two notes read one by one as garbled echo), except on meeting 18 (8 of 13 at 15 to 21 dB) and meeting 12 (17 of 55 at 15 dB or more). On the fixtures the owner's real words among the cut candidates sit 18 to 30 dB over the floor ((a): all 16; FLEURS: 15 of 18), and on (b), where the owner was lowered to 23 dB over the floor, 94 of its 99 sit 12 to 27 dB over it: there F decides.

## 3. The floor rule's F, measured without the masking (plan 16 section 12.7)

The first build chose a single F = 18 dB on bars that could not see the floor rule's own cost: on the FLEURS fixtures whose owner talks only over the far end, the post-canceller correlation layer removes most of the owner's words before the transfer runs ((b): 161 of 212), and a word two layers remove counts once. The re-measurement (27 Oct, fresh GPU runs, EN model, lookahead 13, `v3-offline`, 20 minutes over meeting 12's far end) decides the same words with that layer off (`gate_with`, `correlation_min_attenuation_db: Some(∞)`), the known guard against the owner's level forced unknown at each F. Each cell: "me" WER of the owner's utterances; the owner's words kept (of the canceller's output inside the utterances); words the transfer removed.

| Decision, post-canceller correlation layer off | (b): owner −44.5 dBFS, +23.2 dB over the floor | (b′): owner −38 dBFS, +28.1 dB |
|---|---|---|
| the canceller alone, no gate | 9.48 %; 212 of 212 | 7.11 %; 212 of 212 |
| known guard (the owner's level as estimated: `clean`, 51 and 140 words) | 10.90 %; 207; 5 | 8.06 %; 210; 3 |
| forced unknown, F = 9 | 10.90 %; 207; 5 | 7.11 %; 212; 1 |
| **forced unknown, F = 12** | **10.90 %; 207; 5** | **7.11 %; 212; 1** |
| forced unknown, F = 15 | 14.22 % (+3.32); 199; 13 | 7.58 %; 211; 2 |
| forced unknown, F = 18 | 27.96 % (+17.06); 166; 46 | 8.53 %; 209; 4 |

The bar of 12.7 (ii), the forced-unknown figure within 2 points of the known guard's on both: 9 and 12 pass on both, 15 fails on (b) by 1.3 points over the bar (8 more of the owner's words), 18 by 15. **F_u = 12 dB** (`FLOOR_MARGIN_DB`), as the planner read the histogram of section 2 (the rule reaches 5 of the 99 cut candidates in (b)'s owner utterances at 12, the known guard's own 5, 13 at 15 and 46 at 18). With the layer on, the same decisions read (b) 78.20 known, 77.73 at 9 and 12, 78.20 at 15, 79.15 at 18; (b′) 27.01 known, 26.54 at 9 to 15, 27.01 at 18: the layer's loss is chunk M's (12.7 (iv)). How much the canceller takes from the owner's own words over continuous echo: on (b) 161 of 212 cut by 3 dB or more (median cut 5.6 dB, 90th percentile 11.6), on (b′) 72 (2.1, 6.0), on (a) 22 (1.0, 3.1), on the spliced set 87 of 628 (0.9, 4.0).

The other fixtures at F_u = 12 ("me" WER; echo words left of kept words):

| Fixture | Owner's level known (shipped) | Forced unknown at 12 | Layer off: known / forced unknown at 12 |
|---|---|---|---|
| FLEURS, 55 utterances at random times | 12.69 % (0 of 1,150) | 12.61 % (0 of 1,153) | 10.32 % / 10.32 % |
| (a): ten utterances over the far end, −33 dBFS | 10.43 % (0 of 207) | 10.90 % (0 of 209) | 9.00 % / 9.48 % |
| spliced set; (c): forced unknown | 8.1 % (17 words outside the clips) | (c) 7.9 % (17) | – |

**Natural-unknown runs** (12.7 (ii)): the owner's level unknown by itself, not forced. The two runs the ruling named, fixture (b) with three utterances and (a) with one, did not reach it: with three, 29 of the owner's 75 words were left alone by the canceller, and the one utterance of (a) at −33 dBFS gave exactly 20 clean words, the minimum, so both are known. The same fixture (b) with two and with one utterance does (`too_few`). "Known guard": the owner's true level forced in (the median after the canceller of the owner's words it left alone, as the estimator would read it with enough of them).

| Run | Owner's words (left alone) | Owner's level | Raw echo share | Floor rule | Shipped: WER; owner's kept | Known guard | Layer off: shipped / known guard |
|---|---|---|---|---|---|---|---|
| (b), 3 utterances (73 reference words) | 75 (29) | `clean` −40.9 (+22.7) | 99.91 % | none (known) | 58.90 %; 35 of 75 | the same | 19.18 %; 73 / the same |
| (a), 1 utterance (19) | 20 (20) | `clean` −31.1 (+32.5) | 99.02 % | none (known) | 31.58 %; 20 of 20 | the same | the same |
| **(b), 2 utterances (40)** | 41 (14) | **`too_few`** | 99.92 % | **unknown, 12 dB**, removed 0 | 70.00 %; 12 of 41 | 70.00 %; 12 | 12.50 %; 41 / 12.50 %; 41 |
| **(b), 1 utterance (19)** | 20 (9) | **`too_few`** | 99.92 % | **unknown, 12 dB**, removed 0 | 52.63 %; 11 of 20 | 52.63 %; 11 | 21.05 %; 20 / 21.05 %; 20 |

Both natural-unknown runs keep every owner's word the known guard keeps, with the layer on and off (0 lost; the bar asks for all but those at the floor). The floor rule at 12 takes none of their words; at 15 it would take 5 of 41 and 2 of 20, at 18 9 and 3.

**The raw echo share** (the raw gate's flagged share of the raw mic's words, `transfer.raw_echo_share`) of every recording with a transfer, beside the silent tier's threshold:

| Recording | Raw words | Flagged | Share | Owner's level after the canceller | Floor rule |
|---|---|---|---|---|---|
| 12 | 14,627 | 13,042 | 89.16 % | quiet | none |
| 14 | 1,336 | 1,026 | 76.80 % | quiet | none |
| 16 | 1,196 | 966 | 80.77 % | quiet | none |
| 17 | 1,168 | 1,001 | 85.70 % | quiet | none |
| 18 | 1,618 | 912 | 56.37 % | quiet | none |
| 20 | 10,981 | 9,748 | 88.77 % | quiet | none |
| **21** | 7,611 | 7,606 | **99.93 %** | **implausible** | **silent, 18 dB** |
| FLEURS, 55 utterances | 2,413 | 1,321 | 54.75 % | quiet | none |
| (a) | 2,284 | 2,121 | 92.86 % | clean | none |
| (b) | 2,266 | 2,258 | **99.65 %** | clean | none |
| (b′) | 2,272 | 2,196 | 96.65 % | clean | none |
| (b), 3 / 2 / 1 utterances | 2,336 / 2,353 / 2,361 | 2,334 / 2,351 / 2,359 | **99.91 / 99.92 / 99.92 %** | clean / too_few / too_few | none / unknown / unknown |
| (a), 1 utterance | 2,358 | 2,335 | 99.02 % | clean | none |
| spliced set | 1,173 | 582 | 49.62 % | clean | none |

Meetings 13 and 15 have no canceller and no transfer; 4 and 5 no words. Fixture (b), whose owner talks, reaches 99.5 percent (99.65), so by the ruling **the threshold moves to 99.8 percent** (`SILENT_SHARE`); meeting 21 stays over it. The fixtures with one to three utterances in 20 minutes reach 99.91 to 99.92, over 99.8 as well: the share alone separates a silent owner from one who says a few sentences over a long talk by 0.01 to 0.02 points, which is no separation. What keeps those fixtures out of the silent tier is the other signature: their owner's level is known or `too_few`, never `implausible`, because none of them has 20 words at the floor that the canceller left alone (their far end never pauses long). A recording with both, an owner who says a few sentences over a long talk with pauses that leave 20 or more noise or residue words at the floor, would get F_s = 18 on that owner's cut words; no recording or fixture here is one (on meeting 21 the analysis counts about two words of the owner's own).

**Meeting 21**: owner's level `implausible` (35 words left alone, median 3.3 dB over the floor), raw echo share 99.93 percent, so the silent tier at F_s = 18 dB: 77 kept "me" words (288 before chunk I; target 70) with 42 far-active cut words among them (253; target 40), both missed narrowly and recorded; 1,422 words removed by the floor rule. The floor rule at each F in either tier, for reference:

| F | 21: kept | 21: far-active cut kept | 21: removed by the floor rule | 21: residual duplicates (chance) |
|---|---|---|---|---|
| before chunk I (the unmatched fallback, the residue's level) | 288 | 253 | 0 (level guard 4) | 56 = 19.44 % (2.78 %) |
| 9 | 196 | 161 | 479 | 41 = 20.92 % (3.57 %) |
| 12 (F_u: what the general tier would give) | 155 | 120 | 768 | 36 = 23.23 % (3.23 %) |
| 15 | 104 | 69 | 1,139 | 28 = 26.92 % (1.92 %) |
| **18 (F_s, shipped)** | **77** | **42** | 1,422 | 23 = 29.87 % (1.30 %) |
| ∞ | 49 | 14 | 1,957 | 14 = 28.57 % (2.04 %) |

The canceller meetings with the owner's level forced unknown, the rule at F (owner's words kept, clean owner's kept, far-active cut kept; with the known level, as shipped, F changes nothing on them):

| F | 12: labels strict / fair (of 1,462); owner's (of 1,593); far-active cut | 14: owner's (of 307); far-active cut | 16: owner's (of 248); far-active cut | 17: owner's (of 163) | 18: owner's (of 740); far-active cut | 20: owner's (of 1,221); far-active cut |
|---|---|---|---|---|---|---|
| known (shipped) | 1,413 / 1,437; 1,545; 27 | 301; 1 | 227; 1 | 160 | 729; 27 | 1,198; 15 |
| 9 | 1,413 / 1,437; 1,553; 49 | 301; 5 | 228; 7 | 160 | 736; 35 | 1,200; 45 |
| 12 | 1,413 / 1,437; 1,551; 40 | 301; 3 | 227; 3 | 160 | 736; 35 | 1,198; 29 |
| 15 | 1,413 / 1,437; 1,550; 35 | 301; 1 | 227; 1 | 160 | 735; 33 | 1,198; 18 |
| 18 | 1,413 / 1,437; 1,545; 27 | 301; 1 | 227; 1 | 160 | 729; 27 | 1,198; 13 |
| ∞ | 1,413 / 1,437; 1,536; 16 | 301; 1 | 227; 1 | 160 | 727; 23 | 1,198; 11 |

The clean owner's words are kept at every F on every meeting (12: 1,523 of 1,524; 14: 301 of 301; 16: 227 of 228; 17: 160 of 160; 18: 706 of 706; 20: 1,191 of 1,193), and no fixture's forced-unknown run leaves an echo word.

On the real meetings forced unknown, F = 12 keeps 6 more owner-class words than the known guard on meeting 12 and 7 on meeting 18, with 13, 2, 2, 8 and 14 more far-active cut words on 12, 14, 16, 18 and 20: the general tier is the owner-safe side of the known guard, which sits 15 to 18 dB over the floor on these meetings. No real meeting but 21 has an unknown owner's level.

## 4. The bars (12.2.3 and 12.7)

With F_u = 12 dB, F_s = 18 dB and the silent tier at 99.8 percent shipped. Meeting rows from the replay (section 6, re-run on 27 Oct with the fixed code), meeting 12 also from the real post-pass (`aec_postpass.rs`, canceller, raw mic transcribed once more), FLEURS and the spliced set from the real post-pass and its in-test decision.

| Bar | Result | |
|---|---|---|
| (b) and (b′), layer off: forced unknown within 2 points of the known guard (12.7 (ii)) | at F_u = 12: (b) 10.90 against 10.90, (b′) 7.11 against 8.06 | met |
| Natural-unknown runs keep every owner's word the known guard keeps but those at the floor (12.7 (ii)) | (b) with two and one utterances, `too_few`, tier unknown: 0 lost with the layer on and off; the two runs named in the ruling turned out known (above) | met |
| Silent tier's threshold: no fixture with a talking owner at 99.5 percent (12.7 (ii)) | (b) at 99.65: the threshold moved to 99.8; the one-to-three-utterance fixtures reach 99.91 to 99.92 (their level known or `too_few`, so not in the tier) | moved, recorded |
| Meeting 21 at F_s: 77 kept, 42 far-active cut, `implausible`, tier silent (12.7) | 77 kept, 42 far-active cut; `owner_source` implausible (35 words), raw echo share 99.93, `floor_rule.tier` silent, 1,422 removed | met |
| Meeting 12: labelled owner's words fair 98.29, strict 96.65 percent | real post-pass: 1,437 and 1,413 of 1,462 (98.29 / 96.65 percent), 1,578 kept, 10 duplicates (0.6 percent); `owner_source` quiet, 1,186 words; the replay the same | met |
| Meeting 14: clean owner's 301 of 301 (strict 301 of 307) | 301 of 301 (301 of 307) | met |
| Meeting 16: clean owner's 227 of 228 | 227 of 228 | met |
| Meetings 13 and 15 unchanged (no canceller) | rows identical (13: 292 kept, 285 of 292; 15: 187 of 187) | met |
| Silent-mic controls (4, 5) and the headphone control byte-identical | 4 and 5: 0 mic words on both paths; headphone control: 1,178 "me" words with echo off and auto, identical, 0 dropped, 0 text runs | met |
| Meetings 12 to 20 and every live row identical to section 6 | every post-pass and live row of section 6 identical on meetings 4, 5 and 12 to 21, meeting 21's post-pass row the shipped one (the raw path untouched) | met |
| Spliced set at most 8.1 percent | 8.1 percent (17 words outside the clips) | met |
| FLEURS at most 12.69 percent | 12.69 percent (real post-pass; 0 echo words) | met |
| (a) forced unknown: echo words at most 1 percent of kept; owner's WER within 2 points of the full fixture's | 0 of 209; 10.90 percent at F_u (full fixture 12.69; (a) known 10.43) | met |
| (c) forced unknown: within 1 point of the spliced figure | 7.9 percent at F_u (spliced 8.1) | met |
| Meeting 21: far-active cut words among the kept at most 40; kept at most 70 (reported) | 42 of 77 kept (253 of 288 before chunk I) | both missed narrowly, recorded |
| Meeting 20: far-active cut words no higher than before | 15 (15) | met |
| Meetings 17 and 18 within 2 words of before | identical (17: 160 kept, 0 far-active cut; 18: 757, 27) | met |
| Meeting 21's other layers, measured | after the canceller the utterance sweep 9 → 0 and the level layer 0 → 0 (it is off after the canceller); the 3 level drops are the raw gate's and stay (raw path); the transfer 1 → 221 words beyond the layers | measured |

The (b) and (b′) rows of 12.2.3's first bar (the owner's WER within 2 points of the full fixture's with the layer on) stay missed at every F and with the owner's level known (78.20 and 27.01 against 12.69): the post-canceller correlation layer's loss, chunk M's.

On meeting 21 the owner's class of the matcher (8 non-duplicate words) goes from 5 to 2 kept at every F, 9 to ∞: the 3 words are candidates the canceller cut, 0 to 6 dB over the floor; the one word of the owner's class the canceller left alone is kept, and the analysis counts about two words of the owner's own in the whole recording.

## 5. Meeting 18, a known case (12.2.4)

Measured as variants, not adopted: the transfer's `OVERLAP_MS` at 150 and 200 against 300, and the raw mic's transcript (gated by the raw gate) in place of the cancelled one, as a rule for a canceller ERLE under about 8 to 10 dB (meeting 18 at 7.1 dB and 17 at 9.2 dB qualify; the others are listed for reference). Residual duplicates by the matcher (chance in brackets); the owner's class on each transcript.

| Meeting | Decision | Kept | Far-active cut kept | Residual duplicates | Owner's kept | Clean owner's kept |
|---|---|---|---|---|---|---|
| 18, live raw words | shipped (overlap 300 ms) | 757 | 27 | 22 = 2.91 % (3.04 %) | 729 of 740 | 706 of 706 |
| | overlap 200 ms | 755 | 25 | 19 = 2.52 % (3.05 %) | 728 | 706 |
| | overlap 150 ms | 755 | 25 | 19 = 2.52 % (3.05 %) | 728 | 706 |
| 18, raw mic transcribed (lookahead 13) | shipped | 757 | 27 | 22 = 2.91 % (3.04 %) | 729 of 740 | 706 of 706 |
| | overlap 200 ms | 757 | 27 | 19 = 2.51 % (3.17 %) | 730 | 706 |
| | overlap 150 ms | 756 | 26 | 19 = 2.51 % (3.17 %) | 729 | 706 |
| 20 | overlap 300 / 200 / 150 ms | 1,245 / 1,247 / 1,258 | 15 / 17 / 28 | 1.45 / 1.44 / 1.75 % | 1,198 / 1,198 / 1,201 of 1,221 | 1,191 of 1,193 |
| 16 | overlap 300 / 200 / 150 ms | 230 / 231 / 236 | 1 / 2 / 7 | 0.00 / 0.00 / 0.85 % | 227 / 228 / 228 of 248 | 227 of 228 |
| 14 | overlap 300 / 200 / 150 ms | 308 / 308 / 309 | 1 / 1 / 2 | 0 % | 301 of 307 | 301 of 301 |
| 12 (raw transcribed) | overlap 300 / 200 / 150 ms | 1,578 / 1,581 / 1,585 | 27 / 30 / 34 | 0.63 % | labels 1,413 (fair 1,437) of 1,462 | 1,523 of 1,524 |

| Meeting (canceller ERLE) | Transcript kept | Kept | Far-active kept | Residual duplicates | Owner's kept |
|---|---|---|---|---|---|
| 18 (7.1 dB) | cancelled, shipped | 757 | 542 | 22 = 2.91 % (3.04 %) | 729 of 740 |
| | raw, live words (lookahead 1) | 706 | 501 | 14 = 1.98 % (2.97 %) | 653 of 665 |
| | raw, transcribed (lookahead 13) | 702 | 489 | 13 = 1.85 % (2.56 %) | 640 of 649 |
| 17 (9.2 dB) | cancelled, shipped | 160 | 3 | 0 | 160 of 163 |
| | raw, live words | 167 | 5 | 0 | 152 of 152 |
| 20 (14.6 dB) | cancelled / raw live words | 1,245 / 1,233 | 192 / 172 | 1.45 / 0.81 % | 1,198 of 1,221 / 1,131 of 1,145 |
| 12 (14.4 dB) | cancelled / raw stored | 1,578 / 1,585 | 392 / 359 | 0.63 / 0.82 % | labels 1,413 / 1,436 of 1,462 |

Neither variant moves meeting 18 materially. The overlap at 150 or 200 ms removes 3 of the 22 matcher duplicates and costs at most one owner-class word there, while on meetings 20 and 16 the narrower window lets fragments through (far-active cut kept 15 to 28, 1 to 7). The raw transcript keeps 51 to 55 fewer words and 8 to 9 fewer matcher duplicates on meeting 18, but its owner class is a different word set (649 to 665 words against 740), and the matcher sits at its chance level on meeting 18's kept words either way, so it cannot say how many of the analysis's 95 echo words either variant removes. The cause stays the candidate rule and the canceller's artefacts (12.2.4), not the owner's level; nothing is adopted, and the KNOWN-ISSUES echo row carries the sentence.

## 6. Every meeting's rows, today and shipped

Post-pass rows (shipped, transfer from the live run's mic words; meeting 12 from its raw mic transcribed), today's rows identical on every meeting but 21; the live rows identical on all. Owner's: the matcher's class on that path's words. Re-run on 27 Oct with the fixed code of plan 16 section 12.7 (P = 6, the fall-through, F_u = 12, the silent tier at F_s = 18): every row identical, meeting 21's shipped row now from the silent tier (owner's level `implausible`, raw echo share 99.93 percent).

| Meeting | Path | Mic words | Text | Level | Correlation | Utterance | Transfer | Kept | Residual duplicates (chance) | Owner's kept | Owner source | Far-active cut kept |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4, 5 | both | 0 | 0 | 0 | 0 | 0 | 0 | 0 | – | – | too few | – |
| 12 | post-pass | 2,544 | 576 | 0 | 324 | 8 | 58 | 1,578 | 10 = 0.63 % (3.68 %) | 1,545 of 1,593; labels 96.65 %, fair 98.29 % | quiet | 27 |
| 12 | live (stand-in) | 14,627 | 11,834 | 52 | 1,154 | 2 | – | 1,585 | 13 = 0.82 % (3.34 %) | labels 98.22 %, fair 99.38 % | – | – |
| 13 | post-pass | 386 | 65 | 29 | 0 | 0 | – | 292 | 2 = 0.68 % (1.71 %) | 285 of 292 | quiet | – |
| 13 | live | 375 | 41 | 36 | 0 | 0 | – | 298 | 0 (1.68 %) | 295 of 306 | – | – |
| 14 | post-pass | 369 | 39 | 0 | 5 | 2 | 15 | 308 | 0 (4.22 %) | 301 of 307 | quiet | 1 |
| 14 | live | 1,336 | 961 | 1 | 64 | 0 | – | 310 | 1 = 0.32 % (4.19 %) | 287 of 287 | – | – |
| 15 | post-pass | 187 | 0 | 0 | 0 | 0 | – | 187 | 0 (6.42 %) | 187 of 187 | quiet | – |
| 15 | live | 187 | 0 | 0 | 0 | 0 | – | 187 | 0 (6.42 %) | 187 of 187 | – | – |
| 16 | post-pass | 366 | 67 | 0 | 14 | 3 | 52 | 230 | 0 (2.17 %) | 227 of 248 | quiet | 1 |
| 16 | live | 1,196 | 888 | 1 | 73 | 3 | – | 231 | 0 (2.60 %) | 214 of 219 | – | – |
| 17 | post-pass | 163 | 0 | 0 | 0 | 0 | 3 | 160 | 0 (6.25 %) | 160 of 163 | quiet | 0 |
| 17 | live | 1,168 | 955 | 2 | 40 | 4 | – | 167 | 0 (5.99 %) | 152 of 152 | – | – |
| 18 | post-pass | 769 | 0 | 0 | 0 | 0 | 12 | 757 | 22 = 2.91 % (3.04 %) | 729 of 740 | quiet | 27 |
| 18 | live | 1,618 | 716 | 6 | 188 | 3 | – | 705 | 14 = 1.99 % (3.26 %) | 654 of 664 | – | – |
| 20 | post-pass | 2,626 | 949 | 0 | 298 | 26 | 108 | 1,245 | 18 = 1.45 % (2.17 %) | 1,198 of 1,221 | quiet | 15 |
| 20 | live | 10,981 | 8,960 | 21 | 754 | 6 | – | 1,240 | 10 = 0.81 % (2.26 %) | 1,131 of 1,145 | – | – |
| 21 | post-pass, today | 2,063 | 1,633 | 0 | 132 | 9 | 1 | 288 | 56 = 19.44 % (2.78 %) | 5 of 8 | (unmatched) | 253 |
| **21** | **post-pass, shipped** | 2,063 | 1,633 | 0 | 132 | **0** | **221** | **77** | 23 = 29.87 % (1.30 %) | 2 of 8 | **implausible** (floor rule's silent tier) | **42** |
| 21 | live | 7,611 | 7,227 | 3 | 374 | 0 | – | 7 | 1 (14.29 %) | – | – | – |

The duplicate matcher stays blind to meeting 21's garbled residue, as the reviewer found (B2): its share of the kept words rises as the floor rule removes the fragments it never flagged. The bar that sees the failure, the far-active cut words, falls from 253 to 42.

## 7. Notes

- The raw path is untouched by construction: the live view's estimator (`CalibrationStats::owner_db`), Stop's and the transfer's raw gate keep today's estimator; only words transcribed from the canceller's output use the new one.
- `engine_info.echo` on the real post-pass of meeting 12 (fixed code): `owner_source` quiet, `owner_words` 1,186, `owner_low` false; in `transfer` the raw gate's `raw_owner_db` −33.565, `raw_floor_db` −62.625 and `raw_echo_share` 0.8916, `floor` 0, `floor_rule {tier: null, f_db: null, removed: 0}`. On meeting 21 the post-pass would record `owner_db: null`, `owner_source: implausible`, `owner_words: 35`, `owner_low: false`, `raw_echo_share` 0.9993 and `floor_rule {tier: silent, f_db: 18, removed: 1,422}` (the replay's decision; the stored meeting was not processed again). On fixture (b)'s real post-pass: `owner_source` clean, 51 words, `raw_echo_share` 0.9965, `floor_rule` null.
- The tier on meeting 21: the silent tier needs both signatures, and meeting 21 has both (35 words the canceller left alone at 3.3 dB over the floor; 7,606 of 7,611 raw words flagged). With the general tier alone it would keep 155 words with 120 far-active cut ones (section 3).
- No estimate of the owner's level on any recording or fixture sits between 6 and 15 dB over the floor, so `owner_low` is false everywhere here.
- Every copy of the owner's data (database, meeting folders, cached transcripts, dumps, logs, the rebuilt classes file) lived only in a scratch folder outside the repository and was deleted after the runs. The FLEURS fixtures' audio and words of section 3 (FLEURS speech over meeting 12's far end) are kept outside the repository for chunk M.
