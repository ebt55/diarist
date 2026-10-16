# Branding and rename plan

Status: done, 15 Oct 2026. The name is Diarist (naming notes kept locally, outside the repository); the logo outcome and the rename are recorded below.

## Logo and app icon (brief for an Opus agent)

Concept: a rounded square tile containing three horizontal bars of different lengths, stacked like lines of a transcript, each bar in a different colour from the app's own speaker palette (`SPEAKER_PALETTE` in core, exported to TypeScript). It reads as "lines of a conversation by different people" at 16 px and at 512 px, needs no text, and works on light and dark backgrounds. Optional name tie-in: for Diarist, a small folded corner or bookmark tab on the tile (a diary); for Sayd, a short tail on the tile like a speech balloon.

Deliverables:
- `branding/logo.svg` (master, 512 viewBox, no text), `branding/logo-mono.svg` (single colour for favicons and tray), `branding/wordmark.svg` (icon plus the name set in a humanist sans, e.g. Inter or IBM Plex Sans, as outlines so no font dependency), `branding/tray-recording.svg` (mono tile with a red dot).
- The Tauri icon set regenerated from the master with `bunx tauri icon branding/logo.svg` (icon.ico with 16 to 256 px, icon.png, Square*.png, StoreLogo.png) into `apps/desktop/src-tauri/icons/`.
- A 1200 x 630 social preview `branding/social.png` rendered from SVG (resvg or the browser) for the GitHub repository.
- `branding/README.md`: colours (hex), clear space, minimum size, light and dark usage, "do not" list (no gradients, no stretching, no extra text), licence (CC-BY-4.0 for the marks so forks can keep attribution clear).

Constraints: pure SVG, no raster effects, no embedded fonts, under 8 kB each; the 16 px rendering of the mono icon must stay legible (test by rasterising at 16, 24, 32 px and viewing the PNGs); contrast of each bar colour against both #ffffff and #0f1115 at least 3:1.

## Rename checklist (done 15 Oct 2026)

The project was renamed from meet-assist (now Diarist; the naming decision is in local notes outside the repository). Identifiers, applied everywhere:
- Product name (installer, window title, tray): `Diarist`.
- Bundle identifier: `app.diarist.desktop`, so the default data root is `%LOCALAPPDATA%\app.diarist.desktop` (see migration).
- Executable `diarist.exe`; CLI `diarist-cli` (crate `diarist-cli` in `crates/cli`); core crate `diarist-core` (lib `diarist_core`); Tauri package `diarist` (lib `diarist_lib`).
- Registry key for a moved data root: `HKCU\Software\Diarist\DataRoot`.
- Repository `ebt55/diarist` on the owner's account (an org can take it over later; GitHub redirects). Updater endpoint `https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json` (GitHub's "latest" release skips pre-releases); the release workflow names releases "Diarist <tag>", uploads `Diarist_<version>_x64-setup.exe` and its `.sig` to a draft pre-release and opens a pull request for `updater/latest.json`.
- Environment variables `DIARIST_*` (the long-file acceptance WAV is `DIARIST_LONGFILE_WAV`); the eval harness uses `DIARIST_EVAL_*`.
- Database `diarist.db`; log file `logs\diarist.log` (logs folder unchanged).
- Docs: README, INSTALL, PRIVACY, CONTRIBUTING, CHANGELOG, LICENSES, docs/00 to 05, DEV and docs/plans carry the new name; docs/research and docs/eval stay as historical records with a note at the top.
- Tooling: `tools/eval` runs `.bin/diarist-cli.exe` (rebuild and copy it once, README).

Migration for existing installs (only the developer's today): `crates/core/src/models/legacy.rs` and `apps/desktop/src/components/LegacyMove.svelte`. When the new data root has no `diarist.db` and an old data root with a database exists (the old registry value, else the old default folder), the first launch runs on an empty in-memory store and asks once whether to move it in. Move reuses the Move data folder code (copy with the SQLite backup API, verify, restart, delete the old copy at the next launch, then the old registry value); Start fresh creates `diarist.db` and leaves the old folder alone. `DIARIST_LEGACY_ROOT` points the check at a scratch copy for tests. The old uninstaller is never run against the new root.

Acceptance: `git grep -niE "meet[-_]?assist|meet[-_](core|cli)"` finds the old name only in docs/research, docs/eval, CHANGELOG, the migration code and one-line history notes like the one above; workspace build, tests, clippy, fmt, bun build and check green; installer builds and installs under the new name; the first-run screen shows the new icon; the old data root migration prompt works.

## Logo outcome (15 Oct 2026)

Chosen: variant E, `branding/logo.svg`: ink tile `#1b1f27` with a folded top-right corner in the "me" slate `#64748b`, three lines (medium, long, short) in the first three speaker palette colours `#4e79a7`, `#f28e2b`, `#e15759`, on a 32-unit grid so edges land on whole pixels at 16 and 32 px. Contrast of the lines against the tile: 3.6, 6.8 and 4.5 to 1 (the lines never appear without the tile, so the plan's "against white" rule was relaxed for orange). Mono, tray-recording, wordmark (Inter 4.001 outlines, light and dark files), social preview and contact sheet are in `branding/`; the Tauri icon set is regenerated. Known limit: the tile outline is faint on very dark backgrounds; the lines and corner carry the shape. The app's tray still draws its red dot at runtime over the colour icon; switching it to `tray-recording.svg` is optional polish.

## Domain and key (15 Oct 2026)

- `diarist.dev` is registered to the owner. The landing page (GitHub Pages, plan in the "Landing page for diarist.dev" session card) goes there; the README links to it once the site is live.
- The updater signing key files were renamed to `%USERPROFILE%\.tauri\diarist.key` and `.pub` to match the docs; the key material is unchanged, so the public key already in `tauri.conf.json` still matches. The owner must back the private key up (no password set) before the first release.

## Follow-up: icon v2 (noted 15 Oct 2026)

On a dark Windows taskbar next to saturated app icons (Explorer, Firefox, Brave, VS Code, Notion), the Diarist icon reads as faint: the ink tile is 1.14:1 against the taskbar by design, so only the three thin lines and the grey corner remain, and they are the quietest thing in the row. The mark works as a mark; it fails as a taskbar icon. Rework before 1.0:

- Keep the concept (a page with three speaker lines and a turned corner); change the icon's surface, not the idea.
- Candidates to draw and compare: (a) a paper tile (`#f6f3ec`) with the ink lines, which pops on dark taskbars and was only rejected for fading on white; (b) a brand-blue tile (`#4e79a7`) with paper lines and the orange and red kept as two of the three lines; (c) the ink tile with a 1 px lighter edge (`#2a3040`) and lines 25 percent thicker and brighter, the smallest change. The "no border or glow" rule in branding/README.md is up for revision for the icon only; the in-app mark on the ink UI can stay as it is.
- The test that was missing the first time: a contact sheet of each candidate at 24, 32 and 48 px placed on screenshots of a real dark and a real light taskbar between Explorer, a browser and VS Code. Pick by that sheet, not by the mark alone.
- Scope: `apps/desktop/src-tauri/icons/` (taskbar, Start, tray), the favicon, and `branding/`. The wordmark, social preview and website can follow or keep the current mark; they are not the problem.

Outcome (16 Oct 2026). Seven candidates were drawn on the mark's grid and placed between the apps pinned on the developer's PC (Explorer, Firefox, Brave, VS Code, Notion, as the shell renders them) at 24, 32 and 48 px on a `#202020` and a `#f3f3f3` taskbar, with 16 px at 8x (`branding/explorations/taskbar-sheet-*.png`). That taskbar is auto-hidden and could not be captured, so the strips are drawn in flat colour; `#202020` matches the sliver the hidden taskbar leaves on screen. The numbers settled the approach: a tile that clears 3:1 on both taskbars needs a relative luminance between 0.143 and 0.265 (brand blue, 0.181, qualifies), and the speaker colours are mid-tones too, so on such a tile the red line is 1.2:1 and the orange 1.9:1. The outline comes from an edge instead. Chosen: candidate A4, a paper tile `#f6f3ec` (14.7:1 on `#202020`) with a 16-unit slate edge `#64748b` (3.4:1 on `#202020`, 4.3:1 on `#f3f3f3`), the slate dog-ear, and the three speaker lines with the orange deepened to an amber `#d27401` of the same hue, so every line clears 3:1 on paper (blue 4.1, amber 3.04, red 3.3). Two files: `branding/logo.svg` stays the mark (in-app sidebar, wordmark, social card) and the new `branding/icon.svg` is the source of the app icon set; the mono and tray marks are silhouettes and did not change. The Tauri icon set is regenerated from `icon.svg`, and `branding/tools/build_ico.py` rebuilds `icon.ico` with 16, 20, 24, 32, 48, 64, 128 and 256 px frames, the 16, 20 and 24 px ones from pixel-snapped drawings in `branding/icon-sizes/` (the grid falls on half pixels at 20 and 24 px, and the edge is half a pixel at 16); `taskbar-sheet-ico.png` shows the frames read back from the file. Not covered: the window and tray icons the app sets at runtime are Tauri's default window icon, the 256 px frame scaled down by Windows, so the snapped frames reach Explorer, Start, shortcuts and the installer but not the tray; that needs a change in `tray.rs`.
