import json
import re

import pytest

from bbb_worker import draft, llm_client, schema
from bbb_worker.steps import extract, read_json, reduce

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

MOUNT = "Alex, can you have the mount ready by next Friday?"
PROFILES = "We decided to use aluminium profiles."
BUDGET = "The budget for the supplier is two thousand euro."
BOB = "Bob will send the invoice."
OLD = "Sam checks the old schedule."
SPECIAL = {88: MOUNT, 30: PROFILES, 190: BUDGET, 50: BOB, 120: OLD}  # lines of 10 s: 14:40 ...

RULES = [
    {
        "quote": MOUNT,
        "text": "Alex prepares the monitor mount",
        "fields": {"assignee": "alex", "due": "2026-11-20", "relates_to": "#12"},
    },
    {
        "quote": PROFILES,
        "text": "Use aluminium profiles",
        "fields": {"type": "decision", "relates_to": "does-not-exist"},
    },
    {"quote": BUDGET, "text": "Supplier budget is 2000 euro", "fields": {"type": "budget"}},
    {
        "quote": BOB,
        "text": "Bob sends the invoice",
        "fields": {"assignee": "bob", "due": "2026-11-18"},
    },
    {
        "quote": OLD,
        "text": "Sam checks the schedule",
        "fields": {"assignee": "sam", "due": "2020-01-01"},
    },
]

SECTIONS = [
    {"id": "hardware", "title": "Hardware", "description": "Mounts and profiles"},
    {"id": "money", "title": "Money", "description": "Budgets and invoices"},
    {"id": "planning", "title": "Planning", "description": "Schedules"},
]


def sections_answer(body, n):
    text = body["messages"][1]["content"]
    pick = {"mount": "hardware", "aluminium": "hardware", "budget": "money", "invoice": "money"}
    assignments = []
    for item_id, label in re.findall(r"^(i\d+) \[\w+\] (.*)$", text, re.MULTILINE):
        section = next((v for k, v in pick.items() if k in label.lower()), "planning")
        assignments.append({"item": item_id, "section": section})
    return {"sections": SECTIONS, "assignments": assignments}


def handler(summary=lambda b, n: {"summary": "The meeting in four sentences."}, sections=None):
    return by_kind(
        extraction=extraction_for(RULES),
        summary=summary,
        sections=sections or sections_answer,
        assignments=lambda b, n: {"assignments": []},
    )


def run_both(tmp_path, fake, ctx=None, transcript=None, files=None):
    m = make_meeting(
        tmp_path,
        transcript or long_transcript(35, SPECIAL),
        ctx or context_doc(mode="auto"),
        llm_options(fake.url),
        files,
    )
    extract.run(m)
    reduce.run(m)
    return m


def read_draft(m, name="draft.json"):
    return read_json(m.work_dir / name)


def test_end_to_end_auto_draft_validates_and_is_consistent(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
        kinds = fake.kinds()
    doc = read_draft(m)
    schema.validate("meeting", doc)
    assert (doc["meeting_id"], doc["cycle"], doc["date"]) == ("m1", "TEAM", "2026-11-16")
    assert (doc["language"], doc["mode"]) == ("en", "auto")
    assert doc["summary"] == "The meeting in four sentences."
    assert [s["id"] for s in doc["sections"]] == ["hardware", "money", "planning"]
    # the mount task sits in the overlap of two windows and is reported once
    texts = [i["text"] for i in doc["items"]]
    assert texts.count("Alex prepares the monitor mount") == 1
    assert len(doc["items"]) == 5
    assert [i["id"] for i in doc["items"]] == ["i001", "i002", "i003", "i004", "i005"]
    starts = [i["source"]["t_start_ms"] for i in doc["items"]]
    assert starts == sorted(starts)
    by_text = {i["text"]: i for i in doc["items"]}
    assert by_text["Alex prepares the monitor mount"]["section"] == "hardware"
    assert by_text["Supplier budget is 2000 euro"]["section"] == "money"
    assert all(i["edited"] is False and i["source"]["verified"] for i in doc["items"])
    assert kinds.count("extraction") >= 3 and "summary" in kinds and "sections" in kinds


def test_items_are_validated_against_the_context(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
    doc = read_draft(m)
    by_text = {i["text"]: i for i in doc["items"]}
    mount = by_text["Alex prepares the monitor mount"]
    assert mount["assignee"] == "alex" and mount["due"] == "2026-11-20"
    assert mount["relates_to"] == "#12"
    assert by_text["Use aluminium profiles"]["relates_to"] is None  # unknown link
    bob = by_text["Bob sends the invoice"]
    assert bob["assignee"] is None and bob["certainty"]["assignee"] == "unknown"
    assert bob["due"] == "2026-11-18" and bob["certainty"]["due"] == "stated"
    old = by_text["Sam checks the schedule"]
    assert old["assignee"] == "sam" and old["due"] is None
    assert old["certainty"] == {"assignee": "stated", "due": "unknown"}
    codes = [w["code"] for w in doc["warnings"]]
    assert codes.count("assignee_unknown") == 1
    assert "bob" in next(w for w in doc["warnings"] if w["code"] == "assignee_unknown")["message"]


def test_summary_is_reduced_in_groups_for_many_windows(tmp_path):
    with FakeLlm(handler()) as fake:
        backend = llm_client.Backend(fake.url, "fake")
        summaries = [f"Part {i} was discussed. More detail." for i in range(30)]
        text = reduce.summarize(backend, summaries, "en")
        kinds = fake.kinds()
        first_prompt = fake.requests[0]["messages"][1]["content"]
    assert text == "The meeting in four sentences."
    assert kinds == ["summary"] * 5  # 4 groups of 8 (30 -> 4), then the final one
    assert "Part 0 was discussed" in first_prompt and "Part 8 was" not in first_prompt


def summary_requests(fake):
    return [
        (r["messages"][0]["content"], r["messages"][1]["content"])
        for r in fake.requests
        if r["response_format"]["json_schema"]["name"] == "summary"
    ]


def test_the_summary_prompt_names_the_people_who_have_a_name(tmp_path):
    transcript = long_transcript(35, SPECIAL)
    named = long_transcript(35, SPECIAL)
    # Two voices: one named by the user, one not.
    named["speakers"][1]["label"] = "Radek Wójcik"
    named["speakers"].append(
        {"id": "mic:SPEAKER_01", "label": "Speaker 2", "kind": "voice", "track": "mic"}
    )
    for seg in named["segments"][::2]:
        seg["speaker"] = "mic:SPEAKER_00"
    for seg in named["segments"][1::4]:
        seg["speaker"] = "mic:SPEAKER_01"
    with FakeLlm(handler()) as fake:
        run_both(tmp_path, fake, transcript=transcript, files={"transcript.named.json": named})
        calls = summary_requests(fake)
    assert calls
    for system, user in calls:
        assert 'never replace a name by a general word like "participant"' in system
        assert "People in the meeting (use these names): Radek Wójcik\n" in user


def test_the_summary_prompt_has_no_people_line_without_names(tmp_path):
    with FakeLlm(handler()) as fake:
        run_both(tmp_path, fake)
        calls = summary_requests(fake)
    assert calls and all("People in the meeting" not in user for _system, user in calls)


def test_summary_failure_falls_back_to_first_sentences_with_a_warning(tmp_path):
    with FakeLlm(handler(summary=lambda b, n: {"nope": 1})) as fake:
        m = run_both(tmp_path, fake)
    doc = read_draft(m)
    schema.validate("meeting", doc)
    assert doc["summary"].startswith("Summary of a window with")
    assert "summary_failed" in [w["code"] for w in doc["warnings"]]


def test_section_failure_falls_back_to_type_sections(tmp_path):
    with FakeLlm(handler(sections=lambda b, n: "garbage")) as fake:
        m = run_both(tmp_path, fake)
    doc = read_draft(m)
    schema.validate("meeting", doc)
    assert [s["id"] for s in doc["sections"]] == ["decisions", "tasks", "budgets"]
    by_text = {i["text"]: i for i in doc["items"]}
    assert by_text["Use aluminium profiles"]["section"] == "decisions"
    assert by_text["Alex prepares the monitor mount"]["section"] == "tasks"


def test_polish_fallback_sections_are_in_polish(tmp_path):
    items = [{"type": "task"}, {"type": "decision"}]
    titles = [s["title"] for s in draft.type_sections(items, "pl")]
    assert titles == ["Decyzje", "Zadania"]


def test_fixed_mode_has_no_sections_and_makes_no_section_call(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake, ctx=context_doc(mode="fixed"))
        assert "sections" not in fake.kinds()
    doc = read_draft(m)
    assert doc["sections"] == [] and all(i["section"] is None for i in doc["items"])


def test_template_mode_takes_sections_from_the_context(tmp_path):
    sections = [
        {"id": "hardware", "title": "Hardware", "description": "Parts"},
        {"id": "other", "title": "Other", "description": ""},
    ]
    ctx = context_doc(mode="template", template="tech", meeting_type="technical", sections=sections)

    def with_sections(body, n):
        out = extraction_for(RULES)(body, n)
        for item in out["items"]:
            item["section"] = "hardware" if item["type"] == "task" else "ghost"
        return out

    h = by_kind(
        extraction=with_sections,
        summary=lambda b, n: {"summary": "Done."},
        sections=sections_answer,
    )
    with FakeLlm(h) as fake:
        m = run_both(tmp_path, fake, ctx=ctx)
        assert "sections" not in fake.kinds()
    doc = read_draft(m)
    schema.validate("meeting", doc)
    assert doc["sections"] == sections
    assert (doc["template"], doc["meeting_type"]) == ("tech", "technical")
    got = {i["type"]: i["section"] for i in doc["items"]}
    assert got["task"] == "hardware" and got["decision"] is None


def test_single_window_without_auto_needs_no_model_in_reduce(tmp_path):
    small = transcript_doc(
        [segment(0, 0, 5_000, MOUNT), segment(1, 10_000, 15_000, "Yes, I will do it.")]
    )
    with FakeLlm(handler()) as fake:
        m = make_meeting(tmp_path, small, context_doc(mode="fixed"), llm_options(fake.url))
        extract.run(m)
        before = len(fake.requests)
        reduce.run(m)
        assert len(fake.requests) == before
    doc = read_draft(m)
    assert len(doc["items"]) == 1 and doc["summary"].startswith("Summary of a window")


def test_long_item_lists_are_assigned_in_batches(tmp_path):
    items = [
        {
            "id": f"i{n:03d}",
            "type": "task",
            "text": f"Task {n} about a mount",
            "section": None,
        }
        for n in range(1, 131)
    ]

    def assign(body, n):
        listed = re.findall(r"^(i\d+) ", body["messages"][1]["content"], re.MULTILINE)
        return {"assignments": [{"item": i, "section": "hardware"} for i in listed]}

    h = by_kind(sections=sections_answer, assignments=assign)
    with FakeLlm(h) as fake:
        backend = llm_client.Backend(fake.url, "fake")
        sections = reduce.propose_sections(backend, items, context_doc(), "en")
        kinds = fake.kinds()
    assert kinds == ["sections", "assignments", "assignments"]
    assert len(sections) == 3
    assert all(i["section"] == "hardware" for i in items)


def test_existing_draft_with_edits_is_not_overwritten(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
        mine = read_draft(m)
        mine["items"][0]["text"] = "Edited by the user"
        mine["items"][0]["edited"] = True
        (m.work_dir / "draft.json").write_text(json.dumps(mine), encoding="utf-8")
        before = (m.work_dir / "draft.json").read_bytes()
        reduce.run(m)
    assert (m.work_dir / "draft.json").read_bytes() == before
    new = read_draft(m, "draft.new.json")
    schema.validate("meeting", new)
    assert new["items"][0]["text"] != "Edited by the user"


def test_existing_draft_without_edits_is_replaced(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
        doc = read_draft(m)
        doc["summary"] = "stale"
        (m.work_dir / "draft.json").write_text(json.dumps(doc), encoding="utf-8")
        (m.work_dir / draft.BASE_HASH).unlink()  # a draft from before the hash existed
        reduce.run(m)
    assert read_draft(m)["summary"] == "The meeting in four sentences."
    assert not (m.work_dir / "draft.new.json").exists()


def test_unreadable_draft_is_not_overwritten(tmp_path):
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
        (m.work_dir / "draft.json").write_text("{broken", encoding="utf-8")
        reduce.run(m)
    assert (m.work_dir / "draft.json").read_text(encoding="utf-8") == "{broken"
    assert (m.work_dir / "draft.new.json").exists()


def test_unverified_items_stay_with_a_warning_and_are_not_double_counted(tmp_path):
    rules = [
        *RULES[:1],
        {"quote": "never said anywhere", "text": "Ghost task", "fields": {}},
    ]

    def ghost(body, n):
        window = body["messages"][1]["content"]
        out = extraction_for(rules)(body, n)
        if MOUNT in window:
            out["items"].append(
                {
                    **out["items"][0],
                    "text": "Ghost task",
                    "quote": "never said anywhere",
                }
            )
        return out

    h = by_kind(extraction=ghost, summary=lambda b, n: {"summary": "ok"}, sections=sections_answer)
    with FakeLlm(h) as fake:
        m = run_both(tmp_path, fake)
    doc = read_draft(m)
    ghosts = [i for i in doc["items"] if i["text"] == "Ghost task"]
    assert len(ghosts) == 1 and not ghosts[0]["source"]["verified"]
    assert [w["code"] for w in doc["warnings"]].count("quote_unverified") == 1


def test_skipped_windows_are_carried_into_the_draft(tmp_path):
    def broken_first(body, n):
        if "Filler talk number 0 " in body["messages"][1]["content"]:
            return {"summary": 1}
        return extraction_for(RULES)(body, n)

    h = by_kind(
        extraction=broken_first, summary=lambda b, n: {"summary": "ok"}, sections=sections_answer
    )
    with FakeLlm(h) as fake:
        m = run_both(tmp_path, fake)
    doc = read_draft(m)
    schema.validate("meeting", doc)
    skipped = [w for w in doc["warnings"] if w["code"] == "window_skipped"]
    assert len(skipped) == 1 and skipped[0]["t_start_ms"] == 0


def test_cloud_reduce_is_refused_without_confirmation(tmp_path, monkeypatch):
    monkeypatch.setenv("BBB_LLM_API_KEY", "k")
    with FakeLlm(handler()) as fake:
        m = run_both(tmp_path, fake)
        count = len(fake.requests)
        opts = llm_options(fake.url, "cloud")
        refused = make_meeting(tmp_path / "other", long_transcript(35), context_doc(), opts)
        with pytest.raises(llm_client.LlmError, match="not confirmed"):
            reduce.run(refused)
        assert len(fake.requests) == count
    assert read_draft(m)["items"]


def test_helpers():
    parts = [{"name": "Alex Kowalski", "id": "alex"}]
    assert reduce.resolve_assignee("Alex Kowalski", parts) == "alex"
    assert reduce.resolve_assignee("ALEX", parts) == "alex"
    assert reduce.resolve_assignee("bob", parts) is None
    assert reduce.valid_due("2026-11-15", "2026-11-16")  # the day before is allowed
    assert not reduce.valid_due("2026-11-14", "2026-11-16")
    assert not reduce.valid_due("2026-02-30", "2026-11-16")
    assert not reduce.valid_due("tomorrow", "2026-11-16")
    assert reduce.first_sentence("One two. Three four.") == "One two."
    assert reduce.first_sentence("No end") == "No end"
