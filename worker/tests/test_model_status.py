"""Tests for model cache status detection (ready, older, missing) using a fake cache dir."""

from __future__ import annotations

import io
import json
from pathlib import Path

import pytest

from bbb_worker import cli, models, protocol


def _populate_snapshot(hub_dir: Path, key: str, revision: str) -> Path:
    spec = models.MODELS[key]
    snap = hub_dir / f"models--{spec.repo.replace('/', '--')}" / "snapshots" / revision
    snap.mkdir(parents=True, exist_ok=True)
    files = spec.patterns if spec.patterns else ("config.yaml",)
    for name in files:
        (snap / name).write_bytes(b"placeholder")
    return snap


@pytest.fixture
def isolated_cache(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    hub_dir = tmp_path / "hf_hub"
    hub_dir.mkdir()
    monkeypatch.delenv("BBB_MODELS_DIR", raising=False)
    monkeypatch.delenv("BBB_VISION_DIR", raising=False)
    monkeypatch.delenv("HF_HOME", raising=False)
    monkeypatch.setenv("HF_HUB_CACHE", str(hub_dir))
    return hub_dir


def test_cache_status_distinguishes_ready_older_and_missing(isolated_cache: Path) -> None:
    # Pinned revision -> ready
    pinned_whisper = models.MODELS["whisper-turbo"].revision
    _populate_snapshot(isolated_cache, "whisper-turbo", pinned_whisper)

    # Non-pinned revision -> older
    _populate_snapshot(isolated_cache, "vision", "0" * 40)

    # Empty non-pinned snapshot -> still missing
    empty_snap = (
        isolated_cache
        / f"models--{models.MODELS['diarization'].repo.replace('/', '--')}"
        / "snapshots"
        / ("1" * 40)
    )
    empty_snap.mkdir(parents=True)

    assert models.cache_status("whisper-turbo") == "ready"
    assert models.is_cached("whisper-turbo") is True

    assert models.cache_status("vision") == "older"
    assert models.is_cached("vision") is False

    assert models.cache_status("diarization") == "missing"
    assert models.is_cached("diarization") is False

    assert models.cache_status("whisper-large-v3") == "missing"
    assert models.is_cached("whisper-large-v3") is False


def test_cli_models_status_emits_cached_and_states(
    isolated_cache: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _populate_snapshot(isolated_cache, "whisper-turbo", models.MODELS["whisper-turbo"].revision)
    _populate_snapshot(isolated_cache, "vision", "a" * 40)

    buf = io.StringIO()
    monkeypatch.setattr(protocol, "_OUT", buf)
    rc = cli.main(["models", "--status"])
    assert rc == 0

    payload = json.loads(buf.getvalue().strip())
    assert payload["event"] == "models"
    assert payload["cached"]["whisper-turbo"] is True
    assert payload["cached"]["vision"] is False
    assert payload["cached"]["diarization"] is False
    assert payload["states"]["whisper-turbo"] == "ready"
    assert payload["states"]["vision"] == "older"
    assert payload["states"]["diarization"] == "missing"
