# TTS tool

Turns a `script.json` from `bbb_bench.datasets.generator` into a synthetic multi-speaker recording (Polish, English or mixed) with ground-truth timing. Separate uv project so its PyTorch/CUDA stack does not touch `benchmarks/`. Licenses: [LICENSES.md](LICENSES.md).

Engine: Chatterbox Multilingual (voice cloning). 20 reference voices (10 female, 10 male) are rendered locally by Piper `en_US-libritts-high` and cloned for Polish and English speech. Needs an NVIDIA GPU (about 3 GB of weights, downloaded on first run into `benchmarks/data/tts/`).

## Usage

```bash
cd benchmarks/tools/tts
uv sync
uv run python -m bbb_tts build-refs          # once: reference clips -> benchmarks/data/tts/refs
uv run python -m bbb_tts synth --script <script.json> --out <dir> [--seed N] [--tracks]
uv run pytest && uv run ruff check .
```

`select-voices` re-picks the 20 voices (rewrites `src/bbb_tts/voices.json`); normally not needed.

## Output of `synth`

| File | Content |
|---|---|
| `turns/NNNN-<speaker>.wav` | one turn, 16 kHz mono 16-bit |
| `mixed.wav` | turns in order with 0.2-0.6 s pauses (seeded) |
| `timeline.json` | `{turn, speaker, lang, start_s, end_s}` per turn |
| `reference.rttm` | RTTM for DER scoring |
| `speakers.json` | speaker -> reference voice |
| `track-<speaker>.wav` | with `--tracks`: one speaker's full track on the shared axis |
| `synth_stats.json` | generation time, real-time factor, peak VRAM |

Voices are assigned by a hash of (seed, speaker id), by gender of the speaker name, and are always distinct within one meeting.
