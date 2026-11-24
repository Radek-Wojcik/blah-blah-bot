import { describe, expect, it } from "vitest";
import { parseInline, parseMarkdown, plainText } from "./markdown";

describe("inline", () => {
  it("reads bold, italic, code and links", () => {
    expect(parseInline("a **b** _c_ `d` [e](https://x.org/y)")).toEqual([
      { kind: "text", text: "a " },
      { kind: "bold", children: [{ kind: "text", text: "b" }] },
      { kind: "text", text: " " },
      { kind: "italic", children: [{ kind: "text", text: "c" }] },
      { kind: "text", text: " " },
      { kind: "code", text: "d" },
      { kind: "text", text: " " },
      { kind: "link", href: "https://x.org/y", children: [{ kind: "text", text: "e" }] },
    ]);
  });

  it("only makes a link of http and https", () => {
    for (const bad of [
      "javascript:alert(1)",
      "data:text/html,x",
      "file:///c:/x",
      "//evil.example/x",
    ]) {
      const [node] = parseInline(`[click](${bad})`);
      expect(node.kind).toBe("ref");
    }
    expect(parseInline("[Full transcript](transcript.md)")[0]).toMatchObject({
      kind: "ref",
      target: "transcript.md",
    });
  });

  it("leaves snake_case and lone marks alone", () => {
    expect(plainText(parseInline("use snake_case_names and 2 * 3"))).toBe(
      "use snake_case_names and 2 * 3",
    );
    expect(plainText(parseInline("an \\*escaped\\* star"))).toBe("an *escaped* star");
  });

  it("never turns text into markup: a tag stays text", () => {
    expect(parseInline("<script>alert(1)</script>")).toEqual([
      { kind: "text", text: "<script>alert(1)</script>" },
    ]);
  });
});

describe("blocks", () => {
  const report = [
    "# Meeting NOMAD 2026-11-13",
    "",
    "2026-11-13 · Alex Demo, Jan Demo",
    "",
    "## Tasks",
    "",
    "- [ ] Update the schedule (@Jan Demo, due 2026-11-19)",
    "- [x] Order the valves",
    "- A plain bullet",
    "",
    "1. First",
    "2. Second",
    "",
    "> A quote",
    "",
    "---",
    "[Full transcript](transcript.md)",
    "",
    "```",
    "code **not bold**",
    "```",
  ].join("\n");

  it("reads headings, lists, task boxes, quotes, rules and code", () => {
    const blocks = parseMarkdown(report);
    expect(blocks.map((b) => b.kind)).toEqual([
      "heading",
      "paragraph",
      "heading",
      "list",
      "list",
      "quote",
      "rule",
      "paragraph",
      "code",
    ]);
    expect(blocks[0]).toMatchObject({ kind: "heading", level: 1 });
    const tasks = blocks[3];
    if (tasks.kind !== "list") throw new Error("not a list");
    expect(tasks.items.map((i) => i.checked)).toEqual([false, true, null]);
    expect(tasks.ordered).toBe(false);
    const numbered = blocks[4];
    if (numbered.kind !== "list") throw new Error("not a list");
    expect(numbered.ordered).toBe(true);
    expect(blocks[8]).toEqual({ kind: "code", text: "code **not bold**" });
  });

  it("joins the lines of a paragraph", () => {
    const [p] = parseMarkdown("one\ntwo\n\nthree");
    expect(p.kind === "paragraph" && plainText(p.children)).toBe("one two");
  });

  it("handles windows line breaks and an empty text", () => {
    expect(parseMarkdown("# a\r\n\r\ntext")).toHaveLength(2);
    expect(parseMarkdown("")).toEqual([]);
  });
});
