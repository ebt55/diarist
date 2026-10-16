# 0.1.0-beta.2: accuracy, languages, icon, crash capture

Status: proposed 16 Oct 2026, planner Fable 5.1, implementation Opus 5.5 agents. Replaces the "next" order in the handoff for one release: the items here are what a stranger installing Diarist from the website hits in the first ten minutes, and what the site can claim with numbers behind it. The rest of the hardening plan (`docs/plans/11-hardening.md`: bench, soak, CLI in the installer) follows in beta.3.

Every quality-affecting chunk ships only with a before-and-after table from `tools/eval` on the fixed set (`docs/05-eval-data.md`); nothing regresses any session beyond the noise band and nothing changes the live path without a latency number.

## 1. Findings that set the plan (16 Oct 2026)

The installed runtime (NeMo-Speech.cpp 0.2.0, `nemo-speech help transcribe` and `share/doc/nemo-speech/docs/asr/configuration.md`) exposes three accuracy levers Diarist does not use, and the app lets a user choose a language the selected model cannot transcribe.

| Lever | Evidence | Expected effect | Cost |
|---|---|---|---|
| Decoder lookahead for file jobs: `--asr.streaming.rnnt_right_context N` (default `1`, the low-latency preset; `-1` = model maximum, 1.12 s) | The Nemotron 3.5 model card gives FLEURS WER at 80 ms to 1.12 s lookahead: English 9.43 to 7.91, German 9.81 to 8.31, Spanish 4.87 to 4.11, French 10.97 to 9.03. The runtime default of 1 sits near the 160 ms column | About 1 WER point on English and up to 1.5 on other languages, for every import and every post-pass after Stop, where latency does not matter. Live keeps the low-latency setting | One flag in `NemoCliEngine::args`, an eval run, a VRAM and speed check |
| Diarizer context for file jobs: `--diar-rc`, `--diar-lc`, `--diar-chunk` (`asr.diar.*`, model default) | docs/04: "counting accuracy drops at very low latency"; the file pass uses the live preset | Possibly fewer of the 7 to 9 percent wrong words within a second of a speaker change (the headline weakness) | Flags only, an eval run |
| Punctuation and casing: `--pnc-model <gguf>` (`asr.postproc.pnc_model_path`), a BERT token classifier run on the final transcript inside the runtime | The hook is compiled in; NVIDIA ships no PnC GGUF in the model index. The docs convert a NeMo punctuation-and-capitalization checkpoint with the runtime's `convert_model.py` | Transcripts that read like text instead of a word stream. The most visible quality difference against Whisper-based tools | A spike: convert an English checkpoint, confirm the words JSON keeps punctuation per word, measure. Multilingual punctuation is a later step (section 5) |
| Word boosting: `--speech-context PHRASE` (repeatable) and `speech_contexts` in the WebSocket `session.update`; cache-aware RNNT supports it, typical boost 2 to 3, clamp 5 | Runtime docs "Word boosting". Needs the GGUF-embedded tokenizer; the Nemotron 3.5 repository re-converted its GGUF with the tokenizer embedded (revision `ea30d66`, after the pinned `1c8deae`) | Names and jargon recognised: the words users notice first. Speaker names already in the database can be boosted automatically | A spike (does our pinned GGUF boost at all), then a "Names and terms" field per meeting and in Settings |
| Languages | `apps/desktop/src/lib/languages.ts` offers 10 languages plus auto; the default ASR model is English-only; `select_engine` passes `--language` only to multilingual models and nothing warns. A German meeting on the default model produces English nonsense | Non-English users fail silently on their first try | Model-aware language list and an automatic switch to Nemotron 3.5 with a one-click install when it is missing |

Nemotron 3.5 quality by language (model card, FLEURS WER at the 1.12 s setting, language given; CER for ja, ko, zh):

| Tier | Languages | WER |
|---|---|---|
| Transcription-ready (19 locales) | es, it, pt, hi, ko, en, de, fr, ru, tr, vi, nl, ja, ar, uk | 4.1 to 13.1 |
| Broad coverage (13) | pl, nb, fi, zh, cs, bg, sk, sv, hr, ro, et, da, hu | 15.2 to 28.7 |
| Adaptation-ready (8), not offered | el, lt, lv, mt, sl, he, th, nn | needs fine-tuning |

Diarization is language-independent (Nemotron 3 Diarization). Automatic detection costs about a second of the first utterance (docs/04) and 0.1 to 3 WER points against a given language, most for hi, ru, uk, pl, bg, hr, ro.

Upstream: no runtime release after 0.2.0 (nightly of 12 Oct; no ASR changes on `main` since the release); issues #48, #49, #55 and #67 all open. Stay on 0.2.0 for beta.2.

## 2. What ships in beta.2

| # | Chunk | Why it is in a release meant for strangers | Plan |
|---|---|---|---|
| A | Accuracy measurement spike (no product code) | Every number below is a claim on the website | section 3 |
| B | Icon v2 | The taskbar icon is faint on dark taskbars (docs/plans/09, follow-up) | section 3 |
| C | Languages: model-aware list, automatic model switch, docs | "40+ languages" must not fail silently | section 3 |
| D | Local crash capture (hardening 8a) | A crash on a PC we have never seen must leave a report the user can paste | docs/plans/11-hardening.md 8a, unchanged |
| E | Lookahead default for file jobs and the post-pass, if A shows a gain | Free accuracy on every import | section 4 |
| F | Punctuation for English through the runtime's PnC hook, if A's conversion works and the alignment holds | Readability | section 4 |
| G | Names and terms (word boosting), if A shows our GGUF boosts | Names right | section 4; may slip to beta.3 |
| H | Ids: `AUTOINCREMENT` migration and a sweep of cached ids; the small UI findings; the uninstaller's stray folder | The live-view bug's cause can bite elsewhere (hardening plan, last section) | section 4 |
| I | Release verification: Vulkan and CPU packages on the development GPU, updater from beta.1, eval table, CHANGELOG, KNOWN-ISSUES, benchmarks.json | First update ever delivered; first claim about non-NVIDIA GPUs | section 4 |

Not in beta.2 (beta.3 and later): fusion v1.5 (Viterbi), echo without headphones, speaker identity, `diarist-cli` in the installer (8f), bench (8b), soak (8c), code signing (Ebin), Linux, CJK search segmentation (noted in KNOWN-ISSUES).

Already on `main` and unreleased: the live view opening for every recording (d637410), listed under Unreleased in the CHANGELOG.

## 3. Wave 1 briefs (run in parallel; disjoint files)

### A. Accuracy measurement spike

Goal: numbers for the four levers in section 1 on the fixed eval set, plus a per-language smoke test, written to `docs/research/12-accuracy-levers.md` and `docs/eval/2026-10-16-*.md`. No product code.

Files: `tools/eval/**` (new configs, `run.py` support for per-config engine arguments through `diarist-cli transcribe --engine-arg=...`, a FLEURS fetcher), `docs/research/12-accuracy-levers.md`, `docs/eval/2026-10-16-*.md`. Must not touch `crates/`, `apps/`, `manifests/`.

Environment: the private session list and caches live in the main checkout (`C:\Users\ebin\claude-ground\meet-assist\tools\eval\sessions.toml`, `.work\`, `.bin\diarist-cli.exe`), engine and models in that checkout's `engine\` and `models\`. Copy `sessions.toml` and the `.work\wav`, `.work\refs` and `.work\data` caches into this worktree's `tools/eval/`; never commit any of them (gitignored). Point `DIARIST_EVAL_ENGINE_DIR`, `DIARIST_EVAL_MODELS_DIR` and `DIARIST_EVAL_CLI` at the main checkout's folders. Use a fast subset for sweeps (AMI, Aug 2, Sep 5) and the full set for the final candidate configuration.

Measurements, in this order:

1. **Lookahead.** Configs `en_rc_max` and `n35_rc_max` (`--asr.streaming.rnnt_right_context -1`) against the baselines, plus `n35_rc_6` if `-1` costs speed. Report cpWER on AMI, cpWER and WDER on the Zoom sessions, real-time factor, peak VRAM (`nvidia-smi` deltas), and whether word times still run late (compare against the baseline run's word starts).
2. **Diarizer context.** On the best lookahead config, sweep `--diar-rc` (model default, 2x, 4x) and report WDER near speaker changes and DER; stop if the default is best.
3. **Punctuation.** Obtain an English NeMo punctuation-and-capitalization checkpoint (NGC `punctuation_en_distilbert` or `punctuation_en_bert`; record its licence verbatim), convert it with the runtime's `convert_model.py` from the NeMo-Speech.cpp repository at tag v0.2.0 (Python only under `tools/`, never shipped), run `transcribe ... --pnc-model` on the AMI 5-minute clip and one Zoom session with both ASR models, and answer: does `words[].word` carry the punctuation and casing (so segments and exports pick it up with no re-alignment)? Does cpWER move (the normaliser strips punctuation, so it should not)? What does it cost in time and VRAM? Does it double-punctuate what Nemotron 3.5 already punctuates? Keep the GGUF in `tools/eval/.work/pnc/` with its SHA-256.
4. **Boosting.** With the pinned Nemotron 3.5 and EN GGUFs, run the AMI clip with `--speech-context` for three names that the baseline gets wrong (find them by diffing against the reference) at boost 2, 3 and 5; report whether the words change, and whether the runtime complains about a missing tokenizer. If it does, pull the current revision of the model repository into `.work/` and repeat; report the new file's size and SHA-256.
5. **Languages.** A `fleurs_fetch.py` that downloads up to 10 minutes of FLEURS test audio per language (CC-BY-4.0, the Hugging Face `google/fleurs` parquet files) for de, es, fr, it, pt, hi, ja and en, and a WER-only mode in `run.py` (no diarization reference). Report WER per language with Nemotron 3.5 at the default lookahead and at `-1`, language given and `auto`, next to the model card's numbers. English through the EN model as the control.

Acceptance: the research note has one table per measurement with the exact command lines, and ends with a recommendation per lever (adopt, adopt behind a setting, drop) that cites the numbers. Verification: `uv run python -I test_metrics.py` still passes; every new config runs through `run.py` without edits to the Rust tree.

### B. Icon v2

Goal: an app icon that holds its own on a dark Windows taskbar, per the follow-up in `docs/plans/09-branding-and-rename.md`.

Files: `branding/explorations/v2-*.svg`, `branding/explorations/taskbar-sheet-*.png`, then (after the pick) `branding/logo.svg`, `branding/logo-mono.svg`, `branding/tray-recording.svg`, `branding/preview.png`, `branding/README.md`, `apps/desktop/src-tauri/icons/*`, `docs/plans/09-branding-and-rename.md` (outcome). Must not touch the wordmark, social card or the site.

Steps: draw the three candidates from the plan (paper tile; brand-blue tile; ink tile with a lighter edge and thicker lines) on the same 32-unit grid as the current mark. Rasterise each with `resvg` at 24, 32 and 48 px. Capture the real taskbar of this machine in dark and light mode (PowerShell, `System.Drawing` `CopyFromScreen` of the taskbar strip) and compose a contact sheet per candidate with the icon placed between Explorer, a browser and VS Code at the three sizes. Also render each candidate at 16 px, 8x zoom, to keep the small-size test from the first round. Stop and report with the sheets; the planner picks (the owner can veto). Then regenerate the Tauri icon set with `bunx tauri icon`, update the mono and tray marks and the README's colour table and "do not" list where the choice revises them, and record the outcome in docs/plans/09.

Acceptance: the chosen icon's tile clears 3:1 against `#202020` (a dark taskbar) and 3:1 against `#f3f3f3` (a light one); the three lines stay legible at 16 px; the contact sheets are committed under `branding/explorations/`.

### C. Languages

Goal: a user who picks a language gets a model that can transcribe it, or one click to install it; the list only offers what works, with the quality tier visible.

Files: `apps/desktop/src/lib/languages.ts`, `apps/desktop/src/components/RecordSheet.svelte`, the import entry points (`App.svelte` and whichever component starts an import; find them), `Settings.svelte` (default language, model help text), `apps/desktop/src-tauri/src/commands.rs` and `live.rs` (engine selection by language), `crates/core/src/models/mod.rs` (a `select_for_language` helper next to `select_import_engine` and `low_vram_asr`, plus tests), `crates/core/src/settings.rs` only if a key is needed, `docs/04-model-guide.md` section 6, `docs/KNOWN-ISSUES.md` (a row for CJK search). Must not touch `crates/core/src/engine/**`, `tools/eval/**`, `branding/**`, Diagnostics, `lib.rs`.

Behaviour:

- The language list is built from a static tier table (section 1) intersected with the selected ASR model's `capabilities.languages` (the GGUF header lists them; `classify.rs`). Transcription-ready languages are listed plainly; broad-coverage ones carry "(reduced accuracy)"; adaptation-ready ones are not listed. `auto` stays, with its one-line note about the first second. A single-language model lists only its language plus "Other languages: install Nemotron 3.5".
- Choosing a language the selected model does not cover: if Nemotron 3.5 is installed, the job (import or recording) runs on it automatically, the stored selection unchanged, with the same notice pattern as the low-VRAM switch; if it is not installed, the sheet shows "Install Nemotron 3.5 (742 MB)" with the existing download path, and the job does not start on the wrong model. Live: the server is started with the model that matches the language of the recording about to start; if a server with another model is running idle, restart it.
- Settings: the model help text says which model covers which languages, with the tier table in docs/04 section 6 as the source.
- A `diarist-cli` user passing `--language de` with the EN model selected gets the same switch or a clear error.

Acceptance: unit tests for the selection helper (English on EN model: unchanged; German on EN model with 3.5 installed: switched; German without 3.5: an error naming the model); `cargo test`, `cargo clippy`, `bun run check`, `bun run build` green; a manual run importing a short German clip with the EN model selected ends on Nemotron 3.5 with the notice.

### D. Local crash capture

As written in `docs/plans/11-hardening.md` section 8a. Files: a new `crates/core/src/crash.rs` (or `diagnostics/crash.rs`), `apps/desktop/src-tauri/src/lib.rs` (hook installation), `Diagnostics.svelte` and the notice at launch, `crates/cli/src/main.rs` (hidden `crash` verb), `PRIVACY.md`, README privacy line. Must not touch `commands.rs`, `live.rs`, `models/**`, `apps/desktop/src/lib/languages.ts`, `RecordSheet.svelte`, `Settings.svelte`.

## 4. Wave 2 (after A reports; same warm agents where possible)

- **E. Lookahead default.** `NemoCliEngine::args` adds the measured flag for file jobs and the post-pass; live unchanged. Setting `engine.file_lookahead` (`max` default, `low` to restore) only if VRAM or speed on a 4 GB card argues for it. Eval table in the PR.
- **F. Punctuation (English).** Host the converted PnC GGUF as a release asset of this repository (licence permitting; the licence text travels with it in LICENSES.md), add it to `manifests/models.json` with role `punctuation` (the data model already has the role), optional, recommended for English; `--pnc-model` on file jobs and the post-pass when a punctuation model is selected; the segmenter's sentence-final split already uses punctuation. Live gets it through `serve` only if A measured no latency cost.
- **G. Names and terms.** A "Names and terms" field on the start sheet and the import dialog, a global list in Settings, speaker names from the library offered as defaults; `--speech-context` per phrase at boost 3 for file jobs, `speech_contexts` in `session.update` for live. Manifest bump to the tokenizer-embedded GGUF revision if A needed it (hash, size, eval re-run).
- **H. Ids and polish.** `AUTOINCREMENT` for `meetings` with a rebuild migration; grep the front end for cached meeting ids (window state, pins, retention marks, tray) and clear them on delete; the sidebar brand while recording; the Stop confirmation across entry points; the uninstaller removing `%APPDATA%\app.diarist.desktop`.
- **I. Release verification.** Run the Vulkan and CPU packages on this machine (both run on an NVIDIA GPU too) over the AMI 5-minute clip in file mode and, for Vulkan, one short live session; record speed and any abort in KNOWN-ISSUES. Install beta.1 from the published installer on this machine, build beta.2, publish a draft, and verify "Check for updates" installs it before the release goes public. Full eval table for the shipped configuration; `docs/eval/benchmarks.json` for the site; CHANGELOG; version bump per the handoff section 4.

## 5. Decisions and open questions

- **Punctuation route.** The runtime's own PnC hook (English BERT, converted once) over an app-side ONNX model: no new runtime dependency, works for live and file alike, and the segmenter already understands punctuation. The multilingual follow-up is the 47-language XLM-RoBERTa punctuation model (Apache-2.0, ONNX, 1.1 GB, a pure per-token classifier that preserves words), either converted for the same hook if the runtime's converter accepts its architecture, or run through `ort` in the app; decided after F ships. Contested with Opus; see the review note below when it exists.
- **Language gating.** Automatic switch with a notice (the low-VRAM pattern) rather than a blocking dialog: the product principle is that nobody fights the app.
- **Which languages to verify first.** The spike covers de, es, fr, it, pt, hi, ja. The owner may name others; the tier table says what to expect.
- **Lookahead for live.** Kept at the low-latency setting in beta.2; a 560 ms setting is worth a latency measurement later, since the live bar is 1.5 s and first text today arrives in about 0.2 s.
