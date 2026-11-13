// The single offscreen document of the extension. It owns the WebSocket and every
// captured stream; the service worker only asks it to connect, start and stop.

import { Bridge } from "./bridge.js";
import { StreamMux } from "./mux.js";

const bridge = new Bridge();
const mux = new StreamMux(bridge, {
  workletUrl: chrome.runtime.getURL("pcm-worklet.js"),
  onStopped: (tabId, reason) => {
    chrome.runtime
      .sendMessage({ target: "service-worker", type: "stopped", tabId, reason })
      .catch(() => {});
  },
});
bridge.onclose = () => mux.stopAll("connection_lost");

async function startCapture(tabId, streamId, title) {
  if (!bridge.connected) throw new Error("not paired with the receiver");
  if (mux.has(tabId)) throw new Error("this tab is already being captured");
  const mediaStream = await navigator.mediaDevices.getUserMedia({
    audio: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } },
    video: false,
  });
  await mux.add(tabId, mediaStream, { title, audible: true });
}

const handlers = {
  async connect(msg) {
    await bridge.connect(msg.port, msg.token);
    return {};
  },
  async start(msg) {
    await startCapture(msg.tabId, msg.streamId, msg.title);
    return {};
  },
  async stop(msg) {
    await mux.stop(msg.tabId, "user_stop");
    return {};
  },
  async disconnect() {
    await mux.stopAll("user_stop");
    bridge.close();
    return {};
  },
  async status() {
    return { connected: bridge.connected, streams: mux.status() };
  },
};

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || msg.target !== "offscreen") return false;
  const handler = handlers[msg.type];
  if (!handler) return false;
  handler(msg).then(
    (result) => sendResponse({ ok: true, ...result }),
    (err) => sendResponse({ ok: false, error: String(err && err.message ? err.message : err) }),
  );
  return true; // async response
});
