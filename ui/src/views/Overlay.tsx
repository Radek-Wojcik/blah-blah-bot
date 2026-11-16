import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import type { Region } from "../ipc/types";

interface Point {
  x: number;
  y: number;
}

/** How long the frozen desktop may take to arrive before the overlay gives up. */
export const LOAD_TIMEOUT_MS = 20_000;
/** How long a problem message stays before the overlay closes by itself. */
export const PROBLEM_CLOSE_MS = 6_000;
/** Colour values at or below this count as black when the shown picture is checked. */
const BLACK_LEVEL = 2;

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
 * True when RGBA pixel data shows nothing: every pixel is fully transparent or black. An empty
 * buffer is blank.
 */
export function isBlankPixels(rgba: ArrayLike<number>): boolean {
  for (let i = 0; i + 3 < rgba.length; i += 4) {
    if (
      rgba[i + 3] > 0 &&
      (rgba[i] > BLACK_LEVEL || rgba[i + 1] > BLACK_LEVEL || rgba[i + 2] > BLACK_LEVEL)
    ) {
      return false;
    }
  }
  return true;
}

/** Look at the decoded picture itself, in a small copy. Unknown (no canvas) counts as fine. */
function pictureIsBlank(img: HTMLImageElement): boolean {
  try {
    const canvas = document.createElement("canvas");
    canvas.width = 128;
    canvas.height = 72;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return false;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return isBlankPixels(ctx.getImageData(0, 0, canvas.width, canvas.height).data);
  } catch {
    return false;
  }
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const m = (e as { message: unknown }).message;
    if (typeof m === "string" && m !== "") return m;
  }
  return "The screen capture could not be shown.";
}

type Phase = "loading" | "ready" | "problem";

/**
 * Full-desktop overlay: the frozen desktop with a dimming layer; drag to select a region,
 * Escape, a click or a right click to cancel. The selection is reported as fractions of the
 * image so it does not depend on monitor scaling.
 *
 * The overlay covers every monitor, so it must never stay on screen as an empty black sheet: a
 * capture that does not arrive, cannot be decoded or shows nothing is reported in a message
 * and the overlay closes itself. Escape and any click close it in every state.
 *
 * The window starts hidden. Once the image is decoded and in the page (or a problem is
 * reported) the backend is told to show it, so the live desktop is never replaced by a black
 * window.
 */
export function Overlay() {
  const backend = useBackend();
  const stage = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("loading");
  const [problem, setProblem] = useState("");
  const [drag, setDrag] = useState<{ from: Point; to: Point } | null>(null);

  const fail = useCallback((message: string) => {
    setProblem(message);
    setSrc(null);
    setPhase("problem");
  }, []);

  useEffect(() => {
    document.body.classList.add("overlay-body");
    let url: string | null = null;
    let cancelled = false;
    backend
      .overlayImage()
      .then(
        async (u) => {
          url = u;
          try {
            await decodeImage(u);
          } catch {
            if (!cancelled) {
              fail("The screen capture could not be decoded. The screenshot was cancelled.");
            }
            return;
          }
          if (!cancelled) setSrc(u);
        },
        (e: unknown) => {
          if (!cancelled) fail(messageOf(e));
        },
      )
      .catch(() => undefined);
    return () => {
      cancelled = true;
      document.body.classList.remove("overlay-body");
      if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
    };
  }, [backend, fail]);

  // Show the window only once there is something to show: the decoded picture or the message.
  const shown = src !== null || phase === "problem";
  useEffect(() => {
    if (shown) void backend.overlayReady().catch(() => undefined);
  }, [shown, backend]);

  // A capture that never arrives must not leave an empty sheet over every monitor.
  useEffect(() => {
    if (phase !== "loading") return;
    const id = window.setTimeout(
      () => fail("The screen capture did not arrive in time. The screenshot was cancelled."),
      LOAD_TIMEOUT_MS,
    );
    return () => window.clearTimeout(id);
  }, [phase, fail]);

  const cancel = useCallback(() => void backend.overlayFinish(null), [backend]);

  // After a problem the message is read, then the overlay goes away on its own.
  useEffect(() => {
    if (phase !== "problem") return;
    const id = window.setTimeout(cancel, PROBLEM_CLOSE_MS);
    return () => window.clearTimeout(id);
  }, [phase, cancel]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || phase === "problem") cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cancel, phase]);

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
      data-phase={phase}
      role="application"
      aria-label="Drag to select a screenshot region, Escape to cancel"
      onContextMenu={(e) => {
        e.preventDefault();
        cancel();
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        // Without a usable picture a click can only mean "get me out of here".
        if (phase !== "ready") {
          cancel();
          return;
        }
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
      {src && (
        <img
          className="shot-image"
          src={src}
          alt=""
          draggable={false}
          onLoad={(e) => {
            const img = e.currentTarget;
            if (img.naturalWidth === 0 || img.naturalHeight === 0) {
              fail("The screen capture could not be decoded. The screenshot was cancelled.");
            } else if (pictureIsBlank(img)) {
              fail("The screen capture came back empty (all black). The screenshot was cancelled.");
            } else {
              setPhase("ready");
            }
          }}
          onError={() =>
            fail("The screen capture could not be displayed. The screenshot was cancelled.")
          }
        />
      )}
      <div className="shot-dim" />
      {box && (
        <div className="shot-box" style={box}>
          <span className="shot-size">
            {Math.round(box.width)} × {Math.round(box.height)}
          </span>
        </div>
      )}
      {phase === "ready" && !drag && (
        <p className="shot-hint">Drag to capture a region · Esc to cancel</p>
      )}
      {phase === "loading" && (
        <p className="shot-hint" role="status">
          Preparing the screenshot… · Esc to cancel
        </p>
      )}
      {phase === "problem" && (
        <div className="shot-problem" role="alert">
          <strong>Screenshot failed</strong>
          <span>{problem}</span>
          <small>Click or press any key to close.</small>
        </div>
      )}
    </div>
  );
}
