export const DEFAULT_RECORD_HOTKEY = "Ctrl+Shift+R";
export const DEFAULT_SCREENSHOT_HOTKEY = "Ctrl+Shift+S";

export const ALTGR_WARNING =
  "This is AltGr on many keyboards and blocks typing letters like ś, ą, ę.";

export const SETTINGS_CHANGED_EVENT = "bbb:settings-changed";

export function notifySettingsChanged(): void {
  window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED_EVENT));
}

export function onSettingsChanged(listener: () => void): () => void {
  window.addEventListener(SETTINGS_CHANGED_EVENT, listener);
  return () => window.removeEventListener(SETTINGS_CHANGED_EVENT, listener);
}

const CODE_TO_KEY: Record<string, string> = {
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Insert: "Insert",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  Comma: ",",
  Period: ".",
  Slash: "/",
  NumpadAdd: "NumpadAdd",
  NumpadSubtract: "NumpadSubtract",
  NumpadMultiply: "NumpadMultiply",
  NumpadDivide: "NumpadDivide",
  NumpadDecimal: "NumpadDecimal",
  NumpadEnter: "NumpadEnter",
};

const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
  "AltGraph",
]);

function keyFromCode(code: string): string | null {
  if (/^Key([A-Z])$/.test(code)) {
    return code.slice(3);
  }
  if (/^Digit([0-9])$/.test(code)) {
    return code.slice(5);
  }
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) {
    return code;
  }
  if (/^Numpad([0-9])$/.test(code)) {
    return code;
  }
  return CODE_TO_KEY[code] ?? null;
}

export function normalizeHotkey(hotkey: string): string {
  const parts = hotkey
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return "";
  const mods = new Set<string>();
  let key = "";
  for (const part of parts) {
    const lower = part.toLowerCase();
    if (lower === "ctrl" || lower === "control") mods.add("Ctrl");
    else if (lower === "alt") mods.add("Alt");
    else if (lower === "shift") mods.add("Shift");
    else if (lower === "super" || lower === "meta" || lower === "win" || lower === "cmd") {
      mods.add("Super");
    } else {
      key = part.length === 1 ? part.toUpperCase() : part;
    }
  }
  const ordered: string[] = [];
  if (mods.has("Ctrl")) ordered.push("Ctrl");
  if (mods.has("Alt")) ordered.push("Alt");
  if (mods.has("Shift")) ordered.push("Shift");
  if (mods.has("Super")) ordered.push("Super");
  if (key) ordered.push(key);
  return ordered.join("+");
}

export function isAltGrLetterCombo(hotkey: string): boolean {
  const norm = normalizeHotkey(hotkey);
  if (!norm) return false;
  const parts = norm.split("+");
  const key = parts[parts.length - 1];
  const mods = new Set(parts.slice(0, -1));
  return (
    mods.has("Ctrl") &&
    mods.has("Alt") &&
    !mods.has("Shift") &&
    !mods.has("Super") &&
    /^[A-Z]$/.test(key)
  );
}

export function formatHotkeyDisplay(hotkey: string): string {
  const trimmed = hotkey.trim();
  return trimmed === "" ? "Not set" : trimmed;
}

export type CaptureKeyLike = Pick<
  KeyboardEvent,
  "code" | "key" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey"
> & {
  getModifierState?: (keyArg: string) => boolean;
};

export type CaptureOutcome =
  | { kind: "captured"; hotkey: string }
  | { kind: "cancel" }
  | { kind: "clear" }
  | { kind: "ignore" }
  | { kind: "error"; message: string };

export function captureKeyDown(e: CaptureKeyLike, otherHotkey?: string): CaptureOutcome {
  if (e.code === "Escape" || e.key === "Escape") {
    return { kind: "cancel" };
  }
  if (
    (e.code === "Backspace" || e.key === "Backspace") &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.shiftKey &&
    !e.metaKey
  ) {
    return { kind: "clear" };
  }
  if (MODIFIER_CODES.has(e.code) || e.key === "AltGraph") {
    return { kind: "ignore" };
  }

  const key = keyFromCode(e.code);
  if (!key) {
    return { kind: "error", message: "Unsupported key for global hotkey." };
  }

  const altGraph = typeof e.getModifierState === "function" && e.getModifierState("AltGraph");
  const ctrl = e.ctrlKey || altGraph;
  const alt = e.altKey || altGraph;
  const shift = e.shiftKey;
  const meta = e.metaKey;

  if (!ctrl && !alt && !shift && !meta) {
    return {
      kind: "error",
      message: "Include at least one modifier key (Ctrl, Alt, Shift, or Super).",
    };
  }

  const parts: string[] = [];
  if (ctrl) parts.push("Ctrl");
  if (alt) parts.push("Alt");
  if (shift) parts.push("Shift");
  if (meta) parts.push("Super");
  parts.push(key);

  const hotkey = parts.join("+");
  if (otherHotkey && normalizeHotkey(otherHotkey) === normalizeHotkey(hotkey)) {
    return {
      kind: "error",
      message: "This shortcut is already used by the other hotkey.",
    };
  }

  return { kind: "captured", hotkey };
}
