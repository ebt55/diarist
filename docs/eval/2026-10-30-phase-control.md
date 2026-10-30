# Eval: the phase control and the input-gain report on runtime 0.2.0 (30 Oct 2026)

Plan 18 chunk N2 (`docs/plans/18-beta6.md` section 3.5): two GPU reports on the shipped runtime, nemo-speech 0.2.0, that change no app code. (1) The phase control: the gate set, both models, with the audio shifted by a few tens of milliseconds at its start, to measure how far v0.2.0's own deleted words move with the chunk phase alone. Its band is what chunk R's deletion bar reads (plan 18 section 4.2, item 1), whether or not R runs. (2) The input-gain report: the EN model on two quiet-speech recordings with the input normalised to a −3 dBFS peak. Accuracy runs only, untimed. Opus 5.5 measurement agent.

## Rules, as the plan fixes them (quoted before any run)

1. **Phase control** (plan 18 section 3.5, item 1). "The gate set, both models, with the WAVs (a) trimmed by the first 70 ms, (b) given 20 ms and (c) 40 ms of leading digital silence; configs `shipped_en_ph70`, `_ph20`, `_ph40` and the same for 3.5 [...]. Output: deletions, gaps of 2 and 5 s or more and AMI cpWER per offset beside the unmodified run (research/19's `rs_*_shipped`). The phase noise band for deletions is the largest relative change over the three offsets, per model." Chunk R's bar that reads it (section 4.2, item 1): "pooled deletions per model no higher than v0.2.0's by more than the phase noise band of N2 item 1 [...]; the band is never set after the 0.3.0 numbers are seen".
2. **Input gain, report only** (section 3.5, item 2). "FLEURS English (the 10-minute slice) and AMI EN2002a Array1-01, EN model, with the input normalised to a peak of −3 dBFS over the whole file, against the cached unmodified runs. Output: deletions and WER. If deletions fall by 10 percent or more on both, beta.7 gets a design note [...]; if not, the lever is closed with the numbers."

## Method details fixed before any run

The plan leaves these open; they were fixed here, before the first GPU job, and are not changed after.

- **Deletions** are the speaker-agnostic WER's deletions (`w_del` of `tools/eval/metrics.py`: time-ordered normalised words, Levenshtein), the same count research/19 and research/21 use. **Pooled** means summed over the 7 gate sessions. The **band** per model is max over the three offsets of |pooled deletions at the offset − pooled deletions of the unmodified run| / pooled deletions of the unmodified run. It is a magnitude; the signed changes are reported beside it. Per-session changes are reported but do not enter the band.
- **Gaps** are `gaps` and `gaps_5s` of `metrics.py` (research/19 section 0). Report only, as are the new and gone gaps of 5 s or more against the unmodified run (a gap is new when no gap of the unmodified run overlaps it in time), which chunk R's third clause ("no new gap of 5 s or more that the phase controls do not also show") can read.
- **Timeline.** A shifted input moves every word by the shift. run.py moves the hypothesis times (words, segments and the RTTM) back by the same amount before scoring, so the scored region and the gaps are read on the unmodified audio's timeline: trimmed by 70 ms means +0.070 s, 20 and 40 ms of silence mean −0.020 and −0.040 s. cpWER and the deletions do not depend on it.
- **Samples.** 70, 20 and 40 ms at 16 kHz are 1,120, 320 and 640 samples, cut from or prepended (as zeros) to the 16 kHz mono WAV the harness transcribes (`run.py` `prepare_audio`, config key `audio_shift_ms`).
- **Pipeline.** The configs run the app's file pass (no `engine_args`: lookahead 13 and `v3-offline` from the manifest, `--stream --endpointing --diarize`) with the loop collapse off and punctuation off, as the `rs_*_shipped` runs were made (by a 0.1.0-beta.4 build, which had no collapse). `--no-diar-pass`: the standalone diarize pass never feeds the transcript.
- **The unmodified run.** `rs_en_shipped` and `rs_n35_shipped` (research/19, 26 Oct 2026), copied and rescored with today's `metrics.py` (`--metrics-only`), so both sides are scored by the same code. They were made by another build and from WAVs converted then, so a control comes first: configs `shipped_en_ph0` and `shipped_n35_ph0` (this build, no shift) on AMI ES2002a and Aug 2. If their runtime words (`engine.words.json`) equal the cached runs' word for word, the cached runs are the unmodified side, as the plan says. If not, `*_ph0` runs on the whole gate set and is the unmodified side, and the difference is reported.
- **Input gain.** Config `shipped_en_peak3` (key `audio_peak_dbfs = -3.0`): the WAV is scaled by one factor so its largest sample is 32768 × 10^(−3/20) (rounded, clipped to 16 bits). The unmodified runs: `rs_en_shipped` on `fleurs-en-given` (research/19) and the public baseline's `shipped_en` run of `ami-en2002a-arr1` (docs/eval/2026-10-27-baseline-public.md), both without loop collapse, rescored here. FLEURS deletions are `run.py --fleurs`'s `del` (whole-slice alignment); AMI EN2002a's are `w_del`. "Fall by 10 percent or more on both" is read as (unmodified − gain) / unmodified ≥ 0.10 on each recording.
- **GPU.** Every batch through the shared queue (`gpu-run2.sh`, tier 2), nvidia-smi checked first; untimed, so no quiet.lock, and no speed figure is reported.

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88; the runs went through the shared GPU queue on 30 Oct 2026, 23:16 to 00:01 IST, with nothing but the desktop on the GPU (nvidia-smi before each batch: 0 to 7 % and 1.5 to 1.8 GB) |
| diarist-cli | built in release from `00cc9a6` (0.1.0-beta.5 and the post-release docs), SHA-256 `dc4c7ff307791359e557b84e07ef0600d0ebfc4c1b493e7b478c02364b635003` |
| Engine | nemo-speech 0.2.0, `nemo-speech-0.2.0-windows-x86_64-cuda`: a byte copy of the installed app's engine folder (79 files, every SHA-256 equal); `nemo-speech.exe` `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272` |
| Models | byte copies of the installed app's models, SHA-256 equal to `manifests/models.json`: EN `d9a01898…3812d`, 3.5 `a5c435f2…429ae`, diarizer `08456d9e…a3a1` |
| Settings recorded in run.json | `right_context` 13, `diar_preset` v3-offline, `device` cuda:0, `language` en, `engine_args` empty, `collapse_loops` false, `punctuation` false, `diar_pass` false, `audio_shift_ms` as the config says |
| Sessions | the gate set: Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14 (Zoom references) and AMI ES2002a (manual word reference), 324 min |

## The control: this build reproduces the cached runs

`shipped_en_ph0` and `shipped_n35_ph0` on AMI ES2002a and Aug 2: the runtime's words (`engine.words.json`) equal the cached `rs_*_shipped` runs' word for word, with the same times and tags (only the input path in the JSON differs); the transcripts' words, times, segments and speaker partition are equal; every metric is equal. So the cached runs are the unmodified side, as the plan says, and the remaining gate sessions were not re-run.

Rescored with today's `metrics.py`, the cached runs give pooled deletions of 1,409 (EN) and 1,879 (3.5). Research/19's table, from its scratch analysis of the same runs, has 1,396 and 1,879; every number below comes from `metrics.py` alone, on both sides.

## Results: the phase control

Cells: deletions / gaps of 2 s or more / gaps of 5 s or more / cpWER (%). Offsets: −70 ms is the first 70 ms trimmed, +20 and +40 ms are leading digital silence.

### Nemotron EN

| Session | Min | Unmodified | −70 ms | +20 ms | +40 ms |
|---|---|---|---|---|---|
| Aug 2 | 44 | 268 / 21 / 1 / 9.52 | 247 / 17 / 2 / 9.68 | 251 / 19 / 1 / 9.49 | 269 / 23 / 1 / 9.58 |
| Aug 9 | 44 | 162 / 25 / 4 / 8.11 | 162 / 26 / 4 / 7.73 | 168 / 26 / 4 / 7.97 | 174 / 28 / 4 / 7.96 |
| Sep 7 | 48 | 164 / 50 / 2 / 7.22 | 174 / 60 / 3 / 6.97 | 169 / 53 / 2 / 7.24 | 167 / 53 / 2 / 7.31 |
| Sep 5 | 45 | 93 / 14 / 2 / 4.70 | 97 / 10 / 2 / 4.99 | 104 / 11 / 2 / 4.94 | 98 / 10 / 2 / 4.86 |
| Sep Logistics | 46 | 119 / 10 / 1 / 7.03 | 117 / 10 / 1 / 7.13 | 122 / 13 / 1 / 7.14 | 129 / 13 / 1 / 7.16 |
| Aug 14 | 77 | 264 / 22 / 9 / 43.94 | 260 / 24 / 8 / 45.99 | 266 / 24 / 8 / 44.48 | 270 / 26 / 10 / 46.02 |
| AMI ES2002a | 21 | 339 / 10 / 4 / 18.03 | 344 / 10 / 5 / 18.03 | 345 / 8 / 4 / 18.38 | 340 / 9 / 3 / 18.30 |

### Nemotron 3.5

| Session | Min | Unmodified | −70 ms | +20 ms | +40 ms |
|---|---|---|---|---|---|
| Aug 2 | 44 | 361 / 28 / 5 / 10.31 | 353 / 27 / 4 / 10.70 | 359 / 29 / 5 / 10.45 | 377 / 30 / 6 / 10.71 |
| Aug 9 | 44 | 236 / 31 / 6 / 8.40 | 253 / 35 / 8 / 8.54 | 255 / 32 / 7 / 8.49 | 251 / 33 / 6 / 8.40 |
| Sep 7 | 48 | 251 / 60 / 4 / 7.45 | 265 / 61 / 4 / 7.71 | 250 / 57 / 4 / 7.54 | 249 / 54 / 4 / 7.61 |
| Sep 5 | 45 | 176 / 9 / 1 / 5.12 | 179 / 12 / 3 / 5.05 | 189 / 9 / 2 / 5.35 | 184 / 11 / 3 / 5.39 |
| Sep Logistics | 46 | 141 / 14 / 2 / 6.60 | 136 / 11 / 1 / 6.56 | 133 / 10 / 1 / 6.52 | 135 / 12 / 2 / 6.32 |
| Aug 14 | 77 | 318 / 22 / 9 / 42.93 | 341 / 20 / 7 / 45.17 | 321 / 22 / 10 / 43.52 | 345 / 23 / 10 / 45.05 |
| AMI ES2002a | 21 | 396 / 12 / 3 / 20.67 | 414 / 15 / 3 / 21.29 | 404 / 11 / 2 / 21.67 | 395 / 11 / 2 / 21.55 |

### Pooled over the gate set, and the band

| Model | Offset | Deletions | Δ | Relative change | WER % | ins | sub | Gaps ≥ 2 s | Gaps ≥ 5 s | New / gone ≥ 5 s | AMI cpWER |
|---|---|---|---|---|---|---|---|---|---|---|---|
| EN | unmodified | 1,409 | | | 7.53 | 1,315 | 1,036 | 152 | 23 | | 18.03 |
| EN | −70 ms | 1,401 | −8 | −0.57 % | 7.53 | 1,330 | 1,028 | 157 | 25 | 3 / 1 | 18.03 |
| EN | +20 ms | 1,425 | +16 | +1.14 % | 7.59 | 1,348 | 1,015 | 154 | 22 | 1 / 2 | 18.38 |
| EN | +40 ms | 1,447 | +38 | +2.70 % | 7.58 | 1,333 | 1,007 | 162 | 23 | 1 / 1 | 18.30 |
| 3.5 | unmodified | 1,879 | | | 7.81 | 812 | 1,209 | 176 | 30 | | 20.67 |
| 3.5 | −70 ms | 1,941 | +62 | +3.30 % | 8.00 | 838 | 1,214 | 181 | 30 | 7 / 6 | 21.29 |
| 3.5 | +20 ms | 1,911 | +32 | +1.70 % | 7.91 | 850 | 1,187 | 170 | 31 | 5 / 4 | 21.67 |
| 3.5 | +40 ms | 1,936 | +57 | +3.03 % | 7.99 | 839 | 1,217 | 174 | 33 | 8 / 5 | 21.55 |

**The phase noise band for deletions, per model (rule 1): EN 2.70 percent (the +40 ms offset), Nemotron 3.5 3.30 percent (the −70 ms offset).** This is the band chunk R's first bar reads: 0.3.0's pooled deletions on its sets may be at most 2.70 percent (EN) and 3.30 percent (3.5) above v0.2.0's.

### What else the runs show (report only; nothing here moves a bar)

- **Per session the phase moves deletions much more than pooled.** The largest relative change per session over the three offsets: EN Aug 2 7.8 %, Aug 9 7.4 %, Sep 7 6.1 %, Sep 5 11.8 %, Sep Logistics 8.4 %, Aug 14 2.3 %, AMI 1.8 %; 3.5 4.4, 8.1, 5.6, 7.4, 5.7, 8.5 and 4.5 %. Changes go both ways inside a session (EN Aug 2: −7.8, −6.3, +0.4 %).
- **The unmodified phase is among the best.** Five of the six offset runs have more pooled deletions than the unmodified audio; only EN at −70 ms has fewer (−0.57 %). With 3.5 every offset is worse by 1.7 to 3.3 %. Pooled WER moves by at most +0.06 points (EN) and +0.19 (3.5).
- **AMI cpWER moves with the phase alone:** EN by 0.00 to +0.35 points, 3.5 by +0.62 to +1.00. Aug 14 (the in-room stress session) moves by about +2 points at −70 and +40 ms for both models, from speaker attribution (WDER 19.4 to 21.1 and 21.8 with EN, 19.3 to 20.9 and 21.7 with 3.5); its WER moves by at most 0.35 points.
- **Gaps of 5 s or more:** 1 to 3 new and 1 to 2 gone per offset with EN, 5 to 8 new and 4 to 6 gone with 3.5. `tools/eval/phase.py --json` lists them per session with their times, for R's third bar ("no new gap of 5 s or more that the phase controls do not also show").
- **Beside research/19's build of runtime `main`** (`e680fc3`, gate set, scored by research/19's analysis: EN deletions 1,442 against 1,396, +3.3 %; 3.5 1,897 against 1,879, +1.0 %; AMI cpWER +0.7 and +0.9 points): 3.5's change is inside its band and EN's is 0.6 points above its band. Neither is a verdict on 0.3.0, which R measures on the tag. AMI cpWER moved by +0.62 to +1.00 points with 3.5 from the phase alone, so R's speaker bar of +0.1 on AMI (plan 18 section 4.2, item 2) sits inside the phase noise for 3.5; the bar is the plan's and is not changed here.

## Results: the input-gain report

EN model, the input scaled once so the file's peak is −3 dBFS, against the cached unmodified runs (rescored here; both equal their cached numbers).

| Recording | Source peak | Gain applied | WER % | Deletions | ins | sub | Other |
|---|---|---|---|---|---|---|---|
| FLEURS English, 10-minute slice (55 utterances, 1,186 words) | −0.16 dBFS | −2.84 dB | 21.50 → 24.62 | 189 → 225 (+19.0 %) | 15 → 16 | 51 → 51 | words out 1,012 → 977 |
| AMI EN2002a Array1-01 (36 min, 7,878 words) | −8.02 dBFS | +5.02 dB | 37.66 → 37.13 | 2,555 → 2,475 (−3.1 %) | 28 → 32 | 384 → 418 | cpWER 40.12 → 39.64; gaps 83 / 25 → 84 / 24 |

**Rule 2: deletions did not fall by 10 percent or more on both (FLEURS +19.0 %, AMI EN2002a −3.1 %), so the input-gain lever is closed with these numbers and beta.7 gets no gain-stage design note from N2.**

What the numbers say: a whole-file peak normalisation cannot lift the quiet speech the lever was aimed at. The FLEURS slice joins 55 recordings and one of them peaks at −0.16 dBFS, so "−3 dBFS over the whole file" turned the slice down by 2.84 dB, and the EN model then dropped 36 more words: its deletions are sensitive to level, in the direction research/12 section 5 described. On the far-field AMI array, +5 dB removed 80 of 2,555 deletions. A gain that acts per utterance or per stretch was not part of this report and is not measured here.

## GPU use

Untimed accuracy runs, no quiet.lock. Lock held through `gpu-run2.sh`: the control 2.0 min, the six offset configs 34.5 min in three batches (15.7, 9.4 and 9.5 min; they include the harness's own waits while the desktop app's drawing held the GPU above 25 % utilisation), the input gain 0.6 min. The `nemo-speech transcribe` steps inside them sum to 28 min.

## Reproduce

```sh
cd tools/eval
# DIARIST_EVAL_ENGINE_DIR / DIARIST_EVAL_MODELS_DIR: byte copies of the app's engine and models
uv run python -I run.py --config shipped_en_ph70 --sessions aug2,aug9,sep7,sep5,seplog,aug14,ami --no-diar-pass   # and _ph20, _ph40, shipped_n35_*
uv run python -I phase.py --base rs_en_shipped --configs shipped_en_ph70,shipped_en_ph20,shipped_en_ph40
uv run python -I phase.py --base rs_n35_shipped --configs shipped_n35_ph70,shipped_n35_ph20,shipped_n35_ph40
uv run python -I run.py --config shipped_en_peak3 --fleurs en
uv run python -I run.py --config shipped_en_peak3 --sessions-file sessions.public.toml --sessions ami-en2002a-arr1 --no-diar-pass
```

`rs_*_shipped` and the public baseline's `shipped_en` run are read from copies of their run folders, scored with `--metrics-only` under a scratch configs file that names them with `collapse_loops = false`.
