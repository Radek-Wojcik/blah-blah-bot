"""Verify docs/TRACEABILITY.md: every reference exists, all 52 rows are present, counts match.

A reference in backticks has the form `path` or `path::name`. The prefix `stage5:` reads the
file from the Stage 5 branch (set BBB_STAGE5_REF to change it); when that branch is not
available those references are skipped with a warning.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

DOC = Path("docs/TRACEABILITY.md")
EXPECTED_ROWS = 52
STATUSES = [
    "Covered",
    "Covered (manual)",
    "Covered, gap noted",
    "Pending merge",
    "Reduced for 1.0 (pending merge)",
    "Gap",
]
REF = re.compile(r"^(stage5:)?([\w./@-]+\.(?:rs|py|ts|tsx|json|md|yml|toml|ps1))(?:::(.+))?$")
STAGE5_REF = os.environ.get("BBB_STAGE5_REF", "origin/stage/5-publish")

_cache: dict[tuple[bool, str], str | None] = {}


def read(path: str, branch: bool) -> str | None:
    key = (branch, path)
    if key not in _cache:
        if branch:
            proc = subprocess.run(
                ["git", "show", f"{STAGE5_REF}:{path}"], capture_output=True, encoding="utf-8"
            )
            _cache[key] = proc.stdout if proc.returncode == 0 else None
        else:
            p = Path(path)
            _cache[key] = p.read_text(encoding="utf-8") if p.is_file() else None
    return _cache[key]


def has_test(text: str, path: str, name: str) -> bool:
    if path.endswith(".rs"):
        return re.search(rf"\bfn\s+{re.escape(name)}\b", text) is not None
    if path.endswith(".py"):
        return re.search(rf"\bdef\s+(test_)?{re.escape(name)}\b", text) is not None
    return name in text


def branch_available() -> bool:
    proc = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", STAGE5_REF], capture_output=True
    )
    return proc.returncode == 0


def main() -> int:
    text = DOC.read_text(encoding="utf-8")
    errors: list[str] = []
    warnings: list[str] = []
    have_branch = branch_available()
    rows: dict[int, str] = {}
    checked = 0
    skipped = 0

    for line in text.splitlines():
        m = re.match(r"^\|\s*(\d+)\s*\|", line)
        if not m:
            continue
        number = int(m.group(1))
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if number in rows:
            errors.append(f"row {number} appears twice")
        if len(cells) != 6:
            errors.append(f"row {number}: expected 6 columns, found {len(cells)}")
            continue
        status = cells[4]
        rows[number] = status
        if status not in STATUSES:
            errors.append(f"row {number}: unknown status '{status}'")
        verified = 0
        for span in re.findall(r"`([^`]+)`", cells[3]):
            ref = REF.match(span)
            if not ref:
                continue
            branch, path, name = bool(ref.group(1)), ref.group(2), ref.group(3)
            if branch and not have_branch:
                skipped += 1
                continue
            body = read(path, branch)
            where = f"{'stage5:' if branch else ''}{path}"
            if body is None:
                errors.append(f"row {number}: file not found: {where}")
            elif name and not has_test(body, path, name):
                errors.append(f"row {number}: not found in {where}: {name}")
            else:
                verified += 1
                checked += 1
        if status.startswith("Covered") and verified == 0:
            errors.append(f"row {number}: a covered row needs at least one verified reference")

    missing = sorted(set(range(1, EXPECTED_ROWS + 1)) - set(rows))
    if missing:
        errors.append(f"missing rows: {missing}")
    extra = sorted(set(rows) - set(range(1, EXPECTED_ROWS + 1)))
    if extra:
        errors.append(f"unexpected rows: {extra}")

    counts = {s: sum(1 for v in rows.values() if v == s) for s in STATUSES}
    summary = {
        m.group(1): int(m.group(2))
        for m in re.finditer(r"^\|\s*([^|*]+?)\s*\|\s*(\d+)\s*\|\s*$", text, re.M)
        if m.group(1) in STATUSES
    }
    for status in STATUSES:
        if summary.get(status) != counts[status]:
            errors.append(
                f"summary says {summary.get(status)} for '{status}', the table has {counts[status]}"
            )

    if re.search(r"[A-Za-z]:\\|/Users/|/home/", text):
        errors.append("the document contains an absolute path")
    if skipped:
        warnings.append(f"{skipped} stage5: references skipped ({STAGE5_REF} not available)")

    for w in warnings:
        print(f"warning: {w}")
    for e in errors:
        print(f"error: {e}")
    print(f"{len(rows)} rows, {checked} references verified, counts: {counts}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
