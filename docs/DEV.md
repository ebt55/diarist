# Developer setup

Commands below were run on the reference machine (Windows 11, Rust 1.88.0
MSVC, Node 22.19, bun 1.2.1, VS Build Tools 2022 with the C++ workload,
WebView2 142) on 15 Oct 2026, except those marked *(not yet run)*. Run them
from Git Bash or PowerShell.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust (MSVC) | 1.88 for `crates/*`; **1.90+ for the desktop app on Tauri 2.12** | See "Toolchain blocker" below |
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
versions that support the workspace `rust-version` (1.88) when they exist.

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

### Toolchain blocker (open)

Tauri 2.12.x declares `rust-version = 1.90`, as do the crates released with it
(`tauri-build` 2.7, `tauri-runtime`/`tauri-runtime-wry` 2.12, `tauri-utils`
2.10, `tauri-macros`/`tauri-codegen` 2.7, `tauri-plugin` 2.7, `muda` 0.20,
`tauri-plugin-shell` 2.5, `tauri-plugin-dialog` 2.8, `tauri-plugin-log` 2.10,
`tauri-plugin-fs` 2.7). With Rust 1.88, `cargo build` stops with
`rustc 1.88.0 is not supported by the following packages`.

Until that is resolved, `apps/desktop/src-tauri` is left out of the workspace
`members` (see the comment in the root `Cargo.toml`), so `cargo` commands at
the root build only `crates/*`, and `bun run tauri ...` fails with "current
package believes it's in a workspace when it's not". Two ways to unblock:

1. **Rust 1.90 or newer** (keeps Tauri 2.12 as the ADR says): update the
   toolchain, set `rust-version = "1.90"` in the root `Cargo.toml`, add
   `"apps/desktop/src-tauri"` to `members`, then run the commands above.
2. **Stay on Rust 1.88 with Tauri 2.11.6**: in `apps/desktop/src-tauri/Cargo.toml`
   use `tauri = "2.11"`, and `"2"` for `tauri-build` and the three plugins (the
   resolver then picks tauri-build 2.6.3, shell 2.3.6, dialog 2.7.3, log 2.9.2);
   in `package.json` use `@tauri-apps/api ~2.11.1` and `@tauri-apps/cli ~2.11.5`
   (the CLI refuses mismatched major.minor between the crate and the npm API);
   add the member; `bun install`. This combination was built and smoke-tested
   end to end on the reference machine: all four cargo checks pass,
   `bun run tauri build --debug` produces the exe (18.3 MB) and the NSIS
   installer (3.6 MB), the window appears about 1.5 s after launch and lists
   the three fixture meetings, and the process stays up until stopped.
