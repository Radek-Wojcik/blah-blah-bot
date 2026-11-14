"""Command line: synth (script.json -> meeting audio), select-voices, build-refs."""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from . import audio, paths, timeline, voices
from .rttm import to_rttm

MAX_ATTEMPTS = 4


def load_script(path: Path) -> dict:
    script = json.loads(path.read_text("utf-8"))
    for key in ("speakers", "turns"):
        if key not in script:
            raise ValueError(f"script is missing '{key}'")
    for i, turn in enumerate(script["turns"]):
        if not {"speaker", "lang", "text"} <= turn.keys():
            raise ValueError(f"turn {i} needs speaker, lang and text")
    return script


def _write_json(path: Path, data: object) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", "utf-8")


def synth(args: argparse.Namespace) -> None:
    from .engine import ChatterboxEngine, plausible

    script = load_script(args.script)
    turns = script["turns"]
    catalog = voices.load_catalog()
    by_id = {v["id"]: v for v in catalog}
    speakers = sorted({t["speaker"] for t in turns} | set(script["speakers"]))
    mapping = voices.assign_voices(speakers, catalog, args.seed)

    out: Path = args.out
    (out / "turns").mkdir(parents=True, exist_ok=True)
    engine = ChatterboxEngine(paths.refs_dir(), device=args.device)

    waves, regenerated = [], 0
    started = time.perf_counter()
    for i, turn in enumerate(turns):
        wave = None
        for attempt in range(MAX_ATTEMPTS):
            seed = args.seed * 100003 + i * 31 + attempt
            raw = engine.synth(turn["text"], turn["lang"], mapping[turn["speaker"]], seed)
            wave = audio.resample(raw, engine.sample_rate, audio.OUT_RATE)
            wave = audio.trim_silence(wave, audio.OUT_RATE)
            if plausible(turn["text"], len(wave) / audio.OUT_RATE):
                break
            regenerated += 1
        wave = audio.match_level(wave)
        waves.append(wave)
        audio.write_wav(out / "turns" / f"{i:04d}-{turn['speaker']}.wav", wave)
        print(f"turn {i + 1}/{len(turns)} {turn['speaker']} {len(wave) / audio.OUT_RATE:.1f}s")
    gen_s = time.perf_counter() - started

    pauses = timeline.plan_pauses(len(turns), args.seed, audio.OUT_RATE)
    segments = timeline.layout(turns, [len(w) for w in waves], pauses)
    mixed = timeline.mix(segments, waves)
    audio.write_wav(out / "mixed.wav", mixed)
    tl = timeline.to_timeline(segments, audio.OUT_RATE)
    _write_json(out / "timeline.json", tl)
    (out / "reference.rttm").write_text(to_rttm(tl, out.resolve().name), "utf-8")
    _write_json(
        out / "speakers.json",
        {
            s: {
                "voice": mapping[s],
                "gender": by_id[mapping[s]]["gender"],
                "piper_speaker": by_id[mapping[s]]["piper_speaker"],
                "reference_clip": f"{mapping[s]}.wav",
            }
            for s in speakers
        },
    )
    if args.tracks:
        for speaker, track in timeline.speaker_tracks(segments, waves, len(mixed)).items():
            audio.write_wav(out / f"track-{speaker}.wav", track)

    speech_s = sum(len(w) for w in waves) / audio.OUT_RATE
    stats = {
        "engine": "chatterbox-multilingual",
        "seed": args.seed,
        "turns": len(turns),
        "audio_s": round(len(mixed) / audio.OUT_RATE, 2),
        "speech_s": round(speech_s, 2),
        "generation_s": round(gen_s, 2),
        "rtf": round(gen_s / speech_s, 3) if speech_s else None,
        "regenerated_turns": regenerated,
        "peak_vram_mb": round(engine.peak_vram_mb(), 1),
    }
    _write_json(out / "synth_stats.json", stats)
    print(json.dumps(stats))


def select_voices(args: argparse.Namespace) -> None:
    from . import refs

    for v in refs.select_voices(args.candidates, args.seed):
        print(v)


def build_refs(args: argparse.Namespace) -> None:
    from . import refs

    for path in refs.build_refs(voices.load_catalog()):
        print(path)


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(prog="bbb_tts", description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)

    s = sub.add_parser("synth", help="render a script.json into a multi-speaker recording")
    s.add_argument("--script", type=Path, required=True)
    s.add_argument("--out", type=Path, required=True)
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--tracks", action="store_true", help="also write track-<speaker>.wav")
    s.add_argument("--device", default="cuda")
    s.set_defaults(func=synth)

    v = sub.add_parser("select-voices", help="pick reference voices (slow, rewrites voices.json)")
    v.add_argument("--candidates", type=int, default=400)
    v.add_argument("--seed", type=int, default=20260101)
    v.set_defaults(func=select_voices)

    r = sub.add_parser("build-refs", help="render reference clips into benchmarks/data/tts/refs")
    r.set_defaults(func=build_refs)

    args = p.parse_args(argv)
    args.func(args)
