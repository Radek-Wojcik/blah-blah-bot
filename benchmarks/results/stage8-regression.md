# Stage 8 regression report

Generated: 2026-11-23T11:05:32+00:00. CPU part: run. GPU part: run.

**Recommendation: NO-GO**

Advisory only: the decision is the project owner's. `decisions` lists targets that are missed and need an explicit acceptance (see --accepted).

Blocking:

- time-10h: 10-hour meeting, full pipeline regressed against its baseline

Needs a decision:

- rec-dropped: Dropped buffers in a 10-hour run misses the target <= 0.0 (baseline value)
- term-recall: Term recall, mixed set misses the target >= 0.85 (fresh value)
- translation-rate: Translation rate (worst meeting) misses the target <= 0.01 (fresh value)
- der-2-4: DER, 2-4 speakers (worst meeting) misses the target <= 0.15 (fresh value)

Open:

- manual step: real-app
- rec-cpu: CPU while recording three tracks (share of all CPUs): baseline only, not re-measured
- rec-memory: Memory (peak working set): baseline only, not re-measured
- rec-dropped: Dropped buffers in a 10-hour run: baseline only, not re-measured
- rec-alignment: Track alignment error: baseline only, not re-measured
- resilience: Audio device unplugged (manual)
- resilience: Disk nearly full (manual)
- resilience: No network at briefing (pending merge)
- resilience: No network at publishing (pending merge)
- resilience: Concurrent push to the cycle repository (pending merge)
- resilience: Extension service worker restarted (pending merge)

## Suites

| Suite | Status | Passed | Failed | Skipped | Seconds |
|---|---|---|---|---|---|
| cargo fmt --check | passed | 0 | 0 | 0 | 0.6 |
| cargo clippy | passed | 0 | 0 | 0 | 1 |
| cargo test | passed | 488 | 0 | 10 | 17.5 |
| worker pytest | passed | 217 | 0 | 0 | 5.3 |
| benchmarks pytest | passed | 154 | 0 | 1 | 13.4 |
| ui typecheck | passed | 0 | 0 | 0 | 2.3 |
| ui lint | passed | 0 | 0 | 0 | 4.6 |
| ui format:check | passed | 0 | 0 | 0 | 1.1 |
| ui unit tests | passed | 191 | 0 | 0 | 4.5 |
| ui mock end-to-end tests | passed | 76 | 0 | 0 | 112.8 |

## Test kinds

| Kind | Status | Passed | Failed | Ignored or skipped |
|---|---|---|---|---|
| Formatting, lint and type checks | passed | 5 | 0 | 0 |
| Unit tests (Rust, worker, benchmarks, UI) | passed | 956 | 0 | 2 |
| Contract tests (schemas and shared fixtures) | passed | 73 | 0 | 0 |
| Integration tests (Rust test binaries without models) | passed | 21 | 0 | 9 |
| End-to-end tests of the UI on the mock backend | passed | 76 | 0 | 0 |

## Thresholds and baselines

Values from a fresh run are marked `fresh`; otherwise the recorded baseline is shown (`baseline`). A quality metric may not drop by more than 2 points against its baseline.

| Measure | Target | Now | Source | Baseline | Delta | Verdict | Baseline check |
|---|---|---|---|---|---|---|---|
| Recorder: CPU while recording three tracks (share of all CPUs) | <= 3 % | 0.01741 % | baseline | 0.01741 % | - | pass | baseline only, not re-measured |
| Recorder: Memory (peak working set) | <= 200 MB | 10.15 MB | baseline | 10.15 MB | - | pass | baseline only, not re-measured |
| Recorder: Dropped buffers in a 10-hour run | <= 0 | 194 | baseline | 194 | - | FAIL | baseline only, not re-measured |
| Recorder: Audio lost when the process is killed | <= 60 s | - | test | - | - | pass | no baseline |
| Recorder: Track alignment error | <= 50 ms | 16.18 ms | baseline | 16.18 ms | - | pass | baseline only, not re-measured |
| Transcription: WER, Polish scripted set | <= 15.0 % | 8.9 % | fresh | 8.9 % | +0.0 pp | pass | ok |
| Transcription: WER, mixed set | <= 20.0 % | 16.4 % | fresh | 16.4 % | +0.0 pp | pass | ok |
| Transcription: WER, English set (no target, reported) | none | 14.3 % | fresh | 14.3 % | +0.0 pp | reported | ok |
| Transcription: Term recall, mixed set | >= 85.0 % | 35.1 % | fresh | 35.1 % | +0.0 pp | FAIL | ok |
| Transcription: Translation rate (worst meeting) | <= 1.0 % | 5.0 % | fresh | 5.0 % | +0.0 pp | FAIL | ok |
| Speakers: DER, 2-4 speakers (worst meeting) | <= 15.0 % | 29.4 % | fresh | 29.4 % | +0.0 pp | FAIL | ok |
| Speakers: DER, 6-10 speakers (worst meeting) | <= 30.0 % | 25.7 % | fresh | 25.7 % | +0.0 pp | pass | ok |
| Speakers: DER, 15+ speakers (no target, reported) | none | 27.4 % | fresh | 27.4 % | +0.0 pp | reported | ok |
| Speakers: Speaker accuracy for enrolled voices | >= 90.0 % | 100.0 % | fresh | 100.0 % | +0.0 pp | pass | ok |
| Speakers: False accept rate | <= 2.0 % | 0.0 % | fresh | 0.0 % | +0.0 pp | pass | ok |
| Extraction: Schema validity (up to two retries) | >= 100.0 % | 100.0 % | fresh | 100.0 % | +0.0 pp | pass | ok |
| Extraction: Task recall | >= 80.0 % | 86.0 % | fresh | 86.0 % | +0.0 pp | pass | ok |
| Extraction: Task precision | >= 80.0 % | 89.3 % | fresh | 89.3 % | +0.0 pp | pass | ok |
| Extraction: Assignee accuracy | >= 85.0 % | 92.8 % | fresh | 92.8 % | +0.0 pp | pass | ok |
| Extraction: Due-date accuracy | >= 85.0 % | 92.2 % | fresh | 92.2 % | +0.0 pp | pass | ok |
| Extraction: Quote validity | >= 95.0 % | 100.0 % | fresh | 100.0 % | +0.0 pp | pass | ok |
| Processing: 2-hour meeting, full pipeline | <= 45 min | 18 min | fresh | 16.4 min | +1.6 min | pass | ok |
| Processing: 10-hour meeting, full pipeline | <= 4 h | 1.858 h | fresh | 0.9711 h | +0.8869 h | pass | REGRESSED |
| Processing: Peak VRAM in any stage | <= 11500 MiB | 9813 MiB | fresh | 9743 MiB | +70 MiB | pass | ok |

## GPU and manual steps

| Step | Status | Seconds |
|---|---|---|
| Worker pytest with real models (BBB_MINI_DIR, slides, vision, llama-server) | passed (restored from log: passed in the full GPU run, not re-run) | 135.7 |
| Supervisor pipeline: end to end, worker killed per stage, GPU occupied | passed (restored from log: passed in the full GPU run, not re-run) | 131.2 |
| Speaker recognition accuracy over seven meetings | passed (restored from log: passed in the full GPU run, not re-run) | 520.7 |
| Whole pipeline with extract and reduce on pl-03 and en-03 | passed (passed on a re-run (-Only); the first attempt in the full run failed after 460.7 s (exit code 101) because a concurrent cleanup removed its scratch folder) | 907.6 |
| ASR turbo, auto language, on the Polish, English and mixed meetings | passed (restored from log: passed in the full GPU run, not re-run) | 104.9 |
| Diarization (community-1) on 2-4, 8 and 16 speaker meetings | passed (restored from log: passed in the full GPU run, not re-run) | 334.9 |
| Extraction quality on the labeled meetings | passed (restored from log: passed in the full GPU run, not re-run) | 971.2 |
| Extraction timing on two hours of transcript | passed (restored from log: passed in the full GPU run, not re-run) | 706.5 |
| Worker steps on a two hour recording: time and peak VRAM | passed (restored from log: passed in the full GPU run, not re-run) | 371.8 |
| Real application end to end (`npm run test:real`, user at the machine) | manual | - |
| Ten hour end-to-end run (`scripts/ten-hour-run.ps1`) | passed | 6689.0 |

## Resilience (docs/TESTING.md section 6)

| Fault | Expected | Coverage | Status | Evidence |
|---|---|---|---|---|
| Recorder process killed during recording | Meeting recovered, loss within the threshold | automated | passed | `killed_process_loses_at_most_one_segment` (passed) |
| Captured application closed during recording | Its track ends, others continue | automated | passed | `running_then_stopped_by_itself_is_source_closed` (passed); `removing_a_source_ends_only_that_track` (passed) |
| Audio device unplugged | Track ends cleanly, notification shown | manual | manual | Unplug a USB microphone during a recording and check the notification. |
| Disk nearly full | Recording stops with a clear message, existing segments intact | manual | manual | Record onto a small virtual disk (diskpart, administrator) until it fills. |
| Worker killed in each stage | Retry resumes from that stage | automated | passed | `failure_blocks_the_job_and_retry_resumes_from_that_stage` (passed); `recover_resumes_an_interrupted_stage` (passed); GPU step `pipeline-models` (passed) |
| GPU occupied by another process | Job waits; starts when the GPU is free | automated | passed | `busy_gpu_never_starts` (passed); `busy_gpu_keeps_the_job_waiting_until_it_is_free` (passed); GPU step `pipeline-models` (passed) |
| Model server returns invalid JSON | Retries, then a flagged window | automated | passed | `test_invalid_answer_is_retried_with_the_error_in_the_prompt` (passed); `test_window_is_skipped_after_the_retries_and_the_stage_continues` (passed) |
| No network at briefing | Cached state with its date | pending merge | pending merge | Stage 6 (briefing) is not merged yet. |
| No network at publishing | Outbox, automatic retry | pending merge | pending merge | Stage 5 (publishing and outbox) is not merged yet. |
| Concurrent push to the cycle repository | Rebase and retry succeeds | pending merge | pending merge | Publishing tests of Stage 5 (`resilience`); pull with rebase is covered in `cycle-store`. |
| Extension service worker restarted | Stream reconnects; gap recorded in the journal | pending merge | pending merge | Stage 6 (browser extension) is not merged yet. |

## Temp hygiene

0 directories left behind.

