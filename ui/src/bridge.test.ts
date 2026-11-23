import { describe, expect, it } from "vitest";
import { bridgeSummary, clientDetails, groupSummary, pairingTimeLeft } from "./bridge";
import type { BridgeStatus } from "./ipc/types";

const base: BridgeStatus = {
  enabled: true,
  listening: true,
  port: 17555,
  error: null,
  pairing: null,
  clients: [],
  streams: [],
  groups: [],
};

describe("bridge helpers", () => {
  it("summarizes a tab group", () => {
    const g = {
      groupId: 7,
      tabs: [
        { tabId: 1, status: "captured" },
        { tabId: 2, status: "waiting" },
        { tabId: 3, status: "failed" },
      ],
    };
    expect(groupSummary(g, 0)).toBe("Group 1: 1/3 tabs captured, 1 failed");
  });

  it("counts down a pairing code and stops at expiry", () => {
    expect(pairingTimeLeft(300_000, 0)).toBe("5:00");
    expect(pairingTimeLeft(300_000, 299_500)).toBe("0:01");
    expect(pairingTimeLeft(300_000, 300_000)).toBeNull();
  });

  it("summarizes the server state", () => {
    expect(bridgeSummary(base)).toBe("Listening on 127.0.0.1:17555.");
    expect(
      bridgeSummary({
        ...base,
        streams: [
          { streamId: "a", title: "", host: "", track: "tab", waiting: false },
          { streamId: "b", title: "", host: "", track: "tab-2", waiting: true },
        ],
      }),
    ).toBe("Listening on 127.0.0.1:17555. Recording 2 tabs.");
    expect(bridgeSummary({ ...base, enabled: false, listening: false })).toContain("Off");
    expect(bridgeSummary({ ...base, listening: false, port: null, error: "No free port" })).toBe(
      "No free port",
    );
  });

  it("describes a paired extension", () => {
    const line = clientDetails({
      id: "c-1",
      name: "Chrome 136",
      extVersion: "0.6.0",
      pairedUnixMs: Date.UTC(2026, 10, 10, 9, 30),
      connected: false,
    });
    expect(line).toBe("extension 0.6.0 · paired 2026-11-10 09:30 UTC · not connected");
  });
});
