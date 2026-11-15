"""Merging recognised words and speaker segments into one transcript.

Pure functions on plain data, so they are tested without models. All times are
milliseconds on the session clock.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

# A pause between two words of the same speaker longer than this starts a new segment.
MAX_GAP_MS = 1500
# A word that falls outside every speaker segment is attached to the nearest one when it is
# at most this far away; otherwise it is attributed to "unknown".
NEAREST_TOLERANCE_MS = 500

Turn = tuple[float, float, str]


def speaker_for_word(start_ms: float, end_ms: float, turns: list[Turn]) -> str | None:
    """Speaker whose turns overlap the word most; the nearest within tolerance otherwise."""
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
            order: dict[str, int] = {}
            for _s, _e, who in sorted(turns):
                order.setdefault(who, len(order) + 1)

            def speaker_of(w, turns=turns, order=order, name=name):  # type: ignore[misc]
                who = speaker_for_word(w["start_ms"], w["end_ms"], turns)
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
    transcript: dict[str, Any], offset_ms: float, window_ms: int = 60_000
) -> list[dict[str, Any]]:
    """Segments within `window_ms` before and after a moment, for the vision prompt."""
    return [
        s
        for s in transcript["segments"]
        if s["end_ms"] >= offset_ms - window_ms and s["start_ms"] <= offset_ms + window_ms
    ]
