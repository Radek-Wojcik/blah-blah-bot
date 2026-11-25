import { describe, expect, it, vi } from "vitest";
import {
  captureKeyDown,
  DEFAULT_RECORD_HOTKEY,
  DEFAULT_SCREENSHOT_HOTKEY,
  formatHotkeyDisplay,
  isAltGrLetterCombo,
  notifySettingsChanged,
  onSettingsChanged,
} from "./hotkeys";

describe("hotkeys", () => {
  it("uses Ctrl+Shift+R and Ctrl+Shift+S as defaults", () => {
    expect(DEFAULT_RECORD_HOTKEY).toBe("Ctrl+Shift+R");
    expect(DEFAULT_SCREENSHOT_HOTKEY).toBe("Ctrl+Shift+S");
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
      message: "Include at least one modifier key (Ctrl, Alt, Shift, or Super).",
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
    expect(isAltGrLetterCombo("Ctrl+Alt+S")).toBe(true);
    expect(isAltGrLetterCombo("Ctrl+Alt+R")).toBe(true);
    expect(isAltGrLetterCombo("Ctrl+Shift+S")).toBe(false);
    expect(isAltGrLetterCombo("Ctrl+Alt+F9")).toBe(false);
    expect(isAltGrLetterCombo("")).toBe(false);

    expect(formatHotkeyDisplay("Ctrl+Shift+R")).toBe("Ctrl+Shift+R");
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
