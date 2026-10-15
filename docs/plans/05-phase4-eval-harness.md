# Phase 4: eval harness

Status: draft. Depends on Phase 1 (`diarist-cli transcribe` with JSON and RTTM export). Python under `tools/eval` only; nothing here ships.

## Goal

One command that runs the app's transcription over a fixed set of the user's meetings plus AMI ES2002a, compares with references, and writes a results table to `docs/eval/`. Every diarizer, fusion or post-processing change is judged by this table.

## Layout

```
tools/eval/
  pyproject.toml          uv-managed; deps: meeteval, pyannote.metrics, numpy, pandas, rich, webvtt-py (or own parser)
  sessions.example.toml   committed; sessions.toml is gitignored (points at private m4a and vtt paths)
  zoom_vtt_to_ref.py      transcript.vtt -> SegLST (JSONL) + RTTM, label -> speaker id map kept in a sidecar JSON
  ami_fetch.py            downloads AMI ES2002a Mix-Headset WAV + reference RTTM into data/samples/ami/
  run.py                  runs diarist-cli per session, converts output, computes metrics, writes the table
  metrics.py              DER (collar 0.25 and 0, overlap scored), JER, speaker-count error, WDER (overall and within 1 s of a reference change), cpWER, tcpWER
  README.md
```

## Metrics (from docs/research/04-diarization.md section 6)

- DER and JER via `pyannote.metrics` on RTTM pairs.
- cpWER, tcpWER via `meeteval` on SegLST pairs; `greedy_dicpwer` for the diarization-invariant baseline so cpWER minus DI-cpWER estimates attribution loss.
- WDER: align hypothesis words to reference words (meeteval alignment or a Levenshtein alignment on normalized text), count words whose mapped speaker differs; report overall and for words within 1 s of a reference speaker change.
- Speaker-count error: hypothesis count minus reference count.
- Zoom references are cue-level, so word times are interpolated inside a cue; note this in the table footer.

## Protocol

- Tuning set: Aug 2, Aug 9. Held-out: Sep 7, Sep 5. Controls: Sep Logistics (single speaker; report hypothesis speaker count), AMI ES2002a (word-level reference, overlap).
- `run.py --config <name>` records the engine version, model hashes, diarizer preset and fusion settings in the table header. Results go to `docs/eval/<yyyy-mm-dd>-<config>.md`.
- Hand correction: `run.py --export-for-correction <session>` writes the app's SegLST; the user corrects speaker labels and boundaries in a text editor; corrected files live in `data/eval/refs/` (gitignored) and take precedence over the Zoom reference when present.

## Acceptance

- `uv run run.py --config baseline` completes on the two tuning sessions and AMI, prints the table, writes the markdown file.
- The baseline table is committed and referenced from docs/00-product-spec.md's quality bar.
- README explains how to add a session and how to hand-correct one.

## Notes

- meeteval on Windows is unconfirmed; if it fails, run under WSL and document it. ffmpeg/ffprobe exist on this machine via Chocolatey and may be used here (not in the app).
- Never copy the user's recordings or transcripts into the repo.
