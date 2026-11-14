# P7: Chrome tab capture over a local WebSocket

Prototype for risk R5 in [docs/RISKS-AND-POC.md](../../docs/RISKS-AND-POC.md) (row P7) and
[ADR 0008](../../docs/decisions/0008-chrome-extension.md). It checks that a Manifest V3 extension
can capture tab audio, keep the tab audible, and stream PCM to a local application without gaps.

Success criteria (P7): no gap longer than 100 ms over 30 minutes; the tab stays audible; clean stop
and restart; a second tab can be added without restarting the first.

## Parts

| Path | Role |
|---|---|
| `extension/` | Unpacked MV3 extension (plain JavaScript, no build step, Chrome 116+) |
| `../src/bbb_bench/proto/p7_receiver.py` | WebSocket receiver, WAV writer, gap metrics, result file |
| `../src/bbb_bench/proto/p7_protocol.py` | Parsing, WAV, gap and verdict logic (unit tested) |
| `tools/tone.html` | Test page that plays a tone from an `AudioContext` |
| `tools/selftest_chrome.py` | Automated partial check with a temporary Chrome profile |

### Extension design

- `service-worker.js` calls `chrome.tabCapture.getMediaStreamId` for the tab on which the user
  invoked the extension and forwards commands to the offscreen document. It keeps no capture state,
  so a service worker restart loses nothing.
- `offscreen.js` is the single offscreen document (reason `USER_MEDIA`). It owns the WebSocket and
  all streams, calls `getUserMedia` with the stream id, and multiplexes any number of tabs.
- `mux.js` uses two shared audio contexts. The playback context (default rate) connects every
  source to `destination`, so the tab stays audible. The capture context runs at 16 kHz, so Chrome
  resamples before the AudioWorklet `pcm-worklet.js`, which mixes down to mono and emits 20 ms
  frames (320 samples) of signed 16-bit PCM.
- `bridge.js` is the WebSocket client (`ws://127.0.0.1:<port>/`). If more than 4 MiB is waiting in
  the socket buffer, frames are dropped and counted; sequence numbers expose the hole.
- `popup.html` / `popup.js`: pairing form and a capture toggle for the active tab.
  Opening the popup by clicking the toolbar icon is the user gesture (`activeTab`). With the
  "Start when the popup opens" box ticked (default), one click on the icon starts capture of that tab.
- `manifest.json` contains a public `key`, so the extension id is stable: `mddhpjgdobfbknicbomoojpbpkgfpima`.
  The receiver checks the WebSocket `Origin` against it.

### Wire protocol (version 1)

One WebSocket carries all tabs.

- Text frames, JSON:
  - `{"type":"hello","version":1,"token":"..."}` first; after a successful pairing the receiver
    answers `{"type":"hello_ok","session":"..."}`. The token works once; reconnects use `"session"`.
  - `{"type":"start","tab_id":N,"sample_rate":16000,"channels":1,"format":"s16le","frame_samples":320,"title":"..."}`
  - `{"type":"stop","tab_id":N,"reason":"user_stop|track_ended|connection_lost","frames_sent":..,"samples_sent":..,"frames_dropped":..}`
  - `{"type":"error","reason":"..."}` from the receiver before it closes the connection.
- Binary frames: 16-byte little-endian header `u32 tab_id | u32 seq | f64 sent_ms (epoch ms)`,
  then little-endian signed 16-bit mono samples at 16 kHz.

## Setup

```bash
cd benchmarks
uv run --group p7 python -m bbb_bench.proto.p7_receiver          # prints a one-time pairing token
```

Receiver options: `--port` (default 17555), `--name` (session name, default timestamp),
`--duration SECONDS` (stop automatically), `--audible yes|no` (manual verdict), `--token`,
`--extension-id` (default: the id above; `any` accepts any extension), `--out-dir`.
WAV files go to `benchmarks/data/p7/<name>/`, the result to `benchmarks/results/p7-chrome-<name>.json`.

Load the extension: open `chrome://extensions`, enable Developer mode, choose Load unpacked, select
`benchmarks/p7-chrome/extension`. Confirm that the id shown equals `mddhpjgdobfbknicbomoojpbpkgfpima`.

Test pages: in a second terminal run `cd benchmarks/p7-chrome/tools; python -m http.server 8000`, then
open `http://127.0.0.1:8000/tone.html?f=440` and `http://127.0.0.1:8000/tone.html?f=660` in two tabs
and press Start tone in each. Any tab playing sound (for example a YouTube video) works as well.
The system output may be a virtual device; the tab only needs to be audible to the extension's replay.

## Manual test procedure

Run each session with its own `--name`. Press Ctrl+C in the receiver terminal to finish a session.

### A. One tab for 30 minutes (gap criterion)

1. Start the receiver: `uv run --group p7 python -m bbb_bench.proto.p7_receiver --name long30 --audible yes`
   (omit `--audible` and edit `manual.tab_audible` in the result later if you prefer).
2. Open one tone tab (or a YouTube video) and start the sound.
3. Click the extension icon, enter the port and the token printed by the receiver, press Pair.
4. Pairing hides the form. Click the icon again (or press Capture this tab) on the tone tab. The badge shows REC.
5. Leave the tab capturing for 30 minutes; keep the computer awake and avoid heavy load.
   Listen from time to time: the tab must stay audible (your output device may be a loopback device).
6. Press Ctrl+C in the receiver. Read `benchmarks/results/p7-chrome-long30.json`:
   `criteria.no_gap_over_100ms` and `criteria.long_run_30min` must be `true`.

### B. Two tabs, clean stop and restart (second-tab and restart criteria)

1. Start a new receiver session: `... --name two-tabs`. Pair as above (a new token is printed each time).
2. Tab 1 (tone 440): click the icon. Wait about a minute.
3. Tab 2 (tone 660): click the icon on that tab. Tab 1 must keep its REC badge and keep sounding.
4. On tab 1 open the popup and press Stop this tab (badge disappears), wait 10 s, then press
   Capture this tab again (restart).
5. Wait another minute, then press Ctrl+C in the receiver.
6. In the result file: `criteria.second_tab_without_restart` and `criteria.clean_stop_restart` must be `true`.
   `streams` lists every run (tab 1 appears twice), with its WAV file name.

### Reading the result

- `criteria.*`: `true` met, `false` not met, `null` not exercised in this session. `overall` is `pass`
  only if all four criteria and `manual.tab_audible` are `true`, `fail` if any is `false`, else `incomplete`.
  Combine sessions A and B when judging the whole P7 row.
- Per stream: `max_gap_wall_ms` (longest interval between frame arrivals, wall clock),
  `max_gap_samples_ms` (stall beyond the audio the previous frame carried; a burst after a stall does not hide it),
  `max_gap_sender_ms` (gaps measured on the capture side), `drift_ms` (wall time minus audio time),
  `frame_seq_lost` and `sender_frames_dropped`, `frames`, `audio_duration_s`.
  A stream passes the gap criterion if both gap values are at most 100 ms and no frame is missing.
- `manual.tab_audible` is a listening judgment: `true` if the tab was audible throughout, `false` if not.
- Open the WAV files in an audio editor to confirm the content is the right tab and that tones are clean.

## Automation and limits

`tools/selftest_chrome.py` launches Chrome with a temporary profile and `--load-extension`, then:

1. Drives the real bridge, multiplexer and AudioWorklet with synthetic tone streams (tab 101, tab 102
   joining without restarting 101, clean stop and restart of 101). The result file is
   `results/p7-chrome-selftest.json`; the WAV files are checked for the correct tone frequencies.
2. Sends popup-style messages to the service worker: pairing (offscreen document with `USER_MEDIA`,
   WebSocket, token, origin check), a capture request, status and disconnect.

```bash
cd benchmarks
uv run --group p7 python p7-chrome/tools/selftest_chrome.py
```

It cannot verify `chrome.tabCapture` on a real tab: Chrome requires the extension to be invoked on the
tab by the user, and the tool does not try to bypass that (in stage 2 the capture request is refused
with the expected `activeTab` error). It also says nothing about the 30-minute run, audibility on
real output, or behavior with real tab audio (stereo, silence, tab navigation). Those need the manual
procedure above. The self-test needs a Chrome that still honors `--load-extension` (verified with 136).

Known limits of the prototype: no automatic reconnect, no Meet detection, no tab-group guidance, no icons.
Frames are written to the socket from the offscreen document without a timer, so background timer
throttling should not apply, but only the 30-minute run can confirm that.

## Tests

```bash
cd benchmarks
uv run ruff check .; uv run ruff format .; uv run pytest
uv run --group p7 pytest tests/test_p7_receiver.py   # also runs the socket-level test
```
