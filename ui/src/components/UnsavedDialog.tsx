import { useEffect, useId } from "react";

export interface UnsavedDialogProps {
  /** True when the navigation-blocking modal confirmation dialog should be shown. */
  open: boolean;
  /** Optional flag to also display an inline unsaved-changes banner bar before navigation. */
  dirty?: boolean;
  /** Save changes and proceed. */
  onSave: () => void | Promise<void>;
  /** Discard unsaved edits and proceed. */
  onDiscard: () => void;
  /** Cancel navigation and stay on the current screen. */
  onCancel?: () => void;
  /** True while a save request is in flight. */
  saving?: boolean;
  /** Custom dialog heading. */
  title?: string;
  /** Custom dialog message. */
  message?: string;
}

/**
 * Reusable inline prompt bar for dirty forms and settings screens.
 */
export function UnsavedBar(props: {
  dirty: boolean;
  onSave: () => void | Promise<void>;
  onDiscard: () => void;
  saving?: boolean;
  message?: string;
}) {
  const { dirty, onSave, onDiscard, saving = false, message } = props;
  if (!dirty) return null;
  return (
    <div className="unsaved-bar" role="status" aria-live="polite">
      <span className="unsaved-bar-text">
        {message ?? "You have unsaved changes on this screen."}
      </span>
      <div className="unsaved-bar-actions">
        <button
          type="button"
          className="btn btn-small btn-ghost"
          disabled={saving}
          onClick={onDiscard}
        >
          Discard
        </button>
        <button
          type="button"
          className="btn btn-small"
          disabled={saving}
          onClick={() => void onSave()}
        >
          {saving ? "Saving…" : "Save changes"}
        </button>
      </div>
    </div>
  );
}

/**
 * Reusable prompt banner + modal confirmation shown when the user has unsaved setting edits
 * and attempts to switch tabs or pages.
 */
export function UnsavedDialog(props: UnsavedDialogProps) {
  const {
    open,
    dirty = false,
    onSave,
    onDiscard,
    onCancel,
    saving = false,
    title = "Unsaved changes",
    message = "You have unsaved changes. Would you like to save them before leaving this view?",
  } = props;
  const titleId = useId();

  useEffect(() => {
    if (!open || !onCancel) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onCancel]);

  if (!open && !dirty) return null;

  return (
    <>
      {(dirty || open) && (
        <UnsavedBar dirty={true} onSave={onSave} onDiscard={onDiscard} saving={saving} />
      )}
      {open && (
        <div className="modal-backdrop">
          <div className="panel modal" role="dialog" aria-modal="true" aria-labelledby={titleId}>
            <div className="panel-body">
              <h3 id={titleId} className="modal-title">
                {title}
              </h3>
              <p className="modal-text">{message}</p>
              <div className="actions">
                {onCancel && (
                  <button
                    type="button"
                    className="btn btn-ghost"
                    disabled={saving}
                    onClick={onCancel}
                  >
                    Cancel
                  </button>
                )}
                <button
                  type="button"
                  className="btn btn-ghost"
                  disabled={saving}
                  onClick={onDiscard}
                >
                  Discard
                </button>
                <button
                  type="button"
                  className="btn btn-record"
                  disabled={saving}
                  onClick={() => void onSave()}
                >
                  {saving ? "Saving…" : "Save"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
