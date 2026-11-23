"""Map step: items and a summary for every window of the transcript.

Reads `work/context.json` (schema `cycle-context`), `work/transcript.named.json` when present
(speaker labels already replaced by participant names) or else `work/transcript.json`, and
optionally `work/edits.json` and `work/descriptions.json`. Writes `work/extract/windows.json`.

Each model answer is validated against `schemas/extraction.schema.json`; every quote is then
checked against the transcript (`quotes.verify`) and the item gets its position from the
matched lines, not from the model. The `status_changes` the model proposes for open issues and
topics get the same quote check; whether their target exists is decided by `reduce`.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

from .. import lang, llm_client, prompts, protocol, quotes, schema, windows
from ..manifest import Manifest
from . import read_json, write_json

STEP = "extract"
MAX_TOKENS = 8192
MIN_TOKENS = 1024
CONTEXT_RESERVE = 512
CHARS_PER_TOKEN = 3
MAX_SPLIT_DEPTH = 2
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
    """Verified raw items of one window and the warnings about the unverified ones. Quotes
    are checked against what people said; screenshot descriptions are model text and do not
    count as a source."""
    corpus = quotes.Corpus.from_lines(window.speech)
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
                "part": window.part,
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


def build_status_changes(
    data: dict[str, Any], window: windows.Window
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Raw status changes of one window with the same quote check as items, and the warnings
    about the unverified ones. An answer without the field (older callers) gives none."""
    corpus = quotes.Corpus.from_lines(window.speech)
    changes: list[dict[str, Any]] = []
    warnings: list[dict[str, Any]] = []
    for raw in data.get("status_changes", []):
        found = quotes.verify(raw["quote"], windows.parse_clock(raw["time"]), corpus)
        changes.append(
            {
                "target": raw["target"].strip(),
                "status": raw["status"],
                "reason": raw["reason"].strip(),
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
                "part": window.part,
            }
        )
        if not found.verified:
            warnings.append(
                {
                    "code": "quote_unverified",
                    "message": f"The quote of the status change of {raw['target'].strip()[:40]} "
                    f"was not found in the transcript (match {found.match:.2f})",
                    "t_start_ms": found.t_start_ms,
                    "t_end_ms": found.t_end_ms,
                }
            )
    return changes, warnings


def output_budget(prompt_chars: int, ctx: int | None) -> int:
    """Tokens the answer may use: what the context leaves after the prompt (estimated from
    its length), at most MAX_TOKENS, at least MIN_TOKENS."""
    if not ctx:
        return MAX_TOKENS
    estimated_prompt = prompt_chars // CHARS_PER_TOKEN
    return max(MIN_TOKENS, min(MAX_TOKENS, ctx - estimated_prompt - CONTEXT_RESERVE))


def _empty_result(window: windows.Window) -> dict[str, Any]:
    return {
        "index": window.index,
        "t_start_ms": window.start_ms,
        "t_end_ms": window.end_ms,
        "skipped": False,
        "summary": "",
        "items": [],
        "status_changes": [],
        "attempts": 0,
        "warnings": [],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0},
        "seconds": 0.0,
    }


def _skip(window: windows.Window, error: llm_client.InvalidOutputError) -> dict[str, Any]:
    result = _empty_result(window)
    result.update(skipped=True, attempts=error.attempts, usage=error.usage, seconds=error.seconds)
    result["warnings"].append(
        {
            "code": "window_skipped",
            "message": f"The model gave no valid answer for this window: {error}",
            "t_start_ms": window.start_ms,
            "t_end_ms": window.end_ms,
        }
    )
    return result


def merge_results(
    window: windows.Window, first: dict[str, Any], rest: list[dict[str, Any]]
) -> dict[str, Any]:
    """One record for a window that was answered in parts. `first` carries what the failed
    whole-window attempt cost; `rest` are the part results in time order."""
    merged = _empty_result(window)
    merged["attempts"] = first["attempts"]
    merged["usage"] = dict(first["usage"])
    merged["seconds"] = first["seconds"]
    for part in rest:
        merged["items"] += part["items"]
        merged["status_changes"] += part["status_changes"]
        merged["warnings"] += part["warnings"]
        merged["attempts"] += part["attempts"]
        merged["seconds"] = round(merged["seconds"] + part["seconds"], 3)
        for key in merged["usage"]:
            merged["usage"][key] += part["usage"][key]
    merged["summary"] = " ".join(p["summary"] for p in rest if p["summary"])
    merged["skipped"] = all(p["skipped"] for p in rest)
    merged["split"] = True
    return merged


def extract_window(
    backend: llm_client.Backend,
    window: windows.Window,
    context: dict[str, Any],
    language: str,
    notify: Callable[[str], None] = lambda _message: None,
) -> dict[str, Any]:
    """Run the model on one window. An answer that hit the token limit is not asked for
    again: the window is halved (at most MAX_SPLIT_DEPTH times) and the halves are merged
    into one record. A window that stays invalid is recorded as skipped."""
    system = prompts.extract_system(language)
    user = prompts.extract_user(
        context, windows.render_window(window), windows.roster(window.lines)
    )
    try:
        done = llm_client.complete_json(
            backend,
            system=system,
            user=user,
            schema=schema.load("extraction"),
            name="extraction",
            max_tokens=output_budget(len(system) + len(user), backend.ctx),
        )
    except llm_client.TruncatedOutputError as e:
        halves = windows.split_in_two(window) if len(window.part) < MAX_SPLIT_DEPTH else None
        if halves is None:
            return _skip(window, e)
        notify(f"window {window.index + 1} is too long for one answer, splitting it")
        failed = _skip(window, e)
        parts = [extract_window(backend, half, context, language, notify) for half in halves]
        return merge_results(window, failed, parts)
    except llm_client.InvalidOutputError as e:
        return _skip(window, e)
    result = _empty_result(window)
    items, warnings = build_items(done.data, window, context)
    changes, change_warnings = build_status_changes(done.data, window)
    result.update(
        summary=done.data["summary"].strip(),
        items=items,
        status_changes=changes,
        attempts=done.attempts,
        warnings=warnings + change_warnings,
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
                fraction = i / len(parts)
                results.append(
                    extract_window(
                        backend,
                        window,
                        context,
                        language,
                        lambda message, f=fraction: protocol.progress(STEP, f, message),
                    )
                )
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
    if results and skipped == len(results):
        # Nothing was extracted. Finishing "done" would give an empty draft that looks like a
        # meeting without items; failing lets the stage be retried (windows.json stays).
        raise llm_client.LlmError(
            f"the model gave no usable answer for any of the {len(results)} windows "
            "(rate limit, outage or invalid output); retry the stage"
        )
