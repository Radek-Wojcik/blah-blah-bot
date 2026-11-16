# 0014. Recorder control, IPC conventions and region screenshots

**Status:** Accepted (Stage 1)

## Context

Stage 1 turns the audio prototype into an application: a tray widget starts and stops recordings, sources are added and removed while recording, and a hotkey captures a region of the screen with the offset of that moment. The UI must be testable without audio devices, and the real application must be testable end to end.

## Decision

**Session and journal.** A recording is a `Session` owning the session clock and the journal. Every source runs on its own capture thread with its own stop flag, so removing one source never touches the others. A removed source cannot be reused as the same track: adding it again creates a new track (`name-2`), so earlier segments are never overwritten. The database is filled by replaying the journal, both when a recording stops and when a crashed one is recovered, so there is one code path for both.

**Capture behind a trait.** `CaptureBackend` has a WASAPI implementation and a tone generator that goes through the same clock, writer and journal. Tests, demo mode and the end-to-end suite use tones; only the audio hardware is replaced.

**IPC.** Commands are snake_case and return `Result<T, IpcError>` with a stable `code` and a `message`. Payloads are camelCase JSON. Events (`recording-state`, `meetings-changed`) carry the complete new state, so the UI never merges partial updates. The UI talks to a `Backend` interface only; the Tauri implementation and an in-memory mock implement it. The mock serves a fictional cycle and drives UI tests and the screenshot script.

**Region screenshots.** The desktop is captured first (GDI, all monitors as one frame) and shown frozen in a borderless window that covers every monitor. The UI reports the selection as fractions of the shown image, which keeps the mapping independent of monitor scaling and mixed DPI. The PNG is written first and the journal entry second, so the journal never points at a missing file. The offset is taken when the hotkey is pressed, not when the selection ends.

**Testing the real application.** The debug build runs in demo mode (`BBB_DEMO`) with its data in a temporary folder (`BBB_DATA_DIR`) and is driven through the WebView2 remote debugging port with Playwright. This covers the real IPC path, windows and file output; the recovery test kills the whole process tree.

## Consequences

- A hotkey that another program already owns is reported to the user and is not saved.
- Windows are created from async commands or from a separate thread. Creating a window in a synchronous command deadlocks on Windows (found in testing).
- The embedded UI is not rebuilt by cargo when only the front-end files change; the build has to be forced after `npm run build`.

## Addendum: the black overlay (after 0.5.0)

The overlay showed a black screen on a machine with a 4K monitor at 200 % and a Full HD monitor at 100 %. The live capture (`cargo test -p recorder --lib live_capture -- --ignored --nocapture`) is correct on both monitors, so GDI `BitBlt` with `CAPTUREBLT`, the alpha handling and the multi-monitor geometry were not the cause. In the running application the `img` element existed, filled the window and was not hidden, but `naturalWidth` was 0: the picture could not be decoded. The content security policy had no `connect-src`, so the web view refused `http://ipc.localhost`, Tauri switched to the postMessage interface and returned the PNG as an array of numbers, and `new Blob([array])` holds the text "137,80,78,..." instead of the bytes. Playwright's `toBeVisible` accepts such a broken image, which is why the end-to-end test did not notice.

Decisions: the policy allows `connect-src ipc: http://ipc.localhost`; every binary result goes through one conversion (`bytesToBlob`) that accepts both forms; the end-to-end test requires a decoded picture and the overlay phase `ready`. The command encoding the picture is asynchronous (a synchronous command blocks the main thread) and uses fast PNG compression, because the frame of several monitors is tens of megabytes. The overlay must never remain as a black sheet over all monitors: a capture that is blank (checked in Rust on the full-resolution frame and again on the decoded picture), cannot be decoded or does not arrive in 20 seconds shows a message and closes itself, and Escape and clicks close it in every state. The overlay window is read back after placement and placed again when the system changed it.

## Alternatives

- **A native overlay window drawn in Rust:** no web view cost, but all selection drawing and accessibility work would be duplicated.
- **Selection in physical pixels reported by the web view:** wrong on mixed-DPI setups where one window spans monitors with different scale factors.
- **Writing database rows during recording:** two sources of truth that can disagree after a crash.
