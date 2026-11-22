# 0021. Model revision pinning

**Status:** Accepted

## Context

Models are downloaded from third-party Hugging Face repositories on first use ([ADR 0012](0012-model-licensing.md)). Without a revision, a download takes whatever the default branch points to at that moment, so a changed, replaced or tampered repository would reach users without any change in this project. A repository can also be renamed: the turbo Whisper model moved to a new owner, and the old name worked only through a redirect.

## Decision

- Every model in the worker's registry names one 40-digit commit of its repository (`revision`). Downloads and the offline lookup of an already downloaded model both use that commit, so the cache is keyed by it.
- The pinned commits are listed in [SECURITY-REVIEW.md](../SECURITY-REVIEW.md#model-revisions).
- Changing a pin is a reviewed change: read the repository history between the old and the new commit, re-check the license ([ADR 0012](0012-model-licensing.md)), run the benchmarks that the model affects, then update the registry, the table and `NOTICE`.
- A test checks that every configured model has a well-formed revision.

## Consequences

- A model downloaded before the pin is downloaded again once after the update.
- Repository updates, including security fixes of the files, reach users only with a new release of the application.
- Gated models still need the user's own token and acceptance of terms.
