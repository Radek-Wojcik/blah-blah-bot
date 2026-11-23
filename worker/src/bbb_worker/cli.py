"""Command line entry point: `python -m bbb_worker <step> ...`.

asr | diarize | merge | vision | extract | reduce   --manifest FILE --meeting-dir DIR
ingest  --manifest FILE --meeting-dir DIR  (or --input-path FILE --meeting-dir DIR [--mode MODE])
enroll  --manifest FILE [--meeting-dir DIR] (or --input-path FILE --clip-out-path FILE --output-path FILE)
models --ensure KEY[,KEY...]     (needs network; reads HF_TOKEN from the environment)
models --status                  (prints which models are cached, as JSON)
"""

from __future__ import annotations

import argparse
import importlib
import os
import sys
import traceback
from pathlib import Path

from . import enroll, ingest, models, protocol

PIPELINE_STEPS = ("asr", "diarize", "merge", "vision", "extract", "reduce")
STEPS = (*PIPELINE_STEPS, "ingest", "enroll")


def _run_step(step: str, manifest_path: Path, meeting_dir: Path) -> None:
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    protocol.progress(step, 0.0, "started")
    if step == "ingest":
        doc = ingest.load_ingest_manifest(manifest_path, meeting_dir)
        ingest.run(doc, meeting_dir=meeting_dir)
        return
    if step == "enroll":
        doc = enroll.load_enroll_manifest(manifest_path, meeting_dir)
        enroll.run(doc, meeting_dir=meeting_dir)
        return

    from . import manifest as manifest_mod

    manifest = manifest_mod.load(manifest_path, meeting_dir)
    manifest.work_dir.mkdir(parents=True, exist_ok=True)
    # Models are only read from the local cache here, so no step needs the network.
    module = importlib.import_module(f"bbb_worker.steps.{step}")
    module.run(manifest)


def _run_ingest(args: argparse.Namespace) -> None:
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    protocol.progress("ingest", 0.0, "started")
    meeting_dir = args.meeting_dir or Path(".")
    if args.manifest is not None:
        doc = ingest.load_ingest_manifest(args.manifest, meeting_dir)
        if args.input_path is not None or args.mode is not None:
            opts = doc.setdefault("options", {}).setdefault("ingest", {})
            if args.input_path is not None:
                opts["input_path"] = str(args.input_path)
            if args.mode is not None:
                opts["mode"] = args.mode
        ingest.run(doc, meeting_dir=meeting_dir)
        return
    if args.input_path is None:
        raise ValueError("ingest requires either --manifest or --input-path")
    ingest.ingest_media(
        args.input_path,
        meeting_dir,
        mode=args.mode or "audio_and_slides",
    )


def _run_enroll(args: argparse.Namespace) -> None:
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    protocol.progress("enroll", 0.0, "started")
    if args.manifest is not None:
        doc = enroll.load_enroll_manifest(args.manifest, args.meeting_dir)
        if any(v is not None for v in (args.input_path, args.clip_out_path, args.output_path)):
            opts = doc.setdefault("options", {}).setdefault("enroll", {})
            if args.input_path is not None:
                opts["input_path"] = str(args.input_path)
            if args.clip_out_path is not None:
                opts["clip_out_path"] = str(args.clip_out_path)
            if args.output_path is not None:
                opts["output_path"] = str(args.output_path)
        enroll.run(doc, meeting_dir=args.meeting_dir)
        return
    if args.input_path is None:
        raise ValueError("enroll requires either --manifest or --input-path")
    meeting_dir = args.meeting_dir or args.input_path.parent
    clip_out = args.clip_out_path or (meeting_dir / "work" / "enroll-clip.wav")
    output = args.output_path or (meeting_dir / "work" / "enroll.json")
    enroll.enroll_voice(args.input_path, clip_out, output)


def _run_models(args: argparse.Namespace) -> None:
    if args.status:
        protocol.emit("models", cached={k: models.is_cached(k) for k in models.MODELS})
        return
    token = os.environ.get("HF_TOKEN") or None
    keys = [k for k in args.ensure.split(",") if k]
    for key in keys:
        if key not in models.MODELS:
            raise SystemExit(f"unknown model: {key}")
    for key in keys:
        if not models.is_cached(key):
            models.download(key, token)
    protocol.progress("models", 1.0, "ready")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="bbb_worker")
    sub = parser.add_subparsers(dest="step", required=True)
    for step in PIPELINE_STEPS:
        p = sub.add_parser(step)
        p.add_argument("--manifest", type=Path, required=True)
        p.add_argument("--meeting-dir", type=Path, required=True)

    p_ingest = sub.add_parser("ingest")
    p_ingest.add_argument("--manifest", type=Path, default=None)
    p_ingest.add_argument("--meeting-dir", type=Path, default=None)
    p_ingest.add_argument("--input-path", "--input", dest="input_path", type=Path, default=None)
    p_ingest.add_argument("--mode", choices=("audio_only", "audio_and_slides"), default=None)

    p_enroll = sub.add_parser("enroll")
    p_enroll.add_argument("--manifest", type=Path, default=None)
    p_enroll.add_argument("--meeting-dir", type=Path, default=None)
    p_enroll.add_argument("--input-path", "--input", dest="input_path", type=Path, default=None)
    p_enroll.add_argument(
        "--clip-out-path", "--clip-out", dest="clip_out_path", type=Path, default=None
    )
    p_enroll.add_argument("--output-path", "--output", dest="output_path", type=Path, default=None)

    m = sub.add_parser("models")
    m.add_argument("--ensure", default="")
    m.add_argument("--status", action="store_true")
    args = parser.parse_args(argv)

    # Keep stdout clean for protocol events: library output goes to stderr.
    # `protocol` keeps its own reference to the real stdout.
    sys.stdout = sys.stderr
    try:
        if args.step == "models":
            _run_models(args)
        elif args.step == "ingest":
            _run_ingest(args)
            protocol.done("ingest")
        elif args.step == "enroll":
            _run_enroll(args)
            protocol.done("enroll")
        else:
            _run_step(args.step, args.manifest, args.meeting_dir)
            protocol.done(args.step)
        return 0
    except Exception as e:  # noqa: BLE001 - every failure must reach the application
        traceback.print_exc(file=sys.stderr)
        protocol.error(args.step, f"{type(e).__name__}: {e}")
        return 1
