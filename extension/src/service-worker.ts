// Service worker. It keeps no state of its own: pairing data lives in chrome.storage.local,
// Meet, capture and tab group bookkeeping in chrome.storage.session, and the stream and the
// socket in the offscreen document. A worker restart (Chrome stops it after about 30 s idle)
// loses nothing; every event handler reads what it needs.

import {
  badgeText,
  groupMessage,
  groupStatuses,
  hostOfUrl,
  missingCount,
  nextTab,
  pruneFailed,
  summary,
  tabLabel,
  type GroupTab,
} from "./groups.js";
import type {
  GroupView,
  MeetTab,
  OffscreenStatus,
  StatusReply,
  ToOffscreen,
  ToServiceWorker,
} from "./ipc.js";
import { nextMeetState } from "./meet.js";
import type { Credentials, Group } from "./protocol.js";
import { isBusyCaptureError, retry } from "./retry.js";

const OFFSCREEN_URL = "offscreen.html";
const BADGE_REC = "#c62828";
const BADGE_MEET = "#1a73e8";
const BADGE_WAIT = "#f9a825";
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

/** Tabs whose last capture attempt failed. */
async function readFailed(): Promise<number[]> {
  const d = await chrome.storage.session.get("failed");
  return (d.failed as number[] | undefined) ?? [];
}

/** Ids of the tab groups the user is capturing (guided capture). */
async function readGroups(): Promise<number[]> {
  const d = await chrome.storage.session.get("groups");
  return (d.groups as number[] | undefined) ?? [];
}

interface Label {
  title: string;
  host: string;
}

/** Title and host of the tabs the extension was used on (Chrome shows no others). */
async function readLabels(): Promise<Record<string, Label>> {
  const d = await chrome.storage.session.get("labels");
  return (d.labels as Record<string, Label> | undefined) ?? {};
}

async function meetTabs(): Promise<MeetTab[]> {
  return Object.entries(await readMeet()).map(([id, host]) => ({ tabId: Number(id), host }));
}

// Read-modify-write of the session storage must not interleave.
function makeSerial(): <T>(fn: () => Promise<T>) => Promise<T> {
  let chain: Promise<unknown> = Promise.resolve();
  return (fn) => {
    const next = chain.then(fn, fn);
    chain = next.catch(() => undefined);
    return next;
  };
}
const serial = makeSerial();
// Group reports read the storage and talk to the offscreen document: one at a time.
const groupSerial = makeSerial();

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

/** Badge of one tab: REC or MEET; otherwise the global badge (tabs waiting in a group). */
async function refreshBadge(tabId: number): Promise<void> {
  const captured = (await readCaptured()).includes(tabId);
  const meet = String(tabId) in (await readMeet());
  const [text, color] = captured ? ["REC", BADGE_REC] : meet ? ["MEET", BADGE_MEET] : [null, ""];
  try {
    // null clears the badge of this tab, which then shows the global one.
    await chrome.action.setBadgeText({ tabId, text: text as string });
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
    if (on) {
      await chrome.storage.session.set({ failed: (await readFailed()).filter((i) => i !== tabId) });
    }
  });
  await refreshBadge(tabId);
}

async function setFailed(tabId: number): Promise<void> {
  await serial(async () => {
    const set = new Set(await readFailed());
    set.add(tabId);
    await chrome.storage.session.set({ failed: [...set] });
  });
}

async function rememberLabel(tabId: number, label: Label): Promise<void> {
  await serial(async () => {
    const labels = await readLabels();
    labels[String(tabId)] = label;
    await chrome.storage.session.set({ labels });
  });
}

async function forgetLabel(tabId: number): Promise<void> {
  await serial(async () => {
    const labels = await readLabels();
    delete labels[String(tabId)];
    await chrome.storage.session.set({ labels });
  });
}

// --- tab groups ----------------------------------------------------------------------

async function trackGroup(groupId: number): Promise<void> {
  if (groupId < 0) return;
  await serial(async () => {
    const groups = await readGroups();
    if (!groups.includes(groupId)) {
      await chrome.storage.session.set({ groups: [...groups, groupId] });
    }
  });
}

async function groupTabs(groupId: number): Promise<GroupTab[]> {
  const labels = await readLabels();
  const tabs = await chrome.tabs.query({ groupId });
  return tabs.flatMap((t) => {
    if (t.id === undefined) return [];
    const known = labels[String(t.id)];
    return [
      {
        tabId: t.id,
        index: t.index,
        audible: t.audible === true,
        // Chrome gives a title and an address only for tabs the extension was used on.
        title: t.title || known?.title || "",
        host: hostOfUrl(t.url) || known?.host || "",
      },
    ];
  });
}

/**
 * Reports every tracked group to the application (the offscreen document keeps the last
 * report and sends it again after a reconnect), forgets groups without tabs and shows the
 * number of tabs still waiting on the badge.
 */
function refreshGroups(): Promise<void> {
  return groupSerial(async () => {
    const tracked = await readGroups();
    if (tracked.length === 0) {
      await chrome.action.setBadgeText({ text: "" });
      return;
    }
    const captured = new Set(await readCaptured());
    const open = (await chrome.tabs.query({})).flatMap((t) =>
      t.id === undefined ? [] : [{ tabId: t.id }],
    );
    const failed = new Set(pruneFailed(await readFailed(), open));
    const keep: number[] = [];
    const messages: Group[] = [];
    let missing = 0;
    for (const groupId of tracked) {
      const states = groupStatuses(await groupTabs(groupId), captured, failed);
      // A group without tabs is reported once, empty, and then forgotten.
      messages.push(groupMessage(groupId, states));
      if (states.length > 0) {
        keep.push(groupId);
        missing += missingCount(states);
      }
    }
    await chrome.storage.session.set({ groups: keep, failed: [...failed] });
    await chrome.action.setBadgeBackgroundColor({ color: BADGE_WAIT });
    await chrome.action.setBadgeText({ text: badgeText(missing) });
    if (!(await hasOffscreen())) return;
    for (const group of messages) {
      await toOffscreen({ target: "offscreen", type: "group", group }).catch(() => {});
    }
  });
}

async function groupView(tabId: number): Promise<GroupView | null> {
  const tab = await chrome.tabs.get(tabId);
  if (tab.groupId === undefined || tab.groupId < 0) return null;
  const states = groupStatuses(
    await groupTabs(tab.groupId),
    new Set(await readCaptured()),
    new Set(await readFailed()),
  );
  let title = "";
  try {
    title = (await chrome.tabGroups.get(tab.groupId)).title ?? "";
  } catch {
    // the group is gone
  }
  return {
    groupId: tab.groupId,
    title,
    summary: summary(states),
    tabs: states.map((s, i) => ({
      tabId: s.tabId,
      label: tabLabel(s, i + 1),
      host: s.host,
      status: s.status,
      active: s.tabId === tabId,
    })),
    next: nextTab(states, tabId),
  };
}

/** Brings the next tab that still needs an action to the front. Returns its id, or null. */
async function activateNext(tabId: number): Promise<number | null> {
  const view = await groupView(tabId);
  if (!view || view.next === null) return null;
  await chrome.tabs.update(view.next, { active: true });
  return view.next;
}

// --- capture -------------------------------------------------------------------------

/** A tab is captured: remember it, track its group and report the group. */
async function onCaptured(tabId: number, label: Label): Promise<void> {
  await rememberLabel(tabId, label);
  await setCaptured(tabId, true);
  try {
    await trackGroup((await chrome.tabs.get(tabId)).groupId);
  } catch {
    // the tab closed meanwhile
  }
  await refreshGroups();
}

async function captureTab(tabId: number, tab: chrome.tabs.Tab): Promise<void> {
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
        host: hostOfUrl(tab.url),
        groupId: tab.groupId !== undefined && tab.groupId >= 0 ? tab.groupId : undefined,
      });
    },
    { intervalMs: 200, maxMs: 2000, shouldRetry: isBusyCaptureError },
  );
}

async function capture(tabId: number): Promise<void> {
  if (!(await ensureConnected())) throw new Error("Pair the extension with the application first.");
  const tab = await chrome.tabs.get(tabId);
  try {
    await captureTab(tabId, tab);
  } catch (err) {
    await setFailed(tabId);
    await refreshGroups();
    throw err;
  }
  await onCaptured(tabId, { title: tab.title ?? "", host: hostOfUrl(tab.url) });
}

async function stopCapture(tabId: number): Promise<void> {
  if (await hasOffscreen()) await toOffscreen({ target: "offscreen", type: "stop", tabId });
  await setCaptured(tabId, false);
  await refreshGroups();
}

// --- Meet detection ------------------------------------------------------------------

async function onTabChanged(tabId: number, tab: chrome.tabs.Tab | undefined, closed: boolean) {
  const change = await serial(async () => {
    const meet = await readMeet();
    const joined = String(tabId) in meet;
    const next = nextMeetState(joined, { url: tab?.url, audible: tab?.audible, closed });
    if (!next) return null;
    const host = next === "joined" ? hostOfUrl(tab?.url) : (meet[String(tabId)] ?? "");
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
  // A tab that joins or leaves a group, and a tab that starts or stops playing sound.
  if (info.groupId !== undefined || info.audible !== undefined) void refreshGroups();
});

chrome.tabs.onAttached.addListener(() => void refreshGroups());
chrome.tabGroups.onRemoved.addListener(() => void refreshGroups());

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
    await forgetLabel(tabId);
    await refreshGroups();
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
  await chrome.storage.session.set({ captured: [], failed: [], groups: [], labels: {} });
  await chrome.action.setBadgeText({ text: "" });
}

async function captureGroup(tabId: number): Promise<void> {
  const tab = await chrome.tabs.get(tabId);
  if (tab.groupId === undefined || tab.groupId < 0) {
    throw new Error("This tab is not in a tab group.");
  }
  await trackGroup(tab.groupId);
  // Chrome allows the capture of this tab only; the others wait for their own action.
  if (!(await readCaptured()).includes(tabId)) await capture(tabId);
  else await refreshGroups();
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
    case "group-view":
      return { group: await groupView(msg.tabId) };
    case "capture-group":
      await captureGroup(msg.tabId);
      return {};
    case "next-tab":
      return { tabId: await activateNext(msg.tabId) };
    case "started":
      // The offscreen document started a stream (this is also the path of the test hook).
      await onCaptured(msg.tabId, { title: msg.title, host: msg.host });
      return {};
    case "ended":
      await setCaptured(msg.tabId, false);
      await refreshGroups();
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
      if ((await readCaptured()).includes(tabId)) {
        await stopCapture(tabId);
      } else {
        await capture(tabId);
        // In a tab group: on to the next tab that still needs its own action.
        await activateNext(tabId);
      }
    } catch (err) {
      await chrome.action.setBadgeText({ tabId, text: "ERR" });
      await chrome.action.setBadgeBackgroundColor({ tabId, color: BADGE_REC });
      console.warn("capture-tab failed:", err);
    }
  })();
});

chrome.runtime.onStartup.addListener(() => void ensureConnected());
chrome.runtime.onInstalled.addListener(() => void ensureConnected());
