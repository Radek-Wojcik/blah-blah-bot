"""ASR metrics: WER, term recall, translation rate, RTFx."""

from __future__ import annotations

from collections.abc import Sequence

from .text import normalize


def _edit_counts(ref: Sequence[str], hyp: Sequence[str]) -> tuple[int, int, int]:
    """Return (substitutions, deletions, insertions) for a minimal alignment."""
    n, m = len(ref), len(hyp)
    # Each cell holds (total, subs, dels, ins).
    prev = [(j, 0, 0, j) for j in range(m + 1)]
    for i in range(1, n + 1):
        cur = [(i, 0, i, 0)]
        for j in range(1, m + 1):
            if ref[i - 1] == hyp[j - 1]:
                cur.append(prev[j - 1])
                continue
            sub = prev[j - 1]
            dele = prev[j]
            ins = cur[j - 1]
            cur.append(
                min(
                    (sub[0] + 1, sub[1] + 1, sub[2], sub[3]),
                    (dele[0] + 1, dele[1], dele[2] + 1, dele[3]),
                    (ins[0] + 1, ins[1], ins[2], ins[3] + 1),
                )
            )
        prev = cur
    _, s, d, ins = prev[m]
    return s, d, ins


def wer(reference: str, hypothesis: str) -> dict[str, float | int]:
    """Word error rate after normalization.

    Returns the rate and its components. An empty reference yields a rate of
    0.0 for an empty hypothesis and 1.0 otherwise.
    """
    ref, hyp = normalize(reference), normalize(hypothesis)
    s, d, i = _edit_counts(ref, hyp)
    n = len(ref)
    if n == 0:
        rate = 0.0 if not hyp else 1.0
    else:
        rate = (s + d + i) / n
    return {"wer": rate, "substitutions": s, "deletions": d, "insertions": i, "ref_words": n}


def term_recall(terms: Sequence[str], hypothesis: str) -> dict[str, float | int]:
    """Share of glossary term occurrences that appear in the hypothesis.

    `terms` lists one entry per planted occurrence (repeat a term to count it
    twice). A term counts as found if its normalized word sequence appears in
    the hypothesis as many times as it was planted.
    """
    hyp_joined = " " + " ".join(normalize(hypothesis)) + " "
    needed: dict[str, int] = {}
    for term in terms:
        key = " ".join(normalize(term))
        needed[key] = needed.get(key, 0) + 1
    found = 0
    total = 0
    for key, count in needed.items():
        have = hyp_joined.count(f" {key} ") if key else 0
        found += min(have, count)
        total += count
    return {"term_recall": (found / total) if total else 1.0, "found": found, "total": total}


def translation_rate(spoken_langs: Sequence[str], output_langs: Sequence[str]) -> dict[str, float]:
    """Share of segments output in a language other than the spoken one.

    Both sequences hold one language code per segment. Segment language ids
    come from a detector chosen by the harness; the metric only compares them.
    """
    if len(spoken_langs) != len(output_langs):
        raise ValueError("spoken_langs and output_langs must have equal length")
    if not spoken_langs:
        return {"translation_rate": 0.0}
    wrong = sum(1 for a, b in zip(spoken_langs, output_langs, strict=True) if a != b)
    return {"translation_rate": wrong / len(spoken_langs)}


def rtfx(audio_seconds: float, processing_seconds: float) -> float:
    """Audio duration divided by processing time."""
    if processing_seconds <= 0:
        raise ValueError("processing_seconds must be positive")
    return audio_seconds / processing_seconds
