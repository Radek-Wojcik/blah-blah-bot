"""Unit tests for audio/video media ingest and the cursor-immune slide-change detector."""

from __future__ import annotations

import itertools
import json
import os
import subprocess
import sys
import wave
from pathlib import Path
from typing import Any

import numpy as np

from bbb_worker import ingest, protocol
from bbb_worker.manifest import SAMPLE_RATE

SRC = Path(__file__).resolve().parents[1] / "src"

try:
    from PIL import Image, ImageDraw

    HAS_PIL = True
except ImportError:
    HAS_PIL = False


def make_slide_image(
    width: int = 640,
    height: int = 360,
    bg: tuple[int, int, int] = (245, 245, 245),
    boxes: list[tuple[tuple[int, int, int, int], tuple[int, int, int]]] | None = None,
) -> Any:
    """Create a synthetic slide as a Pillow Image (when Pillow is installed) or RGB ndarray."""
    if HAS_PIL:
        img = Image.new("RGB", (width, height), color=bg)
        draw = ImageDraw.Draw(img)
        for (x0, y0, x1, y1), color in boxes or []:
            draw.rectangle([x0, y0, x1, y1], fill=color)
        return img

    arr = np.empty((height, width, 3), dtype=np.uint8)
    arr[:, :] = bg
    for (x0, y0, x1, y1), color in boxes or []:
        arr[max(0, y0) : min(height, y1), max(0, x0) : min(width, x1)] = color
    return arr


def with_cursor(slide: Any, x: int, y: int, size: int = 16) -> Any:
    """Overlay a high-contrast mouse cursor (affecting 1-2 spatial blocks) on `slide`."""
    if HAS_PIL and hasattr(slide, "copy"):
        img = slide.copy()
        draw = ImageDraw.Draw(img)
        draw.rectangle([x, y, x + size, y + size], fill=(10, 10, 10))
        return img

    arr = np.array(slide, copy=True)
    arr[y : y + size, x : x + size] = (10, 10, 10)
    return arr


def slide_one(width: int = 640, height: int = 360) -> Any:
    """Slide 1: title bar at top and left-side bullet block."""
    return make_slide_image(
        width,
        height,
        bg=(242, 244, 248),
        boxes=[
            ((20, 20, width - 20, 75), (30, 60, 140)),
            ((30, 100, width // 2 - 20, height - 30), (50, 50, 60)),
        ],
    )


def slide_two(width: int = 640, height: int = 360) -> Any:
    """Slide 2: dark theme with right-side chart and bottom summary band (>= 5 blocks changed)."""
    return make_slide_image(
        width,
        height,
        bg=(28, 32, 38),
        boxes=[
            ((width // 2, 30, width - 30, height - 100), (220, 140, 40)),
            ((30, height - 85, width - 30, height - 20), (80, 200, 160)),
        ],
    )


def slide_three(width: int = 640, height: int = 360) -> Any:
    """Slide 3: central architecture diagram with four quadrants."""
    return make_slide_image(
        width,
        height,
        bg=(235, 250, 235),
        boxes=[
            ((80, 60, 280, 160), (180, 40, 40)),
            ((360, 60, 560, 160), (40, 140, 180)),
            ((80, 200, 280, 300), (140, 40, 180)),
            ((360, 200, 560, 300), (60, 60, 60)),
        ],
    )


def test_cursor_movement_and_1_to_2_blocks_produce_zero_extra_screenshots():
    detector = ingest.SlideDetector(capture_initial=False)
    s1 = slide_one()

    # Establish Slide 1 baseline at t=0.
    assert detector.feed(s1, 0.0) is None

    # Move cursor across 1-2 blocks over 10 seconds at 1 fps.
    cursor_positions = [
        (40, 40),
        (120, 80),
        (240, 160),
        (380, 200),
        (500, 260),
        (520, 280),
    ]
    for i, (cx, cy) in enumerate(cursor_positions, start=1):
        frame = with_cursor(s1, cx, cy, size=24)
        changed = ingest.block_diff_count(
            ingest.to_gray_blurred_160x90(frame),
            ingest.to_gray_blurred_160x90(s1),
        )
        assert changed <= 2
        assert detector.feed(frame, i * 1000.0) is None

    assert len(detector.captures) == 0


def test_switching_to_new_slide_plus_two_stable_frames_captures_one_and_deduplicates_slide_1():
    detector = ingest.SlideDetector(capture_initial=False)
    s1 = slide_one()
    s2 = slide_two()

    # Slide 1 baseline + cursor movement -> 0 extra screenshots.
    assert detector.feed(s1, 0.0) is None
    assert detector.feed(with_cursor(s1, 100, 100), 1000.0) is None
    assert detector.feed(with_cursor(s1, 200, 150), 2000.0) is None
    assert len(detector.captures) == 0

    # Verify Slide 2 changes >= 5 blocks compared to Slide 1.
    b1 = ingest.to_gray_blurred_160x90(s1)
    b2 = ingest.to_gray_blurred_160x90(s2)
    assert ingest.block_diff_count(b1, b2) >= 5

    # Switch to Slide 2 at t=6000 ms: triggers candidate change, waits for 2 stable frames.
    assert detector.feed(s2, 6000.0) is None
    assert len(detector.captures) == 0

    # 1st consecutive stable frame at t=7000 ms -> still waiting for 2nd stable frame.
    assert detector.feed(s2, 7000.0) is None
    assert len(detector.captures) == 0

    # 2nd consecutive stable frame at t=8000 ms -> captures exactly 1 screenshot!
    cap = detector.feed(s2, 8000.0)
    assert cap is not None
    assert cap.file == "img/slide-0001-8000.png"
    assert cap.offset_ms == 8000
    assert len(detector.captures) == 1

    # Cursor moves on Slide 2 -> 0 extra screenshots.
    assert detector.feed(with_cursor(s2, 300, 200), 9000.0) is None
    assert len(detector.captures) == 1

    # Flip back to Slide 1 at t=14000 ms + 2 stable frames at t=15000, 16000 ms -> deduplicated!
    assert detector.feed(s1, 14000.0) is None
    assert detector.feed(s1, 15000.0) is None
    assert detector.feed(s1, 16000.0) is None
    assert len(detector.captures) == 1


def test_unstable_scrolling_waits_until_two_consecutive_stable_frames():
    detector = ingest.SlideDetector(capture_initial=False)
    s1 = slide_one()
    s2 = slide_two()
    s3 = slide_three()

    assert detector.feed(s1, 0.0) is None
    # Transition starts to s2 at t=6s, then immediately changes to s3 at t=7s (unstable!).
    assert detector.feed(s2, 6000.0) is None
    assert detector.feed(s3, 7000.0) is None
    assert len(detector.captures) == 0

    # Now s3 stays stable for 2 consecutive frames at t=8s and t=9s.
    assert detector.feed(s3, 8000.0) is None
    cap = detector.feed(s3, 9000.0)
    assert cap is not None
    assert cap.offset_ms == 9000
    assert len(detector.captures) == 1


def test_cooldown_enforces_min_5s_between_captures():
    detector = ingest.SlideDetector(capture_initial=False)
    s1 = slide_one()
    s2 = slide_two()
    s3 = slide_three()

    assert detector.feed(s1, 0.0) is None
    assert detector.feed(s2, 1000.0) is None
    assert detector.feed(s2, 2000.0) is None
    cap1 = detector.feed(s2, 3000.0)
    assert cap1 is not None and cap1.offset_ms == 3000

    # Immediately switch to s3 at t=4000, stable at 5000 and 6000 (< 5000 ms since cap1 at 3000).
    assert detector.feed(s3, 4000.0) is None
    assert detector.feed(s3, 5000.0) is None
    assert detector.feed(s3, 6000.0) is None  # 6000 - 3000 = 3000 < 5000 ms cooldown
    assert detector.feed(s3, 7000.0) is None  # 7000 - 3000 = 4000 < 5000 ms cooldown
    cap2 = detector.feed(s3, 8000.0)  # 8000 - 3000 = 5000 >= 5000 ms cooldown
    assert cap2 is not None and cap2.offset_ms == 8000
    assert len(detector.captures) == 2


def test_save_slide_png_caps_width_at_1280(tmp_path: Path):
    wide = slide_two(width=1920, height=1080)
    out = tmp_path / "img" / "slide-0001-5000.png"
    ingest.save_slide_png(wide, out)
    assert out.exists() and out.stat().st_size > 100
    if HAS_PIL:
        with Image.open(out) as loaded:
            assert loaded.size == (1280, 720)


def _write_test_wav(path: Path, seconds: float) -> None:
    samples = round(seconds * SAMPLE_RATE)
    t = np.arange(samples, dtype=np.float32) / SAMPLE_RATE
    pcm = (np.sin(2.0 * np.pi * 220.0 * t) * 12000).astype("<i2")
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm.tobytes())


def test_ingest_audio_splits_into_60s_segments_and_emits_monotonic_progress(
    tmp_path: Path, monkeypatch
):
    src_wav = tmp_path / "recording.wav"
    _write_test_wav(src_wav, seconds=65.0)
    meeting_dir = tmp_path / "meeting"

    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": fractions.append(fraction) if step == "ingest" else None,
    )

    doc = ingest.ingest_media(src_wav, meeting_dir, mode="audio_only", meeting_id="m-ing")
    assert doc["duration_ms"] == 65_000
    assert len(doc["segments"]) == 2
    assert doc["segments"][0]["file"] == "tracks/imported/000000.wav"
    assert doc["segments"][0]["offset_ms"] == 0
    assert doc["segments"][0]["duration_ms"] == 60_000
    assert doc["segments"][1]["file"] == "tracks/imported/000001.wav"
    assert doc["segments"][1]["offset_ms"] == 60_000
    assert doc["segments"][1]["duration_ms"] == 5_000
    assert (meeting_dir / "tracks" / "imported" / "000000.wav").exists()
    assert (meeting_dir / "tracks" / "imported" / "000001.wav").exists()
    assert (meeting_dir / "work" / "ingest.json").exists()

    intermediate = [f for f in fractions if f < 1.0]
    assert intermediate and intermediate[0] == 0.05
    assert all(0.0 < p1 <= p2 < 1.0 for p1, p2 in itertools.pairwise(intermediate))
    assert fractions[-1] == 1.0


def test_cli_ingest_via_manifest(tmp_path: Path):
    src_wav = tmp_path / "sample.wav"
    _write_test_wav(src_wav, seconds=3.0)
    meeting_dir = tmp_path / "meeting"
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "version": 1,
                "meeting_id": "m-cli-ingest",
                "tracks": [],
                "screenshots": [],
                "options": {
                    "ingest": {
                        "input_path": str(src_wav),
                        "mode": "audio_only",
                    }
                },
            }
        ),
        encoding="utf-8",
    )

    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "bbb_worker",
            "ingest",
            "--manifest",
            str(manifest_path),
            "--meeting-dir",
            str(meeting_dir),
        ],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
        env={**os.environ, "PYTHONPATH": str(SRC)},
    )
    assert proc.returncode == 0, proc.stderr
    events = [json.loads(line) for line in proc.stdout.splitlines() if line.startswith("{")]
    assert events[0] == {
        "event": "progress",
        "step": "ingest",
        "fraction": 0.0,
        "message": "started",
    }
    assert events[-1] == {"event": "done", "step": "ingest"}
    saved = json.loads((meeting_dir / "work" / "ingest.json").read_text(encoding="utf-8"))
    assert saved["duration_ms"] == 3_000
    assert len(saved["segments"]) == 1
