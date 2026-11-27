import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import { DraftEditor } from "./DraftEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(async () => {
  if (root) {
    await act(async () => {
      root?.unmount();
    });
    root = null;
  }
  container?.remove();
  container = null;
  vi.restoreAllMocks();
});

async function flush() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe("DraftEditor report toolbar", () => {
  it("places all report controls in one toolbar container and status below it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    const backend = createMockBackend({ clockRate: 0 });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <BackendProvider value={backend}>
          <DraftEditor
            meetingId="m5"
            onOpenTranscript={() => undefined}
            onOpenQueue={() => undefined}
          />
        </BackendProvider>,
      );
    });
    await flush();

    const reportTabBtn = [...container.querySelectorAll("button")].find(
      (b) => b.textContent?.trim() === "Rendered report",
    );
    expect(reportTabBtn).toBeDefined();

    await act(async () => {
      reportTabBtn?.click();
    });
    await flush();

    const toolbar = container.querySelector(".report-tools");
    expect(toolbar).not.toBeNull();
    expect(toolbar?.getAttribute("role")).toBe("toolbar");

    const leftGroup = toolbar?.querySelector('[role="group"][aria-label="Report view"]');
    const rightGroup = toolbar?.querySelector(".report-tools-right");
    expect(leftGroup).not.toBeNull();
    expect(rightGroup).not.toBeNull();

    const leftButtons = [...(leftGroup?.querySelectorAll("button") ?? [])].map((b) =>
      b.textContent?.trim(),
    );
    expect(leftButtons).toEqual(["Text", "Preview"]);

    const formatSelect = rightGroup?.querySelector('select[aria-label="Report format"]');
    const modeChip = rightGroup?.querySelector(".chip");
    const rightButtons = [...(rightGroup?.querySelectorAll("button") ?? [])].map((b) =>
      b.textContent?.trim(),
    );
    expect(formatSelect).not.toBeNull();
    expect(modeChip?.textContent?.trim()).toBe("Auto");
    expect(rightButtons).toEqual(["Copy", "Download note (.md)"]);

    const copyBtn = [...(rightGroup?.querySelectorAll("button") ?? [])].find(
      (b) => b.textContent?.trim() === "Copy",
    );
    await act(async () => {
      copyBtn?.click();
    });
    await flush();

    expect(writeText).toHaveBeenCalledTimes(1);
    expect(toolbar?.querySelector('[role="status"]')).toBeNull();

    const statusBar = container.querySelector(".report-status");
    expect(statusBar).not.toBeNull();
    expect(statusBar?.querySelector('[role="status"]')?.textContent?.trim()).toBe("Copied");
  });
});
