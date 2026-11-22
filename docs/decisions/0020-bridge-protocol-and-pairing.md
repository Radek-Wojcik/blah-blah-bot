# 0020. Bridge protocol version 2, pairing and pushed tab tracks

**Status:** Accepted (Stage 6)

## Context

[ADR 0008](0008-chrome-extension.md) chose a Manifest V3 extension that streams tab audio to the application over a WebSocket on `127.0.0.1`. The prototype (P7, protocol version 1) used a one-time token per receiver run, numbered streams by tab id, had no reconnect and wrote frames straight to WAV files. Stage 6 needs a bridge inside the application: pairing that survives restarts, several tabs on one connection, a service worker or network restart without losing the stream, and tab tracks that go through the same journal and recovery as every other track. The P7 runs showed delivery stalls of up to 667 ms with no lost frames and, under load, a browser audio clock that lagged wall time by 2 s in 30 minutes.

## Decision

### Transport

- Crate `bridge`, blocking server on [tungstenite](https://github.com/snapview/tungstenite-rs) 0.30 (MIT OR Apache-2.0, default features off except `handshake`). One thread per connection; after the handshake the socket is non-blocking and the loop polls every 10 ms while frames flow and every 50 ms when idle. No async runtime.
- Binds `127.0.0.1` only, port 17555, then 17556 to 17564 when taken. The extension tries the same list.
- Upgrade checks: the peer address is loopback; exactly one `Host` header, equal to `127.0.0.1:<port>` or `localhost:<port>` (against DNS rebinding); exactly one `Origin` header of the form `chrome-extension://<id>`, with `<id>` in `bridge.allowedExtensionIds` when that list is not empty. A refused upgrade gets HTTP 403 and no WebSocket. The list defaults to the id that follows from the key in the extension's manifest (ADR 0021); an empty list accepts any extension origin, which still needs the pairing code or a secret.
- At most two connections (two browser profiles); a third gets `error{busy}`. Text messages up to 16 KiB, binary messages up to 16 + 9600 bytes (300 ms of audio); larger ones close the connection with code 1009. A connection that sends nothing for 15 s is closed; the extension sends `ping` every 5 s (`heartbeat_ms` in `hello_ok`). `hello` must come within 10 s.

### Messages (version 2)

One WebSocket carries every tab. Text frames are JSON with a `type`; receivers ignore unknown fields. The schema is `schemas/bridge-message.schema.json`; `crates/bridge/tests/fixtures` holds valid and invalid messages and binary frames that the Rust tests and the extension tests both read.

| Direction | Message | Purpose |
|---|---|---|
| ext to app | `hello{version:2, client, ext_version, pairing_code}` or `hello{..., client_id, secret}` | Pair, or authenticate a paired extension |
| app to ext | `hello_ok{client_id, secret?, session_id, app_version, recording, heartbeat_ms}` | Accepted; `secret` only right after pairing |
| app to ext | `error{code, message}` then close | `bad_version`, `bad_code`, `code_expired`, `unpaired`, `busy`, `bad_message` |
| ext to app | `start{stream_id, tab_id, group_id?, title, host, sample_rate:16000, channels:1, format:"s16le", frame_samples:320}` | A tab wants to be recorded; frames only after `start_ok` |
| app to ext | `start_ok{stream_id, stream_no, track}` / `start_refused{stream_id, reason}` | `not_recording`, `duplicate`, `declined`, `unknown_stream` |
| ext to app | `resume{stream_id, next_seq}` | After a reconnect; answered with `start_ok` (same `stream_no`) or `start_refused{unknown_stream}` |
| ext to app | `stop{stream_id, reason, frames_sent, samples_sent, frames_dropped}` | `user_stop`, `track_ended`, `tab_closed`, `connection_lost` |
| ext to app | `meet{tab_id, host, state}` | A Meet tab `joined` or `left`; host only, never the meeting code |
| ext to app | `group{group_id, tabs[{tab_id, status}]}` | Status of the tabs of a group the user is capturing: `captured`, `waiting` (needs a click; also a silent tab) or `failed`. Sent after every change and again after a reconnect; a group without tabs is sent once with an empty list. No title or address, only tab ids |
| both | `ping{t?}` / `pong{t?}` | Heartbeat |
| app to ext | `state{recording, meeting_id?, streams[{stream_id, track, status}]}` | Complete state after `hello_ok` and on every change; a stream missing from the list is not recorded |

Binary frames: 16-byte little-endian header `u32 stream_no | u32 seq | f64 sent_ms`, then signed 16-bit little-endian mono samples at 16 kHz. `seq` counts frames of a stream from 0. `sent_ms` is the sender's wall clock at capture; frames buffered during a disconnect keep it.

### Pairing

- The user presses Pair extension in Settings. The application shows an 8-character Crockford base32 code (40 random bits, shown as `7K3M-9QXD`; case, separators and the look-alikes I, L, O are accepted). It is valid for 5 minutes, works once and is locked after 5 wrong attempts.
- A correct code creates a client id (`c-` and 16 hex characters) and a 32-byte random secret. The secret goes to the extension once (`chrome.storage.local`); the application stores only its SHA-256 in `settings.bridge.clients`, compared in constant time. The settings screen never receives the hashes, and a settings save keeps the stored list.
- Unpair removes the client and closes its connections with `error{unpaired}`.

### Streams and recording

- A tab is accepted only while a recording runs; it becomes the source `tab:<stream_id>` of that recording (track `tab`, `tab-2`, ..., label `Tab: <host>`). With `bridge.autoAddTabs` off the answer is `declined` (asking the user comes with M2).
- When the connection drops, the stream waits 30 s for `resume` on a new connection of the same client; frames of a stream are accepted only from the connection that owns it. The hub queues up to 30 s of frames per stream for the recorder and counts what it has to drop.
- Recorder: `PushTrack` places frames by sample count. The first frame starts at the track offset; missing sequence numbers become silence journaled as `discontinuity{kind: stream_gap}` (a new kind; older journals read unchanged); duplicates are dropped. The sender time, measured from the first frame, drives the existing drift corrector and fills capture stalls longer than 1 s (`silence_filled`). Delivery delays and bursts after a reconnect move nothing. `RoutingBackend` sends tab sources to a `TabSource` (the bridge in the application, a tone generator in tests).

## Consequences

- Pairing is per browser profile and survives application and browser restarts; a reinstalled extension pairs again.
- A recording that stops ends every stream (`state{recording:false}`); the extension stops capturing on that message.
- The drift corrector restarts its measurement when the sender's rate is more than about 1000 ppm off, so a sender that slow (P7 run 2 under load) is not corrected; the 1 s stall rule then keeps the track within about a second of the session clock. Measured with a real tab in the M2 30-minute run.
- Absolute sender time is not trusted: a sender clock offset only shifts the stall rule's reference, not the track.
- The bridge runs in demo mode as well, so the extension can be tried against the demo application.

## Alternatives

- **Async server (tokio-tungstenite, axum):** a runtime for two connections; the application has none elsewhere.
- **Placing frames by `sent_ms` against the session start:** exact on paper, but a delivery stall stamped late or a sender clock offset would insert silence and shift the rest of the track.
- **Secret in the Credential Manager instead of a hash in the settings:** a hash of a 256-bit random secret cannot be reversed; the settings file is enough and keeps unpairing simple.
- **Native messaging:** see ADR 0008; still awkward for continuous binary audio.
