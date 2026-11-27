import { type ReactNode, useEffect, useId } from "react";

/** A modal dialog: Escape closes it, the heading names it for screen readers. */
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
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="modal-backdrop">
      <div
        className={wide ? "panel modal modal-wide" : "panel modal"}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <div className="panel-body modal-panel-body">
          <h3 id={titleId} className="modal-title">
            {title}
          </h3>
          <div className="modal-content">{children}</div>
          {footer && <div className="modal-footer">{footer}</div>}
        </div>
      </div>
    </div>
  );
}
