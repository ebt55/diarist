# Licences

## Diarist

The code in this repository is MIT licensed ([LICENSE](LICENSE)).

## Downloaded at first run (not part of the installer)

The app downloads these on your request, from their publishers, under their
own licences. Read them before use, in particular the model licences.

| Component | Publisher | Licence | Source |
|---|---|---|---|
| NeMo-Speech.cpp runtime 0.2.0 (CUDA, Vulkan and CPU builds) | NVIDIA | Apache-2.0 | <https://github.com/NVIDIA/NeMo-Speech.cpp> |
| Nemotron 3 Diarization (q8_0 GGUF) | NVIDIA | OpenMDW-1.1 | <https://huggingface.co/nvidia/Nemotron-3-Diarization> |
| Nemotron Speech Streaming EN 0.6B (q8_0 GGUF) | NVIDIA | NVIDIA Open Model License | <https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b> |
| Nemotron 3.5 ASR Streaming 0.6B (q8_0 GGUF, optional) | NVIDIA | OpenMDW-1.1 | <https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b> |
| Parakeet TDT 0.6B v3 (q8_0 GGUF, experimental, optional) | NVIDIA | CC-BY-4.0 | <https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3> |
| Punctuation restoration small (int8 ONNX: `onnx/model_quantized.onnx` and `tokenizer.json` at revision `ee31fdf`; fine-tuned from jhu-clsp/mmBERT-small) | Horizon-Labs | Apache-2.0 (declared in the model card; no LICENSE file at the pinned revision); base model MIT | <https://huggingface.co/Horizon-Labs/punctuation-restoration-small> |
| ONNX Runtime 1.31.0 CPU, which runs the punctuation model (installed as `diarist-onnxruntime.dll`, from this project's `models-2026-10` release, Microsoft's file renamed) | Microsoft | MIT | <https://github.com/microsoft/onnxruntime> |

The exact files, revisions and SHA-256 hashes are pinned in
[manifests/models.json](manifests/models.json) and
[manifests/engine.json](manifests/engine.json). Models you add yourself carry
their own licences.

The punctuation model's folder also gets, from the same `models-2026-10`
release: `LICENSE-Apache-2.0.txt` (the Apache License 2.0 text, because the
model's repository has no licence file), `NOTICE-punctuation.txt` (the
model, its revision and SHA-256 hashes, the declared licence, the base
model and its licence, and the training data), and ONNX Runtime's
`onnxruntime-LICENSE.txt` and `onnxruntime-ThirdPartyNotices.txt`. These are
installed beside the files they cover. Diarist's own MIT licence is
unchanged.

The CUDA runtime build contains NVIDIA CUDA libraries (for example
`cublas64_13.dll`) redistributed by NVIDIA under the CUDA EULA; the Microsoft
Visual C++ runtime DLLs in the same folder are redistributed under Microsoft's
terms.

## In the installer

The Windows app is a Rust binary with a Svelte web front end, built with
Tauri 2. Its Rust dependency tree (413 third-party crates for
`x86_64-pc-windows-msvc`, counted with `cargo metadata` on 15 Oct 2026) is
under permissive licences:

| Licence (as declared) | Crates | Examples |
|---|---|---|
| MIT OR Apache-2.0 (any order) | 253 | tauri, serde, tokio, rusqlite, ureq, rubato, wasapi |
| MIT | 82 | auto-launch, bytes, derive_more |
| MPL-2.0 | 17 | symphonia (audio decoding), cssparser, selectors |
| Unicode-3.0 | 18 | the icu_* crates |
| Unlicense OR MIT | 11 | memchr, aho-corasick, walkdir |
| Apache-2.0 | 4 | hound, tao, zopfli, sync_wrapper |
| ISC, BSD-2/3-Clause, Zlib, 0BSD, CC0 or MIT-0 alternatives | 27 | rustls, ring, libloading, brotli, zlib-rs, subtle |
| CDLA-Permissive-2.0 | 1 | webpki-roots |

MPL-2.0 is a file-level copyleft: the symphonia, cssparser and selectors
sources are used unmodified and are available from crates.io. The bundled
SQLite (through `libsqlite3-sys`) is in the public domain.

To regenerate the list, run from the repository root:

```sh
cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-msvc
```

and group the `license` field of the packages reachable from `diarist`
(normal and build dependencies); `cargo license` or `cargo about` give the
same data if installed.

Front end (bundled into the app): Svelte (MIT), virtua (MIT),
`@tauri-apps/api` and `@tauri-apps/plugin-dialog` (MIT OR Apache-2.0).
