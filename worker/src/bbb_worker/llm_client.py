"""Language model backends of the extract and reduce stages and a JSON-constrained client.

Three backends, all spoken to through an OpenAI-compatible `chat/completions` endpoint:

- `managed`: a `llama-server` started from a GGUF path (`llama.running`), always terminated.
- `external`: a server the user runs (`base_url` and `model`).
- `cloud`: the same shape plus a key from the environment variable `BBB_LLM_API_KEY`, sent as a
  bearer token. It runs only when `cloud_confirmed` is true; otherwise it is refused before
  any network use. There is no fallback to the cloud.

Neither keys nor prompts are ever printed or logged.
"""

from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import jsonschema

from . import llama

MAX_RETRIES = 2
REQUEST_TIMEOUT_S = 600.0
KEY_VARIABLE = "BBB_LLM_API_KEY"
_LOOPBACK = ("localhost", "127.0.0.1", "::1")
_PREVIOUS_ANSWER_CHARS = 4000


class LlmError(RuntimeError):
    """The backend cannot be used (configuration, refusal, unreachable, rejected request)."""


class InvalidOutputError(LlmError):
    """The model did not produce valid output within the allowed attempts."""

    def __init__(self, message: str, attempts: int, usage: dict[str, int], seconds: float):
        super().__init__(message)
        self.attempts = attempts
        self.usage = usage
        self.seconds = seconds


class _Transient(Exception):
    """A failure worth another attempt (timeout, server error)."""


@dataclass(frozen=True)
class LlmConfig:
    kind: str
    model_path: str | None = None
    ctx: int = 16384
    base_url: str | None = None
    model: str | None = None
    cloud_confirmed: bool = False

    def ensure_allowed(self) -> None:
        """Refuse a cloud backend that the user did not confirm for this meeting."""
        if self.kind == "cloud" and not self.cloud_confirmed:
            raise LlmError("cloud backend refused: the cloud use of this meeting was not confirmed")

    @property
    def endpoint_host(self) -> str:
        return urlparse(self.base_url or "").hostname or ""


def config_from(options: dict[str, Any]) -> LlmConfig:
    raw = options.get("llm")
    if not raw:
        raise LlmError("options.llm is missing from the manifest")
    return LlmConfig(
        kind=raw["kind"],
        model_path=raw.get("model_path"),
        ctx=int(raw.get("ctx", 16384)),
        base_url=raw.get("base_url"),
        model=raw.get("model"),
        cloud_confirmed=bool(raw.get("cloud_confirmed", False)),
    )


@dataclass(frozen=True)
class Backend:
    """An open chat endpoint. `base_url` ends where `/chat/completions` starts."""

    base_url: str
    model: str | None = None
    api_key: str | None = field(default=None, repr=False)
    kind: str = "external"

    def post(self, payload: dict[str, Any], timeout: float = REQUEST_TIMEOUT_S) -> dict[str, Any]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        body = {**payload, **({"model": self.model} if self.model else {})}
        req = urllib.request.Request(
            f"{self.base_url.rstrip('/')}/chat/completions",
            data=json.dumps(body).encode("utf-8"),
            headers=headers,
        )
        try:
            with _opener(self.base_url).open(req, timeout=timeout) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as e:
            if e.code >= 500 or e.code == 429:
                raise _Transient(f"HTTP {e.code}") from None
            raise LlmError(f"the model server rejected the request: HTTP {e.code}") from None
        except TimeoutError:
            raise _Transient("timeout") from None
        except urllib.error.URLError as e:
            if isinstance(e.reason, TimeoutError):
                raise _Transient("timeout") from None
            raise LlmError(f"the model server is not reachable: {e.reason}") from None
        except ValueError:
            raise _Transient("the response was not JSON") from None


def _opener(url: str) -> urllib.request.OpenerDirector:
    """Local servers are never reached through a system proxy."""
    if urlparse(url).hostname in _LOOPBACK:
        return urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return urllib.request.build_opener()


@contextmanager
def open_backend(cfg: LlmConfig) -> Iterator[Backend]:
    cfg.ensure_allowed()
    if cfg.kind == "managed":
        if not cfg.model_path:
            raise LlmError("options.llm.model_path is required for the managed backend")
        with llama.running(Path(cfg.model_path), ctx=cfg.ctx) as base:
            yield Backend(f"{base}/v1", None, None, "managed")
        return
    if not cfg.base_url:
        raise LlmError("options.llm.base_url is required for the external and cloud backends")
    key = None
    if cfg.kind == "cloud":
        key = os.environ.get(KEY_VARIABLE) or None
        if not key:
            raise LlmError(
                f"cloud backend needs the key in the environment variable {KEY_VARIABLE}"
            )
    yield Backend(cfg.base_url, cfg.model, key, cfg.kind)


@dataclass(frozen=True)
class Completion:
    data: Any
    attempts: int
    usage: dict[str, int]
    seconds: float


def response_format(name: str, schema: dict[str, Any]) -> dict[str, Any]:
    clean = {k: v for k, v in schema.items() if k not in ("$schema", "$id")}
    return {"type": "json_schema", "json_schema": {"name": name, "strict": True, "schema": clean}}


def _first_error(validator: jsonschema.Draft202012Validator, data: Any) -> str | None:
    errors = sorted(validator.iter_errors(data), key=lambda e: list(e.path))
    if not errors:
        return None
    where = "/".join(str(p) for p in errors[0].path) or "<root>"
    return f"{where}: {errors[0].message}"


def _parse(resp: dict[str, Any]) -> tuple[Any, str]:
    text = (resp["choices"][0]["message"]["content"] or "").strip()
    if text.startswith("```"):
        text = text.strip("`").removeprefix("json").strip()
    if not text:
        raise ValueError("empty answer")
    return json.loads(text), text


def complete_json(
    backend: Backend,
    *,
    system: str,
    user: str,
    schema: dict[str, Any],
    name: str,
    max_tokens: int,
    max_retries: int = MAX_RETRIES,
    timeout: float = REQUEST_TIMEOUT_S,
) -> Completion:
    """Ask for JSON that validates against `schema`; on invalid output retry with the error.

    Raises `InvalidOutputError` after `max_retries` retries and `LlmError` for a failure that
    another attempt cannot fix (unreachable server, rejected request).
    """
    validator = jsonschema.Draft202012Validator(schema)
    messages: list[dict[str, str]] = [
        {"role": "system", "content": system},
        {"role": "user", "content": user},
    ]
    usage = {"prompt_tokens": 0, "completion_tokens": 0}
    started = time.monotonic()
    last_error = "no attempt was made"
    for attempt in range(1, max_retries + 2):
        payload: dict[str, Any] = {
            "messages": messages,
            "temperature": 0,
            "max_tokens": max_tokens,
            "response_format": response_format(name, schema),
        }
        if backend.kind != "cloud":
            payload["chat_template_kwargs"] = {"enable_thinking": False}
        try:
            resp = backend.post(payload, timeout)
        except _Transient as e:
            last_error = str(e)
            continue
        for key in usage:
            usage[key] += int(resp.get("usage", {}).get(key, 0) or 0)
        try:
            data, text = _parse(resp)
            problem = _first_error(validator, data)
        except (KeyError, IndexError, ValueError, TypeError) as e:
            data, text, problem = None, "", f"not valid JSON ({type(e).__name__})"
        if problem is None:
            return Completion(data, attempt, usage, round(time.monotonic() - started, 3))
        last_error = problem
        messages = [
            *messages[:2],
            {"role": "assistant", "content": text[:_PREVIOUS_ANSWER_CHARS]},
            {
                "role": "user",
                "content": f"Your answer is invalid: {problem}. "
                "Answer again with JSON that follows the schema exactly.",
            },
        ]
    raise InvalidOutputError(
        last_error, max_retries + 1, usage, round(time.monotonic() - started, 3)
    )
