# 0020. Discord bot and Notion export

**Status:** Accepted (Stage 7). REST-only Discord bot with zero privileged intents, permissions `309237746688`, and `on-approval` as the default publish rule for both targets. The webhook path and the outbox are described in [0010](0010-discord-webhook-and-bot.md) and [0018](0018-publishing.md); the publish rules in [0019](0019-default-publish-rule-per-target.md).

## Context

Stage 5 publishes to GitHub and, through a webhook, to Discord. Stage 7 adds two things the webhook cannot do or that are not publishing a file at all:

- **Tasks in a task tracker.** The action items of a meeting should become tasks that a team can sort and assign in Notion, once each, however often the export is retried or repeated.
- **A Discord bot.** A webhook can post but cannot create a thread under its own message through a bot-style identity, cannot look up members to map participants to Discord users, and its messages cannot be edited with a bot token (Discord allows editing a message only to its author, and the author of a webhook message is the webhook). A bot can do all of this, provided it asks for as little as possible.

Both services change quickly and their documentation moved (see Discrepancies below), so the decision records what was read in the documentation on 2026-11-21 and what only a real server can prove.

## Decision

### 1. One interface for task targets, with a stable key

A `TaskExporter` trait in `crates/publishers/src/export.rs` is the shape every task target has: `get(id)`, `find(key)`, `create(task)`, `update(id, task)`. A `TaskRecord` is built once, as a pure function, from a draft item; every target maps it to its own fields.

- **Idempotency key.** Every exported task carries the text `bbb:v1:<cycle>:<meeting>:<item>`, parsed and built by `TaskKey`. It is the same for a retry, a restart and a re-export, and different for any other item.
- **`external_refs`.** The outbox database (schema version 7) gains a table `external_refs (meeting_id, target, ref_key, external_id, url, extra)` that holds where an item or a post lives in a service: `task:<item>` for Notion, `post` for the Discord message. It holds ids and links only, never a secret. The row is written as soon as the page or message exists, before the operation is marked done.
- **One algorithm, `upsert`.** A stored id is looked up first (live: update; in the trash: reported as removed and never recreated; gone: a permanent error that says the page was deleted or is no longer shared). Without a stored id the key is searched (several hits: the oldest is used and a duplicate count is reported, nothing is deleted). Without a hit the task is created and its id is stored at once. A lost answer or a crash between create and completion therefore never produces a second task.
- **GitHub.** Issues keep the Stage 5 path unchanged; an adapter implements the same trait so the algorithm is tested against both, and nothing in Stage 5 changes behaviour.

### 2. Notion through data sources, with the version in one constant

- **Version.** `NOTION_VERSION = "2026-03-11"` is sent on every request and is the only place it is written. The API is the REST one, with an internal integration token that the user stores as a `cred:` reference (the same secret namespace as 0011).
- **Data sources.** Since version 2025-09-03 a database holds one or more data sources, and pages, queries and schema belong to a data source. The exporter resolves the data source from `GET /databases/{id}` (or from `targets.notion.data_source` when there is more than one), queries with `POST /data_sources/{id}/query`, and creates pages with the parent `{"data_source_id": ...}`. A page is looked up by an exact match on a `rich_text` property that holds the key (a `unique_id` property is assigned by Notion and cannot hold our key).
- **Re-export.** Mapped values are updated; the status is only set on create, so a person who moved the task in Notion is not overruled. A page in the trash is not recreated.
- **Limits it respects.** Text cut at 2000 characters per rich text, select names without commas and at most 100 characters, a request pacing of about 350 ms after each write, `Retry-After` honoured on 429. The assignee is written as the participant's name; the `people` property needs Notion user ids and is not supported in this stage.

### 3. A REST-only Discord bot: no gateway, no intents

The bot never opens a gateway connection. It makes plain REST calls with `Authorization: Bot <token>`, so it declares **no intents**, and in the Developer Portal the three privileged intents (Presence, Server Members, Message Content) stay **off**.

- **Why that is enough.** Posting, creating threads and editing the bot's own messages are REST operations. Looking up members uses `GET /guilds/{id}/members/search`, which matches username and nickname prefixes and which the documentation lists among the things that need no privileged intent; only `GET /guilds/{id}/members` (List Guild Members) needs `GUILD_MEMBERS`. The message-content intent only affects reading other people's messages, which the application never does.
- **OAuth2 scope: `bot` only.** No `applications.commands`, because there are no slash commands. The setup advice is to turn "Public Bot" off and leave "Requires OAuth2 Code Grant" off.
- **Permissions: `309237746688`.** The invite link carries this value; the application also computes the bot's effective permissions in the target channel (owner, administrator, `@everyone`, role and member overwrites) and names what is missing before anything is posted.

| Permission | Bit | Value | Why |
|---|---|---|---|
| VIEW_CHANNEL | 1<<10 | 1024 | See the target channel |
| SEND_MESSAGES | 1<<11 | 2048 | Post the summary in the channel (also enough to unarchive an unlocked thread) |
| ATTACH_FILES | 1<<15 | 32768 | Attach screenshots |
| READ_MESSAGE_HISTORY | 1<<16 | 65536 | Conservative: create a thread from an existing message and look messages up. Dropped if the release check shows it is unneeded; the core set would then be `309237681152` |
| CREATE_PUBLIC_THREADS | 1<<35 | 34359738368 | One thread per meeting |
| SEND_MESSAGES_IN_THREADS | 1<<38 | 274877906944 | Post inside the thread; SEND_MESSAGES does not cover threads |

The sum is 309237746688.

**Deliberately not requested:**

| Permission | Reason it is left out |
|---|---|
| ADMINISTRATOR | Never. |
| MANAGE_MESSAGES | A bot can edit and delete its own messages without it. |
| MANAGE_THREADS | Only needed to touch locked threads or to delete a thread; the release check leaves its thread archived. |
| CREATE_PRIVATE_THREADS | The threads are public. |
| MENTION_EVERYONE | Needed only to ping roles that are not marked mentionable. `@everyone` and `@here` are neutralised anyway; the documented advice is to make the role mentionable. |
| EMBED_LINKS | Link previews are suppressed on purpose. |

- **Mentions.** Every message sets `allowed_mentions` explicitly (never the default, which would parse users, roles and everyone), and mentions come only from the participant mapping in `cycle.yaml`.
- **Idempotency of a post.** Each message carries a `nonce` (at most 25 characters, derived from the meeting, the operation and the piece index) and `enforce_nonce: true`, so Discord returns the earlier message instead of a duplicate when an answer was lost. The dedup window is short, so the stored progress (message ids per piece) remains the main protection.
- **One thread per meeting.** The summary goes to the channel, a public thread is created from that message (the thread id equals the message id; text and announcement channels only, not forum or media channels), and the remaining text, issue links and screenshots go into the thread. A second create for the same message (error 160004) counts as success; an archived thread is unarchived once and retried.
- **Edit after a correction, in bot mode only.** When a correction is published and the Discord target is a bot, the summary message is edited in place (deterministic re-render plus a "Corrections" block) and one reply is posted in the thread. A bot token cannot edit a webhook message, so webhook mode does not edit in this stage; editing through the webhook's own id and token is possible and is recorded as a later option.

### 4. Configuration

`targets.discord` holds exactly one of `webhook` or `bot` (`token` as a `cred:` reference, `guild`, `channel`, `thread`, `edit_on_correction`); `targets.notion` holds `token`, `database`, an optional `data_source`, `publish` and a `properties` mapping. Neither contains a secret. The default publish rule stays `on-approval` for both (0019).

## Alternatives considered

- **A gateway bot with the `GUILD_MEMBERS` intent.** Lists and caches all members and could resolve names by substring. Rejected: it needs a long-running connection and a privileged intent that must be switched on by the owner (and, past a size threshold, approved by Discord), the application is not a server, and prefix search on a typed name covers the use. If prefix search ever starts requiring the intent, this is the fallback, together with entering a user id by hand.
- **Editing through the webhook.** Discord supports `PATCH /webhooks/{id}/{token}/messages/{id}` (with `thread_id` for a message in a thread), which would give editing without a bot. Not done in this stage because it needs the webhook message id of every post (`?wait=true`) to be stored and gives no threads or member search; kept as a later option.
- **Notion `database_id` as the page parent.** Still accepted by the schema today, but it breaks as soon as a database has a second data source and is the legacy form. Rejected in favour of `data_source_id`, resolved and cached per run.
- **A Notion `unique_id` property as the key.** Assigned by Notion, so it cannot carry our own key. Rejected for a `rich_text` property.
- **A task exporter per service with its own idempotency logic.** More code to test and more ways to create duplicates. Rejected for one shared algorithm.
- **Broad bot permissions (administrator or "manage" permissions).** Easier setup, much larger damage if the token leaks. Rejected; the application explains each permission and checks them.

## Consequences

- A re-export, a retry, a restart or a lost answer does not create a second Notion page, and a Discord post resumes from where it stopped; the price is one more table and one more pair of operation kinds in the outbox.
- A team that wants threads, members mapped to Discord users and edits after a correction must create a bot, invite it with the generated link and store its token in the application; a team that only wants a post keeps the webhook.
- The token of the bot and of the Notion integration are stored in the Credential Manager under the `target:` namespace, validated by shape on save, and redacted from every log, error and stored result.
- Notion pages carry names, task texts and verified quotes into a workspace whose sharing the application cannot read, so the default rule stays `on-approval`.
- The Notion version and the permission value are single constants; changing either is a one-line change plus the release check.
- Everything is tested against in-test fakes. What the fakes cannot prove is listed next and is covered by release checks that are written but not run.

## Verification status

### Read in the documentation on 2026-11-21

- Search Guild Members is listed among the operations that need no privileged intent; only List Guild Members needs `GUILD_MEMBERS`. The reference entry for the search itself states no requirement; the guide on privileged intents is the evidence. It matches prefixes of username or nickname, with a limit of 1 to 1000 (default 1), and the bot must be in the guild.
- REST operations are independent of the gateway and its intents.
- Creating a thread from a message is REST (thread id equals message id, one thread per message, public thread in a text channel); creating one without a message defaults to a private thread, so the type is sent explicitly if it is ever used.
- Create Message: omitting `allowed_mentions` parses users, roles and everyone.
- Only the author of a message can edit its content; the author of a webhook message is the webhook.
- Message content limit 2000 characters, thread name 1 to 100, rate-limit headers and `Retry-After` on 429; limits are not hard-coded.
- The permission bit positions in the table above.
- Notion: version 2026-03-11 is the current one; data sources, the query endpoint, the page parent, the property value shapes, the filter for a `rich_text` equality, the text and array limits, `Retry-After` on 429, and the setup steps (capabilities, sharing the database with the integration; an unshared database answers 404).

### Must be confirmed by the release check on a real server

1. Member search returns the expected member **without any privileged intent enabled** (the documentation evidence is indirect).
2. `nonce` with `enforce_nonce` returns the same message for the same author and nonce, and how long the dedup lasts.
3. No gateway connection is needed before the first message (an old requirement that the current documentation no longer states).
4. Editing the bot's own message needs no permission beyond those above.
5. `READ_MESSAGE_HISTORY` is actually necessary (if not, it is dropped and the value becomes `309237681152`).
6. The documentation pages do not state which permissions the thread endpoints and `SEND_MESSAGES_IN_THREADS` on message creation need; the table follows the permission semantics and must be proved by a post, a thread, a reply and an edit.
7. For Notion: a query immediately after a create may not see the new page (not documented); the stored id and the back-off cover it, and the check proves the version header is accepted and that exporting twice leaves one page.

## Discrepancies with older knowledge

- **Notion data sources.** Older material (and the 2022-06-28 version) has `POST /databases/{id}/query` and `database_id` parents. Since 2025-09-03 pages belong to data sources and queries go to `/data_sources/{id}/query`.
- **Notion version.** The current version is 2026-03-11, not 2022-06-28; 2026-03-11 also renamed `archived` to `in_trash`. The version is one constant.
- **Notion rate limits.** Not a flat three requests per second: 180 requests per minute per connection (600 for Business and Enterprise), a shared per-workspace budget, and about three requests per second for data source queries. The pacing and the `Retry-After` handling account for both.
- **Discord documentation moved** from `discord.com/developers/docs` to `docs.discord.com/developers`; older links and community posts (for example a report that threads cannot be edited through a webhook, and the advice to open the gateway once before sending) may no longer apply and are covered by the release check.
