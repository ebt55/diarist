# Diarist brand marks

A diarist keeps a record. The mark is a page from that record: an ink tile with its
top right corner turned down, holding three lines of a transcript, each in the colour
the app gives to speakers 1, 2 and 3. It needs no text and reads at 16 px.

The app icon is the same page with a slate edge, in a pair like the wordmark: `icon.svg`,
the ink page, for light backgrounds, and `icon-dark.svg`, the page on paper, for dark ones.
On a taskbar it has to stand out among other apps' icons, which the mark's ink tile alone
does not.

| File | Use |
| --- | --- |
| `logo.svg` | Master mark, colour: the wordmark, the social card. |
| `icon.svg` | App icon for light backgrounds: the ink page with a slate edge and brighter lines. |
| `icon-dark.svg` | App icon for dark backgrounds: the page on paper with a slate edge. The installer and exe icon (`icon.ico`) is `icon-dark`. |
| `icon-sizes/` | Both icons redrawn on the pixel grid at 16, 20 and 24 px (`light-*`, `dark-*`), and `icon.svg` at 48 px, used for those frames. |
| `logo-mono.svg` | One colour (`currentColor`): favicons, monochrome tray, embossing, single-ink print. |
| `tray-recording.svg` | One-colour mark with the red recording dot. |
| `wordmark.svg` | Mark plus the name, for light backgrounds. |
| `wordmark-dark.svg` | Mark plus the name, for dark backgrounds. |
| `social.svg`, `social.png` | GitHub social preview, 1200 x 630. Not a README banner; the README header is the wordmark. |
| `preview.png` | Contact sheet: the mark and both icons at 16 to 128 px on light and dark, mono and tray marks. |
| `tools/` | `build_ico.py` (builds `icon.ico` and the theme frame sets) and `preview.py` (draws `preview.png`). |
| `explorations/` | The variants that were tried: the mark (`compare.png`) and the icon (`taskbar-sheet-*.png`, with the tools that draw them). |

The Windows icon set in `apps/desktop/src-tauri/icons/` is generated from `icon-dark.svg`.
`icons/theme/light/` and `icons/theme/dark/` hold `icon.svg` and `icon-dark.svg` at 16, 20, 24,
32 and 48 px (`<size>.png`), for the running app to follow the taskbar theme.

## Colours

Bar colours are the first three entries of `SPEAKER_PALETTE` in the core crate
(`crates/core/src/views.rs`), in order; the dog-ear is the `ME_COLOR` slate. Keep them in step
if the palette ever changes.

| Role | Hex | On tile `#1b1f27` | On `#ffffff` | On `#0f1115` |
| --- | --- | --- | --- | --- |
| Tile (ink) | `#1b1f27` | | 16.5:1 | 1.14:1 |
| Line 1, speaker 1 (blue) | `#4e79a7` | 3.6:1 | 4.5:1 | 4.2:1 |
| Line 2, speaker 2 (orange) | `#f28e2b` | 6.8:1 | 2.4:1 | 7.8:1 |
| Line 3, speaker 3 (red) | `#e15759` | 4.5:1 | 3.7:1 | 5.1:1 |
| Dog-ear (the "me" slate) | `#64748b` | 3.5:1 | 4.8:1 | 4.0:1 |
| Recording dot | `#dc2626` | | | |
| Name on light | `#1b1f27` | | | |
| Name on dark | `#f3f4f6` | | | |
| Social card paper / tagline | `#f6f3ec` / `#4b5262` | | | |

Every line clears 3:1 against the tile, which is the only surface it ever sits on. The
orange line would not clear 3:1 on white, which is one more reason never to use the lines
without the tile.

The app icons sit on taskbars, `#202020` dark and `#f3f3f3` light:

| Role | `icon.svg` (light backgrounds) | `icon-dark.svg` (dark backgrounds) |
| --- | --- | --- |
| Tile | ink `#1b1f27`: 14.9:1 on `#f3f3f3`, 1.0:1 on `#202020` | paper `#f6f3ec`: 14.7:1 on `#202020`, 1.0:1 on `#f3f3f3` |
| Edge and dog-ear | slate `#64748b`: 4.3:1 on `#f3f3f3`, 3.4:1 on `#202020` | the same |
| Line 1 | `#5b86b5`: 4.3:1 on ink | `#4e79a7` (speaker 1): 4.1:1 on paper |
| Line 2 | `#ffa446`: 8.4:1 on ink | `#d27401` (icon amber): 3.04:1 on paper |
| Line 3 | `#f26767`: 5.4:1 on ink | `#e15759` (speaker 3): 3.3:1 on paper |

Each icon's tile carries it on the taskbar it is made for, and the slate edge gives it an
outline on the other. The line colours are for the icons only; the app keeps the palette.
`icon.svg`'s lines are the speaker colours raised 25 percent in relative luminance (in
OKLab lightness). In `icon-dark.svg` the speaker 2 orange, 2.2:1 on paper, is deepened to
`#d27401`: the same hue (OKLCH 59.4), the most saturated sRGB colour with relative
luminance at or below 0.263.

## Geometry

512-unit square on a 32-unit grid, so edges land on whole pixels at 16, 32, 64, 128 and
256 px. Tile corner radius 112. Lines are 64 tall with 16 radius, start at x 96, sit at
y 96, 224 and 352, and run 224, 320 and 160 long (medium, long, short: turns in a
conversation, not a bullet list). The dog-ear legs are 128 (a quarter of the tile). In
the mono mark the dog-ear stands 32 units clear of the tile, one pixel at 16 px.

The app icons have the same outline and dog-ear. Their edge is the tile filled in slate
with the ink or paper tile inset 16 units on top (1 px at 32 px); no strokes.
`icon-dark.svg` has the mark's lines; `icon.svg`'s are 80 tall (25 percent thicker), tops on
the grid. The 32-unit grid does not land on whole pixels at 20 or 24 px, and the edge is
half a pixel at 16 px, so `icon-sizes/` redraws both icons on the pixel grid at those sizes,
in one geometry (a 1 px edge; 2 px lines at 16 px, 3 px at 20 and 24 px), and `icon.ico`
and the theme frame sets take their 16, 20 and 24 px frames from those files. `icon.svg`'s
thicker lines are 7.5 px at 48 px, so `light-48.svg` sets them at 7 px.

## Clear space and minimum size

- Clear space: at least a quarter of the mark's height on every side (128 at 512, 8 at
  32 px). The wordmark keeps the same clear space, measured from the mark's height.
- Mark: 16 px minimum. Prefer sizes on the 16 px grid (16, 32, 48, 64...) where you can
  choose; 24 px is fine.
- Wordmark: 24 px tall minimum. Below that, use the mark alone.
- Mono and tray marks: 16 px minimum.

## Light and dark

The colour mark carries its own background, so use the same `logo.svg` everywhere. On dark
UI the tile edge is quiet by design (1.14:1 against `#0f1115`); the lines and the dog-ear
give the shape. Do not add a border or glow to lift it.

The app icon comes as a pair, named like the wordmark: `icon.svg` (ink) for light
backgrounds and `icon-dark.svg` (paper) for dark ones. The running app picks the frame set
by the taskbar theme (`icons/theme/light`, `icons/theme/dark`). `icon.ico`, the one static
icon Windows embeds in the exe and the installer, is built from `icon-dark.svg`: its paper
tile stands out on a dark taskbar and its slate edge holds its outline on a light one, so it
reads on both.

For the wordmark, pick the file by background. In a GitHub README:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="branding/wordmark-dark.svg">
  <img src="branding/wordmark.svg" alt="Diarist" height="64">
</picture>
```

`logo-mono.svg` uses `currentColor`. Inline in HTML it takes the text colour; as a
standalone file (an `<img>` or a favicon) `currentColor` falls back to black, so for a
favicon prefer an app icon (`icon.svg` on a light page, `icon-dark.svg` on a dark one), and
use the mono file where one ink is required.

## Tray

The Windows notification area shows icons at 16 px at 100% scaling (20 px at 125%, 24 px at
150%, 32 px at 200%). Today the app uses the colour window icon for the tray and paints the
recording dot at runtime (`#dc2626` on a white ring, lower right), taking the frame for
the display's scale (`apps/desktop/src-tauri/src/icons.rs`); `icons/theme/` holds both
icons for it to follow the taskbar theme. `logo-mono.svg` and `tray-recording.svg` are
ready for a monochrome tray: rasterise them with the colour set to white for a dark
taskbar or `#1b1f27` for a light one, at 16, 20, 24 and 32 px.

## Do not

- Add gradients, shadows, glows, outlines, bevels or textures. The slate edge of the app
  icons (`icon.svg`, `icon-dark.svg`) is the one outline, and it belongs to them only.
- Stretch, skew, rotate or flip the mark, or move the dog-ear to another corner.
- Recolour the lines, reorder them, change their lengths, or add a fourth. The brighter
  lines of `icon.svg` and the amber `#d27401` of `icon-dark.svg` are the only recolourings,
  and they belong to the app icons only.
- Put the app icons in the wordmark or the social card, or use `logo.svg` as the app icon.
- Use the lines without the tile, or put text inside the tile.
- Retype the name in a live font, or set it in another typeface; use the wordmark files.
- Put the colour mark on a busy photo; give it a plain field.
- Use the explorations as the logo.

## Regenerating

```sh
# Windows icon set from the dark-background icon (PNGs, Square*Logo.png, StoreLogo.png, a
# first icon.ico). The tool also writes android/, ios/ and icon.icns, which this Windows
# app does not ship; delete them.
cd apps/desktop && bunx tauri icon ../../branding/icon-dark.svg -o src-tauri/icons

# Then, from the repository root (needs resvg on PATH): icon.ico from icon-dark.svg with 16,
# 20, 24, 32, 48, 64, 128 and 256 px frames, and icons/theme/{light,dark}/<size>.png at 16,
# 20, 24, 32 and 48 px from icon.svg and icon-dark.svg; 16, 20 and 24 px from icon-sizes/.
uv run --with pillow python -I branding/tools/build_ico.py

# Contact sheet
uv run --with pillow python -I branding/tools/preview.py

# Social preview (resvg: cargo install resvg)
resvg branding/social.svg branding/social.png
```

The name and tagline are Inter 4.001 (variable: `InterVariable.ttf` from the rsms/inter
v4.1 release) converted to outlines with fontTools, kerned from the font's GPOS table: the
name at weight 600, optical size 32, the tagline at weight 400, optical size 22. Path data
is relative, in units of 1/1024 em (name) and 1/512 em (tagline), with points rounded to
whole units, placed with a `matrix` transform. The tagline, "Meeting transcripts with
speaker labels, on your own PC. Free forever.", is set at 44 px and does not fit one line
at that size, so it is two lines broken after the comma, each centred on the card by its
advance width. The first baseline is at y 452, the second one Inter line height (1.21 em,
53.2 px) below it, at 505.2.

## Explorations

- `a-ink-fold`: ink tile, top right dog-ear, rounded pill lines. The base of the chosen mark.
- `b-ink-curl`: dog-ear bottom right, lines long to short. Reads as an "align left" icon.
- `c-ink-ribbon`: bookmark ribbon from the top edge. At 16 px the ribbon reads as a fourth line.
- `d-paper-fold`: paper tile with dark lines. Fades on white at small sizes; orange cannot be used.
- `e-ink-fold-square`: as A with squarer line ends (radius 16). Crisper at 16 px and more
  exact in tone; chosen as `logo.svg`.

Icon v2 (October 2026), compared in the taskbar between the apps pinned on the
developer's PC (`taskbar-sheet-*.png`; `tools/` collects the neighbours and draws them):

- `v2-a-paper`: paper tile, ink lines. Strongest on a dark taskbar; vanishes on a light one.
- `v2-a2-paper-colour`: as A with the speaker lines; the orange is 2.2:1 on paper.
- `v2-a3-paper-slate-edge`: as A with the slate edge; holds on both taskbars, ink lines.
- `v2-a4-paper-colour-edge`: as A3 with the speaker lines and the amber. Chosen as `icon-dark.svg`.
- `v2-b-blue`: brand-blue tile, the only tile that clears 3:1 on both taskbars; the orange
  and red lines are 1.9 and 1.2:1 on it.
- `v2-c-ink-edge`, `v2-c2-ink-slate-edge`: the ink tile with a `#2a3040` or a slate edge
  and thicker, brighter lines; the slate edge works, but the icon stays an outline on dark.
  C2 was chosen as `icon.svg`, the pair's icon for light backgrounds.

## Licence

The Diarist logo, app icons, mono and tray marks, wordmarks and social preview in this
folder, and the icon set generated from them in `apps/desktop/src-tauri/icons/`, are
copyright Ebin Babu Thomas and are not covered by the MIT licence. "Diarist" is the name of
the official project. You may use the name and these files unmodified to refer to Diarist:
in articles, reviews and talks, in links to this repository or diarist.dev, and in package
listings (winget, Scoop, Chocolatey and the like) that install the unmodified official
builds. You may not use them, or anything confusingly similar, on a fork, a modified build
or any other product: forks pick their own name and icon. For anything else, open an issue
and ask.

Earlier versions of these files were offered under CC BY 4.0; copies taken under that
licence keep it. The `explorations/taskbar-sheet-*.png` screenshots include other vendors'
icons, which belong to their owners and appear only for comparison. The letterforms in the
wordmark and social preview are outlines of Inter by The Inter Project Authors, licensed
under the SIL Open Font License 1.1.
