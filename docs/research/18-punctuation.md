# Punctuation and casing: add-on or replacement (25 Oct 2026)

The owner wants transcripts with real punctuation and casing, and asked whether that should come from a speech model that punctuates by itself (a replacement) or from a punctuation model run over the finished words (an add-on), without a performance cost. This is research, a recommendation and a spike plan. No product code changed and no model was downloaded: model facts come from the Hugging Face and GitHub APIs and the model cards (sizes and SHA-256 are the LFS values the Hugging Face API reports), runtime facts from the NeMo-Speech.cpp source at `v0.2.0` and `main`. Opus 5.5 research agent. Builds on research/08 section 7, research/12 section 3, plan 12 section 5 and docs/KNOWN-ISSUES.md. Sources are numbered [S1] to [S41] at the end. Statements marked *inference* are reasoning, not something a source says or something we measured.

**Summary.**

- **Add-on, not replacement.** No speech model that NeMo-Speech.cpp runs today both punctuates and keeps all the speech. Parakeet TDT v3 punctuates (12.4 percent of words) but drops whole sentences, and that is still open upstream in NVIDIA's own pipeline (section 2.3). The two Nemotron models are trained to punctuate but put marks on 0.2 to 2.5 percent of words in our meetings. Human and Zoom references have 14 to 22 marks per 100 words (section 1).
- **The runtime hook is dead for Diarist in two ways, not one.** The gate research/12 found is unchanged on `main` (24 Oct 2026). And even for a model the gate lets through, the runtime's PnC model rewrites only the transcript string, never `words[]`, which is all Diarist reads (section 2.2).
- **One cheap check could still change the post-pass answer.** Nobody has measured Nemotron's own punctuation at the shipped file setting (lookahead 13), or without `--endpointing`. Upstream bug reports show these models punctuating dictation fully. Spike step S0 measures this in about an hour, with no downloads.
- **Lead add-on: `Horizon-Labs/punctuation-restoration-small`.** It is an mmBERT-small token classifier released 3 Oct 2026. It covers all 28 of Diarist's listed languages, is Apache-2.0, and ships a 268 MB ONNX file with int8 embeddings. Its own card puts it ahead of the older open models on lowercased FLORES and TED input (TED talks: 0.706 F1 against 0.643 for the 1-800-BAD-CODE XLM-R model), though not on Europarl, which `kredor/punctuate-all` was trained on. It does punctuation only, so casing would come from rules on top of the ASR's own casing.
- **Challenger: `1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase`**, the model plan 12 picked. It does punctuation, true-casing and sentence boundaries in one model, but covers 22 of our 28 languages and its ONNX file is 1.11 GB in fp32.
- **Both are pure token classifiers.** Diarist applies their labels to its own word rows, so no word can be added, dropped, changed or moved by construction. Section 4.4 lists the invariants. Generative models (small LLMs, T5) are out unless forced onto the word list.
- **Integration: `ort` 2.0.0-rc.15 with `load-dynamic`, CPU only.** It runs in the post-pass, imports and Restore, after the echo gate and before segmentation, so segments start to follow sentences. It adds nothing to live latency and uses no VRAM. Estimated cost is 1 to 15 s per hour of audio, partly hidden behind the mic channel's GPU pass, and 0.4 to 0.6 GB of RAM while the job runs. The installer is unchanged if the 17 MB `onnxruntime.dll` comes as a downloaded component with the model.
- **The spike (section 7) uses `tools/eval`.** It scores per-mark F1 and casing against AMI's manual punctuation, FLEURS and the Zoom references, and proves that WER is unchanged by checking that the normalised word sequences are identical before and after.

## 1. The gap

| Transcript | Words | Marks (`.` `,` `?` `!`) per 100 words | Capitalised words | Kind | Source |
|---|---|---|---|---|---|
| Nemotron EN, Aug 14 (77 min), lookahead 1 | 12,266 | 0.2 | – | ASR | research/08 section 7 |
| Nemotron 3.5, same file | 11,896 | 2.5 | – | ASR | research/08 section 7 |
| Parakeet TDT v3, same file | 11,758 | 12.4 | – | ASR, drops speech | research/08 section 7 |
| Nemotron 3.5, AMI clip (50 to 350 s) | 479 | 0.8 | 6.1 % | ASR | research/12 section 3 |
| Nemotron EN, AMI clip | 531 | 0.2 | 5.8 % | ASR | research/12 section 3 |
| AMI ES2002a manual reference, whole meeting | 2,600 | 19.4 (299 `.`, 173 `,`, 33 `?`) | 17.3 % (330 of 332 sentence-initial words) | human | counted today from `ami_public_manual_1.6.2.zip` |
| Zoom transcripts of the six user meetings | 5,994 to 11,659 per session | 13.9 to 22.3 | 8.5 to 13.9 % | machine (Zoom) | counted today from the converted references (counts only) |

All the research/08 and research/12 numbers were taken at the runtime's default lookahead (1). The shipped file setting (13, research/12 section 1) has not been measured for punctuation. That is spike step S0.

## 2. Replacement: a speech model that punctuates by itself

### 2.1 What the runtime can run

NeMo-Speech.cpp has no release after v0.2.0 (2 Oct 2026). There is a nightly from 17 Oct, and `main` is at `e680fc3` (24 Oct 2026) [S1]. The README and `docs/asr/models.md` on `main` list four ASR models [S2]:

| Model | Punctuates itself | Streaming | Status for Diarist |
|---|---|---|---|
| Nemotron 3.5 ASR Streaming 0.6B | yes, per the card [S3] | yes | shipped; 2.5 % of words punctuated |
| Nemotron Speech Streaming EN 0.6B | yes, per the card [S4] | yes | default; 0.2 % |
| Parakeet TDT 0.6B v3 | yes ("self-punctuating") | no, full utterance only | experimental; drops speech |
| Parakeet CTC 1.1B | no (plain-text CTC), English | buffered | the one model the PnC hook would run for |

Canary-1b-v2 and Parakeet TDT v2 punctuate natively, but the runtime does not run them [S2].

`nvidia/parakeet-unified-en-0.6b` (7 Apr 2026, NVIDIA Open Model License, one 2,474,055,680-byte `.nemo`) is an English FastConformer-RNNT for offline and buffered streaming with "built-in support for punctuation and capitalization" [S41]. The runtime does not list it. research/17 (25 Oct 2026, same worktree) ranks "convert it with `convert_model.py`" as its spike 4. Whether it converts, whether it drops speech like other Parakeet models, and how much it punctuates on meetings are all unmeasured. In Diarist it would be English only, and research/17 plans it for file jobs only.

### 2.2 The PnC hook on `main`

- **The gate is unchanged.** `vocab_self_punctuates()` in `src/asr/recognizer.cpp` still returns true for any vocabulary with `.`, `?`, `!`, `।` or `॥`. `Postprocessor::Postprocessor` (`src/asr/postproc/pipeline.cpp`) still drops `--pnc-model` with the warning research/12 quoted. There is no flag to force it. `pipeline.cpp`, `pnc_runner.cpp` and `app/transcribe.cpp` are identical between v0.2.0 and `e680fc3`, and the only change in `recognizer.cpp` is how long the diarizer holds frames (#70) [S1][S5].
- **Even when it runs, it does not reach `words[]`.** `Postprocessor::apply()` passes only the transcript string to `PncRunner::postprocess()`. The word list goes through the profanity filter and ITN only (`apply_cpu`), and `make_alt` in `recognizer.cpp` copies the words out unchanged [S5]. Diarist builds segments, search and exports from `words[]` (research/08 section 1: `words[].word` "carries attached punctuation"). So a converted BERT on Parakeet CTC would add marks to the `text` field and leave Diarist's transcript as it is.
- **What the hook does, from the source:** it runs per final, with no context from the previous utterance. It capitalises with a byte-wise `toupper` on the first byte of a word, so non-ASCII initials (é, ü, Cyrillic) never change. It uses a WordPiece BERT only (`src/asr/pnc/pnc_runner.cpp`) [S5].

Upstream would have to change three things for this route to work: the gate, PnC on words, and a non-BERT model family. The owner could file that as an issue, as with issue 67.

### 2.3 Parakeet TDT v3: speech dropping is known and open

- transcribe.cpp issue #71, "multi-second span silently dropped", opened 7 Jul 2026, is still open [S6]. A second reproduction on 17 Aug 2026 found that moving the input by one 10 ms mel hop changes which span is lost, and that prepending silence moves the gap rather than removing it. The maintainer says the reference implementation shows the same thing, "possibly just an issue with the model itself".
- Hugging Face discussions on `nvidia/parakeet-tdt-0.6b-v3` report the same: #12 "Missing sentences" (13 Sep 2025) and #42 "The model sometimes drops full sentences" (8 May 2026, reproduced "in the official nemo_toolkit[asr] pipeline", with a second user on 31 Jul 2026). Both are open with no NVIDIA reply [S7].
- That matches research/08 section 8: a 46 s gap that a 2-minute cut does not repair. It is a model property, not a runtime bug. The replacement route stays closed.

### 2.4 Nemotron's own punctuation: why so little?

- Both model cards claim "built-in support for punctuation and capitalization". The 3.5 card says its synthetic labels got their punctuation and capitalisation from Qwen3-32B [S3][S4].
- Upstream reports show these models punctuating ordinary dictation fully. Issue #40 has a Nemotron 3.5 batch decode like "In a new work tree based on Maine, can you please have a look at how we package up profiles…" [S8]. Issue #22 shows Nemotron EN putting `?` and `,` on streaming finals [S9].
- On meetings we see 0.2 to 2.5 percent. There are three candidate causes, all unmeasured:
  - The conversational domain itself (*inference*: the PnC labels may be thin for spontaneous speech).
  - `--endpointing`. Issue #40 and the open PR #41 show the token-silence end-of-utterance firing mid-sentence and hard-resetting the decoder [S8][S10]. That changes marks and casing, though in their example it adds marks.
  - Quantisation. PR #46, merged 24 Oct 2026, found that the Q8 conversion of the pointwise convolutions in cache-aware RNNT models lost a passage of speech: in 1 of 17 shifted windows of one recording, against 0 of 17 with those weights kept in F16. It says "Existing GGUFs must be regenerated from the original checkpoint" [S11]. Neither Nemotron repository has a new GGUF since 10 Sep and 5 Aug [S12]. *Inference:* this is a word-accuracy lever in its own right and worth its own measurement when a release carries #46.
- The file CLI keeps the marks it gets: `append_late_punctuation()` in `app/transcribe.cpp` attaches punctuation that arrives after an endpoint to the last word, at v0.2.0 and on `main` [S13]. *Inference:* a grep of `app/serve.cpp` and `server/http/http_server.cpp` at v0.2.0 finds no use of `late_punctuation`, so live finals over the realtime WebSocket may lose end-of-utterance marks. That is the symptom issue #22 describes over gRPC.

### 2.5 Verdict

No replacement exists today. Parakeet TDT v3 drops speech, and the runtime cannot run the other self-punctuating NVIDIA models.

There are two open paths. One is Nemotron's own punctuation under a different decode setting or with re-converted GGUFs; S0 measures it. The other is Parakeet Unified EN for English file jobs, if it converts (research/17 spike 4). Even if either path works, it helps only file jobs and the post-pass, and Parakeet Unified only in English. Neither reaches older meetings or a model the user drops in, and neither adds casing beyond what the model emits. So the add-on is worth building in either case. A positive S0 changes the merge policy (section 4.4) and could make the add-on optional for the post-pass.

## 3. Add-on candidates (October 2026)

"Ours" counts the 28 languages behind the 30 entries in `manifests/languages.json` (ready and broad tiers; Spanish and Portuguese each have two locales). "Label-only" means the model is a token classifier, so Diarist can apply its labels to its own words without letting it rewrite text.

| Id | Model (Hugging Face repo, revision) | Published, last change | Languages (ours) | ONNX file | Licence | Marks | Casing, sentence boundaries | Reported accuracy (test set, who measured) | Label-only |
|---|---|---|---|---|---|---|---|---|---|
| A | `Horizon-Labs/punctuation-restoration-small` @ `ee31fdf` (mmBERT-small, 140M) | 3 Oct 2026, v1.1 on 9 Oct | 88 (28 of 28) | 268 MB int8 embeddings, 563 MB fp32 | Apache-2.0 (base mmBERT MIT) | `.` `,` `?` `:` `-`, script variants in its helper | no casing; `.` is the boundary | punctuation F1 on lowercased input: FLORES 88 languages 0.796, TED 25 languages 0.706, Europarl 0.830; cased input 0.842, 0.786, 0.878 (FLORES-200, TED2020, Europarl; the model's author) [S14] | yes |
| A-base | `Horizon-Labs/punctuation-restoration-base` @ `63ec204` (307M) | same | same | 641 MB int8 embeddings, 1.23 GB fp32 | Apache-2.0 | same | same | lowercased: 0.804, 0.722, 0.839 [S15] | yes |
| B | `1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase` @ `d1769a5` (XLM-R base, 278M) | 7 May 2023, last 15 Jul 2023 | 47 (22 of 28; no vi, nb, cs, sk, sv, da) | 1.11 GB fp32 only | Apache-2.0 | `.` `,` `?` `¿` `<ACRONYM>`, CJK, Arabic, Hindi and Amharic marks | per-character true-casing; separate sentence-boundary head | English, News Crawl held-out: `.` 92.4, `,` 78.8, `?` 80.5; casing UPPER F1 94.9 and boundary 99.75 given reference punctuation (the author) [S16]. Horizon's run, lowercased: FLORES-12 0.818, FLORES-88 0.689, TED 0.643 [S14] | yes, if Diarist applies the labels |
| C | `1-800-BAD-CODE/punctuation_fullstop_truecase_english` @ `b26fd1c` (6 layers, d 512) | 11 Mar 2023 | English | 210 MB fp32 | Apache-2.0 | `.` `,` `?` `<ACRONYM>` | true-casing, boundaries | News Crawl: `.` 91.8, `,` 78.2, `?` 75.6; UPPER 93.8 and boundary 92.5 on predicted punctuation (the author) [S17] | yes |
| D | `1-800-BAD-CODE/punct_cap_seg_47_language` @ `1b9d51f` | 22 Feb 2023 | 47 | 233 MB | Apache-2.0 | as B | as B | the author points to B instead; Horizon: FLORES-12 lowercased 0.565 [S14][S18] | yes |
| E | `oliverguhr/fullstop-punctuation-multilang-large` (XLM-R large, 560M) | 2022, last 16 Nov 2023 | en de fr it (4) | 2.24 GB fp32 | MIT | `.` `,` `?` `-` `:` | none | Europarl, English: `.` 0.948, `,` 0.819, `?` 0.890 (the author) [S19]; Horizon FLORES-12 0.838 lowercased | yes |
| F | `kredor/punctuate-all` (XLM-R base) | 9 Apr 2022, last 26 Apr 2024 | 12 European (11 of ours) | none (PyTorch only) | MIT | as E | none | Europarl: `.` 0.95, `,` 0.86, `?` 0.86 (in domain) [S20] | yes |
| G | `ldenoue/punctuator-multilingual-minilm-v2` (33.8M) | 21 Sep 2026 | 13 European (12 of ours) | 34 MB int8, 135 MB fp32 | MIT (card warns about YouTube caption rights) | `,` `.` `?` `!` | LOWER, CAP, UPPER per word | own eval: punctuation F1 0.58, case F1 0.875; trained on FLEURS text [S21] | yes |
| H | NeMo `punctuation_en_bert` (NGC, BERT-base) | v1.0.0rc1, 4 Apr 2023 | English | `.nemo` 387 MB | NGC Terms of Use | `.` `,` `?` | first-letter casing | "F1 77 %, macro", on an internal set; trained on Tatoeba, Gutenberg and Fisher (LDC) [S22] | yes |
| I | sherpa-onnx `online-punct-en-2024-08-06` (Edge-Punct-Casing CNN-BiLSTM) | 6 Aug 2024 | English | 7.1 MB int8, 28 MB fp32 | code Apache-2.0; weights not stated | `.` `?` in the examples (full set not listed) | casing | IWSLT2011, "9 % relative" over the best non-Transformer model; 13 ms per short sentence, int8, one thread [S23][S24] | yes |
| J | sherpa-onnx CT-Transformer zh-en (FunASR) | 12 Apr 2024 | zh, en | 72 MB int8, 281 MB fp32 | not stated | `，` `。` `？` | none | none [S23] | yes |
| K | Silero `te` | – | en de ru es | PyTorch | CC BY-NC 4.0 [S25] | – | casing | – | not checked |
| L | Small LLMs (Qwen3 0.6 to 1.7B, Gemma 3 1B, GGUF) | – | many | 0.5 to 1.5 GB | Apache-2.0 / Gemma terms | any | any | none comparable | **no** (generative) |

Notes on the table:

- **The numbers do not compare across rows.** Each author uses a different test set, and most are written text. The only table that scores many models with one script is Horizon's own, so it is an interested party. Horizon scored the 1-800-BAD-CODE models through the `punctuators` package on lowercased input only, and counted "about 2 % of words" that package changed beyond casing as unpunctuated. That lowers B's scores somewhat [S14]. The spike has to measure all of them on our audio.
- **Lowercased input is the relevant column.** The ASR output is mostly lowercase (6 percent capitalised), so the "lowercased" numbers apply. On TED (spoken style), A scores 0.706, A-base 0.722, B 0.643 and F 0.654. Per mark for A on lowercased TED: `.` 0.788, `,` 0.626, `?` 0.709, `:` 0.504, `-` 0.062 [S14].
- **On the six languages B lacks**, A's FLORES lowercased F1: Czech 0.915, Slovak 0.910, Danish 0.891, Swedish 0.859, Norwegian Bokmål 0.846, Vietnamese 0.776 [S14]. A also covers the adaptation-tier Greek, Lithuanian, Latvian, Maltese, Slovenian and Hebrew; Nynorsk only as Norwegian (*inference*). It has no Thai.
- **Provenance of A.** Horizon-Labs published 35 models (classifiers, embedders, rerankers) between 23 Sep and 14 Oct 2026. The punctuation pair had about 600 downloads each when checked. Training data is FineWeb and FineWeb-2 (ODC-BY) plus 49,144 "spoken-style" windows the card says an LLM wrote. The ONNX check in the repository reports 99.96 percent label agreement between the int8 and fp32 files, at 39 against 52 ms per document [S14][S26]. A is new and unproven, and S4 and S5 must confirm its card on our audio.
- **What label-only means in practice.** A predicts one label per word, on its last token [S14]. B and C predict per subword, so a mark can land inside a word. B's `<ACRONYM>` label inserts a period after every character of a subword ("pm" becomes "p.m.", but "us" can become "U.S."). The `punctuators` package and A's `punctuate.py` both rebuild the text: they strip existing marks and apply acronyms. Diarist must not use their string output; it applies the labels itself (section 4.4) [S16][S14].
- **B's graph outputs decisions, not scores.** The ONNX file returns label ids for pre-punctuation, post-punctuation, casing and boundaries [S16]. A threshold (for example "fewer commas") would need graph surgery. A outputs logits (`ModernBertForTokenClassification` [S26]).
- **Ruled out:**
  - L: generative models can rewrite words. They would need forced decoding over the word list. *Estimate:* 9,000 to 11,000 words per hour means more than 12,000 decode steps per hour, which is minutes per hour on the CPU or 1 to 2 GB of VRAM beside the ASR.
  - K, plus `Jvachier/punctuation-casing-en-7m`: non-commercial licences.
  - H: NGC terms and LDC data (research/12 section 3).
  - `vosk-recasepunc` (en, ru, de): Apache-2.0 but PyTorch only, 1.1 to 1.6 GB [S27].
  - J: Chinese and English only, licence not stated.
  - G: low self-reported punctuation F1, and it was trained on FLEURS text, so it cannot be scored on FLEURS.

## 4. Integration design (if the add-on wins)

### 4.1 Inference library

| Option | What it costs | Verdict |
|---|---|---|
| `ort` 2.0.0-rc.15 (pykeio, MIT or Apache-2.0, 19 Oct 2026) with `load-dynamic`, CPU execution provider [S28] | `onnxruntime.dll` 1.31.0 (MIT, 9 Oct 2026): 17,253,176 bytes, 6.3 MB compressed [S29]. MSRV 1.92 against the workspace's `rust-version = "1.90"` (bump it, or pin rc.13, MSRV 1.88). rc.15 fixes "over 30 instances of undefined behavior" | **Use.** `default-features = false, features = ["std", "load-dynamic", "api-24"]`, with `ort::init_from(<absolute path>)` before anything else |
| DirectML execution provider | Microsoft Learn says DirectML is "in sustained engineering"; new work goes to Windows ML [S30]. The ORT DirectML NuGet stops at 1.24.4 (17 Mar 2026), DirectML itself at 1.15.4 (28 Oct 2024) [S31] | No. Not needed for seconds of work, and the GPU is busy with the ASR in the post-pass |
| CUDA execution provider | the CUDA EP plugin zip is 149 to 239 MB (8 to 9 Oct 2026) [S29] | No |
| `tract-onnx` 0.23.9 (pure Rust, MIT or Apache-2.0) | no DLL. *Inference:* slower than ORT's MLAS kernels, and ModernBERT's export may use ops it lacks | Fallback if the DLL is unwanted; check op coverage in S7 |
| `candle-transformers` 0.11.0 | native Rust, safetensors. B's custom NeMo heads would need porting | Not now |
| Runtime hook (`--pnc-model`) | dead (section 2.2) | No |

- **Load ORT by absolute path.** Windows ships its own `onnxruntime.dll` in System32; on the development PC it is version 1.17 (`1.17.260914…`). With `load-dynamic` and an absolute path the app never picks it up. *Inference:* `api-24` makes an accidental 1.17 load fail with a clear error instead of misbehaving.
- **Threads.** Use 4 intra-op threads. *Inference:* this leaves cores for the echo canceller and the UI.
- **Tokenisers.** Use `tokenizers` 0.23.2 (Apache-2.0) with `default-features = false, features = ["onig"]`. It reads A's `tokenizer.json` (34 MB, 256k vocabulary) and gives the offsets the word mapping needs [S32]. For B and C, convert `sp.model` into a `tokenizer.json` once, in the spike, with exact token-id parity checked (S3). That avoids the C++ `sentencepiece` build. B's SentencePiece model was "un-hacked" so its ids are the raw SentencePiece ids, not fairseq's [S16]. The usual `XLMRobertaTokenizerFast` conversion would therefore be off by the fairseq offset.
- **Module.** `crates/core/src/punct/`:
  - `mod.rs`: a `Punctuator` trait (`labels(&[&str], lang) -> Vec<WordLabel>`) and the label types.
  - `apply.rs`: chunking, label application and the invariant checks. Pure code, unit-tested without a model.
  - `onnx.rs`: the `ort` session, behind a cargo feature.

### 4.2 Where it runs

Line numbers are at `fc597ba`.

| Path | File, insertion point | Notes |
|---|---|---|
| Post-pass after Stop, loopback channel | `crates/core/src/live/postpass.rs` `run_post_pass`: the loopback words are final at `let lb_words = lb_words?;` (line 1023) | Punctuate on a CPU thread while the mic channel is transcribed on the GPU (lines 1036 to 1042), which hides its cost (*inference*) |
| Post-pass, mic channel | after the echo step (`mic_words = kept;`, line 1087) and the unknown-token check (lines 1096 to 1102), before `build_transcript_channels` (line 1106) | After the gate, so dropped echo words never take part. Write `engine_info.punctuation` beside `"segmentation"` (line 1121) |
| Post-pass that promotes the live run (`transcribe = false`, `post_pass_mode = on_issues`, line 1161) | new helper `punct::rebuild_run`, on the pattern of `crates/core/src/live/restore.rs`: words by channel from the stored run, `build_transcript_channels` with the run's segment options, a new run of the same kind, `carry_over`, `set_current_run` | The live run was materialised unpunctuated (`live/mod.rs` lines 1033 and 1055, `store/live.rs` line 139) |
| File import | `crates/core/src/pipeline/import.rs`, before `build_transcript` (line 394) | One channel, diarized speakers |
| Restore of echo words | `crates/core/src/live/restore.rs`, before `build_transcript_channels` (line 180) | Restored words are raw and the context changes, so re-punctuate the mic channel |
| Older meetings, a model change, the setting switched off | `rebuild_run` as a `JobQueue` job; the old run stays (docs/02: "Nothing is ever deleted to make room for a better run") | Works without audio, from `words.raw_text` (section 4.5) |
| Live finals (phase 2) | a display-only overlay in the desktop app's live module: when a final arrives, punctuate the channel's last turn plus that utterance on a worker thread and emit display text per word as a new `live:*` event (`live/views.rs` types) | Never written into `live_utterances`, because the post-pass's transfer gate reads raw live words (`postpass.rs` `live_mic_words`) and the live echo gate compares revisions by exact text (`live/echo_gate.rs` line 126). The previous utterance's last mark is revised when its right context arrives. The model stays resident while recording |

### 4.3 Turns, windows and subwords

- **Units.** Per channel, in time order. Two variants for the spike to compare:
  - per speaker turn: same label, broken at a label change or a gap of more than 1,500 ms, which is the segmenter's own split;
  - the channel stream with only the gaps as hard breaks.
  *Inference:* the second may be better, because the diarizer flips labels mid-sentence near speaker changes (WDER near changes 5 to 8 percent) and a per-turn window would then add a false period.
- **Input.** Words joined by single spaces, with the ASR's own trailing marks stripped. Lowercased for B and C, which were trained that way. For A, both cased and lowercased; A was trained on 70 percent lowercased text [S26].
- **Windows.**
  - B: at most 256 tokens including BOS and EOS [S16].
  - A: 512 tokens at most (trained with `max_len` 512; its helper uses 400 tokens with a 50-token overlap) [S26].
  - Both: 32 to 64 tokens of context on each side, keeping only the centre's labels. Batch short turns with padding.
- **Subwords to words.** Encode the joined text with offsets (`tokenizers` `Encoding::get_offsets`, `word_ids`) and map every token to a word index.
  - A word's post-mark is its last token's label; a pre-mark (`¿`) is its first token's label.
  - B's and C's casing flags are per character of each subword. Apply them only where the subword's normalised characters equal the word's lowercased characters. Otherwise only first-letter casing applies; SentencePiece's NFKC normalisation can change character counts.
  - Marks predicted inside a word are dropped and counted. For continuous scripts (zh, ja), marks go only at the ends of the runtime's word units.

### 4.4 Applying labels: the invariants

1. Word count, order, `start_ms`, `end_ms`, the speaker and `raw_label` never change.
2. Letters never change except lower to upper case. A letter the ASR capitalised is never lowered.
3. Per word, `norm(new) == norm(old)` must hold for the echo gate's `norm` (`crates/core/src/audio/echo/text.rs` line 124). The app checks this at run time; the spike also checks the harness's `normalize_token` (`tools/eval/common.py`). A word that fails keeps its ASR text and is counted in `engine_info.punctuation.rejected_words`.
4. Only marks from an allow-list per script are used: `.` `,` `?` and their script forms (`。` `，` `？` `、` `।` `؟` `،`). `:` and `-` only if the spike shows precision of at least 0.75; A's `-` scored 0.06 F1 on TED. `<ACRONYM>` is off.
5. ASR marks: either keep them where the ASR put one and let the model fill the rest, or replace them with the model's labels. S5 measures both. *Inference:* the ASR heard the prosody, so its rare `?` may be the more precise one.
6. Casing with A, which has no casing head: capitalise the first cased letter of a stream's first word and of every word after a sentence-final mark, in scripts with case (Latin, Cyrillic, Greek, Armenian). Otherwise keep the ASR's casing, which already gives "I" and some names. With B or C, apply their flags as upgrades only.

### 4.5 Data model

- **Display text in the existing columns.** `words.text` and `segments.text` hold the punctuated text, so the UI, search snippets and exports need no change.
- **A new nullable column, `words.raw_text`** (migration `0004_raw_text.sql`). It holds the ASR's text only where it differs. `NewWord`, `TranscriptWord` and `Word` gain the field. Without it, switching punctuation off or changing the model on a meeting older than the 30-day audio retention could not recover the ASR's casing.
- **`engine_info.punctuation`** records: `{model, revision, sha256, runtime: "onnxruntime 1.31.0", language, applied, reason, words, marks_added, case_upgrades, rejected_words, wall_s, threads}`.
- **Search is unaffected.** `segments_fts` uses `tokenize='unicode61 remove_diacritics 2'`. That tokenizer treats Unicode punctuation as separators and folds case, so search hits are the same and only snippets gain punctuation [S33].
- **The echo gate is unaffected.** It runs before punctuation in the post-pass, and its `norm` lowercases and keeps alphanumerics only. A punctuated run would match the same way.
- **Carry-over is unaffected.** It matches on time and speech regions (docs/02), not on text. Segment boundaries will move, because sentence ends now split segments, and carry-over already replays edits at word level.
- **Segmentation becomes sentence-shaped.** `transcript::segment` already splits after sentence punctuation followed by a pause of more than 500 ms (`SegmentOptions::sentence_gap_ms`).
- **Code fix:** `ends_sentence` (`crates/core/src/transcript/mod.rs` line 150) knows `. ? ! … 。 ？ ！` but not `।` `॥` `؟` `۔` `።` `፧`. Hindi and Arabic sentence ends would therefore never split a segment.

### 4.6 Exports

- Markdown, text, SRT and VTT render `segments.text` (`crates/core/src/export.rs`: `to_markdown` line 227, `to_text` 265, `to_srt` 392, `to_vtt` 415), so punctuation flows through with no change. SRT and VTT cues get shorter and end at sentences.
- JSON (`to_json`, line 330) carries the punctuated word text. Adding `raw_text` to `JsonWord` when present is an additive change.
- RTTM is unaffected.

### 4.7 Settings, models and languages

- **Settings.**
  - `transcript.punctuation`: `auto` (the default; on when a punctuation model is installed and covers the meeting's language) or `off`.
  - `transcript.punctuation_live`: `off` in phase 1.
  - `selected_model.punctuation`, like the other roles.
- **Models.** `ModelRole::Punctuation` already exists (`crates/core/src/types.rs` line 269), and so does the `punctuation` role in `model_files` (migration 0001).
  - The scanner reads GGUF headers only. It needs a folder rule: an `*.onnx` file plus a `tokenizer.json` plus a small `punct.json` sidecar that gives the labels, the token limit, the languages, the input casing and whether the model does casing. Dropping such a folder into `models\` then works, in keeping with the "user owns the folders" principle.
  - `ModelEntry` carries one file (`file`, `url`, `sha256`). So either ship each model as a zip, like the engine, or add a file list.
- **Languages.**
  - Map the meeting's language to its base code (`es-US` becomes `es`). For `auto`, use the run's detected language.
  - A language the sidecar does not list is left unpunctuated, as today, and `engine_info.punctuation.reason` is `"language"`.
  - With A, Thai is never covered. With B, Vietnamese, Norwegian, Czech, Slovak, Swedish and Danish are not covered.

## 5. Performance budget

| Item | Target | Estimate (*inference* unless measured) |
|---|---|---|
| Live latency | 0 ms added | 0 in phase 1: nothing runs live. Phase 2 overlay: 10 to 50 ms of CPU per utterance on a worker thread, off the engine path |
| Post-pass and import, extra wall time | ≤ 15 s per hour of audio | A: 3 to 7 s; B: 4 to 15 s; C: 1 to 3 s per hour. The loopback part runs during the mic channel's GPU pass |
| RAM while the job runs | ≤ 0.8 GB extra | A int8: 0.4 to 0.6 GB. B fp32: 1.4 to 1.8 GB, so B ships only as int8 (about 280 MB with all weights int8). The session is dropped after each job |
| VRAM | 0 | 0 (CPU only) |
| Disk | – | A: 268 MB plus a 34 MB tokenizer. B int8: about 280 MB plus a 5 MB `sp.model`. DLL: 17 MB |
| Installer (7,430,483 bytes for beta.4 [S34]) | unchanged | +0 if the DLL and the model come as a component download; +about 6.3 MB if the DLL is bundled. The exe grows 2 to 4 MB for `ort`, `tokenizers` and `onig` |
| First-run download (about 870 MB today, docs/04) | owner's call | +about 0.3 GB if the punctuation model is included |

How the time estimates were reached:

- **Words per hour.** 9,000 to 11,200 (Aug 2: 6,595 words in 44 min; Sep 5: 8,387 in 45 min; Aug 14: 11,887 in 77 min).
- **Tokens per hour.** 12,000 to 18,000, at 1.3 to 1.6 tokens per word (*inference*).
- **Work per token.** About twice the non-embedding parameters, from the configs: A about 42 M (140 M minus 98 M of embeddings), so 85 MFLOP per token; B about 85 M, 170 MFLOP; C about 19 M, 38 MFLOP.
- **CPU speed.** 0.2 to 0.5 TFLOP/s effective on 4 to 8 Zen 3 threads with ORT's MLAS kernels (*inference*).
- **Caveat.** A's int8 file quantises only the embeddings, so it computes in fp32, as its 39 against 52 ms check shows [S26]. A full dynamic int8 quantisation of the MatMuls is a spike variant.

## 6. Measurement

### 6.1 Metrics

- **Alignment.** Align the hypothesis words to the reference on normalised words, using the harness's `normalize_token` and a Levenshtein alignment of the time-ordered sequences, speaker-agnostic. Score only matched pairs, where the normalised texts are equal.
- **Punctuation.** Classes after each word: none, period (`.` `!` `…` `。` `।`), comma (`,` `،` `、` `，`), question (`?` `？` `؟`, Greek `;`), colon, dash.
  - Precision, recall and F1 per class.
  - Micro F1 over period, comma and question.
  - Boundary F1, with period, question and exclamation merged into one "sentence end" class. This is what segmentation uses.
- **Casing.** On matched pairs whose reference starts with a cased letter:
  - first-letter case accuracy;
  - F1 of the "upper" class;
  - recall of sentence-initial capitals;
  - violations, where an ASR capital was lowered. This must be 0.
- **Two settings per model.** Oracle: the model on the reference's own words, lowercased with marks removed. This measures the model alone. End to end: the model on the ASR's words. This is what users get.
- **WER unchanged.** The normalised word sequence of every session must be identical before and after punctuation. That is stronger than equal WER and implies identical cpWER, tcpWER and WDER. Check it once more through `run.py --metrics-only` on a punctuated `hyp.seglst.jsonl`.
- **Speed and memory.** Seconds per hour of audio, words per second, model load time and peak RSS.

### 6.2 References with punctuation

| Set | What it has | Size here | Licence | Caveats |
|---|---|---|---|---|
| AMI ES2002a manual annotation (`ami_public_manual_1.6.2.zip`, already fetched) | human punctuation as `punc="true"` tokens, casing, word times | 2,600 words, 505 marks | CC-BY-4.0 | one far-field meeting. `ami_fetch.py` drops the punctuation tokens today (line 91), so it needs an option to keep them |
| FLEURS `raw_transcription` (`.work/fleurs/<lang>/refs.jsonl`, already fetched) | written sentences, cased and punctuated | about 1,000 words per language in 8 languages | CC-BY-4.0 [S35] | read speech. Every sentence ends at a 1.5 s gap, so feed the whole slice as one stream (no gap breaks), or boundaries come for free. 40 to 55 sentences per language, so per-mark F1 moves by several points |
| Zoom transcripts (`.work/refs/<session>.ref.seglst.jsonl`) | Zoom's machine punctuation and casing; `zoom_vtt_to_ref.py` keeps the raw cue text | six sessions, 6,000 to 11,700 words each | private (numbers only in docs) | agreement with another machine, not truth; cue-level times |
| Earnings-21 (`revdotcom/speech-datasets`) | token-level `punctuation` and `case` columns in `.nlp` files | earnings calls, English | CC BY-SA 4.0 [S36] | optional: a second human-labelled English set with long monologues and Q&A |
| VoxPopuli `raw_text` | parliament speech with raw text | several EU languages | CC0 [S37] | optional: spoken-style multilingual text |
| FLORES-200 and TED2020 (text) | what Horizon's card scored on | – | – | only to reproduce the card's numbers. Model G trained on FLEURS text, so it cannot be scored on FLEURS |

## 7. Recommendation, candidates and spike

### 7.1 Recommendation

**Build the add-on.** A token classifier over Diarist's finished words keeps every word and time by construction. It covers past meetings as well as new ones, works with any ASR model a user drops in, and costs seconds per hour on the CPU with no VRAM. The replacement does not exist today (section 2.5).

- **Scope.** Phase 1: post-pass, imports, Restore and a rebuild for older meetings. Phase 2: a live overlay.
- **Lead and challenger.** Lead with A plus casing rules. Keep B as the challenger in case true-casing proves worth its size and its six missing languages.
- **Decision.** Take the final decision from S4 to S7, not from the model cards.
- **Run S0 first.** If Nemotron's own marks at the shipped setting turn out near the add-on's F1, keep them (merge policy 5 in section 4.4) and still ship the add-on for casing, for live and for older meetings.

### 7.2 Spike candidates

| Candidate | Repository and revision | Files (bytes, SHA-256) | Licence |
|---|---|---|---|
| A | `Horizon-Labs/punctuation-restoration-small` @ `ee31fdfc5304f876ea3b0dd72d30eda7e940b2b1` | `onnx/model_quantized.onnx` (268,414,381, `5679870be92c7fa47c9be9b2872fac389ef28d43d2c33f35e1315b93a82f4ace`); `onnx/model.onnx` (563,106,940, `751b4ba021f23d18d9cf7ac00938ce497a5ae58baa08b4fe5802f86e5d2e8940`); `tokenizer.json` (34,363,286, `f34b44a6dc8a0eef651bfa10e8325057530152606a69d9c3b6ae02efb1558f0a`); `config.json` (2,110); `punctuate.py` (3,740, for its script-mark mapping) | Apache-2.0; base `jhu-clsp/mmBERT-small` MIT [S38] |
| A-base (optional row, same runner) | `Horizon-Labs/punctuation-restoration-base` @ `63ec204eeed976c17dee33cb4677753ba2c9b605` | `onnx/model_quantized.onnx` (641,137,205, `42f52bb955a7e158e629b4ee59d22ae75300b2ec37b7fb01fea80c09d9566fce`); `tokenizer.json` (same as A) | Apache-2.0 |
| B | `1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase` @ `d1769a597ce8dfaa070d436bc67d4ee761f58884` | `model.onnx` (1,112,481,438, `c43ca686dabc237c3b06be834b9423c07580fef7e2b1a6c09976f7d60caa5d89`); `sp.model` (5,069,059, `7f944d0be93b275f62e1913fd409f378ddbba108e57fe4a9cb47e8c047f6bef1`); `config.yaml` (531) | Apache-2.0 |
| C | `1-800-BAD-CODE/punctuation_fullstop_truecase_english` @ `b26fd1c40e88678859048898218ea4edcc24c84a` | `punct_cap_seg_en.onnx` (209,532,928, `dd922d459da618cd324280889740608b76fb3e9e61d3f402291be1251f91421b`); `spe_32k_lc_en.model` (587,902, `9e86d0263de80b3b68327a21f5350c8cdf846e4c4400253c9baf05e3d44871c3`); `config.yaml` (302) | Apache-2.0 |

All four repositories are public and not gated. Fetching A, B and C is about 2.2 GB, plus 0.7 GB for A-base. None of them carries NGC or other click-through terms.

### 7.3 Spike steps

Run from `tools/eval` after the README setup, with the same environment variables as research/12 section 8. All new code is dev-only: Python under `tools/eval`, a separate `uv` dependency group with `onnxruntime==1.31.*`, `tokenizers`, `sentencepiece`, `onnx` and `psutil`. No product code changes, and no private text goes into reports.

- **S0. Nemotron's own punctuation at the shipped setting (no downloads, about 1 hour).**
  - Make sure the shipped runs exist with their `engine.words.json`: `uv run python -I run.py --config shipped_en --sessions ami,aug2,sep5` and the same for `shipped_n35`.
  - Count: `uv run python -I levers.py punct --words .work/runs/shipped_en/ami/engine.words.json .work/runs/shipped_n35/ami/engine.words.json …`.
  - Add `--no-endpointing` and `--no-stream` switches to `nemo_direct.py`; it always passes `--stream --endpointing` today.
  - On the AMI 5-minute clip run both models at lookahead 13 in three modes: streaming with endpointing (as shipped), streaming without endpointing, and whole-file without `--stream`, which is clip-only because of whole-file memory. For example: `uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr nemotron-en --out .work/spikes/punct/en_rc13_noeou --no-endpointing -- --asr.streaming.rnnt_right_context 13`.
  - Score with `punct_score.py` from S1 against AMI, with the clip offset of 50 s.
  - If streaming without endpointing raises the marks substantially, run it on Aug 14 as well and check it drops nothing (research/08's empty-minute check) before it counts as an option.
  - research/17 spikes 1, 2 and 4 (GGUFs re-converted with F16 pointwise weights, the `main` runtime, file jobs without `--endpointing`, Parakeet Unified EN) produce exactly the transcripts this step needs. Run `levers.py punct` and `punct_score.py` on their outputs too, rather than repeating them.
- **S1. References.**
  - Add `--punct` to `ami_fetch.py` to write `ES2002a.ref.punct.jsonl`: each word with the `punc="true"` tokens that follow it attached, and its original casing.
  - Write `punct_score.py` implementing section 6.1, with synthetic tests in `test_metrics.py` style. Inputs are a reference (AMI, FLEURS `refs.jsonl`, Zoom seglst) and a words JSON. Outputs are per-mark P, R and F1, boundary F1, casing metrics and the invariant counts, as JSON, plus a Markdown table without text.
- **S2. Fetch.** `punct_fetch.py` downloads the files of section 7.2 at the pinned revisions into `.work/punct/models/<repo>/<revision>/`, checks every SHA-256, and stores the licence line from the card. Per the simplewall note, a new interpreter or executable may need the owner's firewall approval first.
- **S3. Tokeniser parity (B, C).**
  - Convert each `sp.model` to `tokenizer.json`, using `tokenizers`' SentencePiece Unigram conversion on the raw ids, not `XLMRobertaTokenizerFast`.
  - Check that the ids equal `sentencepiece`'s on every reference text and on all `engine.words.json` texts, separately with and without lowercasing. The target is 0 mismatches.
  - For A, check that `tokenizers` offsets map every token to exactly one word on the same texts.
- **S4. Oracle runs.**
  - `punct.py --model A|A-base|B|C --input ref --sets ami,fleurs:de,es,fr,it,pt,hi,ja,en,zoom:all`, with C on English only.
  - Variants for A: cased against lowercased input.
  - Variants for B: fp32 against our own dynamic int8 (`onnxruntime.quantization.quantize_dynamic`, QInt8, all MatMul and Gather). Report label agreement with fp32 and the file size.
- **S5. End-to-end runs.**
  - `punct.py --model … --input hyp --configs shipped_en,shipped_n35 --sessions all`, plus the FLEURS runs of `n35_rc_13` (`run.py --config n35_rc_13 --fleurs all` if missing).
  - Variants: per-turn windows against the channel stream; ASR marks kept against replaced; with or without the sentence-start casing rule for A.
  - It writes punctuated words JSON and `hyp.seglst.jsonl` under `.work/punct/runs/<model>/<config>/<session>/`.
- **S6. Invariants and WER.** For every word, check that the echo `norm` (lowercase, keep alphanumerics and spaces) and `normalize_token` are unchanged, and that word count, order and times are identical. Then check per session that the normalised sequence before equals the one after. Finally, `run.py --metrics-only` over one punctuated session must give an identical cpWER, tcpWER and WDER (point it at the punctuated seglst with a temporary config). Every count must be 0.
- **S7. Speed and memory on this PC.**
  - Use the Python ORT CPU execution provider at intra-op threads 1, 2, 4 and 8, batch 1 and 8, and windows of 256, 400 and 512 tokens.
  - Inputs: Aug 14 (77 min, the longest) and the full set. Report seconds per hour of audio, model load time and peak RSS (psutil).
  - Run once with the GPU idle and once while `run.py` transcribes another session, to see contention with the ASR's CPU threads and the echo canceller.
  - Optionally: A in `tract` (op coverage only).
- **S8. Report.** Write `docs/eval/<date>-punct.md` with the tables of S0 and S4 to S7: numbers only, sessions under their docs/05 labels, plus the decision of section 7.4.

### 7.4 What decides

1. Rule out any candidate with a non-zero count in S6.
2. Rule out any candidate above 15 s per hour of audio at 4 threads, or above 0.8 GB peak RSS, in S7.
3. Among the rest, the highest end-to-end micro F1 over `.` `,` `?` on AMI and on the Zoom sessions wins (S5). Use boundary F1 as the tie-breaker. FLEURS decides per language whether a mark ships: a precision of at least 0.75 keeps the mark, otherwise it is dropped for that language.
4. Casing: if B's or C's true-casing beats "ASR casing plus sentence-start rule" by more than 2 points of upper-class F1 on AMI and FLEURS en, de, fr and es, casing is worth the bigger model. Otherwise ship A plus the rule.
5. If S0 shows the ASR's own marks at least as precise as the winner's, keep the ASR's marks where present (policy 5 in section 4.4).

## 8. Questions only the owner can answer

1. **Licences and hosting.**
   - A is Apache-2.0 from an organisation three weeks old, with training text partly written by an LLM. Is that acceptable to ship as the recommended model?
   - Where should a derived file live, for example B quantised to int8: the `ebt55/diarist` releases or a Hugging Face repository under the owner's account?
   - NGC models stay out unless the owner accepts the NGC Terms of Use (research/12 section 3).
2. **Languages.**
   - Must all 28 listed languages get punctuation, which needs A? Or is B's 22 enough, with Vietnamese, Norwegian, Czech, Slovak, Swedish and Danish left unpunctuated?
   - Should the adaptation tier (Greek, Hebrew, Maltese, Slovenian, Lithuanian, Latvian) be punctuated when a user adds it?
3. **Casing ambition.** Is sentence-start casing on top of the ASR's own casing enough? Or are names and German nouns wanted, which only B or C model?
4. **The ONNX Runtime DLL.** Bundle it in the installer (+about 6.3 MB), or download it with the model as a component, keeping the installer at 7.4 MB?
5. **Defaults.** Should punctuation be on by default when installed, and should the first-run download include the model (+about 0.3 GB to the 870 MB)?
6. **Live punctuation.** Is it wanted at all, given that it holds 0.4 to 0.6 GB of RAM while recording and the post-pass replaces the live text anyway?
7. **Older meetings.** Should Diarist offer "Add punctuation" on meetings recorded before the feature?
8. **Marks.** Only `.` `,` `?`, or also `:`? A predicts no `!`.
9. **Upstream.** Should the owner file a NeMo-Speech.cpp issue asking for PnC on `words[]` and a way past the self-punctuation gate (section 2.2)?
10. **Optional reference check of Nemotron.** Should the spike download the original Nemotron checkpoints (2.4 GB each, OpenMDW or NVIDIA Open Model License) to compare their punctuation in NeMo or Transformers with the runtime's? It would tell whether the low rate is the model or the runtime.

## Sources

Accessed 25 Oct 2026 unless stated; dates in brackets are publication or last change.

- [S1] NeMo-Speech.cpp releases and commits: https://github.com/NVIDIA/NeMo-Speech.cpp/releases (v0.2.0, 2 Oct 2026; nightly, 17 Oct 2026), https://github.com/NVIDIA/NeMo-Speech.cpp/commits/main (`e680fc3`, 24 Oct 2026)
- [S2] NeMo-Speech.cpp README and ASR model docs, `main`: https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/README.md, https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/docs/asr/models.md, https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/docs/asr/customization.md
- [S3] Nemotron 3.5 ASR Streaming 0.6B model card: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b (repository last changed 10 Sep 2026)
- [S4] Nemotron Speech Streaming EN 0.6B model card: https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b (last changed 5 Aug 2026)
- [S5] NeMo-Speech.cpp source, `main`: https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/src/asr/recognizer.cpp, https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/src/asr/postproc/pipeline.cpp, https://github.com/NVIDIA/NeMo-Speech.cpp/blob/main/src/asr/pnc/pnc_runner.cpp
- [S6] transcribe.cpp issue #71: https://github.com/handy-computer/transcribe.cpp/issues/71 (opened 7 Jul 2026, open; comments 17 to 18 Aug 2026)
- [S7] parakeet-tdt-0.6b-v3 discussions #12 and #42: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/discussions/12 (13 Sep 2025), https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/discussions/42 (8 May 2026)
- [S8] NeMo-Speech.cpp issue #40, token-silence EOU misfires: https://github.com/NVIDIA/NeMo-Speech.cpp/issues/40 (10 Sep 2026, open)
- [S9] NeMo-Speech.cpp issue #22, punctuation leaking into the next final: https://github.com/NVIDIA/NeMo-Speech.cpp/issues/22 (2 Sep 2026, closed 2 Oct 2026)
- [S10] NeMo-Speech.cpp PR #41, soft EOU: https://github.com/NVIDIA/NeMo-Speech.cpp/pull/41 (10 Sep 2026, open)
- [S11] NeMo-Speech.cpp PR #46, streaming startup and Q8 pointwise precision: https://github.com/NVIDIA/NeMo-Speech.cpp/pull/46 (opened 14 Sep, merged 24 Oct 2026)
- [S12] Commit history of the two Nemotron model repositories: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b/commits/main, https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b/commits/main
- [S13] `app/transcribe.cpp` at v0.2.0 and `main`: https://github.com/NVIDIA/NeMo-Speech.cpp/blob/v0.2.0/app/transcribe.cpp; `app/serve.cpp` and `server/http/http_server.cpp` at v0.2.0 (grep for `late_punctuation`)
- [S14] Horizon-Labs/punctuation-restoration-small model card: https://huggingface.co/Horizon-Labs/punctuation-restoration-small (3 Oct 2026; v1.1 9 Oct 2026)
- [S15] Horizon-Labs/punctuation-restoration-base model card: https://huggingface.co/Horizon-Labs/punctuation-restoration-base (3 Oct 2026; 9 Oct 2026)
- [S16] 1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase model card and `config.yaml`: https://huggingface.co/1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase (last changed 15 Jul 2023)
- [S17] 1-800-BAD-CODE/punctuation_fullstop_truecase_english: https://huggingface.co/1-800-BAD-CODE/punctuation_fullstop_truecase_english (19 Mar 2023)
- [S18] 1-800-BAD-CODE/punct_cap_seg_47_language: https://huggingface.co/1-800-BAD-CODE/punct_cap_seg_47_language (14 Jun 2023)
- [S19] oliverguhr/fullstop-punctuation-multilang-large: https://huggingface.co/oliverguhr/fullstop-punctuation-multilang-large (16 Nov 2023)
- [S20] kredor/punctuate-all: https://huggingface.co/kredor/punctuate-all (26 Apr 2024)
- [S21] ldenoue/punctuator-multilingual-minilm-v2 and its metrics files: https://huggingface.co/ldenoue/punctuator-multilingual-minilm-v2 (21 to 22 Sep 2026)
- [S22] NGC punctuation_en_bert: https://catalog.ngc.nvidia.com/orgs/nvidia/teams/nemo/models/punctuation_en_bert (v1.0.0rc1, 4 Apr 2023)
- [S23] sherpa-onnx punctuation models: https://k2-fsa.github.io/sherpa/onnx/punctuation/pretrained_models.html
- [S24] Edge-Punct-Casing: https://github.com/frankyoujian/Edge-Punct-Casing (Apache-2.0, last push 4 Feb 2025); paper https://arxiv.org/abs/2407.13142 (18 Jul 2024)
- [S25] Silero models README, licence section: https://github.com/snakers4/silero-models (last push 15 Oct 2026)
- [S26] Horizon-Labs/punctuation-restoration-small files `config.json`, `onnx/onnx_check.json`, `training/train_log.json`, `punctuate.py`: https://huggingface.co/Horizon-Labs/punctuation-restoration-small/tree/main; organisation listing https://huggingface.co/Horizon-Labs
- [S27] Vosk models page: https://alphacephei.com/vosk/models
- [S28] ort crate: https://crates.io/crates/ort (2.0.0-rc.15, 19 Oct 2026), release notes https://github.com/pykeio/ort/releases, linking and versions https://ort.pyke.io/setup/linking, https://ort.pyke.io/setup/multiversion
- [S29] ONNX Runtime releases: https://github.com/microsoft/onnxruntime/releases (v1.31.0, 9 Oct 2026; DLL size read from the zip's directory), NuGet https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime/1.31.0
- [S30] DirectML on Microsoft Learn: https://learn.microsoft.com/windows/ai/directml, and https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html
- [S31] NuGet: https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime.DirectML (1.24.4, 17 Mar 2026), https://www.nuget.org/packages/Microsoft.AI.DirectML (1.15.4, 28 Oct 2024)
- [S32] tokenizers crate: https://crates.io/crates/tokenizers (0.23.2, 3 Sep 2026)
- [S33] SQLite FTS5, unicode61 tokenizer: https://www.sqlite.org/fts5.html
- [S34] Diarist releases: https://github.com/ebt55/diarist/releases (v0.1.0-beta.4, 22 Oct 2026)
- [S35] FLEURS dataset card: https://huggingface.co/datasets/google/fleurs
- [S36] Earnings-21 README: https://github.com/revdotcom/speech-datasets/tree/main/earnings21 (CC BY-SA 4.0)
- [S37] VoxPopuli dataset card: https://huggingface.co/datasets/facebook/voxpopuli (30 Jan 2026)
- [S38] jhu-clsp/mmBERT-small: https://huggingface.co/jhu-clsp/mmBERT-small (MIT, 21 Aug 2025)
- [S39] Microsoft ONNX Runtime ThirdPartyNotices and MIT licence: in the release zip of [S29]
- [S40] crates.io pages for `tract-onnx` (0.23.9, 15 Oct 2026), `candle-transformers` (0.11.0, 26 Jun 2026), `sentencepiece` (0.14.0), `sherpa-onnx` (2.0.0, 23 Oct 2026)
- [S41] nvidia/parakeet-unified-en-0.6b model card: https://huggingface.co/nvidia/parakeet-unified-en-0.6b (7 Apr 2026, last changed 9 Jun 2026)
