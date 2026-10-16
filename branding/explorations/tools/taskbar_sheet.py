"""Taskbar contact sheets for the icon v2 candidates (docs/plans/12-release-beta2.md, 3.B).

Each candidate is rasterised with resvg at 16, 24, 32 and 48 px and placed in a Windows 11
taskbar strip between the apps pinned on this machine, in the dark (#202020) and the light
(#f3f3f3) taskbar colour, at the three taskbar icon sizes (24 px at 100 percent scaling,
32 px, 48 px at 200 percent), plus the 16 px rendering at 8x zoom. Strip geometry follows
the Windows 11 taskbar at 100 percent (48 px tall, 44 px per button, a 6 x 3 running pill),
scaled with the icon.

Inputs: the neighbour icons written by taskbar-icons.ps1 (<app>-<size>.png, as the shell
renders them). Outputs: taskbar-sheet-<name>.png per candidate and taskbar-sheet-all.png in
branding/explorations/, and the contrast table on stdout.

    powershell -ExecutionPolicy Bypass -File branding/explorations/tools/taskbar-icons.ps1 -Out <icons>
    uv run --with pillow python -I branding/explorations/tools/taskbar_sheet.py --neighbours <icons> --work <tmp>
"""

import argparse
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
EXPLORATIONS = HERE.parent
BRANDING = EXPLORATIONS.parent

DARK, LIGHT = "#202020", "#f3f3f3"
SIZES = (24, 32, 48)
NEIGHBOURS_LEFT = ("file-explorer", "firefox", "brave")
NEIGHBOURS_RIGHT = ("visual-studio-code", "notion")
RUNNING = {"file-explorer", "firefox", "diarist", "visual-studio-code"}

PALETTE = ("#4e79a7", "#f28e2b", "#e15759")
BRIGHT = ("#5b86b5", "#ffa446", "#f26767")
INK, PAPER, SLATE = "#1b1f27", "#f6f3ec", "#64748b"
# Speaker 2 orange deepened to clear 3:1 on paper: same OKLCH hue (59.4), the most saturated
# sRGB colour at that hue with relative luminance at or below 0.263.
AMBER = "#d27401"

# name, file, label, tile, edge, dog-ear, lines
CANDIDATES = [
    ("current", BRANDING / "logo.svg", "current logo.svg: ink tile", INK, None, SLATE, PALETTE),
    ("v2-a-paper", EXPLORATIONS / "v2-a-paper.svg", "A: paper tile, ink lines", PAPER, None, SLATE, (INK,) * 3),
    ("v2-a2-paper-colour", EXPLORATIONS / "v2-a2-paper-colour.svg", "A2: paper tile, speaker lines", PAPER, None, SLATE, PALETTE),
    ("v2-a3-paper-slate-edge", EXPLORATIONS / "v2-a3-paper-slate-edge.svg", "A3: paper tile, slate edge, ink lines", PAPER, SLATE, SLATE, (INK,) * 3),
    ("v2-a4-paper-colour-edge", EXPLORATIONS / "v2-a4-paper-colour-edge.svg", "A4: paper tile, slate edge, speaker lines", PAPER, SLATE, SLATE, ("#4e79a7", AMBER, "#e15759")),
    ("v2-b-blue", EXPLORATIONS / "v2-b-blue.svg", "B: brand-blue tile", "#4e79a7", None, "#a2b6ca", (PAPER, "#f28e2b", "#e15759")),
    ("v2-c-ink-edge", EXPLORATIONS / "v2-c-ink-edge.svg", "C: ink tile, #2a3040 edge, thicker lines", INK, "#2a3040", SLATE, BRIGHT),
    ("v2-c2-ink-slate-edge", EXPLORATIONS / "v2-c2-ink-slate-edge.svg", "C2: ink tile, slate edge, thicker lines", INK, SLATE, SLATE, BRIGHT),
]
# The short list for the pick (taskbar-sheet-final.png).
FINAL = ("current", "v2-a3-paper-slate-edge", "v2-a4-paper-colour-edge", "v2-c2-ink-slate-edge")


def luminance(hex_colour):
    def lin(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (int(hex_colour[i : i + 2], 16) for i in (1, 3, 5))
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def contrast(a, b):
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def font(size, bold=False):
    name = "segoeuib.ttf" if bold else "segoeui.ttf"
    try:
        return ImageFont.truetype(f"C:/Windows/Fonts/{name}", size)
    except OSError:
        return ImageFont.load_default()


def rasterise(svg, work, name):
    out = {}
    for px in (16, 20, 24, 32, 48):
        png = work / f"{name}-{px}.png"
        subprocess.run(["resvg", "-w", str(px), "-h", str(px), str(svg), str(png)], check=True)
        out[px] = Image.open(png).convert("RGBA")
    return out


def strip(icon, neighbours, size, bg):
    """One taskbar strip: a free button on each side, then the pinned apps with Diarist in the middle."""
    s = size / 24
    pitch, height = round(44 * s), round(48 * s)
    apps = [*NEIGHBOURS_LEFT, "diarist", *NEIGHBOURS_RIGHT]
    im = Image.new("RGB", (pitch * (len(apps) + 2), height), bg)
    d = ImageDraw.Draw(im)
    pill = "#a0a0a0" if bg == DARK else "#6e6e6e"
    pw, ph = round(6 * s), max(2, round(3 * s))
    for i, app in enumerate(apps, start=1):
        img = icon[size] if app == "diarist" else neighbours[app][size]
        cx = i * pitch + pitch // 2
        im.paste(img, (cx - size // 2, (height - size) // 2), img)
        if app in RUNNING:
            y = height - ph - round(3 * s)
            d.rounded_rectangle((cx - pw // 2, y, cx - pw // 2 + pw - 1, y + ph - 1), radius=ph / 2, fill=pill)
    return im


def zoom(icon, bg, px=16):
    """The icon at px on a 2 px margin of taskbar colour, at 8x."""
    field = Image.new("RGB", (px + 4, px + 4), bg)
    field.paste(icon[px], (2, 2), icon[px])
    return field.resize((field.width * 8, field.height * 8), Image.NEAREST)


def framed(canvas, im, xy):
    x, y = xy
    ImageDraw.Draw(canvas).rectangle((x - 1, y - 1, x + im.width, y + im.height), outline="#c4c4c4")
    canvas.paste(im, (x, y))


def contrast_line(c):
    _, _, _, tile, edge, ear, lines = c
    parts = [f"tile {contrast(tile, DARK):.2f}:1 on {DARK}, {contrast(tile, LIGHT):.2f}:1 on {LIGHT}"]
    if edge:
        parts.append(f"edge {contrast(edge, DARK):.2f} / {contrast(edge, LIGHT):.2f}")
    parts.append("lines on tile " + ", ".join(f"{contrast(l, tile):.2f}" for l in lines))
    parts.append(f"dog-ear on tile {contrast(ear, tile):.2f}")
    return ";  ".join(parts)


def candidate_sheet(c, icon, neighbours):
    name, _, label, *_ = c
    label_w, x0 = 70, 24
    widths = {s: strip(icon, neighbours, s, DARK).width for s in SIZES}
    zx = x0 + label_w + max(widths.values()) + 48
    W = zx + 2 * 160 + 24 + x0
    H = 92 + 2 * (36 + sum(round(48 * s / 24) + 18 for s in SIZES)) + 30
    sheet = Image.new("RGB", (W, H), "#ffffff")
    d = ImageDraw.Draw(sheet)
    d.text((x0, 16), f"{name}.svg  ({label})", fill="#111111", font=font(22, True))
    d.text((x0, 50), contrast_line(c), fill="#444444", font=font(15))
    y = 92
    for theme, bg in (("Dark taskbar", DARK), ("Light taskbar", LIGHT)):
        d.text((x0, y), f"{theme} ({bg})", fill="#111111", font=font(17, True))
        y += 36
        top = y
        for s in SIZES:
            st = strip(icon, neighbours, s, bg)
            d.text((x0, y + st.height // 2 - 10), f"{s} px", fill="#444444", font=font(15))
            framed(sheet, st, (x0 + label_w, y))
            y += st.height + 18
        d.text((zx, top - 30), "16 px, 8x", fill="#444444", font=font(15))
        framed(sheet, zoom(icon, bg), (zx, top))
    d.text((x0, H - 26), "Neighbours: the apps pinned on this PC, as the shell renders them at each size. "
           f"Dark is {DARK}, the colour sampled from this PC's taskbar; light is {LIGHT}.",
           fill="#777777", font=font(13))
    return sheet


def matrix_sheet(cands, icons, neighbours, title, zooms=(16,)):
    """One row per candidate: the three taskbar strips, then the zoomed small sizes; dark block, then light."""
    x0, label_w, gap = 24, 300, 24
    widths = [strip(icons["current"], neighbours, s, DARK).width for s in SIZES]
    zw = [(z + 4) * 8 for z in zooms]
    W = x0 + label_w + sum(widths) + gap * len(SIZES) + sum(zw) + gap * len(zooms) + x0
    row_h = max(zw) + 16
    H = 70 + 2 * (40 + row_h * len(cands)) + 30
    sheet = Image.new("RGB", (W, H), "#ffffff")
    d = ImageDraw.Draw(sheet)
    d.text((x0, 16), title, fill="#111111", font=font(22, True))
    y = 70
    for theme, bg in (("Dark taskbar", DARK), ("Light taskbar", LIGHT)):
        d.text((x0, y), f"{theme} ({bg})", fill="#111111", font=font(17, True))
        x = x0 + label_w + sum(widths) + gap * len(SIZES)
        for z, w in zip(zooms, zw):
            d.text((x, y + 4), f"{z} px, 8x", fill="#444444", font=font(13))
            x += w + gap
        y += 40
        for c in cands:
            name, _, label, tile, edge, _, lines = c
            mid = y + (row_h - 16) // 2
            d.text((x0, mid - 44), name, fill="#111111", font=font(16, True))
            d.text((x0, mid - 20), label, fill="#444444", font=font(13))
            rim = f"edge {contrast(edge, bg):.2f}:1, " if edge else ""
            d.text((x0, mid), f"{rim}tile {contrast(tile, bg):.2f}:1 on {bg}", fill="#444444", font=font(13))
            d.text((x0, mid + 20), "lines " + " / ".join(f"{contrast(l, tile):.2f}" for l in lines) + " on tile",
                   fill="#444444", font=font(13))
            x = x0 + label_w
            for s, w in zip(SIZES, widths):
                st = strip(icons[name], neighbours, s, bg)
                framed(sheet, st, (x, y + (row_h - 16 - st.height) // 2))
                x += w + gap
            for z, w in zip(zooms, zw):
                framed(sheet, zoom(icons[name], bg, z), (x, y + (row_h - 16 - w) // 2))
                x += w + gap
            y += row_h
    d.text((x0, H - 26), "Neighbours: the apps pinned on this PC, as the shell renders them at each size. "
           f"Dark is {DARK}, sampled from this PC's taskbar; light is {LIGHT}.", fill="#777777", font=font(13))
    return sheet


def save(im, path):
    # Flat colours: an adaptive palette keeps the sheets small without visible loss.
    im.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE).save(path, optimize=True)
    print(f"wrote {path.relative_to(BRANDING.parent)} ({path.stat().st_size // 1024} kB)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--neighbours", required=True, type=Path, help="folder written by taskbar-icons.ps1")
    ap.add_argument("--work", required=True, type=Path, help="scratch folder for the rasterised icons")
    args = ap.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)

    apps = (*NEIGHBOURS_LEFT, *NEIGHBOURS_RIGHT)
    neighbours = {a: {s: Image.open(args.neighbours / f"{a}-{s}.png").convert("RGBA") for s in SIZES} for a in apps}
    icons = {c[0]: rasterise(c[1], args.work, c[0]) for c in CANDIDATES}

    for c in CANDIDATES:
        save(candidate_sheet(c, icons[c[0]], neighbours), EXPLORATIONS / f"taskbar-sheet-{c[0]}.png")
    save(matrix_sheet(CANDIDATES, icons, neighbours,
                      "Icon v2 candidates in the taskbar: 24, 32 and 48 px, and 16 px at 8x"),
         EXPLORATIONS / "taskbar-sheet-all.png")
    final = [c for c in CANDIDATES if c[0] in FINAL]
    save(matrix_sheet(final, icons, neighbours,
                      "Icon v2 short list: taskbar at 24, 32 and 48 px; 16 px (100%) and 20 px (125%) tray sizes at 8x",
                      zooms=(16, 20)),
         EXPLORATIONS / "taskbar-sheet-final.png")

    print()
    print(f"| Candidate | Tile on {DARK} | Tile on {LIGHT} | Edge on {DARK} / {LIGHT} | Line 1 / 2 / 3 on tile | Dog-ear on tile |")
    print("| --- | --- | --- | --- | --- | --- |")
    for name, _, _, tile, edge, ear, lines in CANDIDATES:
        e = f"{contrast(edge, DARK):.2f} / {contrast(edge, LIGHT):.2f}" if edge else ""
        ls = " / ".join(f"{contrast(l, tile):.2f}" for l in lines)
        print(f"| {name} | {contrast(tile, DARK):.2f} | {contrast(tile, LIGHT):.2f} | {e} | {ls} | {contrast(ear, tile):.2f} |")


if __name__ == "__main__":
    main()
