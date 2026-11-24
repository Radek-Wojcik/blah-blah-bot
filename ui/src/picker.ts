// Pure logic of the source picker: sections, tab groups, the default choice and the warning
// about recording a tab twice. No React and no backend, so it is tested on its own.

import type { SourceInfo, SourceKind } from "./ipc/types";

export type SectionId = "microphone" | "applications" | "browser";

export type SelectionChoice = Set<string>;

export interface SourceSection {
  id: SectionId;
  title: string;
  /** Rows in display order. */
  rows: SourceRow[];
}

/** A group heading (a browser tab group) or a source. */
export type SourceRow = { type: "heading"; text: string } | { type: "source"; source: SourceInfo };

const TITLES: Record<SectionId, string> = {
  microphone: "Microphone",
  applications: "Applications",
  browser: "Browser",
};

function sectionOf(kind: SourceKind): SectionId {
  switch (kind) {
    case "microphone":
      return "microphone";
    case "application":
      return "applications";
    case "browser_tab":
    case "browser":
      return "browser";
  }
}

/**
 * The picker sections in the order Microphone, Applications, Browser; empty ones are left out.
 * In the Browser section the tabs come first, grouped by tab group (a heading for each group,
 * loose tabs last), and the whole browser, the fallback, comes last.
 */
export function groupSources(sources: SourceInfo[]): SourceSection[] {
  const ids: SectionId[] = ["microphone", "applications", "browser"];
  const out: SourceSection[] = [];
  for (const id of ids) {
    const members = sources.filter((s) => sectionOf(s.kind) === id);
    if (members.length === 0) continue;
    out.push({
      id,
      title: TITLES[id],
      rows: id === "browser" ? browserRows(members) : members.map(sourceRow),
    });
  }
  return out;
}

function sourceRow(source: SourceInfo): SourceRow {
  return { type: "source", source };
}

function browserRows(members: SourceInfo[]): SourceRow[] {
  const tabs = members.filter((s) => s.kind === "browser_tab");
  const whole = members.filter((s) => s.kind === "browser");
  const names = [...new Set(tabs.map((t) => t.group).filter((g): g is string => !!g))].sort();
  const rows: SourceRow[] = [];
  for (const name of names) {
    rows.push({ type: "heading", text: name });
    rows.push(...tabs.filter((t) => t.group === name).map(sourceRow));
  }
  const loose = tabs.filter((t) => !t.group);
  if (names.length > 0 && loose.length > 0) rows.push({ type: "heading", text: "Other tabs" });
  rows.push(...loose.map(sourceRow));
  rows.push(...whole.map(sourceRow));
  return rows;
}

/**
 * What is ticked before the user chooses: the microphone (or the first source) and every tab
 * the extension has captured, because capturing a tab is the user's choice to record it.
 */
export function defaultSelection(sources: SourceInfo[]): SelectionChoice {
  if (sources.length === 0) return new Set();
  const first = sources.find((s) => s.kind === "microphone") ?? sources[0];
  return new Set([first.id, ...sources.filter((s) => s.kind === "browser_tab").map((s) => s.id)]);
}

/**
 * Stable identifier for a source across application restarts, even when Windows process PIDs change.
 */
export function sourceStableKey(s: SourceInfo): string {
  switch (s.kind) {
    case "microphone":
      return "mic:default";
    case "browser":
      return "browser:chrome";
    case "browser_tab":
      return s.id.startsWith("tab:") ? s.id : `tab:${s.id}`;
    case "application":
      return `app_name:${(s.name ?? s.label).trim().toLowerCase()}`;
  }
}

/**
 * Restores a persisted source selection from stable keys (or raw source ids), falling back to
 * `defaultSelection(sources)` when no persisted keys match any available source.
 */
export function restorePersistedSelection(
  sources: SourceInfo[],
  lastKeys?: string[] | null,
): SelectionChoice {
  if (sources.length === 0) return new Set();
  if (!lastKeys || lastKeys.length === 0) return defaultSelection(sources);
  const keySet = new Set(lastKeys.map((k) => k.trim().toLowerCase()));
  const rawSet = new Set(lastKeys);
  const picked = new Set<string>();
  for (const s of sources) {
    const stable = sourceStableKey(s);
    if (
      rawSet.has(stable) ||
      keySet.has(stable.toLowerCase()) ||
      rawSet.has(s.id) ||
      keySet.has(s.id.toLowerCase()) ||
      (s.kind === "browser_tab" && rawSet.has(`tab:${s.id}`))
    ) {
      picked.add(s.id);
    }
  }
  return picked.size > 0 ? picked : defaultSelection(sources);
}

/** Converts a set of chosen source ids into stable persistence keys for `saveLastSources`. */
export function selectionStableKeys(
  sources: SourceInfo[],
  selectedIds: Iterable<string>,
): string[] {
  const out: string[] = [];
  for (const id of selectedIds) {
    const src = sources.find((s) => s.id === id);
    const key = src ? sourceStableKey(src) : id;
    if (!out.includes(key)) out.push(key);
  }
  return out;
}

/** The text shown at the end of a row. */
export function kindLabel(source: SourceInfo): string {
  switch (source.kind) {
    case "microphone":
      return "mic";
    case "browser_tab":
      return "tab";
    case "browser":
      return "all Chrome";
    case "application":
      return source.active ? "playing" : "idle";
  }
}

export const DOUBLE_RECORDING_WARNING =
  "A tab and the whole browser are both chosen: the sound of the tab is recorded twice.";

/** A warning when a tab and the whole browser are both among `ids`, otherwise null. */
export function doubleRecording(sources: SourceInfo[], ids: Iterable<string>): string | null {
  const kinds = new Set<SourceKind>();
  for (const id of ids) {
    const source = sources.find((s) => s.id === id);
    if (source) kinds.add(source.kind);
  }
  return kinds.has("browser_tab") && kinds.has("browser") ? DOUBLE_RECORDING_WARNING : null;
}
