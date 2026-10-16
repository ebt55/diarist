"""Builds the Windows icon files in apps/desktop/src-tauri/icons/ from the app icon pair.

- icon.ico, the exe and installer icon, from branding/icon-dark.svg (the paper tile, which
  reads on dark and light taskbars alike): 16, 20, 24, 32, 48, 64, 128 and 256 px frames,
  stored as PNG, as `tauri icon` does.
- theme/dark/<size>.png and theme/light/<size>.png at 16, 20, 24, 32 and 48 px, from
  icon-dark.svg and icon.svg, for the running app to follow the taskbar theme.

Each image is rendered with resvg at its own size. Where branding/icon-sizes/<variant>-<size>.svg
exists (16, 20 and 24 px, and 48 px for icon.svg), that pixel-snapped drawing is used
instead of the 512 master, whose 32-unit grid falls on half pixels at 20 and 24 px. Run it after
`bunx tauri icon ../../branding/icon-dark.svg -o src-tauri/icons`, which writes the other files.

    uv run --with pillow python -I branding/tools/build_ico.py
"""

import struct
import subprocess
import tempfile
from io import BytesIO
from pathlib import Path

from PIL import Image

BRANDING = Path(__file__).resolve().parent.parent
ROOT = BRANDING.parent
ICONS = ROOT / "apps" / "desktop" / "src-tauri" / "icons"
MASTERS = {"dark": BRANDING / "icon-dark.svg", "light": BRANDING / "icon.svg"}
ICO_SIZES = (16, 20, 24, 32, 48, 64, 128, 256)
THEME_SIZES = (16, 20, 24, 32, 48)


def source(variant, size):
    snapped = BRANDING / "icon-sizes" / f"{variant}-{size}.svg"
    return snapped if snapped.exists() else MASTERS[variant]


def render(svg, size, out):
    subprocess.run(["resvg", "-w", str(size), "-h", str(size), str(svg), str(out)], check=True)
    return out.read_bytes()


def write_ico(frames, path):
    """ICONDIR, one ICONDIRENTRY per frame (0 means 256), then the PNG blobs."""
    head = struct.pack("<HHH", 0, 1, len(frames))
    offset = len(head) + 16 * len(frames)
    entries, blobs = b"", b""
    for size, png in frames:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    path.write_bytes(head + entries + blobs)


def verify(path, frames):
    """Read the file back with Pillow's ICO reader: every frame present, pixel for pixel."""
    ico = Image.open(path)
    sizes = sorted(s[0] for s in ico.info["sizes"])
    assert sizes == sorted(ICO_SIZES), sizes
    for size, png in frames:
        got = ico.ico.getimage((size, size)).convert("RGBA")
        want = Image.open(BytesIO(png)).convert("RGBA")
        assert got.size == (size, size) and got.tobytes() == want.tobytes(), size


def rel(p):
    return p.relative_to(ROOT).as_posix()


def main():
    with tempfile.TemporaryDirectory() as tmp:
        frames = []
        for size in ICO_SIZES:
            svg = source("dark", size)
            frames.append((size, render(svg, size, Path(tmp) / f"ico-{size}.png")))
            print(f"icon.ico {size:>3} px  {rel(svg)}")
    out = ICONS / "icon.ico"
    write_ico(frames, out)
    verify(out, frames)
    print(f"wrote {rel(out)} ({out.stat().st_size} bytes, {len(frames)} frames, verified)")

    for variant in MASTERS:
        folder = ICONS / "theme" / variant
        folder.mkdir(parents=True, exist_ok=True)
        for size in THEME_SIZES:
            svg = source(variant, size)
            render(svg, size, folder / f"{size}.png")
            print(f"theme/{variant}/{size}.png  {rel(svg)}")


if __name__ == "__main__":
    main()
