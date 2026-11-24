import { type Page, expect, test } from "@playwright/test";

const MAIN = "/?view=main&backend=mock&rate=0";

async function openProject(page: Page, projectName: string) {
  await page.goto(MAIN);
  await page
    .getByRole("article", { name: `Project ${projectName}` })
    .getByRole("button", { name: "Open project" })
    .click();
  await expect(page.getByRole("heading", { name: projectName, level: 2 })).toBeVisible();
}

test.describe("project briefing on the mock backend", () => {
  test("shows top urgent tasks with overflow, open topics, and fresh state", async ({ page }) => {
    await openProject(page, "Nomad rover");

    // The clone is updated in the background; the date moves from the last pull to today.
    await expect(page.locator(".brief-status")).toHaveText(/^State as of 2026-11-13\./);
    await expect(
      page.getByRole("region", { name: "Action items and tasks" }).locator(".chip"),
    ).toHaveText("7 open tasks, 2 overdue · 2 open topics");

    // Top 5 urgent tasks are shown first (overdue, then earliest due date).
    const top = page.getByRole("list", { name: "Top action items" }).getByRole("listitem");
    await expect(top).toHaveCount(5);
    await expect(top.nth(0)).toContainText("Order the battery pack");
    await expect(top.nth(0)).toContainText("Jan Demo · Overdue by 3 days (2026-11-10) · Issue #12");
    await expect(top.nth(1)).toContainText("Prepare the test plan for the field trip");
    await expect(top.nth(1)).toContainText(
      "Kasia Demo · Overdue by 1 day (2026-11-12) · Meeting 2026-11-06",
    );
    await expect(top.nth(2)).toContainText("Design the monitor mount");
    await expect(top.nth(2)).toContainText("Alex Demo · Due 2026-11-18 · Meeting 2026-11-06");
    await expect(top.nth(3)).toContainText("Review the CAN bus wiring diagram");
    await expect(top.nth(3)).toContainText("Alex Demo · Due 2026-11-20 · Issue #14");
    await expect(top.nth(4)).toContainText("Book the test field for the first drive");
    await expect(top.nth(4)).toContainText("@octo-guest · Due 2026-11-27 · Issue #15");

    // Remaining tasks without due dates are inside the collapsible overflow.
    await page.getByText("Show 2 more tasks").click();
    const extra = page.getByRole("list", { name: "More action items" });
    await expect(extra).toContainText("Ask the supplier about the delivery date");
    await expect(extra).toContainText("Unassigned · No due date");
    await expect(extra).toContainText("Check the GNSS antenna placement");
    await expect(extra).toContainText("Piotr Demo");

    // Open topics only; the closed one is not listed.
    const topics = page.getByRole("list", { name: "Open topics" });
    await expect(topics.getByRole("listitem")).toHaveCount(2);
    await expect(topics).toContainText("Decide the rover wheel size");
    await expect(topics).toContainText("Agree on the date of the first field test");

    // What is done is kept apart.
    await page.getByText("Done (1)").click();
    await expect(page.getByRole("list", { name: "Done tasks" })).toContainText("Order the battery");

    await page.getByRole("button", { name: "Back to projects" }).click();
    await expect(page.getByRole("heading", { name: "Projects", level: 2 })).toBeVisible();
  });

  test("opens directly by its address", async ({ page }) => {
    await page.goto(`${MAIN}&page=project&cycle=NOMAD`);
    await expect(page.getByRole("heading", { name: "Nomad rover", level: 2 })).toBeVisible();
    await expect(page.getByRole("list", { name: "Top action items" })).toContainText(
      "Overdue by 1 day (2026-11-12)",
    );

    // Legacy ?page=briefing&cycle=... address still resolves to the project workspace.
    await page.goto(`${MAIN}&page=briefing&cycle=NOMAD`);
    await expect(page.getByRole("heading", { name: "Nomad rover", level: 2 })).toBeVisible();
  });

  test("a clone that cannot be updated shows the state as of its last pull", async ({ page }) => {
    await page.goto(MAIN);
    await page.getByRole("button", { name: "New Project" }).click();
    const dialog = page.getByRole("dialog", { name: "Create New Project" });
    await dialog.getByLabel("Project name").fill("Offline Cycles");
    await expect(dialog.getByLabel("Project ID")).toHaveValue("OFFLINE-CYCLES");
    await dialog.getByRole("radio", { name: /Connect existing repository/ }).click();
    await dialog
      .getByLabel("Remote repository URL")
      .fill("https://offline.example/offline-cycles.git");
    await dialog.getByRole("button", { name: "Next" }).click();
    await dialog.getByRole("button", { name: "Create project" }).click();

    await expect(page.getByRole("heading", { name: "Offline Cycles", level: 2 })).toBeVisible();
    const status = page.locator(".brief-status");
    await expect(status).toContainText(
      "State as of 2026-11-13. Could not update: could not resolve the host",
    );
    await expect(status).toHaveClass(/brief-stale/);
    await expect(page.getByText("No open discussion topics right now.")).toBeVisible();
    await expect(page.getByText("No open action items for this project.")).toBeVisible();
  });

  test("a local repository has nothing to update", async ({ page }) => {
    await openProject(page, "Sandbox");
    await expect(page.locator(".brief-status")).toHaveText("Local repository, nothing to update.");
  });

  test("projects cards show live briefing summaries and manual refresh pulls updates", async ({
    page,
  }) => {
    await page.goto(MAIN);
    const nomad = page.getByRole("article", { name: "Project Nomad rover" });
    await expect(nomad).toContainText("7 open tasks, 2 overdue · 2 open topics");
    await nomad.getByRole("button", { name: "Open project" }).click();
    await expect(page.locator(".brief-status")).toHaveText(/^State as of 2026-11-13\./);
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.locator(".brief-status")).toHaveText(/^State as of 2026-11-13\./);
  });
});
