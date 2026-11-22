// Service worker. It keeps no state of its own: pairing data lives in chrome.storage.local,
// Meet and capture bookkeeping in chrome.storage.session, and the stream and the socket in
// the offscreen document. A worker restart (Chrome stops it after about 30 s idle) loses
// nothing; every event handler reads what it needs.

import type { MeetTab, OffscreenStatus, StatusReply, ToOffscreen, ToServiceWorker } from "./ipc.js";
import { nextMeetState } from "./meet.js";
import type { Credentials } from "./protocol.js";
import { isBusyCaptureError, retry } from "./retry.js";

const OFFSCREEN_URL = "offscreen.html";
const BADGE_REC = "#c62828";
const BADGE_MEET = "#1a73e8";
const VERSION = chrome.runtime.getManifest().version;

// --- storage -------------------------------------------------------------------------

interface LocalData {
  credentials?: Credentials;
  port?: number;
  autoCapture?: boolean;
}

async function readCredentials(): Promise<{ credentials?: Credentials; port?: number }> {
  const d = (await chrome.storage.local.get(["credentials", "port"])) as LocalData;
  return { credentials: d.credentials, port: d.port };
}

async function readMeet(): Promise<Record<string, string>> {
  const d = await chrome.storage.session.get("meet");
  return (d.meet as Record<string, string> | undefined) ?? {};
}

async function readCaptured(): Promise<number[]> {
  const d = await chrome.storage.session.get("captured");
  return (d.captured as number[] | undefined) ?? [];
}

async function meetTabs(): Promise<MeetTab[]> {
  return Object.entries(await readMeet()).map(([id, host]) => ({ tabId: Number(id), host }));
}

// Read-modify-write of the session storage must not interleave.
let chain: Promise<unknown> = Promise.resolve();
function serial<T>(fn: () => Promise<T>): Promise<T> {
  const next = chain.then(fn, fn);
  chain = next.catch(() => undefined);
  return next;
}

// --- offscreen document --------------------------------------------------------------

let creating: Promise<void> | null = null;

async function hasOffscreen(): Promise<boolean> {
  const contexts = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
  return contexts.length > 0;
}

async function ensureOffscreen(): Promise<void> {
  if (await hasOffscreen()) return;
  creating ??= chrome.offscreen
    .createDocument({
      url: OFFSCREEN_URL,
      reasons: ["USER_MEDIA"],
      justification: "Capture tab audio and stream it to the local application.",
    })
    .finally(() => {
      creating = null;
    });
  await creating;
}

async function toOffscreen<T extends object = object>(msg: ToOffscreen): Promise<T> {
  await ensureOffscreen();
  // A document that was just created needs a moment before its script listens.
  const reply = await retry(
    () =>
      chrome.runtime.sendMessage(msg) as Promise<
        ({ ok: true } & T) | { ok: false; error: string } | undefined
      >,
    {
      intervalMs: 50,
      maxMs: 3000,
      shouldRetry: (err) => /Receiving end does not exist/i.test(String(err)),
    },
  );
  if (!reply || !reply.ok) throw new Error((reply && !reply.ok && reply.error) || "no reply");
  return reply;
}

/** Connects the offscreen document with the stored credentials unless it is connected. */
async function ensureConnected(): Promise<boolean> {
  const { credentials, port } = await readCredentials();
  if (!credentials) return false;
  const existed = await hasOffscreen();
  if (existed) {
    const { status } = await toOffscreen<{ status: OffscreenStatus }>({
      target: "offscreen",
      type: "status",
    });
    if (status.clientState !== "idle" && status.clientState !== "stopped") return true;
  }
  await toOffscreen({
    target: "offscreen",
    type: "connect",
    extVersion: VERSION,
    credentials,
    port,
    meetTabs: await meetTabs(),
  });
  return true;
}

// --- badge ---------------------------------------------------------------------------

async function refreshBadge(tabId: number): Promise<void> {
  const captured = (await readCaptured()).includes(tabId);
  const meet = String(tabId) in (await readMeet());
  const [text, color] = captured ? ["REC", BADGE_REC] : meet ? ["MEET", BADGE_MEET] : ["", ""];
  try {
    await chrome.action.setBadgeText({ tabId, text });
    if (color) await chrome.action.setBadgeBackgroundColor({ tabId, color });
  } catch {
    // the tab is gone
  }
}

async function setCaptured(tabId: number, on: boolean): Promise<void> {
  await serial(async () => {
    const set = new Set(await readCaptured());
    if (on) set.add(tabId);
    else set.delete(tabId);
    await chrome.storage.session.set({ captured: [...set] });
  });
  await refreshBadge(tabId);
}

// --- capture -------------------------------------------------------------------------

function hostOf(url: string | undefined): string {
  try {
    return url ? new URL(url).hostname : "";
  } catch {
    return "";
  }
}

async function capture(tabId: number): Promise<void> {
  if (!(await ensureConnected())) throw new Error("Pair the extension with the application first.");
  const tab = await chrome.tabs.get(tabId);
  await retry(
    async () => {
      // Fails unless the user invoked the extension on this tab (activeTab).
      const mediaStreamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: tabId });
      await toOffscreen({
        target: "offscreen",
        type: "start",
        tabId,
        mediaStreamId,
        title: tab.title ?? "",
        host: hostOf(tab.url),
        groupId: tab.groupId !== undefined && tab.groupId >= 0 ? tab.groupId : undefined,
      });
    },
    { intervalMs: 200, maxMs: 2000, shouldRetry: isBusyCaptureError },
  );
  await setCaptured(tabId, true);
}

async function stopCapture(tabId: number): Promise<void> {
  if (await hasOffscreen()) await toOffscreen({ target: "offscreen", type: "stop", tabId });
  await setCaptured(tabId, false);
}

// --- Meet detection ------------------------------------------------------------------

async function onTabChanged(tabId: number, tab: chrome.tabs.Tab | undefined, closed: boolean) {
  const change = await serial(async () => {
    const meet = await readMeet();
    const joined = String(tabId) in meet;
    const next = nextMeetState(joined, { url: tab?.url, audible: tab?.audible, closed });
    if (!next) return null;
    const host = next === "joined" ? hostOf(tab?.url) : (meet[String(tabId)] ?? "");
    if (next === "joined") meet[String(tabId)] = host;
    else delete meet[String(tabId)];
    await chrome.storage.session.set({ meet });
    return { state: next, host };
  });
  if (!change) return;
  if (await hasOffscreen()) {
    await toOffscreen({ target: "offscreen", type: "meet", tabId, ...change }).catch(() => {});
  }
  if (!closed) await refreshBadge(tabId);
}

chrome.tabs.onUpdated.addListener((tabId, info, tab) => {
  if (info.url !== undefined || info.audible !== undefined) void onTabChanged(tabId, tab, false);
});

chrome.tabs.onRemoved.addListener((tabId) => {
  void (async () => {
    await onTabChanged(tabId, undefined, true);
    if ((await readCaptured()).includes(tabId)) {
      if (await hasOffscreen()) {
        await toOffscreen({ target: "offscreen", type: "stop", tabId, reason: "tab_closed" }).catch(
          () => {},
        );
      }
      await setCaptured(tabId, false);
    }
  })();
});

// --- messages ------------------------------------------------------------------------

async function status(): Promise<StatusReply> {
  const { credentials } = await readCredentials();
  const base = {
    paired: credentials !== undefined,
    meetTabs: await meetTabs(),
    version: VERSION,
  };
  if (!credentials || !(await hasOffscreen())) {
    if (credentials) await ensureConnected();
    if (!credentials || !(await hasOffscreen())) {
      return { ...base, clientState: "no-offscreen", appRecording: false, streams: [] };
    }
  }
  const { status: s } = await toOffscreen<{ status: OffscreenStatus }>({
    target: "offscreen",
    type: "status",
  });
  return {
    ...base,
    clientState: s.clientState,
    detail: s.detail,
    appRecording: s.appRecording,
    streams: s.streams,
  };
}

async function pair(code: string): Promise<void> {
  const reply = await toOffscreen<{ credentials: Credentials; port: number | null }>({
    target: "offscreen",
    type: "pair",
    extVersion: VERSION,
    code,
    meetTabs: await meetTabs(),
  });
  await chrome.storage.local.set({
    credentials: reply.credentials,
    ...(reply.port ? { port: reply.port } : {}),
  });
}

async function unpair(): Promise<void> {
  if (await hasOffscreen()) {
    await toOffscreen({ target: "offscreen", type: "disconnect" }).catch(() => {});
    await chrome.offscreen.closeDocument();
  }
  await chrome.storage.local.remove(["credentials", "port"]);
  await chrome.storage.session.set({ captured: [] });
}

async function dispatch(msg: ToServiceWorker): Promise<object> {
  switch (msg.type) {
    case "status":
      return { status: await status() };
    case "pair":
      await pair(msg.code);
      return {};
    case "unpair":
      await unpair();
      return {};
    case "capture":
      await capture(msg.tabId);
      return {};
    case "stop":
      await stopCapture(msg.tabId);
      return {};
    case "ended":
      await setCaptured(msg.tabId, false);
      return {};
    case "port":
      await chrome.storage.local.set({ port: msg.port });
      return {};
    case "fatal":
      // The application revoked the pairing: forget it so the popup asks for a new code.
      await chrome.storage.local.remove(["credentials", "port"]);
      return {};
  }
}

chrome.runtime.onMessage.addListener((msg: ToServiceWorker, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || msg?.target !== "service-worker") return false;
  dispatch(msg).then(
    (result) => sendResponse({ ok: true, ...result }),
    (err: unknown) => sendResponse({ ok: false, error: String((err as Error)?.message ?? err) }),
  );
  return true;
});

// --- shortcuts and lifecycle ---------------------------------------------------------

chrome.commands.onCommand.addListener((command, tab) => {
  if (command !== "capture-tab" || tab?.id === undefined) return;
  const tabId = tab.id;
  void (async () => {
    try {
      if ((await readCaptured()).includes(tabId)) await stopCapture(tabId);
      else await capture(tabId);
    } catch (err) {
      await chrome.action.setBadgeText({ tabId, text: "ERR" });
      await chrome.action.setBadgeBackgroundColor({ tabId, color: BADGE_REC });
      console.warn("capture-tab failed:", err);
    }
  })();
});

chrome.runtime.onStartup.addListener(() => void ensureConnected());
chrome.runtime.onInstalled.addListener(() => void ensureConnected());
