# Branding and rename plan

Status: draft, 15 Oct 2026. Name decision pending between Diarist (recommended) and the Green Lantern check (Sayd). The logo concept below is name-agnostic; the wordmark and the rename checklist apply once the name is final.

## Logo and app icon (brief for an Opus agent)

Concept: a rounded square tile containing three horizontal bars of different lengths, stacked like lines of a transcript, each bar in a different colour from the app's own speaker palette (`SPEAKER_PALETTE` in core, exported to TypeScript). It reads as "lines of a conversation by different people" at 16 px and at 512 px, needs no text, and works on light and dark backgrounds. Optional name tie-in: for Diarist, a small folded corner or bookmark tab on the tile (a diary); for Sayd, a short tail on the tile like a speech balloon.

Deliverables:
- `branding/logo.svg` (master, 512 viewBox, no text), `branding/logo-mono.svg` (single colour for favicons and tray), `branding/wordmark.svg` (icon plus the name set in a humanist sans, e.g. Inter or IBM Plex Sans, as outlines so no font dependency), `branding/tray-recording.svg` (mono tile with a red dot).
- The Tauri icon set regenerated from the master with `bunx tauri icon branding/logo.svg` (icon.ico with 16 to 256 px, icon.png, Square*.png, StoreLogo.png) into `apps/desktop/src-tauri/icons/`.
- A 1200 x 630 social preview `branding/social.png` rendered from SVG (resvg or the browser) for the GitHub repository.
- `branding/README.md`: colours (hex), clear space, minimum size, light and dark usage, "do not" list (no gradients, no stretching, no extra text), licence (CC-BY-4.0 for the marks so forks can keep attribution clear).

Constraints: pure SVG, no raster effects, no embedded fonts, under 8 kB each; the 16 px rendering of the mono icon must stay legible (test by rasterising at 16, 24, 32 px and viewing the PNGs); contrast of each bar colour against both #ffffff and #0f1115 at least 3:1.

## Rename checklist (from "meet-assist" to the final name)

Decide the identifiers once, then apply everywhere in one agent chunk:
- Product name (installer, window title, tray): `Diarist`.
- Bundle identifier: `app.diarist.desktop` (changes `app_local_data_dir`; see migration).
- Executable: `diarist.exe`; CLI: `diarist-cli` (crate `diarist-cli`); core crate: `diarist-core`; Tauri crate package `diarist`.
- Registry key for the data root: `HKCU\Software\Diarist\DataRoot`.
- GitHub org/repo: `diarist-app/diarist` (bare `diarist` handle is dormant); updater endpoint in `tauri.conf.json`; release workflow; README badges.
- Environment variables used by tests and the CLI: `MEET_ASSIST_*` to `DIARIST_*`.
- Database file name `meet-assist.db` to `diarist.db`; logs folder unchanged.
- Docs: README, INSTALL, PRIVACY, CONTRIBUTING, CHANGELOG, docs/00 to 05, docs/plans; keep docs/research and docs/eval as historical records with a one-line note at the top of each that the project was renamed.
- Memory and tooling outside the repo: `tools/eval` paths, `.bin`, configs.

Migration for existing installs (only the developer's today): on first launch, if the new data root is empty and the old one (`%LOCALAPPDATA%\app.meetassist.desktop`) exists, offer "Move your existing data" using the existing Move data folder code; otherwise start fresh. The old uninstaller is never run against the new root.

Acceptance: no occurrence of the old name remains outside `docs/research`, `docs/eval` and CHANGELOG history (`git grep -iE "meet-assist|meetassist|meet_assist|meet-core|meet_core|meet-cli|meet_cli"` is empty elsewhere); workspace build, tests, clippy, fmt, bun build and check green; installer builds and installs under the new name; the first-run screen shows the new icon; the old data root migration prompt works.
