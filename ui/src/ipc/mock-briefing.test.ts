import { describe, expect, it } from "vitest";
import { createMockBackend } from "./mock";

const backend = () => createMockBackend({ clockRate: 0 });

describe("the mock briefing", () => {
  it("groups the demo tasks by person, with the unknown login and the unassigned last", async () => {
    const b = await backend().getBriefing("NOMAD", false);
    expect(b.groups.map((g) => g.name)).toEqual([
      "Alex Demo",
      "Jan Demo",
      "Kasia Demo",
      "Piotr Demo",
      "@octo-guest",
      "Unassigned",
    ]);
    expect(b.groups.map((g) => g.kind)).toEqual([
      "participant",
      "participant",
      "participant",
      "participant",
      "other",
      "unassigned",
    ]);
    // Sorted by due date within a person, overdue against the mock's today.
    const alex = b.groups[0];
    expect(alex.tasks.map((t) => t.due)).toEqual(["2026-11-18", "2026-11-20"]);
    expect(b.today).toBe("2026-11-13");
    expect(b.groups[1].tasks[0]).toMatchObject({ id: "#12", overdue: true });
    expect(b.overdueCount).toBe(2);
    expect(b.openCount).toBe(7);
    expect(b.done).toHaveLength(1);
    // Open topics only.
    expect(b.topics.map((t) => t.id)).toEqual(["t-1", "t-3"]);
  });

  it("pulls a clone only on refresh and moves its date", async () => {
    const b = backend();
    const before = await b.getBriefing("NOMAD", false);
    expect(before).toMatchObject({ hasRemote: true, stateAsOf: "2026-11-12", pull: null });
    const after = await b.getBriefing("NOMAD", true);
    expect(after).toMatchObject({ stateAsOf: "2026-11-13", pull: "updated", offline: false });
    expect((await b.getBriefing("NOMAD", true)).pull).toBe("upToDate");
  });

  it("keeps the old date and says why when the clone cannot be pulled", async () => {
    const b = backend();
    await b.addCycleRepo({ kind: "clone", url: "https://offline.example/offline-cycles.git" });
    const first = await b.getBriefing("OFFLINE-CYCLES", false);
    expect(first.offline).toBe(false);
    const failed = await b.getBriefing("OFFLINE-CYCLES", true);
    expect(failed).toMatchObject({ pull: "failed", offline: true, stateAsOf: "2026-11-13" });
    expect(failed.error).toContain("could not resolve the host");
    // The failure is remembered by a later call without refresh.
    expect(await b.getBriefing("OFFLINE-CYCLES", false)).toMatchObject({
      pull: null,
      offline: true,
    });
  });

  it("a local repository has no remote and an unknown cycle is not found", async () => {
    const b = backend();
    expect(await b.getBriefing("SANDBOX", true)).toMatchObject({
      hasRemote: false,
      pull: null,
      openCount: 0,
    });
    await expect(b.getBriefing("GHOST", false)).rejects.toMatchObject({ code: "not_found" });
  });

  it("asks the main window to open a briefing when one listens", async () => {
    const b = backend();
    const seen: string[] = [];
    const off = await b.onOpenBriefing((id) => seen.push(id));
    await b.openBriefing("NOMAD");
    off();
    expect(seen).toEqual(["NOMAD"]);
  });
});
