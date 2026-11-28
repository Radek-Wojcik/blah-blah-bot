/**
 * Reproducible screenshots and the demo GIF, taken from the UI in demo mode (mock backend,
 * fictional cycle, frozen clock). Run: npm run screenshots
 *
 * Fixed window size, dark theme, scale 100% and reduced motion: running it twice on the
 * same commit produces identical PNG files.
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { type Page, expect, test } from "@playwright/test";
import { cleanupDirs, freshDir } from "../support/e2e-tmp";

const here = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(here, "../../docs/img");
const TRANSITION_MS = 110;
const HOLD_MS = 320;

test.use({ colorScheme: "dark", deviceScaleFactor: 1, launchOptions: { args: ["--lang=en-US"] } });

// The frame folders of the GIFs live in <repo>/target/e2e-tmp and go after every test.
test.afterEach(() => {
  cleanupDirs();
});

async function settle(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
}

/**
 * Grows the window until the content of the main window fits, so a long screen is captured whole
 * and without a scroll bar. The width stays as it is.
 */
async function fitWindowToPage(page: Page, width = 1280, minHeight = 800, maxHeight = 4000) {
  // The main window scrolls inside `.content`; the window must be as tall as that content needs.
  const height = await page.evaluate(() => {
    const content = document.querySelector(".content");
    const extra = content ? content.scrollHeight - content.clientHeight : 0;
    return Math.ceil(window.innerHeight + Math.max(0, extra));
  });
  await page.setViewportSize({ width, height: Math.min(maxHeight, Math.max(minHeight, height)) });
  await page.evaluate(() => document.querySelector(".content")?.scrollTo(0, 0));
  await settle(page);
}

/** A dialog that scrolls inside itself would be cut off in its picture. */
async function expectDialogNotClipped(dialog: ReturnType<Page["locator"]>) {
  const clipped = await dialog.evaluate((el) => el.scrollHeight > el.clientHeight + 1);
  expect(clipped, "the dialog is taller than the window").toBe(false);
}

interface GifFrame {
  file: string;
  durationMs: number;
}

function createGifRecorder(page: Page) {
  const frames: GifFrame[] = [];
  const dir = freshDir("gif");

  const snap = async (durationMs = TRANSITION_MS, waitMs = 0) => {
    if (waitMs > 0) await page.waitForTimeout(waitMs);
    const file = path.join(dir, `f${String(frames.length).padStart(3, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push({ file, durationMs });
  };

  const hold = async (waitMs = 150) => {
    await snap(HOLD_MS, waitMs);
  };

  const burst = async (count: number, intervalMs = 90, durationMs = TRANSITION_MS) => {
    for (let i = 0; i < count; i++) {
      await snap(durationMs, intervalMs);
    }
  };

  const smoothScrollTo = async (locator: ReturnType<Page["locator"]>, steps = 4) => {
    const targetY = await locator.evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return Math.max(0, window.scrollY + rect.top - 16);
    });
    const startY = await page.evaluate(() => window.scrollY);
    for (let i = 1; i <= steps; i++) {
      const y = Math.round(startY + ((targetY - startY) * i) / steps);
      await page.evaluate((top) => window.scrollTo({ top, behavior: "instant" }), y);
      await snap(TRANSITION_MS, 70);
    }
  };

  const save = (filename: string, maxBytes: number) => {
    fs.mkdirSync(OUT, { recursive: true });
    const outPath = path.join(OUT, filename);
    const durations = frames.map((f) => String(f.durationMs)).join(",");
    execFileSync("python", [
      path.join(here, "make-gif.py"),
      outPath,
      durations,
      ...frames.map((f) => f.file),
    ]);
    const size = fs.statSync(outPath).size;
    expect(size).toBeLessThan(maxBytes);
  };

  return { snap, hold, burst, smoothScrollTo, save };
}

test.describe("static screenshots", () => {
  test.use({ reducedMotion: "reduce" });

  test("tray widget while recording", async ({ page }) => {
    await page.setViewportSize({ width: 340, height: 420 });
    await page.goto("/?view=widget&backend=mock&state=recording&rate=0");
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "tray-widget.png") });
  });

  test("source picker", async ({ page }) => {
    // Taller than the other widget shots: the Browser section lists captured tabs.
    await page.setViewportSize({ width: 340, height: 860 });
    await page.goto("/?view=widget&backend=mock&rate=0&tabs=1");
    await page.getByLabel("Discord").check();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "source-picker.png") });
  });

  test("meeting list", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0");
    await expect(page.getByRole("article")).toHaveCount(3);
    await expect(
      page.getByRole("list", { name: "Standalone meetings" }).getByRole("listitem"),
    ).toHaveCount(4);
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "meeting-list.png") });
  });

  test("processing queue", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0&page=queue");
    await expect(page.getByRole("article")).toHaveCount(4);
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "queue.png") });
  });

  test("transcript", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0&page=transcript&meeting=m5");
    await expect(page.getByRole("img", { name: "Screenshot taken at 0:40" })).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "transcript.png") });
  });

  test("speakers: labeling screen", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.goto("/?view=main&backend=mock&rate=0&page=speakers&meeting=m5");
    await expect(page.getByRole("article")).toHaveCount(4);
    await expect(page.getByText("Recognized automatically · 83%")).toBeVisible();
    await expect(page.getByText("Suggested: Kasia Demo? 61%")).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "speakers-label.png") });
  });

  test("speakers: participants and voice profiles", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1080 });
    await page.goto("/?view=main&backend=mock&rate=0&page=settings&tab=voices");
    const people = page.getByRole("list", { name: "Voice profiles" });
    await expect(people.getByRole("listitem")).toHaveCount(5);
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "speakers-profiles.png") });
  });

  // Stage 4. Run only these with: npx playwright test --project=screenshots -g "project settings|source popover|report mode gate"
  test("project settings", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1100 });
    await page.goto("/?view=main&backend=mock&rate=0&page=project&cycle=NOMAD");
    await page.getByRole("button", { name: "Project settings" }).click();
    await page.getByRole("tab", { name: "People & Glossary" }).click();
    await expect(page.getByRole("heading", { name: "Participants" })).toBeVisible();
    await expect(page.getByLabel("Participant 4 name")).toHaveValue("Piotr Demo");
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "cycle-settings.png") });
  });

  // Stage 6. Run only this one with: npx playwright test --project=screenshots -g "briefing"
  test("briefing", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1250 });
    await page.goto("/?view=main&backend=mock&rate=0&page=project&cycle=NOMAD");
    await expect(page.getByRole("list", { name: "Top action items" })).toBeVisible();
    await expect(page.locator(".brief-status")).toHaveText(/^State as of 2026-11-13\./);
    await expect(page.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "briefing.png") });
  });

  // Stage 6. The popup page of the extension with a stubbed `chrome` API and fictional tabs; it
  // needs `npm run build` in extension/ first. Run with: -g "extension popup"
  test("extension popup", async ({ page }) => {
    const unpacked = path.resolve(here, "../../target/extension/unpacked");
    expect(
      fs.existsSync(path.join(unpacked, "popup.html")),
      "run npm run build in extension/",
    ).toBe(true);
    const types: Record<string, string> = {
      ".html": "text/html",
      ".js": "text/javascript",
    };
    await page.route("https://extension.test/**", async (route) => {
      const name = new URL(route.request().url()).pathname.replace(/^\//, "");
      const file = path.join(unpacked, name);
      if (!file.startsWith(unpacked) || !fs.existsSync(file)) return route.fulfill({ status: 404 });
      return route.fulfill({
        status: 200,
        contentType: types[path.extname(file)] ?? "application/octet-stream",
        body: fs.readFileSync(file),
      });
    });
    await page.addInitScript(() => {
      const group = {
        groupId: 1,
        title: "ORBIT",
        summary: "2/3 tabs",
        tabs: [
          {
            tabId: 11,
            label: "meet.example.org",
            host: "meet.example.org",
            status: "captured",
            active: true,
          },
          {
            tabId: 12,
            label: "docs.example.org",
            host: "docs.example.org",
            status: "captured",
            active: false,
          },
          { tabId: 13, label: "Tab 3", host: "", status: "waiting", active: false },
        ],
        next: 13,
      };
      const status = {
        paired: true,
        clientState: "ready",
        appRecording: true,
        streams: [
          {
            tabId: 11,
            streamId: "s1",
            status: "streaming",
            track: "tab:s1",
            framesSent: 9000,
            framesDropped: 0,
            pendingFrames: 0,
          },
          {
            tabId: 12,
            streamId: "s2",
            status: "streaming",
            track: "tab:s2",
            framesSent: 8800,
            framesDropped: 0,
            pendingFrames: 0,
          },
        ],
        meetTabs: [],
        version: "0.7.0",
      };
      (window as unknown as { chrome: unknown }).chrome = {
        tabs: { query: async () => [{ id: 11, url: "https://meet.example.org/orbit-sync" }] },
        storage: { local: { get: async () => ({}), set: async () => undefined } },
        runtime: {
          sendMessage: async (msg: { type: string }) =>
            msg.type === "group-view" ? { ok: true, group } : { ok: true, status },
        },
      };
    });
    await page.setViewportSize({ width: 324, height: 520 });
    await page.goto("https://extension.test/popup.html");
    await expect(page.locator("#groupTabs li")).toHaveCount(3);
    await expect(page.locator("#line")).toContainText("Connected. The application is recording.");
    await settle(page);
    const height = await page.evaluate(() =>
      Math.ceil(document.getElementById("unpairBtn")!.getBoundingClientRect().bottom + 12),
    );
    await page.screenshot({
      path: path.join(OUT, "extension-popup.png"),
      clip: { x: 0, y: 0, width: 324, height },
    });
  });

  test("draft with the source popover", async ({ page }) => {
    // Tall enough that the whole draft is in view and nothing scrolls.
    await page.setViewportSize({ width: 1280, height: 1250 });
    await page.goto("/?view=main&backend=mock&rate=0&page=draft&meeting=m5");
    await expect(page.getByLabel("Sections", { exact: true }).getByRole("region")).toHaveCount(4);
    await page
      .getByRole("button", { name: "Source of: Update the ground station schedule for Thursday." })
      .click();
    await expect(
      page.getByRole("dialog", {
        name: "Source of: Update the ground station schedule for Thursday.",
      }),
    ).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "draft-source-popover.png") });
  });

  test("report mode gate", async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 780 });
    await page.goto("/?view=widget&backend=mock&rate=0");
    await page.getByLabel("Report mode", { exact: true }).selectOption("by-type");
    await expect(page.getByLabel("Meeting type")).toHaveValue("budget");
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "report-mode-gate.png") });
  });

  test("screenshot description in the transcript", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0&page=transcript&meeting=m5");
    const shot = page.getByRole("img", { name: "Screenshot taken at 2:30" });
    await expect(shot).toBeVisible();
    await shot.scrollIntoViewIfNeeded();
    await settle(page);
    // Only the screenshot with its description and the passages around it.
    const rows = page.getByRole("list", { name: "Transcript" }).getByRole("listitem");
    const row = rows.filter({ has: shot });
    await row.locator("xpath=preceding-sibling::li[2]").scrollIntoViewIfNeeded();
    const box = await row.boundingBox();
    expect(box).not.toBeNull();
    await page.screenshot({
      path: path.join(OUT, "screenshot-description.png"),
      clip: { x: 250, y: Math.max(0, box!.y - 130), width: 1020, height: 330 },
    });
  });

  // Stage 8: the first-run wizard and the readiness banner (setup scenarios of the mock backend).
  // Run only these with: npx playwright test --project=screenshots -g "first run|readiness"
  const setupUrl = (setup: string, jobms = 40) =>
    `/?view=main&backend=mock&rate=0&jobms=${jobms}&setup=${setup}`;
  const stepNav = (page: Page, name: RegExp) =>
    page.getByRole("navigation", { name: "Setup steps" }).getByRole("button", { name }).click();

  test("first run: welcome", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(setupUrl("fresh"));
    await expect(
      page.getByText(
        "This takes 10-30 minutes and needs about 8 GB of free space. Keep the PC on.",
      ),
    ).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "first-run-welcome.png") });
  });

  test("first run: models with a download and the token field", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    // Slow ticks keep the download part way for the picture.
    await page.goto(setupUrl("fresh", 6_000));
    await stepNav(page, /Speech models/);
    await page.getByRole("button", { name: /^Download Whisper turbo/ }).click();
    await expect(
      page.getByRole("progressbar", { name: /^Download of Whisper turbo/ }),
    ).toBeVisible();
    // The token field stays empty and masked: nothing is typed into it.
    await expect(page.getByLabel("Hugging Face token")).toHaveValue("");
    await page.waitForTimeout(2_200);
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "first-run-models.png") });
  });

  test("first run: llama-server not found", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(setupUrl("no-llama"));
    await stepNav(page, /llama-server/);
    await expect(page.getByText("llama-server was not found on this computer.")).toBeVisible();
    await page.getByLabel("Path of llama-server.exe").fill("C:\\Tools\\missing\\llama-server.exe");
    await page.getByRole("button", { name: "Use this llama-server" }).click();
    await expect(page.getByRole("alert")).toContainText("No file exists at");
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "first-run-llama-server.png") });
  });

  test("first run: summary with a missing item and its Fix button", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(setupUrl("no-vision"));
    await stepNav(page, /Summary/);
    await expect(page.getByRole("button", { name: /^Fix / })).toBeVisible();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "first-run-summary.png") });
  });

  test("readiness banner on the meeting page", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(setupUrl("no-vision"));
    await page.getByRole("button", { name: "Leave setup" }).click();
    await expect(page.getByText(/Setup is incomplete/)).toBeVisible();
    await expect(page.getByRole("article")).toHaveCount(3);
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "readiness-banner.png") });
  });

  // Stage 5. Run only these with: npx playwright test --project=screenshots -g "status approval|publishing panel|rendered report|Discord message|token setup"
  test("status approval", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.goto("/?view=main&backend=mock&rate=0&page=draft&meeting=m5");
    const changes = page.getByRole("region", { name: "Status changes" });
    await changes.getByRole("button", { name: "Approve #12" }).click();
    await changes.getByRole("button", { name: "Reject #11" }).click();
    await changes.getByLabel("Comment for #12").fill("Checked on the drawing server.");
    await changes.getByLabel("Comment for #12").blur();
    await expect(changes.getByRole("status")).toContainText("1 of 3 approved, 1 rejected");
    await settle(page);
    await changes.screenshot({ path: path.join(OUT, "status-approval.png") });
  });

  test("publishing panel", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1700 });
    await page.goto("/?view=main&backend=mock&rate=0&jobms=60&page=draft&meeting=m5");
    const changes = page.getByRole("region", { name: "Status changes" });
    for (const target of ["#12", "#11", "t-1"])
      await changes.getByRole("button", { name: `Approve ${target}` }).click();
    const panel = page.getByRole("region", { name: "Publishing" });
    await panel.getByRole("button", { name: "Publish", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
    await dialog.getByRole("checkbox").check();
    await dialog.getByRole("button", { name: "Publish to a public repository" }).click();
    await expect(panel.getByText("Published: everything was sent.")).toBeVisible({
      timeout: 20_000,
    });
    await expect(panel.getByRole("listitem", { name: "Conflict in topic t-1" })).toBeVisible();
    await settle(page);
    await panel.screenshot({ path: path.join(OUT, "publishing-panel.png") });
  });

  test("publishing plan before publishing", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1700 });
    await page.goto("/?view=main&backend=mock&rate=0&jobms=60&page=draft&meeting=m5");
    const changes = page.getByRole("region", { name: "Status changes" });
    for (const target of ["#12", "#11", "t-1"])
      await changes.getByRole("button", { name: `Approve ${target}` }).click();
    const panel = page.getByRole("region", { name: "Publishing" });
    // The Discord message goes out on its own once the draft is complete; GitHub waits.
    await panel
      .getByRole("radiogroup", { name: "Discord publishing rule" })
      .getByRole("radio", { name: "Automatic" })
      .click();
    await expect(panel.getByRole("listitem", { name: "Topic t-1 changed" })).toBeVisible();
    await settle(page);
    await panel.screenshot({ path: path.join(OUT, "publishing-plan.png") });
  });

  test("corrections to a published meeting", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1900 });
    await page.goto("/?view=main&backend=mock&rate=0&jobms=60&page=draft&meeting=m5");
    const panel = page.getByRole("region", { name: "Publishing" });
    await panel.getByRole("button", { name: "Publish", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
    await dialog.getByRole("checkbox").check();
    await dialog.getByRole("button", { name: "Publish to a public repository" }).click();
    await expect(panel.getByText("Published: everything was sent.")).toBeVisible({
      timeout: 20_000,
    });
    const corrections = page.getByRole("region", { name: "Corrections" });
    const add = async (text: string, item?: string) => {
      await corrections.getByRole("button", { name: "Add correction" }).click();
      const form = corrections.getByRole("form", { name: "New correction" });
      await form.getByLabel("Correction text").fill(text);
      if (item) await form.getByLabel("Correction about").selectOption(item);
      await form.getByRole("button", { name: "Add correction" }).click();
    };
    // The first one is about a task that became an issue and is published; the second waits.
    await add("The ground station schedule moved from Thursday to Friday.", "i03");
    await corrections.getByRole("button", { name: "Publish correction c1" }).click();
    await expect(corrections.getByText("and commented on the issue")).toBeVisible({
      timeout: 20_000,
    });
    await add("The test range is booked for 12 December, not 5 December.");
    await corrections.getByRole("button", { name: "Add correction" }).click();
    await corrections.getByLabel("Correction text").fill("The supplier confirmed the delivery.");
    await settle(page);
    await corrections.screenshot({ path: path.join(OUT, "corrections.png") });
  });

  test("example report as rendered Markdown", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.goto("/?view=main&backend=mock&rate=0&page=draft&meeting=m5");
    await page.getByRole("button", { name: "Rendered report" }).click();
    await page.getByRole("button", { name: "Preview", exact: true }).click();
    const report = page.getByRole("article", { name: "Report preview" });
    await expect(report.getByRole("heading", { level: 2 })).toBeVisible();
    await settle(page);
    await report.screenshot({ path: path.join(OUT, "report-rendered.png") });
  });

  test("example Discord message", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await page.goto("/?view=main&backend=mock&rate=0&page=draft&meeting=m5");
    await page.getByRole("button", { name: "Rendered report" }).click();
    await page.getByLabel("Report format").selectOption("chat");
    await page.getByRole("button", { name: "Preview", exact: true }).click();
    const message = page.getByRole("region", { name: "Discord message preview" });
    await expect(message).toContainText("Blah Blah Bot");
    await settle(page);
    await message.screenshot({ path: path.join(OUT, "discord-message.png") });
  });

  test("token setup", async ({ page }) => {
    // Tall enough for the whole page: an element taller than the window is stitched badly.
    await page.setViewportSize({ width: 1280, height: 2300 });
    await page.goto("/?view=main&backend=mock&rate=0&page=settings&tab=connectors");
    const connectors = page.getByRole("tabpanel", { name: "Connectors" });
    await expect(
      connectors.getByRole("form", { name: "GitHub Token (api.github.com)" }),
    ).toContainText("Token stored");
    await settle(page);
    await connectors.screenshot({ path: path.join(OUT, "token-setup.png") });
  });

  // Stage 7. Run only these with: npx playwright test --project=screenshots -g "Notion mapping|Discord bot setup"
  test("Notion mapping", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 2400 });
    await page.goto("/?view=main&backend=mock&rate=0&page=project&cycle=NOMAD");
    await page.getByRole("button", { name: "Project settings" }).click();
    await page.getByRole("tab", { name: "Integrations" }).click();
    await page.getByRole("checkbox", { name: "Export tasks to a Notion database" }).check();
    await page.getByRole("button", { name: "Notion details" }).click();
    await page.getByLabel("Database id").fill("0123456789abcdef0123456789abcdef");
    await page.getByRole("button", { name: "Save project settings" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Project settings saved." }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).first().click();

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("tab", { name: "Connectors" }).click();
    const notionForm = page.getByRole("form", { name: "Notion token for cred:notion/nomad" });
    await notionForm
      .getByLabel("Notion token for cred:notion/nomad")
      .fill(["ntn", "demoSecretIntegrationToken0123456789"].join("_"));
    await notionForm.getByRole("button", { name: "Store" }).click();
    await expect(notionForm).toContainText("Token stored: ntn_...6789");

    await page.getByRole("button", { name: "Projects", exact: true }).click();
    await page
      .getByRole("article", { name: /Project .*NOMAD/i })
      .getByRole("button", { name: "Open project" })
      .click();
    await page.getByRole("button", { name: "Project settings" }).click();
    await page.getByRole("tab", { name: "Integrations" }).click();
    await page.getByRole("button", { name: "Notion details" }).click();
    await page.getByRole("button", { name: "Inspect database & auto-map" }).click();
    await expect(
      page.getByText(/Connected to Notion as "Nomad notes": database "Tasks"/),
    ).toBeVisible();
    await settle(page);
    const fieldset = page
      .locator("fieldset")
      .filter({ hasText: "Export tasks to a Notion database" });
    await fieldset.screenshot({ path: path.join(OUT, "notion-mapping.png") });
  });

  test("Discord bot setup", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 2400 });
    await page.goto("/?view=main&backend=mock&rate=0&page=project&cycle=NOMAD");
    await page.getByRole("button", { name: "Project settings" }).click();
    await page.getByRole("tab", { name: "Integrations" }).click();
    await page.getByRole("button", { name: "Discord details" }).click();
    await page
      .getByRole("radiogroup", { name: "Discord delivery mode" })
      .getByRole("radio", { name: "Bot" })
      .click();
    await page.getByLabel("Server (guild) id").fill("111111111111111111");
    await page.getByLabel("Channel id").fill("222222222222222222");
    await page.getByRole("button", { name: "Save project settings" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Project settings saved." }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close", exact: true }).first().click();

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("tab", { name: "Connectors" }).click();
    const botForm = page.getByRole("form", { name: "Bot token for cred:discord-bot/nomad" });
    await botForm
      .getByLabel("Bot token for cred:discord-bot/nomad")
      .fill(
        ["MTIzNDU2Nzg5MDEyMzQ1Njc4", "Gdemo1", "DemoBotSecretSignature0123456789abcdef"].join("."),
      );
    await botForm.getByRole("button", { name: "Store" }).click();
    await expect(botForm).toContainText("Bot token stored: bot MTIz...");

    await page.getByRole("button", { name: "Projects", exact: true }).click();
    await page
      .getByRole("article", { name: /Project .*NOMAD/i })
      .getByRole("button", { name: "Open project" })
      .click();
    await page.getByRole("button", { name: "Project settings" }).click();
    await page.getByRole("tab", { name: "Integrations" }).click();
    await page.getByRole("button", { name: "Discord details" }).click();
    await page.getByRole("button", { name: "Check channel & permissions" }).click();
    await expect(
      page.getByText('Connected as "Nomad notes" to #meetings in Nomad team.'),
    ).toBeVisible();
    const fieldset = page
      .locator("fieldset")
      .filter({ hasText: "Post a summary to a Discord channel" });
    await fieldset
      .getByLabel("Discord Application ID (for invite link)")
      .fill("123456789012345678");
    await settle(page);
    await fieldset.screenshot({ path: path.join(OUT, "discord-bot-setup.png") });
  });

  // Gap fixes. Run only these with: npx playwright test --project=screenshots -g "gap fixes"
  test.describe("gap fixes", () => {
    // Numbers in fields are written the same way on every machine: the page language and the
    // language of the browser itself (which sets the decimal mark of a number field).
    test.use({ locale: "en-US" });

    test("record view in the main window", async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto("/?view=main&backend=mock&rate=0&tabs=1&page=record");
      await expect(page.getByRole("heading", { name: "Record", level: 2 })).toBeVisible();
      await expect(page.getByRole("complementary", { name: "Recording tips" })).toBeVisible();
      await page.getByLabel("Discord").check();
      await fitWindowToPage(page);
      await page.screenshot({ path: path.join(OUT, "record-view.png") });
    });

    test("new project wizard: private GitHub repository", async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 1100 });
      await page.goto("/?view=main&backend=mock&rate=0&page=projects");
      await page.getByRole("button", { name: "New Project" }).click();
      const wizard = page.getByRole("dialog", { name: "Create New Project" });
      await wizard.getByLabel("Project name").fill("Lunar Scout");
      await wizard.getByRole("button", { name: "Next" }).click();
      await wizard.getByLabel("Create a private GitHub repository").check();
      await expect(wizard.getByText("A saved GitHub token was found")).toBeVisible();
      await expectDialogNotClipped(wizard);
      await settle(page);
      await wizard.screenshot({ path: path.join(OUT, "project-wizard.png") });

      // The confirm step: nothing is sent to GitHub before the user agrees.
      await wizard.getByRole("button", { name: "Create project" }).click();
      const confirm = wizard.getByRole("region", { name: "Confirm the new GitHub repository" });
      await expect(confirm).toContainText("Nothing has been sent to GitHub yet");
      await expectDialogNotClipped(wizard);
      await settle(page);
      await wizard.screenshot({ path: path.join(OUT, "project-wizard-github-confirm.png") });
    });

    test("new project wizard: local-only fallback", async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 1100 });
      await page.goto("/?view=main&backend=mock&rate=0&ghtoken=none&page=projects");
      await page.getByRole("button", { name: "New Project" }).click();
      const wizard = page.getByRole("dialog", { name: "Create New Project" });
      await wizard.getByLabel("Project name").fill("Lunar Scout");
      await wizard.getByRole("button", { name: "Next" }).click();
      await wizard.getByLabel("Create a private GitHub repository").check();
      await expect(wizard.getByText("No GitHub token is saved yet")).toBeVisible();
      await wizard.getByRole("button", { name: "Create project" }).click();
      await expect(wizard.getByRole("status")).toContainText("No GitHub token is saved");
      await expect(wizard.getByRole("button", { name: "Open project" })).toBeVisible();
      await expectDialogNotClipped(wizard);
      await settle(page);
      await wizard.screenshot({ path: path.join(OUT, "project-wizard-local-only.png") });
    });

    test("import meeting with the file picker", async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 1100 });
      // The mock file dialog answers with this path instead of opening a window.
      await page.addInitScript(() => {
        window.__bbbPick = { file: "C:\\Recordings\\ground-station-sync.mp4" };
      });
      await page.goto("/?view=main&backend=mock&rate=0&page=project&cycle=NOMAD");
      await page.getByRole("button", { name: "+ Add meeting" }).click();
      const modal = page.getByRole("dialog", { name: "Add Meeting from Recording" });
      await modal.getByRole("button", { name: "Browse for the audio or video file" }).click();
      await expect(modal.getByLabel("Audio or video file path")).toHaveValue(
        "C:\\Recordings\\ground-station-sync.mp4",
      );
      await expect(modal.getByRole("radiogroup", { name: "Video handling mode" })).toBeVisible();
      await expectDialogNotClipped(modal);
      await settle(page);
      await modal.screenshot({ path: path.join(OUT, "import-meeting.png") });
    });

    for (const [tab, file, ready] of [
      ["models", "settings-ai-models.png", "Context size"],
      ["preferences", "settings-preferences.png", "Local notes folder"],
      ["processing", "settings-processing.png", "Max GPU utilization (%)"],
    ] as const) {
      test(`settings: ${tab} tab`, async ({ page }) => {
        await page.setViewportSize({ width: 1280, height: 800 });
        await page.goto(`/?view=main&backend=mock&rate=0&page=settings&tab=${tab}`);
        await expect(page.getByRole("tab", { selected: true })).toBeVisible();
        await expect(page.getByLabel(ready, { exact: false }).first()).toBeAttached();
        await fitWindowToPage(page);
        await page.screenshot({ path: path.join(OUT, file) });
      });
    }

    test("unsaved changes dialog", async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto("/?view=main&backend=mock&rate=0&page=settings&tab=processing");
      await page.getByLabel("Max GPU utilization (%)").fill("65");
      await page.getByRole("tab", { name: "Voice Profiles" }).click();
      const dialog = page.getByRole("dialog", { name: "You have unsaved changes" });
      await expect(dialog).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(OUT, "unsaved-changes.png") });
    });
  });
});

test("record flow GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 340, height: 680 });
  await page.goto("/?view=widget&backend=mock");
  await gif.hold(350);
  await page.getByLabel("Discord").check();
  await gif.hold(140);
  await page.getByRole("button", { name: "Record" }).click();
  await expect(page.getByText("Recording", { exact: true })).toBeVisible();
  await gif.burst(6, 100);
  await page.getByLabel("Add source").selectOption({ label: "Chrome (whole browser)" });
  await gif.hold(120);
  await page.getByRole("button", { name: /Screenshot/ }).click();
  await gif.burst(4, 100);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(page.getByRole("dialog", { name: "Process now or later?" })).toBeVisible();
  await gif.hold(160);
  gif.save("record-flow.gif", 1_500_000);
});

test("processing GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 960, height: 640 });
  await page.goto("/?view=main&backend=mock&jobms=450");
  await expect(page.getByRole("article")).toHaveCount(3);
  await gif.hold(350);
  const row = page
    .getByRole("list", { name: "Standalone meetings" })
    .getByRole("listitem")
    .filter({ hasText: "Kickoff" });
  await row.getByRole("button", { name: "Process" }).click();
  await gif.hold(140);
  await page.getByRole("button", { name: "Queue", exact: true }).click();
  const job = page.getByRole("article", { name: "Kickoff" });
  await expect(job).toBeVisible();
  await gif.snap(TRANSITION_MS, 80);
  const doneBadge = job.getByText("Processed", { exact: true });
  for (let i = 0; i < 45; i++) {
    if (await doneBadge.isVisible()) break;
    await gif.snap(TRANSITION_MS, 100);
  }
  await expect(doneBadge).toBeVisible({ timeout: 20_000 });
  await gif.hold(160);
  gif.save("processing.gif", 2_000_000);
});

test("naming voices GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 960, height: 760 });
  await page.goto("/?view=main&backend=mock&rate=0&page=speakers&meeting=m5");
  const voice = (label: string) => page.getByRole("article", { name: label });
  await expect(page.getByRole("article")).toHaveCount(4);
  await gif.hold(350);
  // Listen to a clip of the first unknown voice.
  await page.getByRole("button", { name: "Play clip 1 of SPEAKER_02" }).click();
  await gif.burst(3, 110);
  const first = voice("SPEAKER_02");
  await first.getByLabel("Name for SPEAKER_02").selectOption({ label: "New participant…" });
  await gif.snap(TRANSITION_MS, 90);
  const nameInput = first.getByLabel(/Name of the new participant/);
  await nameInput.fill("Mira");
  await gif.snap(TRANSITION_MS, 80);
  await nameInput.fill("Mira Demo");
  await gif.hold(120);
  await first.getByRole("button", { name: "Create and name" }).click();
  await expect(first.getByText("Named by you")).toBeVisible();
  await gif.hold(140);
  // Accept the suggestion and name the second unknown voice from the list.
  await voice("SPEAKER_01")
    .getByRole("button", { name: /^Accept/ })
    .click();
  await gif.hold(140);
  await voice("SPEAKER_03").getByLabel("Name for SPEAKER_03").selectOption({ label: "Jan Demo" });
  await expect(page.getByRole("status")).toContainText("3 of 4");
  await gif.hold(180);
  gif.save("naming-voices.gif", 1_500_000);
});

test("draft editing GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 960, height: 900 });
  await page.goto("/?view=main&backend=mock&rate=0&page=transcript&meeting=m5");
  await expect(page.getByRole("img", { name: "Screenshot taken at 0:40" })).toBeVisible();
  await gif.hold(350);
  // Switch from the Transcript tab directly to the Draft & Report tab.
  await page.getByRole("tab", { name: "Draft & Report" }).click();
  const columns = page.getByLabel("Sections", { exact: true });
  await expect(columns.getByRole("region")).toHaveCount(4);
  await gif.hold(160);
  // The source of an item: the quote and the time in the meeting.
  const ground = "Update the ground station schedule for Thursday.";
  await page.getByRole("button", { name: `Source of: ${ground}` }).click();
  await expect(page.getByRole("dialog", { name: `Source of: ${ground}` })).toBeVisible();
  await gif.hold(180);
  await page.getByRole("button", { name: `Source of: ${ground}` }).click();
  await gif.snap(TRANSITION_MS, 90);
  // A template with its own sections.
  await page.getByLabel("Report mode", { exact: true }).selectOption("template");
  const template = page.getByLabel("Template", { exact: true });
  await expect(template).toBeVisible();
  await gif.snap(TRANSITION_MS, 90);
  await template.selectOption("budget-review");
  await expect(template).toHaveValue("budget-review");
  await expect(columns.getByRole("region").first()).toBeVisible();
  await gif.hold(160);
  // Move an item to another section from its menu.
  const move = page.getByLabel(/^Move to section:/).first();
  const text = (await move.getAttribute("aria-label"))!.replace("Move to section: ", "");
  const options = await move.locator("option").allTextContents();
  const target = options.filter((o) => o !== "Move to…")[0]!;
  await move.focus();
  await gif.snap(TRANSITION_MS, 90);
  await move.selectOption({ label: target });
  await expect(
    page
      .getByRole("list", { name: `Items of ${target}` })
      .getByRole("button", { name: text, exact: true }),
  ).toBeVisible();
  await gif.hold(160);
  await page.getByRole("button", { name: "Rendered report" }).click();
  await expect(page.getByLabel("Rendered report")).toContainText(text);
  await gif.hold(180);
  gif.save("draft-editing.gif", 1_500_000);
});

test("hero GIF", async ({ page }) => {
  // The whole flow in demo mode: first-run setup, processing, naming voices and the report.
  const gif = createGifRecorder(page);
  const next = () => page.getByRole("button", { name: "Next", exact: true }).click();
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/?view=main&backend=mock&rate=0&jobms=450&setup=fresh");
  await expect(
    page.getByText("This takes 10-30 minutes and needs about 8 GB of free space. Keep the PC on."),
  ).toBeVisible();
  await gif.hold(320);
  await next();
  await page.getByRole("button", { name: "Prepare environment" }).click();
  await expect(page.getByRole("progressbar", { name: "Preparing the environment" })).toBeVisible();
  await gif.burst(3, 100);
  await expect(page.getByText("The processing environment is ready.")).toBeVisible();
  await gif.snap(TRANSITION_MS, 80);
  await next();
  await page.getByRole("button", { name: /^Download Whisper turbo/ }).click();
  await expect(page.getByRole("progressbar", { name: /^Download of Whisper turbo/ })).toBeVisible();
  await gif.burst(3, 120);
  await expect(page.getByText("Downloaded", { exact: true })).toBeVisible();
  await page.getByLabel("Hugging Face token").fill("hf_demo_token");
  await page.getByRole("button", { name: "Store token" }).click();
  await expect(page.getByText("A token is stored")).toBeVisible();
  await page.getByRole("button", { name: /^Download Speaker/ }).click();
  await expect(page.getByText("Downloaded", { exact: true })).toHaveCount(2);
  await gif.snap(TRANSITION_MS, 80);
  await next();
  await page.getByRole("radio", { name: /llama-server\.exe/ }).check();
  await page.getByRole("button", { name: "Use this llama-server" }).click();
  await expect(page.getByText(/^In use:/)).toBeVisible();
  await next();
  await page.getByRole("button", { name: "Find model files" }).click();
  await page.getByRole("radio", { name: /gemma-4-12b/ }).check();
  await page.getByRole("button", { name: "Use this model" }).click();
  await expect(page.getByText("A language model is chosen.")).toBeVisible();
  await next();
  await page.getByRole("button", { name: /^Download Screenshot descriptions/ }).click();
  await expect(page.getByText("The screenshot model is downloaded.")).toBeVisible();
  await next();
  await expect(page.getByText("Everything needed is ready.")).toBeVisible();
  await gif.hold(200);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await page.getByLabel("Project id").fill("ORBIT");
  await page.getByLabel("Project name").fill("Orbit weekly");
  await page.getByRole("button", { name: "Create first project" }).click();
  await expect(page.getByText("Project ORBIT was created.")).toBeVisible();
  await gif.snap(TRANSITION_MS, 80);
  await page.getByRole("button", { name: "Finish" }).click();
  await expect(page.getByRole("heading", { name: "Projects", level: 2 })).toBeVisible();
  await gif.hold(200);
  // Process a meeting and follow its stages in the queue.
  const row = page
    .getByRole("list", { name: "Standalone meetings" })
    .getByRole("listitem")
    .filter({ hasText: "Kickoff" });
  await row.getByRole("button", { name: "Process" }).click();
  await page.getByRole("button", { name: "Queue", exact: true }).click();
  const job = page.getByRole("article", { name: "Kickoff" });
  await expect(job).toBeVisible();
  const doneBadge = job.getByText("Processed", { exact: true });
  for (let i = 0; i < 30; i++) {
    if (await doneBadge.isVisible()) break;
    await gif.snap(TRANSITION_MS, 100);
  }
  await expect(doneBadge).toBeVisible({ timeout: 20_000 });
  await gif.hold(160);
  // Speakers and the report of a processed meeting (a fresh mock page each).
  await page.goto("/?view=main&backend=mock&rate=0&page=speakers&meeting=m5");
  await expect(page.getByRole("article")).toHaveCount(4);
  await gif.hold(160);
  await page.getByLabel("Name for SPEAKER_03").selectOption({ label: "Jan Demo" });
  await gif.hold(200);
  await page.goto("/?view=main&backend=mock&rate=0&page=draft&meeting=m5");
  await expect(page.getByLabel("Sections", { exact: true }).getByRole("region")).toHaveCount(4);
  await gif.hold(160);
  const ground = "Update the ground station schedule for Thursday.";
  await page.getByRole("button", { name: `Source of: ${ground}` }).click();
  await expect(page.getByRole("dialog", { name: `Source of: ${ground}` })).toBeVisible();
  await gif.hold(240);
  gif.save("hero.gif", 4_000_000);
});

test("approve statuses and publish GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 960, height: 820 });
  await page.goto("/?view=main&backend=mock&rate=0&jobms=250&page=draft&meeting=m5");
  const changes = page.getByRole("region", { name: "Status changes" });
  await expect(changes.getByRole("listitem")).toHaveCount(3);
  await changes
    .getByRole("heading", { name: "Status changes" })
    .evaluate((el) => el.scrollIntoView({ block: "start" }));
  await gif.hold(250);
  // Each status change shows its quote; the user decides.
  await changes.getByRole("button", { name: "Approve #12" }).click();
  await gif.snap(TRANSITION_MS, 90);
  await changes.getByRole("button", { name: "Approve #11" }).click();
  await gif.snap(TRANSITION_MS, 90);
  await changes.getByRole("button", { name: "Approve t-1" }).click();
  await expect(changes.getByRole("status")).toContainText("3 of 3 approved");
  await gif.hold(140);
  // Smooth scroll to the publishing panel and topics diff, then Publish.
  const panel = page.getByRole("region", { name: "Publishing" });
  await gif.smoothScrollTo(panel.getByRole("heading", { name: "Publishing" }), 2);
  await gif.hold(120);
  await gif.smoothScrollTo(
    panel.getByRole("heading", { name: "topics.yaml: before and after" }),
    2,
  );
  await gif.hold(140);
  await panel.getByRole("button", { name: "Publish", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
  await expect(dialog).toBeVisible();
  await gif.hold(140);
  await dialog.getByRole("checkbox").check();
  await gif.snap(TRANSITION_MS, 90);
  await dialog.getByRole("button", { name: "Publish to a public repository" }).click();
  await gif.smoothScrollTo(panel.getByRole("heading", { name: "Outbox" }), 2);
  const publishedBanner = panel.getByText("Published: everything was sent.");
  for (let i = 0; i < 20; i++) {
    if (await publishedBanner.isVisible()) break;
    await gif.snap(TRANSITION_MS, 120);
  }
  await expect(publishedBanner).toBeVisible({ timeout: 30_000 });
  await gif.hold(140);
  await gif.smoothScrollTo(panel.getByRole("heading", { name: "topics.yaml", exact: true }), 2);
  await gif.hold(180);
  gif.save("approve-and-publish.gif", 2_000_000);
});

// Stage 6. Run only this one with: npx playwright test --project=screenshots -g "detect and record"
test("detect and record GIF", async ({ page }) => {
  const gif = createGifRecorder(page);
  await page.setViewportSize({ width: 340, height: 600 });
  // The mock raises a prompt for a fictional Discord call shortly after it starts.
  await page.goto("/?view=widget&backend=mock&tabs=1&detectms=900");
  await gif.hold(250);
  const prompt = page.getByRole("alertdialog", { name: "Record this call?" });
  await expect(prompt).toBeVisible();
  await gif.hold(160);
  await prompt.getByRole("button", { name: "Record" }).click();
  await expect(page.getByText("Recording", { exact: true })).toBeVisible();
  await gif.burst(6, 100);
  await page
    .getByLabel("Add source")
    .selectOption({ label: "Tab: Budget sheet (sheets.example.org)" });
  await gif.burst(5, 100);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(page.getByRole("dialog", { name: "Process now or later?" })).toBeVisible();
  await gif.hold(160);
  gif.save("detect-and-record.gif", 1_500_000);
});
