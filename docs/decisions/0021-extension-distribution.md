# 0021. Browser extension: distribution, identity and defaults

**Status:** Proposed (Stage 6)

## Context

[ADR 0008](0008-chrome-extension.md) chose a Manifest V3 extension for tab audio and [ADR 0020](0020-bridge-protocol-and-pairing.md) fixed the protocol between it and the application. What was left open is how the extension reaches a user, what identifies it to the application, and which defaults the application and the extension share. The repository is private until 1.0 and a release is a Windows installer plus files attached to a GitHub release.

## Decision

### Distribution

- The extension is distributed as `blah-blah-bot-extension-<version>.zip`, attached to each GitHub release (built by `npm run pack` in `extension/`). The user unpacks it, opens `chrome://extensions`, switches on Developer mode and chooses Load unpacked. The steps are in [docs/guides/extension.md](../guides/extension.md).
- The extension is **not** published in the Chrome Web Store, now or later. Reasons:
  - It captures the audio of web pages. A store review of such an extension asks for a privacy policy, a justification of each permission and a public listing of an author and an address; none of that fits a private tool of a small team.
  - It talks to a local application only. A store listing would invite users who do not have the application.
  - A store update can change behavior between releases of the application. The protocol is versioned (ADR 0020), but one zip per release keeps the two in step.
- Drawbacks, accepted:
  - Chrome shows a "Disable developer mode extensions" warning at start and may switch such extensions off after a browser update; the user has to enable it again.
  - No automatic update: a new release means a new zip and a reload in `chrome://extensions`. The extension reports its version in `hello` and the application stores it with the paired client; the protocol version check refuses an old extension with `bad_version`.
  - Only Chromium browsers with unpacked extension support (Chrome, Edge) work. Chrome 116 or newer.
  - Loading by script (`--load-extension`) is not available to users of Chrome 137 or newer; the manual steps are unaffected.
- A signed `.crx` for policy installation was not built: it would need a private key kept by the project and a managed browser.

### Identity

- The manifest carries a public `key`. The extension id derived from it (`pbgjebpmicmffhjbphlgginplaflodnj`) is therefore the same for every user and on every machine, and the application can check the `Origin` of the socket against it. The key is new for Stage 6; the P7 prototype used another one, so a prototype install is a different extension and does not pair.
- The private half of the key was discarded when the key was generated: nothing is signed with it and nobody can publish an update under this id from a store.
- `bridge.allowedExtensionIds` defaults to this id (`bridge::server::EXTENSION_ID`). A test derives the id from `extension/manifest.json` and fails when the two drift apart. A user who wants to run a self-built extension with another key adds its id in the settings, or clears the list to accept any extension origin (pairing still decides who may stream).

### Defaults, as configuration

| Setting | Default | Where |
|---|---|---|
| `bridge.allowedExtensionIds` | the id above | application settings |
| `bridge.port`, `bridge.portFallbacks` | 17555, then 17556 to 17564 | application settings; the extension tries the same ten ports in order |
| minimum protocol version (`min_protocol`) | 2 (an older extension gets `bad_version`) | `bridge::protocol::PROTOCOL_VERSION` |
| `bridge.autoAddTabs` | on | application settings |

The extension has no setting for the ports: they are constants of the release, like the protocol version. A user who changes the application's ports has to use a matching extension build. The ports are a fixed list so that a browser can find the application without a discovery step and without any message that leaves the machine.

### Extension design decisions

- The service worker keeps no state; pairing data is in `chrome.storage.local`, Meet and capture bookkeeping in `chrome.storage.session`, and the socket, the audio graph and the stream sessions in the offscreen document (reason `USER_MEDIA`). Chrome stops an idle service worker after about 30 s and a stream must not notice.
- After a lost connection the extension retries after 0.5, 1, 2, 4 and then every 5 s, trying the last working port first, keeps up to 30 s of audio (about 960 KB) per stream and sends `resume`; if the application forgot the stream (a restart) it offers it again with `start`. A stream waits (not recording, nothing buffered) while the application is idle and starts when the application reports a recording.
- Re-capturing a tab right after stopping it is retried every 200 ms for up to 2 s, because Chrome releases the capture a moment later.
- Meet detection reads the tab URL (the host permission `https://meet.google.com/*` is the only host access; the `tabs` permission is not requested). A tab on a `xxx-xxxx-xxx` call URL that becomes audible is `joined`, it stays joined until the URL leaves a call or the tab closes. Only the host is sent.
- The extension has a scripted-test hook that starts a synthetic tone stream; it is compiled only into the test build (`npm run build:test`), never into the packaged zip, which `pack.mjs` checks. The test build listens on ports 17564 and 17563 so that a running application is never touched.

## Verified against the installed Chrome (136.0.7103.114)

- `chrome.offscreen.createDocument` with reason `USER_MEDIA` works, and an offscreen document can create a socket to `ws://127.0.0.1`, an `AudioContext` and an `AudioWorklet`. The documentation says only `chrome.runtime` exists there. **Discrepancy:** even inside `chrome.runtime`, `getManifest` is missing in Chrome 136, so the version of the extension is handed to the offscreen document in the `connect` and `pair` messages. The document is created, but its script needs a moment before it listens: the first message from the service worker is retried on "Receiving end does not exist".
- `chrome.tabCapture.getMediaStreamId` needs the extension to be invoked on the tab (click, popup or command) and the id is single use and expires after a few seconds; since Chrome 116 the offscreen document may use an id created in the service worker. That path is the same as in P7 and was not exercised by a script: a script cannot invoke the extension on a tab. The real tab run is manual (see the measurement below).
- Scripted loading: Chrome 136 still honors `--load-extension` together with `--disable-extensions-except`; Playwright 1.65 with `channel: "chrome"` loads the extension and exposes its service worker. Chrome 137 and later ignore the flag in branded builds; the replacement is `Extensions.loadUnpacked` over a pipe with `--enable-unsafe-extension-debugging`. This was **not** exercised here because the installed Chrome still supports the flag.
- **Headless Chrome did not report a tab as audible** (`tab.audible` stayed false for a page that plays a tone, with and without `--mute-audio`). A headed window reports it, so the scripted tests run a headed window positioned off screen. Without `--mute-audio`, because that flag also hides the audible state; the fake Meet page plays a tone at 2 % gain.
- `ServiceWorker.stopAllWorkers` of the DevTools protocol stops the extension service worker when the session is attached to a page of the extension (the popup opened in a tab); attached to a normal page it had no visible effect. The next event, a popup reload, starts a new worker.
- That `chrome.tabs.onUpdated` reports `audible` and `url` of a Meet tab without the `tabs` permission (only the host permission) is what the design relies on; the scripted Meet test checks it (`extension/e2e/bridge.spec.ts`).

## Measurement

The continuity run of a synthetic tone through the extension (worklet, session, socket), the bridge and the recorder's `PushTrack` is `extension/e2e/measure.spec.ts` (`npm run measure`); its result is `benchmarks/results/stage6-tab-30min-synthetic.json`. The file in the repository comes from a **2 minute run** and says so (`full_run: false`, `overall: incomplete`). The 30 minute run on an idle machine, and a run with a real tab (tone page, then a call), are still to be done by hand; the P7 criterion is a longest gap of 100 ms and no lost frame.

## Consequences

- A release has two artifacts that must agree: the installer and the extension zip. The release checklist includes the manifest version and the id test.
- Users need Developer mode. The guide says why and what to expect.
- Moving to the Chrome Web Store later would change the extension id (the store assigns its own key); the application would accept both ids during a transition. This ADR would then be superseded.

## Alternatives

- **Chrome Web Store (unlisted):** automatic updates and no developer mode warning, at the price of review, a privacy policy, a publisher account and a dependency on a third party's policy for a tool that is private to a team.
- **Policy-installed `.crx`:** no developer mode, but needs a managed browser and a signing key.
- **No extension, process loopback of the whole browser:** exists as the fallback (ADR 0008), but records every tab of the browser.
- **Reading the port from a file written by the application:** an extension cannot read files; native messaging would be needed just for discovery.
