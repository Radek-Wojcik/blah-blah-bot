"""P2: ASR comparison on the scripted sets.

Run one configuration over every meeting directory that holds `mixed.wav`,
`reference.txt` and `annotation.json`:

    uv run --group asr python -m bbb_bench.proto.p2_asr \
        --model large-v3 --language auto --glossary --meetings data/scripted/*

Writes benchmarks/results/p2-asr-<model>-<language>-<glossary>.json.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from .. import harness, langid
from ..datasets.pools import GLOSSARY
from ..metrics import rtfx, term_recall, translation_rate, wer


def audio_seconds(path: Path) -> float:
    import wave

    with wave.open(str(path), "rb") as w:
        return w.getnframes() / w.getframerate()


def load_whisper(name: str):
    from faster_whisper import WhisperModel

    return WhisperModel(name, device="cuda", compute_type="int8_float16")


def transcribe_whisper(model, wav: Path, language: str, glossary: bool):
    """`language` is 'auto' or a language code ('forced' is resolved by the caller)."""
    kwargs: dict[str, Any] = {"word_timestamps": True, "vad_filter": True, "beam_size": 5}
    if language != "auto":
        kwargs["language"] = language
    if glossary:
        kwargs["initial_prompt"] = "Glossary: " + ", ".join(GLOSSARY) + "."
    segments, info = model.transcribe(str(wav), **kwargs)
    segs = [{"start": s.start, "end": s.end, "text": s.text.strip()} for s in segments]
    return segs, info.language


def load_parakeet():
    import onnx_asr

    model = onnx_asr.load_model(
        "nemo-parakeet-tdt-0.6b-v3", providers=["CUDAExecutionProvider", "CPUExecutionProvider"]
    )
    return model.with_vad(onnx_asr.load_vad("silero"))


def transcribe_parakeet(model, wav: Path, language: str, glossary: bool):
    segs = []
    for seg in model.recognize(str(wav)):
        segs.append({"start": seg.start, "end": seg.end, "text": seg.text.strip()})
    return segs, "auto"


def run(args: argparse.Namespace) -> dict[str, Any]:
    if args.model == "parakeet":
        model, transcribe = load_parakeet(), transcribe_parakeet
    else:
        model = load_whisper("large-v3" if args.model == "large-v3" else "turbo")
        transcribe = transcribe_whisper

    per_meeting = []
    total_audio = total_proc = 0.0
    with harness.VramSampler() as vram:
        for meeting in map(Path, args.meetings):
            wav = meeting / "mixed.wav"
            ann = json.loads((meeting / "annotation.json").read_text(encoding="utf-8"))
            reference = (meeting / "reference.txt").read_text(encoding="utf-8")
            with harness.timer() as elapsed:
                language = ann["lang"] if args.language == "forced" else args.language
                segs, _lang = transcribe(model, wav, language, args.glossary)
            dur = audio_seconds(wav)
            hyp = " ".join(s["text"] for s in segs)
            score = wer(reference, hyp)
            terms = term_recall(ann["terms"], hyp) if ann.get("terms") else None
            out_langs = [langid.detect(s["text"]) for s in segs]
            # Spoken language per ASR segment: the language of the script turn at that time.
            tl = json.loads((meeting / "timeline.json").read_text(encoding="utf-8"))
            spoken = []
            for s in segs:
                mid = (s["start"] + s["end"]) / 2
                turn = next((t for t in tl if t["start_s"] <= mid <= t["end_s"]), None)
                spoken.append(turn["lang"] if turn else ann["lang"])
            tr = translation_rate(spoken, out_langs)
            total_audio += dur
            total_proc += elapsed()
            per_meeting.append(
                {
                    "meeting": meeting.name,
                    "lang": ann["lang"],
                    "audio_s": dur,
                    "processing_s": elapsed(),
                    "wer": score["wer"],
                    "ref_words": score["ref_words"],
                    "term_recall": terms["term_recall"] if terms else None,
                    "translation_rate": tr["translation_rate"],
                }
            )
    return {
        "config": {
            "model": args.model,
            "language": args.language,
            "glossary": args.glossary,
        },
        "meetings": per_meeting,
        "rtfx": rtfx(total_audio, total_proc),
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
        "mean_wer": sum(m["wer"] for m in per_meeting) / len(per_meeting),
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", choices=["large-v3", "turbo", "parakeet"], required=True)
    p.add_argument(
        "--language",
        choices=["auto", "pl", "en", "forced"],
        default="auto",
        help="forced = the language of each meeting",
    )
    p.add_argument("--glossary", action="store_true")
    p.add_argument("--meetings", nargs="+", required=True)
    p.add_argument("--name", default=None, help="Result file suffix; defaults to the configuration")
    args = p.parse_args()
    name = args.name or f"{args.model}-{args.language}-{'gloss' if args.glossary else 'nogloss'}"
    harness.run_prototype("p2-asr", name, lambda: run(args))


if __name__ == "__main__":
    main()
