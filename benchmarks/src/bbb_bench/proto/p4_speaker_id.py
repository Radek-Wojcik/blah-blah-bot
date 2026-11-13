"""P4: enroll voices from one meeting and recognize them in another.

    uv run --group diar python -m bbb_bench.proto.p4_speaker_id \
        --enroll data/scripted/pl-04a --test data/scripted/pl-04b

Both meetings must be synthesized with the same voice seed so that a speaker
keeps the same voice. Speakers present only in the test meeting are unknown
voices and must not be matched to a profile.

Embeddings come from `DiarizeOutput.speaker_embeddings` (one per detected
speaker, in `speaker_diarization.labels()` order). Matching is done per detected
speaker, so accuracy is measured as the share of enrolled speech time attributed
to the correct name.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any

import numpy as np

from .. import harness
from . import diar_common as dc

THRESHOLDS = [0.3, 0.4, 0.5, 0.6, 0.7, 0.8]


def speaker_embeddings(pipeline, meeting: Path):
    """Diarize with the participant bound; return {ref_speaker: (embedding, seconds)}."""
    reference = dc.read_rttm(meeting / "reference.rttm")
    n = len({s for _, _, s in reference})
    segs, _excl, labels, emb = dc.diarize(pipeline, meeting / "mixed.wav", num_speakers=n)
    mapping = dc.best_overlap_mapping(reference, segs)
    out: dict[str, tuple[np.ndarray, float]] = {}
    for i, label in enumerate(labels):
        if label in mapping:
            ref_speaker, seconds = mapping[label]
            if ref_speaker not in out or seconds > out[ref_speaker][1]:
                out[ref_speaker] = (np.asarray(emb[i]), seconds)
    return out, {
        s: sum(e - b for b, e, sp in reference if sp == s) for s in {sp for _, _, sp in reference}
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    pipeline = dc.load_pipeline()
    profiles, _ = speaker_embeddings(pipeline, Path(args.enroll))
    found, speech = speaker_embeddings(pipeline, Path(args.test))
    rows = []
    for thr in THRESHOLDS:
        correct = total = 0.0
        unknown_total = false_accepts = 0
        for speaker, (vec, _) in found.items():
            scores = {name: dc.cosine(vec, p[0]) for name, p in profiles.items()}
            best_name, best = max(scores.items(), key=lambda kv: kv[1])
            predicted = best_name if best >= thr else None
            if speaker in profiles:
                total += speech[speaker]
                if predicted == speaker:
                    correct += speech[speaker]
            else:
                unknown_total += 1
                if predicted is not None:
                    false_accepts += 1
        rows.append(
            {
                "threshold": thr,
                "speaker_accuracy": correct / total if total else None,
                "false_accept_rate": false_accepts / unknown_total if unknown_total else None,
                "unknown_speakers": unknown_total,
            }
        )
    return {
        "enrolled_speakers": sorted(profiles),
        "test_speakers": sorted(found),
        "embedding_dim": int(next(iter(profiles.values()))[0].shape[0]),
        "thresholds": rows,
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--enroll", required=True)
    p.add_argument("--test", required=True)
    p.add_argument("--name", default="community-1")
    args = p.parse_args()
    harness.run_prototype("p4-speaker-id", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
