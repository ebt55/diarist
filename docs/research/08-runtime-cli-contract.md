# Runtime contract: nemo-speech 0.2.0 (Windows CUDA zip), as observed 15 Oct 2026

This is the interface the Rust adapters code against. Everything here was observed on this machine unless marked (doc), meaning taken from the upstream docs. Raw examples are in `fixtures/`.

## 0. Invocation rules (all commands)

- **Binary:** `<engine>\bin\nemo-speech.exe`. Spawn it with `PATH = <engine>\bin;%SystemRoot%\System32;%SystemRoot%`. The DLLs next to the exe are self-contained; the only thing needed from outside is the driver's `nvcuda.dll`.
- **Always pass absolute local GGUF paths.** An indexed short name (`nemotron-en`) or an omitted model is **downloaded automatically** on first use. `--diarize` with no `--diar-model` pulls `nvidia/Nemotron-3-Diarization`.
- **Model dir:** env `NEMO_SPEECH_MODEL_DIR` (there is no flag). The default is `%LOCALAPPDATA%\NeMoSpeech\models` (doc). Layout: `<dir>\<org>\<repo>\<40-hex revision>\<file>.gguf`, plus `<file>.gguf.verified` and `<file>.gguf.lock`. A local path always wins and never touches the network (doc).
- **Global flags go before the command:** `--json` (machine-readable results and errors), `--quiet`, `--verbose`.
- **Streams:** results go to stdout (or `--output`). Diagnostics go to stderr: `[nemo-speech] <cmd> session started|finished`, plus `[asr] …` and `[model] cached: <path>` lines. **There is no progress percentage for file jobs.** stderr lines are separated by blank lines.
- **Exit codes:**

  | Code | Meaning |
  |---|---|
  | 0 | OK |
  | 1 | runtime error (e.g. bad WAV) |
  | 2 | invalid argument, missing input, unknown flag or command |
  | 3 | missing model |
  | 4 | unsupported feature |

  Codes 3 and 4 come from `app/cli_util.h` and were not triggered here.

- **Errors with `--json`:** one line on **stderr**:
  ```json
  {"error":{"message":"nosuch.wav is not a file or directory (run 'nemo-speech help transcribe')","type":"invalid_argument","command":"transcribe","exit_code":2}}
  ```
  `type` is `invalid_argument` or `runtime_error` (both observed).
- **Input:** RIFF/WAVE only: PCM16 or float32, mono or stereo (stereo is averaged), 8–96 kHz (doc). Convert m4a etc. to 16 kHz mono PCM16 first.

## 1. `transcribe`

**File, Parakeet, no diarization** (offline TDT; rejects `--stream`):
```
nemo-speech.exe transcribe <in.wav> --model <...\parakeet-tdt-0.6b-v3.q8_0.gguf> --device cuda:0 --json --output <out.json> --force
```

**File, Nemotron, diarized (use this for long files):**
```
nemo-speech.exe transcribe <in.wav> --model <...\nemotron-speech-streaming-en-0.6b.q8_0.gguf> --stream --endpointing --diarize --diar-model <...\Nemotron-3-Diarization.q8_0.gguf> --device cuda:0 --json --output <out.json> --force
```

| Flag | Meaning |
|---|---|
| `--model, -m` | ASR GGUF path or indexed name (default `nemotron-3.5`) |
| `--device, --backend` | `auto`, `cpu`, `cuda[:N]`, `vulkan[:N]` |
| `--json` / `-f json` | JSON output. `--format` also accepts `text`, `srt`, `vtt` |
| `--output, -o` | Output file (one input). `--output-dir` for directories; `--force` overwrites |
| `--stream` | Feeds the WAV through the streaming recognizer in 160 ms chunks (cache-aware models only) |
| `--endpointing[=BOOL]` | Silence EOU produces per-utterance finals; `--stop-history-eou-ms N` (default 800) |
| `--diarize` | Tag words with the default diarizer |
| `--diar-model` | Tag words with this diarizer (implies diarization) |
| `--language, -l` | Language code, or `auto` for Nemotron 3.5 (doc) |
| `--no-punctuation`, `--verbatim`, `--speech-context PHRASE` | Optional |
| `--asr.SECTION.KEY V` | Any engine key, e.g. `--asr.batching.state_arena_slots 2` |
| `--no-warmup` | Skip warmup |

There is **no** max-speakers flag and **no** `--offline` on `transcribe`.

**Output** (a single object, pretty-printed):

| Field | Type | Unit / notes |
|---|---|---|
| `file` | string | Input path as given |
| `text` | string | Full transcript |
| `confidence` | number | Always 1 (upstream #55) |
| `duration` | number | Audio seconds |
| `languages` | string[] | Empty for EN models |
| `words[]` | array | In time order |
| `words[].word` | string | Carries attached punctuation |
| `words[].start`, `.end` | number | Seconds, float, 80 ms grid |
| `words[].confidence` | number | Always 1 |
| `words[].speaker` | int | 1-based; **omitted** when untagged or when diarization is off |

```json
{"file":"C:\\...\\clip.wav","text":"Um, well this is...","confidence":1,"duration":3.008,"languages":[],
 "words":[{"word":"Um,","start":0.24,"end":0.32,"confidence":1,"speaker":1}]}
```

Fixtures: `fixtures/parakeet_transcribe_5min.json`, `fixtures/nemotron_transcribe_diarize_5min.json`.

## 2. `diarize`

```
nemo-speech.exe diarize <in.wav> --model <...\Nemotron-3-Diarization.q8_0.gguf> --device cuda:0 --format json|rttm|text --output <out> --force [--recording-id ID]
```

**Preset:** `--preset v3-streaming` is the default for V3 and low latency. `--preset v3-offline` uses bigger chunks and caches but still runs streaming. `--offline` is the stateless full-attention pass, limited to about 6.6 min. It scored **worse** in testing (DER 43% vs 24%), so never use it.

Tuning flags: `--onset`, `--offset`, `--pad-onset`, `--pad-offset`, `--min-duration-on`, `--min-duration-off`.

Output formats (times in seconds, 3 decimals, sorted by start; segments of different speakers may overlap; speakers are 1-based in arrival order, up to 8 for V3):

- **json:** `{"file": "<path>", "segments": [{"start": 0.421, "end": 1.179, "speaker": 1}, …]}`
- **rttm:** `SPEAKER <rec-id> 1 <start> <duration> <NA> <NA> speaker_<N> <NA> <NA>`. `rec-id` defaults to the file stem.
- **text** (default, stdout): `<start>\t<end>\tspeaker <N>`

Fixtures: `fixtures/diarize_5min.json`, `.rttm`, `.txt`.

## 3. `pull`

```
set NEMO_SPEECH_MODEL_DIR=<app models dir>
nemo-speech.exe --json pull nemotron-en              (nvidia/nemotron-speech-streaming-en-0.6b)
nemo-speech.exe --json pull nemotron-3-diarization   (nvidia/Nemotron-3-Diarization)
nemo-speech.exe --json pull parakeet-tdt             (nvidia/parakeet-tdt-0.6b-v3)
```

`--json` result:
```json
{"artifacts":[{"cached":true,"path":"C:\\...\\models\\nvidia\\Nemotron-3-Diarization\\f667ed73aee57d40cc39428eb768b4fd87a0a29e\\Nemotron-3-Diarization.q8_0.gguf","repo":"nvidia/Nemotron-3-Diarization","role":"diarization"}],"repo":"nemotron-3-diarization"}
```

- **Download:** System32 `curl.exe`, HTTPS, resumable. The file is accepted only if its size and SHA-256 match the pins in `share\nemo-speech\model-index.json` (doc + observed).
- **Verify it ourselves:** recompute SHA-256 and compare with `model-index.json` → `models[].artifacts[].sha256` and `size`. The sidecar file `<file>.verified` contains:
  ```
  sha256=<hex>
  size=<bytes>
  mtime=<int>
  ```

| Model | Revision | Bytes | SHA-256 |
|---|---|---|---|
| nemotron-en q8_0 | ebe59e5a… | 699,872,960 | d9a01898… |
| Nemotron-3-Diarization q8_0 | f667ed73… | 107,012,128 | 08456d9e… |
| parakeet-tdt-0.6b-v3 q8_0 | 541d1f99… | 713,975,456 | e3880d0a… |

## 4. `serve` and realtime WebSocket

```
set NEMO_SPEECH_HTTP_API_KEY=<random 32 hex>
nemo-speech.exe --json serve --asr-model <nemotron-en.gguf> --diar-model <Nemotron-3-Diarization.gguf> --device cuda:0 --host 127.0.0.1 --port <free> --no-ui --asr.batching.state_arena_slots 2 --asr.endpointing.enable=true
```

**Readiness:**
- Poll `GET /ready` (unauthenticated). The listener only opens after the models load and warm up, so polls get connection refused until then. No 503 was observed (doc says 503 when no engine is ready). Then it returns 200 with `{"capabilities":["asr","diarization"],"device":"cuda:0","ready":true}`.
- It takes 4.6–5.9 s after launch.
- `--json serve` printed **nothing** on stdout or stderr while running (probably buffered), so don't wait for a ready line.
- `GET /health` gives status and version.
- Stop the server with SIGINT, or kill it. Port, API key and endpointing are fixed at launch; **endpointing is server-wide**.

**WebSocket:** `ws://127.0.0.1:<port>/v1/audio/transcriptions/realtime`, header `Authorization: Bearer <key>`.

1. Server sends `session.created`:
   `{"event_id":"event_3","session":{"input_audio_format":"pcm16","model":".nemotron-speech-streaming-en-0.6b.q8_0.gguf","sample_rate":16000},"type":"session.created"}`
2. Client may send one `session.update` before any audio: `{"type":"session.update","session":{"sample_rate":16000,"word_timestamps":true,"speaker_diarization":true}}`
   - Other fields (doc): `language`, `automatic_punctuation` (default true), `verbatim`, `profanity_filter`, `endpointing_ms`, `speech_contexts`, `prompt`.
   - `max_speaker_count` is ignored.
   - Server replies `session.updated`, echoing the session.
3. Client sends **binary frames**: raw little-endian PCM16 mono at `sample_rate`, any length (we used 3200 B = 100 ms). Base64 inside `input_audio_buffer.append` is also accepted (doc).
4. Server sends `conversation.item.input_audio_transcription.delta`:
   `{"audio_processed":0.9,"delta":"Um","event_id":"event_5","type":"…delta"}`
   - `delta` is the text appended since the last partial. If the hypothesis was rewritten, `delta` is the full partial text. Many deltas are `""`.
   - `audio_processed` is in seconds since session start.
5. Server sends `conversation.item.input_audio_transcription.completed`, one per endpoint:
   `{"audio_processed":4.1,"event_id":"event_37","transcript":"Um, well this is the kickoff meeting for our our project","type":"…completed","words":[{"confidence":1,"end":0.72,"speaker":1,"start":0.64,"word":"Um,"},…]}`
   - Word times are seconds since session start.
   - `speaker` is 1-based, numbered per session in arrival order. It is never revised later.
6. **Ending a session:** send `{"type":"input_audio_buffer.commit"}`. The server flushes and sends the final `.completed`, then `{"type":"input_audio_buffer.committed"}` (0.13 s after 80 s of audio). Then close normally (code 1000).
   - `input_audio_buffer.clear` / `response.cancel` produces `input_audio_buffer.cleared` (doc).
   - Failures produce `{"type":"error","error":{"message":…}}`.
   - Every event carries `event_id`.

Limits: cumulative audio per socket ≤ `--max-upload-mb` (512 MiB ≈ 4.6 h of 16 kHz PCM16). Read and write timeouts are 30 s, so keep sending frames, including silence.

Fixtures: `fixtures/ws_session_events.jsonl`, `fixtures/ws_two_sessions_note.md`.

## 5. `doctor` (engine status check)

`nemo-speech.exe --json doctor` returns one JSON object on stdout with exit code 0.

| Key | Meaning |
|---|---|
| `version` | `"0.2.0"` |
| `accelerator_available`, `accelerator_compiled`, `driver_runtime_compatible` | bool |
| `devices[]` | `{index, name:"CUDA0", description:"NVIDIA GeForce RTX 3080", type:"gpu"\|"cpu", memory_free, memory_total (bytes), async, events}` |
| `features` | bools: `asr`, `diarization`, `backend_cuda`, `realtime_websocket`, `model_pull`, … |
| `model_download` | `{available, executable}` |
| `runtime_warnings` | string[] |

**Healthy means:** `driver_runtime_compatible && features.backend_cuda && devices` contains `type=="gpu"`. Free VRAM must be at least 2.5 GB for live, or 5 GB if a Parakeet file job may run.

## 6. Pitfalls

1. **Long-file tagging bug:** plain `transcribe --diarize` on a 21-minute file mislabels the first ~10 min (63% correct). Always use `--stream --endpointing` (99.6%), or re-tag words from `diarize` output. Draft upstream issue: `07-upstream-longfile-tags-bug.md`.
2. **Parakeet TDT v3 silently drops spans, with no error:** 45 s at 17:38–18:24 plus two ~10 s spans in the 77-minute recording. A 2-minute cut also dropped the same span, so this is model-side and context-dependent, not a long-file issue (§8). Don't use Parakeet as the only ASR.
3. **Endpointing is off by default** in `serve` and `transcribe`. Without it, the WebSocket gives one final at commit only.
4. **`diarize --offline` is worse** than streaming (DER 43% vs 24%) and limited to about 6.6 min.
5. **No max-speakers control, and 8 labels per stream for the whole session.** The 77-minute talk produced all 8 labels (Zoom shows 6 named sources). In a 2-hour concatenation of two different meetings, the second meeting's speakers were **merged into labels already used by the first**. Labels are not unique people once more than 8 have appeared.
6. **Confidence is always 1.0.**
7. **Implicit downloads** when a model is omitted or a short name is used. Always pass paths.
8. **WebSocket labels are never revised.** Treat them as live previews; the post-meeting file pass is canonical.
9. **AMI download:** Windows schannel fails the revocation check against groups.inf.ed.ac.uk. Use `curl --ssl-revoke-best-effort`.
10. **Per-process VRAM is not visible on WDDM.** Measure with total `nvidia-smi` deltas.

## 7. Nemotron 3.5 as the default model: test results (15 Oct 2026)

**Model:** `nemo-speech pull nemotron-3.5` → `nvidia\nemotron-3.5-asr-streaming-0.6b\1c8deaecc64b91f034d73e08dd8b64625eb3395d\nemotron-3.5-asr-streaming-0.6b.q8_0.gguf`, 741,548,352 B, SHA-256 `a5c435f2…29ae`. Index licence line: `"license": "NVIDIA Open Model License (OpenMDW 1.1)"`. The pull took 86 s.

All file runs below use `--stream --endpointing --diarize --diar-model <Nemotron-3-Diarization>`. No `--language` was passed, so 3.5 auto-detects.

| Metric | Nemotron 3.5 | Nemotron EN | Parakeet TDT v3 (no diarize) |
|---|---|---|---|
| AMI 5 min: words | 464 | 531 | 510 |
| AMI 5 min: words carrying punctuation | 5 | 1 | 98 |
| AMI 5 min: tag agreement with `diarize_5min.json` | 97.4% | 97.9% | – |
| CIMC 77 min: wall / × real time | 181 s / 26× | 171 s / 27× | 54 s / 85× |
| CIMC: peak VRAM delta | **1.82 GB** | 2.58 GB | 4.64 GB |
| CIMC: words / punctuated % / um+uh | 11,896 / 2.5% / 180 | 12,266 / 0.2% / 302 | 11,758 / 12.4% / 315 |
| CIMC: speakers / tag agreement with `diarize` | 8 / 99.9% | 8 / 99.9% | – |
| CIMC: words in 17:38–18:24 | **143** | 193 | **0** (dropped) |
| CIMC: empty 60 s buckets where EN > 20 words | **none** | – | 3 spans |
| `languages` field | `["en-US"]` (auto) | `[]` | `[]` |

Excerpt, AMI 125–145 s:

| Nemotron 3.5 | Nemotron EN | Parakeet TDT v3 |
|---|---|---|
| to the design I'm not really sure what what you guys have already received in your emails what did you get um I just got the project announcement designing remote control yeah that's that where everybody got okay so we're | to the design um I'm not really sure what what you guys have already received um in your emails what did you get um I just got the project announcement but project designing a remote control yeah that is that what everybody got okay um so we're gonna | the design. Um I'm not really sure what what you guys have already received. Um in your emails. What did you get? Um I just got the project announcement, but the project is out designing remote control. That's about it. Is that what everybody got? Okay. Um so we're gonna have |

**Live:** `serve --asr-model <3.5> --diar-model <diar> --asr.endpointing.enable=true --asr.batching.state_arena_slots 2`, streaming 60 s from AMI 0:50.

| Metric | 3.5, `language:"en"` | 3.5, `language:"auto"` | EN (earlier) |
|---|---|---|---|
| `language` field | **accepted** (echoed in `session.updated`) | **accepted** | – |
| First text delta after the first word's onset | 0.18 s | 0.10 s (but the first word arrives 1.3 s later) | ~0.2 s |
| Finals without commit / words / speakers | 17 / 114 / 1–4 | 15 / 99 / 1–4 | 20 / 177 / 1–4 (80 s) |
| Final latency after the last word | 0.86 s | 0.80 s | 0.84 s |
| Lag (max) | 0.07 s | 0.10 s | 0.09 s |
| Server VRAM, both models loaded | ~1.05 GB idle; 1.26 GB after sessions; transient +0.5–0.6 GB per session | | 1.18 GB idle, +50–70 MiB per session |

**Recommendation: one model, Nemotron 3.5, for both live and file mode. Keep `nemotron-en` installed as a fallback.** Deciding fact: on the 77-minute file, 3.5 had no dropped spans and tagged speakers exactly as well as EN (99.9%), at the same speed with 30% less peak VRAM. It also auto-detects language and adds some punctuation.

Caveats:
- 3.5 drops fillers (about 40% fewer um/uh), giving 3–13% fewer words.
- `auto` lost about 1 s of the first utterance; pass the user's language when it is known.
- Live per-session VRAM spikes are larger than EN's (+0.6 GB transient), so budget about 2 GB for live.

## 8. Parakeet drops: chunked test (15 Oct 2026)

Parakeet TDT v3 `transcribe --json` (no diarize, explicit path) was run on cuts of the 77-minute CIMC 16 kHz WAV. Times are absolute seconds in the original file.

| Piece | Words | Nemotron EN, same range | Single 77-min run, same range | Words in the dropped spans |
|---|---|---|---|---|
| 0–600 | 1677 | 1700 | 1673 | – |
| 600–1200 | 1714 | 1826 | 1633 | 1058–1104: **105** (single run: 0) |
| 1200–1800 | 1739 | 1808 | 1689 | 1570–1580: **30** (single run: 0) |
| 1800–2400 | 1650 | 1668 | 1614 | 1950–1960: **34** (single run: ~0) |
| 2400–3000 | 1473 | 1476 | 1421 | – |
| 3000–3600 | 1491 | 1526 | 1496 | – |
| 3600–4200 | 1362 | 1382 | 1371 | – |
| 4200–4622 | 848 | 880 | 861 | – |
| **Sum of 10-min pieces** | **11,954** | 12,266 | 11,758 | No empty 60 s buckets |
| 0–1500 (25 min, one run) | 4259 | – | – | 1058–1104: 105. No empty buckets |
| **1020–1140 (17:00–19:00, 2 min)** | 237 | – | – | **1058–1104: 2 words.** Gap 1057.7→1103.4 s, the same span as the full run |
| 1050–1110 (60 s) | 192 | – | – | 146 words in the span |

The 2-minute piece is far below the ~6.6 min positional limit, so the runtime runs one full-attention pass with no internal splitting. It still drops exactly the same 46 s. A 60 s cut and the 10- and 25-minute cuts keep the span. The drop therefore depends on the audio context the model sees, not on the runtime's long-file splitter. This matches the open transcribe.cpp issue #71 (Parakeet TDT v3 offline silently drops a multi-second span) in a different runtime.

**Conclusion: (b) the drop is model-side, so Parakeet TDT v3 stays experimental.** Chunking at 10 minutes happened to recover all three spans here, but a 2-minute chunk reproduced the drop, so no chunk length is safe. Nothing is added to the upstream draft 07, because this is not a NeMo-Speech.cpp runtime bug.
