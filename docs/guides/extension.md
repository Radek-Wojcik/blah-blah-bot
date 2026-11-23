# Browser extension: install and pair

The Chrome extension sends the audio of one browser tab to Blah Blah Bot while a recording runs, and tells the application when a Google Meet call is going on in a tab. It talks only to the application on your own computer (`127.0.0.1`); nothing is sent over the network. The decision records are [ADR 0020](../decisions/0020-bridge-protocol-and-pairing.md) (protocol and pairing) and [ADR 0021](../decisions/0021-extension-distribution.md) (distribution).

## Requirements

- Chrome 116 or newer (tested on Chrome 136), on the computer where the application runs. Other browsers, Edge included, are not tested and not supported in this release.
- Blah Blah Bot of the same release. The extension and the application agree on a protocol version; an old extension is refused with a message.

## Install

The extension is not in the Chrome Web Store and will not be (ADR 0021). It is installed unpacked from the file attached to each release.

1. Download `blah-blah-bot-extension-<version>.zip` from the release page and unpack it to a folder that stays where it is (Chrome loads the extension from that folder every time it starts; do not unpack it into the downloads folder you clean up).
2. Open `chrome://extensions` in Chrome.
3. Switch on **Developer mode** (top right).
4. Choose **Load unpacked** and select the unpacked folder. The extension appears as "Blah Blah Bot". The id shown on its card must be `pbgjebpmicmffhjbphlgginplaflodnj`; the application accepts no other extension unless you add its id in the settings.
5. Pin it to the toolbar (the puzzle piece menu) so the icon is one click away.

Chrome shows a warning about developer mode extensions when it starts, and may switch such extensions off after an update; switch it on again in `chrome://extensions`. A new release means a new zip: unpack it over the old folder and press the reload button on the card.

Building it yourself: `cd extension && npm ci && npm run pack` writes `target/extension/unpacked` (load this folder) and the zip next to it.

## Pair

Pairing is done once per browser profile and survives restarts of Chrome and of the application.

1. In the application open **Settings, Browser extension** and press **Pair extension**. The application shows a code such as `7K3M-9QXD`; it works once and expires after 5 minutes.
2. Click the extension icon and type the code into the popup, then press **Pair**. The popup says Paired and shows the connection state.
3. To remove the pairing choose **Unpair** in the application's settings, or **Forget this pairing** in the popup (the application's list keeps the entry until you unpair there).

If the popup says it cannot reach the application, start Blah Blah Bot. The extension looks for it on ports 17555 to 17564 and keeps trying every few seconds.

## Capture a tab

1. Start a recording in the application. A tab is accepted only while a recording runs.
2. Open the tab, click the extension icon, and the tab is captured when the popup opens (untick "Capture when this popup opens" if you prefer a button), or press the shortcut **Alt+Shift+R** on the tab. The toolbar badge shows `REC`. Press the shortcut again, or use the popup, to stop capturing that tab.
3. The tab stays audible: its sound is replayed to your default output while it is captured.
4. The tab becomes its own track of the recording, labelled with the site name.

If you capture a tab while no recording runs, the extension waits and starts sending when you start a recording. When the recording ends the capture ends too.

Chrome allows capturing a tab only after you invoked the extension on it (click, popup or shortcut); this is a browser rule, so a tab cannot be captured in the background and several tabs take one action each.

## Capture a tab group

Chrome allows capturing a tab only after you acted on that very tab, so a group of tabs is never captured in one step. The extension guides you through it: one action per tab, and it always shows which tabs are still missing.

1. Put the tabs in a Chrome tab group (right click a tab, Add tab to new group) and start a recording in the application.
2. On the first tab click the extension icon and press **Capture this group** (or press **Alt+Shift+R**). The popup lists the tabs of the group with their status:

   | Status | Meaning |
   |---|---|
   | captured | the tab is being recorded |
   | waiting for click | not captured yet; needs the click or the shortcut on that tab |
   | not audible | like waiting, but the tab plays no sound at the moment |
   | failed | the last attempt did not work; try again on that tab |

3. Press **Next tab** in the popup to bring the next missing tab to the front (tabs that play sound come first, then silent ones, then failed ones), then click the icon there. The shortcut does both: **Alt+Shift+R** captures the tab you are on and then switches to the next tab that is still missing, so for a group you press it once per tab. Pressing it on a captured tab stops that tab.
4. A tab added to the group later is waiting: the toolbar badge shows `+1` (the number of tabs that wait for an action) and the application is told. Closing a captured tab ends its stream; the tab leaves the group list.

Every tab is its own track of the recording, labelled `Tab: <site name>`. Only the site name and the tab id are sent to the application, never the page title. Chrome shows the extension a tab's title only for tabs it was used on, so other tabs of the group appear as `Tab 2`, `Tab 3` in the popup until you capture them.

## Google Meet

A tab on a Meet call address that is playing sound is reported to the application as a Meet call (`MEET` on the badge). Only the site name (`meet.google.com`) is sent, never the meeting code or the title. The extension asks Chrome for access to `https://meet.google.com/*` for this and for nothing else; it cannot read other sites.

## When something goes wrong

| Symptom | What to check |
|---|---|
| "Pairing code is not correct" or "expired" | Start pairing again in the application and type the new code. Five wrong attempts lock a code. |
| "Waiting for the application" | The application is not running, the bridge is switched off in its settings, or another program uses the ports 17555 to 17564. |
| Nothing is recorded though the popup says Connected | A tab is accepted only while a recording runs; start the recording first. |
| The extension is gone after Chrome updated | Switch it on again in `chrome://extensions` (developer mode extensions can be switched off by Chrome). |
| A tab of the group stays "waiting for click" | Chrome cannot capture a tab in the background; open the tab and click the icon or press Alt+Shift+R there. |
| "Cannot capture a tab" | The tab is a Chrome page (`chrome://`), the extension was not invoked on it, or Chrome is still releasing an earlier capture; try again after a moment. |

## For developers

- `npm run typecheck`, `npm run lint`, `npm test` and `npm run build` run in CI; the scripted Chrome tests (`npm run test:e2e`) and the continuity measurement (`npm run measure`) run locally. They use a temporary Chrome profile under `target/e2e-tmp` and never touch your profile or your sign-ins. See [TESTING.md](../TESTING.md).
- The bridge alone, without the application, for trying the extension: `cargo run -p bridge --example test_receiver -- --code 7K3M9QXD --extension-id pbgjebpmicmffhjbphlgginplaflodnj`.
