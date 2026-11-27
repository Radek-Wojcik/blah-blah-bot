# 0026. Bundled worker and first-run downloads

**Status:** Proposed

## Context

An earlier Windows installer shipped only `app.exe` without the `worker/` sources or `uv.exe`, because `crates/app/tauri.conf.json` has no `bundle.resources` and an unguarded build succeeded silently. Installed copies then failed to process meetings unless a developer set environment variables manually.

At the same time, storing the multi-gigabyte Python environment (`worker-venv`) in Roaming AppData was wrong for Windows roaming profiles, and non-technical users had no guided way to obtain `llama-server` and the default GGUF language model without manual downloads.

## Decision

1. **Bundle resource guard for installer builds only:** `crates/app/build.rs` runs `bundle_guard::check_bundle_resources` when `TAURI_CONFIG` is non-empty or `BBB_INSTALLER_BUILD == "1"`. It fails the build unless `bundle.resources` includes `worker/pyproject.toml`, `worker/uv.lock` and `bin/uv(.exe)` on disk. Plain `cargo run -p app --release` does not enforce the guard. `scripts/build-installer.ps1` sets `BBB_INSTALLER_BUILD=1` around `tauri build` (with `tauri.bundle.conf.json`) and restores it in a `finally` block.
2. **Local AppData for environment, cache and downloads:** `worker-venv`, `uv-cache` and first-run `downloads/` live under Local AppData (never Roaming AppData and never `%TEMP%`). If an older Roaming `worker-venv` exists, the app shows a one-time notice telling the user it can be removed manually; the app never deletes it automatically.
3. **Pinned first-run downloads with fail-closed SHA256 verification:** `crates/app/src/downloads.rs` pins the URL, exact byte size and SHA256 digest for `llama.cpp` `b12774` Windows CUDA 12.4 (`llama-b12774-bin-win-cuda-12.4-x64.zip`), its matching `cudart` pack (`cudart-llama-bin-win-cuda-12.4-x64.zip`) and the default language model (`gemma-4-12B-it-Q4_K_M.gguf` at commit `2ae7d41be21ca62de00a2d320ee9cec50daa3aa6` of `bartowski/gemma-4-12B-it-GGUF`). Files stream to `<name>.part` in Local AppData `downloads/`, are checked for exact size and streaming SHA256, and are renamed atomically only after both match. On any mismatch, the `.part` file is deleted and the download fails closed.
4. **Resumable and cancellable setup:** Downloads resume from an existing `.part` file with HTTP `Range` (`206 Partial Content`, restarting cleanly if the server replies `200`). Cooperative cancellation stops the transfer and keeps `.part` for a later retry. `uv sync` is idempotent on retry.

## Consequences

- Installer builds produced via `scripts/build-installer.ps1` cannot silently omit the worker payload or `uv.exe`.
- Local development (`cargo run -p app --release`) stays fast and does not require staging resources.
- Large Python environments and model caches stay out of Roaming AppData and `%TEMP%`.
- First-run setup requires 10-30 minutes and about 8 GB of free disk space, but survives interruptions and works offline once finished.
- Existing users upgrading from older builds must remove the legacy Roaming `worker-venv` manually if they want that space back.

## Open points

- **Pins not yet verified by an actual download:** The pinned sizes and SHA256 hashes in `crates/app/src/downloads.rs` have not been verified against live downloads yet. Because verification fails closed, any mismatch will block first-run downloads until verified and updated.
- **Plain `tauri build` is not guarded:** Running `tauri build` directly without `TAURI_CONFIG` or `BBB_INSTALLER_BUILD=1` bypasses `build.rs`. Installers must always be built through `scripts/build-installer.ps1`.
- **Defender and SmartScreen:** False-positive detections and unsigned-binary warnings for the installer and bundled `uv.exe` are tracked separately under WP-02 ([ADR 0024](0024-installer-signing.md)).
