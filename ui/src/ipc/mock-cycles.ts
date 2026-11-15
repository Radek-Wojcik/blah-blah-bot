/**
 * In-memory cycle repositories for the mock backend: the fictional NOMAD cycle, validation with
 * the rules and issue paths of `cycle.schema.json` and `cycle.rs`, templates, the report mode
 * gate and participants. No files and no Git.
 */
import { DEFAULT_TEMPLATE_TEXT, PLACEHOLDERS, validateTemplateDto } from "./mock-render";
import { IDENTIFIER, fail } from "./mock-util";
import type {
  AddRepoRequest,
  CycleDetails,
  CycleDto,
  CycleIssue,
  CycleListItem,
  CycleParticipant,
  CycleRepoView,
  LastReport,
  PullResult,
  ReportChoice,
  ReportDefaults,
  ReportMode,
  SaveCycleResult,
  TemplateCheck,
  TemplateDto,
  TemplateSummary,
  Topic,
} from "./types";

/** The date of the mock's "today": a pull brings a clone up to it. */
export const MOCK_TODAY = "2026-11-13";

const MODES: ReportMode[] = ["fixed", "template", "auto", "by-type"];
const LANGS = ["auto", "pl", "en"];
const PUBLISH = ["auto", "on-approval", "off"];
const GITHUB_LOGIN = /^[A-Za-z0-9][A-Za-z0-9-]*$/;
const SNOWFLAKE = /^[0-9]{1,25}$/;
const CREDENTIAL = /^cred:[A-Za-z0-9][A-Za-z0-9_./-]*$/;
const GITHUB_PATH = /^[A-Za-z0-9._-]+(\/[A-Za-z0-9._-]+)*$/;

const BAD_ID = "must start with a letter or digit and contain only letters, digits, - and _";

/** Schema, semantic and template checks of `cycle.yaml` (`Cycle::validate`). */
export function validateCycle(c: CycleDto, templates: string[]): CycleIssue[] {
  const issues: CycleIssue[] = [];
  const add = (path: string, message: string) => issues.push({ path, message });
  if (!IDENTIFIER.test(c.id)) add("id", `the cycle id ${BAD_ID}`);
  if (c.name.trim() === "") add("name", "the name must not be empty");
  if (!LANGS.includes(c.languages.asr)) add("languages.asr", "must be auto, pl or en");
  if (!LANGS.includes(c.languages.output)) add("languages.output", "must be auto, pl or en");
  const firstId = new Map<string, number>();
  const firstName = new Map<string, number>();
  c.participants.forEach((p, i) => {
    const at = `participants[${i}]`;
    if (p.id === "") add(`${at}.id`, "the participant id must not be empty");
    else if (!IDENTIFIER.test(p.id)) add(`${at}.id`, `the id "${p.id}" ${BAD_ID}`);
    if (p.name.trim() === "") add(`${at}.name`, "a participant needs a name");
    if (p.github !== undefined && p.github !== "" && !GITHUB_LOGIN.test(p.github))
      add(`${at}.github`, `"${p.github}" is not a GitHub login`);
    if (p.discord) {
      const { userId, roleId } = p.discord;
      if ((userId === undefined) === (roleId === undefined))
        add(`${at}.discord`, "give exactly one of user_id and role_id");
      if (userId !== undefined && !SNOWFLAKE.test(userId))
        add(`${at}.discord.user_id`, `"${userId}" is not a Discord id (digits only)`);
      if (roleId !== undefined && !SNOWFLAKE.test(roleId))
        add(`${at}.discord.role_id`, `"${roleId}" is not a Discord id (digits only)`);
    }
    const seenId = firstId.get(p.id);
    if (seenId !== undefined) {
      add(`${at}.id`, `duplicate participant id "${p.id}" (first used at participants[${seenId}])`);
    } else {
      firstId.set(p.id, i);
    }
    const key = p.name.trim().toLowerCase();
    if (key !== "") {
      const seenName = firstName.get(key);
      if (seenName !== undefined)
        add(
          `${at}.name`,
          `the name "${p.name}" is used twice (first at participants[${seenName}])`,
        );
      else firstName.set(key, i);
    }
  });
  c.glossary.forEach((g, i) => {
    if (g.term.trim() === "") add(`glossary[${i}].term`, "the term must not be empty");
  });
  const r = c.report;
  if (!MODES.includes(r.defaultMode)) add("report.default_mode", "unknown report mode");
  if (!IDENTIFIER.test(r.defaultTemplate))
    add("report.default_template", `the template name ${BAD_ID}`);
  const has = (name: string) => templates.includes(name);
  for (const [kind, template] of Object.entries(r.byType)) {
    if (!IDENTIFIER.test(kind)) add(`report.by_type.${kind}`, `the meeting type ${BAD_ID}`);
    else if (!IDENTIFIER.test(template))
      add(`report.by_type.${kind}`, `the template name ${BAD_ID}`);
  }
  if (
    (r.defaultMode === "template" || r.defaultMode === "by-type") &&
    IDENTIFIER.test(r.defaultTemplate) &&
    !has(r.defaultTemplate)
  )
    add(
      "report.default_template",
      `template "${r.defaultTemplate}" does not exist in templates/ but the default mode is ${r.defaultMode}`,
    );
  for (const [kind, template] of Object.entries(r.byType)) {
    if (IDENTIFIER.test(template) && !has(template))
      add(`report.by_type.${kind}`, `template "${template}" does not exist in templates/`);
  }
  const t = c.targets;
  if (t.github) {
    if (!GITHUB_PATH.test(t.github.path))
      add("targets.github.path", "a relative folder such as meetings (letters, digits, . _ - /)");
    if (!PUBLISH.includes(t.github.publish)) add("targets.github.publish", "unknown publish rule");
  }
  if (t.discord) {
    if (!CREDENTIAL.test(t.discord.webhook))
      add(
        "targets.discord.webhook",
        "must be a reference such as cred:discord/nomad; a webhook address is never stored in the repository",
      );
    if (!PUBLISH.includes(t.discord.publish))
      add("targets.discord.publish", "unknown publish rule");
    if (t.discord.template !== undefined && !IDENTIFIER.test(t.discord.template))
      add("targets.discord.template", `the template name ${BAD_ID}`);
  }
  if (t.notion) {
    if (t.notion.database.trim() === "")
      add("targets.notion.database", "the database must not be empty");
    if (!PUBLISH.includes(t.notion.publish)) add("targets.notion.publish", "unknown publish rule");
  }
  return issues;
}

/** Like the Rust conversion: empty optional strings are dropped, map entries sorted. */
function normalize(dto: CycleDto): CycleDto {
  const c = structuredClone(dto);
  c.name = c.name.trim();
  for (const p of c.participants) {
    p.name = p.name.trim();
    if (!p.github) delete p.github;
  }
  for (const g of c.glossary) if (!g.hint) delete g.hint;
  c.report.byType = Object.fromEntries(
    Object.entries(c.report.byType).sort(([a], [b]) => a.localeCompare(b)),
  );
  if (c.targets.discord && !c.targets.discord.template) delete c.targets.discord.template;
  return c;
}

function emptyCycle(id: string, name: string): CycleDto {
  return {
    id,
    name,
    languages: { asr: "auto", output: "auto" },
    participants: [],
    glossary: [],
    report: { defaultMode: "fixed", defaultTemplate: "default", instruction: "", byType: {} },
    targets: {},
  };
}

function person(
  id: string,
  name: string,
  github?: string,
  discord?: CycleParticipant["discord"],
): CycleParticipant {
  return { id, name, ...(github ? { github } : {}), ...(discord ? { discord } : {}) };
}

/** The fictional cycle of the demo data set. */
export function demoCycle(): CycleDto {
  return {
    id: "NOMAD",
    name: "Nomad rover",
    languages: { asr: "auto", output: "auto" },
    participants: [
      person("alex", "Alex Demo", "alex-demo", { userId: "000000000000000001" }),
      person("jan", "Jan Demo", "jan-demo"),
      person("kasia", "Kasia Demo", "kasia-demo", { userId: "000000000000000003" }),
      person("piotr", "Piotr Demo", undefined, { roleId: "000000000000000004" }),
    ],
    glossary: [{ term: "CAN bus" }, { term: "GNSS", hint: "positioning module" }],
    report: {
      defaultMode: "fixed",
      defaultTemplate: "default",
      instruction: "Group tasks by person, keep it short.",
      byType: { budget: "budget-review", casual: "default", technical: "default" },
    },
    targets: {
      github: { path: "meetings", publish: "on-approval" },
      discord: { webhook: "cred:discord/nomad", publish: "on-approval" },
    },
  };
}

const BUDGET_REVIEW: TemplateDto = {
  name: "budget-review",
  sections: [
    {
      id: "spending",
      title: "Spending",
      description: "Money spent or committed since the last review",
    },
    { id: "forecast", title: "Forecast and risks", description: "What may change the budget" },
    { id: "decisions", title: "Decisions", description: "Purchases and approvals" },
  ],
  body: "# {title}\n\n{date} · {participants}\n\n## Summary\n{summary}\n\n{sections}\n\n---\n[Full transcript]({transcript_link})\n",
};

const DEFAULT_TEMPLATE: TemplateDto = {
  name: "default",
  sections: [],
  body: DEFAULT_TEMPLATE_TEXT,
};

interface Entry {
  repoId: string;
  cycle: CycleDto;
  templates: Map<string, TemplateDto>;
  topics: Topic[];
}

interface RepoState extends CycleRepoView {
  /** Pulling this repository fails (the mock's way to show the offline state). */
  failing?: boolean;
}

export type RecordingReport = {
  cycleId: string | null;
  mode: ReportMode | null;
  template: string | null;
  meetingType: string | null;
  outputLanguage: string | null;
};

export const DEFAULT_REPO_ID = "local";

/** The id a name or an address is turned into, e.g. `https://x/team-cycles.git` to `team-cycles`. */
export function slug(source: string, taken: string[], fallback: string): string {
  const last =
    source
      .replace(/[/\\]+$/, "")
      .split(/[/\\:]/)
      .pop() ?? source;
  const stem = last.endsWith(".git") ? last.slice(0, -4) : last;
  const base =
    stem
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || fallback;
  let id = base;
  let n = 2;
  while (taken.some((t) => t.toLowerCase() === id)) {
    id = `${base}-${n}`;
    n += 1;
  }
  return id;
}

export interface CycleStore {
  listCycles(): CycleListItem[];
  listRepos(): CycleRepoView[];
  repoCount(): number;
  getCycle(id: string): CycleDetails;
  createCycle(id: string, name: string, repoId?: string): CycleDetails;
  saveCycle(dto: CycleDto): SaveCycleResult;
  listTemplates(cycleId: string): TemplateSummary[];
  getTemplate(cycleId: string, name: string): TemplateDto;
  saveTemplate(cycleId: string, t: TemplateDto): TemplateCheck;
  validateTemplate(t: TemplateDto): TemplateCheck;
  addRepo(req: AddRepoRequest): CycleRepoView;
  pullRepo(repoId: string): PullResult;
  reportDefaults(cycleId: string): ReportDefaults;
  /** The gate: validates a choice and returns what the recording keeps. */
  validateReport(choice: Partial<ReportChoice>): RecordingReport;
  /** `useLast` makes a missing choice the last one, like `start_recording` does. */
  resolveChoice(sent: ReportChoice | undefined, useLast: boolean): RecordingReport;
  lastReport(): LastReport | null;
  setLastReport(last: LastReport | null): void;
  cycle(id: string): CycleDto | null;
  template(cycleId: string, name: string): TemplateDto | null;
  participants(cycleId: string): CycleParticipant[];
  /** Adds a person (or finds the one with the same name) and returns the participant id. */
  addParticipant(cycleId: string, name: string): string;
  renameParticipant(cycleId: string, id: string, name: string): void;
  removeParticipant(cycleId: string, id: string): void;
}

export function createCycleStore(): CycleStore {
  const repos: RepoState[] = [
    {
      id: DEFAULT_REPO_ID,
      path: "C:\\Users\\demo\\AppData\\Roaming\\BlahBlahBot\\cycles",
      remote: null,
      kind: "local",
      available: true,
      stateAsOf: null,
      offline: false,
      lastError: null,
    },
    {
      id: "team-cycles",
      path: "C:\\Users\\demo\\AppData\\Roaming\\BlahBlahBot\\clones\\team-cycles",
      remote: "https://git.example.org/team/team-cycles.git",
      kind: "clone",
      available: true,
      stateAsOf: "2026-11-12",
      offline: false,
      lastError: null,
    },
  ];
  const entries = new Map<string, Entry>();
  const templatesOf = (...ts: TemplateDto[]) =>
    new Map(ts.map((t) => [t.name, structuredClone(t)]));
  entries.set("NOMAD", {
    repoId: "team-cycles",
    cycle: demoCycle(),
    templates: templatesOf(DEFAULT_TEMPLATE, BUDGET_REVIEW),
    topics: [
      { id: "t-1", text: "Decide the rover wheel size", status: "open", openedIn: "2026-11-06" },
      {
        id: "t-2",
        text: "Order the battery",
        status: "closed",
        openedIn: "2026-11-06",
        closedIn: "2026-11-13",
      },
    ],
  });
  // A broken cycle stays listed, with its issues, and cannot be used for a recording.
  const broken = emptyCycle("OLDPROJ", "Old project");
  broken.participants = [person("sam", "Sam Sample"), person("sam", "Samuel Sample")];
  entries.set("OLDPROJ", {
    repoId: "team-cycles",
    cycle: broken,
    templates: templatesOf(DEFAULT_TEMPLATE),
    topics: [],
  });
  const sandbox = emptyCycle("SANDBOX", "Sandbox");
  sandbox.participants = [person("mira", "Mira Demo")];
  entries.set("SANDBOX", {
    repoId: DEFAULT_REPO_ID,
    cycle: sandbox,
    templates: templatesOf(DEFAULT_TEMPLATE),
    topics: [],
  });
  let last: LastReport | null = { cycleId: "NOMAD", mode: "fixed" };

  const repoOf = (id: string): RepoState =>
    repos.find((r) => r.id === id) ?? fail("not_found", "this cycle repository is not registered");
  const entryOf = (id: string): Entry =>
    entries.get(id) ?? fail("not_found", `the cycle ${id} is not in any registered repository`);
  const issuesOf = (e: Entry) => validateCycle(e.cycle, [...e.templates.keys()]);

  const summaries = (e: Entry): TemplateSummary[] =>
    [...e.templates.values()]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((t) => ({ name: t.name, sections: structuredClone(t.sections) }));

  const details = (e: Entry): CycleDetails => {
    const repo = repoOf(e.repoId);
    const issues = issuesOf(e);
    return {
      cycle: structuredClone(e.cycle),
      repoId: e.repoId,
      valid: issues.length === 0,
      issues,
      templates: summaries(e),
      topics: structuredClone(e.topics),
      meetingTypes: Object.keys(e.cycle.report.byType).sort(),
      stateAsOf: repo.stateAsOf,
      offline: repo.offline,
    };
  };

  const check = (issues: CycleIssue[], saved: boolean): TemplateCheck => ({
    valid: issues.length === 0,
    saved,
    issues,
    placeholders: [...PLACEHOLDERS],
  });

  /** The cycle when it has no issues; else `cycle_invalid` (the gate turns that into its own code). */
  const validCycle = (id: string, code: "cycle_invalid" | "report_mode_required"): Entry => {
    const e = entries.get(id);
    if (!e) fail("report_mode_required", `the cycle ${id} does not exist`);
    const first = issuesOf(e)[0];
    if (first)
      fail(code, `the cycle cannot be used until it is fixed: ${first.path}: ${first.message}`);
    return e;
  };

  const gate = (message: string): never => fail("report_mode_required", message);

  const store: CycleStore = {
    listCycles: () =>
      [...entries.entries()]
        .map(([id, e]): CycleListItem => {
          const repo = repoOf(e.repoId);
          const issues = issuesOf(e);
          return {
            id,
            name: e.cycle.name,
            participantsCount: e.cycle.participants.length,
            defaultMode: e.cycle.report.defaultMode,
            valid: issues.length === 0,
            issues,
            repoId: e.repoId,
            stateAsOf: repo.stateAsOf,
            offline: repo.offline,
          };
        })
        .sort((a, b) => a.id.localeCompare(b.id)),
    listRepos: () => repos.map((r) => ({ ...r })),
    repoCount: () => repos.length,
    getCycle: (id) => details(entryOf(id)),
    createCycle: (rawId, rawName, repoId) => {
      const id = rawId.trim();
      const name = rawName.trim();
      if (!IDENTIFIER.test(id))
        fail(
          "invalid",
          "the cycle id may contain letters, digits, - and _ and must start with a letter or digit",
        );
      if (name === "" || [...name].length > 120)
        fail("invalid", "enter a name of 1 to 120 characters");
      if (entries.has(id)) fail("duplicate", `a cycle with the id ${id} already exists`);
      const target = repoId ? repoOf(repoId).id : DEFAULT_REPO_ID;
      const e: Entry = {
        repoId: target,
        cycle: emptyCycle(id, name),
        templates: templatesOf(DEFAULT_TEMPLATE),
        topics: [],
      };
      entries.set(id, e);
      return details(e);
    },
    saveCycle: (dto) => {
      const e = entryOf(dto.id);
      const cycle = normalize(dto);
      const issues = validateCycle(cycle, [...e.templates.keys()]);
      if (issues.length > 0) return { saved: false, issues, warnings: [], details: null };
      e.cycle = cycle;
      return { saved: true, issues: [], warnings: [], details: details(e) };
    },
    listTemplates: (cycleId) => summaries(entryOf(cycleId)),
    getTemplate: (cycleId, name) => {
      const t = entryOf(cycleId).templates.get(name);
      if (!t) fail("not_found", `the template ${name} does not exist`);
      return structuredClone(t);
    },
    saveTemplate: (cycleId, t) => {
      const e = entryOf(cycleId);
      const issues = validateTemplateDto(t);
      if (issues.length > 0) return check(issues, false);
      e.templates.set(t.name, structuredClone(t));
      return check([], true);
    },
    validateTemplate: (t) => check(validateTemplateDto(t), false),
    addRepo: (req) => {
      if (req.kind === "local") {
        const path = (req.path ?? "").trim();
        if (path === "") fail("invalid", "choose the folder of the repository");
        if (/missing|nonexistent/i.test(path)) fail("invalid", "this folder does not exist");
        if (repos.some((r) => r.path.toLowerCase() === path.toLowerCase()))
          fail("duplicate", "this folder is already registered");
        const repo: RepoState = {
          id: slug(
            path,
            repos.map((r) => r.id),
            "repository",
          ),
          path,
          remote: null,
          kind: "local",
          available: true,
          stateAsOf: null,
          offline: false,
          lastError: null,
        };
        repos.push(repo);
        return { ...repo };
      }
      const url = (req.url ?? "").trim();
      if (url === "" || url.startsWith("-")) fail("invalid", "enter the address of the repository");
      const authority = /^[^:/]+:\/\/([^/?#]*)/.exec(url)?.[1] ?? "";
      if (authority.includes("@") && authority.slice(0, authority.lastIndexOf("@")).includes(":"))
        fail(
          "invalid",
          "do not put a password or token into the address; use the Git credential manager",
        );
      if (/unreachable/i.test(url))
        fail("git_failed", "git clone failed: could not resolve the host");
      const id = slug(
        url,
        repos.map((r) => r.id),
        "repository",
      );
      const repo: RepoState = {
        id,
        path: `C:\\Users\\demo\\AppData\\Roaming\\BlahBlahBot\\clones\\${id}`,
        remote: url,
        kind: "clone",
        available: true,
        stateAsOf: MOCK_TODAY,
        offline: false,
        lastError: null,
        failing: /offline/i.test(url),
      };
      repos.push(repo);
      // The repository brings a cycle of its own, named after it.
      const cycleId = id.toUpperCase().replace(/[^A-Z0-9_-]/g, "-");
      if (!entries.has(cycleId)) {
        entries.set(cycleId, {
          repoId: id,
          cycle: emptyCycle(cycleId, "Cloned cycle"),
          templates: templatesOf(DEFAULT_TEMPLATE),
          topics: [],
        });
      }
      return { ...repo };
    },
    pullRepo: (repoId) => {
      const repo = repoOf(repoId);
      if (repo.remote === null) return { repo: { ...repo }, outcome: "noRemote", error: null };
      if (repo.failing) {
        repo.offline = true;
        repo.lastError = "could not resolve the host; showing the state of the last pull";
        return { repo: { ...repo }, outcome: "failed", error: repo.lastError };
      }
      const outcome = repo.stateAsOf === MOCK_TODAY ? "upToDate" : "updated";
      repo.stateAsOf = MOCK_TODAY;
      repo.offline = false;
      repo.lastError = null;
      return { repo: { ...repo }, outcome, error: null };
    },
    reportDefaults: (cycleId) => {
      const e = entryOf(cycleId);
      const cycle = validCycle(cycleId, "cycle_invalid").cycle;
      const types = Object.entries(cycle.report.byType).sort(([a], [b]) => a.localeCompare(b));
      return {
        cycleId: cycle.id,
        defaultMode: cycle.report.defaultMode,
        defaultTemplate: cycle.report.defaultTemplate,
        defaultMeetingType: types[0]?.[0] ?? null,
        outputLanguage: cycle.languages.output,
        templates: summaries(e),
        meetingTypes: types.map(([name, template]) => ({ name, template })),
        last: last && last.cycleId === cycle.id ? { ...last } : null,
      };
    },
    validateReport: (choice) => {
      const cycleId = (choice.cycleId ?? "").trim();
      if (cycleId === "") gate("choose a cycle before recording");
      const mode = choice.mode as string | undefined;
      if (!mode) gate("choose a report mode");
      if (!MODES.includes(mode as ReportMode)) gate(`unknown report mode "${mode}"`);
      const lang = choice.outputLanguage ?? null;
      if (lang !== null && !LANGS.includes(lang)) gate(`unknown output language "${lang}"`);
      const e = validCycle(cycleId, "report_mode_required");
      const cycle = e.cycle;
      let template = choice.template || null;
      if (template && !e.templates.has(template))
        gate(`the cycle has no template named "${template}"`);
      let meetingType: string | null = null;
      if (mode === "template") {
        const name = template ?? cycle.report.defaultTemplate;
        if (!e.templates.has(name)) gate(`the cycle has no template named "${name}"`);
        template = name;
      } else if (mode === "by-type") {
        const kind = choice.meetingType || null;
        if (!kind) gate("choose the meeting type for the by-type report");
        if (!(kind! in cycle.report.byType))
          gate(`the cycle does not define the meeting type "${kind}"`);
        // The meeting type picks the template.
        template = null;
        meetingType = kind;
      }
      return {
        cycleId: cycle.id,
        mode: mode as ReportMode,
        template,
        meetingType,
        outputLanguage: lang,
      };
    },
    resolveChoice: (sent, useLast) => {
      if (sent) return store.validateReport(sent);
      if (useLast && last) {
        return store.validateReport({
          cycleId: last.cycleId,
          mode: last.mode,
          template: last.template,
          meetingType: last.meetingType,
        });
      }
      return gate("choose a cycle and a report mode before recording");
    },
    lastReport: () => (last ? { ...last } : null),
    setLastReport: (l) => void (last = l ? { ...l } : null),
    cycle: (id) => (entries.get(id) ? structuredClone(entries.get(id)!.cycle) : null),
    template: (cycleId, name) => {
      const t = entries.get(cycleId)?.templates.get(name);
      return t ? structuredClone(t) : null;
    },
    participants: (cycleId) => structuredClone(entryOf(cycleId).cycle.participants),
    addParticipant: (cycleId, rawName) => {
      const e = entryOf(cycleId);
      const name = rawName.trim();
      const same = e.cycle.participants.find(
        (p) => p.name.trim().toLowerCase() === name.toLowerCase(),
      );
      if (same) return same.id;
      const id = slug(
        name,
        e.cycle.participants.map((p) => p.id),
        "participant",
      );
      e.cycle.participants.push({ id, name });
      return id;
    },
    renameParticipant: (cycleId, id, rawName) => {
      const e = entryOf(cycleId);
      const name = rawName.trim();
      if (
        e.cycle.participants.some(
          (p) => p.id !== id && p.name.trim().toLowerCase() === name.toLowerCase(),
        )
      )
        fail("duplicate", "a participant with this name already exists");
      const p = e.cycle.participants.find((x) => x.id === id);
      if (!p) fail("not_found", "participant not found");
      p.name = name;
    },
    removeParticipant: (cycleId, id) => {
      const e = entryOf(cycleId);
      e.cycle.participants = e.cycle.participants.filter((p) => p.id !== id);
    },
  };
  return store;
}
