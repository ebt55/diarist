# 0.1.0-beta.6: missed words, the runtime gate, and a visible recording state

Status: planned 30 Oct 2026 (a Friday) by Fable 5.1 from docs/research/21 (the runtime after PR #87, Nemotron 3.5 Transcribe 30B, code-switching), the beta.5 release check (docs/eval/2026-10-27-beta5-release.md), the public baseline (docs/eval/2026-10-27-baseline-public.md), plan 16 sections 12.4 and 12.7 to 12.9, plan 17 section 10, and the code at `00cc9a6` (0.1.0-beta.5 plus the post-release docs). Implementation by Opus 5.5 agents, one chunk per agent, each behind the checks in section 13. No code, build, GPU run or download happened while this was written. Every measurement rule that decides a chunk is written here before the run, so the orchestrator applies it without coming back to the planner; where a bar is amended after a sweep (chunk M, section 5.1) or after a measurement it was waiting for (chunk R after N2, section 4.4), the amendment and its reason are marked, and it is made before the run it governs. The private notes were read for state only; no meeting detail from them is in this file.

The owner's standing priority (16 Oct 2026): measured accuracy and speed first, features after. Their verdict on beta.5's words: "not mind blowing, but okay". Their asks for beta.6: the plan, the new runtime evaluated, a taskbar "recording" label and a crisper recording indicator. The Settings layout tweak was withdrawn.

## 0. For the owner, in plain words

**What beta.6 goes after.** Missed words. On the public sets with human references, four of every five English errors are words the model never wrote (docs/eval/2026-10-27-baseline-public.md, "Deletions dominate": 46,422 of 56,794 with the EN model). Beta.5 moved the pooled word error from 11.01 to 10.85 by removing bursts; it did not touch deletions. The new NVIDIA runtime does not either: its fix restores speaker accuracy, and its own authors say the transcripts are unchanged (research/21 section 1.3). So beta.6 attacks deletions with what is already on this PC, and takes the runtime only if NVIDIA tags it in time.

**The three accuracy chunks, cheapest first.**

1. **Fill the gaps with the second model (chunk N).** The EN model misses fewer words than Nemotron 3.5 overall but loses them in longer stretches (315 gaps of 5 s or more against 259 on the public English sets), and it drops quiet speech that 3.5 hears (research/12 section 5). Both models are installed on most PCs that use Diarist for English. The measurement costs no GPU at all: every needed transcript is already cached from beta.5's runs. If filling EN's gaps with 3.5's words lowers the error on the 36 recordings and on the public sets by the bars in section 3.3, it ships as a second file pass (about 45 more seconds per hour of audio on the RTX 3080).
2. **Names (spike L).** Half an hour of GPU on the one re-converted EN file that survived, with plan 16's kill rule. On runtime 0.2.0 boosting wrecked transcripts; the expectation is that it still does, but this is the only measurement nobody has run, and names are 1.2 percent of the far-end words.
3. **Your own words over loud far-end speech (chunk M, continued).** The lead from beta.5's sweep missed its bar by one word on one meeting. Section 5 says how it is finished without moving the bar after the fact: one amendment, decided now, that asks you to listen to at most ten words.

**The runtime.** Adopt v0.3.0 only from a tagged release, and only if the tag exists by **Friday 6 Nov 2026**. If it does, about one idle-GPU day of measurement decides (section 4). If it does not, beta.6 ships on 0.2.0 and the runtime opens beta.7. Expected gain from the runtime: speed (about a quarter faster file pass) and the speaker fix; no fewer missed words.

**Also in beta.6.** The recording indicator (option B recommended; section 9), the punctuation model's new upstream pin with its LICENSE and NOTICE (section 8), and housekeeping: the Nemotron 3.5 re-timing, the Tauri bump that clears the `glib` alert, LICENSES.md's crate counts, the disk-size figures (section 10).

**Your decisions** (section 14 has the full table): the recording option (B, or B without the ticking title); whether to spend about three hours of idle CPU on the Transcribe 30B spike and from which file source (recommended: yes, after the runtime decision, from the third-party IQ4_NL files, with the upstream request for expert offload filed now); the 120 s repro zip from PR #46 (recommended: no); a separate repository for future model files (recommended: yes in principle, created at the first file that needs it, which beta.6 as planned has none); the ten-word listening step for chunk M (recommended: yes); the gate date.

## 1. Where beta.5 left us

| Fact | Where |
|---|---|
| Beta.5 shipped 27 Oct 2026 at `eda3998`; pooled cpWER on 36 sessions EN 11.01 → 10.85, 3.5 11.30 → 11.30; no session worse; the speed bar passed for both models, 3.5's −5.4 % as measured being three runs that overlapped the Claude app's GPU use, accepted by the owner with a re-time owed | docs/eval/2026-10-27-beta5-release.md, "Release bars" and "Speed, gate set" |
| Deletions are 82 % of EN's errors and 81 % of 3.5's on the 52 public English sessions (human references); 68 % on the other languages | docs/eval/2026-10-27-baseline-public.md, "What the numbers say", first bullet |
| EN leads 3.5 on English (pooled cpWER 41.6 against 43.3) but leaves more long gaps: 315 gaps of 5 s or more against 259, most of them on NOTSOFAR (74 against 28) and ICSI (111 against 82; Bmr019 44 against 15) | the same note, third bullet |
| On the owner's Zoom sessions: EN 816 gaps of 2 s or more and 61 of 5 s or more; 3.5 877 and 85 | docs/eval/2026-10-27-beta5-release.md, "Pooled", all Zoom rows |
| The gaps are the same stretches with the shipped, fixed and f16 model files, so they are not a quantisation effect; about 29 to 31 per hour of meeting; the next spike should characterise them (level, overlap, whether EN and 3.5 miss the same stretches) | docs/research/19-q8-pointwise-respin.md section 8, last two paragraphs |
| The EN model drops quiet speech: FLEURS English 21.5 % at lookahead 13, nearly all deletions; 36 of 55 recordings peak below −30 dBFS and EN returns nothing or a fraction for eight of them, where 3.5 transcribes all 55 | docs/research/12-accuracy-levers.md section 5, last bullet |
| File lookahead is 13 on both models (the EN model's trained maximum; 3.5's header says 3 but 13 is a real, better decode); live is 6 since beta.5 (first-word median 0.48 to 0.52 s, p90 0.56 to 0.58 s against bars of 1.0 and 1.5 s) | research/12 section 1; docs/eval/2026-10-27-live-lookahead.md, the rule table |
| Word boosting: the pinned GGUFs carry no SentencePiece model, so the runtime disables it; on a re-converted 3.5 file it fixed none of three targets at any level and inserted phrases (clip WER 23.0 → 36.5 at boost 3; 20 absent names: 45.7). Plan 16 dropped it and left a spike brief L with a kill rule, after chunk G, which never shipped | research/12 section 4; plan 16 section 12.4 |
| One re-converted EN file survives on disk: `nemotron-speech-streaming-en-0.6b.q8_0-pwf16.gguf`, 770,986,624 bytes, SHA-256 `3d37c1c7…2a1da`, with its `.meet.json`, under `C:\Users\ebin\claude-ground\meet-assist\.claude\worktrees\diarist-echo-copyright-b63b18\tools\eval\.work\keep-fixed-en\`. It embeds the tokenizer and can boost. The re-converted 3.5 files and the controls were deleted on 27 Oct | `FIXED_FILES.md` in that folder; research/21 section 3.3 is wrong on this point |
| Cached engine words for every session and both models exist: the 36 Zoom and AMI sessions under `…\diarist-echo-copyright-b63b18\tools\eval\.work\runs\rs_en_shipped` and `rs_n35_shipped` (research/19, v0.2.0, shipped files), the 52 public English sessions under `…\beta5-data\tools\eval\.work\runs\shipped_en` and `shipped_n35` | the two worktrees, read on 30 Oct 2026 |
| Runtime `main` (`d8a1abf`, 29 Oct) needs no Diarist code change; PR #87 fixes the speaker half of our #81 and not the deleted-word half (+3.3 % EN, +1.0 % 3.5 deletions on 324 min, which #87 puts down to #46's 70 ms encoder phase); `VERSION` is 0.3.0 with no tag, PR #88 open, no date; the nightly cannot be pinned; the Windows CUDA zip grows from 102,183,465 to 508,605,526 bytes (NVIDIA's cuBLAS) | research/21 summary and sections 1.3 to 1.6 |
| Nemotron 3.5 Transcribe 30B-A4B: OpenMDW-1.1, cpWER about half ours on meeting benchmarks (AMI SDM 17.34 against 30.50 and 35.86; NOTSOFAR 25.91 against 53.44 and 53.96), AMI, ICSI and AliMeeting in its training data; cannot run on a 10 GB GPU in this runtime (no expert offload; smallest documented GPU setup 24 GB); CPU-only about 22 GB of IQ4_NL files, 23 to 24 GB RAM, 30 to 60 min per hour of audio (inference); NVIDIA publishes no GGUF | research/21 sections 2.1, 2.2, 2.4, 2.6, 2.8 |
| Code-switching: of our public sets only Indic DiarBench Hindi mixes English (10.6 % of tokens); AliMeeting 0.77 Latin runs per 1,000 characters; SUMM-RE 0.4 to 0.9 %. NVIDIA has not republished the ASR GGUFs | research/21 sections 3.2 and 3.4 |
| Horizon-Labs added LICENSE (11,358 bytes) and NOTICE (1,567 bytes) and tagged v1.1.1 = `0cea764aa6fc55c8bdfc20a5f0bb4dc27b64753d`; `onnx/model_quantized.onnx` and `tokenizer.json` are byte-identical to the pinned `ee31fdf` | the orchestrator's check through the Hugging Face API, 30 Oct 2026 |
| The manifest's punctuation entry: `revision` `ee31fdf…`, `licence` "Apache-2.0 (declared in the model card; no LICENSE file in the repository at this revision); base model MIT", `files` with `LICENSE-Apache-2.0.txt` (11,358 bytes, SHA-256 `cfc7749b…3d30`, the canonical Apache text) and `NOTICE-punctuation.txt` (2,179 bytes) from the `models-2026-10` release | `manifests/models.json`, entry `punctuation-horizon-small`; `C:\Users\ebin\claude-ground\diarist-releases\models-2026-10\CONTENTS.md` |
| Chunk M (the post-canceller correlation layer): no variant of the fixed rule qualified; one rule outside the list, "spare words within M dB of a known owner level", at M = 6 missed meeting 20's far-active bar by one word (16 against 15) and passed every other bar; the (b) and (b′) fixtures have since been deleted and must be regenerated | docs/eval/2026-10-26-echo-owner-level.md section 8, "Beyond the list"; the private resume note of 28 Oct (state only) |
| The tray's recording icon is painted at run time (`with_red_dot`: a disc of radius 0.27 × size with a white ring, no anti-aliasing); the in-app dot pulses to 35 % opacity; the window title is always "Diarist"; nothing marks the taskbar button | `apps/desktop/src-tauri/src/tray.rs:31-53`; `apps/desktop/src/app.css:286-303`; `apps/desktop/src/components/LiveView.svelte:151`, `Library.svelte:203`; `apps/desktop/src-tauri/tauri.conf.json:16` |
| Tauri 2.12.4 in the lock (`set_overlay_icon` available); `glib` 0.18.5 (Dependabot alert #1, a Linux-only GTK dependency); the `windows` crate at 0.62 with four features, none for the shell | `Cargo.lock:5117`, `:1914`; `apps/desktop/src-tauri/Cargo.toml:28`, `:52-57` |
| `TESTED_ENGINE_VERSIONS = ["0.2.0"]`; `EXPECTED_REALTIME_FACTOR = 26.0`; three 0.2.0 packages in the engine manifest | `crates/core/src/models/manifest.rs:20`; `crates/core/src/engine/nemo_cli.rs:57`; `manifests/engine.json` |
| LICENSES.md's crate table counts 413 crates as of 15 Oct 2026, before `ort` and `tokenizers` | `LICENSES.md:44-45` |
| Plan 17's beta.6 list: the 3.5 re-time (Aug 9, Sep Logistics, Aug 14 against beta.4's 32.4, 32.7 and 54.7 s), chunk M, Process again without a kept live transcript, a runtime release after v0.2.0 behind the full set, LICENSES.md counts | docs/plans/17-release-beta5-checklist.md section 10 |

## 2. Decisions at a glance

| Id | Decision | Where |
|---|---|---|
| D1 | Missed words are attacked with what is on this PC: a gap characterisation and a gap-fill measurement from cached runs (CPU only), then a second file pass that fills the EN model's gaps with Nemotron 3.5's words if the pre-fixed bars hold; the phase control on v0.2.0; the names spike L on the surviving re-converted EN file. Lookahead has nothing left to give; endpointing-off is measured only on 0.3.0 | section 3 |
| D2 | Runtime v0.3.0 is adopted in beta.6 only from a tagged release whose tag exists by Friday 6 Nov 2026, and only if the five measurements of research/21 section 1.7 pass as restated in 4.2 and amended in 4.4 (31 Oct, after N2, before any 0.3.0 run). Otherwise beta.6 ships on 0.2.0 and the runtime is beta.7's first chunk. Never the nightly, never a build of ours | section 4 |
| D3 | Chunk M is in beta.6 as a timeboxed continuation with the rule's form fixed now (spare within M dB of a known owner level, M in {6, 9, 12}), the fixtures regenerated plus one fresh confirmation fixture, and one amendment to plan 16 section 12.7 (iv)'s bars made before any new run: meeting 20's far-active proxy becomes a labelled bar read by the owner from the audio. If the owner declines to label, the old bar stands and M stays out | section 5 |
| D4 | The code-switching measurement is not worth a 3.5 re-conversion now; the design in research/21 section 3.3 waits for NVIDIA's republished GGUF (then it is a manifest bump plus that measurement) or for a user report. The 120 s repro zip is not downloaded | section 6 |
| D5 | The Transcribe 30B spike is the owner's call; recommended in its cheapest form after the runtime decision, on an idle machine, with pre-fixed bars; nothing from it ships in beta.6; the upstream request for MoE expert offload is filed now | section 7 |
| D6 | The punctuation pin moves to v1.1.1 (`0cea764`): same model and tokenizer hashes, new URLs; the authors' LICENSE and NOTICE are fetched from upstream and installed under their own names; Diarist's `NOTICE-punctuation.txt` stays as the dated provenance record; `LICENSE-Apache-2.0.txt` leaves the file list once upstream's LICENSE is verified byte-identical; `models-2026-10` is never changed, only given two more non-manifest snapshot assets; the manifest licence string is restated | section 8 |
| D7 | The recording indicator ships as the option the owner picks; B is recommended, with the title tick as a part the owner can drop; UI only, early, no GPU | section 9 |
| D8 | Housekeeping: the 3.5 re-time under a pre-fixed bar, the Tauri bump that clears `glib`, LICENSES.md counts, the disk-size figures. Process again without a kept live run waits for beta.7 | section 10 |
| D9 | Release shape: freeze when V, Q, U and the measured chunks that pass have merged, about one week; R adds about one idle-GPU day if the tag lands; a plan 19 checklist on plan 17's pattern | section 12 |

## 3. D1: missed words with what we have

### 3.1 What can and cannot move deletions in this release

| Lever | What we know | Verdict for beta.6 |
|---|---|---|
| Decoder lookahead | File pass at 13 on both models (research/12 section 1: AMI cpWER 19.8 → 18.3 EN, 23.0 → 20.8 3.5; faster, not slower); EN's trained maximum is 13; live at 6 passed its rule with a 0.52 s median. Live at 13 would add 7 frames of 80 ms, about 1.08 s median against the 1.0 s bar | Nothing left. Not measured again |
| Endpointing off for file jobs | Expected identical words (research/17 section 7, spike 2: 0.2.0 keeps decoder state across EOU); fewer spurious finals from issue #40. On v0.2.0 it is unsafe for diarized files over 20 minutes (issue #67, docs/KNOWN-ISSUES.md:9) | Measured once inside chunk R on 0.3.0 (where #70 makes it safe), gate set, report only. No change in beta.6 |
| The 70 ms phase of #46 | #87 says main with its first 70 ms dropped reproduces v0.2.0's WER on AMI (research/21 section 1.3). If v0.2.0's own deletions move by about ±3 % with a few tens of ms of leading offset, main's +3.3 % is chunk-phase noise, and the noise band itself is a lever to test later (two phases, union of the words in gaps) | Measured now on v0.2.0 (chunk N2, about 30 minutes of GPU). It is needed to judge 0.3.0 and it stands on its own |
| Filling EN's gaps from Nemotron 3.5 | EN leaves longer gaps than 3.5 and drops quiet speech 3.5 hears (section 1). Whether 3.5 has words where EN has none, and whether they are right, is unmeasured; the words to measure it are cached | **The main chunk (N).** Measurement first, CPU only; code only if the rule in 3.3 passes |
| Word boosting for names | Dropped on 0.2.0 (research/12 section 4); nothing on `main` changes RNNT boosting (research/17 section 7, "not worth a spike now"); the surviving re-converted EN file can boost | Spike L, half an hour of GPU, the kill rule of plan 16 section 12.4 unchanged. No name source in beta.6 even if it passes (3.4) |
| Input gain for quiet speakers | EN's FLEURS deletions sit on recordings peaking below −30 dBFS (research/12 section 5). Unmeasured whether a normalised input changes that | One report-only run inside N2 (about 6 minutes of GPU). Code, if any, is beta.7's: a gain stage would touch the echo gate's level features |
| Runtime 0.3.0 | No transcript change expected on our path (research/21 section 1.3); speed about 26 % better (research/19 section 6: 102 against 81 × real time) | Chunk R, gated (section 4) |
| Parakeet Unified EN 0.6B (research/17 spike 4) | Card AMI 10.14 against EN's 11.73; offline only; needs #70 (0.3.0) for correct tags on long files; conversion unverified; VRAM like Parakeet TDT | Beta.7 candidate after R; not in beta.6 |
| Transcribe 30B | The largest lever on paper, not runnable on this GPU (section 7) | A spike at most; no product work |

### 3.2 Chunk N1: where the missing words are (CPU, now)

One script, `tools/eval/levers.py gaps-why`, over the cached runs of section 1, for the 36 Zoom and AMI sessions and the 52 public English sessions, both models. For every gap of 2 s or more (the definition of research/19 section 0, now in `metrics.py`): its length, the reference words inside, the RMS level of the audio inside the gap against the session's median speech level, how many reference speakers are active inside it (overlap), and whether the other model has a gap covering at least half of it. Output, per set and pooled: gaps by cause class (quiet: more than 12 dB under the session median; overlapped: two or more reference speakers; other), the share of EN's gaps where 3.5 has words, the share of 3.5's gaps where EN has words, and the reference words at stake in each class. This is the characterisation research/19 section 8 asked for. It costs no GPU and decides nothing by itself; it tells N3 what the fill can reach and tells the plan whether a gain stage deserves a beta.7 design.

### 3.3 Chunk N1, second part: the gap-fill measurement and its rule (CPU, now)

**The fill, as measured and as it would ship.** The EN run is primary. Wherever the EN words leave a stretch of at least G seconds with no word (G in {1.0, 1.5, 2.0}, picked on the gate set), the 3.5 words whose spans lie inside that stretch (shrunk by 0.25 s at both ends, so a word EN placed late is not doubled) are inserted. Their speaker: 3.5's labels are mapped to EN's by maximum overlap of co-timed words outside the gaps (an assignment over pairs of words whose spans overlap by at least 60 percent); a filled word takes its mapped label; a 3.5 label with no mapping takes the nearest EN word's label and is counted as `unmapped`. Nothing else changes: no EN word is dropped or moved. The fused words JSON is written per session under a new config name (`en_fill_n35_g10`, `_g15`, `_g20`) and scored through the FakeEngine import (`crates/core/src/pipeline/import.rs`) as chunk K did, so the loop collapse and punctuation run as the app runs them; cpWER, tcpWER, WER with its deletions, WDER and the gap counts come from `run.py --metrics-only`. Also scored, for the record only: 3.5 primary with EN filling, and the oracle (per gap, whichever model's words score better against the reference), which bounds what any fill can reach.

**Decision rule, fixed now.**

1. G is the value with the lowest pooled cpWER on the gate set (7 sessions, 324 min) with EN; ties go to the smaller G.
2. With that G, the fill ships (chunk N3) only if, on the 36 Zoom and AMI sessions with EN primary: pooled cpWER is lower than EN alone by at least 0.2 points; no session is worse by more than 0.3; AMI ES2002a's cpWER is lower; gaps of 5 s or more are at least 30 percent fewer; WDER on no session worse by more than 0.5 points.
3. Confirmation, read once on the held-out public English groups (research/20 section 3: report, never tune): pooled cpWER not higher than EN alone, deletions fewer. A failure here stops the chunk whatever the Zoom sessions said, because the Zoom references are a recogniser's and the public ones are people's.
4. Default: `transcript.gap_fill = auto` (on when both models are installed and the language is English) if the pooled gain in rule 2 is at least 0.3 points; `off` by default, offered in Settings, if it is between 0.2 and 0.3.
5. The talks (7 sessions, 356 min) are reported beside the result as the generalisation check, as plan 16 section 3.13.1 did.

Cost of the measurement: zero GPU; about an hour of CPU for the alignments and the 88 × 3 FakeEngine imports. The harness pitfalls of plan 16 section 1 apply: new config names, and the fused JSON's SHA-256 in `run.json`.

### 3.4 Chunk N3: the second pass in the app (only if 3.3 passes)

- Where: `crates/core/src/engine/nemo_cli.rs` runs the file pass twice when the fill is on, EN then 3.5, sequentially (VRAM: EN peaks at 2.6 GB and 3.5 at 1.82 GB on the 77-minute file, README "Measured"; never both loaded); a new `crates/core/src/transcript/gapfill.rs` (pure code, unit-tested: the gap finder, the label mapping, the insertion, the counts) merges them before anything else sees the words, so the echo gate, the unknown-token check, the loop collapse, punctuation and segmentation run unchanged on one word list. Imports and the post-pass after Stop (both channels); not live.
- Recorded: `engine_info.gap_fill` `{applied, reason, fill_model, revision, sha256, g_s, gaps, words_filled, unmapped, wall_s}`, replaced wholesale on every build (Restore clones `engine_info`). The meeting details say "N words filled from Nemotron 3.5 in M gaps". The eval harness gets `gap_fill` in `configs.toml` and the cache key beside `punctuation` and `collapse_loops`.
- Setting `transcript.gap_fill` (`auto`, `off`; default by rule 4); `reason` `no_model` when 3.5 is not installed, with a Settings line that offers it (3.5 is 0.74 GB and already a manifest entry; the "recommended downloads" notice of plan 16 chunk E gains it).
- Speed: the transcribe step grows by one 3.5 pass, about 45 s per hour of audio on the 3080 at 80 × real time (docs/eval/2026-10-27-beta5-release.md, "Speed"). This is a new step, so beta.4's speed bar does not apply to it; the eval note records the total per hour with and without the fill, and the release notes state it. The progress estimate (`EXPECTED_REALTIME_FACTOR`, `nemo_cli.rs:57`) counts two passes when the fill is on.
- Acceptance: the unit tests above; the full eval set (36 sessions, both configs, `--force`) from the rebuilt `diarist-cli` reproduces the measurement's numbers within 0.05 cpWER on every session (the Rust fill must equal the Python fill; any difference is a bug in one of them); the echo rows of `echo_sample` unchanged with the fill off; one recorded meeting and one import on the reference machine by the owner.
- Ownership: `engine/nemo_cli.rs` (the second pass; append-only around the existing command builder), `transcript/gapfill.rs` and its export in `transcript/mod.rs`, `settings.rs` (the key; append-only), `live/postpass.rs` only the call that runs the file pass (both channels), `pipeline/import.rs` the same call, `models/mod.rs` a helper that finds the installed 3.5 file (append-only), `apps/desktop/src/components/Settings.svelte` (one row) and `MeetingView.svelte` (the details line), `tools/eval/configs.toml` and `run.py` (the key), docs/01, 04 and KNOWN-ISSUES rows, `docs/eval/<date>-gap-fill.md`. Must not touch `audio/echo/**`, `punct/**`, `store/**`, `transcript/loops.rs`.

### 3.5 Chunk N2: two GPU reports (about 40 minutes, an idle machine)

1. **The phase control on v0.2.0.** The gate set, both models, with the WAVs (a) trimmed by the first 70 ms, (b) given 20 ms and (c) 40 ms of leading digital silence; configs `shipped_en_ph70`, `_ph20`, `_ph40` and the same for 3.5, through `nemo_direct.py` or `run.py` on prepared copies of the audio. Output: deletions, gaps of 2 and 5 s or more and AMI cpWER per offset beside the unmodified run (research/19's `rs_*_shipped`). The phase noise band for deletions is the largest relative change over the three offsets, per model. About 6 runs × 324 min at 80 × real time, roughly 30 minutes. This band is what chunk R's deletion bar reads (4.2, item 1), and it is recorded whether or not R runs.
2. **Input gain, report only.** FLEURS English (the 10-minute slice) and AMI EN2002a Array1-01, EN model, with the input normalised to a peak of −3 dBFS over the whole file, against the cached unmodified runs. Output: deletions and WER. If deletions fall by 10 percent or more on both, beta.7 gets a design note (where a gain stage can sit without disturbing the echo gate's floor and owner-level estimates); if not, the lever is closed with the numbers. About 6 minutes of GPU.

Rules for both: quiet.lock and the pmon check of docs/eval/2026-10-27-beta5-release.md "Speed" for anything timed; untimed accuracy runs may share the GPU with nothing but the desktop. No run while the owner uses the PC.

### 3.6 Spike L: names on the surviving EN file (brief, not a chunk; about 30 minutes of GPU)

Plan 16 section 12.4's brief, applied to the file of section 1: research/12's AMI clip test (`tools/eval/nemo_direct.py` on the 5-minute clip, targets Craig, beagle, Planet of the Apes; boosts 0.5, 1 and 2; the 20-absent-names control; `levers.py boost`), plus one of the owner's meetings with the most name errors (private; numbers only) with the speakers' names the owner gave as the phrase list, through the same path at the file setting (lookahead 13, `v3-offline`). Kill rule, unchanged from plan 16 section 12.4: a name source is built only if, at a boost where the 20-absent-names control costs at most 0.2 WER points, at least half the target names are fixed and no boosted phrase is inserted where the reference lacks it more than once per hour. If it passes, the name source (speakers named in this meeting and in the library, never the title) is a beta.7 chunk, and so is the hosting it needs: the shipped EN file cannot boost, so boosting means shipping a re-converted EN file (771 MB) from Diarist's own release, which is where the models repository (section 14) comes in. If it fails, the KNOWN-ISSUES names row quotes the numbers and the lever waits for an upstream change to RNNT boosting or NVIDIA's republished GGUFs.

## 4. D2: the runtime gate (chunk R)

### 4.1 The gate

- **Source.** Only a tagged release (`v0.3.0`) with the three Windows x86-64 archives, pinned by URL, size and SHA-256 in `manifests/engine.json` as today. Not the nightly (its bytes change under a fixed URL, research/21 section 1.5) and not a build of ours (hosting runtime binaries plus the CUDA EULA is a new responsibility, section 1.5).
- **Date.** Chunk R starts only if the tag exists by Friday 6 Nov 2026, 23:59 IST. The reason for a date rather than waiting: NVIDIA publishes none; for 0.2.0 the tag followed the version bump within four minutes and this time 9.6 hours passed without one (research/21 section 1.4); PR #88 is still open; beta.6 has work that does not depend on it. The owner can move the date; moving it later holds the release for an upstream event nobody controls.
- **If the tag lands after the date** (while beta.6 is being measured or built): it waits for beta.7. The KNOWN-ISSUES runtime row says so.
- **If the tag never lands in time:** `manifests/engine.json` and `TESTED_ENGINE_VERSIONS` stay at 0.2.0; the KNOWN-ISSUES row for #67 (`docs/KNOWN-ISSUES.md:9`) records that the fix sits on `main` and in a nightly built from `d8a1abf` or later, which Diarist does not ship because it cannot be pinned; `#81` gets a row: the speaker half fixed by #87 on `main`, the deleted-word half read as chunk phase if N2's band says so.

### 4.2 The five measurements, with bars (research/21 section 1.7, restated as rules)

**Amended by 4.4 on 31 Oct 2026, before any 0.3.0 run: items 1 and 2 are replaced there by bars R1 to R5; items 3 to 5 and the variant stand.**

All on the tagged Windows CUDA archive, with `DIARIST_EVAL_ENGINE_DIR` pointing at it and new config names (`*_030`), on an idle machine through the shared GPU queue. About 1.5 hours of GPU in all, 2 with the phase offsets.

1. **Deleted words.** Gate set and broad set (29 sessions, 1,341 min), both models, 0.3.0 against the cached v0.2.0 runs: deletions, gaps of 2 and 5 s or more, AMI ES2002a cpWER. Bar: pooled deletions per model no higher than v0.2.0's by more than the phase noise band of N2 item 1 (if N2 has not run, R runs the three phase offsets first; the band is never set after the 0.3.0 numbers are seen); no session's cpWER worse by more than 0.5; no new gap of 5 s or more that the phase controls do not also show. About 35 minutes of GPU (the v0.2.0 side is cached), plus the 30 minutes of phase offsets if N2 has not run.
2. **Speakers.** AMI ES2002a cpWER at most v0.2.0's + 0.1 for both models (18.0 and 20.7 today, docs/eval/2026-10-27-beta5-release.md gate table); on `pub-meeting` and `pub-far` (44 sessions, 733 min) pooled cpWER and WDER not higher than the public baseline's (EN 29.9 / 4.8 and 42.7 / 8.8; 3.5 32.2 / 5.0 and 44.4 / 9.6, docs/eval/2026-10-27-baseline-public.md "Pooled"). About 20 minutes of GPU.
3. **Live.** Re-record `docs/research/fixtures/ws_session_events.jsonl` and check every event the adapter reads (research/21 rows 12 and 13: one interim per decoded chunk; `audio_processed` now trails by 0.48 s at R6); `cargo test -p diarist-core --test live_ws_gpu` at R6: first-word latency median at most 1.0 s and p90 at most 1.5 s, two sessions under 2 GB (docs/eval/2026-10-27-live-lookahead.md, the rule); `live_carry_repro`; `echo_sample` and the plan 15 echo replay within 0.5 points. About 15 minutes.
4. **Package.** The archive's SHA-256 against NVIDIA's digest, extracted size, peak VRAM of the file pass and of the server (real cuBLAS may use more than the shim did), × real time on the gate set under quiet.lock; it runs on driver 610.88. Bar: × real time not worse than beta.5's (81.3 EN, 78.6 3.5 pooled; faster is expected); peak VRAM within 300 MiB of today's per path. Expect 8 to 61 word edits per recording from the cuBLAS switch alone (research/19 section 6).
5. **Our tests** with the new manifest: the manifest round trip, `TESTED_ENGINE_VERSIONS`, the package-name tests in `live_ws/supervisor.rs`, `doctor`.

Variant, report only: file jobs without `--endpointing` on 0.3.0, gate set, both models (words expected identical; finals counted). About 8 minutes.

If any bar fails, beta.6 stays on 0.2.0 and the eval note says which bar and by how much; the runtime does not ship "with a known loss" because the owner's bar is no accuracy loss and the gain on offer is speed.

### 4.3 What R changes (research/21 section 1.6)

`manifests/engine.json` (three packages: name pattern `nemo-speech-0.3.0-windows-x86_64-<backend>`, URL, SHA-256, size; the CUDA entry's `licence` becomes "Apache-2.0; bundles NVIDIA cuBLAS and cuBLASLt under the CUDA EULA, redistributed by NVIDIA in the archive"); `TESTED_ENGINE_VERSIONS` (`manifest.rs:20`) gains `"0.3.0"` and keeps `"0.2.0"` so a user who has not updated the engine is not warned; the `supervisor.rs` package-name tests; the WebSocket fixtures; research/08 sections 4 and 6 (the new `audio_processed` meaning, rarer empty deltas, words inside the audio, exit codes); INSTALL, README and the setup screen's download and disk figures (+406 MB for CUDA); the KNOWN-ISSUES rows for #67 (fixed in the release) and #81; optionally `EXPECTED_REALTIME_FACTOR` after item 4. The Vulkan and CPU packages are taken on the same tag with the smoke test of docs/KNOWN-ISSUES.md's package row (5-minute AMI clip through the app's path) and no more, as in beta.2. Installed users keep 0.2.0 until they take the new engine from Settings; the app's "Check for newer runtime" offers it.

Ownership: `manifests/engine.json`, `crates/core/src/models/manifest.rs` (the constant and its tests), `crates/core/src/engine/live_ws/supervisor.rs` (tests only), `crates/core/src/engine/nemo_cli.rs` (`EXPECTED_REALTIME_FACTOR` only), `docs/research/fixtures/ws_session_events.jsonl`, `docs/research/08-runtime-cli-contract.md`, docs/INSTALL.md, README.md, docs/04 sizes, `docs/KNOWN-ISSUES.md` runtime rows, `docs/eval/<date>-runtime-030.md`, `tools/eval/configs.toml` (the `*_030` configs). Must not touch `transcript/**`, `audio/**`, `punct/**`.

### 4.4 Addendum, 31 Oct 2026: R's bars after N2

Written by the planner on 31 Oct 2026 from N2's note (docs/eval/2026-10-30-phase-control.md, commit `f9da9d3` on `claude/beta6-phase-names`) and the v0.3.0 tag, **before any run on 0.3.0: no 0.3.0 number exists or has been seen.** The bars R1 to R5 below replace 4.2 items 1 and 2 for chunk R and are fixed now; 4.2 items 3 to 5 and the endpointing variant stand as written. Nothing in this section changes after a 0.3.0 number is seen; a miss by any margin is a miss, and the eval note records the margin (section 13).

**The gate is met.** NVIDIA tagged `v0.3.0` on 30 Oct 2026 at 11:24 UTC (tag object `69154fd`, commit `20999cc`, which contains #87's `c540e21`), with the three Windows x86-64 archives and a `.sha256` file each: CUDA 508,625,181 bytes, Vulkan 22,417,032, CPU 8,694,509 (reported to the planner on 31 Oct; R verifies the digests itself, 4.2 item 4). The date of 4.1 is met with a week to spare, and the CUDA figure is the +406 MB the plan carries.

#### 4.4.1 What N2 showed, and what it does to 4.2's bars

N2 ran v0.2.0 on the gate set (324 min, both models) at three input offsets, −70 ms (trimmed) and +20 and +40 ms (leading silence), beside the cached unmodified run (the note, "Results: the phase control"). The phase alone moved:

- pooled deletions by at most 2.70 % (EN) and 3.30 % (3.5): the band 4.2 item 1 reads, now fixed;
- deletions per session by up to 11.8 % (EN, Sep 5) and 8.5 % (3.5, Aug 14), in both directions;
- AMI ES2002a cpWER by +0.00 to +0.35 with EN (18.03 → 18.03, 18.38, 18.30) and by +0.62 to +1.00 with 3.5 (20.67 → 21.29, 21.67, 21.55); Aug 14's by about +2 points for both models, from speaker attribution (WDER 19.4 → 21.1 and 21.8 with EN) while its WER moved by at most 0.35;
- the gaps of 5 s or more: 1 to 3 new and 1 to 2 gone per offset with EN, 5 to 8 new and 4 to 6 gone with 3.5, at different places at each offset (`tools/eval/phase.py --json` lists them per session).

So four of 4.2's clauses compare one phase draw with another using a tolerance smaller than the draw: item 2's ES2002a bar of +0.1 (inside the phase move for both models, by a factor of ten for 3.5; N2's note says so in its last "What else" bullet); item 1's per-session 0.5 (Aug 14 with both models and ES2002a with 3.5 move more than that on v0.2.0 alone); item 2's zero tolerance on the public groups' pooled cpWER and WDER; and item 1's "no new gap of 5 s or more that the phase controls do not also show" (the controls show different new gaps at every offset, so a 0.3.0 run at its own phase would show gaps none of them show). The draw is not even the same on both sides: #87 moved the runtime's internal chunk phase by 70 ms (research/21 section 1.3), so 0.3.0 on the unmodified input is a different phase from v0.2.0 on the unmodified input by construction. Research/19's build of `main` read +3.3 % EN deletions at a single phase, 0.6 points above EN's band, and N2 cannot say whether that was the runtime or the draw. The remedy is to compare like with like, which is what changes below.

For the record: N2's input-gain report closed that lever by its rule (the note, "Rule 2"), and spike L failed its kill rule at every boost (docs/eval/2026-10-30-names-spike.md); neither bears on R.

#### 4.4.2 The ruling

Principle: where both runtimes can be run at the same four input conditions (the gate set), the mean over the four conditions decides and the phase averages out; where one phase per side is all there is (the broad set, the public groups), a tolerance fixed here from N2's numbers applies, and a per-session trip escalates to the four-condition comparison instead of failing R outright. Pooled numbers decide; per-session clauses catch a broken session, not a drift. Each model is judged on its own numbers by the same rule, and R passes only if both models pass, because one runtime serves both.

**R1. Deletions, gate set, like for like.** 0.3.0 runs the gate set at N2's four conditions (unmodified, −70, +20, +40 ms), both models. Bar: the mean pooled deletions over 0.3.0's four runs at most v0.2.0's four-run mean plus the band: **EN at most 1,458.85** (mean 1,420.5 of 1,409, 1,401, 1,425 and 1,447, × 1.0270); **3.5 at most 1,980.00** (mean 1,916.75 of 1,879, 1,941, 1,911 and 1,936, × 1.0330). No condition is compared with its namesake. The single-phase difference (0.3.0 unmodified against `rs_*_shipped`) is reported beside research/19's +3.3 % and +1.0 %, for the record only.

**R2. Deletions, broad set (29 sessions, 1,341 min), one phase each side.** Pooled deletions per model at most v0.2.0's × 1.0270 (EN) and × 1.0330 (3.5); the v0.2.0 side is rescored from `rs_*_shipped` with today's `metrics.py` and written into the eval note before the first 0.3.0 run. Pooled over 1,341 min the phase noise is smaller than on the 324 min the band was measured on, so the band is generous here, and it stays as fixed.

**R3. Gaps of 5 s or more.** Gate set: the mean over 0.3.0's four runs of the pooled count at most the largest any v0.2.0 phase run showed, **EN 25, 3.5 33** (N2: 23, 25, 22, 23 and 30, 30, 31, 33); the mean over the four runs of the count of new gaps against `rs_*_shipped` (phase.py's `new_5s`) at most N2's largest, **EN 3, 3.5 8**. Broad set: pooled count at most v0.2.0's × 1.10 per model (the gate's largest phase move in this count: EN 23 → 25, 3.5 30 → 33); the new gaps listed with their times, report only. Gaps of 2 s or more: report.

**R4. Per-session cpWER.** Gate set: 0.3.0's four-run mean per session at most v0.2.0's four-run mean plus the larger of 0.5 and that session's own four-run range (N2's per-session tables; the range exceeds 0.5 only on Aug 14, 2.08 EN and 2.24 3.5, and on ES2002a with 3.5, 1.00). Ceilings, cpWER in percent, two decimals as `metrics.py` prints them:

| Session (min) | EN: v0.2.0 mean of four / ceiling | 3.5: v0.2.0 mean of four / ceiling |
|---|---|---|
| Aug 2 (44) | 9.57 / 10.07 | 10.54 / 11.04 |
| Aug 9 (44) | 7.94 / 8.44 | 8.46 / 8.96 |
| Sep 7 (48) | 7.19 / 7.69 | 7.58 / 8.08 |
| Sep 5 (45) | 4.87 / 5.37 | 5.23 / 5.73 |
| Sep Logistics (46) | 7.12 / 7.62 | 6.50 / 7.00 |
| Aug 14 (77) | 45.11 / 47.19 | 44.17 / 46.41 |
| AMI ES2002a (21) | 18.19 / 18.69 | 21.30 / 22.30 |

Broad set: a session whose 0.3.0 cpWER on the unmodified input is worse than v0.2.0's by more than 0.5 does not fail R by itself. It is re-run at the three offsets on both runtimes and judged by the gate rule (mean against mean plus the larger of 0.5 and the session's v0.2.0 range); if it still fails, R fails. About 3 minutes of GPU per escalated session and model; on N2's gate rate (1 session in 7 with EN, 2 in 7 with 3.5 move more than 0.5 at some offset) about 4 to 12 of the 58 session-model pairs trip by phase alone.

**R5. Speakers (replaces 4.2 item 2).** On `pub-meeting` (8 sessions, 373 min) and `pub-far` (36 sessions, 360 min), 0.3.0 on the unmodified input, both models: pooled cpWER and pooled WDER per group at most the public baseline's plus **0.2 for EN and 0.4 for 3.5** (baselines EN 29.9 / 4.8 and 42.7 / 8.8; 3.5 32.2 / 5.0 and 44.4 / 9.6; docs/eval/2026-10-27-baseline-public.md "Pooled"). Derivation: ES2002a's four-run cpWER range (0.35 EN, 1.00 3.5) over √8 for pub-meeting's eight sessions is 0.12 and 0.35, rounded up; the same figures apply to pub-far (36 sessions, so conservative there) and to WDER (the phase moves on ES2002a and Aug 14 are speaker attribution, 4.4.1). Both tolerances sit below #81's single-session signal (+0.7 EN, +0.9 3.5) and below #87's own pooled main-against-v0.2.0 gap (+0.66 and +0.64 on 16 and 6 AMI sessions, research/21 section 1.3), so a regression of #81's size still fails. **AMI ES2002a is report only for both models:** 0.3.0's four-run table beside v0.2.0's (EN 18.03 to 18.38, 3.5 20.67 to 21.67), with a sentence if 0.3.0's mean lies outside that range; it keeps its R4 ceiling (18.69 EN, 22.30 3.5). The +0.1 bar is withdrawn for both models, not only for 3.5: one 21-minute session cannot resolve 0.1 for either (EN's own phase range is 0.35), and judging the two models by different instruments would make the verdict depend on which model is noisier. Each model's speaker verdict rests on its own R5 numbers.

**R6 to R8.** 4.2 items 3 (live), 4 (package: the digests against NVIDIA's `.sha256` files, the sizes above, extracted size, peak VRAM, × real time not worse than beta.5's, driver 610.88) and 5 (our tests) unchanged. The endpointing variant unchanged, report only.

As 4.2 says: if any of R1 to R8 fails for either model, beta.6 stays on 0.2.0 and the eval note says which bar and by how much.

#### 4.4.3 R's measurement plan, as changed

1. R's branch carries N2's tooling: `run.py`'s `audio_shift_ms` key and `tools/eval/phase.py` from `f9da9d3` on `claude/beta6-phase-names` (merged first, or taken onto R's branch); the `*_030` configs are appended to `configs.toml` after N2's, as 11.3 orders.
2. Before the first 0.3.0 run: the v0.2.0 side is rescored with today's `metrics.py` (`--metrics-only`: `rs_*_shipped` for the 36 sessions, the public baseline's `shipped_*` runs for the 44) and every reference number of R1 to R5 is written into `docs/eval/<date>-runtime-030.md` under "Rules", with this section quoted.
3. Configs: `shipped_en_030`, `shipped_en_030_ph70`, `_ph20`, `_ph40` and the same for 3.5 on the gate set; `shipped_en_030` and `shipped_n35_030` on the broad set and the public groups. `phase.py --base rs_en_shipped --configs <the four 0.3.0 configs>` (and 3.5) gives R1, R3 and R4's gate numbers; `phase.py --base shipped_en_030 --configs <its three offsets>` gives 0.3.0's own phase band, report only, beside N2's.
4. The gate set's unmodified 0.3.0 run is the timed run of item 4 (quiet.lock, pmon); every other run is untimed and shares the GPU with nothing but the desktop (3.5's rules).
5. R4's escalation runs after the broad set is scored and only for the sessions that trip; the eval note lists them.
6. The note's order: R1 to R5 with their numbers, then R6 to R8, then the variant; each bar quoted with its number before the verdict (section 13).

#### 4.4.4 GPU cost, for scheduling

| Step | Runs | Transcribe, at 100 × real time for 0.3.0 (research/19 section 6) and 80 × for v0.2.0 | Lock time with N2's harness overhead (about +25 %) |
|---|---|---|---|
| Gate set, four conditions, both models (R1, R3, R4, R5's AMI table; the unmodified run timed) | 8 × 324 min | 26 min | 33 min |
| Broad set, both models (R2, R3, R4) | 2 × 1,341 min | 27 min | 34 min |
| Public groups, both models (R5) | 2 × 733 min | 15 min | 19 min |
| R4 escalations, 4 to 12 session-model pairs | 6 runs each | 12 to 36 min | 15 to 45 min |
| Endpointing variant | 2 × 324 min | 6 min | 8 min |
| Live fixtures and tests (item 3) | | | 15 min |
| Vulkan and CPU smoke on the 5-minute clip (4.3) | | | 5 min |
| **Total** | | | **about 2 h 10 min to 2 h 40 min** |

Against 4.2's 1.5 to 2 hours: the 30 contingent minutes for v0.2.0's offsets are spent (N2 ran them) and go to 0.3.0's offsets instead; the escalations are the net addition. If 0.3.0 runs at 80 × rather than 100 ×, add about a fifth. Three idle windows fit it: the gate's four conditions with the timed run (about 35 minutes); the broad set and the public groups (about 55 minutes); the escalations, the live tests and the variant (30 to 70 minutes). Nothing runs while the owner uses the PC.

## 5. D3: chunk M, continued (chunk M2)

### 5.1 What stands and what is amended

Plan 16 section 12.7 (iv) fixed the sweep's variants and the ship rule before the sweep; the sweep (docs/eval/2026-10-26-echo-owner-level.md section 8) found that every listed variant misses a bar and that the layer is the only thing removing residue at the owner's level. One rule outside the list was measured as a lead: spare a cancelled word from the post-canceller correlation layer when the owner's level is known and the word sits within M dB of it. At M = 6 it recovers 36 of (b′)'s 48 (bar 24), keeps meeting 21 at 77 / 42, holds every duplicate and clean-owner bar, and misses one bar by one word: meeting 20's far-active cut words kept, 16 against 15.

That bar is a proxy. "Far-active cut words kept" counts kept "me" words spoken while the far end is active and cut by 3 dB or more (plan 16 section 12.2.3), which is also the exact description of the owner's own double-talk words the rule is built to recover. A rule that recovers them raises the count by construction, so the proxy cannot tell success from failure on meeting 20. The honest fix is a labelled bar, and the labelling needs the audio, which only the owner has.

**Amendment, made now, before any new run.** On meeting 20 (and on 21 if any variant changes it), the kept words a variant adds over today's decision are listed with their times by the agent (a private note; at most ten words: 1 at M = 6, 3 at M = 9, 10 at M = 12 by the sweep). The owner listens to those spots in Diarist and labels each word `echo`, `own` or `unclear`, before the per-M result table is written for the record. A variant passes meeting 20 when none of the words it adds is labelled `echo`. Everything else in 12.7 (iv)'s rule stands unchanged: (b′) at least 24 recovered, residual duplicates on 12, 14, 16 and 20 within 0.5 points of 0.63, 0.00, 0.00 and 1.45 percent, meeting 21 at most 42 far-active cut words, the spliced set at most 8.1, FLEURS at most 12.69, the clean owner's words on 12, 14 and 16 at least 1,523, 301 and 227. If the owner declines to label, the proxy bar (at most 15) stands as written and M stays out, with the KNOWN-ISSUES row as it is.

### 5.2 The rule's form, fixed now

In `band_after_canceller` (`crates/core/src/live/postpass.rs`, the post-canceller correlation layer), a cancelled word is spared from removal when `owner_source` is `quiet` or `clean` (the level is known, plan 16 section 12.7 (i) 2) and the word's level after the canceller is at least `owner_db − M`. M is a constant `CORRELATION_SPARE_DB`, swept over {6, 9, 12}; the shipped M is the largest that passes every bar of 5.1. With an unknown level (`too_few`, `implausible`) nothing changes, so meeting 21 is untouched by construction and the silent tier's interaction noted in the eval's section 8 ("The silent tier") does not arise: a recording in the silent tier has no known level.

### 5.3 Data, and the confirmation fixture

- (b) and (b′) are regenerated with the same recipe as docs/eval/2026-10-26-echo-owner-level.md section 3 (FLEURS English utterances over meeting 12's far end, the owner at +23 and +28.1 dB over the cancelled mic's floor), from a fresh scratch copy of meeting 12, outside the repository, deleted after the runs as before.
- A third fixture (b″) at +25.5 dB with FLEURS utterances not used in (b) or (b′) is built with them and **not** used for the pick. After M is chosen on (b), (b′) and the meetings, (b″) is read once: the variant must recover at least half of the owner's words the shipped layer removes on it. This is the check that the pick is not tuned to two fixtures.
- Meetings 12, 14, 16, 20 and 21 replayed from their stored words (`echo_sample.rs`), the spliced set (`aec_postpass.rs`) and the full FLEURS fixture (`echo_fleurs.rs`), as in the sweep.
- GPU: the three fixtures' transcriptions, a few minutes through the shared queue; everything else CPU.

### 5.4 Chunk M2 brief

Owns: `crates/core/src/live/postpass.rs` only inside `band_after_canceller` and its call (append-only, re-read first; chunk N3 edits the file-pass call in the same file); `crates/core/src/audio/echo/layers.rs` only if the owner level has to be passed in; `crates/core/tests/echo_sample.rs`, `echo_fleurs.rs` (the (b″) recipe behind an env var), `aec_postpass.rs`; `docs/eval/2026-10-26-echo-owner-level.md` section 8 (a new sub-section with the table, the labels as counts, (b″)'s read) or a new dated note; `docs/KNOWN-ISSUES.md` echo row (`:49`; handed to H if H has started). Must not touch `echo_gate.rs`, `transfer.rs`, `text.rs`, `presence.rs`, `punct/**`, `store/**`. Timebox: two working days of agent time from the fixtures being on disk, plus the owner's ten minutes; H does not wait beyond that. Acceptance: the table of 5.1 for M in {6, 9, 12} with every bar judged; the labels recorded as counts only; (b″)'s read; unit tests for the spare rule (known level spares inside M and not outside; unknown level spares nothing); meetings 12 to 21's live and Stop rows identical to the eval's section 6; `engine_info.echo` gains `correlation_spared` (a count). Verification as section 13; no eval table (the file path is untouched).

## 6. D4: code-switching, and the repro zip

Not now. The measurement in research/21 section 3.3 is sound and stays the design, but what it needs is gone: the re-converted 3.5 files were deleted on 27 Oct, so it costs a 2.37 GB `.nemo` download (the owner's firewall prompt and approval), the converter venv, minutes of CPU and about 26 minutes of GPU, to answer a question whose only beneficiaries are Hindi and Mandarin users of 3.5 and whose outcome, if positive, is hosting a re-converted 3.5 file (plus licence notices, research/19 section 7) that NVIDIA may republish anyway (research/19 section 8: "take NVIDIA's republished files when they appear"). On 1,697 minutes of English the same fix measured nothing (research/19). The owner's priority is their own recordings, which are English. When NVIDIA republishes the 3.5 GGUF, the measurement runs as part of that manifest bump (Indic DiarBench Hindi and the AliMeeting headset mix, both arms of 3.3), and if a user reports dropped passages in a mixed-language recording, it runs then on a re-conversion. Recorded in KNOWN-ISSUES as a watch item.

The 120 s repro zip: not downloaded. It is a third-party recording with unstated rights; without the re-converted 3.5 file it can only show whether the omission reproduces on our CUDA build with the shipped file, which decides nothing; and the author's regime (no language tag, lookahead 3, no endpointing, no diarizer) is not Diarist's. If the owner wants it anyway, it stays on the PC and out of the repository (research/21 section 3.3, last bullet).

## 7. D5: the Transcribe 30B spike (owner's call)

### 7.1 Why it is not a beta.6 feature

Research/21 section 2.6: the runtime loads every decoder layer on the GPU and exposes no expert offload, so the 20.35 GB IQ4_NL decoder cannot load on a 10 GB RTX 3080; CPU-only needs about 23 to 24 GB of this PC's 31.9 GB and runs at an estimated 30 to 60 minutes per hour of audio against 45 seconds today. A product path exists only for 24 GB GPUs or 32 GB RAM machines, as an opt-in slow pass (section 2.8), and the files to ship do not exist from NVIDIA (section 2.4). Nothing here can reach the owner's "measured accuracy" in beta.6.

### 7.2 What the spike would settle, and its shape

The one open question that matters is whether the model's published advantage (about half our cpWER on meetings, section 2.2) holds on the owner's own recordings, where the test sets are clean (AMI, ICSI and AliMeeting are in its training data; the owner's Zoom recordings and NOTSOFAR are not). Recommended form (research/21 section 2.9, option 1), with bars fixed now:

- **When:** after the runtime decision, on an idle machine, about 1.5 to 3 hours of CPU with every heavy program closed; the runtime is the tagged 0.3.0 if it landed, else a local build of `c540e21` or later for the spike only (research/19 built `main` in about 6 minutes).
- **What:** `--device cpu`, the IQ4_NL decoder and the F16 encoder file; first a 3-minute public clip (peak RAM, tokens per second; stop if peak RAM passes 27 GB), then AMI ES2002a (21 min), then one of the owner's Zoom sessions (the audio stays on the PC); `--diarize` with the speaker-tag instruction and word times (section 2.3), scored with the eval harness against the shipped EN file pass of the same sessions.
- **Bars, pre-registered:** "a large gain" means cpWER on the Zoom session at least 25 percent lower than EN's and deletions at least 30 percent fewer, with speaker count within one; AMI is reported but, being in the training data, decides nothing. Time per hour of audio and peak RAM are recorded as measured.
- **Source:** the owner picks between (a) NVIDIA's 67.5 GB checkpoint converted here (about 155 GB of disk at the peak, hours of CPU, verified provenance) and (b) a third-party pre-converted repository (AtomicChat's NeMo-Speech.cpp files, about 22 GB, unverified bytes parsed by native code). Recommendation: (b) for the spike, because it answers the question at a third of the download and nothing from it ships; the files are hashed and recorded, kept outside every repository and deleted after the spike unless the owner keeps them. Anything that ever ships is converted from NVIDIA's checkpoint by us, or taken from NVIDIA's own GGUF if one appears.
- **If the bars hold:** beta.7 gets a design for an opt-in "high-accuracy pass after Stop" for machines that can hold it (section 2.8: 24 GB VRAM or 32 GB RAM; recordings split at silences into parts of at most 55 minutes with Diarist's own diarization re-tagging the words, section 2.7), with the model as a drop-in folder plus sidecar in the user-owns-the-folders shape until a hosted file exists.
- **Now, free of cost:** the owner files the upstream request for MoE expert offload in SALM (section 2.6: llama.cpp already supports it through tensor overrides; with the experts in RAM a 1-hour meeting might take 10 to 20 minutes on this PC). A positive answer there is what would make the 10 to 16 GB GPUs most Diarist users have workable.

## 8. D6: the punctuation pin (chunk Q)

### 8.1 Decisions

1. **Pin.** `revision` moves to `0cea764aa6fc55c8bdfc20a5f0bb4dc27b64753d` (v1.1.1). The `url` fields of `onnx/model_quantized.onnx` and `tokenizer.json` carry the new revision; their `sha256` and `size_bytes` do not change (the files are byte-identical; chunk Q re-verifies by hashing the downloads). Because the entry is `direct: true` with a `dir`, the installed layout has no revision in its path, so no installed copy re-downloads the 306 MB.
2. **LICENSE.** Fetch upstream's `LICENSE` at `0cea764` and install it as `LICENSE` in the model's folder. Chunk Q hashes it first: if its SHA-256 is `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` (the canonical Apache-2.0 text, which `LICENSE-Apache-2.0.txt` on `models-2026-10` is), the re-hosted `LICENSE-Apache-2.0.txt` leaves the `files` list and the folder carries the authors' file. If the hash differs, the chunk stops and reports the difference (a text that is not the canonical licence needs reading, not pinning).
3. **NOTICE.** Fetch upstream's `NOTICE` (1,567 bytes) at `0cea764` and install it as `NOTICE`; its hash goes into the manifest. Diarist's own `NOTICE-punctuation.txt` stays in the `files` list, unchanged bytes from `models-2026-10`: it is Diarist's dated provenance record of `ee31fdf` (the base model, the training data, the Qwen sentence) and every statement in it is true of that revision, whose model files are the pinned ones. docs/04 section 8 and LICENSES.md gain the sentence that the authors added LICENSE and NOTICE in v1.1.1 and that the model files did not change.
4. **Licence string.** `"Apache-2.0 (LICENSE and NOTICE in the repository from v1.1.1, 0cea764; model files byte-identical to ee31fdf); base model MIT"`. LICENSES.md row 19 says the same.
5. **The upgrade rule** (plan 16 section 12.9 (iii) 6, plan 17 section 8): chunk Q re-reads the card's licence line and the repository's file list at `0cea764` on the day, records both with the date in the eval note, and prepares the dated snapshot: the card at `0cea764` (`punctuation-model-card-0cea764.md`) and the Hugging Face API model-info JSON of that day. They are added to `models-2026-10` as extra assets (the release stays; its five manifest-referenced files are never changed or removed; non-manifest assets may be added) or, if the owner has created the models repository by then (section 14), to its first release. `CONTENTS.md` in `C:\Users\ebin\claude-ground\diarist-releases\models-2026-10\` gains the two rows; the owner uploads.
6. **Upgraders.** A beta.5 data root has the model at the old layout with `LICENSE-Apache-2.0.txt` and without `LICENSE` and `NOTICE`. Chunk Q reads `models/scan.rs` and `models/fetch.rs` and makes the rule explicit: a punctuation entry whose model and tokenizer are present with the right hashes stays selectable and punctuates; missing licence files are fetched by the next `pull` or by the "recommended downloads" path, never by blocking punctuation. A test covers it.
7. **Plan 17 section 9's pending discussion post** (asking for a LICENSE file) is moot. A thank-you note on the model's page is the owner's call and blocks nothing.

### 8.2 Brief

Owns: `manifests/models.json` (the entry), `crates/core/src/models/manifest.rs` tests that assert the entry's hashes and file list, `models/scan.rs` and `models/fetch.rs` only for item 6 if the behaviour is not already so, `docs/04-model-guide.md` section 8, `LICENSES.md` rows 19 and 27 to 32, `docs/KNOWN-ISSUES.md` punctuation rows, `docs/INSTALL.md` firewall row if the host list changes (it does not: `huggingface.co` and `github.com` stay), the snapshot files and `CONTENTS.md` in the release folder, `docs/eval/<date>-punctuation-pin.md` (short: the hashes, the card line, the file list, the date). Must not touch `punct/**`, `apps/desktop/src/**`. Acceptance: `cargo test` with the new hashes; `diarist-cli --data-dir <empty> models pull punctuation-horizon-small` fetches every file from the real hosts and `models scan` lists it as `punctuation` with `manifest_name` `punctuation-horizon-small`; a copy of a beta.5 data root keeps punctuation working before and after the two small files arrive; `diarist-cli punctuate` on a cached-words import gives a word sequence identical to the beta.5 run of the same meeting (the model did not change, so the marks must not either). No GPU.

## 9. D7: the recording indicator (chunk V)

### 9.1 The option

The design step (worktree `beta6-rec-indicator`, no commits) inventoried today's state (section 1) and rendered three options: **A**, a dot overlay on the taskbar button; **B**, a red REC pill overlay plus the window title "● REC 12:34 · Diarist" ticking each second plus a redrawn tray dot plus an in-app REC pill without the fading pulse; **C**, swapping the window icon. B is recommended and this plan assumes it; the owner fixes the option, and may take B without the title tick (each part of B is independent). Reasons for B: the owner's taskbar is dark, at 200 percent and auto-hidden, so a bare dot is small and a swapped icon is easy to miss, while a pill with letters reads at a glance and the title gives the state to screen readers and to the taskbar's hover text; the tray dot is redrawn in any case because today's is painted off-grid without anti-aliasing and its white ring vanishes on the dark icon (`tray.rs:31-53`).

### 9.2 Parts of B

1. **Taskbar overlay.** `window.set_overlay_icon(Some(image))` (Tauri 2.12.4, Windows) with a pre-rendered REC pill at 16, 20, 24 and 32 px for the taskbar's scale and theme (the same approach `icons.rs` already takes for the window and tray icons, per the comment at `apps/desktop/src-tauri/Cargo.toml:49-51`), and `ITaskbarList3::SetOverlayIcon` with the description "Recording" for accessibility, through the `windows` crate with `Win32_UI_Shell` added to its features (`Cargo.toml:52-57`). The overlay is set when the phase becomes recording and cleared on stop; because an overlay is lost while the window is hidden to the tray (no taskbar button), it is re-applied whenever the window is shown again while recording (`lib.rs`, the show path; `tray::show_main`).
2. **Window title.** "● REC 12:34 · Diarist" from the recording phase and the elapsed time, updated once a second while recording, "Diarist" otherwise (`tauri.conf.json:16` keeps the default). Set from the Rust side where the phase and `started_at` are known (`tray::update(app, phase, started_at)`, `tray.rs:152`), so it is independent of which view is shown. Format: `HH:MM:SS` past an hour, as `formatClock` in the live view does.
3. **Tray dot.** `with_red_dot` is replaced by pre-rendered dot assets per scale (100, 125, 150, 200 percent), pixel-aligned with anti-aliasing and a ring that reads on both the light and the dark base icon; the pick by scale lives where `rescale` already picks the base (`tray.rs:107-123`). Stop-armed and phase handling unchanged.
4. **In-app pill.** `.rec-dot.live`'s pulse to 35 percent opacity (`app.css:295-303`) goes; a `.rec-pill` with "REC" and the timer, red at full opacity, in the live view's status strip (`LiveView.svelte:151`) and the Record button (`Library.svelte:203, 215`). `prefers-reduced-motion` stays honoured by having nothing to reduce.

### 9.3 Brief

Owns: `apps/desktop/src-tauri/src/tray.rs`, a new `apps/desktop/src-tauri/src/taskbar.rs` (overlay and description; `#[cfg(windows)]`), `apps/desktop/src-tauri/src/icons.rs` if the assets are loaded there, `lib.rs` only the show path and the module line (append-only), `live.rs` only where the phase changes are already reported to the tray (append-only), `apps/desktop/src-tauri/Cargo.toml` (the `windows` feature list only), `apps/desktop/src-tauri/icons/` (the new PNGs), `apps/desktop/src/app.css`, `LiveView.svelte`, `Library.svelte`, docs/00-product-spec.md or docs/01-architecture.md wherever the tray and the recording state are described, CHANGELOG (handed to H). Must not touch `crates/**`, `Settings.svelte`. Acceptance: on the reference machine, at 200 percent with the dark taskbar and auto-hide: the pill is legible on the taskbar button and in the taskbar's hover text; hiding to the tray and showing again keeps the overlay; the title ticks and returns to "Diarist" on stop and on a failed start; the tray dot is crisp at 100, 150 and 200 percent on both themes; the in-app pill has no fade; keyboard paths unchanged. `cargo fmt`, `clippy -D warnings`, `cargo test`, `bun run check`, `bun run test`, `bun run build`. No GPU. Starts as soon as the owner picks.

## 10. D8: housekeeping (chunk U) and what waits

### 10.1 Chunk U

1. **Re-time Nemotron 3.5** on Aug 9, Sep Logistics and Aug 14 (plan 17 section 10): the beta.5 build `ce5b757b…`, configs `rel_shipped_n35_t2`, `--transcribe-only --no-diar-pass`, under quiet.lock with `nvidia-smi pmon -s u -d 1` running and the Claude desktop app idle (the agent starts the block as a background job and writes nothing until it ends, or the owner runs the script). Bar, fixed now: pooled seconds over the three sessions within 5 percent of beta.4's 32.4 + 32.7 + 54.7 = 119.8 s (at most 125.8 s), with a clean pmon log (no process but nemo-speech above 5 percent SM in any sample). If it passes, the beta.5 release note's speed row gets the figure and the "re-timed in beta.6" sentence closes. If it fails with a clean log, the same executable, models and command line as beta.4 are slower on this machine for an environmental reason to find (driver, power plan, background process), and the note says so; no code is suspected because none of the timed step changed. About 3 minutes of GPU.
2. **Tauri bump.** `tauri` and its plugins to the newest 2.x, `cargo update`; the `glib` version in `Cargo.lock` must leave the alert's range (Dependabot alert #1; `glib` 0.18.5 today, `Cargo.lock:1914`), else `cargo update -p glib` to the fixed minor; the `cargo update` diff reported; every check of section 13; one manual start of the app. `set_overlay_icon` exists from 2.12.4, so chunk V does not depend on this bump; the two chunks touch `apps/desktop/src-tauri/Cargo.toml` in disjoint lines (versions against the `windows` feature list), U landing first.
3. **LICENSES.md** crate table regenerated with the `cargo metadata` command at `LICENSES.md:66` after the bump; `ort` and `tokenizers` appear; the date changes.
4. **Disk and download figures** agree across README (lines 49, 84, 159), docs/INSTALL.md (lines 35 to 40, 174), docs/04 and the setup screen: 1.0 GB of default models, 1.4 GB while downloading and 1.3 GB after with the CUDA runtime (INSTALL's measured figures of 28 Oct). If chunk R lands they change again (+406 MB for CUDA) and R owns that edit; U lands first.

Owns: `apps/desktop/src-tauri/Cargo.toml` (versions), `Cargo.toml` (workspace dependency versions if any), `Cargo.lock`, `LICENSES.md` (the crate table and date), README.md and docs/INSTALL.md (figures only), `docs/eval/<date>-n35-retime.md`, `docs/eval/2026-10-27-beta5-release.md` (one sentence in "Speed"). Must not touch `crates/**`, `apps/desktop/src/**`.

### 10.2 Waits for beta.7

Process again without a kept live transcript (names from the current transcript, the raw microphone transcribed for the echo step; docs/KNOWN-ISSUES.md:56): a feature with a GPU acceptance; it does not serve the accuracy priority and no chunk here needs it. Live punctuation, the `on_issues` default, Parakeet Unified EN (after R), the F32 diarizer, the Transcribe product path, the name source (after L), the code-switching measurement (after NVIDIA republishes), a gain stage (after N2's report), a phase ensemble (after N2's band).

## 11. Chunks, order and shared files

### 11.1 Contents

| # | Chunk | Why in beta.6 | GPU | Starts |
|---|---|---|---|---|
| V | Recording indicator, option B (or the owner's pick) | The owner's ask; UI only | none | when the owner picks |
| Q | Punctuation pin v1.1.1, upstream LICENSE and NOTICE, snapshot, docs | Upstream fixed the one real gap of plan 16 section 12.9 | none | now |
| U | 3.5 re-time, Tauri bump (`glib`), LICENSES.md counts, disk figures | Plan 17 section 10; the owed re-time | 3 min | now |
| N1 | Gap characterisation and the gap-fill measurement from cached runs | The direct attack on deletions; free | none | now |
| N2 | Phase control on v0.2.0; input-gain report | The deletion noise band R needs; a cheap lever check | 40 min | first idle window |
| N3 | The second file pass in the app | Only if 3.3's rule passes | full set, about 45 min for the fused config | after N1 passes |
| M2 | Chunk M continued with the fixed form and the labelled bar | Plan 17 section 10; the owner's words outrank | a few min (fixtures) | now (code and CPU replays); the owner's labels when the list exists |
| L | Names spike on the surviving EN file (brief) | Plan 16 section 12.4; the one unmeasured lever on names | 30 min | any idle window |
| R | Runtime 0.3.0 | The owner's ask, gated by section 4 | about 2 h 10 min to 2 h 40 min (4.4.4) | the tag exists (30 Oct); after 4.4 is in and N2's tooling is on R's branch |
| T | Transcribe 30B spike (brief; owner's call) | Section 7 | none (CPU, 1.5 to 3 h, idle machine) | after R's decision, if the owner says yes |
| H | Release: version, CHANGELOG, KNOWN-ISSUES, plan 19 checklist, benchmarks.json, eval tables from a fresh run of the rebuilt `diarist-cli` | Every release | full set once, about 45 to 65 min | last |

Not in beta.6: see 10.2, and section 6 (code-switching), section 7 (any Transcribe product work).

### 11.2 Order

```
now:          V (after the pick)   Q   U   N1 (CPU)   M2 (code, CPU replays)
idle GPU:     U's re-time (3 min)  N2 (40 min)  L (30 min)  M2's fixtures (minutes)
rule passes:  N3 (code, then its full-set run)   M2's labels (owner, 10 minutes) and table
by 6 Nov:     R starts only if v0.3.0 is tagged; its measurements (2.5 h GPU); endpointing variant
after R:      T (owner's call; CPU only, idle machine)
last:         H (fresh full-set run, both models, shipped configs; the checklist)
```

GPU time is the scarce thing and the owner's use of the PC comes first (the heavy-runs rule): every GPU item queues through the shared queue with quiet.lock for anything timed, and nothing heavy runs while the owner's jobs use the machine. In order of value per GPU minute: U's re-time, N2, M2's fixtures, L, then N3's and R's full-set runs, then H's.

### 11.3 Shared files, in landing order

`apps/desktop/src-tauri/Cargo.toml` (U's versions, then V's `windows` features; disjoint lines, re-read first); `crates/core/src/live/postpass.rs` (M2 inside `band_after_canceller`, then N3's file-pass call; disjoint regions, append-only); `crates/core/src/models/manifest.rs` (Q's test hashes, then R's constant and tests); `manifests/models.json` (Q only); `manifests/engine.json` (R only); `tools/eval/configs.toml` and `run.py` (N1's fused configs and `gap_fill` key, N2's phase configs, R's `*_030` configs, in that order; append-only); README.md and docs/INSTALL.md figures (U, then R); `docs/KNOWN-ISSUES.md` (Q's punctuation rows, M2's echo row, N3's gap-fill row, R's runtime rows; H collects); CHANGELOG.md (H only, from each chunk's hand-over). Every later agent re-reads before editing and commits with `git commit -- <paths>`.

## 12. D9: the release shape

- **Freeze** when V, Q and U have merged and every measured chunk has either merged or been closed by its rule: N3 merged or N1 closed; M2 merged or closed; L's note written. With an idle machine for the GPU items this is about one week, so the freeze is expected around Friday 6 Nov 2026, the same day the runtime gate closes.
- **v0.3.0 was tagged on 30 Oct,** so R runs its 2 h 10 min to 2 h 40 min of GPU (4.4.4) and lands or does not by the bars of 4.4, about one idle-GPU day with the queueing; the freeze moves to R's decision.
- **H** then runs the full set once from the rebuilt `diarist-cli` (both models, shipped configs, every bar of docs/eval/2026-10-27-beta5-release.md "Release bars" against beta.5's numbers: pooled cpWER per model not worse, no session worse by more than 0.5 unless the words show why, no new gap of 5 s or more, speed within 5 percent for the same measurement, with N3's second pass reported separately), writes `docs/eval/<date>-beta6-release.md` and `docs/plans/19-release-beta6-checklist.md` on plan 17's pattern (build, PDB, draft, publish, `updater/latest.json`, the update from beta.5 on this machine; no schema change is planned, so no new `diarist.db.bak`), and the owner publishes.
- **The CHANGELOG entry** states, in the owner's plain words: the recording indicator; the second pass if it ships, with its measured gain and cost; the echo rule if it ships; the runtime if it ships, with its size; the punctuation model's pin and the authors' licence files; the re-timing result.
- **Beta.7** opens with whichever of these did not land (the runtime first if it was the tag), the items of 10.2, and the Transcribe design if T's bars held.

## 13. Checks for every chunk, and the standing rules

| Check | Command | When |
|---|---|---|
| Format | `cargo fmt --all -- --check` | every commit |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | every commit |
| Tests and TypeScript bindings | `cargo test --workspace` (regenerates `apps/desktop/src/lib/generated/`, committed) | every commit |
| Front end | `cd apps/desktop && bun run check && bun run test && bun run build` | V, N3 (Settings and details line), U (after the bump) |
| Eval gate | `tools/eval`: `run.py --config <c> --sessions all` on an idle GPU, the table in `docs/eval/<date>-<config>.md`; punctuation and the loop collapse at the app's defaults as in the beta.5 release check | N3, R, H |
| Live | `cargo test -p diarist-core --test live_ws_gpu`, `live_carry_repro`, `echo_sample` | R; M2 (`echo_sample`, `echo_fleurs`, `aec_postpass`) |
| Rust equals Python | the Rust fill reproduces the measurement's fused words on every session | N3 |

Standing rules, as plan 16 section 7.2: one agent per chunk, disjoint files, small commits on the chunk's branch, no bare `git stash`, no private data in committed files (the privacy scan of plan 17 section 11 before every push; never `tools/eval/sessions.toml`, `.work`, run folders or audio), no GPU jobs or heavy builds while the owner's jobs use the machine, never the owner's name or email in any web request (User-Agent `diarist/<version>`), measurement rules quoted with their numbers in the eval note before the verdict, and a bar is never moved after its numbers are seen (section 5.1's amendment is made here, before M2's run, and section 4.4's on 31 Oct, before any 0.3.0 run; there are no others).

## 14. Decisions only the owner can make

| Decision | Options | Recommendation | If no answer |
|---|---|---|---|
| Recording indicator | A (dot overlay), B (pill overlay, ticking title, tray dot, in-app pill), C (swap the window icon); B without the title tick is a valid pick | **B.** It is the only option that reads at 200 percent on a dark auto-hidden taskbar and also speaks to screen readers | V waits; nothing else does |
| Transcribe 30B spike | Run it or not; now (after R) or beta.7; source (a) NVIDIA's checkpoint converted here, 67.5 GB download and about 155 GB of disk, or (b) a third-party IQ4_NL repository, about 22 GB | **Yes, after R's decision, on an idle machine, from (b), with section 7.2's bars; nothing ships from it.** File the upstream request for expert offload now; it is free and it is what would make 10 to 16 GB GPUs workable | Not run; beta.7 asks again |
| PR #46's 120 s repro zip | Download and run on the PC, or not | **No.** Unstated rights, and without a re-converted 3.5 file it decides nothing (section 6) | Not downloaded |
| A separate repository for model files (`ebt55/diarist-models`) | Create now; create at the first file that needs hosting; never | **In principle yes, created at the first file that needs it.** Beta.6 as planned hosts no new file (section 8 fetches upstream's); the first candidates are a re-converted EN file if spike L passes, or Transcribe files if T's bars hold. `models-2026-10` stays on `ebt55/diarist` forever because beta.5's manifest points at it | Nothing changes |
| Chunk M2's listening step | Label at most ten kept words on meeting 20 (and 21 if touched) from the audio, before the result table is written; or decline | **Yes.** Ten minutes; it is the only way the bar can see the difference between your words and echo (section 5.1) | The proxy bar stands; M stays out |
| The runtime gate date | Friday 6 Nov 2026, or another date | **6 Nov.** One week, the expected freeze; later holds the release for an event nobody controls | 6 Nov |
| Code-switching re-conversion now | Re-convert 3.5 and run research/21 section 3.3, or wait | **Wait** for NVIDIA's republished GGUF or a user report (section 6) | Wait |
| Gap-fill default (if 3.3's gain is between 0.2 and 0.3 points) | Pre-registered: `off` by default, offered in Settings | Already decided by rule 4 of 3.3; listed so the owner can veto either way | The rule applies |
