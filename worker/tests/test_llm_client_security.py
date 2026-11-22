"""Safety and resilience of the model client: redirects, addresses, keys, backoff, and a
stage that fails when no window could be extracted. Fake servers on loopback only."""

import json
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from bbb_worker import llm_client
from bbb_worker.steps import extract, read_json

from .llm_support import (
    FakeLlm,
    by_kind,
    context_doc,
    extraction_for,
    llm_options,
    long_transcript,
    make_meeting,
    segment,
    transcript_doc,
)
from .test_cli_llm_steps import run_cli

SCHEMA = {"type": "object", "required": ["summary"]}
OK = {"summary": "fine"}


@contextmanager
def redirecting_server(location: str):
    """Answers every POST with a 302 to `location` and records what it received."""
    seen: list[dict[str, str]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            self.rfile.read(int(self.headers["Content-Length"]))
            seen.append({k.lower(): v for k, v in self.headers.items()})
            self.send_response(302)
            self.send_header("Location", location)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *args) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01), daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}/v1", seen
    finally:
        server.shutdown()
        server.server_close()


def ask(backend, sleep=lambda _s: None, **kw):
    return llm_client.complete_json(
        backend, system="s", user="u", schema=SCHEMA, name="x", max_tokens=100, sleep=sleep, **kw
    )


# Redirects and addresses ------------------------------------------------------------------


def test_a_redirect_is_refused_and_the_key_never_reaches_the_target():
    with FakeLlm(lambda body, n: OK) as target:
        with redirecting_server(target.url.removesuffix("/v1") + "/stolen") as (url, seen):
            backend = llm_client.Backend(url, "m", "secret-key", "cloud")
            with pytest.raises(llm_client.LlmError, match="redirect"):
                ask(backend)
        assert target.requests == [] and target.headers == []
    assert len(seen) == 1  # asked once, the redirect was not followed


@pytest.mark.parametrize(
    "url",
    [
        "https://user@api.example.com/v1",
        "https://user:pass@api.example.com/v1",
        "https://ghp_token@api.example.com/v1",
        "http://:pw@127.0.0.1:8080/v1",
    ],
)
@pytest.mark.parametrize("kind", ["cloud", "external"])
def test_urls_with_embedded_credentials_are_rejected(kind, url, monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "k")
    cfg = llm_client.LlmConfig(kind, base_url=url, model="m", cloud_confirmed=True)
    with pytest.raises(llm_client.LlmError, match="credentials"), llm_client.open_backend(cfg):
        pass


def test_cloud_needs_https_unless_the_host_is_loopback(monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "k")

    def open_cloud(url):
        cfg = llm_client.LlmConfig("cloud", base_url=url, model="m", cloud_confirmed=True)
        with llm_client.open_backend(cfg) as backend:
            return backend

    with pytest.raises(llm_client.LlmError, match="https"):
        open_cloud("http://api.example.com/v1")
    assert open_cloud("https://api.example.com/v1").api_key == "k"
    assert open_cloud("http://127.0.0.1:9/v1").api_key == "k"
    assert open_cloud("http://localhost:9/v1").api_key == "k"
    assert open_cloud("http://[::1]:9/v1").api_key == "k"


def test_external_may_use_plain_http(monkeypatch):
    monkeypatch.delenv("BBB_LLM_API_KEY", raising=False)
    cfg = llm_client.LlmConfig("external", base_url="http://192.168.1.20:8000/v1", model="m")
    with llm_client.open_backend(cfg) as backend:
        assert backend.api_key is None


def test_a_non_http_scheme_is_rejected():
    for url in ("file:///etc/passwd", "ftp://host/v1", "not a url"):
        with pytest.raises(llm_client.LlmError, match="http"):
            llm_client.check_base_url("external", url)


# Keys -------------------------------------------------------------------------------------


def test_external_sends_the_key_from_the_environment_as_bearer(tmp_path, monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "vllm-key")
    with FakeLlm(extraction_for([])) as fake:
        doc = transcript_doc([segment(0, 0, 5_000, "Some talk about the mount.")])
        m = make_meeting(tmp_path, doc, context_doc(), llm_options(fake.url, "external"))
        extract.run(m)
        assert fake.headers[0]["authorization"] == "Bearer vllm-key"
    assert "vllm-key" not in json.dumps(read_json(m.work_dir / "extract" / "windows.json"))


def test_managed_never_gets_the_key(monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "must-not-leak")

    @contextmanager
    def fake_running(model, *, ctx=8192, **kwargs):
        yield "http://127.0.0.1:1"

    monkeypatch.setattr(llm_client.llama, "running", fake_running)
    cfg = llm_client.LlmConfig("managed", model_path="m.gguf")
    with llm_client.open_backend(cfg) as backend:
        assert backend.api_key is None


def test_the_key_is_not_in_the_backend_repr():
    backend = llm_client.Backend("https://h/v1", "m", "secret-key", "cloud")
    assert "secret-key" not in repr(backend)


# Backoff ----------------------------------------------------------------------------------


def test_transient_errors_back_off_with_jitter_and_then_succeed():
    pauses: list[float] = []

    def handler(body, n):
        return (503, "busy") if n <= 2 else OK

    with FakeLlm(handler) as fake:
        done = ask(llm_client.Backend(fake.url, "m"), sleep=pauses.append)
        assert len(fake.requests) == 3
    assert done.data == OK and done.attempts == 3
    assert len(pauses) == 2
    assert 2 * 0.8 <= pauses[0] <= 2 * 1.2 and 5 * 0.8 <= pauses[1] <= 5 * 1.2


def test_retry_after_is_honoured_and_capped():
    pauses: list[float] = []

    def handler(body, n):
        if n == 1:
            return (429, "slow down", {"Retry-After": "7"})
        if n == 2:
            return (503, "busy", {"Retry-After": "600"})
        return OK

    with FakeLlm(handler) as fake:
        ask(llm_client.Backend(fake.url, "m"), sleep=pauses.append)
    assert pauses == [7.0, llm_client.MAX_RETRY_AFTER_S]


def test_retry_after_that_is_not_a_number_falls_back_to_the_backoff():
    assert llm_client._retry_after({"Retry-After": "Wed, 21 Oct 2026 07:28:00 GMT"}) is None
    assert llm_client._retry_after({}) is None
    assert llm_client._retry_after({"Retry-After": "-3"}) is None
    assert 1.6 <= llm_client.backoff_delay(0, None) <= 2.4


def test_persistent_transient_errors_give_up_after_the_backoff_schedule():
    pauses: list[float] = []
    with FakeLlm(lambda body, n: (500, "down")) as fake:
        with pytest.raises(llm_client.InvalidOutputError, match="HTTP 500") as raised:
            ask(llm_client.Backend(fake.url, "m"), sleep=pauses.append)
        assert len(fake.requests) == 1 + len(llm_client.BACKOFF_S)
    assert len(pauses) == len(llm_client.BACKOFF_S)
    assert raised.value.attempts == 4


def test_a_rejected_request_is_not_retried_and_does_not_sleep():
    pauses: list[float] = []
    with FakeLlm(lambda body, n: (401, "no")) as fake:
        with pytest.raises(llm_client.LlmError, match="401"):
            ask(llm_client.Backend(fake.url, "m"), sleep=pauses.append)
        assert len(fake.requests) == 1
    assert pauses == []


def test_invalid_output_is_retried_without_a_pause():
    pauses: list[float] = []
    with FakeLlm(lambda body, n: "not json" if n == 1 else OK) as fake:
        done = ask(llm_client.Backend(fake.url, "m"), sleep=pauses.append)
    assert done.attempts == 2 and pauses == []


# A stage where every window failed --------------------------------------------------------


def one_window_meeting(tmp_path, url):
    doc = transcript_doc([segment(0, 0, 5_000, "Alex, can you have the mount ready?")])
    return make_meeting(tmp_path, doc, context_doc(), llm_options(url))


def test_extract_fails_when_every_window_is_skipped_after_writing_windows_json(
    tmp_path, monkeypatch
):
    pauses: list[float] = []
    monkeypatch.setattr(llm_client, "_sleep", pauses.append)
    with FakeLlm(lambda body, n: (429, "rate limited")) as fake:
        m = one_window_meeting(tmp_path, fake.url)
        with pytest.raises(llm_client.LlmError, match="no usable answer"):
            extract.run(m)
    doc = read_json(m.work_dir / "extract" / "windows.json")
    assert [w["skipped"] for w in doc["windows"]] == [True]
    assert len(pauses) == len(llm_client.BACKOFF_S)  # it did back off before giving up


def test_extract_cli_exits_with_an_error_when_every_window_is_skipped(tmp_path):
    with FakeLlm(by_kind(extraction=lambda body, n: {"summary": "no items key"})) as fake:
        m = one_window_meeting(tmp_path, fake.url)
        code, events, _err = run_cli("extract", tmp_path / "manifest.json", m.meeting_dir)
    assert code == 1
    assert events[-1]["event"] == "error" and events[-1]["step"] == "extract"
    assert "no usable answer" in events[-1]["message"]
    assert (m.work_dir / "extract" / "windows.json").exists()
    assert not any(e["event"] == "done" for e in events)


def test_a_partly_skipped_extract_is_still_a_success(tmp_path):
    special = {0: "The mount is ready, thanks."}

    def handler(body, n):
        if "The mount is ready, thanks." in body["messages"][1]["content"]:
            return {"summary": "no items key"}
        return extraction_for([])(body, n)

    with FakeLlm(handler) as fake:
        m = make_meeting(
            tmp_path, long_transcript(35, special), context_doc(), llm_options(fake.url)
        )
        extract.run(m)
    flags = [w["skipped"] for w in read_json(m.work_dir / "extract" / "windows.json")["windows"]]
    assert flags[0] is True and not all(flags)


def test_a_schema_problem_names_the_rule_and_not_the_value():
    import jsonschema

    validator = jsonschema.Draft202012Validator(
        {"type": "object", "properties": {"note": {"type": "string", "maxLength": 5}}}
    )
    secret = "the confidential budget figures"
    problem = llm_client._first_error(validator, {"note": secret})
    assert problem is not None and problem.startswith("note: ")
    assert "confidential" not in problem
    assert "too long" in problem
    assert llm_client._first_error(validator, {"note": "ok"}) is None
