import { describe, expect, it } from "vitest";
import { daysBetween, dueLabel, plural, sourceLabel, statusLine, summaryLine } from "./briefing";

describe("dates", () => {
  it("counts whole days, also across a month and a year", () => {
    expect(daysBetween("2026-11-10", "2026-11-13")).toBe(3);
    expect(daysBetween("2026-10-30", "2026-11-02")).toBe(3);
    expect(daysBetween("2026-12-30", "2027-01-02")).toBe(3);
    expect(daysBetween("2026-11-13", "2026-11-10")).toBe(-3);
  });

  it("labels the due date", () => {
    const today = "2026-11-13";
    expect(dueLabel({ due: null, overdue: false }, today)).toBe("No due date");
    expect(dueLabel({ due: "2026-11-20", overdue: false }, today)).toBe("Due 2026-11-20");
    expect(dueLabel({ due: "2026-11-13", overdue: false }, today)).toBe("Due today");
    expect(dueLabel({ due: "2026-11-10", overdue: true }, today)).toBe(
      "Overdue by 3 days (2026-11-10)",
    );
    expect(dueLabel({ due: "2026-11-12", overdue: true }, today)).toBe(
      "Overdue by 1 day (2026-11-12)",
    );
  });
});

describe("wording", () => {
  it("pluralises", () => {
    expect(plural(1, "task", "tasks")).toBe("1 task");
    expect(plural(0, "task", "tasks")).toBe("0 tasks");
    expect(plural(2, "task", "tasks")).toBe("2 tasks");
  });

  it("names where a task comes from", () => {
    expect(sourceLabel({ source: "issue", issue: 12, meeting: null })).toBe("Issue #12");
    expect(sourceLabel({ source: "meeting", issue: null, meeting: "2026-11-06" })).toBe(
      "Meeting 2026-11-06",
    );
    expect(sourceLabel({ source: "meeting", issue: 12, meeting: "2026-11-06" })).toBe(
      "Meeting 2026-11-06 · issue #12",
    );
  });

  it("summarises the screen", () => {
    expect(summaryLine({ openCount: 7, overdueCount: 2, topics: [{}, {}, {}] as never })).toBe(
      "7 open tasks, 2 overdue · 3 open topics",
    );
    expect(summaryLine({ openCount: 1, overdueCount: 0, topics: [{}] as never })).toBe(
      "1 open task · 1 open topic",
    );
  });
});

describe("the status line", () => {
  const clone = {
    hasRemote: true,
    stateAsOf: "2026-11-12",
    offline: false,
    error: null,
    pull: null,
  } as const;

  it("says the state of a clone, with its date", () => {
    expect(statusLine(clone, false)).toEqual({ text: "State as of 2026-11-12.", tone: "ok" });
    expect(statusLine({ ...clone, pull: "updated", stateAsOf: "2026-11-13" }, false).text).toBe(
      "State as of 2026-11-13. Updated just now.",
    );
    expect(statusLine({ ...clone, pull: "upToDate" }, false).text).toBe("State as of 2026-11-12.");
  });

  it("shows that it is updating", () => {
    expect(statusLine(clone, true).text).toBe("State as of 2026-11-12. Updating from the remote…");
  });

  it("keeps the date and gives the reason when the update failed", () => {
    const failed = statusLine(
      { ...clone, offline: true, error: "could not resolve the host", pull: "failed" },
      false,
    );
    expect(failed.tone).toBe("stale");
    expect(failed.text).toBe(
      "State as of 2026-11-12. Could not update: could not resolve the host",
    );
    // A failure remembered from an earlier call (no refresh now) reads the same.
    expect(statusLine({ ...clone, offline: true, error: "offline" }, false).text).toContain(
      "State as of 2026-11-12. Could not update",
    );
    // Even while a new try runs, the failure of the last one is not hidden.
    expect(statusLine({ ...clone, offline: true, error: "x" }, true).tone).toBe("stale");
    // No reason known.
    expect(statusLine({ ...clone, offline: true }, false).text).toBe(
      "State as of 2026-11-12. Could not update",
    );
  });

  it("explains an update that was skipped for uncommitted edits", () => {
    const s = statusLine({ ...clone, pull: "skippedDirty" }, false);
    expect(s.tone).toBe("stale");
    expect(s.text).toContain("State as of 2026-11-12.");
    expect(s.text).toContain("not committed");
  });

  it("handles a clone without a date and a local repository", () => {
    expect(statusLine({ ...clone, stateAsOf: null }, false)).toEqual({
      text: "Not pulled yet.",
      tone: "stale",
    });
    expect(statusLine({ ...clone, stateAsOf: null, offline: true }, false).text).toContain(
      "State as of an unknown date",
    );
    expect(
      statusLine(
        { hasRemote: false, stateAsOf: null, offline: false, error: null, pull: null },
        false,
      ).text,
    ).toBe("Local repository, nothing to update.");
    expect(
      statusLine(
        { hasRemote: false, stateAsOf: "2026-11-06", offline: false, error: null, pull: null },
        false,
      ).text,
    ).toBe("Local repository, nothing to update. Issues as of 2026-11-06.");
  });
});
