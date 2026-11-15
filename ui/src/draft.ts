import type {
  CertaintyLevel,
  Draft,
  DraftItem,
  DraftSection,
  ItemType,
  ReportMode,
} from "./ipc/types";

export const ITEM_TYPE_LABELS: Record<ItemType, string> = {
  task: "Task",
  decision: "Decision",
  topic: "Topic",
  note: "Note",
  idea: "Idea",
  budget: "Budget",
};

export const ITEM_TYPES = Object.keys(ITEM_TYPE_LABELS) as ItemType[];

/** `hh:mm:ss`, always with hours: the time of a quote in the meeting. */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(total / 3600))}:${p(Math.floor((total % 3600) / 60))}:${p(total % 60)}`;
}

/** The due date as shown, or a short note when there is none. */
export function formatDue(due: string | null): string {
  return due === null ? "no due date" : due;
}

/** `YYYY-MM-DD` with a month 1 to 12 and a day 1 to 31, as the backend checks it. */
export function isIsoDate(s: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s)) return false;
  const month = Number(s.slice(5, 7));
  const day = Number(s.slice(8, 10));
  return month >= 1 && month <= 12 && day >= 1 && day <= 31;
}

export interface CertaintyMark {
  /** `~` for a guess, `?` for an unknown value, empty for a stated one. */
  mark: "" | "~" | "?";
  /** The tooltip. */
  title: string;
}

/** The quiet marker next to an assignee or a due date the model was not sure about. */
export function certaintyMark(level: CertaintyLevel): CertaintyMark {
  if (level === "inferred") return { mark: "~", title: "inferred" };
  if (level === "unknown") return { mark: "?", title: "unknown" };
  return { mark: "", title: "" };
}

/** A quote that was not found in the transcript is marked; an item added by hand is not. */
export function isUnverified(item: DraftItem): boolean {
  return item.source !== null && !item.source.verified;
}

/** The match score of a quote as a whole percentage. */
export function matchPercent(match: number): string {
  return `${Math.round(Math.min(1, Math.max(0, match)) * 100)}%`;
}

export const MODE_LABELS: Record<ReportMode, string> = {
  fixed: "Fixed",
  template: "Template",
  auto: "Auto",
  "by-type": "By type",
};

// ----- columns ---------------------------------------------------------------------------

export interface DraftColumn {
  /** The section id; null for a built-in group of the fixed layout. */
  id: string | null;
  /** Key for React, unique among the columns. */
  key: string;
  /** The type of a built-in group of the fixed layout. */
  type?: ItemType;
  title: string;
  description: string;
  items: DraftItem[];
}

const BUILTIN: { type: ItemType; en: string; pl: string }[] = [
  { type: "decision", en: "Decisions", pl: "Decyzje" },
  { type: "task", en: "Tasks", pl: "Zadania" },
  { type: "topic", en: "Open topics", pl: "Otwarte tematy" },
  { type: "note", en: "Notes", pl: "Notatki" },
  { type: "idea", en: "Ideas", pl: "Pomysły" },
  { type: "budget", en: "Budgets", pl: "Budżety" },
];

/** The fixed layout groups items by type and has no sections to move between. */
export function usesSections(draft: Pick<Draft, "mode" | "sections">): boolean {
  return draft.mode !== "fixed" && draft.sections.length > 0;
}

/** The index of the section an item is shown in: unknown or missing ids go to the last one. */
export function sectionIndexOf(sections: DraftSection[], item: DraftItem): number {
  if (sections.length === 0) return -1;
  const found = sections.findIndex((s) => s.id === item.section);
  return found >= 0 ? found : sections.length - 1;
}

/**
 * What the draft editor lists, in the order of the report: the sections (every one, also an
 * empty one, so an item can be added or dropped into it) or, in the fixed layout, the built-in
 * groups by type (only the non-empty ones).
 */
export function buildColumns(draft: Draft): DraftColumn[] {
  if (!usesSections(draft)) {
    return BUILTIN.map((b) => ({
      id: null,
      key: `type:${b.type}`,
      type: b.type,
      title: draft.language === "pl" ? b.pl : b.en,
      description: "",
      items: draft.items.filter((i) => i.type === b.type),
    })).filter((c) => c.items.length > 0);
  }
  const columns: DraftColumn[] = draft.sections.map((s) => ({
    id: s.id,
    key: `section:${s.id}`,
    title: s.title,
    description: s.description,
    items: [],
  }));
  for (const item of draft.items) columns[sectionIndexOf(draft.sections, item)].items.push(item);
  return columns;
}

/** The sections an item can be moved to: every section but the one it is in. */
export function moveTargets(draft: Draft, item: DraftItem): DraftSection[] {
  if (!usesSections(draft)) return [];
  const at = sectionIndexOf(draft.sections, item);
  return draft.sections.filter((_, i) => i !== at);
}

/**
 * The item a dropped item goes in front of. The backend only accepts a neighbour that is in the
 * target section itself (an item without a section is shown in the last one but is not "in" it),
 * so any other target means "at the end".
 */
export function dropBefore(
  targetSectionId: string,
  over: DraftItem | null,
  moving: DraftItem,
): string | undefined {
  if (over === null || over.id === moving.id) return undefined;
  return over.section === targetSectionId ? over.id : undefined;
}

// ----- editing the sections --------------------------------------------------------------

export function addSection(sections: DraftSection[], title: string): DraftSection[] {
  // An empty id asks the backend to make one from the title.
  return [...sections, { id: "", title: title.trim(), description: "" }];
}

export function renameSection(
  sections: DraftSection[],
  index: number,
  title: string,
): DraftSection[] {
  return sections.map((s, i) => (i === index ? { ...s, title } : s));
}

export function removeSection(sections: DraftSection[], index: number): DraftSection[] {
  return sections.filter((_, i) => i !== index);
}

/** Moves a section up (`-1`) or down (`1`); at the ends nothing changes. */
export function moveSection(
  sections: DraftSection[],
  index: number,
  delta: -1 | 1,
): DraftSection[] {
  const to = index + delta;
  if (to < 0 || to >= sections.length) return sections;
  const next = [...sections];
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

/** Why the sections cannot be saved, or null. */
export function sectionsError(sections: DraftSection[]): string | null {
  if (sections.some((s) => s.title.trim() === "")) return "Every section needs a title.";
  const titles = sections.map((s) => s.title.trim().toLowerCase());
  if (new Set(titles).size !== titles.length) return "Two sections have the same title.";
  return null;
}

/** The number of items that would lose their section if `sections` replaced the current ones. */
export function orphanedItems(draft: Draft, sections: DraftSection[]): number {
  const ids = new Set(sections.map((s) => s.id));
  return draft.items.filter((i) => i.section !== null && !ids.has(i.section)).length;
}

/** The layout a draft has, in words: `Auto`, `Template: budget-review`, `By type: budget`. */
export function describeLayout(draft: Draft): string {
  const label = MODE_LABELS[draft.mode];
  if (draft.mode === "template" && draft.template) return `${label}: ${draft.template}`;
  if (draft.mode === "by-type" && draft.meetingType) return `${label}: ${draft.meetingType}`;
  return label;
}
