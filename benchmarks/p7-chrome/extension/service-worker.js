// Service worker: obtains the tab-capture stream id (requires the extension to have been
// invoked on the tab) and forwards commands to the offscreen document. It keeps no
// capture state of its own, so a worker restart loses nothing.

const OFFSCREEN_URL = "offscreen.html";
let creating = null;

async function ensureOffscreen() {
  const contexts = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
  if (contexts.length > 0) return;
  if (!creating) {
    creating = chrome.offscreen
      .createDocument({
        url: OFFSCREEN_URL,
        reasons: ["USER_MEDIA"],
        justification: "Capture and replay tab audio while streaming it to a local receiver.",
      })
      .finally(() => {
        creating = null;
      });
  }
  await creating;
}

async function toOffscreen(message) {
  await ensureOffscreen();
  const reply = await chrome.runtime.sendMessage({ target: "offscreen", ...message });
  if (!reply || !reply.ok) throw new Error((reply && reply.error) || "no reply from offscreen");
  return reply;
}

async function setBadge(tabId, text) {
  try {
    await chrome.action.setBadgeText({ tabId, text });
    if (text) await chrome.action.setBadgeBackgroundColor({ tabId, color: "#c62828" });
  } catch {
    // the tab may be gone already
  }
}

const handlers = {
  async pair(msg) {
    await toOffscreen({ type: "connect", port: msg.port, token: msg.token });
    await chrome.storage.session.set({ port: msg.port }); // the token is never stored
    return {};
  },
  async start(msg) {
    const tab = await chrome.tabs.get(msg.tabId);
    // Fails unless the user invoked the extension on this tab (activeTab).
    const streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: msg.tabId });
    await toOffscreen({ type: "start", tabId: msg.tabId, streamId, title: tab.title || "" });
    await setBadge(msg.tabId, "REC");
    return {};
  },
  async stop(msg) {
    await toOffscreen({ type: "stop", tabId: msg.tabId });
    return {};
  },
  async disconnect() {
    await toOffscreen({ type: "disconnect" });
    const contexts = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
    if (contexts.length > 0) await chrome.offscreen.closeDocument();
    return {};
  },
  async status() {
    const contexts = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
    if (contexts.length === 0) return { connected: false, streams: [] };
    const reply = await toOffscreen({ type: "status" });
    return { connected: reply.connected, streams: reply.streams };
  },
};

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id) return false;
  if (msg.target === "service-worker" && msg.type === "stopped") {
    setBadge(msg.tabId, "");
    return false;
  }
  if (msg.target !== "service-worker") return false;
  const handler = handlers[msg.type];
  if (!handler) return false;
  handler(msg).then(
    (result) => sendResponse({ ok: true, ...result }),
    (err) => sendResponse({ ok: false, error: String(err && err.message ? err.message : err) }),
  );
  return true;
});
