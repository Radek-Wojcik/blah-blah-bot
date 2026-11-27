import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import { RecordView } from "./RecordView";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

async function flush(ticks = 6) {
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

async function mount(backend: ReturnType<typeof createMockBackend>) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <RecordView />
      </BackendProvider>,
    );
  });
  await flush();
}

async function press(btn: HTMLElement, init: KeyboardEventInit) {
  await act(async () => {
    btn.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
  });
  await flush();
}

describe("RecordView hotkeys", () => {
  it("captures, saves, persists and re-registers a new hotkey; conflicts show a message", async () => {
    const events: boolean[] = [];
    const backend = createMockBackend({
      clockRate: 0,
      onHotkeysSuspended: (v) => events.push(v),
    });
    await mount(backend);
    const rec = host.querySelector<HTMLButtonElement>("#record-view-record-hotkey")!;
    expect(rec.textContent).toBe("Ctrl+Alt+Shift+F9");

    // Conflict with the other hotkey: message, nothing saved.
    await act(async () => rec.click());
    await press(rec, { code: "F10", key: "F10", ctrlKey: true, altKey: true, shiftKey: true });
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("already used");
    expect((await backend.getSettings()).recordHotkey).toBe("Ctrl+Alt+Shift+F9");

    // A valid capture is saved and survives a reload of the view.
    await press(rec, { code: "F8", key: "F8", ctrlKey: true, altKey: true });
    expect(rec.textContent).toBe("Ctrl+Alt+F8");
    expect((await backend.getSettings()).recordHotkey).toBe("Ctrl+Alt+F8");
    expect(events.at(-1)).toBe(false);

    await act(async () => root.render(<div />));
    await mount(backend);
    expect(host.querySelector("#record-view-record-hotkey")?.textContent).toBe("Ctrl+Alt+F8");
  });

  it("shows why a configured hotkey is not active", async () => {
    const backend = createMockBackend({ clockRate: 0, takenHotkeys: ["Ctrl+Alt+Shift+F9"] });
    await mount(backend);
    const msg = host.querySelector("#record-view-record-hotkey-failure");
    expect(msg?.textContent).toContain("Ctrl+Alt+Shift+F9");
    expect(msg?.getAttribute("role")).toBe("alert");
    expect(host.querySelector("#record-view-screenshot-hotkey-failure")).toBeNull();
  });

  it("shows the error when saving a taken hotkey is refused", async () => {
    const backend = createMockBackend({ clockRate: 0, takenHotkeys: ["Ctrl+Alt+F8"] });
    await mount(backend);
    const rec = host.querySelector<HTMLButtonElement>("#record-view-record-hotkey")!;
    await act(async () => rec.click());
    await press(rec, { code: "F8", key: "F8", ctrlKey: true, altKey: true });
    expect(host.textContent).toContain("already taken");
    expect((await backend.getSettings()).recordHotkey).toBe("Ctrl+Alt+Shift+F9");
  });

  it("rejects a Shift-only hotkey in the backend like the application does", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const s = await backend.getSettings();
    await expect(backend.saveSettings({ ...s, recordHotkey: "Shift+S" })).rejects.toMatchObject({
      code: "invalid",
    });
  });
});
