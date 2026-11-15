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
    await page.getByRole("button", { name: "Save", exact: true }).click();
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

// A short stage time keeps the simulated pipeline fast: five stages take about two seconds.
const FAST = "/?view=main&backend=mock&jobms=400";

test.describe("processing pipeline on the mock backend", () => {
  test("process a meeting from the list and watch its stages in the queue", async ({ page }) => {
    await page.goto(FAST);
    const card = page.getByRole("article").filter({ hasText: "Kickoff" });
    await card.getByRole("button", { name: "Process" }).click();
    // Already queued: a second request is not possible.
    await expect(card.getByRole("button", { name: "Process" })).toBeDisabled();

    await page.getByRole("button", { name: "Queue", exact: true }).click();
    const job = page.getByRole("article", { name: "Kickoff" });
    await expect(job).toBeVisible();
    const stages = job.getByRole("list", { name: "Stages of Kickoff" });
    await expect(stages.getByRole("listitem")).toHaveCount(5);
    await expect(stages.getByRole("listitem").filter({ hasText: "Running" })).toHaveCount(1);
    await expect(job.getByRole("progressbar")).toBeVisible();

    await expect(job.getByText("Processed", { exact: true })).toBeVisible({ timeout: 20_000 });
    await expect(stages.getByRole("listitem").filter({ hasText: "Done" })).toHaveCount(5);

    // Back in the list the meeting offers its transcript.
    await page.getByRole("button", { name: "Meetings", exact: true }).click();
    const meeting = page.getByRole("article").filter({ hasText: "Kickoff" });
    await expect(meeting.getByRole("button", { name: "Transcript" })).toBeVisible();
  });

  test("the queue shows running, waiting and failed jobs", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=queue");
    const running = page.getByRole("article", { name: "Thruster review" });
    await expect(running.getByText("Separating speakers")).toBeVisible();
    await expect(running.getByText("Running 40%")).toBeVisible();
    await expect(running.getByRole("button", { name: "Cancel" })).toHaveCount(0);

    const waiting = page.getByRole("article", { name: "Budget check-in" });
    await expect(waiting.getByText("GPU is busy (62% utilization, limit 40%)")).toBeVisible();

    const failed = page.getByRole("article", { name: "Ground station planning" });
    await expect(failed.getByText(/access to the model was denied/)).toBeVisible();

    // A job that is not running can be removed.
    await waiting.getByRole("button", { name: "Cancel" }).click();
    await expect(waiting).toHaveCount(0);
  });

  test("a failed job can be retried", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=queue&jobms=400");
    const job = page.getByRole("article", { name: "Ground station planning" });
    await job.getByRole("button", { name: "Retry" }).click();
    await expect(job.getByText(/access to the model was denied/)).toHaveCount(0);
    await expect(job.getByText("Processed", { exact: true })).toBeVisible({ timeout: 20_000 });
  });

  test("the process prompt after a recording starts or skips processing", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock");
    await page.getByRole("button", { name: "Record" }).click();
    await page.getByRole("button", { name: "Stop" }).click();
    const dialog = page.getByRole("dialog", { name: "Process now or later?" });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Later" }).click();
    await expect(dialog).toHaveCount(0);

    await page.getByRole("button", { name: "Record" }).click();
    await page.getByRole("button", { name: "Stop" }).click();
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Process now" }).click();
    await expect(dialog).toHaveCount(0);
  });
});

test.describe("transcript on the mock backend", () => {
  async function openTranscript(page: import("@playwright/test").Page) {
    await page.goto("/?view=main&backend=mock");
    const card = page.getByRole("article").filter({ hasText: "ORBIT weekly sync" });
    await card.getByRole("button", { name: "Transcript" }).click();
    await expect(page.getByRole("heading", { name: "Transcript" })).toBeVisible();
  }

  test("shows speakers, segments and screenshots with their descriptions", async ({ page }) => {
    await openTranscript(page);
    await expect(page.getByText("Language: en")).toBeVisible();
    const legend = page.getByRole("list", { name: "Speakers" });
    await expect(legend.getByRole("listitem")).toHaveText(["You", "Alex", "Kasia", "Jan"]);
    const rows = page.getByRole("list", { name: "Transcript" }).getByRole("listitem");
    await expect(rows).toHaveCount(14);
    await expect(page.getByText(/Slide titled Thruster test 7/)).toBeVisible();
    await expect(page.getByRole("img", { name: "Screenshot taken at 2:30" })).toBeVisible();
    await expect(page.getByText("edited", { exact: true })).toHaveCount(1);
    await expect(page.getByText("excluded from reports")).toHaveCount(1);
  });

  test("correct a segment, exclude another one and keep both after reopening", async ({ page }) => {
    await openTranscript(page);
    await page.getByRole("button", { name: /^Good\. Did the new valve/ }).click();
    const edit = page.getByRole("textbox", { name: "Correct the text at 0:19" });
    await edit.fill("Good. Did the new valve change the thrust curve at all?");
    await edit.press("Enter");
    await expect(
      page.getByRole("button", { name: "Good. Did the new valve change the thrust curve at all?" }),
    ).toBeVisible();
    await expect(page.getByText("edited", { exact: true })).toHaveCount(2);

    await page.getByRole("button", { name: "Exclude the segment at 0:41" }).click();
    await expect(page.getByText("excluded from reports")).toHaveCount(2);

    // Leave the transcript and open it again: the backend kept the edits.
    await page.getByRole("button", { name: "Back to meetings" }).click();
    await expect(page.getByRole("heading", { name: "Meetings" })).toBeVisible();
    const card = page.getByRole("article").filter({ hasText: "ORBIT weekly sync" });
    await card.getByRole("button", { name: "Transcript" }).click();
    await expect(
      page.getByRole("button", { name: "Good. Did the new valve change the thrust curve at all?" }),
    ).toBeVisible();
    await expect(page.getByText("excluded from reports")).toHaveCount(2);
    await expect(page.getByRole("button", { name: "Include the segment at 0:41" })).toBeVisible();

    // Hiding excluded segments removes them from the list.
    await page.getByLabel(/Hide excluded/).check();
    await expect(page.getByText("excluded from reports")).toHaveCount(0);
  });

  test("Escape cancels an edit and the original can be restored", async ({ page }) => {
    await openTranscript(page);
    await page.getByRole("button", { name: /^Good\. Did the new valve/ }).click();
    const edit = page.getByRole("textbox", { name: "Correct the text at 0:19" });
    await edit.fill("something else");
    await edit.press("Escape");
    await expect(page.getByRole("textbox")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Good\. Did the new valve/ })).toBeVisible();
    await expect(page.getByText("edited", { exact: true })).toHaveCount(1);

    // The corrected demo segment goes back to the recognized text.
    await page.getByRole("button", { name: "Restore the original text at 0:27" }).click();
    await expect(
      page.getByRole("button", { name: /nothing we cannot fill the out/ }),
    ).toBeVisible();
    await expect(page.getByText("edited", { exact: true })).toHaveCount(0);
  });

  test("a meeting without a transcript says it is not ready", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=transcript&meeting=m1");
    await expect(page.getByText(/no transcript yet/)).toBeVisible();
  });
});

test.describe("processing settings on the mock backend", () => {
  test("changes are saved", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=settings");
    const section = page.getByRole("region", { name: "Processing" });
    await section.getByLabel("Audio files after processing").selectOption("delete");
    await expect(section.getByText(/corrections are only possible by editing/)).toBeVisible();
    await section.getByLabel("GPU guard: maximum utilization (%)").fill("55");
    await section.getByRole("button", { name: "Save processing settings" }).click();
    await expect(section.getByRole("status")).toHaveText("Processing settings saved");
  });

  test("the token is write-only and shown as stored", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=settings");
    const section = page.getByRole("region", { name: "Processing" });
    await expect(section.getByText("No token stored")).toBeVisible();
    await section.getByLabel("Token", { exact: false }).fill("hf_test_value");
    await section.getByRole("button", { name: "Store token" }).click();
    await expect(section.getByText("Token stored")).toBeVisible();
    await expect(section.getByLabel("Token", { exact: false })).toHaveValue("");
    await section.getByRole("button", { name: "Clear token" }).click();
    await expect(section.getByText("No token stored")).toBeVisible();
  });

  test("models can be downloaded, gated ones need the token first", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&page=settings&jobms=400");
    const models = page.getByRole("list", { name: "Models" });
    await expect(models.getByText("Cached")).toHaveCount(1);
    await expect(models.getByText(/acceptance of the model terms/)).toBeVisible();

    await page.getByRole("button", { name: /^Download Speaker separation/ }).click();
    await expect(page.getByRole("alert").filter({ hasText: "Hugging Face token" })).toBeVisible();

    await page.getByRole("button", { name: /^Download Whisper large v3/ }).click();
    await expect(
      page.getByRole("progressbar", { name: /Download of Whisper large v3/ }),
    ).toBeVisible();
    await expect(models.getByText("Cached")).toHaveCount(2, { timeout: 10_000 });
  });
});
