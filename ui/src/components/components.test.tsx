import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import { HelpPopover } from "./HelpPopover";
import { RecorderPanel } from "./RecorderPanel";
import { UnsavedDialog } from "./UnsavedDialog";
import { WaveformEq } from "./WaveformEq";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

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

describe("HelpPopover", () => {
  it("toggles open on click and closes on Escape or outside click", async () => {
    await act(async () => {
      root.render(
        <div>
          <HelpPopover title="GPU Threshold" recommended="40% max utilization">
            <p>Waits while the GPU is busy in another application.</p>
          </HelpPopover>
          <button type="button" className="outside">
            Outside
          </button>
        </div>,
      );
    });

    const btn = host.querySelector("button.help-popover-btn") as HTMLButtonElement;
    expect(btn.getAttribute("aria-label")).toBe("Help: GPU Threshold");
    expect(btn.getAttribute("aria-expanded")).toBe("false");
    expect(host.querySelector(".help-popover-card")).toBeNull();

    await act(async () => {
      btn.click();
    });
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    const card = host.querySelector(".help-popover-card") as HTMLElement;
    expect(card).not.toBeNull();
    expect(card.textContent).toContain("GPU Threshold");
    expect(card.textContent).toContain("40% max utilization");

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(host.querySelector(".help-popover-card")).toBeNull();

    await act(async () => {
      btn.click();
    });
    expect(host.querySelector(".help-popover-card")).not.toBeNull();

    const outside = host.querySelector("button.outside") as HTMLButtonElement;
    await act(async () => {
      outside.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(host.querySelector(".help-popover-card")).toBeNull();
  });
});

describe("WaveformEq", () => {
  it("renders 12 bars scaled dynamically by real RecordingLevels", async () => {
    const bars = [0.1, 0.2, 0.35, 0.5, 0.75, 0.9, 0.65, 0.4, 0.3, 0.25, 0.15, 0.8];
    await act(async () => {
      root.render(<WaveformEq levels={{ peak: 0.9, rms: 0.48, bars }} active={true} />);
    });

    const container = host.querySelector(".waveform-eq") as HTMLElement;
    expect(container.dataset.active).toBe("true");
    expect(container.dataset.peak).toBe("0.90");
    const barNodes = host.querySelectorAll(".waveform-eq-bar");
    expect(barNodes.length).toBe(12);
    expect((barNodes[5] as HTMLElement).dataset.level).toBe("0.90");
    expect((barNodes[5] as HTMLElement).style.transform).toMatch(/scaleY\(0\.90*\)/);
  });
});

describe("UnsavedDialog", () => {
  it("renders banner and modal confirmation and triggers save, discard, and cancel", async () => {
    const onSave = vi.fn();
    const onDiscard = vi.fn();
    const onCancel = vi.fn();

    await act(async () => {
      root.render(
        <UnsavedDialog
          open={true}
          dirty={true}
          onSave={onSave}
          onDiscard={onDiscard}
          onCancel={onCancel}
        />,
      );
    });

    expect(host.querySelector(".unsaved-bar")).not.toBeNull();
    const dialog = host.querySelector("[role=dialog]") as HTMLElement;
    expect(dialog).not.toBeNull();

    const buttons = Array.from(dialog.querySelectorAll("button"));
    const cancelBtn = buttons.find((b) => b.textContent === "Cancel")!;
    const discardBtn = buttons.find((b) => b.textContent === "Discard")!;
    const saveBtn = buttons.find((b) => b.textContent === "Save")!;

    await act(async () => {
      cancelBtn.click();
      discardBtn.click();
      saveBtn.click();
    });

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onDiscard).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledTimes(1);
  });
});

describe("RecorderPanel", () => {
  it("restores persisted sources, auto-saves toggled sources, records and returns to standby on stop", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.saveLastSources(["mic:default", "app_name:discord"]);

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <RecorderPanel />
        </BackendProvider>,
      );
    });

    // Wait for initial promises (listCycles, getSettings, listSources, reportDefaults).
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });

    const checkboxes = Array.from(
      host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'),
    );
    expect(checkboxes.length).toBeGreaterThanOrEqual(3);
    const checkedCount = checkboxes.filter((c) => c.checked).length;
    expect(checkedCount).toBe(2);

    // Toggle Teams checkbox and verify saveLastSources was updated.
    const teamsCheckbox = checkboxes.find((c) => !c.checked)!;
    await act(async () => {
      teamsCheckbox.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect((await backend.getSettings()).lastSourceKeys).toContain("app_name:microsoft teams");

    // Start recording.
    const recordBtn = host.querySelector("button.btn-record") as HTMLButtonElement;
    expect(recordBtn.disabled).toBe(false);
    await act(async () => {
      recordBtn.click();
      await new Promise((r) => setTimeout(r, 20));
    });
    expect((await backend.recordingState()).status).toBe("recording");

    // Stop recording and verify panel returns cleanly to standby without an inline banner.
    const stopBtn = host.querySelector("button.btn-stop") as HTMLButtonElement;
    expect(stopBtn).not.toBeNull();
    await act(async () => {
      stopBtn.click();
      await new Promise((r) => setTimeout(r, 20));
    });
    expect((await backend.recordingState()).status).toBe("idle");
    expect(host.querySelector(".recorder-post-stop")).toBeNull();
    expect(host.textContent).toContain("Standby");
  });
});
