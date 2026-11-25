import { type Page, expect, test } from "@playwright/test";

const MAIN = "/?view=main&backend=mock&rate=0";
const MEDIA = "C:\\Recordings\\ground-station-sync.mp4";

/** The mock file dialogs answer with this instead of opening a window (see `MockPickResults`). */
async function presetPick(page: Page, pick: { file?: string | null; folder?: string | null }) {
  await page.addInitScript((answers) => {
    window.__bbbPick = answers;
  }, pick);
}

async function openImportModal(page: Page) {
  await page.goto(`${MAIN}&page=project&cycle=NOMAD`);
  await page.getByRole("button", { name: "+ Add meeting" }).click();
  return page.getByRole("dialog", { name: "Add Meeting from Recording" });
}

async function openWizardOnGithubStep(page: Page, query: string) {
  await page.goto(`${MAIN}${query}&page=projects`);
  await page.getByRole("button", { name: "New Project" }).click();
  const wizard = page.getByRole("dialog", { name: "Create New Project" });
  await wizard.getByLabel("Project name").fill("Lunar Scout");
  await wizard.getByRole("button", { name: "Next" }).click();
  await wizard.getByLabel("Create a private GitHub repository").check();
  return wizard;
}

test.describe("import a meeting with the file picker", () => {
  test("the chosen path is imported and the meeting is processed", async ({ page }) => {
    await presetPick(page, { file: MEDIA });
    const modal = await openImportModal(page);
    await expect(modal.getByRole("button", { name: "Import meeting" })).toBeDisabled();

    await modal.getByRole("button", { name: "Browse for the audio or video file" }).click();
    await expect(modal.getByLabel("Audio or video file path")).toHaveValue(MEDIA);
    // A video file offers the slide handling choice.
    await expect(modal.getByRole("radiogroup", { name: "Video handling mode" })).toBeVisible();

    await modal.getByRole("button", { name: "Import meeting" }).click();
    await expect(modal).toHaveCount(0);

    // The recording became a meeting of the project and a job in the queue.
    await page.getByRole("button", { name: "Queue", exact: true }).click();
    const job = page.getByRole("article", { name: "ground-station-sync" });
    await expect(job).toBeVisible();
    await expect(job.getByText("Processed", { exact: true })).toBeVisible({ timeout: 20_000 });
  });

  test("a cancelled dialog changes nothing and nothing is imported", async ({ page }) => {
    await presetPick(page, { file: null });
    const modal = await openImportModal(page);
    await modal.getByRole("button", { name: "Browse for the audio or video file" }).click();
    await expect(modal.getByLabel("Audio or video file path")).toHaveValue("");
    await expect(modal.getByRole("button", { name: "Import meeting" })).toBeDisabled();
    await expect(modal.getByRole("alert")).toHaveCount(0);
  });

  test("the default fake path is used when nothing was preset", async ({ page }) => {
    const modal = await openImportModal(page);
    await modal.getByRole("button", { name: "Browse for the audio or video file" }).click();
    await expect(modal.getByLabel("Audio or video file path")).toHaveValue(
      "C:\\Recordings\\mock-meeting.mp4",
    );
  });
});

test.describe("project wizard: private GitHub repository", () => {
  test("a saved token: confirm step first, then the repository is created", async ({ page }) => {
    const wizard = await openWizardOnGithubStep(page, "");
    await expect(wizard.getByText("A saved GitHub token was found")).toBeVisible();
    await wizard.getByRole("button", { name: "Create project" }).click();

    const confirm = wizard.getByRole("region", { name: "Confirm the new GitHub repository" });
    await expect(confirm).toContainText("lunar-scout (in your account)");
    await expect(confirm).toContainText("Nothing has been sent to GitHub yet");
    await confirm.getByRole("button", { name: "Create repository & project" }).click();

    await expect(wizard.getByRole("status")).toContainText(
      "Created the private GitHub repository demo-user/lunar-scout",
    );
    await wizard.getByRole("button", { name: "Open project" }).click();
    await expect(page.getByRole("heading", { name: "Lunar Scout", level: 2 })).toBeVisible();
  });

  test("no saved token: a local-only project is created with a message", async ({ page }) => {
    const wizard = await openWizardOnGithubStep(page, "&ghtoken=none");
    await expect(wizard.getByText("No GitHub token is saved yet")).toBeVisible();
    await wizard.getByRole("button", { name: "Create project" }).click();

    // Nothing to confirm without a token, so there is no confirm step.
    const status = wizard.getByRole("status");
    await expect(status).toContainText("No GitHub token is saved");
    await expect(status).toContainText("Settings > Connectors");
    await expect(
      wizard.getByRole("region", { name: "Confirm the new GitHub repository" }),
    ).toHaveCount(0);
    await wizard.getByRole("button", { name: "Open project" }).click();
    await expect(page.getByRole("heading", { name: "Lunar Scout", level: 2 })).toBeVisible();
  });

  for (const [fault, text] of [
    ["name-taken", "already has a repository named lunar-scout"],
    ["offline", "GitHub could not be reached"],
  ] as const) {
    test(`GitHub fails (${fault}): a local-only project is created and the reason is shown`, async ({
      page,
    }) => {
      const wizard = await openWizardOnGithubStep(page, `&ghfault=${fault}`);
      await wizard.getByRole("button", { name: "Create project" }).click();
      await wizard.getByRole("button", { name: "Create repository & project" }).click();
      const status = wizard.getByRole("status");
      await expect(status).toContainText(text);
      await expect(status).toContainText("Only a local project folder was created");
      await wizard.getByRole("button", { name: "Open project" }).click();
      await expect(page.getByRole("heading", { name: "Lunar Scout", level: 2 })).toBeVisible();
    });
  }
});

// Not covered here: publishing only the draft's key screenshots. The mock backend sends every
// screenshot of a meeting and has no key-screenshot selection, so a browser test would show
// nothing real; the Rust publishers tests (`cargo test -p publishers`) cover the selection.
