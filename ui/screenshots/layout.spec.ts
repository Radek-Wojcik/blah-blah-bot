/**
 * Layout check at several window sizes, taken from the UI in demo mode (mock backend).
 * Run: npx playwright test --project=screenshots layout.spec.ts
 *
 * Every main screen is captured at 1280x720, 1920x1080, 2560x1440 and 3440x1440 into
 * `<repo>/target/layout-shots/` (ignored by git). Each screen is also checked for horizontal
 * scroll and for text that overflows its box. Two representative 2560 shots are kept in
 * `docs/img/` as `layout-2560-*.png`.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { type Page, expect, test } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(here, "../../target/layout-shots");
const DOCS = path.resolve(here, "../../docs/img");

const SIZES = [
  { width: 1280, height: 720 },
  { width: 1920, height: 1080 },
  { width: 2560, height: 1440 },
  { width: 3440, height: 1440 },
];

interface Screen {
  name: string;
  query: string;
  ready: (page: Page) => Promise<void>;
  /** Also written to docs/img/layout-2560-<name>.png. */
  keep?: boolean;
}

const SCREENS: Screen[] = [
  {
    name: "projects",
    query: "page=projects",
    ready: async (page) => {
      await expect(page.getByRole("article").first()).toBeVisible();
    },
    keep: true,
  },
  {
    name: "project-detail",
    query: "page=project&cycle=NOMAD",
    ready: async (page) => {
      await expect(page.getByRole("list", { name: "Top action items" })).toBeVisible();
    },
    keep: true,
  },
  {
    name: "record",
    query: "page=record",
    ready: async (page) => {
      await expect(page.getByRole("heading", { name: /record/i }).first()).toBeVisible();
    },
  },
  {
    name: "queue",
    query: "page=queue",
    ready: async (page) => {
      await expect(page.getByRole("article").first()).toBeVisible();
    },
  },
  ...["models", "connectors", "preferences", "processing", "voices"].map((tab) => ({
    name: `settings-${tab}`,
    query: `page=settings&tab=${tab}`,
    ready: async (page: Page) => {
      await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
    },
  })),
];

test.use({ colorScheme: "dark", deviceScaleFactor: 1, reducedMotion: "reduce" });

test.beforeAll(() => {
  fs.mkdirSync(OUT, { recursive: true });
});

for (const size of SIZES) {
  test.describe(`${size.width}x${size.height}`, () => {
    for (const screen of SCREENS) {
      test(screen.name, async ({ page }) => {
        await page.setViewportSize(size);
        await page.goto(`/?view=main&backend=mock&rate=0&${screen.query}`);
        await screen.ready(page);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(400);

        const file = path.join(OUT, `${screen.name}-${size.width}.png`);
        await page.screenshot({ path: file });
        if (screen.keep && size.width === 2560) {
          await page.screenshot({ path: path.join(DOCS, `layout-2560-${screen.name}.png`) });
        }

        const problems = await page.evaluate(() => {
          const out: string[] = [];
          const root = document.documentElement;
          if (root.scrollWidth > root.clientWidth) out.push("page scrolls horizontally");
          const content = document.querySelector(".content");
          if (content && content.scrollWidth > content.clientWidth + 1) {
            out.push("content scrolls horizontally");
          }
          return out;
        });
        expect(problems).toEqual([]);
      });
    }
  });
}
