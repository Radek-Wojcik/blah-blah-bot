"""Integration tests on the short reference recordings with real models and a GPU.

Run locally before a release (they are skipped in CI and when the data is missing):

    BBB_MINI_DIR=<folder with mini-en, mini-pl, mini-mixed> uv run pytest -m models

The reference recordings are synthetic (scripted and synthesised speech) and are not part
of the repository.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from bbb_worker import schema
from tests.support import make_meeting, run_step, wer

MINI = os.environ.get("BBB_MINI_DIR")
pytestmark = [
    pytest.mark.models,
    pytest.mark.skipif(not MINI, reason="BBB_MINI_DIR is not set"),
]


def mini(name: str) -> Path:
    return Path(MINI) / name


@pytest.fixture()
def en_meeting(tmp_path):
    """The user's microphone (one voice) plus an application track with all three voices."""
    src = mini("mini-en")
    return make_meeting(
        tmp_path,
        [
            ("mic", "microphone", src / "track-magda.wav"),
            ("meet", "app:meet.exe", src / "mixed.wav"),
        ],
        options={"language": "auto", "asr_model": "turbo", "max_speakers": 3},
    )


def full_pipeline(manifest, meeting):
    for step in ("asr", "diarize", "merge"):
        run_step(step, manifest, meeting)
    return json.loads((meeting / "work" / "transcript.json").read_text(encoding="utf-8"))


def test_pipeline_produces_a_valid_transcript_with_speakers(en_meeting):
    meeting, manifest = en_meeting
    doc = full_pipeline(manifest, meeting)
    schema.validate("transcript", doc)
    kinds = {s["kind"] for s in doc["speakers"]}
    assert "user" in kinds
    voices = [s for s in doc["speakers"] if s["kind"] == "voice"]
    assert 2 <= len(voices) <= 3
    assert doc["language"] == "en"


def test_word_error_rate_on_the_application_track(en_meeting):
    meeting, manifest = en_meeting
    doc = full_pipeline(manifest, meeting)
    hyp = " ".join(s["text"] for s in doc["segments"] if s["track"] == "meet")
    ref = (mini("mini-en") / "reference.txt").read_text(encoding="utf-8")
    # Stage 0 measured 27.0 % for turbo on this very recording (the short clip is dominated
    # by number formatting); the tolerance is two percentage points (docs/TESTING.md).
    assert wer(ref, hyp) <= 0.27 + 0.02


def test_steps_work_without_network_once_models_are_cached(en_meeting):
    meeting, manifest = en_meeting
    offline = {
        "HF_HUB_OFFLINE": "1",
        "HTTP_PROXY": "http://127.0.0.1:9",
        "HTTPS_PROXY": "http://127.0.0.1:9",
    }
    run_step("asr", manifest, meeting, env=offline)
    run_step("diarize", manifest, meeting, env=offline)
    run_step("merge", manifest, meeting, env=offline)


@pytest.mark.parametrize("step", ["asr", "diarize"])
def test_a_killed_step_leaves_no_partial_output_and_can_be_rerun(en_meeting, step):
    meeting, manifest = en_meeting
    if step == "diarize":
        run_step("asr", manifest, meeting)
    proc = subprocess.Popen(
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
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(6)
    proc.send_signal(signal.SIGTERM) if sys.platform != "win32" else proc.kill()
    proc.wait()
    out_dir = meeting / "work" / step
    for f in out_dir.glob("*.json") if out_dir.exists() else []:
        json.loads(f.read_text(encoding="utf-8"))  # every file that exists is complete
    run_step(step, manifest, meeting)
    assert any(out_dir.glob("*.json"))
    assert not list(out_dir.glob("*.tmp"))
