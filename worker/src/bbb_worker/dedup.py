"""Deduplication of items found by overlapping windows.

Two items are the same when they come from different windows (or parts of a split window),
have the same type, say nearly the same thing, are close in time and do not disagree on the
assignee or the due date (two different non-null values mean two different tasks; a null on
one side is filled from the other). Windows overlap by a minute, so a statement near a cut is
extracted twice; items of one window are never merged with each other, not even through a
third item, because a window can legitimately contain two similar items.

Items are the raw dictionaries of `windows.json`: `type`, `text`, `assignee`, `due`,
`certainty`, `window` and `source` (`t_start_ms`, `t_end_ms`, `verified`, `match`).
"""

from __future__ import annotations

from difflib import SequenceMatcher
from typing import Any

from .quotes import normalize

SIMILARITY = 0.8
MAX_GAP_MS = 3 * 60_000
# Containment only counts for a phrase of at least this many words ("fix it" is in many items).
MIN_CONTAINED_WORDS = 3

_RANK = {"stated": 2, "inferred": 1, "unknown": 0}


def similar(a: str, b: str) -> bool:
    na, nb = normalize(a), normalize(b)
    if not na or not nb:
        return False
    if SequenceMatcher(None, na, nb, autojunk=False).ratio() >= SIMILARITY:
        return True
    short, long_ = sorted((na, nb), key=len)
    return len(short.split()) >= MIN_CONTAINED_WORDS and f" {short} " in f" {long_} "


def close_in_time(a: dict[str, Any], b: dict[str, Any]) -> bool:
    sa, sb = a["source"], b["source"]
    return (
        sa["t_start_ms"] <= sb["t_end_ms"] + MAX_GAP_MS
        and sb["t_start_ms"] <= sa["t_end_ms"] + MAX_GAP_MS
    )


def conflicting(a: dict[str, Any], b: dict[str, Any]) -> bool:
    """Two different non-null assignees or due dates make two different tasks ("send the report"
    for Alex and for Bea). A null on either side does not conflict: the other value is kept."""
    return any(
        a[field] is not None and b[field] is not None and a[field] != b[field]
        for field in ("assignee", "due")
    )


def is_duplicate(a: dict[str, Any], b: dict[str, Any]) -> bool:
    return (
        (a["window"], a.get("part", "")) != (b["window"], b.get("part", ""))
        and a["type"] == b["type"]
        and not conflicting(a, b)
        and close_in_time(a, b)
        and similar(a["text"], b["text"])
    )


def _completeness(item: dict[str, Any]) -> tuple[bool, int, float, int]:
    filled = (item["assignee"] is not None) + (item["due"] is not None)
    return (item["source"]["verified"], filled, item["source"]["match"], len(item["text"]))


def best_certainty(a: str, b: str) -> str:
    return a if _RANK[a] >= _RANK[b] else b


def _merge_field(winner: dict[str, Any], other: dict[str, Any], field: str) -> None:
    if winner[field] is None and other[field] is not None:
        winner[field] = other[field]
        winner["certainty"][field] = other["certainty"][field]
    elif winner[field] is not None and winner[field] == other[field]:
        winner["certainty"][field] = best_certainty(
            winner["certainty"][field], other["certainty"][field]
        )


def merge_group(group: list[dict[str, Any]]) -> dict[str, Any]:
    """One item out of duplicates: the verified, more complete one wins and takes over what
    it lacks (assignee, due date) from the others."""
    ranked = sorted(group, key=_completeness, reverse=True)
    winner = {
        **ranked[0],
        "certainty": dict(ranked[0]["certainty"]),
        "source": dict(ranked[0]["source"]),
    }
    for other in ranked[1:]:
        _merge_field(winner, other, "assignee")
        _merge_field(winner, other, "due")
        if winner["relates_to"] is None:
            winner["relates_to"] = other["relates_to"]
        if winner["section"] is None:
            winner["section"] = other["section"]
    return winner


def _can_join(group_a: list[dict[str, Any]], group_b: list[dict[str, Any]]) -> bool:
    """Groups are joined only when nothing in them conflicts and no window would contribute
    two items: a chain a~b~c must not merge a and c when they differ (a null in b links them)."""
    places = {(it["window"], it.get("part", "")) for it in group_a}
    if any((it["window"], it.get("part", "")) in places for it in group_b):
        return False
    return not any(conflicting(x, y) for x in group_a for y in group_b)


def _groups(items: list[dict[str, Any]]) -> list[list[dict[str, Any]]]:
    """Union of duplicate pairs; `items` must be sorted by start time."""
    parent = list(range(len(items)))
    members: dict[int, list[dict[str, Any]]] = {i: [it] for i, it in enumerate(items)}

    def find(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i, a in enumerate(items):
        for j in range(i + 1, len(items)):
            b = items[j]
            if b["source"]["t_start_ms"] > a["source"]["t_end_ms"] + MAX_GAP_MS:
                break
            ri, rj = find(i), find(j)
            if ri != rj and is_duplicate(a, b) and _can_join(members[ri], members[rj]):
                parent[rj] = ri
                members[ri] += members.pop(rj)
    return list(members.values())


def merge_duplicates(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Items ordered by time with duplicates collapsed."""
    ordered = sorted(items, key=lambda it: (it["source"]["t_start_ms"], it["source"]["t_end_ms"]))
    merged = [merge_group(g) for g in _groups(ordered)]
    return sorted(merged, key=lambda it: (it["source"]["t_start_ms"], it["source"]["t_end_ms"]))
