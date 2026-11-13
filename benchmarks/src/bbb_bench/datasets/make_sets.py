"""Generate the script sets used by the prototypes under benchmarks/data/.

    uv run python -m bbb_bench.datasets.make_sets --root data

Writes scripts and annotations only. Audio is produced from the `scripts/`
folders with the speech synthesis tool in benchmarks/tools/tts, see its README.
Sets (all deterministic):

* `extraction/*`   10 text transcripts for the language model comparison (P5)
* `scripts/*`      scripted meetings to synthesize: Polish with 3, 8 and 16 speakers,
                   English with 3, mixed with 4, and a Polish pair for speaker
                   recognition (same voices, one unknown speaker in the second)
"""

from __future__ import annotations

import argparse
from pathlib import Path

from . import pools
from .generator import generate_meeting, to_files

ENROLLED = ["alex", "kasia", "jan", "marta"]
WITH_UNKNOWN = ["alex", "kasia", "jan", "ewa"]  # "ewa" was never enrolled

# (name, lang, mixed, speakers, minutes, seed, people)
SCRIPTS = [
    ("pl-03", "pl", False, 3, 15, 21, None),
    ("pl-08", "pl", False, 8, 15, 22, None),
    ("pl-16", "pl", False, 16, 15, 23, None),
    ("en-03", "en", False, 3, 15, 24, None),
    ("mixed-04", "pl", True, 4, 15, 25, None),
    ("pl-04a", "pl", False, 4, 10, 27, ENROLLED),
    ("pl-04b", "pl", False, 4, 10, 28, WITH_UNKNOWN),
]
# 10 transcripts: 4 Polish, 3 English, 3 mixed; text only.
EXTRACTION = (
    [(f"pl-{i}", "pl", False, 5, 15, 40 + i) for i in range(4)]
    + [(f"en-{i}", "en", False, 5, 15, 50 + i) for i in range(3)]
    + [(f"mixed-{i}", "pl", True, 5, 15, 60 + i) for i in range(3)]
)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", type=Path, default=Path("data"))
    args = p.parse_args()
    for name, lang, mixed, n, minutes, seed, people in SCRIPTS:
        m = generate_meeting(lang, n, minutes, seed, mixed, people=people)
        to_files(m, args.root / "scripts" / name, pools.GLOSSARY if mixed else None)
        print(f"scripts/{name}: {len(m.turns)} turns, {m.words()} words")
    for name, lang, mixed, n, minutes, seed in EXTRACTION:
        m = generate_meeting(lang, n, minutes, seed, mixed, task_density="sparse")
        to_files(m, args.root / "extraction" / name, pools.GLOSSARY if mixed else None)
        print(f"extraction/{name}: {len(m.tasks)} tasks")


if __name__ == "__main__":
    main()
