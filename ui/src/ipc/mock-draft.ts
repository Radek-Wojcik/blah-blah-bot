/**
 * The draft of the demo meeting and the operations on a draft, with the rules of
 * `crates/queue/src/draft.rs`: every user change marks the item as `edited`, a value the user
 * sets counts as stated, a section that does not exist is refused.
 */
import { IDENTIFIER, fail } from "./mock-util";
import type {
  CertaintyLevel,
  Draft,
  DraftItem,
  DraftItemPatch,
  DraftSection,
  DraftSource,
  ItemType,
  NewDraftItem,
  ProposedStatus,
  ReportMode,
  StatusDecision,
  StatusProposal,
} from "./types";

function src(
  speaker: string,
  tStartMs: number,
  tEndMs: number,
  quote: string,
  verified: boolean,
  match: number,
): DraftSource {
  return {
    track: speaker === "user" ? "mic-default" : "app-discord",
    speaker,
    tStartMs,
    tEndMs,
    quote,
    verified,
    match,
  };
}

function item(
  id: string,
  type: ItemType,
  text: string,
  section: string | null,
  source: DraftSource | null,
  extra: Partial<DraftItem> = {},
): DraftItem {
  return {
    id,
    type,
    text,
    assignee: null,
    due: null,
    certainty: { assignee: "unknown", due: "unknown" },
    section,
    relatesTo: null,
    source,
    edited: false,
    ...extra,
  };
}

function proposal(
  id: string,
  target: string,
  status: ProposedStatus,
  reason: string,
  source: DraftSource | null,
): StatusProposal {
  return { id, target, status, reason, comment: null, source, decision: "proposed", edited: false };
}

const level = (assignee: CertaintyLevel, due: CertaintyLevel) => ({ assignee, due });
const MIN = 60_000;

/**
 * The finished draft of the demo meeting: twelve items of every type, mixed certainty, two
 * unverified quotes, one item added by hand. The times point into `demoTranscript`.
 */
export function demoDraft(meetingId: string, cycle = "NOMAD"): Draft {
  return {
    meetingId,
    cycle,
    date: "2026-11-13",
    language: "en",
    mode: "auto",
    template: null,
    meetingType: null,
    sections: [
      { id: "thruster-tests", title: "Thruster tests", description: "Test firings and results" },
      { id: "ground-segment", title: "Ground segment", description: "Ground station and antenna" },
      { id: "budget", title: "Budget and procurement", description: "Spending and orders" },
      { id: "follow-ups", title: "Follow-ups", description: "What happens next" },
    ],
    summary:
      "The team reviewed the latest thruster firing, confirmed that the budget is two percent under plan and agreed to keep the launch window. The ground station schedule and the antenna mount are on track.",
    items: [
      item(
        "i01",
        "note",
        "The last thruster firing ran for 90 seconds and the chamber pressure stayed flat.",
        "thruster-tests",
        src(
          "alex",
          6_000,
          18_500,
          "The last firing ran for ninety seconds and the chamber pressure stayed flat.",
          true,
          0.98,
        ),
      ),
      item(
        "i02",
        "note",
        "The new valve causes only a small ripple at ignition that can be filtered out.",
        "thruster-tests",
        src(
          "alex",
          27_000,
          39_500,
          "Only a small ripple at ignition, nothing we cannot filter out.",
          true,
          0.97,
        ),
      ),
      item(
        "i03",
        "task",
        "Update the ground station schedule for Thursday.",
        "ground-segment",
        src(
          "jan",
          41_000,
          52_000,
          "I will update the ground station schedule for Thursday.",
          true,
          0.99,
        ),
        { assignee: "jan", due: "2026-11-19", certainty: level("stated", "inferred") },
      ),
      item(
        "i04",
        "task",
        "Share the budget sheet before the review.",
        "budget",
        src("user", 55_000, 62_000, "Please share the budget sheet before the review.", true, 1),
      ),
      item(
        "i05",
        "decision",
        "Buy a second test stand with the underspend.",
        "budget",
        src(
          "alex",
          2 * MIN + 20_000,
          2 * MIN + 31_000,
          "We will buy a second test stand with the underspend.",
          false,
          0.41,
        ),
      ),
      item(
        "i06",
        "budget",
        "The budget is two percent under plan.",
        "budget",
        src(
          "kasia",
          2 * MIN + 5_000,
          2 * MIN + 17_000,
          "The budget is on track, we are two percent under.",
          true,
          0.96,
        ),
      ),
      item(
        "i07",
        "topic",
        "Confirm the delivery date of the antenna array; the mount is already on site.",
        "ground-segment",
        src(
          "jan",
          2 * MIN + 43_000,
          2 * MIN + 55_000,
          "The antenna array arrives next week, the mount is already on site.",
          true,
          0.99,
        ),
      ),
      item(
        "i08",
        "decision",
        "Keep the launch window as planned.",
        "follow-ups",
        src(
          "user",
          3 * MIN,
          3 * MIN + 9_000,
          "Great, then we keep the launch window as planned.",
          true,
          0.95,
        ),
      ),
      item(
        "i09",
        "task",
        "Send the meeting summary to everyone.",
        "follow-ups",
        src(
          "alex",
          3 * MIN + 30_000,
          3 * MIN + 41_000,
          "I will send the summary to everyone tomorrow.",
          true,
          0.97,
        ),
        { assignee: "alex", due: "2026-11-14", certainty: level("stated", "inferred") },
      ),
      item(
        "i10",
        "idea",
        "Use the spare budget for a second test stand.",
        "budget",
        src(
          "alex",
          2 * MIN + 20_000,
          2 * MIN + 31_000,
          "That leaves room for a second test stand.",
          true,
          0.95,
        ),
      ),
      item(
        "i11",
        "task",
        "Prepare the filter for the ignition ripple.",
        "thruster-tests",
        src(
          "kasia",
          5 * MIN + 48_000,
          5 * MIN + 58_000,
          "I can take the action item for the test stand.",
          false,
          0.38,
        ),
        { assignee: "kasia", certainty: level("inferred", "unknown") },
      ),
      item("i12", "idea", "Record the next test with a high-speed camera.", "follow-ups", null, {
        edited: true,
      }),
    ],
    statusProposals: [
      proposal(
        "s1",
        "#12",
        "done",
        "Alex said the antenna mount drawings are finished and uploaded.",
        src(
          "alex",
          4 * MIN + 10_000,
          4 * MIN + 22_000,
          "The antenna mount drawings are finished and uploaded.",
          true,
          0.97,
        ),
      ),
      proposal(
        "s2",
        "#11",
        "done",
        "Kasia confirmed that the spare valves were ordered.",
        src(
          "kasia",
          4 * MIN + 40_000,
          4 * MIN + 50_000,
          "The spare valves are on order, the confirmation came in yesterday.",
          false,
          0.43,
        ),
      ),
      proposal(
        "s3",
        "t-1",
        "done",
        "The team settled the rover wheel size.",
        src(
          "jan",
          5 * MIN + 5_000,
          5 * MIN + 14_000,
          "So we go with the two hundred eighty millimetre wheels.",
          true,
          0.94,
        ),
      ),
    ],
    warnings: [
      {
        code: "window_skipped",
        message:
          "The window 15:00 to 30:00 could not be read by the model after two retries and was skipped.",
        tStartMs: 15 * MIN,
        tEndMs: 30 * MIN,
      },
      { code: "quote_unverified", message: "Two quotes were not found in the transcript." },
      {
        code: "assignee_unknown",
        message: "One assignee was not a participant of the cycle and was cleared.",
      },
    ],
  };
}

// ----- operations ------------------------------------------------------------------------

export function isIsoDate(s: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s)) return false;
  const month = Number(s.slice(5, 7));
  const day = Number(s.slice(8, 10));
  return month >= 1 && month <= 12 && day >= 1 && day <= 31;
}

function find(draft: Draft, id: string): number {
  const at = draft.items.findIndex((i) => i.id === id);
  if (at < 0) fail("not_found", `no such item: ${id}`);
  return at;
}

function checkSection(draft: Draft, section: string | null | undefined) {
  if (section && !draft.sections.some((s) => s.id === section))
    fail("not_found", `no such section: ${section}`);
}

const decided = (value: string | null): CertaintyLevel => (value === null ? "unknown" : "stated");

/** A patch with `undefined` fields leaves them as they are; `null` clears assignee and due. */
export function updateItem(draft: Draft, id: string, patch: DraftItemPatch): void {
  const at = find(draft, id);
  if (patch.text !== undefined && patch.text.trim() === "")
    fail("invalid", "the text of an item cannot be empty");
  if (patch.due && !isIsoDate(patch.due))
    fail("invalid", `the due date "${patch.due}" is not YYYY-MM-DD`);
  if (patch.assignee !== undefined && patch.assignee !== null && patch.assignee.trim() === "")
    fail("invalid", "the assignee cannot be an empty id");
  const it = draft.items[at];
  let touched = false;
  if (patch.text !== undefined) {
    it.text = patch.text.trim();
    touched = true;
  }
  if (patch.assignee !== undefined) {
    it.assignee = patch.assignee;
    it.certainty.assignee = decided(it.assignee);
    touched = true;
  }
  if (patch.due !== undefined) {
    it.due = patch.due;
    it.certainty.due = decided(it.due);
    touched = true;
  }
  if (touched) it.edited = true;
}

/** Moves an item to a section (`null`: none) and in front of `before`, else last. */
export function moveItem(draft: Draft, id: string, section: string | null, before?: string): void {
  const at = find(draft, id);
  checkSection(draft, section);
  if (before !== undefined) {
    if (before === id) fail("invalid", "an item cannot be moved before itself");
    const target = draft.items[find(draft, before)];
    if ((target.section ?? null) !== section)
      fail("invalid", `item ${before} is not in the section the item is moved to`);
  }
  const [moved] = draft.items.splice(at, 1);
  moved.section = section;
  moved.edited = true;
  const position = before === undefined ? draft.items.length : find(draft, before);
  draft.items.splice(position, 0, moved);
}

export function deleteItem(draft: Draft, id: string): void {
  draft.items.splice(find(draft, id), 1);
}

export function addItem(draft: Draft, n: NewDraftItem): string {
  if (n.text.trim() === "") fail("invalid", "the text of an item cannot be empty");
  if (n.due && !isIsoDate(n.due)) fail("invalid", `the due date "${n.due}" is not YYYY-MM-DD`);
  checkSection(draft, n.section);
  let k = 1;
  while (draft.items.some((i) => i.id === `u${String(k).padStart(3, "0")}`)) k += 1;
  const id = `u${String(k).padStart(3, "0")}`;
  const assignee = n.assignee ?? null;
  const due = n.due ?? null;
  draft.items.push({
    id,
    type: n.type,
    text: n.text.trim(),
    assignee,
    due,
    certainty: { assignee: decided(assignee), due: decided(due) },
    section: n.section ?? null,
    relatesTo: null,
    source: null,
    edited: true,
  });
  return id;
}

/** An id for a new section made from its title, not used by another section. */
export function sectionIdFor(draft: Pick<Draft, "sections">, title: string): string {
  let base = title
    .split("")
    .map((c) => (/[A-Za-z0-9]/.test(c) ? c.toLowerCase() : "-"))
    .join("")
    .split("-")
    .filter(Boolean)
    .join("-");
  if (!/^[A-Za-z]/.test(base)) base = `section-${base}`.replace(/-+$/, "");
  let id = base;
  let n = 2;
  while (draft.sections.some((s) => s.id === id)) {
    id = `${base}-${n}`;
    n += 1;
  }
  return id;
}

/**
 * Replaces the sections (rename, add, remove, reorder). A section with an empty id gets one
 * made from its title. Items of a removed section lose their section and count as edited.
 */
export function setSections(draft: Draft, sections: DraftSection[]): void {
  const scratch: Pick<Draft, "sections"> = {
    sections: sections.filter((s) => s.id !== "").map((s) => ({ ...s, id: s.id.trim() })),
  };
  const out: DraftSection[] = [];
  for (const s of sections) {
    const section = { id: s.id.trim(), title: s.title.trim(), description: s.description };
    if (section.id === "") {
      section.id = sectionIdFor(scratch, section.title);
      scratch.sections.push({ ...section });
    }
    out.push(section);
  }
  const seen = new Set<string>();
  for (const s of out) {
    if (!IDENTIFIER.test(s.id))
      fail("invalid", `"${s.id}" is not a valid section id (lowercase letters, digits, - and _)`);
    if (s.title === "") fail("invalid", `section ${s.id} needs a title`);
    if (seen.has(s.id)) fail("invalid", `section ${s.id} is listed twice`);
    seen.add(s.id);
  }
  for (const it of draft.items) {
    if (it.section !== null && !seen.has(it.section)) {
      it.section = null;
      it.edited = true;
    }
  }
  draft.sections = out;
}

/** Layout only: no item is touched. */
export function setModeAndTemplate(
  draft: Draft,
  mode: ReportMode,
  template: string | null,
  meetingType: string | null,
): void {
  draft.mode = mode;
  draft.template = template;
  draft.meetingType = meetingType;
}

export function hasUserEdits(draft: Draft): boolean {
  return (
    draft.items.some((i) => i.edited) ||
    (draft.statusProposals ?? []).some((p) => p.edited || p.decision !== "proposed")
  );
}

function findProposal(draft: Draft, id: string): StatusProposal {
  return (
    (draft.statusProposals ?? []).find((p) => p.id === id) ??
    fail("not_found", `no such status change: ${id}`)
  );
}

/**
 * Decides a proposed status change (`set_status_decision`): `comment` undefined keeps the text,
 * an empty one clears it, and a changed text marks the proposal as edited.
 */
export function setStatusDecision(
  draft: Draft,
  id: string,
  decision: StatusDecision,
  comment?: string,
): void {
  const p = findProposal(draft, id);
  if (comment !== undefined) {
    const text = comment.trim() === "" ? null : comment.trim();
    if (text !== p.comment) {
      p.comment = text;
      p.edited = true;
    }
  }
  p.decision = decision;
}

const ISSUE_TARGET = /^#[0-9]+$/;

/**
 * Adds a status change by hand (`add_status_proposal`): an issue is written `#12`, anything else
 * is the id of a topic. One change per target; it waits for approval.
 */
export function addStatusProposal(
  draft: Draft,
  target: string,
  status: ProposedStatus,
  comment?: string,
  topics: string[] = [],
): string {
  const t = target.trim();
  if (!ISSUE_TARGET.test(t)) {
    if (/^[0-9]+$/.test(t) || !IDENTIFIER.test(t))
      fail("invalid", "the target is an issue number such as #12 or the id of a topic");
    if (!topics.includes(t))
      fail("invalid", `${t} is not a topic of the cycle (issues are written #12)`);
  }
  const list = (draft.statusProposals ??= []);
  if (list.some((p) => p.target === t)) fail("invalid", `${t} has a status change already`);
  const text = comment === undefined || comment.trim() === "" ? null : comment.trim();
  const next = Math.max(0, ...list.map((p) => Number(p.id.slice(1)) || 0)) + 1;
  const id = `s${next}`;
  list.push({
    id,
    target: t,
    status,
    reason: text ?? "Added by hand.",
    comment: text,
    source: null,
    decision: "proposed",
    edited: false,
  });
  return id;
}

/** The newer draft a re-run of `reduce` leaves next to an edited one: other summary, one more item. */
export function demoNewDraft(meetingId: string, cycle = "NOMAD"): Draft {
  const draft = demoDraft(meetingId, cycle);
  draft.summary =
    "Second pass: the thruster firing was stable, the budget is two percent under plan, the launch window stays and the antenna mount is already on site.";
  draft.items = draft.items.filter((i) => i.id !== "i12");
  draft.items.push(
    item(
      "i13",
      "note",
      "The valve vendor confirmed the next delivery for the end of the month.",
      "thruster-tests",
      src(
        "kasia",
        2 * MIN + 5_000,
        2 * MIN + 17_000,
        "The valve vendor will deliver at the end of the month.",
        false,
        0.44,
      ),
    ),
  );
  return draft;
}
