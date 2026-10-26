# Eval: punctuation and casing add-on, spike S0 to S8 (2026-10-26)

The spike of `docs/research/18-punctuation.md` section 7.3, prepared on 25 and run on 26 Oct 2026, with the decision rule of its section 7.4 applied. AMD Ryzen 9 5900X (12 cores, 24 threads), 32 GB RAM, RTX 3080 10 GB (used only for the ASR hypotheses and the S0 decodes), Windows 11. Every punctuation model ran on the CPU through the ONNX Runtime CPU execution provider. Sessions appear under their docs/05 labels; the owner's seven Zoom webinar recordings of October 2026 (docs/05 folder D, the `talks` group) are Talk 1 to Talk 7, Talk N being the folder numbered N. No transcript text is reproduced here. Opus 5.5 measurement agent.

**Summary.**

- **S0: Nemotron's own punctuation cannot be switched on by a decode setting.** At the shipped file setting (lookahead 13) the EN model writes 0.0 to 0.6 marks per 100 words on AMI and the six user meetings, and 0.1 to 1.8 on the talks; Nemotron 3.5 writes 0.1 to 2.5. On the AMI clip, turning `--endpointing` off gives word-for-word identical output, and a whole-file decode gives as few marks (1 or 2 in about 500 words). The F16-pointwise GGUFs of the respin agent (research/19) are no different. References have 15 to 31 marks per 100 words. Nemotron does punctuate read speech: 5.9 (EN) and 5 to 10 (3.5) marks per 100 words on FLEURS.
- **The add-on works, and lead candidate A wins by the decision rule** (section 9). A is `Horizon-Labs/punctuation-restoration-small`, run on lowercased words in per-speaker-turn units. End to end on the shipped EN transcripts it scores micro F1 **0.547** over `.` `,` `?` on AMI (the only human reference), against 0.519 for B in int8 and 0.517 for C. Its sentence-boundary F1 is **0.734** on AMI and 0.70 to 0.72 on the Zoom-referenced sets, 5 to 10 points above B and C. Its sentence-start rule lifts upper-case F1 on AMI from 0.509 (the ASR's casing) to **0.748**, more than B's and C's true-casing heads (0.71). On Zoom's own machine punctuation, B and C score higher on commas.
- **WER is unchanged by construction.** Over 8.8 million word checks in 1,425 outputs: 0 invariant violations and 0 words rejected. The compiled copy of the echo gate's `norm` finds 0 mismatches over 1.77 million changed words. `run.py --metrics-only` gives identical cpWER, tcpWER, DI-cpWER, ORC-WER, WDER and DER on AMI, Aug 2 and Talk 1 for A, B int8 and C.
- **Applied (section 12, added 26 Oct).** Plan 16 section 3.13 ships A with `?`, `KeepAsr`, turn units and the sentence-start rule; `,` everywhere but hi, ja and zh (fr and it as a labelled override); `:` and `-` off. Zoom's "..." endings had never been scored; with them left out, the gate pool is a tie (A 0.597, B-q8pc 0.600, C 0.598) that boundary F1 gives to A (0.725 against 0.671 and 0.655), so the plan's conditional ruling was not needed. Through the app's own import and export, all 14 sessions keep identical cpWER, tcpWER and WDER with punctuation on, while DER and JER rise by 2 to 6 points, all of it missed speech, because lines now end at sentence ends.
- **Speed and memory (S7), quiet machine, 4 threads:** A needs **4.1 to 4.6 s per hour of audio** and 0.65 GB of RAM above the process; a GPU transcription running at the same time barely slows it. B in fp32 (1.8 GB) and A-base (1.0 GB) fail research/18's 0.8 GB bar; their speed passes. Our per-channel int8 B (B-q8pc: 3.1 s, 0.63 to 0.74 GB) passes, and C passes (1.2 s, 0.34 GB).
- **Surprises.** A's `tokenizer.json` truncates every encoding at 512 tokens; the Rust `tokenizers` crate reads the same setting. ONNX Runtime's default dynamic int8 quantisation destroys A (micro F1 0.10); per-channel weights with reduced range are needed. Per-turn units beat the channel stream, the opposite of research/18's guess. Lowercasing the ASR's words before A helps by 4 points.

## 0. Setup

| Item | Value |
|---|---|
| Inference | `onnxruntime` 1.31.0 (PyPI wheel; providers `CPUExecutionProvider`, `AzureExecutionProvider`; no GPU package), `tokenizers` 0.23.3, `sentencepiece` 0.2.2, `onnx` 1.23.2, CPython 3.12.9, own venv under `tools/eval/.work/punct` |
| Session options | intra-op threads 4 (Diarist's choice, below) unless a row says otherwise; inter-op 1; sequential execution; all graph optimisations; spinning at the default |
| Windows | A family: 512 tokens including the two specials, 48 tokens of context each side when a unit is longer; B and C: 256 tokens, 32 of context. Batches of 8 windows, sorted by length, padded (A with its attention mask; B's and C's graphs take no mask, and batch 1 against batch 8 gave 0 label differences on 400 AMI words for both) |
| ASR hypotheses | `diarist-cli` (`tools/eval/.bin`, SHA-256 `115f6eeb…`, built 21 Oct), nemo-speech 0.2.0 CUDA, the manifest's pinned GGUFs, the shipped file pass: `--stream --endpointing --asr.streaming.rnnt_right_context 13 --diarize --asr.diar.preset v3-offline`. Configs `pn_shipped_en` and `pn_shipped_n35`, copies of `shipped_en` and `shipped_n35` under new names (fresh data roots). The respin agent's `rs_en_shipped` gives the same words and times as `pn_shipped_en` on Aug 2 and Talk 1 |
| Sets | AMI ES2002a (manual words, 2,600 words); the six user meetings (Zoom references); Talk 1 to 7 (Zoom references, 37 to 67 min, 356 min); FLEURS slices of about 10 minutes for en, de, es, fr, it, pt, hi, ja (`run.py --fleurs`, Nemotron 3.5 at lookahead 13; English also with the EN model) |
| Threads Diarist would use | **4 intra-op threads.** That is research/18 section 4.1's choice; it leaves 8 of the 12 cores to the ASR's CPU side, the echo canceller and the UI. "All cores" below means 12 (ONNX Runtime's default, the physical cores) and 24 (logical) |

Applying labels follows research/18 section 4.4. The model sees each word with the ASR's leading and trailing marks removed. Diarist writes the model's mark after the word's last token, and only if that token ends at the word's end. The allow-list is `.` `,` `?` and `。` `，` `？` `、` `।` `؟` `،`; colons, dashes, `;`, `¿` and `<ACRONYM>` are dropped and counted. Casing only ever turns a lowercase letter into a capital. A uses the sentence-start rule (the stream's first word and every word after `.` or `?`). B and C use their per-character casing flags, applied where the model's pieces spell the word. Any word that fails a check keeps its ASR text and is counted. Policy `replace` drops the ASR's own trailing marks; `keep` leaves a word the ASR marked as it is.

Scoring follows research/18 section 6.1. Hypothesis and reference words are aligned speaker-agnostically by a Levenshtein alignment of normalised pieces (the harness's `normalize_token` for English; run.py's multilingual normaliser elsewhere; characters for ja). Only pairs whose pieces are equal are scored. "Micro F1" is over `.` `,` `?` (`!` and `…` count as `.`), and "boundary" merges `.` and `?`. *A defect found in review (plan 16 section 3.13.3): `normalize_token("word...")` gives the pieces ("word", "."), whose stray "." never aligns, so the 1,444 Zoom reference words ending in "..." (21 to 23 percent of Zoom's period-class marks) were scored as if Zoom had written nothing there. Sections 5, 6 and 9 keep the numbers as spiked; section 12.2 re-scores all 14 sessions with those words left out.* Casing is scored on matched words whose reference starts with a cased letter: "upper F1" is the F1 of the capital class, and sentence-start recall counts reference words that open a sentence of their speaker. On AMI, hypothesis words outside the annotated span ±1 s are left out, as in run.py.

## 1. Candidates (S2)

Fetched with `C:\Windows\System32\curl.exe -L --ssl-revoke-best-effort -A diarist-research` from `https://huggingface.co/<repo>/resolve/<revision>/<path>`. Every size and SHA-256 matched research/18 section 7.2. Each card's YAML gives `license: apache-2.0`.

| Id | Repository @ revision | File | Bytes | SHA-256 |
|---|---|---|---|---|
| A | `Horizon-Labs/punctuation-restoration-small` @ `ee31fdfc5304` | `onnx/model_quantized.onnx` (int8 embeddings, fp32 compute) | 268,414,381 | `5679870be92c7fa4…` ✓ |
| | | `onnx/model.onnx` (A-fp32) | 563,106,940 | `751b4ba021f23d18…` ✓ |
| | | `tokenizer.json` | 34,363,286 | `f34b44a6dc8a0eef…` ✓ |
| | | `config.json`, `punctuate.py`, `tokenizer_config.json`, `special_tokens_map.json`, `onnx/onnx_check.json` | 2,110; 3,740; 46,440; 636; 229 | |
| A-base | `Horizon-Labs/punctuation-restoration-base` @ `63ec204eeed9` | `onnx/model_quantized.onnx` | 641,137,205 | `42f52bb955a7e158…` ✓ |
| B | `1-800-BAD-CODE/xlm-roberta_punctuation_fullstop_truecase` @ `d1769a597ce8` | `model.onnx` (fp32) | 1,112,481,438 | `c43ca686dabc237c…` ✓ |
| | | `sp.model`, `config.yaml` | 5,069,059; 531 | `7f944d0be93b275f…` ✓ |
| C | `1-800-BAD-CODE/punctuation_fullstop_truecase_english` @ `b26fd1c40e88` | `punct_cap_seg_en.onnx` (fp32) | 209,532,928 | `dd922d459da618cd…` ✓ |
| | | `spe_32k_lc_en.model`, `config.yaml` | 587,902; 302 | `9e86d0263de80b3b…` ✓ |

Derived files (ours, `onnxruntime.quantization.quantize_dynamic`, QInt8 weights, MatMul and Gather):

| Id | Source | Options | Bytes | SHA-256 |
|---|---|---|---|---|
| B-q8pc | B fp32 | `per_channel=True, reduce_range=True` | 279,327,734 | `ae1a8ab616b29bfb…` |
| B-q8 | B fp32 | defaults | 278,908,613 | `08fde5a1753050c1…` |
| A-q8pc | A-fp32 | `per_channel=True, reduce_range=True` | 142,020,768 | `bc5060e31ad56d63…` |
| A-q8 | A-fp32 | defaults | 141,553,734 | `ed6f87fd30517c8a…` |
| `tokenizer.json` for B / C | `sp.model` / `spe_32k_lc_en.model` | Unigram on the raw SentencePiece ids (S3) | 17,096,111 / 2,115,778 | `765b3fa0e9cf8319…` / `f47e6410dcdd9629…` |

Label agreement with the source model, over the 105,175 ASR words of all 14 English sessions (EN model, turn units):

| Variant against source | Same post-word label | Marks (variant / source) | Same casing flags |
|---|---|---|---|
| A (shipped int8-embedding file) against A-fp32 | 99.88 % | 15,685 / 15,641 | – |
| A-q8pc against A-fp32 | 95.28 % | 15,003 / 15,641 | – |
| B-q8pc against B | 94.76 % | 18,587 / 21,599 | 97.59 % |
| B-q8 against B | 94.10 % | 18,227 / 21,599 | 95.25 % |

A-q8, with the default options, is broken: oracle micro F1 0.10 on AMI and 0.06 to 0.13 on FLEURS, with no periods at all (section 5). Keeping the embedding Gather in fp32 and quantising only the MatMuls (`A-q8m`, 436 MB) is broken the same way (0.105 end to end on AMI). The Zen 3 CPU has no VNNI. The 7-bit reduced range with per-channel scales avoids the saturation, which matches ONNX Runtime's quantisation guidance for such CPUs (*inference*).

## 2. Nemotron's own punctuation (S0)

### Shipped-setting runs, runtime words JSON (`levers.py punct` counts)

| Session | EN words | EN marks per 100 words | EN capitalised | 3.5 words | 3.5 marks per 100 | 3.5 capitalised |
|---|---|---|---|---|---|---|
| AMI ES2002a | 2,447 | 0.00 | 4.8 % | 2,319 | 0.17 | 3.9 % |
| Aug 2 | 6,731 | 0.62 | 5.1 % | 6,544 | 0.41 | 4.0 % |
| Aug 9 | 6,525 | 0.31 | 6.1 % | – | – | – |
| Sep 5 | 8,558 | 0.25 | 4.0 % | 8,344 | 2.54 | 4.5 % |
| Sep 7 | 7,059 | 0.18 | 4.4 % | – | – | – |
| Sep Logistics | 6,353 | 0.08 | 4.5 % | – | – | – |
| Aug 14 | 12,193 | 0.32 | 3.5 % | – | – | – |
| Talk 1 to Talk 7 | 6,091 to 10,549 | 0.13 to 1.78 (Talk 1 1.40, Talk 4 1.78) | 3.3 to 4.9 % | – | – | – |
| FLEURS en (read speech) | 1,008 | 5.85 | 11.5 % | 1,178 | 7.22 | 10.1 % |
| FLEURS de, es, fr, it, hi | – | – | – | 877 to 1,286 | 10.4, 7.5, 6.8, 7.9, 5.0 | 32.7, 6.5, 5.6, 7.9, 0 % |
| FLEURS pt | – | – | – | 1,048 | 0.57 | 4.3 % |

(The 3.5 runs of Aug 9, Sep 7, Sep Logistics and Aug 14 were made later for S5; the counts above are the S0 set of ami, aug2, sep5.)

Against the references, the ASR's own marks are precise but rare. Micro P / R: EN on the six meetings 0.826 / 0.012, on the talks 0.875 / 0.031; 3.5 on the meetings 0.790 / 0.056; on AMI, EN writes none and 3.5 three, one of them correct.

### AMI clip, 50 to 350 s, lookahead 13 (`nemo_direct_punct.py`, three decode modes)

The reference has 588 words in the clip, 21.1 marks per 100 words and 21.8 % capitalised words.

| Model, mode | Words | Clip WER % | Marks per 100 | Micro F1 | Upper F1 | Wall s | VRAM Δ MiB |
|---|---|---|---|---|---|---|---|
| EN, `--stream --endpointing` (shipped) | 527 | 15.3 | 0.19 | 0.000 | 0.571 | 4.9 | 1,386 |
| EN, `--stream`, no `--endpointing` | 527 (identical words and times) | 15.3 | 0.19 | 0.000 | 0.571 | 3.8 | 1,277 |
| EN, whole file (no `--stream`) | 530 | 14.1 | 0.19 | 0.000 | 0.538 | 8.1 | 5,169 |
| 3.5, `--stream --endpointing` (shipped) | 483 | 20.8 | 0.21 | 0.027 | 0.581 | 4.5 | 1,357 |
| 3.5, no `--endpointing` | 483 (identical words and times) | 20.8 | 0.21 | 0.027 | 0.581 | 4.7 | 1,290 |
| 3.5, whole file | 489 | 21.8 | 0.41 | – | 0.529 | 11.0 | 5,336 |

Speaker tags differ slightly between the two streaming modes; words and times do not. Since endpointing changes nothing, research/18's follow-up run on Aug 14 had nothing to test and was not made.

### Re-converted GGUFs (respin agent, research/19), same counts

| Config | AMI marks per 100 | Talk 1 marks per 100 | Talk 1 micro P / R |
|---|---|---|---|
| `rs_en_shipped` (= `pn_shipped_en`) | 0.00 | 1.40 | 0.867 / 0.071 |
| `rs_en_q8pw` (Q8 with F16 pointwise) | 0.00 | 1.43 | 0.860 / 0.071 |
| `rs_en_f16` | – | 1.47 | 0.846 / 0.073 |
| `rs_n35_shipped` / `rs_n35_q8pw` | – / 0.14 | 1.07 / 1.02 | 0.795 / 0.049, 0.775 / 0.046 |

Parakeet Unified EN (research/17 spike 4) has not been converted, so its row is still open.

## 3. References (S1)

| Set | Words | Marks per 100 words | `.` | `,` | `?` | Other | Capitalised | Source of the marks |
|---|---|---|---|---|---|---|---|---|
| AMI ES2002a (manual, punctuation kept) | 2,600 | 19.4 | 299 | 173 | 33 | – | 17.3 % | human (`punc="true"` tokens attached to the word before, `ami_punct_ref.py`) |
| Aug 2, Aug 9, Sep 7, Sep 5, Sep Logistics, Aug 14 | 6,380; 6,163; 6,647; 8,164; 5,994; 11,659 | 23.2; 24.1; 20.1; 14.5; 23.2; 18.7 | 3,001 in all | 5,494 | 558 | no `:` or `-` | 8.5 to 13.9 % | Zoom (machine) |
| Talk 1 to Talk 7 | 5,786 to 9,574 (51,837 in all) | 18.7 to 31.3 | 3,535 in all | 7,296 | 391 | no `:` or `-` | 8.8 to 12.9 % | Zoom (machine) |
| FLEURS en, de, es, fr, it, pt, hi, ja | 1,176; 871; 1,174; 1,287; 962; 1,035; 1,160; 2,072 characters | 10.6; 12.6; 9.8; 10.3; 9.8; 12.3; 8.0; 5.5 | 63; 45; 49; 61; 43; 50; 54; 49 | 62; 65; 65; 71; 51; 76; 39; 64 | 0; 0; 1; 0; 0; 1; 0; 0 | a few `:` `;` | | FLEURS `raw_transcription` (written) |

FLEURS has almost no questions, so it cannot judge `?`. FLEURS hi ends 41 sentences with `.` and 13 with `।`; both count as `.`. For ja, every character is a word and a mark belongs to the character before it.

## 4. Tokeniser parity (S3)

| Check | Texts or units | Tokens or words | Mismatches |
|---|---|---|---|
| B: `tokenizers` Unigram from `sp.model` against `sentencepiece`, every reference text and every ASR text, as is and lowercased | 22,016 texts | 1,366,130 tokens | **0** |
| C: the same, English sets | 20,600 texts | 1,248,376 tokens | **0** |
| B, C: every token maps to one word, and each word's last token ends at the word's end | 5,460 units | 271,925 words | 0 |
| A (cased and lowercased): the same, with A's own `tokenizer.json` | 5,460 units | 271,925 words | 0 |

The conversion builds `models.Unigram` on the raw pieces, the `Precompiled` NFKC map, whitespace stripping and collapsing, and `Metaspace(prepend_scheme="always")`. It must take the unknown id from the piece of type UNKNOWN. B's edited model has `trainer_spec.unk_id = 0`, while its `<unk>` piece is id 3.

**A's `tokenizer.json` carries `"truncation": {"max_length": 512}`.** Encoding a whole unit silently drops every token after the 512th, which leaves those words unlabelled. The first oracle run showed it: A scored 0.38 on FLEURS en, against 0.80 once fixed. The Rust `tokenizers` crate reads the same field, so Diarist must call `with_truncation(None)` (and drop the padding) after loading it. Every number in this report is from the fixed runs.

## 5. Oracle: the models on the references' own words (S4)

The input is the reference's words, lowercased with the marks removed. Turn units break at a speaker change or a gap over 1.5 s; stream units break at gaps only. FLEURS is one stream with no breaks. Casing comes from A's rule or from B's and C's flags.

| Model | Units | AMI micro F1 | AMI `.` / `,` / `?` F1 | AMI boundary | AMI upper F1 | Zoom 6 micro | Talks 7 micro | Zoom + Talks boundary |
|---|---|---|---|---|---|---|---|---|
| A | turn | **0.543** | 0.638 / 0.446 / 0.319 | **0.715** | 0.516 | 0.639 | 0.605 | **0.730** |
| A | stream | 0.465 | 0.543 / 0.395 / 0.424 | 0.589 | 0.470 | 0.631 | 0.606 | 0.715 |
| A-base | turn | **0.592** | 0.708 / 0.420 / 0.406 | 0.734 | 0.539 | 0.656 | 0.616 | 0.750 |
| A-q8pc | turn | 0.473 | 0.572 / 0.391 / 0.248 | 0.693 | 0.524 | 0.606 | 0.558 | 0.702 |
| A-q8 (defaults) | turn | 0.102 | – / 0.114 / – | – | 0.005 | 0.203 | 0.231 | – |
| B | turn | 0.519 | 0.579 / 0.473 / 0.340 | 0.610 | **0.683** | **0.649** | **0.634** | 0.683 |
| B | stream | 0.405 | 0.429 / 0.376 / 0.476 | 0.488 | 0.603 | 0.634 | 0.625 | 0.666 |
| B-q8pc | turn | 0.506 | 0.574 / 0.435 / 0.345 | 0.608 | 0.670 | 0.604 | 0.576 | 0.673 |
| B-q8 (defaults) | turn | 0.499 | 0.567 / 0.421 / 0.356 | 0.604 | 0.673 | 0.600 | 0.567 | 0.664 |
| C | turn | 0.505 | 0.580 / 0.420 / 0.333 | 0.607 | 0.676 | 0.602 | 0.580 | 0.656 |
| A, cased input (leaks the reference's sentence capitals; an upper bound only) | turn | 0.626 | 0.741 / 0.486 / 0.417 | 0.777 | 0.599 | 0.759 | 0.706 | 0.900 |

FLEURS, micro F1 (boundary F1 in brackets):

| Model | en | de | es | fr | it | pt | hi | ja |
|---|---|---|---|---|---|---|---|---|
| A | 0.802 (0.906) | 0.917 (0.901) | 0.755 (0.863) | 0.828 (0.914) | 0.825 (0.941) | 0.842 (0.913) | 0.770 (0.927) | 0.912 (1.000) |
| A-base | 0.831 (0.954) | 0.913 (0.903) | 0.773 (0.849) | 0.879 (0.958) | 0.825 (0.932) | 0.856 (0.942) | 0.782 (0.954) | 0.909 (1.000) |
| A-q8pc | 0.786 (0.873) | 0.907 (0.874) | 0.795 (0.891) | 0.783 (0.872) | 0.778 (0.881) | 0.815 (0.889) | 0.746 (0.887) | 0.907 (0.990) |
| B | 0.820 (0.923) | 0.890 (0.878) | 0.761 (0.852) | 0.816 (0.887) | 0.792 (0.903) | 0.794 (0.815) | 0.618 (0.770) | 0.824 (0.875) |
| B-q8pc | 0.775 (0.872) | 0.863 (0.820) | 0.719 (0.776) | 0.818 (0.875) | 0.730 (0.830) | 0.738 (0.778) | 0.618 (0.787) | 0.794 (0.852) |
| C | 0.762 (0.828) | – | – | – | – | – | – | – |

On FLEURS, A's oracle numbers (0.76 to 0.92) are in line with its model card's lowercased FLORES figure of 0.796. Meeting speech is harder for every model: 0.50 to 0.65.

## 6. End to end: the models on the ASR's words (S5)

### Meetings, EN model (the English default)

Turn units, lowercased input, policy `replace`. Casing is A's rule for the A family and the flags for B and C.

| Model | AMI micro F1 (P / R) | AMI `.` / `,` / `?` F1 | AMI boundary | Zoom 6 micro (P) | Zoom 6 boundary | Talks 7 micro (P) | Talks 7 boundary | Marks per 100 matched AMI words (reference 15.1) |
|---|---|---|---|---|---|---|---|---|
| ASR alone | – (no marks) | – | – | 0.025 (0.826) | 0.037 | 0.060 (0.875) | 0.095 | 0.0 |
| **A** | **0.547** (0.512 / 0.587) | 0.655 / 0.435 / 0.475 | **0.734** | 0.594 (0.720) | **0.716** | 0.557 (0.731) | **0.702** | 17.3 |
| A-base | 0.589 (0.551 / 0.633) | 0.726 / 0.423 / 0.563 | 0.741 | 0.620 | 0.735 | 0.591 | 0.726 | 17.3 |
| A-q8pc | 0.489 (0.456 / 0.527) | 0.593 / 0.408 / 0.379 | 0.706 | 0.573 | 0.694 | 0.529 | 0.672 | 17.5 |
| B | 0.522 (0.440 / 0.642) | 0.597 / 0.462 / 0.482 | 0.634 | 0.637 | 0.667 | 0.640 | 0.651 | 22.0 |
| B-q8pc | 0.519 (0.461 / 0.593) | 0.610 / 0.429 / 0.481 | 0.633 | 0.596 (0.653) | 0.660 | 0.577 (0.661) | 0.641 | 19.4 |
| C | 0.517 (0.448 / 0.611) | 0.605 / 0.433 / 0.500 | 0.639 | 0.593 (0.628) | 0.642 | 0.596 (0.645) | 0.631 | 20.6 |

Per mark on the 13 Zoom-referenced sessions: A's commas have precision 0.80 and recall 0.37, against 0.76 and 0.45 for B-q8pc and 0.71 and 0.50 for C, the candidates that can ship (B in fp32, which cannot, 0.72 and 0.59; *corrected 26 Oct, plan 16 section 3.13.8: the first version quoted only fp32 B's figures*). Zoom punctuates in a more comma-heavy style than AMI's annotators: the full Zoom references have 1.83 (six meetings) and 2.06 (talks) commas per period, AMI's 0.58 and FLEURS en 0.98. B's comma style is closer to Zoom's, so B in fp32 wins micro F1 against Zoom on both sets; B-q8pc and C tie A on the six meetings and beat it by 2 to 4 points on the talks. A wins against AMI and wins sentence boundaries everywhere. (These figures are as spiked; Zoom's ellipses were not scored, section 12.2.)

Per session, micro F1, EN model:

| Session | ASR marks per 100 | A | A-base | B | B-q8pc | C |
|---|---|---|---|---|---|---|
| AMI ES2002a | 0.0 | 0.547 | 0.589 | 0.522 | 0.519 | 0.517 |
| Aug 2 | 0.6 | 0.580 | 0.604 | 0.630 | 0.582 | 0.594 |
| Aug 9 | 0.3 | 0.591 | 0.595 | 0.662 | 0.607 | 0.616 |
| Sep 5 | 0.2 | 0.657 | 0.676 | 0.661 | 0.630 | 0.612 |
| Sep 7 | 0.2 | 0.513 | 0.552 | 0.607 | 0.555 | 0.561 |
| Sep Logistics | 0.1 | 0.613 | 0.653 | 0.650 | 0.628 | 0.612 |
| Aug 14 | 0.3 | 0.601 | 0.633 | 0.619 | 0.580 | 0.573 |
| Talk 1 | 1.4 | 0.586 | 0.617 | 0.620 | 0.588 | 0.582 |
| Talk 2 | 0.3 | 0.606 | 0.636 | 0.622 | 0.579 | 0.569 |
| Talk 3 | 0.3 | 0.435 | 0.493 | 0.608 | 0.510 | 0.555 |
| Talk 4 | 1.8 | 0.602 | 0.638 | 0.677 | 0.622 | 0.626 |
| Talk 5 | 0.4 | 0.549 | 0.578 | 0.630 | 0.537 | 0.571 |
| Talk 6 | 0.1 | 0.536 | 0.557 | 0.634 | 0.570 | 0.598 |
| Talk 7 | 0.2 | 0.588 | 0.618 | 0.672 | 0.616 | 0.646 |

Talk 3's Zoom reference has 31.3 marks per 100 words (22 commas per 100), the most of any session. A, which writes few commas, scores lowest there.

### Variants (EN model, AMI / Zoom 6 / Talks 7 micro F1, AMI boundary)

| Variant | A | B |
|---|---|---|
| turn units, lowercased input (chosen) | 0.547 / 0.594 / 0.557, 0.734 | 0.522 / 0.637 / 0.640, 0.634 |
| stream units (gap breaks only), lowercased | 0.526 / 0.595 / 0.559, 0.700 | 0.490 / 0.635 / 0.639, 0.601 |
| turn units, ASR casing kept in the input | 0.509 / 0.577 / 0.542, 0.682 | – (lowercase only) |
| stream units, ASR casing kept | 0.443 / 0.574 / 0.541, 0.588 | – |
| policy `keep` (ASR marks stay) instead of `replace` | 0.547 / 0.594 / 0.559 | 0.522 / 0.637 / 0.642 |

### Meetings, Nemotron 3.5

Turn units, lowercased, `replace`.

| Model | AMI micro F1 | AMI boundary | Zoom 6 micro | Zoom 6 boundary | `keep` instead: AMI / Zoom 6 |
|---|---|---|---|---|---|
| ASR alone | 0.006 | – | 0.105 | 0.120 | – |
| A | 0.531 | 0.720 | 0.599 | 0.720 | 0.528 / 0.596 |
| A-base | 0.576 | 0.741 | 0.626 | 0.739 | 0.572 / 0.623 |
| A-q8pc | 0.494 | 0.699 | 0.569 | 0.695 | 0.490 / 0.569 |
| B | 0.521 | 0.646 | 0.638 | 0.672 | 0.521 / 0.638 |
| B-q8pc | 0.502 | 0.637 | 0.595 | 0.668 | 0.501 / 0.595 |
| C | 0.504 | 0.642 | 0.590 | 0.647 | 0.503 / 0.592 |

### Casing (EN model, turn units)

| Casing | AMI upper F1 | AMI case accuracy | AMI sentence-start recall | Zoom 6 upper F1 | Talks 7 upper F1 | Zoom + Talks sentence-start recall |
|---|---|---|---|---|---|---|
| ASR's own | 0.509 | 0.921 | 0.101 | 0.571 | 0.557 | 0.163 |
| A + sentence-start rule | **0.748** | **0.941** | 0.696 | **0.765** | **0.739** | 0.641 |
| A-base + rule | 0.733 | 0.938 | 0.669 | 0.773 | 0.747 | 0.668 |
| A-q8pc + rule | 0.737 | 0.938 | 0.682 | 0.754 | 0.727 | 0.612 |
| B flags | 0.709 | 0.928 | 0.676 | 0.755 | 0.723 | 0.662 |
| B-q8pc flags | 0.696 | 0.925 | 0.655 | 0.744 | 0.717 | 0.665 |
| C flags | 0.705 | 0.924 | **0.716** | 0.735 | 0.697 | 0.663 |

Nemotron 3.5 gives the same order: AMI 0.459 (ASR), A 0.699, B 0.667, C 0.660.

### FLEURS, per language

Nemotron 3.5 at lookahead 13 (English also with the EN model), one stream, policy `replace` unless stated. `P .` and `P ,` are A's precision, which decides whether a mark ships for that language (rule 3).

| Lang | ASR alone micro | A | A, `keep` | A-base | B | B-q8pc | C | A: P `.` | A: P `,` | Ships with A (P ≥ 0.75) |
|---|---|---|---|---|---|---|---|---|---|---|
| en (EN model) | 0.616 | 0.811 | 0.774 | 0.832 | 0.837 | 0.819 | 0.783 | 0.915 | 0.769 | `.` `,` |
| en (3.5) | 0.656 | 0.789 | 0.786 | 0.789 | 0.809 | 0.809 | 0.760 | 0.923 | 0.735 | `.` (`,` just under) |
| de | 0.828 | 0.916 | 0.888 | 0.922 | 0.895 | 0.845 | – | 0.867 | 0.931 | `.` `,` |
| es | 0.731 | 0.772 | 0.786 | 0.719 | 0.741 | 0.744 | – | 0.792 | 0.795 | `.` `,` |
| fr | 0.632 | 0.795 | 0.775 | 0.821 | 0.782 | 0.755 | – | 0.915 | 0.742 | `.` (`,` just under) |
| it | 0.744 | 0.809 | 0.789 | 0.782 | 0.754 | 0.703 | – | 0.923 | 0.694 | `.` |
| pt | 0.089 | 0.788 | 0.788 | 0.812 | 0.782 | 0.688 | – | 0.814 | 0.810 | `.` `,` |
| hi | 0.614 | 0.759 | 0.714 | 0.788 | 0.655 | 0.644 | – | 0.922 (`।`) | 0.533 | `।` |
| ja | 0.450 | 0.592 | 0.592 | 0.611 | 0.621 | 0.621 | – | 1.000 (`。`) | – | `。` (unit ends only) |

`?` cannot be judged on FLEURS, which has 0 or 1 question per slice. Each slice has 39 to 76 commas, so a precision near 0.75 is within noise of the bar. Upper-case F1 with the EN model on FLEURS en: A + rule 0.884, B 0.884, C 0.836. With 3.5: de 0.976 (A) against 0.967 (B), fr 0.848 against 0.846, es 0.835 against 0.819.

For ja, Nemotron 3.5 emits each utterance as one or a few "words" of up to a sentence each: 51 units for 10 minutes. Diarist can place marks only at unit ends, so end-to-end ja punctuation is limited to sentence ends, and the oracle row is the meaningful one. Nemotron 3.5 punctuates read speech well on its own: de 0.828 micro F1 with no model. pt is the exception, at 0.57 marks per 100 words.

### Colon and dash

With every mark of A allowed, every colon and dash A writes on AMI and the Zoom sets is a false positive (these references contain none). On FLEURS en (EN model), A wrote 1 colon, which was correct (2 in the reference). `:` and `-` stay off the allow-list. A predicted 2,579 marks outside the allow-list over all its outputs; Diarist drops and counts them.

## 7. Invariants and WER (S6)

Every output of every variant (oracle and end to end) was checked after the labels were applied. Word count, order, start and end times and speakers were checked, and so were the letters (only lowercase to uppercase). Each word was checked under the echo gate's `norm` (Python port), under the harness's `normalize_token` (or run.py's multilingual normaliser) and against the allow-list, and each output's normalised sequence was compared before and after under both normalisers.

| Model | Outputs | Words checked | Words changed | Rejected before fallback | Violations | Disallowed marks dropped | Marks inside a word dropped | `<ACRONYM>` dropped |
|---|---|---|---|---|---|---|---|---|
| A | 480 | 2,940,818 | 459,108 | 0 | **0** | 2,579 | 309 | – |
| A-base | 240 | 1,470,409 | 248,080 | 0 | **0** | 1,360 | 157 | – |
| A-q8pc | 240 | 1,470,409 | 226,880 | 0 | **0** | 906 | 179 | – |
| A-q8 | 36 | 208,625 | 205,589 | 0 | **0** | 0 | 22,663 | – |
| B | 138 | 839,517 | 203,329 | 0 | **0** | 6 | 709 | 345 |
| B-q8pc | 138 | 839,517 | 186,719 | 0 | **0** | 4 | 564 | 308 |
| B-q8 | 36 | 208,625 | 39,082 | 0 | **0** | 4 | 78 | 79 |
| C | 117 | 817,836 | 196,445 | 0 | **0** | 0 | 291 | 753 |

- **Exact echo `norm`.** `normcheck.rs` carries the function copied verbatim from `crates/core/src/audio/echo/text.rs` (`to_lowercase`, keep `is_alphanumeric` and whitespace, trim), built with rustc 1.99. It ran over every changed (old, new) word pair of every output: 1,765,232 pairs, **0 mismatches**.
- **WER through the harness.** For A, B-q8pc and C (turn units, `replace`), the punctuated `transcript.json` of AMI, Aug 2 and Talk 1 went through `run.py --metrics-only` (tools/eval's own venv, meeteval 0.4.3), with each segment's text rebuilt from its punctuated words (before punctuation, every segment's text equals its words joined by spaces). All of cpWER, tcpWER, DI-cpWER, ORC-WER, WDER, WDER near changes, DER and JER are **identical** to `pn_shipped_en`. For example, AMI cpWER is 18.03 %, tcpWER 18.42 %, WDER 2.33 % and DER (collar 0.25) 19.94 % with and without punctuation; Aug 2 is 9.52, 9.55, 0.31 and 7.36 %; Talk 1 is 6.60, 6.60, 0.19 and 6.93 %. (This check keeps the segments as they were. The app re-segments after punctuation, splitting lines at sentence ends, which moves DER, JER and ORC-WER; section 12.5 measures that through the app's own import and export.)

## 8. Speed and memory (S7)

Measured on 26 Oct between 19:26 and 20:13 IST. Each model's batch ran inside the agents' `gpu-run.sh` queue helper, which held the GPU unused so that no GPU job could start, and under `quiet.lock`, which kept the other agents' heavy CPU work out. Before each batch, nvidia-smi read 0 % and 934 to 1,196 MiB (the desktop) and the CPU was 4 to 16 % busy. Every setting ran in a fresh worker process: the ONNX Runtime CPU execution provider, turn units, the session load timed separately, and the peak working set read from psutil. "Above base" is the peak minus the process before the model was loaded, which held Python, numpy, onnxruntime and the transcripts (66 MB). "s per audio hour" is the labelling wall time divided by the audio's hours.

Aug 14 (77 min, 12,193 words; batch 8, A family at 512-token windows, B and C at 256):

| Model | ONNX MB | Load s | 1 thread | 2 | **4** | 8 | 12 (all cores) | 24 (logical) | 4 threads, batch 1 | Peak RSS MB | Above base MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| A | 268 | 1.9 | 12.2 | 6.7 | **4.6** | 3.4 | 3.0 | 5.4 | 5.8 | 714 | 648 |
| A-q8pc | 142 | 1.8 | 6.1 | 3.4 | **2.2** | 1.9 | 2.2 | 3.4 | 2.5 | 596 | 530 |
| A-base | 641 | 2.4 | – | – | **7.4** | – | 6.2 | – | – | 1,069 | **1,003** |
| B | 1,112 | 2.0 | – | – | **5.3** | – | 4.0 | – | – | 1,870 | **1,804** |
| B-q8pc | 279 | 1.1 | 10.6 | 5.6 | **3.2** | 2.1 | 2.5 | 4.4 | 3.3 | 696 | 630 |
| C | 210 | 0.3 | 4.7 | 2.2 | **1.2** | 0.9 | 0.9 | 1.4 | 1.5 | 399 | 333 |

All 14 English sessions (105,175 words, 11.6 h; 1.13 tokens per word for A):

| Model | Threads | Label s | s per audio hour | CPU s per audio hour | Peak RSS MB | Above base MB |
|---|---|---|---|---|---|---|
| A | 4 | 46.5 | **4.1** | 16.1 | 759 | 648 |
| A | 12 | 30.5 | 2.7 | 31.8 | 760 | 649 |
| A-q8pc | 4 | 22.1 | 1.9 | 7.7 | 640 | 530 |
| A-base | 4 | 76.3 | 6.7 | 26.8 | 1,164 | 1,054 |
| B | 4 | 58.0 | 5.1 | 20.3 | 1,915 | 1,804 |
| B-q8pc | 4 | 34.6 | 3.1 | 12.1 | 851 | 740 |
| C | 4 | 13.7 | 1.2 | 4.7 | 470 | 359 |

Window size made no real difference to A's speed: 4.9, 5.0 and 4.6 s per audio hour at 256, 400 and 512 tokens (A-q8pc 1.9, 2.1, 2.2). Most turn units fit one window. Twenty-four threads were slower than twelve for every model.

Contention: the same Aug 14 timing while `diarist-cli transcribe` ran Aug 2 on the GPU (shipped EN setting, config `pn_contention`, nvidia-smi 94 % during the B-q8pc and C runs). Two runs per setting:

| Model | Threads | Idle | With the transcription running |
|---|---|---|---|
| A | 4 | 4.6 | 3.4, 3.3 |
| A | 12 | 3.0 | 3.4, 3.6 |
| B-q8pc | 4 | 3.2 | 4.4, 3.9 |
| B-q8pc | 12 | 2.5 | 2.6, 2.9 |
| C | 4 | 1.2 | 1.5, 1.7 |
| C | 12 | 0.9 | 1.3, 1.0 |

The transcription barely slows the punctuation models. A's idle 4-thread figure (4.6) is above both of its runs under load and above its own full-set figure (4.1), so single runs vary by about 30 % (*inference*: CPU boost states; the CPU read 16 % busy before A's idle batch). An attempt to repeat the idle runs found the machine busy and was skipped. Every candidate is far inside the 15 s per audio hour budget. Memory is what separates them: A-base (1.0 GB above base) and B in fp32 (1.8 GB) exceed research/18's 0.8 GB.

## 9. Decision (research/18 section 7.4)

1. **No S6 counts: nothing is ruled out.** Every model and variant has 0 invariant violations, 0 rejected words, 0 echo-`norm` mismatches and identical WER (section 7).
2. **S7 rules out A-base and B in fp32 on memory**, at 1,003 and 1,804 MB above base against the 0.8 GB bar. Their speed passes: 7.4 and 5.3 s per audio hour. Everything else passes both bars at 4 threads: A 4.1 to 4.6 s and 648 MB, A-q8pc 1.9 to 2.2 s and 530 MB, B-q8pc 3.1 to 3.2 s and 630 to 740 MB, C 1.2 s and 333 to 359 MB.
3. **Micro F1 end to end among A, A-q8pc, B-q8pc and C** (EN model, turn units, lowercased input):

   | | A | B-q8pc | C | A-q8pc |
   |---|---|---|---|---|
   | AMI (human) | **0.547** | 0.519 | 0.517 | 0.489 |
   | Zoom 6 | 0.594 | **0.596** | 0.593 | 0.573 |
   | Talks 7 | 0.557 | 0.577 | **0.596** | 0.529 |
   | AMI, Nemotron 3.5 | **0.531** | 0.502 | 0.504 | 0.494 |
   | Zoom 6, Nemotron 3.5 | **0.599** | 0.595 | 0.590 | 0.569 |
   | Boundary F1, AMI / Zoom 6 / Talks 7 (tie-breaker) | **0.734 / 0.716 / 0.702** | 0.633 / 0.660 / 0.641 | 0.639 / 0.642 / 0.631 | 0.706 / 0.694 / 0.672 |

   **A wins.** It leads by 2.8 points on AMI, the only human reference, with both ASR models. The six meetings are a tie within 0.2 points, and A leads the tie-breaker everywhere by 5 to 10 points. On the talks, C and B-q8pc lead by 2.0 to 3.9 points. That lead comes from commas in Zoom's own style, which AMI's annotators do not share (section 6). C is English only, and B covers 22 of the 28 languages.

   Per language (FLEURS precision of A at least 0.75): `.` ships in all eight languages (`।` for hi, `。` for ja). `,` ships for en (with the EN model, 0.769), de, es and pt; by the literal bar it would be dropped for fr (0.742), it (0.694) and hi (0.533). For ja, Diarist can place marks only at the ends of Nemotron 3.5's word units. *Applied on 26 Oct (plan 16 section 3.13.6, section 12.1 below): `,` ships for fr and it as a labelled override pending a slice of at least 150 reference commas, and is off for hi, ja and zh.* `?` is decided (plan 16 section 3.13.4, section 12.1): FLEURS has 0 or 1 question per slice, so `?` inherits the pooled decision and ships; on meetings A's `?` precision is 0.39 (AMI) to 0.73, and mapping `?` to `.` would cost 4.2 points of pooled micro F1 on the gate pool.
4. **Casing: ship A plus the sentence-start rule.** B's and C's true-casing never beats the rule by 2 points of upper-case F1; the rule is ahead or level everywhere. On AMI: A 0.748, B 0.709, C 0.705. On FLEURS en: A 0.884, B 0.884, C 0.836. On de: A 0.976, B 0.967. On fr: A 0.848, B 0.846. On es: A 0.835, B 0.819. B-q8pc is lower still.
5. **ASR marks.** Nemotron's own marks, where present, are more precise than A's on the Zoom-referenced sets: micro P 0.83 (six meetings) and 0.88 (talks), against 0.72 and 0.73 for A. So by the rule, keep them where present (policy `keep`). Measured, `keep` changes micro F1 by −0.3 to +0.2 points on the meetings, because Nemotron marks only 0.1 to 1.8 words in 100. On FLEURS it costs en 3.7, de 2.8, fr 2.0, it 2.0 and hi 4.5 points (Nemotron 3.5 except en with the EN model), where Nemotron punctuates read speech and confuses `.` and `,`; es gains 1.4 and pt is unchanged (*corrected 26 Oct: the first version gave "1.4 to 3.7 points in en, de, fr and it", wrong on the range and without hi*). Either policy is safe for meetings. If imports of read speech matter, `replace` is the better default for them.

**Verdict: build the add-on with A.** Use `Horizon-Labs/punctuation-restoration-small` @ `ee31fdf`, `onnx/model_quantized.onnx` (268 MB) plus `tokenizer.json` (34 MB) loaded with truncation off. Lowercase each word before the model, process per-speaker turns (a speaker change or a pause over 1.5 s), run 4 intra-op threads, apply the sentence-start casing rule, use the per-language allow-list above, and keep the ASR's own marks. The cost is about 4 to 5 s of CPU wall time and 0.65 GB of RAM per hour of audio, with no VRAM. Our own int8 file (A-q8pc) halves the time and saves 0.12 GB, but costs 5.8 AMI points; not worth it.

## 10. Notes and caveats

- **Two references, two styles.** AMI's annotators wrote few commas, while Zoom's machine punctuation writes many. Talk 3 alone has 22 commas per 100 words. On Zoom references, a model is judged on how much it matches another machine; AMI is the only ground truth, and it is one far-field meeting of 2,600 words (one point of AMI F1 is about 5 marks).
- **AMI `?` is hard.** It has 33 questions. A's `?` precision is 0.39 on AMI, 0.71 on the six meetings and 0.65 on the talks (0.73 and 0.67 with Zoom's ellipses left out, section 12.2), below rule 3's 0.75 bar, which FLEURS cannot test for `?`. Decided: `?` ships (section 12.1).
- **Per-turn units beat the stream**, against research/18 section 4.3's guess. On AMI the turn units come from the diarizer's labels in the hypothesis and from the true speakers in the oracle. A turn ends at the diarizer's speaker change or at a pause over 1.5 s, and that break is often a real sentence end.
- **Lowercasing before A** helps by 3.8 points on AMI and 1.7 on Zoom. The ASR's few capitals (4 to 5 % of words) seem to look like misplaced sentence starts to the model (*inference*).
- **The whole-file decode** on the 5-minute clip gave the EN model 1.2 points lower WER but used 5.2 GB of VRAM against 1.4 GB. That is a single 5-minute clip and not a finding about accuracy.
- **Options for the owner.** If 1.0 GB of RAM during the post-pass is acceptable, A-base adds 4.2 points of AMI micro F1 (0.589) and 2.6 on the six meetings, at 7 s per audio hour. If Zoom-style commas are wanted, B-q8pc writes 1.2 to 1.3 times as many commas as A (C 1.43 to 1.60 times, B in fp32 1.75 times) and wins the talks. It loses 2.8 points on AMI and does not cover six of Diarist's languages. Commas in fr and it sit just under the 0.75 bar on 10-minute slices; a larger FLEURS sample would settle them.
- **Not done:** Parakeet Unified EN's punctuation (not converted yet); `tract-onnx` op coverage (optional in research/18).

## 11. Reproducing

All code is dev-only. It sits in the gitignored work folder `tools/eval/.work/punct/src/` and is not committed; moving it into `tools/eval` with a `uv` dependency group is a follow-up. Each script carries its own docstring. The commands below run from `tools/eval/.work/punct`. GPU steps go through the agents' `gpu-run.sh` queue helper, and the S7 timings run under `quiet.lock`.

```sh
uv venv venv --python 3.12
uv pip install --python venv/Scripts/python.exe "onnxruntime==1.31.*" tokenizers sentencepiece onnx psutil "numpy==2.5.4" "rapidfuzz==3.14.6" "whisper-normalizer==0.1.15" protobuf "scipy==1.18.1"
venv/Scripts/python.exe -I src/punct_fetch.py                         # S2, curl.exe, sizes and SHA-256
venv/Scripts/python.exe -I src/quantize.py B-q8pc A-q8pc B-q8 A-q8    # derived int8 files
venv/Scripts/python.exe -I src/ami_punct_ref.py                       # S1: refs/ami/ES2002a.ref.punct.jsonl
venv/Scripts/python.exe -I src/zoom_refs.py                           # S1: Zoom references (private)
venv/Scripts/python.exe -I src/fleurs_fetch_curl.py --langs de,es,fr,it,pt,hi,ja   # fleurs_fetch.py through curl.exe
# ASR hypotheses at the shipped file setting (GPU), e.g.
DIARIST_EVAL_MODELS_DIR=<hard links of the manifest GGUFs> DIARIST_EVAL_ENGINE_DIR=<nemo-speech 0.2.0 cuda> \
  venv/Scripts/python.exe -I ../../run.py --config pn_shipped_en --configs-file configs.punct.toml \
  --sessions-file sessions.punct.toml --sessions ami,aug2,aug9,sep5,sep7,seplog,aug14,talk1,...,talk7 --transcribe-only
venv/Scripts/python.exe -I ../../run.py --config pn_shipped_n35 --configs-file configs.punct.toml --fleurs all
venv/Scripts/python.exe -I src/s0_runs.py && venv/Scripts/python.exe -I src/s0_score.py   # S0
venv/Scripts/python.exe -I src/tok_parity.py                          # S3
venv/Scripts/python.exe -I src/spike.py --models A,C,B,B-q8pc,A-q8pc,A-base,A-q8,B-q8 --settings oracle --tag _oracle   # S4
venv/Scripts/python.exe -I src/spike.py --models A,C,B,B-q8pc --settings e2e   # S5 (plus --groups zoom --tag _zoom)
venv/Scripts/python.exe -I src/agreement.py B-q8pc:B B-q8:B A-q8pc:A-fp32 A:A-fp32
rustc -O src/normcheck.rs -o bin/normcheck.exe && for f in runs/pairs/*.tsv; do bin/normcheck.exe $f; done   # S6
venv/Scripts/python.exe -I src/s6_wer.py --models A,B-q8pc,C --sessions ami,aug2,talk1   # S6, run.py --metrics-only
venv/Scripts/python.exe -I src/speed.py plan --models A --grid full --tag idle --inside-gpu   # S7, inside gpu-run.sh
venv/Scripts/python.exe -I src/report.py > runs/tables.md
```

## 12. The verdict applied, the re-score and the app-path checks (26 Oct 2026)

Plan 16 section 3.13 (Fable 5.1 as adjudicator, after an Opus 5.5 recomputation of every pooled number) applied the rules of plan 16 section 3.12 to this report. Chunk D (Opus 5.5) then re-scored Zoom's ellipses on all 14 sessions, wrote the manifest entry and ran the checks below through the app's own code. Unless a row says otherwise: end to end on the shipped EN transcripts, turn units, lowercased input, policy `replace`; "gate pool" is AMI plus the six gate-set Zoom sessions; the talks are held out.

### 12.1 The rules, with their numbers

| Rule (plan 16 section 3.12) | Numbers | Applied |
|---|---|---|
| 1. "Rule out any candidate with a non-zero count in S6"; chunk B repeats the word-sequence check through the Rust path | S6: 0 for every candidate (section 7). Rust path: AMI 532 marks, 0 rejected, word sequence identical (12.4); all 14 sessions through the app's import: 0 rejected, identical (12.5) | Nothing ruled out |
| 2. "Rule out any candidate above 15 s per hour of audio at 4 threads, or above 0.8 GB peak RSS" | A-base 1,003 MB and B in fp32 1,804 MB above base; A 4.1 to 4.6 s and 648 MB; Rust path 2.3 to 2.7 s for the 21-minute AMI meeting, 0.5 GB | A, A-q8pc, B-q8pc and C remain |
| 3. "The highest pooled micro F1 over `.` `,` `?` wins ...; candidates within 1 point are tied, and ties go to boundary F1, then to speed" | Gate pool with Zoom's ellipses left out (12.2): A 0.597, B-q8pc 0.600, C 0.598 (as spiked 0.592, 0.592, 0.589). Boundary F1 0.725 against 0.671 and 0.655 | A three-way tie within 0.3 points; boundary F1 gives it to **A**. Amendment (plan 3.13.1, the only one to rule 3): the pool is AMI plus the six gate-set sessions, because the talks are held out from tuning |
| 3, the conditional ruling of plan 3.13.1: "If the full re-score puts A more than 1 point behind B-q8pc or C on the gate pool, A still ships" | A is 0.3 behind B-q8pc and 0.1 behind C (left out); 0.7 and 0.5 behind under the capitalisation check; 0.9 and 0.7 behind if every ellipsis counted as `.` | **Not triggered**: every scoring keeps A inside the 1-point band. The ruling stays recorded for the owner, unused |
| 3, the generalisation check (talks, held out, never pooled) | Micro F1 A 0.562, B-q8pc 0.586, C 0.607; boundary F1 0.710, 0.657, 0.647 | A fails it on micro F1 (4.5 points behind C, 2.4 behind B-q8pc, nearly all commas, section 6 and plan 3.13.2) and passes it on boundary F1 (5.3 ahead) |
| 3, `?` | FLEURS has 0 or 1 question per slice, so `?` inherits the pooled decision. A's `?` precision: AMI 0.388, gate set 0.728, talks 0.671 (ellipses left out; 0.708 and 0.647 as spiked). Mapping `?` to `.` costs 4.2 points of gate-pool micro F1 (0.592 to 0.549, plan 3.13.4) | `?` ships for every covered language (Spanish without `¿`) |
| 3, marks per language ("precision at least 0.75 with at least 20 reference instances ... keeps it; fewer than 20 inherit the pooled decision") | A's FLEURS comma precision en 0.769, de 0.931, es 0.795, pt 0.810, fr 0.742, it 0.694, hi 0.533, ja none written; pooled over seven languages 0.771 | `,` for en, de, es, pt and every unmeasured language; **fr and it as a labelled override** of the literal bar (plan 3.13.6: one standard error is about 0.06 on 39 to 76 commas, dropping them costs 23 to 26 micro points), pending a slice of at least 150 reference commas each, not measured before this entry was written, the owner can veto; off for hi (a clear fail) and for ja and zh (the runtime emits an utterance as one or a few units, so a comma has nowhere to go). `:` and `-` never (every one A wrote on AMI and Zoom is false) |
| 4. "B's or C's true-casing ships only if its upper-class F1 beats ASR casing plus the sentence-start rule by more than 2 points on average ... and is not worse on any of them" | Upper F1 A + rule against B-q8pc / C / B: AMI 0.748 against 0.696 / 0.705 / 0.709; FLEURS en 0.884 against 0.880 / 0.836 / 0.884; de 0.976 against 0.963; fr 0.848 against 0.827; es 0.835 against 0.786 | A plus the sentence-start rule |
| 5. "`KeepAsr` stays unless `Replace` has a higher pooled micro F1 by more than 0.5 points" | Replace minus keep on the gate pool: −0.05 (EN), +0.29 (Nemotron 3.5). Nemotron's own marks: micro P 0.83 and 0.88 against A's 0.72 and 0.73 | `KeepAsr`. Its cost on read speech (FLEURS): en 3.7, de 2.8, fr 2.0, it 2.0, hi 4.5 points; es +1.4, pt 0 |
| 6. "`Stream` stays unless `Turn` has higher boundary F1 on AMI and a pooled micro F1 within 0.5 points" | AMI boundary 0.734 (turn) against 0.700 (stream); micro +0.02 (EN), +0.06 (3.5) | `Turn` (a speaker change or a pause over 1,500 ms) |
| 7. Languages | A covers every offered language but Thai | 35 primary subtags |
| 8. Licence and provenance | The card's YAML says `license: apache-2.0`; no LICENSE file at `ee31fdf`; base model jhu-clsp/mmBERT-small, MIT; training text FineWeb and FineWeb-2 (ODC-BY) plus 49,144 windows the card says a language model wrote | Ships with `LICENSE-Apache-2.0.txt`, `NOTICE-punctuation.txt`, the revision and every SHA-256 in the manifest; the owner can veto |
| 9. "If no candidate passes rules 1 to 3, ... no model entry" | – | Does not apply |
| Plan 3.7, threads: "the smallest thread count whose seconds per hour ... while another GPU transcription runs, is within 1.5 times the fastest measured" | Under load: 4 threads 3.3 to 3.4 s, 12 threads 3.4 to 3.6 s | 4 threads (the existing default of `transcript.punctuation_threads`) |
| Plan 3.13.5, `asr_punctuates` | Nemotron 3.5 writes 10.4 marks per 100 words on FLEURS de; A still lifts micro F1 there from 0.828 to 0.888 (keep) | The cut-off moves from 8 to 12 marks per 100 words per channel (chunk A's `apply.rs`) |

### 12.2 Zoom's ellipses, re-scored on all 14 sessions

The Zoom references end 626 words of the gate set and 818 of the talks with "..." (20.9 and 23.1 percent of their period-class marks; AMI has none, and no reference word ends in "...?"). The harness's normaliser splits such a word into ("word", "."), the stray "." never aligns, so the reference's mark was lost and the word was scored as if Zoom had written nothing. A Zoom ellipsis is not a sentence end either: Zoom capitalises the same speaker's next word after "..." 36 percent of the time (467 of 1,313 cased next words over the 13 sessions, the pronoun I left out), against 94 percent after `.` and 4 percent after `,`.

Method: the spike's own code (`src/punct.py`, `src/punctlib.py`, `src/spike.py` of `tools/eval/.work/punct`) re-labelled the existing ASR outputs, the shipped EN transcripts of all 14 sessions and the Nemotron 3.5 transcripts of the gate pool, with A, B-q8pc and C at the setting above (AMI through the spike's pipeline with its annotated-span filter); about 3 minutes of CPU. The as-spiked confusion counts reproduce `runs/results/*.jsonl` exactly (63 of 63 rows). Each ellipsis is then removed from the reference word before alignment, and the word is scored four ways: as spiked; **left out** (primary); `.` when the next reference word is capitalised (the pronoun I left out) and left out otherwise (the check); and every ellipsis as `.` (what section 0 said it did, which is not the right correction).

Pooled micro F1 over `.` `,` `?`, matched words summed before scoring:

| Pool | A, as spiked | B-q8pc, as spiked | C, as spiked | **A, left out** | **B-q8pc, left out** | **C, left out** | A minus the best other, left out |
|---|---|---|---|---|---|---|---|
| AMI (human; no ellipses) | 0.547 | 0.519 | 0.517 | **0.547** | 0.519 | 0.517 | +2.8 |
| Zoom 6 (gate set) | 0.594 | 0.596 | 0.593 | 0.599 | **0.604** | 0.603 | −0.5 |
| **AMI + Zoom 6: rule 3's pool** | 0.592 | 0.592 | 0.589 | 0.597 | **0.600** | 0.598 | **−0.3** |
| Gate pool, Nemotron 3.5 | 0.596 | 0.590 | 0.586 | **0.601** | 0.598 | 0.595 | +0.3 |
| Talks 7 (held out, never pooled) | 0.557 | 0.577 | 0.596 | 0.562 | 0.586 | **0.607** | −4.5 |
| Boundary F1, gate pool | 0.717 | 0.658 | 0.642 | **0.725** | 0.671 | 0.655 | +5.3 |
| Boundary F1, talks | 0.702 | 0.641 | 0.631 | **0.710** | 0.657 | 0.647 | +5.3 |
| Boundary F1, all 14 | 0.710 | 0.649 | 0.636 | **0.717** | 0.664 | 0.651 | +5.3 |

The four scorings, micro / boundary F1:

| Scoring of Zoom's ellipsis words | Gate pool: A | B-q8pc | C | Talks: A | B-q8pc | C |
|---|---|---|---|---|---|---|
| As spiked (scored as "none") | 0.592 / 0.717 | 0.592 / 0.658 | 0.589 / 0.642 | 0.557 / 0.702 | 0.577 / 0.641 | 0.596 / 0.631 |
| **Left out (primary)** | **0.597 / 0.725** | 0.600 / 0.671 | 0.598 / 0.655 | 0.562 / 0.710 | 0.586 / 0.657 | 0.607 / 0.647 |
| `.` before a capital, else left out (check) | 0.590 / 0.706 | 0.597 / 0.664 | 0.595 / 0.649 | 0.553 / 0.683 | 0.581 / 0.646 | 0.601 / 0.637 |
| Every ellipsis as `.` (wrong) | 0.576 / 0.676 | 0.585 / 0.643 | 0.583 / 0.628 | 0.537 / 0.641 | 0.567 / 0.617 | 0.586 / 0.609 |

Every correction moves A down against B-q8pc and C, as plan 3.13.3 predicted (0.3 to 0.9 points behind on the gate pool: measured 0.3 left out, 0.7 under the check), because A writes a mark at the fewest of Zoom's trailing-off positions: at the 1,339 matched ellipsis words of the 13 sessions A wrote nothing at 1,075, B-q8pc at 858, C at 742, and a sentence end (`.` or `?`) at 130, 297 and 301. Boundary F1 stays with A on 14 of 14 sessions when the ellipses are left out (by 0.9 to 9.6 points) and on 12 of 14 under the check (Sep Logistics and Talk 3 go to B-q8pc by 0.6 and 2.7).

Per session, micro F1, left out: AMI A 0.547 (B-q8pc 0.519, C 0.517); Aug 2 0.588 (0.593, 0.607); Aug 9 0.597 (0.615, 0.626); Sep 5 0.659 (0.636, 0.619); Sep 7 0.519 (0.566, 0.574); Sep Logistics 0.619 (0.637, 0.621); Aug 14 0.605 (0.587, 0.580); Talk 1 0.589 (0.594, 0.589); Talk 2 0.613 (0.587, 0.579); Talk 3 0.440 (0.526, 0.573); Talk 4 0.607 (0.628, 0.635); Talk 5 0.551 (0.542, 0.577); Talk 6 0.538 (0.576, 0.605); Talk 7 0.596 (0.626, 0.661).

A per mark, left out, precision / recall: gate pool `.` 0.653 / 0.714, `,` 0.791 / 0.404, `?` 0.695 / 0.642; talks `.` 0.679 / 0.693, `,` 0.818 / 0.348, `?` 0.671 / 0.691; AMI `.` 0.637 / 0.675, `,` 0.405 / 0.470, `?` 0.388 / 0.613. A writes about one comma per sentence end (0.90 on the gate set, 1.00 on the talks), the full Zoom references 1.83 (gate set) and 2.06 (talks) commas per period (*corrected: the spike's "about two" and the 2.38 and 2.75 of an earlier draft were taken on matched words after the ellipses had dropped out*), AMI's annotators 0.58 and FLEURS en 0.98.

### 12.3 What shipped: the manifest entry

`manifests/models.json`, entry "Punctuation (Horizon-Labs small)", `role: punctuation`, `required: true` (the owner can veto it there without a code change), `dir` `horizon-labs/punctuation-restoration-small`, licence "Apache-2.0 (declared in the model card; no LICENSE file in the repository at this revision); base model MIT", languages: every offered language but `th` (35 primary subtags; nb and nn are the card's `no`).

| File | Bytes | SHA-256 | From |
|---|---|---|---|
| `onnx/model_quantized.onnx` (primary) | 268,414,381 | `5679870be92c7fa4…` | Hugging Face at `ee31fdfc5304f876ea3b0dd72d30eda7e940b2b1` |
| `tokenizer.json` | 34,363,286 | `f34b44a6dc8a0eef…` | the same |
| `LICENSE-Apache-2.0.txt` | 11,358 | `cfc7749b96f63bd3…` | the project's `models-2026-10` release (to be published by the owner) |
| `NOTICE-punctuation.txt` | 2,179 | `f9d70a3fe979fefa…` | the same |
| `onnxruntime-LICENSE.txt` | 1,094 | `c250d6278f0b47a6…` | the same; from `onnxruntime-win-x64-1.31.0.zip` |
| `onnxruntime-ThirdPartyNotices.txt` | 344,457 | `c53a76501ef60db6…` | the same |
| `diarist-onnxruntime.dll` (ONNX Runtime 1.31.0, CPU, renamed) | 17,253,176 | `0c42dc4e278f0968…` | the same |
| Total (`size_bytes`) | 320,389,931 | | |

The description block: `tokenizer.json`, `hf_json`, input lowercased, 512 tokens per window including the two specials with 48 of context on each side, 8 windows per call, bos 2, eos 1, pad 0, inputs `input_ids` and `attention_mask`, one `post` head over `logits` with the labels `none . , ? - :` at the word's last token, `casing: none` (no casing head: the sentence-start rule only), `unit: turn`, `policy: keep_asr`, runtime `diarist-onnxruntime.dll` of release 1.31, and `marks_by_language` `*`: period, comma, question; `hi`, `ja`, `zh`: period, question (rendered per script: `।` in Devanagari, `。` `？` in Han and kana, `،` `؟` in Arabic, U+037E for the Greek question mark).

### 12.4 The Rust path (chunk B's CLI check)

`diarist-cli punctuate` on the 21-minute AMI meeting imported from its cached runtime output (no GPU): `applied: true`, 532 marks, 0 rejected words, the normalised word sequence identical to the source run through the app's JSON export, 2.3 to 2.7 s of labelling and 0.5 GB peak RSS, inside rule 2's bars.

### 12.5 The app's own import and export: words unchanged, DER and JER move

Plan 3.5's check without the GPU. Every session's cached runtime words JSON of the shipped EN run (`rs_en_shipped`, the respin measurement's copy of `shipped_en`, whose metrics equal docs/eval/2026-10-16-shipped_en.md on all seven sessions) went through the app's file import with a replay engine (chunk B's ignored `cli_check_root` driver) into two scratch data roots: one without a punctuation model (`reason: no_model`) and one with the manifest's model, fetched by `diarist-cli models pull` (12.6). Then `diarist-cli export --format json` and `--format rttm`, and `run.py --metrics-only` over the exports: config `shipped_en_replay` (off; a scratch copy of `configs.toml` adds it) and `shipped_en_punct`, the `punctuation = true` config of `tools/eval/configs.toml`. Each run folder's `run.json` names the replayed source (`replay_of`) and has no speed or VRAM fields.

- **Without punctuation the replay equals the GPU run on every metric** (cpWER, tcpWER, DI-cpWER, WDER, WDER near changes, ORC-WER, DER at both collars, JER, speaker counts; all 14 sessions), so the replay is faithful.
- **With punctuation, cpWER, tcpWER, DI-cpWER, WDER, WDER near changes, word counts and speaker counts are identical** on all 14 sessions (seven of the eval set as in the shipped table: cpWER 9.5, 8.1, 4.7, 7.2, 7.0, 43.9 and 18.0; WDER 0.3, 0.3, 0.2, 0.3, 0.0, 19.4 and 2.3). Every word's normalised pieces are the same (`normalize_token`, 14 of 14), and 0 words were rejected. The add-on wrote 532 to 1,807 marks per session (15,117 in all over 11.6 hours) and raised 273 to 815 first letters per session.
- **DER and JER move, and only through missed speech.** Lines now end at sentence ends where the pause allows (AMI 237 to 293 segments, Aug 2 230 to 403, Aug 14 393 to 660), and the export RTTM has one span per segment, so the pause after a sentence end is no longer covered by a span. Against references that cover those pauses (Zoom's cues; AMI's word-run RTTM) it reads as missed speech: the miss share of DER at collar 0.25 rises by 2.8 to 6.0 points, false alarm falls by 0.0 to 0.3 and speaker confusion is unchanged or lower (0 to −0.8). ORC-WER does not move (0.00 on every session; plan 3.5 expected it might).

| Session | DER c0.25 off → on | DER c0 off → on | JER off → on |
|---|---|---|---|
| Aug 2 | 7.4 → 13.2 (+5.8) | 11.9 → 17.9 (+6.0) | 11.9 → 17.3 (+5.4) |
| Aug 9 | 9.0 → 12.7 (+3.7) | 12.6 → 16.2 (+3.6) | 11.0 → 13.7 (+2.7) |
| Sep 5 | 3.9 → 7.0 (+3.1) | 6.4 → 10.0 (+3.5) | 30.9 → 33.5 (+2.6) |
| Sep 7 | 11.4 → 15.0 (+3.5) | 14.9 → 18.4 (+3.5) | 30.9 → 34.2 (+3.3) |
| Sep Logistics | 6.5 → 12.2 (+5.6) | 12.9 → 18.6 (+5.7) | 12.4 → 18.2 (+5.7) |
| Aug 14 | 25.7 → 29.3 (+3.5) | 29.6 → 33.2 (+3.6) | 58.4 → 60.6 (+2.2) |
| AMI ES2002a | 19.9 → 25.4 (+5.4) | 30.2 → 35.3 (+5.1) | 34.5 → 38.3 (+3.8) |
| **Eval set (7), mean** | **+4.4** (3.1 to 5.8) | **+4.4** (3.5 to 6.0) | **+3.7** (2.2 to 5.7) |
| Talks 1 to 7 (held out), mean | +4.0 (2.6 to 5.3) | +4.1 (2.6 to 5.6) | +4.1 (2.3 to 6.3) |

This is why the eval data roots run with `transcript.punctuation = off` (plan 3.5): the accuracy tables keep measuring the speech model and the diarizer, comparable with their history, and a punctuated run is compared only with a punctuated one. The DER and JER rise is a property of the scoring (segment spans against continuous references), not of who said what: WDER and DI-cpWER, which score every word's speaker, are unchanged. In this replay (a debug build of `diarist-core`, dependencies optimised, not a timed run) labelling took 51.9 s for the 11.6 hours, 4.5 s per hour, in line with section 8.

### 12.6 First run

A fresh scratch data root's first-run items list the model as a required download with all seven files, other files first and the primary last, each with its address, SHA-256 and place (`setup.rs` test `first_run_items_list_the_punctuation_model_with_every_file`). `diarist-cli models pull "Punctuation (Horizon-Labs small)"` against a local server serving exactly the bytes of the table in 12.3 (the release is not published yet) fetched all seven, verified each SHA-256, and the scan recognised the file as the manifest entry with its description block and selected it; the punctuated replays of 12.5 ran from that folder. Once the owner publishes the release, the same pull fetches from Hugging Face and GitHub.

### 12.7 Limits found

- **The ONNX Runtime library does not load when its full path is longer than 250 characters** (`LoadLibraryExW failed`, `reason: "runtime"`; measured with junctions of chosen lengths: 250 loads, 251 does not). With the shipped folder (`models\horizon-labs\punctuation-restoration-small\diarist-onnxruntime.dll`, 87 characters after the data folder) that is a data folder of more than 163 characters. The default data folder is about 45; the engine itself starts from a 264-character one since 0.1.0-beta.5. Punctuation then simply does not run, with the reason in the meeting's details.
- For a meeting in `auto` language, neither imports nor the post-pass pass the runtime's detected language to the punctuator (only rebuilds do), so the language's own mark list is not applied there: a Hindi, Japanese or Chinese meeting in `auto` gets commas (`*`'s list), rendered in its script.

### 12.8 Reproducing the additions

The re-score and check scripts are in the gitignored `tools/eval/.work/chunk-d/` of the chunk's worktree (`rescore14.py`, `pool14.py` run with the spike's venv; `replay.sh`, `inspect_replay.py`, `compare.py`, `derparts.py`, `dll_len.py`), outputs under `rescore/`. The release assets with their CONTENTS.md are in `tools/eval/.work/models-release/`.

Sources: research/18 sections 4 to 7 and its sources [S14] to [S17], [S26], [S29]; the model repositories above at the pinned revisions (card YAML read 26 Oct 2026); the punctuators 0.0.7 source (PyPI) for B's and C's label decoding, which this spike's decoding matches on B (one AMI window, identical text); FLEURS (`google/fleurs`, CC-BY-4.0); AMI manual annotations 1.6.2 (CC-BY-4.0).
