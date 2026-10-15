# Contributing

Thanks for looking. Diarist is a small project with a few firm rules.

## The rules

- **Nothing leaves the machine.** No telemetry, no automatic network calls,
  no cloud fallback. A network request happens only when the user presses a
  button that says so (PRIVACY.md lists them all).
- **Nobody fights the app** (docs/00-product-spec.md). No hard gates: every
  action says what it is missing and offers the fix. Every download has Retry,
  a copyable link, the expected hash and its folder. Every error message ends
  with a next step.
- **The user owns the folders.** Models and runtimes are plain files; a file
  dropped into `models\` or `engine\` must work after Refresh.
- **Logic lives in `crates/core`**, testable without a GUI.
  `apps/desktop/src-tauri` holds only Tauri command wrappers and event
  plumbing; the Svelte code only renders (docs/01-architecture.md).
- **Measured, not assumed.** Claims about accuracy, speed or VRAM in docs and
  UI come from a measurement that is written down (docs/eval, docs/research).

## Setting up

Prerequisites, build, test and run commands are in [docs/DEV.md](docs/DEV.md).
In short, from the repository root:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cd apps/desktop && bun install && bun run build && bun run check
```

All of these must pass on every commit. Tests that need a GPU, the runtime,
models or audio devices are skipped unless their environment variables are
set (docs/DEV.md, "Gated tests").

Run the app against a scratch data folder so you never touch your real
transcripts:

```sh
set DIARIST_DATA_DIR=%TEMP%\diarist-dev
bun run tauri build --debug --no-bundle
target\debug\diarist.exe
```

## Changing types the UI sees

TypeScript types in `apps/desktop/src/lib/generated/` are generated from Rust
by ts-rs when the `diarist-core` tests run. Change the Rust type, run
`cargo test -p diarist-core`, commit both. Never edit the generated files.

## Pull requests

- Small, focused commits with a message that says why.
- New behaviour comes with a test in `crates/core` (or a written manual check
  for UI-only work).
- Keep the docs true: if you change behaviour described in README.md,
  docs/INSTALL.md or docs/DEV.md, update them in the same pull request.

## Reporting problems

Open an issue with what you did, what happened and what you expected. Paste
the output of Diagnostics, **Copy diagnostics**: it lists versions, the GPU and
driver, the selected models with hashes, the checklist and the last engine
errors, and no transcript text.

By contributing you agree that your contribution is licensed under the MIT
licence of this repository.
