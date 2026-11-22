# Security review

Review date: 2026-11-22. This page records what was reviewed, how, what was found and what was
done about it. To report a vulnerability, see `SECURITY.md`.

## Scope

Reviewed:

- The Stage 8 release branch (`main` plus the Stage 8 work): the desktop application (Tauri
  shell, IPC commands, settings, secrets), the Rust crates (queue, cycle store, store,
  recorder), the Python worker, the installer scripts and the CI and release workflows.
- The Stage 5 publish branch (`stage/5-publish`): publishing to GitHub and Discord, the HTTP
  client, the outbox, the secret references and the new UI links.
- A short look at the Stage 6 and Stage 7 branches (see "Other branches").

Not reviewed: third-party code (dependencies are covered by `docs/DEPENDENCY-AUDIT.md`), the
Windows operating system, and the security of the services the application talks to.

## Method

- Manual reading of the source, following each trust boundary in the threat model below:
  webview to Rust commands, application to worker process, application to `git`, application
  to the network, and the build and release pipeline.
- Targeted checks of the places where untrusted data meets a dangerous operation: process
  start, path handling, URL handling, logging, deserialization and model loading.
- Comparison of the preliminary findings of the release plan with the code, and a check of
  the fixes that were already made.
- Each finding was verified in the code before it was fixed. Tests were added where a test is
  practical (listed in the table).

## Summary

| Severity | Count |
|---|---|
| High | 0 |
| Medium | 1 |
| Low | 12 |
| Info | 10 |

No finding allows remote code execution without a preceding step by the user or a party that
already controls a repository, a model repository or the user's account.

## Findings

Status values: **fixed here** (on the Stage 8 branch), **fixed at merge (stage 5)** (the code is
on the Stage 5 branch and is fixed when that branch is merged), **accepted** (with the
reason), **open** (planned, not yet done).

| ID | Severity | Area | Finding | Status |
|---|---|---|---|---|
| F01 | Medium | Worker | Hugging Face downloads were not pinned to a commit. Fix: every model has a pinned 40-digit revision used by download and by the offline lookup; a test checks every configured model. | fixed here |
| F02 | Low | Queue | `HF_TOKEN` is passed to every worker process, not only the model download. | open (planned before 1.0) |
| F03 | Low | Worker | `llama-server` inherits the worker environment (in practice, `HF_TOKEN`). | open (planned before 1.0) |
| F04 | Low | Cycle store | `git` ran the hooks, `core.fsmonitor` and helpers named by the repository's own configuration. Fix: every call now sets `core.hooksPath` to the null device, `core.fsmonitor=false`, `protocol.ext.allow=never` and `protocol.file.allow=user`; a test checks that a hook and a monitor do not run. | fixed here |
| F05 | Low | Stage 5 publish | The publish token is in the environment of every `git` command of a publish, including commands that can start hooks. | fixed at merge (stage 5) |
| F06 | Low | Stage 5 publish | The authorization header is sent without `http.followRedirects=false`. | fixed at merge (stage 5) |
| F07 | Low | Stage 5 UI | Four links take their address from API responses without a scheme check. | fixed at merge (stage 5) |
| F08 | Low | Stage 5 shell | The stage 5 content security policy lacks the hardening directives of the release branch; the merge must keep them. | fixed at merge (stage 5) |
| F09 | Low | App | A stored `llama-server` or `uv` path that failed validation was still started. Fix: a path that fails the check is not returned for starting; a test covers it. | fixed here |
| F10 | Low | App | Without a bundled `uv`, the first `uv.exe` on the `PATH` is used. | open (planned before 1.0) |
| F11 | Low | App | A repository folder or recordings folder could be a network (UNC) or relative path. Fix: both must be absolute and on a local drive when they are set; tests cover UNC, device-namespace and relative paths. Values saved by earlier versions are not changed. | fixed here |
| F12 | Low | App, worker | A failed stage was logged at error level with the worker's message, which can quote meeting content. Fix: the log keeps the stage and the length of the message; the full text stays in the job record. The worker's schema messages no longer quote the offending value; a test covers it. | fixed here |
| F13 | Low | CI | The release workflow kept the workflow token in the checkout. Fix: checkouts in both workflows do not persist credentials; the release workflow has no default permissions and gives its one job `contents: write`, used only by the last step. | fixed here |
| F14 | Info | CI | Third-party actions are pinned to tags, not commit hashes. The CI token is read-only. | open (planned before 1.0) |
| F15 | Info | Worker | The local port of `llama-server` is chosen before the server starts and the server has no API key. Needs a process already running as the user. | accepted for now (local, same-user only) |
| F16 | Info | Stage 5 app | `BBB_LOG` can enable debug logging of the HTTP libraries in a release build. | fixed at merge (stage 5) |
| F17 | Info | Cycle store | `fetch_open_issues` has no caller; its HTTP agent does not require https. | open (remove or route through the shared client) |
| F18 | Info | App | Commands are not restricted per window; the style policy allows inline styles. Defense in depth only. | accepted (no webview content is untrusted today) |
| F19 | Info | Worker | The worker would send an API key over plain http if a caller set it; the application never does. | open |
| F20 | Info | Cycle store | Identifiers may be Windows device names. | open |
| F21 | Info | Stage 5 secrets | A `cred:` reference can name a secret stored for another cycle. | accepted for now (needs the user's approval of the plan, which names the channel) |
| F22 | Info | Stage 6 | An empty allow-list accepts any browser extension origin for pairing. | to be reviewed at merge |
| F23 | Info | App | Development overrides through environment variables are honoured in release builds. | open (planned before 1.0) |

Checks of the preliminary findings of the release plan: the settings path validation, the
external API key rule, the content security policy and the pinned `uv` download were
confirmed fixed earlier on this branch; the unpinned model downloads and the publish-token
exposure were confirmed and are covered above.

## Model revisions

Model files are pinned to one commit per repository. The commits were read from the public
Hugging Face API on 2026-11-22.

| Model | Repository | Pinned commit |
|---|---|---|
| Whisper large-v3-turbo | `dropbox-dash/faster-whisper-large-v3-turbo` | `0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf` |
| Whisper large-v3 | `Systran/faster-whisper-large-v3` | `edaa852ec7e145841d8ffdb056a99866b5f0a478` |
| Speaker diarization | `pyannote/speaker-diarization-community-1` | `3533c8cf8e369892e6b79ff1bf80f7b0286a54ee` |
| Vision (GGUF) | `Qwen/Qwen3-VL-8B-Instruct-GGUF` | `f982a07559d4a2f6c8744d840bf6fccab30eea96` |

The turbo repository moved from `mobiuslabsgmbh` to `dropbox-dash`; the API redirected the old
name to the new one at lookup time, so the registry now names the new owner.

Effect on existing installations: the cache is keyed by commit, so a model that was
downloaded before the pin is downloaded again once. Changing a pin is a reviewed change:
check the repository history between the old and the new commit, then update the table.

## Threat model

Assets: recordings, transcripts and voice profiles on the user's disk; the Hugging Face,
GitHub, Discord and language-model credentials; the user's Windows account.

Trust boundaries and what is assumed:

- **The user and the local machine are trusted.** Malware already running as the user can do
  anything the user can; the application does not try to defend against it. Findings that
  need such code are rated Info.
- **Cycle repositories are data that others can change.** A shared remote, a synced or shared
  folder and committed hooks are written by other people. The application treats a repository
  as untrusted: it runs `git` without its hooks and helpers, validates names and paths from
  cycle files, and does not accept network paths.
- **Model repositories are third-party.** Weights are downloaded only from the pinned
  commits, and processing runs offline afterwards.
- **The webview is not trusted with the system.** The window has no shell, file, http or
  opener plugin, a restrictive content security policy and no generated HTML; every command
  re-validates its input in Rust.
- **The worker is a separate process.** It is started without a shell, with a reduced set of
  secrets, and its output is validated against schemas. Its error text can contain meeting
  content, so logs keep only sizes and identifiers.
- **Remote language models and services are optional and explicit.** Cloud use needs a
  per-meeting confirmation naming the host; keys are stored in the Windows Credential
  Manager and are never sent over plain http to another machine.
- **The build and release pipeline** runs on tags only, creates a draft release, pins the
  bundled `uv` by checksum and installs Python packages from a lock file with hashes.

Out of scope: physical access, a compromised Windows account, and attacks on the services the
user chooses to connect.

## Other branches

- **Stage 6 (browser bridge):** adds a loopback WebSocket server, pairing and tab streams. To
  be reviewed at merge.
- **Stage 7 (integrations):** adds the Discord bot and Notion export configuration and a store
  schema change, on top of Stage 5. To be reviewed at merge; its decision record number
  collides with another record on the release branch and must be renumbered.

## Before the final merge

- Apply the Stage 5 fixes (F05, F06, F07, F08, F16) when that branch is merged.
- Review the Stage 6 and Stage 7 code, then add them to the table.
- Decide on the open items (F02, F03, F10, F14, F17, F19, F20, F23) before 1.0.
