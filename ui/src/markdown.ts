/**
 * A small Markdown reader for the previews of a report (what GitHub shows) and of a chat message
 * (what Discord shows). It understands what the report layouts produce: headings, paragraphs,
 * bullet and numbered lists with task checkboxes, quotes, rules, code fences, and bold, italic,
 * code and links inside text. It returns data, never HTML: the views turn it into elements, so
 * nothing in a transcript can inject markup. Only http and https links are links.
 */

export type Inline =
  | { kind: "text"; text: string }
  | { kind: "bold"; children: Inline[] }
  | { kind: "italic"; children: Inline[] }
  | { kind: "code"; text: string }
  | { kind: "link"; href: string; children: Inline[] }
  /** A link that is not http(s), such as `transcript.md`: shown as text with the target noted. */
  | { kind: "ref"; target: string; children: Inline[] };

export interface ListItem {
  /** `null` for a plain bullet, else the state of the task checkbox. */
  checked: boolean | null;
  children: Inline[];
}

export type Block =
  | { kind: "heading"; level: number; children: Inline[] }
  | { kind: "paragraph"; children: Inline[] }
  | { kind: "list"; ordered: boolean; items: ListItem[] }
  | { kind: "quote"; children: Inline[] }
  | { kind: "rule" }
  | { kind: "code"; text: string };

const SAFE_LINK = /^https?:\/\/[^\s]+$/i;

/** Inline elements of one line of text. */
export function parseInline(text: string): Inline[] {
  const out: Inline[] = [];
  let buffer = "";
  const flush = () => {
    if (buffer !== "") out.push({ kind: "text", text: buffer });
    buffer = "";
  };
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "\\" && i + 1 < text.length && "\\`*_[]()#".includes(text[i + 1])) {
      buffer += text[i + 1];
      i += 2;
      continue;
    }
    if (c === "`") {
      const end = text.indexOf("`", i + 1);
      if (end > i) {
        flush();
        out.push({ kind: "code", text: text.slice(i + 1, end) });
        i = end + 1;
        continue;
      }
    }
    if (c === "*" && text[i + 1] === "*") {
      const end = text.indexOf("**", i + 2);
      if (end > i + 2) {
        flush();
        out.push({ kind: "bold", children: parseInline(text.slice(i + 2, end)) });
        i = end + 2;
        continue;
      }
    }
    if (
      (c === "*" || c === "_") &&
      text[i + 1] !== c &&
      text[i + 1] !== " " &&
      text[i + 1] !== undefined
    ) {
      const end = text.indexOf(c, i + 1);
      // `_` inside a word is not emphasis (snake_case).
      const wordInside = c === "_" && /\w/.test(text[i - 1] ?? "");
      if (end > i + 1 && !wordInside && text[end - 1] !== " ") {
        flush();
        out.push({ kind: "italic", children: parseInline(text.slice(i + 1, end)) });
        i = end + 1;
        continue;
      }
    }
    if (c === "[") {
      const close = text.indexOf("](", i + 1);
      const end = close > i ? text.indexOf(")", close + 2) : -1;
      if (close > i && end > close) {
        const label = text.slice(i + 1, close);
        const target = text.slice(close + 2, end).trim();
        flush();
        const children = parseInline(label);
        out.push(
          SAFE_LINK.test(target)
            ? { kind: "link", href: target, children }
            : { kind: "ref", target, children },
        );
        i = end + 1;
        continue;
      }
    }
    buffer += c;
    i += 1;
  }
  flush();
  return out;
}

const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;
const TASK = /^\[( |x|X)\]\s+(.*)$/;
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;

/** The blocks of a Markdown text. Unknown constructs are paragraphs of plain text. */
export function parseMarkdown(source: string): Block[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  const endParagraph = () => {
    if (paragraph.length > 0)
      blocks.push({ kind: "paragraph", children: parseInline(paragraph.join(" ")) });
    paragraph = [];
  };
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trimStart().startsWith("```")) {
      endParagraph();
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trimStart().startsWith("```")) {
        body.push(lines[i]);
        i += 1;
      }
      i += 1;
      blocks.push({ kind: "code", text: body.join("\n") });
      continue;
    }
    if (line.trim() === "") {
      endParagraph();
      i += 1;
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      endParagraph();
      blocks.push({
        kind: "heading",
        level: heading[1].length,
        children: parseInline(heading[2].replace(/\s+#+\s*$/, "")),
      });
      i += 1;
      continue;
    }
    if (RULE.test(line)) {
      endParagraph();
      blocks.push({ kind: "rule" });
      i += 1;
      continue;
    }
    if (line.startsWith(">")) {
      endParagraph();
      const quoted: string[] = [];
      while (i < lines.length && lines[i].startsWith(">")) {
        quoted.push(lines[i].replace(/^>\s?/, ""));
        i += 1;
      }
      blocks.push({ kind: "quote", children: parseInline(quoted.join(" ")) });
      continue;
    }
    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    if (bullet || numbered) {
      endParagraph();
      const ordered = numbered !== null;
      const items: ListItem[] = [];
      while (i < lines.length) {
        const m = (ordered ? NUMBERED : BULLET).exec(lines[i]);
        if (!m) break;
        const task = ordered ? null : TASK.exec(m[1]);
        items.push(
          task
            ? { checked: task[1] !== " ", children: parseInline(task[2]) }
            : { checked: null, children: parseInline(m[1]) },
        );
        i += 1;
      }
      blocks.push({ kind: "list", ordered, items });
      continue;
    }
    paragraph.push(line.trim());
    i += 1;
  }
  endParagraph();
  return blocks;
}

/** The text of inline elements without any marks, for tests and accessible names. */
export function plainText(inlines: Inline[]): string {
  return inlines
    .map((n) => (n.kind === "text" || n.kind === "code" ? n.text : plainText(n.children)))
    .join("");
}
