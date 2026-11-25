"""Windows of the extract step on several slots, and the `llama-server` command line.

No model is used: a fake OpenAI-compatible server answers, and `subprocess.Popen` is replaced
for the command-line checks.
"""

import re
import threading
import time
from contextlib import contextmanager
from pathlib import Path

import pytest

from bbb_worker import llama, llm_client, protocol
from bbb_worker.steps import extract, read_json

from .llm_support import (
    FakeLlm,
    context_doc,
    extraction_for,
    llm_options,
    long_transcript,
    make_meeting,
)

SPECIAL = {
    5: "Alex will finish the mount prototype by Friday, I promise.",
    95: "We decided to order the second batch from the new supplier.",
    185: "Sam will send the budget to the board before the next meeting.",
    275: "The warranty claim is closed and nobody needs to follow it up.",
}
RULES = [
    {"quote": quote, "text": f"Item {i}", "fields": {}} for i, quote in enumerate(SPECIAL.values())
]


def transcript():
    return long_transcript(60, SPECIAL)


def windows_doc(meeting):
    return read_json(meeting.work_dir / "extract" / "windows.json")


def first_stamp(body) -> int:
    """Seconds of the first line of the window in a request."""
    text = body["messages"][1]["content"]
    h, m, s = re.search(r"\[(\d\d):(\d\d):(\d\d)\]", text).groups()
    return int(h) * 3600 + int(m) * 60 + int(s)


class Tracker:
    """Wraps the extraction handler: counts requests in flight, delays the first window the
    most and records the order in which windows finish."""

    def __init__(self, delay_first: float = 0.0, fail=None) -> None:
        self.inner = extraction_for(RULES)
        self.delay_first = delay_first
        self.fail = fail
        self.lock = threading.Lock()
        self.active = 0
        self.peak = 0
        self.finished: list[int] = []

    def __call__(self, body, n):
        stamp = first_stamp(body)
        with self.lock:
            self.active += 1
            self.peak = max(self.peak, self.active)
        try:
            time.sleep(self.delay_first if stamp == 0 else 0.03)
            if self.fail is not None:
                answer = self.fail(stamp)
                if answer is not None:
                    return answer
            return self.inner(body, n)
        finally:
            with self.lock:
                self.active -= 1
                self.finished.append(stamp)


@pytest.fixture
def progress_log(monkeypatch):
    log: list[tuple[float, str]] = []
    monkeypatch.setattr(
        protocol, "progress", lambda step, fraction, message="": log.append((fraction, message))
    )
    return log


def run_extract(tmp_path, tracker, **llm_extra):
    with FakeLlm(tracker) as fake:
        m = make_meeting(tmp_path, transcript(), context_doc(), llm_options(fake.url, **llm_extra))
        extract.run(m)
    return m


def stable(doc):
    """The document without wall-clock fields."""
    doc = dict(doc)
    doc["windows"] = [{**w, "seconds": 0} for w in doc["windows"]]
    return doc


def test_results_keep_window_order_when_windows_finish_out_of_order(tmp_path, progress_log):
    tracker = Tracker(delay_first=0.5)
    m = run_extract(tmp_path, tracker, parallel=3)
    docs = windows_doc(m)["windows"]
    assert len(docs) >= 4
    assert [w["index"] for w in docs] == list(range(len(docs)))
    assert tracker.peak > 1
    assert (
        tracker.finished[-1] == 0 or tracker.finished.index(0) > 0
    )  # window 0 did not finish first
    assert sum(len(w["items"]) for w in docs) >= 4


def test_progress_is_monotonic_and_counts_finished_windows(tmp_path, progress_log):
    m = run_extract(tmp_path, Tracker(delay_first=0.3), parallel=3)
    total = len(windows_doc(m)["windows"])
    fractions = [f for f, _ in progress_log]
    assert fractions == sorted(fractions)
    assert fractions[-1] == 1.0
    counted = [msg for _, msg in progress_log if re.fullmatch(rf"window \d+/{total}", msg)]
    assert counted == [f"window {k}/{total}" for k in range(1, total + 1)]


def test_one_slot_is_the_sequential_run(tmp_path, progress_log):
    sequential = Tracker()
    m1 = run_extract(tmp_path / "a", sequential)
    assert sequential.peak == 1
    assert sequential.finished == sorted(sequential.finished)
    explicit = Tracker()
    m2 = run_extract(tmp_path / "b", explicit, parallel=1)
    assert explicit.peak == 1
    parallel = Tracker()
    m3 = run_extract(tmp_path / "c", parallel, parallel=4)
    assert parallel.peak > 1
    one, two, three = (stable(windows_doc(m)) for m in (m1, m2, m3))
    assert one == two
    assert one == three  # same content, only the way it was computed differs


def test_sequential_progress_lines_are_unchanged(tmp_path, progress_log):
    m = run_extract(tmp_path, Tracker())
    total = len(windows_doc(m)["windows"])
    messages = [msg for _, msg in progress_log]
    assert messages[:3] == ["preparing windows", f"window 1/{total}", f"window 1/{total}"]
    assert not any("at a time" in msg for msg in messages)


def test_a_server_error_aborts_the_step(tmp_path):
    tracker = Tracker(fail=lambda stamp: (401, "no") if stamp > 0 else None, delay_first=0.2)
    with FakeLlm(tracker) as fake:
        m = make_meeting(tmp_path, transcript(), context_doc(), llm_options(fake.url, parallel=3))
        with pytest.raises(llm_client.LlmError, match="HTTP 401"):
            extract.run(m)
    assert not (m.work_dir / "extract" / "windows.json").exists()


def test_an_invalid_window_is_skipped_and_the_others_are_kept(tmp_path):
    tracker = Tracker(fail=lambda stamp: "not json" if stamp == 0 else None)
    m = run_extract(tmp_path, tracker, parallel=3)
    first, *rest = windows_doc(m)["windows"]
    assert first["skipped"] and first["attempts"] == 3
    assert rest and not any(w["skipped"] for w in rest)


def test_every_window_invalid_still_fails_the_stage(tmp_path):
    tracker = Tracker(fail=lambda stamp: "not json")
    with FakeLlm(tracker) as fake:
        m = make_meeting(tmp_path, transcript(), context_doc(), llm_options(fake.url, parallel=2))
        with pytest.raises(llm_client.LlmError, match="no usable answer"):
            extract.run(m)


# Configuration ----------------------------------------------------------------------------


def managed(**extra):
    return llm_client.LlmConfig("managed", model_path="m.gguf", ctx=8192, **extra)


def test_slots_come_from_the_environment_or_the_configuration(monkeypatch):
    monkeypatch.delenv("BBB_LLAMA_PARALLEL", raising=False)
    assert llm_client.slots_for(managed()) == 1
    assert llm_client.slots_for(managed(parallel=3)) == 3
    monkeypatch.setenv("BBB_LLAMA_PARALLEL", "5")
    assert llm_client.slots_for(managed(parallel=3)) == 5
    monkeypatch.setenv("BBB_LLAMA_PARALLEL", "junk")
    assert llm_client.slots_for(managed(parallel=3)) == 3
    monkeypatch.setenv("BBB_LLAMA_PARALLEL", "1000")
    assert llm_client.slots_for(managed()) == llm_client.MAX_SLOTS
    external = llm_client.LlmConfig("external", base_url="http://x/v1")
    assert llm_client.slots_for(external) == 1  # the environment is for the managed server
    assert llm_client.slots_for(llm_client.LlmConfig("external", base_url="u", parallel=2)) == 2


def test_config_reads_parallel_from_the_options():
    cfg = llm_client.config_from({"llm": {"kind": "managed", "model_path": "m", "parallel": 4}})
    assert cfg.parallel == 4
    assert llm_client.config_from({"llm": {"kind": "managed", "model_path": "m"}}).parallel is None
    assert llm_client.config_from({"llm": {"kind": "managed", "parallel": 0}}).parallel is None


@pytest.fixture
def fake_running(monkeypatch):
    calls: list[dict] = []

    @contextmanager
    def fake(model, **kwargs):
        calls.append({"model": Path(model).name, **kwargs})
        yield "http://127.0.0.1:1"

    monkeypatch.setattr(llm_client.llama, "running", fake)
    for name in ("BBB_LLAMA_PARALLEL", "BBB_LLAMA_KV", "BBB_LLAMA_FA"):
        monkeypatch.delenv(name, raising=False)
    return calls


def test_open_backend_defaults_to_one_slot_and_plain_flags(fake_running):
    with llm_client.open_backend(managed()) as backend:
        assert backend.slots == 1 and backend.ctx == 8192
    (call,) = fake_running
    assert call["parallel"] == 1 and call["kv_cache_type"] is None and call["flash_attn"] is None


def test_open_backend_passes_slots_and_defaults_flash_attention_on(fake_running, monkeypatch):
    with llm_client.open_backend(managed(), parallel=4) as backend:
        assert backend.slots == 4 and backend.ctx == 8192  # ctx stays per request
    assert fake_running[0]["parallel"] == 4 and fake_running[0]["flash_attn"] == "on"
    monkeypatch.setenv("BBB_LLAMA_KV", "q8_0")
    monkeypatch.setenv("BBB_LLAMA_FA", "off")
    with llm_client.open_backend(managed(), parallel=2):
        pass
    assert fake_running[1]["kv_cache_type"] == "q8_0" and fake_running[1]["flash_attn"] == "off"


def test_an_invalid_flash_attention_setting_is_refused(fake_running, monkeypatch):
    monkeypatch.setenv("BBB_LLAMA_FA", "maybe")
    with (
        pytest.raises(llm_client.LlmError, match="BBB_LLAMA_FA"),
        llm_client.open_backend(managed()),
    ):
        pass
    assert fake_running == []


def test_extract_opens_the_managed_server_with_the_configured_slots(
    tmp_path, fake_running, monkeypatch
):
    with FakeLlm(Tracker()) as fake:

        @contextmanager
        def running(model, **kwargs):
            fake_running.append(kwargs)
            yield fake.url.removesuffix("/v1")

        monkeypatch.setattr(llm_client.llama, "running", running)
        opts = {
            "language": "auto",
            "llm": {"kind": "managed", "model_path": "m.gguf", "ctx": 8192, "parallel": 3},
        }
        m = make_meeting(tmp_path, transcript(), context_doc(), opts)
        extract.run(m)
    assert fake_running[0]["parallel"] == 3 and fake_running[0]["ctx"] == 8192
    assert len(windows_doc(m)["windows"]) >= 4


# Command line of llama-server ------------------------------------------------------------


class FakeProcess:
    def poll(self):
        return None

    def terminate(self):
        pass

    def wait(self, timeout=None):
        return 0


def command_for(monkeypatch, **kwargs) -> list[str]:
    seen: list[list[str]] = []

    def popen(cmd, **_):
        seen.append(list(cmd))
        return FakeProcess()

    monkeypatch.setattr(llama.subprocess, "Popen", popen)
    monkeypatch.setattr(llama, "_get", lambda url, timeout=2.0: b'{"status": "ok"}')
    with llama.running(Path("model.gguf"), **kwargs):
        pass
    return seen[0]


def test_default_command_line_is_the_one_from_before_slots(monkeypatch):
    cmd = command_for(monkeypatch, ctx=16384, port=9999)
    assert cmd[1:] == [
        "-m", "model.gguf", "-c", "16384", "-ngl", "99", "--port", "9999", "--host", "127.0.0.1",
    ]  # fmt: skip


def test_parallel_multiplies_the_context_and_sets_the_slots(monkeypatch):
    cmd = command_for(monkeypatch, ctx=16384, parallel=4, flash_attn="on", port=9999)
    assert cmd[cmd.index("-c") + 1] == str(16384 * 4)
    assert cmd[cmd.index("--parallel") + 1] == "4"
    assert cmd[cmd.index("-fa") + 1] == "on"
    assert "--cache-type-k" not in cmd


def test_quantized_kv_cache_switches_flash_attention_on(monkeypatch):
    cmd = command_for(monkeypatch, ctx=8192, kv_cache_type="q8_0", port=9999)
    assert cmd[cmd.index("-fa") + 1] == "on"
    assert cmd[cmd.index("--cache-type-k") + 1] == "q8_0"
    assert cmd[cmd.index("--cache-type-v") + 1] == "q8_0"
    assert "--parallel" not in cmd and cmd[cmd.index("-c") + 1] == "8192"
    cmd = command_for(monkeypatch, ctx=8192, kv_cache_type="q8_0", flash_attn="auto", port=9999)
    assert cmd[cmd.index("-fa") + 1] == "auto"


def test_vision_and_reduce_keep_one_slot():
    import inspect

    from bbb_worker.steps import reduce, vision

    assert "parallel" not in inspect.getsource(vision)
    assert "parallel" not in inspect.getsource(reduce)
    assert inspect.signature(llm_client.open_backend).parameters["parallel"].default == 1
    assert inspect.signature(llama.running).parameters["parallel"].default == 1


def test_asr_knobs_fall_back_to_the_current_behaviour(monkeypatch):
    from bbb_worker.steps import asr

    monkeypatch.delenv("BBB_ASR_BATCH", raising=False)
    assert asr._env_int("BBB_ASR_BATCH", 0) == 0
    assert asr._env_int("BBB_ASR_BEAM", 5) == 5
    monkeypatch.setenv("BBB_ASR_BEAM", "2")
    assert asr._env_int("BBB_ASR_BEAM", 5) == 2
    for bad in ("0", "junk", "-3", "9999"):
        monkeypatch.setenv("BBB_ASR_BEAM", bad)
        assert asr._env_int("BBB_ASR_BEAM", 5) == 5
