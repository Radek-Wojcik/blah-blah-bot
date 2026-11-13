# 0012. Model licensing and distribution

**Status:** Accepted

## Context

The project is MIT-licensed. Model weights carry their own licenses, some require attribution, some forbid commercial use, and some are gated.

## Decision

- Default and recommended models must be under licenses compatible with an MIT project (MIT, Apache-2.0, CC-BY-4.0).
- Weights are never part of the repository or the installer. They are downloaded on first use; gated models use the user's own token and acceptance of terms.
- `NOTICE` lists every dependency and downloadable model with license and attribution.
- Models under non-commercial licenses are excluded. Models under other custom licenses may be used through an external server the user configures; the application does not download or recommend them.
- Licenses are re-checked at every release.

## Consequences

- The repository stays small and free of third-party weights.
- First run requires network access and, for diarization, a Hugging Face account.
- Changing a default model requires a license check and a benchmark run.
