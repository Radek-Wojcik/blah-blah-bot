"""P4b: distribution of voice-embedding similarity for the same and for different voices.

    uv run --group diar python -m bbb_bench.proto.p4_similarity \n        data/audio/pl-04a data/audio/pl-04b ...

Every meeting is diarized with the speaker count bounded by its script and one embedding
is kept per speaker. Speakers are identified across meetings by their synthesized voice,
so a pair is "same" when two different meetings use the same voice and "different"
otherwise. The same meetings are processed in two conditions: as synthesized, and
narrowband with noise (a rough stand-in for compressed conference audio).

The result lists every pair so that thresholds can be chosen from examples.
"""

from __future__ import annotations

import argparse
import itertools
import json
import tempfile
import wave
from pathlib import Path
from typing import Any

import numpy as np

from .. import harness
from . import diar_common as dc

THRESHOLDS = [round(0.30 + 0.05 * i, 2) for i in range(13)]


def narrowband(samples: np.ndarray, rate: int, seed: int = 7) -> np.ndarray:
    """Low-pass at 3.4 kHz, add light noise and 8-bit-like quantization."""
    spectrum = np.fft.rfft(samples)
    freqs = np.fft.rfftfreq(len(samples), 1 / rate)
    spectrum[freqs > 3400] = 0
    out = np.fft.irfft(spectrum, n=len(samples)).astype(np.float32)
    rng = np.random.default_rng(seed)
    peak = float(np.percentile(np.abs(out), 99)) or 1.0
    out += rng.normal(0, 0.01 * peak, out.shape).astype(np.float32)
    out = np.round(out / peak * 127) / 127 * peak
    return np.clip(out, -1.0, 1.0)


def write_wav(path: Path, samples: np.ndarray, rate: int) -> None:
    pcm = (np.clip(samples, -1, 1) * 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())


def embeddings_of(pipeline, wav: Path, meeting: Path) -> dict[str, tuple[np.ndarray, float]]:
    reference = dc.read_rttm(meeting / "reference.rttm")
    n = len({s for _, _, s in reference})
    segs, _excl, labels, emb = dc.diarize(pipeline, wav, num_speakers=n)
    mapping = dc.best_overlap_mapping(reference, segs)
    out: dict[str, tuple[np.ndarray, float]] = {}
    for i, label in enumerate(labels):
        if label in mapping:
            speaker, seconds = mapping[label]
            if speaker not in out or seconds > out[speaker][1]:
                out[speaker] = (np.asarray(emb[i], dtype=np.float64), seconds)
    return out


def summarize(values: list[float]) -> dict[str, float | int | None]:
    if not values:
        return {"count": 0, "min": None, "p05": None, "median": None, "p95": None, "max": None}
    a = np.asarray(values)
    return {
        "count": len(values),
        "min": round(float(a.min()), 4),
        "p05": round(float(np.percentile(a, 5)), 4),
        "median": round(float(np.median(a)), 4),
        "p95": round(float(np.percentile(a, 95)), 4),
        "max": round(float(a.max()), 4),
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    pipeline = dc.load_pipeline()
    meetings = [Path(m) for m in args.meetings]
    conditions: dict[str, Any] = {}
    for condition in ("clean", "narrowband"):
        items = []  # (meeting, speaker, voice, embedding)
        for meeting in meetings:
            voices = json.loads((meeting / "speakers.json").read_text(encoding="utf-8"))
            wav = meeting / "mixed.wav"
            if condition == "narrowband":
                samples, rate = dc.read_wav_mono16k(wav)
                tmp = Path(tempfile.mkdtemp()) / "nb.wav"
                write_wav(tmp, narrowband(samples, rate), rate)
                wav = tmp
            for speaker, (vec, _sec) in embeddings_of(pipeline, wav, meeting).items():
                items.append((meeting.name, speaker, voices[speaker]["voice"], vec))
        pairs = []
        for (ma, sa, va, ea), (mb, sb, vb, eb) in itertools.combinations(items, 2):
            if ma == mb:
                continue  # speakers inside one meeting are distinct by construction
            pairs.append(
                {
                    "a": f"{ma}:{sa}",
                    "b": f"{mb}:{sb}",
                    "same_voice": va == vb,
                    "cosine": round(dc.cosine(ea, eb), 4),
                }
            )
        same = [p["cosine"] for p in pairs if p["same_voice"]]
        diff = [p["cosine"] for p in pairs if not p["same_voice"]]
        rows = [
            {
                "threshold": t,
                "false_reject_rate": (
                    round(sum(c < t for c in same) / len(same), 4) if same else None
                ),
                "false_accept_rate": (
                    round(sum(c >= t for c in diff) / len(diff), 4) if diff else None
                ),
            }
            for t in THRESHOLDS
        ]
        conditions[condition] = {
            "same_voice": summarize(same),
            "different_voice": summarize(diff),
            "thresholds": rows,
            "pairs": pairs,
        }
    return {"meetings": [m.name for m in meetings], "conditions": conditions}


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("meetings", nargs="+")
    p.add_argument("--name", default="community-1")
    args = p.parse_args()
    harness.run_prototype("p4-similarity", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
