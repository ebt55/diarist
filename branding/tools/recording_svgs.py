"""Writes the recording-indicator sources in branding/recording/, one SVG per pixel size:

- overlay-<size>.svg at 16, 20, 24, 32, 40 and 48 px: the taskbar overlay, a red disc in a white
  ring, centred. The shell draws it over the lower right of the taskbar button at the small-icon
  size (16 px at 100 percent, 32 at 200). Every diameter has the canvas's parity, so the extreme
  edges of both circles land on whole pixels.
- <light|dark>-<size>.svg at 16, 20, 24, 32 and 48 px: the app icon pair while recording, for the
  notification area: the frame of icon.svg (light) or icon-dark.svg (dark) that build_ico.py renders
  at that size (the pixel-snapped drawing in icon-sizes/ where there is one, else the 512 master),
  with its lower-right corner cut away and a red disc in the cut. The disc is half the icon wide,
  touches the right and bottom edges, and the clear gap around it (no ring) keeps it apart from
  the tile and the slate edge on either taskbar.

The red is the brand's recording dot, #dc2626. Run it again only when the app icon changes, then
build_ico.py renders the PNGs:

    uv run --with pillow python -I branding/tools/recording_svgs.py
"""

import re
from pathlib import Path

BRANDING = Path(__file__).resolve().parent.parent
OUT = BRANDING / "recording"
MASTERS = {"dark": BRANDING / "icon-dark.svg", "light": BRANDING / "icon.svg"}
RED, WHITE = "#dc2626", "#ffffff"

# Overlay: (ring, disc) diameters in px.
OVERLAY = {16: (14, 10), 20: (18, 14), 24: (22, 16), 32: (28, 22), 40: (36, 28), 48: (42, 32)}
# Notification area: (disc diameter, clear gap) in px.
TRAY = {16: (8, 1), 20: (10, 1.5), 24: (12, 1.5), 32: (16, 2), 48: (24, 3)}


def num(v):
    return f"{v:g}"


def doc(size, title, comment, body):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}">\n'
        f"  <title>{title}</title>\n  <!-- {comment} -->\n{body}\n</svg>\n"
    )


def overlay(size):
    ring, disc = OVERLAY[size]
    c = num(size / 2)
    body = (
        f'  <circle cx="{c}" cy="{c}" r="{num(ring / 2)}" fill="{WHITE}"/>\n'
        f'  <circle cx="{c}" cy="{c}" r="{num(disc / 2)}" fill="{RED}"/>'
    )
    comment = (
        f"Taskbar overlay at {size} px: a {disc} px red disc in a {num((ring - disc) / 2)} px white ring "
        f"({ring} px across), centred. Written by tools/recording_svgs.py."
    )
    return doc(size, f"Diarist recording overlay ({size} px)", comment, body)


def icon_source(variant, size):
    snapped = BRANDING / "icon-sizes" / f"{variant}-{size}.svg"
    return snapped if snapped.exists() else MASTERS[variant]


def inline(src, size):
    """The source drawing as a nested <svg> of the frame's size (title and comments dropped)."""
    text = src.read_text(encoding="utf-8")
    view_box = re.search(r'viewBox="([^"]+)"', text).group(1)
    inner = re.search(r"<svg[^>]*>(.*)</svg>", text, re.S).group(1)
    inner = re.sub(r"<title>.*?</title>|<!--.*?-->", "", inner, flags=re.S)
    lines = "\n".join("      " + l.strip() for l in inner.splitlines() if l.strip())
    return f'    <svg x="0" y="0" width="{size}" height="{size}" viewBox="{view_box}">\n{lines}\n    </svg>'


def tray(variant, size):
    disc, gap = TRAY[size]
    c = num(size - disc / 2)
    src = icon_source(variant, size)
    body = (
        f'  <mask id="cut" maskUnits="userSpaceOnUse" x="0" y="0" width="{size}" height="{size}">\n'
        f'    <rect width="{size}" height="{size}" fill="#fff"/>\n'
        f'    <circle cx="{c}" cy="{c}" r="{num(disc / 2 + gap)}" fill="#000"/>\n'
        f"  </mask>\n"
        f'  <g mask="url(#cut)">\n{inline(src, size)}\n  </g>\n'
        f'  <circle cx="{c}" cy="{c}" r="{num(disc / 2)}" fill="{RED}"/>'
    )
    comment = (
        f"{src.relative_to(BRANDING).as_posix()} at {size} px with the lower-right corner cut away "
        f"({num(gap)} px clear) and a {disc} px red disc touching the right and bottom edges. "
        f"Written by tools/recording_svgs.py."
    )
    background = "light" if variant == "light" else "dark"
    return doc(size, f"Diarist recording, for {background} backgrounds ({size} px)", comment, body)


def main():
    OUT.mkdir(exist_ok=True)
    for size in OVERLAY:
        (OUT / f"overlay-{size}.svg").write_text(overlay(size), encoding="utf-8", newline="\n")
    for variant in ("light", "dark"):
        for size in TRAY:
            (OUT / f"{variant}-{size}.svg").write_text(tray(variant, size), encoding="utf-8", newline="\n")
    print(f"wrote {len(OVERLAY) + 2 * len(TRAY)} files in {OUT.relative_to(BRANDING.parent).as_posix()}/")


if __name__ == "__main__":
    main()
