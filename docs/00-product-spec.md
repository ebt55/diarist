# Diarist: product spec

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

## Guiding principle: nobody fights the app

Like ComfyUI, the user owns the folders. Models and the runtime are plain files in plain folders; drop one in and it is usable after Refresh, no app release needed. Every download has Retry, a copyable link, the expected hash and the destination folder, so a browser plus a file copy always works when the built-in download does not. Nothing is a hard gate: first-run setup can be skipped and each action (Import, Record) says exactly what it is missing and offers the one-click fix. Users can add models the app has never heard of by URL or by file, pick the runtime package that matches their GPU (CUDA, Vulkan, CPU) with a manual override, and read a plain "why isn't it working" checklist with fix buttons. The app checks what it can and explains what it cannot, in the user's words, with the next step attached.

## What it does not do

- No meeting notes, summaries, action items or LLM features in v1 or v2. The transcript is the deliverable. (A later "send transcript to a local LLM" hook is fine, but out of scope now.)
- No cloud, no accounts, no telemetry.
- No macOS or Linux in v1. Keep the door open in the code structure, but do not spend time on it.
- No calendar integration, no auto-join of meetings.

## Quality bar

- Diarization: on 3 to 5 hand-labelled meetings of the user's own, fewer than 10 percent of words attributed to the wrong speaker on average, no single meeting above 20 percent, and the speaker count exact on the single-speaker control and within one on the others. Measured, written down, kept as a regression set (docs/05-eval-data.md).
- ASR: word error rate on AMI ES2002a within 2 points of Parakeet TDT 0.6B v3's published AMI figure. (The user's Zoom captions are machine output and cannot serve as an ASR reference.)
- Live: first text on screen within 1.5 s of speech. Speaker labels in the live view are provisional; the canonical transcript, with full-context speaker labels, is ready within 60 s of Stop for a one-hour meeting.
- File mode: one hour of audio transcribed and diarized in under 6 minutes on the RTX 3080 (real-time factor under 0.1), including 77-minute and 2-hour recordings.
- Robustness: a crash or stall in the inference engine never loses audio or already-transcribed text. Recording continues, the app says so, and the transcript is completed when the engine is back.

Baseline measured 15 Oct 2026 (`docs/eval/2026-10-15-nemotron35.md`, `-nemotron_en.md`, cue-level Zoom references, not yet hand-corrected): WDER 0.2 to 0.6 percent on the four English Zoom meetings, 7 to 9 percent within 1 s of a speaker change, speaker count exact on the single-speaker control and within one elsewhere, AMI cpWER 19.8 (EN model) and 23.0 (3.5). The in-room stress session scores 20.7 percent WDER against a coarse reference. The bar is met on remote meetings; speaker changes and in-room audio are where work remains.

## Constraints

- GPU memory: ASR plus diarizer resident under 4 GB; never more than 6 GB of the 10 GB, leaving room for a browser and the meeting app sharing the GPU.
- The author has RSI. Agents write the code. The UI must be usable with minimal mouse work (keyboard shortcuts for record/stop, search, rename).
- OSS release: permissive licence (MIT or Apache-2.0) for our code; every bundled model and runtime must have a licence that permits redistribution without gating.

## Open decisions (to be settled in 01-architecture.md)

- Inference runtime and the exact engine integration contract.
- UI framework and frontend.
- Audio capture crate and alignment strategy.
- Install/update mechanism for the engine and models.
