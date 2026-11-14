"""Fail when tracked files are too large or binary outside the allowed folders."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

MAX_BYTES = 1_000_000
BINARY_ALLOWED = ("docs/img/",)
BINARY_SUFFIXES = {
    ".wav", ".flac", ".mp3", ".ogg", ".m4a", ".mp4", ".mkv",
    ".gguf", ".onnx", ".safetensors", ".bin", ".pt", ".ckpt",
    ".zip", ".7z", ".exe", ".dll",
}  # fmt: skip


def main() -> int:
    files = subprocess.run(
        ["git", "ls-files", "-z"], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout.split("\0")
    failed = False
    for name in filter(None, files):
        path = Path(name)
        if not path.is_file():
            continue
        if path.stat().st_size > MAX_BYTES and not name.startswith(BINARY_ALLOWED):
            print(f"too large (> {MAX_BYTES} bytes): {name}")
            failed = True
        if path.suffix.lower() in BINARY_SUFFIXES and not name.startswith(BINARY_ALLOWED):
            print(f"binary file not allowed here: {name}")
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
