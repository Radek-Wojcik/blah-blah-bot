"""Transcript lines and the windows the map step works on.

Pure functions on plain data. All times are milliseconds on the session clock.

A window is nominally 15 minutes long. It is cut at the longest pause in the last two minutes
before the nominal boundary (at the boundary itself when there is no pause), and the next
window starts one minute before the cut, so the passage around a cut is seen by both windows.
Passages that the user excluded are removed before windowing and never reach a window.
"""

from __future__ import annotations

import bisect
from dataclasses import dataclass
from typing import Any

from .transcript import CONTEXT_WINDOW_MS

WINDOW_MS = 15 * 60_000
OVERLAP_MS = 60_000
PAUSE_SEARCH_MS = 120_000
_FAR_FUTURE_MS = 10**12

SPEECH = "speech"
SCREENSHOT = "screenshot"


@dataclass(frozen=True)
class Line:
    """One transcript segment (or one screenshot description) as shown to the model."""

    start_ms: int
    end_ms: int
    track: str
    speaker: str  # speaker id of the segment
    label: str  # name the model sees
    text: str
    kind: str = SPEECH
    segment_id: int = -1


@dataclass(frozen=True)
class Window:
    index: int
    span_start_ms: int  # screenshots in [span_start, span_end) belong to the window
    span_end_ms: int
    lines: tuple[Line, ...]
    part: str = ""  # "" for a window; "0"/"1" per split of a window that was too long

    @property
    def start_ms(self) -> int:
        return min(ln.start_ms for ln in self.lines)

    @property
    def end_ms(self) -> int:
        return max(ln.end_ms for ln in self.lines)

    @property
    def speech(self) -> tuple[Line, ...]:
        return tuple(ln for ln in self.lines if ln.kind == SPEECH)


def format_clock(ms: int) -> str:
    s = max(0, int(ms)) // 1000
    return f"{s // 3600:02d}:{s // 60 % 60:02d}:{s % 60:02d}"


def parse_clock(stamp: str) -> int:
    """`hh:mm:ss` to milliseconds."""
    h, m, s = stamp.split(":")
    return (int(h) * 3600 + int(m) * 60 + int(s)) * 1000


def render_line(line: Line) -> str:
    stamp = format_clock(line.start_ms)
    if line.kind == SCREENSHOT:
        return f"[{stamp}] (screenshot) {line.text}"
    return f"[{stamp}] {line.label}: {line.text}"


def render_window(window: Window) -> str:
    return "\n".join(render_line(ln) for ln in window.lines)


def _segment_edits(edits: dict[str, Any] | None) -> dict[str, dict[str, Any]]:
    return (edits or {}).get("segments", {}) or {}


def speech_lines(transcript: dict[str, Any], edits: dict[str, Any] | None = None) -> list[Line]:
    """Segments with the user's text corrections applied, excluded passages removed."""
    labels = {s["id"]: s["label"] for s in transcript["speakers"]}
    changes = _segment_edits(edits)
    lines: list[Line] = []
    for seg in sorted(transcript["segments"], key=lambda s: (s["start_ms"], s["end_ms"], s["id"])):
        edit = changes.get(str(seg["id"]), {})
        if edit.get("excluded"):
            continue
        text = edit["text"] if edit.get("text") is not None else seg["text"]
        text = " ".join(text.split())
        if not text:
            continue
        lines.append(
            Line(
                start_ms=seg["start_ms"],
                end_ms=seg["end_ms"],
                track=seg["track"],
                speaker=seg["speaker"],
                label=labels.get(seg["speaker"], seg["speaker"]),
                text=text,
                segment_id=seg["id"],
            )
        )
    return lines


def excluded_ranges(
    transcript: dict[str, Any], edits: dict[str, Any] | None
) -> list[tuple[int, int]]:
    changes = _segment_edits(edits)
    return [
        (s["start_ms"], s["end_ms"])
        for s in transcript["segments"]
        if changes.get(str(s["id"]), {}).get("excluded")
    ]


def touches_excluded(at_ms: int, excluded: list[tuple[int, int]] | None) -> bool:
    """True when an excluded passage overlaps the discussion the vision model was given for a
    screenshot taken at `at_ms` (CONTEXT_WINDOW_MS before and after it)."""
    lo, hi = at_ms - CONTEXT_WINDOW_MS, at_ms + CONTEXT_WINDOW_MS
    return any(ex_hi >= lo and ex_lo <= hi for ex_lo, ex_hi in excluded or [])


def screenshot_lines(
    descriptions: dict[str, Any] | None, excluded: list[tuple[int, int]] | None = None
) -> list[Line]:
    """Screenshot descriptions as lines. A description is dropped when an excluded passage lies
    within its context window (CONTEXT_WINDOW_MS either side of the screenshot, which includes
    the moment itself): the vision model saw that discussion and the description may repeat
    it. This holds for `descriptions.json` of older runs, which carry no marker of what the
    description was built from. Entries without a usable time or text are ignored."""
    out: list[Line] = []
    for item in (descriptions or {}).get("items", []):
        try:
            at = int(item["offset_ms"])
            text = " ".join(str(item["description"]).split())
        except (KeyError, TypeError, ValueError):
            continue
        if not text or touches_excluded(at, excluded):
            continue
        out.append(Line(at, at, "screenshot", "screenshot", "screenshot", text, SCREENSHOT))
    return sorted(out, key=lambda ln: ln.start_ms)


def _gaps_before(lines: list[Line]) -> list[int]:
    """`gaps[k]` is the silence before line k, measured from the latest end of all earlier
    lines (lines of different tracks overlap)."""
    gaps = [0]
    latest = lines[0].end_ms
    for ln in lines[1:]:
        gaps.append(max(0, ln.start_ms - latest))
        latest = max(latest, ln.end_ms)
    return gaps


def _find_cut(starts: list[int], gaps: list[int], first: int, nominal_end: int) -> int:
    """Index of the first line of the next window (`len(starts)` when this one is the last)."""
    n = len(starts)
    if starts[-1] < nominal_end:
        return n
    lo = bisect.bisect_left(starts, nominal_end - PAUSE_SEARCH_MS, first + 1)
    hi = bisect.bisect_right(starts, nominal_end, lo)
    best: int | None = None
    for k in range(lo, hi):
        if gaps[k] > 0 and (best is None or gaps[k] >= gaps[best]):
            best = k
    if best is not None:
        return best
    return bisect.bisect_left(starts, nominal_end, first + 1)


def split_windows(speech: list[Line], screenshots: list[Line] | None = None) -> list[Window]:
    """Windows over the speech lines, with the screenshot descriptions placed in time."""
    if not speech:
        return []
    starts = [ln.start_ms for ln in speech]
    gaps = _gaps_before(speech)
    cuts: list[tuple[int, int, int, int]] = []  # first line, cut line, span start, span end
    first = 0
    nominal_start = starts[0]
    span_start = 0
    while True:
        cut = _find_cut(starts, gaps, first, nominal_start + WINDOW_MS)
        span_end = _FAR_FUTURE_MS if cut >= len(speech) else starts[cut]
        cuts.append((first, cut, span_start, span_end))
        if cut >= len(speech):
            break
        nominal_start = starts[cut] - OVERLAP_MS
        span_start = max(0, nominal_start)
        first = min(max(bisect.bisect_left(starts, nominal_start), first + 1), cut)
    windows = []
    for i, (a, b, lo, hi) in enumerate(cuts):
        shots = [s for s in screenshots or [] if lo <= s.start_ms < hi]
        lines = sorted([*speech[a:b], *shots], key=lambda ln: (ln.start_ms, ln.kind != SPEECH))
        windows.append(Window(i, lo, hi, tuple(lines)))
    return windows


def split_in_two(window: Window) -> tuple[Window, Window] | None:
    """Halve a window by lines. The second half starts one minute before the cut, like
    windows do, so a statement at the cut is seen whole by one of them. None when the window
    has too little speech to split."""
    lines = window.lines
    if len(window.speech) < 2:
        return None
    mid = len(lines) // 2
    cut = lines[mid].start_ms
    starts = [ln.start_ms for ln in lines]
    second_from = min(max(bisect.bisect_left(starts, cut - OVERLAP_MS), 1), mid)
    first = Window(window.index, window.span_start_ms, cut, lines[:mid], window.part + "0")
    second = Window(
        window.index,
        max(window.span_start_ms, cut - OVERLAP_MS),
        window.span_end_ms,
        lines[second_from:],
        window.part + "1",
    )
    if not first.speech or not second.speech:
        return None
    return first, second
