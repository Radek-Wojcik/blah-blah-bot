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
meetings/
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
  github: { path: meetings, publish: auto }
  discord: { webhook: "cred:discord/nomad", publish: on-approval, template: short }
  notion: { database: "cred:notion/nomad-db", publish: on-approval }
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
| `targets.github.path` | relative path | `meetings` | Folder for meeting reports. |
| `targets.github.publish` | `auto`, `on-approval`, `off` | `on-approval` | Whether the report is committed automatically, after approval in the draft view, or not at all. |
| `targets.discord.webhook` | `cred:` reference | required | Name of a secret in the Windows Credential Manager, for example `cred:discord/nomad`. A URL is rejected: the webhook URL never appears in the repository. |
| `targets.discord.publish` | as above | `on-approval` | |
| `targets.discord.template` | template name | none | Separate, usually shorter, template for the Discord message. |
| `targets.notion.database` | text | required | Database id, or a `cred:` reference to it. |
| `targets.notion.publish` | as above | `on-approval` | |

### Checks beyond the schema

* Participant ids are unique.
* Every template named in `report.by_type` exists in `templates/`.
* `report.default_template` exists when `default_mode` is `template` or `by-type`.
* `id` equals the directory name.

A cycle that fails a check is still listed, with its issues (each with the path of the offending value, for example `participants[2].id`), so a broken cycle never hides the others. It cannot be used for a recording until it is fixed.

### Writing

The application writes `cycle.yaml` only after validation. Keys are written in the order shown above and map entries are sorted, so saving an unchanged cycle produces an unchanged file and Git diffs show only real changes. Comments in a hand-edited file are not preserved when the application saves it.

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

Open topics go into the extraction context. Concurrent edits are resolved last write wins per topic.

## `cycles/<ID>/templates/<name>.md`

A template is Markdown with `{placeholders}` and an optional YAML front matter that declares sections. The file name without `.md` is the template name. Every new cycle gets `default.md`:

```markdown
# {title}

{date} · {participants}

## Summary
{summary}

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
| `{summary}` | The meeting summary. |
| `{sections}` | Every non-empty section as a `## Title` heading followed by its list of items. |
| `{section:<id>}` | The list of items of one section, without a heading. An id the meeting does not have gives an empty result. |
| `{tasks}`, `{decisions}`, `{topics}`, `{notes}`, `{ideas}`, `{budgets}` | The items of that type as a list, without a heading. |
| `{transcript_link}` | Link target of the transcript, `transcript.md` by default. |

* Writing `{{` or `}}` gives a literal brace.
* An unknown or malformed placeholder is an error when the template is saved or rendered; it is never dropped silently. When the front matter declares sections, `{section:<id>}` must name one of them.
* After substitution, headings of level 2 and deeper with nothing under them are removed, and runs of blank lines become one, so empty sections never appear.

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

### Built-in sections (fixed mode)

Items are grouped by type, in this order, with headings in the report language; empty groups are omitted.

| Type | English | Polish |
|---|---|---|
| Summary (the `{summary}` text) | Summary | Podsumowanie |
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

The structured result of one meeting, described by `schemas/meeting.schema.json`. The extraction context of the next meeting is built from the `meeting.json` files of earlier meetings of the cycle: newest first, at most 80 items, without items that relate to a closed topic or issue and without topics whose text equals a closed topic's. A file that does not validate is skipped with a warning and never stops the briefing.

## Offline behaviour

* The working tree is the cache. Everything can be read and edited without network access.
* Before a briefing the application runs `git pull --rebase` with uncommitted edits set aside and restored. A failure (no network, no upstream, a conflict) never discards anything: the clone stays as it was, an interrupted rebase is aborted, and the reason is recorded.
* The date of the last successful pull (or clone) and the last error are kept in `.git/bbb-cache.json`, and the application shows it as "state as of <date>".
* Open issues come from the GitHub REST API (`GET /repos/{owner}/{name}/issues?state=open&labels=cycle:<id>`, pull requests excluded, up to 500 issues). A failed request returns the last cached list with its date. The cache is `.git/bbb-issues-<ID>.json`. Issues are read only.
* A cycle created without a remote lives in a local repository; there is nothing to pull and no error is recorded.

## What is committed and what is not

| In the repository | Not in the repository |
|---|---|
| `cycle.yaml`, `topics.yaml`, templates | Voice profiles and embeddings |
| Reports, transcripts, `meeting.json`, screenshots (written by the publishers) | Audio, work files (`work/draft.json`, `work/context.json`) |
| | Secrets: Discord webhook URLs, API tokens, keys (only `cred:` references appear) |
| | The pull state and the issue cache (inside `.git`, never in the working tree) |

Edits made in the application (cycle settings, topics, templates) are written to the working tree and are not committed by the application; committing and pushing is done by the publishers, which add only new files per meeting.
