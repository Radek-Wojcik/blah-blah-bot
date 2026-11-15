import json
from contextlib import contextmanager
from pathlib import Path

import pytest

from bbb_worker import llm_client, prompts, schema
from bbb_worker.steps import extract, read_json

from .llm_support import (
    FakeLlm,
    Truncated,
    by_kind,
    context_doc,
    extraction_for,
    llm_options,
    long_transcript,
    make_meeting,
    raw_item,
    segment,
    transcript_doc,
)

MOUNT = "Alex, can you have the mount ready by next Friday?"
RULES = [
    {
        "quote": MOUNT,
        "text": "Alex prepares the monitor mount",
        "fields": {"assignee": "alex", "due": "2026-11-20"},
    }
]


def small_transcript() -> dict:
    return transcript_doc(
        [
            segment(0, 0, 5_000, "Okay, let's start with the monitor mount."),
            segment(1, 10_000, 18_000, MOUNT, speaker="mic:SPEAKER_00"),
            segment(2, 20_000, 24_000, "Yes, I will finish the prototype by Friday."),
            segment(3, 30_000, 36_000, "The salary of the new hire is confidential."),
        ]
    )


def windows_doc(meeting) -> dict:
    return read_json(meeting.work_dir / "extract" / "windows.json")


def test_extract_writes_verified_items(tmp_path):
    with FakeLlm(extraction_for(RULES)) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        extract.run(m)
    doc = windows_doc(m)
    assert doc["language"] == "en"
    (window,) = doc["windows"]
    assert not window["skipped"] and window["attempts"] == 1
    assert window["usage"]["prompt_tokens"] == 10
    (item,) = window["items"]
    assert item["source"]["verified"] and item["source"]["match"] == 1.0
    assert (item["source"]["t_start_ms"], item["source"]["t_end_ms"]) == (10_000, 18_000)
    assert item["source"]["speaker"] == "mic:SPEAKER_00"
    assert item["assignee"] == "alex" and item["due"] == "2026-11-20"
    assert window["warnings"] == []


def test_position_comes_from_the_transcript_not_from_the_model(tmp_path):
    def handler(body, n):
        return {
            "summary": "s",
            "items": [raw_item("Alex prepares the mount", MOUNT, "00:00:21", assignee="alex")],
            "status_changes": [],
        }

    with FakeLlm(handler) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        extract.run(m)
    (item,) = windows_doc(m)["windows"][0]["items"]
    assert item["source"]["verified"]
    assert item["source"]["t_start_ms"] == 10_000  # the model said 00:00:21


def test_unverified_quote_is_kept_and_flagged(tmp_path):
    def handler(body, n):
        return {
            "summary": "s",
            "items": [raw_item("Sam buys a laser", "Sam will buy a new laser cutter", "00:00:12")],
            "status_changes": [],
        }

    with FakeLlm(handler) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        extract.run(m)
    (window,) = windows_doc(m)["windows"]
    (item,) = window["items"]
    assert not item["source"]["verified"]
    assert (item["source"]["t_start_ms"], item["source"]["t_end_ms"]) == (10_000, 18_000)
    assert [w["code"] for w in window["warnings"]] == ["quote_unverified"]


def test_prompt_has_context_and_nothing_excluded(tmp_path):
    ctx = context_doc(
        instruction="Prefer short task titles.",
        previous_items=[{"meeting": "2026-11-09", "type": "task", "text": "Order profiles"}],
    )
    edits = {"segments": {"3": {"excluded": True}}}
    with FakeLlm(extraction_for(RULES)) as fake:
        m = make_meeting(
            tmp_path,
            small_transcript(),
            ctx,
            llm_options(fake.url),
            {"edits.json": edits},
        )
        extract.run(m)
        sent = fake.prompts()
        body = fake.requests[0]
    assert "Zephyr: the code name of the monitor mount" in sent
    assert "- alex: Alex Kowalski" in sent
    assert "Meeting date: 2026-11-16 (Monday)" in sent
    assert "Tuesday 2026-11-17" in sent and "Monday 2026-12-14" in sent  # 28 days
    assert "Tuesday 2026-12-15" not in sent
    assert "t1: Supplier choice" in sent and "#12: Mount prototype" in sent
    assert "Prefer short task titles." in sent
    assert "[2026-11-09] task: Order profiles" in sent
    assert "[00:00:10] Speaker 1: " + MOUNT in sent
    assert "salary" not in sent and "confidential" not in sent
    assert "Write `text` and `summary` in English" in sent
    assert body["temperature"] == 0
    assert body["chat_template_kwargs"] == {"enable_thinking": False}
    rf = body["response_format"]["json_schema"]
    assert rf["strict"] is True and rf["schema"]["required"] == [
        "summary",
        "items",
        "status_changes",
    ]
    assert body["model"] == "fake"


def test_named_transcript_is_preferred(tmp_path):
    named = small_transcript()
    named["speakers"][1]["label"] = "Sam"
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(
            tmp_path,
            small_transcript(),
            context_doc(),
            llm_options(fake.url),
            {"transcript.named.json": named},
        )
        extract.run(m)
        sent = fake.prompts()
    assert "] Sam: " in sent and "Speaker 1" not in sent


def test_edited_text_is_sent_instead_of_the_recognised_one(tmp_path):
    edits = {"segments": {"0": {"text": "Corrected opening line."}}}
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(
            tmp_path,
            small_transcript(),
            context_doc(),
            llm_options(fake.url),
            {"edits.json": edits},
        )
        extract.run(m)
        sent = fake.prompts()
    assert "Corrected opening line." in sent and "let's start" not in sent


def test_screenshot_descriptions_are_in_the_prompt_at_their_time(tmp_path):
    shots = {
        "version": 1,
        "meeting_id": "m1",
        "items": [
            {"file": "s/1.png", "offset_ms": 12_000, "description": "A slide titled Budget."}
        ],
    }
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(
            tmp_path,
            small_transcript(),
            context_doc(),
            llm_options(fake.url),
            {"descriptions.json": shots},
        )
        extract.run(m)
        sent = fake.prompts()
    lines = sent.splitlines()
    shot = lines.index("[00:00:12] (screenshot) A slide titled Budget.")
    assert lines[shot - 1].startswith("[00:00:10] ") and lines[shot + 1].startswith("[00:00:20] ")


def test_allowed_sections_are_listed_and_unknown_ones_dropped(tmp_path):
    sections = [{"id": "hardware", "title": "Hardware", "description": "Mounts and parts"}]
    ctx = context_doc(mode="template", template="tech", sections=sections)

    def handler(body, n):
        a = raw_item("Mount", MOUNT, "00:00:10", section="hardware")
        b = raw_item(
            "Prototype", "I will finish the prototype by Friday", "00:00:20", section="made-up"
        )
        return {"summary": "s", "items": [a, b], "status_changes": []}

    with FakeLlm(handler) as fake:
        m = make_meeting(tmp_path, small_transcript(), ctx, llm_options(fake.url))
        extract.run(m)
        sent = fake.prompts()
    assert "- hardware: Hardware (Mounts and parts)" in sent
    sections_out = [i["section"] for i in windows_doc(m)["windows"][0]["items"]]
    assert sections_out == ["hardware", None]


def test_invalid_answer_is_retried_with_the_error_in_the_prompt(tmp_path):
    def handler(body, n):
        if n == 1:
            return "this is not json"
        if n == 2:
            return {"summary": "s", "items": [{"type": "task"}]}
        return extraction_for(RULES)(body, n)

    with FakeLlm(handler) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        extract.run(m)
        third = fake.requests[2]["messages"]
    (window,) = windows_doc(m)["windows"]
    assert window["attempts"] == 3 and not window["skipped"] and len(window["items"]) == 1
    assert third[-1]["role"] == "user" and "invalid" in third[-1]["content"]
    assert "required" in third[-1]["content"]  # the validation error of the second answer


def test_window_is_skipped_after_the_retries_and_the_stage_continues(tmp_path):
    ctx = context_doc()
    special = {0: "The mount is ready, thanks."}

    def handler(body, n):
        window = body["messages"][1]["content"]
        if "The mount is ready, thanks." in window:
            return {"summary": "no items key"}
        return extraction_for([])(body, n)

    with FakeLlm(handler) as fake:
        m = make_meeting(tmp_path, long_transcript(35, special), ctx, llm_options(fake.url))
        extract.run(m)
        assert fake.requests  # the first window was asked three times
    first, *rest = windows_doc(m)["windows"]
    assert first["skipped"] and first["attempts"] == 3 and first["items"] == []
    assert first["warnings"][0]["code"] == "window_skipped"
    assert rest and not any(w["skipped"] for w in rest)


def test_a_rejecting_server_fails_the_stage(tmp_path):
    with FakeLlm(lambda body, n: (401, "no")) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        with pytest.raises(llm_client.LlmError, match="HTTP 401"):
            extract.run(m)
        assert len(fake.requests) == 1


def test_no_transcript_text_means_no_model_is_started(tmp_path):
    doc = transcript_doc([segment(0, 0, 1000, "private")])
    edits = {"segments": {"0": {"excluded": True}}}
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(tmp_path, doc, context_doc(), llm_options(fake.url), {"edits.json": edits})
        extract.run(m)
        assert fake.requests == []
    assert windows_doc(m)["windows"] == []


def test_output_language_is_resolved_from_a_polish_transcript(tmp_path):
    pl = transcript_doc(
        [
            segment(0, 0, 5_000, "Dobrze, ustalmy to jeszcze dzisiaj, bo mamy mało czasu."),
            segment(1, 6_000, 9_000, "Zrobię to na piątek, jak będzie można."),
        ],
        language="pl",
    )
    ctx = context_doc(output_language="auto")
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(tmp_path, pl, ctx, llm_options(fake.url))
        extract.run(m)
        sent = fake.prompts()
    assert windows_doc(m)["language"] == "pl"
    assert "in Polish" in sent


def test_calendar_text():
    text = prompts.calendar_text("2026-11-16")
    assert text.startswith("Tuesday 2026-11-17") and text.endswith("Monday 2026-12-14")
    assert text.count(",") == 27


# Backends ---------------------------------------------------------------------------------


def test_cloud_is_refused_without_confirmation_and_makes_no_request(tmp_path, monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "secret-key")
    with FakeLlm(extraction_for(RULES)) as fake:
        m = make_meeting(
            tmp_path, small_transcript(), context_doc(), llm_options(fake.url, "cloud")
        )
        with pytest.raises(llm_client.LlmError, match="not confirmed"):
            extract.run(m)
        assert fake.requests == []
    assert not (m.work_dir / "extract").exists()


def test_cloud_sends_the_key_as_bearer_when_confirmed(tmp_path, monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "secret-key")
    with FakeLlm(extraction_for(RULES)) as fake:
        opts = llm_options(fake.url, "cloud", cloud_confirmed=True)
        m = make_meeting(tmp_path, small_transcript(), context_doc(), opts)
        extract.run(m)
        assert fake.headers[0]["authorization"] == "Bearer secret-key"
        assert "chat_template_kwargs" not in fake.requests[0]
    assert "secret-key" not in json.dumps(windows_doc(m))


def test_cloud_without_a_key_makes_no_request(tmp_path, monkeypatch):
    monkeypatch.delenv("BBB_LLM_API_KEY", raising=False)
    with FakeLlm(extraction_for(RULES)) as fake:
        opts = llm_options(fake.url, "cloud", cloud_confirmed=True)
        m = make_meeting(tmp_path, small_transcript(), context_doc(), opts)
        with pytest.raises(llm_client.LlmError, match="BBB_LLM_API_KEY"):
            extract.run(m)
        assert fake.requests == []


def test_external_backend_sends_no_authorization_without_a_key(tmp_path, monkeypatch):
    monkeypatch.delenv("BBB_LLM_API_KEY", raising=False)
    with FakeLlm(extraction_for(RULES)) as fake:
        m = make_meeting(tmp_path, small_transcript(), context_doc(), llm_options(fake.url))
        extract.run(m)
        assert "authorization" not in fake.headers[0]


def test_managed_backend_starts_and_always_stops_the_server(tmp_path, monkeypatch):
    log: list = []

    with FakeLlm(extraction_for(RULES)) as fake:

        @contextmanager
        def fake_running(model, *, ctx=8192, **kwargs):
            log.append(("start", Path(model).name, ctx))
            try:
                yield fake.url.removesuffix("/v1")
            finally:
                log.append("stop")

        monkeypatch.setattr(llm_client.llama, "running", fake_running)
        opts = {
            "language": "auto",
            "llm": {"kind": "managed", "model_path": "m/gemma.gguf", "ctx": 8192},
        }
        m = make_meeting(tmp_path, small_transcript(), context_doc(), opts)
        extract.run(m)
        assert "model" not in fake.requests[0]
    assert log == [("start", "gemma.gguf", 8192), "stop"]


def test_managed_server_is_stopped_when_the_stage_fails(tmp_path, monkeypatch):
    log: list = []

    with FakeLlm(lambda body, n: (400, "bad")) as fake:

        @contextmanager
        def fake_running(model, *, ctx=8192, **kwargs):
            try:
                yield fake.url.removesuffix("/v1")
            finally:
                log.append("stop")

        monkeypatch.setattr(llm_client.llama, "running", fake_running)
        opts = {"language": "auto", "llm": {"kind": "managed", "model_path": "m.gguf"}}
        m = make_meeting(tmp_path, small_transcript(), context_doc(), opts)
        with pytest.raises(llm_client.LlmError):
            extract.run(m)
    assert log == ["stop"]


def test_missing_llm_options_fail_clearly(tmp_path):
    m = make_meeting(tmp_path, small_transcript(), context_doc(), {"language": "auto"})
    with pytest.raises(llm_client.LlmError, match="options.llm"):
        extract.run(m)


def test_complete_json_reports_attempts_and_usage():
    with FakeLlm(by_kind(summary=lambda b, n: {"summary": "x"} if n == 2 else "nope")) as fake:
        backend = llm_client.Backend(fake.url, "fake")
        schema_doc = {
            "type": "object",
            "required": ["summary"],
            "properties": {"summary": {"type": "string"}},
        }
        done = llm_client.complete_json(
            backend, system="s", user="u", schema=schema_doc, name="summary", max_tokens=10
        )
    assert done.attempts == 2 and done.data == {"summary": "x"}
    assert done.usage == {"prompt_tokens": 20, "completion_tokens": 10}


def test_extraction_fixture_schema_is_what_the_prompt_asks_for():
    sent = schema.load("extraction")
    assert sent["properties"]["items"]["items"]["required"][-1] == "quote"


# Truncated answers ------------------------------------------------------------------------

PROFILES = "We decided to use aluminium profiles."
SPLIT_RULES = [
    *RULES,
    {"quote": PROFILES, "text": "Use aluminium profiles", "fields": {"type": "decision"}},
]


def too_long_for(max_lines, then=None):
    """Truncated output for a window with more than `max_lines` lines, a valid answer otherwise."""
    answer = then or extraction_for(SPLIT_RULES)

    def handle(body, n):
        text = body["messages"][1]["content"]
        if text.count("Filler talk number") > max_lines:
            return Truncated('{"summary": "A very long start of an answer that never e')
        return answer(body, n)

    return handle


def long_meeting(tmp_path, url, **options):
    # one 14-minute window of 84 lines; the mount line (40) is in the overlap of the halves
    doc = long_transcript(14, {40: MOUNT, 70: PROFILES})
    opts = llm_options(url, **options)
    return make_meeting(tmp_path, doc, context_doc(mode="fixed"), opts)


def user_prompts(fake):
    return [r["messages"][1]["content"] for r in fake.requests]


def test_truncated_window_is_split_not_retried(tmp_path):
    with FakeLlm(too_long_for(50)) as fake:
        m = long_meeting(tmp_path, fake.url)
        extract.run(m)
        sent = user_prompts(fake)
    assert len(sent) == 3 and len(set(sent)) == 3  # whole, first half, second half; no repeats
    (window,) = windows_doc(m)["windows"]
    assert window["split"] and not window["skipped"] and window["index"] == 0
    assert window["attempts"] == 3
    assert window["usage"] == {"prompt_tokens": 30, "completion_tokens": 15}
    assert window["warnings"] == []
    assert window["summary"].count("Summary of a window") == 2
    parts = sorted((i["part"], i["text"]) for i in window["items"])
    assert parts == [
        ("0", "Alex prepares the monitor mount"),
        ("1", "Alex prepares the monitor mount"),
        ("1", "Use aluminium profiles"),
    ]
    assert all(i["source"]["verified"] and i["window"] == 0 for i in window["items"])
    schema_ok = windows_doc(m)
    assert schema_ok["windows"][0]["t_end_ms"] > schema_ok["windows"][0]["t_start_ms"]


def test_halves_overlap_items_are_deduplicated_by_reduce(tmp_path):
    from bbb_worker.steps import reduce

    h = by_kind(
        extraction=too_long_for(50),
        summary=lambda b, n: {"summary": "Joined."},
    )
    with FakeLlm(h) as fake:
        m = long_meeting(tmp_path, fake.url)
        extract.run(m)
        reduce.run(m)
    doc = read_json(m.work_dir / "draft.json")
    schema.validate("meeting", doc)
    assert sorted(i["text"] for i in doc["items"]) == [
        "Alex prepares the monitor mount",
        "Use aluminium profiles",
    ]
    assert all("part" not in i for i in doc["items"])


def test_splitting_stops_after_two_levels_and_the_window_is_skipped(tmp_path):
    with FakeLlm(lambda body, n: Truncated('{"summary": "cut')) as fake:
        m = long_meeting(tmp_path, fake.url)
        with pytest.raises(llm_client.LlmError, match="no usable answer"):
            extract.run(m)
        sent = user_prompts(fake)
    assert len(sent) == 7 and len(set(sent)) == 7  # 1 + 2 + 4
    (window,) = windows_doc(m)["windows"]
    assert window["skipped"] and window["items"] == []
    assert [w["code"] for w in window["warnings"]] == ["window_skipped"] * 4


def test_truncated_window_that_cannot_be_split_is_skipped_after_one_request(tmp_path):
    with FakeLlm(lambda body, n: Truncated('{"summary": "cut')) as fake:
        doc = transcript_doc([segment(0, 0, 5_000, "One short line only.")])
        m = make_meeting(tmp_path, doc, context_doc(), llm_options(fake.url))
        with pytest.raises(llm_client.LlmError, match="no usable answer"):
            extract.run(m)
        assert len(fake.requests) == 1
    (window,) = windows_doc(m)["windows"]
    assert window["skipped"] and window["attempts"] == 1


def test_complete_json_does_not_retry_a_truncated_answer():
    with FakeLlm(lambda body, n: Truncated('{"summary": "cu')) as fake:
        backend = llm_client.Backend(fake.url, "fake")
        schema_doc = {"type": "object", "required": ["summary"]}
        with pytest.raises(llm_client.TruncatedOutputError) as raised:
            llm_client.complete_json(
                backend, system="s", user="u", schema=schema_doc, name="x", max_tokens=10
            )
        assert len(fake.requests) == 1
    assert isinstance(raised.value, llm_client.InvalidOutputError)
    assert raised.value.attempts == 1 and raised.value.usage["prompt_tokens"] == 10


def test_output_budget_follows_the_context():
    assert extract.output_budget(17_400, 16384) == 8192  # about 5.8k prompt tokens
    assert extract.output_budget(17_400, 8192) == 8192 - 5_800 - 512
    assert extract.output_budget(17_400, 4096) == 1024
    assert extract.output_budget(17_400, None) == 8192


def test_request_uses_the_budget_from_the_configured_context(tmp_path):
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(
            tmp_path, small_transcript(), context_doc(), llm_options(fake.url, ctx=2048)
        )
        extract.run(m)
        assert fake.requests[0]["max_tokens"] == 1024
    with FakeLlm(extraction_for([])) as fake:
        m = make_meeting(
            tmp_path / "b", small_transcript(), context_doc(), llm_options(fake.url, ctx=16384)
        )
        extract.run(m)
        assert fake.requests[0]["max_tokens"] == 8192


def test_prompt_asks_for_compact_output():
    system = prompts.extract_system("en")
    assert "never report the same agreed item twice" in system and "Keep `text` short" in system
