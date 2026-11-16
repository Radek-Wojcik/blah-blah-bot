# 0005. Language model and backend

**Status:** Accepted (prototype P5, Stage 0 gate)

## Context

Extraction and summarization must run on a 12 GB GPU with 32 GB of RAM and work well in Polish. Users want to choose their own inference server. Cloud models are allowed only as a deliberate choice for a single meeting.

## Decision

- Talk to any server that exposes an OpenAI-compatible chat completions API.
- Default: a `llama-server` process managed by the application with an 11–14B model in GGUF Q4–Q5. Default: Gemma 4 12B Q4_K_M with the calendar in the prompt. Qwen3 14B is an optional alternative. Mistral Nemo is dropped from the candidates (see ADR 0012 for Bielik and Llama).
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

## Measurements (Stage 0, prototype P5)

Candidates run through `llama-server` (build b12366, CUDA 12.4) with Q4_K_M quantization, context 16384, JSON-schema constrained output, thinking disabled where the template supports it, and a 28-day calendar in the prompt. Data: ten sparse synthesized transcripts (4 Polish, 3 English, 3 mixed; five speakers, 15 minutes each) with planted tasks. Raw results: `benchmarks/results/p5-llm-*.json`. Bielik was dropped before the run (its terms conflict with meeting data; see `NOTICE`), and Llama is not a default because of its license (ADR 0012).

| Model | Valid schema | Task recall | Task precision | Assignee accuracy | Due-date accuracy | Quote validity | Seconds per 1000 words | Peak VRAM above baseline |
|---|---|---|---|---|---|---|---|---|
| Gemma 4 12B | 100% | 89.2% | 95.2% | 90.9% | 88.0% | 100% | 22.9 | 8,875 MiB |
| Qwen3 14B | 100% | 76.0% | 93.9% | 67.0% | 59.9% | 98.6% | 22.5 | 11,133 MiB |
| Mistral Nemo 12B | 80% | 27.8% | 68.0% | 43.3% | 40.4% | 65.0% | 45.1 | 9,719 MiB |

Against the thresholds (recall and precision 80% or more, assignee and due-date accuracy 85% or more, peak VRAM 11.5 GB or less, read as 11,776 MiB because the 12 GB card has 12,288 MiB):

- Gemma 4 12B meets every threshold.
- Qwen3 14B meets precision and VRAM (11,133 MiB, close to the limit) and misses recall, assignee and due-date accuracy.
- Mistral Nemo 12B meets none; on two of ten transcripts it produced output up to the token limit on all three attempts (no valid JSON). The cause was not isolated (model, chat template or server version).

Observations:

- Relative dates ("next Friday") are the weak point of every candidate; the calendar in the prompt was needed for any usable result. Resolving dates deterministically in code from a model-supplied phrase is still worth considering (ADR 0007).
- The data is synthetic and sparse; the task language is simple and explicit, so real meetings will be harder.

**Recommendation for the gate:** accept Gemma 4 12B Q4_K_M as the default, keep the calendar in the prompt, keep Qwen3 14B as an alternative for users who prefer it and drop Mistral Nemo from the candidate list. Confirm on a real meeting transcript before moving this ADR to Accepted.

## Gate decision (Stage 0)

Gemma 4 12B Q4_K_M accepted as the default. Confirmation on a real meeting transcript is a Stage 1 acceptance test; the default changes only if that test fails.

## Measurements (Stage 4, production steps)

The same ten synthetic transcripts (4 Polish, 3 English, 3 mixed; about 2,100 words each), now run through the production `extract` and `reduce` steps (windows, quote verification, deduplication) with `llama-server`, Q4_K_M, context 16384. Raw results: `benchmarks/results/stage4-extraction-*.json`.

| Measure | Gemma 4 12B | Qwen3 14B | Target |
|---|---|---|---|
| Schema validity | 100% (met) | 100% (met) | 100% |
| Task recall | 86.0% (met) | 71.7% (missed) | 80% or more |
| Task precision | 89.3% (met) | 94.1% (met) | 80% or more |
| Assignee accuracy | 92.8% (met) | 65.5% (missed) | 85% or more |
| Due-date accuracy | 92.2% (met) | 60.2% (missed) | 85% or more |
| Decision recall / precision | 49.9% / 90.5% | 40.4% / 77.8% | none |
| Budget recall / precision | 54.3% / 80.0% | 55.3% / 100% | none |
| Quote validity | 100% (met) | 99.4% (met) | 95% or more |
| Seconds per 1000 words (extract and reduce) | 41.3 | 46.3 | none |
| Peak VRAM above baseline | 8,867 MiB (met) | 11,129 MiB (met) | 11.5 GB or less (11,776 MiB) |

- Gemma 4 12B meets every threshold; Qwen3 14B misses task recall, assignee accuracy and due-date accuracy.
- An earlier prompt gave task recall 58.6% (Gemma 4 12B) and 58.4% (Qwen3 14B); the final prompt asks for the task text as an infinitive phrase and takes weekday dates from the calendar (`*-initial-prompt.json` keeps the first run).
- English meetings are near 100% with Gemma 4 12B; per-meeting task recall in Polish and mixed meetings ranges from 43.5% to 100%.
- Text-based assignee and due-date scores are pessimistic where different people have tasks with equal text; the paired metrics in the JSON files are higher (Gemma 4 12B: 99.5% and 98.4%).
- The data is synthetic and the task phrasing is simple; in mixed meetings the task text is written in the report language. Gemma 4 12B on a 10 hour transcript: see ADR 0007.

**Confirmed (Stage 4): Gemma 4 12B Q4_K_M stays the default.** It is the only candidate with a recorded Stage 4 run that meets all extraction thresholds; a model becomes the default only with a recorded run, and the recorded run exists for Gemma 4 12B.

