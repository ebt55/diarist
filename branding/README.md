# Diarist brand marks

A diarist keeps a record. The mark is a page from that record: an ink tile with its
top right corner turned down, holding three lines of a transcript, each in the colour
the app gives to speakers 1, 2 and 3. It needs no text and reads at 16 px.

| File | Use |
| --- | --- |
| `logo.svg` | Master mark, colour. Source of the app icons. |
| `logo-mono.svg` | One colour (`currentColor`): favicons, monochrome tray, embossing, single-ink print. |
| `tray-recording.svg` | One-colour mark with the red recording dot. |
| `wordmark.svg` | Mark plus the name, for light backgrounds. |
| `wordmark-dark.svg` | Mark plus the name, for dark backgrounds. |
| `social.svg`, `social.png` | GitHub social preview, 1200 x 630. Not a README banner; the README header is the wordmark. |
| `preview.png` | Contact sheet: the mark at 16 to 512 px on light and dark, mono and tray marks. |
| `explorations/` | The variants that were tried, and `compare.png` (each at 16 to 128 px, plus 16 and 32 px zoomed). |

The Windows icon set in `apps/desktop/src-tauri/icons/` is generated from `logo.svg`.

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

## Geometry

512-unit square on a 32-unit grid, so edges land on whole pixels at 16, 32, 64, 128 and
256 px. Tile corner radius 112. Lines are 64 tall with 16 radius, start at x 96, sit at
y 96, 224 and 352, and run 224, 320 and 160 long (medium, long, short: turns in a
conversation, not a bullet list). The dog-ear legs are 128 (a quarter of the tile). In
the mono mark the dog-ear stands 32 units clear of the tile, one pixel at 16 px.

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

For the wordmark, pick the file by background. In a GitHub README:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="branding/wordmark-dark.svg">
  <img src="branding/wordmark.svg" alt="Diarist" height="64">
</picture>
```

`logo-mono.svg` uses `currentColor`. Inline in HTML it takes the text colour; as a
standalone file (an `<img>` or a favicon) `currentColor` falls back to black, so for a
favicon prefer `logo.svg`, and use the mono file where one ink is required.

## Tray

The Windows notification area shows icons at 16 px at 100% scaling (20 px at 125%, 24 px at
150%, 32 px at 200%). Today the app uses the colour window icon for the tray and paints the
recording dot at runtime (`#dc2626` on a white ring, lower right). `logo-mono.svg` and
`tray-recording.svg` are ready for a monochrome tray: rasterise them with the colour set to
white for a dark taskbar or `#1b1f27` for a light one, at 16, 20, 24 and 32 px.

## Do not

- Add gradients, shadows, glows, outlines, bevels or textures.
- Stretch, skew, rotate or flip the mark, or move the dog-ear to another corner.
- Recolour the lines, reorder them, change their lengths, or add a fourth.
- Use the lines without the tile, or put text inside the tile.
- Retype the name in a live font, or set it in another typeface; use the wordmark files.
- Put the colour mark on a busy photo; give it a plain field.
- Use the explorations as the logo.

## Regenerating

```sh
# Windows icon set (icon.ico 16 to 256, PNGs, Square*Logo.png, StoreLogo.png).
# The tool also writes android/, ios/ and icon.icns, which this Windows app does not ship.
cd apps/desktop && bunx tauri icon ../../branding/logo.svg -o src-tauri/icons

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

## Licence

The Diarist marks in this folder (logo, mono and tray marks, wordmarks, social preview) are
licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/): reuse and adapt
them with attribution to the Diarist project. The code in this repository stays MIT. The
letterforms in the wordmark and social preview are outlines of Inter by The Inter Project
Authors, licensed under the SIL Open Font License 1.1.
