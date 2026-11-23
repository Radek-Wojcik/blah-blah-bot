# Cycle repository format

A *cycle* is a recurring series of meetings that share participants, vocabulary and report format. Everything the team shares about a cycle lives in a Git repository, so reports and tasks can be read and changed without the application. This page is the reference for that repository. The design background is in [ADR 0009](decisions/0009-git-as-source-of-truth.md) and [ADR 0017](decisions/0017-cycles-extraction-and-drafts.md); the code is the `cycle-store` crate.

Machine-readable definitions: `schemas/cycle.schema.json`, `schemas/topics.schema.json`, `schemas/meeting.schema.json`.

## Layout

```
cycles/
  NOMAD/
    cycle.yaml            # participants, glossary, defaults, targets
    topics.yaml           # open and closed topics
    templates/
      default.md          # created with the cycle
      budget-review.md
meetings/                 # `targets.github.path`; `meetings` unless the cycle says otherwise
  NOMAD/
    2026-11-13/
      report.md           # generated report, never parsed back
      transcript.md       # linked from the report
      meeting.json        # structured items; the next meeting's context is built from it
      img/
```

* The directory name under `cycles/` is the cycle id and must equal `id` in `cycle.yaml`.
* A directory under `cycles/` without a `cycle.yaml` is ignored.
* A second meeting on the same day is stored in a folder with a suffix: `2026-11-13-2`.
* The application creates a repository itself (a local repository without a remote) or clones an existing one. Text encodings are UTF-8; a byte order mark and Windows line endings are accepted when reading.

## `cycles/<ID>/cycle.yaml`

```yaml
id: NOMAD
name: Nomad rover
languages:
  asr: auto
  output: auto
participants:
  - id: alex
    name: Alex Example
    github: alex-example
    discord: { user_id: "000000000000000000" }
glossary:
  - term: CAN bus
  - term: GNSS
    hint: positioning module
report:
  default_mode: fixed
  default_template: default
  instruction: "Group tasks by person, keep it short."
  by_type:
    budget: budget-review
targets:
  github: { path: meetings, publish: on-approval }
  discord: { webhook: "cred:discord/nomad", publish: on-approval, template: short }
  notion: { token: "cred:notion/nomad", database: "0123456789abcdef0123456789abcdef", publish: on-approval }
```

A Discord target posts through a webhook (above) or through a bot, and a Notion target needs the token of an integration and the database it exports tasks to. The bot and the full Notion mapping look like this:

```yaml
targets:
  discord:
    bot:
      token: cred:discord-bot/team          # secret reference, never the token
      guild: "111111111111111111"           # server id, quoted
      channel: "222222222222222222"         # text or announcement channel id, quoted
      thread: true                          # default true: one thread per meeting under the summary
      edit_on_correction: true              # default true: edit the summary when a correction is published
    publish: on-approval
  notion:
    token: cred:notion/nomad
    database: "0123456789abcdef0123456789abcdef"
    data_source: "fedcba98-7654-3210-fedc-ba9876543210"   # only when the database has several data sources
    publish: on-approval
    properties:                             # Notion property names
      title: Name                           # type title (default "Name")
      key: BBB key                          # type rich_text (default "BBB key"), holds the idempotency key
      assignee: Assignee                    # rich_text, select or multi_select
      due: Due                              # date
      status: Status                        # status or select, set when a page is created
      status_value: Not started             # required when status is set
      meeting: Meeting                      # url or rich_text
      cycle: Cycle                          # select or rich_text
```

Only `id` and `name` are required. Unknown keys are rejected, so a misspelled key is reported instead of ignored. Identifiers (`id`, participant ids, template names, meeting types) match `^[A-Za-z0-9][A-Za-z0-9_-]*$`; they are used in file names and keys, so spaces, dots and slashes are not allowed.

| Field | Type | Default | Description |
|---|---|---|---|
| `id` | identifier | required | Cycle id. Also the issue label `cycle:<id>` and the folder name. |
| `name` | text | required | Display name. |
| `languages.asr` | `auto`, `pl`, `en` | `auto` | Transcription language. |
| `languages.output` | `auto`, `pl`, `en` | `auto` | Report language; `auto` follows the dominant language of the meeting. Can be overridden per meeting. |
| `participants` | list | empty | The cycle's people. Voice profiles are keyed by participant id within the cycle. |
| `participants[].id` | identifier | required | Unique within the cycle. |
| `participants[].name` | text | required | Name shown in transcripts and reports. |
| `participants[].github` | GitHub login | none | Assignee of the participant's task issues. |
| `participants[].discord` | map | none | Exactly one of `user_id` or `role_id`, a Discord snowflake. Quote the number: unquoted, YAML reads it as a number and the file is rejected. |
| `glossary` | list | empty | Terms passed to the extraction prompt (and optionally to transcription). |
| `glossary[].term` | text | required | The term as it is written. |
| `glossary[].hint` | text | none | A short explanation. |
| `report.default_mode` | `fixed`, `template`, `auto`, `by-type` | `fixed` | Report mode offered before recording. See [Report modes](#report-modes). |
| `report.default_template` | template name | `default` | Template used by `template` mode, as the fallback of `by-type`, and for the layout of the other modes. |
| `report.instruction` | text | empty | Natural-language instruction given to the extraction, for example "Group tasks by person". |
| `report.by_type` | map | empty | Meeting type (for example `technical`, `budget`, `casual`) to template name. |
| `targets.github.path` | relative path | `meetings` | Folder for meeting reports: a meeting is committed to `<path>/<ID>/<date>/`. Relative to the repository, names separated by `/` and made of letters, digits, `.`, `_` and `-`; no `.` or `..` name, no leading `/`, no drive letter or backslash, no `.git`, no name ending in a dot. Meetings already published under another path stay where they are, and both folders are read for the context of the next meeting. |
| `targets.github.publish` | `auto`, `on-approval`, `off` | `on-approval` | Whether the report, the issues and the topics (and the corrections to a published meeting) are published automatically when the draft is complete, when you press Publish, or not at all. You choose it; until you do, `on-approval` is used. Status changes are never written back automatically. A cycle without `targets.github` still has this target with the default. See [Publishing rules](#publishing-rules). |
| `targets.github.issues` | `true`, `false` | `true` | Whether task items become GitHub issues and approved status changes are written back to them. With `false` only the report is committed. |
| `targets.discord.webhook` | `cred:` reference | none | Exactly one of `webhook` and `bot` is required. Name of a secret, for example `cred:discord/nomad`. A URL is rejected: the webhook URL never appears in the repository. The webhook is entered in the publishing settings and stored in the Windows Credential Manager under `target:<name>` (`target:discord/nomad`), a namespace of its own, so a reference in a repository cannot reach any other secret of the application. |
| `targets.discord.bot.token` | `cred:` reference | required with `bot` | Name of the bot token, for example `cred:discord-bot/team`. A token itself is rejected. Stored in the Windows Credential Manager under `target:<name>` like the webhook. |
| `targets.discord.bot.guild` | Discord snowflake | required with `bot` | The server id. Quote the number. |
| `targets.discord.bot.channel` | Discord snowflake | required with `bot` | The text or announcement channel the summary is posted to. Quote the number. |
| `targets.discord.bot.thread` | `true`, `false` | `true` | One thread per meeting under the summary. Left out of the file while true. |
| `targets.discord.bot.edit_on_correction` | `true`, `false` | `true` | Edit the summary when a correction is published. Left out of the file while true. |
| `targets.discord.publish` | as above | `on-approval` | A cycle without `targets.discord` has no Discord target: nothing is posted. |
| `targets.discord.template` | template name | none | Separate template for the Discord message; without it the built-in short message is used (one message, see below). A template of your own should stay short as well: a long one is split into several messages. |
| `targets.notion.token` | `cred:` reference | required | Name of the integration token, for example `cred:notion/nomad`. A token itself is rejected. Stored in the Windows Credential Manager under `target:<name>`. |
| `targets.notion.database` | Notion id | required | The database id: 32 hexadecimal digits, with or without dashes. It is not a secret. |
| `targets.notion.data_source` | Notion id | none | The data source id; needed only when the database has more than one. |
| `targets.notion.publish` | as above | `on-approval` | A cycle without `targets.notion` has no Notion target. Status changes are never exported. |
| `targets.notion.properties.title` | property name | `Name` | The page title property (type `title`). Left out of the file while default. |
| `targets.notion.properties.key` | property name | `BBB key` | The `rich_text` property that holds the key which makes an export idempotent. |
| `targets.notion.properties.assignee`, `due`, `status`, `meeting`, `cycle` | property name | none | Optional fields: a field without a property name is not exported. |
| `targets.notion.properties.status_value` | text | none | The status a new page gets; required when `properties.status` is set. |

### Checks beyond the schema

* Participant ids are unique.
* Every template named in `report.by_type` exists in `templates/`.
* `report.default_template` exists when `default_mode` is `template` or `by-type`.
* `id` equals the directory name.
* `targets.discord` has exactly one of `webhook` and `bot`.
* `targets.notion.properties.status_value` is set when `properties.status` is.

A cycle that fails a check is still listed, with its issues (each with the path of the offending value, for example `participants[2].id`), so a broken cycle never hides the others. It cannot be used for a recording until it is fixed.

### Writing

The application writes `cycle.yaml` only after validation. Keys are written in the order shown above and map entries are sorted, so saving an unchanged cycle produces an unchanged file and Git diffs show only real changes. Comments in a hand-edited file are not preserved when the application saves it.

## Publishing rules

`targets.<name>.publish` decides when a meeting goes to a target:

| Value | Meaning |
|---|---|
| `auto` | Published as soon as the draft is complete (the pipeline finished and no newer draft waits for your decision). |
| `on-approval` | Published when you press Publish for that target in the draft view. |
| `off` | Never, even when you press Publish. |

The defaults are `on-approval` for both targets ([ADR 0019](decisions/0019-default-publish-rule-per-target.md), accepted) and are constants in `crates/cycle-store/src/cycle.rs`, not part of `cycle.yaml`. A default is not a choice: a target that was just switched on, or a file without `publish`, behaves as `on-approval`, and a rule is `auto` only when somebody set it. Three things hold for every value:

* Status changes proposed from the transcript (an issue or topic reported as finished) are written back only when you approved that proposal and pressed Publish. `auto` does not publish them.
* A repository that is public, or whose visibility cannot be read, needs your confirmation once before anything is published to it, also under `auto`.
* After a meeting is published (or while any of its operations is queued) its draft is read-only.

In the application the rule is a switch of three positions, **Off**, **On approval** and **Automatic**, in the **Targets** section of the cycle editor (where a GitHub or Discord target is switched on it shows **On approval** and is saved so unless you pick another), in the publishing panel of a draft and in **Settings**, **Publishing**, **When to publish**. Under the switch each rule is explained by what it does to you (Off: nothing is ever published, not even when you press Publish; On approval: nothing leaves the computer until you press Publish; Automatic: published as soon as the draft is complete, cannot be taken back, the people named are notified). A change goes through the `set_target_rule` command, which writes `targets.<name>.publish` and nothing else of `cycle.yaml` (a cycle without `targets.github` gets one with the default folder; Discord needs a target with a webhook reference, which the cycle editor adds). The panel says next to the switch that status changes are never written back automatically.

`github` covers the commit with the report, the transcript, the screenshots and `topics.yaml`, the issues, and the write-back of approved status changes. With `targets.github.issues: false` only the commit is made. Without a remote the commit stays in the local repository and no issues are created.

Before anything is published the panel shows `topics.yaml` before and after, topic by topic: the remote's version as last fetched (no network is used to look) against the version the commit writes, with the topics where the remote changed the same topic since your last pull marked, because your version replaces theirs. When the file cannot be compared, for example because the remote's `topics.yaml` is not readable, the topics are named instead and a note says so. The commit itself starts from a fresh fetch, so what a colleague pushed in the meantime shows up in the result.

### What is written to the repository

Only new files go into the meeting folder `meetings/<ID>/<folder>`: `report.md`, `transcript.md`, `meeting.json` (the draft with its decisions on status changes) and `img/` with the screenshots that are not inside an excluded passage. The folder is the date of the meeting, with a suffix `-2`, `-3` if another meeting of the cycle took the date (chosen against the remote at publishing time). `transcript.md` has one line per segment with an anchor `t-hh-mm-ss` and the speaker's name, and leaves out passages you excluded. `topics.yaml` is the only existing file that changes (below). Other edits to the cycle (`cycle.yaml`, templates) are not committed by the application; commit them yourself.

### Issues

One issue per task item, labeled `cycle:<ID>`, assigned to the participant's `github` login when there is one. The body has the text, the due date (in the organization issue field "Target date" when the repository's organization has one, otherwise a `Due: YYYY-MM-DD` line), a link to the report and to the passage of the transcript, the quote, and a hidden marker:

```
<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 t=745000 -->
```

The marker links the issue to its item and keeps a retry from creating a second issue; do not delete it. A status change is written back as a close (reason completed) or reopen plus a comment that carries its own marker (`<!-- bbb:v1 status meeting=... proposal=s2 -->`). Closing an issue on GitHub makes its item count as done in the next briefing.

### Corrections to a published meeting

A published draft is locked. When something changed afterwards, press **Add correction** under the draft: a text, the item and/or issue it is about (optional) and the day (today by default). The note is kept in `work/corrections.json` of the meeting in the application (`schemas/corrections.schema.json`), counts for the next meeting at once, and is published through the same outbox, under the rule of the `github` target:

| Rule | A correction is published |
|---|---|
| `auto` | When you add it. |
| `on-approval` | When you press **Publish** next to it (or **Publish all**). |
| `off` | Never; it stays on the computer and still reaches the next meeting. |

The public-repository confirmation applies as for the meeting, and the meeting has to be in the repository (a Discord-only publication has no folder to add to). Publishing adds one **new file** to the folder of the meeting, next to `meeting.json`: `corrections.md`, then `corrections-2.md`, `corrections-3.md`, and so on. Existing files are never changed:

```
meetings/NOMAD/2026-11-13/
  report.md  transcript.md  meeting.json  img/
  corrections.md          <- first correction
  corrections-2.md        <- second
```

```markdown
<!-- bbb:v1 correction cycle=NOMAD meeting=20261113-140501 id=c1 date=2026-11-16 about=i3 issue=12 -->
# Correction to the meeting of 2026-11-13

Written on 2026-11-16. Refers to item i3 and issue #12.

> The mount is redesigned, the new drawing is attached to the issue.
```

When the note is about an item that became an issue, or names an issue, the same text is also posted as a comment on that issue (once, by its own marker; the issue stays open or closed as it was). A correction that was queued cannot be edited or deleted; one that was never queued can be deleted.

For the next meeting the context gets an optional `corrections` list (`schemas/cycle-context.schema.json`): `meeting` (its date), `date`, `text`, the text of the `item` and the `issue`, newest first, at most 30, taken from the `corrections*.md` files of the earlier meetings in the repository and from the local notes (not yet published ones too). The extraction prompt shows them as information that replaces what the items of those meetings say; a cycle without corrections gets the context and the prompt it always had.

## `cycles/<ID>/topics.yaml`

A list of topics that outlive a single meeting. A new cycle gets an empty list (`[]`).

```yaml
- id: t-1
  text: Decide the rover wheel size
  status: open
  opened_in: 2026-11-06
- id: t-2
  text: Order the battery
  status: closed
  opened_in: 2026-11-06
  closed_in: 2026-11-13-2
```

| Field | Description |
|---|---|
| `id` | Identifier, unique in the file. |
| `text` | The topic. |
| `status` | `open` or `closed`. |
| `opened_in` | Meeting folder name in which the topic was raised: `YYYY-MM-DD` or `YYYY-MM-DD-N`. |
| `closed_in` | Meeting folder name in which it was closed; optional. |

Open topics go into the extraction context. Concurrent edits are resolved last write wins per topic: when a meeting is published, a topic that was added or changed in your clone (or by the meeting, or by an approved status change) replaces the remote's version of that topic, and every other topic keeps the remote's version. When the remote changed the same topic too, yours wins and the overridden version is recorded in the result of the publishing operation. A topic that a meeting adds gets the id `<meeting id>-<item id>`, for example `20261113-140501-i4`.

## `cycles/<ID>/templates/<name>.md`

A template is Markdown with `{placeholders}` and an optional YAML front matter that declares sections. The file name without `.md` is the template name. Every new cycle gets `default.md`:

```markdown
# {title}

{date} · {participants}

{summary_section}

{sections}

---
[Full transcript](transcript.md)
```

### Front matter

If the file starts with a line `---`, everything up to the next line `---` is YAML:

```markdown
---
sections:
  - id: progress
    title: Progress
    description: What was done since the last meeting
  - id: blockers
    title: Blockers
    description: What stops the work
---

# {title}

{section:progress}

{section:blockers}
```

* `sections[].id` starts with a lowercase letter and contains lowercase letters, digits, `_` and `-`; ids are unique.
* `sections[].title` is the heading in the report.
* `sections[].description` tells the model what belongs in the section; optional.
* Only `sections` is allowed in the front matter. A template without front matter has no sections of its own (the built-in sections, or sections proposed in `auto` mode, are used).

### Placeholders

| Placeholder | Replaced by |
|---|---|
| `{title}` | Meeting title. |
| `{date}` | Meeting date, `YYYY-MM-DD`. |
| `{participants}` | Names of the cycle's participants, comma separated. |
| `{summary}` | The meeting summary as bare text. |
| `{summary_section}` | The summary under a localised heading, `## Summary` or `## Podsumowanie` (by the language of the draft); nothing at all, heading included, when the summary is empty. |
| `{sections}` | Every non-empty section as a `## Title` heading followed by its list of items. |
| `{section:<id>}` | The list of items of one section, without a heading. An id the meeting does not have gives an empty result. |
| `{tasks}`, `{decisions}`, `{topics}`, `{notes}`, `{ideas}`, `{budgets}` | The items of that type as a list, without a heading. |
| `{transcript_link}` | Link target of the transcript, `transcript.md` by default. |
| `{report_link}` | Link target of the full report, `report.md` by default; the Discord message gets the address of the published report. |

* Writing `{{` or `}}` gives a literal brace.
* An unknown or malformed placeholder is an error when the template is saved or rendered; it is never dropped silently. When the front matter declares sections, `{section:<id>}` must name one of them.
* After substitution, headings of level 2 and deeper with nothing under them are removed, so empty sections never appear. A heading that is followed only by placeholders that came out empty is removed too, even when other text follows after a blank line (`## Tasks`, `{tasks}`, then `[Full report]({report_link})`: a meeting without tasks has no `## Tasks` heading in the Discord message). Put two placeholders under one heading on lines of one block (no blank line between them) so that either of them keeps the heading. The report then has exactly one blank line between blocks (every heading has one before and after it), no trailing spaces and a single final newline.
* Cycles created before `{summary_section}` existed keep their own `default.md` (with `## Summary` and `{summary}`); it is not migrated. A report from such a template has an English summary heading in Polish too; replace those two lines with `{summary_section}` to localise it.

### Item lines

Every item is a bullet. Tasks carry their assignee and due date:

```
- [ ] Design the monitor mount (@Alex Example, due 2026-11-20)
- [ ] Order parts (@Sam Sample?, due 2026-11-27?)
- [ ] Check the wiring (@?, due ?)
- Wheels are 20 cm (unverified quote)
```

* A trailing `?` marks a value the model inferred; `@?` and `due ?` mark an unknown assignee or due date.
* `(unverified quote)` is appended when the quote of the item was not found in the transcript. Items added by hand have no marker.
* An assignee is shown by the participant's name.
* For Discord and Notion the `[ ]` checkbox is left out.
* The words `due` and `unverified quote` are the same in every report language, so item lines stay machine-friendly; only the built-in headings are localised.
* In a Discord message an assignee whose participant has `discord.user_id` or `discord.role_id` is written as a mention of that user or role instead of the name. Only those ids from `cycle.yaml` can notify anybody: the message sends an explicit `allowed_mentions` list, never `@everyone`, `@here` or other mention syntax from the transcript. Without `targets.discord.template` the built-in short message is used (title, date, summary, decisions, tasks and a link to the full report; decisions or tasks that the meeting does not have get no heading). **Discord messages stay short:** the built-in message is always one message of at most 2000 characters (counted in UTF-16 units). When a long meeting would make it longer, the lists of decisions and tasks are cut at a line, the note "shortened, the rest is in the full report" says so, and the link to the full report on GitHub stays at the end, followed by as many issue links as fit. A template of your own is not cut: it is split into several messages at 2000 characters, at a line, then at a word. Screenshots are attached in batches of at most 10 files and 8 MiB (a conservative cap: Discord documents 20 MiB per file and no total).

### Built-in sections (fixed mode)

Items are grouped by type, in this order, with headings in the report language; empty groups are omitted.

| Type | English | Polish |
|---|---|---|
| Summary (`{summary_section}`) | Summary | Podsumowanie |
| `decision` | Decisions | Decyzje |
| `task` | Tasks | Zadania |
| `topic` | Open topics | Otwarte tematy |
| `note` | Notes | Notatki |
| `idea` | Ideas | Pomysły |
| `budget` | Budgets | Budżety |

## Report modes

| Mode | Sections | Chosen by |
|---|---|---|
| `fixed` | The built-in sections. | The cycle default. |
| `template` | The `sections` of the template named by `report.default_template`; the model assigns each item to one. | The cycle default or the recording dialog. |
| `auto` | Proposed by the model from the extracted items; the user can rename, add, remove and reorder them in the draft. | The recording dialog. |
| `by-type` | The sections of the template that `report.by_type` maps the meeting type to. A type without a mapping falls back to `report.default_template`. | The meeting type at the report mode gate. |

Items whose section is empty, or names a section that does not exist, go to the built-in section of their type in `fixed` mode and to the last section otherwise.

The mode can be changed after recording while the draft is not published: the report is rendered again from the same extracted items.

## Meetings: `meetings/<ID>/<folder>/meeting.json`

The structured result of one meeting, described by `schemas/meeting.schema.json`, published as the draft with your decisions. It may contain `status_proposals`: proposed changes of the status of an open issue or topic, each with the model's reason and quote and your decision (`proposed`, `approved` or `rejected`). The extraction context of the next meeting is built from the `meeting.json` files of earlier meetings of the cycle: newest first, at most 80 items, without items that relate to a closed topic or issue and without topics whose text equals a closed topic's. A file that does not validate is skipped with a warning and never stops the briefing. The `corrections*.md` files next to a `meeting.json` (and the local notes of that meeting) are read with it, see [Corrections](#corrections-to-a-published-meeting).

## Offline behaviour

* The working tree is the cache. Everything can be read and edited without network access.
* Before a briefing the application runs `git pull --rebase` with uncommitted edits set aside and restored. A failure (no network, no upstream, a conflict) never discards anything: the clone stays as it was, an interrupted rebase is aborted, and the reason is recorded.
* The date of the last successful pull (or clone) and the last error are kept in `.git/bbb-cache.json`, and the application shows it as "state as of <date>".
* Issue state comes from the GitHub REST API (`GET /repos/{owner}/{name}/issues?state=all&labels=cycle:<id>`, pull requests excluded, most recently updated first, at most five pages) before a briefing and on a manual pull, for repositories linked to GitHub. A failed request keeps the last cached state with its date. The cache is `.git/bbb-issues-<ID>.json`: open issues, the numbers of closed ones and the links between issues and items. Reading issues does not depend on `targets.github.issues`.
* Publishing without a network is not lost: the operations wait in the outbox and are retried, see [ADR 0018](decisions/0018-publishing.md).
* A cycle created without a remote lives in a local repository; there is nothing to pull and no error is recorded.

## What is committed and what is not

| In the repository | Not in the repository |
|---|---|
| `cycle.yaml`, `topics.yaml`, templates | Voice profiles and embeddings |
| Reports, transcripts, `meeting.json`, screenshots, `corrections*.md` (written by the publishers) | Audio, work files (`work/draft.json`, `work/context.json`, `work/corrections.json`, the frozen snapshot in `work/publish/`) |
| | Secrets: Discord webhook URLs, API tokens, keys (only `cred:` references appear) |
| | The pull state and the issue cache (inside `.git`, never in the working tree) |

Edits made in the application (cycle settings, topics, templates) are written to the working tree and are not committed by the application. The publishers commit and push only the new meeting folder and the merged `topics.yaml`, and they build that commit without touching the working tree, so your uncommitted edits stay as they are.
