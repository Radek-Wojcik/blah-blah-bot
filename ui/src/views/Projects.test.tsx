import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend } from "../ipc/mock";
import { MeetingDetailView } from "./MeetingDetail";
import { ProjectDetailView } from "./ProjectDetail";
import { ProjectsView } from "./Projects";

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

async function flush(rounds = 6) {
  for (let i = 0; i < rounds; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function setInputValue(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

async function setSelectValue(el: HTMLSelectElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

function findButton(text: string, container: ParentNode = host): HTMLButtonElement {
  const btn = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent?.trim() === text || b.textContent?.includes(text),
  );
  if (!btn) throw new Error(`Button not found: ${text}`);
  return btn;
}

describe("ProjectsView and ProjectWizard", () => {
  it("renders project cards with briefing summaries and hides standalone section when all meetings belong to projects", async () => {
    const base = createMockBackend({ clockRate: 0 });
    const backend: Backend = {
      ...base,
      listMeetings: async () => {
        const list = await base.listMeetings();
        return list.map((m) => ({ ...m, cycleId: "NOMAD" }));
      },
    };
    const onOpenProject = vi.fn();
    const onOpenMeeting = vi.fn();

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectsView onOpenProject={onOpenProject} onOpenMeeting={onOpenMeeting} />
        </BackendProvider>,
      );
    });
    await flush();

    expect(host.textContent).toContain("Nomad rover");
    expect(host.textContent).toContain("NOMAD");
    expect(host.textContent).toContain("4 participants");
    expect(host.textContent).toContain("7 open tasks, 2 overdue");
    expect(host.textContent).toContain("Updated 2026-11-12");
    expect(host.querySelector('section[aria-label="Unassigned / standalone meetings"]')).toBeNull();

    const nomadCard = host.querySelector('article[aria-label="Project Nomad rover"]')!;
    await act(async () => {
      findButton("Open project", nomadCard).click();
    });
    expect(onOpenProject).toHaveBeenCalledWith("NOMAD");
  });

  it("shows unassigned / standalone meetings section only when any meeting has cycleId === null", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const onOpenMeeting = vi.fn();

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectsView onOpenProject={vi.fn()} onOpenMeeting={onOpenMeeting} />
        </BackendProvider>,
      );
    });
    await flush();

    const standalone = host.querySelector(
      'section[aria-label="Unassigned / standalone meetings"]',
    ) as HTMLElement;
    expect(standalone).not.toBeNull();
    expect(standalone.textContent).toContain("Chamber valve ripple filter");

    await act(async () => {
      findButton("Open", standalone).click();
    });
    expect(onOpenMeeting).toHaveBeenCalled();
  });

  it("creates a new project via ProjectWizard and opens it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const wizardSpy = vi.spyOn(backend, "wizardCreateProject");
    const onOpenProject = vi.fn();

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectsView onOpenProject={onOpenProject} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    await act(async () => {
      findButton("New Project").click();
    });
    await flush();

    expect(host.querySelectorAll(".wizard-step").length).toBe(2);
    const nameInput = host.querySelector<HTMLInputElement>('input[aria-label="Project name"]')!;
    const idInput = host.querySelector<HTMLInputElement>('input[aria-label="Project ID"]')!;
    await setInputValue(nameInput, "Lunar Scout");
    expect(idInput.value).toBe("LUNAR-SCOUT");

    await act(async () => {
      findButton("Next").click();
    });
    await flush();

    await setSelectValue(
      host.querySelector<HTMLSelectElement>('select[aria-label="Report language"]')!,
      "en",
    );
    await setSelectValue(
      host.querySelector<HTMLSelectElement>('select[aria-label="Default report mode"]')!,
      "template",
    );
    await setInputValue(
      host.querySelector<HTMLInputElement>('input[aria-label="GitHub repository"]')!,
      "demo-org/lunar-scout",
    );
    await setInputValue(
      host.querySelector<HTMLInputElement>('input[aria-label="Discord webhook reference"]')!,
      "cred:discord/lunar",
    );

    const localSaveCheckbox = host.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    await act(async () => {
      localSaveCheckbox.click();
    });
    await flush();

    await act(async () => {
      findButton("Create project").click();
    });
    await flush();

    expect(wizardSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        mode: "auto",
        id: "LUNAR-SCOUT",
        name: "Lunar Scout",
        outputLanguage: "en",
        defaultReportMode: "template",
        githubRepo: "demo-org/lunar-scout",
        discordWebhookRef: "cred:discord/lunar",
        localSaveEnabled: true,
      }),
    );
    expect(onOpenProject).toHaveBeenCalledWith("LUNAR-SCOUT");
  });
});

describe("ProjectDetailView, ProjectSettingsModal, and ImportMeetingModal", () => {
  it("renders top-5 urgent tasks with collapsible overflow, briefing refresh, and safe GitHub links", async () => {
    const base = createMockBackend({ clockRate: 0 });
    const backend: Backend = {
      ...base,
      listMeetings: async () => {
        const list = await base.listMeetings();
        return list.map((m) =>
          m.id === "m4" ? { ...m, cycleId: "NOMAD", reportUrl: "javascript:alert(1)" } : m,
        );
      },
    };
    const briefingSpy = vi.spyOn(backend, "getBriefing");
    const onOpenMeeting = vi.fn();

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={onOpenMeeting} />
        </BackendProvider>,
      );
    });
    await flush();

    expect(host.querySelector(".project-top-grid")).not.toBeNull();
    expect(host.textContent).toContain("What to discuss next");
    expect(host.textContent).toContain("Action items & Tasks");
    expect(host.textContent).toContain("Agree on the date of the first field test");

    // 7 open tasks in NOMAD mock: top 5 shown directly, remaining 2 inside <details>.
    const topTaskRows = host.querySelectorAll('ul[aria-label="Top action items"] > li');
    expect(topTaskRows.length).toBe(5);
    const overflowDetails = host.querySelector(
      'section[aria-label="Action items and tasks"] details',
    );
    expect(overflowDetails).not.toBeNull();
    expect(overflowDetails?.textContent).toContain("Show 2 more tasks");

    // Refresh button calls getBriefing(cycleId, true).
    briefingSpy.mockClear();
    await act(async () => {
      findButton("Refresh").click();
    });
    await flush();
    expect(briefingSpy).toHaveBeenCalledWith("NOMAD", true);

    // Safe GitHub link for m5 is rendered; unsafe javascript: URL on m4 is not rendered as anchor.
    const links = Array.from(
      host.querySelectorAll<HTMLAnchorElement>('ul[aria-label="Meetings list"] a'),
    );
    expect(links.length).toBe(1);
    expect(links[0].href).toContain("https://github.com/demo-org/nomad-cycles");

    await act(async () => {
      findButton("Open").click();
    });
    expect(onOpenMeeting).toHaveBeenCalled();
  });

  it("opens ProjectSettingsModal and saves machine-local preferences and cycle settings", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const savePrefsSpy = vi.spyOn(backend, "saveProjectPrefs");
    const saveCycleSpy = vi.spyOn(backend, "saveCycle");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    const gearBtn = host.querySelector<HTMLButtonElement>('button[aria-label="Project settings"]')!;
    await act(async () => {
      gearBtn.click();
    });
    await flush();

    // Toggle HelpPopover on local auto-save.
    const helpBtn = host.querySelector<HTMLButtonElement>("button.help-popover-btn")!;
    await act(async () => {
      helpBtn.click();
    });
    expect(host.querySelector(".help-popover-card")).not.toBeNull();

    // Enable local save & override LLM backend.
    const autoExportCheck = host.querySelector<HTMLInputElement>(
      'form[aria-label="Machine-local project preferences"] input[type="checkbox"]',
    )!;
    await act(async () => {
      autoExportCheck.click();
    });
    await setSelectValue(
      host.querySelector<HTMLSelectElement>('select[aria-label="Model backend override"]')!,
      "external",
    );
    await setInputValue(
      host.querySelector<HTMLInputElement>('input[aria-label="Model override value"]')!,
      "qwen2.5-14b",
    );

    await act(async () => {
      findButton("Save local preferences").click();
    });
    await flush();

    expect(savePrefsSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        cycleId: "NOMAD",
        localSaveEnabled: true,
        llmKindOverride: "external",
        llmModelOverride: "qwen2.5-14b",
      }),
    );

    // Switch to People & Glossary tab and update project name.
    await act(async () => {
      findButton("People & Glossary").click();
    });
    await flush();

    await setInputValue(
      host.querySelector<HTMLInputElement>('input[aria-label="Project name"]')!,
      "Nomad Rover Mk2",
    );
    await act(async () => {
      findButton("Save project settings").click();
    });
    await flush();

    expect(saveCycleSpy).toHaveBeenCalledWith(
      expect.objectContaining({ id: "NOMAD", name: "Nomad Rover Mk2" }),
    );
  });

  it("lets a project override the global audio retention and saves the choice", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const savePrefsSpy = vi.spyOn(backend, "saveProjectPrefs");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();
    await act(async () => {
      host.querySelector<HTMLButtonElement>('button[aria-label="Project settings"]')!.click();
    });
    await flush();

    const select = host.querySelector<HTMLSelectElement>(
      'select[aria-label="Audio retention override"]',
    )!;
    // A project follows the global setting until it chooses otherwise; the global default is
    // to delete.
    expect(select.value).toBe("");
    expect(select.options[0].textContent).toContain("Use the global setting");
    expect(select.options[0].textContent).toContain("currently: delete audio");
    expect(Array.from(select.options).map((o) => o.value)).toEqual(["", "keep", "delete"]);

    // The popover explains it in the same words as the global setting.
    const help = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Help: Keep audio after processing"]',
    )!;
    await act(async () => {
      help.click();
    });
    expect(host.querySelector(".help-popover-card")!.textContent).toContain("several GB");

    await setSelectValue(select, "keep");
    await act(async () => {
      findButton("Save local preferences").click();
    });
    await flush();
    expect(savePrefsSpy).toHaveBeenCalledWith(
      expect.objectContaining({ cycleId: "NOMAD", audioRetention: "keep" }),
    );
    expect((await backend.getProjectPrefs("NOMAD")).audioRetention).toBe("keep");

    // Back to "use the global setting" clears the override.
    await setSelectValue(select, "");
    await act(async () => {
      findButton("Save local preferences").click();
    });
    await flush();
    expect(savePrefsSpy).toHaveBeenLastCalledWith(
      expect.objectContaining({ cycleId: "NOMAD", audioRetention: null }),
    );
    expect((await backend.getProjectPrefs("NOMAD")).audioRetention).toBeNull();
  });

  it("detects video extension in ImportMeetingModal and imports with audio_and_slides", async () => {
    const backend = createMockBackend({ clockRate: 0, jobStepMs: 5000 });
    const importSpy = vi.spyOn(backend, "importMeeting");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    await act(async () => {
      findButton("+ Add meeting").click();
    });
    await flush();

    const pathInput = host.querySelector<HTMLInputElement>(
      'input[aria-label="Audio or video file path"]',
    )!;

    // Audio file hides video mode cards.
    await setInputValue(pathInput, "C:\\Audio\\sync.mp3");
    expect(host.querySelector(".import-mode-cards")).toBeNull();

    // Video file shows Audio only vs Audio + Visual Slides cards.
    await setInputValue(pathInput, "C:\\Videos\\sprint-demo.mkv");
    const cards = host.querySelector(".import-mode-cards");
    expect(cards).not.toBeNull();
    expect(cards?.textContent).toContain("Audio only");
    expect(cards?.textContent).toContain("Audio + Visual Slides");

    await act(async () => {
      findButton("Audio + Visual Slides").click();
    });
    await act(async () => {
      findButton("Import meeting").click();
    });
    await flush();

    expect(importSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        cycleId: "NOMAD",
        filePath: "C:\\Videos\\sprint-demo.mkv",
        mediaMode: "audio_and_slides",
        processNow: true,
      }),
    );
    expect(host.textContent).toContain("sprint-demo");
  });

  it("uses MeetingSummary.cycleId directly without N+1 IPC calls and renders GitHub issue sync & All tasks", async () => {
    const base = createMockBackend({ clockRate: 0 });
    const draftSpy = vi.spyOn(base, "getDraft");
    const voicesSpy = vi.spyOn(base, "getMeetingVoices");
    const pullSpy = vi.spyOn(base, "pullCycleRepo");
    const backend: Backend = {
      ...base,
      listMeetings: async () => [
        {
          id: "m-fresh",
          title: "Fresh 0-participant meeting",
          startedUnixMs: Date.now(),
          durationMs: 60_000,
          state: "stopped",
          trackCount: 1,
          screenshotCount: 0,
          cycleId: "NOMAD",
        },
      ],
    };

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    // Unprocessed 0-participant meeting with cycleId appears without calling getDraft/getMeetingVoices.
    expect(host.textContent).toContain("Fresh 0-participant meeting");
    expect(draftSpy).not.toHaveBeenCalled();
    expect(voicesSpy).not.toHaveBeenCalled();

    // Toggle "All tasks" to view GitHub issues and sync from GitHub.
    await act(async () => {
      findButton("All tasks").click();
    });
    await flush();
    expect(host.querySelector('ul[aria-label="Open GitHub issues"]')).not.toBeNull();
    expect(host.textContent).toContain("demo-org/nomad-cycles");

    await act(async () => {
      findButton("Sync GitHub issues").click();
    });
    await flush();
    expect(pullSpy).toHaveBeenCalledWith("team-cycles");
    expect(host.textContent).toContain("GitHub issues synced.");
  });

  it("edits GitHub repository link and stores/tests/clears Discord webhook inside ProjectSettingsModal Integrations", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const setRepoSpy = vi.spyOn(backend, "setGithubRepo");
    const testGithubSpy = vi.spyOn(backend, "testGithubAccess");
    const setSecretSpy = vi.spyOn(backend, "setTargetSecret");
    const testWebhookSpy = vi.spyOn(backend, "testDiscordWebhook");
    const clearSecretSpy = vi.spyOn(backend, "clearTargetSecret");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <ProjectDetailView cycleId="NOMAD" onBack={vi.fn()} onOpenMeeting={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    await act(async () => {
      host.querySelector<HTMLButtonElement>('button[aria-label="Project settings"]')!.click();
    });
    await flush();

    await act(async () => {
      findButton("Integrations").click();
    });
    await flush();

    // GitHub repository link editing & access test.
    const repoForm = host.querySelector<HTMLElement>(
      '[role="form"][aria-label="GitHub repository of team-cycles"]',
    )!;
    expect(repoForm).not.toBeNull();
    const repoInput = repoForm.querySelector<HTMLInputElement>('input[placeholder="owner/name"]')!;
    await setInputValue(repoInput, "demo-org/updated-nomad");
    await act(async () => {
      findButton("Save link").click();
    });
    await flush();
    expect(setRepoSpy).toHaveBeenCalledWith("team-cycles", "demo-org/updated-nomad", "");

    await act(async () => {
      findButton("Test access").click();
    });
    await flush();
    expect(testGithubSpy).toHaveBeenCalledWith("NOMAD");

    // Discord webhook store / test / clear.
    const webhookGroup = host.querySelector<HTMLElement>(
      '[role="form"][aria-label="Webhook for cred:discord/nomad"]',
    )!;
    expect(webhookGroup).not.toBeNull();
    const webhookInput = webhookGroup.querySelector<HTMLInputElement>('input[type="password"]')!;
    await setInputValue(
      webhookInput,
      "https://discord.com/api/webhooks/000000000000000999/secret-token",
    );
    await act(async () => {
      findButton("Store", webhookGroup).click();
    });
    await flush();
    expect(setSecretSpy).toHaveBeenCalledWith(
      "cred:discord/nomad",
      "https://discord.com/api/webhooks/000000000000000999/secret-token",
    );
    expect(webhookGroup.textContent).toContain("0000...0999");

    await act(async () => {
      findButton("Test webhook", webhookGroup).click();
    });
    await flush();
    expect(testWebhookSpy).toHaveBeenCalledWith("cred:discord/nomad");
    expect(webhookGroup.textContent).toContain("The webhook exists.");

    await act(async () => {
      findButton("Clear", webhookGroup).click();
    });
    await flush();
    expect(clearSecretSpy).toHaveBeenCalledWith("cred:discord/nomad");
    expect(webhookGroup.textContent).toContain("No webhook stored");
  });
});

describe("MeetingDetailView and DraftEditor markdown export", () => {
  it("switches between Draft & Report, Transcript, and Speakers tabs and exports Markdown", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const exportSpy = vi.spyOn(backend, "exportMeetingMarkdown");

    await act(async () => {
      root.render(
        <BackendProvider value={backend}>
          <MeetingDetailView meetingId="m5" onBack={vi.fn()} onOpenQueue={vi.fn()} />
        </BackendProvider>,
      );
    });
    await flush();

    expect(host.textContent).toContain("Thruster test 7 pressure flat");
    expect(host.textContent).toContain("Draft");

    await act(async () => {
      findButton("Download note (.md)").click();
    });
    await flush();
    expect(exportSpy).toHaveBeenCalledWith("m5");
    expect(host.textContent).toContain("Saved note to");

    // Switch to Transcript tab.
    const tabs = host.querySelector('div[role="tablist"][aria-label="Meeting sections"]')!;
    await act(async () => {
      findButton("Transcript", tabs).click();
    });
    await flush();
    expect(host.querySelector('ol[aria-label="Transcript"]')).not.toBeNull();

    // Switch to Speakers tab.
    await act(async () => {
      findButton("Speakers", tabs).click();
    });
    await flush();
    expect(host.querySelector('ul[aria-label="Detected voices"]')).not.toBeNull();
  });
});
