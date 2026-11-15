# 0002. Audio capture and recording format

**Status:** Accepted (prototype P1, Stage 0 gate)

## Context

Recordings must contain only the chosen applications, keep the microphone separate, survive crashes over ten hours and cost almost nothing while running.

## Decision

- Capture through WASAPI with the `wasapi` crate: one client for the microphone and one application-loopback client per selected process, including its process tree.
- Offer capturable applications from the list of audio sessions.
- Store every track separately as 16 kHz mono 16-bit PCM in 60-second segments, with an append-only session journal.
- Keep a shared session clock and a start offset per track so that sources can be added and removed during a recording.

## Consequences

- Music and notifications from other applications never enter the recording.
- Disk use is about 115 MB per track per hour until the audio is deleted after processing.
- A crash loses at most the segment being written.
- Process loopback has API restrictions (fixed format, some client methods unavailable) that the recorder has to work around.

## Alternatives

- **System-wide loopback:** simple, but records everything.
- **Virtual audio device:** flexible routing, but requires driver installation and user setup.
- **FLAC segments:** smaller files; evaluated in P1 and adopted if the CPU cost is negligible.
- **Own C++ module based on the Microsoft sample:** more control, more maintenance.

## Licenses and requirements

`wasapi`: MIT. Application loopback is documented for Windows 10 build 20348 and reported to work from version 2004; the minimum supported build is fixed by P1.

## Measurements (Stage 0, prototype P1)

A Rust recorder (`benchmarks/p1-audio`) captured the microphone and per-process application loopback on Windows 10 Pro build 19045 (RTX 3080 machine, desktop in normal use). Raw result: `benchmarks/results/p1-audio-win-19045.json`.

| Criterion | Target | Measured | Result |
|---|---|---|---|
| Dropped buffers | 0 in 2 h | 0 in a 2 h run with three silent processes (652 over all short runs on a virtual cable; see below) | Met |
| Track isolation | other sources at -40 dB or lower | -63 dB or lower (three player processes, process tree, Chrome) | Met |
| Alignment between tracks | 50 ms or less | 27 ms worst case in the click test; drift 0.19 ms over a 12 minute run, projected 1.9 ms over 2 h with correction (390 ms without) | Met |
| CPU | 3% or less | 0.03% or lower | Met |
| Memory | 200 MB or less | 10 MB peak | Met |
| Audio lost when the recorder is killed | 60 s or less | 0.25 s | Met |
| Minimum Windows build | works on the minimum supported build | Application loopback activated and recorded on build 19045 | Met |

Notes and limits:

- The documented minimum for application loopback is build 20348; it works on build 19045, which extends the supported range. Windows 11 was not tested (no second machine).
- In `wasapi` 0.27 the parameter named `include_tree` selects process-tree inclusion when true; with false the client records everything except the tree (exclude mode). Both modes work and were verified with tones.
- The loopback signal is taken after the application's own session volume.
- Clock drift between the microphone and loopback is real (about 390 ms over 2 h uncorrected) and is removed by resampling against the performance counter; the correction is required, not optional.
- Drops reported when a virtual cable is used as a microphone (about one flag per 22 ms while audio plays) are an artifact of that driver and were excluded from the dropped-buffer criterion.
- Manual tests with real Teams and Discord calls were not run (Teams was not available, Discord was not in a call during the session).
- FLAC was measured offline with ffmpeg, not in the recorder: 47% of the PCM size for microphone noise, 77% for white noise and under 1% for silence. Real speech was not measured. FLAC is not implemented in the recorder.

**Recommendation for the gate:** accept the design (WASAPI process loopback, 16 kHz mono PCM segments, journal, drift correction). Keep PCM for the first release (about 115 MB per track per hour); revisit FLAC when real speech sizes are known. Treat Windows 11 and the real Teams and Discord checks as Stage 1 acceptance tests.

## Gate decision (Stage 0)

Accepted as recommended. Windows 11 and the real Teams and Discord checks stay open as Stage 1 acceptance tests; PCM is the format for the first release.

## Measurements (Stage 1, application recorder)

A 10 hour soak with the real WASAPI backend (`crates/recorder/examples/soak.rs`) on Windows 10 build 19045, 32 logical processors, recording the default USB microphone and two test player processes that render tones through a virtual cable while the machine was in normal use. Raw result: `benchmarks/results/stage1-soak-10h.json`.

| Criterion | Target | Measured | Result |
|---|---|---|---|
| CPU while recording three tracks | 3% of total CPU or less | 0.017% average (peak of the 30 s averages 0.018%) | Met |
| Memory | 200 MB or less | 10.1 MB peak working set | Met |
| Dropped buffers in 10 hours | 0 | Both application tracks: 0. Microphone: 194 engine-flagged data discontinuities (one about every 3 minutes, reported frame count 0) | **Not met on the microphone** |
| Continuity | no gaps | All three tracks: 601 segments, no missing or short segments, no offset mismatches; stored length within 0.1 s of the session length; no stretch of silence found around a flagged event in the one that was inspected | Met |
| Track alignment | 50 ms or less | Tracks start 2.6 ms (applications) and 16.2 ms (microphone) after the clock start; a source added mid-recording starts within 50 ms of the moment it was added (generated tones) | Met on tones; not measured against an external reference on real devices |
| Audio lost when the process is killed | 60 s or less per track | At most one segment (the one being written): generated tones, whole process killed after 3.4 s with 1 s segments; real application killed after 63 s with 60 s segments kept the closed segment | Met |

Notes and limits:

- The microphone flags look periodic (the interval averages 186 s). They are reported by the audio engine for this USB device; the recorded audio is continuous and the stored length is exact, so they did not show up as missing audio. The cause was not established (device driver, endpoint or system load are all possible), and no second microphone was tried. The run is reported as not meeting the zero-drop target for the microphone.
- The earlier 75 second trial run on the same device showed the first flag after 38 s, which fits the same pattern.
- A track that is removed and added again becomes a new track (`name-2`) so earlier segments are never overwritten.
- Windows 11, real Teams and Discord calls, and multi-monitor setups with different display scaling were not tested.
