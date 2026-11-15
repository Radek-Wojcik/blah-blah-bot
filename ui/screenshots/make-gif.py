"""Assemble PNG frames into an optimized GIF: make-gif.py OUT.gif FRAME_MS FRAME.png..."""

import sys

from PIL import Image


def main() -> int:
    out, ms, frames = sys.argv[1], int(sys.argv[2]), sys.argv[3:]
    if not frames:
        print("no frames given", file=sys.stderr)
        return 2
    images = [Image.open(f).convert("RGB") for f in frames]
    palette = [im.quantize(colors=96, method=Image.Quantize.MEDIANCUT) for im in images]
    palette[0].save(
        out,
        save_all=True,
        append_images=palette[1:],
        duration=ms,
        loop=0,
        optimize=True,
        disposal=2,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
