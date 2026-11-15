"""Stage 2: time and peak VRAM of the processing pipeline on a long recording.

    uv run python -m bbb_bench.proto.stage2_pipeline --audio data/long/long-10h.wav \
        --hours 2 --slides data/slides --worker-python ../worker/.venv/Scripts/python.exe

Cuts the first `--hours` of the audio into 60-second segment files like the recorder
writes them, adds screenshots, and runs the worker steps asr, diarize, merge and vision
one after another as separate processes, exactly as the queue does. Per step it records
the wall time, the peak VRAM above the idle baseline and how far VRAM falls back after
the step. The thresholds are those of docs/TESTING.md (45 minutes for two hours, 11.5 GB).

The audio is a repetition of short synthetic meetings, so this measures time and memory,
not recognition quality. The vision step needs BBB_LLAMA_SERVER and BBB_VISION_DIR.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import wave
from pathlib import Path
from typing import Any

from .. import harness

STEPS = ("asr", "diarize", "merge", "vision")
SEGMENT_SECONDS = 60
TWO_HOUR_LIMIT_S = 45 * 60
VRAM_LIMIT_MIB = 11_500  # "11.5 GB" in docs/TESTING.md, read as 11,500 MiB for headroom


def build_meeting(audio: Path, hours: float, slides: Path | None, out: Path) -> Path:
    """Write the meeting folder and manifest; returns the manifest path."""
    if out.exists():
        shutil.rmtree(out)
    track_dir = out / "meeting" / "meet"
    track_dir.mkdir(parents=True)
    wanted = int(hours * 3600 * 16_000)
    seg_samples = SEGMENT_SECONDS * 16_000
    segments: list[str] = []
    with wave.open(str(audio), "rb") as src:
        written = 0
        index = 0
        while written < wanted:
            frames = src.readframes(min(seg_samples, wanted - written))
            if not frames:
                break
            name = f"{index:06}.wav"
            with wave.open(str(track_dir / name), "wb") as dst:
                dst.setnchannels(1)
                dst.setsampwidth(2)
                dst.setframerate(16_000)
                dst.writeframes(frames)
            segments.append(f"meet/{name}")
            written += len(frames) // 2
            index += 1
    shots = []
    if slides:
        shots_dir = out / "meeting" / "screenshots"
        shots_dir.mkdir()
        images = sorted(slides.glob("*.png"))
        step_ms = int(hours * 3600 * 1000 / (len(images) + 1))
        for i, image in enumerate(images):
            shutil.copy(image, shots_dir / image.name)
            shots.append({"file": f"screenshots/{image.name}", "offset_ms": (i + 1) * step_ms})
    manifest = {
        "version": 1,
        "meeting_id": "stage2-timing",
        "tracks": [
            {"name": "meet", "source": "app:meet.exe", "offset_ms": 0, "segments": segments}
        ],
        "screenshots": shots,
        "options": {"language": "auto", "asr_model": "turbo", "max_speakers": 10},
    }
    path = out / "manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return path


def run_step(python: Path, step: str, manifest: Path, meeting: Path) -> dict[str, Any]:
    baseline = harness.gpu_memory_used_mib()
    with harness.VramSampler() as vram, harness.timer() as elapsed:
        proc = subprocess.run(
            [
                str(python),
                "-m",
                "bbb_worker",
                step,
                "--manifest",
                str(manifest),
                "--meeting-dir",
                str(meeting),
            ],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "HF_HUB_OFFLINE": "1"},
        )
    seconds = elapsed()
    after = harness.gpu_memory_used_mib()
    return {
        "step": step,
        "ok": proc.returncode == 0,
        "seconds": round(seconds, 1),
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
        "baseline_mib": baseline,
        "vram_after_mib": after,
        "stderr_tail": proc.stderr[-300:] if proc.returncode else "",
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    out = Path(args.work)
    manifest = build_meeting(
        Path(args.audio), args.hours, Path(args.slides) if args.slides else None, out
    )
    meeting = out / "meeting"
    results = []
    for step in STEPS:
        if step == "vision" and not args.slides:
            continue
        r = run_step(Path(args.worker_python), step, manifest, meeting)
        results.append(r)
        if not r["ok"]:
            break
    total = sum(r["seconds"] for r in results)
    peak_abs = max((r["baseline_mib"] or 0) + r["peak_vram_above_baseline_mib"] for r in results)
    transcript = meeting / "work" / "transcript.json"
    segments = (
        len(json.loads(transcript.read_text(encoding="utf-8"))["segments"])
        if transcript.exists()
        else None
    )
    scale = 2.0 / args.hours
    return {
        "config": {"hours": args.hours, "steps": [r["step"] for r in results]},
        "steps": results,
        "total_seconds": round(total, 1),
        "total_minutes_for_2h": round(total * scale / 60, 1),
        "threshold_minutes_for_2h": TWO_HOUR_LIMIT_S // 60,
        "peak_vram_absolute_mib": peak_abs,
        "threshold_vram_mib": VRAM_LIMIT_MIB,
        "transcript_segments": segments,
        "passed": all(r["ok"] for r in results)
        and total * scale <= TWO_HOUR_LIMIT_S
        and peak_abs <= VRAM_LIMIT_MIB,
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--audio", required=True)
    p.add_argument("--hours", type=float, default=2.0)
    p.add_argument("--slides", default=None)
    p.add_argument("--worker-python", required=True)
    p.add_argument("--work", default="data/stage2-timing")
    p.add_argument("--name", default="2h")
    args = p.parse_args()
    harness.run_prototype("stage2-pipeline", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
