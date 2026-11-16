import { afterEach, describe, expect, it, vi } from "vitest";
import { pngObjectUrl } from "./tauri";

const PNG_MAGIC = [137, 80, 78, 71];

describe("pngObjectUrl", () => {
  afterEach(() => vi.restoreAllMocks());

  function blobFor(bytes: ArrayBuffer | number[]): Blob {
    const create = vi.fn(() => "blob:test");
    vi.stubGlobal("URL", { createObjectURL: create });
    expect(pngObjectUrl(bytes)).toBe("blob:test");
    vi.unstubAllGlobals();
    return (create.mock.calls[0] as unknown as [Blob])[0];
  }

  it("keeps the bytes of an ArrayBuffer", () => {
    const blob = blobFor(new Uint8Array(PNG_MAGIC).buffer);
    expect(blob.size).toBe(4);
    expect(blob.type).toBe("image/png");
  });

  it("turns the number array of the postMessage fallback into bytes, not into text", () => {
    // A Blob made straight from the array would hold the text "137,80,78,71" (12 bytes).
    const blob = blobFor(PNG_MAGIC);
    expect(blob.size).toBe(4);
    expect(blob.type).toBe("image/png");
  });
});
