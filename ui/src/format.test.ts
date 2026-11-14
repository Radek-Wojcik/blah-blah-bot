import { describe, expect, it } from "vitest";
import { formatDateTimeUtc, formatDuration, screenshotName } from "./format";

describe("formatDuration", () => {
  it("formats minutes and seconds below one hour", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(61_000)).toBe("1:01");
    expect(formatDuration(59 * 60_000 + 59_999)).toBe("59:59");
  });
  it("adds hours from one hour on", () => {
    expect(formatDuration(3_600_000)).toBe("1:00:00");
    expect(formatDuration(10 * 3_600_000 + 5 * 60_000 + 7_000)).toBe("10:05:07");
  });
  it("clamps negative values", () => {
    expect(formatDuration(-5)).toBe("0:00");
  });
});

describe("formatDateTimeUtc", () => {
  it("formats in UTC with padding", () => {
    expect(formatDateTimeUtc(Date.UTC(2026, 10, 3, 4, 5))).toBe("2026-11-03 04:05");
  });
});

describe("screenshotName", () => {
  it("matches the repository naming scheme", () => {
    expect(screenshotName(1, 12 * 60_000 + 41_000)).toBe("0001-00-12-41.png");
    expect(screenshotName(12, 3_725_000)).toBe("0012-01-02-05.png");
  });
});
