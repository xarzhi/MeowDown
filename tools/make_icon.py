"""Turn logo.png into the app icon set.

The source is a 1536px illustration with the cat centred and a "元宝 AI生成"
watermark in the bottom-right corner. We crop the artwork out (dropping the
watermark), normalise the background to white and round the corners, then write:

    assets/icon.png    100x100 RGBA, the requested icon
    assets/app.ico     16..256, embedded into the exe as resource #1

Small shells (16/20/24/32 px) get a close-up of the head: the full picture —
cat, pencil and open book — collapses into mush below ~40px.

Run:  python tools/make_icon.py
"""

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "logo.png"
MASTER = 512         # working resolution before downscaling
RADIUS_RATIO = 0.225  # corners: 22.5% of the edge, ie 22px on a 100px icon
ICON_PX = 100

# Square crops in source pixels: (centre x, centre y, edge).
# The artwork spans x 352..1183, y 333..1201 — the head alone is
# x 352..1183 by y 333..840 — and the watermark starts at x 1374, so the
# 1000px crop stops well short of it.
FULL = (768, 768, 1000)
HEAD = (768, 586, 740)

ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
HEAD_MAX = 32         # at or below this, use the close-up


def rounded(img: Image.Image, radius_ratio: float = RADIUS_RATIO) -> Image.Image:
    """Apply a rounded-rect alpha mask with 4x supersampled edges."""
    size = img.size[0]
    r = int(round(size * radius_ratio))
    scale = 4
    mask = Image.new("L", (size * scale, size * scale), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, size * scale - 1, size * scale - 1), radius=r * scale, fill=255
    )
    mask = mask.resize((size, size), Image.LANCZOS)
    out = img.convert("RGBA")
    out.putalpha(mask)
    return out


def build(src: Image.Image, crop) -> Image.Image:
    """Crop the given square, flatten it onto white and round the corners."""
    cx, cy, edge = crop
    box = (cx - edge // 2, cy - edge // 2, cx + edge // 2, cy + edge // 2)
    flat = Image.new("RGB", (edge, edge), (255, 255, 255))
    flat.paste(src.crop(box), (0, 0))
    return rounded(flat.resize((MASTER, MASTER), Image.LANCZOS))


def write_ico(frames: list[Image.Image], path: Path) -> None:
    """Write an ICO whose entries are PNG-compressed, one per frame.

    Pillow's own `save(sizes=.., append_images=..)` mixes the two paths up and
    labels frames with the wrong dimensions, so the container is assembled here.
    """
    payloads = []
    for frame in frames:
        buffer = io.BytesIO()
        frame.save(buffer, format="PNG", optimize=True)
        payloads.append(buffer.getvalue())

    header = struct.pack("<HHH", 0, 1, len(frames))
    offset = len(header) + 16 * len(frames)
    entries = b""
    for frame, payload in zip(frames, payloads):
        side = frame.size[0]
        # 0 means 256 in the directory entry.
        dimension = 0 if side >= 256 else side
        entries += struct.pack(
            "<BBBBHHII", dimension, dimension, 0, 0, 1, 32, len(payload), offset
        )
        offset += len(payload)

    with open(path, "wb") as handle:
        handle.write(header + entries + b"".join(payloads))


def main() -> None:
    src = Image.open(SRC).convert("RGB")
    full = build(src, FULL)
    head = build(src, HEAD)

    # 100px is the requested icon size.
    icon = full.resize((ICON_PX, ICON_PX), Image.LANCZOS)
    icon.save(ROOT / "assets" / "icon.png")

    frames = [
        (head if size <= HEAD_MAX else full).resize((size, size), Image.LANCZOS)
        for size in ICO_SIZES
    ]
    write_ico(frames, ROOT / "assets" / "app.ico")

    print("assets/icon.png", icon.size)
    print("assets/app.ico", [frame.size[0] for frame in frames])


if __name__ == "__main__":
    main()
