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

test.use({ colorScheme: "dark", deviceScaleFactor: 1 });

// The frame folders of the GIFs live in <repo>/target/e2e-tmp and go after every test.
test.afterEach(() => {
  cleanupDirs();
});

async function settle(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
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
    await page.setViewportSize({ width: 340, height: 420 });
    await page.goto("/?view=widget&backend=mock&rate=0");
    await page.getByLabel("Discord").check();
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "source-picker.png") });
  });

  test("meeting list", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0");
    await expect(page.getByRole("article")).toHaveCount(5);
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
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock&rate=0&page=settings");
    const people = page.getByRole("list", { name: "Participants" });
    await expect(people.getByRole("listitem")).toHaveCount(3);
    await page
      .getByRole("heading", { name: "Voice profiles" })
      .evaluate((el) => el.scrollIntoView({ block: "start" }));
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "speakers-profiles.png") });
  });

  // Stage 4. Run only these with: npx playwright test --project=screenshots -g "cycle settings|source popover|report mode gate"
  test("cycle settings", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1100 });
    await page.goto("/?view=main&backend=mock&rate=0&page=cycle&cycle=NOMAD");
    await expect(page.getByRole("heading", { name: "Participants" })).toBeVisible();
    await expect(page.getByLabel("Participant 4 name")).toHaveValue("Piotr Demo");
    await settle(page);
    await page.screenshot({ path: path.join(OUT, "cycle-settings.png") });
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
    await page.setViewportSize({ width: 360, height: 640 });
    await page.goto("/?view=widget&backend=mock&rate=0");
    await page.getByRole("button", { name: "Record" }).click();
    const gate = page.getByRole("dialog", { name: "Choose the report for this recording" });
    await gate.getByRole("button", { name: "Change…" }).click();
    await gate.getByLabel("Report mode").selectOption("by-type");
    await expect(gate.getByLabel("Meeting type")).toHaveValue("budget");
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
    await page.goto("/?view=main&backend=mock&rate=0&page=settings");
    const publishing = page.getByRole("region", { name: "Publishing" });
    // The GitHub token is stored; the webhook is typed (masked) and not stored yet.
    const webhook = publishing.getByRole("form", { name: "Webhook for cred:discord/nomad" });
    await webhook.getByRole("button", { name: "Clear" }).click();
    await expect(webhook).toContainText("No webhook stored");
    await webhook
      .getByLabel("Webhook for cred:discord/nomad")
      .fill("https://discord.com/api/webhooks/000000000000000007/demo-token");
    await expect(
      publishing.getByRole("form", { name: "GitHub token for api.github.com" }),
    ).toContainText("Token stored");
    await webhook.getByLabel("Webhook for cred:discord/nomad").blur();
    await settle(page);
    await publishing.screenshot({ path: path.join(OUT, "token-setup.png") });
  });

  // Stage 7. Run only these with: npx playwright test --project=screenshots -g "Notion mapping|Discord bot setup"
  test("Notion mapping", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 2400 });
    await page.goto("/?view=main&backend=mock&rate=0&page=cycle&cycle=NOMAD");
    await page.getByRole("checkbox", { name: "Export tasks to a Notion database" }).check();
    await page.getByLabel("Database id").fill("0123456789abcdef0123456789abcdef");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const notionForm = page.getByRole("form", { name: "Notion token for cred:notion/nomad" });
    await notionForm
      .getByLabel("Notion token for cred:notion/nomad")
      .fill(["ntn", "demoSecretIntegrationToken0123456789"].join("_"));
    await notionForm.getByRole("button", { name: "Store" }).click();
    await expect(notionForm).toContainText("Token stored: ntn_...6789");

    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Edit NOMAD" }).click();
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
    await page.goto("/?view=main&backend=mock&rate=0&page=cycle&cycle=NOMAD");
    await page
      .getByRole("radiogroup", { name: "Discord delivery mode" })
      .getByRole("radio", { name: "Bot" })
      .click();
    await page.getByLabel("Server (guild) id").fill("111111111111111111");
    await page.getByLabel("Channel id").fill("222222222222222222");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const botForm = page.getByRole("form", { name: "Bot token for cred:discord-bot/nomad" });
    await botForm
      .getByLabel("Bot token for cred:discord-bot/nomad")
      .fill(["MTIzNDU2Nzg5MDEyMzQ1Njc4", "Gdemo1", "DemoBotSecretSignature0123456789abcdef"].join("."));
    await botForm.getByRole("button", { name: "Store" }).click();
    await expect(botForm).toContainText("Bot token stored: bot MTIz...");

    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Edit NOMAD" }).click();
    await page.getByRole("button", { name: "Check channel & permissions" }).click();
    await expect(
      page.getByText('Connected as "Nomad notes" to #meetings in Nomad team.'),
    ).toBeVisible();
    const fieldset = page.locator("fieldset").filter({ hasText: "Post a summary to a channel" });
    await fieldset
      .getByLabel("Discord Application ID (for invite link)")
      .fill("123456789012345678");
    await settle(page);
    await fieldset.screenshot({ path: path.join(OUT, "discord-bot-setup.png") });
  });
});

test("record flow GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = freshDir("gif");
  const shot = async () => {
    await page.waitForTimeout(450);
    const file = path.join(dir, `f${String(frames.length).padStart(2, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push(file);
  };
  await page.setViewportSize({ width: 340, height: 420 });
  await page.goto("/?view=widget&backend=mock");
  await shot();
  await page.getByLabel("Discord").check();
  await shot();
  await page.getByRole("button", { name: "Record" }).click();
  // The report mode gate comes first, with the last choice ready to confirm.
  await shot();
  await page.getByRole("button", { name: "Start recording" }).click();
  await shot();
  await page.waitForTimeout(1_200);
  await shot();
  await page.getByLabel("Add source").selectOption({ label: "Google Chrome" });
  await shot();
  await page.getByRole("button", { name: /Screenshot/ }).click();
  await shot();
  await page.waitForTimeout(900);
  await shot();
  await page.getByRole("button", { name: "Stop" }).click();
  await shot();
  fs.mkdirSync(OUT, { recursive: true });
  execFileSync("python", [
    path.join(here, "make-gif.py"),
    path.join(OUT, "record-flow.gif"),
    "900",
    ...frames,
  ]);
  expect(fs.statSync(path.join(OUT, "record-flow.gif")).size).toBeLessThan(10_000_000);
});

test("processing GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = freshDir("gif");
  const shot = async (wait = 350) => {
    await page.waitForTimeout(wait);
    const file = path.join(dir, `f${String(frames.length).padStart(2, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push(file);
  };
  await page.setViewportSize({ width: 960, height: 640 });
  await page.goto("/?view=main&backend=mock&jobms=1400");
  await shot();
  const card = page.getByRole("article").filter({ hasText: "Kickoff" });
  await card.getByRole("button", { name: "Process" }).click();
  await shot();
  await page.getByRole("button", { name: "Queue", exact: true }).click();
  const job = page.getByRole("article", { name: "Kickoff" });
  await expect(job).toBeVisible();
  // One frame per stage while the simulated pipeline advances.
  for (let i = 0; i < 6; i++) await shot(1_300);
  await expect(job.getByText("Processed", { exact: true })).toBeVisible({ timeout: 20_000 });
  await shot();
  fs.mkdirSync(OUT, { recursive: true });
  execFileSync("python", [
    path.join(here, "make-gif.py"),
    path.join(OUT, "processing.gif"),
    "900",
    ...frames,
  ]);
  expect(fs.statSync(path.join(OUT, "processing.gif")).size).toBeLessThan(10_000_000);
});

test("naming voices GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = freshDir("gif");
  const shot = async (wait = 350) => {
    await page.waitForTimeout(wait);
    const file = path.join(dir, `f${String(frames.length).padStart(2, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push(file);
  };
  await page.setViewportSize({ width: 960, height: 760 });
  await page.goto("/?view=main&backend=mock&rate=0&page=speakers&meeting=m5");
  const voice = (label: string) => page.getByRole("article", { name: label });
  await expect(page.getByRole("article")).toHaveCount(4);
  await shot();
  // Listen to a clip of the first unknown voice.
  await page.getByRole("button", { name: "Play clip 1 of SPEAKER_02" }).click();
  await shot();
  const first = voice("SPEAKER_02");
  await first.getByLabel("Name for SPEAKER_02").selectOption({ label: "New participant…" });
  await first.getByLabel(/Name of the new participant/).fill("Mira Demo");
  await shot();
  await first.getByRole("button", { name: "Create and name" }).click();
  await expect(first.getByText("Named by you")).toBeVisible();
  await shot();
  // Accept the suggestion and name the second unknown voice from the list.
  await voice("SPEAKER_01")
    .getByRole("button", { name: /^Accept/ })
    .click();
  await shot();
  await voice("SPEAKER_03").getByLabel("Name for SPEAKER_03").selectOption({ label: "Jan Demo" });
  await expect(page.getByRole("status")).toContainText("3 of 4");
  await shot(600);
  fs.mkdirSync(OUT, { recursive: true });
  execFileSync("python", [
    path.join(here, "make-gif.py"),
    path.join(OUT, "naming-voices.gif"),
    "1400",
    ...frames,
  ]);
  expect(fs.statSync(path.join(OUT, "naming-voices.gif")).size).toBeLessThan(10_000_000);
});

test("draft editing GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = freshDir("gif");
  const shot = async (wait = 350) => {
    await page.waitForTimeout(wait);
    const file = path.join(dir, `f${String(frames.length).padStart(2, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push(file);
  };
  await page.setViewportSize({ width: 1000, height: 1000 });
  await page.goto("/?view=main&backend=mock&rate=0&page=transcript&meeting=m5");
  await expect(page.getByRole("img", { name: "Screenshot taken at 0:40" })).toBeVisible();
  await shot();
  // From the transcript back to the list, then into the draft of the meeting.
  await page.getByRole("button", { name: "Back to meetings" }).click();
  const open = page.getByRole("button", { name: /^Open the draft of / }).first();
  await expect(open).toBeVisible();
  await shot();
  await open.click();
  const columns = page.getByLabel("Sections", { exact: true });
  await expect(columns.getByRole("region")).toHaveCount(4);
  await shot();
  // The source of an item: the quote and the time in the meeting.
  const ground = "Update the ground station schedule for Thursday.";
  await page.getByRole("button", { name: `Source of: ${ground}` }).click();
  await expect(page.getByRole("dialog", { name: `Source of: ${ground}` })).toBeVisible();
  await shot();
  await page.getByRole("button", { name: `Source of: ${ground}` }).click();
  // A template with its own sections.
  await page.getByLabel("Report mode").selectOption("template");
  const template = page.getByLabel("Template", { exact: true });
  await expect(template).toBeVisible();
  await template.selectOption("budget-review");
  await expect(template).toHaveValue("budget-review");
  await expect(columns.getByRole("region").first()).toBeVisible();
  await shot(600);
  // Move an item to another section from its menu.
  const move = page.getByLabel(/^Move to section:/).first();
  const text = (await move.getAttribute("aria-label"))!.replace("Move to section: ", "");
  const options = await move.locator("option").allTextContents();
  const target = options.filter((o) => o !== "Move to…")[0]!;
  await move.focus();
  await shot();
  await move.selectOption({ label: target });
  await expect(
    page
      .getByRole("list", { name: `Items of ${target}` })
      .getByRole("button", { name: text, exact: true }),
  ).toBeVisible();
  await shot(600);
  await page.getByRole("button", { name: "Rendered report" }).click();
  await expect(page.getByLabel("Rendered report")).toContainText(text);
  await shot(600);
  fs.mkdirSync(OUT, { recursive: true });
  execFileSync("python", [
    path.join(here, "make-gif.py"),
    path.join(OUT, "draft-editing.gif"),
    "1300",
    ...frames,
  ]);
  expect(fs.statSync(path.join(OUT, "draft-editing.gif")).size).toBeLessThan(1_500_000);
});

test("approve statuses and publish GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = freshDir("gif");
  const shot = async (wait = 350) => {
    await page.waitForTimeout(wait);
    const file = path.join(dir, `f${String(frames.length).padStart(2, "0")}.png`);
    await page.screenshot({ path: file });
    frames.push(file);
  };
  await page.setViewportSize({ width: 1000, height: 820 });
  await page.goto("/?view=main&backend=mock&rate=0&jobms=900&page=draft&meeting=m5");
  const changes = page.getByRole("region", { name: "Status changes" });
  await expect(changes.getByRole("listitem")).toHaveCount(3);
  const top = (locator: ReturnType<Page["locator"]>) =>
    locator.evaluate((el) => el.scrollIntoView({ block: "start" }));
  await top(changes.getByRole("heading", { name: "Status changes" }));
  await shot();
  // Each status change shows its quote; the user decides.
  await changes.getByRole("button", { name: "Approve #12" }).click();
  await shot();
  await changes.getByRole("button", { name: "Approve #11" }).click();
  await shot();
  await changes.getByRole("button", { name: "Approve t-1" }).click();
  await expect(changes.getByRole("status")).toContainText("3 of 3 approved");
  await shot();
  // The publishing panel: what will happen, then Publish, with the public repository asked once.
  const panel = page.getByRole("region", { name: "Publishing" });
  await top(panel.getByRole("heading", { name: "Publishing" }));
  await shot();
  // Before it is published the plan shows each topic of topics.yaml, before and after.
  await top(panel.getByRole("heading", { name: "topics.yaml: before and after" }));
  await shot();
  await panel.getByRole("button", { name: "Publish", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
  await expect(dialog).toBeVisible();
  await shot();
  await dialog.getByRole("checkbox").check();
  await shot();
  await dialog.getByRole("button", { name: "Publish to a public repository" }).click();
  await top(panel.getByRole("heading", { name: "Outbox" }));
  // The outbox runs one operation at a time.
  for (let i = 0; i < 4; i++) await shot(1_700);
  await expect(panel.getByText("Published: everything was sent.")).toBeVisible({
    timeout: 30_000,
  });
  await shot(600);
  await top(panel.getByRole("heading", { name: "topics.yaml", exact: true }));
  await shot(600);
  fs.mkdirSync(OUT, { recursive: true });
  execFileSync("python", [
    path.join(here, "make-gif.py"),
    path.join(OUT, "approve-and-publish.gif"),
    "1300",
    ...frames,
  ]);
  expect(fs.statSync(path.join(OUT, "approve-and-publish.gif")).size).toBeLessThan(3_000_000);
});
