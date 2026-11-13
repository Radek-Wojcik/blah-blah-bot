"""Extraction scoring: item matching, schema validity and quote validity."""

from __future__ import annotations

from collections.abc import Sequence
from difflib import SequenceMatcher
from typing import Any

import jsonschema

from .text import normalize

Item = dict[str, Any]


def _similarity(a: str, b: str) -> float:
    """Token-level F1 overlap between two normalized texts."""
    ta, tb = normalize(a), normalize(b)
    if not ta or not tb:
        return 0.0
    common = 0
    pool = list(tb)
    for tok in ta:
        if tok in pool:
            pool.remove(tok)
            common += 1
    if common == 0:
        return 0.0
    p, r = common / len(tb), common / len(ta)
    return 2 * p * r / (p + r)


def _match(
    reference: Sequence[Item], predicted: Sequence[Item], threshold: float
) -> list[tuple[int, int]]:
    """Greedy one-to-one matching by descending text similarity."""
    pairs = sorted(
        (
            (_similarity(r["text"], p["text"]), i, j)
            for i, r in enumerate(reference)
            for j, p in enumerate(predicted)
        ),
        reverse=True,
    )
    used_r: set[int] = set()
    used_p: set[int] = set()
    matches = []
    for score, i, j in pairs:
        if score < threshold:
            break
        if i in used_r or j in used_p:
            continue
        used_r.add(i)
        used_p.add(j)
        matches.append((i, j))
    return matches


def score_extraction(
    reference: Sequence[Item], predicted: Sequence[Item], threshold: float = 0.5
) -> dict[str, float | int]:
    """Recall, precision, assignee accuracy and due-date accuracy for tasks.

    A predicted task matches a reference task when the token F1 of their texts
    reaches `threshold`; matching is one-to-one. Assignee and due-date accuracy
    are computed over matched pairs only. Items need `text`, optional
    `assignee` and `due`.
    """
    matches = _match(reference, predicted, threshold)
    tp = len(matches)
    recall = tp / len(reference) if reference else 1.0
    precision = tp / len(predicted) if predicted else (1.0 if not reference else 0.0)
    assignee_ok = sum(
        1 for i, j in matches if reference[i].get("assignee") == predicted[j].get("assignee")
    )
    due_ok = sum(1 for i, j in matches if reference[i].get("due") == predicted[j].get("due"))
    return {
        "recall": recall,
        "precision": precision,
        "assignee_accuracy": assignee_ok / tp if tp else 0.0,
        "due_accuracy": due_ok / tp if tp else 0.0,
        "matched": tp,
        "reference": len(reference),
        "predicted": len(predicted),
    }


def schema_validity(responses: Sequence[Any], schema: dict[str, Any]) -> dict[str, float | int]:
    """Share of responses valid against a JSON Schema."""
    validator = jsonschema.Draft202012Validator(schema)
    valid = sum(1 for r in responses if validator.is_valid(r))
    return {"schema_validity": valid / len(responses) if responses else 0.0, "valid": valid}


def _contains_fuzzy(haystack: list[str], needle: list[str], min_ratio: float) -> bool:
    if not needle:
        return False
    n = len(needle)
    for width in {max(1, n - 1), n, n + 1}:
        for start in range(max(1, len(haystack) - width + 1)):
            window = haystack[start : start + width]
            if SequenceMatcher(None, window, needle).ratio() >= min_ratio:
                return True
    return False


def quote_validity(
    items: Sequence[Item],
    transcript: Sequence[dict[str, Any]],
    window_ms: int = 30_000,
    min_ratio: float = 0.85,
) -> dict[str, float | int]:
    """Share of items whose quote is found in the transcript near the stated time.

    `transcript` entries need `t_start_ms`, `t_end_ms` and `text`. Each item
    needs `source.quote`, `source.t_start_ms` and `source.t_end_ms`.
    """
    ok = 0
    for item in items:
        src = item["source"]
        lo, hi = src["t_start_ms"] - window_ms, src["t_end_ms"] + window_ms
        words: list[str] = []
        for seg in transcript:
            if seg["t_end_ms"] >= lo and seg["t_start_ms"] <= hi:
                words.extend(normalize(seg["text"]))
        if _contains_fuzzy(words, normalize(src["quote"]), min_ratio):
            ok += 1
    return {"quote_validity": ok / len(items) if items else 0.0, "valid": ok}
