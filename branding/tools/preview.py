"""Draws branding/preview.png: the mark (logo.svg) and the app icon (icon.svg) side by side at
16 to 256 px on a light and a dark field, the mono and tray marks at 16 to 48 px, and the
16 px renderings at 8x. The icon's small sizes come from branding/icon-sizes/ where a
pixel-snapped drawing exists, as in icon.ico.

    uv run --with pillow python -I branding/tools/preview.py
"""

import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

BRANDING = Path(__file__).resolve().parent.parent
FIELDS = (("#ffffff", "#1b1f27", "#64748b"), ("#0f1115", "#f3f4f6", "#8b95a7"))  # field, ink, label
BIG = (16, 24, 32, 48, 128, 256)
SMALL = (16, 24, 32, 48)
W, PAD, GAP = 1400, 40, 24


def font(size, bold=False):
    name = "segoeuib.ttf" if bold else "segoeui.ttf"
    try:
        return ImageFont.truetype(f"C:/Windows/Fonts/{name}", size)
    except OSError:
        return ImageFont.load_default()


def render(svg, size, tmp, colour=None):
    """resvg at one size; currentColor marks are rendered in `colour`."""
    if colour:
        src = Path(tmp) / f"{svg.stem}-{colour[1:]}.svg"
        src.write_text(svg.read_text(encoding="utf-8").replace("currentColor", colour), encoding="utf-8")
        svg = src
    png = Path(tmp) / f"{svg.stem}-{size}.png"
    subprocess.run(["resvg", "-w", str(size), "-h", str(size), str(svg), str(png)], check=True)
    return Image.open(png).convert("RGBA")


def icon_svg(size):
    snapped = BRANDING / "icon-sizes" / f"icon-{size}.svg"
    return snapped if snapped.exists() else BRANDING / "icon.svg"


def row(canvas, x, base, images, label_colour):
    """Images bottom-aligned on `base`, each labelled with its size underneath."""
    d = ImageDraw.Draw(canvas)
    for size, im in images:
        canvas.paste(im, (x, base - size), im)
        d.text((x, base + 10), str(size), fill=label_colour, font=font(16))
        x += size + GAP
    return x


def zoom(im, field):
    cell = Image.new("RGB", im.size, field)
    cell.paste(im, (0, 0), im)
    return cell.resize((im.width * 8, im.height * 8), Image.NEAREST)


def panel(field, ink, label, tmp):
    h = 620
    im = Image.new("RGB", (W, h), field)
    d = ImageDraw.Draw(im)
    half = (W - 2 * PAD) // 2
    d.text((PAD, 28), f"logo.svg, the mark, on {field}", fill=ink, font=font(20, True))
    d.text((PAD + half, 28), f"icon.svg, the app icon, on {field}", fill=ink, font=font(20, True))
    base = 80 + 256
    row(im, PAD, base, [(s, render(BRANDING / "logo.svg", s, tmp)) for s in BIG], label)
    row(im, PAD + half, base, [(s, render(icon_svg(s), s, tmp)) for s in BIG], label)

    y = base + 70
    mono = "#1b1f27" if field == "#ffffff" else "#ffffff"
    d.text((PAD, y), "logo-mono.svg", fill=ink, font=font(20, True))
    d.text((PAD + 300, y), "tray-recording.svg", fill=ink, font=font(20, True))
    d.text((PAD + 600, y), "16 px, 8x: mark, icon, mono, tray", fill=ink, font=font(20, True))
    base2 = y + 50 + 48
    row(im, PAD, base2, [(s, render(BRANDING / "logo-mono.svg", s, tmp, mono)) for s in SMALL], label)
    row(im, PAD + 300, base2, [(s, render(BRANDING / "tray-recording.svg", s, tmp, mono)) for s in SMALL], label)
    x = PAD + 600
    for svg, colour in ((BRANDING / "logo.svg", None), (icon_svg(16), None),
                        (BRANDING / "logo-mono.svg", mono), (BRANDING / "tray-recording.svg", mono)):
        im.paste(zoom(render(svg, 16, tmp, colour), field), (x, y + 44))
        x += 128 + 16
    return im


def main():
    with tempfile.TemporaryDirectory() as tmp:
        panels = [panel(*f, tmp) for f in FIELDS]
    sheet = Image.new("RGB", (W, sum(p.height for p in panels)))
    y = 0
    for p in panels:
        sheet.paste(p, (0, y))
        y += p.height
    out = BRANDING / "preview.png"
    sheet.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE).save(out, optimize=True)
    print(f"wrote {out.name} ({out.stat().st_size // 1024} kB)")


if __name__ == "__main__":
    main()
