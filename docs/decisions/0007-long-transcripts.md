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

## Measurements (Stage 0, prototypes P5 and P9)

- Extraction ran on whole 15-minute transcripts (about 2,100 words each) in one pass, with timestamps and a calendar in the prompt; the default model (Gemma 4 12B) needed 22.9 s per 1,000 words and met the thresholds (see ADR 0005). Overlapping windows, the reduce step and deduplication were not exercised in Stage 0.
- Quote verification worked: 100% of the quotes of the default model matched the transcript (98.6% for Qwen3 14B, 65% for Mistral Nemo, which also produced invalid JSON on 2 of 10 transcripts).
- Pipeline timing for 10 hours of audio (`benchmarks/results/p9-long-*.json`; the file repeats seven synthetic meetings, so it tests time and memory, not accuracy):

| Stage | Time | Basis |
|---|---|---|
| Transcription, Whisper turbo | 22.5 min (RTFx 26.7; 61,554 words) | Measured |
| Diarization, community-1 | 12.1 min (RTFx 49.5) | Measured |
| Extraction, Gemma 4 12B | 23.5 min | Extrapolated from 22.9 s per 1,000 words |
| Total | about 58 min | Limit 4 h |

- Peak VRAM above baseline: 1.5 GB (transcription), 2.3 GB (diarization), 8.9 GB (extraction, from P5); all below 11.5 GB. After a stage's model was released inside the same process, 285 to 326 MiB stayed allocated above the baseline (CUDA context), so stages should run in separate processes to return to the baseline.
- Not included: summary levels, vision, publishing, the first load of models, and any real 10-hour recording.
