import { describe, expect, it } from "vitest";
import type { SourceInfo, SourceKind } from "./ipc/types";
import {
  DOUBLE_RECORDING_WARNING,
  defaultSelection,
  doubleRecording,
  groupSources,
  kindLabel,
} from "./picker";

function src(id: string, kind: SourceKind, label = id, extra: Partial<SourceInfo> = {}) {
  return { id, kind, label, active: true, ...extra } satisfies SourceInfo;
}

const mic = src("mic:default", "microphone", "Microphone");
const discord = src("app:1", "application", "Discord");
const whole = src("app:100", "browser", "Chrome (whole browser)", {
  hint: "The browser extension is not paired.",
});
const loose = src("tab:s1", "browser_tab", "Tab: Notes (docs.example.org)");
const g1a = src("tab:s2", "browser_tab", "Tab: Sync (meet.example.org)", { group: "Group 1" });
const g1b = src("tab:s3", "browser_tab", "Tab: Board (board.example.org)", { group: "Group 1" });
const g2 = src("tab:s4", "browser_tab", "Tab: Plan (plan.example.org)", { group: "Group 2" });

describe("groupSources", () => {
  it("orders the sections Microphone, Applications, Browser and leaves empty ones out", () => {
    const sections = groupSources([whole, discord, mic]);
    expect(sections.map((s) => s.title)).toEqual(["Microphone", "Applications", "Browser"]);
    expect(groupSources([discord]).map((s) => s.id)).toEqual(["applications"]);
    expect(groupSources([])).toEqual([]);
  });

  it("lists tabs by tab group with the whole browser last", () => {
    const browser = groupSources([whole, loose, g2, g1a, mic, g1b]).find(
      (s) => s.id === "browser",
    )!;
    const flat = browser.rows.map((r) => (r.type === "heading" ? `# ${r.text}` : r.source.id));
    expect(flat).toEqual([
      "# Group 1",
      "tab:s2",
      "tab:s3",
      "# Group 2",
      "tab:s4",
      "# Other tabs",
      "tab:s1",
      "app:100",
    ]);
  });

  it("shows loose tabs without a heading when there is no tab group", () => {
    const browser = groupSources([loose, whole]).find((s) => s.id === "browser")!;
    expect(browser.rows.map((r) => r.type)).toEqual(["source", "source"]);
  });

  it("keeps the whole browser alone when the extension offers no tab", () => {
    const browser = groupSources([mic, whole]).find((s) => s.id === "browser")!;
    expect(browser.rows).toEqual([{ type: "source", source: whole }]);
  });
});

describe("defaultSelection", () => {
  it("ticks the microphone and every captured tab, not the whole browser", () => {
    expect([...defaultSelection([discord, mic, loose, whole, g1a])].sort()).toEqual([
      "mic:default",
      "tab:s1",
      "tab:s2",
    ]);
  });

  it("falls back to the first source without a microphone and to nothing without sources", () => {
    expect([...defaultSelection([discord, whole])]).toEqual(["app:1"]);
    expect(defaultSelection([]).size).toBe(0);
  });
});

describe("doubleRecording", () => {
  const all = [mic, discord, whole, loose, g1a];

  it("warns when a tab and the whole browser are both chosen", () => {
    expect(doubleRecording(all, ["mic:default", "tab:s1", "app:100"])).toBe(
      DOUBLE_RECORDING_WARNING,
    );
  });

  it("is quiet for a tab alone, the whole browser alone, or other sources", () => {
    expect(doubleRecording(all, ["tab:s1", "tab:s2"])).toBeNull();
    expect(doubleRecording(all, ["app:100", "app:1"])).toBeNull();
    expect(doubleRecording(all, [])).toBeNull();
  });

  it("ignores ids that are not in the list", () => {
    expect(doubleRecording(all, ["tab:gone", "app:100"])).toBeNull();
  });
});

describe("kindLabel", () => {
  it("names each kind", () => {
    expect(kindLabel(mic)).toBe("mic");
    expect(kindLabel(loose)).toBe("tab");
    expect(kindLabel(whole)).toBe("all Chrome");
    expect(kindLabel(discord)).toBe("playing");
    expect(kindLabel({ ...discord, active: false })).toBe("idle");
  });
});
