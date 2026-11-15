"""Command line entry point: `python -m bbb_worker <step> ...`.

asr | diarize | merge | vision | extract | reduce   --manifest FILE --meeting-dir DIR
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

from . import models, protocol

STEPS = ("asr", "diarize", "merge", "vision", "extract", "reduce")


def _run_step(step: str, manifest_path: Path, meeting_dir: Path) -> None:
    from . import manifest as manifest_mod

    manifest = manifest_mod.load(manifest_path, meeting_dir)
    manifest.work_dir.mkdir(parents=True, exist_ok=True)
    # Models are only read from the local cache here, so no step needs the network.
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    module = importlib.import_module(f"bbb_worker.steps.{step}")
    protocol.progress(step, 0.0, "started")
    module.run(manifest)


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
    for step in STEPS:
        p = sub.add_parser(step)
        p.add_argument("--manifest", type=Path, required=True)
        p.add_argument("--meeting-dir", type=Path, required=True)
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
        else:
            _run_step(args.step, args.manifest, args.meeting_dir)
            protocol.done(args.step)
        return 0
    except Exception as e:  # noqa: BLE001 - every failure must reach the application
        traceback.print_exc(file=sys.stderr)
        protocol.error(args.step, f"{type(e).__name__}: {e}")
        return 1
