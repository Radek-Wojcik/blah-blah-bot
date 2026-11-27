import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import { Widget } from "./Widget";

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
});

async function flush() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe("Widget", () => {
  it("renders the device list inside a scrollable flex container", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <BackendProvider value={backend}>
          <Widget />
        </BackendProvider>,
      );
    });
    await flush();

    const sourcesPanel = container.querySelector(".recorder-sources");
    expect(sourcesPanel).not.toBeNull();
    expect(sourcesPanel?.classList.contains("grow")).toBe(true);

    const scrollBody = sourcesPanel?.querySelector(".recorder-sources-scroll");
    expect(scrollBody).not.toBeNull();
    expect(scrollBody?.querySelector("fieldset.picker")).not.toBeNull();

    const labels = [...(scrollBody?.querySelectorAll(".pick-label") ?? [])].map(
      (el) => el.textContent,
    );
    expect(labels).toContain("Microphone (USB headset)");
    expect(labels).toContain("Discord");
  });
});
