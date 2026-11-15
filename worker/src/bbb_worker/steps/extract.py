"""Map step: items and a summary for every window of the transcript.

Reads `work/context.json` (schema `cycle-context`), `work/transcript.named.json` when present
(speaker labels already replaced by participant names) or else `work/transcript.json`, and
optionally `work/edits.json` and `work/descriptions.json`. Writes `work/extract/windows.json`.

Each model answer is validated against `schemas/extraction.schema.json`; every quote is then
checked against the transcript (`quotes.verify`) and the item gets its position from the
matched lines, not from the model.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from .. import lang, llm_client, prompts, protocol, quotes, schema, windows
from ..manifest import Manifest
from . import read_json, write_json

STEP = "extract"
MAX_TOKENS = 4096
TEMPLATE_MODES = ("template", "by-type")


def read_transcript(work_dir: Path) -> dict[str, Any]:
    """The named transcript when the application wrote one, else the plain one."""
    named = work_dir / "transcript.named.json"
    doc = read_json(named if named.exists() else work_dir / "transcript.json")
    schema.validate("transcript", doc)
    return doc


def read_optional(path: Path, schema_name: str | None = None) -> dict[str, Any] | None:
    if not path.exists():
        return None
    doc = read_json(path)
    if schema_name:
        schema.validate(schema_name, doc)
    return doc


def allowed_sections(context: dict[str, Any]) -> set[str]:
    if context["mode"] not in TEMPLATE_MODES:
        return set()
    return {s["id"] for s in context["sections"]}


def build_items(
    data: dict[str, Any], window: windows.Window, context: dict[str, Any]
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Verified raw items of one window and the warnings about the unverified ones."""
    corpus = quotes.Corpus.from_lines(window.lines)
    sections = allowed_sections(context)
    items: list[dict[str, Any]] = []
    warnings: list[dict[str, Any]] = []
    for raw in data["items"]:
        found = quotes.verify(raw["quote"], windows.parse_clock(raw["time"]), corpus)
        items.append(
            {
                "type": raw["type"],
                "text": raw["text"].strip(),
                "assignee": raw["assignee"],
                "due": raw["due"],
                "certainty": dict(raw["certainty"]),
                "section": raw["section"] if raw["section"] in sections else None,
                "relates_to": raw["relates_to"],
                "source": {
                    "track": found.track,
                    "speaker": found.speaker,
                    "t_start_ms": found.t_start_ms,
                    "t_end_ms": found.t_end_ms,
                    "quote": raw["quote"],
                    "verified": found.verified,
                    "match": found.match,
                },
                "window": window.index,
            }
        )
        if not found.verified:
            warnings.append(
                {
                    "code": "quote_unverified",
                    "message": f"The quote of this item was not found in the transcript "
                    f"(match {found.match:.2f}): {raw['text'].strip()[:80]}",
                    "t_start_ms": found.t_start_ms,
                    "t_end_ms": found.t_end_ms,
                }
            )
    return items, warnings


def extract_window(
    backend: llm_client.Backend,
    window: windows.Window,
    context: dict[str, Any],
    language: str,
) -> dict[str, Any]:
    """Run the model on one window; a window that stays invalid is recorded as skipped."""
    result: dict[str, Any] = {
        "index": window.index,
        "t_start_ms": window.start_ms,
        "t_end_ms": window.end_ms,
        "skipped": False,
        "summary": "",
        "items": [],
        "attempts": 0,
        "warnings": [],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0},
        "seconds": 0.0,
    }
    try:
        done = llm_client.complete_json(
            backend,
            system=prompts.extract_system(language),
            user=prompts.extract_user(context, windows.render_window(window)),
            schema=schema.load("extraction"),
            name="extraction",
            max_tokens=MAX_TOKENS,
        )
    except llm_client.InvalidOutputError as e:
        result.update(skipped=True, attempts=e.attempts, usage=e.usage, seconds=e.seconds)
        result["warnings"].append(
            {
                "code": "window_skipped",
                "message": f"The model gave no valid answer for this window: {e}",
                "t_start_ms": window.start_ms,
                "t_end_ms": window.end_ms,
            }
        )
        return result
    items, warnings = build_items(done.data, window, context)
    result.update(
        summary=done.data["summary"].strip(),
        items=items,
        attempts=done.attempts,
        warnings=warnings,
        usage=done.usage,
        seconds=done.seconds,
    )
    return result


def prepare(manifest: Manifest) -> tuple[dict[str, Any], str, list[windows.Window]]:
    """Context, resolved output language and the windows of this meeting."""
    work = manifest.work_dir
    context = read_json(work / "context.json")
    schema.validate("cycle-context", context)
    transcript = read_transcript(work)
    edits = read_optional(work / "edits.json")
    descriptions = read_optional(work / "descriptions.json", "screenshot-descriptions")
    speech = windows.speech_lines(transcript, edits)
    shots = windows.screenshot_lines(descriptions, windows.excluded_ranges(transcript, edits))
    language = lang.resolve(
        context["output_language"], transcript["language"], (ln.text for ln in speech)
    )
    return context, language, windows.split_windows(speech, shots)


def run(manifest: Manifest) -> None:
    cfg = llm_client.config_from(manifest.options)
    cfg.ensure_allowed()
    context, language, parts = prepare(manifest)
    results: list[dict[str, Any]] = []
    if parts:
        with llm_client.open_backend(cfg) as backend:
            for i, window in enumerate(parts):
                results.append(extract_window(backend, window, context, language))
                protocol.progress(STEP, (i + 1) / len(parts), f"window {i + 1}/{len(parts)}")
    doc = {
        "version": 1,
        "meeting_id": manifest.meeting_id,
        "language": language,
        "windows": results,
    }
    write_json(manifest.work_dir / "extract" / "windows.json", doc)
    skipped = sum(1 for r in results if r["skipped"])
    protocol.progress(STEP, 1.0, f"{len(results)} windows, {skipped} skipped")
