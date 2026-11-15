import { describe, expect, it } from "vitest";
import {
  addSection,
  buildColumns,
  certaintyMark,
  describeLayout,
  dropBefore,
  formatClock,
  formatDue,
  isIsoDate,
  isUnverified,
  matchPercent,
  moveSection,
  moveTargets,
  orphanedItems,
  removeSection,
  renameSection,
  sectionIndexOf,
  sectionsError,
  usesSections,
} from "./draft";
import { demoDraft } from "./ipc/mock-draft";
import type { Draft, DraftItem } from "./ipc/types";

const draft = (patch: Partial<Draft> = {}): Draft => ({ ...demoDraft("m5"), ...patch });
const item = (d: Draft, id: string): DraftItem => d.items.find((i) => i.id === id)!;

describe("times and dates", () => {
  it("formats the time of a quote as hh:mm:ss", () => {
    expect(formatClock(0)).toBe("00:00:00");
    expect(formatClock(41_000)).toBe("00:00:41");
    expect(formatClock(3_725_900)).toBe("01:02:05");
    expect(formatClock(-5)).toBe("00:00:00");
  });

  it("says so when there is no due date", () => {
    expect(formatDue("2026-11-20")).toBe("2026-11-20");
    expect(formatDue(null)).toBe("no due date");
  });

  it("accepts only whole ISO dates", () => {
    expect(isIsoDate("2026-11-20")).toBe(true);
    expect(isIsoDate("2026-13-01")).toBe(false);
    expect(isIsoDate("2026-11-32")).toBe(false);
    expect(isIsoDate("2026-1-2")).toBe(false);
    expect(isIsoDate("")).toBe(false);
  });

  it("shows the match score as a whole percentage", () => {
    expect(matchPercent(0.956)).toBe("96%");
    expect(matchPercent(1.4)).toBe("100%");
    expect(matchPercent(-1)).toBe("0%");
  });
});

describe("certainty markers", () => {
  it("marks an inferred value with ~ and an unknown one with ?, a stated one not at all", () => {
    expect(certaintyMark("inferred")).toEqual({ mark: "~", title: "inferred" });
    expect(certaintyMark("unknown")).toEqual({ mark: "?", title: "unknown" });
    expect(certaintyMark("stated").mark).toBe("");
  });

  it("marks only a quote that was looked for and not found", () => {
    const d = draft();
    expect(isUnverified(item(d, "i05"))).toBe(true);
    expect(isUnverified(item(d, "i01"))).toBe(false);
    // Added by hand: no source, so no marker.
    expect(isUnverified(item(d, "i12"))).toBe(false);
  });
});

describe("columns", () => {
  it("lists every section in order, items in the order of the draft", () => {
    const cols = buildColumns(draft());
    expect(cols.map((c) => c.title)).toEqual([
      "Thruster tests",
      "Ground segment",
      "Budget and procurement",
      "Follow-ups",
    ]);
    expect(cols[0].items.map((i) => i.id)).toEqual(["i01", "i02", "i11"]);
    expect(cols.reduce((n, c) => n + c.items.length, 0)).toBe(12);
  });

  it("shows an item without a section, or with an unknown one, in the last section", () => {
    const d = draft();
    item(d, "i01").section = null;
    item(d, "i02").section = "gone";
    const cols = buildColumns(d);
    expect(cols[3].items.map((i) => i.id)).toEqual(expect.arrayContaining(["i01", "i02"]));
    expect(sectionIndexOf(d.sections, item(d, "i01"))).toBe(3);
    expect(sectionIndexOf([], item(d, "i01"))).toBe(-1);
  });

  it("keeps an empty section, so an item can be dropped into it", () => {
    const d = draft({
      sections: [...draft().sections, { id: "later", title: "Later", description: "" }],
    });
    const cols = buildColumns(d);
    expect(cols[cols.length - 1].items).toEqual([]);
  });

  it("groups by type in the fixed layout, without the empty groups, in the report language", () => {
    const fixed = draft({ mode: "fixed" });
    expect(usesSections(fixed)).toBe(false);
    expect(buildColumns(fixed).map((c) => c.title)).toEqual([
      "Decisions",
      "Tasks",
      "Open topics",
      "Notes",
      "Ideas",
      "Budgets",
    ]);
    const pl = buildColumns(draft({ mode: "fixed", language: "pl" }));
    expect(pl[0].title).toBe("Decyzje");
    const only = draft({ mode: "fixed", items: draft().items.filter((i) => i.type === "task") });
    expect(buildColumns(only).map((c) => c.type)).toEqual(["task"]);
  });

  it("uses the built-in groups when a non-fixed draft has no sections", () => {
    expect(usesSections(draft({ sections: [] }))).toBe(false);
    expect(usesSections(draft({ mode: "template" }))).toBe(true);
  });
});

describe("moving items", () => {
  it("offers every section but the one the item is in", () => {
    const d = draft();
    expect(moveTargets(d, item(d, "i01")).map((s) => s.id)).toEqual([
      "ground-segment",
      "budget",
      "follow-ups",
    ]);
    // An item shown in the last section by default cannot be moved there.
    item(d, "i01").section = null;
    expect(moveTargets(d, item(d, "i01")).map((s) => s.id)).not.toContain("follow-ups");
  });

  it("offers no target in the fixed layout", () => {
    const d = draft({ mode: "fixed" });
    expect(moveTargets(d, item(d, "i01"))).toEqual([]);
  });

  it("drops in front of a neighbour only when it is in the target section", () => {
    const d = draft();
    const moving = item(d, "i01");
    expect(dropBefore("budget", item(d, "i04"), moving)).toBe("i04");
    expect(dropBefore("budget", item(d, "i03"), moving)).toBeUndefined();
    expect(dropBefore("budget", null, moving)).toBeUndefined();
    expect(dropBefore("thruster-tests", moving, moving)).toBeUndefined();
  });
});

describe("editing the sections", () => {
  const sections = draft().sections;

  it("adds a section with an empty id, so the backend makes one from the title", () => {
    const next = addSection(sections, "  Risks ");
    expect(next).toHaveLength(5);
    expect(next[4]).toEqual({ id: "", title: "Risks", description: "" });
  });

  it("renames without touching the id", () => {
    const next = renameSection(sections, 1, "Ground station");
    expect(next[1]).toMatchObject({ id: "ground-segment", title: "Ground station" });
    expect(sections[1].title).toBe("Ground segment");
  });

  it("removes and reorders", () => {
    expect(removeSection(sections, 0).map((s) => s.id)).toEqual([
      "ground-segment",
      "budget",
      "follow-ups",
    ]);
    expect(moveSection(sections, 2, -1).map((s) => s.id)).toEqual([
      "thruster-tests",
      "budget",
      "ground-segment",
      "follow-ups",
    ]);
    expect(moveSection(sections, 0, -1)).toBe(sections);
    expect(moveSection(sections, 3, 1)).toBe(sections);
  });

  it("refuses an empty or repeated title", () => {
    expect(sectionsError(sections)).toBeNull();
    expect(sectionsError(renameSection(sections, 0, "  "))).toMatch(/needs a title/);
    expect(sectionsError(renameSection(sections, 0, "FOLLOW-UPS"))).toMatch(/same title/);
  });

  it("counts the items that lose their section", () => {
    const d = draft();
    expect(orphanedItems(d, sections)).toBe(0);
    expect(orphanedItems(d, removeSection(sections, 2))).toBe(4);
  });
});

describe("describeLayout", () => {
  it("names the mode and what it uses", () => {
    expect(describeLayout(draft())).toBe("Auto");
    expect(describeLayout(draft({ mode: "template", template: "budget-review" }))).toBe(
      "Template: budget-review",
    );
    expect(describeLayout(draft({ mode: "by-type", meetingType: "budget" }))).toBe(
      "By type: budget",
    );
    expect(describeLayout(draft({ mode: "fixed" }))).toBe("Fixed");
  });
});
