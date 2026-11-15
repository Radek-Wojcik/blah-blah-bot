"""Job manifest and access to the recorded audio."""

from __future__ import annotations

import json
import wave
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from . import schema

SAMPLE_RATE = 16_000


@dataclass(frozen=True)
class Track:
    name: str
    source: str
    offset_ms: float
    segments: tuple[str, ...]

    @property
    def is_microphone(self) -> bool:
        return self.source == "microphone"


@dataclass(frozen=True)
class Manifest:
    meeting_id: str
    meeting_dir: Path
    tracks: tuple[Track, ...]
    screenshots: tuple[tuple[str, float], ...]
    options: dict[str, Any]

    @property
    def work_dir(self) -> Path:
        return self.meeting_dir / "work"

    @property
    def language(self) -> str:
        return self.options.get("language", "auto")


def load(path: Path, meeting_dir: Path) -> Manifest:
    doc = json.loads(path.read_text(encoding="utf-8"))
    schema.validate("job-manifest", doc)
    return Manifest(
        meeting_id=doc["meeting_id"],
        meeting_dir=meeting_dir,
        tracks=tuple(
            Track(t["name"], t["source"], float(t["offset_ms"]), tuple(t["segments"]))
            for t in doc["tracks"]
        ),
        screenshots=tuple((s["file"], float(s["offset_ms"])) for s in doc["screenshots"]),
        options=doc["options"],
    )


def read_wav(path: Path) -> np.ndarray:
    """Read a 16 kHz mono 16-bit WAV file as float32 in [-1, 1]."""
    with wave.open(str(path), "rb") as w:
        if w.getsampwidth() != 2 or w.getnchannels() != 1 or w.getframerate() != SAMPLE_RATE:
            raise ValueError(f"{path.name}: expected 16 kHz mono 16-bit WAV")
        data = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    return data.astype(np.float32) / 32768.0


def load_track_audio(manifest: Manifest, track: Track) -> np.ndarray:
    """All segments of a track as one signal. Segments are contiguous, so the sample index
    maps to the session clock as `track.offset_ms + index / 16`."""
    parts = [read_wav(manifest.meeting_dir / rel) for rel in track.segments]
    if not parts:
        return np.zeros(0, dtype=np.float32)
    return np.concatenate(parts)
