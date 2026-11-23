import { describe, expect, it } from "vitest";
import type {
  Correction,
  CorrectionStep,
  CycleTasks,
  OpResult,
  OutboxOp,
  OutboxState,
  PublishPlan,
  StatusProposal,
  Topic,
} from "./ipc/types";
import {
  BOT_INTENTS_NOTE,
  BOT_PERMISSIONS_VALUE,
  CORRECTION_STATE_LABELS,
  DEFAULT_PUBLISH_RULE,
  NOTION_FIELD_SPECS,
  RULE_HELP,
  RULE_ORDER,
  STATUS_WRITE_BACK_NOTE,
  addableTargets,
  apiHost,
  autoWarning,
  blockOf,
  botTokenProblem,
  canCancel,
  canRetry,
  chosenTargets,
  committedOp,
  conflictRows,
  correctionAbout,
  correctionBadgeClass,
  correctionProblem,
  correctionRuleHelp,
  correctionStatusLine,
  countOps,
  countProposals,
  decisionLabel,
  discordChannelUrl,
  discordInviteUrl,
  discordMemberLabel,
  issueNumber,
  isCorrectionOp,
  issueUrl,
  lockedByOps,
  maskBotToken,
  maskNotionToken,
  maskWebhook,
  missingBotPermissions,
  notionTokenProblem,
  offlineText,
  opLink,
  opStatusLine,
  opsOf,
  planTopicsSummary,
  plannedTopicLine,
  otherBlocks,
  outboxHeadline,
  parseWebhook,
  pendingStatusNote,
  planLines,
  proposalEffect,
  publicRepoBlock,
  referenceKindByName,
  repoWarning,
  targetKind,
  targetsSent,
  taskCounts,
  taskOfTarget,
  tasksAsOf,
  today,
  tokenProblem,
  unqueued,
  topicDifference,
  topicLine,
  topicsSummary,
  waitText,
  webBase,
} from "./publish";

const op = (id: number, patch: Partial<OutboxOp> = {}): OutboxOp => ({
  id,
  meetingId: "m5",
  target: "github",
  kind: "commit_meeting",
  opKey: `k${id}`,
  label: "Add the report",
  state: "queued",
  attempts: 0,
  nextAttemptUnixMs: null,
  lastError: null,
  result: null,
  afterId: null,
  createdUnixMs: 0,
  updatedUnixMs: 0,
  ...patch,
});

const plan = (patch: Partial<PublishPlan> = {}): PublishPlan => ({
  meetingId: "m5",
  cycleId: "NOMAD",
  targets: [
    { target: "github", mode: "on-approval", available: true },
    { target: "discord", mode: "off", available: false },
  ],
  summary: {
    commit: true,
    pushes: true,
    files: ["meeting.json", "report.md", "img/0001.png", "img/0002.png"],
    issues: [
      { itemId: "i03", title: "Update the schedule", assignee: "jan-demo", due: "2026-11-19" },
    ],
    statuses: [{ proposalId: "s1", target: "#12", open: false }],
    topicsAdded: ["m5-i07"],
    topicsSet: [{ id: "t-1", open: false }],
    discord: false,
    discordMode: null,
    discordThread: false,
    notion: [],
    proposalsPending: 2,
  },
  blocks: [],
  notes: [],
  repo: {
    key: "github.com/demo-org/nomad-cycles",
    github: "demo-org/nomad-cycles",
    hasRemote: true,
    visibility: "public",
    confirmed: false,
  },
  locked: false,
  newDraftPending: false,
  ops: [],
  ...patch,
});

describe("GitHub token", () => {
  it("accepts one word and refuses empty text, spaces and line breaks", () => {
    expect(tokenProblem("ghp_demo0123456789")).toBeNull();
    expect(tokenProblem("  ghp_demo  ")).toBeNull();
    expect(tokenProblem("github_pat_unusual-letters_u0")).toBeNull();
    expect(tokenProblem("")).toBe("Enter the token.");
    expect(tokenProblem("two words")).toMatch(/spaces or line breaks/);
    expect(tokenProblem("line\nbreak")).toMatch(/spaces or line breaks/);
  });

  it("never repeats the token in its message", () => {
    expect(tokenProblem("ghp_secret value")).not.toContain("secret");
  });
});

describe("Discord webhook", () => {
  const id = "000000000000000001";

  it("accepts the Discord hosts and a loopback server", () => {
    for (const host of ["discord.com", "discordapp.com", "canary.discord.com", "ptb.discord.com"])
      expect(parseWebhook(`https://${host}/api/webhooks/${id}/tok-en_1`)).toEqual({ host, id });
    expect(parseWebhook(`https://discord.com/api/v10/webhooks/${id}/token`)).toEqual({
      host: "discord.com",
      id,
    });
    expect(parseWebhook(`http://127.0.0.1:8080/api/webhooks/${id}/token`)).toEqual({
      host: "127.0.0.1",
      id,
    });
  });

  it.each([
    ["not a url", /not a web address/],
    ["ftp://discord.com/api/webhooks/000000000000000001/t", /must start with https/],
    ["https://example.org/api/webhooks/000000000000000001/t", /not a Discord address/],
    ["http://example.org/api/webhooks/000000000000000001/t", /only accepted for this machine/],
    ["https://discord.com:8443/api/webhooks/000000000000000001/t", /no port/],
    ["https://user@discord.com/api/webhooks/000000000000000001/t", /credentials/],
    ["https://discord.com/api/webhooks/000000000000000001/t?wait=true", /query or a fragment/],
    ["https://discord.com", /no path/],
    ["https://discord.com/api/webhooks/12/t", /17 to 20 digits/],
    ["https://discord.com/other/path", /must be \/api\/webhooks/],
  ])("refuses %s", (url, message) => {
    const r = parseWebhook(url);
    expect(typeof r).toBe("string");
    expect(r as string).toMatch(message);
    // The message never contains the token part of the address.
    expect(r as string).not.toContain("webhooks/000");
  });

  it("masks all but the host and the ends of the id", () => {
    expect(maskWebhook({ host: "discord.com", id: "123456789012345678" })).toBe(
      "discord.com webhook 1234...5678",
    );
  });
});

describe("GitHub addresses", () => {
  it("takes the host of an API address", () => {
    expect(apiHost("https://api.github.com")).toBe("api.github.com");
    expect(apiHost("https://GHE.example.org/api/v3")).toBe("ghe.example.org");
    expect(apiHost("not an address")).toBeNull();
  });

  it("links to the web side of the server", () => {
    expect(webBase(undefined)).toBe("https://github.com");
    expect(webBase("https://api.github.com")).toBe("https://github.com");
    expect(webBase("https://api.ghe.example.org")).toBe("https://ghe.example.org");
    expect(issueUrl("demo-org/nomad-cycles", 12)).toBe(
      "https://github.com/demo-org/nomad-cycles/issues/12",
    );
  });
});

describe("outbox", () => {
  const state: OutboxState = {
    ops: [op(2, { meetingId: "m5" }), op(1, { meetingId: "m5" }), op(3, { meetingId: "m4" })],
    offline: [],
    nowUnixMs: 0,
  };

  it("lists the operations of one meeting in the order they were queued", () => {
    expect(opsOf(state, "m5").map((o) => o.id)).toEqual([1, 2]);
    expect(opsOf(null, "m5")).toEqual([]);
  });

  it("locks the draft unless every operation is cancelled", () => {
    expect(lockedByOps([])).toBe(false);
    expect(lockedByOps([op(1, { state: "cancelled" })])).toBe(false);
    expect(lockedByOps([op(1, { state: "cancelled" }), op(2, { state: "failed" })])).toBe(true);
  });

  it("counts and summarises", () => {
    const ops = [
      op(1, { state: "done" }),
      op(2, { state: "running" }),
      op(3),
      op(4, { state: "failed" }),
      op(5, { state: "cancelled" }),
    ];
    expect(countOps(ops)).toEqual({ queued: 1, running: 1, done: 1, failed: 1, cancelled: 1 });
    expect(outboxHeadline(ops)).toBe("1 of 4 done, 1 running, 1 waiting, 1 failed");
    expect(outboxHeadline([op(1, { state: "done" }), op(2, { state: "done" })])).toBe(
      "Published: everything was sent.",
    );
    expect(outboxHeadline([op(1, { state: "cancelled" })])).toBe("Nothing is queued.");
  });

  it("writes waiting times", () => {
    expect(waitText(0)).toBe("now");
    expect(waitText(-5)).toBe("now");
    expect(waitText(1)).toBe("in 1 s");
    expect(waitText(45_000)).toBe("in 45 s");
    expect(waitText(90_000)).toBe("in 1 min 30 s");
    expect(waitText(120_000)).toBe("in 2 min");
    expect(waitText(3_900_000)).toBe("in 1 h 5 min");
  });

  it("explains what an operation waits for", () => {
    expect(opStatusLine(op(1, { state: "failed", lastError: "no token" }), 0, false)).toBe(
      "no token",
    );
    expect(opStatusLine(op(1, { state: "done" }), 0, false)).toBeNull();
    expect(opStatusLine(op(2, { afterId: 1 }), 0, false)).toBe(
      "Waits for the operations before it.",
    );
    expect(
      opStatusLine(
        op(3, { lastError: "offline.", attempts: 1, nextAttemptUnixMs: 30_000 }),
        0,
        true,
      ),
    ).toBe("offline. Next attempt when the network is back or in 30 s.");
    expect(
      opStatusLine(op(3, { lastError: "boom.", attempts: 1, nextAttemptUnixMs: 30_000 }), 0, false),
    ).toBe("boom. Next attempt in 30 s.");
  });

  it("allows retry and cancel only where the backend does", () => {
    expect(canRetry(op(1, { state: "failed" }))).toBe(true);
    expect(canRetry(op(1, { lastError: "x", attempts: 1 }))).toBe(true);
    expect(canRetry(op(1))).toBe(false);
    expect(canRetry(op(1, { state: "done" }))).toBe(false);
    for (const [state, cancel] of [
      ["queued", true],
      ["failed", true],
      ["running", false],
      ["done", false],
      ["cancelled", false],
    ] as const)
      expect(canCancel(op(1, { state }))).toBe(cancel);
  });

  it("links to what an operation produced", () => {
    expect(
      opLink(
        op(1, {
          kind: "create_issue",
          state: "done",
          result: { number: 13, url: "https://github.com/o/r/issues/13" },
        }),
      ),
    ).toEqual({ href: "https://github.com/o/r/issues/13", text: "#13" });
    expect(
      opLink(
        op(1, {
          state: "done",
          result: { commit: "abc1234", report_url: "https://github.com/o/r/blob/main/x/report.md" },
        }),
      ),
    ).toEqual({ href: "https://github.com/o/r/blob/main/x/report.md", text: "commit abc1234" });
    expect(opLink(op(1, { state: "queued" }))).toBeNull();
  });

  it("tells which targets are offline", () => {
    expect(offlineText(null)).toBeNull();
    expect(offlineText({ ops: [], offline: [], nowUnixMs: 0 })).toBeNull();
    expect(offlineText({ ops: [], offline: ["github", "issues"], nowUnixMs: 0 })).toMatch(
      /^Offline: GitHub did not answer\./,
    );
    expect(offlineText({ ops: [], offline: ["github", "discord"], nowUnixMs: 0 })).toMatch(
      /GitHub and Discord did not answer/,
    );
  });

  it("knows which targets were sent completely", () => {
    const ops = [
      op(1, { state: "done" }),
      op(2, { target: "issues", kind: "create_issue", state: "done" }),
      op(3, { target: "issues", kind: "create_issue", state: "queued" }),
      op(4, { target: "discord", kind: "post_discord", state: "done" }),
    ];
    expect([...targetsSent(ops)]).toEqual(["discord"]);
    expect([...targetsSent(ops.map((o) => ({ ...o, state: "done" as const })))].sort()).toEqual([
      "discord",
      "github",
    ]);
    // A cancelled operation does not count against a target.
    expect([...targetsSent([op(1, { state: "done" }), op(2, { state: "cancelled" })])]).toEqual([
      "github",
    ]);
    expect(targetsSent([]).size).toBe(0);
  });

  it("finds the finished commit", () => {
    expect(committedOp([op(1), op(2, { state: "done" })])?.id).toBe(2);
    expect(committedOp([op(1)])).toBeNull();
  });
});

describe("plan", () => {
  it("describes what will happen in the order it runs", () => {
    const topics: Topic[] = [
      { id: "t-1", text: "Decide the rover wheel size", status: "open", openedIn: "2026-11-06" },
    ];
    expect(planLines(plan(), topics)).toEqual([
      "Commit and push 4 files to demo-org/nomad-cycles: meeting.json, report.md, 2 screenshots.",
      'Create the issue "Update the schedule", assigned to jan-demo, due 2026-11-19.',
      "Close issue #12 (approved status change s1).",
      'Add the topic "m5-i07" to topics.yaml.',
      "Close the topic t-1 (Decide the rover wheel size) in topics.yaml.",
    ]);
  });

  it("mentions Discord and a commit without a push", () => {
    const p = plan({
      summary: {
        ...plan().summary,
        pushes: false,
        files: ["report.md"],
        issues: [],
        statuses: [],
        topicsAdded: [],
        topicsSet: [],
        discord: true,
      },
    });
    expect(planLines(p)).toEqual([
      "Commit 1 file to demo-org/nomad-cycles: report.md.",
      "Post the summary to Discord, with links to the report and the issues.",
    ]);
  });

  it("finds the blocks and the pending note", () => {
    const blocked = plan({
      blocks: [
        { code: "public_repo_unconfirmed", message: "public", visibility: "public" },
        { code: "discord_webhook_missing", message: "no webhook", visibility: null },
      ],
    });
    expect(publicRepoBlock(blocked)?.visibility).toBe("public");
    expect(blockOf(blocked, "nothing_to_publish")).toBeNull();
    expect(otherBlocks(blocked).map((b) => b.code)).toEqual(["discord_webhook_missing"]);
    expect(publicRepoBlock(null)).toBeNull();
    expect(pendingStatusNote(plan())).toBe(
      "2 status changes are not approved and will not be written back.",
    );
    expect(pendingStatusNote(plan({ summary: { ...plan().summary, proposalsPending: 1 } }))).toBe(
      "1 status change is not approved and will not be written back.",
    );
    expect(
      pendingStatusNote(plan({ summary: { ...plan().summary, proposalsPending: 0 } })),
    ).toBeNull();
  });

  it("warns about a public or an unknown repository", () => {
    expect(repoWarning("public", "demo-org/x")).toBe(
      "demo-org/x is public: everybody can read what is published to it.",
    );
    expect(repoWarning("unknown", null)).toMatch(/could not be checked/);
  });

  it("sends only the targets that are ticked and allowed", () => {
    const p = plan();
    expect(chosenTargets(p, { github: true, discord: true, notion: true })).toEqual(["github"]);
    expect(chosenTargets(p, { github: false, discord: true, notion: true })).toEqual([]);
    expect(chosenTargets(null, { github: true, discord: true, notion: true })).toEqual([]);
  });
});

describe("status proposals", () => {
  const proposal = (
    id: string,
    target: string,
    decision: StatusProposal["decision"],
  ): StatusProposal => ({
    id,
    target,
    status: "done",
    reason: "said so",
    comment: null,
    source: null,
    decision,
    edited: false,
  });

  it("counts decisions", () => {
    expect(countProposals(undefined)).toEqual({ proposed: 0, approved: 0, rejected: 0 });
    expect(
      countProposals([
        proposal("s1", "#1", "approved"),
        proposal("s2", "t-1", "proposed"),
        proposal("s3", "#3", "approved"),
      ]),
    ).toEqual({ proposed: 1, approved: 2, rejected: 0 });
  });

  it("tells an issue from a topic", () => {
    expect(targetKind("#12")).toBe("issue");
    expect(targetKind("t-1")).toBe("topic");
    expect(targetKind("12")).toBe("topic");
    expect(issueNumber("#12")).toBe(12);
    expect(issueNumber("t-1")).toBeNull();
  });

  it("says what approving does", () => {
    expect(proposalEffect(proposal("s1", "#12", "proposed"))).toBe(
      "Closes issue #12 on GitHub and adds a comment with the quote.",
    );
    expect(proposalEffect({ ...proposal("s2", "t-1", "proposed"), status: "open" })).toBe(
      "Reopens topic t-1 in topics.yaml.",
    );
    expect(decisionLabel(proposal("s1", "#1", "proposed"))).toBe("Waiting for your decision");
    expect(decisionLabel(proposal("s1", "#1", "approved"))).toBe("Approved");
    expect(decisionLabel(proposal("s1", "#1", "rejected"))).toBe("Rejected");
  });

  it("offers open targets that have no status change yet", () => {
    const tasks: CycleTasks = {
      cycleId: "NOMAD",
      repo: "demo-org/nomad-cycles",
      asOf: "2026-11-13",
      open: [
        { number: 11, title: "Order valves", assignee: null, url: null, meeting: null },
        { number: 12, title: "Finish drawings", assignee: null, url: null, meeting: null },
      ],
      done: [],
    };
    const topics: Topic[] = [
      { id: "t-1", text: "Wheel size", status: "open", openedIn: "2026-11-06" },
      { id: "t-2", text: "Battery", status: "closed", openedIn: "2026-11-06" },
    ];
    expect(addableTargets([proposal("s1", "#12", "proposed")], tasks, topics)).toEqual([
      { value: "#11", label: "#11 Order valves" },
      { value: "t-1", label: "t-1 Wheel size" },
    ]);
    expect(taskOfTarget(tasks, "#12")?.title).toBe("Finish drawings");
    expect(taskOfTarget(tasks, "t-1")).toBeNull();
    expect(taskOfTarget(null, "#12")).toBeNull();
  });
});

describe("topics.yaml conflicts", () => {
  const topics: Topic[] = [
    { id: "t-1", text: "Decide the rover wheel size", status: "closed", openedIn: "2026-11-06" },
  ];

  it("shows both versions when the backend reports them", () => {
    const result: OpResult = {
      topics: {
        added: 1,
        changed: 1,
        removed: 0,
        conflicts: ["t-1"],
        conflict_details: [
          {
            id: "t-1",
            theirs: { text: "Decide the wheel size and the tyres", status: "open" },
            ours: { text: "Decide the rover wheel size", status: "closed" },
          },
        ],
      },
    };
    const [row] = conflictRows(result, topics);
    expect(row.partial).toBe(false);
    expect(topicLine(row.theirs)).toBe("[open] Decide the wheel size and the tyres");
    expect(topicLine(row.ours)).toBe("[closed] Decide the rover wheel size");
    expect(topicDifference(row.theirs, row.ours)).toEqual(["text", "status"]);
    expect(topicsSummary(result)).toBe("1 added, 1 changed");
  });

  it("falls back to the topic as it is now when only the ids are known", () => {
    const [row] = conflictRows(
      { topics: { added: 0, changed: 1, removed: 0, conflicts: ["t-1"] } },
      topics,
    );
    expect(row.partial).toBe(true);
    expect(row.theirs).toBeNull();
    expect(row.ours).toEqual({ text: "Decide the rover wheel size", status: "closed" });
  });

  it("has nothing to show without topics", () => {
    expect(conflictRows(null, topics)).toEqual([]);
    expect(conflictRows({ topics: null }, topics)).toEqual([]);
    expect(
      topicsSummary({ topics: { added: 0, changed: 0, removed: 0, conflicts: [] } }),
    ).toBeNull();
    expect(topicDifference(null, { text: "x", status: "open" })).toEqual(["presence"]);
    expect(topicLine(null)).toBe("(not there)");
  });
});

describe("tasks", () => {
  const tasks: CycleTasks = {
    cycleId: "NOMAD",
    repo: "demo-org/nomad-cycles",
    asOf: "2026-11-13",
    open: [{ number: 11, title: "x", assignee: null, url: null, meeting: null }],
    done: [
      { number: 9, title: "y", assignee: null, url: null, meeting: null },
      { number: 10, title: "z", assignee: null, url: null, meeting: null },
    ],
  };

  it("counts and dates", () => {
    expect(taskCounts(tasks)).toBe("1 open, 2 done");
    expect(taskCounts(null)).toBe("");
    expect(tasksAsOf(tasks)).toBe("As of 2026-11-13.");
    expect(tasksAsOf({ ...tasks, asOf: null })).toMatch(/never read/);
  });
});

describe("rules and the topics of the plan", () => {
  it("orders the switch from the safest position and says what no rule changes", () => {
    expect(RULE_ORDER).toEqual(["off", "on-approval", "auto"]);
    expect(STATUS_WRITE_BACK_NOTE).toMatch(/never written back automatically/);
    expect(STATUS_WRITE_BACK_NOTE).toMatch(/you approved/);
  });

  it("warns under Automatic only, and says it cannot be taken back", () => {
    expect(autoWarning("github", "off")).toBeNull();
    expect(autoWarning("discord", "on-approval")).toBeNull();
    expect(autoWarning("github", "auto")).toMatch(/cannot be taken back/);
    expect(autoWarning("discord", "auto")).toMatch(/mentions/);
  });

  const view = (text: string, status: "open" | "closed") => ({
    text,
    status,
    openedIn: "2026-11-06",
    closedIn: status === "closed" ? "2026-11-13" : null,
  });

  it("describes the topic changes of a plan in a few words and each topic as a line", () => {
    const topics = {
      againstRemote: true,
      unchanged: 2,
      changes: [
        {
          id: "m5-i07",
          change: "added" as const,
          before: null,
          after: view("Battery supplier", "open"),
          overridesRemote: false,
        },
        {
          id: "t-1",
          change: "changed" as const,
          before: view("Wheel size and tyres", "open"),
          after: view("Wheel size", "closed"),
          overridesRemote: true,
        },
      ],
    };
    expect(planTopicsSummary(topics)).toBe("1 added, 1 changed");
    expect(planTopicsSummary({ ...topics, changes: [] })).toBeNull();
    expect(plannedTopicLine(topics.changes[1].before)).toBe("[open] Wheel size and tyres");
    expect(plannedTopicLine(null)).toBe("(not there)");
    // The names stay in the list with or without the difference, which is the fallback alone.
    const p = plan();
    const names = (q: PublishPlan) => planLines(q).filter((l) => l.includes("topics.yaml"));
    expect(names(plan({ summary: { ...p.summary, topics } }))).toEqual(
      names(plan({ summary: { ...p.summary, topics: null } })),
    );
    expect(names(p)).toEqual([
      'Add the topic "m5-i07" to topics.yaml.',
      "Close the topic t-1 in topics.yaml.",
    ]);
  });
});

describe("corrections", () => {
  const base: Correction = {
    id: "c1",
    date: "2026-11-16",
    text: "The mount moved.",
    item: "i3",
    issue: "#12",
    state: "local",
    file: null,
    comment: null,
    fileName: null,
    url: null,
    canDelete: true,
  };
  const step = (over: Partial<CorrectionStep>): CorrectionStep => ({
    opId: 1,
    state: "done",
    attempts: 1,
    nextAttemptUnixMs: null,
    lastError: null,
    result: null,
    ...over,
  });

  it("explains the rule of the github target in plain words, for every value", () => {
    expect(correctionRuleHelp("on-approval")).toMatch(/until you press Publish/);
    expect(correctionRuleHelp("auto")).toMatch(/as soon as you add it/);
    expect(correctionRuleHelp("auto")).toMatch(/cannot be taken back/);
    expect(correctionRuleHelp("off")).toMatch(/never published/);
    expect(correctionRuleHelp("off")).toMatch(/still reach the next meeting/);
  });

  it("says what a correction is about", () => {
    expect(correctionAbout(base)).toBe("item i3 and issue #12");
    expect(correctionAbout({ item: "i3", issue: null })).toBe("item i3");
    expect(correctionAbout({ item: null, issue: "#4" })).toBe("issue #4");
    expect(correctionAbout({ item: null, issue: null })).toBeNull();
  });

  it("describes where a correction stands, with the comment on the issue", () => {
    expect(correctionStatusLine(base, "on-approval")).toMatch(/Not published yet/);
    expect(correctionStatusLine(base, "off")).toMatch(/publishing is Off/);
    expect(
      correctionStatusLine({ ...base, state: "queued", file: step({ state: "queued" }) }, "auto"),
    ).toBe("Waits to be published.");
    expect(
      correctionStatusLine(
        {
          ...base,
          state: "queued",
          file: step({ state: "queued", lastError: "The network is offline." }),
        },
        "auto",
      ),
    ).toBe("The network is offline.");
    expect(correctionStatusLine({ ...base, state: "running" }, "auto")).toBeNull();
    const published: Correction = {
      ...base,
      state: "published",
      fileName: "corrections-2.md",
      file: step({}),
    };
    expect(correctionStatusLine(published, "auto")).toBe(
      "Published as corrections-2.md in the meeting folder.",
    );
    expect(correctionStatusLine({ ...published, comment: step({}) }, "auto")).toMatch(
      /and commented on the issue/,
    );
    expect(
      correctionStatusLine(
        { ...published, comment: step({ result: { skipped: "no issue was made for this item" } }) },
        "auto",
      ),
    ).toMatch(/No comment was made on an issue: no issue was made/);
    expect(
      correctionStatusLine(
        { ...published, comment: step({ state: "failed", lastError: "Issue not found." }) },
        "auto",
      ),
    ).toMatch(/comment on the issue failed: Issue not found/);
    expect(
      correctionStatusLine({ ...published, comment: step({ state: "queued" }) }, "auto"),
    ).toMatch(/still to come/);
  });

  it("checks a note before it is sent, and offers today in the local calendar", () => {
    expect(correctionProblem("  ", "2026-11-16")).toMatch(/Write what changed/);
    expect(correctionProblem("x".repeat(2001), "2026-11-16")).toMatch(/2000/);
    expect(correctionProblem("x", "16.11.2026")).toMatch(/YYYY-MM-DD/);
    expect(correctionProblem("x", "2026-11-16")).toBeNull();
    expect(today(new Date(2026, 10, 5, 23, 59))).toBe("2026-11-05");
  });

  it("keeps the operations of corrections apart from those of the meeting", () => {
    const op = (kind: OutboxOp["kind"]): OutboxOp => ({
      id: 1,
      meetingId: "m5",
      target: "github",
      kind,
      opKey: "k",
      label: "l",
      state: "done",
      attempts: 0,
      nextAttemptUnixMs: null,
      lastError: null,
      result: { file: "corrections.md", url: "https://github.com/o/r/blob/main/x/corrections.md" },
      afterId: null,
      createdUnixMs: 1,
      updatedUnixMs: 1,
    });
    expect(isCorrectionOp(op("commit_correction"))).toBe(true);
    expect(isCorrectionOp(op("comment_correction"))).toBe(true);
    expect(isCorrectionOp(op("commit_meeting"))).toBe(false);
    expect(opLink(op("commit_correction"))).toEqual({
      href: "https://github.com/o/r/blob/main/x/corrections.md",
      text: "corrections.md",
    });
    expect(unqueued([base, { ...base, id: "c2", state: "published" }]).map((c) => c.id)).toEqual([
      "c1",
    ]);
    expect(CORRECTION_STATE_LABELS.local).toBe("Only on this computer");
    expect(correctionBadgeClass("published")).toBe("badge badge-job-done");
  });

  it("explains each publishing rule by its consequence, and the one in use is the default", () => {
    expect(RULE_HELP.off).toMatch(/Nothing is ever published/);
    expect(RULE_HELP["on-approval"]).toMatch(/until you have looked at the draft/);
    expect(RULE_HELP["on-approval"]).toMatch(/used until you choose another rule/);
    expect(RULE_HELP.auto).toMatch(/cannot be taken back/);
    expect(DEFAULT_PUBLISH_RULE).toEqual({
      github: "on-approval",
      discord: "on-approval",
      notion: "on-approval",
    });
  });
});

describe("Stage 7: Discord bot and Notion helpers", () => {
  const FAKE_BOT_TOKEN = ["MTIzNDU2Nzg5MDEyMzQ1Njc4", "GhIjKl", "MnOpQrStUvWxYz0123456789abcdef"].join(".");
  const FAKE_NOTION_TOKEN = ["ntn", "demo_fake_token_0123456789abcdef"].join("_");

  it("validates and masks Discord bot and Notion tokens without leaking them", () => {
    expect(referenceKindByName("cred:discord-bot/nomad")).toBe("discord-bot");
    expect(referenceKindByName("cred:discord/nomad")).toBe("discord");
    expect(referenceKindByName("cred:notion/nomad")).toBe("notion");
    expect(referenceKindByName("cred:other/x")).toBeNull();

    expect(botTokenProblem(FAKE_BOT_TOKEN)).toBeNull();
    expect(botTokenProblem("bad-secret-value")).not.toContain("bad-secret-value");
    expect(maskBotToken(FAKE_BOT_TOKEN)).toBe("bot MTIz...");

    expect(notionTokenProblem(FAKE_NOTION_TOKEN)).toBeNull();
    expect(notionTokenProblem("bad-notion-secret")).not.toContain("bad-notion-secret");
    expect(maskNotionToken(FAKE_NOTION_TOKEN)).toBe("ntn_...cdef");
  });

  it("computes the exact Discord bot permission integer, invite URL and missing permissions", () => {
    expect(BOT_PERMISSIONS_VALUE).toBe(309237746688);
    expect(BOT_INTENTS_NOTE).toMatch(/no privileged intents/i);
    expect(discordInviteUrl("123456789012345678")).toBe(
      "https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=bot&permissions=309237746688",
    );
    expect(discordInviteUrl("not-an-id")).toBeNull();
    expect(missingBotPermissions(["VIEW_CHANNEL", "SEND_MESSAGES"], true)).toEqual([
      "ATTACH_FILES",
      "READ_MESSAGE_HISTORY",
      "CREATE_PUBLIC_THREADS",
      "SEND_MESSAGES_IN_THREADS",
    ]);
    expect(missingBotPermissions(["VIEW_CHANNEL", "SEND_MESSAGES"], false)).toEqual([
      "ATTACH_FILES",
      "READ_MESSAGE_HISTORY",
    ]);
  });

  it("formats Discord member labels, channel links and Notion field metadata", () => {
    expect(
      discordMemberLabel({
        userId: "1",
        username: "kasia_demo",
        globalName: "Kasia Demo",
        nick: "Kasia (comms)",
        bot: false,
      }),
    ).toBe("Kasia (comms) (@kasia_demo)");
    expect(
      discordMemberLabel({
        userId: "2",
        username: "nomad_notes",
        globalName: null,
        nick: "Nomad notes",
        bot: true,
      }),
    ).toBe("Nomad notes (@nomad_notes) [bot]");
    expect(discordChannelUrl("111", "222")).toBe("https://discord.com/channels/111/222");
    expect(discordChannelUrl("111", "222", "333")).toBe("https://discord.com/channels/111/222/333");
    expect(NOTION_FIELD_SPECS.map((s) => s.field)).toEqual([
      "title",
      "key",
      "assignee",
      "due",
      "status",
      "meeting",
      "cycle",
    ]);
  });

  it("describes Notion and Discord bot plans, links, status lines and correction edits", () => {
    const p = plan({
      summary: {
        ...plan().summary,
        commit: false,
        issues: [],
        statuses: [],
        topicsAdded: [],
        topicsSet: [],
        discord: true,
        discordMode: "bot",
        discordThread: true,
        notion: [
          { itemId: "i1", title: "Order valves", existingUrl: null },
          {
            itemId: "i2",
            title: "Check mount",
            existingUrl: "https://www.notion.so/00000000000000000000000000000001",
          },
        ],
      },
    });
    const lines = planLines(p);
    expect(lines).toContain(
      "Export 2 tasks to Notion, 1 already there (updated, the status is kept).",
    );
    expect(lines).toContain(
      "Post the summary to Discord through the bot, with the issue links and the screenshots in a thread.",
    );

    const notionOp = op(1, {
      target: "notion",
      kind: "export_notion",
      state: "done",
      result: {
        page_id: "p1",
        url: "https://www.notion.so/p1",
        updated: true,
        removed: false,
      },
    });
    expect(opLink(notionOp)).toEqual({ href: "https://www.notion.so/p1", text: "Notion page" });
    expect(opStatusLine(notionOp, 0, false)).toBe("Updated the existing page (status kept).");

    const removedOp = op(2, {
      target: "notion",
      kind: "export_notion",
      state: "done",
      result: { page_id: "p2", removed: true },
    });
    expect(opLink(removedOp)).toBeNull();
    expect(opStatusLine(removedOp, 0, false)).toMatch(/moved to the trash/i);

    const botOp = op(3, {
      target: "discord",
      kind: "post_discord",
      state: "done",
      result: { channel_id: "222", thread_id: "900", message_ids: ["900", "901"] },
    });
    expect(opLink(botOp, "111")).toEqual({
      href: "https://discord.com/channels/111/900",
      text: "Discord thread",
    });
    expect(opStatusLine(botOp, 0, false)).toBe("Posted through the bot with a thread.");

    const editOp = op(4, {
      target: "discord",
      kind: "edit_discord",
      state: "done",
      result: { edited: true, replied: true, thread_id: "900" },
    });
    expect(isCorrectionOp(editOp)).toBe(true);
    expect(targetsSent([notionOp, botOp, editOp])).toEqual(new Set(["notion", "discord"]));

    const editedCorrection: Correction = {
      id: "c1",
      date: "2026-11-15",
      text: "Updated delivery date",
      item: null,
      issue: null,
      state: "published",
      file: null,
      comment: null,
      discordEdit: {
        opId: 4,
        state: "done",
        attempts: 1,
        nextAttemptUnixMs: null,
        lastError: null,
        result: { edited: true, replied: true, thread_id: "900" },
      },
      fileName: "corrections.md",
      url: "https://github.com/o/r/blob/main/x/corrections.md",
      canDelete: false,
    };
    expect(correctionStatusLine(editedCorrection, "on-approval")).toBe(
      "Published as corrections.md in the meeting folder. Discord summary edited and replied in the thread.",
    );
  });
});
