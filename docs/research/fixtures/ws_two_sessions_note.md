# How the two-session (mic + loopback) scenario was driven

This is the spike from 15 Oct 2026; the results are in `../03-runtime-spike.md` §E.

## Server

`nemo-speech serve` (v0.2.0, Windows CUDA zip) was launched once and shared by both sessions:

```
nemo-speech --json serve --asr-model <nemotron-speech-streaming-en-0.6b.q8_0.gguf> \
  --diar-model <Nemotron-3-Diarization.q8_0.gguf> --device cuda:0 --host 127.0.0.1 --port 8766 \
  --no-ui --asr.batching.state_arena_slots 2 --asr.endpointing.enable=true
```

The API key came from the env var `NEMO_SPEECH_HTTP_API_KEY`, a random 32-hex value generated for the test.

## Clients

Two copies of the scratch client (`ws_client.py`, Python 3.13 + `websockets`) ran at the same time as two OS processes. Each:

- opened its own WebSocket to `/v1/audio/transcriptions/realtime` with `Authorization: Bearer <key>`
- sent one `session.update` with `{"sample_rate":16000,"word_timestamps":true,"speaker_diarization":true}`
- streamed binary PCM16 LE mono 16 kHz frames of 3200 bytes (100 ms) on an absolute real-time schedule (frame i sent at t0 + i·0.1 s)

What each session streamed:

| Session | Source (AMI ES2002a Mix-Headset) | Gap | Total | Commit |
|---|---|---|---|---|
| X ("mic") | 90 s starting at 56 s | none | 90 s | no |
| Y ("loopback") | 60 s starting at 400 s, then 10 s more | 20 s of zero frames after the first 60 s | 90 s | no |

Both clients waited 8 s after their last frame and then closed.

## What was measured

| Metric | How |
|---|---|
| Lag | Per `.delta` event: client elapsed time minus the event's `audio_processed` |
| VRAM | `nvidia-smi` total sampled every 250 ms, peak minus baseline |
| Recovery after the zero gap | `.completed` events after the gap that contain real words |

## Results

- **Lag:** X max 0.16 s, Y max 0.15 s (median −0.08 s for both).
- **VRAM:** +100 MiB over the idle server.
- **Finals:** X 25, Y 17.
- **After Y's 20 s silence gap:** 3 finals.

## Caveats

- Both sessions carried the same mixed recording, not a real mic and loopback pair.
- Each session has its own diarizer state, so its speaker ids (1..8) are independent. Speaker 1 in X is not speaker 1 in Y.
- To get one shared speaker namespace, mix both sources into one session.
