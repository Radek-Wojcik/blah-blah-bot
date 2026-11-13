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
