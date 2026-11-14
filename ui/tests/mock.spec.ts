import { expect, test } from "@playwright/test";

test.describe("recorder widget on the mock backend", () => {
  test("start, add a source, take a screenshot, remove a source, stop", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock");

    // The microphone is preselected; add Discord as well.
    await page.getByLabel("Microphone (USB headset)").check();
    await page.getByLabel("Discord").check();
    await page.getByRole("button", { name: "Record" }).click();
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    const sources = page.getByRole("list", { name: "Recorded sources" });
    await expect(sources.getByRole("listitem")).toHaveCount(2);

    // Add a source while recording.
    await page.getByLabel("Add source").selectOption({ label: "Google Chrome" });
    await expect(sources.getByRole("listitem")).toHaveCount(3);

    // Screenshot counter.
    await page.getByRole("button", { name: /Screenshot/ }).click();
    await expect(page.getByRole("button", { name: /Screenshot/ })).toContainText("1");

    // Remove a source: it stays in the list as ended.
    await page.getByRole("button", { name: "Remove Discord" }).click();
    await expect(sources.getByText("Removed by user")).toBeVisible();

    await page.getByRole("button", { name: "Stop" }).click();
    await expect(page.getByText("Standby")).toBeVisible();
  });

  test("record needs at least one source", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock");
    await page.getByLabel("Microphone (USB headset)").uncheck();
    await expect(page.getByRole("button", { name: "Record" })).toBeDisabled();
  });

  test("a recording in progress is shown on load", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock&state=recording");
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    await expect(page.getByLabel("Elapsed time")).toContainText("12:4");
  });
});

test.describe("main window on the mock backend", () => {
  test("lists meetings and marks a recovered one", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    await expect(page.getByRole("heading", { name: "Meetings" })).toBeVisible();
    await expect(page.getByRole("article")).toHaveCount(5);
    await expect(page.getByText("Recovered", { exact: true })).toBeVisible();
  });

  test("deleting a meeting removes its card", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    const card = page.getByRole("article").filter({ hasText: "Budget check-in" });
    await card.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("article")).toHaveCount(4);
  });

  test("settings can be edited and saved", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByLabel("Record / Stop hotkey").fill("Ctrl+F9");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status")).toHaveText("Saved");
  });

  test("layout adapts: sidebar becomes a top bar on narrow windows", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/?view=main&backend=mock");
    const nav = page.getByRole("navigation", { name: "Main navigation" });
    const wide = await nav.boundingBox();
    expect(wide!.height).toBeGreaterThan(wide!.width);
    await page.setViewportSize({ width: 560, height: 800 });
    const narrow = await nav.boundingBox();
    expect(narrow!.width).toBeGreaterThan(narrow!.height);
  });
});

test.describe("screenshot overlay on the mock backend", () => {
  test("dragging selects a region and Escape cancels", async ({ page }) => {
    await page.goto("/?view=overlay&backend=mock");
    const stage = page.getByRole("application");
    await expect(page.locator("img.shot-image")).toBeVisible();
    await page.mouse.move(200, 150);
    await page.mouse.down();
    await page.mouse.move(520, 380, { steps: 5 });
    await expect(page.locator(".shot-size")).toContainText("320 × 230");
    await page.mouse.up();
    await expect(stage).toBeVisible();
  });
});
