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
