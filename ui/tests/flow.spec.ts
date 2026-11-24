import { expect, test } from "@playwright/test";

/**
 * The convenience flow of Stage 6 on the mock backend: read the briefing, get asked about a
 * detected call, record it with a browser tab and answer the prompt that follows the stop.
 * The main window and the widget are separate pages with their own mock, as they are separate
 * windows in the application; the detection and the tabs are the mock's simulated ones.
 */
test.describe("briefing to detected call to recording with a tab to processing", () => {
  test("the whole flow", async ({ page }) => {
    // 1. Briefing before the meeting, opened from the cycle list.
    await page.goto("/?view=main&backend=mock&rate=0");
    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Briefing NOMAD" }).click();
    await expect(page.getByRole("heading", { name: "Briefing", level: 2 })).toBeVisible();
    await expect(page.locator(".brief-summary")).toHaveText(
      "7 open tasks, 2 overdue · 2 open topics",
    );
    await expect(page.getByRole("region", { name: "Tasks of Jan Demo" })).toContainText(
      "Order the battery pack",
    );

    // 2. The call starts: one prompt, asked once.
    await page.goto("/?view=widget&backend=mock&tabs=1&detectms=300");
    const prompt = page.getByRole("alertdialog", { name: "Record this call?" });
    await expect(prompt).toBeVisible();
    await expect(prompt).toContainText("A Discord call seems to have started.");
    await expect(page.getByRole("alertdialog")).toHaveCount(1);

    // 3. Record: the microphone and the application, then a captured tab joins.
    await prompt.getByRole("button", { name: "Record" }).click();
    await expect(prompt).toHaveCount(0);
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    const sources = page.getByRole("list", { name: "Recorded sources" });
    await expect(sources.getByRole("listitem")).toHaveCount(2);
    await page
      .getByLabel("Add source")
      .selectOption({ label: "Tab: Budget sheet (sheets.example.org)" });
    await expect(sources.getByRole("listitem")).toHaveCount(3);
    await expect(sources).toContainText("Tab: Budget sheet (sheets.example.org)");
    await expect(prompt).toHaveCount(0);

    // 4. Stop: the prompt for processing, and processing starts.
    await page.getByRole("button", { name: "Stop" }).click();
    await expect(page.getByText("Standby")).toBeVisible();
    const process = page.getByRole("dialog", { name: "Process now or later?" });
    await expect(process).toBeVisible();
    await process.getByRole("button", { name: "Process now" }).click();
    await expect(process).toHaveCount(0);
  });
});
