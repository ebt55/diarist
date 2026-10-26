# tools/eval

Dev-only evaluation harness (Phase 4, `docs/plans/05-phase4-eval-harness.md`). It runs the app's own `diarist-cli transcribe` over a fixed set of the user's Zoom meetings plus AMI ES2002a, scores the result against references and writes a table to `docs/eval/<yyyy-mm-dd>-<config>.md`. Nothing here ships with the app, and none of the user's recordings or transcripts are ever copied into the repo.

| File | What it does |
|---|---|
| `run.py` | Converts audio, runs diarist-cli per session and config, caches the run, scores it, writes the report |
| `metrics.py` | DER (collar 0.25 and 0), JER, speaker-count error, WDER (overall and near speaker changes), cpWER, tcpWER, DI-cpWER, ORC-WER. Definitions are in its docstring |
| `zoom_vtt_to_ref.py` | Zoom `*.transcript.vtt` to reference SegLST (JSONL), RTTM and a label-to-speaker-id sidecar |
| `ami_fetch.py` | AMI ES2002a WAV, RTTM and word-level manual annotation (downloads only what is missing) |
| `configs.toml` | The configurations: baselines, `--verbatim`, decoder lookahead and diarizer preset variants (per-config `engine_args`) |
| `fleurs_fetch.py` | About 10 minutes of the FLEURS test split per language (CC-BY-4.0) for the WER-only mode |
| `nemo_direct.py` | Runs `nemo-speech transcribe` directly with the app's file-job flags plus extra flags (spikes on runtime options the app does not pass) |
| `levers.py` | Analyses over cached runs: word-start offsets between configs, tag agreement with the standalone diarizer, transcript diffs, session-by-config tables, AMI misses, boosting and FLEURS per-utterance reports, punctuation share |
| `gpu.py` | nvidia-smi sampling: waits for an idle GPU, records peak VRAM deltas |
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
uv run python -I fleurs_fetch.py                                            # FLEURS slices, once
uv run python -I run.py --config nemotron35 --fleurs all                    # WER only, language tag given
uv run python -I run.py --config nemotron35 --fleurs all --fleurs-language auto
uv run python -I run.py --fleurs-report                                     # docs/eval/<date>-fleurs.md
uv run python -I test_metrics.py
```

Before each transcription run.py waits until the GPU holds no more than the idle desktop (`DIARIST_EVAL_GPU_IDLE_MIB`, default 1600 MiB), then samples `nvidia-smi` during the run: `run.json` records the peak VRAM delta of the transcribe step and of the standalone diarize pass, and the wall time of the transcribe step alone (`asr_wall_s`, from engine.log creation to the runtime's words JSON).

Per session and config, run.py:

1. converts the audio once to 16 kHz mono PCM WAV with ffmpeg (`.work/wav/<session>.wav`; a WAV that already is 16 kHz mono is used as is);
2. selects the config's ASR and diarizer in its own scratch data root (`.work/data/<config>`, SQLite DB plus audio) and runs `diarist-cli transcribe <wav> --language <lang> --rttm --format json --out ... [--engine-arg=<flag> ...]`;
3. exports the transcript RTTM (`diarist-cli export --format rttm`) and copies the standalone diarizer RTTM that `--rttm` keeps and the runtime's own words JSON (`engine.words.json`), all under `.work/runs/<config>/<session>/` with a `run.json` (wall times, VRAM, engine version, engine args, binary hash);
4. scores against the reference and writes `metrics.json` there, then the report.

A finished run is cached by (session, config) and is redone when the config's ASR, diarizer, language, engine args or `punctuation` change, so re-running only recomputes the metrics. Delete `.work/runs/<config>/<session>` (or pass `--force`) to transcribe again.

Report naming: sessions appear under their `label` (docs/05 shorthand such as "Aug 2", "Sep 5"); tables hold numbers only.

## Configs

| Config | ASR | Notes |
|---|---|---|
| `nemotron35` | nemotron-3.5 | default; `--language en` |
| `nemotron_en` | nemotron-en | fallback EN model, selected with `diarist-cli models select asr nemotron-en` in its own data root |
| `nemotron35_verbatim` | nemotron-3.5 | `--verbatim`: in nemo-speech 0.2.0 this disables ITN, not filler removal; no ITN grammars are configured, so it changes nothing |
| `en_rc_13`, `en_rc_max` | nemotron-en | decoder lookahead 1.12 s (`--asr.streaming.rnnt_right_context 13`; `-1` resolves to the same) |
| `n35_rc_3`, `n35_rc_6`, `n35_rc_13`, `n35_rc_max` | nemotron-3.5 | lookahead 320 ms, 560 ms, 1.12 s; `-1` resolves to 3 on the pinned GGUF |
| `n35_rc_13_diar_off`, `en_rc_13_diar_off` | both | lookahead 13 and `--asr.diar.preset v3-offline` passed explicitly (the measurements behind 0.1.0-beta.2's defaults) |
| `shipped_n35`, `shipped_en` | both | no `engine_args`: the app's own defaults. With a 0.1.0-beta.2 build (`tools/eval/.bin/diarist-cli.exe`; the earlier build is kept as `diarist-cli-0.1.0-beta.1.exe`) run.json records lookahead 13 and preset v3-offline, and `levers.py diff --a n35_rc_13_diar_off --b shipped_n35 --sessions ami,aug2,aug9,sep5,sep7,seplog,aug14` shows identical words, times and tags |
| `n35_rc_13_diar_off`, `en_rc_13_diar_off` | both | lookahead 1.12 s plus `--asr.diar.preset v3-offline` in the fused pass (21 s diarizer chunks); the standalone `--rttm` pass stays on `v3-streaming` |
| `shipped_en_punct` | nemotron-en | `shipped_en` with `punctuation = true`: the data root runs with `transcript.punctuation = auto` and the manifest's punctuation model selected. Words, times and speakers are unchanged, so cpWER, tcpWER and WDER equal `shipped_en`'s; DER, JER and ORC-WER move because segments end at sentence ends (docs/eval/2026-10-26-punctuation.md section 12) |

To add a config, add a table to `configs.toml` with `asr`, `diarizer` (manifest names from `manifests/models.json`), `language`, a `description` and optionally `engine_args`: a list of flags for `nemo-speech transcribe`, one item per argument (`["--asr.streaming.rnnt_right_context", "13"]`). They reach the transcribe step only, not the standalone diarize pass.

Punctuation is off in every config's data root (run.py writes `transcript.punctuation = off`), so the tables measure the speech model and the diarizer. `punctuation = true` turns it on for one config (`auto`, with the manifest's required punctuation model, or the one named by `punctuation_model`); the model must be in the models folder, else `models select punctuation` fails and the run stops.

## WER-only mode (FLEURS)

`fleurs_fetch.py` streams `data/<config>/audio/test.tar.gz` and `test.tsv` of `google/fleurs` and stops after about 10 minutes per language (de, es, fr, it, pt, hi, ja, en; one recording per sentence, joined with 1.5 s of silence) into `.work/fleurs/<lang>/`. The parquet conversion is not used: each language is a single row group whose audio column is one dictionary page, so any read downloads the whole file. `run.py --fleurs <langs>` transcribes each slice with `--no-diarize` and scores the whole slice: WER with the harness normaliser for en; for the other languages NFKC, casefold and punctuation and symbols removed, with combining marks kept; CER for ja. `--fleurs-language given` passes the tag the runtime honours (`ja-JP`, `es-US`, `pt-BR`; bare `de`, `fr`, `it`, `hi`, `en`), `short` the app's bare code, `auto` detection.

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
