"""Stable speaker -> reference voice assignment (no models involved)."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

CATALOG_PATH = Path(__file__).with_name("voices.json")

# Gender of the fictional participants of the meeting generator; unknown ids fall back to a hash.
NAME_GENDER = {
    "kasia": "f", "marta": "f", "ola": "f", "ewa": "f", "zosia": "f", "ania": "f", "iza": "f",
    "magda": "f", "basia": "f", "nina": "f",
    "jan": "m", "piotr": "m", "tomek": "m", "bartek": "m", "marek": "m", "pawel": "m",
    "darek": "m", "lukasz": "m", "michal": "m",
}  # fmt: skip


def load_catalog(path: Path = CATALOG_PATH) -> list[dict]:
    return json.loads(path.read_text("utf-8"))["voices"]


def _h(*parts: object) -> int:
    digest = hashlib.sha256(":".join(str(p) for p in parts).encode()).digest()
    return int.from_bytes(digest[:8], "big")


def gender_of(speaker: str, seed: int) -> str:
    """Gender used to pick a voice: from the name when known, otherwise from a hash."""
    known = NAME_GENDER.get(speaker.lower())
    return known if known else ("f" if _h("gender", seed, speaker) % 2 else "m")


def assign_voices(speakers: list[str], voices: list[dict], seed: int) -> dict[str, str]:
    """Map every speaker to a distinct voice id.

    A speaker prefers voices of matching gender, ordered by a hash of (seed, speaker, voice), so
    the same id and seed give the same voice whenever there is no clash inside the meeting.
    Speakers are served in sorted order, so the input order does not matter.
    """
    unique = sorted(set(speakers))
    if len(unique) > len(voices):
        raise ValueError(f"{len(unique)} speakers but only {len(voices)} reference voices")
    taken: set[str] = set()
    result: dict[str, str] = {}
    for speaker in unique:
        want = gender_of(speaker, seed)
        ranked = sorted(
            voices, key=lambda v: (v["gender"] != want, _h("voice", seed, speaker, v["id"]))
        )
        choice = next(v["id"] for v in ranked if v["id"] not in taken)
        taken.add(choice)
        result[speaker] = choice
    return result
