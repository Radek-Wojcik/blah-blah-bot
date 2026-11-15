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
