"""Locations of untracked data (models, reference clips). Everything lives under benchmarks/data."""

from __future__ import annotations

import os
from pathlib import Path

# .../benchmarks/tools/tts/src/bbb_tts/paths.py -> .../benchmarks
BENCHMARKS_DIR = Path(__file__).resolve().parents[4]


def data_dir() -> Path:
    """Directory for models and reference clips (override with BBB_TTS_DATA)."""
    override = os.environ.get("BBB_TTS_DATA")
    return Path(override) if override else BENCHMARKS_DIR / "data" / "tts"


def piper_dir() -> Path:
    return data_dir() / "piper"


def refs_dir() -> Path:
    return data_dir() / "refs"


def hf_cache_dir() -> Path:
    return data_dir() / "hf"
