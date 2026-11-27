import { describe, expect, it } from "vitest";
import {
  canGoBack,
  canGoForward,
  createHistory,
  pushRoute,
  routesEqual,
  stepBack,
  stepForward,
} from "./navigation";

describe("navigation history", () => {
  it("pushes, deduplicates, steps back/forward, and truncates forward stack on new push", () => {
    const r1 = { page: "projects" as const, meetingId: null };
    const r2 = { page: "project" as const, meetingId: null, cycleId: "NOMAD" };
    const r3 = { page: "meeting" as const, meetingId: "m5", meetingTab: "draft" as const };
    const r4 = { page: "settings" as const, meetingId: null, settingsTab: "preferences" as const };

    let h = createHistory(r1);
    expect(canGoBack(h)).toBe(false);
    expect(canGoForward(h)).toBe(false);

    // Pushing identical route is a no-op
    expect(pushRoute(h, { ...r1 })).toBe(h);
    expect(routesEqual(r1, { page: "projects", meetingId: null })).toBe(true);

    h = pushRoute(h, r2);
    h = pushRoute(h, r3);
    expect(h.index).toBe(2);
    expect(canGoBack(h)).toBe(true);
    expect(canGoForward(h)).toBe(false);

    h = stepBack(h);
    expect(h.entries[h.index]).toEqual(r2);
    expect(canGoBack(h)).toBe(true);
    expect(canGoForward(h)).toBe(true);

    h = stepForward(h);
    expect(h.entries[h.index]).toEqual(r3);

    // Step back and push r4: r3 is truncated
    h = stepBack(h);
    h = pushRoute(h, r4);
    expect(h.entries).toEqual([r1, r2, r4]);
    expect(canGoForward(h)).toBe(false);
  });
});
