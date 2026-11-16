import type {
  Correction,
  CorrectionState,
  CycleTask,
  CycleTasks,
  OpResult,
  OpState,
  OutboxOp,
  OutboxState,
  PlanBlock,
  PlanBlockCode,
  PlanSummary,
  PlannedTopicView,
  ProposedStatus,
  PublishMode,
  PublishPlan,
  PublishTargetName,
  StatusProposal,
  Topic,
  TopicConflictDetail,
  TopicVersion,
  TopicsPlan,
} from "./ipc/types";

// ----- labels -------------------------------------------------------------------------------

export const TARGET_LABELS: Record<PublishTargetName, string> = {
  github: "GitHub (report, transcript, issues)",
  discord: "Discord (summary)",
};

/** The positions of the rule switch, from the safest. */
export const RULE_ORDER: PublishMode[] = ["off", "on-approval", "auto"];

/**
 * The rule a cycle gets for a target it does not state (ADR 0019). The backend decides it, in
 * `DEFAULT_GITHUB_PUBLISH` and `DEFAULT_DISCORD_PUBLISH`; this is only what is shown before the
 * first choice.
 */
export const DEFAULT_PUBLISH_RULE: Record<PublishTargetName, PublishMode> = {
  github: "on-approval",
  discord: "on-approval",
};

export const RULE_SHORT: Record<PublishMode, string> = {
  off: "Off",
  "on-approval": "On approval",
  auto: "Automatic",
};

export const RULE_HELP: Record<PublishMode, string> = {
  off: "Nothing is ever published to this target, not even when you press Publish.",
  "on-approval":
    "Nothing leaves this computer until you have looked at the draft and press Publish. This is what is used until you choose another rule.",
  auto: "Published as soon as the draft is complete, without a click. It cannot be taken back, and the people it names are notified. A public or unconfirmed repository still waits for your confirmation.",
};

/**
 * What no rule changes: status changes are written back to issues and topics only when you
 * approved them and press Publish.
 */
export const STATUS_WRITE_BACK_NOTE =
  "Status changes are never written back automatically, with any rule: only those you approved, and only when you press Publish.";

/** Extra warning under a target set to Automatic, or null for the other rules. */
export function autoWarning(target: PublishTargetName, mode: PublishMode): string | null {
  if (mode !== "auto") return null;
  return target === "github"
    ? "Automatic: the report, the transcript, the screenshots and the issues are pushed and created as soon as the draft is complete. They cannot be taken back, and assignees are notified."
    : "Automatic: the message is posted as soon as the draft is complete. It cannot be taken back, and the people it mentions are notified.";
}

export const OP_STATE_LABELS: Record<OpState, string> = {
  queued: "Waiting",
  running: "Running",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

export const OP_TARGET_LABELS = { github: "GitHub", issues: "GitHub issues", discord: "Discord" };

/** The class of the badge of an operation state, reusing the colours of the processing jobs. */
export function opBadgeClass(state: OpState): string {
  const cls = {
    queued: "badge-job-queued",
    running: "badge-job-running",
    done: "badge-job-done",
    failed: "badge-job-failed",
    cancelled: "badge-excluded",
  }[state];
  return `badge ${cls}`;
}

// ----- secrets ------------------------------------------------------------------------------

/** Whether a text has a control character (line breaks and tabs included). */
export function hasControlChars(text: string): boolean {
  for (const c of text) if (c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127) return true;
  return false;
}

/** Why a pasted GitHub token cannot be stored, or null. The value is never part of the message. */
export function tokenProblem(token: string): string | null {
  const t = token.trim();
  if (t === "") return "Enter the token.";
  if (t.length > 1024 || /\s/.test(t) || hasControlChars(t))
    return "This does not look like a token: it has spaces or line breaks in it.";
  return null;
}

const DISCORD_HOSTS = ["discord.com", "discordapp.com", "canary.discord.com", "ptb.discord.com"];
const LOOPBACK = ["localhost", "127.0.0.1", "[::1]"];

export interface WebhookParts {
  host: string;
  id: string;
}

/**
 * Checks a webhook address the way the application does before it stores one: https on a
 * Discord host (or plain http on this machine for a test server), no credentials, query or port,
 * and a path `/api/webhooks/<id>/<token>`. The message never repeats the value.
 */
export function parseWebhook(value: string): WebhookParts | string {
  const url = value.trim();
  const at = url.indexOf("://");
  if (at < 0) return "This is not a web address (expected https://discord.com/api/webhooks/...).";
  const scheme = url.slice(0, at).toLowerCase();
  const rest = url.slice(at + 3);
  if (scheme !== "https" && scheme !== "http") return "The address must start with https://.";
  if (/[?#]/.test(rest)) return "The webhook address must not have a query or a fragment.";
  const slash = rest.indexOf("/");
  if (slash < 0) return "The address has no path (expected /api/webhooks/<id>/<token>).";
  const authority = rest.slice(0, slash);
  const path = rest.slice(slash);
  if (authority.includes("@")) return "The webhook address must not contain credentials.";
  const port = /:(\d+)$/.exec(authority);
  const host = (port ? authority.slice(0, port.index) : authority).toLowerCase();
  if (scheme === "https") {
    if (!DISCORD_HOSTS.includes(host))
      return "The host is not a Discord address (discord.com, discordapp.com, canary.discord.com or ptb.discord.com).";
    if (port) return "A Discord webhook address has no port.";
  } else if (!LOOPBACK.includes(host)) {
    return "Plain http is only accepted for this machine, use https.";
  }
  const parts = path.replace(/^\/+|\/+$/g, "").split("/");
  let id: string | undefined;
  let secret: string | undefined;
  if (parts.length === 4 && parts[0] === "api" && parts[1] === "webhooks") [, , id, secret] = parts;
  else if (
    parts.length === 5 &&
    parts[0] === "api" &&
    /^v\d+$/.test(parts[1]) &&
    parts[2] === "webhooks"
  )
    [, , , id, secret] = parts;
  if (id === undefined || secret === undefined)
    return "The path must be /api/webhooks/<id>/<token>.";
  if (!/^\d{17,20}$/.test(id)) return "The webhook id must be 17 to 20 digits.";
  if (!/^[A-Za-z0-9._-]{1,200}$/.test(secret))
    return "The webhook token has unexpected characters.";
  return { host, id };
}

/** The host and the first and last four digits of the id: a form that is safe to show. */
export function maskWebhook(parts: WebhookParts): string {
  return `${parts.host} webhook ${parts.id.slice(0, 4)}...${parts.id.slice(-4)}`;
}

/** The host name of an API address (`https://api.github.com` gives `api.github.com`), or null. */
export function apiHost(api: string): string | null {
  const m = /^https?:\/\/([^/?#\s:]+)/i.exec(api.trim());
  return m ? m[1].toLowerCase() : null;
}

/** The browser address of a GitHub server for an API address (api.github.com is github.com). */
export function webBase(api: string | null | undefined): string {
  const host = api ? apiHost(api) : "api.github.com";
  if (host === null || host === "api.github.com") return "https://github.com";
  return `https://${host.replace(/^api\./, "")}`;
}

export function issueUrl(repo: string, number: number, api?: string | null): string {
  return `${webBase(api)}/${repo}/issues/${number}`;
}

// ----- the outbox ---------------------------------------------------------------------------

/** The operations of one meeting, in the order they were queued. */
export function opsOf(state: OutboxState | null, meetingId: string): OutboxOp[] {
  return (state?.ops ?? []).filter((o) => o.meetingId === meetingId).sort((a, b) => a.id - b.id);
}

/** The draft is read-only while an operation of the meeting exists that is not cancelled. */
export function lockedByOps(ops: OutboxOp[]): boolean {
  return ops.some((o) => o.state !== "cancelled");
}

export interface OpCounts {
  queued: number;
  running: number;
  done: number;
  failed: number;
  cancelled: number;
}

export function countOps(ops: OutboxOp[]): OpCounts {
  const n: OpCounts = { queued: 0, running: 0, done: 0, failed: 0, cancelled: 0 };
  for (const o of ops) n[o.state] += 1;
  return n;
}

/** One line for the whole publication: what is left, what failed. */
export function outboxHeadline(ops: OutboxOp[]): string {
  const live = ops.filter((o) => o.state !== "cancelled");
  if (live.length === 0) return "Nothing is queued.";
  const n = countOps(live);
  if (n.done === live.length) return "Published: everything was sent.";
  const parts = [`${n.done} of ${live.length} done`];
  if (n.running > 0) parts.push(`${n.running} running`);
  if (n.queued > 0) parts.push(`${n.queued} waiting`);
  if (n.failed > 0) parts.push(`${n.failed} failed`);
  return parts.join(", ");
}

/** `in 1 min 30 s`, `in 45 s`, `now`. */
export function waitText(ms: number): string {
  if (ms <= 0) return "now";
  const total = Math.ceil(ms / 1000);
  if (total < 60) return `in ${total} s`;
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  if (minutes < 60) return seconds === 0 ? `in ${minutes} min` : `in ${minutes} min ${seconds} s`;
  return `in ${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** What an operation is doing right now, for the line under its name. */
export function opStatusLine(op: OutboxOp, now: number, offline: boolean): string | null {
  if (op.state === "failed") return op.lastError ?? "Failed.";
  if (op.state === "queued") {
    if (op.afterId !== null && op.attempts === 0 && op.nextAttemptUnixMs === null)
      return "Waits for the operations before it.";
    if (op.lastError !== null) {
      const next =
        op.nextAttemptUnixMs === null
          ? ""
          : ` Next attempt ${offline ? "when the network is back or " : ""}${waitText(op.nextAttemptUnixMs - now)}.`;
      return `${op.lastError}${next}`;
    }
    return null;
  }
  return null;
}

/** An operation the user may retry now: it failed, or it waits for its next attempt. */
export function canRetry(op: OutboxOp): boolean {
  return op.state === "failed" || (op.state === "queued" && op.lastError !== null);
}

/** An operation the user may drop: queued or failed, never a running or finished one. */
export function canCancel(op: OutboxOp): boolean {
  return op.state === "queued" || op.state === "failed";
}

/** The link an operation produced, with its text, when it has one. */
export function opLink(op: OutboxOp): { href: string; text: string } | null {
  const r = op.result;
  if (!r || op.state !== "done") return null;
  if (op.kind === "create_issue" && r.url && r.number !== undefined)
    return { href: r.url, text: `#${r.number}` };
  if (op.kind === "commit_meeting" && r.report_url)
    return { href: r.report_url, text: r.commit ? `commit ${r.commit}` : "report" };
  if (op.kind === "commit_correction" && r.url)
    return { href: r.url, text: r.file ?? "correction" };
  return null;
}

/** An operation that publishes a correction: it belongs to a correction, not to the meeting. */
export function isCorrectionOp(op: OutboxOp): boolean {
  return op.kind === "commit_correction" || op.kind === "comment_correction";
}

/** Text for the offline banner, or null when every target answers. */
export function offlineText(state: OutboxState | null): string | null {
  const targets = state?.offline ?? [];
  if (targets.length === 0) return null;
  const names = [...new Set(targets.map((t) => (t === "discord" ? "Discord" : "GitHub")))];
  return `Offline: ${names.join(" and ")} did not answer. Operations for ${names.length === 1 ? "it" : "them"} wait and are retried by themselves when the network is back. Nothing is lost.`;
}

// ----- the plan -----------------------------------------------------------------------------

export function blockOf(plan: PublishPlan | null, code: PlanBlockCode): PlanBlock | null {
  return plan?.blocks.find((b) => b.code === code) ?? null;
}

/** The warning that has to be confirmed before the first publication to this repository. */
export function publicRepoBlock(plan: PublishPlan | null): PlanBlock | null {
  return blockOf(plan, "public_repo_unconfirmed");
}

/** Blocks that stop a publication and that a confirmation does not solve. */
export function otherBlocks(plan: PublishPlan | null): PlanBlock[] {
  return (plan?.blocks ?? []).filter((b) => b.code !== "public_repo_unconfirmed");
}

export function repoWarning(visibility: string | null | undefined, repo: string | null): string {
  const name = repo ?? "this repository";
  return visibility === "unknown"
    ? `Whether ${name} is public could not be checked. If it is public, everybody can read what is published.`
    : `${name} is public: everybody can read what is published to it.`;
}

/** The lines of "what will happen" for a plan, in the order the operations run. */
export function planLines(plan: PublishPlan, topics: Topic[] = []): string[] {
  const s: PlanSummary = plan.summary;
  const lines: string[] = [];
  const repo = plan.repo.github ?? "the repository";
  if (s.commit) {
    const count = s.files.length;
    lines.push(
      `${s.pushes ? "Commit and push" : "Commit"} ${count === 1 ? "1 file" : `${count} files`} to ${repo}: ${summarizeFiles(s.files)}.`,
    );
  }
  for (const i of s.issues) {
    const who = i.assignee ? `, assigned to ${i.assignee}` : "";
    const due = i.due ? `, due ${i.due}` : "";
    lines.push(`Create the issue "${i.title}"${who}${due}.`);
  }
  for (const st of s.statuses) {
    lines.push(
      `${st.open ? "Reopen" : "Close"} issue ${st.target} (approved status change ${st.proposalId}).`,
    );
  }
  // The names of the topics; with the difference worked out, the panel also shows each topic
  // before and after under this list.
  for (const text of s.topicsAdded) lines.push(`Add the topic "${text}" to topics.yaml.`);
  for (const t of s.topicsSet) {
    const text = topics.find((x) => x.id === t.id)?.text;
    lines.push(
      `${t.open ? "Reopen" : "Close"} the topic ${t.id}${text ? ` (${text})` : ""} in topics.yaml.`,
    );
  }
  if (s.discord)
    lines.push("Post the summary to Discord, with links to the report and the issues.");
  return lines;
}

function summarizeFiles(files: string[]): string {
  const images = files.filter((f) => f.startsWith("img/"));
  const rest = files.filter((f) => !f.startsWith("img/"));
  const parts = [...rest];
  if (images.length > 0)
    parts.push(images.length === 1 ? "1 screenshot" : `${images.length} screenshots`);
  return parts.join(", ");
}

/** The note about status changes that are not approved, or null when none is waiting. */
export function pendingStatusNote(plan: PublishPlan | null): string | null {
  const n = plan?.summary.proposalsPending ?? 0;
  if (n === 0) return null;
  return n === 1
    ? "1 status change is not approved and will not be written back."
    : `${n} status changes are not approved and will not be written back.`;
}

/** The targets a Publish press sends: those the user ticked that the rules allow. */
export function chosenTargets(
  plan: PublishPlan | null,
  ticked: Record<PublishTargetName, boolean>,
): PublishTargetName[] {
  if (!plan) return [];
  return plan.targets.filter((t) => t.available && ticked[t.target]).map((t) => t.target);
}

// ----- status proposals ---------------------------------------------------------------------

export interface ProposalCounts {
  proposed: number;
  approved: number;
  rejected: number;
}

export function countProposals(list: StatusProposal[] | undefined): ProposalCounts {
  const n: ProposalCounts = { proposed: 0, approved: 0, rejected: 0 };
  for (const p of list ?? []) n[p.decision] += 1;
  return n;
}

/** `#12` is an issue; anything else is the id of a topic. */
export function targetKind(target: string): "issue" | "topic" {
  return /^#\d+$/.test(target) ? "issue" : "topic";
}

export function issueNumber(target: string): number | null {
  return targetKind(target) === "issue" ? Number(target.slice(1)) : null;
}

export function statusVerb(status: ProposedStatus): string {
  return status === "done" ? "Mark as done" : "Reopen";
}

/** What approving does, in a sentence, for the target. */
export function proposalEffect(p: StatusProposal): string {
  const what = targetKind(p.target) === "issue" ? `issue ${p.target}` : `topic ${p.target}`;
  const verb = p.status === "done" ? "Closes" : "Reopens";
  return targetKind(p.target) === "issue"
    ? `${verb} ${what} on GitHub and adds a comment with the quote.`
    : `${verb} ${what} in topics.yaml.`;
}

/** `Approved`, `Rejected` or `Waiting for your decision`. */
export function decisionLabel(p: StatusProposal): string {
  return p.decision === "approved"
    ? "Approved"
    : p.decision === "rejected"
      ? "Rejected"
      : "Waiting for your decision";
}

/** Targets a status change may still be added for: not already in the list, open ones only. */
export function addableTargets(
  proposals: StatusProposal[] | undefined,
  tasks: CycleTasks | null,
  topics: Topic[],
): { value: string; label: string }[] {
  const taken = new Set((proposals ?? []).map((p) => p.target));
  const out: { value: string; label: string }[] = [];
  for (const t of tasks?.open ?? []) {
    const value = `#${t.number}`;
    if (!taken.has(value)) out.push({ value, label: `${value} ${t.title}` });
  }
  for (const t of topics) {
    if (t.status === "open" && !taken.has(t.id))
      out.push({ value: t.id, label: `${t.id} ${t.text}` });
  }
  return out;
}

/** The task an issue target stands for, to show its title next to the number. */
export function taskOfTarget(tasks: CycleTasks | null, target: string): CycleTask | null {
  const n = issueNumber(target);
  return n === null ? null : (tasks?.open.find((t) => t.number === n) ?? null);
}

// ----- topics.yaml --------------------------------------------------------------------------

export interface ConflictRow {
  id: string;
  theirs: TopicVersion | null;
  ours: TopicVersion | null;
  /** The two versions were not reported: only the id is known. */
  partial: boolean;
}

/**
 * The topics where our version replaced a different change of the remote (last write wins per
 * topic). With the two versions reported they are shown side by side; with only the ids, ours
 * is taken from the topics as they are now.
 */
export function conflictRows(result: OpResult | null | undefined, topics: Topic[]): ConflictRow[] {
  const t = result?.topics;
  if (!t) return [];
  const details: TopicConflictDetail[] = t.conflict_details ?? [];
  return t.conflicts.map((id) => {
    const d = details.find((x) => x.id === id);
    if (d) return { id, theirs: d.theirs, ours: d.ours, partial: false };
    const now = topics.find((x) => x.id === id);
    return {
      id,
      theirs: null,
      ours: now ? { text: now.text, status: now.status } : null,
      partial: true,
    };
  });
}

/** One topic version as a line: `[closed] Decide the rover wheel size`. */
export function topicLine(v: TopicVersion | null): string {
  return v === null ? "(not there)" : `[${v.status}] ${v.text}`;
}

/** What differs between two versions, for the labels next to the diff. */
export function topicDifference(theirs: TopicVersion | null, ours: TopicVersion | null): string[] {
  if (theirs === null || ours === null) return ["presence"];
  const out: string[] = [];
  if (theirs.text !== ours.text) out.push("text");
  if (theirs.status !== ours.status) out.push("status");
  return out;
}

/** What the plan changes in `topics.yaml`, in words (`1 added, 1 changed`), or null when nothing. */
export function planTopicsSummary(plan: TopicsPlan): string | null {
  const count = (kind: TopicsPlan["changes"][number]["change"]) =>
    plan.changes.filter((c) => c.change === kind).length;
  const parts: string[] = [];
  for (const kind of ["added", "changed", "removed"] as const)
    if (count(kind) > 0) parts.push(`${count(kind)} ${kind}`);
  return parts.length === 0 ? null : parts.join(", ");
}

/** One planned topic version as a line, `[closed] Decide the rover wheel size`, or `(not there)`. */
export function plannedTopicLine(v: PlannedTopicView | null): string {
  return topicLine(v);
}

/** The topic changes the commit made, in words (`1 added, 1 changed`), or null when none. */
export function topicsSummary(result: OpResult | null | undefined): string | null {
  const t = result?.topics;
  if (!t) return null;
  const parts: string[] = [];
  if (t.added > 0) parts.push(`${t.added} added`);
  if (t.changed > 0) parts.push(`${t.changed} changed`);
  if (t.removed > 0) parts.push(`${t.removed} removed`);
  return parts.length === 0 ? null : parts.join(", ");
}

/** The targets whose operations were all sent: GitHub when its commit is done, Discord when its post is. */
export function targetsSent(ops: OutboxOp[]): Set<PublishTargetName> {
  const out = new Set<PublishTargetName>();
  const live = (o: OutboxOp) => o.state !== "cancelled";
  const github = ops.filter((o) => live(o) && o.target !== "discord");
  if (github.length > 0 && github.every((o) => o.state === "done")) out.add("github");
  const discord = ops.filter((o) => live(o) && o.target === "discord");
  if (discord.length > 0 && discord.every((o) => o.state === "done")) out.add("discord");
  return out;
}

/** The commit operation of a meeting that is done, if any. */
export function committedOp(ops: OutboxOp[]): OutboxOp | null {
  return ops.find((o) => o.kind === "commit_meeting" && o.state === "done") ?? null;
}

// ----- tasks --------------------------------------------------------------------------------

/** `3 open, 2 done` for the heading of the task list. */
export function taskCounts(tasks: CycleTasks | null): string {
  if (!tasks) return "";
  return `${tasks.open.length} open, ${tasks.done.length} done`;
}

/** The date the issues were last read, as a sentence. */
export function tasksAsOf(tasks: CycleTasks | null): string {
  if (!tasks) return "";
  return tasks.asOf ? `As of ${tasks.asOf}.` : "The issues were never read: pull the repository.";
}

// ----- corrections --------------------------------------------------------------------------

export const CORRECTION_STATE_LABELS: Record<CorrectionState, string> = {
  local: "Only on this computer",
  queued: "Waiting",
  running: "Publishing",
  published: "Published",
  failed: "Failed",
};

/** The class of the badge of a correction state, reusing the colours of the operations. */
export function correctionBadgeClass(state: CorrectionState): string {
  const cls = {
    local: "badge-excluded",
    queued: "badge-job-queued",
    running: "badge-job-running",
    published: "badge-job-done",
    failed: "badge-job-failed",
  }[state];
  return `badge ${cls}`;
}

/** What the rule of the `github` target means for corrections, in plain words. */
export function correctionRuleHelp(rule: PublishMode): string {
  switch (rule) {
    case "off":
      return "GitHub is Off for this cycle: corrections stay on this computer and are never published, not even when you press Publish. They still reach the next meeting.";
    case "auto":
      return "GitHub is Automatic for this cycle: a correction is published as soon as you add it. It cannot be taken back. A public or unconfirmed repository still waits for your confirmation.";
    default:
      return "GitHub is On approval for this cycle: a correction stays on this computer until you press Publish next to it.";
  }
}

/** What a correction is about, `item i3 and issue #12`, or null for a general note. */
export function correctionAbout(c: Pick<Correction, "item" | "issue">): string | null {
  const parts: string[] = [];
  if (c.item) parts.push(`item ${c.item}`);
  if (c.issue) parts.push(`issue ${c.issue}`);
  return parts.length === 0 ? null : parts.join(" and ");
}

/** The line under a correction: where it stands, and what is still to happen. */
export function correctionStatusLine(c: Correction, rule: PublishMode): string | null {
  switch (c.state) {
    case "local":
      return rule === "off"
        ? "Kept on this computer: publishing is Off."
        : "Not published yet. It already counts for the next meeting.";
    case "queued":
      return c.file?.lastError ?? "Waits to be published.";
    case "running":
      return null;
    case "failed":
      return c.file?.lastError ?? "Publishing failed.";
    case "published": {
      const comment = c.comment;
      const where = c.fileName ? `Published as ${c.fileName} in the meeting folder` : "Published";
      if (!comment) return `${where}.`;
      if (comment.state === "done")
        return comment.result?.skipped
          ? `${where}. No comment was made on an issue: ${comment.result.skipped}.`
          : `${where}, and commented on the issue.`;
      if (comment.state === "failed")
        return `${where}. The comment on the issue failed: ${comment.lastError ?? "see the outbox"}`;
      return `${where}. The comment on the issue is still to come.`;
    }
  }
}

/** Today as `YYYY-MM-DD` in the local calendar, the default day of a correction. */
export function today(now: Date = new Date()): string {
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${now.getFullYear()}-${month}-${day}`;
}

/** Why a correction cannot be written, or null. */
export function correctionProblem(text: string, date: string): string | null {
  if (text.trim() === "") return "Write what changed.";
  if (text.trim().length > 2000) return "A correction is at most 2000 characters.";
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return "The date is YYYY-MM-DD.";
  return null;
}

/** The corrections that were not queued yet: what Publish all would send. */
export function unqueued(list: Correction[]): Correction[] {
  return list.filter((c) => c.state === "local");
}
