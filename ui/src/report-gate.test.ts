import { describe, expect, it } from "vitest";
import type { CycleListItem, ReportDefaults } from "./ipc/types";
import {
  EMPTY_FORM,
  formFromDefaults,
  gateMessage,
  initialCycleId,
  toReportChoice,
  usableLast,
  validateGate,
} from "./report-gate";

const defaults = (patch: Partial<ReportDefaults> = {}): ReportDefaults => ({
  cycleId: "NOMAD",
  defaultMode: "fixed",
  defaultTemplate: "default",
  defaultMeetingType: "budget",
  outputLanguage: "auto",
  templates: [
    { name: "default", sections: [] },
    { name: "budget-review", sections: [] },
  ],
  meetingTypes: [
    { name: "budget", template: "budget-review" },
    { name: "technical", template: "default" },
  ],
  last: null,
  ...patch,
});

const cycle = (id: string, valid = true): CycleListItem => ({
  id,
  name: id,
  participantsCount: 1,
  defaultMode: "fixed",
  valid,
  issues: [],
  repoId: "local",
  stateAsOf: null,
  offline: false,
});

describe("formFromDefaults", () => {
  it("starts from the cycle's defaults", () => {
    expect(formFromDefaults(defaults())).toEqual({
      cycleId: "NOMAD",
      mode: "fixed",
      template: "default",
      meetingType: "budget",
      outputLanguage: "",
    });
  });

  it("prefers the last choice made for this cycle", () => {
    const form = formFromDefaults(
      defaults({ last: { cycleId: "NOMAD", mode: "template", template: "budget-review" } }),
    );
    expect(form).toMatchObject({ mode: "template", template: "budget-review" });
  });
});

describe("validateGate", () => {
  const form = formFromDefaults(defaults());

  it("needs a cycle, loaded defaults and a mode", () => {
    expect(validateGate(EMPTY_FORM, null)).toBe("Choose a project.");
    expect(validateGate(form, null)).toBe("Loading the project…");
    expect(validateGate(form, defaults({ cycleId: "OTHER" }))).toBe("Loading the project…");
    expect(validateGate({ ...form, mode: "" }, defaults())).toBe("Choose a report mode.");
  });

  it("passes fixed and auto without further choices", () => {
    expect(validateGate(form, defaults())).toBeNull();
    expect(validateGate({ ...form, mode: "auto" }, defaults())).toBeNull();
  });

  it("needs a template of the cycle in template mode", () => {
    expect(validateGate({ ...form, mode: "template", template: "" }, defaults())).toBe(
      "Choose a template.",
    );
    expect(validateGate({ ...form, mode: "template", template: "nope" }, defaults())).toMatch(
      /no template named nope/,
    );
    expect(
      validateGate({ ...form, mode: "template", template: "budget-review" }, defaults()),
    ).toBeNull();
  });

  it("needs a meeting type the cycle defines in by-type mode", () => {
    expect(validateGate({ ...form, mode: "by-type", meetingType: "" }, defaults())).toBe(
      "Choose the meeting type.",
    );
    expect(validateGate({ ...form, mode: "by-type", meetingType: "casual" }, defaults())).toMatch(
      /does not define the meeting type casual/,
    );
    expect(
      validateGate({ ...form, mode: "by-type", meetingType: "budget" }, defaults()),
    ).toBeNull();
    expect(validateGate({ ...form, mode: "by-type" }, defaults({ meetingTypes: [] }))).toMatch(
      /defines no meeting types/,
    );
  });
});

describe("toReportChoice", () => {
  const form = formFromDefaults(defaults());

  it("sends only what the mode uses", () => {
    expect(toReportChoice(form)).toEqual({ cycleId: "NOMAD", mode: "fixed" });
    expect(toReportChoice({ ...form, mode: "template", template: "budget-review" })).toEqual({
      cycleId: "NOMAD",
      mode: "template",
      template: "budget-review",
    });
    expect(toReportChoice({ ...form, mode: "by-type", meetingType: "budget" })).toEqual({
      cycleId: "NOMAD",
      mode: "by-type",
      meetingType: "budget",
    });
  });

  it("sends the language override only when one was chosen", () => {
    expect(toReportChoice({ ...form, outputLanguage: "pl" })).toMatchObject({
      outputLanguage: "pl",
    });
    expect(toReportChoice(form)).not.toHaveProperty("outputLanguage");
  });

  it("refuses a form without a mode", () => {
    expect(() => toReportChoice({ ...form, mode: "" })).toThrow();
  });
});

describe("the last choice", () => {
  it("is only usable while its cycle exists and is valid", () => {
    const last = { cycleId: "NOMAD", mode: "fixed" as const };
    expect(usableLast(last, [cycle("NOMAD")])).toBe(last);
    expect(usableLast(last, [cycle("NOMAD", false)])).toBeNull();
    expect(usableLast(last, [cycle("OTHER")])).toBeNull();
    expect(usableLast(null, [cycle("NOMAD")])).toBeNull();
  });

  it("starts the gate with the last cycle, else the only valid one, else none", () => {
    const last = { cycleId: "B", mode: "fixed" as const };
    expect(initialCycleId(last, [cycle("A"), cycle("B")])).toBe("B");
    expect(initialCycleId(null, [cycle("A")])).toBe("A");
    expect(initialCycleId(null, [cycle("A"), cycle("B")])).toBe("");
    expect(initialCycleId(null, [cycle("A", false)])).toBe("");
  });
});

describe("gateMessage", () => {
  it("makes the refusal of the backend readable", () => {
    expect(gateMessage("report_mode_required", "choose a report mode")).toBe(
      "The recording was not started: choose a report mode.",
    );
    expect(gateMessage("report_mode_required", "choose a report mode.")).toBe(
      "The recording was not started: choose a report mode.",
    );
    expect(gateMessage("cycle_invalid", "x")).toMatch(/until it is fixed/);
    expect(gateMessage("io", "disk full")).toBe("disk full");
  });
});
