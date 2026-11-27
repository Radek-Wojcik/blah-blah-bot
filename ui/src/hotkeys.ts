export const DEFAULT_RECORD_HOTKEY = "Ctrl+Alt+Shift+F9";
export const DEFAULT_SCREENSHOT_HOTKEY = "Ctrl+Alt+Shift+F10";

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

const NAMED_KEYS = new Map(
  [...Object.values(CODE_TO_KEY), ...Array.from({ length: 10 }, (_, i) => `Numpad${i}`)].map(
    (k) => [k.toLowerCase(), k],
  ),
);

/** Canonical spelling of a key name: `f9` -> `F9`, `pageup` -> `PageUp`, `s` -> `S`. */
function normalizeKeyName(part: string): string {
  if (part.length === 1) return part.toUpperCase();
  if (/^f([1-9]|1[0-9]|2[0-4])$/i.test(part)) return part.toUpperCase();
  return NAMED_KEYS.get(part.toLowerCase()) ?? part;
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
      key = normalizeKeyName(part);
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

/**
 * True for Ctrl+Alt (with or without Shift) plus a printable key: on many layouts that is AltGr
 * and types a character (Polish ś, ą, ę; German @, {; and so on).
 */
export function isAltGrCombo(hotkey: string): boolean {
  const norm = normalizeHotkey(hotkey);
  if (!norm) return false;
  const parts = norm.split("+");
  const key = parts[parts.length - 1];
  const mods = new Set(parts.slice(0, -1));
  return (
    mods.has("Ctrl") &&
    mods.has("Alt") &&
    !mods.has("Super") &&
    /^[A-Z0-9`\-=[\]\\;',./]$/.test(key)
  );
}

const MODIFIER_HELP = "Include Ctrl, Alt or Super (Shift alone works only with F1-F24).";

function isFunctionKey(key: string): boolean {
  return /^F([1-9]|1[0-9]|2[0-4])$/.test(key);
}

/**
 * Same rule as the application: a global hotkey needs Ctrl, Alt or Super; Shift alone is only
 * allowed with an F-key. Returns an error message, or null when the hotkey is acceptable
 * (an empty string means "not set").
 */
export function validateHotkey(hotkey: string): string | null {
  const norm = normalizeHotkey(hotkey);
  if (!norm) return null;
  const parts = norm.split("+");
  const key = parts[parts.length - 1];
  const mods = new Set(parts.slice(0, -1));
  if (mods.size === 0 || mods.has("Ctrl") || mods.has("Alt") || mods.has("Super")) {
    return mods.size === 0 ? MODIFIER_HELP : null;
  }
  return isFunctionKey(key) ? null : MODIFIER_HELP;
}

export function formatHotkeyDisplay(hotkey: string): string {
  const trimmed = hotkey.trim();
  return trimmed === "" ? "Not set" : trimmed;
}

export type CaptureKeyLike = Pick<
  KeyboardEvent,
  "code" | "key" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey"
> & {
  repeat?: boolean;
  getModifierState?: (keyArg: string) => boolean;
};

export type CaptureOutcome =
  | { kind: "captured"; hotkey: string }
  | { kind: "cancel" }
  | { kind: "clear" }
  | { kind: "ignore" }
  | { kind: "leave" }
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
  // Tab and Shift+Tab move focus out of the field; they are never recorded.
  if (
    (e.code === "Tab" || e.key === "Tab") &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey &&
    !(typeof e.getModifierState === "function" && e.getModifierState("AltGraph"))
  ) {
    return { kind: "leave" };
  }

  if (e.repeat) {
    // A held key must not re-run the capture (it would repeat errors or save twice).
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

  if (!ctrl && !alt && !meta && !(shift && isFunctionKey(key))) {
    return { kind: "error", message: MODIFIER_HELP };
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
