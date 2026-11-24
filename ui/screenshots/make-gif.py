"""Assemble PNG frames into a smooth, compact GIF: make-gif.py OUT.gif DURATIONS FRAME.png...

DURATIONS is either a single millisecond value applied to every frame (e.g. 110)
or a comma-separated list of per-frame millisecond delays matching FRAME count.
Frames wider than MAX_WIDTH (960px) are downscaled proportionally with Lanczos,
quantized with Pillow FASTOCTREE, and written with loop=0, disposal=2, optimize=True.
"""

import sys

from PIL import Image

MAX_WIDTH = 960
PALETTE_COLORS = 192


def parse_durations(raw: str, count: int) -> list[int]:
    if "," not in raw:
        return [int(raw)] * count
    values = [int(part) for part in raw.split(",")]
    if len(values) != count:
        raise ValueError(f"expected {count} durations, got {len(values)}")
    return values


def prepare_frame(path: str) -> Image.Image:
    with Image.open(path) as im:
        rgba = im.convert("RGBA")
    if rgba.width > MAX_WIDTH:
        height = max(1, round(rgba.height * MAX_WIDTH / rgba.width))
        rgba = rgba.resize((MAX_WIDTH, height), Image.Resampling.LANCZOS)
    return rgba.quantize(
        colors=PALETTE_COLORS,
        method=Image.Quantize.FASTOCTREE,
        dither=Image.Dither.NONE,
    )


def main() -> int:
    if len(sys.argv) < 4:
        print("usage: make-gif.py OUT.gif DURATIONS FRAME.png...", file=sys.stderr)
        return 2
    out, raw_durations, frames = sys.argv[1], sys.argv[2], sys.argv[3:]
    if not frames:
        print("no frames given", file=sys.stderr)
        return 2
    durations = parse_durations(raw_durations, len(frames))
    quantized = [prepare_frame(f) for f in frames]
    quantized[0].save(
        out,
        save_all=True,
        append_images=quantized[1:],
        duration=durations,
        loop=0,
        optimize=True,
        disposal=2,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
