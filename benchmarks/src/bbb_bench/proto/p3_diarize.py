"""P3: diarization with pyannote community-1 on scripted meetings.

    uv run --group diar python -m bbb_bench.proto.p3_diarize --meetings data/scripted/*

Each meeting directory needs `mixed.wav` and `reference.rttm`. Every meeting is
run twice: without speaker bounds and with the speaker count bounded by the
participant list (`num_speakers` equal to the number of scripted speakers).

The reference of synthesized meetings spans whole speech clips, which include
silence. Each run therefore reports two error rates: `der` against the clip-level
reference and `der_voiced_reference` against the reference cut down to the stretches
where the recording has sound (see `diar_common.voiced_reference`).
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any

from .. import harness
from ..metrics import der, rtfx
from . import diar_common as dc


def run(args: argparse.Namespace) -> dict[str, Any]:
    pipeline = dc.load_pipeline()
    results = []
    total_audio = total_proc = 0.0
    with harness.VramSampler() as vram:
        for meeting in map(Path, args.meetings):
            wav = meeting / "mixed.wav"
            reference = dc.read_rttm(meeting / "reference.rttm")
            samples, rate = dc.read_wav_mono16k(wav)
            voiced = dc.voiced_reference(samples, rate, reference)
            n_speakers = len({s for _, _, s in reference})
            audio_s = max(e for _, e, _ in reference)
            for mode, bounds in (("unbounded", {}), ("bounded", {"num_speakers": n_speakers})):
                with harness.timer() as elapsed:
                    segs, excl, labels, emb = dc.diarize(pipeline, wav, **bounds)
                total_audio += audio_s
                total_proc += elapsed()
                score = der(reference, segs)
                score_voiced = der(voiced, segs)
                results.append(
                    {
                        "meeting": meeting.name,
                        "mode": mode,
                        "reference_speakers": n_speakers,
                        "detected_speakers": len(labels),
                        "audio_s": audio_s,
                        "processing_s": elapsed(),
                        "der": score["der"],
                        "missed": score["missed"],
                        "false_alarm": score["false_alarm"],
                        "confusion": score["confusion"],
                        "der_voiced_reference": score_voiced["der"],
                        "missed_voiced_reference": score_voiced["missed"],
                        "false_alarm_voiced_reference": score_voiced["false_alarm"],
                        "confusion_voiced_reference": score_voiced["confusion"],
                        "reference_speech_s": sum(e - b for b, e, _ in reference),
                        "voiced_reference_speech_s": sum(e - b for b, e, _ in voiced),
                        "embedding_shape": list(emb.shape) if emb is not None else None,
                    }
                )
    import torch

    return {
        "torch_peak_allocated_mib": round(torch.cuda.max_memory_allocated() / 2**20),
        "runs": results,
        "rtfx": rtfx(total_audio, total_proc),
        "peak_vram_above_baseline_mib": vram.peak_above_baseline_mib,
    }


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--meetings", nargs="+", required=True)
    p.add_argument("--name", default="community-1")
    args = p.parse_args()
    harness.run_prototype("p3-diarize", args.name, lambda: run(args))


if __name__ == "__main__":
    main()
