# 0015. Processing pipeline: queue, worker contract and GPU guard

**Status:** Accepted (Stage 2)

## Context

Stage 2 turns a recording into a transcript with speakers and described screenshots on a GPU that the user also uses for other work. Processing takes minutes to hours, must survive a crash or a closed application at any point, and must never run two model processes at once or start while another program is using the GPU.

## Decision

**Stages and persistence.** One job per meeting, with one row per stage in SQLite: `asr`, `diarize`, `merge`, `audio_cleanup`, `vision`, in this order. A stage is `pending`, `running`, `done` or `failed`. The next step is a pure function of those rows. A stage left `running` by a crash goes back to `pending` at the next start and the job continues there, unless it already crashed three times in a row, in which case it is marked `failed`. A failed stage blocks the job until the user retries it; earlier stages are not repeated. Processing starts only on the user's request (or when "Process now" was chosen after a recording), except that a job that was running when the application stopped continues after the restart.

**One stage at a time.** The supervisor takes a lock for the duration of a stage, so a second request gets `Busy` and there is at most one worker, hence one model process. Every stage runs in its own process; when it exits the memory is released (after a stage 285 to 326 MiB stay allocated inside a process, which a fresh process does not inherit).

**Worker contract.** The worker is a Python package (`worker/`) started with `uv run --frozen`. The application writes a manifest (`schemas/job-manifest.schema.json`) from the database: tracks with their segment files in order, screenshots and options. A step reads the manifest and earlier outputs from `<meeting>/work/` and writes its own outputs there, each file written to a temporary name and renamed, so a killed step leaves nothing that looks complete. Progress goes to stdout as one JSON object per line (`progress`, `done`, `error`); the exit code is 0 or 1. Outputs are `asr/<track>.json`, `diarize/<track>.json`, `transcript.json` (`schemas/transcript.schema.json`) and `descriptions.json` (`schemas/screenshot-descriptions.schema.json`). The schemas are checked by the worker before writing and by Rust and Python tests against shared fixtures. All times are milliseconds on the session clock.

**Models.** Steps read models only from the local cache and never use the network (`HF_HUB_OFFLINE`), so processing works offline once the models are downloaded. Downloading is a separate command with progress events. Gated models need the user's Hugging Face token, which is stored in the Windows Credential Manager and handed to the worker through the environment of the download process only. The vision model is served by a `llama-server` that the worker starts and always terminates; on timeout the application kills the whole process tree.

**Merge.** Words from ASR are attributed to the diarization turn with the largest overlap (the nearest turn within 0.5 s, otherwise "unknown"). The microphone track is the user and is not diarized. A track without diarization is one speaker named after its source. Segments of different tracks that overlap in time are all kept and ordered by start time.

**GPU guard.** Before every stage that loads a model the guard reads NVML (utilization, free memory, compute processes). The stage starts when utilization stayed below a limit for a configurable time (defaults: 40 % for 10 s) and enough memory is free (peak measured in Stage 0 plus about 50 %: 3.0 GB for ASR, 3.5 GB for diarization, 9.5 GB for vision). The reason for waiting is shown in the queue. Without NVML readings the job waits, because processing without an NVIDIA GPU is not supported. The decision logic works on plain readings and is tested on recorded traces. The default limit is 40 % rather than 25 % because a recorded trace of the idle desktop of the reference machine hovered at 21 to 26 %.

**Edits.** Corrections and excluded passages are stored in `edits.json`, separate from the worker's `transcript.json`, so running a stage again never loses them. Excluded passages are omitted from every text that leaves the transcript view.

**Audio retention.** The full audio is deleted only when every stage that reads audio has succeeded (`asr` and `diarize` now; Stage 3 adds `speaker_id` and `clips`). The setting is *delete after processing* (default), *keep* or *keep for N days*. Deletion became the default at the user's decision after Stage 2, once processing had been verified end to end; the settings screen says that afterwards the transcript is the only place to correct what was said.

## Consequences

- The queue, the guard and the worker contract are testable without models: state machine, guard traces, merge and schemas run in CI; the real-model tests run locally before a release.
- A worker that hangs is stopped by the step timeout, and its failure is visible and retryable.
- Every stage reads the audio from disk again, which costs time but keeps stages independent and resumable.
- The Python environment (several GB, created with `uv` on first use) lives outside the installation folder.

## Alternatives

- **One long-running worker with models loaded in turn:** simpler start-up but VRAM is not returned and a crash loses the whole run.
- **A message queue or an external job system:** more machinery than one local job per meeting needs.
- **Starting processing automatically after every recording:** conflicts with a shared GPU; the user decides.
