import { describe, expect, it, vi } from "vitest";
import {
  captureKeyDown,
  DEFAULT_RECORD_HOTKEY,
  DEFAULT_SCREENSHOT_HOTKEY,
  formatHotkeyDisplay,
  isAltGrCombo,
  validateHotkey,
  notifySettingsChanged,
  onSettingsChanged,
} from "./hotkeys";

describe("hotkeys", () => {
  it("uses Ctrl+Alt+Shift+F9 and Ctrl+Alt+Shift+F10 as defaults, valid and without AltGr clash", () => {
    expect(DEFAULT_RECORD_HOTKEY).toBe("Ctrl+Alt+Shift+F9");
    expect(DEFAULT_SCREENSHOT_HOTKEY).toBe("Ctrl+Alt+Shift+F10");
    expect(validateHotkey(DEFAULT_RECORD_HOTKEY)).toBeNull();
    expect(validateHotkey(DEFAULT_SCREENSHOT_HOTKEY)).toBeNull();
    expect(isAltGrCombo(DEFAULT_RECORD_HOTKEY)).toBe(false);
    expect(isAltGrCombo(DEFAULT_SCREENSHOT_HOTKEY)).toBe(false);
  });

  it("rejects Shift alone (except with F-keys) and lets Tab leave without recording", () => {
    const base = { ctrlKey: false, altKey: false, shiftKey: false, metaKey: false };
    const shiftS = captureKeyDown({ ...base, code: "KeyS", key: "S", shiftKey: true });
    expect(shiftS.kind).toBe("error");
    expect(captureKeyDown({ ...base, code: "Space", key: " ", shiftKey: true }).kind).toBe("error");
    expect(captureKeyDown({ ...base, code: "Tab", key: "Tab", shiftKey: true })).toEqual({
      kind: "leave",
    });
    expect(captureKeyDown({ ...base, code: "Tab", key: "Tab" })).toEqual({ kind: "leave" });
    expect(captureKeyDown({ ...base, code: "F9", key: "F9", shiftKey: true })).toEqual({
      kind: "captured",
      hotkey: "Shift+F9",
    });
    expect(captureKeyDown({ ...base, code: "KeyR", key: "r", metaKey: true })).toEqual({
      kind: "captured",
      hotkey: "Super+R",
    });
    expect(captureKeyDown({ ...base, code: "Tab", key: "Tab", ctrlKey: true })).toEqual({
      kind: "captured",
      hotkey: "Ctrl+Tab",
    });
  });

  it("validates hotkeys like the application does", () => {
    expect(validateHotkey("")).toBeNull();
    expect(validateHotkey("Ctrl+S")).toBeNull();
    expect(validateHotkey("Super+R")).toBeNull();
    expect(validateHotkey("Shift+F9")).toBeNull();
    expect(validateHotkey("Shift+S")).not.toBeNull();
    expect(validateHotkey("Shift+Tab")).not.toBeNull();
    expect(validateHotkey("F9")).not.toBeNull();
  });

  it("captures letter, digit, F-key, arrow, punctuation, and numpad shortcuts", () => {
    expect(
      captureKeyDown({
        code: "KeyR",
        key: "r",
        ctrlKey: true,
        altKey: false,
        shiftKey: true,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Ctrl+Shift+R" });

    expect(
      captureKeyDown({
        code: "Digit9",
        key: "9",
        ctrlKey: false,
        altKey: true,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Alt+9" });

    expect(
      captureKeyDown({
        code: "F12",
        key: "F12",
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Ctrl+F12" });

    expect(
      captureKeyDown({
        code: "ArrowUp",
        key: "ArrowUp",
        ctrlKey: false,
        altKey: true,
        shiftKey: true,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Alt+Shift+Up" });

    expect(
      captureKeyDown({
        code: "Slash",
        key: "/",
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Ctrl+/" });

    expect(
      captureKeyDown({
        code: "Numpad1",
        key: "1",
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "captured", hotkey: "Ctrl+Numpad1" });
  });

  it("handles Escape, Backspace, modifier-only keys, AltGraph, missing modifier, and conflicts", () => {
    expect(
      captureKeyDown({
        code: "Escape",
        key: "Escape",
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "cancel" });

    expect(
      captureKeyDown({
        code: "Backspace",
        key: "Backspace",
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "clear" });

    expect(
      captureKeyDown({
        code: "ControlLeft",
        key: "Control",
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({ kind: "ignore" });

    expect(
      captureKeyDown({
        code: "KeyS",
        key: "ś",
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        metaKey: false,
        getModifierState: (k) => k === "AltGraph",
      }),
    ).toEqual({ kind: "captured", hotkey: "Ctrl+Alt+S" });

    expect(
      captureKeyDown({
        code: "KeyR",
        key: "r",
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        metaKey: false,
      }),
    ).toEqual({
      kind: "error",
      message: "Include Ctrl, Alt or Super (Shift alone works only with F1-F24).",
    });

    expect(
      captureKeyDown(
        { code: "KeyS", key: "s", ctrlKey: true, altKey: false, shiftKey: true, metaKey: false },
        "shift+ctrl+s",
      ),
    ).toEqual({
      kind: "error",
      message: "This shortcut is already used by the other hotkey.",
    });
  });

  it("detects AltGr letter combos and formats display", () => {
    expect(isAltGrCombo("Ctrl+Alt+S")).toBe(true);
    expect(isAltGrCombo("Ctrl+Alt+R")).toBe(true);
    expect(isAltGrCombo("Ctrl+Alt+Shift+S")).toBe(true);
    expect(isAltGrCombo("Ctrl+Alt+7")).toBe(true);
    expect(isAltGrCombo("Ctrl+Alt+;")).toBe(true);
    expect(isAltGrCombo("Ctrl+Shift+S")).toBe(false);
    expect(isAltGrCombo("Ctrl+Alt+F9")).toBe(false);
    expect(isAltGrCombo("Ctrl+Alt+Space")).toBe(false);
    expect(isAltGrCombo("Ctrl+Alt+Super+R")).toBe(false);
    expect(isAltGrCombo("")).toBe(false);

    expect(formatHotkeyDisplay("Ctrl+Alt+Shift+F9")).toBe("Ctrl+Alt+Shift+F9");
    expect(formatHotkeyDisplay("")).toBe("Not set");
  });

  it("notifies settings-changed listeners", () => {
    const spy = vi.fn();
    const unsub = onSettingsChanged(spy);
    notifySettingsChanged();
    expect(spy).toHaveBeenCalledTimes(1);
    unsub();
    notifySettingsChanged();
    expect(spy).toHaveBeenCalledTimes(1);
  });
});
