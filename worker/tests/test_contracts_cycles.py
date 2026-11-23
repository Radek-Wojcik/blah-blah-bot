"""Contract tests for the Stage 4 schemas. The fixtures are shared with the Rust side."""

import copy
import json
from pathlib import Path

import jsonschema
import pytest

from bbb_worker import schema

FIXTURES = Path(__file__).parent / "fixtures"
NAMES = ("extraction", "meeting", "cycle-context")


def fixture(name):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


@pytest.mark.parametrize("name", NAMES)
def test_valid_fixture_is_accepted(name):
    schema.validate(name, fixture(f"{name}.valid.json"))


@pytest.mark.parametrize("name", NAMES)
def test_invalid_fixture_is_rejected(name):
    with pytest.raises(jsonschema.ValidationError):
        schema.validate(name, fixture(f"{name}.invalid.json"))


def mutated(name, mutate):
    doc = copy.deepcopy(fixture(f"{name}.valid.json"))
    mutate(doc)
    return doc


EXTRACTION_BREAKS = [
    lambda d: d.pop("summary"),
    lambda d: d.pop("status_changes"),
    lambda d: d["items"][0].pop("quote"),
    lambda d: d["items"][0].update(type="rumor"),
    lambda d: d["items"][0].update(due="next Friday"),
    lambda d: d["items"][0].update(time="12:34"),
    lambda d: d["items"][0]["certainty"].update(due="maybe"),
    lambda d: d["items"][0].update(text=""),
    lambda d: d["items"][0].update(extra=True),
]

MEETING_BREAKS = [
    lambda d: d.update(version=2),
    lambda d: d.update(language="de"),
    lambda d: d.update(mode="manual"),
    lambda d: d.pop("warnings"),
    lambda d: d["sections"][0].update(id="Bad Id"),
    lambda d: d["items"][0].pop("edited"),
    lambda d: d["items"][0]["source"].update(match=1.5),
    lambda d: d["items"][0]["source"].update(t_start_ms=-1),
    lambda d: d["warnings"][0].update(code="other"),
    lambda d: d["items"][0].update(id=""),
]

CONTEXT_BREAKS = [
    lambda d: d.update(output_language="de"),
    lambda d: d.update(meeting_date="16.11.2026"),
    lambda d: d["participants"][0].pop("name"),
    lambda d: d["glossary"][0].pop("term"),
    lambda d: d["sections"][0].update(id="Hardware Parts"),
    lambda d: d["previous_items"][0].update(type="rumor"),
    lambda d: d["open_issues"][0].pop("title"),
    lambda d: d.pop("previous_items"),
]


VIOLATIONS = [
    *[("extraction", m) for m in EXTRACTION_BREAKS],
    *[("meeting", m) for m in MEETING_BREAKS],
    *[("cycle-context", m) for m in CONTEXT_BREAKS],
]


@pytest.mark.parametrize(("name", "mutate"), VIOLATIONS)
def test_each_violation_is_rejected(name, mutate):
    with pytest.raises(jsonschema.ValidationError):
        schema.validate(name, mutated(name, mutate))


def test_optional_fields_may_be_missing():
    ctx = fixture("cycle-context.valid.json")
    del ctx["glossary"][0]["hint"]
    del ctx["open_issues"][0]["assignee"]
    schema.validate("cycle-context", ctx)
    meeting = fixture("meeting.valid.json")
    del meeting["warnings"][0]["t_start_ms"]
    schema.validate("meeting", meeting)


def test_a_hand_added_item_has_no_source():
    assert fixture("meeting.valid.json")["items"][1]["source"] is None


# ----- Stage 5 additions: all optional, the Stage 4 fixtures above stay valid ------------

STAGE5 = (
    ("extraction", "extraction.status.valid.json"),
    ("meeting", "meeting.proposals.valid.json"),
    ("cycle-context", "cycle-context.issue.valid.json"),
    ("cycle-context", "cycle-context.corrections.valid.json"),
)


@pytest.mark.parametrize(("name", "file"), STAGE5)
def test_stage5_fixture_is_accepted(name, file):
    schema.validate(name, fixture(file))


def test_stage4_documents_have_none_of_the_new_fields():
    # They are what older versions wrote; they must keep validating without the additions.
    # (The model answer is the exception: `status_changes` is required in it, see below.)
    assert "status_proposals" not in fixture("meeting.valid.json")
    assert all("issue" not in p for p in fixture("cycle-context.valid.json")["previous_items"])
    assert "corrections" not in fixture("cycle-context.valid.json")


STATUS_CHANGE_BREAKS = [
    lambda d: d["status_changes"][0].pop("quote"),
    lambda d: d["status_changes"][0].pop("target"),
    lambda d: d["status_changes"][0].update(status="open"),
    lambda d: d["status_changes"][0].update(time="13:32"),
    lambda d: d["status_changes"][0].update(reason=""),
    lambda d: d["status_changes"][0].update(extra=True),
]

PROPOSAL_BREAKS = [
    lambda d: d["status_proposals"][0].update(id="p1"),
    lambda d: d["status_proposals"][0].update(status="closed"),
    lambda d: d["status_proposals"][0].update(decision="maybe"),
    lambda d: d["status_proposals"][0].update(target=""),
    lambda d: d["status_proposals"][0].pop("edited"),
    lambda d: d["status_proposals"][0]["source"].update(match=2),
    lambda d: d["status_proposals"][0].update(extra=True),
]

ISSUE_BREAKS = [
    lambda d: d["previous_items"][0].update(issue=14),
    lambda d: d["previous_items"][0].update(extra="x"),
]

CORRECTION_BREAKS = [
    lambda d: d["corrections"][0].pop("text"),
    lambda d: d["corrections"][0].pop("date"),
    lambda d: d["corrections"][0].update(issue=12),
    lambda d: d["corrections"][0].update(extra="x"),
    lambda d: d.update(corrections={"text": "x"}),
]

STAGE5_VIOLATIONS = [
    *[("extraction", "extraction.status.valid.json", m) for m in STATUS_CHANGE_BREAKS],
    *[("meeting", "meeting.proposals.valid.json", m) for m in PROPOSAL_BREAKS],
    *[("cycle-context", "cycle-context.issue.valid.json", m) for m in ISSUE_BREAKS],
    *[("cycle-context", "cycle-context.corrections.valid.json", m) for m in CORRECTION_BREAKS],
]


@pytest.mark.parametrize(("name", "file", "mutate"), STAGE5_VIOLATIONS)
def test_each_stage5_violation_is_rejected(name, file, mutate):
    doc = copy.deepcopy(fixture(file))
    mutate(doc)
    with pytest.raises(jsonschema.ValidationError):
        schema.validate(name, doc)


def test_the_model_answer_must_carry_status_changes_but_it_may_be_empty():
    doc = fixture("extraction.valid.json")
    assert doc["status_changes"] == []
    schema.validate("extraction", doc)
    del doc["status_changes"]
    with pytest.raises(jsonschema.ValidationError, match="status_changes"):
        schema.validate("extraction", doc)


def test_a_hand_added_proposal_has_no_source_or_comment():
    proposals = fixture("meeting.proposals.valid.json")["status_proposals"]
    assert proposals[1]["source"] is None and proposals[1]["comment"] is None
