import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Modal } from "./Modal";

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

function dialogOf(): HTMLElement {
  return host.querySelector('[role="dialog"]') as HTMLElement;
}

describe("Modal", () => {
  it("moves focus into the dialog and gives it back to the opener when closed", async () => {
    const opener = document.createElement("button");
    opener.textContent = "Open";
    document.body.append(opener);
    opener.focus();

    await act(async () => {
      root.render(
        <Modal title="Confirm" onClose={() => {}}>
          <button type="button">Yes</button>
        </Modal>,
      );
    });
    expect(document.activeElement?.textContent).toBe("Yes");

    await act(async () => {
      root.render(<div />);
    });
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("closes on Escape and names the dialog by its heading", async () => {
    const onClose = vi.fn();
    await act(async () => {
      root.render(
        <Modal title="Force process" onClose={onClose}>
          <p>Body</p>
        </Modal>,
      );
    });
    const heading = host.querySelector("h3")!;
    expect(heading.id).toBe(dialogOf().getAttribute("aria-labelledby"));

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps Tab inside the dialog, wrapping from the last control to the first", async () => {
    await act(async () => {
      root.render(
        <Modal title="Edit" onClose={() => {}}>
          <input aria-label="First" />
          <button type="button">Last</button>
        </Modal>,
      );
    });
    const first = host.querySelector<HTMLInputElement>("input")!;
    const last = host.querySelector<HTMLButtonElement>("button")!;
    last.focus();

    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    await act(async () => {
      dialogOf().dispatchEvent(tab);
    });
    expect(tab.defaultPrevented).toBe(true);
    // The scrollable content is the first stop of the dialog, then the controls.
    expect(document.activeElement).toBe(host.querySelector(".modal-content"));
    expect(first).not.toBeNull();
  });

  it("makes the scrollable content reachable by keyboard and keeps the footer", async () => {
    await act(async () => {
      root.render(
        <Modal title="Long" onClose={() => {}} footer={<button type="button">Save</button>}>
          <p>Long text</p>
        </Modal>,
      );
    });
    const content = host.querySelector<HTMLElement>(".modal-content")!;
    expect(content.tabIndex).toBe(0);
    expect(content.getAttribute("aria-label")).toBe("Long content");
    expect(host.querySelector(".modal-footer")?.textContent).toContain("Save");
  });
});
