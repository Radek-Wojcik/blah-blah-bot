import type {
  CycleDto,
  CycleIssue,
  CycleParticipant,
  CycleRepoView,
  TemplateDto,
} from "./ipc/types";

/** Identifiers of cycles, participants, templates and meeting types. */
export const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;

/** Why `id` is not a usable cycle id, or null. */
export function cycleIdError(id: string): string | null {
  const t = id.trim();
  if (t === "") return "Enter an id.";
  if (!IDENTIFIER.test(t))
    return "Use letters, digits, - and _, starting with a letter or digit (no spaces or dots).";
  return null;
}

/** Why a new cycle cannot be created, or null. */
export function newCycleError(id: string, name: string): string | null {
  const e = cycleIdError(id);
  if (e) return e;
  if (name.trim() === "") return "Enter a name.";
  if ([...name.trim()].length > 120) return "The name is at most 120 characters.";
  return null;
}

/**
 * Why a folder for the meeting reports (`targets.github.path`) cannot be used, or null. The
 * same rules as the backend's: relative to the repository, `/` between names, no `.` or `..`
 * name, no drive letter or backslash, no `.git` folder, no name that ends in a dot.
 */
export function githubPathProblem(path: string): string | null {
  if (path === "") return "the folder is empty";
  if (path.startsWith("/") || path.startsWith("\\"))
    return "the folder must be relative to the repository, not absolute";
  if (/^[A-Za-z]:/.test(path)) return "the folder must not start with a drive letter";
  if (path.includes("\\")) return "use / between folder names, not a backslash";
  for (const name of path.split("/")) {
    if (name === "") return "the folder has an empty name (a doubled or trailing /)";
    if (name === "." || name === "..") return "the folder must not contain . or .. names";
    if (name.toLowerCase() === ".git") return "the folder must not be inside .git";
    if (name.endsWith(".")) return "a folder name must not end with a dot";
    if (!/^[A-Za-z0-9._-]+$/.test(name))
      return "folder names may only have letters, digits, dots, hyphens and underscores";
  }
  return null;
}

/** Issues whose path is `path` itself or something inside it (`participants[2]` includes `participants[2].id`). */
export function issuesAt(issues: CycleIssue[], path: string): CycleIssue[] {
  return issues.filter(
    (i) => i.path === path || i.path.startsWith(`${path}.`) || i.path.startsWith(`${path}[`),
  );
}

/** Issues of exactly this path, for a message next to one field. */
export function issuesOf(issues: CycleIssue[], path: string): CycleIssue[] {
  return issues.filter((i) => i.path === path);
}

/** `participants[2].id: duplicate id` as one line. */
export function issueText(issue: CycleIssue): string {
  return issue.path === "" ? issue.message : `${issue.path}: ${issue.message}`;
}

/** The meeting type rows of the editor: a map is edited as an ordered list. */
export interface TypeRow {
  type: string;
  template: string;
}

export function typeRows(byType: Record<string, string>): TypeRow[] {
  return Object.entries(byType)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([type, template]) => ({ type, template }));
}

/** Rows back to a map; blank types are dropped and a later row wins over an earlier one. */
export function rowsToMap(rows: TypeRow[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const r of rows) if (r.type.trim() !== "") out[r.type.trim()] = r.template;
  return out;
}

/** Types used by two rows (the second would silently replace the first). */
export function duplicateTypes(rows: TypeRow[]): string[] {
  const seen = new Set<string>();
  const dup = new Set<string>();
  for (const r of rows) {
    const t = r.type.trim();
    if (t === "") continue;
    if (seen.has(t)) dup.add(t);
    seen.add(t);
  }
  return [...dup];
}

/** The cycle as it would be saved: the type rows folded into `report.byType`. */
export function withTypeRows(cycle: CycleDto, rows: TypeRow[]): CycleDto {
  return { ...cycle, report: { ...cycle.report, byType: rowsToMap(rows) } };
}

/** True while the edited cycle differs from the saved one. */
export function isDirty(saved: CycleDto, edited: CycleDto): boolean {
  // The order of the meeting types does not count: the file keeps them sorted.
  const canonical = (c: CycleDto) =>
    JSON.stringify({ ...c, report: { ...c.report, byType: typeRows(c.report.byType) } });
  return canonical(saved) !== canonical(edited);
}

export function emptyParticipant(): CycleParticipant {
  return { id: "", name: "" };
}

/** How the state of a repository reads: `State as of 2026-11-12`, `Local repository`. */
export function repoState(repo: Pick<CycleRepoView, "kind" | "remote" | "stateAsOf">): string {
  if (repo.kind === "local" && repo.remote === null) return "Local repository";
  return repo.stateAsOf ? `State as of ${repo.stateAsOf}` : "Not pulled yet";
}

export function emptyTemplate(name: string): TemplateDto {
  return {
    name,
    sections: [],
    body: "# {title}\n\n{date} · {participants}\n\n{summary_section}\n\n{sections}\n",
  };
}

/** True while an edited template differs from the one stored. */
export function templateDirty(saved: TemplateDto | null, edited: TemplateDto): boolean {
  return saved === null || JSON.stringify(saved) !== JSON.stringify(edited);
}
