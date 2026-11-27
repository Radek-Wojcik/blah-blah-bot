import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import { createMockBackend } from "../ipc/mock";
import { VoiceProfiles } from "./VoiceProfiles";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;
const origCreateObjectURL = URL.createObjectURL;
const origRevokeObjectURL = URL.revokeObjectURL;

async function flush(ticks = 6) {
  for (let i = 0; i < ticks; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  URL.createObjectURL = origCreateObjectURL;
  URL.revokeObjectURL = origRevokeObjectURL;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("VoiceProfiles sample clip playback", () => {
  it("wraps WAV bytes into an audio/wav Blob, plays via blob: URL, and stops with error on audio.onerror", async () => {
    const createObjectURL = vi.fn((blob: Blob) => {
      expect(blob).toBeInstanceOf(Blob);
      expect(blob.type).toBe("audio/wav");
      return "blob:http://localhost/sample-wav";
    });
    const revokeObjectURL = vi.fn();
    URL.createObjectURL = createObjectURL;
    URL.revokeObjectURL = revokeObjectURL;

    const play = vi.fn(() => Promise.resolve());
    const pause = vi.fn();
    const createdAudio: {
      src: string;
      onended: (() => void) | null;
      onerror: (() => void) | null;
      play: typeof play;
      pause: typeof pause;
    }[] = [];
    class MockAudio {
      src: string;
      onended: (() => void) | null = null;
      onerror: (() => void) | null = null;
      play = play;
      pause = pause;
      constructor(src: string) {
        this.src = src;
        createdAudio.push(this);
      }
    }
    vi.stubGlobal("Audio", MockAudio);

    const backend = createMockBackend({ clockRate: 0 });
    vi.spyOn(backend, "listAllVoiceProfiles").mockResolvedValue([
      {
        id: "p1",
        name: "Alice",
        cycleId: "c1",
        projects: ["Demo"],
        sampleCount: 2,
        hasSampleClip: true,
        updatedUnixMs: 1700000000000,
      },
    ]);
    vi.spyOn(backend, "voiceProfileSampleClip").mockResolvedValue([82, 73, 70, 70]);

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <VoiceProfiles />
        </BackendProvider>,
      );
    });
    await flush();

    const playBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Play sample for Alice"]',
    );
    expect(playBtn).not.toBeNull();

    await act(async () => {
      playBtn!.click();
    });
    await flush();

    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(createdAudio).toHaveLength(1);
    expect(createdAudio[0].src).toMatch(/^blob:/);
    expect(play).toHaveBeenCalledTimes(1);

    const stopBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Stop sample for Alice"]',
    );
    expect(stopBtn).not.toBeNull();

    act(() => {
      createdAudio[0].onerror?.();
    });

    expect(pause).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:http://localhost/sample-wav");
    expect(
      host.querySelector<HTMLButtonElement>('button[aria-label="Play sample for Alice"]'),
    ).not.toBeNull();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain(
      "Sample clip could not be played.",
    );
  });
});
