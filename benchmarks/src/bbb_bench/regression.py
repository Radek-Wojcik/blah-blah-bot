r"""Stage 8 regression report: suites, thresholds, baseline deltas and a go/no-go recommendation.

    uv run python -m bbb_bench.regression \
        --logs ../target/regression/logs --gpu-results ../target/regression/gpu-results \
        --out ../target/regression/report

Inputs (all optional, a missing input is reported as "not run" or "baseline only"):

- `--logs`: the folder `scripts/test-all.ps1 -LogDir` wrote (`steps.json`, one log per step,
  JUnit files of pytest);
- `--gpu-results`: the folder of a GPU run (`scripts/regression.ps1 -Mode gpu`): fresh result
  files with the same names as the recorded ones and `gpu-steps.json`;
- `benchmarks/results/`: the recorded results, which are the baselines;
- `--hygiene-count`: directories left in the temp folders (0 passes).

Outputs: `stage8-regression.json` and `stage8-regression.md` in `--out`.

The thresholds are those of docs/TESTING.md section 5 (`THRESHOLDS` below). A quality metric
may not get worse than its baseline by more than two percentage points (`RATE_TOLERANCE`);
times, memory and counts may not get worse by more than a quarter (`PERF_TOLERANCE`).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import xml.etree.ElementTree as ET
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
RESULTS_DIR = REPO / "benchmarks" / "results"

RATE_TOLERANCE = 0.02
PERF_TOLERANCE = 0.25
PROTOTYPE = "stage8-regression"

Docs = dict[str, dict[str, Any]]

# ---------------------------------------------------------------------------------------------
# Log parsing
# ---------------------------------------------------------------------------------------------

_CARGO_RUNNING = re.compile(r"^\s*Running (unittests )?(.+?) \((.+)\)\s*$")
_CARGO_DOC = re.compile(r"^\s*Doc-tests (\S+)")
_CARGO_TEST = re.compile(r"^test (.+?) \.\.\. (ok|FAILED|ignored)\b")
_CARGO_RESULT = re.compile(
    r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored"
)
_CONTRACT_WORDS = re.compile(r"contract|schema|fixture", re.IGNORECASE)


def parse_cargo_log(text: str) -> tuple[list[dict[str, str]], dict[str, int]]:
    """Per-test lines (with the binary they ran in) and the summed `test result:` totals."""
    tests: list[dict[str, str]] = []
    totals = {"passed": 0, "failed": 0, "ignored": 0}
    crate, binary, kind = "", "", "unit"
    for line in text.splitlines():
        m = _CARGO_RUNNING.match(line)
        if m:
            unit, source, exe = m.groups()
            stem = re.split(r"[\\/]", exe)[-1]
            crate = re.sub(r"-[0-9a-f]{8,}(\.exe)?$", "", stem)
            binary = "unittests" if unit else source.replace("\\", "/")
            kind = "unit" if unit else "integration"
            continue
        m = _CARGO_DOC.match(line)
        if m:
            crate, binary, kind = m.group(1), "doc-tests", "doc"
            continue
        m = _CARGO_TEST.match(line)
        if m:
            name, outcome = m.groups()
            status = {"ok": "passed", "FAILED": "failed", "ignored": "ignored"}[outcome]
            tests.append(
                {"crate": crate, "binary": binary, "kind": kind, "name": name, "status": status}
            )
            continue
        m = _CARGO_RESULT.match(line)
        if m:
            for key, value in zip(("passed", "failed", "ignored"), m.groups(), strict=True):
                totals[key] += int(value)
    return tests, totals


def is_contract(binary: str, name: str) -> bool:
    return bool(_CONTRACT_WORDS.search(binary) or _CONTRACT_WORDS.search(name))


def parse_junit(text: str) -> list[dict[str, str]]:
    """Test cases of a pytest JUnit file: classname, name and passed/failed/skipped."""
    root = ET.fromstring(text)
    cases = []
    for case in root.iter("testcase"):
        if case.find("failure") is not None or case.find("error") is not None:
            status = "failed"
        elif case.find("skipped") is not None:
            status = "skipped"
        else:
            status = "passed"
        cases.append(
            {"classname": case.get("classname", ""), "name": case.get("name", ""), "status": status}
        )
    return cases


_COUNT = re.compile(r"(\d+) (passed|failed|skipped|flaky|interrupted|errors?|deselected|todo)")


def _last_counts(lines: Iterable[str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for line in lines:
        for number, word in _COUNT.findall(line):
            word = "failed" if word.startswith("error") else word
            counts[word] = counts.get(word, 0) + int(number)
    return counts


def parse_pytest_summary(text: str) -> dict[str, int]:
    """The last summary line, e.g. `123 passed, 2 skipped, 5 deselected in 3.4s`."""
    for line in reversed(text.splitlines()):
        if re.search(r"\b(passed|failed|errors?)\b.* in [\d.]+s", line):
            return _last_counts([line])
    return {}


def parse_vitest(text: str) -> dict[str, int]:
    """The `Tests  1 failed | 209 passed (210)` line (not `Test Files`)."""
    for line in reversed(text.splitlines()):
        if re.match(r"^\s*Tests\s+\d", line):
            return _last_counts([line])
    return {}


def parse_playwright(text: str) -> dict[str, int]:
    """The summary lines at the end of a Playwright run (`  45 passed (1.1m)`)."""
    lines = [
        line
        for line in text.splitlines()
        if re.match(r"^\s+\d+ (passed|failed|skipped|flaky|interrupted)\b", line)
    ]
    return _last_counts(lines)


# ---------------------------------------------------------------------------------------------
# Suites and categories
# ---------------------------------------------------------------------------------------------

STATIC_STEPS = ("cargo fmt --check", "cargo clippy", "ui typecheck", "ui lint", "ui format:check")
CATEGORIES = {
    "static": "Formatting, lint and type checks",
    "unit": "Unit tests (Rust, worker, benchmarks, UI)",
    "contract": "Contract tests (schemas and shared fixtures)",
    "integration": "Integration tests (Rust test binaries without models)",
    "e2e_mock": "End-to-end tests of the UI on the mock backend",
}


def _read(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None


def load_suites(
    log_dir: Path | None,
) -> tuple[list[dict[str, Any]], list[dict[str, str]], list[dict[str, str]]]:
    """Suite rows from `steps.json`, plus the per-test data of cargo and pytest."""
    suites: list[dict[str, Any]] = []
    cargo_tests: list[dict[str, str]] = []
    pytest_cases: list[dict[str, str]] = []
    steps_file = (log_dir / "steps.json") if log_dir else None
    raw = _read(steps_file) if steps_file else None
    if raw is None:
        return suites, cargo_tests, pytest_cases
    steps = json.loads(raw.lstrip("﻿") or "[]")
    for step in steps if isinstance(steps, list) else [steps]:
        name = step["name"]
        log = _read(log_dir / step["log"]) or ""
        counts: dict[str, int] = {}
        category = "static"
        if name == "cargo test":
            tests, counts = parse_cargo_log(log)
            cargo_tests.extend(tests)
            category = "unit"
        elif name.endswith("pytest"):
            junit = _read(log_dir / step["log"].replace(".log", ".junit.xml"))
            if junit:
                cases = parse_junit(junit)
                pytest_cases.extend(cases)
                counts = {
                    s: sum(1 for c in cases if c["status"] == s)
                    for s in ("passed", "failed", "skipped")
                }
            else:
                counts = parse_pytest_summary(log)
            category = "unit"
        elif name == "ui unit tests":
            counts = parse_vitest(log)
            category = "unit"
        elif name == "ui mock end-to-end tests":
            counts = parse_playwright(log)
            category = "e2e_mock"
        suites.append(
            {
                "id": re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-"),
                "name": name,
                "group": step.get("group", ""),
                "category": category,
                "status": step["status"],
                "seconds": step.get("seconds"),
                "passed": counts.get("passed", 0),
                "failed": counts.get("failed", 0),
                "skipped": counts.get("skipped", 0) + counts.get("deselected", 0),
                "ignored": counts.get("ignored", 0),
                "log": step["log"],
            }
        )
    return suites, cargo_tests, pytest_cases


def build_categories(
    suites: list[dict[str, Any]],
    cargo_tests: list[dict[str, str]],
    pytest_cases: list[dict[str, str]],
) -> dict[str, dict[str, Any]]:
    """Counts per kind of test. Contract tests are carved out of the suites that hold them."""
    cats = {
        key: {"title": title, "passed": 0, "failed": 0, "ignored": 0, "suites": []}
        for key, title in CATEGORIES.items()
    }
    ran = {s["name"] for s in suites}

    def add(cat: str, status: str, n: int = 1) -> None:
        cats[cat][
            "passed" if status == "passed" else "failed" if status == "failed" else "ignored"
        ] += n

    for s in suites:
        cat = s["category"]
        cats[cat]["suites"].append(s["name"])
        if cat == "static":
            add("static", "passed" if s["status"] == "passed" else "failed")
        elif cat == "e2e_mock":
            add("e2e_mock", "passed", s["passed"])
            add("e2e_mock", "failed", s["failed"])
            add("e2e_mock", "ignored", s["skipped"])
        elif s["name"] == "ui unit tests":
            add("unit", "passed", s["passed"])
            add("unit", "failed", s["failed"])
            add("unit", "ignored", s["skipped"])
    # Per-test data where the tools give it: cargo lines and pytest cases.
    for t in cargo_tests:
        if is_contract(t["binary"], t["name"]):
            cat = "contract"
        elif t["kind"] == "integration":
            cat = "integration"
        else:
            cat = "unit"
        add(cat, t["status"])
    for c in pytest_cases:
        cat = "contract" if is_contract(c["classname"], c["name"]) else "unit"
        add(
            cat,
            "passed"
            if c["status"] == "passed"
            else "failed"
            if c["status"] == "failed"
            else "skipped",
        )
    # A pytest suite without a JUnit file still counts as unit tests.
    if "worker pytest" in ran or "benchmarks pytest" in ran:
        have = {c["classname"].split(".")[0] for c in pytest_cases}
        if not have:
            for s in suites:
                if s["name"].endswith("pytest"):
                    add("unit", "passed", s["passed"])
                    add("unit", "failed", s["failed"])
                    add("unit", "ignored", s["skipped"])
    for cat in cats.values():
        total = cat["passed"] + cat["failed"]
        cat["status"] = "not_run" if total == 0 else "failed" if cat["failed"] else "passed"
    return cats


# ---------------------------------------------------------------------------------------------
# GPU steps
# ---------------------------------------------------------------------------------------------

# Keep in sync with the `$gpuSteps` table of scripts/regression.ps1 (a test compares them).
GPU_STEPS: dict[str, str] = {
    "worker-models": "Worker pytest with real models (BBB_MINI_DIR, slides, vision, llama-server)",
    "pipeline-models": "Supervisor pipeline: end to end, worker killed per stage, GPU occupied",
    "speakers-models": "Speaker recognition accuracy over seven meetings",
    "stage4-models": "Whole pipeline with extract and reduce on pl-03 and en-03",
    "asr": "ASR turbo, auto language, on the Polish, English and mixed meetings",
    "diarization": "Diarization (community-1) on 2-4, 8 and 16 speaker meetings",
    "extraction": "Extraction quality on the labeled meetings",
    "extraction-2h": "Extraction timing on two hours of transcript",
    "pipeline-2h": "Worker steps on a two hour recording: time and peak VRAM",
}
MANUAL_STEPS: dict[str, str] = {
    "real-app": "Real application end to end (`npm run test:real`, user at the machine)",
    "ten-hour": "Ten hour end-to-end run (`scripts/ten-hour-run.ps1`)",
}


def load_gpu_steps(gpu_dir: Path | None) -> list[dict[str, Any]]:
    recorded: dict[str, dict[str, Any]] = {}
    raw = _read(gpu_dir / "gpu-steps.json") if gpu_dir else None
    if raw:
        data = json.loads(raw.lstrip("﻿"))
        for row in data if isinstance(data, list) else [data]:
            recorded[row["id"]] = row
    rows = []
    for step_id, title in GPU_STEPS.items():
        row = recorded.get(step_id)
        rows.append(
            {
                "id": step_id,
                "title": title,
                "status": row["status"] if row else "not_run",
                "seconds": row.get("seconds") if row else None,
                "note": (row.get("note") or row.get("reason") or "") if row else "",
            }
        )
    long_run = load_docs(gpu_dir, ["stage8-e2e-10h"]).get("stage8-e2e-10h")
    for step_id, title in MANUAL_STEPS.items():
        row = {"id": step_id, "title": title, "status": "manual", "seconds": None, "note": ""}
        if step_id == "ten-hour" and long_run is not None:
            hours = long_run.get("hours_of_audio")
            row["seconds"] = long_run.get("total_seconds")
            if not long_run.get("passed"):
                row["status"] = "failed"
                row["note"] = long_run.get("error") or "a target was missed"
            elif not long_run.get("full_length"):
                row["status"] = "partial"
                row["note"] = (
                    f"only {hours} h of audio were processed: the ten hour item is not met"
                )
            else:
                row["status"] = "passed"
        rows.append(row)
    return rows


# ---------------------------------------------------------------------------------------------
# Thresholds
# ---------------------------------------------------------------------------------------------


def _mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def _meetings(doc: dict[str, Any] | None, prefix: str, key: str) -> list[float]:
    if not doc:
        return []
    return [
        m[key]
        for m in doc.get("meetings", [])
        if m.get("meeting", "").startswith(prefix) and m.get(key) is not None
    ]


def _p2(prefix: str, key: str, how: Callable[[list[float]], float | None] = _mean):
    def fn(docs: Docs) -> float | None:
        return how(_meetings(docs.get("p2-asr-turbo-auto-nogloss"), prefix, key))

    return fn


def _worst(values: list[float]) -> float | None:
    return max(values) if values else None


def _der(low: int, high: int):
    def fn(docs: Docs) -> float | None:
        doc = docs.get("p3-diarize-community-1")
        runs = [
            r["der"]
            for r in (doc or {}).get("runs", [])
            if low <= r.get("reference_speakers", 0) <= high
        ]
        return max(runs) if runs else None

    return fn


def _stage4(key: str):
    def fn(docs: Docs) -> float | None:
        doc = docs.get("stage4-extraction-gemma-4-12b-q4")
        return doc.get(key) if doc else None

    return fn


def _soak(fn_doc: Callable[[dict[str, Any]], float | None]):
    def fn(docs: Docs) -> float | None:
        doc = docs.get("stage1-soak-10h")
        return fn_doc(doc) if doc else None

    return fn


def _full_2h_minutes(docs: Docs) -> float | None:
    stage2 = docs.get("stage2-pipeline-2h")
    long = docs.get("stage4-long-gemma-4-12b-q4")
    if not stage2 or not long:
        return None
    return stage2["total_minutes_for_2h"] + long["minutes_for_2h"]


def _ten_hours(docs: Docs) -> float | None:
    long_run = docs.get("stage8-e2e-10h")
    if long_run:
        # A shorter run does not say anything about ten hours.
        return long_run.get("total_hours") if long_run.get("full_length") else None
    if "p9-long-estimate" in docs:
        return docs["p9-long-estimate"].get("total_hours")
    return None


def _peak_vram(docs: Docs) -> float | None:
    values = [
        docs[name][key]
        for name, key in (
            ("stage2-pipeline-2h", "peak_vram_absolute_mib"),
            ("stage4-long-gemma-4-12b-q4", "peak_vram_absolute_mib"),
            ("stage8-e2e-10h", "peak_vram_absolute_mib"),
        )
        if name in docs and docs[name].get(key) is not None
    ]
    return max(values) if values else None


@dataclass(frozen=True)
class Threshold:
    id: str
    area: str
    measure: str
    op: str  # "<=" or ">=", or "" when there is no target
    target: float | None
    unit: str  # rate, percent, mb, mib, count, ms, minutes, hours, seconds
    fn: Callable[[Docs], float | None]
    baseline_files: tuple[str, ...]
    fresh_files: tuple[str, ...] = ()
    fresh_any: bool = False  # a fresh value needs one of the files, not all
    kind: str = "rate"  # rate: tolerance in points; perf: relative tolerance
    proof_tests: tuple[str, ...] = ()  # tests that prove a target that has no number
    note: str = ""

    def files_fresh(self) -> tuple[str, ...]:
        return self.fresh_files or self.baseline_files


_ASR = ("p2-asr-turbo-auto-nogloss",)
_DIAR = ("p3-diarize-community-1",)
_S4 = ("stage4-extraction-gemma-4-12b-q4",)
_SOAK = ("stage1-soak-10h",)

THRESHOLDS: tuple[Threshold, ...] = (
    Threshold(
        "rec-cpu",
        "Recorder",
        "CPU while recording three tracks (share of all CPUs)",
        "<=",
        3.0,
        "percent",
        _soak(lambda d: d["cpu_pct_of_total_avg"]),
        _SOAK,
        kind="perf",
        note="Stage 1 ten hour soak",
    ),
    Threshold(
        "rec-memory",
        "Recorder",
        "Memory (peak working set)",
        "<=",
        200.0,
        "mb",
        _soak(lambda d: d["working_set_mb_peak"]),
        _SOAK,
        kind="perf",
    ),
    Threshold(
        "rec-dropped",
        "Recorder",
        "Dropped buffers in a 10-hour run",
        "<=",
        0.0,
        "count",
        _soak(lambda d: float(sum(t.get("lost_buffer_events", 0) for t in d["tracks"]))),
        _SOAK,
        kind="perf",
        note="the microphone track reports lost buffers; open since Stage 1",
    ),
    Threshold(
        "rec-kill-loss",
        "Recorder",
        "Audio lost when the process is killed",
        "<=",
        60.0,
        "seconds",
        lambda docs: None,
        _SOAK,
        proof_tests=("killed_process_loses_at_most_one_segment",),
        note="proven by the recorder kill test (a segment is 60 s)",
    ),
    Threshold(
        "rec-alignment",
        "Recorder",
        "Track alignment error",
        "<=",
        50.0,
        "ms",
        _soak(lambda d: max(abs(t["offset_ms"]) for t in d["tracks"])),
        _SOAK,
        kind="perf",
    ),
    Threshold(
        "wer-pl",
        "Transcription",
        "WER, Polish scripted set",
        "<=",
        0.15,
        "rate",
        _p2("pl-", "wer"),
        _ASR,
    ),
    Threshold(
        "wer-mixed",
        "Transcription",
        "WER, mixed set",
        "<=",
        0.20,
        "rate",
        _p2("mixed-", "wer"),
        _ASR,
    ),
    Threshold(
        "wer-en",
        "Transcription",
        "WER, English set (no target, reported)",
        "",
        None,
        "rate",
        _p2("en-", "wer"),
        _ASR,
    ),
    Threshold(
        "term-recall",
        "Transcription",
        "Term recall, mixed set",
        ">=",
        0.85,
        "rate",
        _p2("mixed-", "term_recall"),
        _ASR,
    ),
    Threshold(
        "translation-rate",
        "Transcription",
        "Translation rate (worst meeting)",
        "<=",
        0.01,
        "rate",
        _p2("", "translation_rate", _worst),
        _ASR,
    ),
    Threshold(
        "der-2-4",
        "Speakers",
        "DER, 2-4 speakers (worst meeting)",
        "<=",
        0.15,
        "rate",
        _der(2, 4),
        _DIAR,
    ),
    Threshold(
        "der-6-10",
        "Speakers",
        "DER, 6-10 speakers (worst meeting)",
        "<=",
        0.30,
        "rate",
        _der(6, 10),
        _DIAR,
    ),
    Threshold(
        "der-15",
        "Speakers",
        "DER, 15+ speakers (no target, reported)",
        "",
        None,
        "rate",
        _der(15, 999),
        _DIAR,
    ),
    Threshold(
        "speaker-accuracy",
        "Speakers",
        "Speaker accuracy for enrolled voices",
        ">=",
        0.90,
        "rate",
        lambda docs: (docs.get("stage3-speaker-recognition") or {}).get("speaker_accuracy_auto"),
        ("stage3-speaker-recognition",),
    ),
    Threshold(
        "false-accept",
        "Speakers",
        "False accept rate",
        "<=",
        0.02,
        "rate",
        lambda docs: (docs.get("stage3-speaker-recognition") or {}).get("false_accept_rate"),
        ("stage3-speaker-recognition",),
    ),
    Threshold(
        "schema-validity",
        "Extraction",
        "Schema validity (up to two retries)",
        ">=",
        1.0,
        "rate",
        _stage4("schema_validity"),
        _S4,
    ),
    Threshold(
        "task-recall", "Extraction", "Task recall", ">=", 0.80, "rate", _stage4("task_recall"), _S4
    ),
    Threshold(
        "task-precision",
        "Extraction",
        "Task precision",
        ">=",
        0.80,
        "rate",
        _stage4("task_precision"),
        _S4,
    ),
    Threshold(
        "assignee-accuracy",
        "Extraction",
        "Assignee accuracy",
        ">=",
        0.85,
        "rate",
        _stage4("assignee_accuracy"),
        _S4,
    ),
    Threshold(
        "due-accuracy",
        "Extraction",
        "Due-date accuracy",
        ">=",
        0.85,
        "rate",
        _stage4("due_accuracy"),
        _S4,
    ),
    Threshold(
        "quote-validity",
        "Extraction",
        "Quote validity",
        ">=",
        0.95,
        "rate",
        _stage4("quote_validity"),
        _S4,
    ),
    Threshold(
        "time-2h",
        "Processing",
        "2-hour meeting, full pipeline",
        "<=",
        45.0,
        "minutes",
        _full_2h_minutes,
        ("stage2-pipeline-2h", "stage4-long-gemma-4-12b-q4"),
        kind="perf",
        note="worker steps (Stage 2) plus extraction and reduce (Stage 4)",
    ),
    Threshold(
        "time-10h",
        "Processing",
        "10-hour meeting, full pipeline",
        "<=",
        4.0,
        "hours",
        _ten_hours,
        ("p9-long-estimate",),
        ("stage8-e2e-10h",),
        kind="perf",
        note="baseline is the Stage 0 estimate; the fresh value is the ten hour run",
    ),
    Threshold(
        "peak-vram",
        "Processing",
        "Peak VRAM in any stage",
        "<=",
        11500.0,
        "mib",
        _peak_vram,
        ("stage2-pipeline-2h", "stage4-long-gemma-4-12b-q4"),
        ("stage2-pipeline-2h", "stage4-long-gemma-4-12b-q4", "stage8-e2e-10h"),
        fresh_any=True,
        kind="perf",
    ),
)


def load_docs(directory: Path | None, names: Iterable[str]) -> Docs:
    docs: Docs = {}
    if directory is None:
        return docs
    for name in names:
        raw = _read(directory / f"{name}.json")
        if raw is None:
            continue
        try:
            doc = json.loads(raw.lstrip("﻿"))
        except json.JSONDecodeError:
            continue
        if isinstance(doc, dict) and doc.get("status", "ok") == "ok":
            docs[name] = doc
    return docs


def _safe(fn: Callable[[Docs], float | None], docs: Docs) -> float | None:
    try:
        return fn(docs)
    except (KeyError, TypeError, ValueError, ZeroDivisionError):
        return None


def meets(op: str, target: float | None, value: float) -> bool:
    if target is None or not op:
        return True
    eps = 1e-9
    return value <= target + eps if op == "<=" else value >= target - eps


def regression_of(spec: Threshold, baseline: float, now: float) -> str:
    """`ok` or `regressed`: worse than the baseline by more than the tolerance."""
    higher_is_better = spec.op == ">="
    if spec.kind == "rate":
        worse = (baseline - now) if higher_is_better else (now - baseline)
        return "regressed" if worse > RATE_TOLERANCE + 1e-9 else "ok"
    worse = (baseline - now) if higher_is_better else (now - baseline)
    allowed = abs(baseline) * PERF_TOLERANCE
    return "regressed" if worse > allowed + 1e-9 else "ok"


def evaluate_thresholds(
    baseline_dir: Path | None,
    fresh_dir: Path | None,
    passed_tests: set[str] | None = None,
) -> list[dict[str, Any]]:
    """One row per threshold: baseline, fresh value, delta, target verdict and regression."""
    passed_tests = passed_tests or set()
    rows = []
    for spec in THRESHOLDS:
        baseline_docs = load_docs(baseline_dir, spec.baseline_files)
        fresh_names = spec.files_fresh()
        fresh_docs = load_docs(fresh_dir, fresh_names)
        fresh_ok = bool(fresh_docs) if spec.fresh_any else len(fresh_docs) == len(fresh_names)
        baseline = _safe(spec.fn, baseline_docs)
        fresh = _safe(spec.fn, fresh_docs) if fresh_ok else None
        value = fresh if fresh is not None else baseline
        source = "fresh" if fresh is not None else "baseline" if baseline is not None else "none"
        proven = any(t in passed_tests for t in spec.proof_tests)

        if spec.target is None or not spec.op:
            status = "no_target" if value is not None else "not_measured"
        elif value is not None:
            status = "pass" if meets(spec.op, spec.target, value) else "fail"
        elif proven:
            status, source = "pass", "test"
        else:
            status = "not_measured"

        if fresh is not None and baseline is not None:
            regression = regression_of(spec, baseline, fresh)
        elif baseline is not None:
            regression = "baseline_only"
        else:
            regression = "no_baseline"

        delta = None
        if fresh is not None and baseline is not None:
            delta = fresh - baseline
        rows.append(
            {
                "id": spec.id,
                "area": spec.area,
                "measure": spec.measure,
                "target": f"{spec.op} {spec.target}" if spec.target is not None else None,
                "op": spec.op,
                "target_value": spec.target,
                "unit": spec.unit,
                "value": value,
                "source": source,
                "baseline": baseline,
                "baseline_files": list(spec.baseline_files),
                "fresh": fresh,
                "delta": delta,
                "delta_unit": "points" if spec.unit == "rate" else spec.unit,
                "status": status,
                "regression": regression,
                "note": spec.note,
            }
        )
    return rows


# ---------------------------------------------------------------------------------------------
# Resilience matrix (docs/TESTING.md section 6)
# ---------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Fault:
    fault: str
    expected: str
    coverage: str  # automated, manual, pending_merge
    cpu_tests: tuple[str, ...] = ()
    gpu_step: str = ""
    how: str = ""


FAULTS: tuple[Fault, ...] = (
    Fault(
        "Recorder process killed during recording",
        "Meeting recovered, loss within the threshold",
        "automated",
        ("killed_process_loses_at_most_one_segment",),
    ),
    Fault(
        "Captured application closed during recording",
        "Its track ends, others continue",
        "automated",
        (
            "running_then_stopped_by_itself_is_source_closed",
            "removing_a_source_ends_only_that_track",
        ),
    ),
    Fault(
        "Audio device unplugged",
        "Track ends cleanly, notification shown",
        "manual",
        how="Unplug a USB microphone during a recording and check the notification.",
    ),
    Fault(
        "Disk nearly full",
        "Recording stops with a clear message, existing segments intact",
        "manual",
        how="Record onto a small virtual disk (diskpart, administrator) until it fills.",
    ),
    Fault(
        "Worker killed in each stage",
        "Retry resumes from that stage",
        "automated",
        (
            "failure_blocks_the_job_and_retry_resumes_from_that_stage",
            "recover_resumes_an_interrupted_stage",
        ),
        "pipeline-models",
    ),
    Fault(
        "GPU occupied by another process",
        "Job waits; starts when the GPU is free",
        "automated",
        ("busy_gpu_never_starts", "busy_gpu_keeps_the_job_waiting_until_it_is_free"),
        "pipeline-models",
    ),
    Fault(
        "Model server returns invalid JSON",
        "Retries, then a flagged window",
        "automated",
        (
            "test_invalid_answer_is_retried_with_the_error_in_the_prompt",
            "test_window_is_skipped_after_the_retries_and_the_stage_continues",
        ),
    ),
    Fault(
        "No network at briefing",
        "Cached state with its date",
        "pending_merge",
        how="Stage 6 (briefing) is not merged yet.",
    ),
    Fault(
        "No network at publishing",
        "Outbox, automatic retry",
        "pending_merge",
        how="Stage 5 (publishing and outbox) is not merged yet.",
    ),
    Fault(
        "Concurrent push to the cycle repository",
        "Rebase and retry succeeds",
        "pending_merge",
        how="Publishing tests of Stage 5 (`resilience`); pull with rebase is covered in "
        "`cycle-store`.",
    ),
    Fault(
        "Extension service worker restarted",
        "Stream reconnects; gap recorded in the journal",
        "pending_merge",
        how="Stage 6 (browser extension) is not merged yet.",
    ),
)


def passed_test_names(
    cargo_tests: list[dict[str, str]], pytest_cases: list[dict[str, str]]
) -> set[str]:
    names = {t["name"].rsplit("::", 1)[-1] for t in cargo_tests if t["status"] == "passed"}
    names |= {c["name"] for c in pytest_cases if c["status"] == "passed"}
    return names


def build_resilience(
    cargo_tests: list[dict[str, str]],
    pytest_cases: list[dict[str, str]],
    gpu_steps: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    passed = passed_test_names(cargo_tests, pytest_cases)
    failed = {t["name"].rsplit("::", 1)[-1] for t in cargo_tests if t["status"] == "failed"}
    failed |= {c["name"] for c in pytest_cases if c["status"] == "failed"}
    gpu = {s["id"]: s["status"] for s in gpu_steps}
    rows = []
    for f in FAULTS:
        cpu = [
            {
                "test": t,
                "status": "failed" if t in failed else "passed" if t in passed else "not_run",
            }
            for t in f.cpu_tests
        ]
        if f.coverage == "manual":
            status = "manual"
        elif f.coverage == "pending_merge":
            status = "pending_merge"
        else:
            states = [c["status"] for c in cpu]
            if f.gpu_step:
                g = gpu.get(f.gpu_step, "not_run")
                states.append(
                    "passed" if g == "passed" else "failed" if g == "failed" else "not_run"
                )
            status = (
                "failed"
                if "failed" in states
                else "passed"
                if states and all(s == "passed" for s in states)
                else "not_run"
                if "not_run" in states and "passed" not in states
                else "partial"
            )
        rows.append(
            {
                "fault": f.fault,
                "expected": f.expected,
                "coverage": f.coverage,
                "status": status,
                "cpu_tests": cpu,
                "gpu_step": f.gpu_step or None,
                "gpu_status": gpu.get(f.gpu_step) if f.gpu_step else None,
                "how": f.how,
            }
        )
    return rows


# ---------------------------------------------------------------------------------------------
# Recommendation and report
# ---------------------------------------------------------------------------------------------


def recommend(
    suites: list[dict[str, Any]],
    categories: dict[str, dict[str, Any]],
    thresholds: list[dict[str, Any]],
    resilience: list[dict[str, Any]],
    gpu_steps: list[dict[str, Any]],
    hygiene_count: int | None,
    accepted: dict[str, str] | None = None,
) -> dict[str, Any]:
    """`no-go` when something failed or a target is missed without an accepted reason,
    `incomplete` when nothing failed but parts were not measured, else `go`."""
    accepted = accepted or {}
    blocking: list[str] = []
    decisions: list[str] = []
    open_items: list[str] = []
    accepted_items: list[str] = []

    for s in suites:
        if s["status"] != "passed":
            blocking.append(f"suite failed: {s['name']}")
    if not suites:
        open_items.append("the CPU suites were not run")
    if hygiene_count is None:
        open_items.append("the temp hygiene check was not run")
    elif hygiene_count > 0:
        blocking.append(f"temp hygiene: {hygiene_count} directories left behind")
    for step in gpu_steps:
        if step["status"] == "failed":
            blocking.append(f"GPU step failed: {step['id']}")
        elif step["status"] in ("not_run", "skipped"):
            open_items.append(f"GPU step not run: {step['id']}")
        elif step["status"] == "manual":
            open_items.append(f"manual step: {step['id']}")
        elif step["status"] == "partial":
            open_items.append(f"{step['id']} only partly done: {step['note']}")
    for t in thresholds:
        label = f"{t['id']}: {t['measure']}"
        if t["status"] == "fail":
            if t["id"] in accepted:
                accepted_items.append(f"{label} ({accepted[t['id']]})")
            else:
                decisions.append(f"{label} misses the target {t['target']} ({t['source']} value)")
        elif t["status"] == "not_measured":
            open_items.append(f"{label} is not measured")
        if t["regression"] == "regressed":
            blocking.append(f"{label} regressed against its baseline")
        elif t["regression"] == "baseline_only" and t["status"] in ("pass", "fail"):
            open_items.append(f"{label}: baseline only, not re-measured")
    for r in resilience:
        label = f"resilience: {r['fault']}"
        if r["status"] == "failed":
            blocking.append(label + " failed")
        elif r["status"] in ("not_run", "partial"):
            open_items.append(label + " not fully run")
        elif r["status"] in ("manual", "pending_merge"):
            open_items.append(f"{label} ({r['status'].replace('_', ' ')})")
    for cat in categories.values():
        if cat["status"] == "failed":
            blocking.append(f"{cat['title']}: {cat['failed']} failed")

    if blocking or decisions:
        verdict = "no-go"
    elif open_items:
        verdict = "incomplete"
    else:
        verdict = "go"
    return {
        "verdict": verdict,
        "blocking": _unique(blocking),
        "decisions": _unique(decisions),
        "open": _unique(open_items),
        "accepted": accepted_items,
        "note": "Advisory only: the decision is the project owner's. "
        "`decisions` lists targets that "
        "are missed and need an explicit acceptance (see --accepted).",
    }


def _unique(items: list[str]) -> list[str]:
    seen: dict[str, None] = {}
    for item in items:
        seen.setdefault(item)
    return list(seen)


def build_report(
    log_dir: Path | None,
    gpu_dir: Path | None,
    baseline_dir: Path | None = RESULTS_DIR,
    hygiene_count: int | None = None,
    accepted: dict[str, str] | None = None,
    now: datetime | None = None,
) -> dict[str, Any]:
    suites, cargo_tests, pytest_cases = load_suites(log_dir)
    categories = build_categories(suites, cargo_tests, pytest_cases)
    gpu_steps = load_gpu_steps(gpu_dir)
    thresholds = evaluate_thresholds(
        baseline_dir, gpu_dir, passed_test_names(cargo_tests, pytest_cases)
    )
    resilience = build_resilience(cargo_tests, pytest_cases, gpu_steps)
    ran_gpu = any(s["status"] in ("passed", "failed") for s in gpu_steps)
    ran_cpu = bool(suites)
    return {
        "prototype": PROTOTYPE,
        "generated_at": (now or datetime.now(UTC)).isoformat(timespec="seconds"),
        "parts": {"cpu": ran_cpu, "gpu": ran_gpu},
        "tolerance": {"rate_points": RATE_TOLERANCE * 100, "perf_relative": PERF_TOLERANCE},
        "suites": suites,
        "categories": categories,
        "gpu_steps": gpu_steps,
        "thresholds": thresholds,
        "resilience": resilience,
        "hygiene_count": hygiene_count,
        "recommendation": recommend(
            suites, categories, thresholds, resilience, gpu_steps, hygiene_count, accepted
        ),
    }


# ---------------------------------------------------------------------------------------------
# Markdown
# ---------------------------------------------------------------------------------------------

_UNIT_SUFFIX = {
    "percent": " %",
    "mb": " MB",
    "mib": " MiB",
    "count": "",
    "ms": " ms",
    "minutes": " min",
    "hours": " h",
    "seconds": " s",
}


def fmt_value(value: float | None, unit: str) -> str:
    if value is None:
        return "-"
    if unit == "rate":
        return f"{value * 100:.1f} %"
    return f"{value:.4g}{_UNIT_SUFFIX.get(unit, '')}"


def fmt_delta(row: dict[str, Any]) -> str:
    delta = row["delta"]
    if delta is None:
        return "-"
    if row["unit"] == "rate":
        return f"{delta * 100:+.1f} pp"
    return f"{delta:+.4g}{_UNIT_SUFFIX.get(row['unit'], '')}"


def fmt_target(row: dict[str, Any]) -> str:
    if row["target_value"] is None:
        return "none"
    return f"{row['op']} {fmt_value(row['target_value'], row['unit'])}"


_STATUS_TEXT = {
    "pass": "pass",
    "fail": "FAIL",
    "no_target": "reported",
    "not_measured": "not measured",
}
_REGRESSION_TEXT = {
    "ok": "ok",
    "regressed": "REGRESSED",
    "baseline_only": "baseline only, not re-measured",
    "no_baseline": "no baseline",
}


def render_markdown(report: dict[str, Any]) -> str:
    rec = report["recommendation"]
    out = [
        "# Stage 8 regression report",
        "",
        f"Generated: {report['generated_at']}. "
        f"CPU part: {'run' if report['parts']['cpu'] else 'not run'}. "
        f"GPU part: {'run' if report['parts']['gpu'] else 'not run'}.",
        "",
        f"**Recommendation: {rec['verdict'].upper()}**",
        "",
        rec["note"],
        "",
    ]
    for key, title in (
        ("blocking", "Blocking"),
        ("decisions", "Needs a decision"),
        ("accepted", "Accepted deviations"),
        ("open", "Open"),
    ):
        if rec[key]:
            out += [f"{title}:", ""] + [f"- {item}" for item in rec[key]] + [""]

    out += [
        "## Suites",
        "",
        "| Suite | Status | Passed | Failed | Skipped | Seconds |",
        "|---|---|---|---|---|---|",
    ]
    for s in report["suites"]:
        out.append(
            f"| {s['name']} | {s['status']} | {s['passed']} | {s['failed']} | "
            f"{s['skipped'] + s['ignored']} | {s['seconds']} |"
        )
    if not report["suites"]:
        out.append("| (no suite was run) | | | | | |")
    out += [
        "",
        "## Test kinds",
        "",
        "| Kind | Status | Passed | Failed | Ignored or skipped |",
        "|---|---|---|---|---|",
    ]
    for cat in report["categories"].values():
        out.append(
            f"| {cat['title']} | {cat['status']} | {cat['passed']} | {cat['failed']} | "
            f"{cat['ignored']} |"
        )

    out += [
        "",
        "## Thresholds and baselines",
        "",
        "Values from a fresh run are marked `fresh`; otherwise the recorded baseline is shown "
        f"(`baseline`). A quality metric may not drop by more than "
        f"{report['tolerance']['rate_points']:.0f} points against its baseline.",
        "",
        "| Measure | Target | Now | Source | Baseline | Delta | Verdict | Baseline check |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for t in report["thresholds"]:
        out.append(
            f"| {t['area']}: {t['measure']} | {fmt_target(t)} | "
            f"{fmt_value(t['value'], t['unit'])} | "
            f"{t['source']} | {fmt_value(t['baseline'], t['unit'])} | {fmt_delta(t)} | "
            f"{_STATUS_TEXT[t['status']]} | {_REGRESSION_TEXT[t['regression']]} |"
        )

    out += ["", "## GPU and manual steps", "", "| Step | Status | Seconds |", "|---|---|---|"]
    for g in report["gpu_steps"]:
        note = f" ({g['note']})" if g["note"] else ""
        out.append(
            f"| {g['title']} | {g['status']}{note} | "
            f"{g['seconds'] if g['seconds'] is not None else '-'} |"
        )

    out += [
        "",
        "## Resilience (docs/TESTING.md section 6)",
        "",
        "| Fault | Expected | Coverage | Status | Evidence |",
        "|---|---|---|---|---|",
    ]
    for r in report["resilience"]:
        evidence = [f"`{c['test']}` ({c['status']})" for c in r["cpu_tests"]]
        if r["gpu_step"]:
            evidence.append(f"GPU step `{r['gpu_step']}` ({r['gpu_status'] or 'not_run'})")
        if r["how"]:
            evidence.append(r["how"])
        out.append(
            f"| {r['fault']} | {r['expected']} | {r['coverage'].replace('_', ' ')} | "
            f"{r['status'].replace('_', ' ')} | {'; '.join(evidence)} |"
        )
    hygiene = report["hygiene_count"]
    out += [
        "",
        "## Temp hygiene",
        "",
        "Not checked." if hygiene is None else f"{hygiene} directories left behind.",
        "",
    ]
    return "\n".join(out)


# ---------------------------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    p.add_argument("--logs", type=Path, default=None, help="folder written by test-all -LogDir")
    p.add_argument("--gpu-results", type=Path, default=None, help="folder of a GPU run")
    p.add_argument("--baselines", type=Path, default=RESULTS_DIR, help="recorded results")
    p.add_argument("--out", type=Path, default=RESULTS_DIR, help="folder for the two report files")
    p.add_argument("--hygiene-count", type=int, default=None)
    p.add_argument(
        "--accepted",
        type=Path,
        default=None,
        help="JSON object: threshold id -> reason, for misses the owner has accepted",
    )
    args = p.parse_args(argv)

    accepted = None
    if args.accepted:
        accepted = json.loads(args.accepted.read_text(encoding="utf-8"))
    report = build_report(args.logs, args.gpu_results, args.baselines, args.hygiene_count, accepted)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / f"{PROTOTYPE}.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    (args.out / f"{PROTOTYPE}.md").write_text(render_markdown(report) + "\n", encoding="utf-8")
    rec = report["recommendation"]
    print(
        f"{PROTOTYPE}: {rec['verdict']} "
        f"({len(rec['blocking'])} blocking, {len(rec['decisions'])} decisions, "
        f"{len(rec['open'])} open) -> {args.out}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
