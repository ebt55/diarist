# Q8 pointwise re-spin: re-converted Nemotron GGUFs on our setup (26 Oct 2026)

Spike 1 (and spike 2 as a 2 × 2) of `docs/research/17-model-landscape-2026-10.md` section 7: re-convert both Nemotron checkpoints with NeMo-Speech.cpp's fixed converter (PR #46, which keeps the 48 Conformer pointwise convolutions in F16 inside the q8_0 preset) and measure them against NVIDIA's published q8_0 files on the shipped runtime, with the app's file-pass and live settings. No product code changed. Opus 5.5 measurement agent.

**Summary.** The fixed converter does what PR #46 says. The 48 pointwise weights become F16, everything else quantized stays byte-identical to NVIDIA's files, the files grow by 68 MiB, and they load and run in the shipped 0.2.0 runtime. On our data the change makes no measurable difference, for either model.

- **All 36 sessions, 1,697 min.** Fixed against shipped:
  - EN: deletions 6,253 to 6,273, gaps of 2 s or more 826 to 830, gaps of 5 s or more 65 to 67.
  - Nemotron 3.5: deletions 8,737 to 8,723, gaps 889 to 889, 5 s gaps 88 to 86.
- **AMI cpWER, the only human-referenced number.** EN 18.0 to 17.8, 3.5 20.7 to 20.8; f16 gives 17.6 and 20.5.
- **Live setting and FLEURS English.** The same picture: FLEURS for 3.5 is 8.77 % shipped and 8.77 % fixed, with identical error counts.
- **Why this is not a hidden effect.** The runtime is deterministic: re-runs are identical word for word. The fix changes 2.3 to 6.5 words per 1,000, no more than a control that changes only a positional table by float rounding (2.1 to 5.1 per 1,000). Those edits go in both directions, and no gap appears or disappears beyond that control's band.
- **Speed and VRAM.** No speed cost; peak VRAM about +95 MiB (f16: about +575 MiB).
- **Runtime built from main (2 × 2, gate set).** It adds about 3 % deletions and 10 to 15 more gaps on the Zoom sessions with either GGUF. AMI deletions stay flat and AMI cpWER rises 0.7 to 0.9 points through speaker attribution. A v0.2.0 built locally with the same toolchain matches the release, so the change comes from main's code, not the build.
- **Startup fix.** Words in the first 10 s change by at most 2.

So the owner's "many word errors sometimes" is not explained by the Q8 pointwise quantization on these recordings. About 830 to 890 stretches of 2 s or more where Zoom has words and we have none are identical across shipped, fixed and f16. They are the lever to look at next (section 8).

**Recommendation:** do not self-host re-converted files for accuracy, and do not move to `main`. Take NVIDIA's republished GGUFs when they appear: a harmless manifest bump, +68 MiB, and it embeds the tokenizer word boosting needs. Re-run this gate and broad set, with the gap count, against the next tagged runtime before adopting it.

## 0. Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88, 24 logical CPUs. Idle desktop on the GPU about 0.7 to 0.8 GB and 3 to 34 % utilisation (the Claude desktop app). Other agents shared the PC; every GPU job held a shared `gpu.lock` and every timed run also a `quiet.lock` (no other heavy work started meanwhile); nvidia-smi was recorded before each run |
| Runtime | nemo-speech 0.2.0, `nemo-speech-0.2.0-windows-x86_64-cuda`: a byte copy of the app's installed engine (`%LOCALAPPDATA%\app.diarist.desktop\engine`, 79 files, every SHA-256 equal; `nemo-speech.exe` `d9527ade…3272`). The app's data folder was only read |
| diarist-cli | 0.1.0-beta.4, built from this tree (`fc597ba`), SHA-256 `d736a0ae99eb074f…` |
| Shipped GGUFs | NVIDIA's q8_0 files at the manifest revisions, copied from the app's models folder; SHA-256 equal to `manifests/models.json` |
| Converter | NVIDIA/NeMo-Speech.cpp `main` at `e680fc30ac74` (24 Oct 2026, after PR #46 `1a7e945`), `convert_model.py`, CPU only (`CUDA_VISIBLE_DEVICES=-1`), CPython 3.12.9 venv with the clone's `requirements.txt` (torch 2.14.1+cpu, gguf, transformers 5.5.1) |
| File setting | What the app passes for file jobs and the post-pass: `--stream --endpointing --diarize`, lookahead 13 (`--asr.streaming.rnnt_right_context 13`), `--asr.diar.preset v3-offline`. The shipped files get both from the manifest; the re-converted files get the lookahead from a `<file>.meet.json` sidecar `{"file_right_context": 13}` (research/17 pitfall 2) and run with the same manifest diarizer. Every run.json was checked: lookahead 13, preset v3-offline, and the intended ASR file |
| Live setting | Lookahead 1 and `v3-streaming`, passed as engine args after diarist's own (the runtime's parameter parser applies a repeated key in order, so the last value wins; `src/common/parameter_parser.cpp`) |
| Harness | `tools/eval/run.py` with a scratch `--configs-file` (configs named `rs_*`, so the cache key pitfall of research/17 cannot bite), `levers.py diff`, and a scratch `analyze.py` for plain WER, the gap count and first-10-s words (proposed for levers.py in section 7) |
| Sets | The established set of docs/05 (Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14, AMI ES2002a; 324 min) and, reported separately, seven new Zoom webinar recordings (Talk 1 to Talk 7, 37 to 67 min each, 356 min, 2 to 5 Zoom speakers, group `talks`). FLEURS English slice (`fleurs_fetch.py`, 55 utterances) for Nemotron 3.5 |
| Metrics | As in research/12: AMI cpWER (word-level manual reference) is the word-accuracy number; Zoom cpWER and WER are against Zoom's own machine transcript, so they mix in Zoom's errors (docs/05). "WER" is speaker-agnostic: time-ordered normalised words, Levenshtein. Deletions, insertions and substitutions are given for both |

### The gap count

A gap is a stretch of at least 2 s with no hypothesis word in which the reference has at least 3 words. Hypothesis-empty stretches run from one hypothesis word's end to the next one's start. Reference words count when their midpoint lies inside the stretch shrunk by 0.5 s at both ends (Nemotron words run about 0.3 s late, research/08 pitfall 11; Zoom word times are interpolated inside a cue). On AMI only the scored region counts. Reported: gaps, reference words inside them, gaps of at least 5 s and the longest. On a Zoom reference some gaps are Zoom's own insertions or a pause inside a long cue, but those are the same for every config, so differences between configs are the signal.

## 1. Conversion and header check

Sources, downloaded with `curl.exe` from the pinned revisions (both repos are public and not gated); SHA-256 equal to the Hugging Face LFS ids:

| Source | Size, bytes | SHA-256 |
|---|---|---|
| `nvidia/nemotron-speech-streaming-en-0.6b` @ `ebe59e5a8171`, `nemotron-speech-streaming-en-0.6b.nemo` | 2,473,041,920 | `283638054c44f6794e74fe9af9048d78a6d9d6c058c12131856c7859a62ac9cd` |
| `nvidia/nemotron-3.5-asr-streaming-0.6b` @ `1c8deaecc64b`, `nemotron-3.5-asr-streaming-0.6b.nemo` | 2,368,284,501 | `210214ed94039bf6bfbb9a047c7fa289628db75b103e2bf6381fa78285436a74` |

Converted files (each conversion took 9 to 59 s on the CPU):

| File | Policy | Size, bytes | Δ vs NVIDIA q8_0 | SHA-256 |
|---|---|---|---|---|
| `nemotron-speech-streaming-en-0.6b.q8_0-pwf16.gguf` | q8_0, pointwise F16 (the fix) | 770,986,624 | +71,113,664 (+67.8 MiB) | `3d37c1c7e1fe8aea08389c188598895e90f3f8f33b34442db1d53209fac2a1da` |
| `nemotron-speech-streaming-en-0.6b.f16.gguf` | f16 control | 1,278,374,080 | +578,501,120 | `788f055e95dfd29e80198c0d6b6abe934f71736e164c74ab3b15345c2ee7a500` |
| `nemotron-speech-streaming-en-0.6b.q8_0-pwq8.gguf` | q8_0, pointwise Q8 (old policy, drift control) | 700,207,744 | +334,784 | `ca0031d60ed5096bf02c28030694b99af57f5f50c035af02a7bb1d82567ef19b` |
| `nemotron-3.5-asr-streaming-0.6b.q8_0-pwf16.gguf` | q8_0, pointwise F16 (the fix) | 812,869,344 | +71,320,992 (+68.0 MiB) | `f8ae511e816d5b5ee48a634f4c837b9daad2107023c1abae6e3fb098f53e18cf` |
| `nemotron-3.5-asr-streaming-0.6b.f16.gguf` | f16 control | 1,327,494,624 | +585,946,272 | `055a6ba4c2118248e76ec2efe67c5a6240de543e1ada15a6211e47c2b2cac2ff` |
| `nemotron-3.5-asr-streaming-0.6b.q8_0-pwq8.gguf` | q8_0, pointwise Q8 (old policy, drift control) | 742,090,464 | +542,112 | `aab14dce993f18e40548987f5e392ae854671ea0c2ca4b090a569ee4df4ae309` |

The old-policy control is today's converter with the PR #46 branch of `_pick_dtype` switched off (a monkeypatch in a wrapper script, nothing in the checkout edited). It exists to separate the pointwise change from any other difference between NVIDIA's August conversion and today's converter.

Header and tensor comparison (a stdlib GGUF reader; per-tensor SHA-256 of the data bytes):

| Comparison | Tensors | Types | Pointwise conv weights | Tensors with different bytes | Header keys |
|---|---|---|---|---|---|
| NVIDIA EN q8_0 `ebe59e5` | 653 | 272 Q8_0, 30 F16, 351 F32 | 48 of 48 Q8_0 (e.g. `encoder.layers.0.conv.pointwise_conv1.weight`, 1024 × 2048) | – | – |
| fixed EN q8_0 vs NVIDIA | 653 | 224 Q8_0, 78 F16, 351 F32 | **48 of 48 F16** | 49: the 48 pointwise (Q8_0 → F16) and `encoder.pos_enc.pe` | adds `asr.tokenizer.spm_model`; `general.name` differs |
| old-policy EN control vs NVIDIA | 653 | as NVIDIA | 48 Q8_0, byte-identical to NVIDIA's | 1: `encoder.pos_enc.pe` | same two |
| NVIDIA 3.5 q8_0 `1c8deae` | 657 | 272 Q8_0, 30 F16, 355 F32 | 48 of 48 Q8_0 | – | – |
| fixed 3.5 q8_0 vs NVIDIA | 657 | 224 Q8_0, 78 F16, 355 F32 | **48 of 48 F16** | 49: the 48 pointwise and `encoder.pos_enc.pe` | adds `asr.tokenizer.spm_model`; `general.name` differs |
| old-policy 3.5 control vs NVIDIA | 657 | as NVIDIA | 48 Q8_0, byte-identical | 1: `encoder.pos_enc.pe` | same two |

- The fix does exactly what PR #46 says: the 48 pointwise weights become F16 and nothing else quantized changes; every other Q8_0 and F16 tensor is byte-identical to NVIDIA's file. Size grows by 67.8 MiB (EN) and 68.0 MiB (3.5); the PR says 67.5 MiB.
- Two side differences come from today's converter, not from the fix: the positional-encoding table `encoder.pos_enc.pe` (F32, 1024 × 9999) differs in 399,200 of 10,238,976 values by at most 4.9e-4 (values up to 1; float rounding of the sines at large positions), and the files embed the SentencePiece tokenizer (`asr.tokenizer.spm_model`), which NVIDIA's August files lack (research/12 section 4; it is what word boosting needs). The old-policy control carries both and nothing else, so it measures their effect.
- `asr.encoder.train_right_ctx` is unchanged (13 for EN, 3 for 3.5) and the Q8 layout is `block_q8_0` in all files, so the 0.2.0 runtime reads them the same way.

## 2. Gate set, file pass, runtime 0.2.0

The established set (six user meetings and AMI, 324 min). Configs: `rs_en_shipped` / `rs_n35_shipped` (NVIDIA q8_0), `rs_*_q8pw` (fixed q8_0), `rs_*_f16`, `rs_*_pwq8` (old-policy control). "new / gone vs base" counts gaps of the config that overlap no gap of the shipped file, and shipped gaps that overlap none of the config's. Transcribe × RT and VRAM: the `nemo-speech transcribe` step, timed under quiet.lock; VRAM is the peak minus the reading just before each run.

Nemotron EN:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base | Transcribe × RT | VRAM Δ median / max, MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | rs_en_shipped | 7 | 324 | 16.7 | 3692 | 7.5 | 1396 | 1302 | 1037 | 152 (677) | 23 | base | base | 77 | 1460 / 1646 |
| gate | rs_en_q8pw | 7 | 324 | 16.7 | 3689 | 7.5 | 1399 | 1301 | 1030 | 153 (686) | 23 | 1 / 0 | 0 / 0 | 80 | 1556 / 1703 |
| gate | rs_en_f16 | 7 | 324 | 16.7 | 3683 | 7.5 | 1391 | 1301 | 1043 | 152 (669) | 22 | 3 / 3 | 0 / 0 | 80 | 2036 / 2044 |
| gate | rs_en_pwq8 | 7 | 324 | 16.7 | 3693 | 7.5 | 1402 | 1314 | 1041 | 152 (684) | 23 | 1 / 1 | 0 / 0 | 81 | 1474 / 1496 |

Nemotron 3.5:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base | Transcribe × RT | VRAM Δ median / max, MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | rs_n35_shipped | 7 | 324 | 16.8 | 4116 | 7.8 | 1879 | 812 | 1209 | 176 (732) | 30 | base | base | 83 | 1428 / 1468 |
| gate | rs_n35_q8pw | 7 | 324 | 16.8 | 4118 | 7.8 | 1879 | 813 | 1208 | 174 (724) | 29 | 2 / 4 | 0 / 1 | 83 | 1510 / 1573 |
| gate | rs_n35_f16 | 7 | 324 | 16.7 | 4110 | 7.8 | 1878 | 799 | 1202 | 174 (736) | 29 | 4 / 6 | 0 / 1 | 85 | 1998 / 2024 |
| gate | rs_n35_pwq8 | 7 | 324 | 16.8 | 4102 | 7.8 | 1866 | 816 | 1219 | 176 (731) | 29 | 2 / 2 | 0 / 1 | 82 | 1432 / 1460 |

Per session (cp counts are the cpWER alignment's substitutions / deletions / insertions; WER counts the speaker-agnostic ones):

| Session | Config | cpWER | cp sub / del / ins | WER | sub / del / ins | Hyp words | Gaps ≥ 2 s (ref words) | ≥ 5 s | Longest s | Transcribe s | × RT | VRAM Δ MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Aug 2 | rs_en_shipped | 9.5 | 162 / 280 / 203 | 9.1 | 164 / 266 / 189 | 6701 | 21 (89) | 1 | 7.8 | 36.7 | 71 | 1646 |
| Aug 2 | rs_en_q8pw | 9.4 | 163 / 277 / 199 | 9.1 | 165 / 264 / 186 | 6700 | 21 (88) | 1 | 7.8 | 33.6 | 78 | 1556 |
| Aug 2 | rs_en_f16 | 9.5 | 171 / 272 / 202 | 9.2 | 173 / 259 / 189 | 6708 | 20 (83) | 1 | 7.8 | 33.1 | 79 | 2038 |
| Aug 2 | rs_en_pwq8 | 9.5 | 164 / 277 / 200 | 9.1 | 166 / 264 / 187 | 6701 | 21 (88) | 1 | 7.8 | 33.0 | 79 | 1496 |
| Aug 2 | rs_n35_shipped | 10.3 | 203 / 366 / 130 | 10.1 | 199 / 361 / 125 | 6542 | 28 (131) | 5 | 7.6 | 31.8 | 82 | 1410 |
| Aug 2 | rs_n35_q8pw | 10.3 | 206 / 363 / 131 | 10.1 | 203 / 358 / 126 | 6546 | 27 (128) | 5 | 7.6 | 31.6 | 83 | 1484 |
| Aug 2 | rs_n35_f16 | 10.5 | 203 / 369 / 138 | 10.3 | 201 / 363 / 132 | 6547 | 27 (128) | 5 | 7.6 | 31.6 | 83 | 2008 |
| Aug 2 | rs_n35_pwq8 | 10.3 | 208 / 359 / 132 | 10.1 | 203 / 355 / 128 | 6551 | 28 (131) | 5 | 7.6 | 32.7 | 80 | 1422 |
| Aug 9 | rs_en_shipped | 8.1 | 132 / 177 / 220 | 7.6 | 128 / 162 / 205 | 6565 | 25 (99) | 4 | 7.5 | 34.1 | 78 | 1460 |
| Aug 9 | rs_en_q8pw | 8.1 | 134 / 176 / 221 | 7.6 | 130 / 161 / 206 | 6567 | 26 (102) | 4 | 7.5 | 34.3 | 78 | 1559 |
| Aug 9 | rs_en_f16 | 8.2 | 136 / 180 / 216 | 7.8 | 131 / 170 / 206 | 6558 | 27 (109) | 4 | 7.5 | 33.4 | 80 | 2026 |
| Aug 9 | rs_en_pwq8 | 8.1 | 134 / 177 / 218 | 7.6 | 130 / 162 / 203 | 6563 | 25 (100) | 4 | 7.5 | 33.2 | 80 | 1474 |
| Aug 9 | rs_n35_shipped | 8.4 | 146 / 247 / 155 | 8.0 | 145 / 236 / 144 | 6430 | 31 (132) | 6 | 10.5 | 32.4 | 82 | 1468 |
| Aug 9 | rs_n35_q8pw | 8.3 | 143 / 243 / 157 | 8.0 | 142 / 232 / 146 | 6436 | 31 (130) | 5 | 7.4 | 32.2 | 83 | 1534 |
| Aug 9 | rs_n35_f16 | 8.3 | 144 / 247 / 150 | 7.9 | 143 / 236 / 139 | 6425 | 30 (136) | 5 | 7.4 | 30.7 | 87 | 2024 |
| Aug 9 | rs_n35_pwq8 | 8.4 | 147 / 241 / 157 | 8.0 | 145 / 230 / 146 | 6438 | 31 (130) | 5 | 7.4 | 32.1 | 83 | 1460 |
| Sep 7 | rs_en_shipped | 7.2 | 120 / 176 / 209 | 6.9 | 124 / 161 / 194 | 7025 | 50 (192) | 2 | 8.6 | 42.7 | 67 | 1449 |
| Sep 7 | rs_en_q8pw | 7.3 | 119 / 181 / 209 | 6.9 | 123 / 166 / 194 | 7020 | 50 (192) | 2 | 8.6 | 36.2 | 79 | 1519 |
| Sep 7 | rs_en_f16 | 7.3 | 121 / 179 / 210 | 6.9 | 125 / 164 / 195 | 7023 | 50 (191) | 2 | 8.6 | 37.0 | 77 | 2038 |
| Sep 7 | rs_en_pwq8 | 7.3 | 119 / 179 / 215 | 7.0 | 123 / 164 / 200 | 7028 | 50 (192) | 2 | 8.6 | 36.3 | 79 | 1454 |
| Sep 7 | rs_n35_shipped | 7.5 | 136 / 263 / 122 | 7.2 | 141 / 251 / 110 | 6851 | 60 (222) | 4 | 12.9 | 34.2 | 83 | 1410 |
| Sep 7 | rs_n35_q8pw | 7.5 | 137 / 267 / 120 | 7.2 | 140 / 255 / 108 | 6845 | 60 (222) | 4 | 12.9 | 34.2 | 83 | 1470 |
| Sep 7 | rs_n35_f16 | 7.4 | 133 / 266 / 117 | 7.1 | 135 / 255 / 106 | 6843 | 62 (229) | 4 | 12.9 | 33.7 | 85 | 1988 |
| Sep 7 | rs_n35_pwq8 | 7.5 | 139 / 265 / 120 | 7.2 | 141 / 254 / 109 | 6847 | 61 (226) | 4 | 12.9 | 35.0 | 82 | 1406 |
| Sep 5 | rs_en_shipped | 4.7 | 117 / 103 / 182 | 4.5 | 122 / 92 / 171 | 8635 | 14 (53) | 2 | 7.8 | 33.2 | 81 | 1491 |
| Sep 5 | rs_en_q8pw | 4.7 | 115 / 103 / 182 | 4.5 | 120 / 92 / 171 | 8635 | 14 (53) | 2 | 7.8 | 32.4 | 83 | 1558 |
| Sep 5 | rs_en_f16 | 4.7 | 117 / 103 / 180 | 4.5 | 121 / 92 / 169 | 8633 | 13 (50) | 2 | 7.8 | 34.1 | 79 | 2044 |
| Sep 5 | rs_en_pwq8 | 4.7 | 115 / 104 / 181 | 4.5 | 121 / 92 / 169 | 8633 | 14 (53) | 2 | 7.8 | 33.0 | 82 | 1488 |
| Sep 5 | rs_n35_shipped | 5.1 | 153 / 186 / 99 | 4.9 | 158 / 176 / 89 | 8469 | 9 (38) | 1 | 6.4 | 32.3 | 83 | 1440 |
| Sep 5 | rs_n35_q8pw | 5.2 | 156 / 191 / 100 | 5.0 | 161 / 181 / 90 | 8465 | 9 (38) | 1 | 6.4 | 32.0 | 84 | 1510 |
| Sep 5 | rs_n35_f16 | 4.9 | 155 / 176 / 92 | 4.8 | 158 / 167 / 83 | 8472 | 9 (38) | 1 | 6.4 | 31.3 | 86 | 1992 |
| Sep 5 | rs_n35_pwq8 | 5.1 | 153 / 187 / 100 | 5.0 | 158 / 177 / 90 | 8469 | 9 (38) | 1 | 6.4 | 32.6 | 83 | 1444 |
| Sep Logistics | rs_en_shipped | 7.0 | 128 / 117 / 199 | 7.0 | 130 / 116 / 198 | 6397 | 10 (37) | 1 | 8.3 | 33.0 | 83 | 1431 |
| Sep Logistics | rs_en_q8pw | 7.0 | 128 / 118 / 198 | 7.0 | 130 / 117 / 197 | 6395 | 10 (37) | 1 | 8.3 | 32.5 | 84 | 1494 |
| Sep Logistics | rs_en_f16 | 7.0 | 127 / 119 / 194 | 7.0 | 129 / 118 / 193 | 6390 | 10 (36) | 1 | 8.3 | 33.4 | 82 | 1980 |
| Sep Logistics | rs_en_pwq8 | 7.0 | 128 / 117 / 199 | 7.0 | 130 / 116 / 198 | 6397 | 10 (37) | 1 | 8.3 | 33.0 | 83 | 1428 |
| Sep Logistics | rs_n35_shipped | 6.6 | 129 / 141 / 147 | 6.6 | 129 / 141 / 147 | 6321 | 14 (52) | 2 | 17.3 | 32.7 | 84 | 1420 |
| Sep Logistics | rs_n35_q8pw | 6.6 | 127 / 143 / 144 | 6.6 | 127 / 143 / 144 | 6316 | 14 (52) | 2 | 17.3 | 32.4 | 84 | 1500 |
| Sep Logistics | rs_n35_f16 | 6.7 | 128 / 145 / 147 | 6.7 | 128 / 145 / 147 | 6317 | 14 (52) | 2 | 17.3 | 31.5 | 87 | 1980 |
| Sep Logistics | rs_n35_pwq8 | 6.6 | 131 / 143 / 143 | 6.6 | 131 / 143 / 143 | 6315 | 14 (52) | 2 | 17.3 | 32.6 | 84 | 1414 |
| Aug 14 | rs_en_shipped | 43.9 | 294 / 2490 / 2559 | 7.2 | 291 / 260 / 329 | 12230 | 22 (108) | 9 | 25.4 | 56.1 | 82 | 1487 |
| Aug 14 | rs_en_q8pw | 43.9 | 291 / 2489 / 2556 | 7.3 | 285 / 265 / 332 | 12228 | 22 (115) | 9 | 25.4 | 54.6 | 85 | 1542 |
| Aug 14 | rs_en_f16 | 44.0 | 294 / 2492 / 2565 | 7.2 | 286 / 260 / 333 | 12234 | 22 (108) | 9 | 25.4 | 57.0 | 81 | 2036 |
| Aug 14 | rs_en_pwq8 | 44.0 | 295 / 2490 / 2565 | 7.4 | 292 / 265 / 340 | 12236 | 22 (115) | 9 | 25.4 | 56.2 | 82 | 1484 |
| Aug 14 | rs_n35_shipped | 42.9 | 339 / 2511 / 2371 | 7.0 | 353 / 318 / 178 | 12021 | 22 (95) | 9 | 16.6 | 54.7 | 84 | 1448 |
| Aug 14 | rs_n35_q8pw | 43.0 | 342 / 2507 / 2375 | 6.9 | 350 / 312 / 180 | 12029 | 21 (92) | 9 | 16.6 | 55.5 | 83 | 1512 |
| Aug 14 | rs_n35_f16 | 42.9 | 345 / 2508 / 2362 | 7.0 | 354 / 320 / 174 | 12015 | 21 (94) | 9 | 16.6 | 54.0 | 86 | 2006 |
| Aug 14 | rs_n35_pwq8 | 43.0 | 345 / 2507 / 2376 | 7.0 | 355 / 313 / 182 | 12030 | 21 (92) | 9 | 16.6 | 55.7 | 83 | 1450 |
| AMI ES2002a | rs_en_shipped | 18.0 | 96 / 349 / 26 | 16.6 | 78 / 339 / 16 | 2289 | 10 (99) | 4 | 12.1 | 15.4 | 83 | 1437 |
| AMI ES2002a | rs_en_q8pw | 17.8 | 94 / 345 / 26 | 16.3 | 77 / 334 / 15 | 2293 | 10 (99) | 4 | 12.1 | 18.0 | 71 | 1703 |
| AMI ES2002a | rs_en_f16 | 17.6 | 96 / 338 / 26 | 16.2 | 78 / 328 / 16 | 2300 | 10 (92) | 3 | 12.1 | 16.4 | 78 | 1992 |
| AMI ES2002a | rs_en_pwq8 | 18.1 | 97 / 349 / 27 | 16.7 | 79 / 339 / 17 | 2290 | 10 (99) | 4 | 12.1 | 15.4 | 83 | 1444 |
| AMI ES2002a | rs_n35_shipped | 20.7 | 113 / 402 / 25 | 19.1 | 84 / 396 / 19 | 2235 | 12 (62) | 3 | 12.2 | 15.8 | 81 | 1428 |
| AMI ES2002a | rs_n35_q8pw | 20.8 | 114 / 404 / 25 | 19.2 | 85 / 398 / 19 | 2233 | 12 (62) | 3 | 12.2 | 16.1 | 79 | 1573 |
| AMI ES2002a | rs_n35_f16 | 20.5 | 111 / 399 / 25 | 18.9 | 83 / 392 / 18 | 2238 | 11 (59) | 3 | 12.2 | 15.3 | 83 | 1998 |
| AMI ES2002a | rs_n35_pwq8 | 20.6 | 115 / 400 / 24 | 19.1 | 86 / 394 / 18 | 2236 | 12 (62) | 3 | 12.2 | 15.9 | 80 | 1432 |

- AMI, the one human reference: EN 18.0 (shipped), 17.8 (fixed), 17.6 (f16), 18.1 (control); 3.5 20.7, 20.8, 20.5, 20.6. One AMI word is 0.04 points, so the spread is under ten words in each direction, and the control alone moves as much as the fix.
- The Zoom sessions move by 0.0 to 0.3 points either way.
- Speed: within noise (EN 77, 80, 80 × real time; 3.5 83, 83, 85). The fix costs about +95 MiB (EN) and +82 MiB (3.5) of peak VRAM on the median session; f16 about +575 MiB.

## 3. Broad set and talks, file pass

22 more recordings from the owner's three meeting folders (A, B, C; 1,017 min) and the seven talks (folder D, 356 min), reported apart from the gate set. Per the plan, the broad set ran shipped and fixed q8_0 for both models; the talks also ran EN f16. Speed and VRAM are left out here: part of these runs overlapped other agents' builds or ran without quiet.lock (the shipped timings are in the baseline report, from an exclusive window).

Nemotron EN, talks with f16:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| talks | rs_en_shipped | 7 | 356 | 14.0 | 2819 | 8.7 | 1324 | 1708 | 1691 | 172 (676) | 12 | base | base |
| talks | rs_en_q8pw | 7 | 356 | 14.0 | 2821 | 8.7 | 1329 | 1716 | 1681 | 171 (672) | 12 | 0 / 1 | 0 / 0 |
| talks | rs_en_f16 | 7 | 356 | 14.0 | 2828 | 8.7 | 1337 | 1709 | 1689 | 173 (683) | 12 | 3 / 2 | 0 / 0 |

Nemotron EN, shipped against fixed, per folder and pooled ("all" is the 36 sessions, AMI included; folder A includes Aug 2, Aug 9, Aug 14, folder B Sep 5, Sep 7, Sep Logistics):

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| broad | rs_en_shipped | 22 | 1017 | 8.3 | 4784 | 6.8 | 3533 | 4242 | 3448 | 502 (1921) | 30 | base | base |
| broad | rs_en_q8pw | 22 | 1017 | 8.3 | 4797 | 6.8 | 3545 | 4208 | 3447 | 506 (1940) | 32 | 7 / 3 | 2 / 0 |
| folder:A | rs_en_shipped | 15 | 748 | 12.4 | 5846 | 6.5 | 2396 | 2884 | 2353 | 290 (1102) | 29 | base | base |
| folder:A | rs_en_q8pw | 15 | 748 | 12.4 | 5851 | 6.5 | 2409 | 2872 | 2343 | 295 (1131) | 31 | 6 / 1 | 2 / 0 |
| folder:B | rs_en_shipped | 8 | 348 | 6.6 | 1396 | 6.4 | 1339 | 1249 | 1164 | 201 (793) | 11 | base | base |
| folder:B | rs_en_q8pw | 8 | 348 | 6.6 | 1401 | 6.4 | 1344 | 1253 | 1158 | 200 (790) | 11 | 1 / 2 | 0 / 0 |
| folder:C | rs_en_shipped | 5 | 224 | 8.7 | 885 | 8.5 | 855 | 1395 | 890 | 153 (604) | 9 | base | base |
| folder:C | rs_en_q8pw | 5 | 224 | 8.6 | 889 | 8.5 | 857 | 1369 | 899 | 154 (606) | 9 | 1 / 0 | 0 / 0 |
| folder:D | rs_en_shipped | 7 | 356 | 14.0 | 2819 | 8.7 | 1324 | 1708 | 1691 | 172 (676) | 12 | base | base |
| folder:D | rs_en_q8pw | 7 | 356 | 14.0 | 2821 | 8.7 | 1329 | 1716 | 1681 | 171 (672) | 12 | 0 / 1 | 0 / 0 |
| all | rs_en_shipped | 36 | 1697 | 11.0 | 11295 | 7.3 | 6253 | 7252 | 6176 | 826 (3274) | 65 | base | base |
| all | rs_en_q8pw | 36 | 1697 | 11.0 | 11307 | 7.3 | 6273 | 7225 | 6158 | 830 (3298) | 67 | 8 / 4 | 2 / 0 |

Nemotron 3.5, shipped against fixed:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| talks | rs_n35_shipped | 7 | 356 | 14.4 | 3279 | 9.2 | 1782 | 1107 | 2104 | 171 (677) | 12 | base | base |
| talks | rs_n35_q8pw | 7 | 356 | 14.4 | 3279 | 9.2 | 1785 | 1099 | 2107 | 170 (674) | 12 | 0 / 1 | 0 / 0 |
| broad | rs_n35_shipped | 22 | 1017 | 8.6 | 6292 | 7.2 | 5076 | 2573 | 4195 | 542 (2263) | 46 | base | base |
| broad | rs_n35_q8pw | 22 | 1017 | 8.6 | 6281 | 7.2 | 5059 | 2570 | 4205 | 545 (2273) | 45 | 11 / 8 | 0 / 0 |
| folder:A | rs_n35_shipped | 15 | 748 | 12.4 | 6756 | 6.7 | 3393 | 1711 | 2769 | 312 (1307) | 45 | base | base |
| folder:A | rs_n35_q8pw | 15 | 748 | 12.4 | 6730 | 6.7 | 3363 | 1719 | 2786 | 310 (1298) | 43 | 5 / 7 | 0 / 1 |
| folder:B | rs_n35_shipped | 8 | 348 | 7.2 | 1949 | 7.1 | 1890 | 837 | 1399 | 222 (894) | 16 | base | base |
| folder:B | rs_n35_q8pw | 8 | 348 | 7.2 | 1969 | 7.1 | 1909 | 827 | 1391 | 226 (909) | 16 | 6 / 2 | 0 / 0 |
| folder:C | rs_n35_shipped | 5 | 224 | 8.9 | 1301 | 8.8 | 1276 | 818 | 1152 | 172 (732) | 12 | base | base |
| folder:C | rs_n35_q8pw | 5 | 224 | 8.9 | 1296 | 8.8 | 1268 | 818 | 1151 | 171 (728) | 12 | 2 / 3 | 0 / 0 |
| folder:D | rs_n35_shipped | 7 | 356 | 14.4 | 3279 | 9.2 | 1782 | 1107 | 2104 | 171 (677) | 12 | base | base |
| folder:D | rs_n35_q8pw | 7 | 356 | 14.4 | 3279 | 9.2 | 1785 | 1099 | 2107 | 170 (674) | 12 | 0 / 1 | 0 / 0 |
| all | rs_n35_shipped | 36 | 1697 | 11.3 | 13687 | 7.7 | 8737 | 4492 | 7508 | 889 (3672) | 88 | base | base |
| all | rs_n35_q8pw | 36 | 1697 | 11.3 | 13678 | 7.7 | 8723 | 4482 | 7520 | 889 (3671) | 86 | 13 / 13 | 0 / 1 |

The gaps of 5 s or more that change, with the words each config has in them:

| Session | Stretch | Ref words (Zoom) | Words, EN shipped / fixed | Words, 3.5 shipped / fixed / old-policy control |
|---|---|---|---|---|
| Aug 4 | 49.8 to 67.5 s (17.8 s) | 6 | 4 / **0** | 6 / 6 / – |
| Aug 8 | 1541.2 to 1546.7 s (5.5 s) | 4 | 2 / **0** | 4 / 4 / – |
| Aug 9 | 2537.0 to 2547.4 s (10.4 s) | 5 | 6 / 6 | **0** / 1 / 1 |

Two new long gaps on EN with the fix, each a handful of words in a sparse stretch; one 3.5 gap closes, and the old-policy control closes it too (pe-table drift, not the pointwise change). Nothing like PR #46's "speaker's introduction" lost.

## 4. How much the words move, and determinism

Raw word edit operations between configs (`levers.py diff` arithmetic over `engine.words.json`, summed):

| Pair | Set | Sessions | Words (base) | Raw edit ops | Per 1,000 words | Sessions identical |
|---|---|---|---|---|---|---|
| rs_en_shipped vs rs_en_q8pw | gate | 7 | 49866 | 124 | 2.5 | 0 |
| rs_en_shipped vs rs_en_q8pw | broad | 22 | 165528 | 511 | 3.1 | 0 |
| rs_en_shipped vs rs_en_q8pw | talks | 7 | 55309 | 229 | 4.1 | 0 |
| rs_en_shipped vs rs_en_f16 | gate | 7 | 49866 | 302 | 6.1 | 0 |
| rs_en_shipped vs rs_en_f16 | talks | 7 | 55309 | 325 | 5.9 | 0 |
| rs_en_shipped vs rs_en_pwq8 | gate | 7 | 49866 | 103 | 2.1 | 0 |
| rs_en_q8pw vs rs_en_f16 | gate | 7 | 49853 | 308 | 6.2 | 0 |
| rs_en_q8pw vs rs_en_f16 | talks | 7 | 55302 | 286 | 5.2 | 0 |
| rs_n35_shipped vs rs_n35_q8pw | gate | 7 | 48499 | 258 | 5.3 | 0 |
| rs_n35_shipped vs rs_n35_q8pw | broad | 22 | 160479 | 964 | 6.0 | 0 |
| rs_n35_shipped vs rs_n35_q8pw | talks | 7 | 53654 | 347 | 6.5 | 0 |
| rs_n35_shipped vs rs_n35_f16 | gate | 7 | 48499 | 490 | 10.1 | 0 |
| rs_n35_shipped vs rs_n35_pwq8 | gate | 7 | 48499 | 248 | 5.1 | 0 |
| rs_n35_q8pw vs rs_n35_f16 | gate | 7 | 48500 | 488 | 10.1 | 0 |
| rs_en_shipped vs rs_en_shipped_b020 | gate | 7 | 49866 | 159 | 3.2 | 0 |
| rs_en_shipped_b020 vs rs_en_shipped_main | gate | 7 | 49870 | 1186 | 23.8 | 0 |
| rs_n35_shipped vs rs_n35_shipped_b020 | gate | 7 | 48499 | 254 | 5.2 | 0 |
| rs_n35_shipped_b020 vs rs_n35_shipped_main | gate | 7 | 48506 | 1858 | 38.3 | 0 |
| rs_en_pwq8 vs rs_en_q8pw | gate | 7 | 49870 | 116 | 2.3 | 0 |
| rs_n35_pwq8 vs rs_n35_q8pw | gate | 7 | 48521 | 260 | 5.4 | 0 |

- **Runs are deterministic.** Re-running a config gives identical words, times and speaker tags. Checked: `rs_*_shipped_rep` against `rs_*_shipped` on AMI, Aug 2 and Sep 5 for both models (0 edit operations), and the first Talk 1 runs against their re-runs for all five configs (identical; one pair with and one without the standalone diarizer pass). So every difference above comes from the weights, not run-to-run noise.
- The pointwise change alone (old-policy control against fixed) moves 2.3 (EN) and 5.4 (3.5) words per 1,000. The pe-table rounding alone (control against NVIDIA) moves 2.1 and 5.1. The models react at this level to any tiny weight change. f16 moves 6 to 10 per 1,000. None of them shifts deletions or gaps in one direction.

## 5. Live setting and FLEURS

Live setting (lookahead 1, `v3-streaming`, as the live view runs; the same cache-aware decoder through `transcribe --stream`), AMI, Aug 2 and Sep 5 (110 min). run.json and engine.log confirm lookahead 1 and `v3-streaming`.

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| all | rs_en_shipped_live | 3 | 110 | 9.2 | 730 | 8.4 | 677 | 410 | 428 | 45 (227) | 7 | base | base |
| all | rs_en_q8pw_live | 3 | 110 | 9.2 | 734 | 8.5 | 682 | 410 | 427 | 46 (232) | 7 | 1 / 0 | 0 / 0 |

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| all | rs_n35_shipped_live | 3 | 110 | 10.5 | 982 | 9.8 | 935 | 290 | 535 | 58 (275) | 9 | base | base |
| all | rs_n35_q8pw_live | 3 | 110 | 10.4 | 974 | 9.8 | 932 | 295 | 531 | 56 (265) | 9 | 0 / 2 | 0 / 0 |

FLEURS English (55 utterances, 1,186 reference words; `run.py --fleurs en`, file setting). run.py's FLEURS report labels these runs "160 ms" because its `lookahead_ms()` reads only `engine_args`; run.json shows lookahead 13 for every run.

| Config | WER % | Sub | Del | Ins |
|---|---|---|---|---|
| rs_n35_shipped | 8.77 | 63 | 22 | 19 |
| rs_n35_q8pw | 8.77 | 63 | 22 | 19 |
| rs_n35_f16 | 9.02 | 62 | 26 | 19 |
| rs_en_shipped | 21.5 | 51 | 189 | 15 |
| rs_en_q8pw | 21.5 | 51 | 189 | 15 |

The EN model's quiet-speech deletions on FLEURS (research/12 section 5) are untouched by the fix.

## 6. Runtime built from `main`: 2 × 2 and a toolchain control

`main` at `e680fc3` built locally: CUDA 12.8, VS 2022 Build Tools 17.x (MSVC), CMake 3.31.7, Ninja from VS, `-DCMAKE_CUDA_ARCHITECTURES=86`, ASR and diarization only. The build driver's vcpkg step was replaced by a local static SentencePiece v0.2.0, because a new downloading exe is blocked by the PC's firewall. llama.cpp `bd4f514` was checked out with `core.longpaths`. Configure 72 s, build 4 min 30 s, so the one-hour cap was not an issue. The package carries the CUDA 12.8 `cudart64_12`, `cublas64_12` and `cublasLt64_12` DLLs. main's `VERSION` still says 0.2.0, so its runs record engine version 0.2.0; they are told apart by config name and engine folder (`nemo-speech-main-e680fc3-windows-x86_64-cuda`, confirmed in every engine.log). As a control, v0.2.0 (`6a3ca36`, which pins the same llama.cpp commit) was built the same way (`rs_*_shipped_b020`), so the toolchain and the code change can be told apart.

Nemotron EN, gate set:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | rs_en_shipped | 7 | 324 | 16.7 | 3692 | 7.5 | 1396 | 1302 | 1037 | 152 (677) | 23 | base | base |
| gate | rs_en_shipped_b020 | 7 | 324 | 16.8 | 3704 | 7.5 | 1394 | 1303 | 1041 | 152 (672) | 22 | 1 / 1 | 0 / 0 |
| gate | rs_en_shipped_main | 7 | 324 | 16.8 | 3726 | 7.5 | 1442 | 1330 | 983 | 165 (729) | 23 | 27 / 14 | 1 / 1 |
| gate | rs_en_q8pw_main | 7 | 324 | 16.8 | 3730 | 7.5 | 1447 | 1334 | 975 | 167 (740) | 23 | 29 / 14 | 1 / 1 |

Nemotron 3.5, gate set:

| Set | Config | Sessions | Min | cpWER | cp del | WER | del | ins | sub | Gaps ≥ 2 s (ref words) | ≥ 5 s | new / gone ≥ 2 s vs base | new / gone ≥ 5 s vs base |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| gate | rs_n35_shipped | 7 | 324 | 16.8 | 4116 | 7.8 | 1879 | 812 | 1209 | 176 (732) | 30 | base | base |
| gate | rs_n35_shipped_b020 | 7 | 324 | 16.9 | 4131 | 7.8 | 1880 | 815 | 1218 | 177 (741) | 31 | 4 / 3 | 2 / 1 |
| gate | rs_n35_shipped_main | 7 | 324 | 17.1 | 4169 | 7.9 | 1897 | 821 | 1242 | 187 (787) | 32 | 35 / 23 | 5 / 3 |
| gate | rs_n35_q8pw_main | 7 | 324 | 17.1 | 4182 | 8.0 | 1908 | 822 | 1255 | 188 (787) | 33 | 35 / 22 | 6 / 3 |

Speed of the two local builds (EN, gate set, transcribe step): pending; the exclusive timing window was interrupted on 27 Oct and these blocks have not run yet.

- **Toolchain: no effect.** The local v0.2.0 build matches NVIDIA's release package: EN deletions 1,394 against 1,396, gaps 152 against 152. It changes 3.2 (EN) and 5.2 (3.5) words per 1,000, the same level as the weight perturbations above.
- **main's code: a different decode, slightly worse on Zoom.**
  - Against the local v0.2.0 build it changes 24 (EN) and 38 (3.5) words per 1,000.
  - It adds about 3 % deletions on the gate set's Zoom sessions with either GGUF: EN 1,394 to 1,442, 3.5 1,880 to 1,897.
  - It adds 10 to 15 gaps of 2 s or more, and up to 3 gaps of 5 s or more on 3.5.
  - AMI deletions stay flat (EN 339 to 341, 3.5 396 to 392), but AMI cpWER rises 0.7 (EN) and 0.9 (3.5) points: speaker attribution changes, plausibly from main's diarizer changes (#62, #70; inference).
  - With main, the fixed q8_0 is again no better than NVIDIA's (EN deletions 1,442 against 1,447).
- **Inference:** PR #46's first-chunk change shifts every later chunk boundary of a stream, which would explain a whole-file change of this size. Not bisected.
- **Startup words.** Words in the first 10 s per session (reference first: Aug 2 20, Aug 9 24, Sep 7 19, Sep 5 19, Sep Logistics 15, Aug 14 17, AMI 0, because the AMI annotation starts later). EN shipped on 0.2.0 gives 24, 22, 18, 20, 12, 11, 17 and on main 24, 22, 19, 20, 12, 11, 17; 3.5 changes Sep 7 by +2 and Sep 5 by −1. The startup fix is worth at most a word or two per file at lookahead 13.

## 7. Licences: may we ship converted files?

Facts only, read 25 Oct 2026; not a legal opinion and not a decision.

| Model | Card licence | What the licence says about distributing converted weights |
|---|---|---|
| Nemotron Speech Streaming EN 0.6B | `license: other`, `license_name: nvidia-open-model-license`; card: "Use of the model is governed by the NVIDIA Open Model License Agreement" | The Agreement (page "Last Modified: October 24, 2025") says NVIDIA confirms "You are free to create and distribute Derivative Models"; "Derivative Model" covers modifications of and works based on the Model. Its Redistribution clause allows copies of the Model or Derivative Models "in any medium, with or without modifications" on conditions: give recipients a copy of the Agreement, and include a "Notice" text file with the attribution "Licensed by NVIDIA Corporation under the NVIDIA Open Model License". Rights terminate on patent or copyright litigation against the Model or on bypassing its guardrails; NVIDIA may update the Agreement, and a distributor must then comply or stop distributing. No trademark rights beyond describing the origin |
| Nemotron 3.5 ASR Streaming 0.6B | `license: other`, `license_name: openmdw-1.1`; card: "Use of the model is governed by the OpenMDW-1.1 license" | OpenMDW-1.1 grants permission "to deal in the Model Materials without restriction"; a distribution must keep a copy of the agreement and the copyright and other notices of origin; rights end on patent or copyright litigation over the Model Materials. Note: `manifests/models.json` calls this licence "NVIDIA Open Model License (OpenMDW 1.1)", which does not match the card |

Reading (inference): a GGUF converted from the `.nemo` is a "Derivative Model" (EN) or a portion of the "Model Materials" (3.5), so self-hosting both looks permitted with the licence text and notices shipped next to the files, as for NVIDIA's own GGUFs today. Whether to self-host, and where, is the owner's decision.

Sources: https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b and https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b (`README.md` at the pinned revisions), https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/, https://openmdw.ai/license/1-1/ (all read 25 Oct 2026).

## 8. Recommendation

| Option | Evidence | Verdict |
|---|---|---|
| Ship re-converted q8_0 GGUFs on 0.2.0 (self-hosted) | No measurable gain on 1,697 min for either model, file or live setting (sections 2 to 5); +68 MiB download and about +95 MiB VRAM; hosting plus the licence notices (section 7) for nothing measured | **Do not**, for accuracy reasons. Nothing argues against it either, except the hosting work |
| Take NVIDIA's republished GGUFs when they appear | Same weights policy as our fixed files; also embeds the SentencePiece tokenizer that word boosting needs (research/12 section 4) | **Yes, as a routine manifest bump**: run the gate set once to confirm, as for any model change (the fixed files change 2 to 6 words per 1,000) |
| Runtime from `main` (≥ `1a7e945`) or the next tag | main at `e680fc3` adds about 3 % deletions and 10 to 15 more gaps on the gate set's Zoom sessions with either GGUF; AMI cpWER rises 0.7 to 0.9 points through attribution (section 6) | **Wait** for the next tag and re-run the gate set and the broad set with the gap count before adopting it. Bisect between v0.2.0 and main if the next tag shows the same |

What the data says about "many word errors sometimes": with every precision and both runtimes, Zoom has three or more words in about 830 (EN) to 890 (3.5) stretches of 2 s or more where we have none. That is about 29 to 31 per hour of meeting, 65 to 88 of them 5 s or longer. They are the same stretches in shipped, fixed and f16, so they are not a quantization effect. Some are Zoom's own insertions or pauses inside a long Zoom cue, but on AMI's manual reference the same check finds 10 to 12 such gaps in 21 minutes. The next spike should characterise them: audio level inside each gap, overlap with other speakers, and whether EN and 3.5 miss the same stretches. The EN model's FLEURS deletions (quiet speech, research/12 section 5) point the same way.

Proposed harness addition: a `levers.py gaps` subcommand with this section 0 definition (a draft was written in the session scratchpad; it reuses `run.load_reference`, `run.scored_region`, `metrics.hyp_from_export` and `common.interpolate_words`). Not applied, since tracked files were out of scope.

## 9. Reproducing

All from `tools/eval` after the README setup (`MSYS2_ARG_CONV_EXCL='*' CL=/std:c++20 uv sync`). Configs are in a scratch `configs.toml` passed with `--configs-file` (`rs_*` copies of `shipped_en` / `shipped_n35` with `asr` set to the file name for the re-converted files); `sessions.toml` holds the private session paths.

```sh
# 1. sources (curl.exe is the executable the PC's firewall allows)
curl.exe --ssl-revoke-best-effort -L -o src/en/nemotron-speech-streaming-en-0.6b.nemo \
  https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b/resolve/ebe59e5a817142986528bbbee5dba8db7b38ed50/nemotron-speech-streaming-en-0.6b.nemo
curl.exe --ssl-revoke-best-effort -L -o src/n35/nemotron-3.5-asr-streaming-0.6b.nemo \
  https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b/resolve/1c8deaecc64b91f034d73e08dd8b64625eb3395d/nemotron-3.5-asr-streaming-0.6b.nemo

# 2. converter: NeMo-Speech.cpp main (e680fc3) in its own venv, CPU only
git clone --depth 1 https://github.com/NVIDIA/NeMo-Speech.cpp.git nemo-speech-src-main
uv venv --python 3.12 conv-venv && uv pip install --python conv-venv/Scripts/python.exe -r nemo-speech-src-main/requirements.txt
export CUDA_VISIBLE_DEVICES=-1
conv-venv/Scripts/python.exe nemo-speech-src-main/convert_model.py src/en/nemotron-speech-streaming-en-0.6b.nemo \
  --outfile models/user/nemotron-speech-streaming-en-0.6b.q8_0-pwf16.gguf --outtype q8_0
conv-venv/Scripts/python.exe nemo-speech-src-main/convert_model.py src/en/nemotron-speech-streaming-en-0.6b.nemo \
  --outfile models/user/nemotron-speech-streaming-en-0.6b.f16.gguf --outtype f16
# (the same two for the 3.5 .nemo; the old-policy control is the same q8_0 command with
#  conversion.asr._pick_dtype called with cache_aware_rnnt=False)
echo '{"file_right_context": 13}' > models/user/<file>.gguf.meet.json     # every non-manifest file

# 3. runs (engine, models and diarist-cli as in section 0)
export DIARIST_EVAL_ENGINE_DIR=<engine folder> DIARIST_EVAL_MODELS_DIR=<models> DIARIST_EVAL_CLI=<diarist-cli beta.4>
for c in rs_en_shipped rs_en_q8pw rs_en_f16 rs_en_pwq8 rs_n35_shipped rs_n35_q8pw rs_n35_f16 rs_n35_pwq8; do
  uv run python -I run.py --config $c --configs-file <configs> --sessions aug2,aug9,sep7,sep5,seplog,aug14,ami --transcribe-only
done
#    broad and talks: rs_en_shipped rs_en_q8pw rs_n35_shipped rs_n35_q8pw (+ rs_en_f16 on talk1..7)
#    live: rs_*_live (engine_args --asr.streaming.rnnt_right_context 1 --asr.diar.preset v3-streaming) on ami,aug2,sep5
#    FLEURS: run.py --config <c> --fleurs en
#    2 x 2: DIARIST_EVAL_ENGINE_DIR=<main build> with rs_*_main; <local v0.2.0 build> with rs_*_shipped_b020
uv run python -I run.py --config <c> --configs-file <configs> --sessions <ids> --metrics-only --no-report
uv run python -I levers.py diff --a rs_en_shipped --b rs_en_q8pw --sessions <ids>
# WER, gaps, pooled tables: the scratch analyze.py (gap definition, section 0) and tables.py

# 4. runtime builds (Developer environment from vcvars64.bat; SentencePiece v0.2.0 built statically and passed with CMAKE_PREFIX_PATH)
cmake -S nemo-speech-src-main -B nemo-main-cuda -G Ninja -DCMAKE_BUILD_TYPE=Release -DNEMO_SPEECH_BUILD_ASR=ON \
  -DNEMO_SPEECH_BUILD_DIAR=ON -DNEMO_SPEECH_BUILD_TTS=OFF -DNEMO_SPEECH_BUILD_NMT=OFF -DNEMO_SPEECH_BUILD_HTTP=OFF \
  -DNEMO_SPEECH_BUILD_GRPC=OFF -DNEMO_SPEECH_WITH_FLASHLIGHT=OFF -DNEMO_SPEECH_WITH_NORM=OFF -DNEMO_SPEECH_BUILD_TESTS=OFF \
  -DBUILD_TESTING=OFF -DNEMO_SPEECH_BUILD_EXAMPLES=OFF -DNEMO_SPEECH_BUILD_TOOLS=OFF -DGGML_CUDA=ON -DGGML_VULKAN=OFF \
  -DNEMO_SPEECH_CUBLAS_SHIM=OFF -DCMAKE_CUDA_ARCHITECTURES=86 -DCMAKE_PREFIX_PATH=<sentencepiece install>
cmake --build nemo-main-cuda --parallel 16
# (the same for a v0.2.0 checkout, tag v0.2.0 = 6a3ca36, for the toolchain control)
```

Harness notes: most runs used a copy of run.py without the standalone `--rttm` diarizer pass. That pass runs after the transcript is built and never feeds it (`crates/core/src/pipeline/import.rs` step 3), so words, tags and the transcribe-step time are unchanged and the GPU time per session nearly halves. run.py's FLEURS report mislabels the lookahead when it comes from the manifest or a sidecar (section 5). The cache-key pitfall of research/17 was avoided by giving every variant its own config name.

## Sources

NVIDIA/NeMo-Speech.cpp: PR #46 (`1a7e945`, merged 24 Oct 2026), `docs/model-conversion.md` and `conversion/asr.py` at `e680fc3`, tag v0.2.0 (`6a3ca36`), `scripts/windows/build.ps1`, `docs/development/windows-build.md`, `src/common/parameter_parser.cpp`, `src/asr/CMakeLists.txt` (read 25 and 26 Oct 2026). Hugging Face repos and model cards of `nvidia/nemotron-speech-streaming-en-0.6b` and `nvidia/nemotron-3.5-asr-streaming-0.6b` at the pinned revisions (25 Oct 2026). Licence texts as in section 7. Diarist: `crates/core/src/models/classify.rs` and `scan.rs` (sidecars, scan), `crates/core/src/pipeline/import.rs`, `tools/eval` (beta.4 tree).
