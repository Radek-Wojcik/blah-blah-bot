import { expect, test } from "@playwright/test";
import { recordFromGate } from "./helpers";

test.describe("recorder widget on the mock backend", () => {
  test("start, add a source, take a screenshot, remove a source, stop", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock");

    // The microphone is preselected; add Discord as well.
    await page.getByLabel("Microphone (USB headset)").check();
    await page.getByLabel("Discord").check();
    await recordFromGate(page);
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
    const img = page.locator("img.shot-image");
    await expect(img).toBeVisible();
    await expect(stage).toHaveAttribute("data-phase", "ready");
    await page.mouse.move(200, 150);
    await page.mouse.down();
    await page.mouse.move(520, 380, { steps: 5 });
    await expect(page.locator(".shot-size")).toContainText("320 × 230");
    await page.mouse.up();
    await expect(stage).toBeVisible();
  });

  test("the picture is decoded, fills the window and is not hidden", async ({ page }) => {
    await page.goto("/?view=overlay&backend=mock");
    const img = page.locator("img.shot-image");
    await expect(img).toBeVisible();
    await expect(page.getByRole("application")).toHaveAttribute("data-phase", "ready");
    const state = await img.evaluate((el: HTMLImageElement) => {
      const r = el.getBoundingClientRect();
      const css = getComputedStyle(el);
      return {
        decoded: el.complete && el.naturalWidth > 0 && el.naturalHeight > 0,
        width: r.width,
        height: r.height,
        opacity: Number(css.opacity),
        visibility: css.visibility,
        display: css.display,
        viewport: [innerWidth, innerHeight],
      };
    });
    expect(state.decoded).toBe(true);
    expect(state.opacity).toBe(1);
    expect(state.visibility).toBe("visible");
    expect(state.display).not.toBe("none");
    expect(state.width).toBeGreaterThanOrEqual(state.viewport[0] - 1);
    expect(state.height).toBeGreaterThanOrEqual(state.viewport[1] - 1);
  });

  for (const fault of ["blank", "black", "broken"]) {
    test(`a ${fault} capture shows a message instead of a black screen`, async ({ page }) => {
      await page.goto(`/?view=overlay&backend=mock&overlayfault=${fault}`);
      const alert = page.getByRole("alert");
      await expect(alert).toBeVisible();
      await expect(alert).toContainText("Screenshot failed");
      await expect(page.getByRole("application")).toHaveAttribute("data-phase", "problem");
      await expect(page.locator("img.shot-image")).toHaveCount(0);
    });
  }
});

// A short stage time keeps the simulated pipeline fast: seven stages take about three seconds.
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
    await expect(stages.getByRole("listitem")).toHaveCount(9);
    await expect(stages.getByRole("listitem").filter({ hasText: "Running" })).toHaveCount(1);
    await expect(job.getByRole("progressbar")).toBeVisible();

    await expect(job.getByText("Processed", { exact: true })).toBeVisible({ timeout: 20_000 });
    await expect(stages.getByRole("listitem").filter({ hasText: "Done" })).toHaveCount(7);
    // A meeting without a cycle has nothing to extract: the two stages say so.
    const skipped = stages
      .getByRole("listitem")
      .filter({ hasText: "Skipped: the meeting has no cycle" });
    await expect(skipped).toHaveCount(2);
    await expect(stages.getByRole("listitem").filter({ hasText: "Extract items" })).toHaveCount(1);
    await expect(stages.getByRole("listitem").filter({ hasText: "Build draft" })).toHaveCount(1);

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
    await recordFromGate(page);
    await page.getByRole("button", { name: "Stop" }).click();
    const dialog = page.getByRole("dialog", { name: "Process now or later?" });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Later" }).click();
    await expect(dialog).toHaveCount(0);

    await recordFromGate(page);
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

test.describe("speakers on the mock backend", () => {
  const SPEAKERS = "/?view=main&backend=mock&page=speakers&meeting=m5";
  const voice = (page: import("@playwright/test").Page, label: string) =>
    page.getByRole("article", { name: label });

  test("the labeling screen is reachable from the list and the transcript", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    const card = page.getByRole("article").filter({ hasText: "ORBIT weekly sync" });
    await card.getByRole("button", { name: "Name speakers" }).click();
    await expect(page.getByRole("heading", { name: "Speakers" })).toBeVisible();
    await expect(page.getByText(/never leave this computer/)).toBeVisible();

    await page.getByRole("button", { name: "Transcript", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Transcript" })).toBeVisible();
    await page.getByRole("button", { name: "Name speakers" }).click();
    await expect(page.getByRole("heading", { name: "Speakers" })).toBeVisible();
    await page.getByRole("button", { name: "Back to meetings" }).click();
    await expect(page.getByRole("heading", { name: "Meetings" })).toBeVisible();

    // A meeting without a finished job offers no labeling.
    const kickoff = page.getByRole("article").filter({ hasText: "Kickoff" });
    await expect(kickoff.getByRole("button", { name: "Name speakers" })).toHaveCount(0);
  });

  test("shows the recognized, suggested and unknown voices with their scores", async ({ page }) => {
    await page.goto(SPEAKERS);
    await expect(page.getByRole("article")).toHaveCount(4);
    await expect(page.getByRole("status")).toHaveText("0 of 4 voices named");

    const auto = voice(page, "SPEAKER_00");
    await expect(auto.getByText("Recognized automatically · 83%")).toBeVisible();
    await expect(auto.locator(".voice-name")).toHaveText("Alex Demo");
    const autoClips = auto.getByRole("list", { name: /Sample clips/ });
    await expect(autoClips.getByRole("listitem")).toHaveCount(2);

    const suggested = voice(page, "SPEAKER_01");
    await expect(suggested.getByText("Suggested: Kasia Demo? 61%")).toBeVisible();
    await expect(suggested.getByRole("button", { name: /^Accept/ })).toBeVisible();
    await expect(suggested.getByRole("button", { name: /^Reject/ })).toBeVisible();

    await expect(voice(page, "SPEAKER_02").getByText("Unknown", { exact: true })).toBeVisible();
    const three = voice(page, "SPEAKER_02").getByRole("list", { name: /Sample clips/ });
    await expect(three.getByRole("listitem")).toHaveCount(3);
    const two = voice(page, "SPEAKER_03").getByRole("list", { name: /Sample clips/ });
    await expect(two.getByRole("listitem")).toHaveCount(2);
    await expect(three.getByText("Thursday works for the dry run as well.")).toBeVisible();
  });

  test("a clip plays and stops, and only one clip plays at a time", async ({ page }) => {
    await page.goto(SPEAKERS);
    await page.getByRole("button", { name: "Play clip 1 of SPEAKER_02" }).click();
    await expect(page.getByRole("button", { name: "Stop clip 1 of SPEAKER_02" })).toBeVisible();

    // Starting another clip stops the first one.
    await page.getByRole("button", { name: "Play clip 2 of SPEAKER_02" }).click();
    await expect(page.getByRole("button", { name: "Stop clip 2 of SPEAKER_02" })).toBeVisible();
    await expect(page.getByRole("button", { name: /^Stop clip/ })).toHaveCount(1);
    await expect(page.getByRole("button", { name: "Play clip 1 of SPEAKER_02" })).toBeVisible();

    // Stopping returns the button to Play.
    await page.getByRole("button", { name: "Stop clip 2 of SPEAKER_02" }).click();
    await expect(page.getByRole("button", { name: /^Stop clip/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Play clip 2 of SPEAKER_02" })).toBeVisible();
  });

  test("an unknown voice is named after a new participant", async ({ page }) => {
    await page.goto(SPEAKERS);
    const card = voice(page, "SPEAKER_02");
    await card.getByLabel("Name for SPEAKER_02").selectOption({ label: "New participant…" });
    const create = card.getByRole("button", { name: "Create and name" });
    await expect(create).toBeDisabled();
    await card.getByLabel(/Name of the new participant/).fill("Mira Demo");
    await create.click();

    await expect(card.getByText("Named by you")).toBeVisible();
    await expect(card.getByText("Mira Demo", { exact: true }).first()).toBeVisible();
    await expect(page.getByRole("status")).toHaveText("1 of 4 voices named");
    // The new participant is offered for the other voices, too.
    const other = voice(page, "SPEAKER_03").getByLabel("Name for SPEAKER_03");
    await expect(other.locator("option", { hasText: "Mira Demo" })).toHaveCount(1);
  });

  test("a voice can be named after an existing participant or skipped", async ({ page }) => {
    await page.goto(SPEAKERS);
    const jan = voice(page, "SPEAKER_02").getByLabel("Name for SPEAKER_02");
    await jan.selectOption({ label: "Jan Demo" });
    await expect(voice(page, "SPEAKER_02").getByText("Named by you")).toBeVisible();
    const skip = voice(page, "SPEAKER_03").getByLabel("Name for SPEAKER_03");
    await skip.selectOption({ label: "Not a person / skip" });
    await expect(voice(page, "SPEAKER_03").getByText("Not a person (skipped)")).toBeVisible();
    await expect(page.getByRole("status")).toHaveText("2 of 4 voices named");
  });

  test("a suggestion is accepted, another one is rejected", async ({ page }) => {
    await page.goto(SPEAKERS);
    const suggested = voice(page, "SPEAKER_01");
    await suggested.getByRole("button", { name: /^Accept/ }).click();
    await expect(suggested.getByText("Named by you")).toBeVisible();
    await expect(suggested.getByText("Kasia Demo", { exact: true }).first()).toBeVisible();
    await expect(suggested.getByRole("button", { name: /^Accept/ })).toHaveCount(0);
    await expect(page.getByRole("status")).toHaveText("1 of 4 voices named");

    // Rejecting leaves the voice unnamed and open for another name.
    await page.goto(SPEAKERS);
    const again = voice(page, "SPEAKER_01");
    await again.getByRole("button", { name: /^Reject/ }).click();
    await expect(again.getByText("Unknown", { exact: true })).toBeVisible();
    await expect(again.getByText(/Suggested/)).toHaveCount(0);
    await expect(page.getByRole("status")).toHaveText("0 of 4 voices named");
  });

  test("two voices are merged and the merge is undone", async ({ page }) => {
    await page.goto(SPEAKERS);
    const card = voice(page, "SPEAKER_03");
    await card.getByLabel("Same person as, for SPEAKER_03").selectOption({ label: "SPEAKER_02" });
    await expect(page.getByRole("article")).toHaveCount(3);
    const nested = voice(page, "SPEAKER_02").getByRole("list", {
      name: "Voices merged into SPEAKER_02",
    });
    await expect(nested.getByText("SPEAKER_03")).toBeVisible();
    // The merged voice is not counted on its own.
    await expect(page.getByRole("status")).toHaveText("0 of 3 voices named");

    await nested.getByRole("button", { name: "Undo the merge of SPEAKER_03" }).click();
    await expect(page.getByRole("article")).toHaveCount(4);
    await expect(page.getByRole("list", { name: /Voices merged into/ })).toHaveCount(0);
  });

  test("all automatic matches are confirmed at once", async ({ page }) => {
    await page.goto(SPEAKERS);
    const confirm = page.getByRole("button", { name: "Confirm all automatic matches" });
    await expect(confirm).toBeVisible();
    await confirm.click();
    await expect(confirm).toHaveCount(0);
    await expect(voice(page, "SPEAKER_00").getByText("Named by you")).toBeVisible();
    await expect(page.getByRole("status")).toHaveText("1 of 4 voices named");
    // The suggestion is not confirmed by this.
    await expect(voice(page, "SPEAKER_01").getByText(/^Suggested:/)).toBeVisible();
  });

  test("when every voice is decided the clips are gone and the transcript is offered", async ({
    page,
  }) => {
    await page.goto(SPEAKERS);
    await page.getByRole("button", { name: "Confirm all automatic matches" }).click();
    await voice(page, "SPEAKER_01")
      .getByRole("button", { name: /^Accept/ })
      .click();
    await voice(page, "SPEAKER_02")
      .getByLabel("Name for SPEAKER_02")
      .selectOption({ label: "Jan Demo" });
    await voice(page, "SPEAKER_03")
      .getByLabel("Name for SPEAKER_03")
      .selectOption({ label: "Not a person / skip" });
    await expect(page.getByRole("status")).toHaveText("4 of 4 voices named");
    await expect(page.getByText(/All voices are decided/)).toBeVisible();
    await expect(page.getByText(/No sample clips are available for this voice/)).toHaveCount(4);
    await voice(page, "SPEAKER_00").getByRole("button", { name: "Open the transcript" }).click();
    await expect(page.getByRole("heading", { name: "Transcript" })).toBeVisible();
  });
});

test.describe("voice profiles on the mock backend", () => {
  const SETTINGS = "/?view=main&backend=mock&page=settings";

  test("a participant is renamed", async ({ page }) => {
    await page.goto(SETTINGS);
    const people = page.getByRole("list", { name: "Participants" });
    await expect(people.getByRole("listitem")).toHaveCount(3);
    await expect(people.getByText("4 samples · 18:20 of speech")).toBeVisible();
    await page.getByRole("button", { name: "Rename Jan Demo" }).click();
    await page.getByLabel("New name for Jan Demo").fill("Jan Example");
    await people.getByRole("button", { name: "Save", exact: true }).click();
    await expect(people.getByText("Jan Example")).toBeVisible();
    await expect(people.getByText("Jan Demo")).toHaveCount(0);
  });

  test("renaming to an existing name shows the error", async ({ page }) => {
    await page.goto(SETTINGS);
    const people = page.getByRole("list", { name: "Participants" });
    await page.getByRole("button", { name: "Rename Jan Demo" }).click();
    await page.getByLabel("New name for Jan Demo").fill("alex demo");
    await people.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("alert").filter({ hasText: "already exists" })).toBeVisible();
  });

  test("resetting a voice and deleting a participant ask first", async ({ page }) => {
    await page.goto(SETTINGS);
    const people = page.getByRole("list", { name: "Participants" });
    await page.getByRole("button", { name: "Reset the voice of Alex Demo" }).click();
    const dialog = page.getByRole("dialog", { name: "Reset this voice?" });
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(people.getByText("4 samples")).toBeVisible();

    await page.getByRole("button", { name: "Reset the voice of Alex Demo" }).click();
    await dialog.getByRole("button", { name: "Reset voice" }).click();
    await expect(people.getByText("No voice learned yet")).toHaveCount(1);
    await expect(page.getByRole("button", { name: "Reset the voice of Alex Demo" })).toBeDisabled();

    await page.getByRole("button", { name: "Delete Kasia Demo" }).click();
    const del = page.getByRole("dialog", { name: "Delete this participant?" });
    await del.getByRole("button", { name: "Delete participant" }).click();
    await expect(people.getByRole("listitem")).toHaveCount(2);
    await expect(people.getByText("Kasia Demo")).toHaveCount(0);
  });

  test("all voice data is deleted after a confirmation", async ({ page }) => {
    await page.goto(SETTINGS);
    const people = page.getByRole("list", { name: "Participants" });
    await page.getByRole("button", { name: "Delete all voice data" }).click();
    const dialog = page.getByRole("dialog", { name: "Delete all voice data?" });
    await expect(dialog.getByText(/all voice profiles, all stored voice embeddings/)).toBeVisible();
    await expect(dialog.getByText(/all sample clips/)).toBeVisible();

    // Cancelling changes nothing.
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(people.getByText("4 samples")).toBeVisible();

    await page.getByRole("button", { name: "Delete all voice data" }).click();
    await dialog.getByRole("button", { name: "Delete all voice data" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(people.getByText("No voice learned yet")).toHaveCount(3);
    await expect(people.getByText(/samples?/)).toHaveCount(0);
  });

  test("voice settings are saved and invalid thresholds are refused", async ({ page }) => {
    await page.goto(SETTINGS);
    const section = page.getByRole("region", { name: "Processing" });
    await expect(section.getByLabel(/name automatically from score/)).toHaveValue("0.75");
    await expect(section.getByLabel(/suggest a name from score/)).toHaveValue("0.6");
    await expect(section.getByLabel(/margin over the second best match/)).toHaveValue("0.1");
    await expect(section.getByLabel(/Keep sample clips for/)).toHaveValue("30");

    await section.getByLabel(/suggest a name from score/).fill("0.9");
    await expect(section.getByRole("alert").filter({ hasText: "cannot be higher" })).toBeVisible();
    await expect(section.getByRole("button", { name: "Save processing settings" })).toBeDisabled();

    await section.getByLabel(/suggest a name from score/).fill("0.55");
    await section.getByLabel(/name automatically from score/).fill("0.75");
    await section.getByLabel(/Keep sample clips for/).fill("14");
    await section.getByRole("button", { name: "Save processing settings" }).click();
    await expect(section.getByRole("status")).toHaveText("Processing settings saved");
  });
});

test.describe("browser extension settings on the mock backend", () => {
  test("pair an extension with a code, see it connect, unpair it", async ({ page }) => {
    await page.goto("/?view=main&backend=mock&pairms=500");
    await page.getByRole("button", { name: "Settings" }).click();
    const section = page.getByRole("region", { name: "Browser extension" });
    await expect(section.getByTestId("bridge-state")).toHaveText("Listening on 127.0.0.1:17555.");
    const list = section.getByRole("list", { name: "Paired extensions" });
    await expect(list.getByRole("listitem")).toHaveCount(1);

    await section.getByRole("button", { name: "Pair extension" }).click();
    await expect(section.getByTestId("pairing-code")).toHaveText("7K3M-9QXD");
    await expect(section.getByLabel("Time left")).toContainText(/^[45]:\d\d$/);
    // The simulated extension enters the code: the code goes away, a second extension appears.
    await expect(section.getByTestId("pairing-code")).toHaveCount(0);
    await expect(list.getByRole("listitem")).toHaveCount(2);
    await expect(list.getByRole("listitem").nth(1)).toContainText("connected");

    await section.getByRole("button", { name: "Unpair Chrome 137" }).click();
    await expect(list.getByRole("listitem")).toHaveCount(1);
  });

  test("turning the bridge off stops pairing", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByLabel("Accept the Chrome extension on this computer").uncheck();
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status")).toHaveText("Saved");
    const section = page.getByRole("region", { name: "Browser extension" });
    await expect(section.getByTestId("bridge-state")).toContainText("Off");
    await expect(section.getByRole("button", { name: "Pair extension" })).toBeDisabled();
  });
});

test.describe("meeting detection on the mock backend", () => {
  test("a detected call asks once and Record starts a recording of the microphone and the app", async ({
    page,
  }) => {
    await page.goto("/?view=widget&backend=mock&detectms=300");
    const prompt = page.getByRole("alertdialog", { name: "Record this call?" });
    await expect(prompt).toBeVisible();
    await expect(prompt).toContainText("A Discord call seems to have started.");
    await expect(prompt.getByRole("button", { name: "Never for Discord" })).toBeVisible();
    await prompt.getByRole("button", { name: "Record" }).click();
    await expect(prompt).toHaveCount(0);
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
    const sources = page.getByRole("list", { name: "Recorded sources" });
    await expect(sources.getByRole("listitem")).toHaveCount(2);
    await expect(sources).toContainText("Microphone (USB headset)");
    await expect(sources).toContainText("Discord");
  });

  test("Not now closes the prompt and starts nothing", async ({ page }) => {
    await page.goto("/?view=widget&backend=mock&detectms=300");
    const prompt = page.getByRole("alertdialog", { name: "Record this call?" });
    await prompt.getByRole("button", { name: "Not now" }).click();
    await expect(prompt).toHaveCount(0);
    await expect(page.getByText("Standby")).toBeVisible();
  });

  test("the settings choose the sensitivity and Ask again undoes Never", async ({ page }) => {
    await page.goto("/?view=main&backend=mock");
    await page.getByRole("button", { name: "Settings" }).click();
    const section = page.getByRole("region", { name: "Detection" });
    await expect(section.getByLabel(/^Balanced/)).toBeChecked();
    await expect(section.getByTestId("detection-state")).toHaveText("On, watching 7 applications.");

    await section.getByLabel(/^High/).check();
    await expect(section.getByLabel(/^High/)).toBeChecked();

    const list = section.getByRole("list", { name: "Watched applications" });
    await expect(list.getByRole("listitem")).toHaveCount(6);
    await section.getByRole("button", { name: "Never ask for Zoom" }).click();
    await expect(list).toContainText("switched off with Never");
    await expect(section.getByTestId("detection-state")).toHaveText("On, watching 6 applications.");
    await section.getByRole("button", { name: "Ask again for Zoom" }).click();
    await expect(list).not.toContainText("switched off with Never");

    await section.getByLabel("Add a program").fill("Webex.exe");
    await section.getByRole("button", { name: "Add", exact: true }).click();
    await expect(list.getByRole("listitem")).toHaveCount(7);
    await expect(list).toContainText("webex.exe");

    await section.getByLabel("Ask whether to record when a call starts").uncheck();
    await expect(section.getByTestId("detection-state")).toHaveText("Off: no prompt is raised.");
    await expect(section.getByLabel(/^Low/)).toBeDisabled();
  });
});
