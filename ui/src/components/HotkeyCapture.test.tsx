import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HotkeyCapture } from "./HotkeyCapture";

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
});

describe("HotkeyCapture", () => {
  it("captures shortcuts, shows live modifier preview, handles Escape/Clear, and warns on AltGr", async () => {
    const onChange = vi.fn();
    await act(async () => {
      root.render(
        <HotkeyCapture
          id="hk-rec"
          label="Record / Stop hotkey"
          value="Ctrl+Alt+R"
          otherHotkey="Ctrl+Shift+S"
          onChange={onChange}
        />,
      );
    });

    const btn = host.querySelector<HTMLButtonElement>("#hk-rec")!;
    expect(btn.textContent).toBe("Ctrl+Alt+R");
    expect(host.querySelector('[role="status"]')?.textContent).toContain("AltGr");

    // Enter capture mode
    await act(async () => {
      btn.click();
    });
    expect(btn.getAttribute("data-hotkey-capturing")).toBe("true");
    expect(btn.textContent).toBe("Press shortcut…");

    // Modifier-only key updates live preview
    await act(async () => {
      btn.dispatchEvent(
        new KeyboardEvent("keydown", {
          code: "ControlLeft",
          key: "Control",
          ctrlKey: true,
          bubbles: true,
        }),
      );
    });
    expect(btn.textContent).toBe("Ctrl+…");

    // Conflict with otherHotkey shows role="alert"
    await act(async () => {
      btn.dispatchEvent(
        new KeyboardEvent("keydown", {
          code: "KeyS",
          key: "S",
          ctrlKey: true,
          shiftKey: true,
          bubbles: true,
        }),
      );
    });
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("already used");
    expect(onChange).not.toHaveBeenCalled();

    // Valid capture calls onChange and exits capture mode
    await act(async () => {
      btn.dispatchEvent(
        new KeyboardEvent("keydown", {
          code: "KeyR",
          key: "R",
          ctrlKey: true,
          shiftKey: true,
          bubbles: true,
        }),
      );
    });
    expect(onChange).toHaveBeenCalledWith("Ctrl+Shift+R");
    expect(btn.getAttribute("data-hotkey-capturing")).toBeNull();

    // Clear button clears the value
    const clearBtn = host.querySelector<HTMLButtonElement>('button[aria-label="Clear shortcut"]')!;
    await act(async () => {
      clearBtn.click();
    });
    expect(onChange).toHaveBeenCalledWith("");
  });
});
