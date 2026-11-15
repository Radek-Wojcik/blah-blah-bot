# Blah Blah Bot

Local meeting recorder for Windows that turns calls into GitHub issues and Discord summaries, using on-device speech and language models.

![Status: processing](https://img.shields.io/badge/status-processing-lightgrey)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-blue)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

> **Project status:** Stage 2 (the processing pipeline) is complete. The application records the microphone and chosen applications and turns a recording into a transcript with speakers and described screenshots on your own GPU. Speaker naming and recognition, task extraction and reports come in the next stages. Stage 1 still has one open measurement (microphone buffer flags, see below). There is no installer; build from source.

## What works today

- **Tray recorder.** A widget in the tray starts and stops recording. Sources are the microphone and any application that has an audio session; each is stored as its own track.
- **Change sources while recording.** Add or remove a source at any time; a source added later is aligned on the same session clock.
- **Crash-safe storage.** Audio is written in 60 second segments next to an append-only journal. A killed or crashed recording is found again at the next start and can be recovered; at most the segment being written is lost.
- **Region screenshots.** A global hotkey freezes the desktop (all monitors), you drag a rectangle, and the crop is saved with its offset in the recording.
- **Meeting list and settings.** Local list of recordings with their tracks and screenshots; folders and hotkeys are configurable.
- **Processing queue.** One job per meeting, split into stages (transcription, speaker separation, merge, audio cleanup, screenshot description). It starts when you say so, waits while another program is using the GPU and shows why, runs one model at a time, and continues from the stage it stopped in after a crash.
- **Transcript with speakers.** Whisper large-v3-turbo transcribes each track with word timestamps; pyannote separates the voices of application tracks; your microphone is you. Screenshots are described by a local vision model and placed in the transcript by time.
- **Review and edit.** Correct any passage or exclude it; excluded text is kept out of everything that leaves the transcript view. Edits survive a re-run.
- **Audio retention.** Full audio is deleted only after every stage that reads it succeeded, and keeping it is the default. You can choose to delete it after processing or after a number of days.
- **Offline once set up.** Models are downloaded on first use with your Hugging Face token (kept in the Windows Credential Manager); processing itself needs no network.
- **Demo mode.** The interface runs on fictional data without any audio device, which is how the screenshots below are produced.

## Screenshots

Taken from demo mode with fictional data.

| | |
|---|---|
| ![Tray widget while recording, with two sources and level meters](docs/img/tray-widget.png) | ![Source picker before recording](docs/img/source-picker.png) |
| ![List of meetings with their lengths, tracks and screenshots](docs/img/meeting-list.png) | ![Animated recording flow: pick sources, record, add a source, take a screenshot, stop](docs/img/record-flow.gif) |
| ![Processing queue with a running, a waiting, a failed and a finished job](docs/img/queue.png) | ![Animated queue: a job moves through its processing stages](docs/img/processing.gif) |
| ![Transcript with speakers, an edited passage and described screenshots](docs/img/transcript.png) | ![A screenshot with its description between two passages of the transcript](docs/img/screenshot-description.png) |

## Requirements and getting started

| | |
|---|---|
| OS | Windows 10 version 2004 or newer (verified on build 19045); Windows 11 not tested yet |
| Build tools | Rust 1.85 or newer, Node.js 22 or newer |
| GPU | NVIDIA with 12 GB VRAM (reference: RTX 3080); peak use measured at 8.5 GB |
| Processing tools | [uv](https://docs.astral.sh/uv/) on the PATH; about 7.4 GB for the Python environment (created on first use); models downloaded on first use; a Hugging Face token and accepted terms for the speaker model |
| Disk | about 115 MB per track per recorded hour |

```bash
cd ui && npm ci && npm run build && cd ..
cargo run -p app --release --features custom-protocol
```

Open the settings to download the models and, for screenshot description, to point the application to a `llama-server` executable and its model folder. Recordings are written to `Documents\BlahBlahBot\recordings` unless you change the folder in the settings. Run the tests with `cargo test --workspace`, `npm test` and `npx playwright test --project=mock` in `ui`; the end-to-end suite against the real application is `npx playwright test --project=real` after a debug build.

## Stage 2 measurements

A synthetic recording of 2 hours (a repetition of short scripted meetings, one application track, 12 screenshots) processed stage by stage on the reference machine (RTX 3080, Windows 10 build 19045). It measures time and memory, not recognition quality. Raw data: [benchmarks/results/stage2-pipeline-2h.json](benchmarks/results/stage2-pipeline-2h.json).

| Stage | Time | Peak VRAM above baseline |
|---|---|---|
| Transcription | 234 s | 1.5 GB |
| Speaker separation | 106 s | 2.5 GB |
| Merge | 4 s | none |
| Screenshot description (12 images) | 22 s | 7.6 GB |
| **Total** | **6.1 minutes** (target 45 minutes or less) | **8.5 GB absolute** (target 11.5 GB or less) |

VRAM returned to the idle level after every stage. Tests with the real models on the short reference recordings cover the whole job, a worker killed in each stage and resumed, no network, and a job that waits while another process holds the GPU.

## Stage 1 measurements

A 10 hour recording of the default USB microphone and two test processes on the reference machine (Windows 10 build 19045), taken while the computer was in normal use. Raw data: [benchmarks/results/stage1-soak-10h.json](benchmarks/results/stage1-soak-10h.json); details in [ADR 0002](docs/decisions/0002-audio-capture.md).

| Measure | Result | Target |
|---|---|---|
| CPU, three tracks | 0.017% of total CPU | 3% or less |
| Memory | 10.1 MB peak | 200 MB or less |
| Lost buffers, application tracks | 0 | 0 |
| Lost buffers, microphone | 194 flags from the audio engine, about one every 3 minutes; the audio is continuous and its length is exact | 0 (not met) |
| Continuity | 601 complete segments per track, no gaps in the journal | no gaps |
| Process killed during recording | at most the segment being written is lost | 60 s or less |

The microphone flags were not explained: no second microphone was tried and nothing was found missing in the audio. Windows 11, real Teams and Discord calls and multi-monitor setups with different scaling are not tested yet.

## What it will do

- **Record only what matters.** Microphone and selected applications are captured as separate tracks, so music and notifications stay out of the recording.
- **Process after the call.** Transcription, speaker recognition and summarization run locally once the meeting is over and the GPU is free. Nothing runs while you talk.
- **Know who said what.** Name a voice once from a short clip; it is recognized in later meetings of the same project.
- **Keep the team in sync.** Reports are committed to a Git repository, tasks become GitHub issues with owners and due dates, and a short summary goes to Discord.
- **Remember the context.** Each meeting series carries its open tasks, open topics, glossary and participants into the next meeting, starting with a briefing.
- **Show its sources.** Every task and decision links to the quote and timestamp it came from.

## How it works

```mermaid
flowchart LR
    A[Record<br/>mic + apps + browser tabs] --> B[Queue<br/>waits for a free GPU]
    B --> C[Transcribe<br/>and identify speakers]
    C --> D[Describe screenshots]
    D --> E[Extract tasks,<br/>decisions, topics]
    E --> F[Review draft]
    F --> G[Publish<br/>GitHub, Discord, Notion]
```

A Tauri and Rust application handles recording, the job queue and publishing. Model inference runs in a Python process that exists only while a meeting is being processed. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Stage 0 measurements

Prototype results on the reference machine (RTX 3080 with 12 GB, Windows 10 build 19045). Test meetings are synthetic: scripted text, voices cloned from English reference voices (Polish speech therefore carries an English accent). Treat the numbers as a comparison between candidates, not as a promise for real meetings. Raw data is in [benchmarks/results](benchmarks/results); the reasoning is in the [decision records](docs/decisions/README.md).

| Area | Result | Target |
|---|---|---|
| Recording (Windows process loopback) | 0 dropped buffers in 2 h; tracks isolated to -63 dB; alignment error 27 ms; 0.03% CPU; 10 MB memory | 0 drops; -40 dB; 50 ms; 3%; 200 MB |
| Speech recognition, Whisper turbo | WER 8.9% Polish, 14.3% English, 16.4% mixed; 36 times faster than real time | 15%, 20% for mixed; 10 h in 60 min |
| Speech recognition, Whisper large-v3 | WER 11.5% Polish, 29.6% mixed; 7 times faster than real time | as above |
| Term recall of English terms, best configuration | 51% | 85% (not met) |
| Diarization, 2 to 4 speakers | DER 20.5 to 23.3% against a speech-only reference | 15% (not met) |
| Diarization, 8 speakers | DER 23.4% | 30% |
| Speaker recognition, 4 enrolled voices | 100% accuracy, unknown voice rejected | 90% |
| Task extraction, Gemma 4 12B | recall 89%, precision 95%, assignee 91%, due date 88%, all quotes verified | 80%, 80%, 85%, 85% |
| Slide description, Qwen3-VL 8B | 100% of key facts, 1.5 s per image | not set |
| 10 hours of audio, all stages | about 58 minutes (extraction extrapolated) | 4 hours |
| Browser tab capture | idle machine: no lost frames, longest gap 47 ms in 31 minutes; with the machine in use: gaps of 218 to 667 ms | gaps of 100 ms or less (met only when idle) |
| Python environment from lock file | 70 s and 7.4 GB for speech and diarization groups | unattended |

## Limitations

- Processing needs an NVIDIA GPU; without readings from the driver a job waits.
- Speaker separation is measured on synthetic meetings only; on 2 to 4 speakers the error rate was above the 15% target in Stage 0 and is to be confirmed on real recordings.
- Speakers are labelled "Speaker 1", "Speaker 2" and so on; naming and recognition of voices come in Stage 3.
- The Python environment is large (about 7.4 GB) and the first model downloads take a while.

## Roadmap

| Stage | Scope | Release | Status |
|---|---|---|---|
| 0 | Prototypes and decision gate | v0.1.0 | Done, gate passed |
| 1 | Recorder | v0.2.0 | Done; microphone buffer flags in the 10 hour run are open |
| 2 | Processing pipeline | v0.3.0 | Done |
| 3 | Speakers and voice profiles | v0.4.0 | Planned |
| 4 | Cycles and reports | v0.5.0 | Planned |
| 5 | Publishing | v0.6.0 | Planned |
| 6 | Convenience | v0.7.0 | Planned |
| 7 | Integrations | v0.8.0 | Planned |
| 8 | Stabilization | v1.0.0 | Planned |

Details are in [docs/ROADMAP.md](docs/ROADMAP.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Testing strategy](docs/TESTING.md)
- [Risks and prototypes](docs/RISKS-AND-POC.md)
- [Architecture decision records](docs/decisions/README.md)

## License

Released under the [MIT License](LICENSE).

## Author

Radosław Wójcik
