import { describe, expect, it } from "vitest";
import {
  SENSITIVITIES,
  appLabel,
  appProblem,
  detectionSummary,
  normalizeApp,
  withApp,
} from "./detection";
import type { DetectionSettings } from "./ipc/types";

const base: DetectionSettings = {
  enabled: true,
  sensitivity: "balanced",
  knownApps: ["discord", "zoom"],
  optedOut: [],
  meet: true,
};

describe("detection helpers", () => {
  it("normalizes program names", () => {
    expect(normalizeApp(" Zoom.EXE ")).toBe("zoom");
    expect(normalizeApp("ms-teams")).toBe("ms-teams");
  });

  it("refuses empty, duplicate and path-like names", () => {
    expect(appProblem("", [])).not.toBeNull();
    expect(appProblem("Zoom.exe", ["zoom"])).toContain("already");
    expect(appProblem("C:\\Apps\\x.exe", [])).toContain("without a folder");
    expect(appProblem("webex", ["zoom"])).toBeNull();
  });

  it("adds a valid name and leaves the list alone otherwise", () => {
    expect(withApp(["zoom"], " Webex.exe")).toEqual(["zoom", "webex"]);
    expect(withApp(["zoom"], "zoom")).toEqual(["zoom"]);
  });

  it("names the known applications", () => {
    expect(appLabel("ms-teams")).toBe("Microsoft Teams");
    expect(appLabel("discordptb")).toBe("Discord PTB");
    expect(appLabel("webex")).toBe("webex");
  });

  it("offers the three sensitivities with balanced described as the default", () => {
    expect(SENSITIVITIES.map((s) => s.value)).toEqual(["low", "balanced", "high"]);
    expect(SENSITIVITIES[1].text).toContain("default");
  });

  it("summarizes the state", () => {
    expect(detectionSummary({ ...base, enabled: false })).toMatch(/^Off/);
    expect(detectionSummary(base)).toBe("On, watching 3 applications.");
    expect(detectionSummary({ ...base, optedOut: ["zoom"], meet: false })).toBe(
      "On, watching 1 application.",
    );
    expect(detectionSummary({ ...base, knownApps: [], meet: false })).toContain("no application");
  });
});
