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
import {
  createMockIntegrations,
  type IntegrationHooks,
  type NotionTask,
} from "./mock-integrations";
import { fail } from "./mock-util";
import {
  type SecretKind,
  apiHost,
  botTokenProblem,
  correctionProblem,
  hasControlChars,
  isDiscordConfigured,
  issueUrl,
  maskBotToken,
  maskNotionToken,
  maskWebhook,
  notionTokenProblem,
  opLocksDraft,
  parseWebhook,
  referenceKindByName,
  tokenProblem,
  webBase,
} from "../publish";
import type {
  Correction,
  CorrectionState,
  CorrectionStep,
  Corrections,
  CycleTask,
  CycleTasks,
  DiscordBotTarget,
  Draft,
  DraftView,
  GithubDevicePoll,
  OpKind,
  OpResult,
  OpState,
  OpTarget,
  OutboxOp,
  OutboxState,
  PlanBlock,
  PlanSummary,
  PlannedTopicView,
  PublicRepoConfirmation,
  PublishPlan,
  PublishResult,
  PublishedCorrections,
  PublishTargetName,
  PublishTargetView,
  PublishingSecrets,
  RepoVisibility,
  Topic,
  TopicConflictDetail,
  TopicDelta,
  TopicsPlan,
} from "./types";

type PublishMethods = Pick<
  Backend,
  | "publishingSecrets"
  | "setGithubToken"
  | "clearGithubToken"
  | "githubConnectStatus"
  | "githubDeviceStart"
  | "githubDevicePoll"
  | "githubDeviceCancel"
  | "testGithubAccess"
  | "setGithubRepo"
  | "setTargetSecret"
  | "clearTargetSecret"
  | "testDiscordWebhook"
  | "testNotionAccess"
  | "notionSchema"
  | "setNotionMapping"
  | "testDiscordBot"
  | "searchDiscordMembers"
  | "listDiscordRoles"
  | "setParticipantDiscord"
  | "reexportNotion"
  | "publishPlan"
  | "publishMeeting"
  | "confirmPublicRepo"
  | "outboxState"
  | "retryOutboxOp"
  | "cancelOutboxOp"
  | "cycleTasks"
  | "listCorrections"
  | "addCorrection"
  | "publishCorrections"
  | "deleteCorrection"
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
export interface MockHooks extends IntegrationHooks {
  /** No target answers while it is on. Turning it off lets waiting operations run again. */
  setOffline(offline: boolean): void;
  /**
   * The GitHub sign-in: whether a client id is configured, and the answers of the next polls
   * (the default is one "pending" and then "connected" as `demo-user`).
   */
  setGithubConnect(options: { configured?: boolean; script?: GithubDevicePoll[] }): void;
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
  /** The plan cannot compare `topics.yaml` with the remote: it names the topics instead. */
  setTopicsUnknown(unknown: boolean): void;
  /** The comments the fake GitHub holds on an issue. */
  issueComments(number: number): string[];
  /** The correction files the fake repository holds, in the order they were added. */
  correctionFiles(): { path: string; text: string }[];
  /**
   * The corrections the next processing of the cycle would be given (written or published),
   * newest first, with the text of the item they are about.
   */
  nextCorrections(
    cycleId: string,
  ): { text: string; item: string | null; issue: string | null; date: string }[];
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
  /** A cycle was modified through an integration helper (`setNotionMapping`, `setParticipantDiscord`). */
  onCyclesChanged?: () => void;
}

const DEFAULT_API = "https://api.github.com";
const CREDENTIAL_REFERENCE = /^cred:[A-Za-z0-9][A-Za-z0-9_./-]*$/;
const BACKOFF_MS = 30_000;
const BACKOFF_MAX_MS = 30 * 60_000;

type OfflineGroup = "github" | "discord" | "notion";

/** Where an operation talks to: GitHub and its issues are one server. */
const group = (t: OpTarget): OfflineGroup =>
  t === "discord" ? "discord" : t === "notion" ? "notion" : "github";

const offlineHost = (g: OfflineGroup): string =>
  g === "discord" ? "discord.com" : g === "notion" ? "api.notion.com" : "api.github.com";

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

/** A correction as the mock keeps it (what `work/corrections.json` holds). */
interface StoredCorrection {
  id: string;
  date: string;
  text: string;
  item: string | null;
  issue: string | null;
}

type Payload =
  | { kind: "commit" }
  | { kind: "correction"; correction: StoredCorrection; meetingDate: string }
  | {
      kind: "issue";
      itemId: string;
      title: string;
      assignee: string | null;
      due: string | null;
    }
  | { kind: "status"; proposalId: string; number: number; open: boolean; comment: string | null }
  | { kind: "discord"; reference: string; bot: DiscordBotTarget | null }
  | { kind: "notion"; itemId: string; task: NotionTask };

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
    comments: [] as { issue: number; text: string }[],
    correctionFiles: [] as { meeting: string; path: string; text: string }[],
    remoteTopics: null as Topic[] | null,
    /** `topics.yaml` cannot be compared (a hook): the plan falls back to the topic names. */
    topicsUnknown: false,
  };
  world.remoteTopics = demoRemoteTopics(cycles.topics("NOMAD"));
  /** Only whether a token is stored, never the token. */
  const tokens = new Set<string>([apiHost(DEFAULT_API)!]);
  /** The fake GitHub device flow: configured or not, the polls left and whether one is running. */
  const deviceFlow = {
    configured: true,
    script: null as GithubDevicePoll[] | null,
    running: false,
  };
  const defaultDeviceScript = (): GithubDevicePoll[] => [
    { state: "pending" },
    { state: "connected", login: "demo-user" },
  ];
  /** Only the masked form of a stored target secret (webhook, bot token, Notion token). */
  const targetSecrets = new Map<string, string>([
    ["cred:discord/nomad", maskWebhook({ host: "discord.com", id: "000000000000000007" })],
  ]);
  const integrations = createMockIntegrations({
    cycles,
    hasSecret: (ref) => targetSecrets.has(ref),
    isOffline: () => world.offline,
  });
  /** Stored Notion page ids by `${meetingId}:${itemId}` (mirrors `external_refs`). */
  const notionRefs = new Map<string, string>();
  /** Stored Discord bot post metadata by `meetingId` (mirrors `external_refs`). */
  const discordPosts = new Map<
    string,
    { channelId: string; threadId: string | null; messageIds: string[]; baseSummary: string }
  >();
  const confirmed = new Set<string>();
  /** The corrections of each meeting, as `work/corrections.json` has them. */
  const corrections = new Map<string, { cycleId: string; list: StoredCorrection[] }>();
  const caches = new Map<string, IssueCache>([
    ["team-cycles", { asOf: "2026-11-12", issues: structuredClone(world.issues) }],
  ]);

  // ----- the outbox -----
  const ops: OutboxOp[] = [];
  const payloads = new Map<number, Payload>();
  const snapshots = new Map<string, Snapshot>();
  const offlineGroups = new Set<OfflineGroup>();
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
  const isLocked = (meetingId: string) => opsOfMeeting(meetingId).some(opLocksDraft);

  function pump() {
    if (timer !== null) return;
    timer = setInterval(tick, Math.max(20, stepMs / 2));
  }

  function due(op: OutboxOp, now: number): boolean {
    if (op.state !== "queued") return false;
    if (op.nextAttemptUnixMs !== null && op.nextAttemptUnixMs > now) return false;
    if (offlineGroups.has(group(op.target))) return false;
    if (op.afterId !== null && ops.find((o) => o.id === op.afterId)?.state !== "done") return false;
    // A comment on the issue of an item waits until that issue exists.
    if (op.kind === "comment_correction") {
      const payload = payloads.get(op.id);
      if (payload?.kind === "correction" && payload.correction.item !== null) {
        const made = ops.find(
          (o) => o.meetingId === op.meetingId && o.opKey === `issue:${payload.correction.item}`,
        );
        if (made && (made.state === "queued" || made.state === "running")) return false;
      }
    }
    // A Discord edit after a correction waits for the original Discord post to finish.
    if (
      op.kind === "edit_discord" &&
      ops.some(
        (o) =>
          o.meetingId === op.meetingId &&
          o.kind === "post_discord" &&
          (o.state === "queued" || o.state === "running"),
      )
    )
      return false;
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
          lastError: `Could not reach ${offlineHost(group(op.target))}: the network is offline.`,
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
      case "notion":
        return exportNotion(op, snap, payload);
      case "discord":
        return payload.bot !== null
          ? postBot(op, snap, payload.bot)
          : post(op, snap, payload.reference);
      case "correction":
        if (op.kind === "commit_correction") return correctionFile(op, snap, payload);
        if (op.kind === "edit_discord") return correctionDiscordEdit(op, snap, payload);
        return correctionComment(op, snap, payload);
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
    const path = cycles.cycle(snap.cycleId)?.targets.github?.path ?? "meetings";
    const base = `${path}/${snap.cycleId}/${snap.draft.date}`;
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
   * (ours is kept, both versions are reported). Topics only the remote touched are kept. Pure:
   * nothing is written, so the plan can show it before the commit does it.
   */
  function topicsMerge(draft: Draft, cycleId: string) {
    const local = cycles.topics(cycleId);
    const remote = world.remoteTopics ?? local;
    const ours = new Map<string, Topic>();
    for (const item of draft.items) {
      if (item.type === "topic" && item.relatesTo === null)
        ours.set(`${draft.meetingId}-${item.id}`, {
          id: `${draft.meetingId}-${item.id}`,
          text: item.text,
          status: "open",
          openedIn: draft.date,
        });
    }
    for (const p of draft.statusProposals ?? []) {
      if (p.decision !== "approved" || p.target.startsWith("#")) continue;
      const base = local.find((t) => t.id === p.target);
      if (!base) continue;
      const rest: Topic = { ...base };
      delete rest.closedIn;
      ours.set(p.target, {
        ...rest,
        status: p.status === "done" ? "closed" : "open",
        ...(p.status === "done" ? { closedIn: draft.date } : {}),
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
    const added: Topic[] = [];
    for (const [id, t] of ours) {
      if (!remote.some((x) => x.id === id)) {
        merged.push(t);
        added.push(t);
      }
    }
    const changed = merged.filter((t) => {
      const r = remote.find((x) => x.id === t.id);
      return r !== undefined && JSON.stringify(r) !== JSON.stringify(t);
    });
    return { remote, merged, conflicts, added, changed };
  }

  function mergeTopics(snap: Snapshot): {
    topics: Topic[];
    summary: NonNullable<OpResult["topics"]>;
  } {
    const m = topicsMerge(snap.draft, snap.cycleId);
    // What the colleague had is now in the local clone too, and the remote has our change.
    world.remoteTopics = structuredClone(m.merged);
    return {
      topics: m.merged,
      summary: {
        added: m.added.length,
        changed: m.changed.length,
        removed: 0,
        conflicts: m.conflicts.map((c) => c.id),
        conflict_details: m.conflicts,
      },
    };
  }

  /** `topics.yaml` before and after for the plan, per topic. */
  function topicsPlan(draft: Draft, cycleId: string, remote: boolean): TopicsPlan {
    const m = topicsMerge(draft, cycleId);
    const view = (t: Topic): PlannedTopicView => ({
      text: t.text,
      status: t.status,
      openedIn: t.openedIn,
      closedIn: t.closedIn ?? null,
    });
    const overrides = (id: string) => m.conflicts.some((c) => c.id === id);
    const changes: TopicDelta[] = [
      ...m.added.map((t): TopicDelta => ({
        id: t.id,
        change: "added",
        before: null,
        after: view(t),
        overridesRemote: overrides(t.id),
      })),
      ...m.changed.map((t): TopicDelta => ({
        id: t.id,
        change: "changed",
        before: view(m.remote.find((x) => x.id === t.id)!),
        after: view(t),
        overridesRemote: overrides(t.id),
      })),
    ];
    return {
      againstRemote: remote,
      changes,
      unchanged: m.merged.length - m.added.length - m.changed.length,
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

  function exportNotion(
    op: OutboxOp,
    snap: Snapshot,
    p: Extract<Payload, { kind: "notion" }>,
  ): OpResult {
    const notion = cycles.cycle(snap.cycleId)?.targets.notion;
    if (!notion)
      return refuse(
        "This project has no Notion target: add one in the project settings, then retry.",
      );
    const commitOp = ops.find(
      (o) => o.meetingId === op.meetingId && o.kind === "commit_meeting" && o.state === "done",
    );
    const task: NotionTask = {
      ...p.task,
      meetingUrl: commitOp?.result?.report_url ?? p.task.meetingUrl,
    };
    const refKey = `${op.meetingId}:${p.itemId}`;
    const known = notionRefs.get(refKey) ?? null;
    const res = integrations.notion.export(notion, task, known);
    if ("error" in res) return refuse(res.error);
    notionRefs.set(refKey, res.pageId);
    return {
      page_id: res.pageId,
      url: res.outcome === "removed" ? undefined : res.url,
      created: res.outcome === "created",
      updated: res.outcome === "updated",
      removed: res.outcome === "removed",
      duplicates: res.duplicates,
    };
  }

  function post(op: OutboxOp, snap: Snapshot, reference: string): OpResult {
    if (!targetSecrets.has(reference))
      return refuse(
        "No Discord webhook is stored for this project: add it in the publishing settings.",
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

  function postBot(op: OutboxOp, snap: Snapshot, bot: DiscordBotTarget): OpResult {
    const refused = integrations.bot.refusal(bot);
    if (refused) return refuse(refused);
    const links = ops
      .filter(
        (o) => o.meetingId === op.meetingId && o.kind === "create_issue" && o.state === "done",
      )
      .map((o) => `#${o.result?.number}`);
    const commitOp = ops.find(
      (o) => o.meetingId === op.meetingId && o.kind === "commit_meeting" && o.state === "done",
    );
    const summaryLines = [snap.chat.trimEnd()];
    if (commitOp?.result?.report_url)
      summaryLines.push("", `Full report: ${commitOp.result.report_url}`);
    if (!bot.thread && links.length > 0) summaryLines.push(`Issues: ${links.join(", ")}`);
    const baseSummary = summaryLines.join("\n");
    const first = integrations.bot.post(bot.channel, baseSummary, null);
    const messageIds = [first.id];
    let threadId: string | null = null;
    if (bot.thread) {
      const title = `Meeting ${snap.draft.cycle} ${snap.draft.date}`;
      const thread = integrations.bot.openThread(bot.channel, first.id, title);
      threadId = thread.id;
      const details: string[] = [];
      if (links.length > 0) details.push(`Issues: ${links.join(", ")}`);
      if (snap.screenshots > 0)
        details.push(
          snap.screenshots === 1
            ? "1 screenshot attached."
            : `${snap.screenshots} screenshots attached.`,
        );
      if (details.length > 0) {
        const follow = integrations.bot.post(bot.channel, details.join("\n"), threadId);
        messageIds.push(follow.id);
      }
    }
    discordPosts.set(op.meetingId, {
      channelId: bot.channel,
      threadId,
      messageIds,
      baseSummary,
    });
    return {
      sent_chunks: messageIds.length,
      channel_id: bot.channel,
      thread_id: threadId,
      message_ids: messageIds,
    };
  }

  function correctionDiscordEdit(
    op: OutboxOp,
    snap: Snapshot,
    p: Extract<Payload, { kind: "correction" }>,
  ): OpResult {
    const bot = cycles.cycle(snap.cycleId)?.targets.discord?.bot;
    if (!bot)
      return { edited: false, replied: false, skipped: "Discord is not configured in bot mode" };
    const stored = discordPosts.get(op.meetingId);
    if (!stored || stored.messageIds.length === 0)
      return {
        edited: false,
        replied: false,
        skipped: "no Discord post is recorded for this meeting",
      };
    const refused = integrations.bot.refusal(bot);
    if (refused) return refuse(refused);
    const all = corrections.get(op.meetingId)?.list ?? [p.correction];
    const notes = all.map((c) => `- (${c.date}) ${c.text}`).join("\n");
    const updated = `${stored.baseSummary}\n\nCorrections:\n${notes}`;
    const ok = integrations.bot.edit(stored.messageIds[0], updated);
    if (!ok)
      return {
        edited: false,
        replied: false,
        skipped: "the Discord message was deleted",
      };
    if (stored.threadId) {
      const reply = `Correction (${p.correction.date}): ${p.correction.text}`;
      integrations.bot.post(stored.channelId, reply, stored.threadId);
      return { edited: true, replied: true, thread_id: stored.threadId };
    }
    return { edited: true, replied: false };
  }

  function correctionFile(
    op: OutboxOp,
    snap: Snapshot,
    p: Extract<Payload, { kind: "correction" }>,
  ): OpResult {
    const commitOp = ops.find(
      (o) => o.meetingId === op.meetingId && o.kind === "commit_meeting" && o.state === "done",
    );
    const folder = commitOp?.result?.folder;
    if (!folder)
      return refuse("The meeting is not in the repository yet: publish it first, then retry.");
    const done = ops.filter(
      (o) =>
        o.meetingId === op.meetingId &&
        o.kind === "commit_correction" &&
        o.state === "done" &&
        o.id !== op.id,
    ).length;
    const file = done === 0 ? "corrections.md" : `corrections-${done + 1}.md`;
    const path = `${folder}/${file}`;
    world.correctionFiles.push({ meeting: op.meetingId, path, text: p.correction.text });
    const pushed = cycles.repoOf(snap.cycleId).remote !== null;
    return {
      file,
      path,
      commit: fakeSha(`${op.meetingId}:${path}`),
      branch: "main",
      pushed,
      already_published: false,
      local: "updated",
      local_note: null,
      url:
        pushed && snap.github ? `${webBase(snap.api)}/${snap.github}/blob/main/${path}` : undefined,
    };
  }

  function correctionComment(
    op: OutboxOp,
    snap: Snapshot,
    p: Extract<Payload, { kind: "correction" }>,
  ): OpResult {
    if (!tokens.has(apiHost(snap.api ?? DEFAULT_API) ?? ""))
      return refuse(
        `No GitHub token is stored for ${apiHost(snap.api ?? DEFAULT_API)}: add it in the publishing settings, then retry.`,
      );
    let number: number | null = p.correction.issue ? Number(p.correction.issue.slice(1)) : null;
    if (number === null && p.correction.item !== null) {
      const made = ops.find(
        (o) =>
          o.meetingId === op.meetingId &&
          o.opKey === `issue:${p.correction.item}` &&
          o.state === "done",
      );
      number = made?.result?.number ?? null;
    }
    if (number === null) return { skipped: "no issue was made for this item" };
    const issue = world.issues.find((i) => i.number === number);
    if (!issue) return refuse(`Issue #${number} was not found in ${snap.github}.`);
    const marker = `correction ${op.meetingId} ${p.correction.id}`;
    const already = world.comments.some((c) => c.issue === number && c.text.includes(marker));
    if (!already) world.comments.push({ issue: number, text: `${p.correction.text}\n${marker}` });
    return { number, commented: !already };
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
  /** A remote or a linked GitHub repository: without one nothing can go to GitHub. */
  const isGithubConfigured = (cycleId: string) => {
    const info = repoInfo(cycleId);
    return Boolean(info.repo.remote || info.github);
  };
  const targetViews = (cycleId: string): PublishTargetView[] => {
    const cycle = cycles.cycle(cycleId);
    const githubConfigured = isGithubConfigured(cycleId);
    const discordConfigured = isDiscordConfigured(
      cycle?.targets.discord,
      targetSecrets.has(cycle?.targets.discord?.webhook?.trim() ?? ""),
    );
    const notionConfigured = Boolean(cycle?.targets.notion?.database?.trim());
    const github = cycle?.targets.github?.publish ?? "on-approval";
    const discord = cycle?.targets.discord?.publish ?? null;
    const notion = cycle?.targets.notion?.publish ?? null;
    return [
      {
        target: "github",
        mode: github,
        available: githubConfigured && github !== "off",
        configured: githubConfigured,
      },
      {
        target: "discord",
        mode: discord,
        available: discordConfigured && discord !== null && discord !== "off",
        configured: discordConfigured,
      },
      {
        target: "notion",
        mode: notion,
        available: notionConfigured && notion !== null && notion !== "off",
        configured: notionConfigured,
      },
    ];
  };

  const parseTargets = (names: string[]): PublishTargetName[] => {
    const out: PublishTargetName[] = [];
    for (const n of names) {
      if (n !== "github" && n !== "discord" && n !== "notion")
        fail("invalid", `${n} is not a target (github, discord or notion)`);
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
      names.length === 0
        ? (["github", "discord", "notion"] as PublishTargetName[])
        : parseTargets(names);
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
      discordMode: null,
      discordThread: false,
      notion: [],
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
          if (item.assignee === null && item.assigneeName?.trim()) {
            notes.push(
              `task ${item.id} is assigned to "${item.assigneeName.trim()}" (not in project people): the GitHub issue will be created unassigned`,
            );
          }
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
        if (item.type === "topic" && item.relatesTo === null) summary.topicsAdded.push(item.text);
      }
      for (const p of draft.statusProposals ?? []) {
        if (p.decision === "approved" && !p.target.startsWith("#"))
          summary.topicsSet.push({ id: p.target, open: p.status === "open" });
      }
      if (summary.topicsAdded.length > 0 || summary.topicsSet.length > 0) {
        if (world.topicsUnknown) {
          notes.push(
            "topics.yaml could not be compared with the repository: the topics are listed by name only",
          );
        } else {
          summary.topics = topicsPlan(draft, cycleId, info.repo.remote !== null);
          if (info.repo.remote === null)
            notes.push(
              "topics.yaml is compared with this clone only: the repository has no remote.",
            );
        }
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
    if (allowed("notion")) {
      const notion = cycle?.targets.notion;
      if (notion) {
        if (!targetSecrets.has(notion.token))
          blocks.push({
            code: "notion_token_missing",
            message:
              "No Notion integration token is stored for this project: add it in the publishing settings.",
            visibility: null,
          });
        for (const item of draft.items.filter((i) => i.type === "task")) {
          const existingId = notionRefs.get(`${meetingId}:${item.id}`) ?? null;
          summary.notion.push({
            itemId: item.id,
            title: item.text,
            existingUrl: existingId ? integrations.notion.pageUrl(existingId) : null,
          });
        }
      }
    }
    if (allowed("discord")) {
      const discord = cycle?.targets.discord;
      if (discord?.bot) {
        if (!targetSecrets.has(discord.bot.token))
          blocks.push({
            code: "discord_bot_token_missing",
            message:
              "No Discord bot token is stored for this project: add it in the publishing settings.",
            visibility: null,
          });
        summary.discord = true;
        summary.discordMode = "bot";
        summary.discordThread = discord.bot.thread;
      } else {
        summary.discord = true;
        summary.discordMode = "webhook";
        summary.discordThread = false;
      }
    }
    if (!summary.commit && !summary.discord && summary.notion.length === 0)
      blocks.push({
        code: "nothing_to_publish",
        message: "There is nothing to publish with the rules of this project.",
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

  // ----- corrections -----
  const githubRule = (cycleId: string) =>
    cycles.cycle(cycleId)?.targets.github?.publish ?? "on-approval";

  function correctionsView(meetingId: string): Corrections {
    const entry = corrections.get(meetingId);
    const cycleId = entry?.cycleId ?? "";
    const mine = opsOfMeeting(meetingId);
    const find = (key: string) => mine.find((o) => o.opKey === key && o.state !== "cancelled");
    const step = (o: OutboxOp | undefined): CorrectionStep | null =>
      o
        ? {
            opId: o.id,
            state: o.state === "cancelled" ? "queued" : o.state,
            attempts: o.attempts,
            nextAttemptUnixMs: o.nextAttemptUnixMs,
            lastError: o.lastError,
            result: o.result,
          }
        : null;
    const list = (entry?.list ?? []).map((c): Correction => {
      const file = find(`correction:${c.id}`);
      const comment = find(`correction-issue:${c.id}`);
      const discordEdit = find(`discord-edit:${c.id}`);
      const primary = file ?? discordEdit;
      const state: CorrectionState = !primary
        ? "local"
        : primary.state === "done"
          ? "published"
          : primary.state === "failed"
            ? "failed"
            : primary.state === "running"
              ? "running"
              : "queued";
      return {
        ...c,
        state,
        file: step(file),
        comment: step(comment),
        discordEdit: step(discordEdit),
        fileName: state === "published" ? (file?.result?.file ?? null) : null,
        url: state === "published" ? (file?.result?.url ?? null) : null,
        canDelete: !file && !comment && !discordEdit,
      };
    });
    return {
      meetingId,
      cycleId,
      locked: isLocked(meetingId),
      inRepository: mine.some((o) => o.kind === "commit_meeting" && o.state !== "cancelled"),
      rule: githubRule(cycleId),
      corrections: list,
    };
  }

  async function listCorrections(meetingId: string): Promise<Corrections> {
    const view = await drafts.methods.getDraft(meetingId);
    if (!corrections.has(meetingId))
      corrections.set(meetingId, { cycleId: view.draft.cycle, list: [] });
    return correctionsView(meetingId);
  }

  /** Plans and queues one correction, with the checks of the real backend. */
  function queueCorrection(meetingId: string, c: StoredCorrection, explicit: boolean): number {
    const entry = corrections.get(meetingId)!;
    const cycleId = entry.cycleId;
    const rule = githubRule(cycleId);
    if (rule === "off" || !isGithubConfigured(cycleId)) {
      if (explicit)
        fail("nothing_to_publish", "There is nothing to publish with the rules of this project.");
      return 0;
    }
    if (!explicit && rule !== "auto") return 0;
    const mine = opsOfMeeting(meetingId);
    const commit = mine.find((o) => o.kind === "commit_meeting" && o.state !== "cancelled");
    if (!commit)
      fail(
        "not_in_repository",
        "The meeting is not in the repository: publish it to GitHub first, then its corrections can follow.",
      );
    const info = repoInfo(cycleId);
    if (info.repo.remote !== null && !info.confirmed && info.visibility !== "private")
      fail(
        "public_repo_unconfirmed",
        `The repository is ${info.visibility}, so what is published is visible to everybody: confirm publishing to it first.`,
      );
    const snap = snapshots.get(meetingId)!;
    const payload: Payload = { kind: "correction", correction: c, meetingDate: snap.draft.date };
    let queued = 0;
    const count = (r: { created: boolean }) => void (r.created && (queued += 1));
    const file = queue(
      meetingId,
      "commit_correction",
      "github",
      `correction:${c.id}`,
      `Add the correction ${c.id} to the repository`,
      payload,
      commit.id,
    );
    count(file);
    const about = c.issue ?? c.item;
    if (about && info.github && cycles.cycle(cycleId)?.targets.github?.issues !== false)
      count(
        queue(
          meetingId,
          "comment_correction",
          "issues",
          `correction-issue:${c.id}`,
          "Comment the correction on its issue",
          payload,
          file.op.id,
        ),
      );
    const bot = cycles.cycle(cycleId)?.targets.discord?.bot;
    const posted = mine.find((o) => o.kind === "post_discord" && o.state !== "cancelled");
    if (bot && bot.editOnCorrection && (posted || discordPosts.has(meetingId))) {
      count(
        queue(
          meetingId,
          "edit_discord",
          "discord",
          `discord-edit:${c.id}`,
          "Update the Discord summary with the correction",
          payload,
          file.op.id,
        ),
      );
    }
    if (queued > 0) {
      pump();
      emit();
    }
    return queued;
  }

  async function addCorrection(
    meetingId: string,
    text: string,
    item?: string,
    issue?: string,
    date?: string,
  ): Promise<Corrections> {
    const view = await drafts.methods.getDraft(meetingId);
    if (!isLocked(meetingId))
      fail(
        "not_published",
        "Only a published meeting has corrections: edit the draft of this one instead.",
      );
    const entry =
      corrections.get(meetingId) ??
      (() => {
        const e = { cycleId: view.draft.cycle, list: [] as StoredCorrection[] };
        corrections.set(meetingId, e);
        return e;
      })();
    const itemId = item?.trim() || null;
    if (itemId !== null && !view.draft.items.some((i) => i.id === itemId))
      fail("not_found", `The meeting has no item ${itemId}.`);
    const issueText = issue?.trim().replace(/^#/, "") ?? "";
    if (issueText !== "" && !/^[1-9]\d*$/.test(issueText))
      fail("invalid", `${issue} is not an issue number (use #12).`);
    const day = date?.trim() || MOCK_TODAY;
    const problem = correctionProblem(text, day);
    if (problem) fail("invalid", problem);
    const highest = Math.max(0, ...entry.list.map((c) => Number(c.id.slice(1))));
    const correction: StoredCorrection = {
      id: `c${highest + 1}`,
      date: day,
      text: text.trim(),
      item: itemId,
      issue: issueText === "" ? null : `#${issueText}`,
    };
    entry.list.push(correction);
    if (githubRule(entry.cycleId) === "auto") {
      try {
        queueCorrection(meetingId, correction, false);
      } catch {
        // A write that cannot go out yet is not an error of writing it.
      }
    }
    return correctionsView(meetingId);
  }

  async function publishCorrections(
    meetingId: string,
    ids: string[] = [],
  ): Promise<PublishedCorrections> {
    await listCorrections(meetingId);
    const entry = corrections.get(meetingId)!;
    const view = correctionsView(meetingId);
    const wanted = entry.list.filter((c) =>
      ids.length === 0
        ? view.corrections.some((v) => v.id === c.id && v.state === "local")
        : ids.includes(c.id),
    );
    if (wanted.length === 0)
      fail(
        ids.length === 0 ? "nothing_to_publish" : "not_found",
        ids.length === 0
          ? "Every correction is queued or published already."
          : "No such correction.",
      );
    let queued = 0;
    for (const c of wanted) queued += queueCorrection(meetingId, c, true);
    return { corrections: correctionsView(meetingId), queued, notes: [] };
  }

  async function deleteCorrection(meetingId: string, id: string): Promise<Corrections> {
    await listCorrections(meetingId);
    const entry = corrections.get(meetingId)!;
    const shown = correctionsView(meetingId).corrections.find((c) => c.id === id);
    if (!shown) fail("not_found", "No such correction.");
    if (!shown!.canDelete)
      fail(
        "correction_queued",
        "This correction was queued for publishing: it is part of the record and stays.",
      );
    entry.list = entry.list.filter((c) => c.id !== id);
    return correctionsView(meetingId);
  }

  function queue(
    meetingId: string,
    kind: OpKind,
    target: OpTarget,
    opKey: string,
    label: string,
    payload: Payload,
    afterId: number | null,
    forceRequeue = false,
  ): { op: OutboxOp; created: boolean } {
    const now = deps.now();
    const existing = ops.find((o) => o.meetingId === meetingId && o.opKey === opKey);
    if (existing) {
      if (existing.state !== "cancelled" && !forceRequeue) return { op: existing, created: false };
      if (existing.state === "running") return { op: existing, created: false };
      // A cancelled operation (or a Notion re-export) is revived: same row, fresh state.
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

  function buildNotionTask(snap: Snapshot, itemId: string): NotionTask | null {
    const item = snap.draft.items.find((i) => i.id === itemId && i.type === "task");
    if (!item) return null;
    const cycle = cycles.cycle(snap.cycleId);
    const assignee =
      item.assignee !== null
        ? (cycle?.participants.find((p) => p.id === item.assignee)?.name ?? item.assignee)
        : null;
    return {
      key: `bbb:v1:${snap.cycleId}:${snap.draft.meetingId}:${item.id}`,
      title: item.text,
      assignee,
      due: item.due,
      meetingUrl: null,
      cycleName: cycle?.name ?? snap.cycleId,
    };
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
    const snap = snapshots.get(meetingId)!;
    let queued = 0;
    const count = (r: { created: boolean }) => void (r.created && (queued += 1));
    let commitId: number | null =
      ops.find(
        (o) => o.meetingId === meetingId && o.kind === "commit_meeting" && o.state !== "cancelled",
      )?.id ?? null;
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
    const proposals = snap.draft.statusProposals ?? [];
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
    for (const n of preview.summary.notion) {
      const task = buildNotionTask(snap, n.itemId);
      if (!task) continue;
      count(
        queue(
          meetingId,
          "export_notion",
          "notion",
          `notion:${n.itemId}`,
          `Export to Notion: ${n.title}`,
          { kind: "notion", itemId: n.itemId, task },
          commitId,
        ),
      );
    }
    if (preview.summary.discord) {
      const discord = cycles.cycle(view.draft.cycle)?.targets.discord;
      const reference = discord?.webhook ?? "";
      const bot = discord?.bot ? structuredClone(discord.bot) : null;
      count(
        queue(
          meetingId,
          "post_discord",
          "discord",
          "discord",
          bot ? "Post the summary to Discord (bot)" : "Post the summary to Discord",
          { kind: "discord", reference, bot },
          commitId,
        ),
      );
    }
    pump();
    emit();
    return { plan: await plan(meetingId, wanted), queued };
  }

  async function reexportNotion(meetingId: string): Promise<PublishResult> {
    if (!isLocked(meetingId))
      fail("not_published", "Publish the meeting before re-exporting tasks to Notion.");
    const view = await drafts.methods.getDraft(meetingId);
    const cycle = cycles.cycle(view.draft.cycle);
    if (!cycle?.targets.notion)
      fail(
        "no_target",
        "This project has no Notion target: add one in the project settings first.",
      );
    const preview = await plan(meetingId, ["notion"]);
    const block = preview.blocks[0];
    if (block) fail(block.code, block.message);
    if (!snapshots.has(meetingId)) {
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
    const snap = snapshots.get(meetingId)!;
    const commitId =
      ops.find(
        (o) => o.meetingId === meetingId && o.kind === "commit_meeting" && o.state !== "cancelled",
      )?.id ?? null;
    let queued = 0;
    for (const n of preview.summary.notion) {
      const task = buildNotionTask(snap, n.itemId);
      if (!task) continue;
      const r = queue(
        meetingId,
        "export_notion",
        "notion",
        `notion:${n.itemId}`,
        `Export to Notion: ${n.title}`,
        { kind: "notion", itemId: n.itemId, task },
        commitId,
        true,
      );
      if (r.created) queued += 1;
    }
    pump();
    emit();
    return { plan: await plan(meetingId, ["notion"]), queued };
  }

  // ----- secrets -----
  const apiOrDefault = (api?: string): string => {
    const a = (api ?? "").trim();
    return a === "" ? DEFAULT_API : a;
  };
  const hostOf = (api?: string): string =>
    apiHost(apiOrDefault(api)) ?? fail("invalid", "the API address must start with https://");

  function secretKindOf(ref: string): SecretKind | null {
    for (const c of cycles.listCycles()) {
      const t = cycles.cycle(c.id)?.targets;
      if (t?.discord?.webhook?.trim() === ref) return "discord";
      if (t?.discord?.bot?.token.trim() === ref) return "discord-bot";
      if (t?.notion?.token.trim() === ref) return "notion";
    }
    return referenceKindByName(ref);
  }

  const methods: PublishMethods = {
    publishingSecrets: async (): Promise<PublishingSecrets> => {
      const hosts = new Map<string, string>([[apiHost(DEFAULT_API)!, DEFAULT_API]]);
      for (const r of cycles.listRepos()) {
        const host = r.githubApi ? apiHost(r.githubApi) : null;
        if (host && r.githubApi) hosts.set(host, r.githubApi);
      }
      const seen = new Set<string>();
      const targets: PublishingSecrets["targets"] = [];
      const addTarget = (reference: string | undefined, cycleId: string, kind: SecretKind) => {
        const ref = reference?.trim();
        if (!ref || !CREDENTIAL_REFERENCE.test(ref) || seen.has(ref)) return;
        seen.add(ref);
        targets.push({
          reference: ref,
          cycleId,
          kind,
          set: targetSecrets.has(ref),
          masked: targetSecrets.get(ref) ?? null,
        });
      };
      for (const c of cycles.listCycles()) {
        const t = cycles.cycle(c.id)?.targets;
        addTarget(t?.discord?.webhook, c.id, "discord");
        addTarget(t?.discord?.bot?.token, c.id, "discord-bot");
        addTarget(t?.notion?.token, c.id, "notion");
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
    githubConnectStatus: async () => ({ configured: deviceFlow.configured }),
    githubDeviceStart: async () => {
      if (!deviceFlow.configured)
        fail(
          "not_configured",
          "GitHub sign-in is not configured in this build: use a token instead",
        );
      if (world.offline) fail("github", "Could not reach github.com: the network is offline.");
      deviceFlow.running = true;
      deviceFlow.script ??= defaultDeviceScript();
      return {
        userCode: "WDJB-MJHT",
        verificationUri: "https://github.com/login/device",
        interval: 1,
        expiresIn: 900,
      };
    },
    githubDevicePoll: async (): Promise<GithubDevicePoll> => {
      if (!deviceFlow.running)
        fail("not_found", "no GitHub connection is in progress: start again");
      const next = deviceFlow.script?.shift() ?? { state: "pending" as const };
      if (next.state === "connected") tokens.add(apiHost(DEFAULT_API)!);
      if (next.state === "connected" || next.state === "denied" || next.state === "expired") {
        deviceFlow.running = false;
        deviceFlow.script = null;
      }
      return next;
    },
    githubDeviceCancel: async () => {
      deviceFlow.running = false;
      deviceFlow.script = null;
    },
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
      if (!CREDENTIAL_REFERENCE.test(ref))
        fail("invalid", "A reference is written cred:name (letters, digits, . _ / -).");
      const kind = secretKindOf(ref);
      if (kind === "discord") {
        const parts = parseWebhook(value);
        if (typeof parts === "string") fail("invalid_webhook", `Not a Discord webhook: ${parts}`);
        targetSecrets.set(ref, maskWebhook(parts));
        emit();
        return;
      }
      if (kind === "discord-bot") {
        const problem = botTokenProblem(value);
        if (problem) fail("invalid_bot_token", problem);
        targetSecrets.set(ref, maskBotToken(value));
        emit();
        return;
      }
      if (kind === "notion") {
        const problem = notionTokenProblem(value);
        if (problem) fail("invalid_notion_token", problem);
        targetSecrets.set(ref, maskNotionToken(value));
        emit();
        return;
      }
      if (value.trim() === "" || hasControlChars(value))
        fail("invalid", "This value is not acceptable.");
      targetSecrets.set(ref, "value stored");
      emit();
    },
    clearTargetSecret: async (reference) => {
      targetSecrets.delete(reference.trim());
      emit();
    },
    testDiscordWebhook: async (reference) => {
      if (!targetSecrets.has(reference.trim()))
        return { ok: false, message: "No webhook is stored for this reference.", name: null };
      if (world.offline)
        return {
          ok: false,
          message: "Could not reach discord.com: the network is offline.",
          name: null,
        };
      return { ok: true, message: "The webhook exists.", name: "Nomad updates" };
    },
    ...integrations.methods,
    setNotionMapping: async (cycleId, dataSource, properties) => {
      const saved = await integrations.methods.setNotionMapping(cycleId, dataSource, properties);
      deps.onCyclesChanged?.();
      return saved;
    },
    setParticipantDiscord: async (cycleId, participantId, discord) => {
      const saved = await integrations.methods.setParticipantDiscord(
        cycleId,
        participantId,
        discord,
      );
      deps.onCyclesChanged?.();
      return saved;
    },
    reexportNotion,

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
      // Only a waiting or failed operation gets here, and those never lock the draft.
      settle(op, { state: "cancelled" });
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
    listCorrections,
    addCorrection,
    publishCorrections,
    deleteCorrection,
    onOutboxChanged: (cb): Promise<Unsubscribe> => {
      const entry = { cb };
      listeners.add(entry);
      return Promise.resolve(() => void listeners.delete(entry));
    },
  };

  const hooks: MockHooks = {
    ...integrations.hooks,
    setGithubConnect: ({ configured, script }) => {
      if (configured !== undefined) deviceFlow.configured = configured;
      if (script) deviceFlow.script = structuredClone(script);
    },
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
    setTopicsUnknown: (unknown) => void (world.topicsUnknown = unknown),
    issueComments: (n) => world.comments.filter((c) => c.issue === n).map((c) => c.text),
    correctionFiles: () => world.correctionFiles.map(({ path, text }) => ({ path, text })),
    nextCorrections: (cycleId) => {
      const out: { text: string; item: string | null; issue: string | null; date: string }[] = [];
      for (const [meetingId, entry] of corrections) {
        if (entry.cycleId !== cycleId) continue;
        const items = snapshots.get(meetingId)?.draft.items ?? [];
        for (const c of entry.list)
          out.push({
            text: c.text,
            item: items.find((i) => i.id === c.item)?.text ?? null,
            issue: c.issue,
            date: c.date,
          });
      }
      return out.sort((a, b) => b.date.localeCompare(a.date));
    },
  };

  return { methods, hooks, isLocked, refreshIssues };
}
