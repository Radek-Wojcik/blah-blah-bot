# 0007. Long transcripts and structured extraction

**Status:** Accepted

## Context

A 10-hour meeting produces a transcript far beyond the context of a model that fits the reference GPU. Reports must be reliable enough to publish to a team.

## Decision

- Map-reduce over transcript windows of 15–20 minutes with overlap.
- The model returns only structured data validated against versioned JSON Schemas.
- Every item carries a quote and a timestamp; quotes are verified against the transcript by fuzzy matching, and unverified items are flagged.
- Uncertain fields are labeled as stated, inferred or unknown.
- Deduplication and linking to open tasks and topics happen in the reduce step.
- Markdown is rendered by templates, never written by the model.

## Consequences

- Processing time grows linearly with meeting length.
- Hallucinated items are detectable through missing quotes.
- Report layout can change without re-running the model.

## Alternatives

- **Single pass with a long-context model:** simpler, not feasible on the reference hardware.
- **Free-text summaries:** easier prompts, no verification and no task synchronization.
