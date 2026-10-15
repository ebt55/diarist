# Engine protocol

Status: draft v0.1. This is the contract between the Rust app (`core`) and the inference engine process, whichever runtime sits behind it. The Rust side never sees runtime-specific flags; an adapter implements this protocol, either natively (our own thin C++ `meet-engine` binary linking the runtime's library) or by wrapping a vendor CLI.

## Transport

- Engine is a child process. One engine process per session (one file job or one live session). Crash isolation: the app survives an engine crash and can relaunch.
- **stdin (app to engine):** length-prefixed binary frames: `u8 type` + `u32 LE length` + payload.
  - type `0x01` CONTROL: payload is UTF-8 JSON (one object).
  - type `0x02` AUDIO: payload is `u8 channel_id` + `u64 LE pts_us` + int16 LE mono PCM at 16 kHz.
- **stdout (engine to app):** newline-delimited JSON (one event per line). Human-readable for debugging.
- **stderr:** free-form logs, captured and shown in the app's diagnostics pane.

## Control messages (app to engine)

```jsonc
{"cmd":"init","asr":"parakeet-tdt-0.6b-v3","diarizer":"nemotron-3-diarization","device":"cuda","max_speakers":8}
{"cmd":"transcribe_file","job_id":"...","path":"C:/...wav","diarize":true,"language":"en"}
{"cmd":"start_stream","job_id":"...","channels":[{"id":0,"name":"mic","diarize":false},{"id":1,"name":"loopback","diarize":true}]}
{"cmd":"stop_stream","job_id":"..."}        // flush and finalize everything
{"cmd":"cancel","job_id":"..."}
{"cmd":"shutdown"}
```

## Events (engine to app)

```jsonc
{"ev":"ready","engine":"meet-engine 0.1.0","runtime":"...","models":{...},"vram_mb":1234}
{"ev":"progress","job_id":"...","done_ms":120000,"total_ms":3600000}
{"ev":"words","job_id":"...","channel":1,"final":false,
  "words":[{"t0":12.34,"t1":12.61,"text":"hello","conf":0.91,"spk":"spk_1"}]}
{"ev":"relabel","job_id":"...","channel":1,"from_t":10.0,"to_t":20.0,"map":{"spk_1":"spk_0"}}
{"ev":"finalized","job_id":"...","channel":1,"up_to_t":22.0}    // nothing before up_to_t will change
{"ev":"done","job_id":"...","stats":{"rtf":0.08,"speakers":3}}
{"ev":"error","job_id":"...","code":"oom","message":"..."}
```

Rules:
- Words are cumulative; a `words` event with `final:false` may be superseded by later words covering the same time range. The app keeps the latest.
- `relabel` lets the diarizer fix speaker permutations in the recent window (streaming diarizers do this for up to about 10 s).
- `finalized` is the only signal that lets the app commit to the DB.
- For file jobs, the engine may emit everything as `final:true` in chunks and a single `finalized` at the end.

## Word-to-speaker assignment

Lives in the engine (or the adapter), not the UI. Input: word timestamps from ASR, frame-level speaker activity from the diarizer. Algorithm to be fixed in 01-architecture.md after research; default is "majority speaker over the word's span, with a tie-break toward the previous word's speaker, and boundary words snapped to the nearest speaker-change point".

## Performance targets

- File mode: real-time factor under 0.1 on the 3080 for ASR + diarization (one hour in under six minutes).
- Live mode: `words` event within 1.5 s of speech; `finalized` within 10 s.
- VRAM: ASR + diarizer resident together under 4 GB.
