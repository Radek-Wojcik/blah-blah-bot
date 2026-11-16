import { describe, expect, it } from "vitest";
import type { Backend } from "./backend";
import { demoCycle, validateCycle } from "./mock-cycles";
import { addItem, demoDraft, moveItem, setSections, updateItem } from "./mock-draft";
import {
  DEFAULT_TEMPLATE_TEXT,
  itemLine,
  renderReport,
  tokenize,
  validateTemplateDto,
} from "./mock-render";
import { createMockBackend } from "./mock";
import { demoTranscript } from "./mock-demo";
import type { CycleDto, DraftItem, DraftView, TemplateDto } from "./types";

/** The code of the error a promise rejects with. */
async function codeOf(p: Promise<unknown>): Promise<string | undefined> {
  try {
    await p;
  } catch (e) {
    return (e as { code?: string }).code;
  }
  return undefined;
}

const backend = (): Backend => createMockBackend({ clockRate: 0 });

describe("report gate", () => {
  it("refuses to start without a cycle and a mode", async () => {
    const b = backend();
    expect(await codeOf(b.startRecording({ sourceIds: ["mic:default"] }))).toBe(
      "report_mode_required",
    );
    expect(
      await codeOf(
        b.startRecording({ sourceIds: ["mic:default"], report: { cycleId: "", mode: "fixed" } }),
      ),
    ).toBe("report_mode_required");
    expect(
      await codeOf(
        b.startRecording({
          sourceIds: ["mic:default"],
          report: { cycleId: "NOMAD", mode: "" as never },
        }),
      ),
    ).toBe("report_mode_required");
    expect((await b.recordingState()).status).toBe("idle");
  });

  it("checks the template, the meeting type, the language and the cycle", async () => {
    const b = backend();
    const start = (report: Parameters<Backend["startRecording"]>[0]["report"]) =>
      codeOf(b.startRecording({ sourceIds: ["mic:default"], report }));
    expect(await start({ cycleId: "NOMAD", mode: "template", template: "nope" })).toBe(
      "report_mode_required",
    );
    expect(await start({ cycleId: "NOMAD", mode: "by-type" })).toBe("report_mode_required");
    expect(await start({ cycleId: "NOMAD", mode: "by-type", meetingType: "weekly" })).toBe(
      "report_mode_required",
    );
    expect(await start({ cycleId: "NOMAD", mode: "fixed", outputLanguage: "de" as never })).toBe(
      "report_mode_required",
    );
    expect(await start({ cycleId: "GHOST", mode: "fixed" })).toBe("report_mode_required");
    // A cycle with issues cannot be used.
    expect(await start({ cycleId: "OLDPROJ", mode: "fixed" })).toBe("report_mode_required");
    expect((await b.recordingState()).status).toBe("idle");
  });

  it("starts with a mode and remembers it as the last report", async () => {
    const b = backend();
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "by-type", meetingType: "budget" },
    });
    expect((await b.recordingState()).status).toBe("recording");
    expect((await b.getSettings()).lastReport).toEqual({
      cycleId: "NOMAD",
      mode: "by-type",
      template: null,
      meetingType: "budget",
    });
    const defaults = await b.reportDefaults("NOMAD");
    expect(defaults.last?.mode).toBe("by-type");
  });

  it("uses the last report only when asked to, and still checks it", async () => {
    const b = backend();
    // The demo data has a last report (NOMAD, fixed).
    await b.startRecording({ sourceIds: ["mic:default"], useLastReport: true });
    expect((await b.recordingState()).status).toBe("recording");
  });

  it("reports the defaults of a cycle and refuses an invalid one", async () => {
    const b = backend();
    const d = await b.reportDefaults("NOMAD");
    expect(d.defaultMode).toBe("fixed");
    expect(d.templates.map((t) => t.name)).toEqual(["budget-review", "default"]);
    expect(d.meetingTypes.map((t) => t.name)).toEqual(["budget", "casual", "technical"]);
    expect(await codeOf(b.reportDefaults("OLDPROJ"))).toBe("cycle_invalid");
    expect(await codeOf(b.reportDefaults("GHOST"))).toBe("not_found");
  });
});

describe("cycles", () => {
  it("lists the demo cycles, the broken one with its issues", async () => {
    const list = await backend().listCycles();
    expect(list.map((c) => [c.id, c.valid])).toEqual([
      ["NOMAD", true],
      ["OLDPROJ", false],
      ["SANDBOX", true],
    ]);
    const broken = list.find((c) => c.id === "OLDPROJ")!;
    expect(broken.issues[0].path).toBe("participants[1].id");
    expect(list[0].stateAsOf).toBe("2026-11-12");
  });

  it("creates a cycle with the default template and refuses a bad or repeated id", async () => {
    const b = backend();
    const created = await b.createCycle("ROVER-2", "Rover two");
    expect(created.valid).toBe(true);
    expect(created.templates.map((t) => t.name)).toEqual(["default"]);
    expect(created.repoId).toBe("local");
    expect(await codeOf(b.createCycle("ROVER-2", "Again"))).toBe("duplicate");
    expect(await codeOf(b.createCycle("bad id", "x"))).toBe("invalid");
    expect(await codeOf(b.createCycle("OK", "  "))).toBe("invalid");
  });

  it("does not write a cycle with problems and says where they are", async () => {
    const b = backend();
    const cycle = (await b.getCycle("NOMAD")).cycle;
    const bad: CycleDto = structuredClone(cycle);
    bad.participants.push({ id: "alex", name: "Another Alex" }, { id: "", name: "No id" });
    const result = await b.saveCycle(bad);
    expect(result.saved).toBe(false);
    expect(result.details).toBeNull();
    const paths = result.issues.map((i) => i.path);
    expect(paths).toContain("participants[4].id");
    expect(paths).toContain("participants[5].id");
    expect(result.issues.find((i) => i.path === "participants[4].id")?.message).toMatch(
      /duplicate/,
    );
    // Nothing was stored.
    expect((await b.getCycle("NOMAD")).cycle.participants).toHaveLength(4);
  });

  it("saves a valid change and reports the new details", async () => {
    const b = backend();
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.name = "  Nomad rover 2  ";
    cycle.participants.push({ id: "mira", name: "Mira Demo", github: "" });
    const result = await b.saveCycle(cycle);
    expect(result.saved).toBe(true);
    expect(result.details?.cycle.name).toBe("Nomad rover 2");
    expect(result.details?.cycle.participants[4]).toEqual({ id: "mira", name: "Mira Demo" });
    expect(await codeOf(b.saveCycle({ ...cycle, id: "GHOST" }))).toBe("not_found");
  });

  it("checks the targets and the templates the report refers to", () => {
    const base = demoCycle();
    expect(validateCycle(base, ["default", "budget-review"])).toEqual([]);
    const bad = demoCycle();
    bad.targets.discord = { webhook: "https://discord.example/hook", publish: "off" };
    bad.report.byType.weekly = "missing";
    bad.report.defaultMode = "template";
    bad.report.defaultTemplate = "nope";
    const paths = validateCycle(bad, ["default", "budget-review"]).map((i) => i.path);
    expect(paths).toEqual(
      expect.arrayContaining([
        "targets.discord.webhook",
        "report.by_type.weekly",
        "report.default_template",
      ]),
    );
  });

  it("clones and pulls repositories, and shows an offline clone", async () => {
    const b = backend();
    expect(await codeOf(b.addCycleRepo({ kind: "clone", url: "" }))).toBe("invalid");
    expect(
      await codeOf(b.addCycleRepo({ kind: "clone", url: "https://me:secret@host.example/x.git" })),
    ).toBe("invalid");
    expect(await codeOf(b.addCycleRepo({ kind: "local", path: "C:\\missing" }))).toBe("invalid");
    const repo = await b.addCycleRepo({
      kind: "clone",
      url: "https://git.example.org/lab/lab-cycles.git",
    });
    expect(repo).toMatchObject({ id: "lab-cycles", kind: "clone", offline: false });
    expect((await b.listCycles()).some((c) => c.repoId === "lab-cycles")).toBe(true);

    expect((await b.pullCycleRepo("team-cycles")).outcome).toBe("updated");
    expect((await b.pullCycleRepo("team-cycles")).outcome).toBe("upToDate");
    expect((await b.pullCycleRepo("local")).outcome).toBe("noRemote");
    const off = await b.addCycleRepo({ kind: "clone", url: "https://offline.example/o.git" });
    const pulled = await b.pullCycleRepo(off.id);
    expect(pulled.outcome).toBe("failed");
    expect(pulled.repo.offline).toBe(true);
    expect((await b.listCycles()).find((c) => c.repoId === off.id)?.offline).toBe(true);
  });
});

describe("templates", () => {
  const template = (body: string, sections: TemplateDto["sections"] = []): TemplateDto => ({
    name: "weekly",
    sections,
    body,
  });

  it("rejects an unknown placeholder and says on which line", async () => {
    const b = backend();
    const check = await b.validateTemplate(template("# {title}\n\n{nonsense}\n"));
    expect(check.valid).toBe(false);
    expect(check.issues[0]).toEqual({
      path: "body",
      message: "line 3: unknown placeholder {nonsense} (use {{ and }} for literal braces)",
    });
    expect(check.placeholders).toContain("sections");
  });

  it("accepts literal braces and every documented placeholder", () => {
    const body =
      "{title} {date} {participants} {summary} {summary_section} {sections} {tasks} {decisions} {topics} {notes} {ideas} {budgets} {transcript_link} {report_link} {{literal}}";
    expect(validateTemplateDto(template(body))).toEqual([]);
    expect(tokenize("a {{ b }} c").tokens).toEqual([{ text: "a { b } c" }]);
  });

  it("checks the sections of the front matter and the references to them", () => {
    const sections = [
      { id: "Bad Id", title: "", description: "" },
      { id: "ok", title: "Ok", description: "" },
      { id: "ok", title: "Again", description: "" },
    ];
    const paths = validateTemplateDto(template("{section:missing}", sections)).map((i) => i.path);
    expect(paths).toEqual(
      expect.arrayContaining(["sections[0].id", "sections[0].title", "sections[2].id"]),
    );
    const refs = validateTemplateDto(template("{section:missing}", [sections[1]]));
    expect(refs[0].message).toMatch(/does not declare/);
    expect(validateTemplateDto({ ...template("x"), name: "bad name" })[0].path).toBe("name");
  });

  it("is saved only when valid, and then listed", async () => {
    const b = backend();
    const refused = await b.saveTemplate("NOMAD", template("{oops}"));
    expect(refused).toMatchObject({ saved: false, valid: false });
    const saved = await b.saveTemplate(
      "NOMAD",
      template("# {title}\n{section:a}\n", [{ id: "a", title: "A", description: "" }]),
    );
    expect(saved).toMatchObject({ saved: true, valid: true });
    expect(
      (await b.listTemplates("NOMAD")).find((t) => t.name === "weekly")?.sections,
    ).toHaveLength(1);
    expect((await b.getTemplate("NOMAD", "weekly")).body).toContain("{section:a}");
    expect(await codeOf(b.getTemplate("NOMAD", "ghost"))).toBe("not_found");
  });
});

describe("report rendering", () => {
  const people = demoCycle().participants;

  it("writes item lines as docs/CYCLES.md describes them", () => {
    const d = demoDraft("m5");
    const line = (id: string, variant: "markdown" | "chat" = "markdown") =>
      itemLine(
        d.items.find((i) => i.id === id)!,
        people,
        variant,
      );
    // Stated assignee, inferred due date.
    expect(line("i03")).toBe(
      "- [ ] Update the ground station schedule for Thursday. (@Jan Demo, due 2026-11-19?)",
    );
    expect(line("i03", "chat")).toBe(
      "- Update the ground station schedule for Thursday. (@Jan Demo, due 2026-11-19?)",
    );
    // Unknown assignee and due date.
    expect(line("i04")).toBe("- [ ] Share the budget sheet before the review. (@?, due ?)");
    // Inferred assignee, unknown due date, and an unverified quote.
    expect(line("i11")).toBe(
      "- [ ] Prepare the filter for the ignition ripple. (@Kasia Demo?, due ?) (unverified quote)",
    );
    // Only an unverified quote is marked; an item added by hand is not.
    expect(line("i05")).toBe("- Buy a second test stand with the underspend. (unverified quote)");
    expect(line("i12")).toBe("- Record the next test with a high-speed camera.");
  });

  it("renders the sections of the draft under the default template", async () => {
    const view = await backend().getDraft("m5");
    expect(view.reportMarkdown).toMatch(
      /^# Meeting NOMAD 2026-11-13\n\n2026-11-13 · Alex Demo, Jan Demo, Kasia Demo, Piotr Demo\n\n## Summary\n\n/,
    );
    expect(view.reportMarkdown).toContain("## Thruster tests\n\n- The last thruster firing");
    expect(view.reportMarkdown).toContain("## Budget and procurement");
    expect(view.reportMarkdown.endsWith("---\n[Full transcript](transcript.md)\n")).toBe(true);
    expect(view.reportWarning).toBeNull();
  });

  it("uses the built-in sections, in the report language, in fixed mode", () => {
    const d = demoDraft("m5");
    d.mode = "fixed";
    d.sections = [];
    d.language = "pl";
    const text = renderReport(d, people, "{sections}", "pl", "markdown");
    expect(text.indexOf("## Decyzje")).toBeLessThan(text.indexOf("## Zadania"));
    expect(text).toContain("## Budżety");
    expect(text).not.toContain("## Thruster tests");
  });

  // The same draft and templates as the Rust tests of `crates/cycle-store/src/render.rs`
  // (`fixed_draft`), so the expected texts below are the ones the real renderer gives.
  const parityDraft = (language: "pl" | "en") => {
    const d = demoDraft("m5");
    const item = (id: string, type: DraftItem["type"], text: string): DraftItem => ({
      id,
      type,
      text,
      assignee: null,
      due: null,
      certainty: { assignee: "unknown", due: "unknown" },
      section: null,
      relatesTo: null,
      source: null,
      edited: false,
    });
    d.mode = "fixed";
    d.sections = [];
    d.language = language;
    d.date = "2026-11-13";
    d.summary = "We agreed on the mount.";
    d.items = [
      item("n1", "note", "Wheels are 20 cm"),
      {
        ...item("t1", "task", "Design the mount"),
        assignee: "alex",
        due: "2026-11-20",
        certainty: { assignee: "stated", due: "stated" },
      },
      item("d1", "decision", "Use aluminium"),
      item("i1", "idea", "Add a camera"),
      item("b1", "budget", "300 PLN for sensors"),
      item("o1", "topic", "Battery supplier"),
    ];
    return d;
  };
  const parityPeople = [
    { id: "alex", name: "Alex Example" },
    { id: "sam", name: "Sam Sample" },
  ];
  const parityBody = DEFAULT_TEMPLATE_TEXT.replace("# {title}\n\n", "");

  it("gives the text of the Rust renderer for the default template, in English", () => {
    expect(renderReport(parityDraft("en"), parityPeople, parityBody, "en", "markdown")).toBe(
      "2026-11-13 · Alex Example, Sam Sample\n\n## Summary\n\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n## Tasks\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n## Open topics\n\n- Battery supplier\n\n## Notes\n\n- Wheels are 20 cm\n\n## Ideas\n\n- Add a camera\n\n## Budgets\n\n- 300 PLN for sensors\n\n---\n[Full transcript](transcript.md)\n",
    );
  });

  it("gives the text of the Rust renderer for the default template, in Polish", () => {
    const text = renderReport(parityDraft("pl"), parityPeople, parityBody, "pl", "markdown");
    expect(text).toBe(
      "2026-11-13 · Alex Example, Sam Sample\n\n## Podsumowanie\n\nWe agreed on the mount.\n\n## Decyzje\n\n- Use aluminium\n\n## Zadania\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n## Otwarte tematy\n\n- Battery supplier\n\n## Notatki\n\n- Wheels are 20 cm\n\n## Pomysły\n\n- Add a camera\n\n## Budżety\n\n- 300 PLN for sensors\n\n---\n[Full transcript](transcript.md)\n",
    );
    expect(text).not.toMatch(/## (Summary|Decisions|Tasks|Open topics|Notes|Ideas|Budgets)/);
  });

  it("leaves no summary heading for an empty summary and keeps {summary} bare", () => {
    const d = parityDraft("pl");
    expect(renderReport(d, parityPeople, "{summary_section}", "pl", "markdown")).toBe(
      "## Podsumowanie\n\nWe agreed on the mount.\n",
    );
    d.summary = "  \n ";
    expect(renderReport(d, parityPeople, "{summary_section}", "pl", "markdown")).toBe("");
    expect(
      renderReport(d, parityPeople, "# T\n\n{summary_section}\n\n{tasks}\n", "pl", "markdown"),
    ).toBe("# T\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n");
    d.summary = "Ustalono uchwyt.";
    expect(
      renderReport(d, parityPeople, "# T\n\n## Streszczenie\n{summary}\n", "pl", "markdown"),
    ).toBe("# T\n\n## Streszczenie\n\nUstalono uchwyt.\n");
  });

  it("separates blocks by exactly one blank line and neutralises mass mentions", () => {
    const d = parityDraft("en");
    d.summary = "First.  \n\n\n\nSecond.   @everyone";
    const body = "\n\n# T   \n\n\n\n{summary_section}\n\n\n\n{tasks}\n\n\n---\n[x](y)   \n\n\n";
    const text = renderReport(d, parityPeople, body, "en", "markdown");
    expect(text).toBe(
      "# T\n\n## Summary\n\nFirst.\n\nSecond.   @\u200beveryone\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n---\n[x](y)\n",
    );
    expect(renderReport(d, parityPeople, "# A\n## B\ntext\n", "en", "markdown")).toBe(
      "# A\n\n## B\n\ntext\n",
    );
  });

  it("drops headings with nothing under them and refuses an unknown placeholder", () => {
    const d = demoDraft("m5");
    d.items = [];
    const body = "# T\n\n## Empty\n\n{tasks}\n\n## Next\n\ntext\n";
    expect(renderReport(d, people, body, "en", "markdown")).toBe("# T\n\n## Next\n\ntext\n");
    expect(() => renderReport(d, people, "{nope}", "en", "markdown")).toThrow(
      /unknown placeholder/,
    );
  });
});

describe("draft operations", () => {
  const view = (b: Backend) => b.getDraft("m5");
  const find = (v: DraftView, id: string) => v.draft.items.find((i) => i.id === id)!;

  it("serves the demo draft: twelve items, markers, sources and warnings", async () => {
    const v = await view(backend());
    expect(v.draft.items).toHaveLength(12);
    expect(new Set(v.draft.items.map((i) => i.type))).toEqual(
      new Set(["task", "decision", "topic", "note", "idea", "budget"]),
    );
    expect(v.draft.items.filter((i) => i.source && !i.source.verified)).toHaveLength(2);
    expect(v.draft.items.filter((i) => i.edited).map((i) => i.id)).toEqual(["i12"]);
    expect(v.draft.warnings.map((w) => w.code)).toContain("window_skipped");
    expect(v.participants.map((p) => p.id)).toEqual(["alex", "jan", "kasia", "piotr"]);
    expect(v.transcriptPresent).toBe(true);
    expect(v.hasNewDraft).toBe(false);
  });

  it("answers no_draft for a meeting without one", async () => {
    expect(await codeOf(backend().getDraft("m4"))).toBe("no_draft");
  });

  it("edits the text, assignee and due date, marks the item edited and the value stated", async () => {
    const b = backend();
    let v = await b.updateDraftItem("m5", "i11", { text: "  Prepare the ripple filter  " });
    expect(find(v, "i11")).toMatchObject({ text: "Prepare the ripple filter", edited: true });
    v = await b.updateDraftItem("m5", "i11", { assignee: "jan", due: "2026-11-30" });
    expect(find(v, "i11")).toMatchObject({
      assignee: "jan",
      due: "2026-11-30",
      certainty: { assignee: "stated", due: "stated" },
    });
    v = await b.updateDraftItem("m5", "i03", { due: null });
    expect(find(v, "i03")).toMatchObject({
      due: null,
      certainty: { assignee: "stated", due: "unknown" },
    });
    expect(v.reportMarkdown).toContain(
      "Update the ground station schedule for Thursday. (@Jan Demo, due ?)",
    );
  });

  it("refuses an empty text, a bad date, an unknown item and a stranger as assignee", async () => {
    const b = backend();
    expect(await codeOf(b.updateDraftItem("m5", "i01", { text: "  " }))).toBe("invalid");
    expect(await codeOf(b.updateDraftItem("m5", "i03", { due: "20.11.2026" }))).toBe("invalid");
    expect(await codeOf(b.updateDraftItem("m5", "nope", { text: "x" }))).toBe("not_found");
    expect(await codeOf(b.updateDraftItem("m5", "i03", { assignee: "stranger" }))).toBe("invalid");
    // Nothing was changed by the refusals.
    expect(find(await view(b), "i01").edited).toBe(false);
  });

  it("moves an item to another section, last or before a neighbour, and marks it edited", async () => {
    const b = backend();
    let v = await b.moveDraftItem("m5", "i01", "budget");
    expect(find(v, "i01")).toMatchObject({ section: "budget", edited: true });
    expect(v.draft.items[v.draft.items.length - 1].id).toBe("i01");
    v = await b.moveDraftItem("m5", "i02", "budget", "i04");
    const order = v.draft.items.filter((i) => i.section === "budget").map((i) => i.id);
    expect(order.indexOf("i02")).toBeLessThan(order.indexOf("i04"));
    expect(v.reportMarkdown).toMatch(/## Budget and procurement[\s\S]*The last thruster firing/);
  });

  it("refuses a move to a section that does not exist or in front of an item elsewhere", async () => {
    const b = backend();
    expect(await codeOf(b.moveDraftItem("m5", "i01", "ghost"))).toBe("not_found");
    expect(await codeOf(b.moveDraftItem("m5", "i01", "budget", "i03"))).toBe("invalid");
    expect(await codeOf(b.moveDraftItem("m5", "i01", "thruster-tests", "i01"))).toBe("invalid");
  });

  it("deletes and adds items; an added item has no source and counts as edited", async () => {
    const b = backend();
    let v = await b.deleteDraftItem("m5", "i10");
    expect(v.draft.items).toHaveLength(11);
    v = await b.addDraftItem("m5", {
      type: "task",
      text: " Book the test range ",
      assignee: "alex",
      due: "2026-12-01",
      section: "follow-ups",
    });
    const added = v.draft.items[v.draft.items.length - 1];
    expect(added).toMatchObject({
      id: "u001",
      text: "Book the test range",
      source: null,
      edited: true,
      certainty: { assignee: "stated", due: "stated" },
    });
    expect(await codeOf(b.addDraftItem("m5", { type: "note", text: " " }))).toBe("invalid");
    expect(await codeOf(b.addDraftItem("m5", { type: "note", text: "x", section: "ghost" }))).toBe(
      "not_found",
    );
  });

  it("renames, adds, removes and reorders sections; a removed section orphans its items", async () => {
    const b = backend();
    const v0 = await view(b);
    const [first, second, third, fourth] = v0.draft.sections;
    const v = await b.setDraftSections("m5", [
      { ...fourth, title: "Next steps" },
      first,
      { id: "", title: "Open questions", description: "" },
      second,
    ]);
    expect(v.draft.sections.map((s) => s.id)).toEqual([
      "follow-ups",
      "thruster-tests",
      "open-questions",
      "ground-segment",
    ]);
    expect(v.draft.sections[0].title).toBe("Next steps");
    // The removed "budget" section's four items lost their section.
    expect(
      v.draft.items
        .filter((i) => i.section === null)
        .map((i) => i.id)
        .sort(),
    ).toEqual(["i04", "i05", "i06", "i10"]);
    expect(third.id).toBe("budget");
    expect(await codeOf(b.setDraftSections("m5", [{ id: "a", title: " ", description: "" }]))).toBe(
      "invalid",
    );
    expect(
      await codeOf(
        b.setDraftSections("m5", [
          { id: "a", title: "A", description: "" },
          { id: "a", title: "B", description: "" },
        ]),
      ),
    ).toBe("invalid");
  });

  it("makes an id from a title that is not used yet", () => {
    const d = demoDraft("m5");
    setSections(d, [...d.sections, { id: "", title: "Budget", description: "" }]);
    expect(d.sections[4].id).toBe("budget-2");
    setSections(d, [{ id: "", title: "2nd round!", description: "" }]);
    expect(d.sections[0].id).toBe("section-2nd-round");
  });

  it("changes the layout only, and refuses a template or type the cycle does not have", async () => {
    const b = backend();
    let v = await b.setReportMode("m5", "fixed");
    expect(v.draft.mode).toBe("fixed");
    expect(v.draft.items.every((i) => !i.edited || i.id === "i12")).toBe(true);
    expect(v.reportMarkdown).toContain("## Decisions");
    v = await b.setReportMode("m5", "template", "budget-review");
    expect(v.draft).toMatchObject({ mode: "template", template: "budget-review" });
    v = await b.setReportMode("m5", "by-type", undefined, "budget");
    expect(v.draft).toMatchObject({
      mode: "by-type",
      meetingType: "budget",
      template: "budget-review",
    });
    expect(await codeOf(b.setReportMode("m5", "template", "ghost"))).toBe("report_mode_required");
    expect(await codeOf(b.setReportMode("m5", "by-type"))).toBe("report_mode_required");
    expect(await codeOf(b.setReportMode("m5", "by-type", undefined, "weekly"))).toBe(
      "report_mode_required",
    );
  });

  it("renders the chat variant without checkboxes", async () => {
    const b = backend();
    expect(await b.renderReport("m5", "markdown")).toContain("- [ ] ");
    expect(await b.renderReport("m5", "chat")).not.toContain("[ ]");
    expect(await codeOf(b.renderReport("m5", "html" as never))).toBe("invalid");
  });

  it("tells subscribers which draft changed", async () => {
    const b = backend();
    const seen: string[] = [];
    const off = await b.onDraftChanged((id) => seen.push(id));
    await b.deleteDraftItem("m5", "i10");
    off();
    await b.deleteDraftItem("m5", "i09");
    expect(seen).toEqual(["m5"]);
  });

  it("keeps the edited draft until a newer one is accepted or discarded", async () => {
    const edited = createMockBackend({ clockRate: 0, newDraft: true });
    let v = await edited.getDraft("m5");
    expect(v.hasNewDraft).toBe(true);
    v = await edited.discardNewDraft("m5");
    expect(v.hasNewDraft).toBe(false);
    expect(find(v, "i12")).toBeDefined();
    expect(await codeOf(edited.acceptNewDraft("m5"))).toBe("no_new_draft");

    const b = createMockBackend({ clockRate: 0, newDraft: true });
    v = await b.acceptNewDraft("m5");
    expect(v.hasNewDraft).toBe(false);
    expect(v.draft.items.some((i) => i.id === "i13")).toBe(true);
    expect(v.draft.items.some((i) => i.id === "i12")).toBe(false);
  });

  it("applies the pure operations to a copy only when they succeed", () => {
    const d = demoDraft("m5");
    expect(() => updateItem(d, "i01", { text: "" })).toThrow();
    expect(d.items[0].edited).toBe(false);
    moveItem(d, "i01", null);
    expect(d.items[d.items.length - 1]).toMatchObject({ id: "i01", section: null, edited: true });
    expect(addItem(d, { type: "note", text: "n" })).toBe("u001");
    expect(addItem(d, { type: "note", text: "m" })).toBe("u002");
  });
});

describe("cloud confirmation", () => {
  it("shows the endpoint host and the amount of text, and confirms nothing by itself", async () => {
    const b = backend();
    const p = await b.cloudPreview("m5");
    expect(p).toMatchObject({
      endpointHost: "api.llm-vendor.example",
      model: "large-model-1",
      includesAudio: false,
      includesImages: false,
      transcriptReady: true,
      screenshotDescriptions: 2,
      confirmed: false,
    });
    // The excluded passage is left out of the count.
    const words = demoTranscript("m5")
      .segments.filter((x) => !x.excluded)
      .reduce((n, x) => n + x.text.split(" ").length, 0);
    expect(p.wordCount).toBe(words);
    expect(p.summaryText).toContain("No audio and no images are sent.");
    expect(p.summaryText).toContain("api.llm-vendor.example");
    expect((await b.cloudPreview("m5")).confirmed).toBe(false);
  });

  it("counts the items of earlier meetings of the cycle that go along", async () => {
    const b = backend();
    const p = await b.cloudPreview("m5");
    // The only draft of the cycle is the meeting's own: nothing from earlier meetings.
    expect(p.previousItemCount).toBe(0);
    expect(p.summaryText).toContain("no items");
    expect(p.summaryText).toContain("earlier meetings of the cycle");
    expect(p.summaryText).toContain("processed only locally");
    // A second meeting of the same cycle whose draft exists: m5's items now count for it.
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "fixed" },
    });
    await b.stopRecording();
    const id = (await b.listMeetings())[0].id;
    await b.processMeeting(id, {});
    const next = await b.cloudPreview(id);
    expect(next.previousItemCount).toBeGreaterThan(0);
    expect(next.previousItemCount).toBeLessThanOrEqual(30);
    expect(next.summaryText).toContain(`${next.previousItemCount} item`);
  });

  it("says the transcript is not ready for a meeting that has none", async () => {
    const p = await backend().cloudPreview("m1");
    expect(p).toMatchObject({ transcriptReady: false, wordCount: 0, screenshotDescriptions: 0 });
    expect(p.summaryText).toMatch(/once the meeting is transcribed/);
    expect(await codeOf(backend().cloudPreview("nope"))).toBe("not_found");
  });

  it("runs the cloud only after the meeting was confirmed", async () => {
    const b = createMockBackend({ clockRate: 0, jobStepMs: 40 });
    // Use a meeting of the cycle that has a done job and skipped stages: record one first.
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "fixed" },
    });
    await b.stopRecording();
    const id = (await b.listMeetings())[0].id;
    expect(await codeOf(b.processMeeting(id, { llm: { kind: "cloud" } }))).toBe(
      "cloud_not_confirmed",
    );
    expect((await b.queueState()).jobs.some((j) => j.meetingId === id)).toBe(false);
    await b.confirmCloud(id);
    expect((await b.cloudPreview(id)).confirmed).toBe(true);
    await b.processMeeting(id, { llm: { kind: "cloud" } });
    expect((await b.queueState()).jobs.some((j) => j.meetingId === id)).toBe(true);
  });

  it("needs the key, and a confirmation belongs to the endpoint it was given for", async () => {
    const b = backend();
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "fixed" },
    });
    await b.stopRecording();
    const id = (await b.listMeetings())[0].id;
    await b.confirmCloud(id);
    await b.clearCloudKey();
    expect(await codeOf(b.processMeeting(id, { llm: { kind: "cloud" } }))).toBe(
      "cloud_key_missing",
    );
    await b.setCloudKey("test-key");
    const settings = await b.getProcessingSettings();
    await b.saveProcessingSettings({
      ...settings,
      llm: { ...settings.llm!, cloud: { baseUrl: "https://other.example/v1", model: "m" } },
    });
    expect(await codeOf(b.processMeeting(id, { llm: { kind: "cloud" } }))).toBe(
      "cloud_not_confirmed",
    );
  });
});

describe("language model settings and keys", () => {
  it("never returns a key, only whether one is saved", async () => {
    const b = backend();
    let s = await b.getProcessingSettings();
    expect(s.llm).toMatchObject({
      defaultKind: "managed",
      externalKeySet: false,
      cloudKeySet: true,
    });
    expect(JSON.stringify(s)).not.toMatch(/secret-value/);
    await b.setExternalKey("secret-value");
    expect(await b.externalKeyPresent()).toBe(true);
    await b.clearCloudKey();
    expect(await b.cloudKeyPresent()).toBe(false);
    s = await b.getProcessingSettings();
    expect(s.llm).toMatchObject({ externalKeySet: true, cloudKeySet: false });
    expect(JSON.stringify(s)).not.toMatch(/secret-value/);
    expect(await codeOf(b.setCloudKey("  "))).toBeDefined();
  });

  it("warns when the external default server is not on this machine or network", async () => {
    const b = backend();
    const s = await b.getProcessingSettings();
    expect(s.llm?.externalHostIsRemote).toBe(false);
    const save = (patch: Partial<NonNullable<typeof s.llm>>) =>
      b.saveProcessingSettings({ ...s, llm: { ...s.llm!, ...patch } });
    await save({ defaultKind: "external" });
    expect((await b.getProcessingSettings()).llm?.externalHostIsRemote).toBe(false);
    await save({
      defaultKind: "external",
      external: { baseUrl: "https://api.public-vendor.example/v1", model: "m" },
    });
    expect((await b.getProcessingSettings()).llm?.externalHostIsRemote).toBe(true);
    // Not the default any more: no warning.
    await save({
      defaultKind: "managed",
      external: { baseUrl: "https://api.public-vendor.example/v1", model: "m" },
    });
    expect((await b.getProcessingSettings()).llm?.externalHostIsRemote).toBe(false);
    // Plain http is not accepted for the cloud, except for this machine.
    expect(
      await codeOf(save({ cloud: { baseUrl: "http://api.public-vendor.example/v1", model: "m" } })),
    ).toBe("invalid");
  });

  it("keeps the key flags when the settings are saved and validates the rest", async () => {
    const b = backend();
    const s = await b.getProcessingSettings();
    await b.saveProcessingSettings({
      ...s,
      llm: { ...s.llm!, cloudKeySet: false, managed: { modelPath: "m.gguf", ctx: 8192 } },
    });
    const after = await b.getProcessingSettings();
    expect(after.llm?.managed.ctx).toBe(8192);
    expect(after.llm?.cloudKeySet).toBe(true);
    expect(
      await codeOf(
        b.saveProcessingSettings({
          ...s,
          llm: { ...s.llm!, managed: { modelPath: "m", ctx: 100 } },
        }),
      ),
    ).toBe("invalid");
  });

  it("saves the repository list and the last report only through their own commands", async () => {
    const b = backend();
    const s = await b.getSettings();
    expect(s.cycleRepos?.map((r) => r.id)).toEqual(["local", "team-cycles"]);
    await b.saveSettings({ ...s, cycleRepos: [], lastReport: null, recordHotkey: "Ctrl+F9" });
    const after = await b.getSettings();
    expect(after.recordHotkey).toBe("Ctrl+F9");
    expect(after.cycleRepos).toHaveLength(2);
    expect(after.lastReport).not.toBeNull();
  });
});

describe("processing with a cycle", () => {
  it("skips the extraction stages for a meeting without a cycle and says why", async () => {
    const b = createMockBackend({ clockRate: 0, jobStepMs: 40 });
    await b.processMeeting("m1");
    for (let i = 0; i < 200; i += 1) {
      if ((await b.queueState()).jobs.find((j) => j.meetingId === "m1")?.state === "done") break;
      await new Promise((r) => setTimeout(r, 20));
    }
    const job = (await b.queueState()).jobs.find((j) => j.meetingId === "m1")!;
    expect(job.state).toBe("done");
    const extract = job.stages.find((s) => s.stage === "extract")!;
    expect(extract).toMatchObject({ status: "done", note: "the meeting has no cycle" });
    expect(job.stages.find((s) => s.stage === "reduce")?.note).toBe("the meeting has no cycle");
    expect(await codeOf(b.getDraft("m1"))).toBe("no_draft");
  });

  it("makes a draft for a recorded meeting of a cycle once the stages ran", async () => {
    const b = createMockBackend({ clockRate: 0, jobStepMs: 40 });
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "template", template: "budget-review" },
    });
    await b.stopRecording();
    const id = (await b.listMeetings())[0].id;
    await b.processMeeting(id);
    for (let i = 0; i < 300; i += 1) {
      if ((await b.queueState()).jobs.find((j) => j.meetingId === id)?.state === "done") break;
      await new Promise((r) => setTimeout(r, 20));
    }
    const job = (await b.queueState()).jobs.find((j) => j.meetingId === id)!;
    expect(job.stages.find((s) => s.stage === "reduce")?.note ?? null).toBeNull();
    const v = await b.getDraft(id);
    expect(v.draft).toMatchObject({ cycle: "NOMAD", mode: "template", template: "budget-review" });
    expect(v.draft.sections.map((s) => s.id)).toEqual(["spending", "forecast", "decisions"]);
  });

  it("refuses an explicit backend that is not configured", async () => {
    const b = backend();
    const s = await b.getProcessingSettings();
    await b.saveProcessingSettings({
      ...s,
      llm: { ...s.llm!, external: { baseUrl: "", model: "" } },
    });
    await b.startRecording({
      sourceIds: ["mic:default"],
      report: { cycleId: "NOMAD", mode: "fixed" },
    });
    await b.stopRecording();
    const id = (await b.listMeetings())[0].id;
    expect(await codeOf(b.processMeeting(id, { llm: { kind: "external" } }))).toBe(
      "llm_not_configured",
    );
  });
});

describe("participants of a cycle", () => {
  it("come from the cycle of the meeting, the legacy list otherwise", async () => {
    const b = backend();
    const forMeeting = await b.listParticipants(undefined, "m5");
    expect(forMeeting.map((p) => p.id)).toEqual([
      "NOMAD:alex",
      "NOMAD:jan",
      "NOMAD:kasia",
      "NOMAD:piotr",
    ]);
    expect(forMeeting[0]).toMatchObject({ cycleId: "NOMAD", cycleParticipantId: "alex" });
    expect((await b.listParticipants("SANDBOX")).map((p) => p.name)).toEqual(["Mira Demo"]);
    expect((await b.listParticipants()).map((p) => p.id)).toEqual(["p-alex", "p-jan", "p-kasia"]);
    expect((await b.getMeetingVoices("m5")).participants).toHaveLength(4);
  });

  it("adds a new voice name to the cycle", async () => {
    const b = backend();
    const data = await b.assignVoiceNew("m5", "v2", "Mira Example");
    const mira = data.participants.find((p) => p.name === "Mira Example")!;
    expect(mira.cycleId).toBe("NOMAD");
    expect(data.voices.find((v) => v.voiceId === "v2")?.assignedParticipantId).toBe(mira.id);
    expect(
      (await b.getCycle("NOMAD")).cycle.participants.some((p) => p.name === "Mira Example"),
    ).toBe(true);
    // The same name again names the existing person.
    const again = await b.assignVoiceNew("m5", "v3", "mira example");
    expect(again.participants.filter((p) => p.name === "Mira Example")).toHaveLength(1);
  });
});
