"""Builds the Windows icon, apps/desktop/src-tauri/icons/icon.ico, from branding/icon.svg.

Frames: 16, 20, 24, 32, 48, 64, 128 and 256 px, each rendered with resvg at its own size.
Where branding/icon-sizes/icon-<size>.svg exists (16, 20 and 24 px), that pixel-snapped
drawing is used instead of the 512 master, whose 32-unit grid only lands on whole pixels at
16, 32, 48, 64 and up. Every frame is stored as PNG, as `tauri icon` does. Run it after
`bunx tauri icon ../../branding/icon.svg -o src-tauri/icons`, which writes the other files.

    uv run --with pillow python -I branding/tools/build_ico.py
"""

import struct
import subprocess
import sys
import tempfile
from io import BytesIO
from pathlib import Path

from PIL import Image

BRANDING = Path(__file__).resolve().parent.parent
ROOT = BRANDING.parent
MASTER = BRANDING / "icon.svg"
SNAPPED = BRANDING / "icon-sizes"
OUT = ROOT / "apps" / "desktop" / "src-tauri" / "icons" / "icon.ico"
SIZES = (16, 20, 24, 32, 48, 64, 128, 256)


def render(size, tmp):
    svg = SNAPPED / f"icon-{size}.svg"
    if not svg.exists():
        svg = MASTER
    png = Path(tmp) / f"icon-{size}.png"
    subprocess.run(["resvg", "-w", str(size), "-h", str(size), str(svg), str(png)], check=True)
    return svg, png.read_bytes()


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
    assert sizes == sorted(SIZES), sizes
    for size, png in frames:
        got = ico.ico.getimage((size, size)).convert("RGBA")
        want = Image.open(BytesIO(png)).convert("RGBA")
        assert got.size == (size, size) and got.tobytes() == want.tobytes(), size


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else OUT
    with tempfile.TemporaryDirectory() as tmp:
        frames = []
        for size in SIZES:
            svg, png = render(size, tmp)
            frames.append((size, png))
            print(f"{size:>3} px  {svg.relative_to(ROOT).as_posix()}")
    write_ico(frames, out)
    verify(out, frames)
    print(f"wrote {out.relative_to(ROOT).as_posix()} ({out.stat().st_size} bytes, {len(frames)} frames, verified)")


if __name__ == "__main__":
    main()
