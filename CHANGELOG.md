# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Stage 6 (convenience)

Briefing, call detection, a Chrome extension for browser tabs and the whole-browser fallback. Tested on the mock backend, on recorded session traces, over real loopback sockets and in a scripted Chrome; not yet run on real calls.

#### Added

- Bridge (`crates/bridge`): a WebSocket server on `127.0.0.1` for the Chrome extension, protocol version 2 (JSON control messages and binary audio frames on one connection, schema `schemas/bridge-message.schema.json`, fixtures shared with the extension tests). Pairing with a one-time 8-character code valid for 5 minutes, a 32-byte secret per paired browser (only its SHA-256 is kept), checks of the peer, `Host` and `Origin`, size limits, a heartbeat and a resume after a lost connection. `examples/test_receiver` runs the bridge alone. ADR 0020.
- Browser tabs as tracks: a tab is a pushed track placed by sample count; missing frames become silence journaled as `stream_gap`, and the sender clock feeds drift correction. A closed tab ends only its own track. Tabs the application did not take stay on offer and join a recording that starts later.
- Chrome extension (`extension/`, Manifest V3, TypeScript, plain `tsc`): captures a tab with `tabCapture` in an offscreen document, keeps the tab audible, streams 16 kHz mono audio to the application, reconnects with a backoff and a 30 second buffer, survives a stopped service worker, and reports Google Meet calls with the site name only. Permissions: `tabCapture`, `offscreen`, `activeTab`, `storage`, `tabGroups` and `meet.google.com`; no `tabs` or `history`. Shortcut Alt+Shift+R.
- Guided capture of tab groups: the popup lists the tabs of the group with their status (captured, waiting for a click, not audible, failed) and a Next tab button; a tab added later waits and shows a badge. The group status is also shown in Settings, Browser extension.
- Extension packaging: `npm run pack` writes `target/extension/blah-blah-bot-extension-<version>.zip` without the test hook; the extension is installed unpacked from the release zip in developer mode ([docs/guides/extension.md](docs/guides/extension.md), ADR 0021) and has a fixed id derived from its manifest key.
- Settings, Browser extension: pairing code, connection and group status, unpair, switching the bridge off.
- Meeting detection: Discord, Teams, Zoom and Slack (the list is a setting) are recognized by a microphone session that stays open for the sustain time of the chosen sensitivity (low, balanced, high; default balanced); a session is tied to its application through the parent chain of its process (the new Teams plays from a webview child). One prompt per call with Record, Not now and Never for the application, no prompt while the application is recorded, browsers excluded from the audio rule, Meet reported by the extension through the same rules. Nothing is recorded to detect a call. ADR 0022.
- Detection tools: `record_session_trace` records the audio sessions of a machine (enumeration only, no audio) and anonymizes the trace; `score_detection` replays the committed traces for each sensitivity.
- Settings, Detection: on and off, sensitivity, watched programs, opt-out and its undo.
- Source picker grouped as Microphone, Applications and Browser; captured tabs (`Tab: <title> (<host>)`, grouped as they are in Chrome) and `Chrome (whole browser)` as the fallback with the reason it is offered; a warning when a tab and the whole browser are both chosen; a tab captured during a recording joins it. Only the site name of a tab is stored with a recording.
- Briefing screen (`?page=briefing`, opened from the cycle list and from the report mode gate): open tasks by person with overdue marks, tasks without an owner, open topics, finished tasks kept apart, "state as of" when the repository cannot be updated. The data comes through a `BriefingSource` trait, so the source can be replaced.
- Briefing from the issues of Stage 5: `PublishingSource` is the default source and lists open and done tasks from the issues read back from GitHub (a meeting task that became an issue is shown once, topics still from the cycle files); `get_briefing` with `refresh` also reads the issues again and, when the network fails, shows the cached state with its date and the reason. A repository that is not on GitHub or whose issues were never read uses the cycle files as before.
- Tests under `target/` only (`test_tmp::TestDir` under `target/test-tmp`, `target/e2e-tmp`); scripted Chrome tests for the extension; a mock end-to-end test of the briefing, a detected call, a recording with a tab and the process prompt.

#### Measured

- Continuity of a synthetic tone through the extension, the bridge and the recorder, **2 minute run** (the 30 minute run is open): 5,978 frames, none lost, longest gap 46.3 ms (limit 100 ms). Raw data: `benchmarks/results/stage6-tab-30min-synthetic.json`.
- Detection replayed over 11 session traces (one real idle trace of 4 minutes, ten made by hand), per sensitivity (`stage6-detection-*.json`). Balanced: 6 of 6 labelled calls prompted once after 8 to 10 s, no false prompt on the real idle trace, 1 on a synthetic non-call trace (a 12 s microphone test). Low: 5 of 6, after 20 s. High: 6 of 6 after 3 s, 2 false prompts on synthetic non-call traces.

#### Known issues

- A real tab was not recorded for 30 minutes, and Chrome's capture of a real tab and the keyboard shortcut are not covered by automated tests.
- False prompts over a working day, real Discord, Teams and Meet calls and Windows 11 are not measured; the hand-made traces hold assumptions about how those applications use the microphone.
- The default sensitivity (`balanced`, ADR 0022) and unpacked extension distribution (ADR 0021) are Accepted, though the preset numbers have not yet been tuned against a real working-day trace.
- Only Chrome is supported. The extension is installed in developer mode, so Chrome shows a warning and may switch it off after an update; there is no automatic update.
- A call in a browser other than Meet, and a call in an application that closes its microphone while muted, are not detected. A short microphone test in a watched application can raise a prompt.

## [0.6.0] - 2026-11-22

### Fixed

- The first `extract` of any meeting that belongs to a cycle could hang forever in the state running without starting a worker (v0.5.0): the queue reported progress while it held the database lock and the application callback locked it again. Progress is now reported after the lock is released, a panic in the queue marks the stage as failed with a fixed message, the queue driver can be restarted after a panic, and the panic hook logs the location only. A regression test covers it.
- The built-in short Discord message printed an empty `Tasks` heading for a meeting without tasks (the link under it counted as content). A heading followed only by placeholders that came out empty is now removed in every template and variant, so neither the short message nor the report shows an empty section.
- A speaker the user named in the Voices screen was called "participant" ("uczestnik") in the summary. The extraction prompt now lists the speakers of each window as named, unnamed or the person who recorded the meeting, tells the model to call named people by their names and to keep generic labels such as `Speaker 2`, and the summary prompt gets the same names; quotes verify against the same lines as before. A name given after the draft was written applies to the next run of `extract`, not to the existing draft.
- Region screenshot (Ctrl+Alt+S) showed a black screen over every monitor. The desktop was captured correctly; the cause was in the display path. The content security policy did not allow the page to reach the IPC protocol, so Tauri fell back to the postMessage interface, which returns binary results as an array of numbers; the overlay built its image from that array as if it were bytes, the PNG could not be decoded and the black page background was all that was left. The policy now allows the IPC protocol, binary results are converted correctly in both modes, and the frozen desktop is encoded with fast compression off the main thread. The overlay window now stays hidden until its page has decoded the frozen desktop, so the live desktop remains on screen until then, and it closes before the region is cropped and saved, which happen off the window thread. A capture that is all black, cannot be decoded or does not arrive within 20 seconds is reported in a message and the overlay closes itself (Escape and a click close it in every state), and the window is placed again when the system moved or resized it.

### Stage 5

Publishing. Tested against fake servers and local Git repositories, and run once against a real test repository and a real test Discord webhook, including a publish from the application window.

#### Added

- Publishing to the cycle repository: the report, the transcript with a time anchor per line, the meeting data and the screenshots are committed as new files under `<path>/<ID>/<date>` (`meetings` unless `targets.github.path` says otherwise; a suffix `-2`, `-3` for a second meeting on a date) and pushed. The commit is built from the remote's tip with Git plumbing and a temporary index, so the working copy is never touched; a push refused because of a concurrent commit is built again on the new tip, up to three times per run. `topics.yaml` is merged per topic (last write wins, overridden remote versions are recorded).
- GitHub issues: one per task with the label `cycle:<id>`, the assignee from the participant's GitHub login, the due date, a link to the report and to the transcript passage, the quote and a hidden marker that keeps a retry from creating a duplicate. The due date goes into the organization issue field "Target date" when there is one, otherwise a `Due:` line in the body.
- Status proposals: `extract` returns `status_changes` and `reduce` turns them into `status_proposals` of the draft, which you approve, reject or comment on. An approved proposal closes or reopens the issue with a comment (or changes the topic in `topics.yaml`). Nothing is approved automatically.
- Issue state is read before the briefing and on a manual pull (all states, with the links of issues to items): an item whose issue was closed counts as done, an item with an open issue carries its number in the context.
- Discord webhook publisher: per-target template (a built-in short one by default) and `{report_link}`, mentions of participants' Discord users and roles with an explicit `allowed_mentions` on every message, `@everyone` and `@here` neutralized, the built-in message kept to one message of 2000 characters that ends with the link to the full report (a template of your own is split instead), screenshots in batches of at most 10 files and 8 MiB, rate limits honoured, a retry resumes after the messages already sent.
- Rules per target (`auto`, `on-approval`, `off`) and an outbox in the local database (schema version 5, with a migration): operations are retried with a backoff or the wait a service names, survive a restart, are idempotent, pause per target when a host is unreachable, and can be retried or cancelled by the user. A frozen snapshot is what gets published, and the draft of a meeting with queued or finished operations is read-only.
- Public-repository warning: a repository that is public, or whose visibility cannot be read, needs a confirmation once per repository, checked when the plan is built, when publishing starts and before each operation reaches the repository.
- Secrets for publishing: a GitHub token per API host and the Discord webhooks that cycle files refer to as `cred:` references, stored in the Windows Credential Manager in a namespace of their own, validated when saved, never returned to the interface and scrubbed from every stored or logged error.
- `targets.github.issues` in `cycle.yaml` (default `true`); optional `status_proposals` in `meeting.json`, `previous_items[].issue` in the context and `status_changes` in the extraction answer (schema versions unchanged).
- Publishing screens: status changes with approve, reject and comment in the draft editor, the publishing panel (rules per target, what will happen, outbox with retry and cancel, `topics.yaml` differences, the public-repository dialog), previews of the report as GitHub and of the summary as Discord show them, and the token and webhook setup in the settings. Screenshots and a GIF of them were taken on the mock backend with fake services.
- The rule of each target is chosen explicitly, with a switch (Off, On approval, Automatic) that explains what each position does: in the cycle editor where the GitHub or Discord target is switched on (it shows and saves On approval until another is picked), in the publishing panel and in the settings; it writes `targets.<name>.publish` only (`set_target_rule`), and the interface says that status changes are never written back automatically. The public-repository check holds under every rule.
- `targets.github.path` places the meeting folder in the repository (validated: relative, no `..`, no drive letter, no `.git`).
- The publishing panel shows `topics.yaml` before and after per topic before publishing, with a fallback to topic names when it cannot compare.
- Corrections to a published meeting. The draft stays locked after publishing, and what changes afterwards is written as a correction: a text, a date and an optional item or issue, kept in `work/corrections.json` (new `schemas/corrections.schema.json`). **Add correction** in the draft editor writes it; **Publish** (or the Automatic rule) adds a new file `corrections.md`, `corrections-2.md`, ... to the meeting's folder in the repository, never changing an existing one, and, for a note about an item or an issue, a comment on that issue, once. It goes through the same outbox, rules and public-repository check as the meeting (database schema version 6, with a migration: two more kinds of outbox operation). Later meetings are given the corrections: an optional `corrections` list in the cycle context, read from the repository files and the local notes, shown by the extraction prompt only when there are some. Commands `list_corrections`, `add_correction`, `publish_corrections` and `delete_correction`.
- ADR 0018 (publishing) and ADR 0019 (default publish rule per target, explicit choice and corrections, accepted); documentation of the publishing setup in the README and of the publishing rules in `docs/CYCLES.md`.
- Tests: the Git side against local bare remotes, the outbox state machine, the publish plan and rules, both publishers against stateful fake GitHub and Discord servers with fault injection, a whole-loop test and resilience tests (no network and recovery, rejected push, lost answers without duplicates, rate limits, a crash while an operation runs, no secret in any file, log or the database), worker tests for status proposals, and release checks for a real test repository and webhook that are ignored by default.

#### Changed

- The extraction answer must contain `status_changes` (possibly empty); answers without it are retried. Existing `windows.json` files still reduce.
- Open issues are read through the new HTTP layer (no redirects, redaction) and, with a token, also from private repositories.
- Publishing commits only the meeting folder and `topics.yaml`; edits of `cycle.yaml` and templates stay in the working tree.
- An issue whose assignee GitHub refuses with 422 (a login that does not exist) is made without the assignee instead of failing; the outcome says that the assignee was dropped.
- After a push, the clone catches up when its `topics.yaml` was never committed (a cycle created in the clone) or lacks the topics the meeting added; before, the branch stayed behind and the next briefing did not see the meeting.

#### Known issues

- The whole flow was checked once against a private test repository and a test Discord webhook (`full_loop_demo_meeting`, `docs/TESTING.md`): the commit, the issues with assignee, due date and marker, an issue closed by a status change, the post, a correction to the published meeting (a new file in its folder and a comment on its issue) and the next briefing. A public repository, GitHub Enterprise and the rest of this list were not.
- Still unverified: the organization issue field "Target date", the Discord upload limits for webhooks (a conservative 8 MiB per message is used), whether GitHub scrolls to the transcript anchors and the quality of status proposals from a real model.
- A meeting cannot be published twice or edited after publishing; what changes afterwards is a correction, which is a new file and a comment, never a rewrite. A Discord message can be posted twice when a request times out after Discord stored it.
- The Discord bot and Notion publishers are not part of this stage.

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

