# Dependency audit

Date: 2026-11-21 (the `url` advisory was added on a re-run on 2026-11-22). Scope: release 0.5.0 plus the crates added by the Stage 5 branch. Windows 10 x64 is the only supported target.

## How to run

```powershell
pwsh scripts/audit-deps.ps1            # all checks
pwsh scripts/audit-deps.ps1 -SkipPython
```

The script is read-only. It writes reports to `target/audit/` and removes its temporary folder and its `uv` cache when it ends. Only package names and versions are sent to the advisory databases (osv.dev, npm, PyPI). It exits 1 on a finding that is neither fixed nor listed under accepted below.

| Area | Tool used | Notes |
|---|---|---|
| Rust advisories | `cargo deny` or `cargo audit` when installed, else osv.dev (carries the RustSec database) | Neither cargo-deny nor cargo-audit was installed here. `deny.toml` is committed but was not run locally. CI installs both (job `audit`). |
| Rust licenses | `cargo metadata --filter-platform x86_64-pc-windows-msvc` | Reachable through normal dependencies, workspace crates excluded. |
| UI | `npm audit --omit=dev --audit-level=high`, plus a full `npm audit` | No install needed, the lock file is enough. |
| Worker, benchmarks | `uv export --frozen --no-hashes --no-dev`, then `uvx pip-audit --no-deps --disable-pip` | pip-audit 2.10.1 in an ephemeral environment under `target/audit`. |

Tool versions: cargo 1.100.0, npm 11.20.0, node 26.13.1, uv 0.11.15, pip-audit 2.10.1.

## Stage 5 branch additions

`git diff main..origin/stage/5-publish` over all `Cargo.toml` files adds one workspace crate, `publishers`, and moves two dependencies:

- `ureq 3.5` (`default-features = false`, `native-tls-no-default`). Already in the lock file through `cycle-store` on main, so no new third-party crate enters the build. TLS is Windows Schannel (`native-tls`, `schannel`); there is no OpenSSL and no rustls.
- `tempfile 3`: optional, only behind the `test-support` feature and as a dev dependency. Never part of a release build.
- `store` swaps `tempfile` for `serde_json`; `worker/pyproject.toml` loses three lines (no dependency change).

`Cargo.lock` of that branch differs from main only by the `publishers` entry. Re-run the audit after the merge.

## Findings

| # | Area | Finding | Severity | Disposition |
|---|---|---|---|---|
| 1 | Rust | `url 2.5.8`: RUSTSEC-2026-0592, stack overflow in `Url::origin()` on deeply nested `blob:` URLs | Medium (denial of service, needs untrusted URLs) | Fixed: `cargo update -p url --precise 2.5.9` (also `form_urlencoded` 1.2.3). `cargo check --workspace --locked` passes. |
| 2 | Rust | `chrono 0.4.45`: RUSTSEC-2026-0543, unmaintained notice (informational, no vulnerability). Pulled in by `serde_with` inside the Tauri stack | Informational | Accepted. Not a direct dependency, nothing to bump; revisit when Tauri drops it. |
| 3 | Rust | `glib`, `proc-macro-error`, `zbus_names`, `zvariant`, `zvariant_derive` have notices in `Cargo.lock` (RUSTSEC-2024-0429, -2024-0370, -2026-0371, -0370, -0372) | Low | Not applicable: Linux-only crates of Tauri, not in the Windows build (`cargo metadata --filter-platform`). `deny.toml` restricts the graph to the Windows target. |
| 4 | UI | `minimatch` 7.0.0 - 11.0.0 via `typescript-eslint` 8.72.2: GHSA-g3hv-75wg-m249, ReDoS in glob matching. Seven moderate advisories, all one chain | Moderate, dev tooling only | Fixed: `typescript-eslint` 8.73.0 (`package.json` and lock file). `npm audit` reports 0 for all dependencies. Lint was not re-run. |
| 5 | Worker | `torch 2.11.0`: PYSEC-2025-194 (CVE-2025-3000), memory corruption in `torch.jit.script`; fix in 2.13.0 | Medium (local attack surface) | Accepted. The worker never calls `torch.jit.script`; no newer torch exists on the pinned `cu128` index (`uv lock --upgrade-package torch --dry-run` finds nothing). Revisit when a newer cu128 build is published. |
| 6 | Worker | `setuptools 81.0.0`: PYSEC-2026-3447 (CVE-2026-59890, sdist glob on macOS) and GHSA-xmj3-9gf7-h52w (legacy wheel install path on Windows); fixes in 83.0.0 and 84.1.0 | Low | Accepted. setuptools is a dependency of torch and is not imported at run time. Raising it makes uv downgrade torch to 2.10.0 (`uv lock --upgrade-package "setuptools>=84.1.0" --dry-run`), so torch's pin holds it back. Both paths need building a source distribution or installing an attacker-made legacy wheel, which the application never does. |
| 7 | Benchmarks | `benchmarks/uv.lock`: no advisories | None | - |
| 8 | TTS tool (`benchmarks/tools/tts`) | 82 advisories in 7 packages (torch 2.6.0, pillow, transformers 5.2.0, starlette, gradio, diffusers, setuptools) | Not rated | Accepted. Development tooling in a separate environment, pinned by Chatterbox (it needs torch 2.6). It generates synthetic audio from our own text; it is not shipped and processes no untrusted input. Not part of the script. |

The ignore list for the worker (findings 5 and 6) lives in `$AcceptedPython` at the top of `scripts/audit-deps.ps1`.

## Licenses

Rust, Windows build, 296 third-party crates: no GPL, AGPL, LGPL or unknown license. Five crates are MPL-2.0 (`cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext`): file-level copyleft, used unmodified, source available on crates.io, so there is no obligation beyond naming them (done in `NOTICE`). 18 crates are Unicode-3.0 (the ICU family). `rusqlite` compiles SQLite, which is public domain.

UI shipped set: react, react-dom, scheduler (MIT), `@tauri-apps/api` (Apache-2.0 OR MIT). Dev tooling includes 12 MPL-2.0 and 3 BlueOak-1.0.0 packages; none is shipped.

Worker: all direct dependencies are MIT, BSD-3-Clause or Apache-2.0, apart from the NVIDIA CUDA wheels (proprietary, installed by the user's own `uv sync`, not redistributed).

Discrepancy to know about: the PyAV wheels (`av`, pulled in by faster-whisper) bundle FFmpeg. The upstream build links `x264` and `x265`, which are GPL libraries, so the bundled FFmpeg is probably GPL as a whole, although the PyAV package itself is BSD-3-Clause. This is not a problem for this project because the wheels come from PyPI into the user's environment and are not redistributed. It would become one if the installer ever shipped the worker environment. Recorded in `NOTICE`; the exact FFmpeg configuration was not verified.

## Model licenses (checked online 2026-11-21)

Source: Hugging Face model metadata and model cards. ADR 0012 expected permissive licenses; all agree except where noted.

| Model | Expected (ADR 0012, NOTICE) | Found | Result |
|---|---|---|---|
| Whisper large-v3 (Systran) | MIT | MIT | Match. |
| Whisper large-v3-turbo (mobiuslabsgmbh) | MIT, unverified in Stage 2 | MIT, now verified. The repository has moved to `dropbox-dash/faster-whisper-large-v3-turbo`; the old name redirects. The card says it is a CTranslate2 conversion of `openai/whisper-large-v3-turbo` (MIT). | Verified. Discrepancy: the registry in `worker/src/bbb_worker/models.py` names the old owner. It works through the redirect today; pointing it at the new owner is advisable before 1.0 so a removed redirect cannot break first run. Left to the owner of that file. |
| pyannote community-1 | CC-BY-4.0, gated | `cc-by-4.0`, gated (automatic approval), gate form asks for a use case. The card itself is only readable after login, so the exact citation text could not be read. | Match. `NOTICE` carries a generic CC-BY attribution; compare it with the card text after login before 1.0. |
| Qwen3-VL-8B-Instruct and its GGUF repository | Apache-2.0 | Apache-2.0 | Match. |
| Qwen3-14B | Apache-2.0 | Apache-2.0, `LICENSE` file present | Match. |
| Gemma 4 12B instruction-tuned | Apache-2.0 | Metadata and card: Apache-2.0 | Match with a caveat. The repository has no `LICENSE` file, and the license link on the card (`ai.google.dev/gemma/docs/gemma_4_license`) redirects to `gemma.ai`, a domain that cannot be confirmed as Google's. The page there shows the standard Apache-2.0 text. Confirm on Google's own site before 1.0. The Q4_K_M GGUF is a third-party conversion chosen by the user. |
| Parakeet-TDT-0.6B-v3 | CC-BY-4.0 | CC-BY-4.0, "governing terms" in the card | Match. Used for benchmarks only. |
| Mistral-Nemo-Instruct-2407 | Apache-2.0 | Apache-2.0 | Match. Not a candidate. |
| Chatterbox (test tooling) | MIT | MIT | Match. |
| llama.cpp | MIT | MIT (GitHub) | Match. The user installs it; the installer does not carry it. |
| uv | MIT OR Apache-2.0 | GitHub reports Apache-2.0 for the repository; the project is dual-licensed | Match. |

Bielik 11B v3, Llama, DiariZen, Coqui XTTS and MMS-TTS stay excluded (reasons in `NOTICE` and ADR 0012).

## Not run

- `cargo deny` and `cargo audit`: not installed, and installing them means a long compile. The script falls back to osv.dev. `deny.toml` has not been exercised.
- `cargo about` and the `third-party-licenses.html` for the installer resources: not installed, not generated. `NOTICE` carries the direct dependencies and the per-license summary; the per-crate list is `target/audit/rust-licenses.txt`.
- The audit has not run on the merged Stage 5 to 7 tree.
