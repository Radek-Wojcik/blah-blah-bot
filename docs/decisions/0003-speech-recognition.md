# 0003. Speech recognition

**Status:** Proposed (prototype P2)

## Context

Meetings are in Polish, in English, or in Polish with English technical terms. Published benchmarks rank the two candidate models differently depending on the data set, and there are no reliable measurements for mixed Polish and English speech. Whisper is known to translate instead of transcribe in some mixed-language cases.

## Decision

- Primary: faster-whisper with Whisper large-v3, word timestamps, cycle glossary passed as the initial prompt.
- Alternative: Parakeet-TDT-0.6B-v3, considerably faster, without prompt-based vocabulary.
- Transcription language is a cycle setting (`auto`, `pl`, `en`). The default is chosen from P2 results.
- The final model choice is made at the Stage 0 gate on own recordings.

## Consequences

- One GPU process of roughly 3–5 GB VRAM (estimate) for the primary model.
- A glossary can shift the output language; this is measured in P2 with and without the prompt.
- The ASR step sits behind an interface so the model can be replaced without touching later stages.

## Alternatives

- **WhisperX** as a wrapper for alignment: adopted only if word timestamps from faster-whisper prove too imprecise for quote links.
- **whisper.cpp:** avoids Python for this step, but the rest of the pipeline needs Python anyway.

## Licenses

Whisper and faster-whisper: MIT. Parakeet-TDT-0.6B-v3: CC-BY-4.0 (attribution in `NOTICE`).
