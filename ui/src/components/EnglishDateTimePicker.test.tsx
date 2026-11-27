import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EnglishDateTimePicker } from "./EnglishDateTimePicker";

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

async function choose(label: string, value: string) {
  const select = host.querySelector<HTMLSelectElement>(`select[aria-label="${label}"]`)!;
  const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(select, value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

async function render(value: string, onChange: (next: string) => void) {
  await act(async () => {
    root.render(<EnglishDateTimePicker ariaLabel="Run at" value={value} onChange={onChange} />);
  });
}

describe("EnglishDateTimePicker", () => {
  it("clamps the day when the month changes to a shorter one (leap and common years)", async () => {
    const onChange = vi.fn();
    await render("2026-01-31T10:00", onChange);
    await choose("Month", "02");
    expect(onChange).toHaveBeenLastCalledWith("2026-02-28T10:00");

    await render("2028-01-31T10:00", onChange);
    await choose("Month", "02");
    expect(onChange).toHaveBeenLastCalledWith("2028-02-29T10:00");

    await render("2026-03-31T10:00", onChange);
    await choose("Month", "04");
    expect(onChange).toHaveBeenLastCalledWith("2026-04-30T10:00");
  });

  it("keeps the date when a placeholder is chosen, and ignores a value that is not a date", async () => {
    const onChange = vi.fn();
    await render("2026-06-15T08:30", onChange);
    const placeholder = host.querySelector<HTMLOptionElement>(
      'select[aria-label="Hour"] option[value=""]',
    )!;
    expect(placeholder.disabled).toBe(true);

    await choose("Hour", "");
    expect(onChange).not.toHaveBeenCalled();

    await render("15/06/2026", onChange);
    expect(host.querySelector<HTMLSelectElement>('select[aria-label="Year"]')!.value).toBe("");
  });
});
