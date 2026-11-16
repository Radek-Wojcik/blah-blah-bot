"""Stage 4 measurement script: pure helpers and the whole flow against a fake model server.

CPU only. The flow tests start the real worker steps (`python -m bbb_worker extract|reduce`)
as subprocesses and point `options.llm` at an OpenAI-compatible server run in this process.
"""

from __future__ import annotations

import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from bbb_bench import harness
from bbb_bench.proto import stage4_extraction as s4

SPEAKERS = ["alex", "kasia", "jan"]
TURNS = [
    ("alex", "Good morning everyone, let us begin."),
    ("kasia", "Jan, can you order the new sensors by November seventeenth?"),
    ("jan", "Sure, I will have it done by November seventeenth."),
    ("alex", "Let us agree that we move the rollout to next month."),
    ("kasia", "Agreed."),
    ("kasia", "The budget for software licenses is eight thousand euros."),
    ("jan", "Got it, adding it to the plan."),
    ("alex", "I will take this one: write the release notes by November twentieth."),
    ("kasia", "Great, thank you."),
    ("jan", "Thank you all, that is all for today."),
]


def make_script() -> dict[str, Any]:
    return {
        "lang": "en",
        "speakers": SPEAKERS,
        "turns": [{"speaker": s, "lang": "en", "text": t} for s, t in TURNS],
    }


def make_annotation() -> dict[str, Any]:
    return {
        "meeting_date": "2026-11-13",
        "lang": "en",
        "speakers": SPEAKERS,
        "tasks": [
            {
                "text": "order the new sensors",
                "assignee": "jan",
                "due": "2026-11-17",
                "turn": 1,
                "quote": TURNS[1][1],
            },
            {
                "text": "write the release notes",
                "assignee": "alex",
                "due": "2026-11-20",
                "turn": 7,
                "quote": TURNS[7][1],
            },
        ],
        "decisions": [
            {"text": "we move the rollout to next month", "turn": 3, "quote": TURNS[3][1]}
        ],
        "budgets": [
            {
                "item": "software licenses",
                "amount": 8000,
                "currency": "EUR",
                "turn": 5,
                "quote": TURNS[5][1],
            }
        ],
        "terms": [],
    }


def write_labeled(root: Path, name: str) -> Path:
    directory = root / name
    directory.mkdir(parents=True)
    (directory / "script.json").write_text(json.dumps(make_script()), encoding="utf-8")
    (directory / "annotation.json").write_text(json.dumps(make_annotation()), encoding="utf-8")
    return directory


def labeled(name: str = "t-0") -> s4.Labeled:
    return s4.Labeled(name, make_script(), make_annotation())


# Inputs ----------------------------------------------------------------------------------


def test_transcript_is_valid_and_timed_from_the_speaking_rate():
    doc = s4.build_transcript(make_script(), "stage4-t-0", "en")
    assert s4.schema_errors("transcript", doc) == []
    segs = doc["segments"]
    assert [s["id"] for s in segs] == list(range(len(TURNS)))
    assert segs[0]["start_ms"] == 0
    assert all(a["end_ms"] == b["start_ms"] for a, b in zip(segs, segs[1:], strict=False))
    words = len(TURNS[0][1].split())
    assert segs[0]["end_ms"] == int(words / s4.WORDS_PER_MINUTE * 60_000)
    assert {s["speaker"] for s in segs} == set(SPEAKERS)
    assert [(s["id"], s["label"]) for s in doc["speakers"]] == [
        ("alex", "Alex"),
        ("kasia", "Kasia"),
        ("jan", "Jan"),
    ]


def test_context_follows_the_schema_and_the_annotation():
    ann = make_annotation()
    ctx = s4.build_context(
        speakers=SPEAKERS,
        meeting_date=ann["meeting_date"],
        output_language=s4.resolve_output_language(ann, "annotation"),
    )
    assert s4.schema_errors("cycle-context", ctx) == []
    assert ctx["output_language"] == "en"
    assert ctx["participants"][1] == {"id": "kasia", "name": "Kasia"}
    assert ctx["mode"] == "fixed"
    assert ctx["glossary"] == ctx["open_topics"] == ctx["open_issues"] == []
    assert ctx["previous_items"] == []
    assert s4.resolve_output_language(ann, "auto") == "auto"
    assert s4.resolve_output_language({"lang": "de"}, "annotation") == "auto"


def test_mode_context_adds_the_four_sections_only_where_needed():
    base = s4.build_context(speakers=SPEAKERS, meeting_date="2026-11-13")
    for mode in s4.MODES:
        ctx = s4.mode_context(base, mode)
        assert s4.schema_errors("cycle-context", ctx) == []
        assert ctx["mode"] == mode
        with_sections = mode in ("template", "by-type")
        assert len(ctx["sections"]) == (4 if with_sections else 0)
        assert (ctx["meeting_type"] == "technical") == (mode == "by-type")
        assert (ctx["template"] is not None) == with_sections
    assert base["mode"] == "fixed"  # the base context is not changed


def test_manifest_for_both_backends_is_valid(tmp_path):
    managed = s4.llm_options(model_path=tmp_path / "m.gguf", ctx=8192)
    assert managed["kind"] == "managed"
    assert Path(managed["model_path"]).is_absolute()
    external = s4.llm_options(model_path=None, ctx=8192, external_url="http://127.0.0.1:1/v1")
    assert external == {
        "kind": "external",
        "base_url": "http://127.0.0.1:1/v1",
        "model": "external",
        "ctx": 8192,
    }
    for llm in (managed, external):
        assert s4.schema_errors("job-manifest", s4.build_manifest("m1", llm)) == []
    with pytest.raises(ValueError):
        s4.llm_options(model_path=None, ctx=8192)


def test_write_meeting_refuses_an_invalid_document(tmp_path):
    transcript = s4.build_transcript(make_script(), "m1", "en")
    context = s4.build_context(speakers=SPEAKERS, meeting_date="13.11.2026")  # bad date
    manifest = s4.build_manifest("m1", s4.llm_options(model_path=None, ctx=4096, external_url="x"))
    with pytest.raises(ValueError, match="cycle-context"):
        s4.write_meeting(tmp_path, transcript, context, manifest)


def test_expand_meetings_handles_wildcards(tmp_path):
    write_labeled(tmp_path, "en-0")
    write_labeled(tmp_path, "en-1")
    found = s4.expand_meetings([str(tmp_path / "en-*")])
    assert [p.name for p in found] == ["en-0", "en-1"]
    with pytest.raises(FileNotFoundError):
        s4.expand_meetings([str(tmp_path / "none-*")])


# Long transcript -------------------------------------------------------------------------


def test_long_transcript_has_the_requested_length_and_offsets():
    first = labeled("a")
    second = s4.Labeled(
        "b",
        {
            "lang": "en",
            "speakers": ["ola", "ewa"],
            "turns": [{"speaker": "ola", "lang": "en", "text": "word " * 140}],
        },
        {**make_annotation(), "speakers": ["ola", "ewa"]},
    )
    doc, info = s4.build_long_transcript([first, second], hours=0.1)
    assert s4.schema_errors("transcript", doc) == []
    segs = doc["segments"]
    assert segs[-1]["start_ms"] < 6 * 60_000
    assert 5.5 * 60_000 < info["duration_ms"] < 7 * 60_000
    assert [s["id"] for s in segs] == list(range(len(segs)))
    starts = [s["start_ms"] for s in segs]
    assert starts == sorted(starts)
    # the second labeled meeting starts after the first plus the pause
    first_end = s4.segments_from_script(first.script)[1]
    ola = next(s for s in segs if s["speaker"] == "ola")
    assert ola["start_ms"] == first_end + s4.LONG_GAP_MS
    assert [s["id"] for s in doc["speakers"]] == info["speakers"]
    assert {"alex", "ola"} <= set(info["speakers"])
    assert info["words"] == sum(len(s["text"].split()) for s in segs)
    assert info["repeats"] >= 2


def test_long_transcript_of_ten_hours_is_plausible():
    doc, info = s4.build_long_transcript([labeled()], hours=10)
    assert info["duration_ms"] <= 10 * 3600 * 1000 + 60_000
    assert info["duration_ms"] > 9.9 * 3600 * 1000
    assert s4.schema_errors("transcript", doc) == []


def test_long_transcript_rejects_nonsense():
    with pytest.raises(ValueError):
        s4.build_long_transcript([], hours=1)
    with pytest.raises(ValueError):
        s4.build_long_transcript([labeled()], hours=0)


# Scoring ---------------------------------------------------------------------------------


def item(kind, text, quote, *, assignee=None, due=None, verified=True, start=0, end=1000):
    return {
        "id": "i001",
        "type": kind,
        "text": text,
        "assignee": assignee,
        "due": due,
        "certainty": {"assignee": "stated", "due": "stated"},
        "section": None,
        "relates_to": None,
        "source": {
            "track": "meet",
            "speaker": "alex",
            "t_start_ms": start,
            "t_end_ms": end,
            "quote": quote,
            "verified": verified,
            "match": 1.0,
        },
        "edited": False,
    }


def entries_of_script() -> list[dict[str, Any]]:
    segments, _ = s4.segments_from_script(make_script())
    return [
        {"t_start_ms": s["start_ms"], "t_end_ms": s["end_ms"], "text": s["text"]} for s in segments
    ]


def perfect_draft() -> dict[str, Any]:
    segs = s4.segments_from_script(make_script())[0]
    return {
        "warnings": [],
        "items": [
            item(
                "task",
                "order the new sensors",
                TURNS[1][1],
                assignee="jan",
                due="2026-11-17",
                start=segs[1]["start_ms"],
                end=segs[1]["end_ms"],
            ),
            item(
                "task",
                "write the release notes",
                TURNS[7][1],
                assignee="alex",
                due="2026-11-20",
                start=segs[7]["start_ms"],
                end=segs[7]["end_ms"],
            ),
            item(
                "decision",
                "move the rollout to next month",
                TURNS[3][1],
                start=segs[3]["start_ms"],
                end=segs[3]["end_ms"],
            ),
            item(
                "budget",
                "Budget for software licenses is 8000 EUR",
                TURNS[5][1],
                start=segs[5]["start_ms"],
                end=segs[5]["end_ms"],
            ),
            item(
                "note",
                "Everything starts today",
                TURNS[0][1],
                start=segs[0]["start_ms"],
                end=segs[0]["end_ms"],
            ),
        ],
    }


def test_perfect_draft_scores_one():
    score = s4.score_draft(make_annotation(), perfect_draft(), entries_of_script())
    for key in (
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
    ):
        assert score[key] == 1.0, key
    assert score["items_by_type"] == {"task": 2, "decision": 1, "budget": 1, "note": 1}
    assert score["warnings"] == 0


def test_wrong_and_extra_items_lower_the_scores():
    draft = perfect_draft()
    draft["items"][0]["assignee"] = "kasia"  # wrong assignee
    draft["items"][1]["due"] = "2026-11-21"  # wrong date
    draft["items"].append(item("task", "water the plants", "invented words here", verified=False))
    draft["items"].pop(3)  # budget lost
    draft["warnings"] = [{"code": "quote_unverified", "message": "x"}]
    score = s4.score_draft(make_annotation(), draft, entries_of_script())
    assert score["task_recall"] == 1.0
    assert score["task_precision"] == pytest.approx(2 / 3)
    assert score["assignee_accuracy"] == 0.5
    assert score["due_accuracy"] == 0.5
    assert score["budget_recall"] == 0.0
    assert score["budget_precision"] == 0.0  # nothing predicted for a planted budget
    assert score["quote_validity"] < 1.0
    assert score["quote_verified_share"] == pytest.approx(4 / 5)
    assert score["warning_codes"] == {"quote_unverified": 1}


def test_paired_scores_are_reported_but_do_not_change_the_task_scores():
    draft = perfect_draft()
    first, second = draft["items"][0], draft["items"][1]
    first["text"] = second["text"] = "do the work"  # one task for two people
    annotation = make_annotation()
    annotation["tasks"][0]["text"] = annotation["tasks"][1]["text"] = "do the work"
    draft["items"][0], draft["items"][1] = second, first  # listed in the other order
    score = s4.score_draft(annotation, draft, entries_of_script())
    assert score["task_recall"] == 1.0 and score["task_precision"] == 1.0
    assert score["assignee_accuracy"] == 0.0 and score["due_accuracy"] == 0.0
    assert score["assignee_accuracy_paired"] == 1.0 and score["due_accuracy_paired"] == 1.0
    assert "assignee_accuracy_paired" in s4.MEAN_KEYS and "due_accuracy_paired" in s4.MEAN_KEYS
    assert "assignee_accuracy_paired" not in s4.TARGETS
    assert "task_recall_by_owner_due" not in s4.TARGETS
    assert set(s4.METRIC_NOTES) >= {"meets_targets", "task_recall_by_owner_due"}


def test_translated_task_is_found_by_owner_and_due_but_not_by_text():
    draft = perfect_draft()
    draft["items"][0]["text"] = "zamówić nowe czujniki"  # the report language is not the speech
    score = s4.score_draft(make_annotation(), draft, entries_of_script())
    assert score["task_recall"] == 0.5 and score["task_recall_by_owner_due"] == 1.0
    assert score["task_precision_by_owner_due"] == 1.0


def test_items_of_other_types_do_not_count_as_tasks():
    draft = perfect_draft()
    draft["items"] = [i for i in draft["items"] if i["type"] != "task"]
    draft["items"].append(item("idea", "order the new sensors", TURNS[1][1]))
    score = s4.score_draft(make_annotation(), draft, entries_of_script())
    assert score["task_recall"] == 0.0
    assert score["tasks_predicted"] == 0


def test_window_stats_count_attempts_and_skips():
    def window(skipped, attempts, seconds):
        return {
            "skipped": skipped,
            "attempts": attempts,
            "seconds": seconds,
            "usage": {"prompt_tokens": 100, "completion_tokens": 10},
        }

    doc = {"windows": [window(False, 1, 10), window(False, 2, 20), window(True, 3, 30)]}
    stats = s4.window_stats(doc)
    assert stats["windows"] == 3
    assert stats["windows_skipped"] == 1
    assert stats["windows_retried"] == 1
    assert stats["attempts"] == 6
    assert stats["schema_validity"] == pytest.approx(2 / 3)
    assert stats["model_seconds"] == 60
    assert stats["prompt_tokens"] == 300
    assert stats["window_seconds_first_half"] == 10
    assert stats["window_seconds_second_half"] == 25
    assert s4.window_stats(None)["schema_validity"] == 0.0


def test_aggregate_averages_meetings_that_ran_and_checks_targets():
    base = {
        "ok": True,
        "windows": 2,
        "windows_skipped": 0,
        "extract_seconds": 60.0,
        "reduce_seconds": 20.0,
        "model_seconds": 50.0,
        "draft_valid": True,
        "warnings": 2,
        "peak_vram_above_baseline_mib": 9000,
        **{k: 1.0 for k in s4.MEAN_KEYS},
    }
    other = {**base, "task_recall": 0.5, "peak_vram_above_baseline_mib": 9500, "windows_skipped": 1}
    failed = {"ok": False, "error": "boom"}
    agg = s4.aggregate([base, other, failed], total_words=4000)
    assert agg["meetings_ok"] == 2
    assert agg["meetings_failed"] == 1
    assert agg["task_recall"] == 0.75
    assert agg["schema_validity"] == 0.75
    assert agg["seconds_per_1000_words"] == pytest.approx(160 / 4000 * 1000)
    assert agg["extract_seconds_per_1000_words"] == pytest.approx(30)
    assert agg["reduce_seconds_per_1000_words"] == pytest.approx(10)
    assert agg["warnings_total"] == 4
    assert agg["peak_vram_above_baseline_mib"] == 9500
    assert agg["meets_targets"]["task_recall"] is False
    assert agg["meets_targets"]["quote_validity"] is True
    assert agg["meets_targets"]["schema_validity"] is False


def test_parse_event_reads_protocol_lines_only():
    line = '{"event": "progress", "step": "extract", "fraction": 0.5, "message": "window 1/2"}'
    assert s4.parse_event(line + "\n")["fraction"] == 0.5
    assert s4.parse_event("loading model...") is None
    assert s4.parse_event("{broken") is None
    assert s4.parse_event('{"no_event": 1}') is None


def test_extrapolation_check():
    ref = {"seconds_per_1000_words": 10.0}
    ok = s4.extrapolation_check(1100.0, 100_000, ref)
    assert ok["available"] and ok["expected_seconds"] == 1000.0 and ok["ok"] is True
    slow = s4.extrapolation_check(1500.0, 100_000, ref)
    assert slow["ok"] is False
    assert s4.extrapolation_check(1.0, 10, None) == {"available": False, "ok": None}


def test_command_matches_how_the_queue_starts_the_worker(tmp_path):
    runner = s4.Runner(Path("py"), Path("llama-server.exe"))
    cmd = s4.command_for(runner, "extract", tmp_path / "m.json", tmp_path / "meeting")
    assert cmd[1:5] == ["-m", "bbb_worker", "extract", "--manifest"]
    assert cmd[-2] == "--meeting-dir"
    assert runner.environment()["BBB_LLAMA_SERVER"] == "llama-server.exe"
    assert runner.worker_dir.name == "worker"


# Whole flow against a fake OpenAI-compatible server --------------------------------------


class FakeModel:
    """Answers extraction with the planted items of the annotation whose line is in the
    window, and the summary and section calls with canned data."""

    def __init__(self, annotation: dict[str, Any]) -> None:
        self.annotation = annotation
        self.kinds: list[str] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                kind = body["response_format"]["json_schema"]["name"]
                outer.kinds.append(kind)
                content = json.dumps(outer.answer(kind, body["messages"][1]["content"]))
                payload = json.dumps(
                    {
                        "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 5},
                    }
                ).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args: Any) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(
            target=lambda: self.server.serve_forever(poll_interval=0.01), daemon=True
        )

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_address[1]}/v1"

    def __enter__(self) -> FakeModel:
        self.thread.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.server.shutdown()
        self.server.server_close()

    def planted(self) -> list[tuple[str, str, str | None, str | None, str]]:
        ann = self.annotation
        return (
            [("task", t["text"], t["assignee"], t["due"], t["quote"]) for t in ann["tasks"]]
            + [("decision", d["text"], None, None, d["quote"]) for d in ann["decisions"]]
            + [
                (
                    "budget",
                    f"Budget for {b['item']}: {b['amount']} {b['currency']}",
                    None,
                    None,
                    b["quote"],
                )
                for b in ann["budgets"]
            ]
        )

    def answer(self, kind: str, user: str) -> dict[str, Any]:
        if kind == "summary":
            return {"summary": "The team agreed on several things."}
        if kind == "sections":
            ids = re.findall(r"^(i\d{3}) \[", user, re.MULTILINE)
            sections = [
                {"id": "plans", "title": "Plans", "description": "What was planned"},
                {"id": "money", "title": "Money", "description": "Amounts"},
            ]
            return {
                "sections": sections,
                "assignments": [{"item": i, "section": "plans"} for i in ids],
            }
        allowed = re.findall(r"^- ([a-z][a-z0-9_-]*): ", user.split("Allowed sections")[1], re.M)
        window = user.split("<<<TRANSCRIPT BEGIN>>>\n", 1)[1]
        items = []
        for type_, text, assignee, due, quote in self.planted():
            found = re.search(rf"\[(\d\d:\d\d:\d\d)\] [^:\n]+: {re.escape(quote)}", window)
            if found:
                items.append(
                    {
                        "type": type_,
                        "text": text,
                        "assignee": assignee,
                        "due": due,
                        "certainty": {
                            "assignee": "stated" if assignee else "unknown",
                            "due": "stated" if due else "unknown",
                        },
                        "section": allowed[0] if allowed else None,
                        "relates_to": None,
                        "time": found.group(1),
                        "quote": quote,
                    }
                )
        return {"summary": f"A window with {len(items)} items.", "items": items}


@pytest.fixture
def flow_env(tmp_path, monkeypatch):
    """No GPU, results in tmp_path, the worker from this checkout run with this interpreter."""
    monkeypatch.setattr(harness, "gpu_memory_used_mib", lambda: None)
    monkeypatch.setattr(harness, "gpu_info", lambda: {})
    monkeypatch.setattr(harness, "RESULTS_DIR", tmp_path / "results")
    monkeypatch.setenv("PYTHONPATH", str(s4.WORKER_DIR / "src"))
    return tmp_path


def read_result(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def argv(env: Path, url: str, name: str, *extra: str) -> list[str]:
    sets = env / "sets"
    if not sets.exists():
        for i in range(2):
            write_labeled(sets, f"t-{i}")
    meetings = [str(sets / f"t-{i}") for i in range(2)]
    return [
        "--external-url",
        url,
        "--name",
        name,
        "--worker-python",
        sys.executable,
        "--meetings",
        *meetings,
        *extra,
    ]


def test_quality_run_against_a_fake_server(flow_env):
    with FakeModel(make_annotation()) as fake:
        s4.main(argv(flow_env, fake.url, "fake"))
    doc = read_result(flow_env / "results" / "stage4-extraction-fake.json")
    assert doc["status"] == "ok", doc.get("error")
    assert doc["config"]["backend"] == "external"
    assert doc["config"]["ctx"] == 16384
    assert doc["meetings_ok"] == 2
    assert doc["schema_validity"] == 1.0
    assert doc["drafts_schema_valid"] == 1.0
    for key in (
        "task_recall",
        "task_precision",
        "assignee_accuracy",
        "due_accuracy",
        "decision_recall",
        "decision_precision",
        "budget_recall",
        "budget_precision",
        "quote_validity",
    ):
        assert doc[key] == 1.0, key
    assert doc["seconds_per_1000_words"] > 0
    assert doc["extract_seconds_per_1000_words"] > 0
    assert doc["reduce_seconds_per_1000_words"] > 0
    assert doc["peak_vram_above_baseline_mib"] == 0
    assert all(doc["meets_targets"].values())
    first = doc["meetings"][0]
    assert first["ok"] and first["draft_valid"] and first["language"] == "en"
    assert first["items"] == 4  # 2 tasks, 1 decision, 1 budget
    assert first["warnings"] == 0
    assert first["windows"] == 1


def test_failed_worker_step_is_recorded_not_raised(flow_env):
    # nothing listens on this port: the worker cannot reach the model server
    s4.main(argv(flow_env, "http://127.0.0.1:9/v1", "down"))
    doc = read_result(flow_env / "results" / "stage4-extraction-down.json")
    assert doc["status"] == "error"
    assert "no meeting could be processed" in doc["error"]
    assert "not reachable" in doc["error"] or "extract" in doc["error"]


def test_modes_run_produces_a_valid_draft_for_every_mode(flow_env):
    with FakeModel(make_annotation()) as fake:
        s4.main(argv(flow_env, fake.url, "fake", "--modes"))
        kinds = list(fake.kinds)
    doc = read_result(flow_env / "results" / "stage4-modes-fake.json")
    assert doc["status"] == "ok", doc.get("error")
    modes = {m["mode"]: m for m in doc["modes"]}
    assert list(modes) == ["fixed", "template", "auto", "by-type"]
    assert doc["all_modes_valid"] is True
    for mode, rec in modes.items():
        assert rec["draft_valid"], (mode, rec["draft_errors"])
        assert rec["draft_mode"] == mode
        assert rec["items"] == 4
    assert modes["fixed"]["sections"] == 0
    assert modes["template"]["section_ids"] == [s["id"] for s in s4.TEMPLATE_SECTIONS]
    assert modes["by-type"]["sections_match_template"] is True
    assert modes["template"]["items_in_sections"] == 4
    assert modes["auto"]["section_ids"] == ["plans", "money"]
    assert modes["auto"]["extract_reused"] is True
    assert modes["fixed"]["extract_reused"] is False
    # extract ran for fixed, template and by-type (one window each); auto reused fixed
    assert kinds.count("extraction") == 3
    assert kinds.count("sections") == 1


def test_long_run_records_time_windows_and_the_extrapolation(flow_env):
    quality = flow_env / "quality.json"
    quality.write_text(
        json.dumps({"status": "ok", "seconds_per_1000_words": 1000.0}), encoding="utf-8"
    )
    with FakeModel(make_annotation()) as fake:
        s4.main(argv(flow_env, fake.url, "fake", "--hours", "0.5", "--reference", str(quality)))
        kinds = list(fake.kinds)
    doc = read_result(flow_env / "results" / "stage4-long-fake.json")
    assert doc["status"] == "ok", doc.get("error")
    assert doc["ok"] and doc["draft_valid"]
    assert doc["config"]["hours"] == 0.5
    assert 0.49 < doc["transcript_duration_hours"] <= 0.51
    assert doc["windows"] == 3  # 15-minute windows over 30 minutes with 1 minute overlap
    assert doc["words"] > 1000
    assert kinds.count("extraction") == doc["windows"]
    assert kinds.count("summary") == 1
    assert doc["items"] > 0
    assert doc["wall_seconds"] > 0
    assert doc["hours_for_10h"] == pytest.approx(doc["wall_seconds"] * 20 / 3600, abs=0.01)
    assert doc["peak_vram_above_baseline_mib"] == 0
    # the reference claims 1000 s per 1000 words, so the long run is far faster than expected
    assert doc["extrapolation"]["available"] is True
    assert doc["extrapolation"]["ok"] is True
    assert doc["extract"]["timeline"][-1]["fraction"] == 1.0
    assert doc["passed"] is True


def test_cli_requires_a_backend_and_rejects_two_runs(flow_env, capsys):
    base = ["--name", "x", "--meetings", "whatever"]
    with pytest.raises(SystemExit):
        s4.main(base)
    with pytest.raises(SystemExit):
        s4.main([*base, "--external-url", "http://x/v1", "--modes", "--hours", "1"])
    capsys.readouterr()
