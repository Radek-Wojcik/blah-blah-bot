"""Validate commit messages in a git revision range against Conventional Commits.

Usage: python scripts/check_commits.py <revision-range>
"""

from __future__ import annotations

import re
import subprocess
import sys

TYPES = "feat|fix|docs|test|refactor|perf|build|ci|chore|revert"
SUBJECT = re.compile(rf"^({TYPES})(\([a-z0-9._/-]+\))?!?: \S.{{0,99}}$")
TRAILER = re.compile(r"^[A-Za-z][A-Za-z-]*: ")
ALLOWED_TRAILERS = ("Signed-off-by", "BREAKING CHANGE", "Closes", "Refs")


def messages(rev_range: str) -> list[tuple[str, str]]:
    out = subprocess.run(
        ["git", "log", "--format=%H%x1f%B%x1e", rev_range],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    items = []
    for chunk in out.split("\x1e"):
        chunk = chunk.strip("\n")
        if chunk:
            sha, _, body = chunk.partition("\x1f")
            items.append((sha.strip(), body.strip()))
    return items


def check(body: str) -> list[str]:
    lines = body.splitlines()
    problems = []
    if not lines or not SUBJECT.match(lines[0]):
        problems.append("subject must match 'type(scope): description' (max 100 chars)")
    if len(lines) > 1 and lines[1].strip():
        problems.append("second line must be blank")
    for line in lines[2:]:
        if TRAILER.match(line) and not line.startswith(ALLOWED_TRAILERS):
            problems.append(f"trailer not allowed: {line.split(':')[0]}")
    return problems


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    failed = False
    for sha, body in messages(sys.argv[1]):
        for problem in check(body):
            failed = True
            print(f"{sha[:8]}: {problem}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
