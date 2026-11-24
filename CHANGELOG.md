# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Draft notes for 1.0.0. Nothing here is released yet.

### Added

- Windows installer: a per-user NSIS installer that needs no administrator rights. It contains the application, the processing worker, the schemas, the license notices and a pinned `uv`; model files are never part of it. A release workflow builds it on a version tag only, writes `SHA256SUMS.txt` and creates a draft release. The installer is not signed, so Windows SmartScreen shows a warning ([docs/INSTALL.md](docs/INSTALL.md), [ADR 0024](docs/decisions/0024-installer-signing.md), status Proposed).
- First-run setup wizard: welcome, processing environment, speech models and Hugging Face token, `llama-server`, language model, screenshot analysis (download or skip), a summary with a Fix button for every open item, and a first cycle. Every step can be skipped. A readiness banner appears while something that processing needs is missing, and **Run setup again** in Settings reopens the wizard.
- Setup readiness check in the application: processing is refused up front with a stable `setup_*` code and a message that says how to fix it, instead of failing inside a job. Paths entered in the settings are checked before they are saved.
- Option to skip screenshot analysis: with **Describe screenshots** off, the vision stage is skipped and screenshots are kept but not described.
- Regression runner (`scripts/regression.ps1`) with a CPU part and an opt-in GPU part, a report with per-threshold verdicts and a go/no-go recommendation, and a ten hour end-to-end run script ([docs/REGRESSION.md](docs/REGRESSION.md)).
- Requirements traceability matrix for all 52 requirements, with a script that checks its references ([docs/TRACEABILITY.md](docs/TRACEABILITY.md)).
- Documentation: user guide, troubleshooting by error code, limitations, install guide with the clean-machine test procedure, security review, dependency audit, contributor test rules.
- `scripts/test-all.ps1` and helpers that keep every test's temporary files inside the repository and check that none are left in the system temp folder.
- Read-only dependency audit script, `deny.toml` and a non-blocking audit job in CI.
- `NOTICE` completed with every dependency and downloadable model.

### Changed

- Missing `llama-server` and missing or incomplete models now produce messages that say what is missing and how to fix it.
- Screenshot GIFs are assembled with Pillow from a `uv` environment instead of the system Python.
- Release notes of a version are extracted from this changelog by the release workflow, and a check keeps the version numbers consistent.

### Fixed

- Region screenshot (Ctrl+Alt+S) showed a black screen over every monitor. The desktop was captured correctly; the cause was in the display path. The content security policy did not allow the page to reach the IPC protocol, so Tauri fell back to the postMessage interface, which returns binary results as an array of numbers; the overlay built its image from that array as if it were bytes, the PNG could not be decoded and the black page background was all that was left. The policy now allows the IPC protocol, binary results are converted correctly in both modes, and the frozen desktop is encoded with fast compression off the main thread.
- The overlay no longer stays as a black sheet when something goes wrong: a capture that is all black, cannot be decoded or does not arrive within 20 seconds is reported in a message and the overlay closes itself. Escape, a click and a right click close it in every state.
- The overlay window is read back after it is placed over the monitors and placed again when the system changed its position or size.
- The regression report keeps the step summary of partial runs and removes its scratch files.

### Security

- Model downloads are pinned to one commit per repository, and the turbo Whisper model uses its new repository owner. Models downloaded by earlier versions are downloaded once more.
- The application runs `git` without the repository's hooks, filesystem monitor and `ext::` remotes.
- Repository folders and the recordings folder must be absolute paths on a local drive; a stored `llama-server` or `uv` path that fails the check is no longer started.
- An external API key is never sent over plain `http` to another machine.
- Failed stages are logged by stage and message length only; worker schema messages no longer quote model output.
- CI and release checkouts do not keep the workflow token, and the release workflow has no default permissions.
- Updated `url` (RUSTSEC-2026-0592) and `typescript-eslint` (minimatch advisory). Remaining advisories and why they are accepted are listed in [docs/DEPENDENCY-AUDIT.md](docs/DEPENDENCY-AUDIT.md).
- Added [docs/SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md).

### Known limitations

- The installer is unsigned. The clean-machine installation test has not been done, and Windows 11 has not been tested.
- The first setup downloads several GB (a processing environment of about 7.4 GB plus models).
- Quality was measured on synthetic meetings only. The full list is in [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

## [0.5.0] - 2026-11-16

### Added

- Cycles: a cycle repository (a Git clone or a local repository) with `cycle.yaml`, `topics.yaml` and Markdown templates, validated against versioned schemas. The `cycle-store` crate parses, validates and writes the files, runs the `git` command line, keeps the pull state and an issue cache inside `.git`, and works offline with a visible "state as of" date.
- Cycle screens: list of repositories and cycles, add a repository by address or folder, create a cycle, edit participants, glossary, report defaults and targets, and a template editor with placeholder validation. Participants belong to a cycle, and voice profiles are kept per participant of a cycle.
- Report mode gate before recording: cycle, mode, template or meeting type, and report language; a recording cannot start without a cycle and a mode.
- Four report modes (`fixed`, `template`, `auto`, `by-type`) rendered by deterministic templates from the same extracted items, so the mode can be changed after recording while the draft is not published.
- Pipeline stages `extract` and `reduce`: windows of 15 minutes, items validated against `schemas/extraction.schema.json`, quote verification against the transcript, deduplication across windows, links to open topics and issues, hierarchical summary and proposed sections. Result: `work/draft.json` (`schemas/meeting.schema.json`).
- Context for the extraction (`work/context.json`) built from the cycle, its open topics, cached open issues and the items of previous meetings; a named transcript with participant names.
- Draft editor: edit items, assignees and due dates, add and delete items, rename, add and reorder sections, move items between sections, and open the source of an item (quote, time, jump to the transcript). A new draft is kept aside when the existing one has edits.
- Output language `auto`, `pl` or `en` per cycle and per meeting.
- Language model backends: managed `llama-server`, external OpenAI-compatible server and a cloud endpoint chosen per meeting. The cloud needs a confirmation that states the endpoint host and the amount of text, and is refused by the supervisor, the runner and the worker without it. Keys stay in the Windows Credential Manager.
- Schema version 4 of the local database, with a migration of existing data.
- ADR 0017; tests for cycle files, templates and rendering, the context builder, draft operations, the new stages with a fake worker, contracts on shared fixtures, the worker steps against a mock server, the new screens on the mock backend; a test with real models for meetings of a cycle through all nine stages; the benchmark script `bbb_bench.proto.stage4_extraction`.

### Measured

- Extraction on ten synthetic meetings with the production steps (`benchmarks/results/stage4-extraction-*.json`). Gemma 4 12B Q4_K_M: schema validity 100%, task recall 86.0%, task precision 89.3%, assignee accuracy 92.8%, due-date accuracy 92.2%, quote validity 100%, 41.3 s per 1,000 words, peak VRAM 8,867 MiB above baseline; all extraction thresholds met. Qwen3 14B Q4_K_M: recall 71.7%, precision 94.1%, assignee 65.5%, due date 60.2%, quote validity 99.4%, 46.3 s per 1,000 words, 11,129 MiB; recall, assignee and due-date accuracy below the thresholds.
- The first run with an earlier prompt gave task recall of 58.6% (Gemma 4 12B) and 58.4% (Qwen3 14B); the final prompt asks for the task text as an infinitive phrase and takes weekday dates from the calendar.
- 10 hour transcript (repeated synthetic meetings, so time and memory only): 43 windows, none skipped, extraction and reduce 51.4 minutes (21.4% of the 4 hour budget), peak VRAM 9,217 MiB above baseline, valid draft with 1,431 items. With the Stage 0 transcription and diarization times the estimate for the whole pipeline is about 86 minutes.
- All four report modes produce a valid draft on one meeting with both models.

### Changed

- A window whose answer is cut off by the token limit is split in two (at most two levels) instead of being asked again.
- Meetings of a cycle use the participants of the cycle instead of the local participant list of the previous version; a meeting without a cycle still uses the local list.
- `targets.<name>.publish` accepts a third value, `off`.

### Known issues

- The cloud backend is tested only against a mock server.
- Open issues are read from a cache. Fetching them from GitHub needs a token, and the application has no setting for one yet, so the cache stays empty unless it is filled otherwise.
- Cycle files edited in the application are not committed or pushed until the publishing stage.
- Extraction quality in Polish depends on the model and was measured on synthetic transcripts only; per-meeting task recall of Gemma 4 12B is 43.5% to 100% in Polish and mixed meetings.
- In mixed-language meetings the task text is written in the report language, so English tasks are translated.
- Qwen3 14B misses the extraction thresholds (task recall, assignee and due-date accuracy) on this set; Gemma 4 12B stays the default.
- The new screens are tested on the mock backend; an end-to-end test in the real application is not part of this release.

## [0.4.0] - 2026-11-15

### Added

- Speaker recognition: every detected voice gets an embedding that is compared with the known voice profiles. A strong, clearly leading match names the voice automatically, a weaker one is only suggested, and everything else stays unknown. A participant is never assigned to two voices of one meeting automatically.
- Sample clips: up to three short, isolated and voiced clips per detected voice, cut before the full audio is deleted, so a voice can be named by listening.
- Speaker labeling screen: play clips, name a voice after an existing or a new participant, accept or reject a suggestion, skip a voice, merge voices that are the same person, and confirm all automatic matches in one click. Names appear in the transcript.
- Voice profiles per participant that learn only from confirmed labels, with a participant list, reset of a voice, deleting a participant and deleting all voice data (profiles, stored embeddings, clips). Nothing biometric leaves the machine.
- Clip retention: clips are deleted once every voice of the meeting is decided, after 30 days at the latest, and with the meeting.
- Settings for the matching thresholds (0.75 automatic, 0.60 suggestion, 0.10 margin) and the clip limit.
- Pipeline stages `speaker_id` and `clips`; the full audio is deleted only after the clips are cut. Jobs from the previous version get the new stages when they are next processed.
- ADR 0016; tests for similarity and thresholds, profile updates, clip selection, naming and retention; a test with the real models that enrolls voices from one meeting and recognizes them in the next; a series test over seven synthetic meetings.
- Measurements: on synthetic meetings 100% of enrolled speech was named correctly and none of 16 new voices was named automatically (targets 90% and 2%).

### Known issues

- Recognition was measured on synthetic voices only. With band-limited, noisy audio some different voices come close to the same-voice range, so real recordings need the confirmation step and may need other thresholds.
- Rejecting a suggestion only clears it on the screen; it returns after reloading.
- A minimal local participant list is used until cycles arrive in the next stage.

### Changed

- Full audio is now deleted after successful processing by default (it can still be kept, or kept for a number of days). Deletion still happens only after every stage that reads the audio succeeded.

## [0.3.0] - 2026-11-15

### Added

- Processing queue: one job per meeting with resumable stages (transcription, speaker separation, merge, audio cleanup, screenshot description), stored in the local database. Jobs start on request, run one stage at a time, continue after a crash and wait for an explicit retry after a failure.
- GPU guard on NVML readings: a stage starts only when the GPU stayed below a utilization limit for a configurable time and enough memory is free; the queue shows the reason for waiting.
- Python worker (`worker/`) created with `uv` on first use: transcription with Whisper large-v3-turbo and word timestamps, diarization with pyannote community-1, merge of tracks into one transcript, and description of screenshots with Qwen3-VL through a managed `llama-server`. Models are downloaded with progress and read from the local cache afterwards, so processing works offline.
- Versioned schemas for the job manifest, the transcript and the screenshot descriptions (`schemas/`), checked from Python and from Rust.
- Queue screen, transcript viewer and editor (correct a passage, exclude a passage, screenshots with descriptions), a process prompt after recording and processing settings (retention, GPU thresholds, tool paths, Hugging Face token, model downloads).
- Audio retention: full audio is deleted only after every audio-dependent stage succeeded; keeping it was the default in this release (changed afterwards, see Unreleased).
- Hugging Face token in the Windows Credential Manager.
- Tests: state machine, GPU guard on a recorded NVML trace, retention, merge and schema contracts in CI; pipeline tests with the real models (end to end, worker killed in each stage, no network, GPU held by another process); end-to-end test of the real application from recording to an excluded passage.
- ADR 0015 on the processing pipeline; `NOTICE` extended.
- Stage 2 measurements: a 2 hour recording is processed in 6.1 minutes with a peak of 8.5 GB of VRAM.

### Known issues

- Transcription quality and speaker separation were measured on synthetic recordings only.
- Stage 1 open items remain: microphone buffer flags in the 10 hour run, Windows 11, real Teams and Discord calls, multi-monitor setups with different scaling.

## [0.2.0] - 2026-11-15

### Added

- Tray application (Tauri 2) with a recorder widget and a main window: Record and Stop, source picker for the microphone and applications, adding and removing sources while recording, and a local meeting list.
- Recorder crate: session clock, segmented 16 kHz mono PCM tracks, append-only session journal, drift correction, and recovery of a meeting from its journal after a crash.
- Region screenshot overlay with a global hotkey; screenshots are saved with their offset on the session clock.
- Local SQLite store with migrations for meetings, tracks, segments and screenshots.
- Settings for the recordings folder and hotkeys, with validation of hotkeys that are already taken.
- Responsive interface in a cyberpunk style with reduced-motion support.
- Demo mode with a fictional cycle, a reproducible screenshot script and a record-flow GIF.
- Tests: unit and integration tests with generated tones (including a killed process), Playwright flows on a mock backend, and end-to-end tests that drive the real application through its WebView.
- Soak example that measures CPU, memory and lost buffers with the real audio backend.
- ADR 0014 on recorder control, IPC conventions and region screenshots.
- Stage 1 measurements: a 10 hour recording used 0.017% of total CPU and 10.1 MB of memory, with continuous tracks (ADR 0002).

### Known issues

- In the 10 hour recording the microphone track carried 194 data discontinuity flags from the audio engine (about one every 3 minutes) although no audio was found missing; the zero-drop target is not met for the microphone and the cause is not established. The application tracks had none.
- Windows 11, real Teams and Discord calls, and multi-monitor setups with different display scaling are not tested yet.

## [0.1.0] - 2026-11-14

### Added

- Architecture overview, roadmap to 1.0, testing strategy, risk register and architecture decision records.
- Measurement harness in `benchmarks/`: metrics for word error rate, diarization error rate and task extraction, a generator of scripted test meetings with planted tasks, and a unit-tested scoring code.
- Prototype of per-application audio capture on Windows (WASAPI process loopback) with clock drift correction.
- Prototypes and measurements for speech recognition, diarization, speaker recognition, structured extraction with local language models, slide description, Chrome tab capture and Python environment creation, with raw results under `benchmarks/results/`.
- Pipeline timing for a ten-hour recording.
- Continuous integration (lint, tests, secret scan, commit message check) and contribution templates.
- Measured results in the architecture decision records, license notices for models and tools.
- Stage 0 decision gate: Whisper large-v3-turbo as the primary speech recognition model and Gemma 4 12B as the default language model; ADRs 0002 to 0006 accepted. The Chrome tab capture test was repeated on an idle machine and met the 100 ms gap criterion (longest gap 47 ms), so ADR 0008 is accepted too.

