/**
 * Reproducible screenshots and the demo GIF, taken from the UI in demo mode (mock backend,
 * fictional cycle, frozen clock). Run: npm run screenshots
 *
 * Fixed window size, dark theme, scale 100% and reduced motion: running it twice on the
 * same commit produces identical PNG files.
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { type Page, expect, test } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(here, "../../docs/img");

test.use({ colorScheme: "dark", deviceScaleFactor: 1 });

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
});

test("record flow GIF", async ({ page }) => {
  const frames: string[] = [];
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bbb-gif-"));
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
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bbb-gif-"));
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
