import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../backend-context";
import type { Backend } from "../../ipc/backend";
import { createMockBackend } from "../../ipc/mock";
import { SettingsView, type SettingsTab } from "../Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

async function flush(ticks = 6) {
  for (let i = 0; i < ticks; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function mount(backend: Backend, initialTab: SettingsTab) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <SettingsView initialTab={initialTab} />
      </BackendProvider>,
    );
  });
  await flush();
}

function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    setInputValue(input, value);
  });
  await flush();
}

async function clickTab(label: string) {
  const target = Array.from(host.querySelectorAll<HTMLButtonElement>('[role="tab"]')).find(
    (b) => b.textContent?.trim() === label,
  );
  if (!target) throw new Error(`Tab not found: ${label}`);
  await act(async () => {
    target.click();
  });
  await flush();
}

function buttonByText(text: string, container: ParentNode = host): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent?.trim() === text,
  );
  if (!found) throw new Error(`Button not found: ${text}`);
  return found;
}

function inputByAria(label: string): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
  if (!el) throw new Error(`Input not found: ${label}`);
  return el;
}

function secretInput(formLabel: string): HTMLInputElement {
  const form = host.querySelector(`form[aria-label="${formLabel}"]`);
  const input = form?.querySelector<HTMLInputElement>('input[type="password"]');
  if (!input) throw new Error(`Secret field not found: ${formLabel}`);
  return input;
}

function dialog(): Element | null {
  return host.querySelector('[role="dialog"]');
}

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

describe("Connectors tab unsaved-change guard", () => {
  it("asks before leaving with a typed but unstored token, and Save stores it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const hfSpy = vi.spyOn(backend, "setHfToken");
    await mount(backend, "connectors");

    expect(host.querySelector('[data-testid="secret-hint"]')!.textContent).toContain(
      "Saved immediately",
    );

    await type(secretInput("Hugging Face Token"), "hf_exampletoken");
    expect(host.textContent).toContain("Not stored yet");

    await clickTab("Preferences");
    const dlg = dialog();
    expect(dlg).not.toBeNull();
    expect(host.querySelector("#settings-tab-connectors")!.getAttribute("aria-selected")).toBe(
      "true",
    );

    await act(async () => {
      buttonByText("Save", dlg!).click();
    });
    await flush();

    expect(hfSpy).toHaveBeenCalledWith("hf_exampletoken");
    expect(dialog()).toBeNull();
    expect(host.querySelector("#settings-tab-preferences")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("Discard drops the typed token without storing it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const hfSpy = vi.spyOn(backend, "setHfToken");
    await mount(backend, "connectors");

    await type(secretInput("Hugging Face Token"), "hf_exampletoken");
    await clickTab("Processing");
    await act(async () => {
      buttonByText("Discard", dialog()!).click();
    });
    await flush();

    expect(hfSpy).not.toHaveBeenCalled();
    expect(dialog()).toBeNull();
    expect(host.querySelector("#settings-tab-processing")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("stores a token with its own button and then leaves without a dialog", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const hfSpy = vi.spyOn(backend, "setHfToken");
    await mount(backend, "connectors");

    await type(secretInput("Hugging Face Token"), "hf_exampletoken");
    const form = host.querySelector('form[aria-label="Hugging Face Token"]')!;
    await act(async () => {
      buttonByText("Store", form).click();
    });
    await flush();
    expect(hfSpy).toHaveBeenCalledTimes(1);

    await clickTab("Preferences");
    expect(dialog()).toBeNull();
    expect(host.querySelector("#settings-tab-preferences")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("guards an edited endpoint field and saves it through the dialog", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");
    await mount(backend, "connectors");

    const hostInputs = Array.from(
      host.querySelectorAll<HTMLInputElement>('input[placeholder="http://127.0.0.1:11434/v1"]'),
    );
    expect(hostInputs.length).toBe(1);
    await type(hostInputs[0], "http://127.0.0.1:9999/v1");
    expect(host.querySelector('[data-testid="external-endpoint-state"]')!.textContent).toContain(
      "Unsaved",
    );

    await clickTab("Processing");
    expect(dialog()).not.toBeNull();
    await act(async () => {
      buttonByText("Save", dialog()!).click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledTimes(1);
    expect(saveSpy.mock.calls[0][0].llm?.external.baseUrl).toBe("http://127.0.0.1:9999/v1");
    expect(dialog()).toBeNull();
  });
});

describe("Voice Profiles unsaved-change guard", () => {
  it("asks before leaving with an un-enrolled entry, and enrolls on Save", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");
    await mount(backend, "voices");

    expect(host.querySelector('[data-testid="enroll-hint"]')!.textContent).toContain(
      "saves immediately",
    );

    await type(inputByAria("Person name"), "Sam Example");
    await type(inputByAria("15–30 s audio/video sample path"), "C:\\Audio\\sam-20s.wav");
    expect(host.querySelector('[data-testid="enroll-hint"]')!.textContent).toContain(
      "Not enrolled yet",
    );

    await clickTab("Connectors");
    expect(dialog()).not.toBeNull();
    await act(async () => {
      buttonByText("Save", dialog()!).click();
    });
    await flush();

    expect(enrollSpy).toHaveBeenCalledTimes(1);
    expect(dialog()).toBeNull();
    expect(host.querySelector("#settings-tab-connectors")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("a half-filled entry cannot be saved through the dialog, Discard leaves", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");
    await mount(backend, "voices");

    await type(inputByAria("Person name"), "Sam Example");
    await clickTab("Connectors");
    await act(async () => {
      buttonByText("Save", dialog()!).click();
    });
    await flush();
    expect(enrollSpy).not.toHaveBeenCalled();
    expect(host.querySelector("#settings-tab-voices")!.getAttribute("aria-selected")).toBe("true");

    await clickTab("Connectors");
    await act(async () => {
      buttonByText("Discard", dialog()!).click();
    });
    await flush();
    expect(host.querySelector("#settings-tab-connectors")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("enrolling with its own button leaves no unsaved state", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");
    await mount(backend, "voices");

    await type(inputByAria("Person name"), "Sam Example");
    await type(inputByAria("15–30 s audio/video sample path"), "C:\\Audio\\sam-20s.wav");
    await act(async () => {
      buttonByText("Enroll voice profile").click();
    });
    await flush();
    expect(enrollSpy).toHaveBeenCalledTimes(1);

    await clickTab("Connectors");
    expect(dialog()).toBeNull();
    expect(host.querySelector("#settings-tab-connectors")!.getAttribute("aria-selected")).toBe(
      "true",
    );
  });
});
