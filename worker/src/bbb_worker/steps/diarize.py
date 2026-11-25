"""Speaker segments for every track that mixes several people (pyannote community-1).

The microphone track is never diarized: it is the user.
"""

from __future__ import annotations

import os
import time
from collections.abc import Callable
from typing import Any

import numpy as np

from .. import models, protocol
from ..manifest import SAMPLE_RATE, Manifest, load_track_audio
from . import write_json

STEP_RANGES: dict[str, tuple[float, float]] = {
    "segmentation": (0.05, 0.45),
    "embeddings": (0.45, 0.90),
    "speaker_counting": (0.90, 0.98),
    "clustering": (0.90, 0.98),
    "discrete_diarization": (0.90, 0.98),
}


def step_fraction(
    step_name: str, completed: int | None = None, total: int | None = None
) -> float | None:
    """Map a pyannote hook step and its batch completion to a fraction in [0.05, 0.98]."""
    bounds = STEP_RANGES.get(step_name)
    if bounds is None:
        return None
    low, high = bounds
    if completed is not None and total is not None and total > 0:
        ratio = min(1.0, max(0.0, completed / total))
        return low + (high - low) * ratio
    return high


def make_diarize_hook(
    track_name: str,
    track_index: int = 0,
    total_tracks: int = 1,
    throttle_s: float = 0.2,
    clock: Callable[[], float] = time.monotonic,
) -> Callable[..., None]:
    """Build a pyannote progress hook throttled to at most once per `throttle_s` seconds."""
    total_tracks = max(1, total_tracks)
    track_low = 0.05 + 0.93 * (track_index / total_tracks)
    track_span = 0.93 / total_tracks
    last_emit = -throttle_s - 1.0
    last_step: str | None = None
    last_frac = track_low

    def hook(
        step_name: str,
        _artifact: Any = None,
        file: Any = None,
        total: int | None = None,
        completed: int | None = None,
        **_kwargs: Any,
    ) -> None:
        nonlocal last_emit, last_step, last_frac
        _ = file
        raw = step_fraction(step_name, completed=completed, total=total)
        if raw is None:
            return
        # Normalize raw [0.05, 0.98] into this track's slice of [0.05, 0.98].
        normalized = (raw - 0.05) / 0.93
        frac = min(0.98, max(last_frac, track_low + track_span * normalized))
        now = clock()
        is_new_step = step_name != last_step
        is_step_done = completed is not None and total is not None and completed >= total
        if not is_new_step and not is_step_done and (now - last_emit) < throttle_s:
            return
        last_step = step_name
        last_emit = now
        last_frac = frac
        protocol.progress("diarize", frac, f"track {track_name}: {step_name}")

    return hook


def finite_embeddings(embeddings: Any) -> list[list[float]]:
    """Embeddings as plain lists, with an empty list for any that is not finite.

    A voice with very little speech can come back with an embedding of NaN. Such a vector is
    not valid JSON and cannot be compared, so the voice is reported without an embedding and
    stays unknown.
    """
    out: list[list[float]] = []
    array = np.asarray(embeddings, dtype=np.float32)
    if array.size == 0:
        return out
    for row in array.reshape(len(array), -1):
        out.append(row.tolist() if np.isfinite(row).all() else [])
    return out


def run(manifest: Manifest) -> None:
    import torch
    from pyannote.audio import Pipeline

    tracks = [t for t in manifest.tracks if not t.is_microphone]
    out_dir = manifest.work_dir / "diarize"
    if not tracks:
        protocol.progress("diarize", 0.05, "no tracks to diarize")
        out_dir.mkdir(parents=True, exist_ok=True)
        protocol.progress("diarize", 1.0, "done")
        return

    protocol.progress("diarize", 0.05, "loading model")
    path = models.local_path("diarization")
    pipeline = Pipeline.from_pretrained(str(path))
    pipeline.to(torch.device("cuda"))
    # BBB_DIARIZE_SEG_BATCH and BBB_DIARIZE_EMB_BATCH (default unset = the model's own
    # settings): batch sizes of the segmentation and the speaker embedding passes. Larger
    # batches are faster and use more GPU memory; the result does not change.
    for variable, attribute in (
        ("BBB_DIARIZE_SEG_BATCH", "segmentation_batch_size"),
        ("BBB_DIARIZE_EMB_BATCH", "embedding_batch_size"),
    ):
        size = os.environ.get(variable, "")
        if size.isdigit() and 1 <= int(size) <= 256:
            setattr(pipeline, attribute, int(size))

    max_speakers = manifest.options.get("max_speakers")
    bounds: dict[str, Any] = {"max_speakers": max_speakers} if max_speakers else {}

    total_tracks = len(tracks)
    for index, track in enumerate(tracks):
        audio = load_track_audio(manifest, track)
        turns: list[dict[str, Any]] = []
        embeddings: list[list[float]] = []
        labels: list[str] = []
        if len(audio) / SAMPLE_RATE >= 1.0:
            waveform = torch.from_numpy(audio)[None, :]
            hook = make_diarize_hook(track.name, index, total_tracks)
            audio_input = {"waveform": waveform, "sample_rate": SAMPLE_RATE}
            try:
                out = pipeline(audio_input, hook=hook, **bounds)
            except TypeError:
                out = pipeline(audio_input, **bounds)
            # The exclusive output has no overlapping speech, which aligns cleanly with words.
            for turn, _, who in out.exclusive_speaker_diarization.itertracks(yield_label=True):
                turns.append(
                    {
                        "start_ms": round(track.offset_ms + turn.start * 1000),
                        "end_ms": round(track.offset_ms + turn.end * 1000),
                        "speaker": who,
                    }
                )
            labels = list(out.speaker_diarization.labels())
            if out.speaker_embeddings is not None:
                embeddings = finite_embeddings(out.speaker_embeddings)
        write_json(
            out_dir / f"{track.name}.json",
            {"track": track.name, "speakers": labels, "turns": turns, "embeddings": embeddings},
        )
        done_frac = 0.05 + 0.93 * ((index + 1) / total_tracks)
        protocol.progress("diarize", min(0.98, done_frac), f"track {track.name}")
    protocol.progress("diarize", 1.0, "done")
