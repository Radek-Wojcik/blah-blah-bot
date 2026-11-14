# 0008. Browser tab capture

**Status:** Proposed (prototype P7)

## Context

Browser meetings and reference material (for example a video shown during a call) should be recorded per tab, without other tabs. Users also want to capture a whole tab group. Chrome permits tab capture only for tabs on which the user invoked the extension, and obtaining a stream requires a user gesture.

## Decision

- Manifest V3 extension: `tabCapture.getMediaStreamId` in the service worker, capture in an offscreen document, replay through an audio context so the tab stays audible.
- Stream PCM to the application over a WebSocket on `127.0.0.1`, paired with a one-time token.
- Tab groups: guided activation with one click or shortcut per tab, with status per tab and a prompt when a tab joins the group.
- Fallback: capture the whole browser as an application through process loopback.
- Detect Google Meet by tab URL and report it to the application.

## Consequences

- Fully automatic capture of a group is not possible; the per-tab action is a platform restriction.
- Each captured tab is its own track.
- The extension must handle service worker restarts and stream release timing.

## Alternatives

- **Native messaging for audio:** adequate for control messages, awkward for continuous binary streams.
- **`getDisplayMedia`:** requires a picker dialog for every capture.
- **Whole-browser capture only:** no extension to maintain, no tab selection.

## Requirements

Chrome 116 or newer.

## Measurements (Stage 0, prototype P7)

A Manifest V3 extension (`benchmarks/p7-chrome`) captured tab audio with `tabCapture` in an offscreen document, converted it to 16 kHz mono PCM in an AudioWorklet and streamed it over a WebSocket on `127.0.0.1` to a Python receiver (origin check and one-time token). Tests ran in the user's own Chrome 136 on Windows 10; tone pages served from localhost were captured. Raw results: `benchmarks/results/p7-chrome-*.json`.

| Run | Duration | Streams | Max gap | Lost frames | Result |
|---|---|---|---|---|---|
| Self-test (scripted) | short | 1 | under 100 ms | 0 | Incomplete: stop/restart and second tab shown, no 30 minute run |
| Manual run 1 (`long30`) | 30 min | 2 tabs (440 and 660 Hz) | 218 ms, the same instant on both tabs (about 23:12) | 0 | Failed the 100 ms limit; second tab added without restarting the first, stop and restart worked |
| Manual run 2 (`long30c`) | 30 min | 1 tab | 667 ms at about 10 minutes, then gaps of 52 to 635 ms around minute 15 | 0 | Failed the 100 ms limit |

What this shows and does not show:

- No frames were lost in either run (sequence numbers contiguous), so the gaps are delays in delivery or capture, not dropped data; a receiver that places frames by sample count loses no audio, but wall-clock alignment suffers.
- In run 2, audio duration (1,837.3 s) lagged wall time (1,839.3 s) by 2 s over 30 minutes, which is a clock-rate difference that the recorder's drift correction must also handle for tab streams.
- The cause of the gaps was not found. The machine was in normal use during both runs (other applications, audio device activity), so a system or browser scheduling effect is likely, but this is not established. A controlled run on an idle machine was not done.
- Tab audibility was confirmed by listening. Per-tab activation needed the user's click on the extension, as expected.
- Meet detection by tab URL, tab groups and service worker restarts were not exercised.

The 100 ms gap criterion is **not met**. **Recommendation for the gate:** keep the extension design, but do not accept it yet: either repeat the 30 minute run on an otherwise idle machine to see whether the gaps disappear, or relax the criterion to gaps that the receiver can bridge (sequence numbers contiguous, drift corrected) and record the gap length in the journal. This is a requirement decision for the project owner.
