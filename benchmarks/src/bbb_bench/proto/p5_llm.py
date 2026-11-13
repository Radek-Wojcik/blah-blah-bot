"""P5: structured extraction with candidate language models via llama-server.

    uv run python -m bbb_bench.proto.p5_llm --model path/to/model.gguf \
        --name qwen3-14b-q4 --meetings data/extraction/*

Each meeting directory needs `script.json` and `annotation.json` (see
bbb_bench.datasets.generator). The transcript is shown to the model with
timestamps derived from the speaking rate; the model must return items that
validate against EXTRACTION_SCHEMA. Invalid responses are retried up to twice.
"""

from __future__ import annotations

import argparse
import json
from datetime import date
from pathlib import Path
from typing import Any

import jsonschema

from .. import harness
from ..datasets.generator import WORDS_PER_MINUTE
from ..metrics import quote_validity, score_extraction
from . import llama_server

EXTRACTION_SCHEMA: dict[str, Any] = {
    "type": "object",
    "required": ["items"],
    "additionalProperties": False,
    "properties": {
        "items": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["type", "text", "assignee", "due", "time", "quote"],
                "additionalProperties": False,
                "properties": {
                    "type": {"enum": ["task", "decision", "budget"]},
                    "text": {"type": "string"},
                    "assignee": {"type": ["string", "null"]},
                    "due": {"type": ["string", "null"], "pattern": r"^\d{4}-\d{2}-\d{2}$"},
                    "time": {"type": "string", "pattern": r"^\d{2}:\d{2}$"},
                    "quote": {"type": "string"},
                },
            },
        }
    },
}

SYSTEM = (
    "You extract structured items from a meeting transcript. Return JSON only. "
    "Item types: task (someone commits to do something), decision (the group agrees on "
    "something), budget (an amount of money for an item). Rules: include only items that were "
    "actually agreed; skip proposals that were rejected or dropped; if a task is handed over to "
    "another person, the assignee is the person who finally accepts it. `assignee` is the "
    "speaker name exactly as written in the transcript, or null. `due` is an ISO date "
    "(YYYY-MM-DD) resolved against the meeting date, or null. `time` is the mm:ss stamp of the "
    "line where the item is stated. `quote` is copied verbatim from that line. Write `text` in "
    "the language of the transcript. For tasks use the form 'verb + object' from the speech. "
    "For decisions and budgets set assignee and due to null."
)


def transcript_lines(script: dict[str, Any]) -> tuple[list[str], list[dict[str, Any]]]:
    """Timestamped transcript lines and entries for quote checking."""
    lines: list[str] = []
    entries: list[dict[str, Any]] = []
    words = 0
    for turn in script["turns"]:
        n = len(turn["text"].split())
        start_ms = int(words / WORDS_PER_MINUTE * 60_000)
        end_ms = int((words + n) / WORDS_PER_MINUTE * 60_000)
        stamp = f"{start_ms // 60000:02d}:{start_ms // 1000 % 60:02d}"
        lines.append(f"[{stamp}] {turn['speaker']}: {turn['text']}")
        entries.append({"t_start_ms": start_ms, "t_end_ms": end_ms, "text": turn["text"]})
        words += n
    return lines, entries


def _stamp_ms(stamp: str) -> int:
    mm, ss = stamp.split(":")
    return (int(mm) * 60 + int(ss)) * 1000


def extract(base: str, script: dict[str, Any], meeting_date: str, max_retries: int = 2):
    """Run extraction on one transcript; returns (items, attempts, usage, seconds, valid)."""
    d = date.fromisoformat(meeting_date)
    lines, entries = transcript_lines(script)
    user = (
        f"Meeting date: {meeting_date} ({d.strftime('%A')}). Speakers: "
        f"{', '.join(script['speakers'])}.\n\nTranscript:\n" + "\n".join(lines)
    )
    payload = {
        "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": user}],
        "temperature": 0,
        "max_tokens": 8192,
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": "extraction", "strict": True, "schema": EXTRACTION_SCHEMA},
        },
    }
    usage = {"prompt_tokens": 0, "completion_tokens": 0}
    with harness.timer() as elapsed:
        for attempt in range(1, max_retries + 2):
            resp = llama_server.chat(base, payload)
            for key in usage:
                usage[key] += resp.get("usage", {}).get(key, 0)
            try:
                data = json.loads(resp["choices"][0]["message"]["content"])
                jsonschema.validate(data, EXTRACTION_SCHEMA)
                return data["items"], entries, attempt, usage, elapsed(), True
            except (ValueError, KeyError, jsonschema.ValidationError):
                continue
    return [], entries, max_retries + 1, usage, elapsed(), False


def run(args: argparse.Namespace) -> dict[str, Any]:
    per_meeting = []
    with (
        harness.VramSampler() as vram,
        llama_server.running(Path(args.model), ctx=args.ctx) as base,
    ):
        for meeting in map(Path, args.meetings):
            script = json.loads((meeting / "script.json").read_text(encoding="utf-8"))
            ann = json.loads((meeting / "annotation.json").read_text(encoding="utf-8"))
            items, entries, attempts, usage, secs, valid = extract(
                base, script, ann["meeting_date"]
            )
            tasks = [i for i in items if i["type"] == "task"]
            score = score_extraction(ann["tasks"], tasks)
            with_src = [
                {
                    "source": {
                        "quote": i["quote"],
                        "t_start_ms": _stamp_ms(i["time"]),
                        "t_end_ms": _stamp_ms(i["time"]),
                    }
                }
                for i in items
            ]
            qv = quote_validity(with_src, entries) if with_src else {"quote_validity": 0.0}
            per_meeting.append(
                {
                    "meeting": meeting.name,
                    "valid_schema": valid,
                    "attempts": attempts,
                    "seconds": secs,
                    "prompt_tokens": usage["prompt_tokens"],
                    "completion_tokens": usage["completion_tokens"],
                    "task_recall": score["recall"],
                    "task_precision": score["precision"],
                    "assignee_accuracy": score["assignee_accuracy"],
                    "due_accuracy": score["due_accuracy"],
                    "quote_validity": qv["quote_validity"],
                }
            )
    n = len(per_meeting)

    def mean(key: str) -> float:
        return sum(m[key] for m in per_meeting) / n

    total_secs = sum(m["seconds"] for m in per_meeting)
    total_words = sum(
        len(t["text"].split())
        for m in args.meetings
        for t in json.loads((Path(m) / "script.json").read_text(encoding="utf-8"))["turns"]
    )
    return {
        "config": {"model": Path(args.model).name, "ctx": args.ctx},
        "meetings": per_meeting,
        "schema_validity": sum(1 for m in per_meeting if m["valid_schema"]) / n,
        **{k: mean(k) for k in ("task_recall", "task_precision", "assignee_accuracy")},
        **{k: mean(k) for k in ("due_accuracy", "quote_validity")},
        "seconds_per_1000_words": total_secs / total_words * 1000,
        "completion_tokens_per_second": sum(m["completion_tokens"] for m in per_meeting)
        / total_secs,
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", required=True, help="Path to a GGUF file")
    p.add_argument("--name", required=True)
    p.add_argument("--ctx", type=int, default=16384)
    p.add_argument("--meetings", nargs="+", required=True)
    args = p.parse_args()
    harness.run_prototype("p5-llm", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
