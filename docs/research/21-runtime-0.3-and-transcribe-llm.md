# Runtime 0.3 and Nemotron 3.5 Transcribe 30B: research for beta.6 (30 Oct 2026)

Research only, for the beta.6 plan. Opus 5.5 research agent, worktree `claude/beta6-research`. No code changed and no GPU used. Nothing was downloaded beyond API listings, model cards, licence files and small source and doc files read through `gh api` or plain HTTP to huggingface.co with the User-Agent `diarist-research`. Every web source was read on 30 Oct 2026 between 07:00 and 08:45 UTC. Sources are numbered [S1] to [S44] at the end. **Inference** marks our reading or estimate, not something a source says. **Measured** marks a count made on this PC from files already on disk.

Upstream state when read: NeMo-Speech.cpp `main` at `d8a1abf` (29 Oct 2026, 22:15 UTC), 13 commits after `v0.2.0` (`6a3ca36`) [S3].

**Summary.**

- **Runtime `main` needs no Diarist code change on the paths we use.** The JSON `words[]` fields, the flags and presets we pass, and the realtime event names and fields are all unchanged. Four behaviours change underneath:
  - word times are clamped to the input;
  - speakers are read at the encoder-frame centre;
  - live interims arrive once per decoded chunk, not once per push;
  - `audio_processed` now reports decoded progress, so it trails the received audio by the lookahead (0.48 s at our live lookahead 6).
  
  Diarist's adapters cope with all four (section 1.2). Adopting the runtime still means editing the manifest, `TESTED_ENGINE_VERSIONS` and the size figures in the docs: the Windows CUDA archive grows from 102.2 MB to 508.6 MB, because it now carries NVIDIA's cuBLAS and cuBLASLt instead of the in-tree shim.
- **PR #87 fixes the speaker half of our issue #81, not the deleted-word half.**
  - Its AMI table puts main at +0.66 (EN) and +0.64 (3.5) cpWER over v0.2.0. Our gap on ES2002a was +0.7 and +0.9. The fixed branch is within 0.1 of v0.2.0.
  - It also says transcripts are identical to main. So the extra deletions we measured on main (+3.3 % with EN, +1.0 % with 3.5 on 324 minutes) should still be there. #87 puts the remaining difference down to #46's 70 ms encoder phase.
  - #87 was validated on Linux only [S1].
- **There is no v0.3.0 tag, milestone or discussion yet.**
  - PR #84 set `VERSION` to 0.3.0 at 22:15 UTC on 29 Oct. For 0.2.0 the tag followed the version bump within minutes. This time there is none 9.6 hours later.
  - A release-packaging PR (#88) is still open. **Inference:** the tag waits for it.
  - No date is published anywhere.
- **The nightly cannot ship and does not yet contain #87.**
  - The published `nightly` is still built from `66d50d9` (28 Oct).
  - Today's scheduled run at `d8a1abf` has built the Windows CUDA archive. Publishing waits for the Windows GPU smoke test, which was queued from 06:38 UTC and started at 08:10 UTC. At 08:14 UTC it was still running.
  - Each nightly publish deletes and recreates the release, so the bytes behind the URL change and a SHA-256 pin cannot hold [S9, S12, S13].
- **Nemotron 3.5 Transcribe 30B-A4B uses the OpenMDW-1.1 licence,** the same one as Nemotron 3.5 ASR and Nemotron 3 Diarization, which Diarist already downloads at run time. It is permissive: anyone who redistributes it must keep the licence and the notices, patent suits end the grant, and outputs are unrestricted. The repository is not gated [S31, S32].
- **Its published accuracy is far better than our 0.6B models on the same public benchmarks.** Its cpWER is about half: AMI distant microphone 17.3 against 30.5 (EN) and 35.9 (3.5); NOTSOFAR-1 single channel 25.9 against 53.4 and 54.0; CALLHOME 11.5 against 23.3 and 24.9. Subsets and normalisers differ. AMI, ICSI and AliMeeting are in its training mix (section 2.2).
- **It does not run on this PC's GPU in NeMo-Speech.cpp.** The runtime puts every decoder layer on the GPU and has no option to offload experts. The smallest decoder NVIDIA's converter makes, `iq4_nl`, is about 20.35 GB, plus a 1.67 GB encoder file (third-party measurements; NVIDIA publishes no GGUF). It can run CPU-only. **Inference:** that needs about 23 to 24 GB of RAM and takes roughly 30 to 60 minutes per hour of audio, against about 45 seconds for today's file pass.
- **Code-switching (PR #46's author):**
  - Of our public sets, only Indic DiarBench Hindi has real English code-switching: 10.6 % of its reference tokens are English (**Measured**).
  - AliMeeting is Mandarin with almost no English: 0.77 Latin-script words per 1,000 characters.
  - SUMM-RE is French with loanwords and brand names, about 0.4 to 0.9 % of tokens.
  - The re-converted files from research/19 were never run on any of them.

**Recommendation.**

- **(a) Runtime.** Adopt v0.3.0 in beta.6 only if NVIDIA tags it in time, and only from the tagged archive. Do not ship the nightly or a build of our own. Measure five things first (section 1.7):
  1. The deleted-word delta on the gate set and the broad set, with a phase control: v0.2.0 with the first 70 ms trimmed and with a few offsets of leading silence. This tells us whether main's +3.3 % is real or chunk-phase noise.
  2. AMI cpWER and WDER back at v0.2.0's level.
  3. research/08's WebSocket fixtures re-recorded, plus live first-word latency, two sessions and the echo replay at lookahead 6.
  4. The 508.6 MB archive's extracted size, peak VRAM and speed on driver 610.88.
  5. Our tests with the new manifest.

  The expected gain is speed (main ran 102 against 81 × real time in research/19) and staying current. No accuracy gain is expected on our path, so the bar is "no accuracy loss". If no tag arrives in time, stay on v0.2.0.
- **(b) Transcribe 30B.** A spike is worth it in its cheapest form. It is the largest accuracy lever on paper and accuracy is the owner's first priority, but it is not a beta.6 feature. The cheapest form:
  - **When:** after the runtime decision, on an idle machine.
  - **What:** CPU-only (`--device cpu`), IQ4_NL decoder.
  - **Clips:** first a 3-minute clip (peak RAM, tokens per second), then AMI ES2002a and one of the owner's Zoom sessions. The Zoom audio stays on the PC.
  - **Compare:** against the shipped file pass with the eval harness.
  - **Cost:** about 22 GB of model files. The owner chooses the source: a conversion of NVIDIA's own 67.5 GB checkpoint, or a third-party pre-converted repository.
  
  If the gain on the owner's recordings is large, the product options are an opt-in slow pass for machines with 32 GB or more of RAM, a GPU pass for 24 GB cards, and a request to NVIDIA for expert offload. Expert offload would make 10 to 16 GB GPUs workable.

## 1. Runtime `main` after PR #87

### 1.1 What landed since v0.2.0

| Commit | Date | PR | What | Touches Diarist's paths? |
|---|---|---|---|---|
| `b809bbb`, `e59dfb0`, `083861c` | 3 to 13 Oct | #43, #65, #66 | TTS fixes | No |
| `8642eaa` | 7 Oct | #62 | Diarization front end: less redundant work | Yes; #87's bisect finds output identical to v0.2.0 [S1] |
| `e37a54a` | 16 Oct | #69 | Build URLs for OpenFST and Sparrowhawk | No |
| `caa3f47` | 16 Oct | #70 | Diarizer keeps frames for words not yet tagged (fixes our #67) | Yes; identical to v0.2.0 on #87's sets [S1] |
| `2bdfb03` | 20 Oct | #47 | mmap-backed ASR weight loading | Yes (loading); identical output [S1, S20] |
| `1a7e945` | 24 Oct | #46 | Streaming startup geometry and Q8 pointwise conversion | **Yes: changes the transcripts** [S1, S19] |
| `e680fc3` | 24 Oct | #76 | Licence-header check | No (the build our issue #81 tested) |
| `6a62501` | 28 Oct | #82 | SALM speech-LLM transcription (167 files: CLI, server, release workflow, packaging) | Yes (CLI, server, packaging) [S25] |
| `66d50d9` | 28 Oct | #85 | SALM: no decode for audio without a voice | No; the latest nightly is built from it |
| `c540e21` | 29 Oct | #87 | Speaker attribution, word times inside the audio, speaker labels in text, SRT and VTT, interim cadence | **Yes** [S1, S2] |
| `d8a1abf` | 29 Oct | #84 | SALM docs for Transcribe 30B; `VERSION` 0.2.0 to 0.3.0 | Version only [S14] |

### 1.2 Each change against what Diarist uses

Diarist's file adapter is `crates/core/src/engine/nemo_cli.rs` and its live adapter is `crates/core/src/engine/live_ws/mod.rs`. The contract is research/08.

| # | Area | Change on `main` | Source | Diarist code change? |
|---|---|---|---|---|
| 1 | JSON `words[]` | Field names and format unchanged: `word`, `start`, `end`, `confidence`, and `speaker` (1-based, left out when untagged). Top-level keys unchanged: `file`, `text`, `confidence`, `duration`, `languages`, `words` | [S4] | None |
| 2 | Word times | Clamped so that 0 ≤ start ≤ end ≤ input duration. Words flushed at the end of the stream keep their text and sit on the last encoder frame of real audio (before: up to 1.3 s past the end at R13). The 80 ms grid is unchanged | [S1, S7] | None. Our end-of-track clamp (research/08 pitfall 11) becomes a no-op and can stay |
| 3 | Speakers | For causal encoders (both Nemotron streaming models), attribution reads the diarizer at the frame centre, 70 ms before the reported word time. Word times themselves are unchanged. #87 reports speaker accuracy at speaker changes up 2 to 7 points | [S1, S2] | None; measure cpWER and WDER |
| 4 | Words themselves | #46 makes the first chunk of a stream a shorter causal chunk, which shifts every later chunk boundary and the endpointer's clock. #87 says WER and transcripts equal main's, and that the residual difference from v0.2.0 is #46's encoder phase: dropping the first 70 ms of input gives v0.2.0's WER (24.29 against 24.30) | [S1, S6, S19] | None in code. **The accuracy risk to measure** (1.3, 1.7) |
| 5 | `--stream --endpointing` flags | Everything we pass exists with the same meaning: `--stream`, `--endpointing`, `--asr.streaming.rnnt_right_context`, `--diarize`, `--diar-model`, `--asr.diar.preset` (`v3-offline` still listed), `--device`, `--json`, `--output`, `--force` and `--language`. Diarizer presets were only added to | [S3, S4] | None |
| 6 | Exit codes | A failed input now exits with its own error's code (2 invalid argument, 3 missing model, 4 unsupported feature) instead of always 1. New error types `resource_exhausted` and `unavailable` exit 1 | [S4, S8] | None. `map_failure` already maps 2, 3 and 4, so bad inputs are classified better |
| 7 | Input decoding | A named file is decoded by its content (WAV, FLAC, MP3) and resampled to 16 kHz mono. Local files have no length limit; HTTP uploads are capped at 3,600 s | [S4, S8, S26] | None: we pass 16 kHz mono WAV |
| 8 | Weight loading | mmap (#47). CUDA copies the weights to the device and is unaffected; CPU and Vulkan bind them to the mapped file | [S20] | None. **Inference:** the data-folder move copies first and deletes after a restart, so a mapped file is never deleted while in use |
| 9 | Speed | Not a contract change. research/19 measured main at 102 × real time against 81 for a v0.2.0 built with the same toolchain (EN, gate set) | research/19 | Optional: `EXPECTED_REALTIME_FACTOR` (26) only drives the progress estimate |
| 10 | Text, SRT, VTT | Speaker labels in text, SRT and VTT output (#86) | [S1] | None: Diarist reads JSON only |
| 11 | Live protocol | Unchanged: `/v1/audio/transcriptions/realtime`, Bearer key, `session.update`, binary PCM16, `…transcription.delta` {`delta`, `audio_processed`}, `…transcription.completed` {`transcript`, `words[]`, `audio_processed`}, `input_audio_buffer.commit` / `.committed`, and the 512 MiB per-socket audio limit | [S5] | None |
| 12 | Interim cadence | The stream now yields an interim only when `audio_processed` advances, and on the cache-aware runner it advances only when a chunk is decoded. So there is one delta per decoded chunk rather than one per 100 ms push. **Inference:** at live lookahead 6 a chunk is (1 + 6) × 80 = 560 ms. The many empty deltas research/08 saw become rare | [S1, S6] | None: empty deltas were already ignored |
| 13 | `audio_processed` | Now decoded progress: it leaves out audio held as lookahead (0.48 s at R6) and equals the input duration on the end-of-stream result. Before, the cache-aware runner reported all received audio | [S1, S6, S7] | None expected. The adapter spreads partial words up to `audio_processed`, so partials now land about 0.48 s earlier, which is closer to the speech. `TextFinalUntil` uses max(`audio_processed`, last word end), so it is never before the last final word. Re-record `docs/research/fixtures/ws_session_events.jsonl` (documentation) and re-run the live tests |
| 14 | Error events | Carry `type` and `code` as well (for example `streaming_unsupported` when a SALM model is loaded) | [S5] | None: we read `error.message` |
| 15 | Live speakers | The same frame-centre attribution as row 3 | [S2] | None |
| 16 | New options | `--instruction`, `--translate-to` (now without an NMT build), `--salm-encoder`, `--asr.salm.*`, `http.chat.*`; `doctor` gains `features.speechlm` | [S4, S25, S26] | None (additions) |
| 17 | Version | `VERSION` 0.3.0; nightly binaries report `0.3.0+nightly.<sha>` | [S11, S14] | Yes: `manifests/engine.json`, `TESTED_ENGINE_VERSIONS` in `crates/core/src/models/manifest.rs`, and the package-name tests in `live_ws/supervisor.rs` |
| 18 | Windows CUDA package | cuBLAS and cuBLASLt are now NVIDIA's own (package-release.ps1 copies them from the toolkit and adds the CUDA EULA) instead of the in-tree shim. The toolkit is CUDA 13.0, as before, and the arch list `75-real;80-virtual;86-real;89-real;120a-real` is unchanged, so the 3080's sm_86 has native code. The `server` profile now also builds SALM with patched llama.cpp. A new `windows-aarch64-cuda` archive appears | [S9, S10, S12] | Yes: size and SHA-256 in the manifest, the download and disk figures in INSTALL and the app (nightly: 508,605,526 against 102,183,465 bytes for CUDA, 22,408,905 against 19,302,464 for Vulkan, 8,686,073 against 5,528,418 for CPU). The engine licence field should mention the CUDA EULA. **Inference:** Diarist downloads the archive from NVIDIA's release, so it does not redistribute cuBLAS itself |
| 19 | Validation | #87 tested Linux x86-64 only. The release's Windows CUDA smoke test runs on an L4 GPU, blocks only tagged releases, and does not test SALM | [S1, S11, S41] | Our Windows run is the only Windows validation |

**Not changed and worth knowing.**

- **WebSocket drops under load.** cpp-httplib 0.50.1 rejects a frame whose header straddles the server's 4 KiB read buffer, so a busy server can drop a live socket without a close frame. Reported in #72; the fix, PR #73, is open and was updated on 29 Oct [S22]. It is in v0.2.0 too.
- **Concurrent sessions on Vulkan.** #89 reports that two or more realtime sessions with ASR batching on Vulkan give no output or abort. It was reported on v0.2.0, so it may affect Diarist's Vulkan package, which runs mic and loopback as two sessions [S23]. Our CUDA two-session measurement (research/08 fixtures) is not affected.
- **#83 is closed as inconclusive.** It reported a #46 English regression on GB10; the reporter later found the differences changed sign with the lookahead [S21].
- **#80 is open.** It reports "pre_ln transformer variant is not supported" with Nemotron-3-Diarization at e680fc3 [S24]. Our own e680fc3 build ran that model on 324 minutes in research/19, so this is not ours (**Inference**).

### 1.3 PR #87's validation against our issue #81

#87's setting is ours: CUDA, published q8_0 models, Nemotron-3-Diarization, `--stream --endpointing --asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline`, Linux x86-64 [S1].

| Measure | #87 (main 66d50d9 → branch; v0.2.0) | Ours, issue #81 (Windows, main e680fc3) [S18] |
|---|---|---|
| AMI cpWER, EN | Mix-Headset (16 / 6 sessions, as the table gives them): 25.78 → 25.11; v0.2.0 25.12. Main was +0.66, the branch −0.01 | ES2002a: v0.2.0 18.0, main 18.7 (+0.7) |
| AMI cpWER, 3.5 | 31.42 → 30.69; v0.2.0 30.78. Main +0.64, the branch −0.09 | 20.7 → 21.6 (+0.9) |
| Other cpWER rows, branch minus main | AMI SDM −0.48 / −0.49, CH109 −0.59 / −0.83, NOTSOFAR-1 single channel −0.40 / −0.65, multi-headset −0.49 / −0.50, AliMeeting −0.11 (EN / 3.5) | – |
| Plain WER | Identical between main and the branch on every row | +0.2 points main against v0.2.0 (EN 16.6 → 16.8, 3.5 19.1 → 19.3) |
| Deleted words | Not reported. Transcripts are identical to main | AMI flat (339 → 341, 396 → 392). 324 min of Zoom plus AMI: +3.3 % EN (1,396 → 1,442), +1.0 % 3.5 (1,879 → 1,897), gaps of 2 s or more 152 → 165 and 176 → 187 |
| Words ending past the audio | EN 27 → 0, 3.5 125 → 0, over these sets plus CALLHOME 2, DIHARD III and Mixer 6 | Not measured |

**What this says (Inference).**

- Our AMI cpWER rise matches #87's main-against-v0.2.0 gap in sign and size. The branch puts AMI back at v0.2.0's level, so ES2002a should return to about 18.0 and 20.7.
- The deleted-word half of #81 is untouched, because the words are identical to main.
- #87 traces the remaining difference to a 70 ms phase shift: on AMI, main with its first 70 ms dropped reproduces v0.2.0's WER. If that holds on our Zoom recordings too, the +3.3 % is a chunk-boundary lottery rather than a regression. Only a phase control can show it (1.7).

The issue's three questions (expected behaviour, bisect help, release plan) got no maintainer comment on #81 itself; #87 closed it [S18].

### 1.4 When will v0.3.0 be tagged?

- **No sign of a date.**
  - Tags are `v0.1.0`, `v0.2.0` and `nightly` [S12].
  - The repository has no milestones, and discussions are disabled [S16].
  - A search for 0.3.0 finds only #82 and #84 [S16].
- **The version is set.** #84 (merged 29 Oct, 22:15 UTC) changed `VERSION` to 0.3.0 [S14]. The release process is: set `VERSION`, merge, push the matching tag, approve, review the draft [S11].
- **Precedent.** #58 ("release: bump version to 0.2.0", with the release CI) merged on 2 Oct at 13:49 UTC, and the v0.2.0 release was created at 13:53 UTC [S12, S15]. This time no tag exists 9.6 hours after the bump (checked 07:53 UTC, 30 Oct).
- **Still open.** PR #88 (29 Oct) puts NVIDIA's cuBLAS in the Linux archives and the container "as the Windows archives already do", and changes the release Dockerfile and packaging [S17]. **Inference:** the maintainers are finishing release packaging, and the tag may follow #88. A tagged release also waits for the Windows L4 smoke test, which today's nightly is stuck on (1.5).
- **The workflow.** It is unchanged in shape: a `v*` tag builds a draft release that a maintainer reviews and publishes [S9, S11].

### 1.5 Does the nightly now build Windows CUDA assets with c540e21?

- **Published nightly.** Built from `66d50d9` (28 Oct, before #87) by a manual run on 28 Oct (19:45 to 20:35 UTC), with all 14 archives. `nemo-speech-nightly-windows-x86_64-cuda.zip` is 508,605,526 bytes, SHA-256 `5ec1c429…98912` [S12, S13].
- **Today's scheduled run** (41011983696, 06:04 UTC, head `d8a1abf`, which contains `c540e21`):
  - Every build job succeeded, including "Windows x86_64 cuda" (06:05 to 06:27 UTC).
  - The job "Windows x86_64 CUDA archive (L4)" was queued from 06:38 UTC and started at 08:10 UTC.
  - Publishing needs that job to finish. It may fail on a nightly, but it must complete.
  - So at 08:14 UTC the nightly with #87 was built but not published; it will probably be published once the job ends (**Inference**) [S9, S13].
- **The two scheduled runs before it** (27 and 28 Oct, head e680fc3) never started a job and were cancelled. The current nightly came from the manual run [S13].
- **Why it can't be pinned.**
  - Publishing creates a staging release, deletes `nightly` with its tag, and promotes the staging release.
  - It runs whenever `main` has moved since the last nightly [S9, S11].
  - The URL stays and the bytes change, so Diarist's URL plus SHA-256 pin would break at the next publish.
  - The workflow artifacts of a run are not a public release asset.
  - Shipping a binary built by us would mean hosting runtime binaries ourselves: possible under Apache-2.0 plus the CUDA EULA for cuBLAS, but a new responsibility (**Inference**).

### 1.6 What a runtime upgrade changes in Diarist

- `manifests/engine.json`: the three Windows packages (URL, SHA-256, size; name `nemo-speech-0.3.0-windows-x86_64-<backend>`, the same pattern `releases.rs` parses) and the licence note for cuBLAS.
- `TESTED_ENGINE_VERSIONS` (`crates/core/src/models/manifest.rs`), the package names in the `live_ws/supervisor.rs` tests, and any fixture that names `0.2.0`.
- Docs: install size (the beta.5 docs say 1.3 GB on disk; the CUDA zip alone grows by 406 MB), KNOWN-ISSUES row for #67 (fixed in the release), research/08 sections 4 and 6 (new `audio_processed` meaning, rarer empty deltas, words inside the audio, exit codes), and the WebSocket fixtures.
- Optional: `EXPECTED_REALTIME_FACTOR`, after measuring.

### 1.7 What to measure before adopting

The adoption rule in plan 17 ("research/08's fixtures, the full set and the live latency all hold; check #67 and #81 first") still fits. Concretely, on the tagged Windows CUDA archive:

1. **Deleted words with a phase control.**
   - Run the gate set (324 min) and the broad set (36 sessions, 1,697 min), both models, v0.2.0 against 0.3.0: deletions, gaps of 2 s and 5 s or more, AMI cpWER.
   - Phase control: v0.2.0 on the gate set with the first 70 ms trimmed, and with 20 and 40 ms of leading silence.
   - If v0.2.0's own deletions move by about ±3 % across these offsets, main's +3.3 % is phase noise, not a regression.
   - Bar: deletions no higher than v0.2.0 beyond that band. About 6 runs per model on 324 min, roughly 30 minutes of GPU (**Inference**, at 80 × real time).
2. **Speakers.** AMI ES2002a cpWER at most v0.2.0 + 0.1 for both models. WDER and cpWER on the public meeting groups (`pub-meeting`, `pub-far`) at least as good.
3. **Live.**
   - Re-record research/08's WebSocket fixtures and check every event the adapter reads.
   - Run `cargo test` for `live_ws`.
   - First-word latency at R6: median at most 1.0 s and p90 at most 1.5 s (the live-lookahead rule). The interim cadence change can move it.
   - Two sessions under the 2 GB budget.
   - The echo replay of plan 15 within 0.5 points.
4. **Package.**
   - Record the archive's SHA-256, its extracted size, peak VRAM of the file pass and of the server (real cuBLAS may use more than the shim did), and × real time.
   - Check that it runs on driver 610.88.
   - research/19 found that a v0.2.0 built with real cuBLAS (CUDA 12.8) differs from the shim release by 8 to 61 word edits per recording, so expect small word differences from the cuBLAS switch alone.

## 2. Nemotron 3.5 Transcribe 30B-A4B for Diarist

### 2.1 Licence

- **Name.** The repository's `LICENSE` is the "OpenMDW License Agreement, version 1.1 (OpenMDW-1.1)", copyright 2026 NVIDIA. The card metadata says `license: other`, `license_name: openmdw-1.1` [S31, S32]. The card says the model is ready for commercial and non-commercial use [S31].
- **The same licence Diarist already accepts.** `manifests/models.json` lists Nemotron 3.5 ASR and Nemotron 3 Diarization as OpenMDW-1.1. The 0.6B card confirms it for 3.5 [S39].

The terms that matter, paraphrased from `LICENSE` [S32]:

- **Grant.** Free of charge, to deal in the model materials without restriction, under copyright, patent, database and trade-secret rights. There is no field-of-use limit, no acceptable-use policy and no attribution requirement in the product.
- **Redistribution.** Whoever distributes any portion must include a copy of the agreement and the applicable copyright and origin notices.
- **Patent clause.** Suing anyone for patent or copyright infringement by the model ends the grant, unless the suit answers one brought first.
- **Outputs.** No restriction on outputs.
- **Liability.** As is, with no warranty or liability. The user is responsible for clearing any third-party rights.
- **Third-party notices.** `THIRD_PARTY_NOTICES.md` covers two Apache-2.0 code snippets in the card (Qwen3-ASR's loop-cleanup function, a vLLM patch), not the weights [S33].
- **Access.** Not gated on Hugging Face [S31].

**For a free MIT app that downloads models at run time (Inference):**

- Downloading from NVIDIA's repository leaves the obligations with the user, as for today's two OpenMDW models.
- If Diarist or the owner hosted a converted GGUF, which is needed because NVIDIA publishes none (2.4), that repository must carry `LICENSE` and the notices.

### 2.2 Languages and published accuracy

**Languages.** The card covers 40 languages, each with a quality level [S31]:
- **Leading:** English (US, India, UK), French, German, Hindi, Italian, Korean, Polish, Portuguese, Russian, Slovak, Spanish, Czech and Dutch.
- **Production-ready:** Japanese, Thai, Turkish, Ukrainian and eleven European languages.
- **Supported:** Arabic, Mandarin, Hebrew, Greek and others.

Every language with ASR is marked for code-switching. Norwegian has no ASR mark. Multi-speaker ASR is marked for 13 languages, including English, French, German, Hindi, Japanese and Mandarin.

All numbers below are published, not measured by us. They are on different subsets and normalisers, so treat the differences as indicative. Transcribe's numbers are from full recordings [S31]. The 0.6B numbers come from #87's branch with our flags [S1], and the baselines from `docs/eval/2026-10-27-baseline-public.md`.

**Multi-speaker (cpWER, %; Mandarin cpCER).**

| Benchmark | Transcribe 30B | Nemotron EN 0.6B | Nemotron 3.5 0.6B | Our shipped baseline (v0.2.0) |
|---|---|---|---|---|
| AMI, headsets | IHM 10.29 | Mix-Headset 25.11 | 30.69 | EN2002a-d Mix-Headset: EN 31.7, 3.5 35.6. ES2002a: 18.0, 20.7 |
| AMI, distant microphone | SDM 17.34 | 30.50 | 35.86 | EN2002a-d Array1-01: 38.6, 42.6 |
| CALLHOME CH109 | 11.50 | 23.28 | 24.88 | – |
| NOTSOFAR-1, single channel | 25.91 | 53.44 | 53.96 | 32 eval-small meetings: 45.4, 45.5 |
| NOTSOFAR-1, headsets | IHM 14.94 | multi-headset 48.92 | 53.45 | – |
| Mixer 6 | 14.80 | – | – | – |
| AliMeeting (cpCER) | 34.95 | – | Test, 4 sessions: 41.30 | Eval headset mix 36.6, array 42.0 |
| AISHELL-4 (cpCER) | 32.91 | – | – | – |

The card's headline is a mean cpWER of 15.80 over AMI, CALLHOME, Mixer 6 and NOTSOFAR-1 full sessions, against 16.83 for the next best published system, DiCoW v3.3 with DiariZen [S31]. **Inference:** "IHM" for a single-input full recording is most likely the headsets' mix, close to our Mix-Headset.

**Single-speaker, long-form and code-switching (WER %, CER for zh, ja, ko).** The 0.6B column is NVIDIA's FLEURS result at a 1.12 s chunk with the language given [S39].

| Benchmark | Transcribe 30B | Nemotron 3.5 0.6B |
|---|---|---|
| FLEURS en | 3.35 | 7.91 (our own measurement: 8.77, research/19) |
| FLEURS fr / de / es / it | 3.16 / 2.73 / 2.20 / 1.41 | 9.03 / 8.31 / 4.11 / 4.25 |
| FLEURS pt / ru / nl / pl | 3.01 / 3.80 / 3.75 / 3.81 | 5.48 / 9.17 / 11.46 / 15.15 |
| FLEURS hi | 7.47 | 6.81 (the one language where the 0.6B's number is lower) |
| FLEURS zh / ja / ko | 8.62 / 4.42 / 2.78 | 19.28 / 11.48 / 7.12 |
| Open ASR Leaderboard, 9 English sets (mean) | 4.71 (AMI 7.81, Earnings-22 subset 5.29) | – |
| Long-form, in 55-min chunks with 5 s overlap: Earnings-21 / Earnings-22 / CORAAL | 7.57 / 11.99 / 13.39 | Our 8 Earnings-22 calls: EN 11.0, 3.5 13.4 (different subset and normaliser) |
| Code-switching: ASCEND zh–en (MER) / MUCS hi–en / MUCS bn–en | 18.74 / 19.28 / 35.46 | – |

**Independence of our test sets.**
- The training mix lists AMI, ICSI, AliMeeting, AISHELL-4 and DiPCo among its public sources (splits not stated), plus vendor data including MLC-SLM-2 [S31].
- So for this model, our AMI, ICSI and AliMeeting sets may not be held out.
- NOTSOFAR-1, Earnings-22, SUMM-RE, Indic DiarBench and the owner's Zoom recordings are not listed. **Inference:** they are the cleaner tests.

**Inference.** On meetings the model makes about half the speaker-attributed errors of our 0.6B models on paper, which is the gap that matters for Diarist. On single-speaker English phone calls the gap may be small: Earnings-22 is about 12 against our 11 to 13. Deletions are 81 to 82 % of our English errors (public baseline), and whether the LLM drops fewer words in overlap is unmeasured.

### 2.3 Word timestamps and speaker labels

- **Two steps, in the runtime as on vLLM.** The model first writes the transcript. Then the encoder file's CTC timestamp head aligns each word with the audio, and its speaker branch (the Nemotron-3-Diarization encoder, about 99M parameters) tells up to eight voices apart. Chinese, Japanese and Thai are timed per character [S26, S31].
- **Times are an alignment of generated text.** They are not decoder emission frames like Nemotron streaming's. **Inference:** they should not carry the roughly 0.3 s lag of research/08 pitfall 11, but this is unmeasured.
- **Speakers come from tags in the text.** The transcript's speaker tags (`<spk:0>` …) are mapped to the speaker branch's speakers. The vLLM output also returns the diarization segments, the tag-to-speaker map and an alignment-error field [S31].
- **Quality is not published.** The card lists word-level timestamp accuracy among its metrics but publishes no number, and it advises using the diarization segments, not the word times, to evaluate diarization [S31]. The best published evidence of speaker quality is the cpWER table in 2.2.
- **What Diarist would have to handle (from the runtime docs [S26]).**
  - `--diarize` sends a speaker prompt that asks for lowercase text without punctuation, with numbers as spoken. For punctuated text with speakers, pass the speaker-tag instruction yourself (`--instruction`, with tags up to `<spk:7>` for more than four speakers) and ask for word times.
  - If the alignment fails, the transcript comes back without word times rather than with invented ones, so the pass needs a fallback.
  - Non-speech stretches such as music or noise can come out as invented text. The docs advise trimming them.
  - The card recommends cleaning generation loops (a repeated block collapsed to one copy) [S31]; the runtime has a `salm_guards` module, which we did not read.
  - Phrase boosting is not supported. Names go into the instruction ("Context terms: …"), which the card shows lowering Earnings-22 WER from 12.54 to 9.58 [S26, S31].

### 2.4 The GGUF route and its sizes

- **Conversion route (runtime `docs/salm`, `docs/model-conversion.md` [S27, S29]).**
  1. Download the BF16 checkpoint: `model.safetensors` is 67,496,013,734 bytes, 67.5 GB with the other files [S31].
  2. Run `convert_model.py <checkpoint> --architecture salm --outtype <type>`.
  3. The converter writes a BF16 intermediate the size of the BF16 decoder, then quantizes it with a patched `llama-quantize`, which it builds on first use.
  4. Output: a decoder GGUF (a standard llama.cpp NemotronH GGUF that names its encoder) and `<checkpoint>-Encoder-F16.gguf`. The encoder file holds the F16 speech encoder, the speaker branch and the F32 CTC head, and it is shared by every decoder type.
  5. `--salm-encoder none` writes a second decoder type without rewriting the encoder.
- **Outtypes.**
  - `nvfp4` (the default): NVFP4 routed experts, BF16 dense linears, for Blackwell.
  - `iq4_nl`: IQ4_NL experts, Q8_0 dense linears, for "Ampere, Ada, CPU, Vulkan".
  - Embeddings, the output head and the norms stay BF16 or F32 in both [S29].
- **Sizes.** The docs give none. Third-party measurements of NVIDIA's converter output:

  | File | Size | Source |
  |---|---|---|
  | `iq4_nl` decoder | 20.35 GB | AtomicChat [S36] |
  | `nvfp4` decoder | 21.79 GB | AtomicChat [S36] |
  | BF16 decoder | 65.85 GB | AtomicChat [S36] |
  | IQ4_NL decoder with the same tensor policy | 20,370,389,728 bytes | Jmica [S37] |
  | Encoder file | 1,670,410,080 bytes | Jmica [S37] |

  **Inference:** a cross-check from the backbone config [S34] gives the same `iq4_nl` size. It has 23 MoE layers of 128 experts with 2 × 2,688 × 1,856 weights each (plus the MTP layer's), about 30.7B routed parameters at 4.5 bits, about 17.3 GB. About 1.5B dense parameters at Q8_0 are about 1.6 GB, and the embeddings and head at BF16 about 1.4 GB. Total: about 20.3 GB.
- **Disk for a local conversion (Inference).** About 67.5 + 65.9 + 20.4 + 1.7, roughly 155 GB at the peak.
- **No NVIDIA GGUF exists.**
  - NVIDIA's repositories hold the BF16 safetensors, FP8 (about 32 GB) and NVFP4 (about 19 GB) safetensors for vLLM and Transformers, and the DFlash2 drafter (1.14 GB) [S35, S40].
  - The runtime's model index has no Transcribe entry [S42].
- **Third-party GGUFs** (all OpenMDW-1.1, created 28 to 30 Oct) [S35, S36, S37, S38]:
  - AtomicChat: for NeMo-Speech.cpp, seven decoders of 13.57 to 35.00 GB plus the encoder and DFlash2. It reports every decoder of 15.55 GB and up within ±0.18 points of BF16 on 2,200 utterances.
  - AMAImedia: one of the same AtomicChat-named files.
  - Jmica: for audio.cpp.
  - sakasegawa: for speech.cpp, its own layout, unreadable by NeMo-Speech.cpp.

### 2.5 Windows CUDA build and Ampere

- **The code paths are there on Windows and Ampere.**
  - The release's Windows CUDA job builds the `server` profile, which now includes SALM and the patched llama.cpp, with CUDA 13.0 and sm_86 native code [S9].
  - The batched CUDA path needs a GPU of compute capability 8.0 or newer and CUDA 12.8 or newer. The RTX 3080 is 8.6, so it qualifies [S27, S30].
  - `iq4_nl` is the documented decoder type for it [S27, S29].
  - The CPU and Vulkan builds run upstream llama.cpp's path with IQ4_NL, without speculative decoding [S27].
- **NVIDIA has not validated that combination.**
  - The card's compatibility table lists native Windows with GGML only for the RTX 5090 and RTX PRO 6000 (Blackwell, NVFP4). Its only Ampere entry is the A100 with vLLM or Transformers [S31].
  - The release smoke test does not run SALM [S41].
  - **Inference:** SALM on Windows with Ampere is supported by the code but unvalidated.

### 2.6 Memory and speed on this PC

- **GPU.** No, in NeMo-Speech.cpp as it stands.
  - The SALM engine loads every decoder layer on the GPU: `n_gpu_layers = 999` with `split_mode none` whenever a GPU is selected. It uses a tensor override only to put the token embeddings on the GPU, and exposes no expert offload [S30].
  - The runtime's smallest documented GPU configuration is 24 GB: IQ4_NL, 4 concurrent requests, at most 29 minutes per request, MTP off. The default needs 32 GB for one hour [S28].
  - The decoder alone is about 20.35 GB, so a 10 GB RTX 3080 cannot load it (**Inference**). Even AtomicChat's 13.57 GB `AD-Q2_0_M` needs about 15.6 GiB at its tightest settings [S36].
- **CPU only (`--device cpu`).** Possible in principle on 31.9 GB of RAM, but tight. **Inference**, from the sizes and [S28]:

  | Item | Estimate |
  |---|---|
  | Decoder | 20.35 GB |
  | Encoder | 1.67 GB |
  | KV cache, one 1-hour request | 0.5 GiB |
  | Recurrent state | about 48 MiB per slot |
  | Encoder output | about 134 KB per second of audio, 0.5 GB per hour |
  | Compute buffers | 1 GB or so |
  | **Total** | **about 23 to 24 GB** |

  With Windows and the app this leaves little headroom, and other heavy programs would have to be closed.
- **Speed (Inference, unmeasured; the reasoning, so it can be checked).**

  | Stage | Basis | Estimate |
  |---|---|---|
  | Decode | Each token reads about 3.1 GB of weights: 6 of 128 experts in 23 layers at IQ4_NL (0.78 GB), the Q8_0 dense layers (1.6 GB) and the BF16 output head (0.7 GB). At 35 to 45 GB/s of effective DDR4 bandwidth that caps decoding at about 12 to 15 tokens per second, so 6 to 10 in practice | – |
  | Transcript of a 1-hour meeting | About 2.5 words per second at about 1.3 tokens per word, roughly 11,000 to 14,000 tokens (the runtime budgets up to 6 per second [S28]) | 20 to 40 min of decoding |
  | Prefill of the audio | 45,000 tokens at 12.5 per second of audio [S28], compute-bound on 12 Zen 3 cores | 8 to 17 min |
  | Encoder | 0.8B parameters in F16 | 3 to 6 min |
  | **Total per hour of audio** | – | **30 to 60 min**, all cores busy, against about 45 s for today's file pass (80 × real time) |

- **A third-party data point** (speech.cpp, not NeMo-Speech.cpp) [S38]. On an RTX 2080 8 GB with an i9-9900K, the experts of 18 of 23 layers ran on the CPU. A 98-second meeting took 46 s, using 6.1 GiB of GPU memory and 13 GB of RAM. So hybrid offload makes 8 to 10 GB GPUs workable at about 2 × real time on short clips.
- **If NeMo-Speech.cpp offered expert offload (Inference).**
  - The GPU would hold the dense layers, embeddings, head, encoder and caches, about 5 to 7 GB, and the 17 GB of experts would stay in RAM.
  - Decoding would read only about 0.8 GB per token from RAM, roughly 20 to 40 tokens per second.
  - A 1-hour meeting might take 10 to 20 minutes on this PC.
  - It would take an upstream request (the owner's call to file).

### 2.7 Recording length and longer meetings

- **Limit: one hour per request.**
  - The model is trained on recordings of up to one hour with a 65,536-token context, and the card does not promise anything beyond that [S31].
  - The runtime takes 20 ms to one hour per request (`max_context` 73,728 tokens by default) [S26, S28].
  - The 24 GB GPU configuration caps a request at 29 minutes [S28].
- **Splitting longer meetings (Inference, a design suggestion).**
  - Split the recording at silences into parts of at most 55 minutes (29 on a 24 GB GPU), overlapping by about 5 s. NVIDIA's own long-form evaluation used 55-minute chunks with 5 s of overlap [S31].
  - Keep each word from the part where it is furthest from an edge.
  - Do not stitch SALM's per-part speaker labels; they are per request. Re-tag the words from Diarist's own full-file diarization: the file pass already has the Nemotron 3 speakers, and `fusion::assign_words` already assigns words to diarization segments. That keeps one speaker namespace across a 2-hour meeting.

### 2.8 Gain against cost for an optional "high-accuracy" pass after Stop

| | Gain | Cost |
|---|---|---|
| Accuracy | On paper about half the cpWER of our 0.6B models on meeting benchmarks (2.2), with code-switching, 40 languages, and names from an instruction | Unmeasured on the owner's recordings. Three of our public sets may be in its training data. Invented text in non-speech, generation loops, and alignment failures need guards and fallbacks |
| Output | Its own punctuation and casing (no separate punctuation model for that pass); verbatim or clean style by instruction | Speaker prompts change the style (2.3). Speaker labels are per request (2.7) |
| Speed | – | This PC: CPU only, about 30 to 60 min per recorded hour (**Inference**), against about 45 s today. A 24 GB GPU: probably minutes per hour (**Inference**), but each request capped at 29 min |
| Download and disk | – | About 22 GB (IQ4_NL decoder and encoder), against 0.7 GB for each of today's ASR models. No NVIDIA GGUF: Diarist would host its own conversion (allowed with `LICENSE` and notices), point at a third-party repository (supply-chain risk), or ask users to convert 67.5 GB with Python (not a consumer path) |
| Machine | – | At least 24 GB of VRAM (≤ 29-min parts) or 32 GB (one hour) for the GPU path, or at least 32 GB of RAM for CPU only. The 10 to 16 GB GPUs most Diarist users have cannot hold it in this runtime |
| Runtime | It needs the same 0.3.0 runtime | The runtime upgrade of section 1 comes first |

### 2.9 Spike options on this PC

1. **CPU-only, short clips. The cheapest one that answers the open question.**
   - Get the IQ4_NL decoder and encoder (about 22 GB). The owner picks the source:
     - convert NVIDIA's checkpoint: 67.5 GB download, about 155 GB of disk at the peak, CPU Python;
     - or take a third-party pre-converted repository such as AtomicChat's NeMo-Speech.cpp files, an unverified source parsed by the runtime.
   - Runtime: 0.3.0 when tagged, the nightly for development only, or a local build of `c540e21` or later (research/19 built main in about 6 minutes).
   - Run `--device cpu` on an idle machine (heavy-run rule):
     - a 3-minute public clip, for peak RAM and tokens per second;
     - AMI ES2002a (21 min);
     - one of the owner's Zoom sessions (stays on the PC).
   - Score with the eval harness against the shipped file pass.
   - **Inference:** about 1.5 to 3 hours of CPU in all.
2. **Nothing without a download answers it.** NVIDIA's hosted demo could take public clips, but that uploads audio (the owner's call), and it cannot touch the private recordings, where the question lies.
3. **Upstream.** Ask NVIDIA for MoE expert offload in SALM, which llama.cpp already supports through tensor overrides (2.6). Owner's call.

## 3. The code-switching point from PR #46's author

### 3.1 Their case against ours

The author's reproducer [S19]:
- **Recording.** A 120-second technical presentation, mainly Mandarin with English terms and names. The omitted passage was a Mandarin job introduction.
- **Settings.** Language auto-detected, lookahead 3, Metal, endpointing and VAD off.
- **Result.** Making only the 48 pointwise tensors F16 kept the passage. The 17 windows in the PR came from that one recording, and they claim no general WER gain.
- **Their suggestion.** The language mix may matter, so evaluate non-English and code-switched speech.

Ours (research/19): English meetings (36 sessions, about 28 h) and FLEURS English. Language given, lookahead 13 (file) and 1 (live), CUDA, endpointing on. **The re-converted files were never run on AliMeeting, SUMM-RE or Indic DiarBench** (research/19 sections 0 and 7; the public sets were built the same day, research/20).

### 3.2 Which public sets test it

**Measured** on 30 Oct 2026 from the local references (`tools/eval/.work/public/<set>/refs/*.seglst.jsonl` in the `beta5-data` worktree), with a script in the scratchpad:

| Set | What we counted | Result | Tests code-switching? |
|---|---|---|---|
| Indic DiarBench Hindi (20 clips, 221 min) | Tokens in Latin script in `words_mixed`, the reference's code-mixed form with English in Roman script (research/20 section 5) | 5,419 of 51,081 tokens (10.6 %); per clip 2.4 to 19.1 %; 18 to 82 % of turns contain at least one English word. Most frequent: time, job, internet, but, work, office, private, from, phone, home | **Yes**, Hindi–English as spoken in conversation |
| AliMeeting Eval, headset mix (8 meetings, 252 min) | Latin-letter runs inside the Mandarin turns | 62 runs in 80,943 Han characters (0.77 per 1,000): app 16, v 12, logo 8, ok 6, single letters | No: near-monolingual Mandarin (it does test "non-English") |
| SUMM-RE ASRU (10 meetings, 213 min) | Matches against a fixed list of about 150 common English words and anglicisms (a lower bound) | 419 of 48,332 tokens (0.9 %). Without "ok" and "oh", which French shares, about 207 (0.4 %): facebook 28, instagram 15, marketing 14, cool 10, netflix 9, week-end 8, youtube 8; full English function words almost absent (the 5, you 5, and 2) | No: French with loanwords and brand names |

research/20's smoke run also found that Nemotron 3.5 writes English loanwords in Devanagari on Hindi (0 of 460 words in Latin script), so Indic is scored against the Devanagari `words` by default.

### 3.3 The smallest measurement that settles it

- **What is compared.** Nemotron 3.5 only. Three files already on disk (research/19's folder `tools/eval/.work/respin/models/` in the `diarist-echo-copyright-b63b18` worktree), so nothing needs converting:
  - the shipped q8_0;
  - the re-converted `q8_0-pwf16`;
  - the old-policy drift control `q8_0-pwq8`, whose spread is the noise band.
- **Runtime.** The shipped v0.2.0, which isolates the conversion as research/19 did.
- **Arm A, the app's settings.**
  - `--language hi` and `zh-CN`, lookahead 13, `v3-offline`, `--stream --endpointing --diarize`.
  - On Indic DiarBench Hindi (221 min) and the AliMeeting headset mix (252 min).
  - 3 files × 473 min is about 18 minutes of GPU at the measured 80 × real time (**Inference**).
- **Arm B, the author's regime** (only on Indic, or both sets if A is flat and the owner wants it). No `--language`, lookahead 3, `--stream` without `--endpointing`, no `--diarize` (that path has the #67 tagging bug in v0.2.0), WER only. About 8 minutes of GPU on Indic.
- **Metrics.**
  - WER (CER for Mandarin) and its deletions; for Indic also WER against `words_mixed`.
  - Gaps of 2 s and 5 s or more.
  - Word edits per 1,000 words against the shipped file, beside the control's edits.
- **Decision rule (suggestion).** Re-convert 3.5 for Diarist only if pwf16's deletions and WER improve beyond the control's spread, in the same direction on most clips (for example 14 of 20 on Indic or 6 of 8 on AliMeeting). Otherwise close the question for both models.
- **Optional, the owner's call.** The author's 120-second Mandarin–English clip, through the three files at their settings, would show whether their omission reproduces on our CUDA build. It is a third-party recording with unstated rights, so if used it stays on the PC and out of the repository. It was not downloaded.

### 3.4 Side note: NVIDIA has not republished the ASR GGUFs

Checked through the Hugging Face API [S43]:
- `nvidia/nemotron-speech-streaming-en-0.6b` is unchanged since 5 Aug 2026.
- `nvidia/nemotron-3.5-asr-streaming-0.6b`'s head (`ea30d66`, 10 Sep 2026) holds a q8_0 of 742,090,464 bytes, the size of research/19's old-policy conversion, not the 812,869,344 bytes of a pointwise-F16 one. Our manifest pins the earlier revision `1c8deae`.
- So research/19's "take NVIDIA's republished files when they appear" is still waiting.

## 4. Blocks and limits

- **Not read.**
  - The technical report PDF, which sits on research.nvidia.com: the request rule names only GitHub and Hugging Face. Word-timing and long-form details may be in it. The owner can allow it.
  - The openmdw.ai licence page. The `LICENSE` file in the Hugging Face repository was read instead [S32].
  - The 120 s reproducer zip on PR #46 (the owner decides).
- **Not downloaded.** No runtime archive or workflow artifact, so the extracted size, the DLL list and the VRAM of the 0.3.0 package are unknown.
- **Still moving.** The Windows L4 smoke job of today's nightly was still running at 08:14 UTC. Re-check `gh api repos/NVIDIA/NeMo-Speech.cpp/releases/tags/nightly` (its `target_commitish` should become `d8a1abf` or later) before relying on 1.5.
- **Estimates.** All speed and memory figures for Transcribe on this PC are reasoned, not measured. Third-party file sizes and accuracy deltas are the publishers' own.

## Sources

All read 30 Oct 2026.

- [S1] PR #87, body with changelog, behaviour changes and validation table: https://github.com/NVIDIA/NeMo-Speech.cpp/pull/87
- [S2] Commit `c540e21`, patches to `recognizer.cpp`, `runner.cpp`, `runner.h`, `types.h`, `fastconformer.h`, `model.h`, `decoder.h`: https://github.com/NVIDIA/NeMo-Speech.cpp/commit/c540e21167017d2e34444f2fc2d70649065dc500
- [S3] Compare `v0.2.0...main` (13 commits, file list): https://github.com/NVIDIA/NeMo-Speech.cpp/compare/v0.2.0...d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14
- [S4] `app/transcribe.cpp` at `d8a1abf` against `v0.2.0` (options, JSON renderer, exit codes): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/app/transcribe.cpp
- [S5] `server/http/http_server.cpp` at `d8a1abf` against `v0.2.0` (realtime handler, error events, SALM refusal): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/server/http/http_server.cpp
- [S6] `src/asr/recognizer.cpp` and `src/asr/runner.cpp` at `d8a1abf` against `v0.2.0` (`next()` interim rule, decoded progress, first-chunk geometry, word bounds): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/src/asr/runner.cpp
- [S7] `docs/asr/configuration.md`, section "Word timestamps and progress": https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/asr/configuration.md
- [S8] `app/cli_util.cpp` (error kinds, inputs decoded by content) and `src/common/audio_ingest.h` (local files without a length limit, uploads 3,600 s): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/app/cli_util.cpp
- [S9] `.github/workflows/release.yml` at `d8a1abf` against `v0.2.0` (triggers, nightly logic, Windows matrix and CUDA arch, publish dependencies, nightly replacement): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/.github/workflows/release.yml
- [S10] `scripts/windows/package-release.ps1` and `scripts/windows/build.ps1` against `v0.2.0` (cuBLAS bundling, CUDA EULA, `server` profile with SALM): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/scripts/windows/package-release.ps1
- [S11] `docs/development/releasing.md` (cutting a release, nightly, smoke-test coverage): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/development/releasing.md
- [S12] Releases API (nightly target `66d50d9`, published 28 Oct 20:35 UTC; asset names, sizes and digests; v0.2.0 created 2 Oct 13:53 UTC) and tags: https://api.github.com/repos/NVIDIA/NeMo-Speech.cpp/releases
- [S13] Release workflow runs and jobs: runs 41011983696 (scheduled, `d8a1abf`, Windows L4 job queued 06:38 to 08:10 UTC, then running), 40757118465 (manual, `66d50d9`, published), 40641687248 and 40459115412 (scheduled, cancelled, no jobs): https://github.com/NVIDIA/NeMo-Speech.cpp/actions/workflows/release.yml
- [S14] PR #84 (SALM docs; `VERSION` 0.2.0 → 0.3.0, merged 29 Oct 22:15 UTC): https://github.com/NVIDIA/NeMo-Speech.cpp/pull/84
- [S15] PR #58 ("release: bump version to 0.2.0", merged 2 Oct 13:49 UTC): https://github.com/NVIDIA/NeMo-Speech.cpp/pull/58
- [S16] Milestones (none), repository settings (`has_discussions` false), issue search for "0.3.0": https://api.github.com/repos/NVIDIA/NeMo-Speech.cpp/milestones?state=all
- [S17] PR #88 (open; NVIDIA cuBLAS in Linux archives and the container): https://github.com/NVIDIA/NeMo-Speech.cpp/pull/88
- [S18] Issue #81 (ours; closed by #87 on 29 Oct, no comments): https://github.com/NVIDIA/NeMo-Speech.cpp/issues/81
- [S19] PR #46 thread, including the author's reply of 26 Oct: https://github.com/NVIDIA/NeMo-Speech.cpp/pull/46#issuecomment-6429083932
- [S20] PR #47 (mmap loading; CUDA unaffected): https://github.com/NVIDIA/NeMo-Speech.cpp/pull/47
- [S21] Issue #83 (closed, inconclusive): https://github.com/NVIDIA/NeMo-Speech.cpp/issues/83
- [S22] Issue #72 and PR #73 (WebSocket frames dropped under load; open): https://github.com/NVIDIA/NeMo-Speech.cpp/issues/72
- [S23] Issue #89 (Vulkan, concurrent realtime sessions with batching): https://github.com/NVIDIA/NeMo-Speech.cpp/issues/89
- [S24] Issue #80 (Nemotron-3-Diarization "pre_ln" error report): https://github.com/NVIDIA/NeMo-Speech.cpp/issues/80
- [S25] PR #82 (SALM; file list, `app/doctor.cpp`, `include/nemo_speech/asr.h`): https://github.com/NVIDIA/NeMo-Speech.cpp/pull/82
- [S26] `docs/salm/README.md`: https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/salm/README.md
- [S27] `docs/salm/models.md` (outtype per hardware, platform paths): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/salm/models.md
- [S28] `docs/salm/configuration.md` (keys, GPU memory table, tokens per second, recurrent state): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/salm/configuration.md
- [S29] `docs/model-conversion.md`, section "Speech-LLM (SALM)": https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/docs/model-conversion.md
- [S30] `src/speechlm/salm_engine.cpp` and `src/speechlm/CMakeLists.txt` (fast path conditions, `n_gpu_layers = 999`, tensor override for embeddings only): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/src/speechlm/salm_engine.cpp
- [S31] Model card and API listing of `nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B` at revision `c17667641642` (files and sizes, parameters, languages, benchmarks, training data, timestamps, limits, hardware table): https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B
- [S32] `LICENSE` at the same revision (OpenMDW-1.1): https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B/blob/c17667641642c4265ff2b7037dd101b7d4567634/LICENSE
- [S33] `THIRD_PARTY_NOTICES.md` at the same revision: https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B/blob/c17667641642c4265ff2b7037dd101b7d4567634/THIRD_PARTY_NOTICES.md
- [S34] `llm_backbone/config.json` at the same revision (layer types, experts, dimensions): https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B/blob/c17667641642c4265ff2b7037dd101b7d4567634/llm_backbone/config.json
- [S35] Hugging Face search for Nemotron-3.5-Transcribe (23 repositories): https://huggingface.co/api/models?search=Nemotron-3.5-Transcribe
- [S36] AtomicChat GGUF card and file sizes (NeMo-Speech.cpp format; sizes of NVIDIA's own conversions; accuracy deltas; 16 GB settings): https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF
- [S37] Jmica GGUF card (audio.cpp; byte sizes of IQ4_NL decoder and encoder): https://huggingface.co/Jmica/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF
- [S38] sakasegawa GGUF card (speech.cpp; RTX 2080 offload figures): https://huggingface.co/sakasegawa/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF
- [S39] Model card of `nvidia/nemotron-3.5-asr-streaming-0.6b` (OpenMDW-1.1; FLEURS by chunk size): https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b
- [S40] API listings of NVIDIA's FP8, NVFP4 and DFlash2 repositories: https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-NVFP4, https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-FP8, https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-DFlash2
- [S41] `tests/ci/model_smoke.py` at `d8a1abf` (ASR, diarization and TTS only): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/tests/ci/model_smoke.py
- [S42] `models/index.json` at `d8a1abf` (no Transcribe entry): https://github.com/NVIDIA/NeMo-Speech.cpp/blob/d8a1abf2ea9d11e8f98ad0e10cc2c12ef1cb3a14/models/index.json
- [S43] Hugging Face API listings of `nvidia/nemotron-speech-streaming-en-0.6b`, `nvidia/nemotron-3.5-asr-streaming-0.6b` and `nvidia/Nemotron-3-Diarization` (heads, GGUF sizes)
- [S44] In this repository: `crates/core/src/engine/live_ws/mod.rs`, `crates/core/src/engine/nemo_cli.rs`, `crates/core/src/engine/live_ws/supervisor.rs`, `manifests/engine.json`, `manifests/models.json`, `docs/research/08-runtime-cli-contract.md`, `docs/research/19-q8-pointwise-respin.md`, `docs/research/20-public-eval-data.md`, `docs/eval/2026-10-27-baseline-public.md`, `docs/eval/2026-10-27-live-lookahead.md`, `docs/plans/17-release-beta5-checklist.md`
