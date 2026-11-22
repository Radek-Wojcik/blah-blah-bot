"""Fail when the version differs between the files that carry it.

Usage: python scripts/check_versions.py [--tag vX.Y.Z]

Checks the workspace version in Cargo.toml, the `app` package in Cargo.lock, the version of
crates/app/tauri.conf.json, and ui/package.json with the root entry of ui/package-lock.json.
With --tag the common version must also equal the tag without its leading "v".
The release build uses this check so that an installer is never built from files that
disagree about what is being released.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"^\d+\.\d+\.\d+$")


def cargo_workspace_version(text: str) -> str | None:
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    if not section:
        return None
    found = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M)
    return found.group(1) if found else None


def cargo_lock_version(text: str, package: str) -> str | None:
    found = re.search(rf'^name = "{re.escape(package)}"\r?\nversion = "([^"]+)"', text, re.M)
    return found.group(1) if found else None


def collect(root: Path) -> dict[str, str | None]:
    def read(rel: str) -> str:
        return (root / rel).read_text(encoding="utf-8")

    lock = json.loads(read("ui/package-lock.json"))
    return {
        "Cargo.toml [workspace.package]": cargo_workspace_version(read("Cargo.toml")),
        "Cargo.lock (app)": cargo_lock_version(read("Cargo.lock"), "app"),
        "crates/app/tauri.conf.json": json.loads(read("crates/app/tauri.conf.json")).get("version"),
        "ui/package.json": json.loads(read("ui/package.json")).get("version"),
        "ui/package-lock.json": lock.get("packages", {}).get("", {}).get("version"),
    }


def check(versions: dict[str, str | None], tag: str | None = None) -> list[str]:
    problems = []
    for name, version in versions.items():
        if not version:
            problems.append(f"{name}: no version found")
        elif not SEMVER.match(version):
            problems.append(f"{name}: '{version}' is not MAJOR.MINOR.PATCH (the installer needs it)")
    if len({v for v in versions.values() if v}) > 1:
        problems.append("versions differ: " + ", ".join(f"{n} = {v}" for n, v in versions.items()))
    if tag is not None:
        expected = tag[1:] if tag.startswith("v") else tag
        for name, version in versions.items():
            if version and version != expected:
                problems.append(f"{name}: {version} does not match tag {tag}")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", help="release tag the version must match, for example v1.0.0")
    args = parser.parse_args()
    versions = collect(ROOT)
    problems = check(versions, args.tag)
    for problem in problems:
        print(problem)
    if not problems:
        print(f"version {next(iter(versions.values()))} is consistent in {len(versions)} places")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
