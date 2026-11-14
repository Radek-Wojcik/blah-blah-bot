"""P8: create the Python environment from the lock file with an empty cache.

    uv run python -m bbb_bench.proto.p8_env

Copies pyproject.toml and uv.lock into a temporary directory and runs
`uv sync --frozen --no-install-project` with an empty uv cache, once per group
set. Records wall time, the size of the environment and of the downloaded cache,
and whether PyTorch sees the GPU. This approximates a first run on a machine
that has never used the project (it does not replace a test on a clean machine).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from .. import harness

ROOT = Path(__file__).resolve().parents[3]
GROUP_SETS = {
    "asr": ["asr"],
    "diarization": ["diar"],
    "asr+diarization": ["asr", "diar"],
}


def dir_size_mb(path: Path) -> float:
    total = sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
    return round(total / 2**20, 1)


def build(groups: list[str]) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="bbb-p8-") as tmp:
        work = Path(tmp)
        for name in ("pyproject.toml", "uv.lock"):
            shutil.copy(ROOT / name, work / name)
        (work / "src" / "bbb_bench").mkdir(parents=True)
        (work / "src" / "bbb_bench" / "__init__.py").write_text("", encoding="utf-8")
        env = {
            **os.environ,
            "UV_CACHE_DIR": str(work / "cache"),
            "UV_PROJECT_ENVIRONMENT": str(work / "venv"),
        }
        cmd = ["uv", "sync", "--frozen", "--no-install-project"]
        for g in groups:
            cmd += ["--group", g]
        with harness.timer() as elapsed:
            proc = subprocess.run(cmd, cwd=work, env=env, capture_output=True, text=True)
        seconds = elapsed()
        result: dict[str, Any] = {
            "groups": groups,
            "ok": proc.returncode == 0,
            "seconds": round(seconds, 1),
            "stderr_tail": proc.stderr[-400:] if proc.returncode else "",
        }
        if proc.returncode == 0:
            python = work / "venv" / "Scripts" / "python.exe"
            result["environment_mb"] = dir_size_mb(work / "venv")
            result["download_cache_mb"] = dir_size_mb(work / "cache")
            if "diar" in groups:
                probe = subprocess.run(
                    [str(python), "-c", "import torch; print(torch.cuda.is_available())"],
                    capture_output=True,
                    text=True,
                )
                result["gpu_detected"] = probe.stdout.strip() == "True"
        return result


def run() -> dict[str, Any]:
    return {"runs": {name: build(groups) for name, groups in GROUP_SETS.items()}}


def main() -> None:
    harness.run_prototype("p8-env", "uv-lock-empty-cache", run)


if __name__ == "__main__":
    main()
