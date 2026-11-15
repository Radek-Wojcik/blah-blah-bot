"""Status proposals: the model reports open issues and topics that were finished (extract), the
draft carries them as proposals that are never approved by the worker (reduce). A mock server
stands in for the model; nothing here needs a model or a GPU."""

import copy
import json

import jsonschema
import pytest

from bbb_worker import draft, prompts, schema
from bbb_worker.steps import extract, read_json, reduce

from .llm_support import (
    FakeLlm,
    by_kind,
    context_doc,
    extraction_for,
    llm_options,
    make_meeting,
    raw_status,
    segment,
    transcript_doc,
)

DONE = "The prototype of the mount is done, I tested it yesterday."
LATER = "Yes the prototype is really finished now."
TOPIC = "We settled the supplier question, it is closed."
ISSUE_14 = "The wiring harness work is complete."
NOISE = "We should look at the battery soon."


def transcript() -> dict:
    return transcript_doc(
        [
            segment(0, 0, 5_000, "Okay, let's start."),
            segment(1, 10_000, 18_000, DONE, speaker="mic:SPEAKER_00"),
            segment(2, 20_000, 24_000, TOPIC),
            segment(3, 30_000, 34_000, ISSUE_14),
            segment(4, 40_000, 44_000, NOISE),
            segment(5, 50_000, 56_000, LATER, speaker="mic:SPEAKER_00"),
        ]
    )


def context(**over):
    fields = {
        "previous_items": [
            {
                "meeting": "2026-11-09",
                "type": "task",
                "text": "Finish the wiring harness",
                "assignee": "sam",
                "issue": "#14",
            }
        ],
        **over,
    }
    return context_doc(**fields)


def run(tmp_path, answer, ctx=None, files=None, steps=("extract", "reduce")):
    with FakeLlm(by_kind(extraction=answer, summary=lambda b, n: {"summary": "Ok."})) as fake:
        m = make_meeting(tmp_path, transcript(), ctx or context(), llm_options(fake.url), files)
        extract.run(m)
        if "reduce" in steps:
            reduce.run(m)
        return m, fake


def windows(m):
    return read_json(m.work_dir / "extract" / "windows.json")["windows"]


def proposals(m, name="draft.json"):
    return read_json(m.work_dir / name).get("status_proposals", [])


def answering(*statuses):
    """An extraction answer with these `status_changes` (quote, target) and no items."""

    def handle(body, n):
        out = extraction_for([])(body, n)
        window = body["messages"][1]["content"]
        for quote, target, *rest in statuses:
            if quote in window:
                line = next(ln for ln in window.splitlines() if quote in ln)
                out["status_changes"].append(
                    raw_status(target, quote, line[1:9], *(rest[:1] or []))
                )
        return out

    return handle


# ----- the prompt and the contract ---------------------------------------------------------


def test_the_prompt_explains_status_changes_and_lists_the_targets(tmp_path):
    _, fake = run(tmp_path, answering(), steps=("extract",))
    sent = fake.prompts()
    system = fake.requests[0]["messages"][0]["content"]
    assert "`status_changes`" in system and '"done"' in system
    assert "never anything else" in system  # only ids of the lists
    assert "Open issues (ids for relates_to and status_changes):" in sent
    assert "Open topics (ids for relates_to and status_changes):" in sent
    assert "#12: Mount prototype" in sent and "t1: Supplier choice" in sent
    # The issue of an earlier item is shown with the item, so the model can target it.
    assert "[2026-11-09] task: Finish the wiring harness (assignee: sam, issue: #14)" in sent
    assert "status_changes" in prompts.extract_system("pl")


def test_a_previous_item_without_an_issue_is_listed_as_before():
    ctx = context_doc(
        previous_items=[{"meeting": "2026-11-09", "type": "task", "text": "Order profiles"}]
    )
    user = prompts.extract_user(ctx, "[00:00:01] You: hello")
    assert "[2026-11-09] task: Order profiles\n" in user and "issue:" not in user


def test_the_model_contract_requires_the_field_and_the_request_sends_it(tmp_path):
    _, fake = run(tmp_path, answering(), steps=("extract",))
    sent = fake.requests[0]["response_format"]["json_schema"]["schema"]
    assert "status_changes" in sent["required"]
    answer = {"summary": "s", "items": []}
    with pytest.raises(jsonschema.ValidationError, match="status_changes"):
        schema.validate("extraction", answer)
    schema.validate("extraction", {**answer, "status_changes": []})


def test_an_answer_without_the_field_is_retried_like_any_invalid_answer(tmp_path):
    def handler(body, n):
        if n == 1:
            return {"summary": "no status changes", "items": []}
        return answering((DONE, "#12"))(body, n)

    m, fake = run(tmp_path, handler, steps=("extract",))
    (window,) = windows(m)
    assert window["attempts"] == 2 and not window["skipped"]
    assert "status_changes" in fake.requests[1]["messages"][-1]["content"]
    assert [c["target"] for c in window["status_changes"]] == ["#12"]


# ----- extract: quote checks ---------------------------------------------------------------


def test_extract_verifies_the_quote_of_a_status_change_against_the_transcript(tmp_path):
    def handler(body, n):
        out = extraction_for([])(body, n)
        # The model names a wrong time; the position still comes from the matched line.
        out["status_changes"] = [raw_status("#12", DONE, "00:00:50", "Alex finished it.")]
        return out

    m, _ = run(tmp_path, handler, steps=("extract",))
    (window,) = windows(m)
    (change,) = window["status_changes"]
    assert change["target"] == "#12" and change["status"] == "done"
    assert change["reason"] == "Alex finished it."
    src = change["source"]
    assert src["verified"] and src["match"] == 1.0
    assert (src["t_start_ms"], src["t_end_ms"]) == (10_000, 18_000)
    assert src["speaker"] == "mic:SPEAKER_00" and src["quote"] == DONE
    assert window["warnings"] == [] and window["items"] == []


def test_a_quote_that_is_not_in_the_transcript_is_kept_flagged_with_a_warning(tmp_path):
    def handler(body, n):
        out = extraction_for([])(body, n)
        out["status_changes"] = [
            raw_status("#12", "Everything about the mount is wrapped up", "00:00:12")
        ]
        return out

    m, _ = run(tmp_path, handler, steps=("extract",))
    (window,) = windows(m)
    (change,) = window["status_changes"]
    assert change["source"]["verified"] is False
    assert [w["code"] for w in window["warnings"]] == ["quote_unverified"]
    assert "#12" in window["warnings"][0]["message"]


def test_a_screenshot_description_is_not_a_source_for_a_status_change(tmp_path):
    descriptions = {
        "version": 1,
        "meeting_id": "m1",
        "items": [
            {
                "file": "screenshots/a.png",
                "offset_ms": 30_000,
                "description": "A board that says: Mount prototype closed for good.",
            }
        ],
    }
    # When the file does not validate the test fails loudly instead of passing for the wrong
    # reason.
    schema.validate("screenshot-descriptions", descriptions)

    def handler(body, n):
        out = extraction_for([])(body, n)
        out["status_changes"] = [raw_status("#12", "Mount prototype closed for good.", "00:00:30")]
        return out

    m, _ = run(tmp_path, handler, files={"descriptions.json": descriptions}, steps=("extract",))
    (change,) = windows(m)[0]["status_changes"]
    assert change["source"]["verified"] is False


def test_split_windows_keep_the_status_changes_of_every_part():
    line = extract.windows.Line(
        start_ms=0, end_ms=100, track="mic", speaker="user", label="You", text="x"
    )
    window = extract.windows.Window(
        index=0, span_start_ms=0, span_end_ms=100, lines=(line,), part=""
    )
    first = extract._empty_result(window)
    part_a = extract._empty_result(window)
    part_b = extract._empty_result(window)
    part_a["status_changes"] = [{"target": "#1"}]
    part_b["status_changes"] = [{"target": "#2"}]
    merged = extract.merge_results(window, first, [part_a, part_b])
    assert [c["target"] for c in merged["status_changes"]] == ["#1", "#2"]


# ----- reduce: proposals -------------------------------------------------------------------


def test_reduce_turns_verified_changes_into_proposals_that_are_never_approved(tmp_path):
    m, _ = run(
        tmp_path,
        answering(
            (DONE, "#12", "Alex says the prototype is tested and finished."),
            (TOPIC, "t1"),
            (ISSUE_14, "#14"),
        ),
    )
    doc = read_json(m.work_dir / "draft.json")
    schema.validate("meeting", doc)
    got = doc["status_proposals"]
    assert [p["id"] for p in got] == ["s1", "s2", "s3"]
    assert [p["target"] for p in got] == ["#12", "t1", "#14"]  # by time of the quote
    for p in got:
        assert p["status"] == "done"
        assert p["decision"] == "proposed" and p["edited"] is False
        assert p["comment"] is None and p["reason"]
        assert p["source"]["verified"] is True
    assert got[0]["reason"] == "Alex says the prototype is tested and finished."
    assert got[0]["source"]["t_start_ms"] == 10_000
    # The proposals sit between the items and the warnings, as in the application's type.
    assert list(doc)[-2:] == ["status_proposals", "warnings"]
    assert doc["warnings"] == []


def test_targets_that_are_not_in_the_context_are_dropped(tmp_path):
    m, _ = run(
        tmp_path,
        answering((DONE, "#99"), (TOPIC, "made-up-topic"), (ISSUE_14, "#12")),
    )
    assert [p["target"] for p in proposals(m)] == ["#12"]
    assert proposals(m)[0]["source"]["quote"] == ISSUE_14  # the one with a real target


def test_the_target_may_be_written_loosely(tmp_path):
    m, _ = run(tmp_path, answering((DONE, "12"), (TOPIC, " T1 "), (ISSUE_14, "#14")))
    assert [p["target"] for p in proposals(m)] == ["#12", "t1", "#14"]


def test_an_issue_that_only_an_earlier_item_links_to_is_a_valid_target(tmp_path):
    ctx = context(open_issues=[], open_topics=[])
    m, _ = run(tmp_path, answering((DONE, "#12"), (ISSUE_14, "#14")), ctx=ctx)
    assert [p["target"] for p in proposals(m)] == ["#14"]


def test_one_proposal_per_target_and_the_latest_in_the_meeting_wins(tmp_path):
    m, _ = run(tmp_path, answering((DONE, "#12", "first mention"), (LATER, "#12", "confirmed")))
    (only,) = proposals(m)
    assert only["id"] == "s1" and only["reason"] == "confirmed"
    assert only["source"]["quote"] == LATER and only["source"]["t_start_ms"] == 50_000


def test_a_verified_quote_beats_a_later_unverified_one_for_the_same_target(tmp_path):
    def handler(body, n):
        out = extraction_for([])(body, n)
        out["status_changes"] = [
            raw_status("#12", DONE, "00:00:10", "heard it"),
            raw_status("#12", "this sentence is nowhere", "00:00:52", "invented"),
        ]
        return out

    m, _ = run(tmp_path, handler)
    (only,) = proposals(m)
    assert only["reason"] == "heard it" and only["source"]["verified"] is True
    assert read_json(m.work_dir / "draft.json")["warnings"] == []


def test_an_unverified_proposal_is_kept_and_warned_about(tmp_path):
    def handler(body, n):
        out = extraction_for([])(body, n)
        out["status_changes"] = [raw_status("#12", "it is all finished", "00:00:12")]
        return out

    m, _ = run(tmp_path, handler)
    (only,) = proposals(m)
    assert only["source"]["verified"] is False and only["decision"] == "proposed"
    doc = read_json(m.work_dir / "draft.json")
    schema.validate("meeting", doc)
    assert [w["code"] for w in doc["warnings"]] == ["quote_unverified"]
    assert "#12" in doc["warnings"][0]["message"]


def test_proposals_are_independent_of_items(tmp_path):
    rules = [{"quote": DONE, "text": "Alex finished the prototype", "fields": {"type": "note"}}]

    def handler(body, n):
        out = extraction_for(rules, [{"quote": DONE, "target": "#12"}])(body, n)
        return out

    m, _ = run(tmp_path, handler)
    doc = read_json(m.work_dir / "draft.json")
    assert len(doc["items"]) == 1 and len(doc["status_proposals"]) == 1
    schema.validate("meeting", doc)


def test_an_empty_context_gives_no_proposals_and_no_key(tmp_path):
    ctx = context(open_issues=[], open_topics=[], previous_items=[])
    m, _ = run(tmp_path, answering((DONE, "#12"), (TOPIC, "t1")), ctx=ctx)
    doc = read_json(m.work_dir / "draft.json")
    assert "status_proposals" not in doc
    schema.validate("meeting", doc)


def test_a_draft_without_proposals_is_byte_identical_to_the_stage_4_draft(tmp_path):
    rules = [{"quote": DONE, "text": "Alex finished", "fields": {"type": "note"}}]
    with FakeLlm(
        by_kind(extraction=extraction_for(rules), summary=lambda b, n: {"summary": "Ok."})
    ) as fake:
        m = make_meeting(tmp_path, transcript(), context(), llm_options(fake.url))
        extract.run(m)
        reduce.run(m)
    doc = read_json(m.work_dir / "draft.json")
    assert "status_proposals" not in doc
    assert list(doc) == [
        "version",
        "meeting_id",
        "cycle",
        "date",
        "language",
        "mode",
        "template",
        "meeting_type",
        "sections",
        "summary",
        "items",
        "warnings",
    ]


def test_a_windows_file_from_before_status_changes_existed_still_reduces(tmp_path):
    m, _ = run(tmp_path, answering((DONE, "#12")), steps=("extract",))
    path = m.work_dir / "extract" / "windows.json"
    old = read_json(path)
    for w in old["windows"]:
        del w["status_changes"]
    path.write_text(json.dumps(old), encoding="utf-8")
    with FakeLlm(by_kind(summary=lambda b, n: {"summary": "Ok."})) as fake:
        m.options["llm"]["base_url"] = fake.url
        reduce.run(m)
    doc = read_json(m.work_dir / "draft.json")
    schema.validate("meeting", doc)
    assert "status_proposals" not in doc


def test_proposals_survive_the_pure_functions_without_a_model():
    ctx = context_doc()
    src = {
        "track": "mic",
        "speaker": "user",
        "t_start_ms": 1000,
        "t_end_ms": 2000,
        "quote": "done",
        "verified": True,
        "match": 1.0,
    }
    change = {"target": "#12", "status": "done", "reason": "r", "source": src}
    later = copy.deepcopy(change)
    later["reason"] = "later"
    later["source"]["t_start_ms"] = 9000
    results = [{"status_changes": [change]}, {"status_changes": [later]}, {}]
    got, warnings = reduce.build_status_proposals(results, ctx)
    assert [(p["id"], p["target"], p["reason"]) for p in got] == [("s1", "#12", "later")]
    assert warnings == []
    assert reduce.build_status_proposals([], ctx) == ([], [])


# ----- draft protection --------------------------------------------------------------------


def test_a_draft_with_a_decided_proposal_is_not_overwritten(tmp_path):
    m, _ = run(tmp_path, answering((DONE, "#12")))
    path = m.work_dir / "draft.json"
    mine = read_json(path)
    mine["status_proposals"][0]["decision"] = "approved"
    path.write_text(json.dumps(mine), encoding="utf-8")
    assert draft.has_user_edits(path)
    before = path.read_bytes()
    with FakeLlm(by_kind(summary=lambda b, n: {"summary": "Ok."})) as fake:
        m.options["llm"]["base_url"] = fake.url
        reduce.run(m)
    assert path.read_bytes() == before
    new = read_json(m.work_dir / "draft.new.json")
    assert new["status_proposals"][0]["decision"] == "proposed"


@pytest.mark.parametrize(
    "change",
    [
        lambda p: p.update(decision="rejected"),
        lambda p: p.update(edited=True),
    ],
)
def test_a_flag_on_a_proposal_counts_as_an_edit_even_without_a_recorded_hash(tmp_path, change):
    m, _ = run(tmp_path, answering((DONE, "#12")))
    path = m.work_dir / "draft.json"
    assert not draft.has_user_edits(path)
    mine = read_json(path)
    change(mine["status_proposals"][0])
    path.write_text(json.dumps(mine), encoding="utf-8")
    (m.work_dir / draft.BASE_HASH).unlink()
    assert draft.has_user_edits(path)


def test_an_untouched_draft_with_proposals_is_replaced_by_the_next_reduce(tmp_path):
    m, _ = run(tmp_path, answering((DONE, "#12")))
    path = m.work_dir / "draft.json"
    assert not draft.has_user_edits(path)
    assert (m.work_dir / draft.BASE_HASH).read_text(encoding="ascii") == draft.sha256_hex(
        path.read_bytes()
    )
    with FakeLlm(by_kind(summary=lambda b, n: {"summary": "Ok."})) as fake:
        m.options["llm"]["base_url"] = fake.url
        reduce.run(m)
    assert not (m.work_dir / "draft.new.json").exists()
    assert len(proposals(m)) == 1
