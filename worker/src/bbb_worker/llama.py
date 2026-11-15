"""A managed `llama-server` process and a small client for its OpenAI-compatible API."""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.request
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path


def server_binary() -> str:
    """`$BBB_LLAMA_SERVER` (set by the application from its settings) or PATH."""
    return os.environ.get("BBB_LLAMA_SERVER", "llama-server")


def _get(url: str, timeout: float = 2.0) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return resp.read()


@contextmanager
def running(
    model: Path,
    *,
    mmproj: Path | None = None,
    ctx: int = 8192,
    port: int = 0,
    extra_args: Sequence[str] = (),
    startup_timeout_s: float = 300.0,
) -> Iterator[str]:
    """Run llama-server with all layers on the GPU and yield its base URL.

    The process is always terminated on exit, so no model stays loaded between stages.
    `port=0` picks a free port.
    """
    if port == 0:
        import socket

        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
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
        while True:
            if proc.poll() is not None:
                raise RuntimeError(f"llama-server exited early with code {proc.returncode}")
            try:
                if json.loads(_get(f"{base}/health")).get("status") == "ok":
                    break
            except (OSError, ValueError):
                if time.time() > deadline:
                    raise TimeoutError("llama-server did not become ready") from None
                time.sleep(1.0)
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            proc.kill()


def chat(base: str, payload: dict, timeout: float = 600.0) -> dict:
    req = urllib.request.Request(
        f"{base}/v1/chat/completions",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read())
