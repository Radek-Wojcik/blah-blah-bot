# Limitations

What this build does not do, and what has not been measured. Numbers come from the files in [benchmarks/results](../benchmarks/results) and were taken on one machine (RTX 3080 with 12 GB, Windows 10 build 19045). The test meetings are synthetic: scripted text and synthesized voices, so treat them as a comparison between choices, not as a promise for real meetings. The numbered gaps (G1 to G13) are the open items of [TRACEABILITY.md](TRACEABILITY.md).

## Platform

- **Windows only.** Recording uses Windows audio capture. Windows 10 was verified on build 19045 only; Windows 11 has not been tested, neither recording nor installation (G2).
- **NVIDIA GPU of the 12 GB class.** Processing needs an NVIDIA GPU and waits when the driver gives no readings. Peak video memory was 8.5 GB for a 2 hour recording and 9.7 GB (absolute) for the extraction of a 10 hour transcript. Smaller cards have not been tried.
- **Large first setup.** The Python environment is about 7.4 GB and models are downloaded on first use.
- **Unsigned installer.** Windows SmartScreen warns about it ([INSTALL.md](INSTALL.md)). The clean-machine installation test has not been done for any version yet.
- **Installer and publishing.** Only a per-user NSIS installer exists. Publishing to GitHub and Discord is not part of this build.

## Recording

- Sound from the speakers that the microphone picks up appears in both tracks; echo is not removed (G10).
- On the microphone track the audio engine flagged 194 discontinuities in a 10 hour run, about one every 3 minutes, although the audio was continuous and its length exact. The cause is unexplained and the zero-loss target is not met for the microphone. Application tracks had none (G5).
- Per-application capture was measured with test processes. Real Teams and Discord calls are not captured by hand, and the behavior of the new Teams client is unknown (G3).
- Region screenshots on several monitors with mixed scaling are tested with synthetic frames only (G7).

## Languages

- Polish and English, including mixed speech. Other languages are not supported.
- Whisper turbo on synthetic meetings: word error rate 8.9% Polish, 14.3% English, 16.4% mixed. The best recall of English terms inside Polish speech was 51% against a target of 85%, and the translation rate was 1.3% against a target of 1% (G1).
- The language of transcription is always detected automatically. The cycle's transcription language and glossary are stored but not applied to transcription; the glossary reaches only the extraction prompt (G9).
- In mixed-language meetings the model writes task text in the report language, so English tasks are translated.

## Speakers

- Speaker separation error rate on synthetic meetings was 20.5% to 23.3% for 2 to 4 speakers (target 15%, not met) and 23.4% for 8 speakers. These are measured on synthetic voices; real conference audio is not measured (G1).
- **Many speakers.** Meetings were measured up to 16 speakers, with error rates of 21.3% and 27.4%. Automatic separation of more than about 8 voices from one mixed track is not promised. The requirement of up to 30 participants is an open gap (G4): separate source tracks, the participant list and voice profiles are the mitigations, not a solution.
- Voice recognition was measured with synthetic voices: 100% of enrolled speech named correctly and no false accept among 16 new voices. On band-limited, noisy audio some different voices come close to the range of the same voice, so a suggestion should be checked (G1).
- The speaker count cannot be bound from the cycle's participant list; it is not applied (G9).
- A meeting processed after its audio was deleted has no sample clips, so voices are named from the transcript.

## Reports

- Extraction was measured on ten synthetic meetings (4 Polish, 3 English, 3 mixed). With the default model, Gemma 4 12B, task recall is 86.0% and precision 89.3%; decision recall is 49.9% and budget recall 54.3%, so part of the decisions and budget lines are not extracted. Per-meeting task recall is 43.5% to 100% on Polish and mixed meetings. Qwen3 14B misses the task recall, assignee and due-date targets (G1).
- Quotes are matched against the transcript, but quality on real recordings with recognition errors is not measured.
- Other model servers (Ollama, LM Studio) and the cloud path are tested against mock servers only (G12).
- Open issues of a cycle are read from a cache; the application cannot yet configure the GitHub token that fetching them needs.
- Screenshots are described and placed in the transcript by time. The report text does not embed images (G8).

## Scale

- Measured: a 2 hour recording processed in 6.1 minutes, and a 10 hour transcript extracted in 51.4 minutes. The 10 hour estimate for transcription, speaker separation and extraction together, about 86 minutes, is a sum of separately measured stages. The 10 hour end-to-end run is not done, and vision, publishing and model loading are not included (G11).

## Quality assurance

- The real-application end-to-end suite is unreliable in some sessions, so real-application behavior rests on mock suites plus manual runs (G6).
- There is no automated scan for hard-coded paths; they are checked by review (G13).
