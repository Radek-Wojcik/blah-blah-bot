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
