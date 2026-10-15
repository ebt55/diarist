# meet-assist

A fully local meeting transcriber for Windows. It records your microphone and
system audio (or imports a file), transcribes it, and labels who said what,
with all inference on your own GPU via NVIDIA NeMo-Speech.cpp. No cloud, no
account, no Python at runtime. Built with Rust, Tauri 2 and Svelte 5.

**Status: pre-alpha scaffold.** The repository builds, the domain model and
engine protocol are typed and tested, and the desktop shell lists fixture
meetings. There is no recording, transcription or database yet.

## Layout

```
crates/core/        meet-core: domain types, engine protocol; later db, audio, engine, models, transcript
crates/meet-cli/    developer CLI (no UI)
apps/desktop/       Tauri 2 app: src-tauri/ (thin Rust commands) + src/ (Svelte 5 + TypeScript)
manifests/          engine and model download manifests
tools/eval/         dev-only evaluation harness (not shipped)
docs/               product spec, architecture, data model, protocol, plans
```

## Development

Prerequisites, build, test and run commands: [docs/DEV.md](docs/DEV.md).
Design: [docs/01-architecture.md](docs/01-architecture.md).

## Licence

MIT for the code in this repository (see [LICENSE](LICENSE)). The inference
runtime (Apache-2.0) and the models (OpenMDW, NVIDIA Open Model License,
CC-BY-4.0) are downloaded separately at first run under their own licences.
