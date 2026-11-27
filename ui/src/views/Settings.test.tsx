import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend } from "../ipc/mock";
import { Main } from "./Main";
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
  const input =
    (label?.htmlFor
      ? host.querySelector<HTMLInputElement>(`#${CSS.escape(label.htmlFor)}`)
      : null) ?? label?.querySelector("input");
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
    expect(host.textContent).toContain("Keep audio after processing");
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
    const backend = createMockBackend({
      clockRate: 0,
      pick: { file: "C:\\Audio\\marta-22s.wav" },
    });
    const clipSpy = vi.spyOn(backend, "voiceProfileSampleClip");
    const enrollSpy = vi.spyOn(backend, "enrollVoiceProfile");
    const pickFileSpy = vi.spyOn(backend, "pickFile");

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
    });
    // The path comes from the native file dialog (the mock answers with the preset path).
    await act(async () => {
      buttonByText("Browse…", enrollForm).click();
    });
    await flush();
    expect(pickFileSpy).toHaveBeenCalledWith(expect.objectContaining({ kind: "audio" }));
    expect(pathInput.value).toBe("C:\\Audio\\marta-22s.wav");

    await act(async () => {
      buttonByText("Enroll voice profile", enrollForm).click();
    });
    await flush();

    expect(enrollSpy).toHaveBeenCalledWith("Marta Enroll", "NOMAD", "C:\\Audio\\marta-22s.wav");
    expect(host.querySelector('ul[aria-label="Voice profiles"]')!.textContent).toContain(
      "Marta Enroll",
    );
  });

  it("persists vision model path and skip-screenshot toggle through ProcessingSettings", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");
    await mount(backend, "models");

    await act(async () => {
      setInputValue(inputByLabel("Vision model path (GGUF)"), "D:\\Models\\qwen3-vl-8b-q4.gguf");
      inputByLabel("Skip screenshot analysis").click();
    });
    await flush();

    await act(async () => {
      buttonByText("Save AI model settings").click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        visionModelPath: "D:\\Models\\qwen3-vl-8b-q4.gguf",
        skipScreenshotAnalysis: true,
      }),
    );

    // Switch away and back; settings reload from backend.
    await clickTab("Connectors");
    await clickTab("AI Models");
    expect(inputByLabel("Vision model path (GGUF)").value).toBe("D:\\Models\\qwen3-vl-8b-q4.gguf");
    expect(inputByLabel("Skip screenshot analysis").checked).toBe(true);
  });

  it("renders Connected project repositories in Preferences, pulls repos, and updates copy to Projects page", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const pullSpy = vi.spyOn(backend, "pullCycleRepo");
    await mount(backend, "preferences");

    expect(host.textContent).toContain("Connected project repositories");
    const repoList = host.querySelector('ul[aria-label="Project repositories"]')!;
    expect(repoList).not.toBeNull();
    expect(repoList.querySelectorAll("li").length).toBe(2);
    expect(repoList.textContent).toContain("State as of 2026-11-12");

    await act(async () => {
      host.querySelector<HTMLButtonElement>('button[aria-label="Pull team-cycles"]')!.click();
    });
    await flush();
    expect(pullSpy).toHaveBeenCalledWith("team-cycles");

    await clickTab("Connectors");
    expect(host.textContent).not.toContain("Cycles page");
    expect(host.textContent).toContain("Projects page");
  });

  it("guards main sidebar navigation when Settings has unsaved changes", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <Main />
        </BackendProvider>,
      );
    });
    await flush();

    const sidebar = host.querySelector('nav[aria-label="Main navigation"]')!;
    await act(async () => {
      buttonByText("Settings", sidebar).click();
    });
    await flush();

    await clickTab("Processing");
    await act(async () => {
      setInputValue(inputByLabel("Max GPU utilization (%)"), "52");
    });
    await flush();

    // Clicking Queue in main sidebar opens UnsavedDialog.
    await act(async () => {
      buttonByText("Queue", sidebar).click();
    });
    await flush();

    const dialog = host.querySelector('[role="dialog"]')!;
    expect(dialog).not.toBeNull();
    expect(dialog.textContent).toContain("Save your changes before leaving Settings?");

    // Cancel keeps user in Settings.
    await act(async () => {
      buttonByText("Cancel", dialog).click();
    });
    await flush();
    expect(host.querySelector("#settings-heading")).not.toBeNull();
    expect(inputByLabel("Max GPU utilization (%)").value).toBe("52");

    // Clicking Queue again and choosing Save persists and navigates to Queue.
    await act(async () => {
      buttonByText("Queue", sidebar).click();
    });
    await flush();
    const dialog2 = host.querySelector('[role="dialog"]')!;
    await act(async () => {
      buttonByText("Save", dialog2).click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledWith(expect.objectContaining({ gpuMaxUtilPercent: 52 }));
    expect(host.querySelector('ul[aria-label="Processing jobs"]')).not.toBeNull();
  });

  it("does not reveal duplicate Global rows when renaming or resetting a cycle voice profile", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.renameParticipant("NOMAD:jan", "Janek Demo");
    await backend.resetProfile("NOMAD:alex");

    const all = await backend.listAllVoiceProfiles();
    const names = all.map((p) => p.name);
    expect(names).not.toContain("Jan Demo");
    expect(names.filter((n) => n === "Alex Demo")).toHaveLength(1);
    const alex = all.find((p) => p.id === "NOMAD:alex")!;
    expect(alex.sampleCount).toBe(0);
    expect(alex.hasSampleClip).toBe(false);
    await expect(backend.voiceProfileSampleClip("NOMAD:alex")).rejects.toMatchObject({
      code: "not_found",
    });
  });
});

describe("Keep audio after processing (global setting)", () => {
  function keepCheckbox(): HTMLInputElement {
    const box = host.querySelector<HTMLInputElement>(
      'input[type="checkbox"][aria-label="Keep audio after processing"]',
    );
    if (!box) throw new Error("the checkbox is missing");
    return box;
  }

  it("is off by default, so the audio is deleted, and explains itself in a help popover", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend, "processing");

    expect(keepCheckbox().checked).toBe(false);
    expect(host.textContent).toContain("The audio is deleted once the meeting is processed");
    expect(host.textContent).not.toContain("Keep for N days");

    const help = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Help: Keep audio after processing"]',
    )!;
    expect(help).not.toBeNull();
    await act(async () => {
      help.click();
    });
    const text = host.querySelector(".help-popover-card")!.textContent!;
    expect(text).toContain("transcript");
    expect(text).toContain("voice sample clips");
    expect(text).toContain("report is ready");
    expect(text).toContain("several GB");
  });

  it("saves the choice as keep or delete", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");
    await mount(backend, "processing");

    await act(async () => {
      keepCheckbox().click();
    });
    await flush();
    expect(keepCheckbox().checked).toBe(true);
    expect(host.textContent).toContain("A 10 hour meeting takes several GB");
    await act(async () => {
      buttonByText("Save processing policy").click();
    });
    await flush();
    expect(saveSpy).toHaveBeenCalledTimes(1);
    expect(saveSpy.mock.calls[0][0].audioRetention).toEqual({ mode: "keep" });
    expect((await backend.getProcessingSettings()).audioRetention.mode).toBe("keep");

    await act(async () => {
      keepCheckbox().click();
    });
    await flush();
    await act(async () => {
      buttonByText("Save processing policy").click();
    });
    await flush();
    expect(saveSpy.mock.calls[1][0].audioRetention).toEqual({ mode: "delete" });
  });

  it("shows a value saved by an older version (keep for N days) as keeping the audio", async () => {
    const base = createMockBackend({ clockRate: 0 });
    const backend: Backend = {
      ...base,
      getProcessingSettings: async () => ({
        ...(await base.getProcessingSettings()),
        audioRetention: { mode: "keepDays" as const },
      }),
    };
    const saveSpy = vi.spyOn(backend, "saveProcessingSettings");
    await mount(backend, "processing");

    expect(keepCheckbox().checked).toBe(true);
    // Nothing changed, so nothing is offered for saving.
    expect(host.querySelector(".unsaved-bar")).toBeNull();

    // Another change saves the retired value as a plain "keep".
    await act(async () => {
      setInputValue(inputByLabel("Max GPU utilization (%)"), "55");
    });
    await flush();
    await act(async () => {
      buttonByText("Save processing policy").click();
    });
    await flush();
    expect(saveSpy.mock.calls[0][0].audioRetention).toEqual({ mode: "keep" });
  });

  it("shows removable project badges, + Add to project, duplicate hint, and merge modal in Voice profiles", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const p1 = await backend.enrollVoiceProfile("Dup Person", "NOMAD", "C:\\Audio\\d1.wav");
    await backend.enrollVoiceProfile("Dup Person", null, "C:\\Audio\\d2.wav");
    await mount(backend, "voices");

    expect(host.textContent).toContain("Possible duplicate name");
    const removeBtn = host.querySelector<HTMLButtonElement>(
      `button[aria-label="Remove NOMAD from ${p1.name}"]`,
    )!;
    expect(removeBtn).not.toBeNull();
    await act(async () => {
      removeBtn.click();
    });
    await flush();

    // After removing NOMAD, p1 falls back to Global and + Add to project has NOMAD.
    const addSelect = host.querySelector<HTMLSelectElement>(
      `select[aria-label="Add ${p1.name} to project"]`,
    )!;
    expect(addSelect).not.toBeNull();
    expect(Array.from(addSelect.options).map((o) => o.value)).toContain("NOMAD");

    // Merge selector opens confirmation modal.
    const mergeSelect = host.querySelector<HTMLSelectElement>(
      `select[aria-label="Same person as ${p1.name}"]`,
    )!;
    expect(mergeSelect).not.toBeNull();
    const targetOpt = Array.from(mergeSelect.options).find((o) =>
      o.textContent?.includes("Dup Person"),
    )!;
    await act(async () => {
      mergeSelect.value = targetOpt.value;
      mergeSelect.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await flush();
    expect(host.textContent).toContain("Merge these voice profiles?");
    await act(async () => {
      buttonByText("Merge profiles").click();
    });
    await flush();
    expect(host.textContent).not.toContain("Possible duplicate name");
  });
});

describe("Settings cards and compact Detection panel", () => {
  it("renders separate cards with topic headings in AI Models and Processing tabs", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend, "models");

    const modelCards = Array.from(host.querySelectorAll(".panel.quiet"));
    expect(modelCards.length).toBe(4);
    expect(host.querySelectorAll("fieldset.llm-group").length).toBe(0);
    const modelHeadings = modelCards.map((c) =>
      c.querySelector("h3.sub-head")?.textContent?.trim(),
    );
    expect(modelHeadings).toEqual(["Language model", "llama-server", "Vision", "Speech models"]);

    await clickTab("Processing");
    const procCards = Array.from(host.querySelectorAll(".panel.quiet"));
    expect(procCards.length).toBe(5);
    expect(host.querySelectorAll("fieldset.llm-group").length).toBe(0);
    const procHeadings = procCards.map((c) => c.querySelector("h3.sub-head")?.textContent?.trim());
    expect(procHeadings).toEqual([
      "Processing & Hardware Policy",
      "GPU guard thresholds",
      "Audio after processing",
      "Voice recognition thresholds",
      "Worker Python environment",
    ]);
  });

  it("uses Turn off wording and switches app state without Never in Detection", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveDetectionSettings");
    await mount(backend, "preferences");

    const section = host.querySelector('section[aria-labelledby="detection-heading"]')!;
    expect(section).not.toBeNull();
    const list = section.querySelector('ul[aria-label="Watched applications"]')!;
    expect(list.textContent).not.toContain("Never ask");

    const turnOffZoom = section.querySelector<HTMLButtonElement>(
      'button[aria-label="Turn off Zoom"]',
    )!;
    expect(turnOffZoom).not.toBeNull();
    expect(turnOffZoom.textContent?.trim()).toBe("Turn off");

    await act(async () => {
      turnOffZoom.click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledWith(
      expect.objectContaining({ optedOut: expect.arrayContaining(["zoom"]) }),
    );
    expect(list.textContent).toContain("switched off");
    expect(list.textContent).not.toContain("switched off with Never");
    expect(section.querySelector('[data-testid="detection-state"]')?.textContent).toBe(
      "On, watching 6 applications.",
    );

    const askAgainZoom = section.querySelector<HTMLButtonElement>(
      'button[aria-label="Ask again for Zoom"]',
    )!;
    await act(async () => {
      askAgainZoom.click();
    });
    await flush();
    expect(list.textContent).not.toContain("switched off");
  });

  it("renders sensitivity presets as a segmented control and updates aria-pressed on click", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const saveSpy = vi.spyOn(backend, "saveDetectionSettings");
    await mount(backend, "preferences");

    const group = host.querySelector('[role="group"][aria-label="Detection sensitivity"]')!;
    expect(group).not.toBeNull();
    const buttons = Array.from(group.querySelectorAll<HTMLButtonElement>("button"));
    expect(buttons.map((b) => b.textContent?.trim())).toEqual(["Low", "Balanced", "High"]);
    expect(buttons[0].getAttribute("aria-pressed")).toBe("false");
    expect(buttons[1].getAttribute("aria-pressed")).toBe("true");
    expect(buttons[2].getAttribute("aria-pressed")).toBe("false");

    await act(async () => {
      buttons[2].click();
    });
    await flush();

    expect(saveSpy).toHaveBeenCalledWith(expect.objectContaining({ sensitivity: "high" }));
    expect(buttons[1].getAttribute("aria-pressed")).toBe("false");
    expect(buttons[2].getAttribute("aria-pressed")).toBe("true");
  });

  it("renders ready, older, and missing model states with Update and Download actions", async () => {
    const base = createMockBackend({ clockRate: 0 });
    const downloadSpy = vi.fn().mockResolvedValue(undefined);
    const backend: Backend = {
      ...base,
      modelStatus: async () => [
        {
          key: "whisper-turbo",
          label: "Whisper turbo (speech recognition)",
          cached: true,
          status: "ready",
          gated: false,
          sizeMb: 1600,
        },
        {
          key: "vision",
          label: "Screenshot descriptions (vision model)",
          cached: false,
          status: "older",
          gated: false,
          sizeMb: 4800,
        },
        {
          key: "diarization",
          label: "Speaker separation (pyannote)",
          cached: false,
          status: "missing",
          gated: true,
          sizeMb: 1200,
        },
      ],
      downloadModels: downloadSpy,
    };

    await mount(backend, "models");

    const items = Array.from(host.querySelectorAll<HTMLLIElement>('ul[aria-label="Models"] li'));
    expect(items).toHaveLength(3);

    expect(items[0].textContent).toContain("Downloaded");
    expect(items[0].querySelector("button")).toBeNull();

    expect(items[1].textContent).toContain("Downloaded (older version, update?)");
    const updateBtn = items[1].querySelector<HTMLButtonElement>(
      'button[aria-label="Update Screenshot descriptions (vision model)"]',
    );
    expect(updateBtn).not.toBeNull();
    expect(updateBtn!.textContent?.trim()).toBe("Update");

    const downloadBtn = items[2].querySelector<HTMLButtonElement>(
      'button[aria-label="Download Speaker separation (pyannote)"]',
    );
    expect(downloadBtn).not.toBeNull();
    expect(downloadBtn!.textContent?.trim()).toBe("Download");

    await act(async () => {
      updateBtn!.click();
    });
    await flush();
    expect(downloadSpy).toHaveBeenCalledWith(["vision"]);
  });
});
