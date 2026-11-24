import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend } from "../ipc/mock";
import { SettingsView, type SettingsTab } from "./Settings";

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

async function mount(backend: Backend, initialTab?: SettingsTab) {
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

function setSelectValue(select: HTMLSelectElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!;
  setter.call(select, value);
  select.dispatchEvent(new Event("change", { bubbles: true }));
}

async function clickTab(label: string) {
  const tabs = Array.from(host.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
  const target = tabs.find((b) => b.textContent?.trim() === label);
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

function inputByLabel(labelPrefix: string, container: ParentNode = host): HTMLInputElement {
  const label = Array.from(container.querySelectorAll<HTMLLabelElement>("label")).find((l) =>
    l.textContent?.trim().startsWith(labelPrefix),
  );
  const input = label?.querySelector("input");
  if (!input) throw new Error(`Input not found for label: ${labelPrefix}`);
  return input;
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

describe("SettingsView", () => {
  it("switches across all 5 tabs and renders each section cleanly", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);

    expect(host.querySelector(".settings-shell")).not.toBeNull();
    expect(host.querySelectorAll('[role="tab"]').length).toBe(5);
    expect(host.textContent).toContain("Auto-detect models on disk");

    await clickTab("Connectors");
    expect(host.textContent).toContain("GitHub Token");
    expect(host.textContent).toContain("Hugging Face Token");
    expect(host.textContent).toContain("External / Cloud LLM API Keys");
    expect(host.textContent).toContain("no global Discord webhook here");
    expect(host.querySelector('input[placeholder*="discord.com/api/webhooks"]')).toBeNull();

    await clickTab("Preferences");
    expect(host.textContent).toContain(
      "Automatically save meeting notes & transcripts to a local folder",
    );
    expect(host.textContent).toContain("When to publish");
    expect(host.textContent).toContain("Detection");
    expect(host.textContent).toContain("Browser extension");

    await clickTab("Processing");
    expect(host.textContent).toContain("GPU guard thresholds");
    expect(host.textContent).toContain("Audio retention policy");
    expect(host.textContent).toContain("Worker Python environment");

    await clickTab("Voice Profiles");
    expect(host.textContent).toContain("Add voice profile manually");
    expect(host.textContent).toContain("Voice profiles");
  });

  it("opens and closes HelpPopover when clicking ? buttons", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend, "models");

    const helpBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Help: Context size"]',
    );
    expect(helpBtn).not.toBeNull();
    expect(helpBtn!.getAttribute("aria-expanded")).toBe("false");

    await act(async () => {
      helpBtn!.click();
    });
    expect(helpBtn!.getAttribute("aria-expanded")).toBe("true");
    const card = host.querySelector(".help-popover-card");
    expect(card).not.toBeNull();
    expect(card!.textContent).toContain("16384");

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(host.querySelector(".help-popover-card")).toBeNull();
  });

  it("prompts UnsavedDialog when leaving a dirty tab and supports Discard and Save", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");
    await mount(backend, "processing");

    const utilInput = inputByLabel("Max GPU utilization (%)");
    await act(async () => {
      setInputValue(utilInput, "55");
    });
    await flush();

    // UnsavedBar is visible before navigating away.
    expect(host.querySelector(".unsaved-bar")).not.toBeNull();

    // Attempting to switch tabs opens UnsavedDialog.
    await clickTab("Connectors");
    const dialog = host.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.textContent).toContain("You have unsaved changes");

    // Cancel keeps us on Processing with our edit intact.
    await act(async () => {
      buttonByText("Cancel", dialog!).click();
    });
    expect(host.querySelector('[role="dialog"]')).toBeNull();
    expect(inputByLabel("Max GPU utilization (%)").value).toBe("55");

    // Discard reverts changes and switches to Connectors.
    await clickTab("Connectors");
    const discardDialog = host.querySelector('[role="dialog"]')!;
    await act(async () => {
      buttonByText("Discard", discardDialog).click();
    });
    await flush();
    expect(host.textContent).toContain("GitHub Token");
    expect(saveSpy).not.toHaveBeenCalled();

    // Switch back to Processing, edit again, and Save via UnsavedDialog.
    await clickTab("Processing");
    await act(async () => {
      setInputValue(inputByLabel("Max GPU utilization (%)"), "65");
    });
    await flush();

    await clickTab("Voice Profiles");
    const saveDialog = host.querySelector('[role="dialog"]')!;
    await act(async () => {
      buttonByText("Save", saveDialog).click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledTimes(1);
    expect(saveSpy.mock.calls[0][0].gpuMaxUtilPercent).toBe(65);
    expect(host.textContent).toContain("Add voice profile manually");
  });

  it("auto-detects models on disk and populates llama-server, LLM GGUF, and vision GGUF paths", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const detectSpy = vi.spyOn(backend, "autoDetectModels").mockResolvedValue({
      llamaServerPath: "E:\\Bin\\llama-server.exe",
      llmModels: ["E:\\Models\\detected-llm.gguf"],
      visionModels: ["E:\\Models\\detected-vision.gguf"],
      defaultLocalSaveDir: "Documents\\BlahBlahBot\\notes",
    });

    await mount(backend, "models");
    await act(async () => {
      buttonByText("Auto-detect models on disk").click();
    });
    await flush();

    expect(detectSpy).toHaveBeenCalledTimes(1);
    expect(inputByLabel("llama-server executable").value).toBe("E:\\Bin\\llama-server.exe");
    expect(inputByLabel("Default language model (GGUF)").value).toBe(
      "E:\\Models\\detected-llm.gguf",
    );
    expect(inputByLabel("Vision model path (GGUF)").value).toBe("E:\\Models\\detected-vision.gguf");
  });

  it("persists Local Folder Auto-Save checkbox and pre-populated folder path in Preferences", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveSettings");
    await mount(backend, "preferences");

    const dirInput = inputByLabel("Local notes folder");
    expect(dirInput.value).toBe("Documents\\BlahBlahBot\\notes");

    const checkbox = inputByLabel(
      "Automatically save meeting notes & transcripts to a local folder",
    );
    expect(checkbox.checked).toBe(false);

    await act(async () => {
      checkbox.click();
    });
    await flush();
    expect(checkbox.checked).toBe(true);

    await act(async () => {
      buttonByText("Save preferences").click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledTimes(1);
    expect(saveSpy.mock.calls[0][0].localSaveEnabled).toBe(true);
    expect(saveSpy.mock.calls[0][0].localSaveDir).toBe("Documents\\BlahBlahBot\\notes");

    const persisted = await backend.getSettings();
    expect(persisted.localSaveEnabled).toBe(true);
    expect(persisted.localSaveDir).toBe("Documents\\BlahBlahBot\\notes");
  });

  it("displays VoiceProfiles project badges, plays/stops WAV sample clips, and enrolls a new profile", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const clipSpy = vi.spyOn(backend, "voiceProfileSampleClip");
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");

    const createObjectURL = vi.fn(() => "blob:voice-sample-1");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL,
      revokeObjectURL,
    });

    const playMock = vi.fn().mockResolvedValue(undefined);
    const pauseMock = vi.fn();
    vi.stubGlobal(
      "Audio",
      vi.fn().mockImplementation(function (this: HTMLAudioElement) {
        return {
          play: playMock,
          pause: pauseMock,
          onended: null,
          onerror: null,
        } as unknown as HTMLAudioElement;
      }),
    );

    await mount(backend, "voices");

    // Project membership badges shown for mock profiles.
    const list = host.querySelector('ul[aria-label="Voice profiles"]')!;
    expect(list.textContent).toContain("Alex Demo");
    expect(list.textContent).toContain("NOMAD");

    // Play sample clip and then stop (revoking object URL cleanly).
    const playBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Play sample for Alex Demo"]',
    )!;
    await act(async () => {
      playBtn.click();
    });
    await flush();

    expect(clipSpy).toHaveBeenCalledTimes(1);
    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(playMock).toHaveBeenCalledTimes(1);

    const stopBtn = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Stop sample for Alex Demo"]',
    )!;
    expect(stopBtn.textContent).toBe("Stop");

    await act(async () => {
      stopBtn.click();
    });
    await flush();
    expect(pauseMock).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:voice-sample-1");

    // Manual 15-30 s enrollment.
    const enrollForm = host.querySelector<HTMLFormElement>(
      'form[aria-label="Add voice profile manually"]',
    )!;
    const nameInput = enrollForm.querySelector<HTMLInputElement>(
      'input[aria-label="Person name"]',
    )!;
    const projectSelect = enrollForm.querySelector<HTMLSelectElement>(
      'select[aria-label="Project"]',
    )!;
    const pathInput = enrollForm.querySelector<HTMLInputElement>(
      'input[aria-label="15–30 s audio/video sample path"]',
    )!;

    await act(async () => {
      setInputValue(nameInput, "Marta Enroll");
      setSelectValue(projectSelect, "NOMAD");
      setInputValue(pathInput, "C:\\Audio\\marta-22s.wav");
    });
    await flush();

    await act(async () => {
      buttonByText("Enroll voice profile", enrollForm).click();
    });
    await flush();

    expect(enrollSpy).toHaveBeenCalledWith("Marta Enroll", "NOMAD", "C:\\Audio\\marta-22s.wav");
    expect(host.querySelector('ul[aria-label="Voice profiles"]')!.textContent).toContain(
      "Marta Enroll",
    );
  });
});
