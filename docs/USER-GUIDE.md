# User guide

This guide covers installing Blah Blah Bot on Windows, completing the first-run setup, organizing meetings into Projects, recording or importing media, managing the processing queue, naming speakers, editing drafts, publishing to GitHub and Discord, and configuring the 5-tab Settings screen. Problems are covered in [TROUBLESHOOTING.md](TROUBLESHOOTING.md), known limits in [LIMITATIONS.md](LIMITATIONS.md).

## Requirements

| | |
|---|---|
| OS | Windows 10 version 2004 or newer (verified on build 19045). Windows 11 has not been tested. |
| GPU | NVIDIA with 12 GB of video memory (reference machine: RTX 3080). A job waits while the driver gives no readings. |
| Disk | About 7.4 GB for the Python environment, the models on top of that, and about 115 MB per track per recorded hour. |
| Network | Needed once to create the Python environment and download models, and when pulling/publishing Git remotes. Processing itself works offline. |
| Git | The `git` command line on the `PATH` if you use Git-backed project repositories. |
| Browser | Chrome 116 or newer (tested on 136) for browser tab capture and Google Meet detection. |

## Install

Download `Blah-Blah-Bot_<version>_x64-setup.exe` and `SHA256SUMS.txt` from the same release, compare the checksum, and run the installer. It installs for the current user and needs no administrator rights. Windows SmartScreen warns about the unsigned installer; see [INSTALL.md](INSTALL.md) for the checksum command and SmartScreen details.

You can also build from source using the commands in [README.md](../README.md) and [CONTRIBUTING.md](../CONTRIBUTING.md).

## First run

On the first start the application opens a setup wizard. It prepares what processing needs and can be skipped step by step: nothing blocks you from exploring the app, and any incomplete item is listed at the end and explained again if you try to process a meeting.

1. **Welcome.** Overview of local processing and a reminder that the Python environment and models require tens of gigabytes of disk space. Audio, transcripts, and reports stay on your computer unless you choose a cloud language model per meeting.
2. **Processing environment.** Creates the isolated Python environment with `uv` from the bundled `worker/` lockfile. Progress streams live during creation.
3. **Speech models.** Downloads Whisper `large-v3-turbo` (~1.6 GB) and pyannote speaker diarization. The speaker model is gated: accept its terms on Hugging Face, create a read token, and store it under **Hugging Face token** (saved in the Windows Credential Manager).
4. **llama-server.** Scans `BBB_LLAMA_SERVER`, `PATH`, WinGet, and common folders for `llama-server.exe`, or lets you browse/enter a local path (`winget install ggml.LlamaCpp`).
5. **Language model.** Pick a local `.gguf` file for `llama-server`, configure an external OpenAI-compatible server, or skip reports for transcript-only mode.
6. **Screenshot analysis.** Download the vision model (~5 GB) or choose **Skip screenshot analysis** to keep captured screenshots without describing them.
7. **Summary.** Shows every item's readiness state (`Ready`, `Missing`, `Needs fixing`, `Skipped`) with **Fix** buttons.
8. **First cycle / project.** Create your first project or skip and create one later from the **Projects** workspace.

**Leave setup** closes the wizard at any point. While required processing pieces are missing, a readiness banner appears on the **Projects** page with **Open setup**. You can re-open the wizard anytime from **Settings > Processing > Run setup again**.

## Projects & briefings

The **Projects** view is the home screen of the application. Each project represents a recurring series of meetings backed by a cycle Git repository (`cycle.yaml`, `topics.yaml`, and Markdown templates).

- **Project cards:** Each card shows participant count, repository sync state, and a live briefing summary (open tasks, overdue items, open discussion topics). Any unassigned/standalone recordings appear in **Unassigned / standalone meetings** below the project grid.
- **New Project (`ProjectWizard`):** A 2-step wizard creates a new project:
  1. Choose **Automatic local workspace** or **Connect existing repository** (local folder or Git remote URL), then enter the project name and ID.
  2. Set default output language, default report mode, optional GitHub repository link, optional Discord webhook secret reference, and optional **Local Folder Auto-Save** for `.md` notes.
- **Project workspace (`ProjectDetail`):**
  - **What to discuss next:** Open topics and briefing status (`Refresh` pulls the remote repository).
  - **Action items & Tasks:** Urgent tasks sorted by overdue/due date, plus **All tasks** and **Sync GitHub issues** to inspect open and closed GitHub issues.
  - **Meetings list:** All recordings and imported meetings belonging to the project.
- **Import existing media (`+ Add meeting`):** Opens `ImportMeetingModal` to ingest an external audio or video file (`.wav`, `.mp3`, `.m4a`, `.mp4`, `.mkv`, `.webm`, etc.):
  - **Audio only:** Extracts and chunks audio into 60-second 16 kHz mono WAV segments.
  - **Audio + Visual Slides:** Extracts audio and detects key slide changes from the video track into timestamped screenshots.
  - Choose the report mode, output language, and whether to start processing immediately or add the job to the queue.
- **Project settings (`⚙ Settings`):** Modal with tabs for **People & Glossary**, **Report & Templates**, **Integrations** (GitHub repository link, Discord webhook secret Store/Clear/Test, per-target publishing rules), and **Local Preferences** (local `.md` auto-save folder and per-project AI backend/model override).

## Recording

You can record from either the **Record** tab in the Main UI or the compact **Tray widget**:

- **Live metering & controls:** Both views show a 12-bar live audio waveform equalizer (`WaveformEq`), elapsed time, active sources, and a **Screenshot** button.
- **Source picker:** Sources are grouped into **Microphone**, **Applications** (desktop apps with active audio sessions, filtering out `AudioSrv`), and **Browser** (tabs captured via the Chrome extension, or `Chrome (whole browser)` fallback).
- **Dynamic sources:** Add or remove sources mid-recording; tracks stay aligned on the session clock.
- **Report gate:** Choose the target project (cycle), report mode, template/meeting type, and output language before recording starts.
- **Region screenshots:** Press the global hotkey (`Ctrl+Alt+S` by default) to freeze the desktop, drag a rectangle, and save a timestamped PNG.
- **Crash recovery & wake lock:** Audio writes in 60-second segments alongside an append-only journal. Crashed recordings appear as **Needs recovery** and can be recovered with at most the in-progress segment lost. A `SystemWakeLock` keeps the computer awake while recording or while jobs are scheduled/processing.

### Browser extension & call detection

The unpacked Chrome extension captures individual browser tabs and reports Google Meet calls over `127.0.0.1` without `tabs` or `history` permissions. Setup and pairing live under **Settings > Preferences > Browser extension**; see [guides/extension.md](guides/extension.md) for step-by-step installation. Call detection watches configured desktop apps (Discord, Teams, Zoom, Slack) and Google Meet and prompts once per call (`Record`, `Not now`, `Never`).

## Processing queue

Open the **Queue** page to monitor and control background processing jobs (`Transcription`, `Speaker separation`, `Merge`, `Speaker recognition`, `Clips`, `Audio cleanup`, `Screenshot description`, `Extraction`, `Draft`).

- **Progress & ETA:** Each job shows weighted stage progress, per-stage status/messages, and an estimated time remaining.
- **GPU guard & temporary Queue overrides:** By default, jobs pause if another app (such as a game) uses the GPU above the configured threshold or if free VRAM is below the stage budget. On the right side of the Queue page, **Queue GPU overrides (temporary)** lets you raise or lower the utilization limit and idle wait just for the current queue; it automatically reverts to global Settings once the queue finishes.
- **Schedule start:** For queued or waiting jobs, pick a **Run at** date/time and press **Schedule** (or **Clear schedule**). The PC stays awake while scheduled or running jobs exist.
- **Force process:** Bypasses the GPU utilization limit and any scheduled delay once to start a waiting/queued job immediately.
- **Crash resume:** Interrupted jobs continue from the stage where they stopped.

## Meeting detail: Draft, Transcript, Speakers

Opening any meeting opens a tabbed workspace:

- **Draft & Report:**
  - Edit item text, assignee, and due date inline; add/delete items; rename, add, or reorder sections; move items between sections.
  - Inspect each item's verified source quote, timestamp, and jump directly to the transcript passage. Inferred values and unverified quotes carry visible markers.
  - Switch report mode (`fixed`, `template`, `auto`, `by-type`) while unpublished; a re-run never overwrites your manual edits unless you accept the new draft.
  - **Download note (.md):** Exports a clean standalone Markdown report with relative screenshot references to a chosen path or your local notes folder.
  - **Status changes & Publishing:** Review AI-proposed issue/topic closures (`Approve` / `Reject`), preview rendered GitHub Markdown or Discord Chat formatting, review the operation plan and `topics.yaml` diff, and press **Publish** (public repositories require explicit confirmation). Failed or offline operations stay in the persistent **Outbox** for retry. After publishing, the draft locks and later updates are added under **Corrections** (`corrections.md` + optional issue comment).
- **Transcript:** View speaker-attributed segments and timestamped screenshot descriptions. Click any segment to correct its text or exclude it from report extraction (edits survive re-processing).
- **Speakers:** Play short audio clips for detected voices in application tracks, assign them to participants, create new participants, merge duplicate voices, and confirm automatic matches.

## Settings (5 tabs)

Settings uses left-hand section navigation and prompts via `UnsavedDialog` if you switch tabs or leave **Settings** with unsaved edits:

1. **AI Models:** Click **Auto-detect models on disk** to locate `llama-server.exe`, local LLM `.gguf` files, and vision `.gguf` models. Configure the default backend (`Managed local server` or `External local server`; `Cloud` is per-meeting only), context size (default `16384`), models folder, vision model path / **Skip screenshot analysis**, and download or check cached speech/vision models.
2. **Connectors:** Store and test write-only credentials in the Windows Credential Manager: **GitHub Token** (per host), **Hugging Face Token** (for gated diarization weights), and **External / Cloud LLM API Keys**. Per-project Discord webhooks are configured in **Project Settings > Integrations**.
3. **Preferences:** Configure **Automatically save meeting notes & transcripts to a local folder**, default recordings directory, global hotkeys, connected project Git repositories (add/pull), **Call detection** sensitivity (`low`, `balanced`, `high`) and watched apps, and **Browser extension** pairing.
4. **Processing:** Configure post-recording prompt behavior (`Ask me`, `Process immediately`, `Do nothing`), global **GPU guard thresholds**, **Audio retention policy** (`Keep`, `Delete after processing`, `Keep for N days`), **Voice recognition thresholds** (`0.75` automatic, `0.60` suggestion, `0.10` margin, `30` days clip retention), worker Python environment (`uv` and `worker/` paths), and **Run setup again**.
5. **Voice Profiles:** View all enrolled profiles across projects with project badges, sample counts, and sample clip playback. Use **Add voice profile manually** to enroll a speaker ahead of time from a 15–30 s audio/video sample, or rename, reset, and delete individual profiles or all biometric voice data.

## Configuration & environment reference

- **`cycles/<ID>/cycle.yaml`:** Plain YAML validated before saving; see [CYCLES.md](CYCLES.md) and `schemas/cycle.schema.json`.
- **Environment variables (development and tests):** `BBB_DEMO=1` runs on demo data without audio hardware, `BBB_DATA_DIR` overrides the application data directory, `BBB_LOG` sets the log filter, `BBB_WORKER_DIR` points to the Python worker source folder, `BBB_WORKER_ENV_DIR` shares a worker virtual environment across runs, and `BBB_VISION_DIR` / `BBB_LLAMA_SERVER` override vision model / binary discovery.
