# Regression suite and ten hour run (Stage 8)

The regression suite decides whether a release candidate may go out. It has a CPU part that runs on any machine and a GPU part that needs the reference machine, and it ends with one report: per-threshold verdicts, deltas against the recorded baselines, the resilience matrix and a go/no-go recommendation. The thresholds are those of [TESTING.md](TESTING.md) section 5, the resilience cases those of section 6.

## Commands

Every script is started with `powershell -ExecutionPolicy Bypass -File scripts\<name>.ps1 ...`.

| What | Command |
|---|---|
| CPU part: every CPU suite once, then the report | `scripts\regression.ps1` (first time on a fresh checkout: `scripts\regression.ps1 -Prepare`) |
| Plan only, nothing executed | `scripts\regression.ps1 -DryRun` and `scripts\regression.ps1 -Mode gpu -DryRun` |
| Only some CPU suites | `-Skip ui` or `-Suite worker,benchmarks` (suites: `rust`, `worker`, `benchmarks`, `ui`) |
| GPU part (opt-in) | `scripts\regression.ps1 -Mode gpu -AudioDir ... -MiniDir ... -SlidesDir ... -VisionDir ... -ExtractionDir ... -LongAudio ... -LlamaServer ... -Model ...` |
| Only some GPU steps | `-Only asr,diarization` (the rows are merged into `gpu-steps.json`: the other steps keep their last result) |
| Rebuild the report from what exists | `scripts\regression.ps1 -ReportOnly` |
| Write the final report into `benchmarks\results\` | add `-WriteResults` (default: `target\regression\report\`) |
| Accept a known miss with a reason | `-Accepted <file.json>`, an object such as `{"term-recall": "reason"}` |
| Ten hour end-to-end run (plan only without `-Run`) | `scripts\ten-hour-run.ps1 -AudioDir ... -Model ... -LlamaServer ... [-Run]` |

## CPU part

`scripts\regression.ps1` calls `scripts\test-all.ps1 -LogDir target\regression\logs`, which runs each suite once and keeps one log per step, a JUnit file per pytest run and `steps.json`. The report generator sorts the results into kinds:

| Kind | Where it comes from |
|---|---|
| Static checks | `cargo fmt --check`, `cargo clippy`, `tsc`, ESLint, Prettier |
| Unit tests | `cargo test` unit tests and doc tests, worker and benchmarks pytest, Vitest |
| Contract tests | Tests whose binary or name contains `contract`, `schema` or `fixture` (Rust test lines, pytest cases): the JSON Schema fixtures checked from both sides |
| Integration tests | Rust test binaries in `crates/*/tests` that need no models (recorder with tones, queue with the fake worker) |
| End-to-end on the mock backend | Playwright `--project=mock` |

Tests marked `#[ignore]` or `models` belong to the GPU part and show up as ignored.

## GPU part

Opt-in, never started by the CPU part, never scheduled by a script. The steps run one after the other, each once, each with its own log and time limit (`-StepTimeoutMinutes`, default 120; a step over the limit has its own process tree stopped and fails). Before **every** step `scripts\gpu-preflight.ps1` must pass: the first GPU holds less than 2 GB, `nvidia-smi` lists no compute process, and no `llama-server`, `llama-cli`, `python` or `pythonw` process is running. If it refuses, the step is reported as skipped with the reason; the script never stops other people's processes. A step whose input parameter is missing is skipped as well.

| Step id | What | Input |
|---|---|---|
| `worker-models` | Worker pytest with real models | `-MiniDir -SlidesDir -VisionDir -LlamaServer` |
| `pipeline-models` | Supervisor pipeline: end to end, worker killed per stage, GPU occupied | same |
| `speakers-models` | Speaker accuracy and false accept rate over seven meetings | `-AudioDir` |
| `stage4-models` | Whole pipeline with extract and reduce on `pl-03` and `en-03` | `-AudioDir -Model -LlamaServer` |
| `asr` | Whisper turbo, language auto, on `pl-03`, `en-03`, `mixed-04` | `-AudioDir` |
| `diarization` | community-1 on `pl-03`, `pl-08`, `pl-16`, `en-03`, `mixed-04` | `-AudioDir` |
| `extraction` | Extraction quality on the labeled meetings | `-ExtractionDir -Model -LlamaServer` |
| `extraction-2h` | Extraction timing on two hours of transcript | same |
| `pipeline-2h` | Worker steps on a two hour recording: time and peak VRAM | `-LongAudio -SlidesDir -VisionDir -LlamaServer` |

Fresh measurements are written to `target\regression\gpu-results\` under the file names of the recorded ones (`BBB_RESULTS_DIR`, honoured by `bbb_bench.harness`), so the recorded baselines in `benchmarks\results\` are never overwritten. The real application test (`npm run test:real`, which needs the user at the machine because WebView2 crashes in remote sessions) and the ten hour run are listed in the report as manual steps. The worker environment must exist in the checkout (`-Prepare` creates it; it comes from the uv cache when the lock file is the same as elsewhere).

## Report

`benchmarks\src\bbb_bench\regression.py` (run by the script, or `cd benchmarks; uv run python -m bbb_bench.regression --logs ... --gpu-results ... --out ...`) writes `stage8-regression.json` and `stage8-regression.md`.

- **Thresholds.** One row per target of TESTING section 5 (recorder, transcription, speakers, extraction, processing time and VRAM). The value is the fresh one when a GPU run produced it, otherwise the recorded baseline (source `baseline`, check "baseline only, not re-measured"). Where several meetings are measured the worst one counts (translation rate, DER). The target "audio lost when the process is killed" has no number in a result file; it is proven by the recorder kill test.
- **Baseline check.** A fresh quality value may not be worse than its baseline by more than two percentage points (`RATE_TOLERANCE`). Times, memory and counts may not be worse by more than 25 % (`PERF_TOLERANCE`). Baselines are the recorded results: Stage 0 (`p2-asr`, `p3-diarize`), Stage 1 for the recorder (`stage1-soak-10h`), Stage 2 for the pipeline time (`stage2-pipeline-2h`), Stage 3 (`stage3-speaker-recognition`) and Stage 4 (`stage4-extraction`, `stage4-long`) for what was first measured there. The ten hour baseline is the recorded ten hour run of 1.858 hours (`stage8-e2e-10h.json`; re-baselined from the Stage 0 estimate of 0.97 hours, see LIMITATIONS).
- **Resilience matrix.** The eleven faults of TESTING section 6, each mapped to automated tests (with their result from the log), a GPU step, a manual procedure (device unplugged, disk nearly full) or "pending merge" (Stages 5 and 6: outbox, briefing, concurrent push, extension restart).
- **Recommendation.** `recommendation.verdict` is one of:
  - `no-go`: a suite, a GPU step or an automated resilience case failed, the hygiene count is above 0, a metric regressed, or a target is missed without an accepted reason (listed under `decisions`);
  - `incomplete`: nothing failed but parts were not measured (listed under `open`: GPU steps not run, baseline-only values, manual and pending rows);
  - `go`: everything passed, was measured, and nothing is open.

  It is advice. A missed target needs an explicit decision by the project owner; accepted misses are passed with `-Accepted` and are listed under `accepted`. The recorded results already miss four targets (term recall on the mixed set, translation rate, DER for 2-4 speakers, dropped buffers of the microphone track), so the verdict is `no-go` until they are accepted or fixed.

## Ten hour end-to-end run

`scripts\ten-hour-run.ps1` builds a ten hour meeting from the synthetic reference meetings (repeated, cut into 60 s segments like the recorder writes them) and processes it with the real worker, real models, a managed `llama-server` and a cycle, through the supervisor (`crates\queue\tests\long_e2e_models.rs`, ignored by default). Only synthetic audio is used. The recorder is not run again: its ten hour soak is `benchmarks\results\stage1-soak-10h.json`.

Without `-Run` the script validates its inputs, the free disk space (1.5 GB per hour of audio, at least 5 GB) and the worker environment, prints the plan and exits; it does not touch the GPU. With `-Run` it

1. requires `scripts\gpu-preflight.ps1` to pass;
2. builds the test, then runs it with `TEMP` and `TMP` in `target\test-tmp` and the work folder under `target\e2e-tmp`, with a time limit (`-TimeoutMinutes`, default 0.4 h per hour of audio plus 2.5 h);
3. samples the machine every `-SampleSeconds` (60) into `target\ten-hour-run\samples.csv` (GPU memory and load, free disk, work folder size, memory of the worker and llama-server processes); the test output goes to `target\ten-hour-run\run.log`;
4. removes the scratch files in a `finally` block, runs `scripts\check-temp-hygiene.ps1` (a count above 0 fails the run) and writes `target\regression\gpu-results\stage8-e2e-10h.json`: seconds and peak VRAM per stage, draft validity, items, GPU waits, the sampler summary and the verdicts against the targets (4 hours, 11.5 GB). `-ReportOnly` of the regression script picks the file up.

`-Hours` below 10 is allowed for a shorter run. The result then says `full_length: false`, the report lists the ten hour item as partial, and the ten hour target is not claimed. The expected duration is about 1.5 hours for the extraction alone (Stage 4 measurement) plus transcription and diarization (Stage 0 estimate: about 35 minutes for ten hours); the target is 4 hours in total.

## Hygiene

All scripts follow the conventions of `scripts\test-all.ps1`: `TEMP` and `TMP` point to `target\test-tmp` for the processes they start, a `finally` block runs `scripts\clean-test-tmp.ps1`, and `scripts\check-temp-hygiene.ps1` runs last. No script registers a scheduled task or a service.
