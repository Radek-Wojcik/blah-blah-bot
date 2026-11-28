import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const UI_SRC = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(UI_SRC, "../..");

/**
 * Explicit allowlist for non-user-facing tokens and on-disk file names that legitimately
 * contain "cycle" (such as `cycle.yaml`, repository folder names, GitHub label prefixes,
 * HTML/CSS classes, SQL queries, and Windows Recycle Bin messages).
 */
const ALLOWLIST_PATTERNS: RegExp[] = [
  /\bcycle\.yaml\b/i,
  /\brecycle bin\b/i,
  /\bcycles\//i,
  /\bcycle:/i,
  /\bcycle-row\b|\bcycle-name\b/,
  /\bcycles-changed\b|\bcycles-repo\b|\bcycle-repos\b|\bcycle_invalid\b/,
  /^demo-org\/nomad-cycles$|^team-cycles$/,
  /MARKER_START.*cycle=/,
  /bbb-issues-\{cycle_id\}\.json/,
  /^(?:SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER|PRAGMA)\b/i,
  /https?:\/\//i,
  /^[A-Za-z]:\\/,
];

function isAllowlisted(literal: string): boolean {
  return ALLOWLIST_PATTERNS.some((re) => re.test(literal));
}

function walkFiles(dir: string, predicate: (filePath: string) => boolean): string[] {
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walkFiles(full, predicate));
    } else if (entry.isFile() && predicate(full)) {
      out.push(full);
    }
  }
  return out;
}

function stripTestModule(rustSource: string): string {
  const lines = rustSource.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    if (/^\s*#\[cfg\(test\)\]/.test(lines[i])) {
      const next = `${lines[i + 1] ?? ""} ${lines[i + 2] ?? ""}`;
      if (/\bmod\s+\w+/.test(next)) {
        return lines.slice(0, i).join("\n");
      }
    }
  }
  return rustSource;
}

describe("user-visible wording guard (no 'cycle')", () => {
  it("has no user-visible 'cycle' strings in ui/src", () => {
    const files = walkFiles(
      UI_SRC,
      (p) => (p.endsWith(".ts") || p.endsWith(".tsx")) && !p.includes(".test."),
    );
    const hits: string[] = [];

    for (const file of files) {
      const rel = path.relative(REPO_ROOT, file).replace(/\\/g, "/");
      const lines = fs.readFileSync(file, "utf8").split(/\r?\n/);

      for (let i = 0; i < lines.length; i += 1) {
        const line = lines[i];
        const trimmed = line.trim();
        if (
          trimmed.startsWith("import ") ||
          /^\}?\s*from\s+["']/.test(trimmed) ||
          trimmed.startsWith("//") ||
          trimmed.startsWith("*") ||
          trimmed.startsWith("/**")
        ) {
          continue;
        }

        // 1. Quoted/template string literals containing standalone "cycle" or "cycles"
        for (const m of line.matchAll(/(["'`])((?:\\.|(?!\1).)*)\1/g)) {
          const val = m[2];
          // Remove `${...}` code interpolations before checking user-visible text.
          const plain = val.replace(/\$\{[^}]*\}/g, "");
          if (!/\bcycles?\b/i.test(plain)) continue;
          if (isAllowlisted(plain)) continue;
          const before = line.slice(0, m.index ?? 0);
          if (
            /(?:data-testid|className|id|htmlFor|invoke|listen|params\.get|params\.set|field|page\s*===|step\s*===|key)\s*[:=(]\s*$/.test(
              before,
            )
          ) {
            continue;
          }
          // Skip lowercase protocol/field keys like `"cycle"` or `"cycles"` (but flag `"Cycle"`).
          if (plain === "cycle" || plain === "cycles") continue;
          hits.push(`${rel}:${i + 1}: ${trimmed}`);
        }

        // 2. JSX text nodes between `>` and `<`
        const noStrings = trimmed.replace(/(["'`])(?:\\.|(?!\1).)*\1/g, '""');
        for (const m of noStrings.matchAll(/>([^<>{}]+)</g)) {
          if (/\bcycles?\b/i.test(m[1]) && !isAllowlisted(m[1])) {
            hits.push(`${rel}:${i + 1}: ${trimmed}`);
          }
        }
      }
    }

    expect(hits, `Found user-visible "cycle" wording in ui/src:\n${hits.join("\n")}`).toEqual([]);
  });

  it("has no user-visible 'cycle' string literals in Rust crates", () => {
    const cratesDir = path.join(REPO_ROOT, "crates");
    const files = walkFiles(cratesDir, (p) => {
      const norm = p.replace(/\\/g, "/");
      return (
        norm.includes("/src/") &&
        norm.endsWith(".rs") &&
        !norm.endsWith("tests.rs") &&
        !norm.includes("/tests/") &&
        !norm.includes("/testing/") &&
        !norm.includes("/release_check/")
      );
    });
    const hits: string[] = [];

    for (const file of files) {
      const rel = path.relative(REPO_ROOT, file).replace(/\\/g, "/");
      const prod = stripTestModule(fs.readFileSync(file, "utf8"));
      const lines = prod.split(/\r?\n/);

      for (let i = 0; i < lines.length; i += 1) {
        const line = lines[i];
        const trimmed = line.trim();
        if (trimmed.startsWith("//")) continue;

        for (const m of line.matchAll(/"((?:[^"\\]|\\.)*)"/g)) {
          const val = m[1];
          // Remove `{...}` format placeholders before checking user-visible text.
          const plain = val.replace(/\{[^}]*\}/g, "");
          if (!/\bcycles?\b/i.test(plain)) continue;
          if (isAllowlisted(val) || isAllowlisted(plain)) continue;
          // Skip lowercase JSON/serde/path keys without spaces (e.g. "cycle", "cycles", "cycle_id").
          if (/^[a-z0-9_./:-]+$/.test(plain) && !plain.includes(" ")) continue;
          hits.push(`${rel}:${i + 1}: ${trimmed}`);
        }
      }
    }

    expect(hits, `Found user-visible "cycle" wording in Rust crates:\n${hits.join("\n")}`).toEqual(
      [],
    );
  });
});
