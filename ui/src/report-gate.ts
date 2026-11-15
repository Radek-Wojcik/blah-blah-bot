import type {
  CycleListItem,
  LanguageSetting,
  LastReport,
  ReportChoice,
  ReportDefaults,
  ReportMode,
} from "./ipc/types";

export const REPORT_MODES: ReportMode[] = ["fixed", "template", "auto", "by-type"];

/** One line each: what the mode does. Shown at the gate and in the cycle editor. */
export const MODE_HELP: Record<ReportMode, string> = {
  fixed: "Built-in sections: decisions, tasks, open topics, notes, ideas and budgets.",
  template: "Sections come from a template of the cycle; the model puts each item in one.",
  auto: "The model proposes the sections for this meeting; you can change them in the draft.",
  "by-type": "The meeting type picks a template of the cycle.",
};

export const MODE_NAMES: Record<ReportMode, string> = {
  fixed: "Fixed",
  template: "Template",
  auto: "Auto",
  "by-type": "By type",
};

/** What the gate form holds. An empty `mode` is "not chosen yet". */
export interface GateForm {
  cycleId: string;
  mode: ReportMode | "";
  template: string;
  meetingType: string;
  /** Empty means the cycle's own setting. */
  outputLanguage: LanguageSetting | "";
}

export const EMPTY_FORM: GateForm = {
  cycleId: "",
  mode: "",
  template: "",
  meetingType: "",
  outputLanguage: "",
};

/** The form a cycle's defaults give, with the last choice for this cycle taking precedence. */
export function formFromDefaults(d: ReportDefaults): GateForm {
  const last = d.last;
  return {
    cycleId: d.cycleId,
    mode: last?.mode ?? d.defaultMode,
    template: last?.template ?? d.defaultTemplate,
    meetingType: last?.meetingType ?? d.defaultMeetingType ?? "",
    outputLanguage: "",
  };
}

/** Why the form cannot start a recording, or null when it can. */
export function validateGate(form: GateForm, d: ReportDefaults | null): string | null {
  if (form.cycleId === "") return "Choose a cycle.";
  if (d === null || d.cycleId !== form.cycleId) return "Loading the cycle…";
  if (form.mode === "") return "Choose a report mode.";
  if (form.mode === "template") {
    if (form.template === "") return "Choose a template.";
    if (!d.templates.some((t) => t.name === form.template))
      return `The cycle has no template named ${form.template}.`;
  }
  if (form.mode === "by-type") {
    if (d.meetingTypes.length === 0)
      return "This cycle defines no meeting types. Add one in the cycle settings.";
    if (form.meetingType === "") return "Choose the meeting type.";
    if (!d.meetingTypes.some((t) => t.name === form.meetingType))
      return `The cycle does not define the meeting type ${form.meetingType}.`;
  }
  return null;
}

/** The choice sent to `startRecording`; only the fields the mode uses are set. */
export function toReportChoice(form: GateForm): ReportChoice {
  if (form.mode === "") throw new Error("no report mode chosen");
  return {
    cycleId: form.cycleId,
    mode: form.mode,
    ...(form.mode === "template" ? { template: form.template } : {}),
    ...(form.mode === "by-type" ? { meetingType: form.meetingType } : {}),
    ...(form.outputLanguage === "" ? {} : { outputLanguage: form.outputLanguage }),
  };
}

/** `NOMAD · fixed`, `NOMAD · template (budget-review)`, `NOMAD · by-type (budget)`. */
export function describeChoice(
  c: Pick<LastReport, "cycleId" | "mode" | "template" | "meetingType">,
): string {
  let detail = "";
  if (c.mode === "template" && c.template) detail = ` (${c.template})`;
  if (c.mode === "by-type" && c.meetingType) detail = ` (${c.meetingType})`;
  return `${c.cycleId} · ${c.mode}${detail}`;
}

/** The last report when it is still usable: its cycle exists and has no issues. */
export function usableLast(
  last: LastReport | null | undefined,
  cycles: CycleListItem[],
): LastReport | null {
  if (!last) return null;
  return cycles.some((c) => c.id === last.cycleId && c.valid) ? last : null;
}

/** The cycle the gate starts with: the last used one, else the only valid one, else none. */
export function initialCycleId(
  last: LastReport | null | undefined,
  cycles: CycleListItem[],
): string {
  const usable = usableLast(last, cycles);
  if (usable) return usable.cycleId;
  const valid = cycles.filter((c) => c.valid);
  return valid.length === 1 ? valid[0].id : "";
}

/** A readable text for a refusal of the backend at the gate. */
export function gateMessage(code: string | undefined, message: string): string {
  if (code === "report_mode_required")
    return `The recording was not started: ${message.replace(/\.$/, "")}.`;
  if (code === "cycle_invalid") return `This cycle cannot be used until it is fixed: ${message}`;
  return message;
}
