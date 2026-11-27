import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The elements a user can reach with Tab inside `root`, in document order. */
export function focusableWithin(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => !el.hasAttribute("hidden") && el.getAttribute("aria-hidden") !== "true",
  );
}

/**
 * A modal dialog: Escape closes it, Tab stays inside it, the heading names it for screen
 * readers. Focus moves into the dialog on open and returns to the control that opened it.
 */
export function Modal(props: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Optional footer pinned at the bottom of the dialog while body content scrolls. */
  footer?: ReactNode;
  /** A wider dialog for forms. */
  wide?: boolean;
}) {
  const { title, onClose, children, footer, wide } = props;
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = dialogRef.current;
    // Start with the first control, not the scrollable body.
    const first = dialog
      ? focusableWithin(dialog).find((el) => !el.classList.contains("modal-content"))
      : undefined;
    (first ?? dialog)?.focus();
    return () => {
      if (opener && opener.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Tab" || !dialogRef.current) return;
    const items = focusableWithin(dialogRef.current);
    if (items.length === 0) {
      e.preventDefault();
      dialogRef.current.focus();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || active === dialogRef.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="modal-backdrop">
      <div
        ref={dialogRef}
        className={wide ? "panel modal modal-wide" : "panel modal"}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="panel-body modal-panel-body">
          <h3 id={titleId} className="modal-title">
            {title}
          </h3>
          <div className="modal-content" tabIndex={0} aria-label={`${title} content`}>
            {children}
          </div>
          {footer && <div className="modal-footer">{footer}</div>}
        </div>
      </div>
    </div>
  );
}
