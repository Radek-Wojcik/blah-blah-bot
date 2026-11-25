"""Language model backends of the extract and reduce stages and a JSON-constrained client.

Three backends, all spoken to through an OpenAI-compatible `chat/completions` endpoint:

- `managed`: a `llama-server` started from a GGUF path (`llama.running`), always terminated.
- `external`: a server the user runs (`base_url` and `model`). A key from the environment
  variable `BBB_LLM_API_KEY` is sent as a bearer token when present (vLLM `--api-key`, a proxy).
- `cloud`: the same shape; the key is required. It runs only when `cloud_confirmed` is true;
  otherwise it is refused before any network use. There is no fallback to the cloud. The
  address must be `https://` (plain http only for loopback).

The key is never sent to the managed backend, never embedded in a URL, and never forwarded
on a redirect: a redirect is refused.

Neither keys nor prompts are ever printed or logged.
"""

from __future__ import annotations

import json
import os
import random
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import jsonschema

from . import llama

MAX_RETRIES = 2
DEFAULT_CTX = 16384
REQUEST_TIMEOUT_S = 600.0
KEY_VARIABLE = "BBB_LLM_API_KEY"
PARALLEL_VARIABLE = "BBB_LLAMA_PARALLEL"
KV_VARIABLE = "BBB_LLAMA_KV"
FLASH_VARIABLE = "BBB_LLAMA_FA"
MAX_SLOTS = 16
_LOOPBACK = ("localhost", "127.0.0.1", "::1")
_PREVIOUS_ANSWER_CHARS = 4000
# Pauses before the retries of a transient failure (timeout, 429, 5xx), in seconds, each
# stretched or shrunk by up to JITTER. `Retry-After` replaces the pause, capped at MAX_RETRY_AFTER_S.
BACKOFF_S = (2.0, 5.0, 10.0)
JITTER = 0.2
MAX_RETRY_AFTER_S = 30.0


def _sleep(seconds: float) -> None:
    """The pause between retries; tests replace it so that they do not wait."""
    time.sleep(seconds)


class LlmError(RuntimeError):
    """The backend cannot be used (configuration, refusal, unreachable, rejected request)."""


class InvalidOutputError(LlmError):
    """The model did not produce valid output within the allowed attempts."""

    def __init__(self, message: str, attempts: int, usage: dict[str, int], seconds: float):
        super().__init__(message)
        self.attempts = attempts
        self.usage = usage
        self.seconds = seconds


class TruncatedOutputError(InvalidOutputError):
    """The answer hit the token limit. Asking again with the same prompt gives the same
    result, so the caller must ask for less (a smaller window) instead of retrying."""


class _Transient(Exception):
    """A failure worth another attempt (timeout, rate limit, server error)."""

    def __init__(self, message: str, retry_after: float | None = None):
        super().__init__(message)
        self.retry_after = retry_after


def _retry_after(headers: Any) -> float | None:
    """Seconds from a numeric `Retry-After` header, None when absent or not a number."""
    try:
        value = float(headers.get("Retry-After"))
    except (AttributeError, TypeError, ValueError):
        return None
    return value if value >= 0 else None


def backoff_delay(retries_so_far: int, retry_after: float | None) -> float:
    """Pause before the next attempt of a transient failure."""
    if retry_after is not None:
        return min(retry_after, MAX_RETRY_AFTER_S)
    base = BACKOFF_S[min(retries_so_far, len(BACKOFF_S) - 1)]
    return base * (1 + random.uniform(-JITTER, JITTER))


@dataclass(frozen=True)
class LlmConfig:
    kind: str
    model_path: str | None = None
    ctx: int | None = None  # None: not configured (the managed server then uses DEFAULT_CTX)
    base_url: str | None = None
    model: str | None = None
    cloud_confirmed: bool = False
    parallel: int | None = None  # requests in flight; None: not configured (1)

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
        ctx=int(raw["ctx"]) if raw.get("ctx") else None,
        base_url=raw.get("base_url"),
        model=raw.get("model"),
        cloud_confirmed=bool(raw.get("cloud_confirmed", False)),
        parallel=_clamp_slots(raw.get("parallel")),
    )


def _clamp_slots(value: Any) -> int | None:
    """A number of slots from 1 to MAX_SLOTS; None for a missing or unusable value."""
    try:
        n = int(value)
    except (TypeError, ValueError):
        return None
    return max(1, min(MAX_SLOTS, n)) if n >= 1 else None


def slots_for(cfg: LlmConfig) -> int:
    """How many requests the stage may have in flight. For the managed backend
    `BBB_LLAMA_PARALLEL` overrides the configured `parallel`; the other backends use only the
    configuration (the server behind them decides what it can serve). Default 1."""
    if cfg.kind == "managed":
        from_env = _clamp_slots(os.environ.get(PARALLEL_VARIABLE))
        if from_env is not None:
            return from_env
    return cfg.parallel or 1


@dataclass(frozen=True)
class Backend:
    """An open chat endpoint. `base_url` ends where `/chat/completions` starts."""

    base_url: str
    model: str | None = None
    api_key: str | None = field(default=None, repr=False)
    kind: str = "external"
    ctx: int | None = None  # context size in tokens of one request when known
    slots: int = 1  # requests that may be in flight at once

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
                raise _Transient(f"HTTP {e.code}", _retry_after(e.headers)) from None
            raise LlmError(f"the model server rejected the request: HTTP {e.code}") from None
        except TimeoutError:
            raise _Transient("timeout") from None
        except urllib.error.URLError as e:
            if isinstance(e.reason, TimeoutError):
                raise _Transient("timeout") from None
            raise LlmError(f"the model server is not reachable: {e.reason}") from None
        except ValueError:
            raise _Transient("the response was not JSON") from None


class _RefuseRedirects(urllib.request.HTTPRedirectHandler):
    """urllib would copy the Authorization header to the new address (any host, any scheme).
    A model endpoint has no reason to redirect, so a redirect is an error."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        raise LlmError(f"the model server answered with a redirect (HTTP {code}), which is refused")


def _opener(url: str) -> urllib.request.OpenerDirector:
    """Local servers are never reached through a system proxy. Redirects are refused."""
    if urlparse(url).hostname in _LOOPBACK:
        return urllib.request.build_opener(urllib.request.ProxyHandler({}), _RefuseRedirects())
    return urllib.request.build_opener(_RefuseRedirects())


def check_base_url(kind: str, url: str) -> None:
    """Refuse an address that would expose the transcript or the key: embedded credentials,
    a scheme other than http(s), and for the cloud anything but https (loopback excepted)."""
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise LlmError("options.llm.base_url must be an http(s) address with a host")
    if parsed.username is not None or parsed.password is not None or "@" in parsed.netloc:
        raise LlmError("options.llm.base_url must not contain credentials")
    if kind == "cloud" and parsed.scheme != "https" and parsed.hostname not in _LOOPBACK:
        raise LlmError("the cloud backend needs an https:// address")


@contextmanager
def open_backend(cfg: LlmConfig, *, parallel: int = 1) -> Iterator[Backend]:
    """Open the backend of `cfg`. `parallel` is the number of requests the caller will send
    at once (`slots_for(cfg)` for a stage that can do so); the managed server gets that many
    slots, each with the full `ctx`. Callers that work sequentially keep the default of 1."""
    cfg.ensure_allowed()
    if cfg.kind == "managed":
        if not cfg.model_path:
            raise LlmError("options.llm.model_path is required for the managed backend")
        ctx = cfg.ctx or DEFAULT_CTX
        slots = max(1, parallel)
        flash = os.environ.get(FLASH_VARIABLE) or ("on" if slots > 1 else None)
        if flash is not None and flash not in llama.FLASH_ATTN_VALUES:
            raise LlmError(f"{FLASH_VARIABLE} must be one of {llama.FLASH_ATTN_VALUES}")
        with llama.running(
            Path(cfg.model_path),
            ctx=ctx,
            parallel=slots,
            kv_cache_type=os.environ.get(KV_VARIABLE) or None,
            flash_attn=flash,
        ) as base:
            yield Backend(f"{base}/v1", None, None, "managed", ctx, slots)
        return
    if not cfg.base_url:
        raise LlmError("options.llm.base_url is required for the external and cloud backends")
    check_base_url(cfg.kind, cfg.base_url)
    key = os.environ.get(KEY_VARIABLE) or None
    if cfg.kind == "cloud" and not key:
        raise LlmError(f"cloud backend needs the key in the environment variable {KEY_VARIABLE}")
    yield Backend(cfg.base_url, cfg.model, key, cfg.kind, cfg.ctx, max(1, parallel))


@dataclass(frozen=True)
class Completion:
    data: Any
    attempts: int
    usage: dict[str, int]
    seconds: float


def response_format(name: str, schema: dict[str, Any]) -> dict[str, Any]:
    clean = {k: v for k, v in schema.items() if k not in ("$schema", "$id")}
    return {"type": "json_schema", "json_schema": {"name": name, "strict": True, "schema": clean}}


_PROBLEM_CHARS = 200


def _first_error(validator: jsonschema.Draft202012Validator, data: Any) -> str | None:
    errors = sorted(validator.iter_errors(data), key=lambda e: list(e.path))
    if not errors:
        return None
    first = errors[0]
    where = "/".join(str(p) for p in first.path) or "<root>"
    # jsonschema quotes the offending value ("'<text>' is too long"). That value is model
    # output and can contain meeting content, and this text ends up in error messages and
    # logs, so it names the rule only.
    message = first.message.replace(repr(first.instance), "<value>")
    return f"{where}: {message[:_PROBLEM_CHARS]}"


def _parse(resp: dict[str, Any]) -> tuple[Any, str]:
    text = (resp["choices"][0]["message"]["content"] or "").strip()
    if text.startswith("```"):
        text = text.strip("`").removeprefix("json").strip()
    if not text:
        raise ValueError("empty answer")
    return json.loads(text), text


def _finish_reason(resp: dict[str, Any]) -> str | None:
    try:
        return resp["choices"][0].get("finish_reason")
    except (KeyError, IndexError, AttributeError, TypeError):
        return None


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
    sleep: Callable[[float], None] | None = None,
) -> Completion:
    """Ask for JSON that validates against `schema`; on invalid output retry with the error.

    A transient failure (timeout, 429, 5xx) is retried separately, after a pause that grows
    (`BACKOFF_S`, with jitter) or follows `Retry-After`. `sleep` replaces the pause in tests.

    Raises `InvalidOutputError` after `max_retries` retries of invalid output (or the retries
    of transient failures) and `LlmError` for a failure that another attempt cannot fix
    (unreachable server, rejected request).
    """
    pause = sleep or _sleep
    validator = jsonschema.Draft202012Validator(schema)
    messages: list[dict[str, str]] = [
        {"role": "system", "content": system},
        {"role": "user", "content": user},
    ]
    usage = {"prompt_tokens": 0, "completion_tokens": 0}
    started = time.monotonic()
    last_error = "no attempt was made"
    attempt = 0
    invalid_retries = 0
    transient_retries = 0

    def failure() -> InvalidOutputError:
        return InvalidOutputError(last_error, attempt, usage, round(time.monotonic() - started, 3))

    while True:
        attempt += 1
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
            if transient_retries >= len(BACKOFF_S):
                raise failure() from None
            pause(backoff_delay(transient_retries, e.retry_after))
            transient_retries += 1
            continue
        for key in usage:
            usage[key] += int(resp.get("usage", {}).get(key, 0) or 0)
        if _finish_reason(resp) == "length":
            raise TruncatedOutputError(
                "the answer was cut off at the token limit",
                attempt,
                usage,
                round(time.monotonic() - started, 3),
            )
        try:
            data, text = _parse(resp)
            problem = _first_error(validator, data)
        except (KeyError, IndexError, ValueError, TypeError) as e:
            data, text, problem = None, "", f"not valid JSON ({type(e).__name__})"
        if problem is None:
            return Completion(data, attempt, usage, round(time.monotonic() - started, 3))
        last_error = problem
        if invalid_retries >= max_retries:
            raise failure()
        invalid_retries += 1
        messages = [
            *messages[:2],
            {"role": "assistant", "content": text[:_PREVIOUS_ANSWER_CHARS]},
            {
                "role": "user",
                "content": f"Your answer is invalid: {problem}. "
                "Answer again with JSON that follows the schema exactly.",
            },
        ]
