r"""Stage 4: quality, time and VRAM of the production `extract` and `reduce` steps.

Unlike P5 (a prototype prompt against llama-server) this runs the worker exactly as the queue
does: `python -m bbb_worker <step> --manifest ... --meeting-dir ...` in a subprocess with the
working directory `worker/` and `BBB_LLAMA_SERVER` set. The inputs are built from the labeled
`extraction` sets, so the scores are those of the real prompts, windowing, quote verification,
deduplication and language handling.

Quality and speed over the labeled sets (docs/TESTING.md, section 7):

    uv run python -m bbb_bench.proto.stage4_extraction \
        --model path/to/model.gguf --name gemma-4-12b-q4 --meetings data/extraction/* \
        --llama-server path/to/llama-server.exe

writes `results/stage4-extraction-<name>.json`. `meets_targets` is computed on the text-based
task scores (`task_recall`, `task_precision`, `assignee_accuracy`, `due_accuracy`). Next to them
the file reports informative scores that never enter `meets_targets` (`METRIC_NOTES`):
`assignee_accuracy_paired` / `due_accuracy_paired` pair tasks of equal text by agreement instead
of by list order, `task_recall_by_owner_due` / `task_precision_by_owner_due` match tasks by
assignee and due date alone, so differently phrased (inflected, translated) tasks still count.
Timing on a transcript of N hours made of the
labeled transcripts repeated with time offsets (one extract and one reduce run):

    ... --hours 10 --name gemma-4-12b-q4         -> results/stage4-long-<name>.json

The four report modes on one meeting (first of `--meetings`), for the definition of done:

    ... --modes --name gemma-4-12b-q4            -> results/stage4-modes-<name>.json

An already running OpenAI-compatible server is used instead of a managed llama-server with the
hidden option `--external-url http://host:port/v1` (tests, development).
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Sequence
from contextlib import ExitStack
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import jsonschema

from .. import harness
from ..datasets.generator import WORDS_PER_MINUTE
from ..metrics import quote_validity, score_by_owner_due, score_extraction

PROTOTYPE = "stage4-extraction"
PROTOTYPE_LONG = "stage4-long"
PROTOTYPE_MODES = "stage4-modes"

REPO_ROOT = Path(__file__).resolve().parents[4]
SCHEMAS_DIR = REPO_ROOT / "schemas"
WORKER_DIR = REPO_ROOT / "worker"

MODES = ("fixed", "template", "auto", "by-type")
TRACK = "meet"
CYCLE_ID = "BENCH"
LONG_GAP_MS = 5_000  # pause between two repeated transcripts, so windows can be cut there
VRAM_LIMIT_MIB = 11_500  # as in stage2_pipeline: "11.5 GB" read as MiB for headroom
TEN_HOUR_LIMIT_S = 4 * 3600  # full pipeline for 10 hours (docs/TESTING.md, section 5)
TWO_HOUR_LIMIT_S = 45 * 60
EXTRAPOLATION_TOLERANCE = 0.25  # long run may take 25% more per word than the labeled runs

# Targets of docs/TESTING.md, section 5 (Extraction).
TARGETS: dict[str, float] = {
    "schema_validity": 1.0,
    "task_recall": 0.80,
    "task_precision": 0.80,
    "assignee_accuracy": 0.85,
    "due_accuracy": 0.85,
    "quote_validity": 0.95,
}
MEAN_KEYS = (
    "task_recall",
    "task_precision",
    "assignee_accuracy",
    "due_accuracy",
    "decision_recall",
    "decision_precision",
    "budget_recall",
    "budget_precision",
    "quote_validity",
    "quote_verified_share",
    "assignee_accuracy_paired",
    "due_accuracy_paired",
    "task_recall_by_owner_due",
    "task_precision_by_owner_due",
)

# Written into the result file next to the scores, so that nobody reads them as targets.
METRIC_NOTES: dict[str, str] = {
    "meets_targets": "computed on the text-based task scores and the other TARGETS only",
    "assignee_accuracy_paired": (
        "assignee accuracy over matched tasks; equal-text tasks of different people are paired "
        "by agreement on assignee and due date, not by list order (informative)"
    ),
    "due_accuracy_paired": "the same for the due date (informative)",
    "task_recall_by_owner_due": (
        "recall with tasks matched by assignee and due date alone, blind to the wording: "
        "an upper view that ignores inflection and translation of the text (informative)"
    ),
    "task_precision_by_owner_due": "the same for precision (informative)",
}

# The four sections used for the template and by-type modes of `--modes`.
TEMPLATE_SECTIONS: list[dict[str, str]] = [
    {"id": "decisions", "title": "Decisions", "description": "Agreements the group made"},
    {"id": "actions", "title": "Action items", "description": "Tasks with an owner and a date"},
    {"id": "budget", "title": "Budget", "description": "Amounts of money for items"},
    {"id": "other", "title": "Other", "description": "Anything that fits no other section"},
]


# Inputs: transcript, context, manifest ----------------------------------------------------


@dataclass(frozen=True)
class Labeled:
    """One labeled meeting of the `extraction` set."""

    name: str
    script: dict[str, Any]
    annotation: dict[str, Any]

    @property
    def words(self) -> int:
        return script_words(self.script)


def script_words(script: dict[str, Any]) -> int:
    return sum(len(t["text"].split()) for t in script["turns"])


def load_labeled(directory: Path) -> Labeled:
    return Labeled(
        directory.name,
        json.loads((directory / "script.json").read_text(encoding="utf-8")),
        json.loads((directory / "annotation.json").read_text(encoding="utf-8")),
    )


def expand_meetings(patterns: Sequence[str]) -> list[Path]:
    """Directories named by the arguments; wildcards are expanded here so that shells that
    leave them alone (PowerShell, cmd) work as well."""
    found: list[Path] = []
    for pattern in patterns:
        hits = sorted(glob.glob(pattern)) if glob.has_magic(pattern) else [pattern]
        found.extend(Path(h) for h in hits if Path(h).is_dir())
    if not found:
        raise FileNotFoundError(f"no meeting directory matches {list(patterns)}")
    return found


def display_name(participant_id: str) -> str:
    """The name the model sees for a participant id ("kasia" -> "Kasia")."""
    return participant_id.capitalize()


def participants_of(speakers: Sequence[str]) -> list[dict[str, str]]:
    return [{"id": s, "name": display_name(s)} for s in speakers]


def segments_from_script(
    script: dict[str, Any], *, start_ms: int = 0, first_id: int = 0, track: str = TRACK
) -> tuple[list[dict[str, Any]], int]:
    """Transcript segments of a script and the end time of the last one.

    Timing follows the speaking rate of the generator: turns follow each other without
    pauses, a turn lasts as long as its words take at WORDS_PER_MINUTE.
    """
    segments: list[dict[str, Any]] = []
    words = 0
    end_ms = start_ms
    for i, turn in enumerate(script["turns"]):
        n = len(turn["text"].split())
        begin = start_ms + int(words / WORDS_PER_MINUTE * 60_000)
        end_ms = start_ms + int((words + n) / WORDS_PER_MINUTE * 60_000)
        segments.append(
            {
                "id": first_id + i,
                "track": track,
                "speaker": turn["speaker"],
                "start_ms": begin,
                "end_ms": end_ms,
                "text": turn["text"],
                "words": [],
            }
        )
        words += n
    return segments, end_ms


def transcript_speakers(speakers: Sequence[str]) -> list[dict[str, str]]:
    return [{"id": s, "label": display_name(s), "kind": "voice", "track": TRACK} for s in speakers]


def build_transcript(script: dict[str, Any], meeting_id: str, language: str) -> dict[str, Any]:
    """`work/transcript.json` of one labeled meeting (schemas/transcript.schema.json)."""
    segments, _ = segments_from_script(script)
    speakers = list(script["speakers"])
    speakers += [
        s for s in dict.fromkeys(t["speaker"] for t in script["turns"]) if s not in speakers
    ]
    return {
        "version": 1,
        "meeting_id": meeting_id,
        "language": language,
        "speakers": transcript_speakers(speakers),
        "segments": segments,
    }


def resolve_output_language(annotation: dict[str, Any] | None, choice: str) -> str:
    """`annotation` takes the language of the annotation file, anything else is passed on."""
    if choice == "annotation":
        lang = (annotation or {}).get("lang")
        return lang if lang in ("pl", "en") else "auto"
    return choice


def build_context(
    *,
    speakers: Sequence[str],
    meeting_date: str,
    mode: str = "fixed",
    output_language: str = "auto",
    sections: Sequence[dict[str, str]] = (),
    template: str | None = None,
    meeting_type: str | None = None,
) -> dict[str, Any]:
    """`work/context.json` (schemas/cycle-context.schema.json): no glossary, no open topics,
    no issues, no previous items."""
    return {
        "version": 1,
        "cycle": CYCLE_ID,
        "name": "Stage 4 benchmark",
        "meeting_date": meeting_date,
        "participants": participants_of(speakers),
        "glossary": [],
        "instruction": "",
        "output_language": output_language,
        "mode": mode,
        "template": template,
        "meeting_type": meeting_type,
        "sections": [dict(s) for s in sections],
        "open_topics": [],
        "open_issues": [],
        "previous_items": [],
    }


def mode_context(
    base: dict[str, Any], mode: str, sections: Sequence[dict[str, str]] = TEMPLATE_SECTIONS
) -> dict[str, Any]:
    """The context of `base` for another report mode (template and by-type get the sections)."""
    ctx = {**base, "mode": mode, "sections": [], "template": None, "meeting_type": None}
    if mode in ("template", "by-type"):
        ctx["sections"] = [dict(s) for s in sections]
        ctx["template"] = f"stage4-{mode}"
    if mode == "by-type":
        ctx["meeting_type"] = "technical"
    return ctx


def llm_options(
    *,
    model_path: str | Path | None,
    ctx: int,
    external_url: str | None = None,
    external_model: str = "external",
) -> dict[str, Any]:
    """`options.llm` of the manifest: a managed llama-server or an external endpoint."""
    if external_url:
        return {"kind": "external", "base_url": external_url, "model": external_model, "ctx": ctx}
    if model_path is None:
        raise ValueError("a model path is needed for the managed backend")
    return {"kind": "managed", "model_path": str(Path(model_path).resolve()), "ctx": ctx}


def build_manifest(meeting_id: str, llm: dict[str, Any]) -> dict[str, Any]:
    """`manifest.json` (schemas/job-manifest.schema.json); extract and reduce need no audio."""
    return {
        "version": 1,
        "meeting_id": meeting_id,
        "tracks": [],
        "screenshots": [],
        "options": {"language": "auto", "llm": llm},
    }


def load_schema(name: str) -> dict[str, Any]:
    return json.loads((SCHEMAS_DIR / f"{name}.schema.json").read_text(encoding="utf-8"))


def schema_errors(name: str, document: Any, limit: int = 3) -> list[str]:
    """Readable messages of the first validation errors (empty when the document is valid)."""
    validator = jsonschema.Draft202012Validator(load_schema(name))
    errors = sorted(validator.iter_errors(document), key=lambda e: list(e.path))
    return [f"{'/'.join(str(p) for p in e.path) or '<root>'}: {e.message}" for e in errors[:limit]]


def write_meeting(
    root: Path, transcript: dict[str, Any], context: dict[str, Any], manifest: dict[str, Any]
) -> tuple[Path, Path]:
    """Write the meeting folder (`work/transcript.json`, `work/context.json`) and the manifest
    next to it. Everything is validated first; returns (manifest path, meeting dir)."""
    for name, doc in (
        ("transcript", transcript),
        ("cycle-context", context),
        ("job-manifest", manifest),
    ):
        problems = schema_errors(name, doc)
        if problems:
            raise ValueError(f"the {name} built for the benchmark is invalid: {problems[0]}")
    meeting = root / "meeting"
    work = meeting / "work"
    work.mkdir(parents=True, exist_ok=True)
    (work / "transcript.json").write_text(json.dumps(transcript, ensure_ascii=False), "utf-8")
    (work / "context.json").write_text(json.dumps(context, ensure_ascii=False), "utf-8")
    manifest_path = root / "manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    return manifest_path, meeting


def build_long_transcript(
    meetings: Sequence[Labeled], hours: float, gap_ms: int = LONG_GAP_MS
) -> tuple[dict[str, Any], dict[str, Any]]:
    """A transcript of `hours` hours: the labeled meetings one after another (repeated in
    order as often as needed) with time offsets and a pause between two of them.

    Turns that would start after the target length are dropped. Returns the transcript and
    a description (`words`, `duration_ms`, `repeats`, `speakers`, `meetings`).
    """
    if not meetings:
        raise ValueError("no meetings to build a long transcript from")
    if hours <= 0:
        raise ValueError("hours must be positive")
    target_ms = int(hours * 3600 * 1000)
    segments: list[dict[str, Any]] = []
    speakers: dict[str, None] = {}
    offset = 0
    repeats = 0
    while offset < target_ms:
        meeting = meetings[repeats % len(meetings)]
        part, end_ms = segments_from_script(meeting.script, start_ms=offset, first_id=len(segments))
        segments.extend(s for s in part if s["start_ms"] < target_ms)
        speakers.update(dict.fromkeys(t["speaker"] for t in meeting.script["turns"]))
        offset = end_ms + gap_ms
        repeats += 1
    transcript = {
        "version": 1,
        "meeting_id": f"stage4-long-{hours:g}h",
        "language": meetings[0].annotation.get("lang", "und"),
        "speakers": transcript_speakers(list(speakers)),
        "segments": segments,
    }
    info = {
        "words": sum(len(s["text"].split()) for s in segments),
        "duration_ms": segments[-1]["end_ms"],
        "repeats": repeats,
        "speakers": list(speakers),
        "meetings": [m.name for m in meetings],
    }
    return transcript, info


# Running the worker ----------------------------------------------------------------------


def default_worker_python() -> Path:
    scripts = "Scripts/python.exe" if sys.platform == "win32" else "bin/python"
    return WORKER_DIR / ".venv" / scripts


@dataclass(frozen=True)
class Runner:
    """How the worker is started: its interpreter, `worker/` as working directory and the
    llama-server binary in the environment, like crates/queue/src/worker.rs does it."""

    python: Path
    llama_server: Path | None = None
    worker_dir: Path = WORKER_DIR

    def environment(self) -> dict[str, str]:
        env = {**os.environ, "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"}
        if self.llama_server:
            env["BBB_LLAMA_SERVER"] = str(self.llama_server)
        return env


@dataclass
class StepRun:
    step: str
    ok: bool
    seconds: float
    peak_vram_above_baseline_mib: int
    baseline_mib: int | None
    returncode: int
    done: bool
    events: list[dict[str, Any]] = field(default_factory=list)
    error: str = ""
    reused: bool = False

    @property
    def last_message(self) -> str:
        progress = [e for e in self.events if e.get("event") == "progress"]
        return str(progress[-1].get("message", "")) if progress else ""

    def summary(self, timeline: bool = False) -> dict[str, Any]:
        out: dict[str, Any] = {
            "step": self.step,
            "ok": self.ok,
            "seconds": round(self.seconds, 1),
            "peak_vram_above_baseline_mib": self.peak_vram_above_baseline_mib,
            "baseline_mib": self.baseline_mib,
            "progress_events": sum(1 for e in self.events if e.get("event") == "progress"),
            "last_message": self.last_message,
            "error": self.error,
        }
        if timeline:
            out["timeline"] = [
                {"t": e["t"], "fraction": e.get("fraction"), "message": e.get("message", "")}
                for e in self.events
                if e.get("event") == "progress"
            ]
        return out


def parse_event(line: str) -> dict[str, Any] | None:
    """One line of the worker's stdout as an event (`progress`, `done`, `error`), else None."""
    line = line.strip()
    if not line.startswith("{"):
        return None
    try:
        event = json.loads(line)
    except ValueError:
        return None
    return event if isinstance(event, dict) and "event" in event else None


def command_for(runner: Runner, step: str, manifest: Path, meeting_dir: Path) -> list[str]:
    return [
        str(runner.python),
        "-m",
        "bbb_worker",
        step,
        "--manifest",
        str(manifest),
        "--meeting-dir",
        str(meeting_dir),
    ]


def run_step(runner: Runner, step: str, manifest: Path, meeting_dir: Path) -> StepRun:
    """Run one worker step; parse the progress and done lines; sample VRAM meanwhile."""
    baseline = harness.gpu_memory_used_mib()
    events: list[dict[str, Any]] = []
    stderr: list[str] = []
    with harness.VramSampler() as vram:
        started = time.perf_counter()
        proc = subprocess.Popen(
            command_for(runner, step, manifest, meeting_dir),
            cwd=runner.worker_dir,
            env=runner.environment(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        assert proc.stdout is not None and proc.stderr is not None
        err_stream = proc.stderr
        drain = threading.Thread(target=lambda: stderr.append(err_stream.read()), daemon=True)
        drain.start()
        for line in proc.stdout:
            event = parse_event(line)
            if event is not None:
                events.append({**event, "t": round(time.perf_counter() - started, 2)})
        returncode = proc.wait()
        drain.join()
        seconds = time.perf_counter() - started
    done = any(e["event"] == "done" and e.get("step") == step for e in events)
    problems = [e.get("message", "") for e in events if e["event"] == "error"]
    ok = returncode == 0 and done and not problems
    error = ""
    if not ok:
        tail = (stderr[0] if stderr else "")[-400:].strip()
        error = "; ".join(str(p) for p in problems) or tail or f"exit code {returncode}"
    return StepRun(
        step=step,
        ok=ok,
        seconds=seconds,
        peak_vram_above_baseline_mib=vram.peak_above_baseline_mib,
        baseline_mib=baseline,
        returncode=returncode,
        done=done,
        events=events,
        error=error,
    )


@dataclass
class Execution:
    """Outcome of extract followed by reduce on one prepared meeting folder."""

    manifest: Path
    meeting_dir: Path
    extract: StepRun
    reduce: StepRun | None
    windows_doc: dict[str, Any] | None
    draft: dict[str, Any] | None

    @property
    def ok(self) -> bool:
        return bool(self.extract.ok and self.reduce and self.reduce.ok and self.draft)

    @property
    def seconds(self) -> float:
        return self.extract.seconds + (self.reduce.seconds if self.reduce else 0.0)

    @property
    def peak_above_baseline_mib(self) -> int:
        steps = [s for s in (self.extract, self.reduce) if s]
        return max(s.peak_vram_above_baseline_mib for s in steps)

    @property
    def peak_absolute_mib(self) -> int:
        steps = [s for s in (self.extract, self.reduce) if s]
        return max((s.baseline_mib or 0) + s.peak_vram_above_baseline_mib for s in steps)

    @property
    def error(self) -> str:
        for step in (self.extract, self.reduce):
            if step and not step.ok:
                return f"{step.step}: {step.error}"
        return ""


def read_json_if(path: Path) -> dict[str, Any] | None:
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else None


def execute(
    runner: Runner,
    root: Path,
    transcript: dict[str, Any],
    context: dict[str, Any],
    llm: dict[str, Any],
    *,
    reuse_windows: Path | None = None,
) -> Execution:
    """Build the meeting folder under `root`, run `extract` and `reduce` as the queue does.

    With `reuse_windows` the extract step is not run: the given `windows.json` is copied in
    (extract does not depend on the report mode except for the sections list).
    """
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    manifest, meeting = write_meeting(
        root, transcript, context, build_manifest(transcript["meeting_id"], llm)
    )
    windows_path = meeting / "work" / "extract" / "windows.json"
    if reuse_windows is not None:
        windows_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(reuse_windows, windows_path)
        extract_run = StepRun("extract", True, 0.0, 0, None, 0, True, reused=True)
    else:
        extract_run = run_step(runner, "extract", manifest, meeting)
    reduce_run = run_step(runner, "reduce", manifest, meeting) if extract_run.ok else None
    return Execution(
        manifest,
        meeting,
        extract_run,
        reduce_run,
        read_json_if(windows_path),
        read_json_if(meeting / "work" / "draft.json"),
    )


# Scoring --------------------------------------------------------------------------------


def window_stats(windows_doc: dict[str, Any] | None) -> dict[str, Any]:
    """Counts from `windows.json`: schema validity is the share of windows that got a valid
    answer within the allowed retries (a skipped window stayed invalid)."""
    windows = (windows_doc or {}).get("windows", [])
    n = len(windows)
    skipped = sum(1 for w in windows if w["skipped"])
    seconds = [float(w["seconds"]) for w in windows]
    half = n // 2
    return {
        "windows": n,
        "windows_skipped": skipped,
        "windows_retried": sum(1 for w in windows if w["attempts"] > 1 and not w["skipped"]),
        "attempts": sum(w["attempts"] for w in windows),
        "schema_validity": (n - skipped) / n if n else 0.0,
        "model_seconds": round(sum(seconds), 2),
        "prompt_tokens": sum(w["usage"]["prompt_tokens"] for w in windows),
        "completion_tokens": sum(w["usage"]["completion_tokens"] for w in windows),
        "window_seconds_first_half": round(sum(seconds[:half]) / half, 2) if half else None,
        "window_seconds_second_half": (
            round(sum(seconds[half:]) / (n - half), 2) if half and n - half else None
        ),
    }


def _of_type(items: Sequence[dict[str, Any]], kind: str) -> list[dict[str, Any]]:
    return [i for i in items if i["type"] == kind]


def _text_with_quote(text: str, quote: str | None) -> dict[str, Any]:
    return {"text": f"{text} {quote or ''}".strip()}


def score_draft(
    annotation: dict[str, Any], draft: dict[str, Any], entries: Sequence[dict[str, Any]]
) -> dict[str, Any]:
    """Scores of one draft against the annotation.

    Tasks use `score_extraction` on the item text like the P5 baselines (assignees are
    participant ids on both sides). Decisions and budgets have no stable wording in the
    annotation, so reference and draft are matched on text plus quote (the planted line).
    Quote validity is the existing metric on the quotes and the times the worker stored.
    `entries` are transcript segments with `t_start_ms`, `t_end_ms` and `text`.

    `assignee_accuracy_paired` and `due_accuracy_paired` are reported next to the task scores
    that `meets_targets` checks (see `score_extraction`): they pair equal-text tasks of
    different people by agreement instead of by list order. `task_recall_by_owner_due` and
    `task_precision_by_owner_due` match tasks by assignee and due date alone, so a task that
    was phrased differently (inflection, a sentence translated into the report language) is
    still counted (see `score_by_owner_due`). All four are informative only.
    """
    items = draft["items"]
    tasks = score_extraction(annotation["tasks"], _of_type(items, "task"))
    by_owner = score_by_owner_due(annotation["tasks"], _of_type(items, "task"))
    decisions = score_extraction(
        [_text_with_quote(d["text"], d.get("quote")) for d in annotation["decisions"]],
        [
            _text_with_quote(i["text"], (i["source"] or {}).get("quote"))
            for i in _of_type(items, "decision")
        ],
    )
    budgets = score_extraction(
        [
            _text_with_quote(f"{b['item']} {b['amount']} {b['currency']}", b.get("quote"))
            for b in annotation["budgets"]
        ],
        [
            _text_with_quote(i["text"], (i["source"] or {}).get("quote"))
            for i in _of_type(items, "budget")
        ],
    )
    sourced = [i for i in items if i["source"]]
    quotes = (
        quote_validity([{"source": i["source"]} for i in sourced], entries)
        if sourced
        else {"quote_validity": 0.0}
    )
    verified = sum(1 for i in sourced if i["source"]["verified"])
    by_type: dict[str, int] = {}
    for i in items:
        by_type[i["type"]] = by_type.get(i["type"], 0) + 1
    codes: dict[str, int] = {}
    for w in draft["warnings"]:
        codes[w["code"]] = codes.get(w["code"], 0) + 1
    return {
        "task_recall": tasks["recall"],
        "task_precision": tasks["precision"],
        "assignee_accuracy": tasks["assignee_accuracy"],
        "due_accuracy": tasks["due_accuracy"],
        "assignee_accuracy_paired": tasks["assignee_accuracy_paired"],
        "due_accuracy_paired": tasks["due_accuracy_paired"],
        "task_recall_by_owner_due": by_owner["recall"],
        "task_precision_by_owner_due": by_owner["precision"],
        "tasks_matched": tasks["matched"],
        "tasks_reference": tasks["reference"],
        "tasks_predicted": tasks["predicted"],
        "decision_recall": decisions["recall"],
        "decision_precision": decisions["precision"],
        "budget_recall": budgets["recall"],
        "budget_precision": budgets["precision"],
        "quote_validity": quotes["quote_validity"],
        "quote_verified_share": verified / len(sourced) if sourced else 0.0,
        "items": len(items),
        "items_by_type": by_type,
        "warnings": len(draft["warnings"]),
        "warning_codes": codes,
    }


def mean(values: Sequence[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def aggregate(per_meeting: Sequence[dict[str, Any]], total_words: int) -> dict[str, Any]:
    """Means over the meetings that ran, seconds per 1000 words and the target check."""
    ok = [m for m in per_meeting if m.get("ok")]
    windows = sum(m["windows"] for m in ok)
    valid_windows = windows - sum(m["windows_skipped"] for m in ok)
    extract_s = sum(m["extract_seconds"] for m in ok)
    reduce_s = sum(m["reduce_seconds"] for m in ok)
    out: dict[str, Any] = {
        "meetings_ok": len(ok),
        "meetings_failed": len(per_meeting) - len(ok),
        "schema_validity": valid_windows / windows if windows else 0.0,
        "drafts_schema_valid": mean([1.0 if m["draft_valid"] else 0.0 for m in ok]),
        **{k: mean([m[k] for m in ok]) for k in MEAN_KEYS},
        "seconds_per_1000_words": (extract_s + reduce_s) / total_words * 1000 if total_words else 0,
        "extract_seconds_per_1000_words": extract_s / total_words * 1000 if total_words else 0,
        "reduce_seconds_per_1000_words": reduce_s / total_words * 1000 if total_words else 0,
        "extract_model_seconds_per_1000_words": (
            sum(m["model_seconds"] for m in ok) / total_words * 1000 if total_words else 0
        ),
        "warnings_total": sum(m["warnings"] for m in ok),
        "warnings_per_meeting": mean([m["warnings"] for m in ok]),
        "peak_vram_above_baseline_mib": max(
            (m["peak_vram_above_baseline_mib"] for m in ok), default=0
        ),
    }
    out["meets_targets"] = {k: out[k] >= v for k, v in TARGETS.items()}
    return out


def config_of(args: argparse.Namespace, llm: dict[str, Any]) -> dict[str, Any]:
    external = llm["kind"] == "external"
    return {
        "model": llm["model"] if external else Path(llm["model_path"]).name,
        "ctx": llm["ctx"],
        "backend": "external" if external else "llama-server (managed)",
        "mode": getattr(args, "mode", "fixed"),
        "output_language": args.output_language,
        "llama_server": Path(args.llama_server).name if args.llama_server else None,
    }


def meeting_result(labeled: Labeled, run: Execution) -> dict[str, Any]:
    """Per-meeting record: timings, window statistics and scores (or the failure)."""
    out: dict[str, Any] = {
        "meeting": labeled.name,
        "ok": run.ok,
        "words": labeled.words,
        "extract_seconds": round(run.extract.seconds, 1),
        "reduce_seconds": round(run.reduce.seconds, 1) if run.reduce else 0.0,
        "seconds_per_1000_words": round(run.seconds / labeled.words * 1000, 1),
        "peak_vram_above_baseline_mib": run.peak_above_baseline_mib,
        "error": run.error,
        **window_stats(run.windows_doc),
    }
    if run.draft is not None and run.ok:
        transcript = json.loads((run.meeting_dir / "work" / "transcript.json").read_text("utf-8"))
        entries = [
            {"t_start_ms": s["start_ms"], "t_end_ms": s["end_ms"], "text": s["text"]}
            for s in transcript["segments"]
        ]
        problems = schema_errors("meeting", run.draft)
        out |= {
            "draft_valid": not problems,
            "draft_errors": problems,
            "language": run.draft["language"],
            **score_draft(labeled.annotation, run.draft, entries),
        }
    else:
        out |= {"draft_valid": False, "draft_errors": [], "warnings": 0}
    return out


# Runs ------------------------------------------------------------------------------------


def make_llm(args: argparse.Namespace) -> dict[str, Any]:
    return llm_options(
        model_path=args.model,
        ctx=args.ctx,
        external_url=args.external_url,
        external_model=args.external_model,
    )


def make_runner(args: argparse.Namespace) -> Runner:
    python = Path(args.worker_python) if args.worker_python else default_worker_python()
    if not python.exists():
        raise FileNotFoundError(f"worker python not found: {python} (use --worker-python)")
    return Runner(python.resolve(), Path(args.llama_server) if args.llama_server else None)


def load_all(args: argparse.Namespace) -> list[Labeled]:
    return [load_labeled(d) for d in expand_meetings(args.meetings)]


def work_root(stack: ExitStack, args: argparse.Namespace) -> Path:
    if args.work:
        root = Path(args.work).resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root
    return Path(
        stack.enter_context(
            tempfile.TemporaryDirectory(prefix="bbb-stage4-", ignore_cleanup_errors=True)
        )
    )


def run(args: argparse.Namespace) -> dict[str, Any]:
    """Quality and speed of extract and reduce over the labeled meetings."""
    runner, llm, labeled = make_runner(args), make_llm(args), load_all(args)
    per_meeting = []
    with ExitStack() as stack:
        root = work_root(stack, args)
        for item in labeled:
            language = resolve_output_language(item.annotation, args.output_language)
            context = mode_context(
                build_context(
                    speakers=item.script["speakers"],
                    meeting_date=item.annotation["meeting_date"],
                    output_language=language,
                ),
                args.mode,
            )
            transcript = build_transcript(
                item.script, f"stage4-{item.name}", item.annotation.get("lang", "und")
            )
            result = execute(runner, root / item.name, transcript, context, llm)
            per_meeting.append(meeting_result(item, result))
            print(
                f"{item.name}: {'ok' if result.ok else 'FAILED ' + result.error} "
                f"{result.seconds:.0f}s",
                file=sys.stderr,
            )
    if not any(m["ok"] for m in per_meeting):
        raise RuntimeError(f"no meeting could be processed: {per_meeting[0]['error']}")
    total_words = sum(m["words"] for m in per_meeting if m["ok"])
    return {
        "config": config_of(args, llm),
        "metric_notes": METRIC_NOTES,
        "meetings": per_meeting,
        **aggregate(per_meeting, total_words),
    }


def extrapolation_check(
    measured_s: float, words: int, reference: dict[str, Any] | None
) -> dict[str, Any]:
    """Compare the long run with the speed measured on the labeled sets: the expected time is
    `seconds_per_1000_words` of the reference result times the words of the long transcript."""
    if not reference or not reference.get("seconds_per_1000_words"):
        return {"available": False, "ok": None}
    expected = reference["seconds_per_1000_words"] * words / 1000
    ratio = measured_s / expected if expected else None
    return {
        "available": True,
        "reference_seconds_per_1000_words": reference["seconds_per_1000_words"],
        "expected_seconds": round(expected, 1),
        "measured_seconds": round(measured_s, 1),
        "ratio": round(ratio, 3) if ratio is not None else None,
        "tolerance": EXTRAPOLATION_TOLERANCE,
        "ok": ratio is not None and ratio <= 1 + EXTRAPOLATION_TOLERANCE,
    }


def find_reference(args: argparse.Namespace) -> dict[str, Any] | None:
    """`--reference` file, else the quality run with the same name."""
    path = (
        Path(args.reference)
        if args.reference
        else harness.RESULTS_DIR / f"{PROTOTYPE}-{args.name}.json"
    )
    doc = read_json_if(path)
    return doc if doc and doc.get("status") == "ok" else None


def run_long(args: argparse.Namespace) -> dict[str, Any]:
    """One extract and one reduce over a transcript of `--hours` hours."""
    runner, llm, labeled = make_runner(args), make_llm(args), load_all(args)
    transcript, info = build_long_transcript(labeled, args.hours)
    context = mode_context(
        build_context(
            speakers=info["speakers"],
            meeting_date=labeled[0].annotation["meeting_date"],
            output_language=(
                "auto" if args.output_language == "annotation" else args.output_language
            ),
        ),
        args.mode,
    )
    with ExitStack() as stack:
        root = work_root(stack, args)
        result = execute(runner, root / "long", transcript, context, llm)
    stats = window_stats(result.windows_doc)
    words = info["words"]
    scale_2h, scale_10h = 2.0 / args.hours, 10.0 / args.hours
    check = extrapolation_check(result.seconds, words, find_reference(args))
    draft_problems = schema_errors("meeting", result.draft) if result.draft else ["no draft"]
    peak_abs = result.peak_absolute_mib
    return {
        "config": {**config_of(args, llm), "hours": args.hours},
        "ok": result.ok,
        "error": result.error,
        "words": words,
        "transcript_segments": len(transcript["segments"]),
        "transcript_duration_hours": round(info["duration_ms"] / 3_600_000, 3),
        "repeats_of_labeled_meetings": info["repeats"],
        **stats,
        "extract": result.extract.summary(timeline=True),
        "reduce": result.reduce.summary(timeline=True) if result.reduce else None,
        "wall_seconds": round(result.seconds, 1),
        "seconds_per_1000_words": round(result.seconds / words * 1000, 2),
        "minutes_for_2h": round(result.seconds * scale_2h / 60, 1),
        "hours_for_10h": round(result.seconds * scale_10h / 3600, 2),
        "threshold_minutes_for_2h_full_pipeline": TWO_HOUR_LIMIT_S // 60,
        "threshold_hours_for_10h_full_pipeline": TEN_HOUR_LIMIT_S // 3600,
        "share_of_10h_budget": round(result.seconds * scale_10h / TEN_HOUR_LIMIT_S, 3),
        "peak_vram_above_baseline_mib": result.peak_above_baseline_mib,
        "peak_vram_absolute_mib": peak_abs,
        "threshold_vram_mib": VRAM_LIMIT_MIB,
        "draft_valid": not draft_problems,
        "draft_errors": draft_problems,
        "items": len(result.draft["items"]) if result.draft else 0,
        "warnings": len(result.draft["warnings"]) if result.draft else 0,
        "extrapolation": check,
        "passed": bool(
            result.ok
            and not draft_problems
            and peak_abs <= VRAM_LIMIT_MIB
            and check["ok"] is not False
        ),
    }


def mode_record(mode: str, run: Execution, sections: Sequence[dict[str, str]]) -> dict[str, Any]:
    """What `--modes` records per mode: validity, number of items and sections."""
    draft = run.draft
    problems = schema_errors("meeting", draft) if draft else ["no draft"]
    ids = [s["id"] for s in draft["sections"]] if draft else []
    items = draft["items"] if draft else []
    dangling = [i["id"] for i in items if i["section"] is not None and i["section"] not in ids]
    expected = [s["id"] for s in sections] if mode in ("template", "by-type") else None
    return {
        "mode": mode,
        "ok": run.ok,
        "error": run.error,
        "extract_reused": run.extract.reused,
        "extract_seconds": round(run.extract.seconds, 1),
        "reduce_seconds": round(run.reduce.seconds, 1) if run.reduce else 0.0,
        "draft_valid": not problems,
        "draft_errors": problems,
        "draft_mode": draft["mode"] if draft else None,
        "items": len(items),
        "sections": len(ids),
        "section_ids": ids,
        "items_in_sections": sum(1 for i in items if i["section"] in ids),
        "items_with_unknown_section": dangling,
        "sections_match_template": ids == expected if expected is not None else None,
        "warnings": len(draft["warnings"]) if draft else 0,
        "reduce_note": run.reduce.last_message if run.reduce else "",
        "passed": bool(
            run.ok
            and not problems
            and draft
            and draft["mode"] == mode
            and not dangling
            and (expected is None or ids == expected)
        ),
    }


def run_modes(args: argparse.Namespace) -> dict[str, Any]:
    """Reduce in every report mode on one meeting. Extract runs once for fixed and auto (it
    does not depend on the mode) and again with the sections for template and by-type."""
    runner, llm = make_runner(args), make_llm(args)
    item = load_all(args)[0]
    language = resolve_output_language(item.annotation, args.output_language)
    base = build_context(
        speakers=item.script["speakers"],
        meeting_date=item.annotation["meeting_date"],
        output_language=language,
    )
    transcript = build_transcript(
        item.script, f"stage4-{item.name}", item.annotation.get("lang", "und")
    )
    records = []
    with ExitStack() as stack:
        root = work_root(stack, args)
        shared: Path | None = None
        for mode in MODES:
            context = mode_context(base, mode)
            reuse = shared if mode in ("fixed", "auto") else None
            result = execute(runner, root / mode, transcript, context, llm, reuse_windows=reuse)
            if mode == "fixed" and result.extract.ok:
                shared = result.meeting_dir / "work" / "extract" / "windows.json"
                # keep a copy outside the folder that the next mode would delete
                shared = Path(shutil.copy(shared, root / "windows-shared.json"))
            records.append(mode_record(mode, result, TEMPLATE_SECTIONS))
            print(f"{mode}: {'passed' if records[-1]['passed'] else 'FAILED'}", file=sys.stderr)
    return {
        "config": {**config_of(args, llm), "meeting": item.name},
        "modes": records,
        "all_modes_valid": all(r["passed"] for r in records),
    }


# Command line ----------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    p.add_argument("--model", help="Path to a GGUF file (managed llama-server)")
    p.add_argument("--name", required=True, help="Run name used in the result file name")
    p.add_argument("--meetings", nargs="+", required=True, help="Labeled meeting directories")
    p.add_argument("--ctx", type=int, default=16384)
    p.add_argument("--worker-python", default=None, help="Default: worker/.venv interpreter")
    p.add_argument("--mode", choices=MODES, default="fixed", help="Report mode of the run")
    p.add_argument("--hours", type=float, default=None, help="Timing run on N hours of transcript")
    p.add_argument("--modes", action="store_true", help="Reduce in all four modes on one meeting")
    p.add_argument("--llama-server", default=None, help="Path of llama-server (BBB_LLAMA_SERVER)")
    p.add_argument(
        "--output-language",
        choices=("annotation", "auto", "pl", "en"),
        default="annotation",
        help="output_language of the context (default: language of the annotation)",
    )
    p.add_argument("--work", default=None, help="Keep the meeting folders here")
    p.add_argument("--reference", default=None, help="Result file for the extrapolation check")
    p.add_argument("--external-url", default=None, help=argparse.SUPPRESS)
    p.add_argument("--external-model", default="external", help=argparse.SUPPRESS)
    return p


def main(argv: Sequence[str] | None = None) -> None:
    p = build_parser()
    args = p.parse_args(argv)
    if not args.model and not args.external_url:
        p.error("--model is required (or the hidden --external-url)")
    if args.modes and args.hours:
        p.error("--modes and --hours are separate runs")
    if args.hours is not None and args.hours <= 0:
        p.error("--hours must be positive")
    if args.modes:
        harness.run_prototype(PROTOTYPE_MODES, args.name, lambda: run_modes(args))
    elif args.hours:
        harness.run_prototype(PROTOTYPE_LONG, args.name, lambda: run_long(args))
    else:
        harness.run_prototype(PROTOTYPE, args.name, lambda: run(args))


if __name__ == "__main__":
    main()
