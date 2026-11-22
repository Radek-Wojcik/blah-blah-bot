"""Checks of the regression scripts that need neither a GPU nor models."""

from __future__ import annotations

import re
import shutil
import subprocess

import pytest

from bbb_bench import regression as rg

SCRIPTS = rg.REPO / "scripts"
POWERSHELL = shutil.which("powershell") or shutil.which("pwsh")
pytestmark = pytest.mark.skipif(POWERSHELL is None, reason="PowerShell is not installed")

NAMES = [
    "regression.ps1",
    "ten-hour-run.ps1",
    "gpu-preflight.ps1",
    "test-all.ps1",
    "check-temp-hygiene.ps1",
    "clean-test-tmp.ps1",
]


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [POWERSHELL, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", *args],
        capture_output=True,
        text=True,
        timeout=120,
    )


@pytest.mark.parametrize("name", NAMES)
def test_script_parses(name):
    command = (
        "$e = $null; [System.Management.Automation.Language.Parser]::ParseFile("
        f"'{SCRIPTS / name}', [ref]$null, [ref]$e) | Out-Null; "
        "if ($e.Count -gt 0) { $e | ForEach-Object { $_.Message }; exit 1 }"
    )
    done = subprocess.run(
        [POWERSHELL, "-NoProfile", "-Command", command], capture_output=True, text=True, timeout=60
    )
    assert done.returncode == 0, done.stdout + done.stderr


def test_gpu_step_ids_match_the_report_generator():
    text = (SCRIPTS / "regression.ps1").read_text(encoding="utf-8")
    ids = re.findall(r'@\{ id = "([a-z0-9-]+)"', text)
    assert ids == list(rg.GPU_STEPS)


def test_regression_dry_run_plans_both_parts_without_running_anything(tmp_path):
    cpu = run_script(str(SCRIPTS / "regression.ps1"), "-DryRun", "-Skip", "ui")
    assert cpu.returncode == 0, cpu.stdout + cpu.stderr
    assert "cargo test" in cpu.stdout and "worker pytest" in cpu.stdout
    assert "ui lint" not in cpu.stdout
    gpu = run_script(
        str(SCRIPTS / "regression.ps1"), "-Mode", "gpu", "-DryRun", "-AudioDir", str(tmp_path)
    )
    assert gpu.returncode == 0, gpu.stdout + gpu.stderr
    for step_id in rg.GPU_STEPS:
        assert step_id in gpu.stdout
    assert "would run" in gpu.stdout and "SKIPPED, missing" in gpu.stdout


def test_ten_hour_run_without_run_only_validates(tmp_path):
    audio = tmp_path / "audio" / "pl-03"
    audio.mkdir(parents=True)
    (audio / "mixed.wav").write_bytes(b"RIFF")
    (audio / "annotation.json").write_text("{}", encoding="utf-8")
    model = tmp_path / "model.gguf"
    llama = tmp_path / "llama-server.exe"
    model.write_bytes(b"x")
    llama.write_bytes(b"x")
    done = run_script(
        str(SCRIPTS / "ten-hour-run.ps1"),
        "-AudioDir",
        str(tmp_path / "audio"),
        "-Model",
        str(model),
        "-LlamaServer",
        str(llama),
        "-Hours",
        "2",
    )
    # The plan is printed; only the worker environment of a fresh checkout may be missing.
    assert "ten hour end-to-end run" in done.stdout
    assert "from 1 meeting(s)" in done.stdout
    assert "NOTE: 2 h is shorter than 10 h" in done.stdout
    assert "-AudioDir is not a folder" not in done.stdout
    assert "Add -Run to execute" in done.stdout
    assert not (rg.REPO / "target" / "regression" / "gpu-results" / "stage8-e2e-10h.json").exists()


def test_ten_hour_run_refuses_missing_inputs():
    done = run_script(str(SCRIPTS / "ten-hour-run.ps1"))
    assert done.returncode == 1
    assert "-AudioDir is not a folder" in done.stdout
    assert "-Model is not a file" in done.stdout


def test_test_all_lists_the_steps_of_the_selected_suites():
    done = run_script(str(SCRIPTS / "test-all.ps1"), "-List", "-Suite", "worker,benchmarks")
    assert done.returncode == 0, done.stdout + done.stderr
    assert "worker pytest" in done.stdout and "benchmarks pytest" in done.stdout
    assert "cargo" not in done.stdout and "ui lint" not in done.stdout
    skipped = run_script(str(SCRIPTS / "test-all.ps1"), "-List", "-Skip", "rust,worker,benchmarks")
    assert "ui lint" in skipped.stdout and "pytest" not in skipped.stdout
