# Research 16: other ways to keep far-end words out of "me"

Status: 21 Oct 2026, Opus 5.5 research agent. No code changed. Sources: plan 15, `docs/eval/2026-10-20-echo.md`, research 14, the 21 Oct failure analysis (session scratchpad `failures/`), a text-gate sweep over the stored runs (scratchpad `r16/textgate.json`, counts only) and the bibliography.

## 1. What the 21 Oct recordings add

- **Headphones.** In 300 to 3,400 Hz, 20 ms frames:

  | Signal | Level |
  |---|---|
  | Far-end-only mic frames | −61.3 dBFS |
  | Mic floor | −61.6 dBFS |
  | Leak, floor removed | about −73 dBFS (11 dB under the floor) |
  | Owner (median) | −45 dBFS |

  The loopback is 38 dB above the mic there. Yet 50 live and 69 post-pass mic words (of about 380) are text duplicates, against 2 percent chance.
- **Headphone correlation.**
  - NCC median 0.132 broadband, 0.176 band-limited; null p99 0.087 and 0.081.
  - At a confidence threshold of 0.1, a lag-consistent lock (2 ms, 10 of 10 blocks) forms within 6 s.
  - Word gate: leak words score 0.18 (median), the owner's 0.08 (p90 0.15). No band drops any; the return-loss rule keeps the canceller off.
- **Speakers.** The canceller removed 16.1 dB.
  - Post-pass: the 22 surviving duplicates had lost 17.5 dB (median) and were still recognised.
  - Live, with no canceller: 158 duplicates survive among 476 words (median coefficient 0.48), kept by the owner-first band.
- **Duplicate stamps, mic minus loopback:** live speakers +0.07 s median (p10 −0.01, p90 +0.22); post-pass +0.31 to +0.40 s (p90 up to +0.63); live headphones +0.27 s.

**Lesson:** the ASR transcribes far-end speech at or under the mic's floor, and after 17 dB of cancellation. Attenuation does not stop echo words. Removing the frames, or recognising and removing the words, does.

## 2. Text-level dedup

A mic word matches when a loopback word with the same normalised text starts in the window (loopback first); a run is consecutive matching mic words. Cells, in percent:
- **recall:** the share of the matcher's duplicates found (the matcher: ±0.8 s single word or 3-gram);
- **chance:** the share of mic words flagged with the mic moved ±45 or ±90 s.

| Rule | 19 Oct raw mic (14,627 words) | 21 Oct headphones, post-pass (386) | 21 Oct speakers, live (476) | 21 Oct speakers, post-pass (340) |
|---|---|---|---|---|
| Single, ±0.8 s | 99.9 / 3.97 | 94.2 / 1.94 | 100 / 2.63 | 100 / 2.28 |
| ≥2, ±0.8 s | 99.5 / 0.77 | 82.6 / 0 | 98.1 / 0.37 | 90.9 / 0.22 |
| ≥3, ±0.8 s | 98.9 / 0.11 | 71.0 / 0 | 95.6 / 0.16 | 90.9 / 0.22 |
| ≥3, −0.1 to +1.0 s | 91.8 / 0.07 | 81.2 / 0 | 94.9 / 0 | 90.9 / 0 |
| ≥3, −0.1 to +0.6 s | 90.8 / 0.04 | 42.0 / 0 | 94.3 / 0 | 86.4 / 0 |
| ≥3 in loopback order, −0.1 to +1.0 s | 91.5 / 0.04 | 76.8 / 0 | 76.6 / 0 | 68.2 / 0 |

**What the chance rates support:**
- **Single words: never on text alone.** Chance is 2 to 4 percent, and the owner's backchannels ("yeah", "okay") are exactly these.
- **Three or more consecutive: safe on text.** Chance is at most 0.16 percent of words (about two of the 1,462 owner's words on 19 Oct); recall is 71 to 99 percent.
- **Two: only with an acoustic tie-breaker.** Chance reaches 0.8 percent.
- **The window.** 5 to 7 percent of 19 Oct's duplicates precede their loopback word by over 0.1 s (the loopback ASR is late too), and a +0.6 s bound loses half the leak duplicates. Candidate: −0.5 to +1.0 s, to be swept.
- **Loopback order: a tie-breaker, not a requirement.** It halves chance but costs 18 points of recall live (two far-end speakers interleave, and revisions reorder).

**Against the owner repeating the far end:**
- **Timing.** An echo copy has a small, constant offset; a repetition starts after the phrase, about a second for three words (close shadowing at 250 ms is deliberate; Marslen-Wilson 1973). Offsets inside a run must agree within about 0.3 s.
- **Share of the utterance.** Echo stretches run mostly 8 to 23 words (21 Oct live), while a quote is a minority of an owner utterance. So drop only runs that cover most of their endpoint-delimited utterance, or that the tie-breaker confirms.
- **Two tie-breakers, each blind where the other sees:**
  - Speakers: the word coefficient (echo 0.74, the owner in double talk 0.06).
  - Headphones: level over the floor (−45 against −61 dBFS).

**Edge cases:** numbers (same model on both channels; normalise digits anyway); function words (56 percent of 19 Oct's mic words are on a 76-word stop list, so require a content word); the owner reading what the far end just said (outside the window unless simultaneous, then the tie-breakers decide); two far-end speakers (pool their words; 21 Oct's echo stretches mix both).

**Live:** both channels' words arrive within about a second and a run decides at its third word, so a mic word shows and retracts within a second, as the live gate does. Cost: a hash lookup per word.

**Headphones:** no coupling is needed. Runs of three or more catch 74 to 81 percent of the leak duplicates, runs of two or more 90 to 98 percent. Section 5 takes the isolated rest.

## 3. Band-limited, SNR-aware detection

**The literature's thresholds answer another question:** whether the near end talks over strong echo.
- WebRTC's legacy AEC averages magnitude-squared coherence (MSC) over about 250 to 1,700 Hz, smoothed about 57 ms. It calls near end at far-to-mic coherence under 0.1 with mic-to-error over 0.98, and echo again above 0.2 or under 0.95.
- Tashev's optimised baseline coherence detector uses 0.96, and he notes that noise and reverberation make any fixed threshold SNR-dependent.

**A weak echo is a question of the null.** Welch MSC over N independent segments is significant above 1 − α^(1/(N−1)) (Carter 1987):
- 0.073 for a 2 s block of 62 segments of 32 ms (α = 0.01);
- 0.015 for 10 s.

MSC is r/(1 + r) for an echo-to-rest ratio r: 0.09 at −10 dB. It ignores the echo path's colour; the NCC peak does not (meeting 12's room lowers echo words from 0.98 to 0.74).

**On 21 Oct the present statistic already separates** (band NCC 0.176, null p99 0.081). Lag consistency (7 of 10 within ±3 ms; null 6 percent) does the safety work, not the 0.35 coefficient: a "weak coupling" state at 0.1 to 0.15 would have engaged at 26 s. MSC over 10 to 30 s suits deeper leaks.

**Detection alone removes nothing here**; it gives the warning, the return loss by level and section 5's trigger.

## 4. Residual echo suppression

| System | Echo-only frames |
|---|---|
| Diarist | Wiener gain, floored at −12 dB |
| Speex preprocess | −40 dB, or −15 dB with near-end speech (by speech probability) |
| WebRTC AEC3 | Transparent while the residual is under 0.3 × the band's capture power. Gain 0 at 0.4 ×, floored so the echo ends about 5 dB under the comfort-noise masker. Near-end state: capture over 4 × residual and 30 × noise in 125 to 1,875 Hz for 48 ms, held 200 ms |
| WebRTC legacy AEC | The coherence gain is raised so that the echo-only minimum maps to 10⁻³ to 10⁻⁸; comfort noise is added |

**Near-end cost.** Call products accept some clipping at double-talk transitions; AEC3 softens it with a near-end tuning, Speex with its −15 dB floor.

**For ASR.** A neural suppressor after a linear AEC lowered WER 38 to 53 percent (Panchapagesan et al. 2023). Here the canceller with suppression still left 681 duplicates on 19 Oct (26.8 percent of "me"), and the 21 Oct survivors had lost 17.5 dB. Suppression stops echo words only when echo-only frames fall below what the ASR ignores, here under the mic's own floor: AEC3's target, not a fixed −12 dB.

## 5. Near-end gate at the signal level

**Rule.** Per 20 ms frame, while the loopback is active:
1. Pass the frame if its 300 to 3,400 Hz level exceeds the floor plus the predicted echo by X dB. The predicted echo is the loopback minus the return loss, or, once the canceller runs, its residual estimate.
2. Otherwise attenuate the frame 30 to 40 dB.
3. Hold each pass 200 to 300 ms.
4. Offline, also widen the pass mask 200 ms both ways, so onsets survive.

Geigel's detector is the ancestor, unreliable for its fixed 6 dB return loss (Tashev); here the return loss is measured.

**Margins:**
- **Headphones:** owner −45 against leak frames at −61 dBFS, about 16 dB. Separable, isolated leak words included.
- **Raw speakers:** owner −40 against echo −44 (21 Oct), 5 dB apart on 19 Oct. Not separable raw; about 20 dB apart after the canceller.

**Failure modes:** clipped onsets and word ends (hold and mask; live needs a 100 to 200 ms feed delay), double talk on the raw signal, the owner near the floor, level steps (a floor tracker), and whether muted frames make the ASR hallucinate.

## 6. Call products against a transcriber

**Call products:**
- **WebRTC (Chrome, Meet):** AEC3 as above; Google adds detection of failing device AECs and "adaptive audio" for several laptops in a room.
- **Zoom:** echo cancellation Auto, Low or High (a CPU trade-off).
- **Teams voice isolation:** removes voices that do not match an on-device voice profile; enrolment can be passive.

All clean the sender's mic before it leaves the device, so their transcripts never see a second copy, and all must stay causal (20 ms in the ICASSP 2023 AEC challenge) and listenable.

**A transcriber has more:** the far end's words; the whole file after Stop; no listener on the processed mic (muting is free, the archive stays raw); the owner's clean speech for enrolment; a second of live slack.

## 7. Model-side options

- **Target-speaker ASR** (multitalker Parakeet, conditioned SSA): not in NeMo-Speech.cpp (research 04).
- **Personal VAD** (130k parameters, per-frame target / other / none) **and VoiceFilter-Lite** (2.2 MB, −25.1 percent WER on overlap): the right shape, no released weights.
- **Owner check by embedding:**
  - Stack: plan 07's TitaNet-L or CAM++ (about 27 MB) through sherpa-onnx or `ort`, which plan 12 brings in for punctuation.
  - Enrolment: the same recording's owner-alone stretches (19 Oct: 1,290 owner's words with the loopback silent). Same mic, room and day; Teams' passive enrolment is the precedent.
  - Limits: short segments (ECAPA-class EER about 18 percent at 1 s, against under 1 percent on full trials; Kang et al. 2026), mixtures and backchannels.
  - Budget: plan 07's 20 crops in under 2 s of CPU.
- **Cheapest probe:** diarise the 21 Oct mic WAVs with the existing runtime (`record.diarize_mic`) and see whether the duplicates leave the owner's speaker. No new model.
- **Neural AEC:** DTLN-aec (MIT, 1.8 to 10.4M parameters, TF-Lite only) or personalised AEC (ICASSP 2023, 15 to 25 s of enrolment), later.

## 8. The approaches against the three recordings

| Approach | 19 Oct speakers | 21 Oct headphones | 21 Oct speakers | Cost, live / post-pass | Risk with headphones, no leak | Proof |
|---|---|---|---|---|---|---|
| Text runs | 92 percent of raw duplicates; the 53 after B2a to measure | 81 percent post-pass, 74 live | 95 percent of 158 live survivors; 20 of 22 post-pass | µs per word, 1 s retraction / seconds | About 0.1 percent chance, plus quotes | Shifted chance; a fixture of the owner repeating far-end phrases |
| Weak-coupling detection | Coupled already | Couples at 26 s; removes nothing | Coupled already | Under 1 percent of a core / seconds | Null coupling (lag consistency guards) | 19 Oct null and silent-mic controls never couple |
| Deeper suppression | Fewer of the 681 pre-gate duplicates | Not engaged | Depth alone not enough (17.5 dB survivors) | None extra | None | Spliced double-talk WER ≤ 7.6 percent |
| Floor and residual gate | After the canceller only | Removes leak words, isolated ones too | After the canceller | 100 to 200 ms feed delay / negligible | Very soft owner speech over the far end | Owner's headphone recording; quiet FLEURS; clipped onsets |
| Owner check | Garbled echo, quotes; mixtures risky | Rejects leak utterances of 2 s or more | Same | 27 to 100 MB model, about 100 ms per utterance | Short owner words | Accuracy on 19 Oct's labelled stretches |

## 9. Recommendation

**1. A text-run gate, after the acoustic gate, in the post-pass and live.**
- **The rule.** Drop three or more consecutive mic words that each match a loopback word at −0.5 to +1.0 s, with agreeing offsets and a content word.
- **Shorter runs** drop only with a tie-breaker: the word coefficient when coupled, level over the floor otherwise. Single words never drop on text.
- **Reversible:** everything goes to `echo_dropped`, so Restore works.

It needs no coupling and catches 74 to 95 percent of what survived on 21 Oct, live view included; quotes are its bounded risk.

**2. A floor-and-residual gate on the ASR feed, post-pass first.**
- **The rule.** While the loopback is active, frames within X dB of the floor plus the predicted echo (the canceller's residual once it is engaged) reach the ASR 30 to 40 dB down. Hold 200 to 300 ms, and widen the mask offline.
- **Inputs.** The weak-coupling state supplies the warning and the return loss by level. Live follows once the replay harness shows no clipped onsets.

AEC3's goal (echo under the masker) aimed at a recogniser: it stops the isolated and garbled words text cannot match.

**3. An owner check as a tie-breaker, starting with the mic-diarisation probe.**
- **Then:** plan 07's embeddings, enrolled from the recording's own owner-alone stretches.
- **Scope:** only what parts 1 and 2 cannot decide (quotes, two-word runs, unmatched utterances inside echo stretches), and only for utterances of 2 s or more.

It alone separates voices when timing, text and level agree.

## 10. Open questions for a reviewer

1. Why does Nemotron emit words from a signal under the mic's floor, and would attenuated frames make it hallucinate?
2. Does "owner first" allow a gate that will remove some quotes? Which rate is acceptable, and should two-word runs ever drop?
3. Should the canceller engage on a headphone leak (38 dB level difference, 55 dB by the formula, adaptation under the floor), or does the floor gate suffice?
4. Why are post-pass duplicates stamped 0.3 to 0.4 s late against 0.07 s live? It sets the window.
5. Self-enrolment stores a voiceprint: consent, display and deletion (plan 07 shares this).
6. In the stored runs every mic word is `me` and no loopback word repeats across loopback speakers, so the far-end-labelled duplicates sit in segmentation, merge or display, beyond these approaches.

## Bibliography

- Carter, "Coherence and time delay estimation", Proc. IEEE 75(2), 1987.
- Cutler et al., "ICASSP 2023 Acoustic Echo Cancellation Challenge", IEEE OJSP 5, 2024.
- Ding et al., "Personal VAD: speaker-conditioned voice activity detection", Odyssey 2020.
- Google Workspace, "Echo cancellation" (Meet), workspace.google.com/resources/echo-cancellation.
- Kang, Cho, Kim, "Beyond short segments: expanding speaker embeddings with vector archives", arXiv 2609.25007, 2026.
- Marslen-Wilson, "Linguistic structure and speech shadowing at very short latencies", Nature 244, 1973.
- Microsoft Learn, "Manage voice isolation for your users' Microsoft Teams calls and meetings", 2026.
- Panchapagesan, Shabestary, Narayanan, "On training a neural residual acoustic echo suppressor for improved ASR", Interspeech 2023.
- Speex DSP, `libspeexdsp/preprocess.c` (echo suppression defaults −40 and −15 dB).
- Tashev, "Coherence based double talk detector with soft decision", ICASSP 2012.
- Wang et al., "VoiceFilter-Lite: streaming targeted voice separation for on-device speech recognition", Interspeech 2020.
- WebRTC: `api/audio/echo_canceller3_config.h`, `aec3/suppression_gain.cc`, `aec3/dominant_nearend_detector.cc`, legacy `aec/aec_core.c`.
- Westhausen, Meyer, "Acoustic echo cancellation with the dual-signal transformation LSTM network", ICASSP 2021.
- Zoom, "Managing advanced audio settings" (KB0066398).
