"""P6: describe synthetic slides with a vision-language model via llama-server.

    uv run python -m bbb_bench.proto.p6_vision --model Qwen3VL-8B-Instruct-Q4_K_M.gguf \
        --mmproj mmproj-Qwen3VL-8B-Instruct-F16.gguf --slides data/slides

Scores the share of key facts present in each description, counts numbers in the
description that appear nowhere in the image's facts (candidate inventions, to be
reviewed by hand) and records the time per image.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
from pathlib import Path
from typing import Any

from .. import harness
from ..metrics.text import normalize
from . import llama_server

PROMPT = (
    "Describe this slide for a meeting transcript. State every title, label, name and number "
    "you can read, and what the chart, table or diagram shows. Do not add anything that is "
    "not visible."
)


def facts_present(description: str, facts: list[str]) -> list[str]:
    """Return the facts found in the description (normalized, whole-word match)."""
    haystack = " " + " ".join(normalize(description)) + " "
    return [f for f in facts if f" {' '.join(normalize(f))} " in haystack]


def invented_numbers(description: str, facts: list[str]) -> list[str]:
    """Numbers of two or more digits in the description that occur in no fact of the image.

    Single digits are ignored because they are usually list numbering or unit names
    such as "3D"; they are covered by the manual review of the descriptions.
    """
    allowed = {n for f in facts for n in re.findall(r"\d+", f)}
    return sorted({n for n in re.findall(r"\d{2,}", description) if n not in allowed})


def describe(base: str, image: Path) -> tuple[str, float]:
    b64 = base64.b64encode(image.read_bytes()).decode("ascii")
    payload = {
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": PROMPT},
                    {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}},
                ],
            }
        ],
        "temperature": 0,
        "max_tokens": 600,
    }
    with harness.timer() as elapsed:
        resp = llama_server.chat(base, payload)
    return resp["choices"][0]["message"]["content"], elapsed()


def run(args: argparse.Namespace) -> dict[str, Any]:
    slides = Path(args.slides)
    facts = json.loads((slides / "facts.json").read_text(encoding="utf-8"))
    per_image = []
    with (
        harness.VramSampler() as vram,
        llama_server.running(Path(args.model), mmproj=Path(args.mmproj), ctx=8192) as base,
    ):
        for name, key_facts in facts.items():
            text, secs = describe(base, slides / name)
            found = facts_present(text, key_facts)
            per_image.append(
                {
                    "image": name,
                    "seconds": secs,
                    "fact_recall": len(found) / len(key_facts),
                    "missing": [f for f in key_facts if f not in found],
                    "invented_numbers": invented_numbers(text, key_facts),
                    "description": text,
                }
            )
    n = len(per_image)
    return {
        "config": {"model": Path(args.model).name, "mmproj": Path(args.mmproj).name},
        "images": per_image,
        "mean_fact_recall": sum(i["fact_recall"] for i in per_image) / n,
        "mean_seconds": sum(i["seconds"] for i in per_image) / n,
        "images_with_invented_numbers": sum(1 for i in per_image if i["invented_numbers"]),
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", required=True)
    p.add_argument("--mmproj", required=True)
    p.add_argument("--slides", default="data/slides")
    p.add_argument("--name", default="qwen3-vl-8b")
    args = p.parse_args()
    harness.run_prototype("p6-vision", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
