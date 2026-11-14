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

## Measurements (Stage 0, prototype P2)

Reference machine: RTX 3080 (12 GB), Windows 10 build 19045. Data: three synthesized meetings
(Polish 3 speakers, English 3 speakers, Polish with English terms 4 speakers; 15 minutes each, voices
cloned from English reference voices, so Polish speech carries an English accent). Models ran with
int8_float16 (Whisper) and onnxruntime GPU (Parakeet). Raw results: `benchmarks/results/p2-asr-*.json`.

| Configuration | WER Polish | WER English | WER mixed | Term recall mixed | Translation rate mixed | RTFx | Peak VRAM above baseline |
|---|---|---|---|---|---|---|---|
| large-v3, auto language, no glossary | 11.5% | 14.7% | 29.6% | 51% | 10.8% | 6.8 | 1,910 MiB |
| large-v3, auto language, glossary | 17.6% | 15.2% | 31.3% | 34% | 13.1% | 4.4 | 1,940 MiB |
| large-v3, forced language, no glossary | 19.1% | 18.6% | 30.7% | 43% | 11.5% | 4.6 | 1,948 MiB |
| large-v3, forced language, glossary | 16.9% | 15.3% | 25.5% | 35% | 11.7% | 4.9 | 1,912 MiB |
| turbo, auto language, no glossary | 8.9% | 14.3% | 16.4% | 35% | 2.4% | 35.8 | 214 MiB |
| turbo, auto language, glossary | 9.5% | 15.0% | 19.7% | 37% | 1.3% | 34.6 | 277 MiB |
| turbo, forced language, no glossary | 8.9% | 14.3% | 16.4% | 35% | 2.4% | 35.9 | 214 MiB |
| turbo, forced language, glossary | 9.5% | 15.0% | 19.1% | 37% | 1.6% | 34.7 | 274 MiB |
| Parakeet-TDT-0.6B-v3 (no glossary support) | 15.0% | 11.4% | 16.8% | 21% | 25.8% | 15.1 | 12 MiB |

Against the thresholds in `docs/TESTING.md`:

- Polish WER (15% or less): met by turbo in every configuration and by large-v3 only without glossary and with automatic language; Parakeet sits on the limit.
- Mixed WER (20% or less): met by turbo and Parakeet; not met by large-v3 in any configuration.
- Term recall (85% or more): not met by any configuration; the best is 51%.
- Translation rate (1% or less): not met; the best is 1.3% (turbo with glossary). Segment language detection on short segments is noisy, so these rates are indicative.
- Speed (10 hours of audio in 60 minutes or less, which needs RTFx 10 or more): met by turbo (about 17 minutes) and Parakeet (about 40 minutes); not met by large-v3 (88 to 136 minutes).

Observations:

- Whisper turbo was both faster and more accurate than large-v3 on this data. Part of the large-v3 result is its tendency to translate or switch language.
- Forcing the language did not change turbo's result; automatic detection loses nothing for this model, while large-v3 was best with automatic detection.
- The glossary lowered term recall for large-v3 (51% to 34%) and did not help turbo; it should stay optional.
- The test audio is synthetic with an English accent on Polish text, so absolute numbers (especially term recall of English words said inside Polish sentences) are not a prediction for real meetings. The ranking should be confirmed on a real recording before the choice is final.

**Recommendation for the gate:** switch the primary model to Whisper large-v3-turbo with automatic language (no glossary by default); keep Parakeet as the fast alternative; keep large-v3 only as an option for single-language audio. Term recall and translation rate need a decision: keep the requirements and confirm on real recordings, or lower them.
