"""Pure logic: pauses, turn layout on a shared time axis, mixing and per-speaker tracks."""

from __future__ import annotations

import random
from dataclasses import dataclass

import numpy as np

PAUSE_MIN_S = 0.2
PAUSE_MAX_S = 0.6


@dataclass(frozen=True)
class Segment:
    """One placed turn. Positions are in samples so that the axis never drifts."""

    turn: int
    speaker: str
    lang: str
    start: int
    end: int


def plan_pauses(n_turns: int, seed: int, sample_rate: int) -> list[int]:
    """Pause (in samples) after each turn except the last; deterministic for a seed."""
    rng = random.Random(f"pauses:{seed}")
    return [
        round(rng.uniform(PAUSE_MIN_S, PAUSE_MAX_S) * sample_rate)
        for _ in range(max(n_turns - 1, 0))
    ]


def layout(
    turns: list[dict], lengths: list[int], pauses: list[int], lead_in: int = 0
) -> list[Segment]:
    """Place turns one after another, separated by the given pauses (all in samples)."""
    if len(turns) != len(lengths):
        raise ValueError("turns and lengths differ in size")
    if len(pauses) != max(len(turns) - 1, 0):
        raise ValueError("expected one pause between each pair of consecutive turns")
    segments = []
    cursor = lead_in
    for i, (turn, length) in enumerate(zip(turns, lengths, strict=True)):
        segments.append(Segment(i, turn["speaker"], turn["lang"], cursor, cursor + length))
        cursor += length + (pauses[i] if i < len(pauses) else 0)
    return segments


def to_timeline(segments: list[Segment], sample_rate: int) -> list[dict]:
    """The `timeline.json` structure: seconds rounded to milliseconds."""
    return [
        {
            "turn": s.turn,
            "speaker": s.speaker,
            "lang": s.lang,
            "start_s": round(s.start / sample_rate, 3),
            "end_s": round(s.end / sample_rate, 3),
        }
        for s in segments
    ]


def mix(segments: list[Segment], waves: list[np.ndarray], total: int | None = None) -> np.ndarray:
    """Concatenate turns on the shared axis (silence in the gaps)."""
    end = max((s.end for s in segments), default=0)
    out = np.zeros(max(total or 0, end), dtype=np.float32)
    for seg, wave in zip(segments, waves, strict=True):
        out[seg.start : seg.end] = wave
    return out


def speaker_tracks(
    segments: list[Segment], waves: list[np.ndarray], total: int
) -> dict[str, np.ndarray]:
    """One full-length track per speaker: own turns on the shared axis, silence elsewhere."""
    tracks: dict[str, np.ndarray] = {}
    for seg, wave in zip(segments, waves, strict=True):
        track = tracks.setdefault(seg.speaker, np.zeros(total, dtype=np.float32))
        track[seg.start : seg.end] = wave
    return tracks
