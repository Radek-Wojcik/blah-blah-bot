import { describe, expect, it } from "vitest";
import { isMeetCallUrl, nextMeetState } from "../src/meet.js";

describe("isMeetCallUrl", () => {
  it.each([
    "https://meet.google.com/abc-defg-hij",
    "https://meet.google.com/abc-defg-hij?authuser=1",
    "https://meet.google.com/abc-defg-hij/",
    "https://meet.google.com/abc-defg-hij#x",
  ])("accepts %s", (url) => {
    expect(isMeetCallUrl(url)).toBe(true);
  });

  it.each([
    "https://meet.google.com/",
    "https://meet.google.com/landing",
    "https://meet.google.com/new",
    "https://meet.google.com/abc-defg-hi",
    "https://meet.google.com/ABC-DEFG-HIJ",
    "https://meet.google.com/abc-defg-hij/extra",
    "http://meet.google.com/abc-defg-hij",
    "https://evil.example/meet.google.com/abc-defg-hij",
    "https://meet.google.com.evil.example/abc-defg-hij",
    "https://notmeet.google.com/abc-defg-hij",
    "not a url",
    "",
    undefined,
    null,
  ])("refuses %s", (url) => {
    expect(isMeetCallUrl(url)).toBe(false);
  });
});

describe("nextMeetState", () => {
  const call = "https://meet.google.com/abc-defg-hij";

  it("joins when the call tab becomes audible", () => {
    expect(nextMeetState(false, { url: call, audible: true })).toBe("joined");
  });

  it("does not join a silent lobby", () => {
    expect(nextMeetState(false, { url: call, audible: false })).toBeNull();
    expect(nextMeetState(false, { url: call })).toBeNull();
  });

  it("stays joined when the sound stops", () => {
    expect(nextMeetState(true, { url: call, audible: false })).toBeNull();
  });

  it("leaves when the URL is no longer a call", () => {
    expect(nextMeetState(true, { url: "https://meet.google.com/landing", audible: true })).toBe(
      "left",
    );
    expect(nextMeetState(true, { url: "https://example.org/", audible: true })).toBe("left");
  });

  it("leaves when the tab closes", () => {
    expect(nextMeetState(true, { closed: true })).toBe("left");
  });

  it("ignores other tabs", () => {
    expect(nextMeetState(false, { url: "https://example.org/", audible: true })).toBeNull();
    expect(nextMeetState(false, { closed: true })).toBeNull();
  });
});
