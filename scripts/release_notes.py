"""Print the CHANGELOG.md section of one released version.

Usage: python scripts/release_notes.py <version> [--changelog PATH]

The version may carry a leading "v". Exits with 1 when the section is missing or has no text,
so a release is never created with empty notes.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def extract(changelog: str, version: str) -> str:
    version = version[1:] if version.startswith("v") else version
    heading = re.compile(rf"^## \[{re.escape(version)}\](?: - .*)?[ \t]*$", re.M)
    start = heading.search(changelog)
    if not start:
        raise ValueError(f"CHANGELOG.md has no section for version {version}")
    rest = changelog[start.end() :]
    nxt = re.search(r"^## \[", rest, re.M)
    body = (rest[: nxt.start()] if nxt else rest).strip()
    # Reference links at the end of the file are not part of the last section.
    body = re.sub(r"\n+(\[[^\]]+\]: \S+\s*)+\Z", "", body).strip()
    if not re.search(r"\w", body):
        raise ValueError(f"the CHANGELOG.md section for version {version} is empty")
    return body + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version")
    parser.add_argument("--changelog", type=Path, default=ROOT / "CHANGELOG.md")
    args = parser.parse_args()
    try:
        text = extract(args.changelog.read_text(encoding="utf-8"), args.version)
    except (OSError, ValueError) as err:
        print(err, file=sys.stderr)
        return 1
    sys.stdout.buffer.write(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
