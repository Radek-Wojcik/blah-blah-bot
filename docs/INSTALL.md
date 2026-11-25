# Installing Blah Blah Bot on Windows

The release is one installer, `Blah-Blah-Bot_<version>_x64-setup.exe`, published as a GitHub release together with `SHA256SUMS.txt`. It installs for the current user only (no administrator rights, no Program Files). The Python environment with the models' runtime (several GB) is not part of the installer: the application creates it with `uv` on first use, as decided in [ADR 0015](decisions/0015-processing-pipeline.md).

## What the installer contains

| Item | Location after installation |
|---|---|
| The application | install folder |
| `worker/` (Python sources, `pyproject.toml`, `uv.lock`) | install folder |
| `schemas/` (JSON schemas the worker validates against) | install folder, next to `worker/` |
| `bin/uv.exe` (pinned release, checked against its published SHA256 at build time) | install folder |
| `NOTICE.txt`, `LICENSE.txt` | install folder |

It contains no models, no meeting data and no secrets. The build script (`scripts/build-installer.ps1`) lists the payload in `CONTENTS.txt` and fails when a model file, an environment file, a key, a token or an absolute path of the build machine is found.

The installer assumes the Microsoft Edge WebView2 runtime is already present (`webviewInstallMode: skip`). Windows 11 and current Windows 10 include it by default; if it is missing on an older system, install the Evergreen WebView2 Runtime from Microsoft before launching the application.

## Verify the download

Compare the checksum with `SHA256SUMS.txt` from the same release:

```powershell
Get-FileHash .\Blah-Blah-Bot_<version>_x64-setup.exe -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
```

The two hashes must be identical.

## SmartScreen

The installer is **not signed** by default ([ADR 0024](decisions/0024-installer-signing.md), status Proposed). Windows SmartScreen therefore shows "Windows protected your PC" when the file is run. After verifying the checksum, choose "More info", then "Run anyway". An organization's policy can remove the "Run anyway" choice; then the installer cannot be run on that machine. On Windows 11 with Smart App Control switched on, an unsigned installer can be blocked outright; this has not been tested.

## Windows Defender and false-positive checks

Because the installer is unsigned, Windows Defender's machine-learning heuristics can occasionally flag a new build (for example `Bearfoos.A!ml` observed on an earlier LZMA-compressed build with the WebView2 bootstrapper). To reduce false positives, the installer uses `zlib` compression, skips downloading external bootstrappers, and embeds complete PE/NSIS version metadata.

To scan a locally built installer without adding exclusions or altering quarantine state, pass `-Scan` to the build script:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Scan
```

This runs `MpCmdRun.exe -Scan -ScanType 3 -File <installer> -DisableRemediation`. If a release binary still triggers a false positive, submit it to Microsoft Security Intelligence (WDSI) for analysis rather than adding Defender exclusions.

### Defender scan log

| Date | Version | Compression / WebView2 | Result | Notes |
|---|---|---|---|---|
| 2026-11-25 | 0.7.0 (pre-WP-02) | `lzma` / `downloadBootstrapper` | Flagged (`Trojan:Win32/Bearfoos.A!ml`) | Unsigned local build; prompted WP-02 changes |
| — | 0.7.0 (post-WP-02) | `zlib` / `skip` | Pending | Re-verify after next full installer build |

## Building the installer

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

Needs Rust, Node 22, Python 3 (for the version check) and 7-Zip (to inspect the result). Output is under `target\installer\`: the installer, `SHA256SUMS.txt` and `CONTENTS.txt`. The release workflow (`.github/workflows/release.yml`) runs the same script when a tag `vX.Y.Z` is pushed and creates a **draft** release; publishing it is a manual step.

Before tagging, the version must be identical in `Cargo.toml`, `Cargo.lock`, `crates/app/tauri.conf.json`, `ui/package.json` and `ui/package-lock.json` (`python scripts/check_versions.py --tag vX.Y.Z`), and `CHANGELOG.md` needs a non-empty `## [X.Y.Z]` section (`python scripts/release_notes.py X.Y.Z`).

Only an NSIS installer is built. An MSI installs per machine into Program Files, which is read-only for the user; the worker environment has not been tested from there, so MSI is not offered.

## Clean-machine test (manual)

Run this before every release, on a Windows 10 and a Windows 11 machine that has never had the application, with no Python, `uv` or `llama-server` installed. Do it in a virtual machine, in Windows Sandbox or on a spare computer; never on a machine whose settings or data matter.

1. Copy the installer and `SHA256SUMS.txt` to the machine. Verify the checksum (above).
2. Run the installer. Expect the SmartScreen prompt (above). Choose the current-user installation. Expect: no administrator prompt; the app appears in the Start menu.
3. Also verify the writable-folder guard: try choosing a read-only folder such as `C:\Program Files\Blah Blah Bot` on the directory page (or via `/D=`) and confirm the installer refuses it with a plain-language message and returns to the directory page.
4. Check the install folder (`%LOCALAPPDATA%\Programs\Blah Blah Bot`): `worker\`, `schemas\`, `bin\uv.exe`, `NOTICE.txt`, `LICENSE.txt` exist.
4. Start the application. Expect the main window and a tray icon. Open Settings and confirm the Processing section opens.
5. Start a processing job (or the setup, when the first-run wizard is present). Expect the environment to be created from the bundled `uv` without any tool installed on the machine, with visible progress. Without an NVIDIA GPU, expect the job to wait with a clear message instead of failing.
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
