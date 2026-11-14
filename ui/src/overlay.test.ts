import { describe, expect, it } from "vitest";
import { toRegion } from "./views/Overlay";

describe("toRegion", () => {
  it("normalizes a drag in any direction", () => {
    const r = toRegion({ x: 300, y: 200 }, { x: 100, y: 50 }, 1000, 500);
    expect(r).toEqual({ x: 0.1, y: 0.1, w: 0.2, h: 0.3 });
  });
  it("clamps to the stage", () => {
    const r = toRegion({ x: -50, y: 0 }, { x: 2000, y: 600 }, 1000, 500);
    expect(r.x).toBe(0);
    expect(r.w).toBe(1);
    expect(r.h).toBe(1);
  });
});
