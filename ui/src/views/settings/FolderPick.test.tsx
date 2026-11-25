import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../backend-context";
import { createMockBackend, type MockOptions } from "../../ipc/mock";
import { SettingsView, type SettingsTab } from "../Settings";

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
  vi.restoreAllMocks();
});

async function flush(ticks = 6) {
  for (let i = 0; i < ticks; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function mount(tab: SettingsTab, opts: MockOptions) {
  const backend = createMockBackend({ clockRate: 0, ...opts });
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <SettingsView initialTab={tab} />
      </BackendProvider>,
    );
  });
  await flush();
  return backend;
}

function input(label: string): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
  if (!el) throw new Error(`Input not found: ${label}`);
  return el;
}

async function browse(label: string) {
  const btn = host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  if (!btn) throw new Error(`Browse button not found: ${label}`);
  await act(async () => {
    btn.click();
  });
  await flush();
}

describe("Settings: folder dialogs", () => {
  it.each([
    ["models", "Browse for the models folder", "Models folder"],
    ["preferences", "Browse for the local notes folder", "Local notes folder"],
    ["preferences", "Browse for the recordings folder", "Recordings folder"],
    ["processing", "Browse for the worker folder", "Worker folder"],
  ] as const)("%s tab: %s fills the field with the chosen folder", async (tab, button, field) => {
    const backend = await mount(tab, { pick: { folder: "D:\\Picked\\folder" } });
    const folderSpy = vi.spyOn(backend, "pickFolder");
    await browse(button);
    expect(folderSpy).toHaveBeenCalledTimes(1);
    expect(input(field).value).toBe("D:\\Picked\\folder");
    // The change is an edit like any other: the unsaved bar offers to keep or drop it.
    expect(host.querySelector(".unsaved-bar")).not.toBeNull();
  });

  it("a cancelled folder dialog leaves the field and the saved state alone", async () => {
    await mount("preferences", { pick: { folder: null } });
    const before = input("Recordings folder").value;
    await browse("Browse for the recordings folder");
    expect(input("Recordings folder").value).toBe(before);
    expect(host.querySelector(".unsaved-bar")).toBeNull();
    expect(host.querySelector('[role="alert"]')).toBeNull();
  });

  it("a folder dialog that fails shows the message next to the button", async () => {
    const backend = await mount("preferences", {});
    vi.spyOn(backend, "pickFolder").mockRejectedValue(
      new Error("The file dialog did not return a path. Type the path instead."),
    );
    const before = input("Recordings folder").value;
    await browse("Browse for the recordings folder");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("did not return a path");
    expect(input("Recordings folder").value).toBe(before);
  });
});

describe("Voice Profiles: sample dialog", () => {
  it("a cancelled dialog leaves the path empty and nothing is enrolled", async () => {
    const backend = await mount("voices", { pick: { file: null } });
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");
    await browse("Browse for the voice sample");
    const path = host.querySelector<HTMLInputElement>(
      'input[aria-label="15–30 s audio/video sample path"]',
    )!;
    expect(path.value).toBe("");
    expect(enrollSpy).not.toHaveBeenCalled();
  });
});
