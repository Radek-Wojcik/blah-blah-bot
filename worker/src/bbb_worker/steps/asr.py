"""Transcription per track with word timestamps (faster-whisper)."""

from __future__ import annotations

from typing import Any

from .. import models, protocol
from ..manifest import SAMPLE_RATE, Manifest, load_track_audio
from . import write_json


def _prepare_cuda_libraries() -> None:
    """Make the cuBLAS and cuDNN DLLs from the nvidia-* wheels findable by CTranslate2."""
    import os
    import sys
    from pathlib import Path

    if sys.platform != "win32":
        return
    for root in sys.path:
        base = Path(root) / "nvidia"
        if not base.is_dir():
            continue
        for lib in base.iterdir():
            bin_dir = lib / "bin"
            if bin_dir.is_dir():
                os.add_dll_directory(str(bin_dir))
                os.environ["PATH"] = str(bin_dir) + os.pathsep + os.environ.get("PATH", "")


def run(manifest: Manifest) -> None:
    protocol.progress("asr", 0.05, "loading model")
    _prepare_cuda_libraries()
    from faster_whisper import WhisperModel

    key = models.asr_key(manifest.options.get("asr_model", "turbo"))
    path = models.local_path(key)
    model = WhisperModel(str(path), device="cuda", compute_type="int8_float16")

    language = manifest.language
    glossary = manifest.options.get("glossary") or []
    kwargs: dict[str, Any] = {"word_timestamps": True, "vad_filter": True, "beam_size": 5}
    if language != "auto":
        kwargs["language"] = language
    if glossary:
        kwargs["initial_prompt"] = "Glossary: " + ", ".join(glossary) + "."

    protocol.progress("asr", 0.10, "transcribing")
    out_dir = manifest.work_dir / "asr"
    total = max(1, len(manifest.tracks))
    for index, track in enumerate(manifest.tracks):
        track_start = 0.10 + 0.88 * (index / total)
        track_span = 0.88 / total
        if index > 0:
            protocol.progress("asr", track_start, f"track {track.name}")
        audio = load_track_audio(manifest, track)
        duration_s = len(audio) / SAMPLE_RATE
        segments_out: list[dict[str, Any]] = []
        detected = "und"
        if duration_s > 0:
            segments, info = model.transcribe(audio, **kwargs)
            detected = info.language
            for seg in segments:
                words = [
                    {
                        "start_ms": round(track.offset_ms + w.start * 1000),
                        "end_ms": round(track.offset_ms + w.end * 1000),
                        "text": w.word,
                    }
                    for w in (seg.words or [])
                ]
                segments_out.append(
                    {
                        "start_ms": round(track.offset_ms + seg.start * 1000),
                        "end_ms": round(track.offset_ms + seg.end * 1000),
                        "text": seg.text.strip(),
                        "words": words,
                    }
                )
                seg_frac = min(1.0, max(0.0, seg.end / duration_s))
                protocol.progress(
                    "asr",
                    min(0.98, track_start + track_span * seg_frac),
                    f"track {track.name}",
                )
        write_json(
            out_dir / f"{track.name}.json",
            {"track": track.name, "language": detected, "segments": segments_out},
        )
        protocol.progress(
            "asr",
            min(0.98, track_start + track_span),
            f"track {track.name} done",
        )
    protocol.progress("asr", 1.0, "done")
