// Messages between the popup, the service worker and the offscreen document
// (chrome.runtime.sendMessage; `target` names the receiver).

import type { Credentials, StopReason } from "./protocol.js";
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
  // from the offscreen document
  | {
      target: "service-worker";
      type: "ended";
      tabId: number;
      streamId: string;
      reason: StopReason | "recording_ended";
    }
  | { target: "service-worker"; type: "fatal"; code: string }
  | { target: "service-worker"; type: "port"; port: number };

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
