import { type Page, expect, test } from "@playwright/test";
import { recordFromGate } from "./helpers";

const MAIN = "/?view=main&backend=mock&rate=0";
const DRAFT = `${MAIN}&page=draft&meeting=m5`;

async function openCycles(page: Page) {
  await page.goto(MAIN);
  await page.getByRole("button", { name: "Cycles", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Cycles", level: 2 })).toBeVisible();
}

test.describe("cycles on the mock backend", () => {
  test("lists the repositories with their state and the cycles in them", async ({ page }) => {
    await openCycles(page);
    const clone = page.getByRole("article", { name: "Repository team-cycles" });
    await expect(clone.getByText("State as of 2026-11-12")).toBeVisible();
    await expect(clone.getByRole("list", { name: "Cycles of team-cycles" })).toContainText(
      "Nomad rover",
    );
    await expect(
      page.getByRole("article", { name: "Repository local" }).getByText("Local repository"),
    ).toBeVisible();
    // A broken cycle is listed with the reason, not hidden.
    await expect(clone.getByText("Needs fixing")).toBeVisible();
    await expect(
      clone.getByText(/participants\[1\]\.id: duplicate participant id "sam"/),
    ).toBeVisible();

    await clone.getByRole("button", { name: "Pull team-cycles" }).click();
    await expect(clone.getByRole("status")).toHaveText("Updated from the remote.");
    await expect(clone.getByText("State as of 2026-11-13")).toBeVisible();
  });

  test("a clone that cannot be updated is shown as offline", async ({ page }) => {
    await openCycles(page);
    await page.getByLabel("Repository address").fill("https://offline.example/offline-cycles.git");
    await page.getByRole("button", { name: "Add repository" }).click();
    const repo = page.getByRole("article", { name: "Repository offline-cycles" });
    await expect(repo).toBeVisible();
    await repo.getByRole("button", { name: "Pull offline-cycles" }).click();
    await expect(repo.getByText("Offline", { exact: true })).toBeVisible();
    await expect(repo.getByText(/showing the state of the last successful pull/)).toBeVisible();
  });

  test("create a cycle with a participant and a template", async ({ page }) => {
    await openCycles(page);
    await page.getByLabel("Cycle id").fill("ROVER2");
    await page.getByLabel("Name", { exact: true }).fill("Rover two");
    await page.getByRole("button", { name: "Create cycle" }).click();
    await expect(page.getByText("Rover two · repository local")).toBeVisible();

    // Nothing changed yet: Save is disabled.
    const save = page.getByRole("button", { name: "Save", exact: true });
    await expect(save).toBeDisabled();

    await page.getByRole("button", { name: "Add participant" }).click();
    await page.getByLabel("Participant 1 id").fill("kat");
    await page.getByLabel("Participant 1 name").fill("Kat Demo");
    await page.getByLabel("Participant 1 GitHub login").fill("kat-demo");
    await expect(save).toBeEnabled();
    await save.click();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
    await expect(save).toBeDisabled();

    // A template: an unknown placeholder is shown while typing and blocks saving.
    await page.getByLabel("New template name").fill("weekly");
    await page.getByRole("button", { name: "New template" }).click();
    const group = page.getByRole("group", { name: "Template weekly" });
    const body = group.getByRole("textbox", { name: "Template body" });
    await body.fill("# {title}\n\n{nonsense}\n");
    await expect(group.getByRole("alert", { name: "Template problems" })).toContainText(
      "unknown placeholder {nonsense}",
    );
    await expect(group.getByRole("button", { name: "Save template" })).toBeDisabled();

    await group.getByRole("button", { name: "Add section" }).click();
    await group.getByLabel("Section 1 id").fill("progress");
    await group.getByLabel("Section 1 title").fill("Progress");
    await body.fill("# {title}\n\n{section:progress}\n");
    await expect(group.getByText("The template is valid.")).toBeVisible();
    await group.getByRole("button", { name: "Save template" }).click();
    await expect(group.getByRole("status")).toHaveText("Template saved");
    await expect(page.getByRole("list", { name: "Templates" })).toContainText(
      "weekly · 1 sections",
    );

    // The new cycle and its template are usable in the report defaults.
    const defaultTemplate = page.getByLabel("Default template");
    await expect(defaultTemplate.locator("option", { hasText: "weekly" })).toHaveCount(1);
  });

  test("problems of a save are shown at the top and next to the field", async ({ page }) => {
    await page.goto(`${MAIN}&page=cycle&cycle=NOMAD`);
    await page.getByRole("button", { name: "Add participant" }).click();
    await page.getByLabel("Participant 5 id").fill("alex");
    await page.getByLabel("Participant 5 name").fill("Another Alex");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    const problems = page.getByRole("alert", { name: "Problems" });
    await expect(problems).toContainText("participants[4].id: duplicate participant id");
    await expect(
      page.getByRole("alert").filter({ hasText: /^duplicate participant id/ }),
    ).toBeVisible();
    // Fixing it saves and clears the problems.
    await page.getByLabel("Participant 5 id").fill("alex2");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(problems).toHaveCount(0);
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  });

  test("an empty participant id and a bad webhook are refused", async ({ page }) => {
    await page.goto(`${MAIN}&page=cycle&cycle=NOMAD`);
    await page.getByRole("button", { name: "Add participant" }).click();
    await page.getByLabel("Participant 5 name").fill("No Id");
    await page.getByLabel("Webhook reference").fill("https://discord.example/hook");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    const problems = page.getByRole("alert", { name: "Problems" });
    await expect(problems).toContainText(
      "participants[4].id: the participant id must not be empty",
    );
    await expect(problems).toContainText("targets.discord.webhook");
  });

  test("the report defaults and the meeting types can be edited", async ({ page }) => {
    await page.goto(`${MAIN}&page=cycle&cycle=NOMAD`);
    await page.getByLabel("Default mode").selectOption("by-type");
    await expect(page.getByText("The meeting type picks a template of the cycle.")).toBeVisible();
    await page.getByRole("button", { name: "Add meeting type" }).click();
    await page.getByLabel("Meeting type 4", { exact: true }).fill("weekly");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
    await page.getByRole("button", { name: "Remove meeting type 4" }).click();
    await page.getByLabel("Instruction").fill("Keep it short.");
    await expect(page.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  });
});

test.describe("report mode gate on the mock backend", () => {
  const WIDGET = "/?view=widget&backend=mock&rate=0";
  const gateOf = (page: Page) =>
    page.getByRole("dialog", { name: "Choose the report for this recording" });

  test("shows the last choice and starts only when it is confirmed", async ({ page }) => {
    await page.goto(WIDGET);
    await page.getByRole("button", { name: "Record" }).click();
    const gate = gateOf(page);
    await expect(gate.getByText("Record with:")).toBeVisible();
    await expect(gate.getByText("NOMAD · fixed")).toBeVisible();
    // Nothing is recording yet.
    await expect(page.getByText("Standby")).toBeVisible();
    await gate.getByRole("button", { name: "Cancel" }).click();
    await expect(gate).toHaveCount(0);
    await expect(page.getByText("Standby")).toBeVisible();

    await page.getByRole("button", { name: "Record" }).click();
    await gate.getByRole("button", { name: "Start recording" }).click();
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
  });

  test("refuses without a mode and passes with one", async ({ page }) => {
    await page.goto(WIDGET);
    await page.getByRole("button", { name: "Record" }).click();
    const gate = gateOf(page);
    await gate.getByRole("button", { name: "Change…" }).click();
    const start = gate.getByRole("button", { name: "Start recording" });
    await expect(gate.getByLabel("Cycle", { exact: true })).toHaveValue("NOMAD");
    // The cycle's default is preselected; no mode is also a state the user can choose.
    await expect(gate.getByLabel("Report mode")).toHaveValue("fixed");
    await gate.getByLabel("Report mode").selectOption("");
    await expect(start).toBeDisabled();
    await expect(gate.getByRole("status")).toHaveText("Choose a report mode.");

    await gate.getByLabel("Report mode").selectOption("by-type");
    await expect(gate.getByText("The meeting type picks a template of the cycle.")).toBeVisible();
    await gate.getByLabel("Meeting type").selectOption("");
    await expect(start).toBeDisabled();
    await expect(gate.getByRole("status")).toHaveText("Choose the meeting type.");
    await gate.getByLabel("Meeting type").selectOption("budget");
    await expect(start).toBeEnabled();
    await start.click();
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();

    // The choice is remembered for the next time.
    await page.getByRole("button", { name: "Stop" }).click();
    await page.getByRole("button", { name: "Later" }).click();
    await page.getByRole("button", { name: "Record" }).click();
    await expect(gate.getByText("NOMAD · by-type (budget)")).toBeVisible();
  });

  test("a cycle that needs fixing cannot be chosen", async ({ page }) => {
    await page.goto(WIDGET);
    await page.getByRole("button", { name: "Record" }).click();
    const gate = gateOf(page);
    await gate.getByRole("button", { name: "Change…" }).click();
    await expect(gate.getByRole("option", { name: /OLDPROJ.*needs fixing/ })).toBeDisabled();
    await gate.getByLabel("Cycle", { exact: true }).selectOption("SANDBOX");
    await expect(gate.getByLabel("Report mode")).toHaveValue("fixed");
    await expect(gate.getByRole("button", { name: "Start recording" })).toBeEnabled();
  });

  test("a new cycle can be created from the gate", async ({ page }) => {
    await page.goto(WIDGET);
    await page.getByRole("button", { name: "Record" }).click();
    const gate = gateOf(page);
    await gate.getByRole("button", { name: "Change…" }).click();
    await gate.getByRole("button", { name: "New cycle…" }).click();
    await gate.getByLabel("Cycle id").fill("BRAND-NEW");
    await gate.getByLabel("Name", { exact: true }).fill("Brand new");
    await gate.getByRole("button", { name: "Create cycle" }).click();
    await expect(gate.getByLabel("Cycle", { exact: true })).toHaveValue("BRAND-NEW");
    await gate.getByRole("button", { name: "Start recording" }).click();
    await expect(page.getByText("Recording", { exact: true })).toBeVisible();
  });
});

test.describe("draft editor on the mock backend", () => {
  const item = (page: Page, text: string | RegExp) =>
    page
      .getByRole("listitem")
      .filter({ has: page.getByRole("button", { name: text, exact: typeof text === "string" }) });

  test("is reachable from the meeting list and the queue", async ({ page }) => {
    await page.goto(MAIN);
    const card = page.getByRole("article").filter({ hasText: "ORBIT weekly sync" });
    await card.getByRole("button", { name: "Open the draft of ORBIT weekly sync" }).click();
    await expect(page.getByRole("heading", { name: "Draft" })).toBeVisible();

    await page.goto(`${MAIN}&page=queue`);
    await page.getByRole("button", { name: "Open the draft of ORBIT weekly sync" }).click();
    await expect(page.getByRole("heading", { name: "Draft" })).toBeVisible();
    await expect(page.getByLabel("Report mode")).toHaveValue("auto");
  });

  test("shows the sections, the markers and the summary", async ({ page }) => {
    await page.goto(DRAFT);
    await expect(page.getByText("Language: en")).toBeVisible();
    await expect(page.getByRole("region", { name: "Summary" })).toContainText(
      "two percent under plan",
    );
    const columns = page.getByLabel("Sections", { exact: true });
    await expect(columns.getByRole("region")).toHaveCount(4);
    await expect(
      page.getByRole("list", { name: "Items of Thruster tests" }).getByRole("listitem"),
    ).toHaveCount(3);
    // Quiet markers: inferred and unknown values, unverified quotes, one edited item.
    await expect(columns.getByText("unverified quote", { exact: true })).toHaveCount(2);
    await expect(page.getByRole("img", { name: "inferred" }).first()).toHaveAttribute(
      "title",
      "inferred",
    );
    await expect(page.getByRole("img", { name: "unknown" }).first()).toHaveAttribute(
      "title",
      "unknown",
    );
    await expect(page.getByRole("img", { name: "Edited by you" })).toHaveCount(1);
    await page.getByText("Warnings (3)").click();
    await expect(page.getByRole("list", { name: "Warnings" })).toContainText(
      "(00:15:00 to 00:30:00)",
    );
  });

  test("an item is edited in place: Enter saves, Escape cancels", async ({ page }) => {
    await page.goto(DRAFT);
    const text =
      "The last thruster firing ran for 90 seconds and the chamber pressure stayed flat.";
    await page.getByRole("button", { name: text, exact: true }).click();
    const edit = page.getByRole("textbox", { name: `Text of the item: ${text}` });
    await edit.fill("A different text");
    await edit.press("Escape");
    await expect(page.getByRole("textbox", { name: /^Text of the item/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: text, exact: true })).toBeVisible();
    await expect(page.getByRole("img", { name: "Edited by you" })).toHaveCount(1);

    await page.getByRole("button", { name: text, exact: true }).click();
    const again = page.getByRole("textbox", { name: `Text of the item: ${text}` });
    await again.fill("The last firing ran for 90 seconds.");
    await again.press("Enter");
    await expect(
      page.getByRole("button", { name: "The last firing ran for 90 seconds.", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("img", { name: "Edited by you" })).toHaveCount(2);
  });

  test("the assignee and the due date of a task are set from the cycle's participants", async ({
    page,
  }) => {
    await page.goto(DRAFT);
    const task = "Share the budget sheet before the review.";
    const assignee = page.getByLabel(`Assignee of: ${task}`);
    await expect(assignee.locator("option")).toHaveText([
      "Unassigned",
      "Alex Demo",
      "Jan Demo",
      "Kasia Demo",
      "Piotr Demo",
    ]);
    await assignee.selectOption("kasia");
    await expect(assignee).toHaveValue("kasia");
    await page.getByLabel(`Due date of: ${task}`).fill("2026-12-01");
    await expect(page.getByLabel(`Due date of: ${task}`)).toHaveValue("2026-12-01");
    // Both were unknown and are now stated: no marker, and the item counts as edited.
    await expect(item(page, task).getByRole("img", { name: "unknown" })).toHaveCount(0);
    await expect(item(page, task).getByRole("img", { name: "Edited by you" })).toBeVisible();
    await page.getByRole("button", { name: "Rendered report" }).click();
    await expect(page.getByLabel("Rendered report")).toContainText(
      "- [ ] Share the budget sheet before the review. (@Kasia Demo, due 2026-12-01)",
    );
  });

  test("an item is moved to another section from the menu", async ({ page }) => {
    await page.goto(DRAFT);
    const text = "The new valve causes only a small ripple at ignition that can be filtered out.";
    const from = page.getByRole("list", { name: "Items of Thruster tests" });
    const to = page.getByRole("list", { name: "Items of Ground segment" });
    await expect(from.getByRole("button", { name: text, exact: true })).toBeVisible();
    await page.getByLabel(`Move to section: ${text}`).selectOption({ label: "Ground segment" });
    await expect(to.getByRole("button", { name: text, exact: true })).toBeVisible();
    await expect(from.getByRole("button", { name: text, exact: true })).toHaveCount(0);
    await expect(
      to
        .getByRole("listitem")
        .filter({ hasText: text })
        .getByRole("img", { name: "Edited by you" }),
    ).toBeVisible();
    // The rendered report follows.
    await page.getByRole("button", { name: "Rendered report" }).click();
    await expect(page.getByLabel("Rendered report")).toContainText(
      /## Ground segment[\s\S]*The new valve causes/,
    );
  });

  test("an item is moved to another section by dragging", async ({ page }) => {
    // Tall enough that nothing scrolls while dragging.
    await page.setViewportSize({ width: 1280, height: 1500 });
    await page.goto(DRAFT);
    const text = "The new valve causes only a small ripple at ignition that can be filtered out.";
    const handle = page.getByRole("listitem").filter({ hasText: text }).locator(".drag-handle");
    await handle.dragTo(page.getByRole("region", { name: "Follow-ups" }).locator(".column-head"));
    await expect(
      page
        .getByRole("list", { name: "Items of Follow-ups" })
        .getByRole("button", { name: text, exact: true }),
    ).toBeVisible();
    await expect(
      page
        .getByRole("list", { name: "Items of Thruster tests" })
        .getByRole("button", { name: text, exact: true }),
    ).toHaveCount(0);
  });

  test("the source popover shows quote, speaker, time and match, and jumps to the transcript", async ({
    page,
  }) => {
    await page.goto(DRAFT);
    const task = "Update the ground station schedule for Thursday.";
    await page.getByRole("button", { name: `Source of: ${task}` }).click();
    const pop = page.getByRole("dialog", { name: `Source of: ${task}` });
    await expect(
      pop.getByText("I will update the ground station schedule for Thursday."),
    ).toBeVisible();
    await expect(pop.getByText("jan", { exact: true })).toBeVisible();
    await expect(pop.getByText("00:00:41")).toBeVisible();
    await expect(pop.getByText("99% · found")).toBeVisible();
    // Escape closes the popover.
    await pop.getByRole("button", { name: "Close" }).focus();
    await page.keyboard.press("Escape");
    await expect(pop).toHaveCount(0);

    await page.getByRole("button", { name: `Source of: ${task}` }).click();
    await page.getByRole("button", { name: "Jump to transcript" }).click();
    await expect(page.getByRole("heading", { name: "Transcript" })).toBeVisible();
    const current = page.locator('li[aria-current="true"]');
    await expect(current).toHaveCount(1);
    await expect(current).toContainText("I will update the ground station schedule for Thursday.");
    await expect(current).toHaveClass(/seg-highlight/);
  });

  test("an unverified quote is shown with its low match", async ({ page }) => {
    await page.goto(DRAFT);
    await page
      .getByRole("button", { name: "Source of: Buy a second test stand with the underspend." })
      .click();
    const pop = page.getByRole("dialog", { name: /Source of: Buy a second/ });
    await expect(pop.getByText("41% · not found")).toBeVisible();
  });

  test("an item is added to a section and one is deleted", async ({ page }) => {
    await page.goto(DRAFT);
    await page.getByRole("button", { name: "Add item to Ground segment" }).click();
    const form = page.getByRole("form", { name: "Add item to Ground segment" });
    await form.getByLabel("Type of the new item").selectOption("task");
    await form.getByLabel("Text of the new item").fill("Check the antenna cable");
    await form.getByRole("button", { name: "Add", exact: true }).click();
    const list = page.getByRole("list", { name: "Items of Ground segment" });
    await expect(
      list.getByRole("button", { name: "Check the antenna cable", exact: true }),
    ).toBeVisible();
    await expect(list.getByRole("listitem")).toHaveCount(3);

    await page.getByRole("button", { name: "Delete: Check the antenna cable" }).click();
    await expect(list.getByRole("listitem")).toHaveCount(2);
  });

  test("sections are renamed, added, reordered and removed", async ({ page }) => {
    await page.goto(DRAFT);
    await page.getByRole("button", { name: "Edit sections" }).click();
    const editor = page.getByRole("region", { name: "Edit sections" });
    await editor.getByLabel("Section 2 title").fill("Ground station");
    await editor.getByLabel("Title of the new section").fill("Open questions");
    await editor.getByRole("button", { name: "Add section" }).click();
    await editor.getByRole("button", { name: "Move section Open questions up" }).click();
    await editor.getByRole("button", { name: "Remove section Follow-ups" }).click();
    await expect(editor.getByText("3 items lose their section")).toBeVisible();
    await editor.getByRole("button", { name: "Save sections" }).click();

    const columns = page.getByLabel("Sections", { exact: true }).getByRole("heading", { level: 3 });
    await expect(columns).toHaveText([
      "Thruster tests",
      "Ground station",
      "Budget and procurement",
      "Open questions",
    ]);
    // The items of the removed section are shown in the last one.
    await expect(
      page
        .getByRole("list", { name: "Items of Open questions" })
        .getByRole("button", { name: "Keep the launch window as planned.", exact: true }),
    ).toBeVisible();
  });

  test("the report mode changes the layout, not the items", async ({ page }) => {
    await page.goto(DRAFT);
    await page.getByLabel("Report mode").selectOption("fixed");
    await expect(page.getByText(/groups items by type/)).toBeVisible();
    await expect(
      page.getByLabel("Sections", { exact: true }).getByRole("region", { name: "Decisions" }),
    ).toBeVisible();
    // Fixed mode has no sections to move between.
    await expect(page.getByLabel(/^Move to section:/)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Edit sections" })).toHaveCount(0);

    await page.getByLabel("Report mode").selectOption("by-type");
    await page.getByLabel("Meeting type").selectOption("budget");
    await expect(page.getByLabel("Meeting type")).toHaveValue("budget");
    await page.getByLabel("Report mode").selectOption("template");
    await page.getByLabel("Template", { exact: true }).selectOption("budget-review");
    await expect(page.getByLabel("Template", { exact: true })).toHaveValue("budget-review");
    // The items are the same, so nothing counts as edited by the mode change.
    await expect(page.getByRole("img", { name: "Edited by you" })).toHaveCount(1);
  });

  test("the rendered report is shown as text with a format and a copy button", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto(DRAFT);
    await page.getByRole("button", { name: "Rendered report" }).click();
    const pre = page.getByLabel("Rendered report", { exact: true });
    await expect(pre).toContainText("# Meeting NOMAD 2026-11-13");
    await expect(pre).toContainText(
      "- [ ] Update the ground station schedule for Thursday. (@Jan Demo, due 2026-11-19?)",
    );
    await expect(pre).toContainText("(unverified quote)");
    await page.getByLabel("Report format").selectOption("chat");
    await expect(pre).toContainText(
      "- Update the ground station schedule for Thursday. (@Jan Demo, due 2026-11-19?)",
    );
    await expect(pre).not.toContainText("[ ]");
    await page.getByRole("button", { name: "Copy" }).click();
    await expect(page.getByRole("region", { name: "Report" }).getByRole("status")).toHaveText(
      "Copied",
    );
    expect(await page.evaluate(() => navigator.clipboard.readText())).toContain("# Meeting NOMAD");
    await page.getByRole("button", { name: "Items", exact: true }).click();
    await expect(page.getByLabel("Sections", { exact: true })).toBeVisible();
  });

  test("a newer draft is accepted from the banner", async ({ page }) => {
    await page.goto(`${DRAFT}&newdraft=1`);
    const banner = page.getByRole("status").filter({ hasText: "A newer draft is available." });
    await expect(banner).toBeVisible();
    await banner.getByRole("button", { name: "Accept new draft" }).click();
    await expect(banner).toHaveCount(0);
    await expect(
      page.getByRole("button", {
        name: "The valve vendor confirmed the next delivery for the end of the month.",
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", {
        name: "Record the next test with a high-speed camera.",
        exact: true,
      }),
    ).toHaveCount(0);
  });

  test("a newer draft can be discarded and the edited one stays", async ({ page }) => {
    await page.goto(`${DRAFT}&newdraft=1`);
    const banner = page.getByRole("status").filter({ hasText: "A newer draft is available." });
    await banner.getByRole("button", { name: "Discard new draft" }).click();
    await expect(banner).toHaveCount(0);
    await expect(
      page.getByRole("button", {
        name: "Record the next test with a high-speed camera.",
        exact: true,
      }),
    ).toBeVisible();
  });

  test("a meeting without a draft offers to build one", async ({ page }) => {
    await page.goto(`${MAIN}&page=draft&meeting=m4`);
    await expect(page.getByText(/no draft for this meeting yet/)).toBeVisible();
    await page.getByRole("button", { name: "Build draft…" }).click();
    const dialog = page.getByRole("dialog", { name: "Build the draft" });
    await expect(dialog.getByLabel(/^Local model/)).toBeChecked();
    await expect(dialog.getByText("gemma-4-12b-q4_k_m.gguf")).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
  });
});

test.describe("cloud confirmation on the mock backend", () => {
  test("before the transcript exists it explains and offers to process locally", async ({
    page,
  }) => {
    await page.goto("/?view=widget&backend=mock&rate=0&jobms=400");
    await recordFromGate(page);
    await page.getByRole("button", { name: "Stop" }).click();
    const prompt = page.getByRole("dialog", { name: "Process now or later?" });
    await expect(prompt.getByLabel(/^Local model/)).toBeChecked();
    await prompt.getByLabel(/^Cloud/).check();
    await prompt.getByRole("button", { name: "Process now" }).click();

    const dialog = page.getByRole("dialog", { name: "Send this meeting to the cloud?" });
    await expect(dialog.getByText(/The confirmation happens after transcription/)).toBeVisible();
    // Nothing can be sent from here: there is no send button.
    await expect(dialog.getByRole("button", { name: /^Send text to/ })).toHaveCount(0);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    // Cancelling changed nothing: the prompt is still there.
    await expect(prompt).toBeVisible();
    await prompt.getByRole("button", { name: "Process now" }).click();
    await dialog.getByRole("button", { name: "Process locally up to the transcript" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(prompt).toHaveCount(0);
  });

  test("shows the host, the model and the counts, and cancelling does nothing", async ({
    page,
  }) => {
    await page.goto(`${MAIN}&jobms=400`);
    // Kickoff has no cycle, so its extraction is skipped; it still gets a transcript.
    await page
      .getByRole("article")
      .filter({ hasText: "Kickoff" })
      .getByRole("button", { name: "Process", exact: true })
      .click();
    await page.getByRole("button", { name: "Queue", exact: true }).click();
    await expect(
      page.getByRole("article", { name: "Kickoff" }).getByText("Processed", { exact: true }),
    ).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: "Meetings", exact: true }).click();
    await page.getByRole("button", { name: "Open the draft of Kickoff" }).click();
    await page.getByRole("button", { name: "Build draft…" }).click();
    const build = page.getByRole("dialog", { name: "Build the draft" });
    await build.getByLabel(/^Cloud/).check();
    await expect(build.getByText(/only after you confirm what is sent/)).toBeVisible();
    await build.getByRole("button", { name: "Build draft" }).click();

    const dialog = page.getByRole("dialog", { name: "Send this meeting to the cloud?" });
    await expect(dialog.getByText("api.llm-vendor.example", { exact: true })).toBeVisible();
    await expect(dialog.getByText("large-model-1", { exact: true })).toBeVisible();
    await expect(
      dialog.getByText("Words of text").locator("xpath=following-sibling::dd"),
    ).toHaveText(/^\d+$/);
    await expect(
      dialog.getByText("Screenshot descriptions").locator("xpath=following-sibling::dd"),
    ).toHaveText("2");
    await expect(
      dialog.getByText("No audio and no images are sent.", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: "Send text to api.llm-vendor.example" }),
    ).toBeVisible();

    // Cancel: back at the choice, nothing was confirmed or started.
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(build).toBeVisible();
    await build.getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("button", { name: "Back to meetings" }).click();
    await page.getByRole("button", { name: "Queue", exact: true }).click();
    const job = page.getByRole("article", { name: "Kickoff" });
    await expect(job.getByText("Processed", { exact: true })).toBeVisible();
    await expect(job.getByText("Skipped: the meeting has no cycle")).toHaveCount(2);
  });
});

test.describe("language model settings on the mock backend", () => {
  const SETTINGS = `${MAIN}&page=settings`;

  test("keys are write-only: only the saved flag is shown", async ({ page }) => {
    await page.goto(SETTINGS);
    const section = page.getByRole("region", { name: "Language model" });
    await expect(section.getByText("Cloud API key: saved")).toBeVisible();
    await expect(section.getByText("External server key: not saved")).toBeVisible();

    await section
      .getByRole("form", { name: "External server key" })
      .locator("input")
      .fill("not-shown-anywhere");
    await section.getByRole("button", { name: "Save external key" }).click();
    await expect(section.getByText("External server key: saved")).toBeVisible();
    await expect(
      section.getByRole("form", { name: "External server key" }).locator("input"),
    ).toHaveValue("");
    await expect(page.locator("body")).not.toContainText("not-shown-anywhere");

    await section.getByRole("button", { name: "Clear cloud key" }).click();
    await expect(section.getByText("Cloud API key: not saved")).toBeVisible();
  });

  test("the model settings are validated and saved", async ({ page }) => {
    await page.goto(SETTINGS);
    const section = page.getByRole("region", { name: "Language model" });
    await expect(section.getByLabel("Model file (GGUF)")).toHaveValue(/gemma-4-12b/);
    await section.getByLabel(/Context size/).fill("1000");
    await expect(section.getByRole("alert")).toContainText("between 2048");
    await expect(
      section.getByRole("button", { name: "Save language model settings" }),
    ).toBeDisabled();
    await section.getByLabel(/Context size/).fill("8192");
    await section.getByLabel("Default language model").selectOption("external");
    await section.getByRole("button", { name: "Save language model settings" }).click();
    await expect(section.getByRole("status")).toHaveText("Language model settings saved");
  });

  test("the cycle repositories are listed in the settings", async ({ page }) => {
    await page.goto(SETTINGS);
    const repos = page.getByRole("list", { name: "Cycle repositories" });
    await expect(repos.getByRole("listitem")).toHaveCount(2);
    await expect(repos).toContainText("State as of 2026-11-12");
  });
});

test.describe("speakers of a cycle on the mock backend", () => {
  test("the header names the cycle and the participants are the cycle's", async ({ page }) => {
    await page.goto(`${MAIN}&page=speakers&meeting=m5`);
    await expect(page.getByText("Cycle: Nomad rover (NOMAD)")).toBeVisible();
    const select = page
      .getByRole("article", { name: "SPEAKER_03" })
      .getByLabel("Name for SPEAKER_03");
    await expect(select.locator("option", { hasText: "Piotr Demo" })).toHaveCount(1);
    await select.selectOption({ label: "Piotr Demo" });
    await expect(page.getByRole("status")).toHaveText("1 of 4 voices named");
  });
});
