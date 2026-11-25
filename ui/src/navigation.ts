export type Page = "projects" | "project" | "record" | "queue" | "settings" | "meeting";
export type MeetingTab = "draft" | "transcript" | "speakers";
export type SettingsTab = "models" | "connectors" | "preferences" | "processing" | "voices";

export interface Route {
  page: Page;
  meetingId: string | null;
  cycleId?: string | null;
  returnCycleId?: string | null;
  meetingTab?: MeetingTab;
  highlightMs?: number | null;
  settingsTab?: SettingsTab;
}

export interface HistoryState {
  entries: Route[];
  index: number;
}

export function routesEqual(a: Route, b: Route): boolean {
  return (
    a.page === b.page &&
    (a.meetingId ?? null) === (b.meetingId ?? null) &&
    (a.cycleId ?? null) === (b.cycleId ?? null) &&
    (a.returnCycleId ?? null) === (b.returnCycleId ?? null) &&
    (a.meetingTab ?? undefined) === (b.meetingTab ?? undefined) &&
    (a.highlightMs ?? null) === (b.highlightMs ?? null) &&
    (a.settingsTab ?? undefined) === (b.settingsTab ?? undefined)
  );
}

export function createHistory(initial: Route): HistoryState {
  return { entries: [initial], index: 0 };
}

export function pushRoute(state: HistoryState, next: Route): HistoryState {
  const current = state.entries[state.index];
  if (current && routesEqual(current, next)) {
    return state;
  }
  const entries = [...state.entries.slice(0, state.index + 1), next];
  return { entries, index: entries.length - 1 };
}

export function canGoBack(state: HistoryState): boolean {
  return state.index > 0;
}

export function canGoForward(state: HistoryState): boolean {
  return state.index < state.entries.length - 1;
}

export function stepBack(state: HistoryState): HistoryState {
  if (!canGoBack(state)) return state;
  return { ...state, index: state.index - 1 };
}

export function stepForward(state: HistoryState): HistoryState {
  if (!canGoForward(state)) return state;
  return { ...state, index: state.index + 1 };
}
