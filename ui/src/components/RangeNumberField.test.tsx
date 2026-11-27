import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseFormNumber, RangeNumberField } from "./RangeNumberField";

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

describe("parseFormNumber", () => {
  it("treats an empty or non-numeric field as NaN, never as 0", () => {
    expect(Number.isNaN(parseFormNumber(""))).toBe(true);
    expect(Number.isNaN(parseFormNumber("   "))).toBe(true);
    expect(Number.isNaN(parseFormNumber("abc"))).toBe(true);
    expect(parseFormNumber(" 12 ")).toBe(12);
    expect(parseFormNumber("0")).toBe(0);
  });
});

describe("RangeNumberField", () => {
  it("shows the number and the slider together, and says so for an empty field", async () => {
    const onChange = vi.fn();
    const field = (value: string) => (
      <RangeNumberField
        id="idle"
        label="Wait for idle (seconds)"
        unit="s"
        min={0}
        max={3600}
        value={value}
        onChange={onChange}
      />
    );
    await act(async () => {
      root.render(field("45"));
    });
    const slider = host.querySelector<HTMLInputElement>('input[type="range"]')!;
    const number = host.querySelector<HTMLInputElement>('input[type="number"]')!;
    expect(slider.value).toBe("45");
    expect(number.value).toBe("45");
    expect(slider.getAttribute("aria-valuetext")).toBe("45 s");
    expect(host.querySelector(".range-number-unit")?.getAttribute("aria-hidden")).toBe("true");

    await act(async () => {
      root.render(field(""));
    });
    expect(slider.value).toBe("0");
    expect(slider.getAttribute("aria-valuetext")).toBe("no value");
    expect(number.value).toBe("");
  });
});
