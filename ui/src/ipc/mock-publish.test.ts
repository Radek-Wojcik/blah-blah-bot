import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "./backend";
import { createMockBackend } from "./mock";
import type { MockHooks } from "./mock-publish";
import type { DraftView, OutboxState } from "./types";

const WEBHOOK = "https://discord.com/api/webhooks/000000000000000099/secret-token_1";
const TOKEN = "ghp_demoSecretToken0123456789";

/** The error code a promise rejects with. */
async function codeOf(p: Promise<unknown>): Promise<string | undefined> {
  try {
    await p;
  } catch (e) {
    return (e as { code?: string }).code;
  }
  return undefined;
}

const hooks = () => (globalThis as { __bbbMock?: MockHooks }).__bbbMock!;

/** A backend whose outbox runs fast; the network is on. */
const fresh = (): Backend => createMockBackend({ clockRate: 0, jobStepMs: 40 });

async function until<T>(read: () => Promise<T>, done: (v: T) => boolean, ms = 4000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await read();
    if (done(v) || Date.now() > end) return v;
    await new Promise((r) => setTimeout(r, 15));
  }
}

const settled = (b: Backend, meetingId = "m5") =>
  until(
    () => b.outboxState(),
    (s) => s.ops.filter((o) => o.meetingId === meetingId).every((o) => o.state === "done"),
  );

async function approveAll(b: Backend): Promise<DraftView> {
  let view = await b.getDraft("m5");
  for (const p of view.draft.statusProposals ?? [])
    view = await b.setStatusDecision("m5", p.id, "approved");
  return view;
}

async function publishAll(b: Backend) {
  await b.confirmPublicRepo("NOMAD");
  return b.publishMeeting("m5", ["github", "discord"]);
}

afterEach(() => {
  delete (globalThis as { __bbbMock?: MockHooks }).__bbbMock;
});

describe("status proposals", () => {
  it("shows three proposals of the demo draft, none decided", async () => {
    const view = await fresh().getDraft("m5");
    expect(view.locked).toBe(false);
    expect(view.draft.statusProposals?.map((p) => [p.id, p.target, p.decision])).toEqual([
      ["s1", "#12", "proposed"],
      ["s2", "#11", "proposed"],
      ["s3", "t-1", "proposed"],
    ]);
  });

  it("approves, rejects and keeps the comment rules of the backend", async () => {
    const b = fresh();
    let v = await b.setStatusDecision("m5", "s1", "approved", "  Closed after the review  ");
    let p = v.draft.statusProposals!.find((x) => x.id === "s1")!;
    expect([p.decision, p.comment, p.edited]).toEqual([
      "approved",
      "Closed after the review",
      true,
    ]);
    // A decision without a comment keeps it; an empty comment clears it.
    v = await b.setStatusDecision("m5", "s1", "rejected");
    p = v.draft.statusProposals!.find((x) => x.id === "s1")!;
    expect([p.decision, p.comment]).toEqual(["rejected", "Closed after the review"]);
    v = await b.setStatusDecision("m5", "s1", "proposed", "");
    p = v.draft.statusProposals!.find((x) => x.id === "s1")!;
    expect([p.decision, p.comment]).toEqual(["proposed", null]);
    expect(await codeOf(b.setStatusDecision("m5", "s9", "approved"))).toBe("not_found");
  });

  it("adds a proposal by hand for an issue or a topic, once per target", async () => {
    const b = fresh();
    const v = await b.addStatusProposal("m5", "#14", "open", "Needs more work");
    const added = v.draft.statusProposals!.at(-1)!;
    expect([added.id, added.target, added.status, added.source, added.decision]).toEqual([
      "s4",
      "#14",
      "open",
      null,
      "proposed",
    ]);
    expect(await codeOf(b.addStatusProposal("m5", "#14", "done"))).toBe("invalid");
    expect(await codeOf(b.addStatusProposal("m5", "12", "done"))).toBe("invalid");
    expect(await codeOf(b.addStatusProposal("m5", "no-such-topic", "done"))).toBe("invalid");
    await b.addStatusProposal("m5", "t-2", "open");
  });
});

describe("secrets", () => {
  it("lists hosts and references with flags only, never a value", async () => {
    const b = fresh();
    const s = await b.publishingSecrets();
    expect(s.github).toEqual([
      { host: "api.github.com", api: "https://api.github.com", tokenSet: true },
    ]);
    expect(s.targets).toEqual([
      {
        reference: "cred:discord/nomad",
        cycleId: "NOMAD",
        kind: "discord",
        set: true,
        masked: "discord.com webhook 0000...0007",
      },
    ]);
    await b.clearGithubToken();
    await b.setGithubToken(TOKEN);
    await b.setTargetSecret("cred:discord/nomad", WEBHOOK);
    const after = JSON.stringify(await b.publishingSecrets());
    expect(after).not.toContain(TOKEN);
    expect(after).not.toContain("secret-token_1");
    expect(after).toContain("0000...0099");
  });

  it("refuses a bad token or webhook without repeating it", async () => {
    const b = fresh();
    expect(await codeOf(b.setGithubToken("   "))).toBe("invalid");
    expect(await codeOf(b.setGithubToken("two words"))).toBe("invalid");
    let message = "";
    try {
      await b.setTargetSecret(
        "cred:discord/nomad",
        "https://evil.example/api/webhooks/1/zzz-secret",
      );
    } catch (e) {
      message = (e as { message: string }).message;
    }
    expect(message).toMatch(/not a Discord address/);
    expect(message).not.toContain("zzz-secret");
    expect(await codeOf(b.setTargetSecret("no-prefix", WEBHOOK))).toBe("invalid");
    expect(await codeOf(b.setTargetSecret("cred:discord/nomad", "nonsense"))).toBe(
      "invalid_webhook",
    );
  });

  it("clears a token and a webhook", async () => {
    const b = fresh();
    await b.clearGithubToken();
    await b.clearTargetSecret("cred:discord/nomad");
    const s = await b.publishingSecrets();
    expect(s.github[0].tokenSet).toBe(false);
    expect(s.targets[0]).toMatchObject({ set: false, masked: null });
  });

  it("tests the access with a message, not an error", async () => {
    const b = fresh();
    expect(await b.testGithubAccess("NOMAD")).toMatchObject({
      ok: true,
      repo: "demo-org/nomad-cycles",
      login: "demo-bot",
      visibility: "public",
    });
    await b.clearGithubToken();
    expect(await b.testGithubAccess("NOMAD")).toMatchObject({ ok: false, tokenSet: false });
    // A cycle in a repository that is not linked to GitHub.
    expect((await b.testGithubAccess("SANDBOX")).message).toMatch(/not linked/);
    expect(await b.testDiscordWebhook("cred:discord/nomad")).toMatchObject({ ok: true });
    await b.clearTargetSecret("cred:discord/nomad");
    expect(await b.testDiscordWebhook("cred:discord/nomad")).toMatchObject({ ok: false });
  });

  it("links a repository to GitHub", async () => {
    const b = fresh();
    const repo = await b.setGithubRepo("local", "demo-org/sandbox");
    expect(repo.githubRepo).toBe("demo-org/sandbox");
    expect(await codeOf(b.setGithubRepo("local", "not a repo"))).toBe("invalid");
    expect((await b.setGithubRepo("local", "")).githubRepo).toBeNull();
    const gh = await b.setGithubRepo(
      "team-cycles",
      "demo-org/nomad-cycles",
      "https://api.ghe.example.org",
    );
    expect(gh.githubApi).toBe("https://api.ghe.example.org");
    expect((await b.publishingSecrets()).github.map((h) => h.host)).toEqual([
      "api.github.com",
      "api.ghe.example.org",
    ]);
  });
});

describe("plan and the public repository gate", () => {
  it("plans the commit, the issues, the approved statuses and the topics", async () => {
    const b = fresh();
    await approveAll(b);
    const plan = await b.publishPlan("m5");
    expect(plan.targets).toEqual([
      { target: "github", mode: "on-approval", available: true, configured: true },
      { target: "discord", mode: "on-approval", available: true, configured: true },
      { target: "notion", mode: null, available: false, configured: false },
    ]);
    expect(plan.summary.commit && plan.summary.pushes).toBe(true);
    expect(plan.summary.files).toContain("transcript.md");
    expect(plan.summary.issues.map((i) => i.itemId)).toEqual(["i03", "i04", "i09", "i11"]);
    expect(plan.summary.issues[0]).toMatchObject({ assignee: "jan-demo", due: "2026-11-19" });
    expect(plan.summary.statuses.map((s) => s.target)).toEqual(["#12", "#11"]);
    expect(plan.summary.topicsAdded).toEqual([
      "Confirm the delivery date of the antenna array; the mount is already on site.",
    ]);
    expect(plan.summary.topicsSet).toEqual([{ id: "t-1", open: false }]);
    expect(plan.summary.proposalsPending).toBe(0);
    expect(plan.summary.discord).toBe(true);
  });

  it("writes back only approved changes and counts the others", async () => {
    const b = fresh();
    await b.setStatusDecision("m5", "s1", "approved");
    await b.setStatusDecision("m5", "s2", "rejected");
    const plan = await b.publishPlan("m5");
    expect(plan.summary.statuses.map((s) => s.proposalId)).toEqual(["s1"]);
    expect(plan.summary.topicsSet).toEqual([]);
    expect(plan.summary.proposalsPending).toBe(1);
  });

  it("refuses a public repository until the user confirms it", async () => {
    const b = fresh();
    const plan = await b.publishPlan("m5");
    expect(plan.blocks.map((x) => x.code)).toEqual(["public_repo_unconfirmed"]);
    expect(plan.repo).toMatchObject({ visibility: "public", confirmed: false });
    expect(await codeOf(b.publishMeeting("m5", ["github"]))).toBe("public_repo_unconfirmed");
    expect((await b.outboxState()).ops).toEqual([]);
    expect(await b.confirmPublicRepo("NOMAD")).toMatchObject({
      key: "github.com/demo-org/nomad-cycles",
      visibility: "public",
    });
    expect((await b.publishPlan("m5")).blocks).toEqual([]);
    expect((await b.publishMeeting("m5", ["github"])).queued).toBeGreaterThan(0);
  });

  it("does not ask for a private repository, and asks when the visibility is unknown", async () => {
    const b = fresh();
    hooks().setVisibility("private");
    expect((await b.publishPlan("m5")).blocks).toEqual([]);
    hooks().setVisibility("public");
    hooks().setOffline(true);
    const offline = await b.publishPlan("m5");
    expect(offline.repo.visibility).toBe("unknown");
    expect(offline.blocks[0]).toMatchObject({
      code: "public_repo_unconfirmed",
      visibility: "unknown",
    });
  });

  it("leaves Discord out without a stored webhook and blocks a target the rules turn off", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    await b.clearTargetSecret("cred:discord/nomad");
    expect((await b.publishPlan("m5", ["discord"])).blocks.map((x) => x.code)).toEqual([
      "nothing_to_publish",
    ]);
    expect(await codeOf(b.publishMeeting("m5", ["discord"]))).toBe("nothing_to_publish");
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.github = { path: "meetings", publish: "off" };
    cycle.targets.discord = { webhook: "cred:discord/nomad", publish: "off" };
    expect((await b.saveCycle(cycle)).saved).toBe(true);
    const off = await b.publishPlan("m5");
    expect(off.targets.map((t) => t.available)).toEqual([false, false, false]);
    expect(off.blocks.map((x) => x.code)).toEqual(["nothing_to_publish"]);
    expect(await codeOf(b.publishMeeting("m5", ["github"]))).toBe("nothing_to_publish");
  });

  it("does not create issues when the cycle turns them off", async () => {
    const b = fresh();
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.github = { path: "meetings", publish: "on-approval", issues: false };
    await b.saveCycle(cycle);
    const plan = await b.publishPlan("m5", ["github"]);
    expect(plan.summary.issues).toEqual([]);
    expect(plan.summary.commit).toBe(true);
  });

  it("refuses a newer draft that waits for a decision", async () => {
    const b = createMockBackend({ clockRate: 0, jobStepMs: 40, newDraft: true });
    await b.confirmPublicRepo("NOMAD");
    expect(await codeOf(b.publishMeeting("m5", ["github"]))).toBe("new_draft_pending");
    expect((await b.publishPlan("m5")).newDraftPending).toBe(true);
  });
});

describe("the full loop", () => {
  it("publishes: commit, issues, status write-back, Discord, in order", async () => {
    const b = fresh();
    await approveAll(b);
    const seen: OutboxState[] = [];
    const off = await b.onOutboxChanged((s) => seen.push(s));
    const { queued } = await publishAll(b);
    // The commit, four issues, two issue statuses and the Discord message.
    expect(queued).toBe(8);
    const state = await settled(b);
    off();
    const ops = state.ops;
    expect(ops.map((o) => o.kind)).toEqual([
      "commit_meeting",
      "create_issue",
      "create_issue",
      "create_issue",
      "create_issue",
      "set_issue_state",
      "set_issue_state",
      "post_discord",
    ]);
    expect(ops.every((o) => o.state === "done")).toBe(true);
    expect(seen.length).toBeGreaterThan(8);
    const commit = ops[0].result!;
    expect(commit).toMatchObject({
      folder: "meetings/NOMAD/2026-11-13",
      pushed: true,
      branch: "main",
    });
    expect(commit.report_url).toBe(
      "https://github.com/demo-org/nomad-cycles/blob/main/meetings/NOMAD/2026-11-13/report.md",
    );
    const issues = hooks().issues();
    expect(issues.filter((i) => i.meeting === "m5").map((i) => i.number)).toEqual([13, 14, 15, 16]);
    expect(issues.find((i) => i.number === 12)?.state).toBe("closed");
    expect(issues.find((i) => i.number === 11)?.state).toBe("closed");
    const posts = hooks().discordPosts();
    expect(posts).toHaveLength(1);
    // The message goes out after the issues exist, and links the report and the issues.
    expect(posts[0].content).toContain("Full report: https://github.com/demo-org");
    expect(posts[0].content).toContain("Issues: #13, #14, #15, #16");
  });

  it("locks the draft once external operations are sent", async () => {
    const b = fresh();
    await approveAll(b);
    await publishAll(b);
    await settled(b);
    const view = await b.getDraft("m5");
    expect(view.locked).toBe(true);
    for (const attempt of [
      b.updateDraftItem("m5", "i01", { text: "changed" }),
      b.deleteDraftItem("m5", "i01"),
      b.addDraftItem("m5", { type: "note", text: "late" }),
      b.setStatusDecision("m5", "s1", "rejected"),
      b.addStatusProposal("m5", "#20", "done"),
      b.setReportMode("m5", "fixed"),
      b.discardNewDraft("m5"),
    ])
      expect(await codeOf(attempt)).toBe("published");
    expect((await b.publishPlan("m5")).locked).toBe(true);
  });

  it("publishing again changes nothing: the operations are idempotent", async () => {
    const b = fresh();
    await publishAll(b);
    await settled(b);
    const again = await b.publishMeeting("m5", ["github", "discord"]);
    expect(again.queued).toBe(0);
    expect(
      hooks()
        .issues()
        .filter((i) => i.meeting === "m5"),
    ).toHaveLength(4);
  });

  it("reports the topics.yaml conflict: last write wins per topic, both versions visible", async () => {
    const b = fresh();
    await approveAll(b);
    await publishAll(b);
    const state = await settled(b);
    const topics = state.ops[0].result!.topics!;
    expect(topics).toMatchObject({ added: 1, changed: 1, removed: 0, conflicts: ["t-1"] });
    expect(topics.conflict_details).toEqual([
      {
        id: "t-1",
        theirs: { text: "Decide the rover wheel size and the tyre type", status: "open" },
        ours: { text: "Decide the rover wheel size", status: "closed" },
      },
    ]);
    // Ours won in the local clone; what only the colleague added is kept too.
    const local = (await b.getCycle("NOMAD")).topics;
    expect(local.find((t) => t.id === "t-1")).toMatchObject({
      status: "closed",
      text: "Decide the rover wheel size",
      closedIn: "2026-11-13",
    });
    expect(local.map((t) => t.id)).toContain("t-3");
    expect(local.map((t) => t.id)).toContain("m5-i07");
    expect(
      hooks()
        .remoteTopics()
        .find((t) => t.id === "t-1")?.status,
    ).toBe("closed");
  });
});

describe("a closed issue is done in the next context", () => {
  it("shows in the tasks after a pull, and leaves the next context", async () => {
    const b = fresh();
    await approveAll(b);
    await publishAll(b);
    await settled(b);
    // Until the issues are read again, the clone knows only what it knew.
    let tasks = await b.cycleTasks("NOMAD");
    expect(tasks.repo).toBe("demo-org/nomad-cycles");
    expect(tasks.asOf).toBe("2026-11-12");
    expect(tasks.open.map((t) => t.number)).toEqual([10, 11, 12]);
    expect(tasks.done.map((t) => t.number)).toEqual([9]);
    expect(
      hooks()
        .nextContext("NOMAD")
        .map((i) => i.issue),
    ).toEqual(["#10", "#11", "#12"]);

    // A colleague closes the issue made from the task of Alex.
    hooks().closeIssue(15);
    await b.pullCycleRepo("team-cycles");
    tasks = await b.cycleTasks("NOMAD");
    expect(tasks.asOf).toBe("2026-11-13");
    expect(tasks.done.map((t) => t.number).sort((a, b) => a - b)).toEqual([9, 11, 12, 15]);
    expect(tasks.open.map((t) => t.number)).toEqual([10, 13, 14, 16]);
    expect(tasks.open[1].url).toBe("https://github.com/demo-org/nomad-cycles/issues/13");
    const context = hooks().nextContext("NOMAD");
    expect(context.map((i) => i.issue)).toEqual(["#10", "#13", "#14", "#16"]);
    expect(context.map((i) => i.issue)).not.toContain("#15");
  });

  it("does not read the issues while the network is off", async () => {
    const b = fresh();
    hooks().closeIssue(11);
    hooks().setOffline(true);
    await b.pullCycleRepo("team-cycles");
    expect((await b.cycleTasks("NOMAD")).asOf).toBe("2026-11-12");
    hooks().setOffline(false);
    await b.pullCycleRepo("team-cycles");
    expect((await b.cycleTasks("NOMAD")).done.map((t) => t.number)).toEqual([9, 11]);
  });
});

describe("offline, retry, cancel", () => {
  it("queues while offline and finishes by itself when the network is back", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    hooks().setOffline(true);
    await b.publishMeeting("m5", ["github", "discord"]);
    const waiting = await until(
      () => b.outboxState(),
      (s) => s.offline.length > 0,
    );
    expect(waiting.offline).toContain("github");
    const commit = waiting.ops[0];
    expect(commit).toMatchObject({ state: "queued", attempts: 1 });
    expect(commit.lastError).toMatch(/network is offline/);
    expect(commit.nextAttemptUnixMs! - waiting.nowUnixMs).toBeGreaterThan(20_000);
    expect(
      hooks()
        .issues()
        .filter((i) => i.meeting === "m5"),
    ).toEqual([]);
    expect(hooks().discordPosts()).toEqual([]);

    hooks().setOffline(false);
    const done = await settled(b);
    expect(done.ops.every((o) => o.state === "done")).toBe(true);
    expect(done.offline).toEqual([]);
    expect(hooks().discordPosts()).toHaveLength(1);
  });

  it("retries a waiting operation now", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    hooks().setOffline(true);
    await b.publishMeeting("m5", ["github"]);
    await until(
      () => b.outboxState(),
      (s) => s.offline.length > 0,
    );
    // Still offline: the retry fails again, with one more attempt.
    await b.retryOutboxOp();
    const again = await until(
      () => b.outboxState(),
      (s) => s.ops[0].attempts >= 2,
    );
    expect(again.ops[0].attempts).toBe(2);
    expect(await codeOf(b.retryOutboxOp(999))).toBe("invalid");
  });

  it("fails an issue for good without a token, and retries after storing one", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    await b.clearGithubToken();
    await b.publishMeeting("m5", ["github"]);
    const state = await until(
      () => b.outboxState(),
      (s) => s.ops.filter((o) => o.state === "failed").length === 4,
    );
    const failed = state.ops.filter((o) => o.state === "failed");
    expect(failed).toHaveLength(4);
    expect(failed[0].lastError).toMatch(/No GitHub token is stored for api\.github\.com/);
    expect(state.ops[0].state).toBe("done");
    await b.setGithubToken(TOKEN);
    await b.retryOutboxOp();
    const done = await settled(b);
    expect(done.ops.every((o) => o.state === "done")).toBe(true);
  });

  it("cancels waiting operations without locking the draft while queued", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    hooks().setOffline(true);
    await b.publishMeeting("m5", ["github"]);
    await until(
      () => b.outboxState(),
      (s) => s.offline.length > 0,
    );
    expect((await b.getDraft("m5")).locked).toBe(false);
    const ids = (await b.outboxState()).ops.map((o) => o.id);
    for (const id of ids) await b.cancelOutboxOp(id);
    expect((await b.getDraft("m5")).locked).toBe(false);
    await b.updateDraftItem("m5", "i01", { text: "An edit after cancelling" });
    expect(await codeOf(b.cancelOutboxOp(ids[0]))).toBe("invalid");
    expect(await codeOf(b.cancelOutboxOp(12345))).toBe("not_found");
  });

  it("publishes again after everything was cancelled, from a new snapshot", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    hooks().setOffline(true);
    await b.publishMeeting("m5", ["github"]);
    await until(
      () => b.outboxState(),
      (s) => s.offline.length > 0,
    );
    for (const o of (await b.outboxState()).ops) await b.cancelOutboxOp(o.id);
    await b.updateDraftItem("m5", "i01", { text: "Changed before publishing again" });
    hooks().setOffline(false);
    const again = await b.publishMeeting("m5", ["github"]);
    expect(again.queued).toBeGreaterThan(0);
    const done = await settled(b);
    expect(done.ops.filter((o) => o.state === "done").length).toBeGreaterThan(0);
  });
});

describe("the plan of topics.yaml", () => {
  it("lists each topic with its version on the remote and the one the commit writes", async () => {
    const b = fresh();
    await approveAll(b);
    const topics = (await b.publishPlan("m5")).summary.topics!;
    expect(topics.againstRemote).toBe(true);
    expect(topics.unchanged).toBe(2);
    expect(topics.changes.map((c) => [c.id, c.change, c.overridesRemote])).toEqual([
      ["m5-i07", "added", false],
      ["t-1", "changed", true],
    ]);
    const closed = topics.changes[1];
    // Before is what the colleague left on the remote, after is this meeting's version.
    expect(closed.before).toMatchObject({
      text: "Decide the rover wheel size and the tyre type",
      status: "open",
    });
    expect(closed.after).toMatchObject({
      text: "Decide the rover wheel size",
      status: "closed",
      closedIn: "2026-11-13",
    });
  });

  it("is not made when the meeting changes no topic, and looking changes nothing", async () => {
    const b = fresh();
    const plan = await b.publishPlan("m5");
    // Not approved: only the new topic is planned, and the closing of t-1 is not.
    expect(plan.summary.topics?.changes.map((c) => c.id)).toEqual(["m5-i07"]);
    expect(
      hooks()
        .remoteTopics()
        .map((t) => t.id),
    ).toEqual(["t-1", "t-2", "t-3"]);
    await b.publishPlan("m5");
    expect(
      hooks()
        .remoteTopics()
        .map((t) => t.id),
    ).toEqual(["t-1", "t-2", "t-3"]);
  });

  it("falls back to the topic names, with a note, when the comparison is not possible", async () => {
    const b = fresh();
    await approveAll(b);
    hooks().setTopicsUnknown(true);
    const plan = await b.publishPlan("m5");
    expect(plan.summary.topics ?? null).toBeNull();
    expect(plan.summary.topicsAdded).toHaveLength(1);
    expect(plan.summary.topicsSet).toEqual([{ id: "t-1", open: false }]);
    expect(plan.notes.join(" ")).toContain("listed by name only");
  });

  it("shows nothing to compare after the commit, because the clone has the merged file", async () => {
    const b = fresh();
    await approveAll(b);
    await publishAll(b);
    await settled(b);
    const plan = await b.publishPlan("m5");
    expect(plan.summary.topics?.changes ?? []).toEqual([]);
  });
});

describe("the publishing rule of a target", () => {
  it("sets only the rule and keeps the rest of the target", async () => {
    const b = fresh();
    let changes = 0;
    await b.onCyclesChanged(() => void (changes += 1));
    const targets = await b.setTargetRule("NOMAD", "discord", "auto");
    expect(targets.discord).toMatchObject({ webhook: "cred:discord/nomad", publish: "auto" });
    expect(targets.github).toMatchObject({ publish: "on-approval", path: "meetings" });
    expect(changes).toBe(1);
    const saved = (await b.getCycle("NOMAD")).cycle;
    expect(saved.targets.discord?.publish).toBe("auto");
    expect(saved.targets.discord?.webhook).toBe("cred:discord/nomad");
    const plan = await b.publishPlan("m5");
    expect(plan.targets.map((t) => [t.target, t.mode, t.available])).toEqual([
      ["github", "on-approval", true],
      ["discord", "auto", true],
      ["notion", null, false],
    ]);
  });

  it("creates the GitHub target with its defaults for a cycle that has none", async () => {
    const b = fresh();
    await b.createCycle("PLAIN", "Plain");
    expect((await b.getCycle("PLAIN")).cycle.targets.github ?? null).toBeNull();
    const targets = await b.setTargetRule("PLAIN", "github", "auto");
    expect(targets.github).toEqual({ path: "meetings", publish: "auto" });
    // Discord and Notion need a target that only the cycle editor adds.
    expect(await codeOf(b.setTargetRule("PLAIN", "discord", "auto"))).toBe("no_target");
    expect(await codeOf(b.setTargetRule("PLAIN", "notion", "auto"))).toBe("no_target");
    expect(await codeOf(b.setTargetRule("PLAIN", "slack" as never, "auto"))).toBe("invalid");
    expect(await codeOf(b.setTargetRule("PLAIN", "github", "sometimes" as never))).toBe("invalid");
    expect(await codeOf(b.setTargetRule("GHOST", "github", "auto"))).toBe("not_found");
  });

  it("never lets the public-repository check go, whatever the rule", async () => {
    const b = fresh();
    for (const mode of ["on-approval", "auto"] as const) {
      await b.setTargetRule("NOMAD", "github", mode);
      const plan = await b.publishPlan("m5", ["github"]);
      expect(
        plan.blocks.map((x) => x.code),
        mode,
      ).toEqual(["public_repo_unconfirmed"]);
      expect(await codeOf(b.publishMeeting("m5", ["github"])), mode).toBe(
        "public_repo_unconfirmed",
      );
    }
    expect((await b.outboxState()).ops).toEqual([]);
  });

  it("makes Off absolute, also for an explicit publication", async () => {
    const b = fresh();
    await b.setTargetRule("NOMAD", "github", "off");
    await b.setTargetRule("NOMAD", "discord", "off");
    const plan = await b.publishPlan("m5");
    expect(plan.targets.every((t) => (t.mode === "off" || t.mode === null) && !t.available)).toBe(
      true,
    );
    expect(await codeOf(b.publishMeeting("m5", ["github", "discord"]))).toBe("nothing_to_publish");
  });

  it("does not write a status change back by itself under Automatic", async () => {
    const b = fresh();
    await b.setTargetRule("NOMAD", "github", "auto");
    const view = await b.getDraft("m5");
    // Nothing is approved by the application: every proposal still waits for the user.
    expect((view.draft.statusProposals ?? []).every((p) => p.decision === "proposed")).toBe(true);
    const plan = await b.publishPlan("m5");
    expect(plan.summary.statuses).toEqual([]);
    expect(plan.summary.proposalsPending).toBe(3);
  });
});

describe("the folder for meeting reports", () => {
  it("is where the meeting goes, and an unsafe path is refused by the cycle", async () => {
    const b = fresh();
    const cycle = (await b.getCycle("NOMAD")).cycle;
    const bad = await b.saveCycle({
      ...cycle,
      targets: { ...cycle.targets, github: { ...cycle.targets.github!, path: "../outside" } },
    });
    expect(bad.saved).toBe(false);
    expect(bad.issues.map((i) => i.path)).toEqual(["targets.github.path"]);

    const ok = await b.saveCycle({
      ...cycle,
      targets: { ...cycle.targets, github: { ...cycle.targets.github!, path: "docs/notes" } },
    });
    expect(ok.saved).toBe(true);
    await approveAll(b);
    await publishAll(b);
    const state = await settled(b);
    const commit = state.ops.find((o) => o.kind === "commit_meeting")!;
    expect(commit.result?.folder).toBe("docs/notes/NOMAD/2026-11-13");
    expect(commit.result?.report_url).toBe(
      "https://github.com/demo-org/nomad-cycles/blob/main/docs/notes/NOMAD/2026-11-13/report.md",
    );
  });
});

describe("corrections to a published meeting", () => {
  /** The demo meeting published to GitHub (its issues made), ready for corrections. */
  async function published(): Promise<Backend> {
    const b = fresh();
    await publishAll(b);
    await settled(b);
    return b;
  }
  const states = (b: Backend) =>
    b.listCorrections("m5").then((v) => v.corrections.map((c) => c.state));

  it("needs a published meeting and a sensible note, and writes nothing otherwise", async () => {
    const b = fresh();
    expect(await codeOf(b.addCorrection("m5", "Moved."))).toBe("not_published");
    await publishAll(b);
    await settled(b);
    expect(await codeOf(b.addCorrection("m5", "   "))).toBe("invalid");
    expect(await codeOf(b.addCorrection("m5", "x", "i99"))).toBe("not_found");
    expect(await codeOf(b.addCorrection("m5", "x", undefined, "twelve"))).toBe("invalid");
    expect(await codeOf(b.addCorrection("m5", "x", undefined, undefined, "16.11.2026"))).toBe(
      "invalid",
    );
    expect((await b.listCorrections("m5")).corrections).toEqual([]);
    const view = await b.addCorrection("m5", "  Moved.  ", undefined, "12", "2026-11-15");
    expect(view.corrections[0]).toMatchObject({
      id: "c1",
      text: "Moved.",
      issue: "#12",
      date: "2026-11-15",
      state: "local",
      canDelete: true,
    });
    expect(view.locked && view.inRepository).toBe(true);
    expect(view.rule).toBe("on-approval");
  });

  it("waits for the user, then adds a file next to the meeting and comments on the issue", async () => {
    const b = await published();
    const draft = await b.getDraft("m5");
    const task = draft.draft.items.find((i) => i.type === "task")!;
    await b.addCorrection("m5", "The part arrived early.", task.id);
    expect(await states(b)).toEqual(["local"]);
    expect(hooks().correctionFiles()).toEqual([]);

    const result = await b.publishCorrections("m5");
    expect(result.queued).toBe(2);
    await settled(b);
    const view = await b.listCorrections("m5");
    expect(view.corrections[0]).toMatchObject({
      state: "published",
      fileName: "corrections.md",
      canDelete: false,
    });
    expect(view.corrections[0].url).toMatch(/meetings\/NOMAD\/.*corrections\.md$/);
    const files = hooks().correctionFiles();
    expect(files).toHaveLength(1);
    expect(files[0].text).toBe("The part arrived early.");
    const made = hooks()
      .issues()
      .find((i) => i.meeting === "m5" && i.item === task.id)!;
    expect(hooks().issueComments(made.number)).toHaveLength(1);
    expect(made.state).toBe("open");

    // A second one is a second file; a note about no item gets no comment.
    await b.addCorrection("m5", "Another.");
    await b.publishCorrections("m5");
    await settled(b);
    expect(
      hooks()
        .correctionFiles()
        .map((f) => f.path.split("/").pop()),
    ).toEqual(["corrections.md", "corrections-2.md"]);
    expect(hooks().issueComments(made.number)).toHaveLength(1);
    // Asked again, nothing is queued twice, and a queued correction stays.
    expect((await b.publishCorrections("m5", ["c1"])).queued).toBe(0);
    expect(await codeOf(b.deleteCorrection("m5", "c1"))).toBe("correction_queued");
    expect(await codeOf(b.publishCorrections("m5"))).toBe("nothing_to_publish");
  });

  it("deletes only a correction that was never queued", async () => {
    const b = await published();
    await b.addCorrection("m5", "Typo.");
    expect((await b.deleteCorrection("m5", "c1")).corrections).toEqual([]);
    expect(await codeOf(b.deleteCorrection("m5", "c1"))).toBe("not_found");
  });

  it("follows the rule of the github target: Automatic queues at once, Off never", async () => {
    const b = await published();
    await b.setTargetRule("NOMAD", "github", "auto");
    const view = await b.addCorrection("m5", "Automatic.");
    expect(view.rule).toBe("auto");
    expect(view.corrections[0].state).not.toBe("local");
    await settled(b);
    expect(hooks().correctionFiles()).toHaveLength(1);

    await b.setTargetRule("NOMAD", "github", "off");
    const off = await b.addCorrection("m5", "Off.");
    expect(off.corrections[1].state).toBe("local");
    expect(await codeOf(b.publishCorrections("m5"))).toBe("nothing_to_publish");
    expect(hooks().correctionFiles()).toHaveLength(1);
  });

  it("keeps the public repository check for a repository nobody confirmed", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    await b.publishMeeting("m5", ["github"]);
    await settled(b);
    await b.addCorrection("m5", "Moved.");
    // Confirmed once for this repository: the correction goes out.
    expect((await b.publishCorrections("m5")).queued).toBeGreaterThan(0);

    // A meeting whose repository was never confirmed (private then, public now) is asked first.
    const c = fresh();
    hooks().setVisibility("private");
    await c.publishMeeting("m5", ["github"]);
    await settled(c);
    hooks().setVisibility("public");
    await c.addCorrection("m5", "Moved.");
    expect(await codeOf(c.publishCorrections("m5"))).toBe("public_repo_unconfirmed");
    expect(hooks().correctionFiles()).toEqual([]);
    await c.confirmPublicRepo("NOMAD");
    expect((await c.publishCorrections("m5")).queued).toBeGreaterThan(0);
    // Automatic does not get around it either.
    const d = fresh();
    hooks().setVisibility("private");
    await d.publishMeeting("m5", ["github"]);
    await settled(d);
    hooks().setVisibility("public");
    await d.setTargetRule("NOMAD", "github", "auto");
    const view = await d.addCorrection("m5", "Auto.");
    expect(view.corrections[0].state).toBe("local");
  });

  it("refuses a correction for a meeting that is not in the repository", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    await b.publishMeeting("m5", ["discord"]);
    await settled(b);
    const view = await b.addCorrection("m5", "Moved.");
    expect(view.inRepository).toBe(false);
    expect(await codeOf(b.publishCorrections("m5"))).toBe("not_in_repository");
  });

  it("waits for the issue of its item and survives the network going away", async () => {
    const b = fresh();
    await publishAll(b);
    await until(
      () => b.outboxState(),
      (s) => s.ops.some((o) => o.state === "done" || o.state === "running"),
    );
    const task = (await b.getDraft("m5")).draft.items.find((i) => i.type === "task")!;
    await b.addCorrection("m5", "Moved.", task.id);
    // Queued before the meeting finished: the comment still finds the issue that gets made.
    await b.publishCorrections("m5");
    await settled(b);
    const made = hooks()
      .issues()
      .find((i) => i.meeting === "m5" && i.item === task.id)!;
    expect(hooks().issueComments(made.number)).toHaveLength(1);

    hooks().setOffline(true);
    await b.addCorrection("m5", "Later.");
    await b.publishCorrections("m5", ["c2"]);
    const waiting = await until(
      () => b.listCorrections("m5"),
      (v) => v.corrections[1].file?.lastError != null,
    );
    expect(waiting.corrections[1].state).toBe("queued");
    expect(hooks().correctionFiles()).toHaveLength(1);
    hooks().setOffline(false);
    const back = await until(
      () => b.listCorrections("m5"),
      (v) => v.corrections[1].state === "published",
    );
    expect(back.corrections[1].fileName).toBe("corrections-2.md");
    expect(hooks().correctionFiles()).toHaveLength(2);
  });

  it("hands the corrections to the next processing, with the text of the item", async () => {
    const b = await published();
    const task = (await b.getDraft("m5")).draft.items.find((i) => i.type === "task")!;
    await b.addCorrection("m5", "Moved.", task.id, undefined, "2026-11-15");
    expect(hooks().nextCorrections("NOMAD")).toEqual([
      { text: "Moved.", item: task.text, issue: null, date: "2026-11-15" },
    ]);
  });

  it("keeps the corrections out of a locked draft", async () => {
    const b = await published();
    const before = await b.getDraft("m5");
    await b.addCorrection("m5", "Moved.");
    const after = await b.getDraft("m5");
    expect(after.draft).toEqual(before.draft);
    expect(after.locked).toBe(true);
    expect(await codeOf(b.updateDraftItem("m5", before.draft.items[0].id, { text: "x" }))).toBe(
      "published",
    );
  });
});

describe("Stage 7: Notion and Discord bot integrations", () => {
  const NOTION_TOKEN = ["ntn", "demoSecretIntegrationToken0123456789"].join("_");
  const BOT_TOKEN = [
    "MTIzNDU2Nzg5MDEyMzQ1Njc4",
    "Gdemo1",
    "DemoBotSecretSignature0123456789abcdef",
  ].join(".");

  it("validates, stores, masks and tests Notion tokens, schema and mapping", async () => {
    const b = fresh();
    expect(await codeOf(b.testNotionAccess("NOMAD"))).toBe("no_target");
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.notion = {
      token: "cred:notion/nomad",
      database: "0123456789abcdef0123456789abcdef",
      publish: "on-approval",
      properties: { title: "Name", key: "BBB key" },
    };
    expect((await b.saveCycle(cycle)).saved).toBe(true);

    expect(await codeOf(b.setTargetSecret("cred:notion/nomad", "bad-token"))).toBe(
      "invalid_notion_token",
    );
    await b.setTargetSecret("cred:notion/nomad", NOTION_TOKEN);
    const secrets = await b.publishingSecrets();
    const notionSecret = secrets.targets.find((t) => t.kind === "notion")!;
    expect(notionSecret).toMatchObject({
      reference: "cred:notion/nomad",
      set: true,
      masked: "ntn_...6789",
    });

    const access = await b.testNotionAccess("NOMAD");
    expect(access).toMatchObject({
      ok: true,
      botName: "Nomad notes",
      databaseTitle: "Tasks",
      tokenSet: true,
    });
    const schema = await b.notionSchema("NOMAD");
    expect(schema.proposed).toMatchObject({
      title: "Name",
      key: "BBB key",
      assignee: "Assignee",
      due: "Due",
      status: "Status",
      statusValue: "Not started",
      meeting: "Meeting",
    });
    const savedTargets = await b.setNotionMapping("NOMAD", null, schema.proposed);
    expect(savedTargets.notion?.properties).toEqual(schema.proposed);
  });

  it("exports tasks to Notion idempotently and re-exports while keeping status and skipping trashed pages", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.notion = {
      token: "cred:notion/nomad",
      database: "0123456789abcdef0123456789abcdef",
      publish: "on-approval",
      properties: {
        title: "Name",
        key: "BBB key",
        assignee: "Assignee",
        due: "Due",
        status: "Status",
        statusValue: "Not started",
        meeting: "Meeting",
      },
    };
    await b.saveCycle(cycle);
    await b.setTargetSecret("cred:notion/nomad", NOTION_TOKEN);

    const first = await b.publishMeeting("m5", ["github", "notion"]);
    expect(first.queued).toBe(9);
    await settled(b);

    const pages = hooks().notionPages();
    expect(pages).toHaveLength(4);
    expect(pages[0]).toMatchObject({
      key: "bbb:v1:NOMAD:m5:i03",
      status: "Not started",
      trashed: false,
    });

    // Second publish queues nothing and creates no duplicate pages.
    const again = await b.publishMeeting("m5", ["notion"]);
    expect(again.queued).toBe(0);
    expect(hooks().notionPages()).toHaveLength(4);

    // User changes status of page 0 to "In progress" and trashes page 1 in Notion.
    hooks().setNotionProperty(pages[0].id, "Status", "In progress");
    hooks().trashNotionPage(pages[1].id);

    const reexported = await b.reexportNotion("m5");
    expect(reexported.queued).toBe(4);
    await settled(b);

    const after = hooks().notionPages();
    expect(after).toHaveLength(4);
    expect(after[0].status).toBe("In progress");
    expect(after[1].trashed).toBe(true);
    const ops = (await b.outboxState()).ops.filter((o) => o.kind === "export_notion");
    expect(ops.find((o) => o.opKey === "notion:i03")?.result).toMatchObject({
      updated: true,
      created: false,
      removed: false,
    });
    expect(ops.find((o) => o.opKey === "notion:i04")?.result).toMatchObject({
      removed: true,
      created: false,
      updated: false,
    });
  });

  it("checks Discord bot permissions, searches members and roles, posts a thread and edits on correction", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.discord = {
      bot: {
        token: "cred:discord-bot/nomad",
        guild: "111111111111111111",
        channel: "222222222222222222",
        thread: true,
        editOnCorrection: true,
      },
      publish: "on-approval",
    };
    expect((await b.saveCycle(cycle)).saved).toBe(true);

    expect(await codeOf(b.setTargetSecret("cred:discord-bot/nomad", "not-a-bot-token"))).toBe(
      "invalid_bot_token",
    );
    await b.setTargetSecret("cred:discord-bot/nomad", BOT_TOKEN);

    const check = await b.testDiscordBot("NOMAD");
    expect(check).toMatchObject({
      ok: true,
      botName: "Nomad notes",
      guildName: "Nomad team",
      channelName: "meetings",
      missing: [],
    });

    hooks().setBotPermissions(["VIEW_CHANNEL", "SEND_MESSAGES"]);
    const missing = await b.testDiscordBot("NOMAD");
    expect(missing.ok).toBe(false);
    expect(missing.missing).toContain("CREATE_PUBLIC_THREADS");
    hooks().setBotPermissions([
      "VIEW_CHANNEL",
      "SEND_MESSAGES",
      "ATTACH_FILES",
      "READ_MESSAGE_HISTORY",
      "CREATE_PUBLIC_THREADS",
      "SEND_MESSAGES_IN_THREADS",
    ]);

    const members = await b.searchDiscordMembers("NOMAD", "al");
    expect(members.map((m) => m.username)).toEqual(["alex_demo", "alexandra_sample", "alek_test"]);
    const roles = await b.listDiscordRoles("NOMAD");
    expect(roles.map((r) => r.name)).toContain("Mechanics");

    const updatedCycle = await b.setParticipantDiscord("NOMAD", "alex", {
      userId: members[0].userId,
    });
    expect(updatedCycle.participants.find((p) => p.id === "alex")?.discord).toEqual({
      userId: "000000000000000001",
    });

    await b.publishMeeting("m5", ["github", "discord"]);
    await settled(b);

    const threads = hooks().discordThreads();
    expect(threads).toHaveLength(1);
    expect(hooks().discordBotMessages().length).toBeGreaterThanOrEqual(2);

    await b.addCorrection("m5", "Delivery moved to Monday.", undefined, undefined, "2026-11-16");
    const pubCorr = await b.publishCorrections("m5");
    expect(pubCorr.queued).toBe(2);
    await settled(b);

    const corrView = await b.listCorrections("m5");
    expect(corrView.corrections[0].discordEdit).toMatchObject({
      state: "done",
      result: {
        edited: true,
        replied: true,
        thread_id: threads[0].id,
      },
    });
    const botMsgs = hooks().discordBotMessages();
    expect(botMsgs[0].content).toContain("Corrections:");
    expect(botMsgs[0].content).toContain("Delivery moved to Monday.");
    expect(botMsgs.at(-1)?.content).toContain("Correction (2026-11-16): Delivery moved to Monday.");
  });
});
