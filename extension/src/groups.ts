// Guided capture of a Chrome tab group: pure state logic, no extension API.
//
// Chrome lets the extension capture a tab only after the user invoked it on that tab (click,
// popup or shortcut), so a group is never captured in one action: it is one action per tab,
// and this module tells the user which tabs are still missing and which one to visit next.
//
// Status of a tab in a group, as the popup shows it:
//   captured      a stream of the tab is running
//   waiting       not captured yet; needs a click or the shortcut on the tab
//   not_audible   like waiting, but the tab plays no sound right now (it may be a paused page)
//   failed        the last capture attempt failed; try again on the tab
// The application hears only captured / waiting / failed (`not_audible` is a waiting tab).

import type { Group, GroupTabStatus } from "./protocol.js";

export type TabStatus = "captured" | "waiting" | "not_audible" | "failed";

/** What the extension knows about one tab of the group. */
export interface GroupTab {
  tabId: number;
  /** Position of the tab in the window (`tab.index`); the group is visited left to right. */
  index: number;
  audible: boolean;
  /** Local label only (popup); never sent to the application. */
  title: string;
  host: string;
}

export interface GroupTabState extends GroupTab {
  status: TabStatus;
}

export function tabStatus(
  tab: Pick<GroupTab, "tabId" | "audible">,
  captured: ReadonlySet<number>,
  failed: ReadonlySet<number>,
): TabStatus {
  if (captured.has(tab.tabId)) return "captured";
  if (failed.has(tab.tabId)) return "failed";
  return tab.audible ? "waiting" : "not_audible";
}

/** The tabs of one group with their status, in window order. */
export function groupStatuses(
  tabs: readonly GroupTab[],
  captured: ReadonlySet<number>,
  failed: ReadonlySet<number>,
): GroupTabState[] {
  return [...tabs]
    .sort((a, b) => a.index - b.index)
    .map((t) => ({ ...t, status: tabStatus(t, captured, failed) }));
}

/** A tab that still needs an action of the user. */
export function isMissing(status: TabStatus): boolean {
  return status !== "captured";
}

/** How many tabs of the group still need an action. */
export function missingCount(states: readonly GroupTabState[]): number {
  return states.filter((s) => isMissing(s.status)).length;
}

/** "3/4 tabs": captured tabs of the whole group. */
export function summary(states: readonly GroupTabState[]): string {
  const done = states.filter((s) => s.status === "captured").length;
  return `${done}/${states.length} tabs`;
}

// A tab that plays sound is the most useful one to visit, a tab that failed is a retry.
const PRIORITY: Record<TabStatus, number> = { waiting: 0, not_audible: 1, failed: 2, captured: 3 };

/**
 * The tab to activate next: the tab still missing that comes first after `currentTabId`
 * (wrapping around), preferring tabs that play sound, then silent ones, then failed ones.
 * Null when every tab is captured. The current tab itself is chosen only when it is the
 * single tab left (a retry).
 */
export function nextTab(
  states: readonly GroupTabState[],
  currentTabId?: number | null,
): number | null {
  const missing = states.filter((s) => isMissing(s.status));
  if (missing.length === 0) return null;
  const at = states.findIndex((s) => s.tabId === currentTabId);
  // Distance walking right from the current tab and wrapping; the current tab is last.
  const rank = (s: GroupTabState): number => {
    if (at < 0) return states.indexOf(s);
    const i = states.indexOf(s);
    return i === at ? states.length : (i - at - 1 + states.length) % states.length;
  };
  const sorted = [...missing].sort(
    (a, b) => PRIORITY[a.status] - PRIORITY[b.status] || rank(a) - rank(b),
  );
  return sorted[0].tabId;
}

/** The `group` message for the application. */
export function groupMessage(groupId: number, states: readonly GroupTabState[]): Group {
  return {
    type: "group",
    group_id: groupId,
    tabs: states.map((s) => ({ tab_id: s.tabId, status: wireStatus(s.status) })),
  };
}

export function wireStatus(status: TabStatus): GroupTabStatus {
  if (status === "captured" || status === "failed") return status;
  return "waiting";
}

/** Badge text for the number of tabs that wait for the user: "+1", "" for none. */
export function badgeText(missing: number): string {
  if (missing <= 0) return "";
  return missing > 99 ? "+99" : `+${missing}`;
}

/** Forgets failed marks of tabs that left the group (closed or moved out). */
export function pruneFailed(
  failed: readonly number[],
  tabs: readonly Pick<GroupTab, "tabId">[],
): number[] {
  const present = new Set(tabs.map((t) => t.tabId));
  return failed.filter((id) => present.has(id));
}

/**
 * Name of a tab in the popup list. Chrome gives the extension the title of a tab only after
 * the extension was used on it (no `tabs` permission), so other tabs are named by position.
 */
export function tabLabel(
  tab: Pick<GroupTab, "title" | "host" | "index">,
  position: number,
): string {
  return tab.title.trim() || tab.host || `Tab ${position}`;
}

/** Popup wording of a status. */
export function statusLabel(status: TabStatus): string {
  switch (status) {
    case "captured":
      return "captured";
    case "waiting":
      return "waiting for click";
    case "not_audible":
      return "not audible";
    case "failed":
      return "failed";
  }
}

/** Host of a tab address for labels; empty when the address has none (chrome://, about:). */
export function hostOfUrl(url: string | undefined): string {
  try {
    return url ? new URL(url).hostname : "";
  } catch {
    return "";
  }
}
