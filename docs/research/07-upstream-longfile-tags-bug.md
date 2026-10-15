# Draft upstream issue (not posted; the user decides)

Target: https://github.com/NVIDIA/NeMo-Speech.cpp/issues

---

**Title:** `transcribe --diarize` assigns wrong speaker tags to the first ~10 min of a 21 min file (standalone `diarize` on the same file is correct)

## Summary

On a 21 min 12 s recording from the public AMI Meeting Corpus (session ES2002a, 4 speakers, Mix-Headset channel, CC-BY-4.0), `nemo-speech transcribe <file> --diarize --json` returns word speaker tags that are about 62% correct for the first 10 minutes and about 99% correct afterwards, scored against the corpus's reference speaker labels. In the early part, most of one speaker's words (AMI participant FEE005) are merged into another speaker's label (MEE008).

The diarizer itself is not at fault. Standalone `nemo-speech diarize` on the same file has 0.6% speaker confusion, and re-tagging the same ASR words from that RTTM gives 99.5%.

Files cut from the same recording at 350, 420 and 600 s are tagged correctly (99.5–99.7%). Adding `--stream --endpointing` to the same 21-minute command also fixes it (99.6%). The fault therefore looks specific to long inputs that produce a single end-of-file final.

## Environment

- nemo-speech 0.2.0, release archive `nemo-speech-0.2.0-windows-x86_64-cuda.zip` (SHA-256 `9ffe5e842c39a7d3211f3b2c3b3aa1bca8c0b2455d96d961ed5f22ec5108f98d`)
- Windows 11 Pro 10.0.26200, RTX 3080 10 GB, driver 610.88, `--device cuda:0`
- ASR: `nvidia/nemotron-speech-streaming-en-0.6b` q8_0 (rev ebe59e5). Diarizer: default `nvidia/Nemotron-3-Diarization` q8_0 (rev f667ed7)
- Also reproduces with `nvidia/parakeet-tdt-0.6b-v3` q8_0 as the ASR: 63.2% overall

## Repro (public data)

```bash
# AMI ES2002a, Mix-Headset, CC-BY-4.0, 16 kHz mono, 21:12
curl -L -o ES2002a.Mix-Headset.wav https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/ES2002a/audio/ES2002a.Mix-Headset.wav
curl -L -o ES2002a.rttm https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/main/only_words/rttms/train/ES2002a.rttm

nemo-speech pull nemotron-en
nemo-speech pull nemotron-3-diarization

# A: wrong tags early in the file
nemo-speech transcribe ES2002a.Mix-Headset.wav --model nemotron-en --diarize --json --device cuda:0 -o A.json
# B: correct
nemo-speech transcribe ES2002a.Mix-Headset.wav --model nemotron-en --stream --endpointing --diarize --json --device cuda:0 -o B.json
# C: standalone diarizer, correct
nemo-speech diarize ES2002a.Mix-Headset.wav --device cuda:0 --format rttm -o C.rttm
# D: first 600 s only, correct
ffmpeg -i ES2002a.Mix-Headset.wav -t 600 -c copy first600.wav
nemo-speech transcribe first600.wav --model nemotron-en --diarize --json --device cuda:0 -o D.json
```

**Scoring.** For each output word, take the midpoint and keep the word only when exactly one reference speaker is active there. Map hypothesis labels to reference labels with the best one-to-one mapping, then report the share of words whose label matches.

## Results

| Window (s) | A: `--diarize` | B: `--stream --endpointing --diarize` |
|---|---|---|
| 0–300 | 62.2% | 99.5% |
| 300–600 | 63.5% | 99.8% |
| 600–900 | 99.8% | 99.8% |
| 900–1272 | 99.1% | 99.1% |
| Overall (2020 words) | **63.3%** | **99.6%** |

| Other checks | Result |
|---|---|
| A's words re-tagged from C.rttm by maximum overlap | 99.5% |
| C.rttm vs the reference (pyannote.metrics, collar 0) | DER 27.2% (miss 11.0, FA 15.6, **confusion 0.6**) |
| Same command as A on cuts of 0–350, 0–420 and 0–600 s | 99.6%, 99.5%, 99.7% |
| Confusion in A's first 10 minutes | Hyp speaker 2 holds 608 FEE005 words and 768 MEE008 words; hyp speaker 1 holds only 467 FEE005 words |

## Workarounds

1. Add `--stream --endpointing`. This emits per-utterance finals, so tags are assigned incrementally. It only applies to cache-aware streaming models; Parakeet TDT rejects `--stream`.
2. Run standalone `nemo-speech diarize` and re-assign each word's speaker by maximum time overlap with its segments.

## Hypothesis

Without endpointing, the whole recording ends up in one final, and its words are tagged only once at the end. `diar.h` says long streams compact older per-frame probabilities into finalized segments past roughly a 20 minute horizon (`nemo_speech_diar_frame_probs_start`). The tag lookup for words inside the compacted region may read the wrong state. That would fit both observations: per-utterance finals (tagged before compaction) are correct, and files of 10 minutes or less are correct. This is unverified; we have not read `RecognitionStream::flush_diar_deficit_` or the tag refresh path closely.
