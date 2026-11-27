import pytest

from bbb_bench.metrics import (
    quote_validity,
    schema_validity,
    score_by_owner_due,
    score_extraction,
)

REF = [
    {"text": "Design the monitor mount", "assignee": "alex", "due": "2026-11-20"},
    {"text": "Order the GNSS module", "assignee": "kasia", "due": "2026-11-25"},
    {"text": "Write the test plan", "assignee": "alex", "due": None},
    {"text": "Book the workshop room", "assignee": "jan", "due": "2026-12-01"},
]


def test_perfect_prediction():
    r = score_extraction(REF, REF)
    assert r["recall"] == 1.0 and r["precision"] == 1.0
    assert r["assignee_accuracy"] == 1.0 and r["due_accuracy"] == 1.0


def test_hand_computed_scores():
    # 3 of 4 reference tasks are found (recall 3/4); one hallucinated task among
    # 4 predictions (precision 3/4). Over the 3 matches the assignee is right
    # for 2 (the GNSS task went to alex) and the due date is right for 2
    # (the test plan got a date that was never stated).
    pred = [
        {"text": "Design the monitor mount", "assignee": "alex", "due": "2026-11-20"},
        {"text": "Order GNSS module", "assignee": "alex", "due": "2026-11-25"},
        {"text": "Write the test plan", "assignee": "alex", "due": "2026-11-30"},
        {"text": "Clean the whole garage thoroughly", "assignee": "jan", "due": None},
    ]
    r = score_extraction(REF, pred)
    assert r["recall"] == pytest.approx(3 / 4)
    assert r["precision"] == pytest.approx(3 / 4)
    assert r["assignee_accuracy"] == pytest.approx(2 / 3)
    assert r["due_accuracy"] == pytest.approx(2 / 3)


def test_matching_is_one_to_one():
    pred = [REF[0], dict(REF[0])]
    r = score_extraction(REF, pred)
    assert r["matched"] == 1
    assert r["precision"] == pytest.approx(0.5)


REPEATED = [
    {"text": "order the module", "assignee": "ann", "due": "2026-11-20"},
    {"text": "order the module", "assignee": "bob", "due": "2026-11-25"},
]


def test_equal_texts_are_paired_by_list_order_in_the_plain_metric():
    # Both predictions are right, but listed in the other order than the reference: the plain
    # matching pairs equal texts by position, so assignee and due look wrong ...
    pred = [REPEATED[1], REPEATED[0]]
    r = score_extraction(REPEATED, pred)
    assert r["recall"] == 1.0 and r["precision"] == 1.0
    assert r["assignee_accuracy"] == 0.0 and r["due_accuracy"] == 0.0
    # ... while the paired variant takes the pair that agrees.
    assert r["assignee_accuracy_paired"] == 1.0 and r["due_accuracy_paired"] == 1.0


def test_paired_accuracy_does_not_hide_real_mistakes():
    pred = [
        {"text": "order the module", "assignee": "ann", "due": "2026-11-20"},
        {"text": "order the module", "assignee": "cid", "due": "2026-11-30"},
    ]
    r = score_extraction(REPEATED, pred)
    assert r["assignee_accuracy_paired"] == 0.5 and r["due_accuracy_paired"] == 0.5
    assert r["matched"] == 2 and r["recall"] == 1.0


def test_paired_accuracy_follows_the_plain_one_without_repeats():
    r = score_extraction(REF, REF)
    assert r["assignee_accuracy_paired"] == r["assignee_accuracy"] == 1.0
    assert score_extraction(REF, [])["assignee_accuracy_paired"] == 0.0


def test_owner_and_due_match_ignores_the_wording():
    # A Polish noun form instead of the infinitive scores zero on the text overlap ...
    ref = [{"text": "zamówić moduł GNSS", "assignee": "zosia", "due": "2026-11-24"}]
    pred = [{"text": "Zamówienie modułu GNSS", "assignee": "zosia", "due": "2026-11-24"}]
    assert score_extraction(ref, pred)["recall"] == 0.0
    # ... but it is the same commitment by owner and date.
    r = score_by_owner_due(ref, pred)
    assert r["recall"] == 1.0 and r["precision"] == 1.0 and r["matched"] == 1


def test_owner_and_due_match_is_one_to_one_and_needs_both_fields():
    ref = [
        {"text": "a", "assignee": "ann", "due": "2026-11-20"},
        {"text": "b", "assignee": "ann", "due": "2026-11-21"},
    ]
    pred = [
        {"text": "x", "assignee": "ann", "due": "2026-11-20"},
        {"text": "y", "assignee": "ann", "due": "2026-11-20"},  # the same pair again
        {"text": "z", "assignee": "bob", "due": "2026-11-21"},  # wrong owner
        {"text": "w", "assignee": "ann", "due": None},  # no date
    ]
    r = score_by_owner_due(ref, pred)
    assert r["matched"] == 1
    assert r["recall"] == 0.5 and r["precision"] == 0.25


def test_owner_and_due_match_of_empty_lists():
    assert score_by_owner_due([], [])["recall"] == 1.0
    assert score_by_owner_due(REF, [])["recall"] == 0.0
    assert score_by_owner_due([], REF)["precision"] == 0.0


def test_empty_prediction():
    r = score_extraction(REF, [])
    assert r["recall"] == 0.0 and r["precision"] == 0.0


SCHEMA = {
    "type": "object",
    "required": ["type", "text"],
    "properties": {"type": {"enum": ["task", "decision"]}, "text": {"type": "string"}},
}


def test_schema_validity():
    resp = [{"type": "task", "text": "x"}, {"type": "bogus", "text": "x"}, {"text": "y"}]
    r = schema_validity(resp, SCHEMA)
    assert r["valid"] == 1
    assert r["schema_validity"] == pytest.approx(1 / 3)


TRANSCRIPT = [
    {"t_start_ms": 0, "t_end_ms": 5000, "text": "Good morning everyone."},
    {
        "t_start_ms": 5000,
        "t_end_ms": 12000,
        "text": "Alex, can you have the mount ready by next Friday?",
    },
    {"t_start_ms": 300_000, "t_end_ms": 305_000, "text": "Totally unrelated sentence later."},
]


def _item(quote, start, end):
    return {"source": {"quote": quote, "t_start_ms": start, "t_end_ms": end}}


def test_quote_validity_exact_and_near_miss():
    items = [
        _item("Alex, can you have the mount ready by next Friday?", 5000, 12000),
        _item("Alex can you have the mount ready by next Friday", 4000, 11000),
    ]
    assert quote_validity(items, TRANSCRIPT)["quote_validity"] == 1.0


def test_quote_validity_rejects_invented_and_misplaced_quotes():
    items = [
        _item("We agreed to double the budget", 5000, 12000),  # invented
        _item("Alex, can you have the mount ready by next Friday?", 300_000, 305_000),  # wrong time
    ]
    assert quote_validity(items, TRANSCRIPT)["valid"] == 0


def test_outside_assignee_names_are_compared_case_and_space_insensitively():
    ref = [{"text": "Send the invoice", "assignee": None, "assignee_name": "Marek", "due": None}]
    same = [{"text": "Send the invoice", "assignee": None, "assignee_name": " marek ", "due": None}]
    other = [{"text": "Send the invoice", "assignee": None, "assignee_name": "Tomek", "due": None}]
    assert score_extraction(ref, same)["assignee_accuracy"] == 1.0
    assert score_extraction(ref, other)["assignee_accuracy"] == 0.0
    assert score_by_owner_due(ref, same)["recall"] == 1.0
    assert score_by_owner_due(ref, other)["recall"] == 0.0


def test_a_blank_assignee_name_equals_no_name():
    ref = [{"text": "Send the invoice", "assignee": None, "due": None}]
    blank = [{"text": "Send the invoice", "assignee": None, "assignee_name": "  ", "due": None}]
    assert score_extraction(ref, blank)["assignee_accuracy"] == 1.0
