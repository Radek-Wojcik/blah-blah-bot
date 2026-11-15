"""The meeting draft (`work/draft.json`, schema `meeting`): construction and safe writing."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from . import schema
from .steps import read_json, write_json

DRAFT = "draft.json"
DRAFT_NEW = "draft.new.json"

# Built-in sections of the fixed layout, in this order.
FIXED_ORDER = ("decision", "task", "topic", "note", "idea", "budget")
FIXED_IDS = {
    "decision": "decisions",
    "task": "tasks",
    "topic": "topics",
    "note": "notes",
    "idea": "ideas",
    "budget": "budgets",
}
FIXED_TITLES = {
    "en": {
        "decision": "Decisions",
        "task": "Tasks",
        "topic": "Open topics",
        "note": "Notes",
        "idea": "Ideas",
        "budget": "Budgets",
    },
    "pl": {
        "decision": "Decyzje",
        "task": "Zadania",
        "topic": "Otwarte tematy",
        "note": "Notatki",
        "idea": "Pomysły",
        "budget": "Budżety",
    },
}


def type_sections(items: list[dict[str, Any]], language: str) -> list[dict[str, str]]:
    """Sections named after the item types that occur (the fallback of the auto mode)."""
    present = {it["type"] for it in items}
    titles = FIXED_TITLES.get(language, FIXED_TITLES["en"])
    return [
        {"id": FIXED_IDS[t], "title": titles[t], "description": ""}
        for t in FIXED_ORDER
        if t in present
    ]


def assign_by_type(items: list[dict[str, Any]]) -> None:
    for it in items:
        it["section"] = FIXED_IDS[it["type"]]


def has_user_edits(path: Path) -> bool:
    """True when the draft at `path` has an item the user touched. A draft that cannot be
    read is treated as edited, because it is not ours to overwrite blindly."""
    if not path.exists():
        return False
    try:
        doc = read_json(path)
        return any(it.get("edited") for it in doc.get("items", []))
    except (ValueError, AttributeError, TypeError):
        return True


def write_draft(work_dir: Path, draft: dict[str, Any]) -> Path:
    """Validate and write the draft. A draft with the user's edits is never overwritten: the
    new one goes to `draft.new.json` and the caller reports that."""
    schema.validate("meeting", draft)
    target = work_dir / DRAFT
    if has_user_edits(target):
        target = work_dir / DRAFT_NEW
    write_json(target, draft)
    return target
