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

## Measurements (Stage 0, prototype P8)

The Python environment was created from `benchmarks/uv.lock` with `uv sync --frozen` and an empty uv cache, unattended, in a temporary directory (`benchmarks/results/p8-env-uv-lock-empty-cache.json`). The interpreter and the network connection were already present, so this approximates a first run and does not replace a test on a clean machine.

| Groups | Time | Environment size | Download cache | GPU seen by PyTorch |
|---|---|---|---|---|
| Speech recognition | 30 s | 3.0 GB | 3.0 GB | not checked |
| Diarization | 50 s | 4.6 GB | 4.6 GB | yes |
| Both | 70 s | 7.4 GB | 7.4 GB | yes |

The fallback in `docs/RISKS-AND-POC.md` (a prebuilt environment archive) is not needed on this evidence; model weights (several GB) come on top and are downloaded separately.

