# 0019. Default publish rule per target

**Status:** Accepted (Stage 5). The mechanism is described in [0018](0018-publishing.md); this record decides the defaults, how the rule is chosen, and what happens to a draft after it was published.

## Context

Every publishing target has a rule in `cycle.yaml`, `targets.<name>.publish`, with three values:

| Value | Meaning |
|---|---|
| `auto` | The operations are queued as soon as the draft of a meeting is complete, with no click. A draft is complete when `reduce` has finished and no newer draft (`draft.new.json`) waits for a decision. |
| `on-approval` | The operations are queued when the user presses Publish for that target in the draft view. |
| `off` | Never, even when the user asks. |

A rule that a cycle does not state needs a default, and `auto` is the only value that sends data without a person looking at the result. What is published cannot be undone:

- A transcript pushed to a repository stays in its history, and it contains personal data. If the repository is public, it is public.
- An issue with an assignee sends that person an e-mail and a notification. Closing someone else's issue is visible to them.
- A Discord message with a mention pings a person or a role. A webhook cannot edit or delete what it posted.
- The values the model fills in, the assignee and the due date above all, are guesses that are marked as such in the draft. They are wrong often enough that an unchecked assignment is an annoyance for the person who receives it.

## Decision

Three decisions, confirmed for Stage 5:

1. **The defaults stay `on-approval`** for both targets, one constant each (below).
2. **The rule is chosen explicitly, per cycle and per target.** Nothing is ever published automatically because of a default or a guess: the interface shows the three rules, each with its consequences in plain words, wherever a target is switched on, and `on-approval` is what is used (and shown, and saved) until the user picks another.
3. **A published draft stays locked, and what changes afterwards is written as a correction.** The draft is the record of what was approved, so it is never edited or republished. A correction is a dated note, local to the meeting, that can be published as a new file next to the meeting and as a comment on the issue it is about, and that the following meetings are given as current information.

### The defaults

One constant each (`DEFAULT_GITHUB_PUBLISH` and `DEFAULT_DISCORD_PUBLISH` in `crates/cycle-store/src/cycle.rs`), read by the parser of `cycle.yaml` and by the publish rules alike:

| Target | Covers | Default | Cycle without the key |
|---|---|---|---|
| `github` | The commit with the report, transcript, screenshots and `topics.yaml`, the issues, and the write-back of approved status changes | `on-approval` | The report target exists and uses the default; it needs a repository with a GitHub remote only for the issues |
| `discord` | The summary message with mentions and screenshots | `on-approval` | There is no Discord target at all: nothing is posted |
| Status changes | Closing or reopening issues and topics | Never `auto` | Written back only when the user approved the proposal **and** pressed Publish, whatever the rule of `github` |

Rules that hold for every value:

1. **Status proposals are never approved by the application.** A model's statement that a task is finished is not enough to close an issue of other people. With `auto`, a meeting still publishes its commit, issues and Discord message, and the status changes wait for an explicit publication.
2. **The public-repository gate applies to `auto`.** A repository that is public, or whose visibility is unknown, blocks an automatic publication until the user has confirmed that repository once.
3. **`off` is absolute.** An explicit Publish does not override it, for a meeting or for a correction; the publish plan notes that the target was left out.
4. **A target that is not in the cycle does not exist.** The only implicit target is the report of the cycle's own repository.

### Choosing the rule

The three rules are shown, with what each means for the user, in the same words wherever they appear:

| Rule | What the interface says |
|---|---|
| Off | Nothing is ever published to this target, not even when you press Publish. |
| On approval | Nothing leaves this computer until you have looked at the draft and press Publish. This is what is used until you choose another rule. |
| Automatic | Published as soon as the draft is complete, without a click. It cannot be taken back, and the people it names are notified. A public or unconfirmed repository still waits for your confirmation. |

- **Where.** In the **Targets** section of the cycle editor, where a GitHub or Discord target is switched on (the switch shows **On approval** from the first moment and the cycle is saved with that value unless another is picked), in the **Publishing** panel of a draft, and in **Settings**, **Publishing**, **When to publish**. Under Automatic a warning says that it cannot be taken back.
- **Defaults are not choices.** A cycle file without `publish` behaves as `on-approval`, and so does a target that was just added. A rule is Automatic only when somebody set it.
- **Status changes** are never written back automatically, with any rule (rule 1 above).

### Corrections after publication

A published meeting is locked, in the state the user approved (ADR 0018). Real life moves on: a task is done, a date shifts, a decision is reversed. Editing the published draft would make the repository disagree with itself, republishing would rewrite history, and ignoring the change would give the next meeting wrong information. A **correction** keeps all three honest.

- **What it is.** A text, a date (the day it was written, today by default), and optionally the item of the meeting and/or the issue it is about. Stored in the meeting folder of the application as `work/corrections.json` (`schemas/corrections.schema.json`), outside the draft, so the draft and `draft.base.sha256` never change.
- **When it can be added.** Only to a meeting that is published or queued (its draft is locked); for any other meeting the draft is simply edited.
- **Publishing.** Through the same outbox, with the same rules and checks as the meeting, and under the rule of the `github` target: `auto` queues a correction when it is added, `on-approval` when the user presses Publish next to it, `off` never (it stays on the computer and still counts for the next meeting). The public-repository check applies as for the meeting (also under `auto`, in the plan, in the command and in the dispatcher), and the meeting has to be in the repository, or its commit queued.
- **What is written.** Only a **new file** in the meeting's folder: `corrections.md`, then `corrections-2.md`, `corrections-3.md`, ... A name that is taken by other content gets the next number; a file that is identical to one already there is reported as published and nothing is committed. `meeting.json`, `report.md` and every other file stay as they were. The file starts with a hidden marker (`<!-- bbb:v1 correction cycle=.. meeting=.. id=.. date=.. about=.. issue=.. -->`) and holds the text as a block quote, so a colleague's clone and the next briefing can read it back.
- **The issue.** A correction about an item, or naming an issue, is also posted as a comment on that issue (the issue made from the item when only the item is given), once: the comment ends with its own marker, so a retry does not repeat it. A comment never changes the state of an issue.
- **The record.** A correction that was queued is part of the record and cannot be changed or deleted by the application; one that was never queued can be deleted.
- **The next meeting.** The cycle context gains an optional `corrections` list (newest first, at most 30, absent when empty): the local notes of earlier meetings (published or not) and the `corrections*.md` files in their repository folders, one entry per correction id. The extraction prompt shows them under the items of earlier meetings as information that replaces them, only when there are some, so the prompt of a cycle without corrections is unchanged.


Nothing here is code that a user has to touch:

- **In the application:** the switch **Off / On approval / Automatic** of each target, in the cycle editor, in the publishing panel of a draft and in **Settings**, **Publishing**, **When to publish**, always with the three rules explained. Outside the cycle editor it writes only `targets.<name>.publish` (the `set_target_rule` command) and the panel repeats that status changes are never written back automatically.
- **Per cycle:** set `publish` under the target in `cycle.yaml`, in the cycle editor or by hand:

  ```yaml
  targets:
    github: { publish: auto }
    discord: { webhook: cred:discord/nomad, publish: on-approval }
  ```

- **For every cycle that does not say:** change the constants `DEFAULT_GITHUB_PUBLISH` and `DEFAULT_DISCORD_PUBLISH` in `cycle.rs`. They are the only place where a default is written; the parser's defaults and the rules read them. The tests that pin them (`the_defaults_are_one_constant_each_and_nothing_goes_out_on_its_own`) are changed together with them.
- **Without the commit:** `targets.github.issues: false` keeps the report and drops the issues and the status write-back.

## Consequences

- A new cycle publishes nothing until the user looks at the draft and presses Publish. The price is one click per meeting and target.
- A correction is one more click per note under `on-approval`, and none under `automatic`; with `off` it never leaves the computer.
- What was published is never rewritten. A mistake in a published meeting is fixed by a correction next to it, which costs a file and a comment and keeps the history honest.
- A team that has used the application for a while and trusts the extraction can switch one target to `auto` in one line, per cycle, and can switch back.
- The behaviour is the same as the one the parser of `cycle.yaml` already had for a missing `publish` since Stage 4, so existing cycle files are not changed by this decision.

## Alternatives

- **`auto` for the report, `on-approval` for issues and Discord:** the report is the most useful thing to publish on its own, but it is also the part that contains the transcript. A split would need a second rule for `github`; `targets.github.issues: false` already covers the case where only the report should go out.
- **`auto` for Discord (a quick note to the channel):** the only message that people read at once, and the one that pings them. A wrong mention cannot be taken back.
- **`auto` everywhere with a delay (an undo window):** the outbox could hold operations for a few minutes. It is more machinery for something a click does, and an application that is closed during the window would either publish late or not at all.
- **No default (the key is required):** a cycle file would be longer and an existing file would stop validating.
- **Status changes written back under `auto`:** rejected, see rule 1.
- **Unlocking the draft for edits and republishing:** a second commit that changes `meeting.json` and `report.md` breaks "only new files are added", rewrites what colleagues have pulled and what issues link to, and needs a merge of two versions of one meeting. Rejected.
- **A correction as an edited `meeting.json`:** the same reasons; a separate file is also what makes the correction visible as such.
- **One file with all the corrections of a meeting:** it would have to be changed every time. One file per correction is only ever added.
- **A default that depends on the repository (`auto` for private ones):** the visibility can change, and the rule would then change under the user. The user chooses; the gate for public repositories stays separate.

## Update: global default with a per-project override

The rule above is now the global default of the preferences (one per target, `on-approval` until changed). A project follows it while its `cycle.yaml` has no `publish` key for the target, and overrides it only with an explicit key; one resolver in `crates/publishers/src/rules.rs` (`resolve_rule`) decides and names the source. The three constants stay the defaults of the global rules. Existing files keep working: a missing key now means "use the global setting", which is `on-approval` by default, so behaviour does not change until the user changes a global rule. Projects that already store an explicit rule keep it; nothing is rewritten at start, and a project can go back to the global setting with one click.
