"""Helpers for tests that need recorded audio: build a meeting folder the way the recorder does."""

from __future__ import annotations

import json
import subprocess
import sys
import wave
from pathlib import Path

SEGMENT_SAMPLES = 16_000 * 60


def split_into_segments(wav: Path, out_dir: Path, rel_prefix: str) -> list[str]:
    """Write `wav` as 60-second segment files and return their paths relative to the meeting."""
    out_dir.mkdir(parents=True, exist_ok=True)
    with wave.open(str(wav), "rb") as src:
        assert src.getframerate() == 16_000 and src.getnchannels() == 1
        rels = []
        index = 0
        while True:
            frames = src.readframes(SEGMENT_SAMPLES)
            if not frames:
                break
            name = f"{index:06}.wav"
            with wave.open(str(out_dir / name), "wb") as dst:
                dst.setnchannels(1)
                dst.setsampwidth(2)
                dst.setframerate(16_000)
                dst.writeframes(frames)
            rels.append(f"{rel_prefix}/{name}")
            index += 1
    return rels


def make_meeting(
    root: Path,
    tracks: list[tuple[str, str, Path]],
    options: dict | None = None,
    screenshots: list[tuple[Path, int]] | None = None,
) -> tuple[Path, Path]:
    """Create a meeting folder with segmented tracks and a manifest.

    `tracks` are `(name, source, wav)`. Returns `(meeting_dir, manifest_path)`.
    """
    meeting = root / "meeting"
    manifest = {
        "version": 1,
        "meeting_id": "test-meeting",
        "tracks": [],
        "screenshots": [],
        "options": options or {"language": "auto", "asr_model": "turbo"},
    }
    for name, source, wav in tracks:
        segs = split_into_segments(wav, meeting / name, name)
        manifest["tracks"].append(
            {"name": name, "source": source, "offset_ms": 0, "segments": segs}
        )
    for image, offset_ms in screenshots or []:
        rel = f"screenshots/{image.name}"
        (meeting / "screenshots").mkdir(parents=True, exist_ok=True)
        (meeting / rel).write_bytes(image.read_bytes())
        manifest["screenshots"].append({"file": rel, "offset_ms": offset_ms})
    path = root / "manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return meeting, path


def run_step(
    step: str, manifest: Path, meeting: Path, env: dict | None = None, timeout: float = 1800
) -> list[dict]:
    """Run a worker step as the application does and return its protocol events."""
    import os

    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "bbb_worker",
            step,
            "--manifest",
            str(manifest),
            "--meeting-dir",
            str(meeting),
        ],
        capture_output=True,
        check=False,
        text=True,
        timeout=timeout,
        env={**os.environ, **(env or {})},
    )
    events = []
    for line in proc.stdout.splitlines():
        try:
            events.append(json.loads(line))
        except ValueError:
            pass
    if proc.returncode != 0:
        raise AssertionError(f"{step} failed ({proc.returncode}): {proc.stderr[-800:]}")
    return events


def wer(reference: str, hypothesis: str) -> float:
    """Word error rate with the normalisation used in the Stage 0 benchmarks (case,
    punctuation and numerals), so results are comparable with the recorded baseline."""
    bench = Path(__file__).resolve().parents[2] / "benchmarks" / "src"
    if str(bench) not in sys.path:
        sys.path.insert(0, str(bench))
    from bbb_bench.metrics import wer as bench_wer

    return float(bench_wer(reference, hypothesis)["wer"])
