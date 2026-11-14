"""Diarization error rate (no collar, overlap included)."""

from __future__ import annotations

from collections.abc import Iterable

from scipy.optimize import linear_sum_assignment

Segment = tuple[float, float, str]  # (start_s, end_s, speaker)


def _elementary(
    reference: Iterable[Segment], hypothesis: Iterable[Segment]
) -> list[tuple[float, frozenset[str], frozenset[str]]]:
    ref = [s for s in reference if s[1] > s[0]]
    hyp = [s for s in hypothesis if s[1] > s[0]]
    bounds = sorted({t for s in ref + hyp for t in (s[0], s[1])})
    out = []
    for a, b in zip(bounds, bounds[1:], strict=False):
        mid = (a + b) / 2
        r = frozenset(s[2] for s in ref if s[0] <= mid < s[1])
        h = frozenset(s[2] for s in hyp if s[0] <= mid < s[1])
        out.append((b - a, r, h))
    return out


def der(reference: Iterable[Segment], hypothesis: Iterable[Segment]) -> dict[str, float]:
    """Diarization error rate with an optimal one-to-one speaker mapping.

    DER = (missed speech + false alarm + speaker confusion) / total reference
    speech, where overlapping speakers count once per speaker. The mapping from
    hypothesis to reference speakers maximizes the time they are active together.
    """
    pieces = _elementary(reference, hypothesis)
    ref_spk = sorted({s for _, r, _ in pieces for s in r})
    hyp_spk = sorted({s for _, _, h in pieces for s in h})

    overlap = [[0.0] * len(hyp_spk) for _ in ref_spk]
    for dur, r, h in pieces:
        for s in r:
            for t in h:
                overlap[ref_spk.index(s)][hyp_spk.index(t)] += dur
    mapping: dict[str, str] = {}
    if ref_spk and hyp_spk:
        rows, cols = linear_sum_assignment([[-v for v in row] for row in overlap])
        mapping = {hyp_spk[c]: ref_spk[r] for r, c in zip(rows, cols, strict=True)}

    total = miss = fa = conf = 0.0
    for dur, r, h in pieces:
        nr, nh = len(r), len(h)
        correct = len({mapping[t] for t in h if t in mapping} & r)
        total += dur * nr
        miss += dur * max(0, nr - nh)
        fa += dur * max(0, nh - nr)
        conf += dur * (min(nr, nh) - correct)
    if total == 0:
        raise ValueError("reference contains no speech")
    return {
        "der": (miss + fa + conf) / total,
        "missed": miss / total,
        "false_alarm": fa / total,
        "confusion": conf / total,
        "total_speech_s": total,
    }
