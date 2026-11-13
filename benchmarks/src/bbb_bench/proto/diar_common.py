"""Shared helpers for the diarization (P3) and speaker recognition (P4) prototypes."""

from __future__ import annotations

import wave
from pathlib import Path

import numpy as np

from ..metrics.der import Segment

PIPELINE_ID = "pyannote/speaker-diarization-community-1"


def read_rttm(path: Path) -> list[Segment]:
    """Parse an RTTM file into (start_s, end_s, speaker) tuples."""
    segments = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) >= 8 and parts[0] == "SPEAKER":
            start, dur = float(parts[3]), float(parts[4])
            segments.append((start, start + dur, parts[7]))
    return segments


def read_wav_mono16k(path: Path) -> tuple[np.ndarray, int]:
    """Read a 16-bit mono WAV file as float32 in [-1, 1]."""
    with wave.open(str(path), "rb") as w:
        if w.getsampwidth() != 2 or w.getnchannels() != 1:
            raise ValueError(f"{path.name}: expected 16-bit mono WAV")
        rate = w.getframerate()
        data = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    return data.astype(np.float32) / 32768.0, rate


def load_pipeline(device: str = "cuda"):
    """Load the community-1 pipeline using the Hugging Face login of the current user."""
    import torch
    from pyannote.audio import Pipeline

    pipeline = Pipeline.from_pretrained(PIPELINE_ID)
    pipeline.to(torch.device(device))
    return pipeline


def diarize(pipeline, wav: Path, **bounds):
    """Run the pipeline on a WAV file; returns (segments, exclusive_segments, embeddings).

    The audio is decoded here and passed as a waveform, so no system decoder is needed.
    `bounds` may contain num_speakers, min_speakers or max_speakers.
    """
    import torch

    samples, rate = read_wav_mono16k(wav)
    out = pipeline({"waveform": torch.from_numpy(samples)[None, :], "sample_rate": rate}, **bounds)
    ann = out.speaker_diarization
    segs = [(t.start, t.end, label) for t, _, label in ann.itertracks(yield_label=True)]
    excl = [
        (t.start, t.end, label)
        for t, _, label in out.exclusive_speaker_diarization.itertracks(yield_label=True)
    ]
    return segs, excl, ann.labels(), out.speaker_embeddings


def best_overlap_mapping(
    reference: list[Segment], hypothesis: list[Segment]
) -> dict[str, tuple[str, float]]:
    """Map each hypothesis speaker to the reference speaker it overlaps most.

    Returns {hyp_speaker: (ref_speaker, overlap_seconds)}.
    """
    overlaps: dict[tuple[str, str], float] = {}
    for rs, re_, rl in reference:
        for hs, he, hl in hypothesis:
            ov = min(re_, he) - max(rs, hs)
            if ov > 0:
                overlaps[(hl, rl)] = overlaps.get((hl, rl), 0.0) + ov
    best: dict[str, tuple[str, float]] = {}
    for (hl, rl), ov in overlaps.items():
        if hl not in best or ov > best[hl][1]:
            best[hl] = (rl, ov)
    return best


def cosine(a: np.ndarray, b: np.ndarray) -> float:
    denom = float(np.linalg.norm(a) * np.linalg.norm(b))
    return float(np.dot(a, b) / denom) if denom else 0.0
