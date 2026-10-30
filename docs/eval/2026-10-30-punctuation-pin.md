# Punctuation model pin: v1.1.1 (`0cea764`), 30 Oct 2026

Chunk Q of docs/plans/18 section 8: `punctuation-horizon-small` moves from `ee31fdf` (v1.1, pinned in 0.1.0-beta.5) to `0cea764` (v1.1.1), and the model's folder gets the authors' `LICENSE` and `NOTICE` instead of the re-hosted `LICENSE-Apache-2.0.txt`. Read on Friday 30 Oct 2026, about 08:40 UTC, through the Hugging Face API and file URLs, with User-Agent `diarist-research` and nothing else identifying. No GPU was used.

## The rules, before the reading

- **The upgrade rule** (docs/plans/16 section 12.9 (iii) 6, docs/plans/18 section 8.1 item 5): before any new pinned revision, re-read the card's licence line and the repository's file list at that revision, and record both with the date (this note).
- **The licence check** (plan 18 section 8.1 item 2): upstream's `LICENSE` at `0cea764` must have SHA-256 `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30`, the canonical Apache License 2.0 text that `LICENSE-Apache-2.0.txt` on `models-2026-10` is. If it differs, the chunk stops and reports the difference.
- **The model files** (plan 18 section 8.1 item 1): `onnx/model_quantized.onnx` and `tokenizer.json` keep their SHA-256 and sizes; the chunk re-verifies by hashing the downloads.

## What the repository says on 30 Oct 2026

**Revision.** `main` is `0cea764aa6fc55c8bdfc20a5f0bb4dc27b64753d`, last modified 28 Oct 2026 10:34:54 UTC, with the commit title "Add LICENSE (Apache-2.0) and NOTICE (base model attribution)"; its parent is `ee31fdf` (v1.1, 9 Oct 2026). The tags: `v1.0` at `fca13dd`, `v1.1` at `ee31fdf`, and `v1.1.1`, an annotated tag whose object is `3232085aff49a080be77dbaec8a1d399acce2f4f` and which resolves to commit `0cea764` (the API's model info at `revision/v1.1.1` and at `revision/3232085…` both report `sha` `0cea764…`, and the file tree at `v1.1.1` is byte-identical to the tree at `0cea764`). The pin is the commit hash, as for every manifest entry.

**The card's licence line.** Line 2 of the YAML header, `license: apache-2.0`; the API's `cardData.license` is `apache-2.0`, `base_model` `jhu-clsp/mmBERT-small`, datasets FineWeb-2 and FineWeb. The card at `0cea764` is byte-identical to the card at `ee31fdf` (14,142 bytes, 340 lines, SHA-256 `4f4ec573102819c5a12ffc86b4ecfbf9706d78cf5587f64d11a6cb6190907a78`, the hash plan 16 section 12.9 (ii) recorded on 27 Oct).

**The file list at `0cea764`** (28 files; the 27 Oct snapshot at `ee31fdf` had 26). Added: `LICENSE` and `NOTICE`. Removed: none. Changed: none. The tree API at both revisions gives the same git object and size for each of the 26 files they share, so v1.1.1 is `ee31fdf` plus the two licence files, and the card, the weights, the ONNX files and the tokenizer are the same bytes.

| Path | Bytes | SHA-256 (LFS files) or git blob |
|---|---|---|
| `.gitattributes` | 1,570 | blob `52373fe2…` |
| `LICENSE` | 11,358 | blob `d6456956…` (new) |
| `NOTICE` | 1,567 | blob `8bfba562…` (new) |
| `README.md` | 14,142 | blob `ac53c0dd…` |
| `code/emb/build_texts.py`, `code/lid/fw_files.json`, `code/pii/export_onnx_tok.py` | 4,013; 109,043; 2,911 | |
| `code/punct/` `build_data.py`, `build_evals.py`, `build_spoken.py`, `common.py`, `eval_pcs.py`, `eval_punct.py`, `gen_spoken.py`, `train_punct.py` | 3,783; 4,251; 2,439; 5,074; 3,841; 4,409; 3,778; 6,311 | |
| `config.json`, `special_tokens_map.json`, `tokenizer_config.json` | 2,110; 636; 46,440 | |
| `eval/baselines.json`, `eval/this_model.json` | 564,609; 140,652 | |
| `model.safetensors` | 562,589,552 | `85f724b6…` |
| `onnx/model.onnx` | 563,106,940 | `751b4ba0…` |
| `onnx/model_quantized.onnx` | 268,414,381 | `5679870be92c7fa47c9be9b2872fac389ef28d43d2c33f35e1315b93a82f4ace` |
| `onnx/onnx_check.json`, `punctuate.py` | 229; 3,740 | |
| `tokenizer.json` | 34,363,286 | `f34b44a6dc8a0eef651bfa10e8325057530152606a69d9c3b6ae02efb1558f0a` |
| `training/data_stats.json`, `training/train_log.json` | 5,275; 3,000 | |

**`NOTICE`** (1,567 bytes) names the model and the authors' copyright line, states the Apache License 2.0 with a pointer to `LICENSE`, says the model is fine-tuned from mmBERT-small (MIT) and reproduces mmBERT-small's MIT notice, and points to the card's "Training" section for the training data.

## Hashes

| File | Bytes | SHA-256 | How checked |
|---|---|---|---|
| `LICENSE` at `0cea764` | 11,358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` | `sha256sum` of the download: **equals the required hash**; `cmp` against `LICENSE-Apache-2.0.txt` of `models-2026-10`: byte-identical. Its git blob, `d645695673349e3947e8e5ae42332d0ac3164cd7`, is the well-known blob of the Apache License 2.0 text |
| `NOTICE` at `0cea764` | 1,567 | `74b3592d7abe959e9f2c3693203a60e3844d482742aaffb599105ec063b30af3` | `sha256sum` of the download; now in the manifest |
| `onnx/model_quantized.onnx` | 268,414,381 | `5679870be92c7fa47c9be9b2872fac389ef28d43d2c33f35e1315b93a82f4ace` | the LFS object at `0cea764` (the tree API's `lfs.oid` and size) equals beta.5's pin; `diarist-cli models pull` into an empty data root fetched it from `huggingface.co` at `0cea764` and verified it, and `sha256sum` of the file on disk gives the same |
| `tokenizer.json` | 34,363,286 | `f34b44a6dc8a0eef651bfa10e8325057530152606a69d9c3b6ae02efb1558f0a` | the same: the tree API, the pull's own check and `sha256sum` of the download |
| the card at `0cea764` | 14,142 | `4f4ec573102819c5a12ffc86b4ecfbf9706d78cf5587f64d11a6cb6190907a78` | `sha256sum`; byte-identical to `punctuation-model-card-ee31fdf.md` |
| HF API model info, 30 Oct | 4,319 | `f8055030629ce0ffbc7a77046402c7a63f651208a1b8e6dcd6363c34f9622d16` | `sha256sum`; the same bytes at `revision/0cea764…` and `revision/v1.1.1` |

**Verdict on the rules:** the licence check passes (the authors' `LICENSE` is the canonical text), the card's licence line is unchanged, and the file list changed only by the two licence files. The pin moves.

## The manifest entry

`revision` `0cea764aa6fc55c8bdfc20a5f0bb4dc27b64753d`; the URLs of the model, the tokenizer, `LICENSE` and `NOTICE` at that revision on `huggingface.co`; `LICENSE-Apache-2.0.txt` leaves the `files` list; `NOTICE-punctuation.txt` (Diarist's dated provenance record of `ee31fdf`), ONNX Runtime's two licence texts and `diarist-onnxruntime.dll` stay on `models-2026-10`, unchanged. `licence`: "Apache-2.0 (LICENSE and NOTICE in the repository from v1.1.1, 0cea764; model files byte-identical to ee31fdf); base model MIT". `size_bytes` 320,391,498 (beta.5: 320,389,931; plus `NOTICE`'s 1,567, `LICENSE` replacing a file of the same size). The entry's `dir` has no revision, so an installed copy stays in `models\horizon-labs\punctuation-restoration-small\` and is not downloaded again. The hosts are unchanged (`huggingface.co` and `github.com`), so docs/INSTALL.md's firewall row stands.

## Acceptance (plan 18 section 8.2)

The rules (plan 18 section 8.2): `cargo test` with the new hashes; `diarist-cli --data-dir <empty> models pull punctuation-horizon-small` fetches every file from the real hosts and `models scan` lists it as `punctuation` with `manifest_name` `punctuation-horizon-small`; a copy of a beta.5 data root keeps punctuation working before and after the two small files arrive; `diarist-cli punctuate` on a cached-words import gives a word sequence identical to the beta.5 run of the same meeting. No GPU.

**Checks.** `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` 634 passed, 0 failed, 8 ignored over 22 test binaries (diarist-core's unit tests 599 passed, 4 ignored), the generated TypeScript bindings unchanged. The entry's tests assert the new revision, the four Hugging Face URLs, the file list in download order with sizes and hashes, the licence string and the total; two new tests cover plan 18 section 8.1 item 6 with the shipped entry's layout and stand-in bytes: a beta.5 install gets only `LICENSE` and `NOTICE` on the next fetch, the model never requested or rewritten (`models::fetch::tests::a_beta5_install_gets_only_the_two_new_licence_files`), and a punctuation entry whose model and tokenizer are present stays selected as `punctuation` while the licence files are missing, the download row naming exactly those two (`models::scan::tests::missing_licence_files_never_block_punctuation`). The behaviour was already so (the scanner keys on the model and its tokenizer, the runner checks only the runtime library's hash, and `models pull` and "Download recommended" both go through the downloader, which keeps every verified file); no code changed.

**Setup.** A debug `diarist-cli` built from this branch. Every root holds the 5-minute AMI clip (`crates/core/tests/fixtures/nemotron35_transcribe_diarize_5min.json`, 464 words, public data) imported through the app's file import from its cached words with a replay engine (`punct::onnx::tests::cli_check_root`, no engine, no GPU), before any model was scanned, so it starts unpunctuated. The beta.5 roots hold a copy of the punctuation folder a 0.1.0-beta.5 install downloaded on 28 Oct (`LICENSE-Apache-2.0.txt`, `NOTICE-punctuation.txt`, the two ONNX Runtime texts, the DLL, `tokenizer.json`, `onnx/model_quantized.onnx`, each with its `.sha256` cache). The code outside test modules is unchanged since `00cc9a6` (0.1.0-beta.5 plus documentation), so the beta.5 baseline is this build with beta.5's `manifests/models.json` (pinned at `ee31fdf`) as the data root's own list, the override every data root may carry.

| Root | Models | Model list | Step | Result |
|---|---|---|---|---|
| e2 | none (empty data root) | built-in, v1.1.1 | `models pull punctuation-horizon-small` from `huggingface.co` and `github.com` | every file downloaded and verified (8 files, 320,391,498 bytes), about 49 s with the scan; `sha256sum` of each equals the manifest; no `LICENSE-Apache-2.0.txt`. `models scan`: the model under `punctuation`, selected, `[punctuation-horizon-small]` |
| b5 | beta.5 copy | beta.5's, `ee31fdf` | `models scan`, `punctuate 1` | selected; applied, 91 marks, 56 capitals, 0 words kept as heard; `engine_info.punctuation.revision` `ee31fdf…` |
| b6 | beta.5 copy, no `LICENSE` or `NOTICE` | built-in, v1.1.1 | `models scan`, `punctuate 1` | selected; applied, 91 marks, 56 capitals, 0 kept; revision `0cea764…` |
| b6 | the same | the same | `models pull punctuation-horizon-small` | downloaded only `LICENSE` and `NOTICE` (both hashes as in the manifest); the model's modification time and size unchanged; `LICENSE-Apache-2.0.txt` left in place |
| b6 | now with `LICENSE` and `NOTICE` | the same | `punctuate 2` (the second import) | applied, 91 marks, 56 capitals, 0 kept; revision `0cea764…` |
| a | e2's fresh download (`--models-dir`) | built-in, v1.1.1 | `models scan`, `punctuate 1` | selected; applied, 91 marks, 56 capitals, 0 kept; revision `0cea764…` |

**The marks did not change.** Each run's JSON export compared with the beta.5 baseline's, word by word (speaker label, start, end, text, the speech model's text) and segment by segment (start, end, speaker label, text): all four identical, 464 words, 96 of them ending in `.` `,` or `?`, 131 changed from the speech model's text, 57 segments.

| Export | Words | Ending in a mark | Changed from the ASR text | Segments | Equal to beta.5 |
|---|---|---|---|---|---|
| b5, beta.5 baseline | 464 | 96 | 131 | 57 | (baseline) |
| b6 before the two files | 464 | 96 | 131 | 57 | yes, words and segments |
| b6 after the two files | 464 | 96 | 131 | 57 | yes |
| a, fresh v1.1.1 download | 464 | 96 | 131 | 57 | yes |

The only difference a run records is the revision in `engine_info.punctuation` (`0cea764…` instead of `ee31fdf…`).

**The firewall.** The first `models pull` of the new `diarist-cli.exe` into the empty root failed at its first request with os error 10013 (the socket refused by the per-application firewall, docs/INSTALL.md). Later requests of the same executable went through (most likely the firewall's prompt was allowed meanwhile); that attempt left no file, and the root above is a new empty one.

**Verdict:** every acceptance rule of plan 18 section 8.2 holds.

## The dated snapshot (prepared, not uploaded)

`C:\Users\ebin\claude-ground\diarist-releases\models-2026-10-v1.1.1-snapshot\` (outside the repository) holds `punctuation-model-card-0cea764.md` (14,142 bytes, `4f4ec573…`) and `punctuation-model-info-2026-10-30.json` (4,319 bytes, `f8055030…`), with a `CONTENTS.md` giving their sizes, hashes and sources. Where they are published (extra assets on `models-2026-10`, or the first release of a models repository) is the owner's decision; nothing was uploaded and the `models-2026-10` folder was not changed.
