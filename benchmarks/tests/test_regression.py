"""Unit tests of the regression report on synthetic logs and result files."""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path

import pytest

from bbb_bench import regression as rg

CARGO_LOG = """\
   Compiling queue v0.5.0
     Running unittests src\\lib.rs (target\\debug\\deps\\queue-0123456789abcdef.exe)

running 3 tests
test gpu::tests::busy_gpu_never_starts ... ok
test supervisor::tests::failure_blocks_the_job_and_retry_resumes_from_that_stage ... ok
test supervisor::tests::slow_one ... ignored, needs a GPU

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.1s

     Running tests\\contract_stage4.rs (target\\debug\\deps\\contract_stage4-fedcba9876543210.exe)

running 1 test
test extraction_fixture_validates ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.0s

     Running tests\\session_tones.rs (target\\debug\\deps\\session_tones-00112233445566aa.exe)

running 2 tests
test killed_process_loses_at_most_one_segment ... ok
test two_tones_are_recorded ... FAILED

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.0s

   Doc-tests queue

running 1 test
test src\\lib.rs - (line 1) ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.0s
"""

JUNIT = """\
<testsuites><testsuite name="pytest" tests="4">
<testcase classname="tests.test_contracts" name="test_transcript_fixture" time="0.1"/>
<testcase classname="tests.test_extract"
  name="test_invalid_answer_is_retried_with_the_error_in_the_prompt"/>
<testcase classname="tests.test_extract"
  name="test_window_is_skipped_after_the_retries_and_the_stage_continues">
  <failure message="boom">trace</failure></testcase>
<testcase classname="tests.test_windows" name="test_skipped"><skipped message="x"/></testcase>
</testsuite></testsuites>
"""


def write_json(path: Path, doc: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc), encoding="utf-8")


# --- parsing -------------------------------------------------------------------------------


def test_cargo_log_is_split_by_binary_and_totals_are_summed():
    tests, totals = rg.parse_cargo_log(CARGO_LOG)
    assert totals == {"passed": 5, "failed": 1, "ignored": 1}
    by_name = {t["name"]: t for t in tests}
    assert by_name["gpu::tests::busy_gpu_never_starts"]["kind"] == "unit"
    assert by_name["gpu::tests::busy_gpu_never_starts"]["crate"] == "queue"
    assert by_name["extraction_fixture_validates"]["binary"] == "tests/contract_stage4.rs"
    assert by_name["extraction_fixture_validates"]["kind"] == "integration"
    assert by_name["two_tones_are_recorded"]["status"] == "failed"
    assert by_name["supervisor::tests::slow_one"]["status"] == "ignored"
    assert [t["kind"] for t in tests][-1] == "doc"


def test_contract_detection_uses_the_binary_and_the_test_name():
    assert rg.is_contract("tests/contract_stage4.rs", "anything")
    assert rg.is_contract("unittests", "transcript_fixture_loads")
    assert not rg.is_contract("unittests", "busy_gpu_never_starts")


def test_junit_cases_have_a_status():
    cases = rg.parse_junit(JUNIT)
    assert [c["status"] for c in cases] == ["passed", "passed", "failed", "skipped"]


def test_tool_summaries():
    assert rg.parse_pytest_summary("....\n123 passed, 2 skipped, 5 deselected in 3.4s\n") == {
        "passed": 123,
        "skipped": 2,
        "deselected": 5,
    }
    assert rg.parse_pytest_summary("1 failed, 4 passed in 0.2s") == {"failed": 1, "passed": 4}
    vitest = " Test Files  3 passed (3)\n      Tests  1 failed | 209 passed (210)\n"
    assert rg.parse_vitest(vitest) == {"failed": 1, "passed": 209}
    playwright = "Running 47 tests using 1 worker\n  2 skipped\n  45 passed (1.1m)\n"
    assert rg.parse_playwright(playwright) == {"skipped": 2, "passed": 45}


# --- suites and categories -----------------------------------------------------------------


def make_logs(tmp_path: Path, failed: str | None = None) -> Path:
    logs = tmp_path / "logs"
    logs.mkdir()
    steps = [
        ("rust", "cargo fmt --check", "01-cargo-fmt-check.log", ""),
        ("rust", "cargo test", "03-cargo-test.log", CARGO_LOG),
        ("worker", "worker pytest", "04-worker-pytest.log", "4 passed in 1s"),
        ("ui", "ui unit tests", "07-ui-unit-tests.log", "      Tests  10 passed (10)\n"),
        ("ui", "ui mock end-to-end tests", "09-e2e.log", "  45 passed (1.0m)\n"),
    ]
    rows = []
    for group, name, log, text in steps:
        (logs / log).write_text(text, encoding="utf-8")
        status = "failed" if name == failed else "passed"
        rows.append({"group": group, "name": name, "status": status, "seconds": 1.5, "log": log})
    (logs / "04-worker-pytest.junit.xml").write_text(JUNIT, encoding="utf-8")
    (logs / "steps.json").write_text(json.dumps(rows), encoding="utf-8-sig")
    return logs


def test_suites_and_categories(tmp_path):
    suites, cargo, cases = rg.load_suites(make_logs(tmp_path))
    by_name = {s["name"]: s for s in suites}
    assert by_name["cargo test"]["passed"] == 5 and by_name["cargo test"]["failed"] == 1
    assert by_name["worker pytest"]["passed"] == 2 and by_name["worker pytest"]["failed"] == 1
    assert by_name["ui mock end-to-end tests"]["passed"] == 45
    cats = rg.build_categories(suites, cargo, cases)
    assert cats["e2e_mock"]["passed"] == 45
    assert cats["integration"]["passed"] == 1 and cats["integration"]["failed"] == 1
    # one cargo contract test and one pytest contract test
    assert cats["contract"]["passed"] == 2
    assert cats["static"]["passed"] == 1
    assert cats["unit"]["failed"] == 1  # the failing pytest case


def test_without_logs_nothing_is_invented(tmp_path):
    assert rg.load_suites(None) == ([], [], [])
    assert rg.load_suites(tmp_path / "missing") == ([], [], [])


# --- thresholds ----------------------------------------------------------------------------

P2 = {
    "status": "ok",
    "meetings": [
        {"meeting": "pl-03", "wer": 0.09, "term_recall": None, "translation_rate": 0.05},
        {"meeting": "en-03", "wer": 0.14, "term_recall": None, "translation_rate": 0.0},
        {"meeting": "mixed-04", "wer": 0.16, "term_recall": 0.35, "translation_rate": 0.02},
    ],
}
P3 = {
    "status": "ok",
    "runs": [
        {"meeting": "pl-03", "mode": "bounded", "reference_speakers": 3, "der": 0.29},
        {"meeting": "pl-08", "mode": "bounded", "reference_speakers": 8, "der": 0.25},
        {"meeting": "pl-16", "mode": "bounded", "reference_speakers": 16, "der": 0.27},
    ],
}


def baselines(tmp_path: Path) -> Path:
    base = tmp_path / "baselines"
    write_json(base / "p2-asr-turbo-auto-nogloss.json", P2)
    write_json(base / "p3-diarize-community-1.json", P3)
    write_json(
        base / "stage3-speaker-recognition.json",
        {"speaker_accuracy_auto": 1.0, "false_accept_rate": 0.0},
    )
    write_json(
        base / "stage1-soak-10h.json",
        {
            "cpu_pct_of_total_avg": 0.017,
            "working_set_mb_peak": 10.1,
            "tracks": [
                {"offset_ms": 16.2, "lost_buffer_events": 194},
                {"offset_ms": 2.6, "lost_buffer_events": 0},
            ],
        },
    )
    return base


def row(rows: list[dict], id_: str) -> dict:
    return next(r for r in rows if r["id"] == id_)


def test_baseline_only_rows_are_marked_and_judged_against_the_target(tmp_path):
    rows = rg.evaluate_thresholds(baselines(tmp_path), None)
    wer = row(rows, "wer-pl")
    assert wer["value"] == pytest.approx(0.09) and wer["status"] == "pass"
    assert wer["source"] == "baseline" and wer["regression"] == "baseline_only"
    assert row(rows, "wer-mixed")["status"] == "pass"
    assert row(rows, "term-recall")["status"] == "fail"
    assert row(rows, "translation-rate")["value"] == pytest.approx(0.05)
    assert row(rows, "der-2-4")["value"] == pytest.approx(0.29)
    assert row(rows, "der-2-4")["status"] == "fail"
    assert row(rows, "der-6-10")["status"] == "pass"
    assert row(rows, "der-15")["status"] == "no_target"
    assert row(rows, "wer-en")["status"] == "no_target"
    assert row(rows, "rec-dropped")["value"] == 194 and row(rows, "rec-dropped")["status"] == "fail"
    assert row(rows, "rec-alignment")["value"] == pytest.approx(16.2)
    assert row(rows, "task-recall")["status"] == "not_measured"
    assert row(rows, "task-recall")["regression"] == "no_baseline"


def test_a_kill_test_proves_the_loss_target_without_a_number(tmp_path):
    base = baselines(tmp_path)
    no_proof = row(rg.evaluate_thresholds(base, None, set()), "rec-kill-loss")
    assert no_proof["status"] == "not_measured"
    proof = row(
        rg.evaluate_thresholds(base, None, {"killed_process_loses_at_most_one_segment"}),
        "rec-kill-loss",
    )
    assert proof["status"] == "pass" and proof["source"] == "test"


def test_fresh_values_give_deltas_and_regressions(tmp_path):
    base = baselines(tmp_path)
    fresh = tmp_path / "fresh"
    worse = json.loads(json.dumps(P2))
    worse["meetings"][0]["wer"] = 0.12  # +3 points on the Polish set
    write_json(fresh / "p2-asr-turbo-auto-nogloss.json", worse)
    rows = rg.evaluate_thresholds(base, fresh)
    wer = row(rows, "wer-pl")
    assert wer["source"] == "fresh" and wer["status"] == "pass"
    assert wer["delta"] == pytest.approx(0.03)
    assert wer["regression"] == "regressed"
    # English: +0 points
    assert row(rows, "wer-en")["regression"] == "ok"

    within = json.loads(json.dumps(P2))
    within["meetings"][0]["wer"] = 0.1  # +1 point: inside the tolerance
    write_json(fresh / "p2-asr-turbo-auto-nogloss.json", within)
    assert row(rg.evaluate_thresholds(base, fresh), "wer-pl")["regression"] == "ok"


def test_quality_that_improves_is_not_a_regression_and_failed_runs_are_ignored(tmp_path):
    base = baselines(tmp_path)
    fresh = tmp_path / "fresh"
    write_json(fresh / "stage3-speaker-recognition.json", {"status": "error", "error": "x"})
    rows = rg.evaluate_thresholds(base, fresh)
    assert row(rows, "speaker-accuracy")["source"] == "baseline"
    better = json.loads(json.dumps(P3))
    better["runs"][0]["der"] = 0.1
    write_json(fresh / "p3-diarize-community-1.json", better)
    der = row(rg.evaluate_thresholds(base, fresh), "der-2-4")
    assert der["status"] == "pass" and der["regression"] == "ok"


def test_perf_regression_uses_a_relative_tolerance():
    spec = next(t for t in rg.THRESHOLDS if t.id == "time-10h")
    assert rg.regression_of(spec, 1.0, 1.2) == "ok"
    assert rg.regression_of(spec, 1.0, 1.3) == "regressed"
    rate = next(t for t in rg.THRESHOLDS if t.id == "task-recall")
    assert rg.regression_of(rate, 0.86, 0.84) == "ok"
    assert rg.regression_of(rate, 0.86, 0.83) == "regressed"


def test_full_pipeline_time_and_vram_combine_the_stage_files(tmp_path):
    base = tmp_path / "b"
    write_json(
        base / "stage2-pipeline-2h.json",
        {"total_minutes_for_2h": 6.1, "peak_vram_absolute_mib": 8495},
    )
    write_json(
        base / "stage4-long-gemma-4-12b-q4.json",
        {"minutes_for_2h": 10.3, "peak_vram_absolute_mib": 9743},
    )
    write_json(base / "p9-long-estimate.json", {"total_hours": 0.97})
    rows = rg.evaluate_thresholds(base, None)
    assert row(rows, "time-2h")["value"] == pytest.approx(16.4)
    assert row(rows, "peak-vram")["value"] == 9743
    assert row(rows, "time-10h")["value"] == pytest.approx(0.97)
    fresh = tmp_path / "f"
    write_json(
        fresh / "stage8-e2e-10h.json",
        {"total_hours": 5.0, "full_length": True, "peak_vram_absolute_mib": 12000},
    )
    rows = rg.evaluate_thresholds(base, fresh)
    assert row(rows, "time-10h")["status"] == "fail" and row(rows, "time-10h")["source"] == "fresh"
    assert row(rows, "peak-vram")["status"] == "fail"
    # time-2h needs both of its fresh files, so it stays on the baseline
    assert row(rows, "time-2h")["source"] == "baseline"


# --- resilience, recommendation, report ----------------------------------------------------


def test_resilience_has_the_eleven_rows_of_the_testing_document():
    rows = rg.build_resilience([], [], rg.load_gpu_steps(None))
    assert len(rows) == 11
    assert [r["coverage"] for r in rows].count("manual") == 2
    assert [r["coverage"] for r in rows].count("pending_merge") == 4
    assert all(r["status"] in ("manual", "pending_merge", "not_run") for r in rows)


def test_resilience_status_follows_cpu_tests_and_the_gpu_step():
    tests = [
        {
            "crate": "queue",
            "binary": "unittests",
            "kind": "unit",
            "status": "passed",
            "name": "supervisor::tests::failure_blocks_the_job_and_retry_resumes_from_that_stage",
        },
        {
            "crate": "queue",
            "binary": "unittests",
            "kind": "unit",
            "status": "passed",
            "name": "supervisor::tests::recover_resumes_an_interrupted_stage",
        },
    ]
    gpu = [{"id": "pipeline-models", "status": "not_run"}]
    worker_killed = next(
        r for r in rg.build_resilience(tests, [], gpu) if r["fault"].startswith("Worker killed")
    )
    assert worker_killed["status"] == "partial"
    gpu = [{"id": "pipeline-models", "status": "passed"}]
    worker_killed = next(
        r for r in rg.build_resilience(tests, [], gpu) if r["fault"].startswith("Worker killed")
    )
    assert worker_killed["status"] == "passed"
    gpu = [{"id": "pipeline-models", "status": "failed"}]
    worker_killed = next(
        r for r in rg.build_resilience(tests, [], gpu) if r["fault"].startswith("Worker killed")
    )
    assert worker_killed["status"] == "failed"


def test_gpu_steps_default_to_not_run_and_follow_the_recorded_file(tmp_path):
    rows = rg.load_gpu_steps(None)
    assert {r["id"] for r in rows if r["status"] == "not_run"} == set(rg.GPU_STEPS)
    write_json(
        tmp_path / "gpu-steps.json",
        [
            {"id": "asr", "status": "passed", "seconds": 90.5},
            {"id": "diarization", "status": "skipped", "reason": "no audio dir"},
        ],
    )
    rows = {r["id"]: r for r in rg.load_gpu_steps(tmp_path)}
    assert rows["asr"]["status"] == "passed" and rows["asr"]["seconds"] == 90.5
    assert rows["diarization"]["note"] == "no audio dir"
    assert rows["extraction"]["status"] == "not_run"


def test_recommendation_levels(tmp_path):
    base = baselines(tmp_path)
    logs = make_logs(tmp_path, failed="cargo test")
    report = rg.build_report(logs, None, base, hygiene_count=0)
    rec = report["recommendation"]
    assert rec["verdict"] == "no-go"
    assert any("cargo test" in b for b in rec["blocking"])  # failing suite from the log set

    healthy = rg.recommend([], {}, [], [], [], hygiene_count=0)
    assert healthy["verdict"] == "incomplete"  # no suite was run
    suites = [{"name": "x", "status": "passed"}]
    assert rg.recommend(suites, {}, [], [], [], hygiene_count=0)["verdict"] == "go"
    assert rg.recommend(suites, {}, [], [], [], hygiene_count=2)["verdict"] == "no-go"
    assert rg.recommend(suites, {}, [], [], [], hygiene_count=None)["verdict"] == "incomplete"
    miss = [
        {
            "id": "m",
            "measure": "M",
            "status": "fail",
            "target": "<= 1",
            "source": "fresh",
            "regression": "ok",
        }
    ]
    assert rg.recommend(suites, {}, miss, [], [], 0)["verdict"] == "no-go"
    accepted = rg.recommend(suites, {}, miss, [], [], 0, accepted={"m": "known miss"})
    assert accepted["verdict"] == "go" and "known miss" in accepted["accepted"][0]
    gpu_failed = [{"id": "asr", "status": "failed"}]
    assert rg.recommend(suites, {}, [], [], gpu_failed, 0)["verdict"] == "no-go"


def test_report_and_markdown_cover_every_section(tmp_path):
    report = rg.build_report(
        make_logs(tmp_path),
        None,
        baselines(tmp_path),
        0,
        now=datetime(2026, 11, 22, tzinfo=UTC),
    )
    assert report["generated_at"].startswith("2026-11-22")
    assert report["parts"] == {"cpu": True, "gpu": False}
    assert len(report["thresholds"]) == len(rg.THRESHOLDS)
    md = rg.render_markdown(report)
    for heading in (
        "# Stage 8 regression report",
        "## Suites",
        "## Test kinds",
        "## Thresholds and baselines",
        "## GPU and manual steps",
        "## Resilience",
    ):
        assert heading in md
    assert "Recommendation: NO-GO" in md
    assert "baseline only, not re-measured" in md
    assert "| Polish" not in md  # no stray formatting from the synthetic data
    json.dumps(report)  # serialisable


def test_cli_writes_both_files(tmp_path, capsys):
    out = tmp_path / "out"
    code = rg.main(
        [
            "--logs",
            str(make_logs(tmp_path)),
            "--baselines",
            str(baselines(tmp_path)),
            "--out",
            str(out),
            "--hygiene-count",
            "0",
        ]
    )
    assert code == 0
    doc = json.loads((out / "stage8-regression.json").read_text(encoding="utf-8"))
    assert doc["recommendation"]["verdict"] in ("go", "no-go", "incomplete")
    assert (out / "stage8-regression.md").read_text(encoding="utf-8").startswith("# Stage 8")
    assert "stage8-regression:" in capsys.readouterr().out


def test_value_formatting():
    assert rg.fmt_value(0.0891, "rate") == "8.9 %"
    assert rg.fmt_value(None, "rate") == "-"
    assert rg.fmt_value(9743, "mib") == "9743 MiB"
    assert rg.fmt_delta({"delta": 0.012, "unit": "rate"}) == "+1.2 pp"
    assert rg.fmt_delta({"delta": None, "unit": "rate"}) == "-"


def test_a_partial_long_run_is_not_a_ten_hour_result(tmp_path):
    write_json(
        tmp_path / "stage8-e2e-10h.json",
        {
            "status": "ok",
            "hours_of_audio": 6.0,
            "full_length": False,
            "passed": True,
            "total_hours": 1.9,
            "total_seconds": 6840,
            "peak_vram_absolute_mib": 9000,
        },
    )
    steps = {s["id"]: s for s in rg.load_gpu_steps(tmp_path)}
    assert steps["ten-hour"]["status"] == "partial"
    assert "not met" in steps["ten-hour"]["note"]
    rows = rg.evaluate_thresholds(None, tmp_path)
    assert row(rows, "time-10h")["value"] is None  # not a 10 h measurement
    assert row(rows, "peak-vram")["value"] == 9000  # the memory peak still counts
    rec = rg.recommend([{"name": "x", "status": "passed"}], {}, [], [], list(steps.values()), 0)
    assert any("only partly done" in item for item in rec["open"])
    write_json(
        tmp_path / "stage8-e2e-10h.json",
        {
            "status": "ok",
            "hours_of_audio": 10.0,
            "full_length": True,
            "passed": True,
            "total_hours": 3.2,
        },
    )
    steps = {s["id"]: s for s in rg.load_gpu_steps(tmp_path)}
    assert steps["ten-hour"]["status"] == "passed"
    assert row(rg.evaluate_thresholds(None, tmp_path), "time-10h")["status"] == "pass"
    write_json(
        tmp_path / "stage8-e2e-10h.json",
        {
            "status": "ok",
            "hours_of_audio": 10.0,
            "full_length": True,
            "passed": False,
            "error": "stage extract failed",
        },
    )
    steps = {s["id"]: s for s in rg.load_gpu_steps(tmp_path)}
    assert steps["ten-hour"]["status"] == "failed"
