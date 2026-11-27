import { expect, test } from "@playwright/test";

const MAIN = "/?view=main&backend=mock";

test.describe("wave 3 UX overhaul flows on the mock backend", () => {
  test("projects wizard, video import with slides, meeting detail md export, and project prefs", async ({
    page,
  }) => {
    await page.goto(`${MAIN}&rate=0`);

    // 1. Create a new project via the 2-step ProjectWizard with Local Folder Auto-Save.
    await page.getByRole("button", { name: "New Project" }).click();
    const wizard = page.getByRole("dialog", { name: "Create New Project" });
    await wizard.getByLabel("Project name").fill("Orbit Launch");
    await expect(wizard.getByLabel("Project ID")).toHaveValue("ORBIT-LAUNCH");
    await wizard.getByRole("button", { name: "Next" }).click();

    await wizard.getByLabel("Report language").selectOption("en");
    await wizard
      .getByLabel("Automatically save a Markdown (.md) note to a local folder after processing")
      .check();
    await expect(wizard.getByLabel("Local notes folder", { exact: true })).toHaveValue(
      "Documents\\BlahBlahBot\\notes",
    );
    await wizard.getByRole("button", { name: "Create project" }).click();

    // 2. Lands on ProjectDetailView; import a video recording with Audio + Visual Slides.
    await expect(page.getByRole("heading", { name: "Orbit Launch", level: 2 })).toBeVisible();
    await page.getByRole("button", { name: "+ Add meeting" }).click();
    const modal = page.getByRole("dialog", { name: "Add Meeting from Recording" });
    await modal.getByLabel("Audio or video file path").fill("C:\\Recordings\\launch-readiness.mp4");

    const modeGroup = modal.getByRole("radiogroup", { name: "Video handling mode" });
    await expect(modeGroup).toBeVisible();
    await modeGroup.getByRole("radio", { name: /Audio only/ }).click();
    await expect(modeGroup.getByRole("radio", { name: /Audio only/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await modeGroup.getByRole("radio", { name: /Audio \+ Visual Slides/ }).click();
    await modal.getByRole("button", { name: "Import meeting" }).click();
    await expect(modal).toHaveCount(0);

    // 3. Imported meeting appears inside ProjectDetail with slide count, opens MeetingDetail, and exports .md.
    const meetingsList = page.getByRole("list", { name: "Meetings list" });
    const row = meetingsList.getByRole("listitem").filter({ hasText: "launch-readiness" });
    await expect(row).toContainText("4 slides/shots");
    await row.getByRole("button", { name: "Open" }).click();

    await expect(page.getByRole("tab", { name: "Draft & Report" })).toBeVisible();
    await page.getByRole("button", { name: "Download note (.md)" }).click();
    // The mock Save as dialog answers with a fake path built from the offered file name.
    await expect(page.getByRole("status")).toContainText(
      /Saved note to Documents\\BlahBlahBot\\notes\\.+\.md/,
    );

    // 4. Back to ProjectDetail and save per-project local AI override in ProjectSettingsModal.
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await page.getByRole("button", { name: "Project settings" }).click();
    const settingsModal = page.getByRole("dialog", { name: /Project Settings/ });
    await settingsModal.getByLabel("Model backend override").selectOption("managed");
    await settingsModal
      .getByLabel("Model override value")
      .fill("D:\\Models\\gemma-4-12b-q4_k_m.gguf");
    // The project keeps its audio although the global setting deletes it.
    await settingsModal.getByLabel("Audio retention override").selectOption("keep");
    await settingsModal.getByRole("button", { name: "Save local preferences" }).click();
    await expect(settingsModal.getByRole("status")).toHaveText("Local preferences saved.");
  });

  test("main UI Record tab with live WaveformEq, screenshot, and post-stop flow", async ({
    page,
  }) => {
    await page.goto(`${MAIN}&rate=200`);
    await page.getByRole("button", { name: "Record", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Record", level: 2 })).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Recording tips" })).toContainText(
      "Ctrl+Alt+R",
    );

    const eq = page.getByRole("img", { name: /Audio waveform/ });
    await expect(eq).toHaveAttribute("data-active", "false");
    await expect(eq.locator(".waveform-eq-bar")).toHaveCount(12);

    await page.getByLabel("Discord").check();
    await page.getByRole("button", { name: "Record", exact: true }).last().click();
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    await expect(eq).toHaveAttribute("data-active", "true");
    await expect.poll(async () => Number(await eq.getAttribute("data-peak"))).toBeGreaterThan(0);

    await page.getByRole("button", { name: /Screenshot/ }).click();
    await expect(page.getByRole("button", { name: /Screenshot/ })).toContainText("1");

    await page.getByRole("button", { name: "Stop" }).click();
    const prompt = page.getByRole("dialog", { name: "Process now or later?" });
    await expect(prompt).toBeVisible();
    await prompt.getByRole("button", { name: "Process now" }).click();
    await expect(prompt).toHaveCount(0);
  });

  test("queue temporary GPU override, schedule start, and force process", async ({ page }) => {
    await page.goto(`${MAIN}&page=queue&jobms=400`);

    const side = page.getByRole("complementary", { name: "Queue GPU overrides (temporary)" });
    await side.getByLabel("Max GPU utilization (%)", { exact: true }).fill("85");
    await side.getByLabel("Wait for idle (seconds)", { exact: true }).fill("10");
    await side.getByRole("button", { name: "Apply override" }).click();
    await expect(side.getByRole("status")).toHaveText("Temporary GPU override applied.");
    await expect(side).toContainText("Override active: 85% max util · 10s idle");

    await side.getByRole("button", { name: "Reset" }).click();
    await expect(side.getByRole("status")).toHaveText("Reverted to global GPU settings.");
    await expect(side).toContainText("Using global settings: 40% max util · 30s idle");

    const waitingJob = page.getByRole("article", { name: "Budget check-in" });
    await waitingJob
      .getByLabel("Schedule date and time for Budget check-in")
      .fill("2026-11-15T22:30");
    await waitingJob.getByRole("button", { name: "Schedule", exact: true }).click();
    await expect(waitingJob.getByText(/^Scheduled for /)).toBeVisible();
    await waitingJob.getByRole("button", { name: "Clear schedule" }).click();
    await expect(waitingJob.getByRole("button", { name: "Clear schedule" })).toHaveCount(0);

    await waitingJob.getByRole("button", { name: "Force process", exact: true }).click();
    await expect(waitingJob.getByText("Processed", { exact: true })).toBeVisible({
      timeout: 20_000,
    });
  });

  test("tabbed settings: auto-detect, HelpPopover, Local Auto-Save, UnsavedDialog, and voice enrollment", async ({
    page,
  }) => {
    await page.goto(`${MAIN}&page=settings`);

    // 1. AI Models tab: HelpPopover & Auto-detect models.
    await page.getByRole("button", { name: "Help: Context size" }).click();
    await expect(page.getByRole("region", { name: "Context size" })).toContainText("16384");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("region", { name: "Context size" })).toHaveCount(0);

    await page.getByRole("button", { name: "Auto-detect models on disk" }).click();
    await expect(
      page.getByRole("status").filter({
        hasText: "Found 2 language models and 1 vision model on disk.",
      }),
    ).toBeVisible();

    // 2. Discard detected model edits via UnsavedBar, then switch to Preferences for Local Auto-Save.
    await page.getByRole("button", { name: "Discard", exact: true }).click();
    await page.getByRole("tab", { name: "Preferences" }).click();
    const localCheck = page.getByLabel(
      "Automatically save meeting notes & transcripts to a local folder",
    );
    await expect(localCheck).not.toBeChecked();
    await expect(page.getByLabel("Local notes folder", { exact: true })).toHaveValue(
      "Documents\\BlahBlahBot\\notes",
    );
    await localCheck.check();
    await page.getByRole("button", { name: "Save preferences" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Preferences saved." })).toBeVisible();

    // 3. Processing tab: edit triggers UnsavedDialog when switching to Voice Profiles.
    await page.getByRole("tab", { name: "Processing" }).click();
    await page.getByLabel("Max GPU utilization (%)").fill("65");
    await page.getByRole("tab", { name: "Voice Profiles" }).click();
    const unsaved = page.getByRole("dialog", { name: "You have unsaved changes" });
    await expect(unsaved).toBeVisible();
    await unsaved.getByRole("button", { name: "Save", exact: true }).click();
    await expect(unsaved).toHaveCount(0);

    // 4. Voice Profiles tab: manual 15-30s enrollment + project badge + sample playback.
    await expect(page.getByRole("heading", { name: "Add voice profile manually" })).toBeVisible();
    const enrollForm = page.getByRole("form", { name: "Add voice profile manually" });
    await enrollForm.getByLabel("Person name").fill("Elena Vance");
    await enrollForm.getByLabel("Project").selectOption("NOMAD");
    await enrollForm
      .getByLabel("15–30 s audio/video sample path")
      .fill("C:\\Audio\\elena-sample-20s.wav");
    await enrollForm.getByRole("button", { name: "Enroll voice profile" }).click();
    await expect(enrollForm.getByRole("status")).toHaveText(
      "Enrolled voice profile for Elena Vance.",
    );

    const profiles = page.getByRole("list", { name: "Voice profiles" });
    const elena = profiles.getByRole("listitem").filter({
      has: page.getByRole("button", { name: "Rename Elena Vance" }),
    });
    await expect(elena).toContainText("NOMAD");
    await expect(elena).toContainText("1 sample");

    await elena.getByRole("button", { name: "Play sample for Elena Vance" }).click();
    await expect(elena.getByRole("button", { name: "Stop sample for Elena Vance" })).toBeVisible();
    await elena.getByRole("button", { name: "Stop sample for Elena Vance" }).click();
    await expect(elena.getByRole("button", { name: "Play sample for Elena Vance" })).toBeVisible();
  });
});
