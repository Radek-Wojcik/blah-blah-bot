/**
 * Publishing in the mock backend: secrets that are only ever flags, the publication plan, an
 * outbox that runs operations one by one against a fake GitHub and a fake Discord (nothing
 * leaves the page), the public repository gate, the read-only draft after publishing, the
 * `topics.yaml` merge with its conflicts, and the tasks read from the issues.
 *
 * Fictional data only. The fake world can be driven from tests and screenshots through
 * `MockHooks` (`window.__bbbMock`): switch the network off, close an issue as a colleague
 * would, make the repository private.
 */
import type { Backend, Unsubscribe } from "./backend";
import { MOCK_TODAY, type CycleStore } from "./mock-cycles";
import type { MockDrafts } from "./mock-drafts";
import { fail } from "./mock-util";
import {
  apiHost,
  hasControlChars,
  issueUrl,
  maskWebhook,
  parseWebhook,
  tokenProblem,
  webBase,
} from "../publish";
import type {
  CycleTask,
  CycleTasks,
  Draft,
  DraftView,
  OpKind,
  OpResult,
  OpState,
  OpTarget,
  OutboxOp,
  OutboxState,
  PlanBlock,
  PlanSummary,
  PublicRepoConfirmation,
  PublishPlan,
  PublishResult,
  PublishTargetName,
  PublishTargetView,
  PublishingSecrets,
  RepoVisibility,
  Topic,
  TopicConflictDetail,
} from "./types";

type PublishMethods = Pick<
  Backend,
  | "publishingSecrets"
  | "setGithubToken"
  | "clearGithubToken"
  | "testGithubAccess"
  | "setGithubRepo"
  | "setTargetSecret"
  | "clearTargetSecret"
  | "testDiscordWebhook"
  | "publishPlan"
  | "publishMeeting"
  | "confirmPublicRepo"
  | "outboxState"
  | "retryOutboxOp"
  | "cancelOutboxOp"
  | "cycleTasks"
  | "onOutboxChanged"
>;

/** An issue of the fake GitHub. */
export interface FakeIssue {
  number: number;
  title: string;
  /** The GitHub login. */
  assignee: string | null;
  state: "open" | "closed";
  /** The meeting and item the issue was made from; null for an issue made by hand. */
  meeting: string | null;
  item: string | null;
  due: string | null;
}

/** A message the fake Discord received. */
export interface FakeDiscordPost {
  reference: string;
  content: string;
}

/** What a test or a screenshot may do to the fake world. */
export interface MockHooks {
  /** No target answers while it is on. Turning it off lets waiting operations run again. */
  setOffline(offline: boolean): void;
  /** A colleague closes (or reopens) an issue on GitHub. It shows in the tasks after a pull. */
  closeIssue(number: number): void;
  reopenIssue(number: number): void;
  setVisibility(visibility: "public" | "private"): void;
  issues(): FakeIssue[];
  discordPosts(): FakeDiscordPost[];
  /** The earlier items the next processing of the cycle would be given (done ones are left out). */
  nextContext(cycleId: string): { text: string; issue: string }[];
  /** The `topics.yaml` of the remote, as a colleague left it. */
  remoteTopics(): Topic[];
}

export interface MockPublishing {
  methods: PublishMethods;
  hooks: MockHooks;
  /** Operations of the meeting exist that are not cancelled: its draft is read-only. */
  isLocked(meetingId: string): boolean;
  /** The pull of a repository also reads the issues again. */
  refreshIssues(repoId: string): void;
}

export interface PublishDeps {
  cycles: CycleStore;
  drafts: MockDrafts;
  /** Milliseconds one operation takes (two ticks). */
  stepMs: number;
  /** The simulated time, in Unix milliseconds. */
  now: () => number;
  screenshotCount: (meetingId: string) => number;
  /** The draft of a meeting changed for a reason other than an edit (it was unlocked). */
  onUnlocked: (meetingId: string) => void;
}

const DEFAULT_API = "https://api.github.com";
const DISCORD_REFERENCE = /^cred:[A-Za-z0-9][A-Za-z0-9_./-]*$/;
const BACKOFF_MS = 30_000;
const BACKOFF_MAX_MS = 30 * 60_000;

/** Where an operation talks to: GitHub and its issues are one server. */
const group = (t: OpTarget): "github" | "discord" => (t === "discord" ? "discord" : "github");

/** Eight hex digits made from a text: a stand-in for a commit hash. */
function fakeSha(seed: string): string {
  let h = 2166136261;
  for (const c of seed) h = Math.imul(h ^ c.codePointAt(0)!, 16777619);
  return (h >>> 0).toString(16).padStart(8, "0").slice(0, 7);
}

/** The issues the demo repository already has, from earlier meetings. */
function demoIssues(): FakeIssue[] {
  const issue = (
    number: number,
    title: string,
    assignee: string | null,
    state: FakeIssue["state"],
    meeting: string,
    item: string,
  ): FakeIssue => ({ number, title, assignee, state, meeting, item, due: null });
  return [
    issue(9, "Book the test range for the December firing", "alex-demo", "closed", "m3", "i02"),
    issue(10, "Prepare the field test checklist", "jan-demo", "open", "m3", "i04"),
    issue(11, "Order the spare thruster valves", "kasia-demo", "open", "m4", "i05"),
    issue(12, "Finish the antenna mount drawings", "jan-demo", "open", "m4", "i07"),
  ];
}

/** What the colleague left in `topics.yaml` of the remote, which the local clone does not have. */
function demoRemoteTopics(local: Topic[]): Topic[] {
  const out = local.map((t) =>
    t.id === "t-1" ? { ...t, text: "Decide the rover wheel size and the tyre type" } : { ...t },
  );
  out.push({
    id: "t-3",
    text: "Plan the field test in December",
    status: "open",
    openedIn: "2026-11-12",
  });
  return out;
}

interface IssueCache {
  asOf: string | null;
  issues: FakeIssue[];
}

type Payload =
  | { kind: "commit" }
  | {
      kind: "issue";
      itemId: string;
      title: string;
      assignee: string | null;
      due: string | null;
    }
  | { kind: "status"; proposalId: string; number: number; open: boolean; comment: string | null }
  | { kind: "discord"; reference: string };

interface Snapshot {
  draft: Draft;
  cycleId: string;
  github: string | null;
  api: string | null;
  markdown: string;
  chat: string;
  screenshots: number;
}

/** A fault of the fake servers: the network is off, or the request is refused for good. */
type Fault = { offline: true } | { offline: false; message: string };

export function createMockPublishing(deps: PublishDeps): MockPublishing {
  const { cycles, drafts } = deps;
  const stepMs = Math.max(20, deps.stepMs);
  const listeners = new Set<{ cb: (s: OutboxState) => void }>();

  // ----- the fake world -----
  const world = {
    offline: false,
    visibility: "public" as "public" | "private",
    issues: demoIssues(),
    nextIssue: 13,
    discord: [] as FakeDiscordPost[],
    remoteTopics: null as Topic[] | null,
  };
  world.remoteTopics = demoRemoteTopics(cycles.topics("NOMAD"));
  /** Only whether a token is stored, never the token. */
  const tokens = new Set<string>([apiHost(DEFAULT_API)!]);
  /** Only the masked form of a stored webhook, never the address. */
  const webhooks = new Map<string, string>([
    ["cred:discord/nomad", maskWebhook({ host: "discord.com", id: "000000000000000007" })],
  ]);
  const confirmed = new Set<string>();
  const caches = new Map<string, IssueCache>([
    ["team-cycles", { asOf: "2026-11-12", issues: structuredClone(world.issues) }],
  ]);

  // ----- the outbox -----
  const ops: OutboxOp[] = [];
  const payloads = new Map<number, Payload>();
  const snapshots = new Map<string, Snapshot>();
  const offlineGroups = new Set<"github" | "discord">();
  let nextId = 1;
  let timer: ReturnType<typeof setInterval> | null = null;

  const state = (): OutboxState => ({
    ops: structuredClone(ops),
    offline: ops
      .filter((o) => offlineGroups.has(group(o.target)) && o.state === "queued")
      .map((o) => o.target)
      .filter((t, i, all) => all.indexOf(t) === i),
    nowUnixMs: deps.now(),
  });
  const emit = () => listeners.forEach((l) => l.cb(state()));
  const opsOfMeeting = (meetingId: string) => ops.filter((o) => o.meetingId === meetingId);
  const isLocked = (meetingId: string) =>
    opsOfMeeting(meetingId).some((o) => o.state !== "cancelled");

  function pump() {
    if (timer !== null) return;
    timer = setInterval(tick, Math.max(20, stepMs / 2));
  }

  function due(op: OutboxOp, now: number): boolean {
    if (op.state !== "queued") return false;
    if (op.nextAttemptUnixMs !== null && op.nextAttemptUnixMs > now) return false;
    if (offlineGroups.has(group(op.target))) return false;
    if (op.afterId !== null && ops.find((o) => o.id === op.afterId)?.state !== "done") return false;
    // The message links the issues, so it waits for the issue operations of its meeting.
    if (
      op.kind === "post_discord" &&
      ops.some(
        (o) =>
          o.meetingId === op.meetingId &&
          o.target === "issues" &&
          (o.state === "queued" || o.state === "running"),
      )
    )
      return false;
    return true;
  }

  function tick() {
    const running = ops.find((o) => o.state === "running");
    if (running) {
      finish(running);
      return;
    }
    const now = deps.now();
    const next = ops.find((o) => due(o, now));
    if (next) {
      next.state = "running";
      next.updatedUnixMs = now;
      emit();
      return;
    }
    if (!ops.some((o) => o.state === "queued")) {
      if (timer !== null) clearInterval(timer);
      timer = null;
    }
  }

  function settle(op: OutboxOp, patch: Partial<OutboxOp>) {
    Object.assign(op, patch, { updatedUnixMs: deps.now() });
    emit();
  }

  function finish(op: OutboxOp) {
    try {
      const result = perform(op);
      offlineGroups.delete(group(op.target));
      settle(op, { state: "done", result, lastError: null, nextAttemptUnixMs: null });
    } catch (e) {
      const fault = e as Fault;
      if (fault.offline) {
        offlineGroups.add(group(op.target));
        const attempts = op.attempts + 1;
        const wait = Math.min(BACKOFF_MS * 2 ** (attempts - 1), BACKOFF_MAX_MS);
        settle(op, {
          state: "queued",
          attempts,
          lastError: `Could not reach ${group(op.target) === "discord" ? "discord.com" : "api.github.com"}: the network is offline.`,
          nextAttemptUnixMs: deps.now() + wait,
        });
      } else {
        settle(op, {
          state: "failed",
          attempts: op.attempts + 1,
          lastError: fault.message,
          nextAttemptUnixMs: null,
        });
      }
    }
  }

  // ----- what the fake servers do -----
  const refuse = (message: string): never => {
    throw { offline: false, message } satisfies Fault;
  };
  const unreachable = (): never => {
    throw { offline: true } satisfies Fault;
  };

  function perform(op: OutboxOp): OpResult {
    if (world.offline) return unreachable();
    const payload = payloads.get(op.id)!;
    const snap = snapshots.get(op.meetingId)!;
    switch (payload.kind) {
      case "commit":
        return commit(op, snap);
      case "issue":
        return createIssue(op, snap, payload);
      case "status":
        return setIssueState(snap, payload);
      case "discord":
        return post(op, snap, payload.reference);
    }
  }

  const reportUrl = (snap: Snapshot, folder: string) =>
    `${webBase(snap.api)}/${snap.github}/blob/main/${folder}/report.md`;

  function commit(op: OutboxOp, snap: Snapshot): OpResult {
    const repo = cycles.repoOf(snap.cycleId);
    // Folder: `<path>/<cycle>/<date>`, then `-2`, `-3` when a commit has it already.
    const taken = new Set(
      ops
        .filter((o) => o.kind === "commit_meeting" && o.state === "done" && o.id !== op.id)
        .map((o) => o.result?.folder),
    );
    const base = `meetings/${snap.cycleId}/${snap.draft.date}`;
    let folder = base;
    for (let n = 2; taken.has(folder); n += 1) folder = `${base}-${n}`;
    const merge = mergeTopics(snap);
    cycles.setTopics(snap.cycleId, merge.topics);
    const pushed = repo.remote !== null;
    return {
      folder,
      commit: fakeSha(`${op.meetingId}:${folder}`),
      branch: "main",
      pushed,
      already_published: false,
      local: "updated",
      local_note: null,
      report_url: pushed && snap.github ? reportUrl(snap, folder) : null,
      topics: merge.summary,
    };
  }

  /**
   * The `topics.yaml` merge: last write wins per topic. Topics this meeting added or changed
   * replace the remote's; a topic the remote also changed since the last pull is a conflict
   * (ours is kept, both versions are reported). Topics only the remote touched are kept.
   */
  function mergeTopics(snap: Snapshot): {
    topics: Topic[];
    summary: NonNullable<OpResult["topics"]>;
  } {
    const local = cycles.topics(snap.cycleId);
    const remote = world.remoteTopics ?? local;
    const ours = new Map<string, Topic>();
    for (const item of snap.draft.items) {
      if (item.type === "topic" && item.relatesTo === null)
        ours.set(`${snap.draft.meetingId}-${item.id}`, {
          id: `${snap.draft.meetingId}-${item.id}`,
          text: item.text,
          status: "open",
          openedIn: snap.draft.date,
        });
    }
    for (const p of snap.draft.statusProposals ?? []) {
      if (p.decision !== "approved" || p.target.startsWith("#")) continue;
      const base = local.find((t) => t.id === p.target);
      if (!base) continue;
      const rest: Topic = { ...base };
      delete rest.closedIn;
      ours.set(p.target, {
        ...rest,
        status: p.status === "done" ? "closed" : "open",
        ...(p.status === "done" ? { closedIn: snap.draft.date } : {}),
      });
    }
    const conflicts: TopicConflictDetail[] = [];
    const merged = remote.map((t) => {
      const mine = ours.get(t.id);
      if (!mine) return t;
      const before = local.find((x) => x.id === t.id);
      if (before && JSON.stringify(before) !== JSON.stringify(t))
        conflicts.push({
          id: t.id,
          theirs: { text: t.text, status: t.status },
          ours: { text: mine.text, status: mine.status },
        });
      return mine;
    });
    let added = 0;
    for (const [id, t] of ours) {
      if (!remote.some((x) => x.id === id)) {
        merged.push(t);
        added += 1;
      }
    }
    const changed = merged.filter((t) => {
      const r = remote.find((x) => x.id === t.id);
      return r !== undefined && JSON.stringify(r) !== JSON.stringify(t);
    }).length;
    // What the colleague had is now in the local clone too, and the remote has our change.
    world.remoteTopics = structuredClone(merged);
    return {
      topics: merged,
      summary: {
        added,
        changed,
        removed: 0,
        conflicts: conflicts.map((c) => c.id),
        conflict_details: conflicts,
      },
    };
  }

  function createIssue(
    op: OutboxOp,
    snap: Snapshot,
    p: Extract<Payload, { kind: "issue" }>,
  ): OpResult {
    if (!tokens.has(apiHost(snap.api ?? DEFAULT_API) ?? ""))
      return refuse(
        `No GitHub token is stored for ${apiHost(snap.api ?? DEFAULT_API)}: add it in the publishing settings, then retry.`,
      );
    // The marker scan: an issue made from this item exists already (a retry after a lost answer).
    const existing = world.issues.find((i) => i.meeting === op.meetingId && i.item === p.itemId);
    const issue: FakeIssue = existing ?? {
      number: world.nextIssue++,
      title: p.title,
      assignee: p.assignee,
      state: "open",
      meeting: op.meetingId,
      item: p.itemId,
      due: p.due,
    };
    if (!existing) world.issues.push(issue);
    return { number: issue.number, url: issueUrl(snap.github!, issue.number, snap.api) };
  }

  function setIssueState(snap: Snapshot, p: Extract<Payload, { kind: "status" }>): OpResult {
    if (!tokens.has(apiHost(snap.api ?? DEFAULT_API) ?? ""))
      return refuse(
        `No GitHub token is stored for ${apiHost(snap.api ?? DEFAULT_API)}: add it in the publishing settings, then retry.`,
      );
    const issue = world.issues.find((i) => i.number === p.number);
    if (!issue) return refuse(`Issue #${p.number} was not found in ${snap.github}.`);
    issue.state = p.open ? "open" : "closed";
    return { number: issue.number, url: issueUrl(snap.github!, issue.number, snap.api) };
  }

  function post(op: OutboxOp, snap: Snapshot, reference: string): OpResult {
    if (!webhooks.has(reference))
      return refuse(
        "No Discord webhook is stored for this cycle: add it in the publishing settings.",
      );
    const links = ops
      .filter(
        (o) => o.meetingId === op.meetingId && o.kind === "create_issue" && o.state === "done",
      )
      .map((o) => `#${o.result?.number}`);
    const commitOp = ops.find(
      (o) => o.meetingId === op.meetingId && o.kind === "commit_meeting" && o.state === "done",
    );
    const lines = [snap.chat.trimEnd()];
    if (commitOp?.result?.report_url) lines.push("", `Full report: ${commitOp.result.report_url}`);
    if (links.length > 0) lines.push(`Issues: ${links.join(", ")}`);
    world.discord.push({ reference, content: lines.join("\n") });
    return { sent_chunks: 1 };
  }

  // ----- the issues as the clone knows them -----
  function refreshIssues(repoId: string) {
    if (world.offline) return;
    const repo = cycles.listRepos().find((r) => r.id === repoId);
    if (!repo?.githubRepo) return;
    caches.set(repoId, { asOf: MOCK_TODAY, issues: structuredClone(world.issues) });
  }

  const taskOf = (i: FakeIssue, repo: string, api: string | null): CycleTask => ({
    number: i.number,
    title: i.title,
    assignee: i.assignee,
    url: issueUrl(repo, i.number, api),
    meeting: i.meeting,
  });

  // ----- planning -----
  const targetViews = (cycleId: string): PublishTargetView[] => {
    const cycle = cycles.cycle(cycleId);
    const github = cycle?.targets.github?.publish ?? "on-approval";
    const discord = cycle?.targets.discord?.publish ?? null;
    return [
      { target: "github", mode: github, available: github !== "off" },
      { target: "discord", mode: discord, available: discord !== null && discord !== "off" },
    ];
  };

  const parseTargets = (names: string[]): PublishTargetName[] => {
    const out: PublishTargetName[] = [];
    for (const n of names) {
      if (n !== "github" && n !== "discord")
        fail("invalid", `${n} is not a target (github or discord)`);
      if (!out.includes(n)) out.push(n);
    }
    return out;
  };

  function repoInfo(cycleId: string) {
    const repo = cycles.repoOf(cycleId);
    const github = repo.githubRepo ?? null;
    const api = repo.githubApi ?? null;
    const key = github
      ? `${apiHost(api ?? DEFAULT_API) === "api.github.com" ? "github.com" : apiHost(api ?? DEFAULT_API)}/${github}`
      : null;
    const isConfirmed = key !== null && confirmed.has(key);
    // Offline and unconfirmed, the visibility cannot be read.
    const visibility: RepoVisibility = !repo.remote
      ? "private"
      : !github || (world.offline && !isConfirmed)
        ? "unknown"
        : world.visibility;
    return { repo, github, api, key, confirmed: isConfirmed, visibility };
  }

  async function plan(meetingId: string, names: string[] = []): Promise<PublishPlan> {
    const view: DraftView = await drafts.methods.getDraft(meetingId);
    const draft = view.draft;
    const cycleId = draft.cycle;
    const info = repoInfo(cycleId);
    const cycle = cycles.cycle(cycleId);
    const targets = targetViews(cycleId);
    const wanted =
      names.length === 0 ? (["github", "discord"] as PublishTargetName[]) : parseTargets(names);
    const allowed = (t: PublishTargetName) =>
      wanted.includes(t) && targets.find((x) => x.target === t)!.available;
    const issuesOn = cycle?.targets.github?.issues !== false;
    const notes: string[] = [];
    const blocks: PlanBlock[] = [];
    const summary: PlanSummary = {
      commit: false,
      pushes: false,
      files: [],
      issues: [],
      statuses: [],
      topicsAdded: [],
      topicsSet: [],
      discord: false,
      proposalsPending: 0,
    };
    if (allowed("github")) {
      summary.commit = true;
      summary.pushes = info.repo.remote !== null;
      const shots = deps.screenshotCount(meetingId);
      summary.files = [
        "meeting.json",
        "report.md",
        "transcript.md",
        ...Array.from({ length: shots }, (_, i) => `img/${String(i + 1).padStart(4, "0")}.png`),
      ];
      if (info.github && issuesOn) {
        const people = cycle?.participants ?? [];
        for (const item of draft.items.filter((i) => i.type === "task")) {
          const login = people.find((p) => p.id === item.assignee)?.github ?? null;
          summary.issues.push({
            itemId: item.id,
            title: item.text,
            assignee: login,
            due: item.due,
          });
        }
        for (const p of draft.statusProposals ?? []) {
          if (p.decision === "approved" && p.target.startsWith("#"))
            summary.statuses.push({
              proposalId: p.id,
              target: p.target,
              open: p.status === "open",
            });
        }
      } else if (!info.github) {
        notes.push(
          "This repository is not linked to a GitHub repository, so only the files are committed. Set the repository as owner/name in the publishing settings to create issues.",
        );
      }
      for (const item of draft.items) {
        if (item.type === "topic" && item.relatesTo === null)
          summary.topicsAdded.push(`${draft.meetingId}-${item.id}`);
      }
      for (const p of draft.statusProposals ?? []) {
        if (p.decision === "approved" && !p.target.startsWith("#"))
          summary.topicsSet.push({ id: p.target, open: p.status === "open" });
      }
      summary.proposalsPending = (draft.statusProposals ?? []).filter(
        (p) => p.decision === "proposed",
      ).length;
      if (info.repo.remote !== null && !info.confirmed && info.visibility !== "private") {
        blocks.push({
          code: "public_repo_unconfirmed",
          message: `The repository is ${info.visibility}, so what is published is visible to everybody: confirm publishing to it first.`,
          visibility: info.visibility,
        });
      }
    }
    if (allowed("discord")) {
      const reference = cycle?.targets.discord?.webhook ?? "";
      if (!webhooks.has(reference))
        blocks.push({
          code: "discord_webhook_missing",
          message:
            "No Discord webhook is stored for this cycle: add it in the publishing settings.",
          visibility: null,
        });
      summary.discord = true;
    }
    if (!summary.commit && !summary.discord)
      blocks.push({
        code: "nothing_to_publish",
        message: "There is nothing to publish with the rules of this cycle.",
        visibility: null,
      });
    return {
      meetingId,
      cycleId,
      targets,
      summary,
      blocks,
      notes,
      repo: {
        key: info.key,
        github: info.github,
        hasRemote: info.repo.remote !== null,
        visibility: info.visibility,
        confirmed: info.confirmed,
      },
      locked: isLocked(meetingId),
      newDraftPending: view.hasNewDraft,
      ops: structuredClone(opsOfMeeting(meetingId)),
    };
  }

  function queue(
    meetingId: string,
    kind: OpKind,
    target: OpTarget,
    opKey: string,
    label: string,
    payload: Payload,
    afterId: number | null,
  ): { op: OutboxOp; created: boolean } {
    const now = deps.now();
    const existing = ops.find((o) => o.meetingId === meetingId && o.opKey === opKey);
    if (existing) {
      if (existing.state !== "cancelled") return { op: existing, created: false };
      // A cancelled operation is revived: same row, fresh state.
      Object.assign(existing, {
        state: "queued" as OpState,
        attempts: 0,
        lastError: null,
        nextAttemptUnixMs: null,
        result: null,
        label,
        afterId,
        updatedUnixMs: now,
      });
      payloads.set(existing.id, payload);
      return { op: existing, created: true };
    }
    const op: OutboxOp = {
      id: nextId++,
      meetingId,
      target,
      kind,
      opKey,
      label,
      state: "queued",
      attempts: 0,
      nextAttemptUnixMs: null,
      lastError: null,
      result: null,
      afterId,
      createdUnixMs: now,
      updatedUnixMs: now,
    };
    ops.push(op);
    payloads.set(op.id, payload);
    return { op, created: true };
  }

  async function publish(meetingId: string, names: string[]): Promise<PublishResult> {
    const view = await drafts.methods.getDraft(meetingId);
    if (view.hasNewDraft)
      fail(
        "new_draft_pending",
        "A newer draft waits for a decision: accept it or keep yours, then publish.",
      );
    const wanted = parseTargets(names);
    if (wanted.length === 0) fail("invalid", "Choose where to publish.");
    const preview = await plan(meetingId, wanted);
    const block = preview.blocks[0];
    if (block) fail(block.code, block.message);
    // The draft, its report and the chat text are frozen now; later edits cannot change them.
    if (!isLocked(meetingId) || !snapshots.has(meetingId)) {
      const info = repoInfo(view.draft.cycle);
      snapshots.set(meetingId, {
        draft: structuredClone(view.draft),
        cycleId: view.draft.cycle,
        github: info.github,
        api: info.api,
        markdown: view.reportMarkdown,
        chat: await drafts.methods.renderReport(meetingId, "chat"),
        screenshots: deps.screenshotCount(meetingId),
      });
    }
    let queued = 0;
    const count = (r: { created: boolean }) => void (r.created && (queued += 1));
    let commitId: number | null = null;
    if (preview.summary.commit) {
      const r = queue(
        meetingId,
        "commit_meeting",
        "github",
        "commit",
        "Add the report and transcript to the repository",
        { kind: "commit" },
        null,
      );
      count(r);
      commitId = r.op.id;
    }
    for (const i of preview.summary.issues) {
      count(
        queue(
          meetingId,
          "create_issue",
          "issues",
          `issue:${i.itemId}`,
          `Create the issue: ${i.title}`,
          { kind: "issue", itemId: i.itemId, title: i.title, assignee: i.assignee, due: i.due },
          commitId,
        ),
      );
    }
    const proposals = snapshots.get(meetingId)!.draft.statusProposals ?? [];
    for (const s of preview.summary.statuses) {
      const p = proposals.find((x) => x.id === s.proposalId)!;
      const number = Number(s.target.slice(1));
      count(
        queue(
          meetingId,
          "set_issue_state",
          "issues",
          `status:${s.proposalId}`,
          `${s.open ? "Reopen" : "Close"} issue #${number}`,
          { kind: "status", proposalId: s.proposalId, number, open: s.open, comment: p.comment },
          commitId,
        ),
      );
    }
    if (preview.summary.discord) {
      const reference = cycles.cycle(view.draft.cycle)?.targets.discord?.webhook ?? "";
      count(
        queue(
          meetingId,
          "post_discord",
          "discord",
          "discord",
          "Post the summary to Discord",
          { kind: "discord", reference },
          commitId,
        ),
      );
    }
    pump();
    emit();
    return { plan: await plan(meetingId, wanted), queued };
  }

  // ----- secrets -----
  const apiOrDefault = (api?: string): string => {
    const a = (api ?? "").trim();
    return a === "" ? DEFAULT_API : a;
  };
  const hostOf = (api?: string): string =>
    apiHost(apiOrDefault(api)) ?? fail("invalid", "the API address must start with https://");

  const methods: PublishMethods = {
    publishingSecrets: async (): Promise<PublishingSecrets> => {
      const hosts = new Map<string, string>([[apiHost(DEFAULT_API)!, DEFAULT_API]]);
      for (const r of cycles.listRepos()) {
        const host = r.githubApi ? apiHost(r.githubApi) : null;
        if (host && r.githubApi) hosts.set(host, r.githubApi);
      }
      const seen = new Set<string>();
      const targets: PublishingSecrets["targets"] = [];
      for (const c of cycles.listCycles()) {
        const reference = cycles.cycle(c.id)?.targets.discord?.webhook?.trim();
        if (!reference || !DISCORD_REFERENCE.test(reference) || seen.has(reference)) continue;
        seen.add(reference);
        targets.push({
          reference,
          cycleId: c.id,
          kind: "discord",
          set: webhooks.has(reference),
          masked: webhooks.get(reference) ?? null,
        });
      }
      return {
        github: [...hosts].map(([host, api]) => ({ host, api, tokenSet: tokens.has(host) })),
        targets,
      };
    },
    setGithubToken: async (token, api) => {
      const problem = tokenProblem(token);
      if (problem) fail("invalid", problem);
      tokens.add(hostOf(api));
    },
    clearGithubToken: async (api) => void tokens.delete(hostOf(api)),
    testGithubAccess: async (cycleId) => {
      const info = repoInfo(cycleId);
      const host = apiHost(info.api ?? DEFAULT_API);
      const base = {
        ok: false,
        repo: info.github,
        host,
        tokenSet: false,
        login: null,
        visibility: null,
        canPush: null,
        hasIssues: null,
      };
      if (!info.github)
        return {
          ...base,
          message:
            "This repository is not linked to a GitHub repository: its address is not on github.com, so set the repository as owner/name.",
        };
      if (!host || !tokens.has(host))
        return { ...base, message: `No GitHub token is stored for ${host}.` };
      if (world.offline)
        return {
          ...base,
          tokenSet: true,
          message: `Could not reach ${host}: the network is offline.`,
        };
      return {
        ...base,
        ok: true,
        tokenSet: true,
        login: "demo-bot",
        visibility: world.visibility,
        canPush: true,
        hasIssues: true,
        message: "Connected to " + info.github + " as demo-bot",
      };
    },
    setGithubRepo: async (repoId, githubRepo, githubApi) =>
      cycles.setGithubRepo(repoId, githubRepo, githubApi),
    setTargetSecret: async (reference, value) => {
      const ref = reference.trim();
      if (!DISCORD_REFERENCE.test(ref))
        fail("invalid", "A reference is written cred:name (letters, digits, . _ / -).");
      const isWebhook =
        ref.startsWith("cred:discord/") ||
        cycles.listCycles().some((c) => cycles.cycle(c.id)?.targets.discord?.webhook === ref);
      if (isWebhook) {
        const parts = parseWebhook(value);
        if (typeof parts === "string") fail("invalid_webhook", `Not a Discord webhook: ${parts}`);
        webhooks.set(ref, maskWebhook(parts));
        emit();
        return;
      }
      if (value.trim() === "" || hasControlChars(value))
        fail("invalid", "This value is not acceptable.");
      webhooks.set(ref, "value stored");
    },
    clearTargetSecret: async (reference) => {
      webhooks.delete(reference.trim());
      emit();
    },
    testDiscordWebhook: async (reference) => {
      if (!webhooks.has(reference.trim()))
        return { ok: false, message: "No webhook is stored for this reference.", name: null };
      if (world.offline)
        return {
          ok: false,
          message: "Could not reach discord.com: the network is offline.",
          name: null,
        };
      return { ok: true, message: "The webhook exists.", name: "Nomad updates" };
    },

    publishPlan: (meetingId, targets) => plan(meetingId, targets ?? []),
    publishMeeting: (meetingId, targets) => publish(meetingId, targets),
    confirmPublicRepo: async (cycleId): Promise<PublicRepoConfirmation> => {
      const info = repoInfo(cycleId);
      if (info.key) confirmed.add(info.key);
      return { key: info.key, visibility: info.visibility, released: 0 };
    },
    outboxState: async () => state(),
    retryOutboxOp: async (id) => {
      const now = deps.now();
      const retry = (o: OutboxOp) => {
        if (o.state === "failed")
          Object.assign(o, {
            state: "queued",
            attempts: 0,
            lastError: null,
            nextAttemptUnixMs: null,
          });
        else if (o.state === "queued") o.nextAttemptUnixMs = now;
        else fail("invalid", `the operation ${o.id} is not waiting`);
        o.updatedUnixMs = now;
      };
      if (id === undefined) {
        for (const o of ops) if (o.state === "failed" || o.state === "queued") retry(o);
      } else {
        const op = ops.find((o) => o.id === id) ?? fail("invalid", `no operation ${id}`);
        retry(op);
      }
      offlineGroups.clear();
      pump();
      emit();
      return state();
    },
    cancelOutboxOp: async (id) => {
      const op =
        ops.find((o) => o.id === id) ?? fail("not_found", "This operation does not exist.");
      if (op.state === "running") fail("busy", "The operation is running; wait for it to finish.");
      if (op.state === "done" || op.state === "cancelled")
        fail("invalid", "Only a waiting or failed operation can be cancelled.");
      const wasLocked = isLocked(op.meetingId);
      settle(op, { state: "cancelled" });
      if (wasLocked && !isLocked(op.meetingId)) deps.onUnlocked(op.meetingId);
      return state();
    },
    cycleTasks: async (cycleId): Promise<CycleTasks> => {
      const repo = cycles.repoOf(cycleId);
      const cache = caches.get(repo.id) ?? null;
      const github = repo.githubRepo ?? null;
      const api = repo.githubApi ?? null;
      const issues = cache?.issues ?? [];
      return {
        cycleId,
        repo: github,
        asOf: cache?.asOf ?? null,
        open: github
          ? issues.filter((i) => i.state === "open").map((i) => taskOf(i, github, api))
          : [],
        done: github
          ? issues.filter((i) => i.state === "closed").map((i) => taskOf(i, github, api))
          : [],
      };
    },
    onOutboxChanged: (cb): Promise<Unsubscribe> => {
      const entry = { cb };
      listeners.add(entry);
      return Promise.resolve(() => void listeners.delete(entry));
    },
  };

  const hooks: MockHooks = {
    setOffline: (offline) => {
      world.offline = offline;
      if (!offline) {
        // The network is back: what waited for it runs at the next tick.
        offlineGroups.clear();
        const now = deps.now();
        for (const o of ops)
          if (o.state === "queued" && o.lastError !== null) o.nextAttemptUnixMs = now;
        pump();
      }
      emit();
    },
    closeIssue: (n) => {
      const i = world.issues.find((x) => x.number === n);
      if (i) i.state = "closed";
    },
    reopenIssue: (n) => {
      const i = world.issues.find((x) => x.number === n);
      if (i) i.state = "open";
    },
    setVisibility: (v) => void (world.visibility = v),
    issues: () => structuredClone(world.issues),
    discordPosts: () => structuredClone(world.discord),
    nextContext: (cycleId) => {
      const cache = caches.get(cycles.repoOf(cycleId).id);
      return (cache?.issues ?? [])
        .filter((i) => i.meeting !== null && i.state === "open")
        .map((i) => ({ text: i.title, issue: `#${i.number}` }));
    },
    remoteTopics: () => structuredClone(world.remoteTopics ?? []),
  };

  return { methods, hooks, isLocked, refreshIssues };
}
