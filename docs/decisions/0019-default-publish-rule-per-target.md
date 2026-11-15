# 0019. Default publish rule per target

**Status:** Proposed (awaiting confirmation at the Stage 5 stop-and-ask point "default rule per target"). The mechanism is described in [0018](0018-publishing.md) and is not affected by this decision: only the defaults are.

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

Proposed defaults, one constant each (`DEFAULT_GITHUB_PUBLISH` and `DEFAULT_DISCORD_PUBLISH` in `crates/cycle-store/src/cycle.rs`), read by the parser of `cycle.yaml` and by the publish rules alike:

| Target | Covers | Default | Cycle without the key |
|---|---|---|---|
| `github` | The commit with the report, transcript, screenshots and `topics.yaml`, the issues, and the write-back of approved status changes | `on-approval` | The report target exists and uses the default; it needs a repository with a GitHub remote only for the issues |
| `discord` | The summary message with mentions and screenshots | `on-approval` | There is no Discord target at all: nothing is posted |
| Status changes | Closing or reopening issues and topics | Never `auto` | Written back only when the user approved the proposal **and** pressed Publish, whatever the rule of `github` |

Rules that hold for every value:

1. **Status proposals are never approved by the application.** A model's statement that a task is finished is not enough to close an issue of other people. With `auto`, a meeting still publishes its commit, issues and Discord message, and the status changes wait for an explicit publication.
2. **The public-repository gate applies to `auto`.** A repository that is public, or whose visibility is unknown, blocks an automatic publication until the user has confirmed that repository once.
3. **`off` is absolute.** An explicit Publish does not override it; the publish plan notes that the target was left out.
4. **A target that is not in the cycle does not exist.** The only implicit target is the report of the cycle's own repository.

### How to change it

Nothing here is code that a user has to touch:

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
- A team that has used the application for a while and trusts the extraction can switch one target to `auto` in one line, per cycle, and can switch back.
- The behaviour is the same as the one the parser of `cycle.yaml` already had for a missing `publish` since Stage 4, so existing cycle files are not changed by this decision.

## Alternatives

- **`auto` for the report, `on-approval` for issues and Discord:** the report is the most useful thing to publish on its own, but it is also the part that contains the transcript. A split would need a second rule for `github`; `targets.github.issues: false` already covers the case where only the report should go out.
- **`auto` for Discord (a quick note to the channel):** the only message that people read at once, and the one that pings them. A wrong mention cannot be taken back.
- **`auto` everywhere with a delay (an undo window):** the outbox could hold operations for a few minutes. It is more machinery for something a click does, and an application that is closed during the window would either publish late or not at all.
- **No default (the key is required):** a cycle file would be longer and an existing file would stop validating.
- **Status changes written back under `auto`:** rejected, see rule 1.
