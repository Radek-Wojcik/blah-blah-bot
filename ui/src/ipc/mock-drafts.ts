/**
 * The draft commands of the mock backend (`get_draft` and the operations of
 * `crates/app/src/draft_commands.rs`): in-memory drafts that really change, rendered with the
 * cycle's template.
 */
import type { Backend, Unsubscribe } from "./backend";
import type { CycleStore, RecordingReport } from "./mock-cycles";
import { demoDraft, demoNewDraft } from "./mock-draft";
import * as ops from "./mock-draft";
import { DEFAULT_TEMPLATE_TEXT, renderReport } from "./mock-render";
import { fail } from "./mock-util";
import type { Draft, DraftView, RenderVariant, ReportMode } from "./types";

type DraftMethods = Pick<
  Backend,
  | "getDraft"
  | "updateDraftItem"
  | "moveDraftItem"
  | "deleteDraftItem"
  | "addDraftItem"
  | "setDraftSections"
  | "setReportMode"
  | "acceptNewDraft"
  | "discardNewDraft"
  | "renderReport"
  | "setStatusDecision"
  | "addStatusProposal"
  | "onDraftChanged"
>;

interface Held {
  draft: Draft;
  newDraft: Draft | null;
}

export interface MockDrafts {
  methods: DraftMethods;
  has(meetingId: string): boolean;
  /** Items in the drafts of the cycle's meetings other than `exceptMeetingId`. */
  itemsOfCycle(cycleId: string, exceptMeetingId: string): number;
  /** The draft `reduce` writes for a meeting recorded with `report`. */
  create(meetingId: string, report: RecordingReport): void;
  /** Leaves a newer draft next to the edited one (a re-run of `reduce`). */
  leaveNewDraft(meetingId: string): void;
  /** Tells the views that the draft changed without an edit (it was unlocked). */
  touch(meetingId: string): void;
}

export function createMockDrafts(
  cycles: CycleStore,
  hasTranscript: (meetingId: string) => boolean,
  /** Operations for the meeting are in the outbox: its draft is read-only. */
  isLocked: (meetingId: string) => boolean = () => false,
): MockDrafts {
  const drafts = new Map<string, Held>();
  const listeners = new Set<{ cb: (id: string) => void }>();
  const emit = (id: string) => listeners.forEach((l) => l.cb(id));

  const held = (id: string): Held =>
    drafts.get(id) ?? fail("no_draft", "the draft is not ready for this meeting");

  /** The template that lays out the report (`template_name` in `draft_commands.rs`). */
  function templateName(d: Draft): string {
    const cycle = cycles.cycle(d.cycle);
    const fallback = cycle?.report.defaultTemplate ?? "default";
    if (d.mode === "by-type")
      return (d.meetingType ? cycle?.report.byType[d.meetingType] : undefined) ?? fallback;
    return d.template ?? fallback;
  }

  function render(d: Draft, variant: RenderVariant): { text: string; warning: string | null } {
    const people = cycles.cycle(d.cycle)?.participants ?? [];
    const name = templateName(d);
    const template = cycles.template(d.cycle, name);
    let warning: string | null = null;
    if (template) {
      try {
        return { text: renderReport(d, people, template.body, d.language, variant), warning };
      } catch (e) {
        warning = `template ${name} cannot be used: ${(e as Error).message}`;
      }
    } else {
      warning = `template ${name} cannot be read: it does not exist`;
    }
    return { text: renderReport(d, people, DEFAULT_TEMPLATE_TEXT, d.language, variant), warning };
  }

  /** Like `ensure_draft_unlocked`: every change of the draft starts with it. */
  function unlocked(id: string) {
    if (isLocked(id))
      fail(
        "published",
        "the meeting is published or queued for publishing: its draft is read-only until the operations are cancelled",
      );
  }

  function view(id: string): DraftView {
    const h = held(id);
    const { text, warning } = render(h.draft, "markdown");
    return {
      draft: structuredClone(h.draft),
      reportMarkdown: text,
      hasNewDraft: h.newDraft !== null,
      participants: (cycles.cycle(h.draft.cycle)?.participants ?? []).map((p) => ({
        id: p.id,
        name: p.name,
      })),
      transcriptPresent: hasTranscript(id),
      reportWarning: warning,
      locked: isLocked(id),
    };
  }

  /** Applies a change to a copy and stores it only when it did not throw. */
  function edit(id: string, change: (d: Draft) => void): DraftView {
    const h = held(id);
    unlocked(id);
    const copy = structuredClone(h.draft);
    change(copy);
    h.draft = copy;
    emit(id);
    return view(id);
  }

  /** An assignee must be a participant of the cycle (or none). */
  function checkAssignee(id: string, assignee: string | null | undefined) {
    if (!assignee) return;
    const cycle = cycles.cycle(held(id).draft.cycle);
    if (cycle && !cycle.participants.some((p) => p.id === assignee))
      fail("invalid", `${assignee} is not a participant of the cycle`);
  }

  /** The layout a new draft starts with, following the report settings of the recording. */
  function layout(base: Draft, report: RecordingReport): Draft {
    const cycle = cycles.cycle(report.cycleId ?? "");
    const mode: ReportMode = report.mode ?? "fixed";
    const d = structuredClone(base);
    d.mode = mode;
    d.template = report.template;
    d.meetingType = report.meetingType;
    const lang = report.outputLanguage ?? cycle?.languages.output ?? "auto";
    d.language = lang === "pl" ? "pl" : "en";
    if (mode === "auto") return d;
    const templateOf =
      mode === "by-type" && report.meetingType
        ? cycle?.report.byType[report.meetingType]
        : (report.template ?? cycle?.report.defaultTemplate);
    if (mode === "by-type") d.template = templateOf ?? null;
    const sections =
      mode === "fixed" ? [] : (cycles.template(d.cycle, templateOf ?? "")?.sections ?? []);
    d.sections = structuredClone(sections);
    // The model assigns each item to one section; the mock deals them out in turn.
    d.items.forEach((item, i) => {
      item.section = sections.length === 0 ? null : sections[i % sections.length].id;
    });
    return d;
  }

  const methods: DraftMethods = {
    getDraft: async (id) => view(id),
    updateDraftItem: async (id, itemId, patch) => {
      if (patch.assignee) checkAssignee(id, patch.assignee);
      return edit(id, (d) => ops.updateItem(d, itemId, patch));
    },
    moveDraftItem: async (id, itemId, sectionId, beforeItemId) =>
      edit(id, (d) => ops.moveItem(d, itemId, sectionId, beforeItemId)),
    deleteDraftItem: async (id, itemId) => edit(id, (d) => ops.deleteItem(d, itemId)),
    addDraftItem: async (id, item) => {
      checkAssignee(id, item.assignee);
      return edit(id, (d) => void ops.addItem(d, item));
    },
    setDraftSections: async (id, sections) => edit(id, (d) => ops.setSections(d, sections)),
    setReportMode: async (id, mode, template, meetingType) => {
      unlocked(id);
      const draft = held(id).draft;
      const checked = cycles.validateReport({ cycleId: draft.cycle, mode, template, meetingType });
      const cycle = cycles.cycle(draft.cycle);
      const layoutTemplate = checked.meetingType
        ? (cycle?.report.byType[checked.meetingType] ?? null)
        : checked.template;
      return edit(id, (d) => ops.setModeAndTemplate(d, mode, layoutTemplate, checked.meetingType));
    },
    acceptNewDraft: async (id) => {
      unlocked(id);
      const h = held(id);
      if (h.newDraft === null) fail("no_new_draft", "there is no newer draft to accept");
      h.draft = h.newDraft;
      h.newDraft = null;
      emit(id);
      return view(id);
    },
    discardNewDraft: async (id) => {
      unlocked(id);
      held(id).newDraft = null;
      emit(id);
      return view(id);
    },
    renderReport: async (id, variant) => {
      if (variant !== "markdown" && variant !== "chat")
        fail("invalid", "the variant is markdown or chat");
      return render(held(id).draft, variant).text;
    },
    setStatusDecision: async (id, proposalId, decision, comment) =>
      edit(id, (d) => ops.setStatusDecision(d, proposalId, decision, comment)),
    addStatusProposal: async (id, target, status, comment) =>
      edit(
        id,
        (d) =>
          void ops.addStatusProposal(
            d,
            target,
            status,
            comment,
            cycles.topics(d.cycle).map((t) => t.id),
          ),
      ),
    onDraftChanged: (cb): Promise<Unsubscribe> => {
      const entry = { cb };
      listeners.add(entry);
      return Promise.resolve(() => void listeners.delete(entry));
    },
  };

  return {
    methods,
    has: (id) => drafts.has(id),
    itemsOfCycle: (cycleId, exceptMeetingId) =>
      [...drafts.entries()]
        .filter(([id, h]) => id !== exceptMeetingId && h.draft.cycle === cycleId)
        .reduce((n, [, h]) => n + h.draft.items.length, 0),
    create: (id, report) => {
      const base = demoDraft(id, report.cycleId ?? "NOMAD");
      drafts.set(id, { draft: layout(base, report), newDraft: null });
      emit(id);
    },
    touch: (id) => emit(id),
    leaveNewDraft: (id) => {
      const h = held(id);
      h.newDraft = demoNewDraft(id, h.draft.cycle);
      emit(id);
    },
  };
}
