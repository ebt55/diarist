# meet-assist: product spec

Status: draft v0.1, 15 Oct 2026. Owner: Ebin. Planner: Fable 5.1. Implementation: Opus 5.5 agents.

## One sentence

A fully local Windows app that turns meetings (live, or from an audio file) into speaker-separated transcripts and stores them in a searchable local database.

## Who it is for

Primarily the author, on one machine: Windows 11, RTX 3080 10 GB, 32 GB RAM. Secondarily anyone with a similar NVIDIA GPU after the OSS release. Nothing leaves the machine, ever.

## What it must do (v1 + v2)

1. **Post-hoc transcription (v1).** Drop an audio/video file in. Get a transcript where every word carries a speaker label and a timestamp. Up to 8 speakers. Runs on the GPU. A one-hour meeting should finish in well under five minutes.
2. **Live transcription (v2).** Press Record. The app captures the microphone and the system loopback (what the user hears from Teams, Zoom, Meet, browser). Transcript appears within about a second, speaker labels appear and may be revised for a few seconds, then settle. The mic channel is "me"; the loopback channel is diarized into "them".
3. **Speaker separation quality is the headline feature.** Use the strongest open, locally runnable diarizer available, and measure it on the user's own recordings, not just published benchmarks.
4. **Local database.** Every meeting, segment, word, speaker and rename lands in SQLite with full-text search. The DB is the product; the UI is a window onto it.
5. **Speaker management.** Rename "Speaker 2" to "Alice". Merge two labels. Later: recognize Alice across meetings via voice embeddings.
6. **Export.** Markdown, plain text with timestamps, JSON, SRT/VTT.
7. **Fast and light.** Native-feeling Windows app. Cold start to window well under a second. Installer small; the ML models download on first run into a local models folder.
8. **Painless install.** No Python, no torch, no CUDA toolkit required from the user. A single installer or portable zip plus a first-run model download. The GPU driver is the only prerequisite.

## What it does not do

- No meeting notes, summaries, action items or LLM features in v1 or v2. The transcript is the deliverable. (A later "send transcript to a local LLM" hook is fine, but out of scope now.)
- No cloud, no accounts, no telemetry.
- No macOS or Linux in v1. Keep the door open in the code structure, but do not spend time on it.
- No calendar integration, no auto-join of meetings.

## Quality bar

- Diarization: on 3 to 5 hand-labelled meetings of the user's own, fewer than 10 percent of words attributed to the wrong speaker, and the speaker count correct in most of them. Measured, written down, kept as a regression set.
- ASR: word error rate comparable to Parakeet TDT 0.6B published numbers on clean English meeting audio.
- Live: end-to-end latency (speech to text on screen) under 1.5 seconds; final speaker labels settle within 10 seconds.
- Robustness: a crash in the inference engine never loses already-transcribed text. The app keeps running and offers to restart the engine.

## Constraints

- GPU memory budget for inference: 6 GB of the 10 GB, leaving room for a browser and the meeting app sharing the GPU.
- The author has RSI. Agents write the code. The UI must be usable with minimal mouse work (keyboard shortcuts for record/stop, search, rename).
- OSS release: permissive licence (MIT or Apache-2.0) for our code; every bundled model and runtime must have a licence that permits redistribution without gating.

## Open decisions (to be settled in 01-architecture.md)

- Inference runtime and the exact engine integration contract.
- UI framework and frontend.
- Audio capture crate and alignment strategy.
- Install/update mechanism for the engine and models.
