"""Start and stop a local llama-server process for the LLM and vision prototypes."""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.request
from collections.abc import Sequence
from contextlib import contextmanager
from pathlib import Path


def server_binary() -> str:
    """Path to llama-server: $BBB_LLAMA_SERVER or `llama-server` on PATH."""
    return os.environ.get("BBB_LLAMA_SERVER", "llama-server")


def _get(url: str, timeout: float = 2.0) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return resp.read()


@contextmanager
def running(
    model: Path,
    *,
    mmproj: Path | None = None,
    ctx: int = 16384,
    port: int = 8089,
    extra_args: Sequence[str] = (),
    startup_timeout_s: float = 300.0,
):
    """Context manager that runs llama-server with all layers on the GPU."""
    cmd = [
        server_binary(),
        "-m",
        str(model),
        "-c",
        str(ctx),
        "-ngl",
        "99",
        "--port",
        str(port),
        "--host",
        "127.0.0.1",
        *extra_args,
    ]
    if mmproj:
        cmd += ["--mmproj", str(mmproj)]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + startup_timeout_s
        while time.time() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"llama-server exited early with code {proc.returncode}")
            try:
                if json.loads(_get(f"{base}/health")).get("status") == "ok":
                    break
            except Exception:
                time.sleep(1.0)
        else:
            raise TimeoutError("llama-server did not become ready")
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            proc.kill()


def chat(base: str, payload: dict, timeout: float = 600.0) -> dict:
    """POST to /v1/chat/completions and return the parsed JSON response."""
    req = urllib.request.Request(
        f"{base}/v1/chat/completions",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read())
