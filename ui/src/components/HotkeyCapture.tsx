import { useState } from "react";
import { ALTGR_WARNING, captureKeyDown, formatHotkeyDisplay, isAltGrLetterCombo } from "../hotkeys";

export interface HotkeyCaptureProps {
  id: string;
  label: string;
  value: string;
  otherHotkey?: string;
  disabled?: boolean;
  onChange: (next: string) => void;
}

function previewModifiers(e: React.KeyboardEvent<HTMLButtonElement>): string {
  const altGraph = e.getModifierState("AltGraph");
  const mods: string[] = [];
  if (e.ctrlKey || altGraph) mods.push("Ctrl");
  if (e.altKey || altGraph) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return mods.length > 0 ? `${mods.join("+")}+…` : "Press shortcut…";
}

export function HotkeyCapture({
  id,
  label,
  value,
  otherHotkey,
  disabled = false,
  onChange,
}: HotkeyCaptureProps) {
  const [capturing, setCapturing] = useState(false);
  const [preview, setPreview] = useState<string>("Press shortcut…");
  const [error, setError] = useState<string | null>(null);

  const stopCapture = () => {
    setCapturing(false);
    setPreview("Press shortcut…");
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!capturing) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setError(null);
        setCapturing(true);
      }
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    const outcome = captureKeyDown(e.nativeEvent, otherHotkey);
    switch (outcome.kind) {
      case "captured":
        setError(null);
        stopCapture();
        onChange(outcome.hotkey);
        break;
      case "cancel":
        setError(null);
        stopCapture();
        break;
      case "clear":
        setError(null);
        stopCapture();
        onChange("");
        break;
      case "ignore":
        setPreview(previewModifiers(e));
        break;
      case "error":
        setError(outcome.message);
        setPreview(previewModifiers(e));
        break;
    }
  };

  const handleKeyUp = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!capturing) return;
    setPreview(previewModifiers(e));
  };

  const showAltGr = !capturing && isAltGrLetterCombo(value);

  return (
    <div className="hotkey-field">
      <label htmlFor={id}>{label}</label>
      <div className="hotkey-capture-row">
        <button
          id={id}
          type="button"
          className={
            capturing ? "btn btn-small hotkey-btn hotkey-capturing" : "btn btn-small hotkey-btn"
          }
          data-hotkey-capturing={capturing ? "true" : undefined}
          disabled={disabled}
          onClick={() => {
            if (capturing) {
              stopCapture();
            } else {
              setError(null);
              setCapturing(true);
            }
          }}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
          onBlur={() => {
            if (capturing) stopCapture();
          }}
        >
          {capturing ? preview : formatHotkeyDisplay(value)}
        </button>
        {value.trim() !== "" && (
          <button
            type="button"
            className="btn btn-small btn-ghost"
            aria-label="Clear shortcut"
            disabled={disabled}
            onClick={() => {
              setError(null);
              stopCapture();
              onChange("");
            }}
          >
            Clear
          </button>
        )}
      </div>
      {showAltGr && (
        <p className="hotkey-warning" role="status">
          {ALTGR_WARNING}
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
