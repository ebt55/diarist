# Eval: echo gate, the owner's level after the canceller and the floor rule (2026-10-26)

Plan 16 section 12.2 (D7, fix 1) as chunk I of section 12.5: the design in 12.2.2 to 12.2.4, the reviewer's findings B1 to B3 in 12.6. Release build, AMD Ryzen 9 5900X; the GPU runs (EN model, lookahead 13, `v3-offline`) went through the shared queue, one batch each. A copy of the database and of the meeting folders, read only; numbers per meeting number only, no titles, names or transcript text.

What changed (`crates/core/src/audio/echo/{layers,transfer}.rs`, `crates/core/src/live/postpass.rs` inside `gate_mic`):

- **The owner's level after the canceller** (`CalibrationStats::owner`): the median level of the words in no text run that the canceller cut by under 3 dB, spoken while the far end was silent (`quiet`), else of all such words (`clean`), from 20 or more; else unknown (`none`); and unknown when the estimate sits under the floor plus `OWNER_PLAUSIBLE_DB` (P = 15 dB). Before the canceller (the raw gate of the transfer, the live view, Stop) the estimator is today's: quiet words, else every word in no run (`unmatched`), no plausibility check.
- **The transfer guard without the owner's level** (`transfer::transfer`, which now takes the cancelled mic's floor): a candidate goes when the canceller cut it by 3 dB or more and its level after the canceller is at most `FLOOR_MARGIN_DB` (F) over that floor. With a known level the guard is today's (12 dB under the owner).
- **Recorded**: `engine_info.echo.owner_source` and `owner_words`; in `engine_info.echo.transfer` the raw gate's `owner_db` and `floor_db` and `floor_rule: {f_db, removed}`.

Method. `echo_sample.rs` replays both paths of every stored meeting on the CPU from `playback.flac`, the stored words and the database copy (live revisions in arrival order); "today" is the same decision with today's estimator forced in (`gate_with`), which reproduces the stored `engine_info.echo` of meetings 17, 18, 20 and 21 exactly (meeting 21: 1,993 candidates, 1,989 guarded, 288 kept). Meeting 12's cancelled mic was transcribed once on the GPU (48 s), meeting 18's raw mic once (6 s, for the 12.2.4 variant). `echo_fleurs.rs` and `aec_postpass.rs` ran the real post-pass on the GPU (FLEURS 20 minutes over meeting 12's far end; meeting 12 off and auto and the spliced set), the F sweep in-test over the same words. The meeting 12 measurement's per-word classes file is gone; it was rebuilt from the stored transcript by the same matcher (`DIARIST_ECHO_CLASSES_OUT`): 14,627 words, 12,260 duplicates, 1,462 owner's words, as in docs/eval/2026-10-20-echo.md. "Far-active cut kept" (12.2.3's bar): kept "me" words spoken while the far end was active that the canceller cut by 3 dB or more. "Owner's" is the matcher's class (non-duplicates whose ±5 neighbours are at most 30 percent duplicates); "clean owner's" those of them the canceller cut by under 3 dB.

## 1. First step: where the owner's level comes from

Before any threshold was set (12.2.3, B3). The sets are the words in no text run: quiet (the far end silent), unmatched (all), clean quiet and clean (of them, cut by under 3 dB; only after a canceller). "Today" is the estimator before this change, "shipped" the new one; the level is the median in dBFS, then the words it came from and its distance over the floor of the mic that was transcribed.

| Recording | Path | Words | Quiet | Unmatched | Clean quiet | Clean | Today | Shipped | Floor |
|---|---|---|---|---|---|---|---|---|---|
| 4, 5 (silent mic) | post-pass, Stop | 0 | 0 | 0 | 0 | 0 | none | none | – |
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
| **21** | **post-pass (cancelled)** | 2,063 | **16** | 422 | **16** | **35** | **unmatched −50.2 (422; +11.2)** | **none (35 clean words, median +3.3)** | −61.4 |
| 21 | raw gate (live words) | 7,611 | 4 | 431 | – | – | unmatched −29.0 (431; +31.1) | the same | −60.1 |
| 21 | Stop | 7,611 | 5 | 384 | – | – | unmatched −29.1 (384; +31.0) | the same | −60.1 |
| FLEURS, 55 utterances | post-pass (cancelled) | 1,182 | 378 | 1,182 | 378 | 1,079 | quiet −31.1 (378; +32.3) | quiet −31.1 (378; +32.3) | −63.4 |
| FLEURS | raw gate (the off run's words) | 2,413 | 386 | 1,182 | – | – | quiet −31.3 (386; +31.9) | the same | −63.1 |
| FLEURS (a): 10 utterances over the far end, −33 dBFS | post-pass (cancelled) | 214 | 5 | 214 | 5 | 190 | unmatched −31.4 (214; +32.2) | clean −31.2 (190; +32.5) | −63.6 |
| FLEURS (a) | raw gate | 2,284 | 8 | 317 | – | – | unmatched −32.7 (317; +30.7) | the same | −63.4 |
| FLEURS (b): the same, −44.5 dBFS | post-pass (cancelled) | 212 | 5 | 212 | 5 | 51 | unmatched −43.1 (212; +20.5) | clean −40.4 (51; +23.2) | −63.6 |
| FLEURS (b) | raw gate | 2,266 | 7 | 143 | – | – | unmatched −39.8 (143; +23.6) | the same | −63.4 |
| Spliced set (meeting 12, 34 to 44 min) | post-pass (cancelled) | 659 | 11 | 652 | 11 | 547 | unmatched −33.9 (652; +29.5) | clean −32.9 (547; +30.4) | −63.4 |
| Spliced set | raw gate (the raw window transcribed) | 1,173 | 18 | 666 | – | – | unmatched −33.3 (666; +29.3) | the same | −62.6 |
| Spliced set (c): the 11 quiet owner's words out | post-pass (cancelled) | 648 | 0 | 641 | 0 | 536 | unmatched −33.9 (641; +29.5) | clean −32.9 (536; +30.5) | −63.4 |

Read: meetings 12 to 20 keep their estimate to the hundredth: after the canceller every quiet word in no run is also one it left alone (clean quiet = quiet), and no raw path moved. Meeting 21 is the case of 12.2.2: 16 quiet words (under 20) sent today's estimator to all 422 words in no run, the residue, 11.2 dB over the floor; 35 of them were left alone by the canceller, and their median sits 3.3 dB over the floor, so the plausibility check makes the level unknown. The FLEURS fixture (a), whose ten owner utterances all lie over far-end speech, has only 5 quiet words, as the reviewer expected of such a fixture: today's estimator falls back to all words in no run (here the owner's own, +32.2), the shipped one to the 190 clean words (+32.5), so the owner is known and the fixture's `owner_source` is `clean`, not `none`. No fixture with a talking owner reaches the unknown regime by itself, because the owner's words over echo are mostly cut by under 3 dB (190 of 212 on (a)). Fixtures (a), (b) and (c) are therefore run **forced unknown**: the post-canceller owner's level set to unknown in the in-test decision (`gate_with`), every other feature as the post-pass computes it. The spliced set confirms the reviewer's B3: its owner's clips sit over the window with the most far-end speech, only 11 of its words in no run were spoken while the far end was silent, and today's estimator passed through the `unmatched` fallback (−33.9 dBFS, which happened to be the owner's own level because the owner's words dominate the window); the shipped one takes the 547 clean words (−32.9) and keeps the case right, also with the 11 quiet owner's words removed (536 clean words, −32.9): so variant (c), too, needs the owner's level forced unknown.

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
| Spliced: owner's words (in the clips), candidates cut | 6 | 0 | 1 | 0 | 1 | 0 | 0 | 1 | 2 | 0 | 1 | 0 | 0 |
| FLEURS: owner's words cut under 3 dB (median +32.2) | 1,079 | 0 | 9 | 1 | 2 | 0 | 2 | 6 | 4 | 13 | 53 | 164 | 825 |
| FLEURS (a): the same (median +32.5) | 190 | 0 | 1 | 0 | 0 | 1 | 1 | 0 | 2 | 2 | 10 | 32 | 141 |
| FLEURS (b): the same (median +23.2) | 51 | 0 | 0 | 1 | 1 | 1 | 0 | 2 | 8 | 15 | 19 | 4 | 0 |
| Spliced: owner's words cut under 3 dB | 541 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 32 | 83 | 120 | 297 |

Read: the residue the floor rule is for (meeting 21's kept candidates) sits 0 to 21 dB over the floor, 184 of its 239 under 15 and 211 under 18. The owner's words the canceller left alone sit 18 dB or more over the floor: 94 to 100 percent of them on every meeting and fixture, 90 percent on (b). The owner's words among the cut candidates on the real meetings sit mostly low (0 to 12 dB: floor-level fragments under far-end speech, which the round-two notes read one by one as garbled echo), except on meeting 18 (8 of 13 at 15 to 21 dB) and meeting 12 (17 of 55 at 15 dB or more). On the fixtures the owner's real words among the cut candidates sit 18 to 30 dB over the floor ((a): all 16; FLEURS: 15 of 18), and on (b), where the owner was lowered to 23 dB over the floor, 94 of its 99 sit 12 to 27 dB over it: there F decides.

## 3. The F sweep

The floor rule at each F of {9, 12, 15, 18, ∞} (∞: the canceller's cut alone). On every recording whose owner's level is known (all but meeting 21) F changes nothing; the rows that decide are meeting 21, unknown by itself, and the forced-unknown ones: the fixtures (a), (b) and (c), and for reference the full FLEURS fixture and every canceller meeting with the owner's level forced unknown. Meetings on their shipped raw source (live words; meeting 12 its raw mic transcribed). "Today" is today's estimator and guard.

Meeting 21 and the fixtures ("me" WER of the owner's utterances; the spliced set's owner's WER; echo words left over kept words in brackets):

| F | 21: kept | 21: far-active cut kept | 21: removed by the floor rule | 21: residual duplicates (chance) | FLEURS, forced unknown | (a), forced unknown | (b) −44.5 dBFS, forced unknown | (c) spliced, forced unknown |
|---|---|---|---|---|---|---|---|---|
| today (owner's level known on all but 21) | 288 | 253 | 0 (level guard 4) | 56 = 19.44 % (2.78 %) | 12.69 % (0 of 1,150) | 10.43 % (0 of 207) | 78.20 % (0 of 49) | 8.1 % (17 outside the clips) |
| 9 | 196 | 161 | 479 | 41 = 20.92 % (3.57 %) | 12.61 % (0 of 1,153) | 10.90 % (0 of 209) | 77.73 % (0 of 50) | 7.9 % (17) |
| 12 | 155 | 120 | 768 | 36 = 23.23 % (3.23 %) | 12.61 % | 10.90 % | 77.73 % | 7.9 % |
| 15 | 104 | 69 | 1,139 | 28 = 26.92 % (1.92 %) | 12.61 % | 10.90 % | 78.20 % (0 of 49) | 7.9 % |
| **18** | **77** | **42** | 1,422 | 23 = 29.87 % (1.30 %) | 12.61 % (0 of 1,152) | **10.90 %** | 79.15 % (0 of 46) | **8.1 %** |
| ∞ | 49 | 14 | 1,957 | 14 = 28.57 % (2.04 %) | 12.94 % (0 of 1,147) | **15.64 %** (0 of 195) | 79.62 % (0 of 45) | 8.6 % (17) |

The canceller meetings with the owner's level forced unknown (owner's words kept, clean owner's kept, far-active cut kept; with the known level the shipped rows are today's at every F):

| F | 12: labels strict / fair (of 1,462); owner's (of 1,593); far-active cut | 14: owner's (of 307); far-active cut | 16: owner's (of 248); far-active cut | 17: owner's (of 163) | 18: owner's (of 740); far-active cut | 20: owner's (of 1,221); far-active cut |
|---|---|---|---|---|---|---|
| known (shipped) | 1,413 / 1,437; 1,545; 27 | 301; 1 | 227; 1 | 160 | 729; 27 | 1,198; 15 |
| 9 | 1,413 / 1,437; 1,553; 49 | 301; 5 | 228; 7 | 160 | 736; 35 | 1,200; 45 |
| 12 | 1,413 / 1,437; 1,551; 40 | 301; 3 | 227; 3 | 160 | 736; 35 | 1,198; 29 |
| 15 | 1,413 / 1,437; 1,550; 35 | 301; 1 | 227; 1 | 160 | 735; 33 | 1,198; 18 |
| 18 | 1,413 / 1,437; 1,545; 27 | 301; 1 | 227; 1 | 160 | 729; 27 | 1,198; 13 |
| ∞ | 1,413 / 1,437; 1,536; 16 | 301; 1 | 227; 1 | 160 | 727; 23 | 1,198; 11 |

The clean owner's words are kept at every F on every meeting (12: 1,523 of 1,524; 14: 301 of 301; 16: 227 of 228; 17: 160 of 160; 18: 706 of 706; 20: 1,191 of 1,193), and no fixture's forced-unknown run leaves an echo word.

**F = 18 dB**, by the rule of 12.2.2 step 3 (the largest F at which every owner-word bar of 12.2.3 holds):

- ∞ fails: on (a) the cut alone takes the owner's own words spoken over the far end (17 removed against 1 at F = 18) and the owner's WER goes from 10.43 (known) and 10.90 (F = 18) to 15.64 percent, 5.2 points over (a)'s own figure and 3.0 over the full fixture's 12.69, against the 2-point bar; the full fixture moves 12.69 to 12.94 and the spliced set 8.1 to 8.6, both inside their bars, and meeting 12's matcher class loses 9 more words.
- 18 holds: (a) 10.90 percent (0.5 over its known figure, under the full fixture's); (c) 8.1 percent, the spliced figure itself; the full fixture 12.61; (b) 79.15 against its known 78.20 (see the bars: (b) misses the bar against the full fixture at every F, the known decision included); and on every real meeting the forced-unknown rows at 18 equal the known ones but for 2 more kept words on meeting 12 (one more matcher duplicate) and 2 fewer far-active cut words on meeting 20: with the owner at their usual level the rule at 18 does what today's guard does.
- What it reaches on meeting 21: 77 kept "me" words (288 today; target 70) with 42 far-active cut words among them (253 today; target 40). Both targets are missed narrowly; as 12.2.3 says the owner's words outrank them, and the gap is recorded. 15 dB would leave 104 and 69, ∞ would meet both (49 and 14) at (a)'s cost above.
- Open: a second reading of (b), the owner at −38 dBFS RMS (5 dB under the standard fixture in the plan's own units, "−33 dBFS over a −59 floor"), was queued and withdrawn for an exclusive timing window; if it shows F = 18 more than 2 points over its own known-owner figure, F steps down to 15 (meeting 21 then keeps 104 words, 69 far-active cut).

## 4. The bars (12.2.3)

With F = 18 dB shipped. Meeting rows from the replay (section 6), meeting 12 also from the real post-pass (`aec_postpass.rs`, canceller, raw mic transcribed once more, shipped code), FLEURS and the spliced set from the real post-pass and its in-test decision.

| Bar (12.2.3) | Result | |
|---|---|---|
| Meeting 12: labelled owner's words fair 98.29, strict 96.65 percent | real post-pass: 1,437 and 1,413 of 1,462 (98.29 / 96.65 percent), 1,578 kept, 10 duplicates (0.6 percent); `owner_source` quiet, 1,186 words; the replay the same | met |
| Meeting 14: clean owner's 301 of 301 (strict 301 of 307) | 301 of 301 (301 of 307) | met |
| Meeting 16: clean owner's 227 of 228 | 227 of 228 | met |
| Meetings 13 and 15 unchanged (no canceller) | rows identical (13: 292 kept, 285 of 292; 15: 187 of 187) | met |
| Silent-mic controls (4, 5) and the headphone control byte-identical | 4 and 5: 0 mic words on both paths, as before; headphone control: 1,178 "me" words with echo off and auto, identical, 0 dropped, 0 text runs | met |
| Live-view and Stop rows of every meeting unchanged | every live row, Stop's drops by reason and the view's end state identical on meetings 4, 5 and 12 to 21 (22 lines compared, the raw path untouched) | met |
| Spliced set at most 8.1 percent | 8.1 percent (17 words outside the clips) | met |
| FLEURS at most 12.69 percent | 12.69 percent (real post-pass; 0 echo words) | met |
| (a) forced unknown: echo words at most 1 percent of kept; owner's WER within 2 points of the full fixture's | 0 of 209; 10.90 percent (full fixture 12.69; (a) with its own level known 10.43) | met |
| (b) forced unknown: the same | 0 of 46 kept; 79.15 percent against 78.20 with its own level known (+0.95), but 66 points over the full fixture: missed, at every F and with the owner's level known (78.20), because at this level the owner is under the echo and the post-canceller correlation layer takes 161 of its 212 words, not the floor rule (the canceller alone leaves 9.48 percent). | missed (not by F) |
| (c) forced unknown: within 1 point of the spliced figure | 8.1 percent (spliced 8.1) | met |
| Meeting 21: far-active cut words among the kept at most 40; kept at most 70 (reported) | 42 of 77 kept (253 of 288 today) | both missed narrowly, recorded |
| Meeting 20: far-active cut words no higher than today | 15 (15) | met |
| Meetings 17 and 18 within 2 words of today | identical (17: 160 kept, 0 far-active cut; 18: 757, 27) | met |
| Meeting 21's other layers, measured | after the canceller the utterance sweep 9 → 0 and the level layer 0 → 0 (it is off after the canceller); the 3 level drops are the raw gate's and stay (raw path); the transfer 1 → 221 words beyond the layers | measured |

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

Post-pass rows (shipped, transfer from the live run's mic words; meeting 12 from its raw mic transcribed), today's rows identical on every meeting but 21; the live rows identical on all. Owner's: the matcher's class on that path's words.

| Meeting | Path | Mic words | Text | Level | Correlation | Utterance | Transfer | Kept | Residual duplicates (chance) | Owner's kept | Owner source | Far-active cut kept |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4, 5 | both | 0 | 0 | 0 | 0 | 0 | 0 | 0 | – | – | none | – |
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
| **21** | **post-pass, shipped** | 2,063 | 1,633 | 0 | 132 | **0** | **221** | **77** | 23 = 29.87 % (1.30 %) | 2 of 8 | **none** | **42** |
| 21 | live | 7,611 | 7,227 | 3 | 374 | 0 | – | 7 | 1 (14.29 %) | – | – | – |

The duplicate matcher stays blind to meeting 21's garbled residue, as the reviewer found (B2): its share of the kept words rises as the floor rule removes the fragments it never flagged. The bar that sees the failure, the far-active cut words, falls from 253 to 42.

## 7. Notes

- The raw path is untouched by construction: the live view's estimator (`CalibrationStats::owner_db`), Stop's and the transfer's raw gate keep today's estimator; only words transcribed from the canceller's output use the new one.
- `engine_info.echo` on the real post-pass of meeting 12: `owner_source` quiet, `owner_words` 1,186; in `transfer` the raw gate's `owner_db` −33.565 and `floor_db` −62.625, `floor_rule {f_db, removed: 0}`. On meeting 21 the post-pass would record `owner_db: null`, `owner_source: none`, `owner_words: 35` and `floor_rule.removed` 1,422.
- The copies (database, meeting folders, cached transcripts, dumps, the rebuilt classes file) live only in a scratch folder outside the repository and are deleted once the open item of section 3 is run.
