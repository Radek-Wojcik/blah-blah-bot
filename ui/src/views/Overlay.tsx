import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import type { Region } from "../ipc/types";

interface Point {
  x: number;
  y: number;
}

/** Turn two corner points into a region in fractions of the stage size. */
export function toRegion(a: Point, b: Point, width: number, height: number): Region {
  const x0 = Math.min(a.x, b.x);
  const y0 = Math.min(a.y, b.y);
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  return {
    x: clamp(x0 / width),
    y: clamp(y0 / height),
    w: clamp(Math.abs(a.x - b.x) / width),
    h: clamp(Math.abs(a.y - b.y) / height),
  };
}

/** Resolves once the browser has decoded the image, so showing it later costs no waiting. */
export function decodeImage(url: string): Promise<void> {
  const img = new Image();
  img.src = url;
  return "decode" in img ? img.decode() : Promise.resolve();
}

/**
 * Full-desktop overlay: the frozen desktop with a dimming layer; drag to select a region,
 * Escape or right click to cancel. The selection is reported as fractions of the image so it
 * does not depend on monitor scaling.
 *
 * The window starts hidden. Once the image is decoded and in the page (or could not be loaded)
 * the backend is told to show it, so the live desktop is never replaced by a black window.
 */
export function Overlay() {
  const backend = useBackend();
  const stage = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [drag, setDrag] = useState<{ from: Point; to: Point } | null>(null);

  useEffect(() => {
    document.body.classList.add("overlay-body");
    let url: string | null = null;
    let cancelled = false;
    backend
      .overlayImage()
      .then(async (u) => {
        url = u;
        await decodeImage(u);
        if (!cancelled) setSrc(u);
      })
      .catch(() => {
        // Say so instead of leaving a black screen.
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
      document.body.classList.remove("overlay-body");
      if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
    };
  }, [backend]);

  const shown = src !== null || failed;
  useEffect(() => {
    if (shown) void backend.overlayReady().catch(() => undefined);
  }, [shown, backend]);

  const cancel = useCallback(() => void backend.overlayFinish(null), [backend]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && cancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cancel]);

  const point = (e: React.PointerEvent): Point => {
    const r = stage.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const box = drag && {
    left: Math.min(drag.from.x, drag.to.x),
    top: Math.min(drag.from.y, drag.to.y),
    width: Math.abs(drag.from.x - drag.to.x),
    height: Math.abs(drag.from.y - drag.to.y),
  };

  return (
    <div
      ref={stage}
      className="shot-stage"
      role="application"
      aria-label="Drag to select a screenshot region, Escape to cancel"
      onContextMenu={(e) => {
        e.preventDefault();
        cancel();
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        const p = point(e);
        setDrag({ from: p, to: p });
      }}
      onPointerMove={(e) => drag && setDrag({ ...drag, to: point(e) })}
      onPointerUp={(e) => {
        if (!drag) return;
        const r = stage.current!.getBoundingClientRect();
        void backend.overlayFinish(toRegion(drag.from, point(e), r.width, r.height));
        setDrag(null);
      }}
    >
      {src && <img className="shot-image" src={src} alt="" draggable={false} />}
      <div className="shot-dim" />
      {box && (
        <div className="shot-box" style={box}>
          <span className="shot-size">
            {Math.round(box.width)} × {Math.round(box.height)}
          </span>
        </div>
      )}
      {!drag && (
        <p className="shot-hint">
          {failed
            ? "The screenshot could not be shown · Esc to cancel"
            : "Drag to capture a region · Esc to cancel"}
        </p>
      )}
    </div>
  );
}
