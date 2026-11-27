import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend } from "../ipc/mock";
import type { DraftItem, DraftView, ParticipantView } from "../ipc/types";
import { DraftEditor } from "./DraftEditor";

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

function makeTask(patch: Partial<DraftItem> = {}): DraftItem {
  return {
    id: "i001",
    type: "task",
    text: "Opróżnić plecak",
    assignee: null,
    assigneeName: "Marek",
    due: null,
    certainty: { assignee: "stated", due: "unknown" },
    section: null,
    relatesTo: null,
    source: {
      track: "mic",
      speaker: "alex",
      tStartMs: 1000,
      tEndMs: 4000,
      quote: "Marek ma do zrobienia zadanie numer 37, opróżnić plecak.",
      verified: true,
      match: 1.0,
    },
    edited: false,
    ...patch,
  };
}

function makeView(item: DraftItem, extra: Partial<DraftView> = {}): DraftView {
  return {
    draft: {
      meetingId: "m1",
      cycle: "NOMAD",
      date: "2026-11-13",
      language: "pl",
      mode: "fixed",
      template: null,
      meetingType: null,
      sections: [],
      summary: "Summary.",
      items: [item],
      warnings: [],
    },
    reportMarkdown: "# Report",
    hasNewDraft: false,
    participants: [{ id: "alex", name: "Alex" }],
    transcriptPresent: false,
    reportWarning: null,
    locked: false,
    ...extra,
  };
}

function mountEditor(backend: Backend) {
  act(() => {
    root.render(
      <BackendProvider value={backend}>
        <DraftEditor
          meetingId="m1"
          onBack={() => undefined}
          onOpenTranscript={() => undefined}
          onOpenQueue={() => undefined}
        />
      </BackendProvider>,
    );
  });
}

function clickButton(text: string) {
  const btn = [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === text);
  expect(btn).toBeTruthy();
  act(() => {
    btn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  return btn!;
}

describe("DraftEditor assigneeName", () => {
  it("shows external assigneeName and adds them to project people", async () => {
    const view = makeView(makeTask());
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);
    const createParticipant = vi.spyOn(backend, "createParticipant").mockResolvedValue({
      id: "NOMAD:marek",
      name: "Marek",
      sampleCount: 0,
      speechMs: 0,
      cycleId: "NOMAD",
      cycleParticipantId: "marek",
    });
    const updateDraftItem = vi.spyOn(backend, "updateDraftItem").mockResolvedValue({
      ...view,
      draft: {
        ...view.draft,
        items: [makeTask({ assignee: "marek", assigneeName: null })],
      },
    });

    mountEditor(backend);
    await flush();

    expect(host.textContent).toContain("Marek (not in project people)");
    clickButton("Add to project people");
    await flush();

    expect(createParticipant).toHaveBeenCalledWith("Marek", "NOMAD");
    expect(updateDraftItem).toHaveBeenCalledWith("m1", "i001", { assignee: "marek" });
  });

  it("reuses existing participant case-insensitively without calling createParticipant", async () => {
    const view = makeView(makeTask({ assigneeName: "marek" }), {
      participants: [
        { id: "alex", name: "Alex" },
        { id: "marek", name: "Marek" },
      ],
    });
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);
    const createParticipant = vi.spyOn(backend, "createParticipant");
    const updateDraftItem = vi.spyOn(backend, "updateDraftItem").mockResolvedValue(view);

    mountEditor(backend);
    await flush();

    clickButton("Add to project people");
    await flush();

    expect(createParticipant).not.toHaveBeenCalled();
    expect(updateDraftItem).toHaveBeenCalledWith("m1", "i001", { assignee: "marek" });
  });

  it("recovers from duplicate participant error via listParticipants", async () => {
    const view = makeView(makeTask());
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);
    vi.spyOn(backend, "createParticipant").mockRejectedValue({
      code: "duplicate",
      message: "a participant with this name already exists",
    });
    const existing: ParticipantView = {
      id: "NOMAD:marek-1",
      name: "Marek",
      sampleCount: 0,
      speechMs: 0,
      cycleId: "NOMAD",
      cycleParticipantId: "marek-1",
    };
    const listParticipants = vi.spyOn(backend, "listParticipants").mockResolvedValue([existing]);
    const updateDraftItem = vi.spyOn(backend, "updateDraftItem").mockResolvedValue(view);

    mountEditor(backend);
    await flush();

    clickButton("Add to project people");
    await flush();

    expect(listParticipants).toHaveBeenCalledWith("NOMAD");
    expect(updateDraftItem).toHaveBeenCalledWith("m1", "i001", { assignee: "marek-1" });
  });

  it("clears the assignee name with the Clear button", async () => {
    const view = makeView(makeTask());
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);
    const updateDraftItem = vi.spyOn(backend, "updateDraftItem").mockResolvedValue(view);

    mountEditor(backend);
    await flush();

    clickButton("Clear");
    await flush();

    expect(updateDraftItem).toHaveBeenCalledWith("m1", "i001", { assignee: null });
  });

  it("keeps the Add button outside the assignee label", async () => {
    const view = makeView(makeTask());
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);

    mountEditor(backend);
    await flush();

    const add = [...host.querySelectorAll("button")].find(
      (b) => b.textContent?.trim() === "Add to project people",
    );
    expect(add).toBeTruthy();
    expect(add!.closest("label")).toBeNull();
  });

  it("disables Add to project people when draft is locked", async () => {
    const view = makeView(makeTask(), { locked: true });
    const backend = createMockBackend();
    vi.spyOn(backend, "getDraft").mockResolvedValue(view);

    mountEditor(backend);
    await flush();

    const btn = [...host.querySelectorAll("button")].find(
      (b) => b.textContent?.trim() === "Add to project people",
    );
    expect(btn).toBeTruthy();
    expect(btn?.disabled).toBe(true);
  });
});
