// Popup: pairing form and the capture toggle for the active tab. Opening the popup is the
// user gesture that grants the extension access to the active tab (activeTab).

import type { Reply, StatusReply, ToServiceWorker } from "./ipc.js";
import { isMeetCallUrl } from "./meet.js";

const $ = <T extends HTMLElement>(id: string): T => document.getElementById(id) as T;

type Without<T, K extends string> = T extends unknown ? Omit<T, K> : never;

async function send<T extends object = object>(
  msg: Without<ToServiceWorker, "target">,
): Promise<T> {
  const reply = (await chrome.runtime.sendMessage({ target: "service-worker", ...msg })) as
    Reply<T> | undefined;
  if (!reply || !reply.ok) throw new Error((reply && !reply.ok && reply.error) || "no reply");
  return reply;
}

function show(text: string, cls = ""): void {
  const el = $("line");
  el.textContent = text;
  el.className = cls;
}

let activeTabId: number | null = null;
let activeUrl: string | undefined;

function describe(s: StatusReply): [string, string] {
  if (!s.paired) return ["Not paired. Start pairing in the application and enter the code.", ""];
  switch (s.clientState) {
    case "ready":
      return s.appRecording
        ? ["Connected. The application is recording.", "ok"]
        : ["Connected. A tab is recorded once a recording runs in the application.", "ok"];
    case "stopped":
      return ["Disconnected. Pair again.", "err"];
    default:
      return ["Waiting for the application. Is Blah Blah Bot running?", "err"];
  }
}

async function render(): Promise<StatusReply> {
  const { status } = await send<{ status: StatusReply }>({ type: "status" });
  $("pair").hidden = status.paired;
  $("capture").hidden = !status.paired;
  $("count").textContent = String(status.streams.length);
  const capturing = status.streams.some((x) => x.tabId === activeTabId);
  $("captureBtn").textContent = capturing ? "Stop this tab" : "Capture this tab";
  const [text, cls] = describe(status);
  const meet = isMeetCallUrl(activeUrl) ? " This tab is a Google Meet call." : "";
  show(text + meet, cls);
  return status;
}

async function capturing(): Promise<boolean> {
  const { status } = await send<{ status: StatusReply }>({ type: "status" });
  return status.streams.some((x) => x.tabId === activeTabId);
}

async function main(): Promise<void> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  activeTabId = tab?.id ?? null;
  activeUrl = tab?.url;
  const local = await chrome.storage.local.get("autoCapture");
  const auto = local.autoCapture !== false;
  ($("auto") as HTMLInputElement).checked = auto;

  const status = await render();

  $("pairBtn").onclick = async () => {
    try {
      await send({ type: "pair", code: ($("code") as HTMLInputElement).value.trim() });
      ($("code") as HTMLInputElement).value = "";
      await render();
      show("Paired.", "ok");
    } catch (err) {
      show((err as Error).message, "err");
    }
  };

  $("captureBtn").onclick = async () => {
    if (activeTabId === null) return;
    try {
      if (await capturing()) await send({ type: "stop", tabId: activeTabId });
      else await send({ type: "capture", tabId: activeTabId });
      await render();
    } catch (err) {
      show((err as Error).message, "err");
    }
  };

  $("unpairBtn").onclick = async () => {
    try {
      await send({ type: "unpair" });
      await render();
    } catch (err) {
      show((err as Error).message, "err");
    }
  };

  $("auto").onchange = () =>
    void chrome.storage.local.set({ autoCapture: ($("auto") as HTMLInputElement).checked });

  if (auto && status.paired && activeTabId !== null && !(await capturing())) {
    try {
      await send({ type: "capture", tabId: activeTabId });
      await render();
    } catch (err) {
      show((err as Error).message, "err");
    }
  }
}

main().catch((err: unknown) => show(String((err as Error).message ?? err), "err"));
