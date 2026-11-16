# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Changed

- A window whose answer is cut off by the token limit is split in two (at most two levels) instead of being asked again.
- Meetings of a cycle use the participants of the cycle instead of the local participant list of the previous version; a meeting without a cycle still uses the local list.
- `targets.<name>.publish` accepts a third value, `off`.

### Known issues

- The cloud backend is tested only against a mock server.
- Open issues are read from a cache. Fetching them from GitHub needs a token, and the application has no setting for one yet, so the cache stays empty unless it is filled otherwise.
- Cycle files edited in the application are not committed or pushed until the publishing stage.
- Extraction quality in Polish depends on the model and was measured on synthetic transcripts only.
- The new screens are tested on the mock backend; an end-to-end test in the real application is not part of this release.

### Stage 5

Publishing. Implemented and tested against fake servers and local Git repositories; not yet run against a real repository or channel.

#### Added

- Publishing to the cycle repository: the report, the transcript with a time anchor per line, the meeting data and the screenshots are committed as new files under `meetings/<ID>/<date>` (a suffix `-2`, `-3` for a second meeting on a date) and pushed. The commit is built from the remote's tip with Git plumbing and a temporary index, so the working copy is never touched; a push refused because of a concurrent commit is built again on the new tip, up to three times per run. `topics.yaml` is merged per topic (last write wins, overridden remote versions are recorded).
- GitHub issues: one per task with the label `cycle:<id>`, the assignee from the participant's GitHub login, the due date, a link to the report and to the transcript passage, the quote and a hidden marker that keeps a retry from creating a duplicate. The due date goes into the organization issue field "Target date" when there is one, otherwise a `Due:` line in the body.
- Status proposals: `extract` returns `status_changes` and `reduce` turns them into `status_proposals` of the draft, which you approve, reject or comment on. An approved proposal closes or reopens the issue with a comment (or changes the topic in `topics.yaml`). Nothing is approved automatically.
- Issue state is read before the briefing and on a manual pull (all states, with the links of issues to items): an item whose issue was closed counts as done, an item with an open issue carries its number in the context.
- Discord webhook publisher: per-target template (a built-in short one by default) and `{report_link}`, mentions of participants' Discord users and roles with an explicit `allowed_mentions` on every message, `@everyone` and `@here` neutralized, splitting at 2000 characters, screenshots in batches of at most 10 files and 8 MiB, rate limits honoured, a retry resumes after the messages already sent.
- Rules per target (`auto`, `on-approval`, `off`) and an outbox in the local database (schema version 5, with a migration): operations are retried with a backoff or the wait a service names, survive a restart, are idempotent, pause per target when a host is unreachable, and can be retried or cancelled by the user. A frozen snapshot is what gets published, and the draft of a meeting with queued or finished operations is read-only.
- Public-repository warning: a repository that is public, or whose visibility cannot be read, needs a confirmation once per repository, checked when the plan is built, when publishing starts and before each operation reaches the repository.
- Secrets for publishing: a GitHub token per API host and the Discord webhooks that cycle files refer to as `cred:` references, stored in the Windows Credential Manager in a namespace of their own, validated when saved, never returned to the interface and scrubbed from every stored or logged error.
- `targets.github.issues` in `cycle.yaml` (default `true`); optional `status_proposals` in `meeting.json`, `previous_items[].issue` in the context and `status_changes` in the extraction answer (schema versions unchanged).
- Publishing screens: status changes with approve, reject and comment in the draft editor, the publishing panel (rules per target, what will happen, outbox with retry and cancel, `topics.yaml` differences, the public-repository dialog), previews of the report as GitHub and of the summary as Discord show them, and the token and webhook setup in the settings. Screenshots and a GIF of them were taken on the mock backend with fake services.
- ADR 0018 (publishing) and ADR 0019 (default publish rule per target, proposed); documentation of the publishing setup in the README and of the publishing rules in `docs/CYCLES.md`.
- Tests: the Git side against local bare remotes, the outbox state machine, the publish plan and rules, both publishers against stateful fake GitHub and Discord servers with fault injection, a whole-loop test and resilience tests (no network and recovery, rejected push, lost answers without duplicates, rate limits, a crash while an operation runs, no secret in any file, log or the database), worker tests for status proposals, and release checks for a real test repository and webhook that are ignored by default.

#### Changed

- The extraction answer must contain `status_changes` (possibly empty); answers without it are retried. Existing `windows.json` files still reduce.
- Open issues are read through the new HTTP layer (no redirects, redaction) and, with a token, also from private repositories.
- Publishing commits only the meeting folder and `topics.yaml`; edits of `cycle.yaml` and templates stay in the working tree.

#### Known issues

- Nothing has been published to a real repository or Discord channel yet. The organization issue field "Target date", the Discord upload limits for webhooks (a conservative 8 MiB per message is used), whether GitHub resolves the transcript anchors and the quality of status proposals from a real model are unverified.
- The default rule per target (`on-approval` for both) is a proposal awaiting confirmation.
- `targets.github.path` is accepted but not used: reports always go to `meetings/<ID>/<folder>`.
- A meeting cannot be published twice or edited after publishing. A Discord message can be posted twice when a request times out after Discord stored it.
- The Discord bot and Notion publishers are not part of this stage.

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

