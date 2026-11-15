import { describe, expect, it } from "vitest";
import {
  cycleIdError,
  duplicateTypes,
  emptyTemplate,
  isDirty,
  issueText,
  issuesAt,
  issuesOf,
  newCycleError,
  repoState,
  rowsToMap,
  templateDirty,
  typeRows,
  withTypeRows,
} from "./cycles";
import { demoCycle } from "./ipc/mock-cycles";

describe("new cycle", () => {
  it("accepts an identifier and refuses spaces, dots and slashes", () => {
    expect(cycleIdError("NOMAD")).toBeNull();
    expect(cycleIdError("rover-2_a")).toBeNull();
    expect(cycleIdError("")).toBe("Enter an id.");
    for (const bad of ["a b", "a.b", "a/b", "-a", "_a"]) expect(cycleIdError(bad)).not.toBeNull();
  });

  it("needs a name of at most 120 characters", () => {
    expect(newCycleError("NOMAD", "Nomad rover")).toBeNull();
    expect(newCycleError("NOMAD", "  ")).toBe("Enter a name.");
    expect(newCycleError("NOMAD", "x".repeat(121))).toMatch(/at most 120/);
    expect(newCycleError("a b", "Name")).not.toBeNull();
  });
});

describe("issues", () => {
  const issues = [
    { path: "participants[1].id", message: "duplicate" },
    { path: "participants[1].name", message: "twice" },
    { path: "participants[10].id", message: "empty" },
    { path: "report.by_type.budget", message: "missing template" },
  ];

  it("finds the issues of a field, exactly", () => {
    expect(issuesOf(issues, "participants[1].id")).toHaveLength(1);
    expect(issuesOf(issues, "participants[1]")).toHaveLength(0);
  });

  it("finds everything inside a path, but not a neighbour that shares the prefix", () => {
    expect(issuesAt(issues, "participants[1]").map((i) => i.message)).toEqual([
      "duplicate",
      "twice",
    ]);
    expect(issuesAt(issues, "report")).toHaveLength(1);
  });

  it("prints a path and a message on one line", () => {
    expect(issueText(issues[0])).toBe("participants[1].id: duplicate");
    expect(issueText({ path: "", message: "bad" })).toBe("bad");
  });
});

describe("meeting type rows", () => {
  it("turns the map into sorted rows and back", () => {
    const rows = typeRows({ technical: "default", budget: "budget-review" });
    expect(rows.map((r) => r.type)).toEqual(["budget", "technical"]);
    expect(rowsToMap(rows)).toEqual({ budget: "budget-review", technical: "default" });
  });

  it("drops blank types and lets a later row win", () => {
    expect(
      rowsToMap([
        { type: " ", template: "x" },
        { type: "a", template: "one" },
        { type: "a", template: "two" },
      ]),
    ).toEqual({ a: "two" });
    expect(
      duplicateTypes([
        { type: "a", template: "one" },
        { type: "b", template: "one" },
        { type: "a", template: "two" },
      ]),
    ).toEqual(["a"]);
  });
});

describe("dirty", () => {
  const saved = demoCycle();

  it("is false for the same cycle, whatever the order of the meeting types", () => {
    const rows = typeRows(saved.report.byType).reverse();
    expect(isDirty(saved, withTypeRows(saved, rows))).toBe(false);
  });

  it("is true after any change", () => {
    expect(isDirty(saved, { ...saved, name: "Nomad" })).toBe(true);
    const more = typeRows(saved.report.byType);
    more.push({ type: "weekly", template: "default" });
    expect(isDirty(saved, withTypeRows(saved, more))).toBe(true);
    expect(isDirty(saved, { ...saved, participants: saved.participants.slice(1) })).toBe(true);
  });

  it("compares templates with what was saved, and a new template is always unsaved", () => {
    const t = emptyTemplate("weekly");
    expect(templateDirty(null, t)).toBe(true);
    expect(templateDirty(t, structuredClone(t))).toBe(false);
    expect(templateDirty(t, { ...t, body: `${t.body}x` })).toBe(true);
  });
});

describe("repoState", () => {
  it("shows the date of the last pull of a clone and says so for a local repository", () => {
    expect(repoState({ kind: "clone", remote: "https://x/y.git", stateAsOf: "2026-11-12" })).toBe(
      "State as of 2026-11-12",
    );
    expect(repoState({ kind: "local", remote: null, stateAsOf: null })).toBe("Local repository");
    expect(repoState({ kind: "clone", remote: "https://x/y.git", stateAsOf: null })).toBe(
      "Not pulled yet",
    );
  });
});
