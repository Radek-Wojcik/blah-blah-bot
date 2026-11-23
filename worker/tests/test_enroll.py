"""Unit tests for manual 15-30 s voice enrollment (`bbb_worker.enroll`)."""

from __future__ import annotations

import json
import math
import os
import subprocess
import sys
import wave
from pathlib import Path

import numpy as np
import pytest

from bbb_worker import enroll, protocol
from bbb_worker.manifest import SAMPLE_RATE

SRC = Path(__file__).resolve().parents[1] / "src"


def _make_enrollment_sample(seconds: float = 18.0, fundamental_hz: float = 180.0) -> np.ndarray:
    """Build a synthetic 18 s audio sample with leading silence, a clean 6 s voiced middle
    passage, a brief clipped spike, and trailing quiet noise."""
    total = round(seconds * SAMPLE_RATE)
    audio = np.zeros(total, dtype=np.float32)
    t = np.arange(total, dtype=np.float32) / SAMPLE_RATE

    # 0..3s: very quiet background hum
    s0 = 3 * SAMPLE_RATE
    audio[:s0] = 0.002 * np.sin(2.0 * np.pi * 60.0 * t[:s0])

    # 3..10s: clean harmonic voiced speech-like signal (moderate amplitude, no clipping)
    s1 = 10 * SAMPLE_RATE
    voiced_t = t[s0:s1]
    voiced = (
        0.35 * np.sin(2.0 * np.pi * fundamental_hz * voiced_t)
        + 0.18 * np.sin(2.0 * np.pi * (2.0 * fundamental_hz) * voiced_t)
        + 0.09 * np.sin(2.0 * np.pi * (3.0 * fundamental_hz) * voiced_t)
    )
    audio[s0:s1] = voiced.astype(np.float32)

    # 10..14s: heavily clipped distorted segment
    s2 = 14 * SAMPLE_RATE
    audio[s0 + (s1 - s0) : s2] = np.where(np.sin(2.0 * np.pi * 400.0 * t[s1:s2]) >= 0, 1.0, -1.0)

    # 14..18s: silence
    return audio


def test_extract_cleanest_clip_picks_clean_4_to_6s_voiced_region():
    audio = _make_enrollment_sample(seconds=18.0)
    clip, start_ms, duration_ms = enroll.extract_cleanest_clip(audio)
    assert 4_000 <= duration_ms <= 6_000
    # The clean voiced region sits between 3000 ms and 10000 ms.
    assert 2_500 <= start_ms <= 5_500
    assert float(np.max(np.abs(clip))) < 0.99


def test_enroll_voice_writes_preview_clip_and_256d_normalized_embedding(
    tmp_path: Path, monkeypatch
):
    monkeypatch.setenv("BBB_ENROLL_FALLBACK", "1")
    sample_wav = tmp_path / "voice_sample.wav"
    audio = _make_enrollment_sample(seconds=20.0, fundamental_hz=195.0)
    enroll.write_wav_16k_mono(sample_wav, audio)

    clip_out = tmp_path / "clips" / "participant.wav"
    out_json = tmp_path / "work" / "enroll.json"

    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": fractions.append(fraction) if step == "enroll" else None,
    )

    doc = enroll.enroll_voice(sample_wav, clip_out, out_json)
    assert doc["duration_ms"] == 20_000
    assert doc["clip_file"] == str(clip_out)
    assert clip_out.exists()
    with wave.open(str(clip_out), "rb") as w:
        assert w.getframerate() == SAMPLE_RATE
        assert w.getnchannels() == 1
        assert w.getsampwidth() == 2
        clip_s = w.getnframes() / SAMPLE_RATE
        assert 4.0 <= clip_s <= 6.0

    emb = doc["embedding"]
    assert len(emb) == 256
    assert all(isinstance(x, float) and math.isfinite(x) for x in emb)
    norm = math.sqrt(sum(x * x for x in emb))
    assert abs(norm - 1.0) < 1e-5

    saved = json.loads(out_json.read_text(encoding="utf-8"))
    assert saved == doc
    assert fractions == [0.05, 0.35, 0.65, 1.0]


def test_normalize_embedding_rejects_nan_and_zero_vectors():
    with pytest.raises(ValueError, match="non-finite"):
        enroll.normalize_embedding([float("nan")] * 256)
    with pytest.raises(ValueError, match="zero"):
        enroll.normalize_embedding([0.0] * 256)


def test_cli_enroll_subcommand(tmp_path: Path):
    sample_wav = tmp_path / "input.wav"
    enroll.write_wav_16k_mono(sample_wav, _make_enrollment_sample(seconds=16.0))
    clip_out = tmp_path / "preview.wav"
    out_json = tmp_path / "enroll.json"

    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "bbb_worker",
            "enroll",
            "--input-path",
            str(sample_wav),
            "--clip-out-path",
            str(clip_out),
            "--output-path",
            str(out_json),
        ],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
        env={**os.environ, "PYTHONPATH": str(SRC), "BBB_ENROLL_FALLBACK": "1"},
    )
    assert proc.returncode == 0, proc.stderr
    events = [json.loads(line) for line in proc.stdout.splitlines() if line.startswith("{")]
    assert events[0] == {
        "event": "progress",
        "step": "enroll",
        "fraction": 0.0,
        "message": "started",
    }
    assert events[-1] == {"event": "done", "step": "enroll"}
    doc = json.loads(out_json.read_text(encoding="utf-8"))
    assert len(doc["embedding"]) == 256
    assert doc["duration_ms"] == 16_000
    assert clip_out.exists()
