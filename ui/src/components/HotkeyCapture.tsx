import { useCallback, useEffect, useRef, useState } from "react";
import { useOptionalBackend } from "../backend-context";
import { ALTGR_WARNING, captureKeyDown, formatHotkeyDisplay, isAltGrCombo } from "../hotkeys";

export interface HotkeyCaptureProps {
  id: string;
  label: string;
  value: string;
  otherHotkey?: string;
  disabled?: boolean;
  /** Registration failure of the saved shortcut (taken by another application), if any. */
  registrationError?: string | null;
  onChange: (next: string) => void;
}

const CAPTURE_HINT = "Press the new shortcut. Escape cancels, Backspace clears.";

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
  registrationError = null,
  onChange,
}: HotkeyCaptureProps) {
  const backend = useOptionalBackend();
  const [capturing, setCapturing] = useState(false);
  const [preview, setPreview] = useState<string>("Press shortcut…");
  const [error, setError] = useState<string | null>(null);
  const capturingRef = useRef(false);

  // While recording a shortcut the global hotkeys are unregistered, so pressing the current
  // combination is captured instead of triggering its action.
  const startCapture = useCallback(() => {
    if (capturingRef.current) return;
    capturingRef.current = true;
    setCapturing(true);
    void backend?.suspendHotkeys().catch(() => {});
  }, [backend]);

  const stopCapture = useCallback(() => {
    setPreview("Press shortcut…");
    setCapturing(false);
    if (!capturingRef.current) return;
    capturingRef.current = false;
    void backend?.resumeHotkeys().catch(() => {});
  }, [backend]);

  // Leaving the screen mid-capture must not leave the hotkeys unregistered.
  useEffect(
    () => () => {
      if (capturingRef.current) {
        capturingRef.current = false;
        void backend?.resumeHotkeys().catch(() => {});
      }
    },
    [backend],
  );

  const handleKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!capturing) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setError(null);
        startCapture();
      }
      return;
    }

    const outcome = captureKeyDown(e.nativeEvent, otherHotkey);
    if (outcome.kind === "leave") {
      // Let the browser move focus; blur ends the capture without recording anything.
      stopCapture();
      return;
    }

    e.preventDefault();
    e.stopPropagation();

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

  const showAltGr = !capturing && isAltGrCombo(value);
  const warningId = `${id}-warning`;
  const errorId = `${id}-error`;
  const failureId = `${id}-failure`;
  const describedBy =
    [
      showAltGr ? warningId : null,
      error ? errorId : null,
      registrationError && !capturing ? failureId : null,
    ]
      .filter(Boolean)
      .join(" ") || undefined;

  return (
    <div className="hotkey-field">
      <label id={`${id}-label`} htmlFor={id}>
        {label}
      </label>
      <div className="hotkey-capture-row">
        <button
          id={id}
          type="button"
          className={
            capturing ? "btn btn-small hotkey-btn hotkey-capturing" : "btn btn-small hotkey-btn"
          }
          aria-labelledby={`${id}-label ${id}`}
          aria-describedby={describedBy}
          data-hotkey-capturing={capturing ? "true" : undefined}
          disabled={disabled}
          onClick={() => {
            if (capturing) {
              stopCapture();
            } else {
              setError(null);
              startCapture();
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
            aria-label={`Clear ${label}`}
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
      <div className="visually-hidden" aria-live="polite">
        {capturing ? CAPTURE_HINT : ""}
      </div>
      {showAltGr && (
        <p id={warningId} className="hotkey-warning" role="status">
          {ALTGR_WARNING}
        </p>
      )}
      {error && (
        <p id={errorId} className="error" role="alert">
          {error}
        </p>
      )}
      {registrationError && !capturing && (
        <p id={failureId} className="error" role="alert">
          {registrationError}
        </p>
      )}
    </div>
  );
}
