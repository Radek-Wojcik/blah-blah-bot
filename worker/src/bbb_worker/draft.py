"""The meeting draft (`work/draft.json`, schema `meeting`): construction and safe writing."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from . import schema
from .steps import write_bytes, write_json

DRAFT = "draft.json"
DRAFT_NEW = "draft.new.json"
# SHA-256 (lowercase hex) of the exact bytes of the `draft.json` that `reduce` wrote last.
BASE_HASH = "draft.base.sha256"

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


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def has_user_edits(path: Path) -> bool:
    """True when the draft at `path` was touched by the user: an item or a status proposal has
    `edited: true` (or a proposal was decided), or the file no longer has the bytes `reduce`
    wrote (the hash recorded next to it in `draft.base.sha256`). Deleting items or changing sections and the report mode set no flag,
    so the hash is what catches them. Without a recorded hash only the flags count. A draft
    that cannot be read, or a recorded hash that is not a hash, is treated as edited, because
    the draft is not ours to overwrite blindly."""
    if not path.exists():
        return False
    try:
        raw = path.read_bytes()
        doc = json.loads(raw.decode("utf-8"))
        if any(it.get("edited") for it in doc.get("items", [])):
            return True
        if any(
            p.get("edited") or p.get("decision", "proposed") != "proposed"
            for p in doc.get("status_proposals", [])
        ):
            return True
        recorded_file = path.with_name(BASE_HASH)
        if not recorded_file.exists():
            return False
        recorded = recorded_file.read_text(encoding="utf-8").strip().lower()
    except (OSError, ValueError, AttributeError, TypeError):
        return True
    return recorded != sha256_hex(raw)


def write_draft(work_dir: Path, draft: dict[str, Any]) -> Path:
    """Validate and write the draft. A draft with the user's edits is never overwritten: the
    new one goes to `draft.new.json` and the caller reports that. A written `draft.json` is
    followed by `draft.base.sha256`, the SHA-256 (lowercase hex) of its exact bytes, which the
    editor compares with the file to know whether it was changed since."""
    schema.validate("meeting", draft)
    target = work_dir / DRAFT
    if has_user_edits(target):
        write_json(work_dir / DRAFT_NEW, draft)
        return work_dir / DRAFT_NEW
    data = write_json(target, draft)
    write_bytes(work_dir / BASE_HASH, sha256_hex(data).encode("ascii"))
    return target
