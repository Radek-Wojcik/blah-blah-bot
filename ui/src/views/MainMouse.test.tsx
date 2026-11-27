import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import type { MouseButtonsMode } from "../ipc/types";
import { Main } from "./Main";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

async function flush(ticks = 8) {
  for (let i = 0; i < ticks; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

async function mount(mode: MouseButtonsMode) {
  const backend = createMockBackend({ clockRate: 0 });
  const s = await backend.getSettings();
  await backend.saveSettings({ ...s, mouseButtons: mode });
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <Main />
      </BackendProvider>,
    );
  });
  await flush();
}

async function nav(text: string) {
  const sidebar = host.querySelector('nav[aria-label="Main navigation"]')!;
  const btn = Array.from(sidebar.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === text,
  )!;
  await act(async () => btn.click());
  await flush();
}

/** Dispatches a side-button press and reports whether the default action was prevented. */
async function side(button: number): Promise<{ down: boolean; up: boolean }> {
  let down = false;
  let up = false;
  await act(async () => {
    const d = new MouseEvent("mousedown", { button, bubbles: true, cancelable: true });
    window.dispatchEvent(d);
    down = d.defaultPrevented;
    const u = new MouseEvent("mouseup", { button, bubbles: true, cancelable: true });
    window.dispatchEvent(u);
    up = u.defaultPrevented;
  });
  await flush();
  return { down, up };
}

const heading = () => host.querySelector("h2")?.textContent?.trim();

describe("Main mouse side buttons", () => {
  it("normal: button 3 goes back and button 4 goes forward", async () => {
    await mount("normal");
    await nav("Record");
    expect(heading()).toBe("Record");
    await side(3);
    expect(heading()).toBe("Projects");
    await new Promise((r) => setTimeout(r, 100));
    await side(4);
    expect(heading()).toBe("Record");
  });

  it("swapped: button 4 goes back", async () => {
    await mount("swapped");
    await nav("Record");
    await side(3);
    expect(heading()).toBe("Record");
    await new Promise((r) => setTimeout(r, 100));
    await side(4);
    expect(heading()).toBe("Projects");
  });

  it("off: does not navigate but still swallows the buttons", async () => {
    await mount("off");
    await nav("Record");
    const r = await side(3);
    expect(r).toEqual({ down: true, up: true });
    expect(heading()).toBe("Record");
  });

  it("Back from another Settings tab returns to the default tab", async () => {
    await mount("normal");
    await nav("Settings");
    const selected = () =>
      host.querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim();
    const first = selected();
    const other = Array.from(host.querySelectorAll<HTMLButtonElement>('[role="tab"]')).find(
      (t) => t.textContent?.trim() !== first,
    )!;
    await act(async () => other.click());
    await flush();
    expect(selected()).not.toBe(first);
    await side(3);
    expect(selected()).toBe(first);
  });
});
