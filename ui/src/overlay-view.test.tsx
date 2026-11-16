import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "./backend-context";
import type { Backend } from "./ipc/backend";
import { bytesToBlob } from "./ipc/bytes";
import { createMockBackend } from "./ipc/mock";
import { LOAD_TIMEOUT_MS, Overlay, PROBLEM_CLOSE_MS, isBlankPixels } from "./views/Overlay";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.useFakeTimers();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  // jsdom has no canvas: the picture check is then skipped, as in any browser without one.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function show(backend: Backend) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <Overlay />
      </BackendProvider>,
    );
  });
}

/** What the browser does when the picture has been decoded (jsdom never loads images). */
async function decoded(width: number, height: number) {
  const img = host.querySelector("img.shot-image") as HTMLImageElement;
  Object.defineProperty(img, "naturalWidth", { value: width, configurable: true });
  Object.defineProperty(img, "naturalHeight", { value: height, configurable: true });
  await act(async () => {
    img.dispatchEvent(new Event("load"));
  });
  return img;
}

const stage = () => host.querySelector(".shot-stage") as HTMLElement;
const alertText = () => host.querySelector("[role=alert]")?.textContent ?? "";

function spyFinish(backend: Backend) {
  const finish = vi.fn(backend.overlayFinish);
  return { ...backend, overlayFinish: finish } as Backend & { overlayFinish: typeof finish };
}

describe("screenshot overlay view", () => {
  it("shows the picture visibly once it is decoded", async () => {
    await show(createMockBackend({ clockRate: 0 }));
    expect(stage().dataset.phase).toBe("loading");
    const img = await decoded(1600, 900);
    expect(stage().dataset.phase).toBe("ready");
    expect(img.src).toMatch(/^data:image\/svg/);
    expect(img.classList.contains("shot-image")).toBe(true);
    expect(img.hidden).toBe(false);
    expect(host.querySelector(".shot-hint")?.textContent).toContain("Drag to capture");
    expect(host.querySelector("[role=alert]")).toBeNull();
  });

  it("says it is preparing while the capture has not arrived", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await show({ ...backend, overlayImage: () => new Promise<string>(() => {}) });
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector(".shot-hint")?.textContent).toContain("Preparing");
  });

  it("cancels with a message when the capture never arrives", async () => {
    const backend = spyFinish(createMockBackend({ clockRate: 0 }));
    await show({ ...backend, overlayImage: () => new Promise<string>(() => {}) });
    await act(async () => {
      vi.advanceTimersByTime(LOAD_TIMEOUT_MS + 1);
    });
    expect(stage().dataset.phase).toBe("problem");
    expect(alertText()).toContain("did not arrive");
    await act(async () => {
      vi.advanceTimersByTime(PROBLEM_CLOSE_MS + 1);
    });
    expect(backend.overlayFinish).toHaveBeenCalledWith(null);
  });

  it("shows the backend's refusal of a blank capture and closes itself", async () => {
    const backend = spyFinish(createMockBackend({ clockRate: 0, overlayFault: "blank" }));
    await show(backend);
    expect(stage().dataset.phase).toBe("problem");
    expect(alertText()).toContain("came back empty");
    expect(host.querySelector("img")).toBeNull();
    expect(backend.overlayFinish).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(PROBLEM_CLOSE_MS + 1);
    });
    expect(backend.overlayFinish).toHaveBeenCalledWith(null);
  });

  it("reports a picture the browser cannot decode", async () => {
    await show(createMockBackend({ clockRate: 0, overlayFault: "broken" }));
    await decoded(0, 0);
    expect(stage().dataset.phase).toBe("problem");
    expect(alertText()).toContain("could not be decoded");
  });

  it("reports an image error", async () => {
    await show(createMockBackend({ clockRate: 0 }));
    const img = host.querySelector("img.shot-image") as HTMLImageElement;
    await act(async () => {
      img.dispatchEvent(new Event("error"));
    });
    expect(stage().dataset.phase).toBe("problem");
    expect(alertText()).toContain("could not be displayed");
  });

  it("reports a decoded picture that is black", async () => {
    const black = new Uint8ClampedArray(128 * 72 * 4);
    for (let i = 3; i < black.length; i += 4) black[i] = 255;
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      drawImage: () => {},
      getImageData: () => ({ data: black }),
    } as unknown as CanvasRenderingContext2D);
    await show(createMockBackend({ clockRate: 0 }));
    await decoded(1600, 900);
    expect(stage().dataset.phase).toBe("problem");
    expect(alertText()).toContain("all black");
  });

  it("Escape closes the overlay in every state", async () => {
    for (const fault of [undefined, "blank"] as const) {
      const backend = spyFinish(createMockBackend({ clockRate: 0, overlayFault: fault }));
      await show(backend);
      if (!fault) await decoded(1600, 900);
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      });
      expect(backend.overlayFinish).toHaveBeenCalledWith(null);
    }
  });

  it("a click closes the overlay while loading and after a problem", async () => {
    const loading = spyFinish(createMockBackend({ clockRate: 0 }));
    await show({ ...loading, overlayImage: () => new Promise<string>(() => {}) });
    await act(async () => {
      stage().dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    });
    expect(loading.overlayFinish).toHaveBeenCalledWith(null);

    const failed = spyFinish(createMockBackend({ clockRate: 0, overlayFault: "blank" }));
    await show(failed);
    await act(async () => {
      stage().dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    });
    expect(failed.overlayFinish).toHaveBeenCalledWith(null);
  });

  it("any key closes the message after a problem", async () => {
    const backend = spyFinish(createMockBackend({ clockRate: 0, overlayFault: "blank" }));
    await show(backend);
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    });
    expect(backend.overlayFinish).toHaveBeenCalledWith(null);
  });
});

describe("isBlankPixels", () => {
  it("treats black and transparent pictures as blank", () => {
    expect(isBlankPixels([])).toBe(true);
    expect(isBlankPixels([0, 0, 0, 255, 1, 2, 2, 255])).toBe(true);
    expect(isBlankPixels([200, 100, 50, 0])).toBe(true);
  });
  it("finds a single visible pixel", () => {
    expect(isBlankPixels([0, 0, 0, 255, 0, 0, 3, 255])).toBe(false);
    expect(isBlankPixels([255, 255, 255, 255])).toBe(false);
  });
});

describe("bytesToBlob", () => {
  const png = [137, 80, 78, 71, 13, 10, 26, 10];

  async function bytesOf(blob: Blob): Promise<number[]> {
    return Array.from(
      new Uint8Array(
        await new Promise<ArrayBuffer>((resolve) => {
          const r = new FileReader();
          r.onload = () => resolve(r.result as ArrayBuffer);
          r.readAsArrayBuffer(blob);
        }),
      ),
    );
  }

  it("keeps the bytes of an ArrayBuffer and of the postMessage fallback array", async () => {
    vi.useRealTimers();
    const fast = bytesToBlob(new Uint8Array(png).buffer, "image/png");
    const fallback = bytesToBlob(png, "image/png");
    expect(fast.type).toBe("image/png");
    expect(await bytesOf(fast)).toEqual(png);
    // A Blob made from the array itself would hold the text "137,80,78,..." (the black screen).
    expect(await bytesOf(fallback)).toEqual(png);
    expect(fallback.size).toBe(png.length);
  });
});
