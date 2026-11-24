import { type ReactNode, useEffect, useId, useRef, useState } from "react";

export interface HelpPopoverProps {
  /** Heading inside the popover and default basis for the trigger button's aria-label. */
  title?: string;
  /** Accessible label for the `?` trigger button (defaults to `Help: <title>` or `Help`). */
  label?: string;
  /** Optional plain-text explanation when `children` is not provided. */
  text?: string;
  /** Optional recommended value or tip highlighted at the bottom of the popover. */
  recommended?: string;
  /** Rich explanation content inside the popover. */
  children?: ReactNode;
}

/**
 * Accessible `?` icon button that toggles an explanation popover on click or keyboard activation
 * and closes on outside click or `Escape`.
 */
export function HelpPopover(props: HelpPopoverProps) {
  const { title, label, text, recommended, children } = props;
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLSpanElement | null>(null);
  const popId = useId();
  const ariaLabel = label ?? (title ? `Help: ${title}` : "Help");

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    const onPointerDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    document.addEventListener("mousedown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [open]);

  return (
    <span className="help-popover" ref={rootRef}>
      <button
        type="button"
        className={open ? "help-popover-btn help-popover-btn-open" : "help-popover-btn"}
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={open ? popId : undefined}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setOpen((prev) => !prev);
        }}
      >
        ?
      </button>
      {open && (
        <div id={popId} className="help-popover-card" role="region" aria-label={title ?? ariaLabel}>
          {title && <strong className="help-popover-title">{title}</strong>}
          <div className="help-popover-body">
            {text && <p>{text}</p>}
            {children}
          </div>
          {recommended && (
            <p className="help-popover-rec">
              <strong>Recommended:</strong> {recommended}
            </p>
          )}
        </div>
      )}
    </span>
  );
}
