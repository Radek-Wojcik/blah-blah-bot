"""The extract and reduce steps through the command line, as the application starts them."""

import json
import os
import subprocess
import sys
from pathlib import Path

from bbb_worker import cli, schema

from .llm_support import (
    FakeLlm,
    by_kind,
    context_doc,
    extraction_for,
    llm_options,
    make_meeting,
    segment,
    transcript_doc,
)

SRC = Path(__file__).resolve().parents[1] / "src"
MOUNT = "Alex, can you have the mount ready by next Friday?"


def run_cli(step, manifest_path, meeting_dir, env=None):
    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "bbb_worker",
            step,
            "--manifest",
            str(manifest_path),
            "--meeting-dir",
            str(meeting_dir),
        ],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
        env={**os.environ, "PYTHONPATH": str(SRC), **(env or {})},
    )
    events = [json.loads(line) for line in proc.stdout.splitlines() if line.startswith("{")]
    return proc.returncode, events, proc.stderr


def small_meeting(tmp_path, options):
    doc = transcript_doc([segment(0, 0, 5_000, MOUNT)])
    return make_meeting(tmp_path, doc, context_doc(mode="fixed"), options)


def test_steps_are_registered():
    assert {"extract", "reduce"} <= set(cli.STEPS)


def test_extract_and_reduce_follow_the_protocol(tmp_path):
    rules = [{"quote": MOUNT, "text": "Alex prepares the mount", "fields": {"assignee": "alex"}}]
    with FakeLlm(by_kind(extraction=extraction_for(rules))) as fake:
        m = small_meeting(tmp_path, llm_options(fake.url))
        manifest_path = tmp_path / "manifest.json"
        for step in ("extract", "reduce"):
            code, events, err = run_cli(step, manifest_path, m.meeting_dir)
            assert code == 0, err
            assert events[0]["event"] == "progress" and events[0]["message"] == "started"
            assert events[-1] == {"event": "done", "step": step}
            assert all(e["step"] == step for e in events)
    draft = json.loads((m.work_dir / "draft.json").read_text(encoding="utf-8"))
    schema.validate("meeting", draft)
    assert draft["items"][0]["assignee"] == "alex"
    assert not list(m.work_dir.rglob("*.tmp"))


def test_cloud_without_confirmation_exits_with_an_error_and_no_request(tmp_path):
    with FakeLlm(extraction_for([])) as fake:
        m = small_meeting(tmp_path, llm_options(fake.url, "cloud"))
        code, events, _err = run_cli(
            "extract", tmp_path / "manifest.json", m.meeting_dir, {"BBB_LLM_API_KEY": "secret-key"}
        )
        assert fake.requests == []
    assert code == 1
    assert events[-1]["event"] == "error" and "not confirmed" in events[-1]["message"]
    assert "secret-key" not in json.dumps(events)
