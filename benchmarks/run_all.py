"""Run the full measurement campaign unattended, one step after another.

    uv run python run_all.py [--only p2 p3 ...] [--skip-synth]

Every step runs as a subprocess and writes its own JSON result file under
`results/`. A failing step is logged and the run continues. Logs go to
`data/logs/`. Audio is synthesized first (it is not committed). The expected
locations of models and tools are described in README.md.

Known problem on Windows: when this script is started through `uv run`, the
synthesis step can fail with "No Python at ..." because the speech tool's own
environment does not start from a child process of that interpreter; the same
command works from a plain shell. Run the synthesis from a shell once
(`cd tools/tts && uv run python -m bbb_tts synth --script ... --out ... --seed 1`),
copy `annotation.json` and `reference.txt` next to `mixed.wav`, and start this script
afterwards: finished audio is skipped.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
LOGS = DATA / "logs"
MODELS = DATA / "models"
TTS = ROOT / "tools" / "tts"

SYNTH_SETS = ["pl-03", "pl-08", "pl-16", "en-03", "mixed-04", "pl-04a", "pl-04b"]
ASR_MEETINGS = ["pl-03", "en-03", "mixed-04"]
DIAR_MEETINGS = ["pl-03", "pl-08", "pl-16", "en-03", "mixed-04"]
LLM_CANDIDATES = {
    "qwen3-14b-q4": "Qwen3-14B-Q4_K_M.gguf",
    "mistral-nemo-12b-q4": "Mistral-Nemo-Instruct-2407-Q4_K_M.gguf",
    "gemma-4-12b-q4": "gemma-4-12b-it-Q4_K_M.gguf",
}


def run(label: str, cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> bool:
    LOGS.mkdir(parents=True, exist_ok=True)
    log = LOGS / f"{label}.log"
    print(f"[{time.strftime('%H:%M:%S')}] {label}", flush=True)
    start = time.time()
    with log.open("w", encoding="utf-8") as fh:
        proc = subprocess.run(
            cmd, cwd=cwd, stdout=fh, stderr=subprocess.STDOUT, env={**os.environ, **(env or {})}
        )
    ok = proc.returncode == 0
    print(f"    {'ok' if ok else 'FAILED'} in {time.time() - start:.0f}s -> {log.name}", flush=True)
    return ok


def uv(*args: str) -> list[str]:
    return ["uv", "run", "--quiet", *args]


def step_synth() -> None:
    for name in SYNTH_SETS:
        out = DATA / "audio" / name
        if (out / "mixed.wav").exists():
            continue
        script = DATA / "scripts" / name / "script.json"
        run(
            f"synth-{name}",
            uv(
                "python",
                "-m",
                "bbb_tts",
                "synth",
                "--script",
                str(script),
                "--out",
                str(out),
                "--seed",
                "1",
            ),
            TTS,
        )
        for extra in ("annotation.json", "reference.txt"):
            src, dst = DATA / "scripts" / name / extra, out / extra
            if src.exists() and out.exists():
                dst.write_bytes(src.read_bytes())
        if not (out / "mixed.wav").exists():
            print(f"    no audio for {name}: see the note at the top of this file", flush=True)


def step_p2() -> None:
    meetings = [str(DATA / "audio" / m) for m in ASR_MEETINGS]
    configs = []
    for model in ("large-v3", "turbo"):
        for language in ("auto", "forced"):
            for glossary in (False, True):
                configs.append((model, language, glossary))
    configs.append(("parakeet", "auto", False))
    for model, language, glossary in configs:
        name = f"{model}-{language}-{'gloss' if glossary else 'nogloss'}"
        cmd = uv("--group", "asr", "python", "-m", "bbb_bench.proto.p2_asr", "--model", model)
        cmd += ["--language", language, "--meetings", *meetings]
        if glossary:
            cmd.append("--glossary")
        run(f"p2-{name}", cmd, ROOT)


def step_p3() -> None:
    meetings = [str(DATA / "audio" / m) for m in DIAR_MEETINGS]
    cmd = uv(
        "--group", "diar", "python", "-m", "bbb_bench.proto.p3_diarize", "--meetings", *meetings
    )
    run("p3", cmd, ROOT)


def step_p4() -> None:
    cmd = uv(
        "--group", "diar", "python", "-m", "bbb_bench.proto.p4_speaker_id",
        "--enroll", str(DATA / "audio" / "pl-04a"), "--test", str(DATA / "audio" / "pl-04b"),
    )  # fmt: skip
    run("p4", cmd, ROOT)


def _server_env() -> dict[str, str]:
    return {"BBB_LLAMA_SERVER": str(MODELS / "llama.cpp" / "llama-server.exe")}


def step_p5() -> None:
    meetings = sorted(str(p) for p in (DATA / "extraction").iterdir())
    for name, filename in LLM_CANDIDATES.items():
        model = next(MODELS.rglob(filename), None)
        if model is None:
            print(f"    skipping {name}: {filename} not found", flush=True)
            continue
        cmd = uv("python", "-m", "bbb_bench.proto.p5_llm", "--model", str(model), "--name", name)
        cmd += ["--meetings", *meetings]
        run(f"p5-{name}", cmd, ROOT, _server_env())


def step_p6() -> None:
    model = next(MODELS.rglob("Qwen3VL-8B-Instruct-Q4_K_M.gguf"), None)
    proj = next(MODELS.rglob("mmproj-Qwen3VL-8B-Instruct-F16.gguf"), None)
    if not model or not proj:
        print("    skipping p6: vision model files not found", flush=True)
        return
    run(
        "slides",
        uv(
            "--group",
            "vision",
            "python",
            "-m",
            "bbb_bench.datasets.slides",
            "--out",
            str(DATA / "slides"),
        ),
        ROOT,
    )
    cmd = uv(
        "python", "-m", "bbb_bench.proto.p6_vision", "--model", str(model), "--mmproj", str(proj)
    )
    cmd += ["--slides", str(DATA / "slides")]
    run("p6", cmd, ROOT, _server_env())


def step_p8() -> None:
    run("p8", uv("python", "-m", "bbb_bench.proto.p8_env"), ROOT)


STEPS = {
    "synth": step_synth,
    "p2": step_p2,
    "p3": step_p3,
    "p4": step_p4,
    "p5": step_p5,
    "p6": step_p6,
    "p8": step_p8,
}


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only", nargs="+", choices=list(STEPS), default=list(STEPS))
    args = p.parse_args()
    run("make-sets", uv("python", "-m", "bbb_bench.datasets.make_sets", "--root", str(DATA)), ROOT)
    for name in args.only:
        STEPS[name]()
    print("done", flush=True)


if __name__ == "__main__":
    sys.exit(main())
