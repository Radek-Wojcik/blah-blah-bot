import { type Page, expect, test } from "@playwright/test";

/** A short step makes the simulated outbox fast; the clock is frozen like in the other tests. */
const MAIN = "/?view=main&backend=mock&rate=0&jobms=80";
const DRAFT = `${MAIN}&page=draft&meeting=m5`;

interface Hooks {
  setOffline(offline: boolean): void;
  closeIssue(n: number): void;
  setVisibility(v: "public" | "private"): void;
  discordPosts(): { content: string }[];
  nextContext(cycleId: string): { text: string; issue: string }[];
  setTopicsUnknown(unknown: boolean): void;
}
const hooks = <T>(page: Page, run: (h: Hooks) => T): Promise<T> =>
  page.evaluate(`(${run.toString()})(window.__bbbMock)`) as Promise<T>;

const panel = (page: Page) => page.getByRole("region", { name: "Publishing" });
/** The switch of a target in the panel: Off, On approval, Automatic. */
const rule = (page: Page, target: "GitHub" | "Discord") =>
  panel(page).getByRole("radiogroup", { name: `${target} publishing rule` });
const statuses = (page: Page) => page.getByRole("region", { name: "Status changes" });

async function approveAll(page: Page) {
  for (const target of ["#12", "#11", "t-1"])
    await statuses(page)
      .getByRole("button", { name: `Approve ${target}` })
      .click();
  await expect(statuses(page).getByRole("status")).toContainText("3 of 3 approved");
}

/** Presses Publish, ticks the confirmation of the public repository and confirms. */
async function publishConfirmed(page: Page) {
  await panel(page).getByRole("button", { name: "Publish", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Publish to a public repository" }).click();
  await expect(dialog).toHaveCount(0);
}

test.describe("publishing on the mock backend", () => {
  test("demo cycle: approve the status changes and publish, the whole loop", async ({ page }) => {
    await page.goto(DRAFT);
    await expect(statuses(page).getByRole("listitem")).toHaveCount(3);
    // Each proposal shows the quote it rests on, the issue link and what approving does.
    const first = statuses(page).getByRole("listitem", { name: "Status change #12" });
    await expect(first.getByRole("blockquote")).toContainText(
      "The antenna mount drawings are finished and uploaded.",
    );
    await expect(first.getByRole("link", { name: "#12" })).toHaveAttribute(
      "href",
      "https://github.com/demo-org/nomad-cycles/issues/12",
    );
    await expect(first).toContainText("Closes issue #12 on GitHub");
    await expect(statuses(page).getByRole("listitem", { name: "Status change #11" })).toContainText(
      "unverified quote",
    );

    // Nothing is approved by itself, so nothing is written back yet.
    const plan = panel(page).getByRole("list", { name: "What will happen" });
    await expect(plan).toContainText("Create the issue");
    await expect(plan).not.toContainText("Close issue #12");
    await expect(panel(page)).toContainText("3 status changes are not approved");

    await approveAll(page);
    await expect(plan).toContainText("Close issue #12 (approved status change s1).");
    await expect(plan).toContainText("Close the topic t-1");
    await expect(panel(page)).not.toContainText("are not approved");

    await publishConfirmed(page);
    const outbox = panel(page).getByRole("list", { name: "Outbox" });
    await expect(outbox.getByRole("listitem")).toHaveCount(8);
    await expect(panel(page).getByText("Published: everything was sent.")).toBeVisible({
      timeout: 15_000,
    });
    for (const done of await outbox.getByRole("listitem").all())
      await expect(done).toContainText("Done");
    await expect(outbox.getByRole("link", { name: "#13" })).toHaveAttribute(
      "href",
      "https://github.com/demo-org/nomad-cycles/issues/13",
    );
    // The message went out after the issues, with links to them.
    const posts = await hooks(page, (h) => h.discordPosts());
    expect(posts).toHaveLength(1);
    expect(posts[0].content).toContain("Issues: #13, #14, #15, #16");

    // Everything chosen was sent: there is nothing left to publish.
    await expect(panel(page).getByRole("button", { name: "Publish", exact: true })).toBeDisabled();
    await expect(panel(page).getByRole("list", { name: "What was published" })).toBeVisible();

    // The draft is read-only now.
    await expect(page.getByText("This draft is locked.")).toBeVisible();
    await expect(page.getByRole("button", { name: /^Delete:/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Approve #12" })).toBeDisabled();
    await expect(page.getByLabel("Report mode")).toBeDisabled();
    await expect(page.getByRole("button", { name: /^Add item/ })).toHaveCount(0);
  });

  test("last write wins per topic, and the difference stays visible", async ({ page }) => {
    await page.goto(DRAFT);
    await approveAll(page);
    await publishConfirmed(page);
    await expect(panel(page).getByText("Published: everything was sent.")).toBeVisible({
      timeout: 15_000,
    });
    const conflict = panel(page).getByRole("listitem", { name: "Conflict in topic t-1" });
    await expect(panel(page)).toContainText(
      "The last write wins per topic: your version was kept.",
    );
    await expect(conflict).toContainText("text differs");
    await expect(conflict).toContainText("status differs");
    await expect(conflict).toContainText("− [open] Decide the rover wheel size and the tyre type");
    await expect(conflict).toContainText("+ [closed] Decide the rover wheel size");
  });

  test("the public repository has to be confirmed, and cancelling publishes nothing", async ({
    page,
  }) => {
    await page.goto(DRAFT);
    await expect(panel(page)).toContainText("demo-org/nomad-cycles · public");
    await expect(panel(page)).toContainText(
      "is public: everybody can read what is published to it",
    );
    await panel(page).getByRole("button", { name: "Publish", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
    const confirm = dialog.getByRole("button", { name: "Publish to a public repository" });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("checkbox").check();
    await expect(confirm).toBeEnabled();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(panel(page).getByRole("list", { name: "Outbox" })).toHaveCount(0);
    await expect(page.getByText("This draft is locked.")).toHaveCount(0);
    // Escape closes it too.
    await panel(page).getByRole("button", { name: "Publish", exact: true }).click();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });

  test("a private repository needs no confirmation", async ({ page }) => {
    await page.goto(DRAFT);
    await hooks(page, (h) => h.setVisibility("private"));
    // The plan is made again when the draft changes.
    await statuses(page).getByRole("button", { name: "Reject t-1" }).click();
    await expect(panel(page)).not.toContainText("is public:");
    await panel(page).getByRole("button", { name: "Publish", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(panel(page).getByText("Published: everything was sent.")).toBeVisible({
      timeout: 15_000,
    });
  });

  test("offline: operations wait, then finish when the network is back", async ({ page }) => {
    await page.goto(`${DRAFT}&net=offline`);
    // Offline and not confirmed, whether the repository is public cannot be read.
    await expect(panel(page)).toContainText("could not be checked");
    await publishConfirmed(page);
    const banner = panel(page)
      .getByRole("status")
      .filter({ hasText: /^Offline:/ });
    await expect(banner).toContainText("GitHub did not answer");
    const outbox = panel(page).getByRole("list", { name: "Outbox" });
    await expect(outbox.getByRole("listitem").first()).toContainText("Waiting");
    await expect(outbox.getByRole("listitem").first()).toContainText("the network is offline");
    await expect(outbox.getByRole("listitem").first()).toContainText("1 attempt");
    // A retry while still offline fails again and counts the attempt.
    await panel(page).getByRole("button", { name: "Retry all now" }).click();
    await expect(outbox.getByRole("listitem").first()).toContainText("2 attempts");
    await expect(page.getByText("This draft is locked.")).toBeVisible();

    await hooks(page, (h) => h.setOffline(false));
    await expect(panel(page).getByText("Published: everything was sent.")).toBeVisible({
      timeout: 15_000,
    });
    await expect(banner).toHaveCount(0);
  });

  test("cancelling the waiting operations gives the draft back", async ({ page }) => {
    await page.goto(`${DRAFT}&net=offline`);
    await publishConfirmed(page);
    const outbox = panel(page).getByRole("list", { name: "Outbox" });
    await expect(outbox.getByRole("listitem").first()).toContainText("Waiting");
    await expect(page.getByText("This draft is locked.")).toBeVisible();
    // The commit comes first and the rest follows it: cancel from the last one back.
    const count = await outbox.getByRole("listitem").count();
    for (let i = 0; i < count; i += 1) {
      await outbox
        .getByRole("button", { name: /^Cancel:/ })
        .last()
        .click();
    }
    await expect(page.getByText("This draft is locked.")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Delete:/ }).first()).toBeVisible();
    await expect(outbox.getByRole("listitem").first()).toContainText("Cancelled");
  });

  test("a closed issue is done in the tasks of the cycle after the next pull", async ({ page }) => {
    await page.goto(DRAFT);
    await approveAll(page);
    await publishConfirmed(page);
    await expect(panel(page).getByText("Published: everything was sent.")).toBeVisible({
      timeout: 15_000,
    });
    // A colleague closes the issue made from a task; the state is read at the next pull.
    await hooks(page, (h) => h.closeIssue(15));
    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Edit NOMAD" }).click();
    const tasks = page.getByRole("region", { name: "Tasks" });
    await expect(tasks).toContainText("demo-org/nomad-cycles · 3 open, 1 done");
    await expect(tasks).toContainText("As of 2026-11-12");
    await tasks.getByRole("button", { name: "Refresh from GitHub" }).click();
    await expect(tasks).toContainText("As of 2026-11-13");
    const open = tasks.getByRole("list", { name: "Open tasks" });
    const done = tasks.getByRole("list", { name: "Done tasks" });
    await expect(open.getByRole("listitem")).toHaveCount(4);
    await expect(done.getByRole("listitem")).toHaveCount(4);
    // #15 was closed by the colleague, #11 and #12 by the approved status changes.
    for (const n of ["#9", "#11", "#12", "#15"])
      await expect(done.getByRole("listitem").filter({ hasText: n }).first()).toContainText("Done");
    await expect(open).not.toContainText("#15");
    // The next processing is not given the closed one.
    const context = await hooks(page, (h) => h.nextContext("NOMAD"));
    expect(context.map((c) => c.issue)).toEqual(["#10", "#13", "#14", "#16"]);
  });

  test("the rule of a target is changed in the panel", async ({ page }) => {
    await page.goto(DRAFT);
    const discord = panel(page).getByRole("checkbox", { name: /Publish to Discord/ });
    await expect(discord).toBeChecked();
    await expect(rule(page, "Discord").getByRole("radio")).toHaveText([
      "Off",
      "On approval",
      "Automatic",
    ]);
    await expect(rule(page, "Discord").getByRole("radio", { name: "On approval" })).toBeChecked();
    await rule(page, "Discord").getByRole("radio", { name: "Off" }).click();
    await expect(rule(page, "Discord").getByRole("radio", { name: "Off" })).toBeChecked();
    await expect(discord).toBeDisabled();
    await expect(discord).not.toBeChecked();
    await expect(panel(page).getByRole("list", { name: "What will happen" })).not.toContainText(
      "Discord",
    );
    await rule(page, "Discord").getByRole("radio", { name: "On approval" }).click();
    await expect(discord).toBeEnabled();
    await expect(panel(page).getByRole("list", { name: "What will happen" })).toContainText(
      "Post the summary to Discord",
    );
    // The choice is the cycle's: it shows in the cycle settings too.
    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Edit NOMAD" }).click();
    await expect(page.getByLabel("Discord publish")).toHaveValue("on-approval");
  });

  test("Automatic is a position of the switch with its warning, and status changes stay manual", async ({
    page,
  }) => {
    await page.goto(DRAFT);
    const auto = rule(page, "GitHub").getByRole("radio", { name: "Automatic" });
    await auto.click();
    await expect(auto).toBeChecked();
    await expect(panel(page)).toContainText("Automatic: the report, the transcript");
    await expect(panel(page)).toContainText("cannot be taken back");
    // What no rule changes is said next to the switch, and holds: nothing is approved by itself.
    await expect(panel(page)).toContainText(
      "Status changes are never written back automatically, with any rule",
    );
    await expect(panel(page)).toContainText("3 status changes are not approved");
    await expect(panel(page).getByRole("list", { name: "What will happen" })).not.toContainText(
      "Close issue",
    );
    // The choice reaches cycle.yaml: the cycle editor shows it.
    await page.getByRole("button", { name: "Cycles", exact: true }).click();
    await page.getByRole("button", { name: "Edit NOMAD" }).click();
    await expect(page.getByLabel("GitHub publish")).toHaveValue("auto");
  });

  test("the public repository check still holds under Automatic", async ({ page }) => {
    await page.goto(DRAFT);
    await rule(page, "GitHub").getByRole("radio", { name: "Automatic" }).click();
    await expect(panel(page)).toContainText(
      "is public: everybody can read what is published to it",
    );
    await panel(page).getByRole("button", { name: "Publish", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Publish to a public repository?" });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    // Nothing was queued, and Off makes the target unavailable, also for a click.
    await expect(panel(page).getByRole("list", { name: "Outbox" })).toHaveCount(0);
    await rule(page, "GitHub").getByRole("radio", { name: "Off" }).click();
    await expect(panel(page).getByRole("checkbox", { name: /Publish to GitHub/ })).toBeDisabled();
  });

  test("topics.yaml is shown before and after, per topic, before publishing", async ({ page }) => {
    await page.goto(DRAFT);
    const diff = panel(page).getByRole("list", { name: "Planned topic changes" });
    // Only the new topic is planned until the closing of t-1 is approved.
    await expect(diff.getByRole("listitem")).toHaveCount(1);
    await expect(diff.getByRole("listitem", { name: "Topic m5-i07 added" })).toContainText(
      "+ [open] Confirm the delivery date of the antenna array",
    );
    await statuses(page).getByRole("button", { name: "Approve t-1" }).click();
    const changed = diff.getByRole("listitem", { name: "Topic t-1 changed" });
    await expect(changed).toContainText("− [open] Decide the rover wheel size and the tyre type");
    await expect(changed).toContainText("+ [closed] Decide the rover wheel size");
    await expect(changed).toContainText("text differs");
    await expect(changed).toContainText("Someone else changed this topic since your last pull");
    await expect(panel(page)).toContainText("Compared with the remote as of the last time");
    // Looking changes nothing: no operation exists.
    await expect(panel(page).getByRole("list", { name: "Outbox" })).toHaveCount(0);
  });

  test("when topics.yaml cannot be compared the plan names the topics and says so", async ({
    page,
  }) => {
    await page.goto(DRAFT);
    await hooks(page, (h) => h.setTopicsUnknown(true));
    await statuses(page).getByRole("button", { name: "Approve t-1" }).click();
    await expect(panel(page)).toContainText("listed by name only");
    await expect(panel(page).getByRole("list", { name: "Planned topic changes" })).toHaveCount(0);
    const plan = panel(page).getByRole("list", { name: "What will happen" });
    await expect(plan).toContainText("Add the topic");
    await expect(plan).toContainText("Close the topic t-1");
  });

  test("the rule of every cycle is set in the settings as well", async ({ page }) => {
    await page.goto(MAIN + "&page=settings");
    const github = page.getByRole("radiogroup", {
      name: "GitHub, cycle Nomad rover publishing rule",
    });
    await expect(github.getByRole("radio", { name: "On approval" })).toBeChecked();
    await github.getByRole("radio", { name: "Automatic" }).click();
    await expect(github.getByRole("radio", { name: "Automatic" })).toBeChecked();
    await expect(
      page.getByText("Status changes are never written back automatically"),
    ).toBeVisible();
    await page.getByRole("button", { name: "Meetings", exact: true }).click();
    await page.getByRole("button", { name: /^Open the draft of ORBIT weekly sync/ }).click();
    await expect(rule(page, "GitHub").getByRole("radio", { name: "Automatic" })).toBeChecked();
  });

  test("a missing webhook blocks the Discord post and the settings fix it", async ({ page }) => {
    await page.goto(MAIN + "&page=settings");
    const form = page.getByRole("form", { name: "Webhook for cred:discord/nomad" });
    await form.getByRole("button", { name: "Clear" }).click();
    await expect(form).toContainText("No webhook stored");
    await page.getByRole("button", { name: "Meetings", exact: true }).click();
    await page.getByRole("button", { name: /^Open the draft of ORBIT weekly sync/ }).click();
    await expect(panel(page).getByRole("alert")).toContainText("No Discord webhook is stored");
    await expect(panel(page).getByRole("button", { name: "Publish", exact: true })).toBeDisabled();
    await panel(page)
      .getByRole("checkbox", { name: /Publish to Discord/ })
      .uncheck();
    await expect(panel(page).getByRole("button", { name: "Publish", exact: true })).toBeDisabled();
  });
});

test.describe("status changes in the draft editor", () => {
  test("approve, reject and take a decision back; a comment is kept", async ({ page }) => {
    await page.goto(DRAFT);
    const card = statuses(page).getByRole("listitem", { name: "Status change #12" });
    await card.getByRole("button", { name: "Approve #12" }).click();
    await expect(card.getByRole("button", { name: "Approve #12" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(card).toContainText("Approved");
    await card.getByRole("button", { name: "Reject #12" }).click();
    await expect(card).toContainText("Rejected");
    await expect(card.getByRole("button", { name: "Approve #12" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    // Pressing the decision that is set takes it back.
    await card.getByRole("button", { name: "Reject #12" }).click();
    await expect(card).toContainText("Waiting for your decision");
    await card.getByLabel("Comment for #12").fill("Checked on the drawing server.");
    await card.getByLabel("Comment for #12").blur();
    await expect(card.getByRole("img", { name: "Edited by you" })).toBeVisible();
    await card.getByRole("button", { name: "Approve #12" }).click();
    await expect(card.getByLabel("Comment for #12")).toHaveValue("Checked on the drawing server.");
    await expect(statuses(page).getByRole("status")).toContainText("1 of 3 approved");
  });

  test("a status change is added by hand for an open issue", async ({ page }) => {
    await page.goto(DRAFT);
    // #11, #12 and t-1 have one already: only #10 is left to choose from.
    await statuses(page).getByRole("button", { name: "+ Add a status change" }).click();
    const form = statuses(page).getByRole("form", { name: "Add a status change" });
    await expect(form.getByLabel("Issue or topic").locator("option")).toHaveCount(1);
    await form.getByLabel("New status", { exact: true }).selectOption("done");
    await form.getByLabel("Comment of the new status change").fill("Finished last week");
    await form.getByRole("button", { name: "Add" }).click();
    const card = statuses(page).getByRole("listitem", { name: "Status change #10" });
    await expect(card).toContainText("Added by hand: there is no quote.");
    await expect(card.getByLabel("Comment for #10")).toHaveValue("Finished last week");
    await expect(card).toContainText("Waiting for your decision");
    await expect(statuses(page).getByRole("status")).toContainText("0 of 4 approved");
    // Every target has a change now: nothing is left to add.
    await expect(statuses(page).getByRole("button", { name: "+ Add a status change" })).toHaveCount(
      0,
    );
  });

  test("the quote jumps to the transcript", async ({ page }) => {
    await page.goto(DRAFT);
    await statuses(page).getByRole("button", { name: "Jump to the quote of #12" }).click();
    await expect(page.getByRole("heading", { name: "Transcript", level: 2 })).toBeVisible();
  });
});

test.describe("report previews", () => {
  test("the report as GitHub shows it and the message as Discord shows it", async ({ page }) => {
    await page.goto(DRAFT);
    await page.getByRole("button", { name: "Rendered report" }).click();
    await page.getByRole("button", { name: "Preview", exact: true }).click();
    const report = page.getByRole("article", { name: "Report preview" });
    await expect(
      report.getByRole("heading", { level: 2, name: "Meeting NOMAD 2026-11-13" }),
    ).toBeVisible();
    await expect(report.getByRole("checkbox").first()).toBeDisabled();
    await page.getByLabel("Report format").selectOption("chat");
    const message = page.getByRole("region", { name: "Discord message preview" });
    await expect(message).toContainText("Blah Blah Bot");
    await expect(message).toContainText("Meeting NOMAD 2026-11-13");
    await page.getByRole("button", { name: "Text", exact: true }).click();
    await expect(page.getByLabel("Rendered report", { exact: true })).toBeVisible();
  });
});

test.describe("token setup", () => {
  test("a token is typed masked, stored, and not shown again", async ({ page }) => {
    await page.goto(`${MAIN}&page=settings`);
    const form = page.getByRole("form", { name: "GitHub token for api.github.com" });
    await form.getByRole("button", { name: "Clear" }).click();
    await expect(form).toContainText("No token stored");
    const field = form.getByLabel("GitHub token for api.github.com");
    await expect(field).toHaveAttribute("type", "password");
    await field.fill("two words");
    await form.getByRole("button", { name: "Store" }).click();
    await expect(form.getByRole("alert")).toContainText("spaces or line breaks");
    await expect(form.getByRole("alert")).not.toContainText("two words");
    await field.fill("github_pat_DemoOnly0123456789");
    await form.getByRole("button", { name: "Store" }).click();
    await expect(form).toContainText("Token stored");
    await expect(field).toHaveValue("");
    await expect(page.locator("body")).not.toContainText("github_pat_DemoOnly0123456789");
  });

  test("a webhook is checked, stored masked and tested", async ({ page }) => {
    await page.goto(`${MAIN}&page=settings`);
    const form = page.getByRole("form", { name: "Webhook for cred:discord/nomad" });
    await expect(form).toContainText("Webhook stored: discord.com webhook 0000...0007");
    await form.getByLabel("Webhook for cred:discord/nomad").fill("https://example.org/hook");
    await form.getByRole("button", { name: "Store" }).click();
    await expect(form.getByRole("alert")).toContainText("not a Discord address");
    await form
      .getByLabel("Webhook for cred:discord/nomad")
      .fill("https://discord.com/api/webhooks/000000000000000123/demo-token");
    await form.getByRole("button", { name: "Store" }).click();
    await expect(form).toContainText("Webhook stored: discord.com webhook 0000...0123");
    await expect(page.locator("body")).not.toContainText("demo-token");
    await form.getByRole("button", { name: "Test webhook" }).click();
    await expect(form.getByRole("status")).toContainText("The webhook exists.");
  });

  test("a repository is linked to GitHub and the access is tested", async ({ page }) => {
    await page.goto(`${MAIN}&page=settings`);
    const form = page.getByRole("form", { name: "GitHub repository of team-cycles" });
    await form.getByRole("button", { name: "Test access" }).click();
    await expect(form.getByRole("status")).toContainText(
      "Connected to demo-org/nomad-cycles as demo-bot",
    );
    const bad = page.getByRole("form", { name: "GitHub repository of local" });
    await bad.getByLabel("GitHub repository of local").fill("not a repository");
    await bad.getByRole("button", { name: "Save link" }).click();
    await expect(bad.getByRole("alert")).toContainText("owner/name");
  });
});
