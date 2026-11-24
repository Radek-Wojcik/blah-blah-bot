import { formatDateTimeUtc, formatDuration } from "./format";
import type { BridgeClientView, BridgeGroupView, BridgeStatus } from "./ipc/types";

/** Time left on a pairing code as M:SS, or null once it expired. */
export function pairingTimeLeft(expiresUnixMs: number, nowMs: number): string | null {
  const left = expiresUnixMs - nowMs;
  return left > 0 ? formatDuration(left + 999) : null;
}

/** One line about the bridge server for the settings screen. */
export function bridgeSummary(s: BridgeStatus): string {
  if (!s.enabled) return "Off: the browser extension cannot connect.";
  if (!s.listening) return s.error ?? "Not running.";
  const tabs = s.streams.length;
  const recording = tabs === 0 ? "" : ` Recording ${tabs === 1 ? "1 tab" : `${tabs} tabs`}.`;
  return `Listening on 127.0.0.1:${s.port}.${recording}`;
}

/** One line per tab group, such as "Group 1: 3/4 tabs captured". */
export function groupSummary(g: BridgeGroupView, index: number): string {
  const captured = g.tabs.filter((t) => t.status === "captured").length;
  const failed = g.tabs.filter((t) => t.status === "failed").length;
  const failure = failed === 0 ? "" : `, ${failed} failed`;
  return `Group ${index + 1}: ${captured}/${g.tabs.length} tabs captured${failure}`;
}

/** The second line of a paired extension. */
export function clientDetails(c: BridgeClientView): string {
  const state = c.connected ? "connected" : "not connected";
  return `extension ${c.extVersion} · paired ${formatDateTimeUtc(c.pairedUnixMs)} UTC · ${state}`;
}
