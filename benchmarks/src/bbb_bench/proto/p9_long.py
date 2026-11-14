"""P9: pipeline timing on a 10 hour recording.

    uv run python -m bbb_bench.proto.p9_long build --audio data/audio --out data/long
    uv run --group asr python -m bbb_bench.proto.p9_long asr --wav data/long/long-10h.wav
    uv run --group diar python -m bbb_bench.proto.p9_long diarize --wav data/long/long-10h.wav
    uv run python -m bbb_bench.proto.p9_long estimate

`build` concatenates the synthesized meetings (repeated) into one 16 kHz mono WAV of
about ten hours. `asr` and `diarize` run the chosen models over the whole file and
record time and peak VRAM; they also record how far VRAM falls back to the baseline
once the model is released. `estimate` adds the language model stage, extrapolated from
the P5 run because the stage works on chunks and its cost scales with the word count,
and writes the end-to-end estimate. The long file is a repetition of short meetings:
it tests duration, memory and time, not recognition quality.
"""

from __future__ import annotations

import argparse
import gc
import json
import time
import wave
from pathlib import Path
from typing import Any

from .. import harness
from ..datasets.generator import WORDS_PER_MINUTE

TARGET_HOURS = 10.0


def build(audio_dir: Path, out_dir: Path, hours: float = TARGET_HOURS) -> Path:
    """Concatenate `mixed.wav` files (round robin) until `hours` of audio are written."""
    sources = sorted(audio_dir.glob("*/mixed.wav"))
    if not sources:
        raise SystemExit(f"no mixed.wav files under {audio_dir}")
    out_dir.mkdir(parents=True, exist_ok=True)
    target = out_dir / f"long-{int(hours)}h.wav"
    wanted = int(hours * 3600 * 16000)
    written = 0
    with wave.open(str(target), "wb") as dst:
        dst.setnchannels(1)
        dst.setsampwidth(2)
        dst.setframerate(16000)
        while written < wanted:
            for src in sources:
                with wave.open(str(src), "rb") as w:
                    frames = w.readframes(w.getnframes())
                take = min(len(frames) // 2, wanted - written)
                dst.writeframes(frames[: take * 2])
                written += take
                if written >= wanted:
                    break
    return target


def _seconds(wav: Path) -> float:
    with wave.open(str(wav), "rb") as w:
        return w.getnframes() / w.getframerate()


def run_asr(wav: Path, model_name: str) -> dict[str, Any]:
    from .p2_asr import load_whisper, transcribe_whisper

    audio_s = _seconds(wav)
    baseline = harness.gpu_memory_used_mib() or 0
    with harness.VramSampler() as vram:
        with harness.timer() as load_time:
            model = load_whisper(model_name)
        loaded_s = load_time()
        with harness.timer() as elapsed:
            segments, language = transcribe_whisper(model, wav, "auto", False)
        seconds = elapsed()
    words = sum(len(s["text"].split()) for s in segments)
    del model
    gc.collect()
    time.sleep(3)
    return {
        "config": {"model": model_name, "language": "auto"},
        "audio_s": audio_s,
        "model_load_s": loaded_s,
        "processing_s": seconds,
        "rtfx": audio_s / seconds,
        "segments": len(segments),
        "words": words,
        "detected_language": language,
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
        "vram_after_release_above_baseline_mib": (harness.gpu_memory_used_mib() or 0) - baseline,
    }


def run_diarize(wav: Path) -> dict[str, Any]:
    import torch

    from . import diar_common as dc

    audio_s = _seconds(wav)
    baseline = harness.gpu_memory_used_mib() or 0
    with harness.VramSampler() as vram:
        with harness.timer() as load_time:
            pipeline = dc.load_pipeline()
        loaded_s = load_time()
        with harness.timer() as elapsed:
            segs, _excl, labels, _emb = dc.diarize(pipeline, wav)
        seconds = elapsed()
    del pipeline
    gc.collect()
    torch.cuda.empty_cache()
    time.sleep(3)
    return {
        "audio_s": audio_s,
        "model_load_s": loaded_s,
        "processing_s": seconds,
        "rtfx": audio_s / seconds,
        "segments": len(segs),
        "detected_speakers": len(labels),
        "torch_peak_allocated_mib": round(torch.cuda.max_memory_allocated() / 2**20),
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
        "vram_after_release_above_baseline_mib": (harness.gpu_memory_used_mib() or 0) - baseline,
    }


def estimate(llm_result: Path, asr_result: Path, diar_result: Path) -> dict[str, Any]:
    llm = json.loads(llm_result.read_text(encoding="utf-8"))
    asr = json.loads(asr_result.read_text(encoding="utf-8"))
    diar = json.loads(diar_result.read_text(encoding="utf-8"))
    hours = asr["audio_s"] / 3600
    words = asr["words"] or int(hours * 60 * WORDS_PER_MINUTE)
    llm_s = llm["seconds_per_1000_words"] * words / 1000
    stages = {
        "transcription_s": asr["processing_s"] + asr["model_load_s"],
        "diarization_s": diar["processing_s"] + diar["model_load_s"],
        "extraction_s": llm_s,
    }
    total = sum(stages.values())
    return {
        "audio_hours": hours,
        "words_used": words,
        "language_model": llm["config"]["model"],
        "stages": stages,
        "total_s": total,
        "total_hours": total / 3600,
        "peak_vram_above_baseline_mib": max(
            asr["peak_vram_above_baseline_mib"],
            diar["peak_vram_above_baseline_mib"],
            llm["peak_vram_above_baseline_mib"],
        ),
        "note": (
            "Transcription and diarization are measured on the long file. Extraction is "
            "extrapolated from seconds per 1000 words in P5 (chunked, linear in words). "
            "Summary levels and publishing are not included."
        ),
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--audio", type=Path, default=Path("data/audio"))
    b.add_argument("--out", type=Path, default=Path("data/long"))
    b.add_argument("--hours", type=float, default=TARGET_HOURS)
    a = sub.add_parser("asr")
    a.add_argument("--wav", type=Path, required=True)
    a.add_argument("--model", default="turbo")
    d = sub.add_parser("diarize")
    d.add_argument("--wav", type=Path, required=True)
    e = sub.add_parser("estimate")
    e.add_argument("--llm", type=Path, default=harness.RESULTS_DIR / "p5-llm-gemma-4-12b-q4.json")
    args = p.parse_args()
    if args.cmd == "build":
        print(build(args.audio, args.out, args.hours))
    elif args.cmd == "asr":
        harness.run_prototype("p9-long", "asr", lambda: run_asr(args.wav, args.model))
    elif args.cmd == "diarize":
        harness.run_prototype("p9-long", "diarize", lambda: run_diarize(args.wav))
    else:
        asr = harness.RESULTS_DIR / "p9-long-asr.json"
        diar = harness.RESULTS_DIR / "p9-long-diarize.json"
        harness.run_prototype("p9-long", "estimate", lambda: estimate(args.llm, asr, diar))


if __name__ == "__main__":
    main()
