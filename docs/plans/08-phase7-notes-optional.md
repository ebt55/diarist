# Phase 7 (optional, unscheduled): transcript to meeting notes with a local LLM

Status: recorded 15 Oct 2026 so the architecture keeps a clean slot. Not scheduled; the transcript is the product.

## Design (if and when wanted)

- Same pattern as the speech engine: a `NotesEngine` trait in `crates/core` with one adapter that drives a child process, `llama-server` from the llama.cpp project (prebuilt Windows CUDA zips exist, Apache/MIT licensed), installed into the data root by the same downloader with SHA verification. No Python.
- Model picker driven by hardware: the app reads free VRAM (NVML) and RAM and shows GGUFs that fit, from a small curated manifest (`manifests/llm.json`: name, HF URL pinned to a commit, quant, file size, VRAM estimate at 8k context, licence). First candidates for a 10 GB card with the speech models unloaded: Qwen3 8B Q4_K_M (about 5.5 GB), Gemma 3 12B Q4_K_M (about 7.5 GB), Llama 3.x 8B Q4_K_M (about 5 GB); for 32 GB RAM with CPU offload, larger quants are allowed with a speed warning. Users can also point at any GGUF on disk.
- Prompting: a notes template (summary, decisions, action items with owner where a speaker name exists, open questions) over the speaker-labelled transcript, chunked by time with a final merge pass for long meetings. Output stored as a `notes` row per meeting with the model name and prompt version, editable in the UI, exported with the transcript.
- VRAM rule: notes never run while a recording is active; the `JobQueue` serialises them after post-passes.

## Why it waits

The user does not need notes; speaker-separated transcripts in a searchable DB are the deliverable. Everything above is additive and touches no existing table except adding one.
