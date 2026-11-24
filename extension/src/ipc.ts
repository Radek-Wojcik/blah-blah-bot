// Messages between the popup, the service worker and the offscreen document
// (chrome.runtime.sendMessage; `target` names the receiver).

import type { Credentials, Group, StopReason } from "./protocol.js";
import type { TabStatus } from "./groups.js";
import type { ClientState } from "./bridge-client.js";
import type { StreamStatusInfo } from "./mux.js";

export interface MeetTab {
  tabId: number;
  host: string;
}

export type ToOffscreen =
  | {
      target: "offscreen";
      type: "connect";
      extVersion: string;
      credentials: Credentials;
      port?: number;
      meetTabs: MeetTab[];
    }
  | { target: "offscreen"; type: "pair"; extVersion: string; code: string; meetTabs: MeetTab[] }
  | { target: "offscreen"; type: "disconnect" }
  | {
      target: "offscreen";
      type: "start";
      tabId: number;
      mediaStreamId: string;
      title: string;
      host: string;
      groupId?: number;
    }
  | { target: "offscreen"; type: "stop"; tabId: number; reason?: StopReason }
  | { target: "offscreen"; type: "meet"; tabId: number; host: string; state: "joined" | "left" }
  | { target: "offscreen"; type: "group"; group: Group }
  | { target: "offscreen"; type: "status" };

export interface OffscreenStatus {
  clientState: ClientState;
  detail?: string;
  port: number | null;
  appRecording: boolean;
  streams: StreamStatusInfo[];
}

export type ToServiceWorker =
  | { target: "service-worker"; type: "status" }
  | { target: "service-worker"; type: "pair"; code: string }
  | { target: "service-worker"; type: "unpair" }
  | { target: "service-worker"; type: "capture"; tabId: number }
  | { target: "service-worker"; type: "stop"; tabId: number }
  | { target: "service-worker"; type: "group-view"; tabId: number }
  | { target: "service-worker"; type: "capture-group"; tabId: number }
  | { target: "service-worker"; type: "next-tab"; tabId: number }
  // from the offscreen document
  | {
      target: "service-worker";
      type: "started";
      tabId: number;
      streamId: string;
      title: string;
      host: string;
    }
  | {
      target: "service-worker";
      type: "ended";
      tabId: number;
      streamId: string;
      reason: StopReason | "recording_ended";
    }
  | { target: "service-worker"; type: "fatal"; code: string }
  | { target: "service-worker"; type: "port"; port: number };

/** The tab group of the active tab as the popup shows it. */
export interface GroupView {
  groupId: number;
  /** Title of the group as set in Chrome (local label, never sent to the application). */
  title: string;
  summary: string;
  tabs: { tabId: number; label: string; host: string; status: TabStatus; active: boolean }[];
  /** The tab "Next tab" activates, null when nothing is missing. */
  next: number | null;
}

export interface StatusReply {
  paired: boolean;
  clientState: ClientState | "no-offscreen";
  detail?: string;
  appRecording: boolean;
  streams: StreamStatusInfo[];
  meetTabs: MeetTab[];
  version: string;
}

export type Reply<T = object> = ({ ok: true } & T) | { ok: false; error: string };
