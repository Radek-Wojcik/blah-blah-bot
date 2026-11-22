// The single offscreen document of the extension. It owns the WebSocket and every captured
// stream; the service worker only asks it to connect, start and stop. The offscreen document
// can use no extension API except chrome.runtime, so credentials travel through messages.

import { BridgeClient, BridgeError, type SocketLike } from "./bridge-client.js";
import type { MeetTab, OffscreenStatus, Reply, ToOffscreen } from "./ipc.js";
import { StreamMux } from "./mux.js";
import type { ServerMessage, StopReason } from "./protocol.js";

// Offscreen documents have only a part of chrome.runtime (no getManifest): the version
// comes with the connect and pair messages.
let extVersion = "0.0.0";

let appRecording = false;
const meetTabs = new Map<number, string>();

function notify(msg: Record<string, unknown>): void {
  chrome.runtime.sendMessage({ target: "service-worker", ...msg }).catch(() => {
    // the service worker may be stopped; it wakes for the next event and asks for the status
  });
}

function sendMeetTabs(): void {
  for (const [tabId, host] of meetTabs) {
    client.sendText({ type: "meet", tab_id: tabId, host, state: "joined" });
  }
}

const client: BridgeClient = new BridgeClient({
  createSocket: (url) => new WebSocket(url) as unknown as SocketLike,
  extVersion: () => extVersion,
  clientName: `Chrome ${/Chrome\/(\d+)/.exec(navigator.userAgent)?.[1] ?? "?"}`,
  onReady: (ok) => {
    appRecording = ok.recording;
    if (client.port !== null) notify({ type: "port", port: client.port });
    sendMeetTabs();
    mux.onConnected();
  },
  onState: (state) => {
    if (state !== "ready") mux.onDisconnected();
  },
  onMessage: (msg: ServerMessage) => {
    if (msg.type === "start_ok") mux.onStartOk(msg);
    else if (msg.type === "start_refused") mux.onStartRefused(msg);
    else if (msg.type === "state") {
      appRecording = msg.recording;
      mux.onAppState(msg);
    }
  },
  onFatal: (code) => {
    void mux.stopAll("connection_lost");
    notify({ type: "fatal", code });
  },
});

const mux = new StreamMux(client, {
  workletUrl: new URL("pcm-worklet.js", location.href).href,
  onEnded: (tabId, streamId, reason) => notify({ type: "ended", tabId, streamId, reason }),
});

export const offscreen = { client, mux };

function setMeetTabs(tabs: MeetTab[]): void {
  meetTabs.clear();
  for (const t of tabs) meetTabs.set(t.tabId, t.host);
}

async function startCapture(msg: Extract<ToOffscreen, { type: "start" }>): Promise<void> {
  if (!client.isReady && client.state === "stopped") throw new Error("not connected");
  if (mux.has(msg.tabId)) throw new Error("this tab is already being captured");
  const media = await navigator.mediaDevices.getUserMedia({
    audio: {
      mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: msg.mediaStreamId },
    },
    video: false,
  } as MediaStreamConstraints);
  const streamId = `t${msg.tabId}-${Date.now().toString(36)}`;
  await mux.add(
    { streamId, tabId: msg.tabId, groupId: msg.groupId, title: msg.title, host: msg.host },
    media,
    true,
  );
}

function status(): OffscreenStatus {
  return {
    clientState: client.state,
    port: client.port,
    appRecording,
    streams: mux.status(),
  };
}

async function handle(msg: ToOffscreen): Promise<object> {
  switch (msg.type) {
    case "connect":
      extVersion = msg.extVersion;
      setMeetTabs(msg.meetTabs);
      client.connect(msg.credentials, msg.port);
      return {};
    case "pair": {
      extVersion = msg.extVersion;
      setMeetTabs(msg.meetTabs);
      const ok = await client.pair(msg.code);
      return { credentials: { client_id: ok.client_id, secret: ok.secret }, port: client.port };
    }
    case "disconnect":
      await mux.stopAll("user_stop");
      client.disconnect();
      return {};
    case "start":
      await startCapture(msg);
      return {};
    case "stop":
      await mux.stop(msg.tabId, (msg.reason ?? "user_stop") as StopReason);
      return {};
    case "meet":
      if (msg.state === "joined") meetTabs.set(msg.tabId, msg.host);
      else meetTabs.delete(msg.tabId);
      client.sendText({ type: "meet", tab_id: msg.tabId, host: msg.host, state: msg.state });
      return {};
    case "status":
      return { status: status() };
  }
}

chrome.runtime.onMessage.addListener((msg: ToOffscreen, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || msg?.target !== "offscreen") return false;
  handle(msg).then(
    (result) => sendResponse({ ok: true, ...result } satisfies Reply<object>),
    (err: unknown) =>
      sendResponse({
        ok: false,
        error:
          err instanceof BridgeError
            ? `${err.code}: ${err.message}`
            : String((err as Error)?.message ?? err),
        code: err instanceof BridgeError ? err.code : undefined,
      }),
  );
  return true; // async response
});
