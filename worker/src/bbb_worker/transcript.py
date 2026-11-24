"""Merging recognised words and speaker segments into one transcript.

Pure functions on plain data, so they are tested without models. All times are
milliseconds on the session clock.
"""

from __future__ import annotations

import math
from bisect import bisect_left, bisect_right
from collections.abc import Callable, Sequence
from typing import Any

# A pause between two words of the same speaker longer than this starts a new segment.
MAX_GAP_MS = 1500
# A word that falls outside every speaker segment is attached to the nearest one when it is
# at most this far away; otherwise it is attributed to "unknown".
NEAREST_TOLERANCE_MS = 500

# How far around a screenshot the discussion is given to the vision model as context.
CONTEXT_WINDOW_MS = 60_000

Turn = tuple[float, float, str]
_NEG_INF = -math.inf


class TurnIndex:
    """Interval index over speaker turns for O(log T) overlap and nearest-turn lookups.

    Preserves exact tie-breaking by original turn order (`orig_idx`).
    """

    def __init__(self, turns: Sequence[Turn]) -> None:
        indexed = sorted(
            ((s, e, who, idx) for idx, (s, e, who) in enumerate(turns)),
            key=lambda item: (item[0], item[3]),
        )
        self._turns = indexed
        self._starts = [item[0] for item in indexed]
        self._n = len(indexed)
        cap = 1
        while cap < self._n:
            cap *= 2
        self._cap = cap
        tree = [_NEG_INF] * (2 * cap)
        for i, item in enumerate(indexed):
            tree[cap + i] = item[1]
        for k in range(cap - 1, 0, -1):
            tree[k] = max(tree[2 * k], tree[2 * k + 1])
        self._tree = tree

    def _query_candidates(self, q_start: float, q_end: float, *, strict: bool) -> list[int]:
        if self._n == 0:
            return []
        hi = bisect_left(self._starts, q_end) if strict else bisect_right(self._starts, q_end)
        if hi <= 0:
            return []

        cap = self._cap
        tree = self._tree
        out: list[int] = []
        stack = [(1, 0, cap)]
        while stack:
            node, node_l, node_r = stack.pop()
            if node_l >= hi:
                continue
            max_end = tree[node]
            if max_end <= q_start if strict else max_end < q_start:
                continue
            if node >= cap:
                out.append(node - cap)
                continue
            mid = (node_l + node_r) // 2
            if mid < hi:
                stack.append((2 * node + 1, mid, node_r))
            stack.append((2 * node, node_l, mid))
        return out

    def speaker_for_word(self, start_ms: float, end_ms: float) -> str | None:
        """Speaker whose turns overlap the word most; the nearest within tolerance otherwise."""
        if self._n == 0:
            return None
        turns = self._turns

        best_overlap = 0.0
        best_idx = -1
        best_who: str | None = None
        for i in self._query_candidates(start_ms, end_ms, strict=True):
            t_start, t_end, who, orig_idx = turns[i]
            overlap = min(end_ms, t_end) - max(start_ms, t_start)
            if overlap > 0 and (
                best_who is None
                or overlap > best_overlap
                or (overlap == best_overlap and orig_idx < best_idx)
            ):
                best_overlap = overlap
                best_idx = orig_idx
                best_who = who
        if best_who is not None:
            return best_who

        mid = (start_ms + end_ms) / 2
        best_dist = math.inf
        nearest_idx = -1
        nearest_who: str | None = None
        for i in self._query_candidates(
            mid - NEAREST_TOLERANCE_MS, mid + NEAREST_TOLERANCE_MS, strict=False
        ):
            t_start, t_end, who, orig_idx = turns[i]
            distance = max(t_start - mid, mid - t_end, 0.0)
            if distance <= NEAREST_TOLERANCE_MS and (
                nearest_who is None
                or distance < best_dist
                or (distance == best_dist and orig_idx < nearest_idx)
            ):
                best_dist = distance
                nearest_idx = orig_idx
                nearest_who = who
        return nearest_who


_INDEX_CACHE: list[tuple[Sequence[Turn], int, Turn, Turn, TurnIndex]] = []


def _get_turn_index(turns: Sequence[Turn]) -> TurnIndex:
    n = len(turns)
    first, last = turns[0], turns[-1]
    for entry in _INDEX_CACHE:
        if entry[0] is turns and entry[1] == n and entry[2] == first and entry[3] == last:
            return entry[4]
    index = TurnIndex(turns)
    _INDEX_CACHE.insert(0, (turns, n, first, last, index))
    if len(_INDEX_CACHE) > 4:
        _INDEX_CACHE.pop()
    return index


def speaker_for_word(
    start_ms: float, end_ms: float, turns: Sequence[Turn] | TurnIndex
) -> str | None:
    """Speaker whose turns overlap the word most; the nearest within tolerance otherwise."""
    if isinstance(turns, TurnIndex):
        return turns.speaker_for_word(start_ms, end_ms)
    if len(turns) <= 32:
        best: tuple[float, str] | None = None
        for t_start, t_end, who in turns:
            overlap = min(end_ms, t_end) - max(start_ms, t_start)
            if overlap > 0 and (best is None or overlap > best[0]):
                best = (overlap, who)
        if best:
            return best[1]
        mid = (start_ms + end_ms) / 2
        nearest: tuple[float, str] | None = None
        for t_start, t_end, who in turns:
            distance = max(t_start - mid, mid - t_end, 0.0)
            if nearest is None or distance < nearest[0]:
                nearest = (distance, who)
        if nearest and nearest[0] <= NEAREST_TOLERANCE_MS:
            return nearest[1]
        return None
    return _get_turn_index(turns).speaker_for_word(start_ms, end_ms)


def words_of(asr: dict[str, Any]) -> list[dict[str, Any]]:
    """Flat word list of one ASR result (`segments[].words[]`), in time order."""
    out: list[dict[str, Any]] = []
    for seg in asr["segments"]:
        out.extend(seg.get("words", []))
    return sorted(out, key=lambda w: w["start_ms"])


def join_words(tokens: list[str]) -> str:
    """Whisper tokens carry their own leading space; join and normalise whitespace."""
    return " ".join("".join(tokens).split())


def segments_for_track(
    track: str,
    words: list[dict[str, Any]],
    speaker_of: Callable[[dict[str, Any]], str],
) -> list[dict[str, Any]]:
    """Group the words of one track into segments with one speaker each."""
    segments: list[dict[str, Any]] = []
    current: dict[str, Any] | None = None
    for w in words:
        who = speaker_of(w)
        if (
            current is None
            or current["speaker"] != who
            or w["start_ms"] - current["end_ms"] > MAX_GAP_MS
        ):
            current = {"track": track, "speaker": who, "words": []}
            segments.append(current)
        current["words"].append(
            {"start_ms": int(w["start_ms"]), "end_ms": int(w["end_ms"]), "text": w["text"]}
        )
        current["end_ms"] = int(w["end_ms"])
    for seg in segments:
        seg["start_ms"] = seg["words"][0]["start_ms"]
        seg["text"] = join_words([w["text"] for w in seg["words"]])
    return segments


def pretty_source(source: str) -> str:
    """`app:discord.exe` becomes `Discord`; `tab:Docs` becomes `Docs`."""
    name = source.split(":", 1)[-1]
    name = name.removesuffix(".exe").replace("-", " ").strip()
    return name[:1].upper() + name[1:] if name else "Source"


def merge(
    meeting_id: str,
    tracks: list[dict[str, Any]],
    asr: dict[str, dict[str, Any]],
    diarization: dict[str, list[Turn]],
) -> dict[str, Any]:
    """Build the transcript document.

    `tracks` are `{"name", "source"}` in manifest order. `asr[track]` is the ASR result of a
    track and `diarization[track]` its speaker turns (only for diarized tracks). The
    microphone is the user; a track without diarization is one speaker named after its
    source.
    """
    speakers: dict[str, dict[str, Any]] = {}
    all_segments: list[dict[str, Any]] = []
    languages: dict[str, float] = {}

    def add_speaker(sid: str, label: str, kind: str, track: str | None = None) -> str:
        entry: dict[str, Any] = {"id": sid, "label": label, "kind": kind}
        if track:
            entry["track"] = track
        speakers.setdefault(sid, entry)
        return sid

    for track in tracks:
        name, source = track["name"], track["source"]
        if name not in asr:
            continue
        words = words_of(asr[name])
        lang = asr[name].get("language", "und")
        languages[lang] = languages.get(lang, 0.0) + sum(w["end_ms"] - w["start_ms"] for w in words)

        speaker_of: Callable[[dict[str, Any]], str]
        if source == "microphone":
            sid = add_speaker("user", "You", "user")
            speaker_of = lambda _w, sid=sid: sid
        elif name in diarization:
            turns = diarization[name]
            turn_index = TurnIndex(turns)
            order: dict[str, int] = {}
            for _s, _e, who in sorted(turns):
                order.setdefault(who, len(order) + 1)

            def speaker_of(w, turn_index=turn_index, order=order, name=name):  # type: ignore[misc]
                who = turn_index.speaker_for_word(w["start_ms"], w["end_ms"])
                if who is None:
                    return add_speaker(f"{name}:unknown", "Unknown", "unknown", name)
                return add_speaker(f"{name}:{who}", f"Speaker {order[who]}", "voice", name)

        else:
            sid = add_speaker(f"source:{name}", pretty_source(source), "source", name)
            speaker_of = lambda _w, sid=sid: sid

        all_segments.extend(segments_for_track(name, words, speaker_of))

    all_segments.sort(key=lambda s: (s["start_ms"], s["end_ms"]))
    for i, seg in enumerate(all_segments):
        seg["id"] = i
    dominant = max(languages, key=lambda k: languages[k]) if languages else "und"
    keys = ("id", "track", "speaker", "start_ms", "end_ms", "text", "words")
    return {
        "version": 1,
        "meeting_id": meeting_id,
        "language": dominant,
        "speakers": list(speakers.values()),
        "segments": [{k: seg[k] for k in keys} for seg in all_segments],
    }


def context_around(
    transcript: dict[str, Any], offset_ms: float, window_ms: int = CONTEXT_WINDOW_MS
) -> list[dict[str, Any]]:
    """Segments within `window_ms` before and after a moment, for the vision prompt."""
    return [
        s
        for s in transcript["segments"]
        if s["end_ms"] >= offset_ms - window_ms and s["start_ms"] <= offset_ms + window_ms
    ]
