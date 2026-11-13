# 0006. Screenshot description

**Status:** Proposed (prototype P6)

## Context

Region screenshots taken during a meeting show slides, charts and diagrams. Their content has to reach the language model as text, and the images are placed in the report by timestamp.

## Decision

- Describe each screenshot with Qwen3-VL-8B (GGUF Q4_K_M with projector) served by `llama-server`.
- Pass the transcript around the screenshot's timestamp as context.
- Always run this step locally, also when a cloud language model is chosen for the meeting.
- Add an OCR pre-step only if P6 shows invented or missing numbers on text-dense images.

## Consequences

- One additional model load of roughly 6–8 GB VRAM (estimate), sequential with the others.
- Descriptions are stored with the meeting and are editable in the draft.

## Alternatives

- **Dedicated OCR model only:** good for text, poor for charts and diagrams.
- **Vision through the configured LLM server:** possible when the user's server supports images; kept as an option, not the default.

## Licenses

Qwen3-VL: Apache-2.0.
