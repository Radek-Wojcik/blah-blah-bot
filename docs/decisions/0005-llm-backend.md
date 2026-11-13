# 0005. Language model and backend

**Status:** Proposed (prototype P5)

## Context

Extraction and summarization must run on a 12 GB GPU with 32 GB of RAM and work well in Polish. Users want to choose their own inference server. Cloud models are allowed only as a deliberate choice for a single meeting.

## Decision

- Talk to any server that exposes an OpenAI-compatible chat completions API.
- Default: a `llama-server` process managed by the application with an 11–14B model in GGUF Q4–Q5. Default candidate: Bielik 11B v3 Instruct; comparison candidates include a Qwen3 model of similar size.
- Supported external servers: Ollama, LM Studio, Strata and others with a compatible API.
- Cloud models are selected per meeting, confirmed in a dialog, and never used as a fallback. Only text is sent.
- A model becomes the default only with a recorded run on the comparison set.

## Consequences

- The application is independent of any single inference engine.
- Structured output relies on JSON Schema or grammar support where the server provides it, with validation and retry everywhere else.
- Strata's main model does not fit the reference machine in good quality, so Strata is supported but neither default nor part of release benchmarks.

## Alternatives

- **Embedding the model runtime in the application:** fewer moving parts, less choice for the user.
- **Larger models with partial offloading:** better quality, much slower on 12 GB.

## Licenses and requirements

llama.cpp: MIT (to confirm in the Stage 0 audit). Bielik 11B v3: Apache-2.0 with publisher terms of use. About 8–11 GB VRAM with 16–32k context (estimate).
