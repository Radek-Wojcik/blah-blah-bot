import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "./backend-context";
import type { Backend } from "./ipc/backend";
import { Overlay } from "./views/Overlay";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement | null = null;

async function mount(overlayImage: () => Promise<string>) {
  const events: string[] = [];
  const backend = {
    overlayImage: vi.fn(async () => {
      events.push("image");
      return overlayImage();
    }),
    overlayReady: vi.fn(async () => {
      events.push("ready");
    }),
    overlayFinish: vi.fn(async () => {}),
  } as unknown as Backend;
  host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <Overlay />
      </BackendProvider>,
    );
  });
  return { backend, events, root };
}

afterEach(() => {
  host?.remove();
  host = null;
});

describe("overlay window readiness", () => {
  it("asks for the window only after the image is in the page", async () => {
    let release!: (url: string) => void;
    const { backend, events } = await mount(
      () => new Promise<string>((resolve) => (release = resolve)),
    );
    expect(backend.overlayReady).not.toHaveBeenCalled();
    expect(host!.querySelector("img.shot-image")).toBeNull();

    await act(async () => release("data:image/gif;base64,R0lGODlhAQABAAAAACw="));
    expect(host!.querySelector("img.shot-image")).not.toBeNull();
    expect(events).toEqual(["image", "ready"]);
    expect(backend.overlayReady).toHaveBeenCalledTimes(1);
  });

  it("still shows the window, with the error hint, when the image fails", async () => {
    const { backend, events } = await mount(() => Promise.reject(new Error("no capture")));
    expect(host!.querySelector("img.shot-image")).toBeNull();
    expect(host!.textContent).toContain("The screenshot could not be shown");
    expect(events).toEqual(["image", "ready"]);
    expect(backend.overlayReady).toHaveBeenCalledTimes(1);
  });
});
