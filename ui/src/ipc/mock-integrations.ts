/**
 * The integrations of Stage 7 in the mock backend: a fake Notion workspace with one database
 * ("Tasks") and a fake Discord server ("Nomad team") that a bot can post to. Nothing leaves the
 * page. The publishing mock (`mock-publish.ts`) runs the outbox operations against these worlds;
 * this file also serves the setup calls: testing the tokens, reading the Notion properties,
 * mapping fields, searching members and roles.
 *
 * Fictional data only. Tests and screenshots drive the fake worlds through `IntegrationHooks`
 * (`window.__bbbMock`): see what Notion and the server hold, take a permission from the bot,
 * trash a page as a person would.
 */
import type { Backend } from "./backend";
import type { CycleStore } from "./mock-cycles";
import { fail } from "./mock-util";
import {
  ALLOWED_NOTION_TYPES as ALLOWED_TYPES,
  REQUIRED_BOT_PERMISSIONS,
  checkNotionMapping,
  missingBotPermissions,
  type MappedNotionField as MappedField,
} from "../publish";
import type {
  CycleDto,
  CycleTargets,
  DiscordBotCheck,
  DiscordChannelKind,
  DiscordMember,
  DiscordRef,
  DiscordRole,
  NotionAccess,
  NotionProperties,
  NotionPropertyType,
  NotionSchema,
} from "./types";

type NotionTarget = NonNullable<CycleTargets["notion"]>;
type DiscordBotTarget = NonNullable<NonNullable<CycleTargets["discord"]>["bot"]>;

// ----- the fictional world: ids and demo targets ----------------------------------------------

export const DEMO_NOTION_REF = "cred:notion/nomad";
export const DEMO_NOTION_DATABASE = "0123456789abcdef0123456789abcdef";
export const DEMO_NOTION_DATA_SOURCE = "fedcba98-7654-3210-fedc-ba9876543210";
export const DEMO_BOT_REF = "cred:discord-bot/nomad";
export const DEMO_GUILD_ID = "111111111111111111";
export const DEMO_CHANNEL_ID = "222222222222222222";

/** A Notion target that points at the fake database, with the mapping its properties suggest. */
export function demoNotionTarget(): NotionTarget {
  return {
    token: DEMO_NOTION_REF,
    database: DEMO_NOTION_DATABASE,
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
}

/** A Discord bot target that points at `#meetings` of the fake server. */
export function demoBotTarget(): DiscordBotTarget {
  return {
    token: DEMO_BOT_REF,
    guild: DEMO_GUILD_ID,
    channel: DEMO_CHANNEL_ID,
    thread: true,
    editOnCorrection: true,
  };
}

const BOT_NAME = "Nomad notes";
const GUILD_NAME = "Nomad team";
const NOTION_BOT_NAME = "Nomad notes";
const DATABASE_TITLE = "Tasks";

/** The properties of the fake database, in the order Notion shows them. */
const NOTION_SCHEMA: { name: string; type: NotionPropertyType }[] = [
  { name: "Name", type: "title" },
  { name: "BBB key", type: "rich_text" },
  { name: "Assignee", type: "select" },
  { name: "Due", type: "date" },
  { name: "Status", type: "status" },
  { name: "Meeting", type: "url" },
  { name: "Notes", type: "rich_text" },
];
const STATUS_OPTIONS = ["Not started", "In progress", "Done"];

const MEMBERS: DiscordMember[] = [
  {
    userId: "000000000000000001",
    username: "alex_demo",
    globalName: "Alex Demo",
    nick: "Alex",
    bot: false,
  },
  {
    userId: "000000000000000002",
    username: "jan_demo",
    globalName: "Jan Demo",
    nick: null,
    bot: false,
  },
  {
    userId: "000000000000000003",
    username: "kasia_demo",
    globalName: "Kasia Demo",
    nick: "Kasia (comms)",
    bot: false,
  },
  {
    userId: "000000000000000005",
    username: "piotr_demo",
    globalName: "Piotr Demo",
    nick: null,
    bot: false,
  },
  {
    userId: "000000000000000006",
    username: "alexandra_sample",
    globalName: "Alexandra Sample",
    nick: null,
    bot: false,
  },
  {
    userId: "000000000000000007",
    username: "alek_test",
    globalName: "Alek Test",
    nick: "Alek",
    bot: false,
  },
  {
    userId: "000000000000000100",
    username: "nomad_notes",
    globalName: null,
    nick: BOT_NAME,
    bot: true,
  },
];

const ROLES: DiscordRole[] = [
  { roleId: "000000000000000004", name: "Mechanics", mentionable: true },
  { roleId: "000000000000000014", name: "Electronics", mentionable: true },
  { roleId: "000000000000000015", name: "Software", mentionable: false },
  { roleId: "000000000000000016", name: "Interns", mentionable: true },
];

const CHANNELS: { id: string; name: string; kind: DiscordChannelKind }[] = [
  { id: DEMO_CHANNEL_ID, name: "meetings", kind: "text" },
  { id: "333333333333333333", name: "ideas", kind: "forum" },
  { id: "444444444444444444", name: "announcements", kind: "announcement" },
];

// ----- the field mapping (pure) ---------------------------------------------------------------

/** The mapping the properties of a database suggest: names and types decide, nothing else. */
export function proposeNotionMapping(
  schema: { name: string; type: NotionPropertyType }[],
): NotionProperties {
  const pick = (field: MappedField, names: string[]): string | undefined =>
    schema.find(
      (p) => names.includes(p.name.toLowerCase()) && ALLOWED_TYPES[field].includes(p.type),
    )?.name;
  const out: NotionProperties = {
    title: schema.find((p) => p.type === "title")?.name ?? "Name",
    key: pick("key", ["bbb key", "key"]) ?? "BBB key",
  };
  const assignee = pick("assignee", ["assignee", "owner", "assigned to"]);
  const due = pick("due", ["due", "due date", "deadline"]);
  const status = pick("status", ["status"]);
  const meeting = pick("meeting", ["meeting", "report", "link"]);
  const cycle = pick("cycle", ["cycle"]);
  if (assignee) out.assignee = assignee;
  if (due) out.due = due;
  if (status) {
    out.status = status;
    out.statusValue = STATUS_OPTIONS[0];
  }
  if (meeting) out.meeting = meeting;
  if (cycle) out.cycle = cycle;
  return out;
}

const cut = (text: string, max: number) => Array.from(text).slice(0, max).join("");

/** The value as Notion takes it: text cut at 2000 characters, an option name without commas. */
function fit(type: NotionPropertyType | undefined, value: string): string {
  if (type === "rich_text" || type === "title") return cut(value, 2000);
  if (type === "select" || type === "multi_select") return cut(value.replaceAll(",", " "), 100);
  return value;
}

const sameId = (a: string, b: string) =>
  a.replaceAll("-", "").toLowerCase() === b.replaceAll("-", "").toLowerCase();

// ----- the hooks ------------------------------------------------------------------------------

/** A page of the fake Notion database. */
export interface FakeNotionPage {
  id: string;
  url: string;
  /** The idempotency key, `bbb:v1:<cycle>:<meeting>:<item>`. */
  key: string;
  trashed: boolean;
  /** The values by the name of the Notion property. An empty property is null. */
  properties: Record<string, string | null>;
  /** The values of the default mapping, for convenience. */
  title: string | null;
  assignee: string | null;
  due: string | null;
  status: string | null;
  meeting: string | null;
}

/** A message the fake server holds that the bot sent. */
export interface FakeBotMessage {
  id: string;
  channelId: string;
  /** The thread the message is in; null for a message in the channel itself. */
  threadId: string | null;
  content: string;
}

/** A thread of the fake server. Its id is the id of the message it was opened from. */
export interface FakeThread {
  id: string;
  channelId: string;
  name: string;
  messageIds: string[];
}

/** What a test or a screenshot may do to the fake Notion workspace and the fake server. */
export interface IntegrationHooks {
  /** The pages of the database, oldest first, trashed ones included. */
  notionPages(): FakeNotionPage[];
  /** A person moves a page to the trash in Notion. */
  trashNotionPage(id: string): void;
  /** A person changes a property of a page in Notion (`null` empties it). */
  setNotionProperty(id: string, property: string, value: string | null): void;
  /** The messages the bot sent, oldest first. A deleted message is not listed. */
  discordBotMessages(): FakeBotMessage[];
  discordThreads(): FakeThread[];
  /**
   * Replaces the permissions the bot has in `#meetings` (names such as `SEND_MESSAGES`). All the
   * required ones until it is called.
   */
  setBotPermissions(names: string[]): void;
  /** A person deletes a message of the bot in Discord. */
  deleteBotMessage(id: string): void;
}

type IntegrationMethods = Pick<
  Backend,
  | "testNotionAccess"
  | "notionSchema"
  | "setNotionMapping"
  | "testDiscordBot"
  | "searchDiscordMembers"
  | "listDiscordRoles"
  | "setParticipantDiscord"
>;

/** What the Notion export needs of a task, frozen when it was queued. */
export interface NotionTask {
  key: string;
  title: string;
  assignee: string | null;
  due: string | null;
  meetingUrl: string | null;
  cycleName: string;
}

export type NotionExport =
  | { error: string }
  | {
      pageId: string;
      url: string;
      outcome: "created" | "updated" | "removed";
      /** How many other pages had the same key (none is deleted). */
      duplicates: number;
    };

export type BotPost = { error: string } | { message: FakeBotMessage };

export interface MockIntegrations {
  methods: IntegrationMethods;
  hooks: IntegrationHooks;
  notion: {
    /** The page of an earlier export, for the plan. */
    pageUrl(id: string): string | null;
    /** Upserts one task by its key: no page is made twice. */
    export(target: NotionTarget, task: NotionTask, known: string | null): NotionExport;
  };
  bot: {
    /** Why the bot cannot post with this target, or null. Checks guild, channel and permissions. */
    refusal(target: DiscordBotTarget): string | null;
    post(channelId: string, content: string, threadId: string | null): FakeBotMessage;
    openThread(channelId: string, messageId: string, name: string): FakeThread;
    /** `false` when the message is gone (a person deleted it). */
    edit(messageId: string, content: string): boolean;
    exists(messageId: string): boolean;
  };
}

export interface IntegrationDeps {
  cycles: CycleStore;
  /** Whether a token is stored for a `cred:` reference (never the token). */
  hasSecret: (reference: string) => boolean;
  isOffline: () => boolean;
}

const fakePageId = (n: number) => `5e0c0000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const fakeSnowflake = (n: number) => `9${String(n).padStart(17, "0")}`;

export function createMockIntegrations(deps: IntegrationDeps): MockIntegrations {
  const { cycles } = deps;

  // ----- the fake Notion workspace -----
  const pages: FakeNotionPage[] = [];
  let nextPage = 1;

  // ----- the fake server -----
  const messages: FakeBotMessage[] = [];
  const deleted = new Set<string>();
  const threads: FakeThread[] = [];
  let nextMessage = 1;
  let botPermissions = new Set(REQUIRED_BOT_PERMISSIONS.map((p) => p.name));

  // ----- access to the targets of a cycle -----
  const cycleOf = (cycleId: string): CycleDto =>
    cycles.cycle(cycleId) ?? fail("not_found", `The cycle ${cycleId} does not exist.`);
  const notionOf = (cycleId: string): NotionTarget =>
    cycleOf(cycleId).targets.notion ??
    fail("no_target", "This cycle has no Notion target: add one in the cycle settings first.");
  const botOf = (cycleId: string): DiscordBotTarget =>
    cycleOf(cycleId).targets.discord?.bot ??
    fail(
      "no_target",
      "This cycle has no Discord bot: choose Bot for the Discord target in the cycle settings first.",
    );

  const NOT_SHARED =
    "Notion cannot find this database: it does not exist, or it is not shared with the integration (in Notion, open the database, then ... > Connections).";
  const OFFLINE_NOTION = "Could not reach api.notion.com: the network is offline.";
  const OFFLINE_DISCORD = "Could not reach discord.com: the network is offline.";

  // ----- Notion -----
  function notionProblem(t: NotionTarget): string | null {
    if (!deps.hasSecret(t.token))
      return `No Notion token is stored for ${t.token}: store it in the settings.`;
    if (deps.isOffline()) return OFFLINE_NOTION;
    if (!sameId(t.database, DEMO_NOTION_DATABASE)) return NOT_SHARED;
    return null;
  }

  const testNotionAccess: Backend["testNotionAccess"] = async (cycleId) => {
    const t = notionOf(cycleId);
    const base: NotionAccess = {
      ok: false,
      message: "",
      botName: null,
      databaseTitle: null,
      dataSources: [],
      tokenSet: deps.hasSecret(t.token),
    };
    const problem = notionProblem(t);
    if (problem) return { ...base, message: problem };
    return {
      ...base,
      ok: true,
      message: `Connected to Notion as "${NOTION_BOT_NAME}": database "${DATABASE_TITLE}".`,
      botName: NOTION_BOT_NAME,
      databaseTitle: DATABASE_TITLE,
      dataSources: [{ id: DEMO_NOTION_DATA_SOURCE, name: DATABASE_TITLE }],
    };
  };

  const notionSchema: Backend["notionSchema"] = async (cycleId): Promise<NotionSchema> => {
    const t = notionOf(cycleId);
    const problem = notionProblem(t);
    if (problem) {
      fail(
        !deps.hasSecret(t.token) ? "secrets" : problem === NOT_SHARED ? "not_found" : "io",
        problem,
      );
    }
    if (t.dataSource !== undefined && !sameId(t.dataSource, DEMO_NOTION_DATA_SOURCE))
      fail("not_found", "This data source is not part of the database: choose another.");
    return {
      dataSourceId: DEMO_NOTION_DATA_SOURCE,
      properties: structuredClone(NOTION_SCHEMA),
      proposed: proposeNotionMapping(NOTION_SCHEMA),
      current: structuredClone(t.properties),
      problems: checkNotionMapping(t.properties, NOTION_SCHEMA),
    };
  };

  const setNotionMapping: Backend["setNotionMapping"] = async (cycleId, dataSource, properties) => {
    const cycle = cycleOf(cycleId);
    const notion =
      cycle.targets.notion ??
      fail("no_target", "This cycle has no Notion target: add one in the cycle settings first.");
    if (dataSource === null) delete notion.dataSource;
    else notion.dataSource = dataSource;
    notion.properties = structuredClone(properties);
    const saved = cycles.saveCycle(cycle);
    if (!saved.saved) fail("cycle_invalid", saved.issues[0]?.message ?? "The cycle is not valid.");
    return structuredClone(cycleOf(cycleId).targets);
  };

  /** Values by property name, as the mapping writes them. */
  function propertyValues(
    map: NotionProperties,
    task: NotionTask,
    create: boolean,
  ): Record<string, string | null> {
    const type = (name: string) => NOTION_SCHEMA.find((p) => p.name === name)?.type;
    const out: Record<string, string | null> = {};
    const put = (name: string | undefined, value: string | null) => {
      if (name === undefined || name === "") return;
      // An optional field without a value is left out on create and emptied on update.
      if (value === null) {
        if (!create) out[name] = null;
        return;
      }
      out[name] = fit(type(name), value);
    };
    put(map.title, task.title);
    put(map.key, task.key);
    put(map.assignee, task.assignee);
    put(map.due, task.due);
    // A person may have moved the task: only a new page gets the initial status.
    if (create) put(map.status, map.statusValue ?? null);
    put(map.meeting, task.meetingUrl);
    put(map.cycle, task.cycleName);
    return out;
  }

  function rebuildViews(p: FakeNotionPage) {
    p.title = p.properties["Name"] ?? null;
    p.assignee = p.properties["Assignee"] ?? null;
    p.due = p.properties["Due"] ?? null;
    p.status = p.properties["Status"] ?? null;
    p.meeting = p.properties["Meeting"] ?? null;
  }

  function exportTask(t: NotionTarget, task: NotionTask, known: string | null): NotionExport {
    const problem = notionProblem(t);
    if (problem) return { error: problem };
    const problems = checkNotionMapping(t.properties, NOTION_SCHEMA);
    if (problems.length > 0)
      return {
        error: `Notion rejected the task: ${problems[0].message} Fix the mapping, then retry.`,
      };
    const write = (page: FakeNotionPage, create: boolean) => {
      Object.assign(page.properties, propertyValues(t.properties, task, create));
      rebuildViews(page);
    };
    const result = (
      page: FakeNotionPage,
      outcome: "created" | "updated" | "removed",
      duplicates = 0,
    ): NotionExport => ({ pageId: page.id, url: page.url, outcome, duplicates });

    let page: FakeNotionPage | undefined;
    let duplicates = 0;
    if (known !== null) {
      page = pages.find((p) => p.id === known);
      if (!page)
        return { error: "The page was deleted or is no longer shared with the integration." };
    } else {
      const hits = pages.filter((p) => p.key === task.key);
      page = hits[0];
      duplicates = Math.max(0, hits.length - 1);
    }
    if (page) {
      if (page.trashed) return result(page, "removed", duplicates);
      write(page, false);
      return result(page, "updated", duplicates);
    }
    // Only a new page gets the status, and Notion refuses a status the property does not have.
    const statusValue = t.properties.statusValue;
    if (t.properties.status && statusValue !== undefined && !STATUS_OPTIONS.includes(statusValue))
      return {
        error: `Notion rejected the task: "${statusValue}" is not an option of "${t.properties.status}" (${STATUS_OPTIONS.join(", ")}).`,
      };
    const id = fakePageId(nextPage++);
    page = {
      id,
      url: `https://www.notion.so/${id.replaceAll("-", "")}`,
      key: task.key,
      trashed: false,
      properties: {},
      title: null,
      assignee: null,
      due: null,
      status: null,
      meeting: null,
    };
    pages.push(page);
    write(page, true);
    return result(page, "created");
  }

  // ----- Discord -----
  const channelOf = (id: string) => CHANNELS.find((c) => c.id === id);

  /** The reason the bot cannot post with this target, or null. */
  function botRefusal(t: DiscordBotTarget): string | null {
    if (!deps.hasSecret(t.token))
      return `No Discord bot token is stored for ${t.token}: store it in the settings.`;
    if (deps.isOffline()) return OFFLINE_DISCORD;
    if (t.guild !== DEMO_GUILD_ID)
      return "Discord: the bot is not in this server, or the server id is wrong. Add it with the invite link.";
    const channel = channelOf(t.channel);
    if (!channel) return "Discord: this channel does not exist in the server.";
    if (channel.kind === "forum")
      return `#${channel.name} is a forum channel: choose a text or announcement channel.`;
    if (!botPermissions.has("VIEW_CHANNEL")) return `The bot cannot see #${channel.name}.`;
    const missing = missingBotPermissions([...botPermissions], t.thread);
    if (missing.length > 0)
      return `The bot is missing ${missing.length === 1 ? "a permission" : "permissions"} in #${channel.name}: ${missing.join(", ")}.`;
    return null;
  }

  const testDiscordBot: Backend["testDiscordBot"] = async (cycleId): Promise<DiscordBotCheck> => {
    const t = botOf(cycleId);
    const channel = channelOf(t.channel);
    const base: DiscordBotCheck = {
      ok: false,
      message: "",
      botName: null,
      guildName: null,
      channelName: null,
      channelKind: null,
      missing: [],
      tokenSet: deps.hasSecret(t.token),
    };
    if (!base.tokenSet)
      return { ...base, message: `No Discord bot token is stored for ${t.token}.` };
    if (deps.isOffline()) return { ...base, message: OFFLINE_DISCORD };
    if (t.guild !== DEMO_GUILD_ID)
      return {
        ...base,
        botName: BOT_NAME,
        message:
          "Discord: the bot is not in this server, or the server id is wrong. Add it with the invite link.",
      };
    const found = {
      ...base,
      botName: BOT_NAME,
      guildName: GUILD_NAME,
      channelName: channel?.name ?? null,
      channelKind: channel?.kind ?? null,
    };
    if (!channel)
      return { ...found, message: "Discord: this channel does not exist in the server." };
    if (channel.kind === "forum")
      return {
        ...found,
        message: `#${channel.name} is a forum channel: choose a text or announcement channel.`,
      };
    const missing = missingBotPermissions([...botPermissions], t.thread);
    if (missing.length > 0)
      return {
        ...found,
        missing,
        message: botPermissions.has("VIEW_CHANNEL")
          ? `The bot is missing ${missing.length === 1 ? "a permission" : "permissions"} in #${channel.name}: ${missing.join(", ")}.`
          : `The bot cannot see #${channel.name}.`,
      };
    return {
      ...found,
      ok: true,
      message: `Connected as "${BOT_NAME}" to #${channel.name} in ${GUILD_NAME}.`,
    };
  };

  /** The bot of a cycle that can ask Discord something: token, network and server. */
  function askable(cycleId: string): void {
    const t = botOf(cycleId);
    if (!deps.hasSecret(t.token))
      fail("secrets", `No Discord bot token is stored for ${t.token}: store it in the settings.`);
    if (deps.isOffline()) fail("io", OFFLINE_DISCORD);
    if (t.guild !== DEMO_GUILD_ID) fail("not_found", "The bot is not in this server.");
  }

  const searchDiscordMembers: Backend["searchDiscordMembers"] = async (cycleId, query) => {
    askable(cycleId);
    const q = query.trim().toLowerCase();
    if (Array.from(q).length < 2) fail("invalid", "Type at least 2 characters to search.");
    // Discord matches the start of the user name or of the nickname.
    return MEMBERS.filter(
      (m) => m.username.toLowerCase().startsWith(q) || (m.nick ?? "").toLowerCase().startsWith(q),
    )
      .slice(0, 10)
      .map((m) => ({ ...m }));
  };

  const listDiscordRoles: Backend["listDiscordRoles"] = async (cycleId) => {
    askable(cycleId);
    return ROLES.map((r) => ({ ...r }));
  };

  const setParticipantDiscord: Backend["setParticipantDiscord"] = async (
    cycleId,
    participantId,
    discord: DiscordRef | null,
  ) => {
    const cycle = cycleOf(cycleId);
    const person =
      cycle.participants.find((p) => p.id === participantId) ??
      fail("not_found", "This participant does not exist.");
    if (discord === null) delete person.discord;
    else {
      const { userId, roleId } = discord;
      if ((userId === undefined) === (roleId === undefined))
        fail("invalid", "Give exactly one of a user and a role.");
      const id = userId ?? roleId!;
      if (!/^[0-9]{1,25}$/.test(id)) fail("invalid", `"${id}" is not a Discord id (digits only).`);
      person.discord = userId !== undefined ? { userId } : { roleId };
    }
    const saved = cycles.saveCycle(cycle);
    if (!saved.saved) fail("cycle_invalid", saved.issues[0]?.message ?? "The cycle is not valid.");
    return cycleOf(cycleId);
  };

  const hooks: IntegrationHooks = {
    notionPages: () => structuredClone(pages),
    trashNotionPage: (id) => {
      const page = pages.find((p) => p.id === id);
      if (page) page.trashed = true;
    },
    setNotionProperty: (id, property, value) => {
      const page = pages.find((p) => p.id === id);
      if (!page) return;
      page.properties[property] = value;
      rebuildViews(page);
    },
    discordBotMessages: () => messages.filter((m) => !deleted.has(m.id)).map((m) => ({ ...m })),
    discordThreads: () =>
      threads.map((t) => ({
        ...t,
        messageIds: messages
          .filter((m) => m.threadId === t.id && !deleted.has(m.id))
          .map((m) => m.id),
      })),
    setBotPermissions: (names) => void (botPermissions = new Set(names)),
    deleteBotMessage: (id) => void deleted.add(id),
  };

  return {
    methods: {
      testNotionAccess,
      notionSchema,
      setNotionMapping,
      testDiscordBot,
      searchDiscordMembers,
      listDiscordRoles,
      setParticipantDiscord,
    },
    hooks,
    notion: {
      pageUrl: (id) => pages.find((p) => p.id === id)?.url ?? null,
      export: exportTask,
    },
    bot: {
      refusal: botRefusal,
      post: (channelId, content, threadId) => {
        const message: FakeBotMessage = {
          id: fakeSnowflake(nextMessage++),
          channelId,
          threadId,
          content,
        };
        messages.push(message);
        return { ...message };
      },
      openThread: (channelId, messageId, name) => {
        // Discord answers a second request for the same message with the thread it has.
        const existing = threads.find((t) => t.id === messageId);
        if (existing) return { ...existing, messageIds: [] };
        const thread: FakeThread = {
          id: messageId,
          channelId,
          name: cut(name, 100),
          messageIds: [],
        };
        threads.push(thread);
        return { ...thread };
      },
      edit: (messageId, content) => {
        const m = messages.find((x) => x.id === messageId);
        if (!m || deleted.has(m.id)) return false;
        m.content = content;
        return true;
      },
      exists: (messageId) => messages.some((m) => m.id === messageId && !deleted.has(m.id)),
    },
  };
}
