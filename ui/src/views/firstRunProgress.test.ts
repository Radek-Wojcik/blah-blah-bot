import { describe, expect, it } from "vitest";
import type { SetupProgress } from "../ipc/types";
import {
  OFFLINE_MESSAGE,
  SETUP_TIME_NOTE,
  formatCancelledMessage,
  formatElapsed,
  formatFailureMessage,
  formatMb,
  formatSetupProgress,
  isBusyError,
  isNetworkFailure,
  isTaskActive,
  progressPercent,
  taskActionForPhase,
} from "./firstRunProgress";

const MB = 1024 * 1024;

function progress(overrides: Partial<SetupProgress> = {}): SetupProgress {
  return {
    task: "uv-sync",
    phase: "progress",
    downloadedBytes: null,
    totalBytes: null,
    elapsedSecs: 0,
    message: "",
    ...overrides,
  };
}

describe("progressPercent", () => {
  it("computes clamped integer percentages when totalBytes is positive", () => {
    expect(progressPercent(0, 200 * MB)).toBe(0);
    expect(progressPercent(80 * MB, 200 * MB)).toBe(40);
    expect(progressPercent(200 * MB, 200 * MB)).toBe(100);
    expect(progressPercent(250 * MB, 200 * MB)).toBe(100);
    expect(progressPercent(-10, 200 * MB)).toBe(0);
  });

  it("returns null when downloadedBytes or totalBytes is unknown or non-positive", () => {
    expect(progressPercent(null, 200 * MB)).toBeNull();
    expect(progressPercent(80 * MB, null)).toBeNull();
    expect(progressPercent(80 * MB, 0)).toBeNull();
    expect(progressPercent(80 * MB, -5)).toBeNull();
  });
});

describe("formatMb and formatElapsed", () => {
  it("formats byte counts in MB", () => {
    expect(formatMb(0)).toBe("0 MB");
    expect(formatMb(512 * 1024)).toBe("0.5 MB");
    expect(formatMb(120 * MB)).toBe("120 MB");
    expect(formatMb(2048 * MB)).toBe("2048 MB");
  });

  it("formats elapsed seconds as seconds or minutes and seconds", () => {
    expect(formatElapsed(0)).toBe("0s");
    expect(formatElapsed(12)).toBe("12s");
    expect(formatElapsed(65)).toBe("1m 05s");
    expect(formatElapsed(600)).toBe("10m 00s");
  });
});

describe("formatSetupProgress", () => {
  it("shows percent and MB when totalBytes is known", () => {
    expect(
      formatSetupProgress(
        progress({
          task: "llama-server",
          downloadedBytes: 80 * MB,
          totalBytes: 200 * MB,
          elapsedSecs: 5,
          message: "Downloading llama-server",
        }),
      ),
    ).toBe("40% (80 MB / 200 MB)");
  });

  it("shows MB downloaded and elapsed time when totalBytes is unknown", () => {
    expect(
      formatSetupProgress(
        progress({
          task: "uv-sync",
          downloadedBytes: 120 * MB,
          totalBytes: null,
          elapsedSecs: 74,
          message: "Downloading packages",
        }),
      ),
    ).toBe("120 MB downloaded · 1m 14s");
  });

  it("formats verifying, done, cancelled and failed phases", () => {
    expect(formatSetupProgress(progress({ phase: "verifying", message: "Checking SHA256" }))).toBe(
      "Checking SHA256",
    );
    expect(formatSetupProgress(progress({ phase: "done", message: "Ready" }))).toBe("Ready");
    expect(formatSetupProgress(progress({ phase: "cancelled", message: "Cancelled." }))).toBe(
      "Cancelled.",
    );
    expect(
      formatSetupProgress(
        progress({ phase: "failed", message: "Network error: no internet connection." }),
      ),
    ).toBe(OFFLINE_MESSAGE);
    expect(formatSetupProgress(progress({ phase: "failed", message: "Disk full" }))).toBe(
      "Disk full",
    );
  });
});

describe("network failure detection and setup note", () => {
  it("maps network errors to the plain offline message and keeps other errors", () => {
    expect(isNetworkFailure("DNS lookup failed: could not resolve host")).toBe(true);
    expect(isNetworkFailure("Request timed out")).toBe(true);
    expect(isNetworkFailure("Checksum mismatch")).toBe(false);
    expect(formatFailureMessage("Offline: failed to connect")).toBe(OFFLINE_MESSAGE);
    expect(formatFailureMessage("Checksum mismatch")).toBe("Checksum mismatch");
    expect(formatFailureMessage("   ")).toBe("The setup task failed.");
    expect(SETUP_TIME_NOTE).toContain("Keep the PC on.");
  });

  it("matches exact network failure strings from Rust downloads.rs", () => {
    const messages = [
      "could not reach github.com; check your internet connection and retry",
      "connection to github.com was interrupted; you can retry to resume",
      "secure connection to huggingface.co failed; check your network or system clock",
      "download from huggingface.co was interrupted; you can retry to resume",
      "download from github.com timed out; you can retry",
    ];
    for (const msg of messages) {
      expect(isNetworkFailure(msg)).toBe(true);
      expect(formatFailureMessage(msg)).toBe(OFFLINE_MESSAGE);
    }
  });
});

describe("phase-to-button logic", () => {
  it("identifies active phases", () => {
    expect(isTaskActive("started")).toBe(true);
    expect(isTaskActive("progress")).toBe(true);
    expect(isTaskActive("verifying")).toBe(true);
    expect(isTaskActive("done")).toBe(false);
    expect(isTaskActive("failed")).toBe(false);
    expect(isTaskActive("cancelled")).toBe(false);
    expect(isTaskActive(null)).toBe(false);
  });

  it("maps every phase and readiness state to the right button action", () => {
    expect(taskActionForPhase(null, false)).toBe("start");
    expect(taskActionForPhase(null, true)).toBe("none");
    expect(taskActionForPhase("started", false)).toBe("cancel");
    expect(taskActionForPhase("progress", false)).toBe("cancel");
    expect(taskActionForPhase("verifying", false)).toBe("cancel");
    expect(taskActionForPhase("failed", false)).toBe("retry");
    expect(taskActionForPhase("failed", true)).toBe("none");
    expect(taskActionForPhase("cancelled", false)).toBe("retry");
    expect(taskActionForPhase("cancelled", true)).toBe("none");
    expect(taskActionForPhase("done", false)).toBe("retry");
    expect(taskActionForPhase("done", true)).toBe("none");
  });

  it("detects busy IPC errors and ensures cancelled text ends with a period", () => {
    expect(isBusyError({ code: "busy", message: "already running" })).toBe(true);
    expect(isBusyError({ code: "internal", message: "busy" })).toBe(false);
    expect(isBusyError(new Error("busy"))).toBe(false);
    expect(isBusyError(null)).toBe(false);
    expect(formatCancelledMessage(undefined)).toBe("Cancelled.");
    expect(formatCancelledMessage("Cancelled.")).toBe("Cancelled.");
    expect(
      formatCancelledMessage("Environment preparation was cancelled; you can resume later"),
    ).toBe("Environment preparation was cancelled; you can resume later.");
  });
});
