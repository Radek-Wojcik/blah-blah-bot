"""RTTM output (NIST Rich Transcription Time Marked) for the reference diarization."""

from __future__ import annotations


def to_rttm(timeline: list[dict], file_id: str) -> str:
    """One SPEAKER line per turn: type, file, channel, onset, duration, then the label."""
    lines = []
    for item in timeline:
        onset = item["start_s"]
        duration = round(item["end_s"] - item["start_s"], 3)
        lines.append(
            f"SPEAKER {file_id} 1 {onset:.3f} {duration:.3f} <NA> <NA> {item['speaker']} <NA> <NA>"
        )
    return "\n".join(lines) + ("\n" if lines else "")
