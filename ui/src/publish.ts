import type {
  Correction,
  CorrectionState,
  CycleTask,
  CycleTasks,
  DiscordMember,
  NotionMappingProblem,
  NotionProperties,
  NotionPropertyType,
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
  notion: "Notion (tasks database)",
};

/** Shown next to a disabled rule switch when the target is not configured yet. */
export const UNCONFIGURED_TARGET_HINT: Record<PublishTargetName, string> = {
  github: "Connect a GitHub repository first",
  discord: "Configure a Discord webhook or bot channel first",
  notion: "Configure a Notion database first",
};

/**
 * Whether the Discord target can publish: a bot needs its channel, a webhook needs its secret
 * to be stored (the same rule the backend applies).
 */
export function isDiscordConfigured(
  discord: { webhook?: string | null; bot?: { channel: string } | null } | null | undefined,
  webhookSet: boolean,
): boolean {
  if (!discord) return false;
  if (discord.bot) return discord.bot.channel.trim() !== "";
  return Boolean(discord.webhook?.trim()) && webhookSet;
}

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
  notion: "on-approval",
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
  if (target === "github")
    return "Automatic: pushes report, transcript, screenshots and issues on completion; cannot be taken back.";
  if (target === "notion")
    return "Automatic: a page is made in Notion for every task as soon as the draft is complete. Everybody who can open the database can read the pages, and trashing them is up to you.";
  return "Automatic: posts the message and notifies mentions on completion; cannot be taken back.";
}

export const OP_STATE_LABELS: Record<OpState, string> = {
  queued: "Waiting",
  running: "Running",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

export const OP_TARGET_LABELS = {
  github: "GitHub",
  issues: "GitHub issues",
  discord: "Discord",
  notion: "Notion",
};

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

/** The three kinds of secret a `cred:` reference can stand for. */
export type SecretKind = "discord" | "discord-bot" | "notion";

/** What a reference is for, judging by its name alone (`cred:notion/nomad`), or null. */
export function referenceKindByName(reference: string): SecretKind | null {
  const name = reference.trim().replace(/^cred:/, "");
  if (name.startsWith("discord-bot/")) return "discord-bot";
  if (name.startsWith("discord/")) return "discord";
  if (name.startsWith("notion/")) return "notion";
  return null;
}

/**
 * Why a pasted Discord bot token cannot be stored, or null: three parts of letters, digits, `_`
 * and `-` separated by dots, 50 to 120 characters. Only the shape is checked; Discord decides
 * whether the token works. The value is never part of the message.
 */
export function botTokenProblem(token: string): string | null {
  const t = token.trim();
  if (t === "") return "Enter the token.";
  if (/\s/.test(t) || hasControlChars(t))
    return "This does not look like a bot token: it has spaces or line breaks in it.";
  if (
    !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(t) ||
    t.length < 50 ||
    t.length > 120
  )
    return "This does not look like a Discord bot token: it has three parts separated by dots and is 50 to 120 characters long. Copy it with Reset Token in the Developer Portal (Bot page).";
  return null;
}

/** The first four characters of the bot's user id (the first part of the token), never more. */
export function maskBotToken(token: string): string {
  return `bot ${token.trim().split(".")[0].slice(0, 4)}...`;
}

/**
 * Why a pasted Notion integration token cannot be stored, or null: it starts with `ntn_` (or the
 * older `secret_`), is 20 to 200 characters long and has no spaces. The value is never part of
 * the message.
 */
export function notionTokenProblem(token: string): string | null {
  const t = token.trim();
  if (t === "") return "Enter the token.";
  if (/\s/.test(t) || hasControlChars(t))
    return "This does not look like a Notion token: it has spaces or line breaks in it.";
  if (!/^(ntn_|secret_)/.test(t) || t.length < 20 || t.length > 200)
    return "This does not look like a Notion integration token: it starts with ntn_ and is 20 to 200 characters long. Copy it from the Configuration tab of the integration.";
  return null;
}

/** The prefix and the last four characters of a Notion token. */
export function maskNotionToken(token: string): string {
  const t = token.trim();
  return `${t.startsWith("secret_") ? "secret_" : "ntn_"}...${t.slice(-4)}`;
}

/** A permission the bot needs in the channel, with the reason shown on the setup screen. */
export interface BotPermission {
  /** The name Discord gives it in the API and in the Developer Portal. */
  name: string;
  /** The bit of the permission integer. */
  bit: number;
  why: string;
  /** Only needed when the cycle asks for one thread per meeting. */
  threadOnly: boolean;
}

/**
 * What the invite link asks for. Nothing else: no administrator, no managing of messages or
 * threads, no mentioning of everyone.
 */
export const REQUIRED_BOT_PERMISSIONS: BotPermission[] = [
  {
    name: "VIEW_CHANNEL",
    bit: 2 ** 10,
    why: "See the channel the summary goes to.",
    threadOnly: false,
  },
  {
    name: "SEND_MESSAGES",
    bit: 2 ** 11,
    why: "Post the summary in the channel.",
    threadOnly: false,
  },
  {
    name: "ATTACH_FILES",
    bit: 2 ** 15,
    why: "Attach the screenshots of the meeting.",
    threadOnly: false,
  },
  {
    name: "READ_MESSAGE_HISTORY",
    bit: 2 ** 16,
    why: "Start a thread from the summary and find the message again to edit it.",
    threadOnly: false,
  },
  {
    name: "CREATE_PUBLIC_THREADS",
    bit: 2 ** 35,
    why: "Open one thread per meeting under the summary.",
    threadOnly: true,
  },
  {
    name: "SEND_MESSAGES_IN_THREADS",
    bit: 2 ** 38,
    why: "Post the issue links, the screenshots and corrections inside the thread.",
    threadOnly: true,
  },
];

/** The permission integer of the invite link: the sum of `REQUIRED_BOT_PERMISSIONS`. */
export const BOT_PERMISSIONS_VALUE = REQUIRED_BOT_PERMISSIONS.reduce((sum, p) => sum + p.bit, 0);

/** The bot needs no gateway connection, so it needs no privileged intent. */
export const BOT_INTENTS_NOTE =
  "The bot needs no privileged intents: leave Presence, Server Members and Message Content switched off in the Developer Portal. It only posts, opens threads and edits its own messages.";

/** The names of the permissions the bot needs, for one thread per meeting or without threads. */
export function requiredBotPermissionNames(thread: boolean): string[] {
  return REQUIRED_BOT_PERMISSIONS.filter((p) => thread || !p.threadOnly).map((p) => p.name);
}

/** The required permissions that are not in `have`, in the order of the setup screen. */
export function missingBotPermissions(have: string[], thread: boolean): string[] {
  return requiredBotPermissionNames(thread).filter((name) => !have.includes(name));
}

/**
 * The link that adds the bot to a server (scope `bot`, the permissions above), made from the
 * application id of the Developer Portal; null when the id is not 17 to 20 digits.
 */
export function discordInviteUrl(appId: string): string | null {
  const id = appId.trim();
  if (!/^\d{17,20}$/.test(id)) return null;
  return `https://discord.com/oauth2/authorize?client_id=${id}&scope=bot&permissions=${BOT_PERMISSIONS_VALUE}`;
}

/** Readable label for a Discord server member in the participant picker. */
export function discordMemberLabel(m: DiscordMember): string {
  const display = m.nick ?? m.globalName;
  const tag = display && display !== m.username ? `${display} (@${m.username})` : `@${m.username}`;
  return m.bot ? `${tag} [bot]` : tag;
}

/** A link to a Discord channel, thread or message inside a server. */
export function discordChannelUrl(
  guildId: string,
  channelOrThreadId: string,
  messageId?: string,
): string {
  return messageId
    ? `https://discord.com/channels/${guildId}/${channelOrThreadId}/${messageId}`
    : `https://discord.com/channels/${guildId}/${channelOrThreadId}`;
}

export type NotionMappedField = Exclude<keyof NotionProperties, "statusValue">;

export interface NotionFieldSpec {
  field: NotionMappedField;
  label: string;
  required: boolean;
  allowedTypes: NotionPropertyType[];
  placeholder: string;
  help: string;
}

/** Metadata for each mapped Notion database property shown in the cycle editor. */
export const NOTION_FIELD_SPECS: NotionFieldSpec[] = [
  {
    field: "title",
    label: "Task title",
    required: true,
    allowedTypes: ["title"],
    placeholder: "Name",
    help: "Notion title property that holds the task text.",
  },
  {
    field: "key",
    label: "Key (idempotency)",
    required: true,
    allowedTypes: ["rich_text"],
    placeholder: "BBB key",
    help: "Text property storing the task key so re-exporting updates the same page.",
  },
  {
    field: "assignee",
    label: "Assignee",
    required: false,
    allowedTypes: ["rich_text", "select", "multi_select"],
    placeholder: "Assignee",
    help: "Optional text, select or multi-select property for the assignee name.",
  },
  {
    field: "due",
    label: "Due date",
    required: false,
    allowedTypes: ["date"],
    placeholder: "Due",
    help: "Optional date property for the task due date.",
  },
  {
    field: "status",
    label: "Status",
    required: false,
    allowedTypes: ["status", "select"],
    placeholder: "Status",
    help: "Optional status or select property; only set when a page is first created.",
  },
  {
    field: "meeting",
    label: "Meeting link",
    required: false,
    allowedTypes: ["url", "rich_text"],
    placeholder: "Meeting",
    help: "Optional link or text property pointing to the meeting report.",
  },
  {
    field: "cycle",
    label: "Cycle",
    required: false,
    allowedTypes: ["select", "rich_text"],
    placeholder: "Cycle",
    help: "Optional select or text property holding the cycle name.",
  },
];

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
    return "The webhook address must use https.";
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

/**
 * Whether a single operation holds the draft lock.
 * Only operations that have actually been sent (`done`) or are currently running (`running`)
 * lock the draft; queued, failed, cancelled, or local-only unsent commits (`pushed: false`) do not.
 */
export function opLocksDraft(op: OutboxOp): boolean {
  if (op.state === "running") return true;
  if (op.state !== "done") return false;
  if (
    (op.kind === "commit_meeting" || op.kind === "commit_correction") &&
    op.result?.pushed === false
  ) {
    return false;
  }
  return true;
}

/** The draft is read-only only once an external publishing operation is running or done. */
export function lockedByOps(ops: OutboxOp[]): boolean {
  return ops.some(opLocksDraft);
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
  if (op.state === "done" && op.result) {
    const r = op.result;
    if (op.kind === "export_notion") {
      if (r.removed) return "Page was moved to the trash in Notion; left alone and not recreated.";
      if (r.duplicates && r.duplicates > 0)
        return `Updated the oldest page (${r.duplicates} duplicate ${r.duplicates === 1 ? "page" : "pages"} with the same key left untouched).`;
      if (r.updated) return "Updated the existing page (status kept).";
      if (r.created) return "Created a new page.";
    }
    if (op.kind === "post_discord") {
      if (r.thread_id) return "Posted through the bot with a thread.";
      if (r.channel_id) return "Posted through the bot.";
    }
    if (op.kind === "edit_discord") {
      if (r.skipped) return `Discord edit skipped: ${r.skipped}.`;
      if (r.edited && r.replied) return "Edited the Discord summary and replied in the thread.";
      if (r.edited) return "Edited the Discord summary.";
    }
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

/** Keeps only `https:` and `http:` links from API responses (`javascript:`/`data:` become null). */
export function safeHttpUrl(raw: string | null | undefined): string | null {
  if (!raw) return null;
  try {
    const u = new URL(raw);
    return u.protocol === "https:" || u.protocol === "http:" ? u.toString() : null;
  } catch {
    return null;
  }
}

/** Keeps only `https:` links (for external token/setup pages). */
export function safeHttpsUrl(raw: string | null | undefined): string | null {
  const url = safeHttpUrl(raw);
  return url && url.startsWith("https://") ? url : null;
}

/** The link an operation produced, with its text, when it has one. */
export function opLink(
  op: OutboxOp,
  guildId?: string | null,
): { href: string; text: string } | null {
  const r = op.result;
  if (!r || op.state !== "done") return null;
  const link = (url: string, text: string) => {
    const href = safeHttpUrl(url);
    return href ? { href, text } : null;
  };
  if (op.kind === "create_issue" && r.url && r.number !== undefined)
    return link(r.url, `#${r.number}`);
  if (op.kind === "commit_meeting" && r.report_url)
    return link(r.report_url, r.commit ? `commit ${r.commit}` : "report");
  if (op.kind === "commit_correction" && r.url) return link(r.url, r.file ?? "correction");
  if (op.kind === "export_notion" && r.url && !r.removed) return link(r.url, "Notion page");
  if (op.kind === "post_discord") {
    if (r.thread_id && guildId)
      return link(discordChannelUrl(guildId, r.thread_id), "Discord thread");
    if (r.channel_id && r.message_ids?.[0] && guildId)
      return link(discordChannelUrl(guildId, r.channel_id, r.message_ids[0]), "Discord message");
    if (r.url) return link(r.url, r.thread_id ? "Discord thread" : "Discord message");
  }
  if (op.kind === "edit_discord" && r.thread_id && guildId)
    return link(discordChannelUrl(guildId, r.thread_id), "Discord thread");
  return null;
}

/** An operation that publishes a correction: it belongs to a correction, not to the meeting. */
export function isCorrectionOp(op: OutboxOp): boolean {
  return (
    op.kind === "commit_correction" ||
    op.kind === "comment_correction" ||
    op.kind === "edit_discord"
  );
}

/** Text for the offline banner, or null when every target answers. */
export function offlineText(state: OutboxState | null): string | null {
  const targets = state?.offline ?? [];
  if (targets.length === 0) return null;
  const label = { discord: "Discord", notion: "Notion", github: "GitHub", issues: "GitHub" };
  const names = [...new Set(targets.map((t) => label[t]))];
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
  if (s.notion.length > 0) {
    const there = s.notion.filter((n) => n.existingUrl !== null).length;
    const count = s.notion.length === 1 ? "1 task" : `${s.notion.length} tasks`;
    lines.push(
      `Export ${count} to Notion${there > 0 ? `, ${there} already there (updated, the status is kept)` : ""}.`,
    );
  }
  if (s.discord) {
    lines.push(
      s.discordMode === "bot"
        ? s.discordThread
          ? "Post the summary to Discord through the bot, with the issue links and the screenshots in a thread."
          : "Post the summary to Discord through the bot, with links to the report and the issues."
        : "Post the summary to Discord, with links to the report and the issues.",
    );
  }
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

/**
 * The targets whose operations were all sent: GitHub when its commit and issues are done, Discord
 * when its post is, Notion when its pages are. The edit of a Discord message after a correction
 * belongs to the correction, not to the publication of the meeting.
 */
export function targetsSent(ops: OutboxOp[]): Set<PublishTargetName> {
  const out = new Set<PublishTargetName>();
  const live = (o: OutboxOp) => o.state !== "cancelled";
  const named: [PublishTargetName, (o: OutboxOp) => boolean][] = [
    ["github", (o) => o.target === "github" || o.target === "issues"],
    ["discord", (o) => o.target === "discord" && o.kind !== "edit_discord"],
    ["notion", (o) => o.target === "notion"],
  ];
  for (const [name, mine] of named) {
    const list = ops.filter((o) => live(o) && mine(o));
    if (list.length > 0 && list.every((o) => o.state === "done")) out.add(name);
  }
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
      return "GitHub is Off for this project: corrections stay on this computer and are never published, not even when you press Publish. They still reach the next meeting.";
    case "auto":
      return "GitHub is Automatic for this project: a correction is published as soon as you add it. It cannot be taken back. A public or unconfirmed repository still waits for your confirmation.";
    default:
      return "GitHub is On approval for this project: a correction stays on this computer until you press Publish next to it.";
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
      let base = `${where}.`;
      if (comment) {
        if (comment.state === "done")
          base = comment.result?.skipped
            ? `${where}. No comment was made on an issue: ${comment.result.skipped}.`
            : `${where}, and commented on the issue.`;
        else if (comment.state === "failed")
          base = `${where}. The comment on the issue failed: ${comment.lastError ?? "see the outbox"}`;
        else base = `${where}. The comment on the issue is still to come.`;
      }
      const edit = c.discordEdit;
      if (!edit) return base;
      if (edit.state === "done") {
        if (edit.result?.skipped) return `${base} Discord edit skipped: ${edit.result.skipped}.`;
        if (edit.result?.edited && edit.result?.replied)
          return `${base} Discord summary edited and replied in the thread.`;
        if (edit.result?.edited) return `${base} Discord summary edited.`;
        return base;
      }
      if (edit.state === "failed")
        return `${base} The Discord edit failed: ${edit.lastError ?? "see the outbox"}`;
      return `${base} The Discord summary edit is still to come.`;
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

// ----- the Notion field mapping (pure) --------------------------------------------------------

export type MappedNotionField =
  "title" | "key" | "assignee" | "due" | "status" | "meeting" | "cycle";

/** The property types each field can be written to. */
export const ALLOWED_NOTION_TYPES: Record<MappedNotionField, NotionPropertyType[]> = {
  title: ["title"],
  key: ["rich_text"],
  assignee: ["rich_text", "select", "multi_select"],
  due: ["date"],
  status: ["status", "select"],
  meeting: ["url", "rich_text"],
  cycle: ["select", "rich_text"],
};

const FIELD_LABELS: Record<MappedNotionField, string> = {
  title: "Task title",
  key: "Key",
  assignee: "Assignee",
  due: "Due date",
  status: "Status",
  meeting: "Meeting link",
  cycle: "Cycle",
};

const TYPE_LABELS: Record<NotionPropertyType, string> = {
  title: "title",
  rich_text: "text",
  select: "select",
  multi_select: "multi-select",
  date: "date",
  status: "status",
  url: "link",
  number: "number",
  checkbox: "checkbox",
  people: "people",
};

const MAPPED_FIELDS: MappedNotionField[] = [
  "title",
  "key",
  "assignee",
  "due",
  "status",
  "meeting",
  "cycle",
];

/**
 * What is wrong with a mapping against the properties of a database: a property that is not
 * there, one of a type the field cannot use, a status without the value a new page gets.
 */
export function checkNotionMapping(
  map: NotionProperties,
  schema: { name: string; type: NotionPropertyType }[],
): NotionMappingProblem[] {
  const problems: NotionMappingProblem[] = [];
  for (const field of MAPPED_FIELDS) {
    const name = map[field];
    if (name === undefined || name === "") continue;
    const found = schema.find((p) => p.name === name);
    if (!found) {
      problems.push({
        field,
        message: `The database has no property "${name}" (${FIELD_LABELS[field]}).`,
      });
      continue;
    }
    const allowed = ALLOWED_NOTION_TYPES[field];
    if (!allowed.includes(found.type))
      problems.push({
        field,
        message: `"${name}" is a ${TYPE_LABELS[found.type]} property; ${FIELD_LABELS[field]} needs ${allowed.map((t) => TYPE_LABELS[t]).join(" or ")}.`,
      });
  }
  if (map.status !== undefined && map.status !== "" && !map.statusValue)
    problems.push({ field: "statusValue", message: "Choose the status a new page gets." });
  return problems;
}
