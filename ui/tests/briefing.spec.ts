import { type Page, expect, test } from "@playwright/test";

const MAIN = "/?view=main&backend=mock&rate=0";

async function openBriefingFromCycles(page: Page, cycleId: string) {
  await page.goto(MAIN);
  await page.getByRole("button", { name: "Cycles", exact: true }).click();
  await page.getByRole("button", { name: `Briefing ${cycleId}` }).click();
  await expect(page.getByRole("heading", { name: "Briefing", level: 2 })).toBeVisible();
}

test.describe("briefing on the mock backend", () => {
  test("shows the open tasks by person and the open topics, then the fresh state", async ({
    page,
  }) => {
    await openBriefingFromCycles(page, "NOMAD");

    // The clone is updated in the background; the date moves from the last pull to today.
    // (In development React runs the effect twice, so the second pull may say "up to date".)
    await expect(page.locator(".brief-status")).toHaveText(/^State as of 2026-11-13\./);
    await expect(page.locator(".brief-summary")).toHaveText(
      "7 open tasks, 2 overdue · 2 open topics",
    );

    const jan = page.getByRole("region", { name: "Tasks of Jan Demo" });
    await expect(jan).toContainText("Jan Demo (@jan-demo)");
    await expect(jan).toContainText("Order the battery pack");
    await expect(jan).toContainText("Overdue by 3 days (2026-11-10)");
    await expect(jan).toContainText("1 overdue");

    // Sorted by due date within a person.
    const alex = page.getByRole("list", { name: "Alex Demo tasks" }).getByRole("listitem");
    await expect(alex).toHaveCount(2);
    await expect(alex.nth(0)).toContainText("Design the monitor mount");
    await expect(alex.nth(0)).toContainText("Due 2026-11-18 · Meeting 2026-11-06");
    await expect(alex.nth(1)).toContainText("Due 2026-11-20 · Issue #14");

    // A login that is nobody's and the tasks without an owner have groups of their own.
    await expect(page.getByRole("region", { name: "Tasks of @octo-guest" })).toBeVisible();
    const nobody = page.getByRole("region", { name: "Tasks of Unassigned" });
    await expect(nobody).toContainText("Ask the supplier about the delivery date");
    await expect(nobody).toContainText("No due date");

    // Open topics only; the closed one is not listed.
    const topics = page.getByRole("list", { name: "Open topics" });
    await expect(topics.getByRole("listitem")).toHaveCount(2);
    await expect(topics).toContainText("Decide the rover wheel size");
    await expect(topics).toContainText("Agree on the date of the first field test");

    // What is done is kept apart.
    await page.getByText("Done (1)").click();
    await expect(page.getByRole("list", { name: "Done tasks" })).toContainText("Order the battery");

    await page.getByRole("button", { name: "Back to cycles" }).click();
    await expect(page.getByRole("heading", { name: "Cycles", level: 2 })).toBeVisible();
  });

  test("opens directly by its address", async ({ page }) => {
    await page.goto(`${MAIN}&page=briefing&cycle=NOMAD`);
    await expect(page.getByRole("heading", { name: "Briefing", level: 2 })).toBeVisible();
    await expect(page.getByRole("region", { name: "Tasks of Kasia Demo" })).toContainText(
      "Overdue by 1 day (2026-11-12)",
    );
  });

  test("a clone that cannot be updated shows the state as of its last pull", async ({ page }) => {
    await page.goto(MAIN);
    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByLabel("Repository address").fill("https://offline.example/offline-cycles.git");
    await page.getByRole("button", { name: "Add repository" }).click();
    await page.getByRole("button", { name: "Briefing OFFLINE-CYCLES" }).click();

    const status = page.locator(".brief-status");
    await expect(status).toContainText(
      "State as of 2026-11-13. Could not update: could not resolve the host",
    );
    await expect(status).toHaveClass(/brief-stale/);
    await expect(page.getByText("Nothing is open for this cycle")).toBeVisible();
  });

  test("a local repository has nothing to update", async ({ page }) => {
    await openBriefingFromCycles(page, "SANDBOX");
    await expect(page.locator(".brief-status")).toHaveText("Local repository, nothing to update.");
  });

  test("the report mode gate links to the briefing of the cycle", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock&rate=0");
    await page.getByRole("button", { name: "Record" }).click();
    const gate = page.getByRole("dialog", { name: "Choose the report for this recording" });
    await gate.getByRole("button", { name: "Open the briefing of NOMAD" }).click();
    // In the mock the page itself becomes the main window.
    await expect(page.getByRole("heading", { name: "Briefing", level: 2 })).toBeVisible();
    await expect(page).toHaveURL(/page=briefing&cycle=NOMAD/);
  });
});
