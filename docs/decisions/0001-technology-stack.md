# 0001. Technology stack and build-versus-fork strategy

**Status:** Accepted

## Context

The application needs a lightweight always-on recorder, a desktop UI, and a model pipeline that runs only after meetings. Existing open-source meeting assistants are built around live transcription and none provides meeting cycles, a Git-backed history, issue synchronization or publishing to Discord.

## Decision

- Build an own application rather than fork an existing one. Use Meetily (MIT) as a reference and reuse individual fragments with attribution.
- Tauri 2 with Rust for the shell, recorder, queue, storage and publishers.
- React with TypeScript for the UI.
- A Python sidecar for model inference, started only for processing, with its environment managed by `uv`.
- SQLite for local state; Git for shared state.

## Consequences

- The recorder has full control over resource use, and no live-transcription code has to be removed.
- Two languages for backend logic (Rust and Python) with JSON Schemas as the contract between them.
- The Python environment is large and is created on first use rather than bundled.

## Alternatives

- **Fork Meetily:** faster start, but requires reworking the live-first design and data model and tracking a very active upstream.
- **Electron:** larger footprint for an always-on tray application.
- **Python-only desktop app:** simpler model integration, weaker fit for low-overhead capture and a native tray experience.
- **Rust-only inference:** avoids the sidecar, but diarization and the reference ASR tooling live in the Python ecosystem.

## Licenses and requirements

Tauri: MIT or Apache-2.0. Windows 10 version 2004 or newer, subject to Stage 0 verification.
