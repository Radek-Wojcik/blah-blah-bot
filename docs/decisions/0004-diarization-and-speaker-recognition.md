# 0004. Diarization and speaker recognition

**Status:** Proposed (prototypes P3 and P4)

## Context

Meetings have 2 to 30 participants. Speakers must be separated, named with minimal effort, and recognized in later meetings of the same cycle. Diarization labels are arbitrary per recording, so remembering names across meetings requires comparing voices. Open diarization models degrade quickly as the number of speakers in one mixed track grows.

## Decision

- Diarize with pyannote.audio 4 and the `speaker-diarization-community-1` pipeline, using its exclusive (non-overlapping) output for alignment with the transcript.
- Never diarize the microphone track; it is the user.
- Bound the number of speakers with the cycle's participant list.
- Compute a voice embedding per detected speaker and match it against per-participant voice profiles of the cycle. Strong matches are assigned, weak matches suggested, the rest left unknown.
- Provide sample clips per detected voice so the user names voices by listening to a few seconds instead of searching the recording.
- Update profiles from confirmed labels.
- Store profiles and clips locally only.

## Consequences

- Naming is needed once per new voice; afterwards it is mostly confirmation.
- Short clips must outlive the full audio until labeling is finished.
- Voice profiles are biometric data and need explicit management and deletion functions.
- For large meetings in a single mixed track the result depends on profiles and manual merging; automatic separation is not promised.

## Alternatives

- **DiariZen:** stronger on several benchmarks, but its weights are licensed for non-commercial use only.
- **NVIDIA Sortformer:** limited to four speakers.
- **CPU-only clustering libraries:** attractive footprint, unreliable speaker counting above about seven speakers.
- **Per-user recording through a Discord bot:** perfect separation on Discord only; deferred beyond 1.0.

## Licenses and requirements

pyannote.audio: MIT. Pipeline weights: CC-BY-4.0, gated download with the user's Hugging Face token. Roughly 1–2 GB VRAM (estimate).
