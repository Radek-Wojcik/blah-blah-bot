import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import type { QueueState } from "../ipc/types";
import { Main } from "./Main";
import { ProcessPrompt } from "./ProcessPrompt";
import { formatEta, QueueView } from "./Queue";
import { RecordView } from "./RecordView";
import { Widget } from "./Widget";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

async function flush(ticks = 5) {
  for (let i = 0; i < ticks; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function setInputValue(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.restoreAllMocks();
});

describe("RecordView", () => {
  it("renders full RecorderPanel, hotkey tips, HelpPopover, and handles post-stop ProcessPrompt", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const processSpy = vi.spyOn(backend, "processMeeting");
    const briefingSpy = vi.spyOn(backend, "openBriefing").mockResolvedValue();

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <RecordView />
          <ProcessPrompt />
        </BackendProvider>,
      );
    });
    await flush(6);

    expect(host.querySelector(".recorder-panel-full")).not.toBeNull();
    expect(host.querySelector(".ticker")).toBeNull();
    expect(host.textContent).toContain("Ctrl+Alt+R");
    expect(host.textContent).toContain("Ctrl+Alt+S");

    // Open briefing button calls openBriefing for selected project.
    const openBriefingBtn = Array.from(host.querySelectorAll("button")).find(
      (b) => b.textContent === "Open briefing",
    ) as HTMLButtonElement;
    expect(openBriefingBtn).not.toBeUndefined();
    await act(async () => {
      openBriefingBtn.click();
    });
    expect(briefingSpy).toHaveBeenCalledWith("NOMAD");

    // Open tip HelpPopover.
    const helpBtn = host.querySelector(
      'button.help-popover-btn[aria-label="Help: Main window recorder"]',
    ) as HTMLButtonElement;
    expect(helpBtn).not.toBeNull();
    await act(async () => {
      helpBtn.click();
    });
    expect(host.querySelector(".help-popover-card")?.textContent).toContain("Main window recorder");

    // Record and stop to trigger ProcessPrompt dialog, then dismiss with Later.
    const recordBtn = host.querySelector("button.btn-record") as HTMLButtonElement;
    await act(async () => {
      recordBtn.click();
    });
    await flush();
    expect((await backend.recordingState()).status).toBe("recording");

    const stopBtn = host.querySelector("button.btn-stop") as HTMLButtonElement;
    await act(async () => {
      stopBtn.click();
    });
    await flush();

    const dialog1 = host.querySelector('[role="dialog"]') as HTMLElement;
    expect(dialog1).not.toBeNull();
    expect(dialog1.textContent).toContain("Process now or later?");
    const laterBtn = Array.from(dialog1.querySelectorAll("button")).find(
      (b) => b.textContent === "Later",
    ) as HTMLButtonElement;
    expect(laterBtn).not.toBeUndefined();
    await act(async () => {
      laterBtn.click();
    });
    await flush();
    expect(host.querySelector('[role="dialog"]')).toBeNull();
    expect(processSpy).not.toHaveBeenCalled();

    // Record and stop again, then click Process now.
    await act(async () => {
      (host.querySelector("button.btn-record") as HTMLButtonElement).click();
    });
    await flush();
    await act(async () => {
      (host.querySelector("button.btn-stop") as HTMLButtonElement).click();
    });
    await flush();

    const dialog2 = host.querySelector('[role="dialog"]') as HTMLElement;
    expect(dialog2).not.toBeNull();
    const processNowBtn = Array.from(dialog2.querySelectorAll("button")).find(
      (b) => b.textContent === "Process now",
    ) as HTMLButtonElement;
    expect(processNowBtn).not.toBeUndefined();
    await act(async () => {
      processNowBtn.click();
    });
    await flush();
    expect(processSpy).toHaveBeenCalledTimes(1);
    expect(host.querySelector('[role="dialog"]')).toBeNull();
  });

  it("opens ProcessPrompt modal with backend selection when stopping in Main Record view", async () => {
    const backend = createMockBackend({ clockRate: 0 });

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <Main />
        </BackendProvider>,
      );
    });
    await flush(6);

    const recordNav = Array.from(host.querySelectorAll("nav button")).find(
      (b) => b.textContent === "Record",
    ) as HTMLButtonElement;
    await act(async () => {
      recordNav.click();
    });
    await flush(6);

    await act(async () => {
      (host.querySelector("button.btn-record") as HTMLButtonElement).click();
    });
    await flush();
    await act(async () => {
      (host.querySelector("button.btn-stop") as HTMLButtonElement).click();
    });
    await flush();

    expect(host.querySelector(".recorder-post-stop")).toBeNull();
    const dialogs = host.querySelectorAll('[role="dialog"]');
    expect(dialogs).toHaveLength(1);
    expect(dialogs[0].textContent).toContain("Process now or later?");
    expect(dialogs[0].textContent).toContain("Local model");
  });
});

describe("Widget", () => {
  it("renders compact WaveformEq driven by live levels and persists source selection", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.saveLastSources(["mic:default", "app_name:discord"]);
    const saveSpy = vi.spyOn(backend, "saveLastSources");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <Widget />
        </BackendProvider>,
      );
    });
    await flush(6);

    // No fake .ticker and no static CSS .eq-live class.
    expect(host.querySelector(".ticker")).toBeNull();
    expect(host.querySelector(".eq-live")).toBeNull();
    expect(host.querySelector(".recorder-panel-compact")).not.toBeNull();

    const eq = host.querySelector(".waveform-eq") as HTMLElement;
    expect(eq).not.toBeNull();
    expect(host.querySelectorAll(".waveform-eq-bar")).toHaveLength(12);
    expect(eq.dataset.active).toBe("false");

    // Restored selection matches mic + Discord.
    const checkboxes = Array.from(
      host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'),
    );
    expect(checkboxes.filter((c) => c.checked)).toHaveLength(2);

    // Toggling Teams saves updated stable source keys.
    const teamsBox = checkboxes.find((c) => !c.checked)!;
    await act(async () => {
      teamsBox.click();
    });
    await flush();
    expect(saveSpy).toHaveBeenCalled();
    expect((await backend.getSettings()).lastSourceKeys).toContain("app_name:microsoft teams");

    // Start recording and confirm WaveformEq transitions to active with non-zero levels.
    await act(async () => {
      (host.querySelector("button.btn-record") as HTMLButtonElement).click();
    });
    await flush(6);

    const liveEq = host.querySelector(".waveform-eq") as HTMLElement;
    expect(liveEq.dataset.active).toBe("true");
    expect(liveEq.classList.contains("waveform-eq-live")).toBe(true);
    expect(Number(liveEq.dataset.peak)).toBeGreaterThan(0);
    const firstBar = liveEq.querySelector(".waveform-eq-bar") as HTMLElement;
    expect(firstBar.style.transform).toMatch(/^scaleY\(/);
    expect(firstBar.style.height).toBe("");

    await act(async () => {
      (host.querySelector("button.btn-stop") as HTMLButtonElement).click();
    });
    await flush();

    // Widget opens ProcessPrompt modal on stop without an inline post-stop banner.
    expect(host.querySelector(".recorder-post-stop")).toBeNull();
    const dialog = host.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain("Process now or later?");
  });

  it("ticks captured browser_tab sources by default in mock backend", async () => {
    const backend = createMockBackend({ clockRate: 0, browserTabs: "ok" });
    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <Widget />
        </BackendProvider>,
      );
    });
    await flush(6);

    const checkboxes = Array.from(
      host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'),
    );
    // mic:default + 3 captured browser tabs should be ticked by default.
    expect(checkboxes.filter((c) => c.checked)).toHaveLength(4);
  });
});

describe("QueueView", () => {
  it("formats ETA values cleanly", () => {
    expect(formatEta(null)).toBeNull();
    expect(formatEta(undefined)).toBeNull();
    expect(formatEta(-5)).toBeNull();
    expect(formatEta(0)).toBe("< 1s left");
    expect(formatEta(45)).toBe("~45s left");
    expect(formatEta(260)).toBe("~4m 20s left");
    expect(formatEta(120)).toBe("~2m left");
    expect(formatEta(3900)).toBe("~1h 5m left");
  });

  it("renders ETA, percentage, Force process, Schedule controls, and temporary GPU overrides", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const baseQueue = await backend.queueState();
    const customQueue: QueueState = {
      gpuOverride: null,
      jobs: baseQueue.jobs.map((j) => {
        if (j.meetingId === "m4") return { ...j, progress: 0.42, etaSeconds: 260 };
        return j;
      }),
    };
    vi.spyOn(backend, "queueState").mockResolvedValue(customQueue);
    const setOverrideSpy = vi.spyOn(backend, "setQueueGpuOverride");
    const forceSpy = vi.spyOn(backend, "forceProcess");
    const scheduleSpy = vi.spyOn(backend, "scheduleJob");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <QueueView />
        </BackendProvider>,
      );
    });
    await flush(6);

    expect(host.querySelector(".queue-layout-grid")).not.toBeNull();

    // Verify granular progress % and ETA display on running job.
    const runningCard = host.querySelector('article[aria-label="Thruster review"]') as HTMLElement;
    expect(runningCard.textContent).toContain("42%");
    expect(runningCard.textContent).toContain("~4m 20s left");

    // Verify GPU overrides side panel note and controls.
    const sidePanel = host.querySelector(
      'aside[aria-label="Queue GPU overrides (temporary)"]',
    ) as HTMLElement;
    expect(sidePanel).not.toBeNull();
    expect(sidePanel.textContent).toContain(
      "Applies only to jobs currently in the queue. Automatically reverts to global settings once the queue finishes.",
    );

    const maxUtilLabel = Array.from(sidePanel.querySelectorAll("label")).find((l) =>
      l.textContent?.includes("Max GPU utilization"),
    )!;
    const maxUtilInput = (
      maxUtilLabel.htmlFor
        ? sidePanel.querySelector(`#${CSS.escape(maxUtilLabel.htmlFor)}`)
        : maxUtilLabel.querySelector("input")
    ) as HTMLInputElement;
    const idleLabel = Array.from(sidePanel.querySelectorAll("label")).find((l) =>
      l.textContent?.includes("Wait for idle"),
    )!;
    const idleInput = (
      idleLabel.htmlFor
        ? sidePanel.querySelector(`#${CSS.escape(idleLabel.htmlFor)}`)
        : idleLabel.querySelector("input")
    ) as HTMLInputElement;

    expect(maxUtilInput.min).toBe("1");
    expect(maxUtilInput.max).toBe("100");
    await setInputValue(maxUtilInput, "0");
    const overrideForm = sidePanel.querySelector("form") as HTMLFormElement;
    await act(async () => {
      overrideForm.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    await flush();
    expect(sidePanel.textContent).toContain("Max GPU utilization must be between 1% and 100%.");
    expect(setOverrideSpy).not.toHaveBeenCalled();

    await setInputValue(maxUtilInput, "85");
    await setInputValue(idleInput, "10");

    await act(async () => {
      overrideForm.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    await flush();

    expect(setOverrideSpy).toHaveBeenCalledWith({
      gpuMaxUtilPercent: 85,
      gpuIdleSeconds: 10,
    });

    const resetBtn = Array.from(sidePanel.querySelectorAll("button")).find(
      (b) => b.textContent === "Reset",
    ) as HTMLButtonElement;
    await act(async () => {
      resetBtn.click();
    });
    await flush();
    expect(setOverrideSpy).toHaveBeenLastCalledWith(null);

    // Verify schedule picker + wake lock note + scheduleJob call on waiting job.
    const waitingCard = host.querySelector('article[aria-label="Budget check-in"]') as HTMLElement;
    expect(waitingCard.textContent).toContain("PC stays awake (monitors may turn off)");

    const scheduleInput = waitingCard.querySelector(
      'input[type="datetime-local"]',
    ) as HTMLInputElement;
    const targetLocal = "2026-12-01T22:30";
    const expectedMs = new Date(targetLocal).getTime();
    await setInputValue(scheduleInput, targetLocal);

    const scheduleBtn = Array.from(waitingCard.querySelectorAll("button")).find(
      (b) => b.textContent === "Schedule",
    ) as HTMLButtonElement;
    await act(async () => {
      scheduleBtn.click();
    });
    await flush();

    expect(scheduleSpy).toHaveBeenCalledWith("m3", expectedMs);

    // Clear schedule should now be visible and call scheduleJob("m3", null).
    const clearScheduleBtn = Array.from(waitingCard.querySelectorAll("button")).find(
      (b) => b.textContent === "Clear schedule",
    ) as HTMLButtonElement;
    expect(clearScheduleBtn).not.toBeUndefined();
    await act(async () => {
      clearScheduleBtn.click();
    });
    await flush();

    expect(scheduleSpy).toHaveBeenLastCalledWith("m3", null);

    // Verify Force process button + HelpPopover.
    const forceHelpBtn = waitingCard.querySelector(
      'button.help-popover-btn[aria-label="Help: Force process"]',
    ) as HTMLButtonElement;
    expect(forceHelpBtn).not.toBeNull();
    await act(async () => {
      forceHelpBtn.click();
    });
    expect(waitingCard.querySelector(".help-popover-card")?.textContent).toContain(
      "GPU utilization limit",
    );

    const forceBtn = Array.from(waitingCard.querySelectorAll("button")).find(
      (b) => b.textContent === "Force process",
    ) as HTMLButtonElement;
    await act(async () => {
      forceBtn.click();
    });
    await flush();

    expect(forceSpy).toHaveBeenCalledWith("m3");
  });
});
