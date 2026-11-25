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


def llama_env() -> dict[str, str]:
    """Environment for `llama-server`, without Hugging Face credentials."""
    env = dict(os.environ)
    env.pop("HF_TOKEN", None)
    env.pop("HUGGING_FACE_HUB_TOKEN", None)
    return env


def server_binary() -> str:
    """`$BBB_LLAMA_SERVER` (set by the application from its settings) or PATH."""
    return os.environ.get("BBB_LLAMA_SERVER", "llama-server")


def not_found_message(binary: str) -> str:
    """What the user sees when `llama-server` cannot be started: where it was looked for and
    what to do about it."""
    where = f"at '{binary}'" if os.path.dirname(binary) else f"on the PATH (looked for '{binary}')"
    return (
        f"llama-server was not found {where}. "
        "Set its location in Settings > Processing or run the setup again."
    )


def _get(url: str, timeout: float = 2.0) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return resp.read()


FLASH_ATTN_VALUES = ("on", "off", "auto")


def parallel_args(slots: int) -> list[str]:
    """`--parallel N` for N > 1 slots; nothing for one (the server default is kept)."""
    return ["--parallel", str(slots)] if slots > 1 else []


def cache_args(kv_cache_type: str | None, flash_attn: str | None) -> list[str]:
    """Flags for flash attention and the KV cache type. A quantized V cache needs flash
    attention, so a non-f16 type without an explicit setting switches it on."""
    if flash_attn is not None and flash_attn not in FLASH_ATTN_VALUES:
        raise ValueError(f"flash attention must be one of {FLASH_ATTN_VALUES}, not {flash_attn!r}")
    kv = kv_cache_type if kv_cache_type and kv_cache_type != "f16" else None
    if kv is not None and flash_attn is None:
        flash_attn = "on"
    args: list[str] = []
    if flash_attn is not None:
        args += ["-fa", flash_attn]
    if kv_cache_type:
        args += ["--cache-type-k", kv_cache_type, "--cache-type-v", kv_cache_type]
    return args


@contextmanager
def running(
    model: Path,
    *,
    mmproj: Path | None = None,
    ctx: int = 8192,
    parallel: int = 1,
    kv_cache_type: str | None = None,
    flash_attn: str | None = None,
    port: int = 0,
    extra_args: Sequence[str] = (),
    startup_timeout_s: float = 300.0,
) -> Iterator[str]:
    """Run llama-server with all layers on the GPU and yield its base URL.

    The process is always terminated on exit, so no model stays loaded between stages.
    `port=0` picks a free port.

    `ctx` is the context of one request. With `parallel` > 1 the server gets that many slots
    and a total context of `ctx * parallel`, so every slot keeps `ctx`. `parallel` = 1 passes
    neither `-np` nor a changed `-c`: the command line is the one used before slots existed.
    `kv_cache_type` ("q8_0", "f16", ...) sets both KV cache types and needs flash attention;
    `flash_attn` is "on", "off" or "auto". Both are left to the server when None.
    """
    if port == 0:
        import socket

        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
    binary = server_binary()
    slots = max(1, int(parallel))
    cmd = [
        binary,
        "-m",
        str(model),
        "-c",
        str(ctx * slots),
        "-ngl",
        "99",
        "--port",
        str(port),
        "--host",
        "127.0.0.1",
        *parallel_args(slots),
        *cache_args(kv_cache_type, flash_attn),
        *extra_args,
    ]
    if mmproj:
        cmd += ["--mmproj", str(mmproj)]
    try:
        proc = subprocess.Popen(
            cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=llama_env()
        )
    except FileNotFoundError as e:
        raise RuntimeError(not_found_message(binary)) from e
    except OSError as e:
        raise RuntimeError(
            f"llama-server at '{binary}' could not be started ({e}). "
            "Check the file in Settings > Processing or run the setup again."
        ) from e
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
