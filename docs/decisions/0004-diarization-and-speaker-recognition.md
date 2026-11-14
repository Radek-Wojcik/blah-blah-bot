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

## Measurements (Stage 0, prototypes P3 and P4)

`pyannote/speaker-diarization-community-1` (pyannote.audio 4.0.7, GPU) on five synthesized meetings of 14 to 23 minutes, and a speaker recognition test on a pair of synthesized meetings. Raw results: `benchmarks/results/p3-diarize-community-1.json`, `benchmarks/results/p4-speaker-id-community-1.json`.

Diarization error rate (no collar, overlap included), speaker count bounded by the scripted number of speakers; the unbounded run found the same number of speakers in every meeting and gave identical results. "Clip reference" is the reference of whole synthesized turns; those turns contain silence, so a second reference was cut down to the stretches where the recording has sound (a simple energy detector, see `diar_common.voiced_reference`).

| Meeting | Speakers | DER, clip reference | DER, speech-only reference | Missed / false alarm / confusion (speech-only) |
|---|---|---|---|---|
| pl-03 | 3 | 29.4% | 23.3% | 4.9% / 12.0% / 6.4% |
| en-03 | 3 | 24.9% | 21.9% | 0.8% / 13.1% / 7.9% |
| mixed-04 | 4 | 22.0% | 20.5% | 1.1% / 15.4% / 4.1% |
| pl-08 | 8 | 25.7% | 23.4% | 2.9% / 13.8% / 6.7% |
| pl-16 | 16 | 27.4% | 21.3% | 2.5% / 12.4% / 6.4% |

- Thresholds: 15% or less for 2 to 4 speakers (**not met**: 20.5 to 23.3% on the speech-only reference, 22.0 to 29.4% on the clip reference); 30% or less for 6 to 10 speakers (met: 23.4% and 25.7%); 15 and more speakers reported only (21.3% and 27.4%, with all 16 speakers found).
- Speaker confusion is small (4 to 8%) in every meeting. The error is mostly false alarm, that is, speech assigned where the energy detector sees none; this may come from the detector being strict, from the pipeline marking low-level synthesized sound as speech, or from both. The cause was not isolated.
- Speed: 66 times faster than real time on 15-minute meetings (RTFx 66); 49.5 on a 10-hour file (12 minutes, see ADR 0007 and the pipeline timing below).
- Peak VRAM above baseline: 11.2 GB in the five-meeting run (the process kept its allocator cache between meetings) and 2.3 GB on the single 10-hour file. Run the model in its own process so that memory is returned.
- The pipeline returned a (speakers, 256) array of speaker embeddings, which P4 used.

Speaker recognition (P4): four enrolled voices (alex, kasia, jan, marta) from one synthesized meeting, tested on a second meeting with three of them and one unknown voice (ewa). Embedding dimension 256. With a cosine similarity threshold of 0.5 to 0.8, speaker accuracy for enrolled voices was 100% and the unknown voice was rejected (false accept rate 0%); at 0.3 and 0.4 the unknown voice was accepted. The threshold of 90% or more is met, but the test is easy: four voices cloned from distinct reference voices, one meeting each, no channel or microphone variation. A threshold of 0.5 or higher is the starting point, to be tuned on real recordings.

**Recommendation for the gate:** accept pyannote community-1 with embedding-based voice profiles for 6 or more speakers; for 2 to 4 speakers the 15% DER requirement is not met on synthetic data, so either confirm it on real recordings (where speakers are not cloned from similar voices and the reference is manual) or relax the requirement to about 25%. Keep using the participant list as a bound and the profile matching to name voices.
