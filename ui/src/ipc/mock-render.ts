/**
 * The mock's version of the report template engine (`crates/cycle-store/src/template.rs` and
 * `render.rs`), following `docs/CYCLES.md`: placeholders, item lines, built-in sections and the
 * removal of empty headings. Only the mock uses it; the real application renders in Rust.
 */
import { SECTION_ID } from "./mock-util";
import type {
  CycleIssue,
  Draft,
  DraftItem,
  ItemType,
  RenderVariant,
  TemplateDto,
  TemplateSection,
} from "./types";

/** The default template (ADR 0017), the text of `templates/default.md` of a new cycle. */
export const DEFAULT_TEMPLATE_TEXT =
  "# {title}\n\n{date} · {participants}\n\n{summary_section}\n\n{sections}\n\n---\n[Full transcript](transcript.md)\n";

/** The placeholders a template body may use. */
export const PLACEHOLDERS = [
  "title",
  "date",
  "participants",
  "summary",
  "summary_section",
  "sections",
  "section:<id>",
  "tasks",
  "decisions",
  "topics",
  "notes",
  "ideas",
  "budgets",
  "transcript_link",
  "report_link",
];

const SIMPLE = new Set(PLACEHOLDERS.filter((p) => !p.includes(":")));

export const UNVERIFIED_MARKER = "(unverified quote)";

type Token = { text: string } | { placeholder: string };

function placeholderOf(name: string): string | null {
  if (SIMPLE.has(name)) return name;
  if (name.startsWith("section:") && SECTION_ID.test(name.slice("section:".length))) return name;
  return null;
}

/** Splits a body into text and placeholders. `{{` and `}}` are literal braces. */
export function tokenize(body: string): { tokens: Token[]; issues: CycleIssue[] } {
  const chars = [...body];
  const tokens: Token[] = [];
  const issues: CycleIssue[] = [];
  let text = "";
  let line = 1;
  let i = 0;
  while (i < chars.length) {
    const c = chars[i];
    if (c === "\n") {
      line += 1;
      text += c;
      i += 1;
    } else if (c === "{" && chars[i + 1] === "{") {
      text += "{";
      i += 2;
    } else if (c === "}" && chars[i + 1] === "}") {
      text += "}";
      i += 2;
    } else if (c === "{") {
      let off = -1;
      for (let j = i + 1; j < chars.length; j += 1) {
        if (chars[j] === "}" || chars[j] === "{" || chars[j] === "\n") {
          off = j - (i + 1);
          break;
        }
      }
      if (off >= 0 && chars[i + 1 + off] === "}") {
        const name = chars.slice(i + 1, i + 1 + off).join("");
        const found = placeholderOf(name);
        if (found) {
          if (text !== "") {
            tokens.push({ text });
            text = "";
          }
          tokens.push({ placeholder: found });
        } else {
          issues.push({
            path: "body",
            message: `line ${line}: unknown placeholder {${name}} (use {{ and }} for literal braces)`,
          });
        }
        i += off + 2;
      } else {
        issues.push({
          path: "body",
          message: `line ${line}: '{' is not closed on the same line (use {{ for a literal brace)`,
        });
        i += 1;
      }
    } else {
      text += c;
      i += 1;
    }
  }
  if (text !== "") tokens.push({ text });
  return { tokens, issues };
}

function sectionIssues(sections: TemplateSection[]): CycleIssue[] {
  const issues: CycleIssue[] = [];
  const seen = new Map<string, number>();
  sections.forEach((s, i) => {
    const path = `sections[${i}]`;
    if (!SECTION_ID.test(s.id)) {
      issues.push({
        path: `${path}.id`,
        message: `section id "${s.id}" must start with a lowercase letter and contain only lowercase letters, digits, '_' and '-'`,
      });
    }
    if (s.title.trim() === "") {
      issues.push({ path: `${path}.title`, message: "the title must not be empty" });
    }
    const first = seen.get(s.id);
    if (first !== undefined) {
      issues.push({
        path: `${path}.id`,
        message: `duplicate section id "${s.id}" (first used at sections[${first}])`,
      });
    } else {
      seen.set(s.id, i);
    }
  });
  return issues;
}

/** Everything that can be wrong with a template: its sections, its name and its placeholders. */
export function validateTemplateDto(t: TemplateDto, checkName = true): CycleIssue[] {
  const issues = sectionIssues(t.sections);
  if (checkName && !/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(t.name)) {
    issues.push({ path: "name", message: `template name "${t.name}" is not a valid file name` });
  }
  const { tokens, issues: bodyIssues } = tokenize(t.body);
  issues.push(...bodyIssues);
  if (bodyIssues.length === 0 && t.sections.length > 0) {
    for (const token of tokens) {
      if (!("placeholder" in token) || !token.placeholder.startsWith("section:")) continue;
      const id = token.placeholder.slice("section:".length);
      if (!t.sections.some((s) => s.id === id)) {
        issues.push({
          path: "body",
          message: `{section:${id}} refers to a section the front matter does not declare`,
        });
      }
    }
  }
  return issues;
}

// ----- rendering -------------------------------------------------------------------------

type Lang = "pl" | "en";

const BUILTIN: { type: ItemType; en: string; pl: string }[] = [
  { type: "decision", en: "Decisions", pl: "Decyzje" },
  { type: "task", en: "Tasks", pl: "Zadania" },
  { type: "topic", en: "Open topics", pl: "Otwarte tematy" },
  { type: "note", en: "Notes", pl: "Notatki" },
  { type: "idea", en: "Ideas", pl: "Pomysły" },
  { type: "budget", en: "Budgets", pl: "Budżety" },
];

interface Person {
  id: string;
  name: string;
}

/** Puts a zero-width space after the `@` of `@everyone` and `@here` (any case). */
export function neutraliseMentions(text: string): string {
  return text.replace(/@(?=everyone|here)/gi, "@\u200b");
}

/** One item as a Markdown bullet line (`docs/CYCLES.md`, "Item lines"). */
export function itemLine(item: DraftItem, people: Person[], variant: RenderVariant): string {
  const text = item.text.split(/\s+/).filter(Boolean).join(" ");
  let line = "- ";
  if (item.type === "task") {
    if (variant === "markdown") line += "[ ] ";
    line += `${text} (${assigneeLabel(item, people)}, ${dueLabel(item)})`;
  } else {
    line += text;
  }
  if (item.source && !item.source.verified) line += ` ${UNVERIFIED_MARKER}`;
  return line;
}

function assigneeLabel(item: DraftItem, people: Person[]): string {
  const mark = item.certainty.assignee === "stated" ? "" : "?";
  if (item.assignee !== null) {
    const name = people.find((p) => p.id === item.assignee)?.name ?? item.assignee;
    return `@${name}${mark}`;
  }
  const outside = item.assigneeName?.trim();
  if (outside) {
    return `@${outside}${mark}`;
  }
  return "@?";
}

function dueLabel(item: DraftItem): string {
  if (item.due === null) return "due ?";
  return `due ${item.due}${item.certainty.due === "stated" ? "" : "?"}`;
}

export function defaultTitle(draft: Draft, lang: Lang): string {
  return `${lang === "pl" ? "Spotkanie" : "Meeting"} ${draft.cycle} ${draft.date}`;
}

/** Stands in the text for a placeholder that expanded to nothing; `tidy` removes it. */
const EMPTY_MARK = "\ue000";

function headingLevel(line: string): number | null {
  let hashes = 0;
  while (line[hashes] === "#") hashes += 1;
  return hashes >= 1 && hashes <= 6 && line[hashes] === " " ? hashes : null;
}

/**
 * Removes empty headings (level 2 and deeper), trailing spaces and extra blank lines, and puts
 * one blank line around every heading. A heading is empty when nothing but blank lines follows
 * it before the next heading or `---`, or when the first block under it is made of placeholders
 * that expanded to nothing (`## Tasks`, `{tasks}`, a blank line, then a link).
 */
function tidy(text: string): string {
  const raw = text.split("\n");
  if (raw[raw.length - 1] === "") raw.pop();
  const isMark = (l: string) =>
    l.includes(EMPTY_MARK) && l.split(EMPTY_MARK).join("").trim() === "";
  const marked = raw.map(isMark);
  const lines = raw.map((l) => l.split(EMPTY_MARK).join("").trimEnd());
  let inFence = false;
  const fenced = lines.map((l) => {
    if (l.trimStart().startsWith("```")) {
      inFence = !inFence;
      return true;
    }
    return inFence;
  });
  const levelAt = (i: number) => (fenced[i] ? null : headingLevel(lines[i]));
  // A line that is blank for real, not just because an empty placeholder stood on it.
  const plainBlank = (j: number) => lines[j] === "" && !marked[j];
  const firstBlockIsEmpty = (i: number): boolean => {
    let j = i + 1;
    while (j < lines.length && plainBlank(j)) j += 1;
    const start = j;
    while (j < lines.length && !plainBlank(j)) {
      if (!marked[j]) return false;
      j += 1;
    }
    return j > start;
  };
  const keep = lines.map(() => true);
  lines.forEach((_, i) => {
    const level = levelAt(i);
    if (level === null || level < 2) return;
    if (firstBlockIsEmpty(i)) {
      keep[i] = false;
      return;
    }
    let hasContent = false;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (!fenced[j] && lines[j] === "---") break;
      const l = levelAt(j);
      if (l !== null && l <= level) break;
      if (l === null && lines[j] !== "") {
        hasContent = true;
        break;
      }
    }
    keep[i] = hasContent;
  });
  let out = "";
  // True at the start of the output and right after a blank line.
  let atBlockStart = true;
  let prevHeading = false;
  lines.forEach((line, i) => {
    if (!keep[i]) return;
    if (line === "") {
      if (!atBlockStart) {
        out += "\n";
        atBlockStart = true;
      }
      return;
    }
    const heading = levelAt(i) !== null;
    // A heading is always set apart from the text before and after it.
    if (!atBlockStart && (heading || prevHeading)) out += "\n";
    out += `${line}\n`;
    atBlockStart = false;
    prevHeading = heading;
  });
  while (out.endsWith("\n\n")) out = out.slice(0, -1);
  return out;
}

/**
 * Renders a draft with a template body. Throws an `Error` with the first problem when the body
 * has an unknown placeholder (the real renderer reports it as an error too).
 */
export function renderReport(
  draft: Draft,
  people: Person[],
  body: string,
  lang: Lang,
  variant: RenderVariant,
): string {
  const { tokens, issues } = tokenize(body.replace(/\r\n/g, "\n"));
  if (issues.length > 0) throw new Error(issues[0].message);
  const list = (items: DraftItem[]) => items.map((i) => itemLine(i, people, variant)).join("\n");
  const ofType = (t: ItemType) => list(draft.items.filter((i) => i.type === t));
  const builtin = draft.mode === "fixed" || draft.sections.length === 0;
  // Items without a section, or with an unknown one, go to the last section.
  const sectionIndex = (item: DraftItem): number => {
    const found = draft.sections.findIndex((s) => s.id === item.section);
    return found >= 0 ? found : draft.sections.length - 1;
  };
  const sectionsBlock = (): string => {
    const blocks: string[] = [];
    if (builtin) {
      for (const b of BUILTIN) {
        const l = ofType(b.type);
        if (l !== "") blocks.push(`## ${lang === "pl" ? b.pl : b.en}\n\n${l}`);
      }
    } else {
      draft.sections.forEach((s, idx) => {
        const l = list(draft.items.filter((i) => sectionIndex(i) === idx));
        if (l !== "") blocks.push(`## ${s.title}\n\n${l}`);
      });
    }
    return blocks.join("\n\n");
  };
  const sectionList = (id: string): string => {
    const idx = draft.sections.findIndex((s) => s.id === id);
    if (idx < 0 || builtin) return "";
    return list(draft.items.filter((i) => sectionIndex(i) === idx));
  };
  const expand = (name: string): string => {
    switch (name) {
      case "title":
        return defaultTitle(draft, lang);
      case "date":
        return draft.date;
      case "participants":
        return people.map((p) => p.name).join(", ");
      case "summary":
        return draft.summary.trim();
      case "summary_section": {
        const summary = draft.summary.trim();
        return summary === ""
          ? ""
          : `## ${lang === "pl" ? "Podsumowanie" : "Summary"}\n\n${summary}`;
      }
      case "sections":
        return sectionsBlock();
      case "tasks":
        return ofType("task");
      case "decisions":
        return ofType("decision");
      case "topics":
        return ofType("topic");
      case "notes":
        return ofType("note");
      case "ideas":
        return ofType("idea");
      case "budgets":
        return ofType("budget");
      case "transcript_link":
        return "transcript.md";
      case "report_link":
        return "report.md";
      default:
        return name.startsWith("section:") ? sectionList(name.slice("section:".length)) : "";
    }
  };
  let out = "";
  for (const t of tokens) {
    if ("text" in t) {
      out += t.text;
    } else {
      const text = expand(t.placeholder);
      out += text.trim() === "" ? EMPTY_MARK : text;
    }
  }
  // Item text, the summary and section titles come from a model: the report must not be able to
  // notify everybody where it is posted.
  return neutraliseMentions(tidy(out));
}
