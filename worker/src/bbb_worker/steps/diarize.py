"""Speaker segments for every track that mixes several people (pyannote community-1).

The microphone track is never diarized: it is the user.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from .. import models, protocol
from ..manifest import SAMPLE_RATE, Manifest, load_track_audio
from . import write_json


def run(manifest: Manifest) -> None:
    import torch
    from pyannote.audio import Pipeline

    tracks = [t for t in manifest.tracks if not t.is_microphone]
    out_dir = manifest.work_dir / "diarize"
    if not tracks:
        out_dir.mkdir(parents=True, exist_ok=True)
        return

    path = models.local_path("diarization")
    pipeline = Pipeline.from_pretrained(str(path))
    pipeline.to(torch.device("cuda"))

    max_speakers = manifest.options.get("max_speakers")
    bounds: dict[str, Any] = {"max_speakers": max_speakers} if max_speakers else {}

    for index, track in enumerate(tracks):
        audio = load_track_audio(manifest, track)
        turns: list[dict[str, Any]] = []
        embeddings: list[list[float]] = []
        labels: list[str] = []
        if len(audio) / SAMPLE_RATE >= 1.0:
            waveform = torch.from_numpy(audio)[None, :]
            out = pipeline({"waveform": waveform, "sample_rate": SAMPLE_RATE}, **bounds)
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
                embeddings = np.asarray(out.speaker_embeddings, dtype=np.float32).tolist()
        write_json(
            out_dir / f"{track.name}.json",
            {"track": track.name, "speakers": labels, "turns": turns, "embeddings": embeddings},
        )
        protocol.progress("diarize", (index + 1) / len(tracks), f"track {track.name}")
