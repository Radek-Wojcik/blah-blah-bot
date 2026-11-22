# Benchmarks

Harness, metric code and prototype runners used to measure the risky components before the application is written. Thresholds are defined in [docs/TESTING.md](../docs/TESTING.md) and [docs/RISKS-AND-POC.md](../docs/RISKS-AND-POC.md).

## Layout

| Path | Content |
|---|---|
| `src/bbb_bench/metrics/` | WER, term recall, translation rate, DER, extraction scoring |
| `tests/` | Unit tests with hand-computed examples (CPU only, no models) |
| `results/` | JSON result files written by the prototypes |
| `data/` | Generated audio and downloaded models. Not tracked. |

## Setup and tests

```bash
cd benchmarks
uv run pytest
uv run ruff check .
```

## Text normalization

WER and term recall compare normalized text: lowercase, punctuation removed, Polish letters kept, thousand separators in digit groups joined. Spelled-out numbers are not converted, so test scripts write numbers as digits.

## Regression report

`src/bbb_bench/regression.py` turns the suite logs and the result files into the Stage 8 report (`stage8-regression.json` and `.md`): per-threshold verdicts against the targets of docs/TESTING.md, deltas against the recorded results in `results/`, the resilience matrix and a go/no-go recommendation. It is run by `scripts/regression.ps1`; see [docs/REGRESSION.md](../docs/REGRESSION.md), which also describes the ten hour end-to-end run. Set `BBB_RESULTS_DIR` to send the result files of a prototype somewhere other than `results/`.
