import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { EQ_BAR_COUNT, WaveformEq } from "./WaveformEq";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

describe("WaveformEq", () => {
  it("renders idle bars without a peak-hold marker when inactive", async () => {
    await act(async () => {
      root.render(<WaveformEq active={false} />);
    });

    const eq = host.querySelector(".waveform-eq") as HTMLElement;
    expect(eq).not.toBeNull();
    expect(eq.dataset.active).toBe("false");
    expect(eq.getAttribute("aria-label")).toBe("Audio waveform idle");

    const bars = Array.from(host.querySelectorAll<HTMLElement>(".waveform-eq-bar"));
    expect(bars).toHaveLength(EQ_BAR_COUNT);
    for (const bar of bars) {
      expect(bar.style.transform).toBe("scaleY(0.12)");
    }
    expect(host.querySelector(".waveform-eq-peak")).toBeNull();
  });

  it("renders dB-normalized bar heights and peak-hold marker from RecordingLevels", async () => {
    const levels = {
      peak: 0.67,
      rms: 0.58,
      peakHold: 0.9,
      bars: [0.3, 0.35, 0.45, 0.55, 0.62, 0.67, 0.65, 0.58, 0.5, 0.42, 0.33, 0.02],
    };

    await act(async () => {
      root.render(<WaveformEq levels={levels} active />);
    });

    const eq = host.querySelector(".waveform-eq") as HTMLElement;
    expect(eq.dataset.active).toBe("true");
    expect(eq.dataset.peak).toBe("0.67");
    expect(eq.dataset.rms).toBe("0.58");
    expect(eq.dataset.peakHold).toBe("0.90");
    expect(eq.getAttribute("aria-label")).toBe("Audio waveform level 67 percent");

    const bars = Array.from(host.querySelectorAll<HTMLElement>(".waveform-eq-bar"));
    expect(bars).toHaveLength(EQ_BAR_COUNT);
    expect(bars[0].style.transform).toBe("scaleY(0.3)");
    expect(bars[5].style.transform).toBe("scaleY(0.67)");
    // Very quiet bar is floored to 0.08 so active bars remain subtly visible.
    expect(bars[11].style.transform).toBe("scaleY(0.08)");

    const peakMarker = host.querySelector(".waveform-eq-peak") as HTMLElement;
    expect(peakMarker).not.toBeNull();
    expect(peakMarker.dataset.peakHold).toBe("0.90");
    expect(peakMarker.style.bottom).toBe("90%");
  });
});
