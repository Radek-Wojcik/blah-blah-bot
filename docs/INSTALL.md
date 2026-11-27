# Installing Blah Blah Bot on Windows

The release is one installer, `Blah-Blah-Bot_<version>_x64-setup.exe`, published as a GitHub release together with `SHA256SUMS.txt`. It installs for the current user only (no administrator rights, no Program Files). The Python environment, speech models, `llama-server` and the default language model are not bundled in the installer; the first-run wizard downloads and sets them up ([ADR 0015](decisions/0015-processing-pipeline.md), [ADR 0026](decisions/0026-bundled-worker-and-first-run-downloads.md)).

## What the installer contains

| Item | Location after installation |
|---|---|
| The application (`app.exe`) | install folder |
| `worker/` (Python sources, `pyproject.toml`, `uv.lock`, `.python-version`) | install folder |
| `schemas/` (JSON schemas the worker validates against) | install folder, next to `worker/` |
| `bin/uv.exe` (pinned release, checked against its published SHA256 at build time) | install folder |
| `NOTICE.txt`, `LICENSE.txt` | install folder |

It contains no models, no meeting data and no secrets. The build script (`scripts/build-installer.ps1`) lists the payload in `CONTENTS.txt` and fails when a model file, an environment file, a key, a token or an absolute path of the build machine is found.

The installer downloads the Microsoft WebView2 runtime when the machine does not have it (Windows 11 and current Windows 10 have it). That one case needs an internet connection during installation.

### Downloaded on first run (pinned)

- **Python worker environment:** created by the bundled `uv.exe` from `worker/pyproject.toml` and `worker/uv.lock`.
- **Speech models:** pinned by 40-digit commit hash ([ADR 0025](decisions/0025-model-revision-pinning.md)); the gated speaker diarization model requires your own Hugging Face token.
- **`llama-server` (Windows CUDA 12.4):** pinned `llama.cpp` release `b12774` (`llama-b12774-bin-win-cuda-12.4-x64.zip`) and its matching CUDA runtime pack (`cudart-llama-bin-win-cuda-12.4-x64.zip`).
- **Default language model:** Gemma 4 12B Q4_K_M (`gemma-4-12B-it-Q4_K_M.gguf`) from pinned commit `2ae7d41be21ca62de00a2d320ee9cec50daa3aa6` of `bartowski/gemma-4-12B-it-GGUF`, or you can point the wizard to an existing local GGUF file.

> **Pin verification risk:** The pinned sizes and SHA256 hashes for `llama.cpp` `b12774`, its `cudart` pack and `gemma-4-12B-it-Q4_K_M.gguf` in `crates/app/src/downloads.rs` were **not verified by an actual download**. At runtime the downloader checks both exact byte size and SHA256 and fails closed (deleting the partial file) if either differs. If an upstream file does not match the pin, the first-run download will fail until the pin is verified and updated.

## Where things live

| Data | Location |
|---|---|
| Installed application and bundled worker payload | `%LOCALAPPDATA%\Programs\Blah Blah Bot` |
| Python worker environment | Local AppData (`worker-venv`) |
| `uv` download cache | Local AppData (`uv-cache`) |
| Pinned first-run downloads (`llama.cpp`, `cudart`, GGUF, `.part` files) | Local AppData (`downloads`) |
| Speech models cache | Hugging Face cache (or the configured models folder) |
| App settings, database and recordings | Roaming AppData |

No setup downloads or caches are written to `%TEMP%`.

## First-run wizard

On first start (or later from Settings), the wizard walks through:

1. **Welcome**
2. **Processing environment** (runs `uv sync` with the bundled `uv.exe`)
3. **Speech models** (downloads pinned speech models; prompts for a Hugging Face token for the gated speaker model)
4. **llama-server** (downloads pinned `llama.cpp` `b12774` + `cudart`, or picks an existing `llama-server.exe`)
5. **Language model** (downloads pinned Gemma 4 12B Q4_K_M, or picks an existing GGUF file)
6. **Screenshot analysis** (optional)
7. **Summary**
8. **First cycle**

Creating the Python environment and downloading the default models takes **10-30 minutes** and needs about **8 GB** of free disk space. Keep the PC on while it runs.

### Cancel, retry and offline behaviour

- **Cancel and retry:** Any running setup task can be cancelled. Retrying `uv sync` reuses what is already installed. Retrying a pinned binary or model download resumes from the saved `.part` file using HTTP `Range` requests (or restarts cleanly if the server does not support ranges).
- **Checksum or size failure:** If verification fails, the `.part` file is deleted and the download stops with an error so you can retry.
- **Offline behaviour:** If there is no internet connection, setup downloads show a plain offline message and can be retried once connected. After the environment and models are in place, recording and processing run offline.

## Removing the old Roaming `worker-venv` folder

Earlier builds stored `worker-venv` under Roaming AppData (`%APPDATA%\...\worker-venv`). Current versions use Local AppData instead and show a one-time notice if the old folder is present. The application **never deletes the old folder automatically**. Once the new environment in Local AppData is ready, you can delete the old Roaming `worker-venv` folder manually in File Explorer to reclaim disk space.

## Verify the download

Compare the installer checksum with `SHA256SUMS.txt` from the same release:

```powershell
Get-FileHash .\Blah-Blah-Bot_<version>_x64-setup.exe -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
```

The two hashes must be identical.

## SmartScreen

The installer is **not signed** by default ([ADR 0024](decisions/0024-installer-signing.md), status Proposed). Windows SmartScreen therefore shows "Windows protected your PC" when the file is run. After verifying the checksum, choose "More info", then "Run anyway". An organization's policy can remove the "Run anyway" choice; then the installer cannot be run on that machine. On Windows 11 with Smart App Control switched on, an unsigned installer can be blocked outright; this has not been tested.

## Building the installer

Always build installers with `scripts\build-installer.ps1`, **never** with plain `tauri build`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

`scripts\build-installer.ps1` stages the worker payload and pinned `uv.exe`, passes `crates/app/tauri.bundle.conf.json`, and sets `BBB_INSTALLER_BUILD=1` for the `tauri build` step. That turns on the bundle resource guard in `crates/app/build.rs`, which fails the build if `worker/pyproject.toml`, `worker/uv.lock` or `bin/uv.exe` is missing from `bundle.resources`. Plain `cargo run -p app --release` does not enforce this guard.

Needs Rust, Node 22, Python 3 (for the version check) and 7-Zip (to inspect the result). Output is under `target\installer\`: the installer, `SHA256SUMS.txt` and `CONTENTS.txt`. The release workflow (`.github/workflows/release.yml`) runs the same script when a tag `vX.Y.Z` is pushed and creates a **draft** release; publishing it is a manual step.

Before tagging, the version must be identical in `Cargo.toml`, `Cargo.lock`, `crates/app/tauri.conf.json`, `ui/package.json` and `ui/package-lock.json` (`python scripts/check_versions.py --tag vX.Y.Z`), and `CHANGELOG.md` needs a non-empty `## [X.Y.Z]` section (`python scripts/release_notes.py X.Y.Z`).

Only an NSIS installer is built. An MSI installs per machine into Program Files, which is read-only for the user; the worker environment has not been tested from there, so MSI is not offered.

## Clean-machine test (manual)

Run this before every release, on a Windows 10 and a Windows 11 machine that has never had the application, with no Python, `uv` or `llama-server` installed. Do it in a virtual machine, in Windows Sandbox or on a spare computer; never on a machine whose settings or data matter.

1. Copy the installer and `SHA256SUMS.txt` to the machine. Verify the checksum (above).
2. Run the installer. Expect the SmartScreen prompt (above). Choose the current-user installation. Expect: no administrator prompt; the app appears in the Start menu.
3. Check the install folder (`%LOCALAPPDATA%\Programs\Blah Blah Bot`): `worker\`, `schemas\`, `bin\uv.exe`, `NOTICE.txt`, `LICENSE.txt` exist.
4. Start the application. Expect the main window and a tray icon. Open Settings and confirm the Processing section opens.
5. Start the first-run wizard. Expect the environment to be created in Local AppData from the bundled `uv` without any tool installed on the machine, with visible progress. Without an NVIDIA GPU, expect the job to wait with a clear message instead of failing.
6. Download the speech models (needs the user's own Hugging Face token for the gated diarization model).
7. Uninstall from Settings, Apps. Expect the install folder to be removed. Application data and the worker environment are kept unless the uninstaller option to delete them is ticked; check what is left and note it.
8. Record the result per machine in the release checklist: Windows version and build, date, pass or fail per step, and the exact text of any error.

Limits of the test: a virtual machine or Sandbox has no CUDA, so it covers installation, the first run, environment creation and the "no GPU, waiting" path, not GPU processing.

### Windows Sandbox (Windows 10 Pro or Enterprise, Windows 11 Pro or Enterprise)

Windows Sandbox is an optional Windows feature that needs administrator rights and a restart to turn on (`Containers-DisposableClientVM`), and it is off by default. This project never turns it on for you. When it is on, save the following as `installer-test.wsb`, adjust the host folder to the folder that holds the installer, and open the file:

```xml
<Configuration>
  <Networking>Enable</Networking>
  <vGPU>Disable</vGPU>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>C:\path\to\folder\with\installer</HostFolder>
      <SandboxFolder>C:\Installer</SandboxFolder>
      <ReadOnly>true</ReadOnly>
    </MappedFolder>
  </MappedFolders>
</Configuration>
```

Everything in the Sandbox, including the installed application and the created environment, is discarded when its window is closed. Follow the steps above inside it. Networking is on because the first run downloads packages and models.

### Status

The clean-machine test has **not been done** for any version yet. Windows 10 (Sandbox) and Windows 11 (virtual machine or second computer) are both open.
