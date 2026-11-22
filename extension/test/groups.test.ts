import { describe, expect, it } from "vitest";
import {
  badgeText,
  groupMessage,
  groupStatuses,
  hostOfUrl,
  missingCount,
  nextTab,
  pruneFailed,
  statusLabel,
  tabLabel,
  summary,
  tabStatus,
  wireStatus,
  type GroupTab,
} from "../src/groups.js";

const tab = (tabId: number, index: number, audible = true): GroupTab => ({
  tabId,
  index,
  audible,
  title: `Title ${tabId}`,
  host: `h${tabId}.example`,
});
const set = (...ids: number[]) => new Set(ids);

describe("tabStatus", () => {
  it("is captured for a captured tab, even if it failed before", () => {
    expect(tabStatus(tab(1, 0), set(1), set(1))).toBe("captured");
  });
  it("is failed for a tab whose capture failed", () => {
    expect(tabStatus(tab(1, 0), set(), set(1))).toBe("failed");
  });
  it("is waiting for an audible tab and not_audible for a silent one", () => {
    expect(tabStatus(tab(1, 0, true), set(), set())).toBe("waiting");
    expect(tabStatus(tab(1, 0, false), set(), set())).toBe("not_audible");
  });
});

describe("groupStatuses", () => {
  it("lists the tabs in window order with their status", () => {
    const states = groupStatuses([tab(3, 2), tab(1, 0), tab(2, 1, false)], set(1), set(3));
    expect(states.map((s) => [s.tabId, s.status])).toEqual([
      [1, "captured"],
      [2, "not_audible"],
      [3, "failed"],
    ]);
  });

  it("counts the missing tabs and writes the summary", () => {
    const states = groupStatuses([tab(1, 0), tab(2, 1), tab(3, 2), tab(4, 3)], set(1, 2, 3), set());
    expect(missingCount(states)).toBe(1);
    expect(summary(states)).toBe("3/4 tabs");
    expect(summary([])).toBe("0/0 tabs");
  });
});

describe("nextTab", () => {
  const four = (captured: number[], failed: number[] = [], silent: number[] = []) =>
    groupStatuses(
      [1, 2, 3, 4].map((id) => tab(id, id - 1, !silent.includes(id))),
      new Set(captured),
      new Set(failed),
    );

  it("is the first missing tab right of the current one", () => {
    expect(nextTab(four([1]), 1)).toBe(2);
    expect(nextTab(four([1, 2]), 2)).toBe(3);
  });

  it("skips captured tabs and wraps around", () => {
    expect(nextTab(four([1, 2, 3]), 3)).toBe(4);
    expect(nextTab(four([2, 3, 4]), 4)).toBe(1);
    expect(nextTab(four([1, 3]), 3)).toBe(4);
    expect(nextTab(four([3, 4]), 4)).toBe(1);
  });

  it("is null when every tab is captured", () => {
    expect(nextTab(four([1, 2, 3, 4]), 2)).toBeNull();
    expect(nextTab([], 1)).toBeNull();
  });

  it("prefers tabs that play sound, then silent ones, then failed ones", () => {
    expect(nextTab(four([1], [], [2]), 1)).toBe(3); // 2 is silent, 3 plays sound
    expect(nextTab(four([1, 3, 4], [], [2]), 1)).toBe(2);
    expect(nextTab(four([1], [2], []), 1)).toBe(3); // 2 failed before
    expect(nextTab(four([1, 3, 4], [2], []), 1)).toBe(2);
  });

  it("starts from the left when the current tab is not in the group", () => {
    expect(nextTab(four([1]), 99)).toBe(2);
    expect(nextTab(four([]), undefined)).toBe(1);
  });

  it("returns to the current tab only when it is the one left (a retry)", () => {
    expect(nextTab(four([1, 2, 4], [3]), 3)).toBe(3);
    expect(nextTab(four([1, 2], []), 3)).toBe(4);
  });
});

describe("a tab added to the group later", () => {
  it("becomes waiting, counts for the badge and appears in the message", () => {
    const before = groupStatuses([tab(1, 0), tab(2, 1)], set(1, 2), set());
    expect(missingCount(before)).toBe(0);
    expect(badgeText(missingCount(before))).toBe("");

    const after = groupStatuses([tab(1, 0), tab(2, 1), tab(9, 2)], set(1, 2), set());
    expect(after.find((s) => s.tabId === 9)?.status).toBe("waiting");
    expect(missingCount(after)).toBe(1);
    expect(badgeText(missingCount(after))).toBe("+1");
    expect(groupMessage(7, after)).toEqual({
      type: "group",
      group_id: 7,
      tabs: [
        { tab_id: 1, status: "captured" },
        { tab_id: 2, status: "captured" },
        { tab_id: 9, status: "waiting" },
      ],
    });
    expect(nextTab(after, 2)).toBe(9);
  });
});

describe("a closed tab", () => {
  it("leaves the group message and the failed marks", () => {
    const states = groupStatuses([tab(1, 0), tab(3, 2)], set(1), set(2, 3));
    expect(groupMessage(7, states).tabs.map((t) => t.tab_id)).toEqual([1, 3]);
    expect(pruneFailed([2, 3], states)).toEqual([3]);
  });

  it("an empty group has an empty message", () => {
    expect(groupMessage(7, [])).toEqual({ type: "group", group_id: 7, tabs: [] });
  });
});

describe("wire format", () => {
  it("sends silent tabs as waiting", () => {
    expect(wireStatus("not_audible")).toBe("waiting");
    expect(wireStatus("waiting")).toBe("waiting");
    expect(wireStatus("captured")).toBe("captured");
    expect(wireStatus("failed")).toBe("failed");
  });

  it("never carries a title or an address", () => {
    const states = groupStatuses([tab(1, 0)], set(), set());
    const text = JSON.stringify(groupMessage(1, states));
    expect(text).not.toContain("Title");
    expect(text).not.toContain("example");
  });
});

describe("labels", () => {
  it("writes the popup wording", () => {
    expect(statusLabel("captured")).toBe("captured");
    expect(statusLabel("waiting")).toBe("waiting for click");
    expect(statusLabel("not_audible")).toBe("not audible");
    expect(statusLabel("failed")).toBe("failed");
  });

  it("names a tab by its title, then its host, then its position", () => {
    expect(tabLabel({ title: " Plan ", host: "a.example", index: 0 }, 1)).toBe("Plan");
    expect(tabLabel({ title: "", host: "a.example", index: 0 }, 1)).toBe("a.example");
    expect(tabLabel({ title: "", host: "", index: 2 }, 3)).toBe("Tab 3");
  });

  it("caps the badge at 99", () => {
    expect(badgeText(0)).toBe("");
    expect(badgeText(3)).toBe("+3");
    expect(badgeText(250)).toBe("+99");
  });

  it("takes only the host of an address", () => {
    expect(hostOfUrl("https://docs.example.org/a/b?q=1#x")).toBe("docs.example.org");
    expect(hostOfUrl("chrome://extensions")).toBe("extensions");
    expect(hostOfUrl("not a url")).toBe("");
    expect(hostOfUrl(undefined)).toBe("");
  });
});
