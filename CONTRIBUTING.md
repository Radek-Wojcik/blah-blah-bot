# Contributing

## Ground rules

- Everything in the repository is in English: code, comments, documentation, commit messages.
- Test data is synthetic or scripted. Never commit real recordings, real meeting content, personal data or secrets.
- Model weights are never committed.

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): subject`, where type is one of `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`. Keep commits small and focused.

Check your messages locally before pushing:

```bash
python scripts/check_commits.py origin/main..HEAD
```

## Benchmarks

The benchmark harness lives in `benchmarks/`. See `benchmarks/README.md` for setup and for how to run each prototype. Unit tests for the metric code run on CPU without models:

```bash
cd benchmarks
uv run pytest
```

## Building and testing

Build from source with Rust 1.85 or newer and Node.js 22 or newer (commands in the README). `scripts/test-all.ps1` runs the checks and tests of every part in turn and cleans up after itself. Its suites are `rust` (format, clippy, tests), `worker` (pytest), `benchmarks` (pytest) and `ui` (typecheck, lint, format, unit tests and the mock end-to-end tests):

```powershell
scripts	est-all.ps1 -List                   # show the steps, run nothing
scripts	est-all.ps1 -Suite worker,benchmarks
scripts	est-all.ps1 -Skip ui
```

Tests that need a GPU or models, and the end-to-end tests against the real application, are not part of that script; run them only when a change calls for it (see [docs/TESTING.md](docs/TESTING.md)). Run each suite once per change. Before pushing, run `python scripts/check_files.py` and `python scripts/check_commits.py origin/main..HEAD`.

## Test hygiene

Tests must not leave anything behind. Thousands of leftover folders in the temp directory made a development machine take minutes to log in, so these rules apply to every contributor and every script or tool that runs tests in this repository.

1. **Nothing stays in the temp directory after a test, script or job.** Whatever creates temporary files or folders removes them itself: `try`/`finally`, `afterEach`/`afterAll`, `globalTeardown`, a `Drop` guard. This holds for failed, interrupted and killed runs too (rule 6).
2. **End-to-end data lives in the repository, not in `os.tmpdir()`.** Use `target/e2e-tmp/` (ignored by Git). `ui/support/e2e-tmp.ts` does this: `globalSetup` removes entries older than one hour, `globalTeardown` removes the folder of the run, `afterEach` removes what a test made.
3. **Do not create a new `worker-venv` (torch, CUDA) for each test.** The Python environment is shared and given to the application with `BBB_WORKER_ENV_DIR` (for example `worker/.venv`); a test that is not about the worker uses a stand-in. A test that installs tens of thousands of files is allowed only as a single, clearly marked heavy test that cleans up.
4. **Rust tests clean up on their own.** Hold a `tempfile::TempDir` for the whole test, or put manual folders in a guard with `Drop`. Tests that run Git (a repository leaves read-only object files that Windows will not delete) use `test_tmp::TestDir` from `crates/test-tmp`: it lives in `target/test-tmp`, clears the read-only attributes on drop and fails the test, naming the folder, when the folder cannot be removed. Never use `TempDir::into_path()`, `.keep()` or `std::mem::forget` on a test folder. Do not kill test processes (`taskkill`, a timeout without cleanup); if a run has to be aborted, run `scripts/clean-test-tmp.ps1` first. `scripts/test-all.ps1` additionally points `TEMP` and `TMP` of the test processes at `target/test-tmp` (do not set this in `.cargo/config.toml`: a missing folder would break compilation).
5. **`scripts/check-temp-hygiene.ps1` is part of "done".** It counts the folders `bbb-*`, `.tmpXXXXXX` and `ci-venv*` (and their files and size) in the temp directory, and everything in `target/test-tmp` and `target/e2e-tmp`. Run it at the end of every session before calling the work finished. Any folder is a failure, as are more than 500 files or 50 MB; clean up with `scripts/clean-test-tmp.ps1` and fix the cause.
6. **An interrupted run cleans up as well.** Scripts that run tests wrap them in `try { ... } finally { clean-test-tmp }` (PowerShell), `trap` (bash) or `globalTeardown` (Node), see `scripts/test-all.ps1`.
7. **Do not register scheduled tasks, autostart entries or services** without the explicit consent of the person whose machine it is. A registered job has a time limit, logs to a known folder, runs the hygiene check at the end, and is listed in the session summary together with how to remove it.
8. **Run heavy tests (GPU, torch, the full pipeline) only when needed, and once.** Check the hygiene before and after. Do not repeat the whole suite in a loop.
9. **Report the hygiene.** A session summary has the line `Temp after the session: <number of bbb-*/.tmp* folders>, <MB>; scheduled tasks added: <list or none>`.
10. **A test that leaves files behind is a bug**, not a detail to tidy up later. Fix it, and do not hide it.

Run everything in one go, with the cleanup and the check at the end:

```powershell
scripts\test-all.ps1
```

Short form, for reviewers: nothing is left in the temp directory (1, 4); end-to-end data goes under `target/e2e-tmp` (2); the worker environment is shared through `BBB_WORKER_ENV_DIR` (3); `scripts/check-temp-hygiene.ps1` runs at the end and reports zero (5, 9); no scheduled tasks, autostart entries or services from tests or tooling (7); heavy suites run once (8).
