# 0002. Audio capture and recording format

**Status:** Proposed (prototype P1)

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
