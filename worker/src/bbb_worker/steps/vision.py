"""Describe each screenshot with a local vision model.

The transcript around the screenshot's moment is given as context so that names and
terms spoken in the meeting are used correctly. This step always runs locally.
"""

from __future__ import annotations

import base64
from contextlib import nullcontext
from typing import Any

from .. import llama, models, protocol, schema, transcript
from ..manifest import Manifest
from . import read_json, write_json

MAX_CONTEXT_CHARS = 3000

PROMPT = (
    "Describe this screenshot from a meeting for the meeting notes. State every title, label, "
    "name and number you can read, and what the chart, table, code or diagram shows. Do not add "
    "anything that is not visible. Use the discussion only to interpret what you see; do not "
    "summarise it. Write the description in {language}.\n\n"
    "Discussion around the moment of the screenshot:\n{context}"
)

LANGUAGE_NAMES = {"pl": "Polish", "en": "English"}


def excluded_ids(edits: dict[str, Any] | None) -> set[int]:
    """Ids of the segments the user excluded (`edits.json`)."""
    segments = (edits or {}).get("segments", {}) or {}
    return {int(k) for k, v in segments.items() if isinstance(v, dict) and v.get("excluded")}


def context_text(doc: dict[str, Any], offset_ms: float, excluded: set[int] | None = None) -> str:
    """Speaker-labelled transcript lines around a moment, capped in length. Excluded segments
    never become context: the description would carry them into the report."""
    labels = {s["id"]: s["label"] for s in doc["speakers"]}
    lines = [
        f"{labels.get(s['speaker'], s['speaker'])}: {s['text']}"
        for s in transcript.context_around(doc, offset_ms)
        if s["id"] not in (excluded or ())
    ]
    text = "\n".join(lines)
    return text[-MAX_CONTEXT_CHARS:] if len(text) > MAX_CONTEXT_CHARS else text


def build_payload(image_png: bytes, prompt: str) -> dict[str, Any]:
    b64 = base64.b64encode(image_png).decode("ascii")
    return {
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}},
                ],
            }
        ],
        "temperature": 0,
        "max_tokens": 600,
    }


def run(manifest: Manifest) -> None:
    out_path = manifest.work_dir / "descriptions.json"
    items: list[dict[str, Any]] = []
    if not manifest.screenshots:
        protocol.progress("vision", 0.05, "no screenshots")
        doc = {"version": 1, "meeting_id": manifest.meeting_id, "items": items}
        schema.validate("screenshot-descriptions", doc)
        write_json(out_path, doc)
        protocol.progress("vision", 1.0, "no screenshots")
        return

    protocol.progress("vision", 0.05, "starting vision model")
    transcript_doc = read_json(manifest.work_dir / "transcript.json")
    edits_path = manifest.work_dir / "edits.json"
    excluded = excluded_ids(read_json(edits_path) if edits_path.exists() else None)
    language = LANGUAGE_NAMES.get(transcript_doc["language"], "the language of the discussion")

    external = manifest.options.get("vision_base_url")
    if external:
        server = nullcontext(external)
        model_name = "external"
    else:
        folder = models.local_path("vision")
        server = llama.running(
            folder / "Qwen3VL-8B-Instruct-Q4_K_M.gguf",
            mmproj=folder / "mmproj-Qwen3VL-8B-Instruct-F16.gguf",
            ctx=8192,
        )
        model_name = "Qwen3-VL-8B-Instruct"

    total = len(manifest.screenshots)
    with server as base:
        for i, (rel, offset_ms) in enumerate(manifest.screenshots):
            start_frac = 0.10 + 0.85 * (i / total)
            end_frac = 0.10 + 0.85 * ((i + 1) / total)
            protocol.progress("vision", start_frac, f"describing {rel} ({i + 1}/{total})")
            image = (manifest.meeting_dir / rel).read_bytes()
            prompt = PROMPT.format(
                language=language, context=context_text(transcript_doc, offset_ms, excluded)
            )
            resp = llama.chat(base, build_payload(image, prompt))
            items.append(
                {
                    "file": rel,
                    "offset_ms": int(offset_ms),
                    "description": resp["choices"][0]["message"]["content"].strip(),
                    "model": model_name,
                }
            )
            protocol.progress("vision", end_frac, rel)

    doc = {"version": 1, "meeting_id": manifest.meeting_id, "items": items}
    schema.validate("screenshot-descriptions", doc)
    write_json(out_path, doc)
    protocol.progress("vision", 1.0, f"{len(items)} screenshots")
