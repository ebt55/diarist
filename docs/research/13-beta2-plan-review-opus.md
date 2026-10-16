# Review of the beta.2 plan (docs/plans/12-release-beta2.md)

Reviewer: Opus 5.5, 16 Oct 2026, on d502b87 with wave 1 in progress in the same worktree. Second opinion for the planner; the verdicts below are mine, the decisions are Fable's.

## 1. Punctuation route

**Verdict: modify.** Keep A's measurement. Do not host a converted NGC GGUF. Make the app-side ONNX model the real punctuation plan.

- Both NGC checkpoints the plan names say "License to use this model is covered by the NGC TERMS OF USE" (NGC catalog pages for `punctuation_en_bert` and `punctuation_en_distilbert`, read 16 Oct 2026). They were also trained partly on Fisher, an LDC corpus. A GGUF converted from them is a derivative, and hosting it fails docs/00 ("a licence that permits redistribution without gating"). F would most likely stop at its own licence check after the spike work is done.
- A GGUF we convert ourselves is a maintenance trap. The converter's tensor and metadata layout is not a stable contract: NVIDIA already re-converted 3.5 to embed the tokenizer. NVIDIA re-pins its own files in `share/nemo-speech/model-index.json` with each runtime release; nobody would re-pin ours. And the PnC model loads inside the single `transcribe` process, so a load failure would fail the whole import.
- The runtime hook has two more limits. First, customization.md says the punctuation gate "preserves punctuation from self-punctuating models or runs a loaded PnC model for plain-text models", so PnC may never run on 3.5 (the low-VRAM English path). Second, an English BERT would run on whatever language a 3.5 `auto` job turns out to be.
- The app-side ONNX route (47-language XLM-RoBERTa, Apache-2.0) is the only one that is licensable, multilingual, independent of the runtime version, costs zero VRAM (it runs on CPU), and works live (a few ms per final). Word times survive by construction if its labels are applied to our existing `words[]` (never re-split into new words). Its cost: `ort` with `load-dynamic`, with onnxruntime.dll downloaded as a component like the runtime (the installer stays under 10 MB); a SentencePiece tokenizer; and a scanner rule for an `.onnx` + `sp.model` folder, because the GGUF-header scanner cannot see it. The runtime hook is the better fit for ComfyUI-style dropping; the ONNX route needs that rule.
- Parakeet dual pass: disagree. It drops speech (research/08 section 8), its 4.6 GB peak rules out the 4 GB tier, it works on files only, and it needs a sequence alignment between two different hypotheses whose times are 0.3 s apart.

**Change.** In F, replace "host the converted GGUF" with plumbing only, or nothing:
- classify the PnC GGUF's architecture as `punctuation`. classify.rs's test currently asserts that `bert` is Unknown, so that assertion changes.
- pass `--pnc-model` only when the job language is English (EN model, or 3.5 with `en` given).
- retry once without it on failure, with a notice.

Make the ONNX route the beta.3 plan: quantise the 47-language model to int8 (roughly 300 MB instead of 1.1 GB) and host it, which Apache-2.0 permits. Apply case upgrades only: never lowercase a letter the ASR capitalised. No live PnC through `serve` in beta.2.

## 2. Lookahead for file jobs and the post-pass

**Verdict: agree. Modify the values and the checks.**

- "-1 = 1.12 s" holds only for the EN model. The pinned 3.5 GGUF header has `asr.encoder.train_right_ctx = 3` (EN: 13), so -1 on 3.5 means 320 ms. A has already caught this (`n35_rc_13` in tools/eval/configs.toml). The runtime accepts 13 on 3.5 without complaint (exit 0, checked). Before the site quotes the model card's 1.12 s numbers, A must show that R=13 output differs from R=3. Sections 1 and E of the plan should name explicit values per model, not -1.
- Diarization tags: the fused pass tags each word by its time. If more lookahead shortens the 0.3 s lateness, words near a speaker change move, which can help or hurt the 7 to 9 percent near-change WDER. Report that column. Also repeat the 77-minute tag-agreement check (99.9 percent, research/08 section 7); the fast subset has no long file.
- Live and post-pass drift apart: live stays at R=1. Carry-over is time-based (store/carry_over.rs: 60 percent overlap share; reassign edits replayed by word midpoint), so a constant shift between the two runs can move the boundary words of an edit made while recording. Report the mean word-start offset between R=1 and R=max on the same file; above about 150 ms, note it.
- VRAM: a chunk grows from 2 to 14 frames. Measure on the 77-minute file, where the 1.82 GB and 2.58 GB peaks came from. Speed should improve (fewer steps).
- `post_pass_mode = on_issues` promotes the live run, so those users lose the gain; the setting's help text should say so.
- The flag is safe to add: `-1` parses as a value, and a misspelt `--asr.*` key exits with code 2 (both checked).

**Change.** Add a stored key with no UI, `engine.file_lookahead` (a number of frames, or `low`). It is one line of code and gives 4 GB users and bug reports an escape hatch. Pass the measured value per model.

## 3. Language gating

**Verdict: agree with the automatic switch. Modify the list and the check for an installed model. Disagree with `auto` as the default.**

- Language codes:
  - 3.5's `asr.rnnt.prompt_dictionary` has no bare `ja`, `zh`, `vi`, `th` or `he`. Bare `es` maps to es-US, `pt` to pt-PT, `en` to en-US.
  - Test on a 15 s English clip: `--language ja`, `zh`, `vi` and `xx` all give the same output as `auto` (`languages: ["en-US"]`, i.e. detection). `de`, `de-DE`, `ja-JP` and `zh-CN` are honoured (26 and 1 words instead of 53).
  - So beta.1's Japanese and Chinese choices silently ran as `auto`, and forcing the wrong language destroys the transcript.
  - C's in-progress `runtime_tag` (models/languages.rs) already maps ja to ja-JP.
  - A's FLEURS run must pass ja-JP and zh-CN, or "language given" measures `auto`.
- How "3.5 installed" is detected: `low_vram_asr` matches `manifest_name`, which is set by a SHA-256 match. A hand-dropped or re-converted 3.5 (G's revision bump) counts as missing, and the switch would offer a 742 MB download of a file the user already has.
- Models with no language list (Parakeet, user GGUFs): no switch.
- Live: start the server for the new language when the sheet's language changes, not at Start. A server takes 2.6 to 5.9 s to become ready and the feed holds 10 s (`FEED_CAPACITY`), so a slow PC could start in record-only mode.
- `auto` as the default: no. `select_import_engine` moves low-VRAM English users onto 3.5, and `auto` would cost them the first second and some WER.
- The real gap is elsewhere: imports have no language choice at all (`import_file(path, title)`, language taken from `default_language` = "en"), and Setup installs the EN model. A German stranger who drags in a file never reaches C's switch.

**Change.**
- Keep C's `ja` to `ja-JP` mapping and its test. List pt-BR and pt-PT separately (bare `pt` means European Portuguese), and the same for es-ES and es-US. Map stored `default_language` values and old meetings' languages when they are used.
- Choose the model by what it can do, not by manifest name: the selected model if it covers the language, else any installed streaming model whose dictionary covers it, else the manifest download.
- For models with no language list, show the full list marked "not checked for this model".
- Seed `default_language` from the Windows display language at first run, and let Setup pick EN or 3.5 from it.
- Give each finished import a "Re-run as <language>" button (live/rerun.rs already exists).

## 4. Word boosting

**Verdict: move G to beta.3; keep A's measurement 4.**

- It is not a first-ten-minutes item, and its UI is large: the start sheet, an import dialog that does not exist, Settings, and live `session.update`.
- If boosting needs the newer GGUF, runtime 0.2.0's model-index.json pins 3.5 at `1c8deae`, and the manifest installs it through `pull_name`. A bump means switching to a URL download, a 742 MB re-download for every 3.5 user, a full eval re-run, and old files losing `manifest_name` (point 3).
- The pinned files already carry `asr.tokenizer.type = sentencepiece_bpe` and a vocabulary (13,087 and 1,024 pieces), but no scores or merges. Whether that is enough is exactly A's question.
- Hazards of feeding library names in automatically:
  - placeholder labels (`spk_N`, "Speaker 2", "me").
  - short or common-word names (Will, Mark, May, Bill, Grace, Pat) pulling in ordinary words ("and a" becomes "Anna").
  - a library that grows without limit and biases unrelated meetings.
  - the CLI's single `--speech-context-boost` for all phrases, so suggested names and typed names cannot get different strengths in file jobs.

**Change (when it ships).**
- Offer library names as opt-in chips, not defaults.
- Prefer full names, skip names of three letters or fewer and names on a common-word list, cap the list at about 30, and boost at 2.
- Eval control: boost 20 names that do not occur in the audio; cpWER must stay inside the noise band.

## 5. Scope for a stranger's first ten minutes

**Verdict: modify.** Four things are missing or would break; three should wait.

- **First update (I) cannot be verified as written.** beta.1 reads `updater/latest.json` on `main` (tauri.conf.json), draft release assets are not public, and updater/README.md says to merge the manifest only after publishing. Instead: publish the pre-release, merge the manifest PR, and immediately run Check for updates on this machine's beta.1; the check is manual only, so the exposure is short. Revert latest.json if it fails. `install_update` refuses only while recording; an import in flight dies with the restart, so refuse while the job queue is busy too.
- **H's migration can wipe every transcript.** It would be the first migration ever (only `0001_init.sql` exists), shipped with the first update ever. Seven tables reference `meetings(id) ON DELETE CASCADE`, and `foreign_keys` is on at open (sqlite/mod.rs:114). `migrate()` wraps each migration in a transaction, where `PRAGMA foreign_keys = OFF` has no effect. A textbook rebuild (create, copy, `DROP TABLE meetings`, rename) would therefore cascade-delete every segment, word, run and speaker. Fix id reuse without a rebuild: a high-water-mark row, written on insert and delete, and explicit ids. Run `backup()` before any migration.
- **First run in another language** (point 3): a Setup language step, plus seeding the default from Windows.
- **First recording with the microphone blocked:** nothing in crates/core/src/audio or the components detects Windows' "Let desktop apps access your microphone" set to off. Add a Diagnostics row that reads the CapabilityAccessManager consent store (`microphone\NonPackaged`), with a button that opens `ms-settings:privacy-microphone`, and a "mic silent" hint on the record sheet.
- I's Vulkan and CPU runs on an NVIDIA card prove nothing about AMD or Intel GPUs. KNOWN-ISSUES and the site should say "Vulkan tested on NVIDIA only".
- Wait: G; F's hosted GGUF; live PnC.

## 6. Four agents in one worktree

**Verdict: the file boundaries overlap in practice.** Real collisions found in the files:

- `crates/cli/src/main.rs` is D's (the `crash` verb), but C's requirement that `diarist-cli --language de` switch models lands there too (`select_engine_with` at main.rs:554).
- `apps/desktop/src/lib/api.ts` and `lib/state.svelte.ts` are on nobody's list:
  - C needs both: `importFile(path, title)` takes no language, and imports start at state.svelte.ts:326.
  - D needs api.ts for the report commands and for WebView error forwarding.
- `apps/desktop/src-tauri/src/lib.rs`: any command C adds must be registered in `generate_handler!` (lib.rs:226 to 278). C may not touch that file, and D is editing it.
- Also shared:
  - `App.svelte`: C's drag-drop handler at :36, and D's launch notice and `onerror` hook.
  - `Overlays.svelte`.
  - `settings.rs`: C "if needed"; D needs somewhere to store the last acknowledged report.
- Build: the `target/` directory, the build lock and `Cargo.lock` are shared, and D's new crates force a rebuild. A half-edited file from one agent breaks the other's `cargo test`, `clippy` and `bun run check`, because each covers the whole crate or the whole front end. Every `cargo test` rewrites `lib/generated` (C also generates `languageTiers.ts`).
- Git: the index is shared. One agent's `git add` plus another's plain `git commit` commits both agents' files.
- GPU: A's VRAM figures are total `nvidia-smi` deltas (research/08 pitfall 10). C's manual German import on the same GPU would spoil them.
- D's crash monitor: minidumper writes dumps from a separate monitor process. If that process is a second `diarist.exe`, it must branch before `tauri_plugin_single_instance` (lib.rs:176); otherwise the plugin hands it to the running app.
- D's dumps hold process memory (transcript text, the WebSocket bearer key), which Copy report's path redaction cannot clean. Keep the text report as the thing users share, and say that .dmp files stay private.

**Change.**
- Run C and D in separate worktrees and merge D first.
- If they must share one: C asks D for its lib.rs and main.rs lines; both only append to api.ts.
- Commits name their paths (`git commit -- <paths>`), never `-A` or `-a`.
- C runs its GPU acceptance test only after A has reported VRAM.

## What I would add or remove

- **Add to A (measurement 2):** replace the `--diar-rc` 2x and 4x sweep (80 to 320 ms, too small to matter) with `--diar-preset v3-offline` in the fused pass. That is the model card's 30.4 s geometry (research/04: cache 264, FIFO 40, chunk 340, right context 40, update 300). Only the stateless `diarize --offline` was ever measured (DER 43, research/03 section D); the v3-offline streaming preset never was. Run it on the 77-minute and 2-hour files as well, since it changes the FIFO and the speaker cache.
- **Add to C:**
  - a Setup language step.
  - `default_language` seeded from Windows.
  - pt-BR/pt-PT and es-ES/es-US entries.
  - model selection by capability rather than `manifest_name`.
  - the languages shown on the import job row, with "Re-run as".
- **Change H:** high-water-mark ids instead of a rebuild, and a backup before any migration.
- **Change I:** the corrected updater order. Add a CHANGELOG line that beta.1 ran Japanese and Chinese as automatic detection.
- **Add to Diagnostics:** the microphone-privacy row.
- **Remove from beta.2:** G (to beta.3), F's hosted GGUF, live PnC.

## Facts I checked

- Plan and context: `docs/plans/12-release-beta2.md`, `docs/00-product-spec.md`, `docs/01-architecture.md` (ASR, punctuation, fusion and model-management rows), `docs/04-model-guide.md`, `docs/KNOWN-ISSUES.md`, `docs/plans/11-hardening.md`, `docs/research/08-runtime-cli-contract.md`, `docs/research/03-runtime-spike.md` section D, `docs/research/04-diarization.md` section 2.
- Runtime docs (read-only) under `engine/nemo-speech/nemo-speech-0.2.0-windows-x86_64-cuda/share/doc/nemo-speech/docs/`:
  - `asr/configuration.md`: the key table and PnC.
  - `asr/customization.md`: the punctuation gate.
  - `asr/models.md`: the V3 diarizer geometry, chunk 13 and right context 1.
- `share/nemo-speech/model-index.json`: 3.5 pinned at `1c8deae`; no PnC entry.
- `nemo-speech help transcribe`: `--pnc-model`, `--no-punctuation`, `--speech-context`, and one global `--speech-context-boost`.
- GGUF headers, read with a scratchpad parser, of `models/nvidia/nemotron-3.5-asr-streaming-0.6b/1c8deae.../*.gguf` and `models/nvidia/nemotron-speech-streaming-en-0.6b/ebe59e5.../*.gguf`:
  - `train_right_ctx` 3 and 13.
  - `asr.tokenizer.type` and `asr.tokenizer.vocab` present.
  - the full 121-entry prompt dictionary.
- Runtime runs on CPU (scratchpad only; only word counts and the `languages` field were printed):
  - `-1` accepted; unknown `--asr.*` key gives exit 2; `rnnt_right_context 13` on 3.5 gives exit 0.
  - language-code behaviour on a 15 s cut of `tools/eval/.work/wav/aug2.wav` (main checkout): en, auto, ja, zh, vi and xx identical (53 words); de and de-DE 26 words; ja-JP and zh-CN 1 word.
- Code:
  - `crates/core/src/engine/nemo_cli.rs`: `args`, and the parse that uses only `words[].word`.
  - `crates/core/src/models/mod.rs`: `select_engine_with`, `low_vram_asr`, `select_import_engine`, `is_multilingual`.
  - `crates/core/src/models/classify.rs`: languages from `asr.rnnt.prompt_dictionary`; `bert` maps to Unknown.
  - `crates/core/src/types.rs`: the `ModelRole::Punctuation` role exists.
  - `crates/core/src/transcript/mod.rs`: sentence split on `ends_sentence` plus 500 ms; utterance boundaries are not used.
  - `crates/core/src/export.rs`: one cue per segment.
  - `crates/core/src/store/carry_over.rs`: time-based.
  - `crates/core/src/settings.rs`: `default_language` = "en".
  - `crates/core/src/audio/feed.rs`: 10 s capacity.
  - `crates/core/src/store/sqlite/mod.rs`: `foreign_keys` on, transactional `migrate()`.
  - `crates/core/src/store/sqlite/migrations/0001_init.sql`: seven cascading foreign keys.
  - `crates/core/src/live/recover.rs`.
  - `crates/cli/src/main.rs:554`.
  - `apps/desktop/src-tauri/src/commands.rs`: `import_file` takes no language; `install_update` checks only `is_recording`.
  - `apps/desktop/src-tauri/src/lib.rs`: single-instance at :176, `generate_handler!` at :226.
  - `apps/desktop/src-tauri/src/live.rs`.
  - `apps/desktop/src-tauri/tauri.conf.json`: updater endpoint on raw `main`.
  - `updater/README.md`.
  - `apps/desktop/src/lib/languages.ts`, `api.ts:65`, `state.svelte.ts:326`.
  - `apps/desktop/src/App.svelte:36`.
  - `apps/desktop/src/components/Setup.svelte`: installs the diarizer and the EN model.
  - `manifests/models.json`: `pull_name` and SHA-256.
- Work in progress in this worktree (`git status`), read only: `crates/core/src/models/languages.rs` (C's `runtime_tag`) and `tools/eval/configs.toml` (A's `n35_rc_13`).
- Licences (web, 16 Oct 2026): catalog.ngc.nvidia.com pages for `nvidia/nemo/punctuation_en_bert` and `punctuation_en_distilbert` (NGC Terms of Use; Tatoeba, Gutenberg and Fisher data); huggingface.co/1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase (Apache-2.0, 47 languages, `model.onnx` and `sp.model`; outputs pre- and post-punctuation, per-character true-casing and sentence boundaries).

## Planner dispositions (Fable 5.1, 16 Oct 2026)

Accepted outright, applied to `docs/plans/12-release-beta2.md` and sent to the running agents: the locale-tag finding (bare `ja`/`zh` ran as `auto` in beta.1; A uses `ja-JP`/`zh-CN`, C maps codes and splits es and pt by locale); explicit lookahead values per model instead of `-1` (3.5 header says 3, EN says 13; A shows R=13 differs on 3.5 before quoting it), with the tag-agreement, word-offset and 77-minute VRAM columns and a hidden `engine.file_lookahead` key; the diarizer measurement switched to `--asr.diar.preset v3-offline`; model choice by capability rather than manifest name; no switch for models without a language list; imports get a language; the Setup language step and the system-language seed; live server restart on sheet language change; `auto` not the default; G (word boosting) to beta.3 with the opt-in and filtering rules recorded for then; the NGC licence finding, so no converted NGC checkpoint is hosted; the migration hazard (high-water-mark ids, backup before any migration, no table rebuild); `install_update` refusing while jobs run; the microphone-privacy Diagnostics row; the corrected updater verification order; "Vulkan tested on NVIDIA only" wording; `.dmp` files never copied by Copy report; the shared-file protocol (append-only, pathspec commits).

Modified: F keeps the runtime hook as plumbing (role `punctuation` from the GGUF header, `--pnc-model` when a punctuation model is selected and the job is English, retry without it on failure) so a user who converts a model can drop it in, and A first tries converting the Apache-2.0 NeMo checkpoints from 1-800-BAD-CODE (pcs47, 47 languages) through the runtime's converter; if that converts, the hosted-model path reopens with a clean licence, else the shipped punctuation model is the beta.3 ONNX route (int8, `ort` with a downloaded runtime, case upgrades only). "Re-run as <language>" ships in C only if `live/rerun.rs` makes it cheap.

Rejected: none. Separate worktrees for C and D were right in principle but both were already running; the append-only and pathspec protocol stands in.
