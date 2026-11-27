"""Extraction scoring: item matching, schema validity and quote validity."""

from __future__ import annotations

from collections import Counter
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


def _owner(item: Item) -> tuple[str | None, str | None]:
    name = item.get("assignee_name")
    norm_name = str(name).strip().casefold() if isinstance(name, str) and name.strip() else None
    return (item.get("assignee"), norm_name)


def _agrees(r: Item, p: Item) -> bool:
    return _owner(r) == _owner(p) and r.get("due") == p.get("due")


def _match(
    reference: Sequence[Item],
    predicted: Sequence[Item],
    threshold: float,
    prefer_agreement: bool = False,
) -> list[tuple[int, int]]:
    """Greedy one-to-one matching by descending text similarity.

    With `prefer_agreement`, pairs of equal similarity are ordered so that those that agree on
    assignee and due date are taken first. The default order of equal pairs is arbitrary (it
    follows the item order), which matters when a meeting repeats one task for several people.
    """
    pairs = sorted(
        (
            (_similarity(r["text"], p["text"]), prefer_agreement and _agrees(r, p), i, j)
            for i, r in enumerate(reference)
            for j, p in enumerate(predicted)
        ),
        reverse=True,
    )
    used_r: set[int] = set()
    used_p: set[int] = set()
    matches = []
    for score, _agree, i, j in pairs:
        if score < threshold:
            break
        if i in used_r or j in used_p:
            continue
        used_r.add(i)
        used_p.add(j)
        matches.append((i, j))
    return matches


def _field_accuracy(
    reference: Sequence[Item],
    predicted: Sequence[Item],
    matches: Sequence[tuple[int, int]],
    field: str,
) -> float:
    if field == "assignee":
        ok = sum(1 for i, j in matches if _owner(reference[i]) == _owner(predicted[j]))
    else:
        ok = sum(1 for i, j in matches if reference[i].get(field) == predicted[j].get(field))
    return ok / len(matches) if matches else 0.0


def score_extraction(
    reference: Sequence[Item], predicted: Sequence[Item], threshold: float = 0.5
) -> dict[str, float | int]:
    """Recall, precision, assignee accuracy and due-date accuracy for tasks.

    A predicted task matches a reference task when the token F1 of their texts
    reaches `threshold`; matching is one-to-one. Assignee and due-date accuracy
    are computed over matched pairs only. Items need `text`, optional
    `assignee` and `due`.

    Also reported, next to the figures above and never replacing them:
    `assignee_accuracy_paired` and `due_accuracy_paired`. Meetings repeat one task for several
    people ("order the module" for Ann and for Bob), and among such equal texts the plain
    matching pairs a predicted task with an arbitrary reference task, so a correct list of
    (task, assignee, due) triples can score low. The paired variant breaks ties of equal text
    similarity in favour of the pair that agrees on assignee and due date. The same
    threshold applies; recall and precision are those of the plain matching.
    """
    matches = _match(reference, predicted, threshold)
    paired = _match(reference, predicted, threshold, prefer_agreement=True)
    tp = len(matches)
    recall = tp / len(reference) if reference else 1.0
    precision = tp / len(predicted) if predicted else (1.0 if not reference else 0.0)
    return {
        "recall": recall,
        "precision": precision,
        "assignee_accuracy": _field_accuracy(reference, predicted, matches, "assignee"),
        "due_accuracy": _field_accuracy(reference, predicted, matches, "due"),
        "assignee_accuracy_paired": _field_accuracy(reference, predicted, paired, "assignee"),
        "due_accuracy_paired": _field_accuracy(reference, predicted, paired, "due"),
        "matched": tp,
        "reference": len(reference),
        "predicted": len(predicted),
    }


def score_by_owner_due(
    reference: Sequence[Item], predicted: Sequence[Item]
) -> dict[str, float | int]:
    """Recall and precision of tasks identified by who does them and by when, not by wording.

    A report in one language may phrase a task differently from the reference (another
    inflection, a translation of a sentence spoken in the other language), which the token
    overlap of `score_extraction` counts as a miss. Here a predicted task matches a reference
    task when both have the same `assignee` and `due`; matching is one-to-one, so a task
    repeated for the same person and day must be predicted as often as it is annotated. This is
    a diagnostic next to `score_extraction`, never a replacement: it is blind to the task text
    and so more lenient than the text-based figures.
    """
    pool = Counter((_owner(r), r.get("due")) for r in reference)
    tp = 0
    for p in predicted:
        key = (_owner(p), p.get("due"))
        if pool[key] > 0:
            pool[key] -= 1
            tp += 1
    return {
        "recall": tp / len(reference) if reference else 1.0,
        "precision": tp / len(predicted) if predicted else (1.0 if not reference else 0.0),
        "matched": tp,
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
