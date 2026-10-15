# Developer setup

Commands below were run on the reference machine (Windows 11, Rust 1.99.0
MSVC, Node 22.19, bun 1.2.1, VS Build Tools 2022 with the C++ workload,
WebView2 142) on 15 Oct 2026, except those marked *(not yet run)*. Run them
from Git Bash or PowerShell.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust (MSVC) | 1.90+ (workspace `rust-version`; Tauri 2.12 needs it) | `rustup update stable` |
| VS Build Tools 2022 | C++ workload | linker and Windows SDK |
| bun | 1.2 | package manager and script runner for `apps/desktop` |
| Node | 22.12+ | Vite 8 runs on Node |
| WebView2 runtime | evergreen | preinstalled on Windows 11 |

No global installs: the Tauri CLI is a devDependency (`@tauri-apps/cli`) and is
run through `bun run tauri`.

## Rust workspace

From the repo root:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Dev CLI:

```sh
cargo run -p meet-cli -- version               # prints "meet-cli 0.1.0"
cargo run -p meet-cli -- transcribe some.wav   # stub: "not implemented", exit code 2
```

`.cargo/config.toml` turns on the MSRV-aware resolver
(`incompatible-rust-versions = "fallback"`), so `cargo update` picks dependency
versions that support the workspace `rust-version` (1.90) when they exist.

## Desktop frontend

From `apps/desktop`:

```sh
bun install
bun run build     # vite build into apps/desktop/dist
bun run check     # svelte-check (TypeScript + Svelte)
bun run dev       # Vite dev server on http://127.0.0.1:1420 (no Tauri APIs in a plain browser)
```

If `bun install` fails with `ConnectionRefused downloading package manifest`,
re-run it; the registry connection is occasionally refused on this network.

## Desktop app (Tauri)

From `apps/desktop`:

```sh
bun run tauri dev            # dev window with hot reload (not yet run)
bun run tauri build --debug  # target/debug/meet-assist.exe + target/debug/bundle/nsis/*-setup.exe
bun run tauri build          # release build and NSIS installer (not yet run)
```

The Tauri CLI downloads NSIS into its cache on the first bundle. If that
download is blocked, add `--no-bundle` to get just the exe.

Smoke test (PowerShell), as used for Phase 0 acceptance:

```powershell
$p = Start-Process -FilePath .\target\debug\meet-assist.exe -PassThru
Start-Sleep -Seconds 5
$p.Refresh(); "alive: $(-not $p.HasExited)"
Stop-Process -Id $p.Id; $p.WaitForExit(5000) | Out-Null; "exited: $($p.HasExited)"
```

Runtime locations: app log under `%LOCALAPPDATA%\app.meetassist.desktop\logs\`,
WebView2 profile under `%LOCALAPPDATA%\app.meetassist.desktop\EBWebView\`.
To inspect the page from outside, start the exe with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` and
attach any Chrome DevTools client to `http://127.0.0.1:9223`.

### Tauri versions

The `tauri` crate (2.12) and the npm `@tauri-apps/api` / `@tauri-apps/cli`
packages (~2.12) must stay on the same major.minor: the CLI refuses to build
otherwise. Same for any `@tauri-apps/plugin-*` package added later and its
crate.
