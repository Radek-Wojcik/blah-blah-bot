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
      { target: "github", mode: "on-approval", available: true },
      { target: "discord", mode: "on-approval", available: true },
    ]);
    expect(plan.summary.commit && plan.summary.pushes).toBe(true);
    expect(plan.summary.files).toContain("transcript.md");
    expect(plan.summary.issues.map((i) => i.itemId)).toEqual(["i03", "i04", "i09", "i11"]);
    expect(plan.summary.issues[0]).toMatchObject({ assignee: "jan-demo", due: "2026-11-19" });
    expect(plan.summary.statuses.map((s) => s.target)).toEqual(["#12", "#11"]);
    expect(plan.summary.topicsAdded).toEqual(["m5-i07"]);
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

  it("blocks Discord without a webhook and a target the rules turn off", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    await b.clearTargetSecret("cred:discord/nomad");
    expect((await b.publishPlan("m5", ["discord"])).blocks.map((x) => x.code)).toEqual([
      "discord_webhook_missing",
    ]);
    expect(await codeOf(b.publishMeeting("m5", ["discord"]))).toBe("discord_webhook_missing");
    const cycle = (await b.getCycle("NOMAD")).cycle;
    cycle.targets.github = { path: "meetings", publish: "off" };
    cycle.targets.discord = { webhook: "cred:discord/nomad", publish: "off" };
    expect((await b.saveCycle(cycle)).saved).toBe(true);
    const off = await b.publishPlan("m5");
    expect(off.targets.map((t) => t.available)).toEqual([false, false]);
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

  it("locks the draft once operations exist", async () => {
    const b = fresh();
    await approveAll(b);
    await publishAll(b);
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
    expect(tasks.open.map((t) => t.number)).toEqual([11, 12]);
    expect(tasks.done.map((t) => t.number)).toEqual([9]);
    expect(
      hooks()
        .nextContext("NOMAD")
        .map((i) => i.issue),
    ).toEqual(["#11", "#12"]);

    // A colleague closes the issue made from the task of Alex.
    hooks().closeIssue(15);
    await b.pullCycleRepo("team-cycles");
    tasks = await b.cycleTasks("NOMAD");
    expect(tasks.asOf).toBe("2026-11-13");
    expect(tasks.done.map((t) => t.number).sort((a, b) => a - b)).toEqual([9, 11, 12, 15]);
    expect(tasks.open.map((t) => t.number)).toEqual([13, 14, 16]);
    expect(tasks.open[0].url).toBe("https://github.com/demo-org/nomad-cycles/issues/13");
    const context = hooks().nextContext("NOMAD");
    expect(context.map((i) => i.issue)).toEqual(["#13", "#14", "#16"]);
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

  it("cancels waiting operations and unlocks the draft with the last one", async () => {
    const b = fresh();
    await b.confirmPublicRepo("NOMAD");
    hooks().setOffline(true);
    await b.publishMeeting("m5", ["github"]);
    await until(
      () => b.outboxState(),
      (s) => s.offline.length > 0,
    );
    expect((await b.getDraft("m5")).locked).toBe(true);
    const unlocks: string[] = [];
    await b.onDraftChanged((id) => unlocks.push(id));
    const ids = (await b.outboxState()).ops.map((o) => o.id);
    for (const id of ids.slice(0, -1)) await b.cancelOutboxOp(id);
    expect((await b.getDraft("m5")).locked).toBe(true);
    await b.cancelOutboxOp(ids[ids.length - 1]);
    expect((await b.getDraft("m5")).locked).toBe(false);
    expect(unlocks).toContain("m5");
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
