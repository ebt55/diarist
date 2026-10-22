# Echo without headphones: status and open questions for an external reviewer

Written 21 Oct 2026 by the planner (Fable 5.1) for someone outside the project who is asked for a second opinion. It is self-contained: what the product does, what the problem is, what was measured, what was built, what failed in the owner's tests on 21 Oct, and the questions we want answered. Numbers are from the owner's own PC (Windows 11, RTX 3080, HyperX QuadCast USB condenser mic, Realtek outputs). Nothing here is private: no transcript text, no names, only counts and signal statistics.

## 1. The product in two sentences

Diarist is a local Windows app that records a meeting from two audio channels at once, the microphone (the owner, speaker "me", transcribed without diarization) and the system-audio loopback (everyone else, diarized), streams both to a GGUF speech runtime (NeMo-Speech.cpp: Nemotron streaming ASR plus a Sortformer-family diarizer), shows a provisional live transcript, and after Stop runs a full "post-pass" over the stored WAVs that becomes the stored transcript. Nothing leaves the machine; there is no PyTorch, only C++ runtimes and Rust.

## 2. The problem

When the far end plays on speakers, the mic picks it up acoustically. Two things go wrong:

1. The far end is transcribed twice: once from the loopback with the right speaker, once from the mic as "me", a few tens of milliseconds later.
2. The owner's own words spoken while the far end talks are recognised from a mixture, so they come out wrong or missing.

The owner's requirement: "me" should contain only the owner's words, including those spoken over the far end; a headphone recording must be processed exactly as before; accuracy and speed come before features.

## 3. What was measured on the reference recording (19 Oct 2026, 1 h 51 min, speakers)

| Quantity | Value |
|---|---|
| Mic words / loopback words (post-pass) | 14,627 / 13,551 |
| Mic words that are text copies of a loopback word within ±0.8 s | 83.7 percent (chance level of that matcher 4.0 percent) |
| Mic 3-word runs repeating a loopback run | 72.6 percent (chance 0.01 percent) |
| Owner's own words | about 1,462 (10 percent of the mic transcript); 172 of them spoken over the far end |
| Echo lag, loopback → mic, on a common clock | median 3.3 ms; under that a hardware sawtooth: 8.8 ppm drift and a 5 ms step every 9.45 min |
| Mic ASR stamps echo words late | +75 ms median, +275 ms p90 |
| Echo return loss (loopback level minus echo level at the mic) | about 10 dB |
| Owner's voice at the mic against the echo at the mic | only about 5 dB louder |
| Broadband normalised cross-correlation, 2 s blocks, loopback active | median 0.72 (null with the mic shifted 7 s: 0.075, p99 0.21) |
| Same on 100 ms frames | echo 0.76; null median 0.195, 13.5 percent of frames at or above 0.35 |
| Word-level correlation (span padded) | echo words 0.74 median; owner's words during far-end speech 0.06 median, 7 percent above 0.3 |

Key arithmetic: the correlation coefficient is about the square root of the echo's share of the mic's power. An echo-only word sits near 0.76; the owner at +5 dB over the echo sits near 0.47; a quiet leak near the noise floor sits near zero even when it is intelligible in the speech band.

## 4. What was built (branch `claude/diarist-echo-copyright-b63b18`, PR 9, plan `docs/plans/15-echo-without-headphones.md`)

1. **Coupling monitor.** Both channels are tapped before the engine feed into a 60 s ring on a common clock. On 2 s blocks where the loopback is active, FFT cross-correlation at sample resolution over causal lags (narrow −5..+60 ms for 5 blocks, then −5..+500 ms until a lock, then ±25 ms around the tracked lag); coupled when the median over 10 active blocks is at least 0.35 and 7 of 10 peaks lie within ±3 ms of the tracked lag; released after 30 active blocks under 0.2. Cost 0.03 percent of a core.
2. **Word gate.** Each mic word scored once: correlation between mic and loopback over [start − 350 ms, end + 160 ms] at the tracked lag ±20 ms; hysteresis band keep < 0.20 / neighbours' median ≥ 0.50 / drop ≥ 0.60 (0.40 / 0.60 / 0.80 after the canceller). Live (what the view shows and the live run at Stop) and in the post-pass. Removed words are kept in a column with a Restore action. On the 19 Oct recording: 98.97 percent of duplicates removed, 98.84 percent of the owner's words kept; on a double-talk fixture with ground truth (public FLEURS utterances stood in for the owner over the real far end with the measured echo path) the band change lowered "me" WER by 0.7 points where a lower band had raised it by 15.9. The band was chosen on the rule that the owner's words outrank residual duplicates.
3. **Windows' own echo cancellation, probed and rejected.** On this PC `IAcousticEchoCancellationControl` is offered only under the Speech stream categories, cancelled nothing even on an exact digital loop through a virtual cable, and added about 20 dB of automatic gain and 110 to 130 ms of delay (`docs/research/14-echo-cancellation.md` section 1).
4. **Diarist's own canceller, post-pass only.** A partitioned-block frequency-domain adaptive filter (Speex MDF lineage, pure Rust) with a mild residual suppression stage, mic as input, loopback as reference with its slow AGC gain replayed backwards from a gain track now written beside the WAVs, aligned through the recording's clock anchors onto the mic's clock. On the 19 Oct recording: 14.7 dB of echo removed on far-end-only spans (the mic's noise floor caps the measurable figure near 20 dB), the owner's own stretches changed −0.12 dB, no divergence in 111 minutes, 0.15 percent of one core; the cancelled mic re-transcribed and then gated gives 3.2 percent residual duplicates in "me", 98.3 percent of the owner's words kept, and the owner's double-talk WER falls from 17.1 to 7.6 percent on a spliced set with ground truth. A causal (live-capable) configuration measured 14.0 dB and 8.3 percent. The live view is still gate-only.
5. Settings (`echo.mode` auto / off / always, a threshold), a live banner with "Turn off for this recording", a meeting-details line with Restore, a diagnostics row.

Verification before the PR: 415 tests, clippy and formatting clean, a GPU replay of the first 20 minutes of the 19 Oct recording, the file-import evaluation byte-identical (imports do not touch this code).

## 5. What failed in the owner's two test recordings (21 Oct 2026, the build above)

Both about 8.4 minutes, same PC, same mic, "Keep live runs" on. The owner's verdict: "still bad in both, not astronomically bad like before, but still unusable."

**Meeting 13, headphones in the Realtek analog jack, loopback on "Speakers (Realtek(R) Audio)".** The monitor reported Clear throughout: coupling 0.13, the lag never locked, 0 of 386 mic words gated live or in the post-pass. Yet from about minute 3 the far end's voice appears in "me": the headphones leak into the condenser mic at a level too low to correlate broadband but high enough for the speech model to transcribe. This is the "quiet leak" case the correlation design cannot see by construction (section 3's arithmetic). Detailed numbers: section 6.

**Meeting 14, speakers, loopback on "Realtek Digital Output".** Detection and cancellation worked by their own measures: coupled at 39 s (coupling 0.45, lag 8.9 ms), 860 of 1,336 live mic words gated; post-pass coupling 0.52, lag 6.8 ms, canceller ERLE 16.1 dB over 169 far-end blocks, 29 of 369 mic words dropped after the canceller. The owner nevertheless saw far-end duplicates, now labelled as the far-end speaker rather than "me". Mic words can only carry a loopback speaker's label if the new canceller path diarized the cancelled mic or collided labels between channels, so a bug in the day-old wiring is the first suspect; and 340 kept mic words in 8 minutes is more than the owner is likely to have said. Detailed numbers: section 6.

## 6. Findings from the analysis of meetings 13 and 14

Read-only analysis of the stored audio, word files, runs and logs (counts only). Short version: no bug; the code did what it was built to do; the speech model transcribes echo far below the level where the signal statistics can see it.

**No labelling bug.** In both meetings every stored mic word carries the label `me` on the mic's own speaker row; `diarize_mic` was false; the canceller path transcribes `mic-aec.wav` without diarization; speakers are created per (channel, label) with a UNIQUE constraint, so a cross-channel label collision is impossible. The "duplicates of speaker X" the owner saw are the loopback's correct lines plus a second copy in "me".

**Meeting 13, headphones (505 s).** Post-pass: 386 mic words, 928 loopback words (two far-end speakers). 69 mic words (17.9 percent; chance 1.9) are duplicates of a loopback word, 30 of them in matched 3-word runs; the mic copy is 0.40 s late at the median. They cluster where the owner was silent and the far end talked: in minute 4, 45 of 56 mic words are duplicates and none are the owner's. The leak is tiny: return loss 54 dB by the plan's formula (38 dB as a plain level difference); the mic's speech-band level (300 to 3,400 Hz) is −61.3 dBFS with only the far end playing against a noise floor of −61.6 dBFS and −44.8 dBFS when the owner speaks, so the leak itself sits about 11 dB under the mic's own noise floor. Block coefficients: broadband p50 0.13, p99 0.26, no block reaches 0.30; band-limited p50 0.18, p99 0.33; the null (mic shifted 7 s) p99 0.09. The few blocks at or above 0.2 share a consistent 1.5 to 3.7 ms lag, so a lower lock confidence would have locked (0.20 locks at 34 s), but the 0.35 coupling median is never reached at any setting, the return-loss rule would have kept the canceller off, and the duplicate words' own coefficients (median 0.18, p90 0.24, none at 0.40) fall under every gate band tried: all ten bands drop zero words. Nothing changed around minute 3: coefficient, levels, AGC (0 dB from minute 2), endpoint and device state are flat; the far end simply talked while the owner was silent.

**Meeting 14, speakers (499 s).** Coupling found at 36 s; block coefficients 0.50 to 0.57 broadband (0.72 to 0.80 band-limited), lag track 5.3 to 9.0 ms, return loss 14 dB, loopback 8 to 10 dB quieter than in meeting 13. The canceller matched the log exactly when re-run in memory: 16.1 dB over 169 far-end blocks (per minute 11.5 to 21.9 dB), one jump, no resets. Of the 369 cancelled-mic words before the gate: 41 duplicates, 307 owner's words, 21 other. The gate (band 0.40 / 0.60 / 0.80) dropped 29: 19 duplicates, 1 owner's word, 9 other. Kept: 340 words, of which 22 duplicates (6.5 percent; chance 2.3) and 306 of the owner's 307. The 22 surviving duplicates have coefficients 0.46 to 0.75 (median 0.55), 18 of them in the 0.40 to 0.60 band, and the canceller had removed 17.5 dB over them (median), 0.2 dB over the owner's words. Other bands on the same words: 0.40 / 0.50 / 0.70 drops 40 of 41 duplicates and keeps 305 of 307 owner's words; the pre-canceller band 0.20 / 0.50 / 0.60 drops 40 and keeps 303. The live run (gate only, no canceller) kept 476 of 1,336 mic words, 158 of them duplicates (33 percent) scoring 0.08 to 0.60 (median 0.48, lower than the 0.74 of the 19 Oct recording because the echo sat nearer the noise floor); 20 of them arrived before coupling. Live and post-pass scoring agree on 1,321 of 1,336 words.

**Text-level check** (3-word runs, loopback first, on the stored transcripts): a −0.2 to +1.0 s window removes 51 of 68 duplicates in meeting 13 (post-pass), 14 of 22 in meeting 14 (post-pass), 26 of 50 and 100 of 158 in the two live runs, and removes zero of the owner's words in every window up to ±1.5 s; only at +3.0 s does it start taking owner's words (3 in meeting 14). Chance level with the mic moved 45 s: 3 words.

**Also seen.** The UI's live-latency statistic counted the words re-sent when coupling arrived (maximum 34 s in meeting 14 against 1.2 s in meeting 13); a cosmetic defect in the metric. In both tests the owner spoke almost only while the far end was silent (about 1,900 owner frames with the loopback silent against about 60 with it active), so double-talk recognition was barely exercised; the complaint is about duplicates.

**What this means.** The correlation gate's separability depends on the echo's level relative to the mic's noise floor (0.74 for echo words on the loud 19 Oct recording, 0.48 on the quieter 21 Oct one, 0.18 for the headphone leak), while the speech model's ability to transcribe echo does not: it produced words from a leak 11 dB under the broadband floor and from residual echo after 17 dB of cancellation. A gate built only on signal statistics therefore cannot reach "usable" on its own. The text of the far end is known, and text-run matching removed most residual duplicates on all three recordings with no loss of the owner's words; the owner's words also sit 16 dB above the leak in level in the headphone case. The round-two design (plan 15 section 9) layers these.

## 7. Approaches under research

(Pending: `docs/research/16-echo-approaches.md` is being written; the ranked recommendation lands here.)

The candidates, stated so the reviewer can react before the note exists:

- **Text-level dedup as a gate.** Match mic word runs against loopback word runs in a causal window (loopback first, 0 to about +1 s), remove the mic copy. Needs no acoustic coupling, so it covers the headphone leak. Chance rates measured: 4.0 percent per word, 0.01 percent per 3-word run. Risk: the owner repeating the far end's phrase.
- **Band-limited or coherence-based detection** (300 to 3,400 Hz, per-band magnitude-squared coherence) so a quiet leak that the ASR hears still registers.
- **Residual echo suppression** after the linear canceller (coherence masks, as in WebRTC AEC3 / Speex preprocess), aiming to stop the ASR from producing echo words rather than only attenuating them.
- **Near-end activity gating at the signal level** (half-duplex style): mute or attenuate the mic frames sent to the ASR when the loopback is active and the mic's speech-band energy is explained by the echo estimate.
- **Owner-voice enrollment** (speaker embeddings on the CPU, already planned for identity): keep only the owner's voice from the mic.

## 8. Questions for the reviewer

1. Given the measured numbers (10 dB return loss, owner only 5 dB above the echo, 14 to 16 dB achievable cancellation, a hardware lag sawtooth), is there a signal-level design that gets "me" to usable with speakers, or is a text-level gate the only robust remaining lever for the residual?
2. For the headphone leak (coupling 0.13 broadband, intelligible to the ASR): would speech-band coherence detect it reliably without false positives on a true headphone recording, or is text matching the right tool there too?
3. Text dedup safety: what window, run length and tie-breakers would you require so that the owner repeating a far-end phrase is kept? Is "most of the mic utterance matches" a sound rule?
4. Would you run the canceller live (causal 14.0 dB measured offline) given the sawtooth, or keep live gate-only and spend the effort on the post-pass?
5. Owner-voice enrollment: worth it as the primary mic filter (a few minutes of enrolled speech, CPU embeddings per mic utterance), or too fragile against double-talk and short utterances?
6. Anything in the measurement method you distrust (the duplicate matcher as a label source, the spliced double-talk set, the FLEURS stand-in fixture)?

## 9. Where things are

Branch and PR as above, not merged; the owner's tracker holds the merge as blocked until this is resolved. All code was written by Opus 5.5 agents against briefs; plans and measurements are in `docs/plans/15-echo-without-headphones.md`, `docs/eval/2026-10-20-echo.md`, `docs/research/14-echo-cancellation.md`, and (soon) `docs/research/16-echo-approaches.md`.
