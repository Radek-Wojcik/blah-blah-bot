import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
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
          otherHotkey="Ctrl+Alt+Shift+F10"
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
          code: "F10",
          key: "F10",
          ctrlKey: true,
          altKey: true,
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
    const clearBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Clear Record / Stop hotkey"]',
    )!;
    await act(async () => {
      clearBtn.click();
    });
    expect(onChange).toHaveBeenCalledWith("");
  });

  function renderWithBackend(
    onChange = vi.fn(),
    extra: { registrationError?: string; value?: string } = {},
  ) {
    const events: boolean[] = [];
    const backend = createMockBackend({
      clockRate: 0,
      onHotkeysSuspended: (v) => events.push(v),
    });
    const render = async () => {
      await act(async () => {
        root.render(
          <BackendProvider value={backend}>
            <HotkeyCapture
              id="hk-x"
              label="Record / Stop hotkey"
              value={extra.value ?? "Ctrl+Alt+Shift+F9"}
              otherHotkey="Ctrl+Alt+Shift+F10"
              registrationError={extra.registrationError}
              onChange={onChange}
            />
          </BackendProvider>,
        );
      });
    };
    return { events, render, onChange };
  }

  const key = (btn: HTMLElement, init: KeyboardEventInit) =>
    act(async () => {
      btn.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
    });

  it("suspends the global hotkeys during capture and resumes on capture, Escape and blur", async () => {
    const { events, render, onChange } = renderWithBackend();
    await render();
    const btn = host.querySelector<HTMLButtonElement>("#hk-x")!;

    await act(async () => btn.click());
    expect(events).toEqual([true]);
    await key(btn, { code: "Escape", key: "Escape" });
    expect(events).toEqual([true, false]);

    btn.focus();
    await act(async () => btn.click());
    expect(events.at(-1)).toBe(true);
    await act(async () => btn.blur());
    expect(events.at(-1)).toBe(false);
    expect(btn.getAttribute("data-hotkey-capturing")).toBeNull();

    await act(async () => btn.click());
    expect(events.at(-1)).toBe(true);
    await key(btn, { code: "KeyR", key: "r", ctrlKey: true, altKey: true });
    expect(onChange).toHaveBeenCalledWith("Ctrl+Alt+R");
    expect(events.at(-1)).toBe(false);
  });

  it("resumes the global hotkeys when unmounted mid-capture", async () => {
    const { events, render } = renderWithBackend();
    await render();
    await act(async () => host.querySelector<HTMLButtonElement>("#hk-x")!.click());
    expect(events).toEqual([true]);
    await act(async () => root.render(<div />));
    expect(events).toEqual([true, false]);
  });

  it("leaves capture on Tab and Shift+Tab without recording, and rejects Shift+letter", async () => {
    const { events, render, onChange } = renderWithBackend();
    await render();
    const btn = host.querySelector<HTMLButtonElement>("#hk-x")!;

    await act(async () => btn.click());
    await key(btn, { code: "KeyS", key: "S", shiftKey: true });
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("Ctrl, Alt or Super");
    expect(onChange).not.toHaveBeenCalled();

    let prevented = true;
    await act(async () => {
      const ev = new KeyboardEvent("keydown", {
        code: "Tab",
        key: "Tab",
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      btn.dispatchEvent(ev);
      prevented = ev.defaultPrevented;
    });
    expect(prevented).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
    expect(btn.getAttribute("data-hotkey-capturing")).toBeNull();
    expect(events.at(-1)).toBe(false);
  });

  it("exposes value, warning, errors and capture state to assistive technology", async () => {
    const { render } = renderWithBackend(vi.fn(), {
      value: "Ctrl+Alt+Shift+S",
      registrationError: "`Ctrl+Alt+Shift+S` cannot be used as a hotkey",
    });
    await render();
    const btn = host.querySelector<HTMLButtonElement>("#hk-x")!;
    expect(btn.getAttribute("aria-labelledby")).toBe("hk-x-label hk-x");
    expect(host.querySelector("#hk-x-label")?.textContent).toBe("Record / Stop hotkey");
    const described = (btn.getAttribute("aria-describedby") ?? "").split(" ");
    expect(described).toEqual(["hk-x-warning", "hk-x-failure"]);
    expect(host.querySelector("#hk-x-warning")?.textContent).toContain("AltGr");
    expect(host.querySelector("#hk-x-failure")?.textContent).toContain("cannot be used");

    await act(async () => btn.click());
    expect(host.querySelector("[aria-live]")?.textContent).toContain("Press the new shortcut");
    await key(btn, { code: "KeyS", key: "S", shiftKey: true });
    expect(btn.getAttribute("aria-describedby")).toBe("hk-x-error");
    expect(host.querySelector("#hk-x-error")?.getAttribute("role")).toBe("alert");
  });

  it("Backspace clears, Escape cancels without a change, and a held key is ignored", async () => {
    const { events, render, onChange } = renderWithBackend();
    await render();
    const btn = host.querySelector<HTMLButtonElement>("#hk-x")!;

    await act(async () => btn.click());
    await key(btn, { code: "KeyS", key: "s", ctrlKey: true, repeat: true });
    expect(onChange).not.toHaveBeenCalled();
    expect(btn.getAttribute("data-hotkey-capturing")).toBe("true");
    await key(btn, { code: "Escape", key: "Escape" });
    expect(onChange).not.toHaveBeenCalled();
    expect(btn.getAttribute("data-hotkey-capturing")).toBeNull();

    await act(async () => btn.click());
    await key(btn, { code: "Backspace", key: "Backspace" });
    expect(onChange).toHaveBeenCalledWith("");
    expect(events.at(-1)).toBe(false);
  });
});
