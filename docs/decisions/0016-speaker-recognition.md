# 0016. Speaker recognition: embeddings, thresholds, profiles and sample clips

**Status:** Accepted (Stage 3)

## Context

Stage 2 separates voices per recording ("Speaker 1", "Speaker 2"). Naming them once and recognizing them in later meetings needs a voice comparison, a rule for when a name may be applied without the user, a way to learn from the user's labels, and short audio samples so that naming takes seconds. Voice profiles are biometric data.

## Decision

- **Embeddings** come from the diarization pipeline (`speaker_embeddings`, 256 dimensions, one per detected speaker); no extra model is loaded. They are read from the diarization output by a stage that runs in the application process, so it needs no GPU.
- **Stages.** `speaker_id` (after `merge`) matches every detected voice with the known profiles; `clips` cuts sample clips from the audio and counts as audio-dependent, so the full audio is deleted only after it. Jobs created by Stage 2 get the new stages on demand.
- **Similarity** is cosine similarity between the voice embedding and the profile vector.
- **Thresholds** (all settings): automatic assignment at 0.75 or more **and** at least 0.10 ahead of the second best profile; suggestion at 0.60 or more; below that the voice stays unknown. A participant is assigned automatically to at most one voice of a meeting (the closest); another voice that matches the same participant becomes a suggestion. An unknown voice is never assigned silently.
- **Profile.** One sample per confirmed voice and meeting (embedding and speech time). The profile vector is the unit-length mean of the normalized sample embeddings, weighted by speech time capped at 60 s. At most 20 samples; when full, the shortest is replaced by a longer one. A voice with less than 5 s of speech is named but not learned.
- **Update rule.** Only confirmed labels teach a profile. An automatic match is applied to the transcript at once but teaches nothing until the user confirms it (one button for all automatic matches), so a wrong match cannot reinforce itself. Changing or removing a label removes the sample from the earlier name.
- **Clips.** Per voice up to three clips of 2.5 to 8 s: stretches of one speaker (turns closer than 0.4 s joined), with no other speaker within 0.5 s and at least 60% voiced frames, preferably 30 s or more apart, longest first. Clips are WAV files in the meeting folder. They are deleted when every voice of the meeting is decided, after 30 days at the latest (setting), and together with the meeting.
- **Naming.** The labeling screen supports an existing participant, a new participant, skipping (not a person), merging voices that are the same person, and unmerging. Names reach the transcript view as labels; the stored transcript is unchanged.
- **Participants.** A minimal local list stands in for the cycle's participants and is replaced in Stage 4.
- **Privacy.** Profiles, stored embeddings and clips live only in the local database and recordings folder. There is a function to forget one participant's voice, to delete a participant, and to delete all voice data. Nothing biometric is sent anywhere.

## Measurements

Raw data: `benchmarks/results/p4-similarity-community-1.json` and `benchmarks/results/stage3-speaker-recognition.json`.

**Similarity distribution** (seven synthetic meetings, speakers identified by their synthesized voice, pairs from different meetings):

| Condition | Same voice (pairs) | Different voice (pairs) |
|---|---|---|
| As synthesized | 48 pairs, 0.76 to 0.99, median 0.98 | 641 pairs, up to 0.64, 95th percentile 0.54 |
| 3.4 kHz band limit, noise, 8-bit quantization | 46 pairs, 0.68 to 0.99 | 617 pairs, up to 0.92, 95th percentile 0.68 |

Examples at the chosen thresholds (as synthesized): the same voice in two meetings at 0.78 (`michal`, pl-03 against en-03) and 0.79 (`pawel`, en-03 against mixed-04) is assigned; the closest different pair, 0.64 (`lukasz` in pl-08 against `tomek` in mixed-04), is below the automatic limit and is only a suggestion. Band-limited, a pair of different voices reaches 0.92 (`darek` in pl-08 against `bartek` in pl-16), which no threshold below 0.92 separates from the same-voice pairs; for such cases the margin rule and the confirmation step matter more than the number.

**Recognition over a series** (seven synthetic meetings in order with 2 to 16 speakers; after each meeting the true names are confirmed, which teaches the profiles; the production code and the default thresholds run on real models):

| Measure | Result | Target |
|---|---|---|
| Speaker accuracy for enrolled voices (speech time assigned automatically to the correct name) | 100% of 3302 s (no wrong, suggested-only or unknown outcome for an enrolled voice) | 90% or more |
| False accept rate (new voices named automatically) | 0 of 16 new voices (one was shown as a suggestion) | 2% or less |

These numbers describe clean synthesized voices. They are not a promise for real conference audio: the band-limited condition above shows overlaps, and real recordings (compression, echo, short turns, similar voices) are not yet measured.

## Consequences

- The first meeting needs naming; later ones need confirmation at most.
- Thresholds are tuned on synthetic data and are settings, to be revisited on real recordings.
- Clips need the audio, so a meeting processed with audio already deleted gets none; the user names voices from the transcript.
- Deleting the full audio waits for the clips stage, which is cheap.

## Alternatives

- **A separate speaker verification model:** better separation of similar voices is possible, but it adds a model, a download and VRAM; not needed for the measured data.
- **Learning from automatic matches without confirmation:** would improve profiles without effort but lets one mistake reinforce itself.
- **Fixed participant list only:** simpler, but new voices could never be named without a cycle; the local list also serves Stage 4.
