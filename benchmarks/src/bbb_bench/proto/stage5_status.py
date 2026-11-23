r"""Stage 5: quality of the status proposals (`status_changes`) of the production extract step.

Runs the worker exactly as the queue does (see `stage4_extraction`) on twelve short synthetic
meetings, six English and six Polish (`datasets/status_scenarios.py`). Every meeting has open
issues of earlier meetings in its context, and its transcript says that some of them are done and
that others are blocked, reopened, in progress or not mentioned. Only "done" is a status the
model may propose, so every proposal for another target is a false positive:

    uv run python -m bbb_bench.proto.stage5_status \
        --model path/to/model.gguf --name gemma-4-12b-q4 --llama-server path/to/llama-server.exe

writes `results/stage5-status-<name>.json`. Scores are taken from what the user would see in
the draft (`status_proposals` after `reduce`); the raw answers of the model (`windows.json`) are
used for the share of invented targets.

An already running OpenAI-compatible server is used instead of a managed llama-server with the
hidden option `--external-url http://host:port/v1` (tests, development).
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Sequence
from contextlib import ExitStack
from typing import Any

from .. import harness
from ..datasets.status_scenarios import DONE, NOT_DONE_KINDS, SCENARIOS, expected_done
from ..metrics import quote_validity
from .stage4_extraction import (
    Execution,
    build_context,
    build_transcript,
    config_of,
    execute,
    make_llm,
    make_runner,
    mode_context,
    schema_errors,
    window_stats,
    work_root,
)

PROTOTYPE = "stage5-status"
ISSUE_NUMBER = re.compile(r"\d+")

# Proposed for this measurement, not gated: there is no threshold policy for status proposals
# yet (docs/TESTING.md, section 5). Precision matters most, a wrong proposal costs a click to
# reject, a missed one costs nothing but the user's own decision.
PROPOSED_THRESHOLDS: dict[str, float] = {
    "proposal_precision": 0.90,
    "proposal_recall": 0.80,
    "evidence_verified_share": 0.95,
    "schema_validity": 1.0,
    "negative_meeting_false_positive_rate": 0.0,
}


def context_of(scenario: dict[str, Any]) -> dict[str, Any]:
    """`work/context.json` of a scenario: participants, open issues and topics, earlier items."""
    context = build_context(
        speakers=scenario["speakers"],
        meeting_date=scenario["meeting_date"],
        output_language=scenario["lang"],
    )
    context["open_issues"] = [dict(i) for i in scenario["open_issues"]]
    context["open_topics"] = [dict(t) for t in scenario["open_topics"]]
    context["previous_items"] = [dict(p) for p in scenario["previous_items"]]
    return mode_context(context, "fixed")


def listed_targets(scenario: dict[str, Any]) -> dict[str, str]:
    """Every id a proposal may name (open issues, issues of earlier items, topics), by the
    lower-case key. The same rule as `reduce` applies."""
    ids = [i["id"] for i in scenario["open_issues"]]
    ids += [p["issue"] for p in scenario["previous_items"] if p.get("issue")]
    ids += [t["id"] for t in scenario["open_topics"]]
    return {i.casefold(): i for i in ids}


def resolve_target(value: str, targets: dict[str, str]) -> str | None:
    wanted = value.strip()
    if ISSUE_NUMBER.fullmatch(wanted):
        wanted = f"#{wanted}"
    return targets.get(wanted.casefold())


def all_targets(scenario: dict[str, Any]) -> list[str]:
    """Targets the transcript has a label for."""
    return list(scenario["expect"])


def score_scenario(
    scenario: dict[str, Any],
    windows_doc: dict[str, Any] | None,
    draft: dict[str, Any] | None,
    segments: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    """Scores of one meeting. `segments` are transcript segments (`start_ms`, `end_ms`, `text`)."""
    expected = expected_done(scenario)
    targets = listed_targets(scenario)
    proposals = list((draft or {}).get("status_proposals", []))
    predicted = {p["target"] for p in proposals}
    tp = sorted(predicted & expected)
    fp = sorted(predicted - expected)
    fn = sorted(expected - predicted)
    raw = [c for w in (windows_doc or {}).get("windows", []) for c in w.get("status_changes", [])]
    raw_unknown = [c["target"] for c in raw if resolve_target(c["target"], targets) is None]
    entries = [
        {"t_start_ms": s["start_ms"], "t_end_ms": s["end_ms"], "text": s["text"]} for s in segments
    ]
    sourced = [p for p in proposals if p.get("source")]
    valid = (
        quote_validity([{"source": p["source"]} for p in sourced], entries)["valid"]
        if sourced
        else 0
    )
    on_turn = 0
    for p in proposals:
        turn = scenario["evidence"].get(p["target"])
        if (
            turn is not None
            and p.get("source")
            and p["source"]["t_start_ms"] == (segments[turn]["start_ms"])
        ):
            on_turn += 1
    return {
        "scenario": scenario["id"],
        "lang": scenario["lang"],
        "negative": not expected,
        "expected": sorted(expected),
        "proposed": sorted(predicted),
        "true_positives": tp,
        "false_positives": [
            {"target": t, "kind": scenario["expect"].get(t, "unlisted")} for t in fp
        ],
        "false_negatives": fn,
        "raw_changes": len(raw),
        "raw_unknown_targets": raw_unknown,
        "proposals": [
            {
                "target": p["target"],
                "reason": p["reason"],
                "quote": (p.get("source") or {}).get("quote"),
                "verified": bool((p.get("source") or {}).get("verified")),
            }
            for p in proposals
        ],
        "evidence_sourced": len(sourced),
        "evidence_verified": sum(1 for p in sourced if p["source"]["verified"]),
        "evidence_valid": valid,
        "evidence_on_expected_turn": on_turn,
    }


def ratio(numerator: float, denominator: float, empty: float = 0.0) -> float:
    return numerator / denominator if denominator else empty


def aggregate_scores(records: Sequence[dict[str, Any]]) -> dict[str, Any]:
    """Micro-averaged scores over the meetings that ran, with the false positives by kind."""
    ok = [r for r in records if r.get("ok")]
    tp = sum(len(r["true_positives"]) for r in ok)
    fp = sum(len(r["false_positives"]) for r in ok)
    fn = sum(len(r["false_negatives"]) for r in ok)
    precision = ratio(tp, tp + fp, 1.0)
    recall = ratio(tp, tp + fn, 1.0)
    negatives = [r for r in ok if r["negative"]]
    not_done = sum(1 for r in ok for k in r["labels"].values() if k != DONE)
    sourced = sum(r["evidence_sourced"] for r in ok)
    expected_turns = sum(r["evidence_expected"] for r in ok)
    by_kind: dict[str, dict[str, int]] = {}
    for kind in (*NOT_DONE_KINDS, "unlisted"):
        count = sum(1 for r in ok for k in r["labels"].values() if k == kind)
        wrong = sum(1 for r in ok for f in r["false_positives"] if f["kind"] == kind)
        if count or wrong:
            by_kind[kind] = {"targets": count, "proposed_as_done": wrong}
    raw = sum(r["raw_changes"] for r in ok)
    windows = sum(r["windows"] for r in ok)
    skipped = sum(r["windows_skipped"] for r in ok)
    by_lang: dict[str, dict[str, float]] = {}
    for lang in sorted({r["lang"] for r in ok}):
        sub = [r for r in ok if r["lang"] == lang]
        ltp = sum(len(r["true_positives"]) for r in sub)
        lfp = sum(len(r["false_positives"]) for r in sub)
        lfn = sum(len(r["false_negatives"]) for r in sub)
        by_lang[lang] = {
            "proposal_precision": ratio(ltp, ltp + lfp, 1.0),
            "proposal_recall": ratio(ltp, ltp + lfn, 1.0),
            "true_positives": ltp,
            "false_positives": lfp,
            "false_negatives": lfn,
        }
    out: dict[str, Any] = {
        "meetings_ok": len(ok),
        "meetings_failed": len(records) - len(ok),
        "targets_done": tp + fn,
        "targets_not_done": not_done,
        "true_positives": tp,
        "false_positives": fp,
        "false_negatives": fn,
        "proposal_precision": precision,
        "proposal_recall": recall,
        "proposal_f1": ratio(2 * precision * recall, precision + recall),
        "false_positive_rate_per_open_target": ratio(fp, not_done),
        "negative_meetings": len(negatives),
        "negative_meeting_false_positive_rate": ratio(
            sum(1 for r in negatives if r["false_positives"]), len(negatives)
        ),
        "false_positives_by_kind": by_kind,
        "proposals": tp + fp,
        "evidence_verified_share": ratio(sum(r["evidence_verified"] for r in ok), sourced),
        "evidence_quote_validity": ratio(sum(r["evidence_valid"] for r in ok), sourced),
        "evidence_on_expected_turn": ratio(
            sum(r["evidence_on_expected_turn"] for r in ok), expected_turns
        ),
        "raw_changes": raw,
        "raw_unknown_target_rate": ratio(sum(len(r["raw_unknown_targets"]) for r in ok), raw),
        "schema_validity": ratio(windows - skipped, windows),
        "drafts_schema_valid": ratio(sum(1 for r in ok if r["draft_valid"]), len(ok)),
        "windows_retried": sum(r["windows_retried"] for r in ok),
        "by_language": by_lang,
    }
    out["meets_proposed_thresholds"] = {
        k: (out[k] <= v if k == "negative_meeting_false_positive_rate" else out[k] >= v)
        for k, v in PROPOSED_THRESHOLDS.items()
    }
    return out


def scenario_record(
    scenario: dict[str, Any], run: Execution, segments: Sequence[dict[str, Any]]
) -> dict[str, Any]:
    stats = window_stats(run.windows_doc)
    out: dict[str, Any] = {
        "ok": run.ok,
        "error": run.error,
        "labels": dict(scenario["expect"]),
        "seconds": round(run.seconds, 1),
        "windows": stats["windows"],
        "windows_skipped": stats["windows_skipped"],
        "windows_retried": stats["windows_retried"],
        "draft_valid": False,
        "draft_errors": [],
        "evidence_expected": sum(1 for t in expected_done(scenario) if t in scenario["evidence"]),
    }
    if run.ok and run.draft is not None:
        problems = schema_errors("meeting", run.draft)
        out |= {"draft_valid": not problems, "draft_errors": problems}
        out |= score_scenario(scenario, run.windows_doc, run.draft, segments)
    else:
        out |= {
            "scenario": scenario["id"],
            "lang": scenario["lang"],
            "negative": not expected_done(scenario),
        }
    return out


def run(args: argparse.Namespace) -> dict[str, Any]:
    runner, llm = make_runner(args), make_llm(args)
    chosen = [s for s in SCENARIOS if not args.only or s["id"] in args.only]
    if not chosen:
        raise ValueError(f"no scenario matches {args.only}")
    records = []
    with ExitStack() as stack:
        root = work_root(stack, args)
        for scenario in chosen:
            script = {"speakers": scenario["speakers"], "turns": scenario["turns"]}
            transcript = build_transcript(script, f"stage5-{scenario['id']}", scenario["lang"])
            result = execute(runner, root / scenario["id"], transcript, context_of(scenario), llm)
            record = scenario_record(scenario, result, transcript["segments"])
            records.append(record)
            print(
                f"{scenario['id']}: {'ok' if result.ok else 'FAILED ' + result.error} "
                f"proposed={record.get('proposed')} expected={record.get('expected')} "
                f"{result.seconds:.0f}s",
                file=sys.stderr,
            )
    if not any(r["ok"] for r in records):
        raise RuntimeError(f"no meeting could be processed: {records[0]['error']}")
    return {
        "config": {**config_of(args, llm), "scenarios": len(chosen)},
        "metric_notes": {
            "scores": "micro-averaged over the proposals of the draft (status_proposals after "
            "reduce); only 'done' is a status the model may propose, so a proposal for a "
            "blocked, reopened, in progress, not mentioned or unlisted target is a false positive",
            "evidence_verified_share": "share of proposals whose quote the worker found in the "
            "transcript (source.verified)",
            "evidence_quote_validity": "the extraction quote_validity metric over the proposals",
            "evidence_on_expected_turn": "share of the expected proposals whose quote sits on "
            "the turn that says the work is done",
            "raw_unknown_target_rate": "share of the model's raw status changes whose target is "
            "not on any list of the context (reduce drops them)",
            "schema_validity": "share of windows with a valid answer within the retries",
            "proposed_thresholds": "proposed for this measurement and not gated; "
            "there is no threshold policy for status proposals yet",
        },
        "proposed_thresholds": PROPOSED_THRESHOLDS,
        "meetings": records,
        **aggregate_scores(records),
    }


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    p.add_argument("--model", help="Path to a GGUF file (managed llama-server)")
    p.add_argument("--name", required=True, help="Run name used in the result file name")
    p.add_argument("--ctx", type=int, default=16384)
    p.add_argument("--worker-python", default=None, help="Default: worker/.venv interpreter")
    p.add_argument("--llama-server", default=None, help="Path of llama-server (BBB_LLAMA_SERVER)")
    p.add_argument("--only", nargs="+", default=None, help="Run only these scenario ids")
    p.add_argument("--work", default=None, help="Keep the meeting folders here")
    p.add_argument("--external-url", default=None, help=argparse.SUPPRESS)
    p.add_argument("--external-model", default="external", help=argparse.SUPPRESS)
    return p


def main(argv: Sequence[str] | None = None) -> None:
    p = build_parser()
    args = p.parse_args(argv)
    if not args.model and not args.external_url:
        p.error("--model is required (or the hidden --external-url)")
    # the shared helpers read these from the Stage 4 namespace
    args.mode = "fixed"
    args.output_language = "annotation"
    harness.run_prototype(PROTOTYPE, args.name, lambda: run(args))


if __name__ == "__main__":
    main()
