# 0018. Publishing: commit, issues, Discord and the outbox

**Status:** Accepted (Stage 5). The default rule per target is a separate record, [0019](0019-default-publish-rule-per-target.md), which is *Proposed*.

## Context

Stage 5 closes the loop: an approved draft becomes files in the cycle repository, GitHub issues and a Discord message. What is published cannot be taken back (a pushed transcript stays in the history, an assignment sends an e-mail, a Discord mention pings a person), the network is unreliable, and the tokens are the most sensitive data the application holds. The design therefore has to make publishing repeatable without duplicates, safe to retry, hard to do by accident to a public repository, and unable to leak a secret.

Nothing in this stage was verified against a real GitHub repository or Discord channel; see "Verification status" at the end.

## Decision

### Crates and what lives where

| Crate | Publishing role |
|---|---|
| `publishers` (new) | HTTP layer, GitHub and Discord publishers, per-target rules, public-repository check, publish plan, snapshot and lock (`flow`), outbox dispatcher, webhook validation, and (feature `test-support`) the fake servers |
| `cycle-store` | Everything that touches the cycle repository: the off-index commit and push (`publish`), the `topics.yaml` merge (`topics`), the issue markers and the issue cache (`issues`), the default-rule constants (`cycle`), the report and chat renderers |
| `store` | Schema v5: the `outbox`, `publications` and `repo_confirmations` tables and the outbox state machine |
| `queue` | The published transcript (`transcript::published_markdown`) and the draft operations for status changes |
| `app` | Glue only: the environment the dispatcher runs in (registered repositories, Credential Manager), the background thread, the IPC commands and events |
| `worker` | Status changes in `extract` and `reduce` |

The plan, the queueing and the dispatcher live in `publishers` and not in `app`, so the integration tests run the same code as the application.

### HTTP client: `ureq`, no async runtime

All requests go through one blocking client (`ureq` 3 with the platform TLS stack), the same dependency `cycle-store` already used for reading issues. The dispatcher runs on its own thread, so an async runtime would add weight without a benefit. JSON is handled with `serde_json`; the Discord `multipart/form-data` body is built by hand and covered by a golden test. The client enforces three rules for every publisher:

- **Safety.** `https` only (plain `http` only for this machine, which tests and local servers use), no credentials in the URL, redirects are never followed, so a token cannot be handed to another host. Responses are capped at 4 MiB.
- **Classification.** A failure is *transient* (no connection, timeout, 408, 5xx, 429, and a 403 that is a rate limit) or *permanent* (400, 401, any other 403, 404, 410, 422, a TLS error, a refused URL). A server-provided wait (`Retry-After`, `x-ratelimit-reset`, `retry_after` in a Discord body) travels with a transient failure.
- **Redaction.** Error texts are built from the application's own words plus a redacted excerpt of the underlying error. Token prefixes, `Authorization` values, base64 of credentials and the secret part of a webhook URL are masked before anything is stored or logged.

### Git: off-index commit, rebuild on rejection

The meeting is committed by shelling out to the `git` CLI (as ADR 0017 does for pull) with plumbing commands and a temporary index file, so that the user's working tree and real index are never read, staged or stashed:

```
fetch -> tip of the remote branch -> read-tree into a temporary index
      -> hash-object + update-index for the new files (and the merged topics.yaml)
      -> write-tree -> commit-tree -p tip -> push <commit>:refs/heads/<branch>
```

- Only new files go into the meeting folder `meetings/<ID>/<date>`; a second meeting on the same day gets `<date>-2`, `-3`, chosen against the tip when the commit is built.
- A push the remote refuses (somebody else pushed first) is not an error: the commit is **rebuilt on the new tip** and pushed again, up to three times, with `topics.yaml` merged again. For a commit that only adds files this is what a rebase and a retry would produce, without ever touching the clone. A remote that keeps changing makes the operation transient and the outbox tries again later.
- Publishing is idempotent: when the tip already holds the folder with identical files (the answer of an earlier push was lost) nothing is committed and the existing commit is reported.
- After the push the local branch is moved to the new commit with `git merge --ff-only`, after setting aside only those files the commit also contains where the working copy is byte-identical. Unrelated uncommitted files stay where they are. When the branch cannot be moved (unpushed local commits, a conflicting dirty file) the commit is still pushed and the interface says the clone is behind; the next pull brings it up to date.
- A repository without a remote receives a local commit and no push, and creates no issues.
- Credentials: the machine's Git credential helper, as for clone and pull. If a GitHub token is stored for the host and the remote is under that host, the token is handed to Git through the **environment** (`GIT_CONFIG_COUNT` and an `http.<base>.extraheader`), never on the command line (needs Git 2.31 or newer). Git's output is scrubbed of the token.
- Only the meeting folder and `topics.yaml` are committed. Other edits of the cycle (`cycle.yaml`, templates) stay in the working tree for the user to commit; the application does not stage them.

### Topics: last write wins per topic

`topics.yaml` is merged from three sides: the topics where the local work started (`base`), the topics now including those the meeting adds and the approved closings and reopenings (`ours`), and the topics on the remote (`upstream`). A topic that differs between `base` and `ours` is ours and overrides the remote; every other topic is the remote's. When the remote changed the same topic differently, ours still wins and a conflict (`id`, theirs, ours) is recorded in the result of the operation. Topics a meeting adds get the id `<meeting id>-<item id>`, so two meetings never collide. A diff of the planned change is available before publishing.

### Issues

- One issue per task item, labeled `cycle:<id>`, assigned to the participant's GitHub login (GitHub drops a login that cannot be assigned and the outcome records it).
- The issue links back to the meeting with a hidden marker in the body, `<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 t=745000 -->`, followed by the visible text, the due date, a link to the report and to the passage of the transcript, and the quote. The marker is the key of idempotency: every issue operation first searches the cycle's issues (all states, most recently updated first) for its marker and reuses what it finds, so a request whose answer was lost does not create a second issue. The list can lag behind a write by a moment, so a duplicate remains possible in that window.
- **Due date.** For a repository owned by an organization, the date is written to an organization issue field named "Target date" of type date, if there is one (found by a probe). In every other case, and on every error, it is a `Due: YYYY-MM-DD` line in the body. The endpoints (`GET /orgs/{org}/issue-fields`, `PUT /repos/{owner}/{repo}/issues/{n}/issue-field-values`) exist in the GitHub documentation, but whether the feature is generally available, which token permission it needs, and whether a given organization defines such a field could not be checked. Both paths are therefore implemented, the body line is the guaranteed one, and the field is unverified until the release check runs on a real organization.
- **Status write-back.** An approved status change closes (reason `completed`) or reopens the issue with a `PATCH` that is skipped when the issue already has that state, then adds a comment (the user's comment, else the model's reason, the quote and a transcript link) that carries its own marker, `<!-- bbb:v1 status meeting=.. proposal=s2 -->`, so a retry does not comment twice. For a topic the change goes into the `topics.yaml` of the commit.
- `@name` in text taken from the transcript gets a zero-width space so nothing notifies anybody.
- Writes are paced at one per second after a write that changed something.
- **Reading state.** Before the briefing and on a manual pull, the application reads the cycle's issues (all states, sorted by update, at most five pages) into the cache in `.git`. The cache holds the open issues, the closed numbers and the links between issues and items (from the markers). An item whose issue is closed is treated as done and leaves the next context; an item with an open issue appears in the context with its issue number. A failed read keeps the last cache and never stops a briefing. Reading is not limited by `targets.github.issues`.

### Status proposals

The `extract` stage returns `status_changes` (an open issue or topic of the prompt that the window reports as finished, with time and quote; required in the model answer, possibly empty). `reduce` keeps those whose target is an open issue, the issue of a previous item or an open topic, verifies the quote like an item's, keeps one proposal per target (the latest in the meeting, but a verified quote beats a later unverified one) and writes `status_proposals` into the draft, numbered `s1`, `s2`, and all `proposed`. They are never approved by the application: the user approves, rejects or comments in the draft view, and only an approved one is written back, only by an explicit publication, whatever the rule of the target. A proposal can also be added by hand (including reopening). A decided or edited proposal counts as a user edit, so the draft is protected from being replaced by a new one. Drafts without proposals keep byte-identical files.

### Discord

A webhook posts content-only messages rendered with the per-target template (`targets.discord.template`, else a built-in short template with title, date, summary, decisions, tasks and `{report_link}`) in the chat variant.

- An assignee whose participant has a `discord` user or role id is written as a mention. Mention syntax that comes from the model or the transcript is defused, and `@everyone` and `@here` are neutralized, so only mentions made from `cycle.yaml` exist in the content.
- Every message carries its own `allowed_mentions` with `parse: []` and exactly the user and role ids (at most 100 of each) that this message contains and `cycle.yaml` lists. No other mention can notify anybody.
- **Short messages.** The built-in template is one message: when the lists of a long meeting do not fit, they are cut at a line, a note says so and the link to the full report (and as many issue links as fit) stays at the end. A cycle's own template is split instead, because it is the user's choice.
- **Limits chosen.** Content is cut at 2000 UTF-16 units (never fewer than characters), at a line, then at a word, then anywhere, never inside a mention. Messages carry the flag that suppresses link previews. The planner supports 10 embeds per message but the publisher sends none. Screenshots go in separate messages of at most 10 files and 8 MiB, and at most three such messages; the rest is mentioned in the text and left to the report. A single file over 8 MiB is skipped with a note. Discord documents 20 MiB per file and no total for a message, and reports about webhooks disagree, so 8 MiB is a deliberately conservative choice that can be raised without changing the design.
- Messages are posted with `?wait=true` one after another; the outbox keeps the number sent, so a retry resumes after them. A 429 with a wait of at most five seconds is slept through (twice at most); a longer wait is handed to the outbox. A webhook has no idempotency key, so a timeout after Discord stored a message can produce a duplicate on retry; this was accepted over losing a message.
- A Discord post waits for the issue operations of its meeting so that it can link them. A failed or cancelled issue operation does not hold it back.

### The outbox

Publishing a meeting creates rows in the `outbox` table of the local database; a dispatcher thread carries them out. The rows hold ids, text of the meeting and the name of a frozen snapshot, never a secret.

- **Operations.** `commit_meeting`, `create_issue`, `set_issue_state`, `post_discord`, each with an `op_key` (`commit`, `issue:<item>`, `status:<proposal>`, `discord`) that is unique per meeting, so queueing the same plan twice changes nothing. Issue and status operations follow the commit, so links in them are valid; they do not follow each other, so one failing issue does not block the rest.
- **States.** `queued`, `running`, `done`, `failed`, `cancelled`. A transient failure puts an operation back to `queued` with a backoff of 30 s doubling up to 30 min (a server-provided wait replaces the backoff, clamped to between 1 s and 1 h) and counts an attempt; after 8 attempts, or on a permanent failure, it is `failed` and waits for the user, who can retry (attempts start over) or cancel. An operation left `running` by a crash goes back to `queued` at the next start without counting an attempt. A user can run a waiting operation right away.
- **Offline.** A request that gets no answer pauses the other operations of that target until the next tick (every 30 s or when something is queued), so a dead network costs one timeout per tick, not one per operation. The interface shows which targets are paused. Operations of the other targets continue: a dead Discord does not hold back the report.
- **Idempotency of every operation.** The commit finds its folder in the tip; an issue is found by its marker; a status change reads the issue state and the comment markers; a Discord post resumes after the messages that are out.
- **Snapshot.** When publishing starts, the draft (with the decisions), `report.md`, `transcript.md` and the screenshots outside excluded passages are copied to `work/publish/<n>/` of the meeting. Every operation reads from there, so a retry days later publishes what the user approved. The files of the draft itself are not touched.
- **Lock.** From the first non-cancelled operation of a meeting until all are cancelled, the draft is read-only: edits, mode changes, decisions on status changes, reprocessing and `retry_job` are refused with the code `published`, and the pipeline does not replace the draft. Cancelling every operation and publishing again freezes a new snapshot. Publishing again after a successful publication is not supported.
- **Automatic rule.** After `reduce` completes a draft, the publishing thread applies the `auto` rules of the cycle; it skips a meeting while a newer draft (`draft.new.json`) waits for a decision.

### Public-repository gate

A report, a transcript and issues in a public repository are public. The repository's visibility is read from the GitHub API (`private` and `visibility`). A repository that is public, or whose visibility cannot be read (offline, no access, not GitHub), needs the user's confirmation once per repository (`repo_confirmations`, keyed by host and name). The same rule is applied in three places, in the spirit of the three-place refusal of the cloud backend (ADR 0017): when the plan is built (and so in the dialog that previews publishing), when the publish command starts, and again by the dispatcher right before any operation that goes to the repository, so an operation queued earlier cannot slip past a repository that has become public. In the dispatcher an unknown answer caused by a dead network is a transient failure, not a refusal, because the repository may be private. A repository without a remote is never checked, since nothing leaves the machine.

### Secrets: token namespace and webhook validation

- The GitHub token is stored in the Windows Credential Manager under `github-token@<api host>` and is found again only for that host, the same rule as the endpoint keys of ADR 0017. A token saved for `api.github.com` cannot be sent to another server.
- The secrets cycle files refer to (`cred:discord/nomad`) live in a **separate namespace**: the reference resolves to the entry `target:discord/nomad`. No secret of the application itself starts with `target:`, so a reference written in a repository (which a teammate controls) can never reach the Hugging Face token or a model key.
- A webhook is validated when it is saved: `https` on `discord.com`, `discordapp.com`, `canary.discord.com` or `ptb.discord.com` with the path `/api/webhooks/<id>/<token>`, no query, port or credentials (plain `http` on this machine for tests).
- Values are never returned to the interface. It learns only whether a secret is set, and a masked webhook id. Every error text that is stored or logged passes through a scrubber that knows all stored secrets plus the pattern redaction. The resilience tests scan the database file, the outbox rows, the snapshots, the working tree, the history of both repositories and the captured log for the mock token and webhook.

### Schema and migration

Database schema version 5 adds three tables, with the migration of existing data like the earlier versions: `outbox` (state machine above, `UNIQUE (meeting_id, op_key)`, foreign key to `meetings` with cascade), `publications` (where a meeting went: folder, commit, report address, snapshot) and `repo_confirmations`. File schemas gain optional parts only and keep `version` 1: `meeting.schema.json` `status_proposals`, `cycle-context.schema.json` `previous_items[].issue`, `cycle.schema.json` `targets.github.issues`, and `extraction.schema.json` `status_changes`, which is required in the model answer (answers without it are retried; `windows.json` files written earlier still reduce).

### Fake servers

Tests never contact a real host. `publishers` provides, behind the `test-support` feature, a stateful fake GitHub (repositories with visibility and push permission, issues with labels, assignees, comments and state, paging, an organization issue field, rate limits) and a fake Discord webhook (message and attachment limits, `allowed_mentions` checks, 429 with `retry_after`), both hand-written on `TcpListener` with fault injection (status for the next N requests, dropped connections, a closed port as "offline"), plus a lab that wires them to a bare Git remote, a clone, a file database and a manual clock. No HTTP mocking framework is used, which keeps `tokio` and `hyper` out of the build.

## Consequences

- A meeting can be published, interrupted at any point and published again without a duplicate commit, issue, comment or (in the normal case) message.
- Publishing never touches the user's working tree, and a concurrent push by a colleague is absorbed.
- A secret has no path to a file, a log, the database or the repository.
- Publishing happens at most once per meeting: the draft is read-only afterwards and there is no edit-and-republish.
- Committing other cycle edits (`cycle.yaml`, templates) is left to the user.
- The publish plan shows `topics.yaml` before and after per topic, from the clone and the remote branch as last fetched (`preview_topics`, no network), and falls back to the topic names with a note when it cannot compare.
- The rule of a target is changed with `set_target_rule`, which writes `targets.<name>.publish` only.
- `targets.github.path` sets the folder of the meetings in the repository (`meetings` by default). It is validated when the cycle is saved and again by the publisher, and meetings published under an earlier path stay there.
- The only thing that decides what is published without a click is the rule of each target; its default is proposed in ADR 0019.

## Alternatives

- **libgit2 (`git2`) instead of the `git` CLI:** a C build and a second authentication path next to the credential helper the user already configured.
- **`git pull --rebase` and `git push` in the user's clone:** requires stashing or refusing when the clone has uncommitted edits, which are common because the application itself writes cycle files there.
- **A Git hosting API for the commit (the contents API):** one request per file, no atomic multi-file commit, and no support for other remotes.
- **`reqwest` with an async runtime or an HTTP mocking crate:** a heavier dependency tree for a worker that is one thread.
- **Linking issues and meetings by editing `meeting.json` afterwards:** breaks the rule that a commit only adds new files and needs a second commit; the hidden marker needs neither.
- **One webhook message with embeds:** embeds have their own length limits and cannot carry mentions that notify.
- **Retrying only in memory:** a restart or a closed laptop would lose the publication; the outbox survives both.

## Verification status

Everything above is covered by tests against fake servers and local bare repositories (see TESTING.md section 14). Not yet verified against the real services, and left open in the roadmap: the first write to a real repository and channel, the organization "Target date" field, the Discord upload limits for webhooks, whether GitHub resolves the transcript anchors (`#t-hh-mm-ss`) in the rendered Markdown, and the quality of status proposals from a real model. The release check (`crates/app/src/release_check.rs`) is written for a test repository and a test webhook and has not been run.
