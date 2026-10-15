# Branding and rename plan

Status: done, 15 Oct 2026. The name is Diarist (`docs/research/09-naming.md`); the logo outcome and the rename are recorded below.

## Logo and app icon (brief for an Opus agent)

Concept: a rounded square tile containing three horizontal bars of different lengths, stacked like lines of a transcript, each bar in a different colour from the app's own speaker palette (`SPEAKER_PALETTE` in core, exported to TypeScript). It reads as "lines of a conversation by different people" at 16 px and at 512 px, needs no text, and works on light and dark backgrounds. Optional name tie-in: for Diarist, a small folded corner or bookmark tab on the tile (a diary); for Sayd, a short tail on the tile like a speech balloon.

Deliverables:
- `branding/logo.svg` (master, 512 viewBox, no text), `branding/logo-mono.svg` (single colour for favicons and tray), `branding/wordmark.svg` (icon plus the name set in a humanist sans, e.g. Inter or IBM Plex Sans, as outlines so no font dependency), `branding/tray-recording.svg` (mono tile with a red dot).
- The Tauri icon set regenerated from the master with `bunx tauri icon branding/logo.svg` (icon.ico with 16 to 256 px, icon.png, Square*.png, StoreLogo.png) into `apps/desktop/src-tauri/icons/`.
- A 1200 x 630 social preview `branding/social.png` rendered from SVG (resvg or the browser) for the GitHub repository.
- `branding/README.md`: colours (hex), clear space, minimum size, light and dark usage, "do not" list (no gradients, no stretching, no extra text), licence (CC-BY-4.0 for the marks so forks can keep attribution clear).

Constraints: pure SVG, no raster effects, no embedded fonts, under 8 kB each; the 16 px rendering of the mono icon must stay legible (test by rasterising at 16, 24, 32 px and viewing the PNGs); contrast of each bar colour against both #ffffff and #0f1115 at least 3:1.

## Rename checklist (done 15 Oct 2026)

The project was renamed from meet-assist (now Diarist; decision in `docs/research/09-naming.md`). Identifiers, applied everywhere:
- Product name (installer, window title, tray): `Diarist`.
- Bundle identifier: `app.diarist.desktop`, so the default data root is `%LOCALAPPDATA%\app.diarist.desktop` (see migration).
- Executable `diarist.exe`; CLI `diarist-cli` (crate `diarist-cli` in `crates/cli`); core crate `diarist-core` (lib `diarist_core`); Tauri package `diarist` (lib `diarist_lib`).
- Registry key for a moved data root: `HKCU\Software\Diarist\DataRoot`.
- Repository `ebt55/diarist` on the owner's account (an org can take it over later; GitHub redirects). Updater endpoint `https://github.com/ebt55/diarist/releases/latest/download/latest.json`; the release workflow names releases "Diarist <tag>" and uploads `Diarist_<version>_x64-setup.exe`.
- Environment variables `DIARIST_*` (the long-file acceptance WAV is `DIARIST_LONGFILE_WAV`); the eval harness uses `DIARIST_EVAL_*`.
- Database `diarist.db`; log file `logs\diarist.log` (logs folder unchanged).
- Docs: README, INSTALL, PRIVACY, CONTRIBUTING, CHANGELOG, LICENSES, docs/00 to 05, DEV and docs/plans carry the new name; docs/research and docs/eval stay as historical records with a note at the top.
- Tooling: `tools/eval` runs `.bin/diarist-cli.exe` (rebuild and copy it once, README).

Migration for existing installs (only the developer's today): `crates/core/src/models/legacy.rs` and `apps/desktop/src/components/LegacyMove.svelte`. When the new data root has no `diarist.db` and an old data root with a database exists (the old registry value, else the old default folder), the first launch runs on an empty in-memory store and asks once whether to move it in. Move reuses the Move data folder code (copy with the SQLite backup API, verify, restart, delete the old copy at the next launch, then the old registry value); Start fresh creates `diarist.db` and leaves the old folder alone. `DIARIST_LEGACY_ROOT` points the check at a scratch copy for tests. The old uninstaller is never run against the new root.

Acceptance: `git grep -niE "meet[-_]?assist|meet[-_](core|cli)"` finds the old name only in docs/research, docs/eval, CHANGELOG, the migration code and one-line history notes like the one above; workspace build, tests, clippy, fmt, bun build and check green; installer builds and installs under the new name; the first-run screen shows the new icon; the old data root migration prompt works.

## Logo outcome (15 Oct 2026)

Chosen: variant E, `branding/logo.svg`: ink tile `#1b1f27` with a folded top-right corner in the "me" slate `#64748b`, three lines (medium, long, short) in the first three speaker palette colours `#4e79a7`, `#f28e2b`, `#e15759`, on a 32-unit grid so edges land on whole pixels at 16 and 32 px. Contrast of the lines against the tile: 3.6, 6.8 and 4.5 to 1 (the lines never appear without the tile, so the plan's "against white" rule was relaxed for orange). Mono, tray-recording, wordmark (Inter 4.001 outlines, light and dark files), social preview and contact sheet are in `branding/`; the Tauri icon set is regenerated. Known limit: the tile outline is faint on very dark backgrounds; the lines and corner carry the shape. The app's tray still draws its red dot at runtime over the colour icon; switching it to `tray-recording.svg` is optional polish.
