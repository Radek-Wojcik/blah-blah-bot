"""Shared harness code for prototype runners: environment capture and result files."""

from __future__ import annotations

import json
import platform
import subprocess
import sys
import time
from collections.abc import Callable
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

RESULTS_DIR = Path(__file__).resolve().parents[2] / "results"


def gpu_info() -> dict[str, Any]:
    """Name, total and used memory and driver version from nvidia-smi, if present."""
    try:
        out = subprocess.run(
            [
                "nvidia-smi",
                "--query-gpu=name,memory.total,memory.used,driver_version",
                "--format=csv,noheader,nounits",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return {}
    name, total, used, driver = (p.strip() for p in out.splitlines()[0].split(","))
    return {
        "name": name,
        "memory_total_mib": int(total),
        "memory_used_mib": int(used),
        "driver": driver,
    }


def gpu_memory_used_mib() -> int | None:
    info = gpu_info()
    return info.get("memory_used_mib")


def environment() -> dict[str, Any]:
    """Machine description recorded in every result file (no user names or paths)."""
    return {
        "os": platform.platform(),
        "os_build": platform.version(),
        "python": sys.version.split()[0],
        "machine": platform.machine(),
        "gpu": gpu_info(),
    }


@contextmanager
def timer():
    """Context manager yielding a callable that returns elapsed seconds."""
    start = time.perf_counter()
    yield lambda: time.perf_counter() - start


def write_result(
    prototype: str, name: str, payload: dict[str, Any], out_dir: Path | None = None
) -> Path:
    """Write `benchmarks/results/<prototype>-<name>.json` with environment metadata."""
    out_dir = out_dir or RESULTS_DIR
    out_dir.mkdir(parents=True, exist_ok=True)
    doc = {
        "prototype": prototype,
        "name": name,
        "timestamp": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "environment": environment(),
        **payload,
    }
    path = out_dir / f"{prototype}-{name}.json"
    path.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return path


def run_prototype(prototype: str, name: str, fn: Callable[[], dict[str, Any]]) -> Path:
    """Run `fn`, store its metrics (or the failure) as a result file and return the path."""
    try:
        payload = {"status": "ok", **fn()}
    except Exception as exc:  # recorded so that a failed prototype still leaves evidence
        payload = {"status": "error", "error": f"{type(exc).__name__}: {exc}"}
    path = write_result(prototype, name, payload)
    print(f"{prototype}: {payload['status']} -> {path.name}")
    return path
