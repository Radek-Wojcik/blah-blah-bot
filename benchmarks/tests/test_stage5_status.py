"""Stage 5 status proposal measurement: the scenario set and the scoring. CPU only, no model."""

from __future__ import annotations

from typing import Any

from bbb_bench.datasets.status_scenarios import DONE, NOT_DONE_KINDS, SCENARIOS, expected_done
from bbb_bench.proto import stage5_status as s5
from bbb_bench.proto.stage4_extraction import build_transcript, schema_errors


def by_id(scenario_id: str) -> dict[str, Any]:
    return next(s for s in SCENARIOS if s["id"] == scenario_id)


def segments_of(scenario: dict[str, Any]) -> list[dict[str, Any]]:
    script = {"speakers": scenario["speakers"], "turns": scenario["turns"]}
    return build_transcript(script, scenario["id"], scenario["lang"])["segments"]


def proposal(scenario: dict[str, Any], target: str, turn: int, verified: bool = True):
    seg = segments_of(scenario)[turn]
    return {
        "id": "s1",
        "target": target,
        "status": "done",
        "reason": "said so",
        "comment": None,
        "source": {
            "track": "meet",
            "speaker": seg["speaker"],
            "t_start_ms": seg["start_ms"],
            "t_end_ms": seg["end_ms"],
            "quote": seg["text"],
            "verified": verified,
            "match": 1.0,
        },
        "decision": "proposed",
        "edited": False,
    }


def test_the_set_is_small_balanced_and_in_two_languages():
    assert len(SCENARIOS) == 12
    assert {s["lang"] for s in SCENARIOS} == {"pl", "en"}
    assert len({s["id"] for s in SCENARIOS}) == 12
    labels = [k for s in SCENARIOS for k in s["expect"].values()]
    assert set(labels) <= {DONE, *NOT_DONE_KINDS}
    assert labels.count(DONE) == 14
    for kind in NOT_DONE_KINDS:
        assert kind in labels
    assert sum(1 for s in SCENARIOS if not expected_done(s)) == 2


def test_every_label_names_a_target_of_the_context_and_every_context_is_valid():
    for s in SCENARIOS:
        assert schema_errors("cycle-context", s5.context_of(s)) == []
        targets = s5.listed_targets(s)
        for target in s["expect"]:
            assert s5.resolve_target(target, targets) == target, (s["id"], target)


def test_the_evidence_turn_of_a_done_target_exists_and_nothing_else_has_one():
    for s in SCENARIOS:
        assert set(s["evidence"]) <= expected_done(s)
        for target in expected_done(s):
            assert 0 <= s["evidence"][target] < len(s["turns"])


def test_targets_resolve_like_reduce_does():
    targets = s5.listed_targets(by_id("en-s5"))
    assert s5.resolve_target("61", targets) == "#61"
    assert s5.resolve_target("#61", targets) == "#61"
    assert s5.resolve_target("T4", targets) == "t4"
    assert s5.resolve_target("#99", targets) is None
    # an issue known only from an earlier item is a target too
    assert s5.resolve_target("#74", s5.listed_targets(by_id("en-s6"))) == "#74"


def test_a_perfect_answer_scores_one_and_a_wrong_one_is_counted_by_kind():
    s = by_id("en-s5")
    segs = segments_of(s)
    good = {
        "status_proposals": [proposal(s, "#61", 1), proposal(s, "#62", 3), proposal(s, "t4", 6)]
    }
    scored = s5.score_scenario(s, {"windows": []}, good, segs)
    assert scored["true_positives"] == ["#61", "#62", "t4"]
    assert scored["false_positives"] == [] and scored["false_negatives"] == []
    assert scored["evidence_on_expected_turn"] == 3 and scored["evidence_valid"] == 3

    bad = {"status_proposals": [proposal(s, "#61", 1), proposal(s, "#63", 5, verified=False)]}
    scored = s5.score_scenario(s, {"windows": []}, bad, segs)
    assert scored["false_positives"] == [{"target": "#63", "kind": "blocked"}]
    assert scored["false_negatives"] == ["#62", "t4"]
    assert scored["evidence_verified"] == 1


def test_raw_targets_that_are_on_no_list_are_counted():
    s = by_id("en-s4")
    windows = {"windows": [{"status_changes": [{"target": "#51"}, {"target": "the newsletter"}]}]}
    scored = s5.score_scenario(s, windows, {"status_proposals": []}, segments_of(s))
    assert scored["raw_changes"] == 2
    assert scored["raw_unknown_targets"] == ["the newsletter"]


def record(scenario_id: str, proposals: list[tuple[str, int]]) -> dict[str, Any]:
    s = by_id(scenario_id)
    draft = {"status_proposals": [proposal(s, t, turn) for t, turn in proposals]}
    scored = s5.score_scenario(s, {"windows": []}, draft, segments_of(s))
    return {
        "ok": True,
        "labels": dict(s["expect"]),
        "windows": 1,
        "windows_skipped": 0,
        "windows_retried": 0,
        "draft_valid": True,
        "evidence_expected": sum(1 for t in expected_done(s) if t in s["evidence"]),
        **scored,
    }


def test_aggregate_scores_and_the_negative_meeting_rate():
    records = [
        record("en-s5", [("#61", 1), ("#62", 3), ("t4", 6)]),
        record("en-s1", [("#21", 1), ("#22", 3)]),
        record("en-s4", [("#52", 0)]),
        record("pl-s4", []),
    ]
    agg = s5.aggregate_scores(records)
    assert (
        agg["true_positives"] == 4 and agg["false_positives"] == 2 and agg["false_negatives"] == 0
    )
    assert agg["proposal_recall"] == 1.0
    assert abs(agg["proposal_precision"] - 4 / 6) < 1e-9
    assert agg["negative_meetings"] == 2
    assert agg["negative_meeting_false_positive_rate"] == 0.5
    assert agg["false_positives_by_kind"]["in_progress"] == {"targets": 7, "proposed_as_done": 1}
    assert agg["false_positives_by_kind"]["untouched"]["proposed_as_done"] == 1
    assert agg["meets_proposed_thresholds"]["proposal_precision"] is False
    assert agg["schema_validity"] == 1.0
