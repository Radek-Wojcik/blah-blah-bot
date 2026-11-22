# User guide

This guide covers what the application does in this build: installing it, preparing it for the first meeting, recording, processing, naming speakers, and working with cycles and draft reports. Problems are covered in [TROUBLESHOOTING.md](TROUBLESHOOTING.md), the known limits in [LIMITATIONS.md](LIMITATIONS.md).

Publishing a report to GitHub or Discord is not part of this build. Reports stay as drafts in the application.

## Requirements

| | |
|---|---|
| OS | Windows 10 version 2004 or newer (verified on build 19045). Windows 11 has not been tested. |
| GPU | NVIDIA with 12 GB of video memory (reference machine: RTX 3080). A job waits while the driver gives no readings. |
| Disk | About 7.4 GB for the processing environment, the models on top of that, and about 115 MB per track per recorded hour. |
| Network | Needed once, to create the processing environment and download models. Processing itself works offline. |
| Git | The `git` command line on the PATH, if you use cycle repositories. |

## Install

Download `Blah-Blah-Bot_<version>_x64-setup.exe` and `SHA256SUMS.txt` from the same release, compare the checksum, and run the installer. It installs for the current user and needs no administrator rights. Windows SmartScreen warns about the unsigned installer; see [INSTALL.md](INSTALL.md) for the checksum command and for what the warning means.

You can also build from source; the commands are in the README and in [CONTRIBUTING.md](../CONTRIBUTING.md).

## First run

Open **Settings > Processing**. Processing needs the pieces below, and each is checked before a job starts. A missing piece is reported with a code and a way to fix it instead of failing in the middle of a job (see [TROUBLESHOOTING.md](TROUBLESHOOTING.md)).

1. **Processing environment.** A Python environment created with `uv` from the files shipped with the application. The first run downloads several GB and needs a network connection. `uv` is taken from the path in the settings, then from the installation, then from the PATH.
2. **Speech model** (Whisper turbo, about 1.6 GB). Download it in the settings.
3. **Speaker model** (pyannote). It is gated: accept its terms on its page on Hugging Face, create a read token and enter it under **Hugging Face token**. The token is kept in the Windows Credential Manager. It is not needed once the model has been downloaded.
4. **Screenshot model** (about 5 GB) and **llama-server**. Both are needed only while screenshot analysis is on. Turn **Describe screenshots** off to skip them.
5. **Language model for reports.** Choose one of: a local model (a `.gguf` file run by `llama-server`), an external OpenAI-compatible server, or a cloud endpoint chosen per meeting. You can also turn reports off, which gives a transcript only.

`llama-server` is part of llama.cpp and is not shipped with the application. If it is not on the PATH, the application also looks in the WinGet folders and the usual install folders. You can install it with `winget install ggml.LlamaCpp`, or point to your own `llama-server.exe`.

Paths you enter are checked before they are saved: each must be an absolute local path to a file of the expected kind. Network paths are refused.

## Recording

The tray widget starts and stops a recording. Before it starts you pick the sources: the microphone and any application that currently has an audio session. Each source becomes its own track. You can add or remove a source while recording; a source added later is aligned to the same clock.

A global hotkey freezes the desktop, lets you drag a rectangle and saves the crop with its time in the recording.

Audio is written in 60 second segments next to a journal. After a crash the recording is found again at the next start and can be recovered; at most the segment being written is lost.

Before a recording starts you choose the **cycle**, the **report mode**, a template or meeting type where the mode needs one, and the **report language**. A recording cannot start without a cycle and a mode.

## Processing

After a recording ends the application asks whether to process it now or leave it for manual processing. The queue shows one job per meeting with these stages: transcription, speaker separation, merge, audio cleanup, screenshot description and report extraction.

- Jobs start when you say so. A job waits while another program is using the GPU or too little video memory is free, and the queue shows the reason.
- One model runs at a time.
- After a crash or restart, a job continues from the stage it stopped in.
- The language is detected automatically. Polish, English and mixed meetings are supported; you can force `pl` or `en` for a meeting.
- You can correct or exclude any passage of the transcript. Excluded text stays out of everything derived from it, and edits survive a re-run.

**Audio retention.** The default is to delete the full audio after processing, and only after every stage that reads it has succeeded. You can keep it, or keep it for a number of days.

## Speakers and voice profiles

Each detected voice in an application track comes with a few short clips. Play them, then pick a name, add a new participant, or merge voices that are the same person. Your microphone track is you. Names appear in the transcript.

In later meetings a voice that matches a profile is named automatically, a weaker match is only suggested, and an unknown voice stays unknown. Profiles are learned only from labels you confirmed. The thresholds (0.75 automatic, 0.60 suggestion, 0.10 margin) are settings.

Voice profiles and sample clips are biometric data. They stay on your computer. Sample clips are deleted once you have named the voices, or after 30 days. In the settings you can reset or delete the voice of one participant, or all voice data.

## Cycles and reports

A cycle is a recurring series of meetings with its own participants, glossary, templates and report defaults. Cycles live in a Git repository: cloned from an address, an existing folder, or a new local repository. The files are plain Markdown and YAML, so a team can also edit them without the application. The application shows the state of the last pull and keeps working offline. Changes made in the application are written to the working tree; they are not committed in this build.

| Report mode | Sections |
|---|---|
| `fixed` | Built in: summary, decisions, tasks, open topics, notes, ideas, budgets |
| `template` | The sections declared by a template of the cycle |
| `auto` | Proposed from the extracted items, editable in the draft |
| `by-type` | The template that the cycle maps the meeting type to |

The language model extracts items per 15 minute window. Every quote is matched against the transcript; an item whose quote cannot be matched, or whose value was inferred, carries a visible marker. Items are merged across windows and linked to the open topics and tasks of the cycle.

The **draft editor** lets you change the text, assignee and due date of an item, add and delete items, and rename, add and reorder sections. Each item shows its source: the quote, the time and a jump to the transcript. A new draft never overwrites your edits; you accept or discard it. The report mode can be changed while the draft is unpublished.

The file layout, front matter and placeholders of templates are described in [CYCLES.md](CYCLES.md).

### Language model backends

- **Local (default).** `llama-server` runs a `.gguf` model on the GPU for the duration of the job.
- **External server.** Any OpenAI-compatible server. An API key is never sent over plain `http` to another machine.
- **Cloud.** Chosen for one meeting only, never a default and never a fallback. Each meeting needs a confirmation that names the endpoint host and says how much text will be sent.

## Demo mode

The interface can run on fictional data without an audio device. This is how the screenshots in the README are produced.
