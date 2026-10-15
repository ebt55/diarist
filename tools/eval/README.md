# tools/eval

Dev-only evaluation harness (Phase 4, `docs/plans/05-phase4-eval-harness.md`). It runs the app's own `diarist-cli transcribe` over a fixed set of the user's Zoom meetings plus AMI ES2002a, scores the result against references and writes a table to `docs/eval/<yyyy-mm-dd>-<config>.md`. Nothing here ships with the app, and none of the user's recordings or transcripts are ever copied into the repo.

| File | What it does |
|---|---|
| `run.py` | Converts audio, runs diarist-cli per session and config, caches the run, scores it, writes the report |
| `metrics.py` | DER (collar 0.25 and 0), JER, speaker-count error, WDER (overall and near speaker changes), cpWER, tcpWER, DI-cpWER, ORC-WER. Definitions are in its docstring |
| `zoom_vtt_to_ref.py` | Zoom `*.transcript.vtt` to reference SegLST (JSONL), RTTM and a label-to-speaker-id sidecar |
| `ami_fetch.py` | AMI ES2002a WAV, RTTM and word-level manual annotation (downloads only what is missing) |
| `configs.toml` | The configurations (`nemotron35`, `nemotron_en`, `nemotron35_verbatim`) |
| `sessions.example.toml` | Template for `sessions.toml` (gitignored, holds the private paths) |
| `test_metrics.py` | Synthetic sanity checks for the metrics (no GPU, no private data) |

## Setup

1. **uv** 0.13 or newer (`uv self update`). uv fetches Python 3.12 itself.
2. **meeteval needs C++20 on MSVC.** It ships only as source and its C++ uses designated initialisers, which MSVC rejects in its default mode. Pass the flag through MSVC's `CL` variable for the first sync (VS Build Tools 2022 with the C++ workload must be installed). From PowerShell:

   ```powershell
   cd tools\eval
   $env:CL = '/std:c++20'; uv sync
   ```

   From Git Bash, stop MSYS from rewriting the flag as a path: `MSYS2_ARG_CONV_EXCL='*' CL=/std:c++20 uv sync`. Later syncs reuse the cached wheel.
3. **diarist-cli**: build it once and copy it next to the harness, so a workspace rebuild by someone else does not change the binary mid-run:

   ```sh
   cargo build --release -p diarist-cli
   mkdir -p tools/eval/.bin && cp target/release/diarist-cli.exe tools/eval/.bin/
   ```

   The report records the binary's SHA-256.
4. **Engine and models**: by default the repo's `engine\nemo-speech\nemo-speech-0.2.0-windows-x86_64-cuda` and `models\` (passed as `--engine-dir` / `--models-dir`). Override with `DIARIST_EVAL_ENGINE_DIR`, `DIARIST_EVAL_MODELS_DIR`, and `DIARIST_EVAL_CLI` for the binary.
5. **ffmpeg / ffprobe** on PATH (dev machine only; the app does not use them).
6. `cp sessions.example.toml sessions.toml` and fill in the paths; then `uv run python -I ami_fetch.py`.

Run every script with `python -I` (isolated mode): they read downloaded data, and `-I` keeps the data folders and environment out of the import path.

## Running

```sh
cd tools/eval
uv run python -I run.py --config nemotron35 --sessions aug2,aug9,seplog,ami
uv run python -I run.py --config nemotron35 --sessions all
uv run python -I run.py --config nemotron35 --sessions all --metrics-only   # never touches the GPU
uv run python -I run.py --config nemotron35 --sessions all --force          # re-transcribe
uv run python -I test_metrics.py
```

Per session and config, run.py:

1. converts the audio once to 16 kHz mono PCM WAV with ffmpeg (`.work/wav/<session>.wav`; a WAV that already is 16 kHz mono is used as is);
2. selects the config's ASR and diarizer in its own scratch data root (`.work/data/<config>`, SQLite DB plus audio) and runs `diarist-cli transcribe <wav> --language <lang> --rttm --format json --out ...`;
3. exports the transcript RTTM (`diarist-cli export --format rttm`) and copies the standalone diarizer RTTM that `--rttm` keeps, all under `.work/runs/<config>/<session>/` with a `run.json` (wall time, engine version, binary hash);
4. scores against the reference and writes `metrics.json` there, then the report.

A finished run is cached by (session, config), so re-running only recomputes the metrics. Delete `.work/runs/<config>/<session>` (or pass `--force`) to transcribe again.

Report naming: sessions appear under their `label` (docs/05 shorthand such as "Aug 2", "Sep 5"); tables hold numbers only.

## Configs

| Config | ASR | Notes |
|---|---|---|
| `nemotron35` | nemotron-3.5 | default; `--language en` |
| `nemotron_en` | nemotron-en | fallback EN model, selected with `diarist-cli models select asr nemotron-en` in its own data root |
| `nemotron35_verbatim` | nemotron-3.5 | listed but skipped: diarist-cli cannot pass `--verbatim` to nemo-speech |

To add a config, add a table to `configs.toml` with `asr`, `diarizer` (manifest names from `manifests/models.json`), `language` and a `description`.

## Adding a session

1. Add a table to `sessions.toml`: an id (`[sessions.sep4]`), a `label` in the docs/05 shorthand, `kind = "zoom"`, a `group` (`tuning`, `heldout`, `control`, `overlap`, `stress`) and the absolute `audio` (`*.m4a`) and `vtt` (`*.transcript.vtt`) paths.
2. Check the reference converts cleanly (counts only are printed; names stay in the sidecar under `.work/refs`):

   ```sh
   uv run python -I zoom_vtt_to_ref.py "<path>\<recording>.transcript.vtt" --session-id sep4 --out-dir .work/refs
   ```

3. `uv run python -I run.py --config nemotron35 --sessions sep4`.

Never put recording paths, participant names or transcript text in committed files; `sessions.toml`, `.work/` and `data/` are gitignored.

## Hand-correcting a reference

Zoom references are cue-level: speaker identity per cue is near ground truth, but word times inside a cue are interpolated and overlap is serialised. To make a word-accurate reference from the app's own output:

1. Run the session once, then export the app's transcript as SegLST:

   ```sh
   uv run python -I run.py --config nemotron35 --export-for-correction aug2
   ```

   This writes `data/eval/refs/aug2/draft.seglst.jsonl`, one segment per line: `{"session_id", "speaker", "start_time", "end_time", "words"}`.
2. Edit it in a text editor: give every segment the right speaker (any consistent ids, such as `S1`, `S2`), split segments at speaker changes, fix boundaries and words. Keep one JSON object per line.
3. Save it as `data/eval/refs/aug2/ref.seglst.jsonl`. Optionally add `ref.rttm` (otherwise the RTTM is derived from the segments).

A `ref.seglst.jsonl` under `data/eval/refs/<session>/` takes precedence over the Zoom reference; the report's "Ref source" column then says `hand-corrected`.

## WSL fallback

Not needed on the reference machine (meeteval builds natively with the `CL` flag above) and not exercised there, but if meeteval or pyannote.metrics ever fails to install on Windows, score under WSL Ubuntu with a separate environment. meeteval is source-only, so WSL needs a C++ compiler (`sudo apt install build-essential`; the reference machine's Ubuntu has uv and Python 3.12 but no compiler). Transcription stays on Windows (it needs the CUDA runtime and diarist-cli.exe); WSL only recomputes metrics from the cached runs. Windows paths in `sessions.toml` are translated to `/mnt/c/...` automatically.

```sh
# Windows: transcribe and cache
uv run python -I run.py --config nemotron35 --sessions all --transcribe-only
# WSL Ubuntu
sudo apt install build-essential
curl -LsSf https://astral.sh/uv/install.sh | sh     # if uv is missing
cd /mnt/c/Users/<you>/claude-ground/diarist/tools/eval
UV_PROJECT_ENVIRONMENT=.venv-wsl uv run python -I run.py --config nemotron35 --sessions all --metrics-only
```

The report header then still reads the run metadata written on Windows.
