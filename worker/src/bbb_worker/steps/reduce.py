"""Reduce step: one draft out of the window results.

Reads `work/extract/windows.json`, `work/context.json` and the transcript (for the names of the
speakers that have one, which the summary prompt keeps, and to resolve the output language when
`windows.json` does not say), writes `work/draft.json` (schema
`meeting`) or, when that file already holds user edits, `work/draft.new.json`.

In order: deduplicate across windows, validate assignees, due dates and links against the
context, number the items by time, write the hierarchical summary, and choose the sections.
A failed summary or section call never fails the step; it falls back and says so.

The status changes the model proposed for open issues and topics become `status_proposals`:
only targets that exist in the context are kept, one proposal per target (see
`build_status_proposals`), always with the decision `proposed`. Nothing here ever approves one.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Sequence
from contextlib import nullcontext
from datetime import date, timedelta
from pathlib import Path
from typing import Any, TypeVar

from .. import dedup, draft, lang, llm_client, prompts, protocol, schema
from ..manifest import Manifest
from ..windows import format_clock, named_people
from . import extract, read_json

STEP = "reduce"
SUMMARY_TOKENS = 1024
SECTIONS_TOKENS = 4096
FIRST_SENTENCE = re.compile(r"^.*?[.!?](?=\s|$)", re.DOTALL)
ISO_DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
ISSUE_NUMBER = re.compile(r"\d+")
PLACEHOLDER_SUMMARY = re.compile(r"^Summary of a window with \d+ items\.?$", re.IGNORECASE)
WORD_TOKEN = re.compile(r"[A-Za-zÀ-ÖØ-öø-ÿĄĆĘŁŃÓŚŹŻąćęłńóśźż0-9]+")
SHORT_TITLE_MAX_CHARS = 70
SHORT_TITLE_MIN_WORDS = 3
SHORT_TITLE_MAX_WORDS = 8
MAX_KEY_SCREENSHOTS = 8
TRAILING_CONNECTORS = frozenset(
    {
        "a",
        "an",
        "and",
        "at",
        "by",
        "do",
        "for",
        "from",
        "i",
        "in",
        "into",
        "na",
        "o",
        "of",
        "on",
        "or",
        "oraz",
        "po",
        "the",
        "to",
        "w",
        "with",
        "z",
        "za",
    }
)
LOW_VALUE_DESCRIPTIONS = (
    "blank screen",
    "black screen",
    "empty screen",
    "no content",
    "nothing visible",
    "loading screen",
    "pusty ekran",
    "czarny ekran",
    "brak treści",
)
HIGH_INFO_TERMS = frozenset(
    {
        "architecture",
        "budget",
        "chart",
        "code",
        "data",
        "diagram",
        "error",
        "graph",
        "kod",
        "metric",
        "metrics",
        "plan",
        "roadmap",
        "schedule",
        "schema",
        "schemat",
        "slajd",
        "slide",
        "table",
        "tabela",
        "timeline",
        "wykres",
    }
)

SUMMARY_SCHEMA: dict[str, Any] = {
    "type": "object",
    "required": ["summary"],
    "additionalProperties": False,
    "properties": {"summary": {"type": "string", "minLength": 1}},
}
_ASSIGNMENTS: dict[str, Any] = {
    "type": "array",
    "items": {
        "type": "object",
        "required": ["item", "section"],
        "additionalProperties": False,
        "properties": {"item": {"type": "string"}, "section": {"type": "string"}},
    },
}
SECTIONS_SCHEMA: dict[str, Any] = {
    "type": "object",
    "required": ["sections", "assignments"],
    "additionalProperties": False,
    "properties": {
        "sections": {
            "type": "array",
            "minItems": 1,
            "maxItems": 7,
            "items": {
                "type": "object",
                "required": ["id", "title", "description"],
                "additionalProperties": False,
                "properties": {
                    "id": {"type": "string", "pattern": "^[a-z][a-z0-9_-]*$"},
                    "title": {"type": "string", "minLength": 1},
                    "description": {"type": "string"},
                },
            },
        },
        "assignments": _ASSIGNMENTS,
    },
}
ASSIGN_SCHEMA: dict[str, Any] = {
    "type": "object",
    "required": ["assignments"],
    "additionalProperties": False,
    "properties": {"assignments": _ASSIGNMENTS},
}

T = TypeVar("T")


def chunks(seq: Sequence[T], size: int) -> list[list[T]]:
    return [list(seq[i : i + size]) for i in range(0, len(seq), size)]


# Validation of items against the context ------------------------------------------------


def _warning(code: str, message: str, item: dict[str, Any] | None = None) -> dict[str, Any]:
    src = item["source"] if item else None
    return {
        "code": code,
        "message": message,
        "t_start_ms": src["t_start_ms"] if src else None,
        "t_end_ms": src["t_end_ms"] if src else None,
    }


_GENERIC_ASSIGNEE_RE = re.compile(
    r"^(?:speaker|mówca|participant|uczestnik)(?:[\s_-]*\d+)?$", re.IGNORECASE
)
_GENERIC_ASSIGNEE_LABELS = frozenset({"you", "unknown", "ty", "ja", "none", "null", "n/a"})


def _clean_assignee_name(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    cleaned = " ".join(value.split())
    if not cleaned:
        return None
    if cleaned.casefold() in _GENERIC_ASSIGNEE_LABELS or _GENERIC_ASSIGNEE_RE.fullmatch(cleaned):
        return None
    return cleaned


def resolve_assignee(value: str | None, participants: list[dict[str, str]]) -> str | None:
    """A participant id; a model that wrote the name or another case still gets the id."""
    if value is None:
        return None
    wanted = value.strip().casefold()
    for p in participants:
        if wanted in (p["id"].casefold(), p["name"].casefold()):
            return p["id"]
    return None


def valid_due(due: str, meeting_date: str) -> bool:
    if not ISO_DATE.fullmatch(due):
        return False
    try:
        return date.fromisoformat(due) >= date.fromisoformat(meeting_date) - timedelta(days=1)
    except ValueError:
        return False


def validate_items(items: list[dict[str, Any]], context: dict[str, Any]) -> list[dict[str, Any]]:
    """Normalise assignee, due and relates_to in place; return the warnings."""
    warnings: list[dict[str, Any]] = []
    links = {t["id"] for t in context["open_topics"]} | {i["id"] for i in context["open_issues"]}
    for it in items:
        asked = it["assignee"]
        asked_name = _clean_assignee_name(it.get("assignee_name"))
        it["assignee"] = resolve_assignee(asked, context["participants"])
        if it["assignee"] is None and asked_name is not None:
            it["assignee"] = resolve_assignee(asked_name, context["participants"])
            if it["assignee"] is not None and it["certainty"]["assignee"] == "unknown":
                it["certainty"]["assignee"] = "inferred"
        if it["assignee"] is not None:
            it["assignee_name"] = None
        else:
            it["assignee_name"] = asked_name
            if it["assignee_name"] is not None:
                if it["certainty"]["assignee"] not in ("stated", "inferred"):
                    it["certainty"]["assignee"] = "inferred"
            else:
                it["certainty"]["assignee"] = "unknown"
                if asked is not None:
                    warnings.append(
                        _warning(
                            "assignee_unknown",
                            f"The assignee '{asked}' is not a participant of the cycle: "
                            f"{it['text'][:80]}",
                            it,
                        )
                    )
        if it["due"] is not None and not valid_due(it["due"], context["meeting_date"]):
            it["due"] = None
        if it["due"] is None:
            it["certainty"]["due"] = "unknown"
        if it["relates_to"] not in links:
            it["relates_to"] = None
    return warnings


def number_items(items: list[dict[str, Any]]) -> None:
    for n, it in enumerate(items, 1):
        it["id"] = f"i{n:03d}"


# Status proposals -----------------------------------------------------------------------


def status_targets(context: dict[str, Any]) -> dict[str, str]:
    """Targets a status change may name, by lower-case key: the open issues, the issues linked
    to items of earlier meetings, and the open topics of the cycle."""
    targets: dict[str, str] = {}
    ids = [i["id"] for i in context["open_issues"]]
    ids += [p["issue"] for p in context["previous_items"] if p.get("issue")]
    ids += [t["id"] for t in context["open_topics"]]
    for target in ids:
        targets.setdefault(target.casefold(), target)
    return targets


def resolve_target(value: str, targets: dict[str, str]) -> str | None:
    """The canonical id for what the model wrote (`#12`, `12` and a topic id in any case)."""
    wanted = value.strip()
    if ISSUE_NUMBER.fullmatch(wanted):
        wanted = f"#{wanted}"
    return targets.get(wanted.casefold())


def build_status_proposals(
    results: list[dict[str, Any]], context: dict[str, Any]
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Proposals of the draft and the warnings about the ones whose quote is unverified.

    A change whose target is not an open issue, a linked issue or an open topic of the
    context is dropped, so the model cannot invent targets. Several changes of one target
    become one: the latest in the meeting wins, but a change with a verified quote beats one
    without. The proposals are ordered by time, numbered `s1`, `s2`, ... and never approved.
    A window result written before status changes existed has none."""
    targets = status_targets(context)
    best: dict[str, dict[str, Any]] = {}
    for window in results:
        for change in window.get("status_changes", []):
            target = resolve_target(change["target"], targets)
            if target is None:
                continue
            rank = (change["source"]["verified"], change["source"]["t_start_ms"])
            current = best.get(target)
            if current is None or rank >= current["rank"]:
                best[target] = {"rank": rank, "target": target, "change": change}
    chosen = sorted(
        best.values(),
        key=lambda c: (c["change"]["source"]["t_start_ms"], c["target"]),
    )
    proposals: list[dict[str, Any]] = []
    warnings: list[dict[str, Any]] = []
    for n, entry in enumerate(chosen, 1):
        change = entry["change"]
        source = dict(change["source"])
        proposals.append(
            {
                "id": f"s{n}",
                "target": entry["target"],
                "status": change["status"],
                "reason": change["reason"],
                "comment": None,
                "source": source,
                "decision": "proposed",
                "edited": False,
            }
        )
        if not source["verified"]:
            warnings.append(
                {
                    "code": "quote_unverified",
                    "message": f"The quote of the status change of {entry['target']} was not found "
                    f"in the transcript (match {source['match']:.2f}, "
                    f"at {format_clock(source['t_start_ms'])})",
                    "t_start_ms": source["t_start_ms"],
                    "t_end_ms": source["t_end_ms"],
                }
            )
    return proposals, warnings


# Summary --------------------------------------------------------------------------------


def first_sentence(text: str) -> str:
    text = " ".join(text.split())
    found = FIRST_SENTENCE.match(text)
    return found.group(0) if found else text


def fallback_summary(summaries: list[str]) -> str:
    return " ".join(first_sentence(s) for s in summaries if s.strip())


def summarize_once(
    backend: llm_client.Backend,
    parts: list[str],
    language: str,
    final: bool,
    people: Sequence[str] = (),
) -> str:
    done = llm_client.complete_json(
        backend,
        system=prompts.summary_system(language, final),
        user=prompts.summary_user(parts, people),
        schema=SUMMARY_SCHEMA,
        name="summary",
        max_tokens=SUMMARY_TOKENS,
    )
    return str(done.data["summary"]).strip()


def summarize(
    backend: llm_client.Backend | None,
    summaries: list[str],
    language: str,
    on_call: Callable[[], None] = lambda: None,
    people: Sequence[str] = (),
) -> str:
    """Window summaries to one meeting summary; many windows are reduced in groups first.
    `people` are the names of the speakers that have one; the model is told to keep them."""
    level = [s for s in summaries if s.strip()]
    if len(level) <= 1:
        return level[0] if level else ""
    assert backend is not None
    while len(level) > prompts.SUMMARY_MAX_DIRECT:
        next_level: list[str] = []
        for group in chunks(level, prompts.SUMMARY_GROUP):
            on_call()
            next_level.append(summarize_once(backend, group, language, final=False, people=people))
            on_call()
        level = next_level
    on_call()
    result = summarize_once(backend, level, language, final=True, people=people)
    on_call()
    return result


# Short title and key screenshots --------------------------------------------------------


def _clean_title_clause(text: str) -> str:
    sentence = first_sentence(text).strip()
    for sep in (";", " — ", " – ", ": "):
        if sep in sentence:
            head = sentence.split(sep, 1)[0].strip()
            if len(head.split()) >= SHORT_TITLE_MIN_WORDS:
                sentence = head
                break
    if "," in sentence:
        head = sentence.split(",", 1)[0].strip()
        if SHORT_TITLE_MIN_WORDS <= len(head.split()) <= SHORT_TITLE_MAX_WORDS:
            sentence = head
    return sentence.strip(" \t\r\n.!?;:,-—–\"'()[]{}")


def _bound_title_words(clause: str) -> str:
    if not clause:
        return ""
    raw_words = [w.strip(".,!?;:\"'()[]{}") for w in clause.split()]
    words = [w for w in raw_words if w]
    if len(words) < SHORT_TITLE_MIN_WORDS:
        return ""
    words = words[:SHORT_TITLE_MAX_WORDS]
    while len(words) > SHORT_TITLE_MIN_WORDS and words[-1].casefold() in TRAILING_CONNECTORS:
        words.pop()
    while len(words) > SHORT_TITLE_MIN_WORDS and len(" ".join(words)) > SHORT_TITLE_MAX_CHARS:
        words.pop()
    while len(words) > SHORT_TITLE_MIN_WORDS and words[-1].casefold() in TRAILING_CONNECTORS:
        words.pop()
    title = " ".join(words)
    if len(title) > SHORT_TITLE_MAX_CHARS:
        title = title[:SHORT_TITLE_MAX_CHARS].rstrip()
    return title if len(title.split()) >= SHORT_TITLE_MIN_WORDS else ""


def derive_short_title(summary: str, items: Sequence[dict[str, Any]] = ()) -> str:
    """Derive a concise 3 to 8 word title (<= 70 chars) from `summary` or top decision/topic."""
    clean_summary = " ".join(summary.split())
    if clean_summary and not PLACEHOLDER_SUMMARY.match(clean_summary):
        title = _bound_title_words(_clean_title_clause(clean_summary))
        if title:
            return title

    priority_order = ("decision", "topic", "task", "budget", "idea", "note")
    for item_type in priority_order:
        for it in items:
            if it.get("type") == item_type:
                title = _bound_title_words(_clean_title_clause(str(it.get("text", ""))))
                if title:
                    return title
    for it in items:
        title = _bound_title_words(_clean_title_clause(str(it.get("text", ""))))
        if title:
            return title
    return ""


def _description_tokens(text: str) -> set[str]:
    return {t.casefold() for t in WORD_TOKEN.findall(text) if len(t) >= 3}


def _is_nontrivial_description(text: str) -> bool:
    clean = " ".join(text.split())
    if len(clean) < 20 or len(clean.split()) < 4:
        return False
    lower = clean.casefold()
    if len(clean) < 80 and any(phrase in lower for phrase in LOW_VALUE_DESCRIPTIONS):
        return False
    return len(_description_tokens(clean)) >= 3


def _screenshot_info_score(text: str) -> float:
    tokens = _description_tokens(text)
    if not tokens:
        return 0.0
    digits = len(re.findall(r"\d+", text))
    domain_hits = sum(1 for t in tokens if t in HIGH_INFO_TERMS)
    length_bonus = min(len(text), 600) / 100.0
    return len(tokens) + 1.5 * digits + 2.0 * domain_hits + length_bonus


def select_key_screenshots(
    descriptions: dict[str, Any] | Sequence[dict[str, Any]] | None,
    max_screenshots: int = MAX_KEY_SCREENSHOTS,
) -> list[str]:
    """Select at most `max_screenshots` distinct, highest-information screenshot filenames."""
    if not descriptions or max_screenshots <= 0:
        return []
    raw_items = (
        descriptions.get("items", []) if isinstance(descriptions, dict) else list(descriptions)
    )
    candidates: list[dict[str, Any]] = []
    for idx, entry in enumerate(raw_items):
        if not isinstance(entry, dict):
            continue
        file_name = str(entry.get("file", "")).strip()
        desc = str(entry.get("description", "")).strip()
        if not file_name or not _is_nontrivial_description(desc):
            continue
        tokens = _description_tokens(desc)
        candidates.append(
            {
                "index": idx,
                "offset_ms": float(entry.get("offset_ms", idx)),
                "file": file_name,
                "tokens": tokens,
                "score": _screenshot_info_score(desc),
            }
        )
    if not candidates:
        return []

    # Rank by information score descending, breaking ties by earlier timestamp.
    ranked = sorted(candidates, key=lambda c: (-c["score"], c["offset_ms"], c["index"]))
    selected: list[dict[str, Any]] = []
    seen_files: set[str] = set()
    for cand in ranked:
        if cand["file"] in seen_files:
            continue
        tokens = cand["tokens"]
        is_dup = False
        for kept in selected:
            kept_tokens = kept["tokens"]
            union = len(tokens | kept_tokens)
            inter = len(tokens & kept_tokens)
            jaccard = inter / union if union else 1.0
            containment = inter / max(1, len(tokens))
            if jaccard >= 0.72 or containment >= 0.85:
                is_dup = True
                break
        if is_dup:
            continue
        selected.append(cand)
        seen_files.add(cand["file"])
        if len(selected) >= max_screenshots:
            break

    selected.sort(key=lambda c: (c["offset_ms"], c["index"]))
    return [c["file"] for c in selected]


# Sections -------------------------------------------------------------------------------


def _apply_assignments(
    items: list[dict[str, Any]], assignments: list[dict[str, str]], valid: set[str]
) -> None:
    by_id = {it["id"]: it for it in items}
    for a in assignments:
        if a["item"] in by_id and a["section"] in valid:
            by_id[a["item"]]["section"] = a["section"]


def _sample(items: list[dict[str, Any]], size: int) -> list[dict[str, Any]]:
    step = len(items) / size
    return [items[int(i * step)] for i in range(size)]


def propose_sections(
    backend: llm_client.Backend,
    items: list[dict[str, Any]],
    context: dict[str, Any],
    language: str,
) -> list[dict[str, str]]:
    """Ask the model for sections and assign the items in place. A long item list is first
    sampled for the sections and then assigned in batches."""
    batch = prompts.SECTION_BATCH
    sample = items if len(items) <= batch else _sample(items, batch)
    done = llm_client.complete_json(
        backend,
        system=prompts.sections_system(language),
        user=prompts.sections_user(sample, context["instruction"]),
        schema=SECTIONS_SCHEMA,
        name="sections",
        max_tokens=SECTIONS_TOKENS,
    )
    sections: list[dict[str, str]] = done.data["sections"]
    ids = {s["id"] for s in sections}
    if len(ids) != len(sections):
        raise llm_client.LlmError("the proposed sections have duplicate ids")
    if len(items) <= batch:
        _apply_assignments(items, done.data["assignments"], ids)
        return sections
    for group in chunks(items, batch):
        part = llm_client.complete_json(
            backend,
            system=prompts.ASSIGN_SYSTEM,
            user=prompts.assign_user(group, sections),
            schema=ASSIGN_SCHEMA,
            name="assignments",
            max_tokens=SECTIONS_TOKENS,
        )
        _apply_assignments(group, part.data["assignments"], ids)
    return sections


def layout(
    backend: llm_client.Backend | None,
    items: list[dict[str, Any]],
    context: dict[str, Any],
    language: str,
) -> tuple[list[dict[str, str]], str]:
    """Sections for the draft according to the mode; sets `section` of the items.
    Returns the sections and a note for the progress line."""
    mode = context["mode"]
    if mode == "fixed":
        for it in items:
            it["section"] = None
        return [], "fixed sections"
    if mode in extract.TEMPLATE_MODES:
        known = {s["id"] for s in context["sections"]}
        for it in items:
            if it["section"] not in known:
                it["section"] = None
        return [dict(s) for s in context["sections"]], "template sections"
    for it in items:
        it["section"] = None
    if not items:
        return [], "no items"
    try:
        assert backend is not None
        sections = propose_sections(backend, items, context, language)
        return sections, f"{len(sections)} sections proposed"
    except llm_client.LlmError:
        for it in items:
            it["section"] = None
        draft.assign_by_type(items)
        return draft.type_sections(items, language), "sections by type (the model failed)"


# Step -----------------------------------------------------------------------------------


def draft_item(raw: dict[str, Any]) -> dict[str, Any]:
    item = {
        "id": raw["id"],
        "type": raw["type"],
        "text": raw["text"],
        "assignee": raw["assignee"],
        "due": raw["due"],
        "certainty": {"assignee": raw["certainty"]["assignee"], "due": raw["certainty"]["due"]},
        "section": raw["section"],
        "relates_to": raw["relates_to"],
        "source": dict(raw["source"]),
        "edited": False,
    }
    if raw.get("assignee_name") is not None:
        item["assignee_name"] = raw["assignee_name"]
    return item


def unverified_warnings(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        _warning(
            "quote_unverified",
            f"The quote of this item was not found in the transcript "
            f"(match {it['source']['match']:.2f}, at {format_clock(it['source']['t_start_ms'])}): "
            f"{it['text'][:80]}",
            it,
        )
        for it in items
        if not it["source"]["verified"]
    ]


def build_draft(
    manifest: Manifest,
    context: dict[str, Any],
    language: str,
    backend: llm_client.Backend | None,
    windows_doc: dict[str, Any],
    on_call: Callable[[], None],
    people: Sequence[str] = (),
) -> tuple[dict[str, Any], str]:
    results = windows_doc["windows"]
    raw_items = [it for w in results for it in w["items"]]
    warnings = [wa for w in results for wa in w["warnings"] if wa["code"] == "window_skipped"]
    items = dedup.merge_duplicates(raw_items)
    warnings += validate_items(items, context)
    number_items(items)

    on_call()
    try:
        summary = summarize(backend, [w["summary"] for w in results], language, on_call, people)
    except llm_client.LlmError as e:
        summary = fallback_summary([w["summary"] for w in results])
        warnings.append(
            _warning("summary_failed", f"The summary was not written by the model: {e}")
        )
    on_call()
    sections, note = layout(backend, items, context, language)
    on_call()
    final = [draft_item(it) for it in items]
    proposals, proposal_warnings = build_status_proposals(results, context)
    warnings = [*unverified_warnings(items), *proposal_warnings, *warnings]
    short_title = derive_short_title(summary, final)
    descriptions_doc = extract.read_optional(
        manifest.work_dir / "descriptions.json", "screenshot-descriptions"
    )
    key_screenshots = select_key_screenshots(descriptions_doc)
    doc: dict[str, Any] = {
        "version": 1,
        "meeting_id": manifest.meeting_id,
        "cycle": context["cycle"],
        "date": context["meeting_date"],
        "language": language,
        "mode": context["mode"],
        "template": context["template"],
        "meeting_type": context["meeting_type"],
        "sections": sections,
        "summary": summary,
    }
    if short_title:
        doc["short_title"] = short_title
    if key_screenshots:
        doc["key_screenshots"] = key_screenshots
    doc["items"] = final
    if proposals:
        # Left out while empty: a draft without proposals is byte for byte what Stage 4 wrote.
        # Placed before the warnings, the order of the draft type of the application.
        doc["status_proposals"] = proposals
    doc["warnings"] = warnings
    return doc, note


def output_language(
    manifest: Manifest, context: dict[str, Any], windows_doc: dict[str, Any]
) -> str:
    if windows_doc.get("language") in lang.SUPPORTED:
        return str(windows_doc["language"])
    transcript = extract.read_transcript(manifest.work_dir)
    texts = (s["text"] for s in transcript["segments"])
    return lang.resolve(context["output_language"], transcript["language"], texts)


def speaker_names(work: Path) -> list[str]:
    """Names of the speakers that have one, from the transcript the extraction read (the
    named one when the application wrote it). None when there is no transcript file."""
    if not (work / "transcript.named.json").exists() and not (work / "transcript.json").exists():
        return []
    return named_people(extract.read_transcript(work))


def run(manifest: Manifest) -> None:
    cfg = llm_client.config_from(manifest.options)
    cfg.ensure_allowed()
    work = manifest.work_dir
    context = read_json(work / "context.json")
    schema.validate("cycle-context", context)
    windows_doc = read_json(work / "extract" / "windows.json")
    language = output_language(manifest, context, windows_doc)

    summaries = [w["summary"] for w in windows_doc["windows"] if w["summary"].strip()]
    has_items = any(w["items"] for w in windows_doc["windows"])
    needs_model = len(summaries) > 1 or (context["mode"] == "auto" and has_items)
    total = max(3, len(windows_doc["windows"]) * 2 + 2)
    calls = 0

    def on_call() -> None:
        nonlocal calls
        calls += 1
        frac = min(0.95, 0.10 + 0.85 * min(1.0, calls / total))
        protocol.progress(STEP, frac, f"reduce step {calls}")

    protocol.progress(STEP, 0.05, f"{len(windows_doc['windows'])} windows")
    backend_cm = llm_client.open_backend(cfg) if needs_model else nullcontext(None)
    with backend_cm as backend:
        people = speaker_names(work) if backend is not None else []
        doc, note = build_draft(manifest, context, language, backend, windows_doc, on_call, people)
    target = draft.write_draft(work, doc)
    if target.name == draft.DRAFT_NEW:
        protocol.progress(STEP, 1.0, f"{draft.DRAFT} has edits; wrote {draft.DRAFT_NEW} instead")
    else:
        proposed = len(doc.get("status_proposals", []))
        extra = f", {proposed} status proposals" if proposed else ""
        protocol.progress(STEP, 1.0, f"{len(doc['items'])} items, {note}{extra}")
