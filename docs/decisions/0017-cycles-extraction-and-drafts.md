# 0017. Cycles, extraction, report modes and drafts

**Status:** Accepted (design); the two product decisions marked *Proposed* below wait for confirmation.

## Context

Stage 4 turns a processed transcript into an editable draft report. It adds the cycle (the unit that carries participants, glossary, templates and defaults), the two language-model stages `extract` and `reduce`, verification of quotes, the four report modes and the draft editor. Writing to GitHub and Discord is Stage 5.

## Decision

### Cycle repository

- A *cycle repository* is a Git repository with the layout of `docs/CYCLES.md`: `cycles/<ID>/cycle.yaml`, `cycles/<ID>/topics.yaml`, `cycles/<ID>/templates/*.md`, and later `meetings/<ID>/<date>/...`.
- `cycle-store` (crate `cycle-store`) owns the local clone or the local-only repository, parses and validates the files against `schemas/cycle.schema.json` and `schemas/topics.schema.json`, runs the `git` CLI as a subprocess, and keeps an offline cache with the date of the last successful pull.
- A cycle can be created without a remote: the application then initializes a local repository. Edits made in the application are written to the working tree and not committed (committing and pushing is Stage 5).
- The cycle's participants replace the Stage 3 local participant list: `cycle.yaml` is the list, and voice profiles are keyed by the participant id within the cycle.

### Pipeline

`extract` and `reduce` follow `vision`. Both use a language model and so pass the GPU gate when the backend is the managed `llama-server`; with an external or cloud backend they skip the gate.

1. **Context** (application, before `extract`): `work/context.json` (`schemas/cycle-context.schema.json`) with the cycle (participants, glossary, instruction, language), the open topics, open issues (read-only, from the cache), and the items of the previous meetings of the cycle (their `meeting.json` files), newest first, capped.
2. **Windows** (worker): windows of 15 minutes with 1 minute overlap, cut at the longest pause near the boundary. Passages excluded in `edits.json` are removed before windowing, and the transcript uses participant names.
3. **Map** (`extract`): per window the model returns items validated against `schemas/extraction.schema.json` by the server (JSON Schema constrained output) and again by the worker, with at most two retries. A window that stays invalid is recorded as skipped with a warning. The prompt contains the meeting date and a calendar of the next 28 days, the participants (ids and names), the glossary, the previous items, the sections the items may be assigned to, and the output language.
4. **Verify** (`extract`): every quote is matched against the transcript near the stated time (normalized, fuzzy). The item gets `source.verified` and `source.match`; the time range is taken from the matched segments, not from the model.
5. **Reduce** (`reduce`): deduplicates items across windows (same type, similar text, overlapping or near time; the verified or more complete one wins), links items to open topics and issues named in the context, validates assignees against the participant ids (an unknown assignee becomes `null` with certainty `unknown`), and asks the model for the hierarchical summary and, in auto mode, the proposed sections. Output: `work/draft.json` (`schemas/meeting.schema.json`).
6. **Render** (application): deterministic templates turn the draft into Markdown; the model never writes Markdown.

### Items and certainty

Types are `task`, `decision`, `topic`, `note`, `idea`, `budget`. `certainty` has `assignee` and `due`, each `stated`, `inferred` or `unknown`. An item with an unverified quote is shown with a marker in the draft. Items carry `section` (the section id) so every mode uses the same data.

### Report modes

- **fixed:** built-in sections, items grouped by type.
- **template:** the cycle's template file defines the sections (front matter `sections`: id, title, description); the model assigns each item to one section.
- **auto:** the model proposes sections from the extracted items; the user may rename, add, remove and reorder them in the draft editor.
- **by-type:** `report.by_type` in `cycle.yaml` maps a meeting type (for example `technical`, `budget`, `casual`) to a template; the type is chosen at the report mode gate.

The mode is chosen before recording (the cycle supplies the default); a recording cannot start without a cycle and a mode. The mode can be changed after recording while the draft is not yet published, because the layout is rendered from the same extracted items; changing the sections of a template or auto mode re-assigns items by a cheap model call or manually.

### Language

`output` is `auto`, `pl` or `en` per cycle and can be overridden per meeting. `auto` resolves to the dominant language of the transcript (the ASR language, with a stop-word check for the mixed case; ties go to the cycle default, then English). The resolved language is written to the draft; built-in headings come from a table in the renderer for `pl` and `en`.

### Backends

The extraction stages accept three backends in `options.llm`: `managed` (a `llama-server` started by the worker from a GGUF path), `external` (an OpenAI-compatible base URL and model name) and `cloud` (the same shape plus a key, chosen for one meeting). Keys are read from the Windows Credential Manager by the application and passed to the worker through the environment of that process only. A cloud backend runs only when `options.llm.cloud_confirmed` is true, which the application sets after the user confirmed a dialog that states the endpoint host and the amount of text that will be sent (word count, number of screenshot descriptions); the worker refuses to start otherwise. There is no automatic fallback to the cloud. Cloud is tested only against a mock server.

### Draft editing

The draft is `work/draft.json`. User changes (edited text, assignee, due date, moved items, deleted items, added items, renamed sections) are saved into the same file with `edited: true` on touched items; running `reduce` again does not overwrite a draft that has user edits unless the user asks for it. Every item shows its quote, the time and a jump to the transcript.

## Proposed: built-in sections and default template (awaiting confirmation)

Built-in sections, in this order, empty ones omitted from the output: Summary, Decisions, Tasks, Open topics, Notes, Ideas, Budgets. Default template:

```
# {title}

{date} · {participants}

{summary_section}

{sections}

---
[Full transcript](transcript.md)
```

`{summary_section}` renders the summary under a localised heading (`## Summary` or `## Podsumowanie`) and renders to nothing when the summary is empty, so a Polish report has no English heading and an empty summary leaves no empty heading; `{summary}` stays available as the bare text for custom templates. Item lines keep `due` in every language. Existing cycles keep the `default.md` they have; files are not migrated. The same text is a file in the repository (`templates/default.md`), so wording can change without code. The choice is recorded here once confirmed.

## Proposed: default model after the measurements (awaiting confirmation)

Gemma 4 12B Q4_K_M stays the default unless the Stage 4 measurements (`benchmarks/results/stage4-*.json`) show otherwise; see the measurements section. The default is a setting, not code.

## Implementation notes

What was built where it refines or differs from the design above.

- **Truncated answers are split, not retried.** When the model's answer for a window is cut off by the token limit, repeating the same request would give the same result. The worker halves the window by transcript lines (the second half starts one minute early, like consecutive windows) and asks for each half separately, at most two levels deep, then merges the results into one record for the window. A window that is still cut off after the second split, or has too little speech to split, is recorded as skipped with a warning. Other invalid answers keep the bounded retries.
- **Output budget from the server context.** The answer limit of a window is derived from the context size of the backend minus an estimate of the prompt, capped at 8192 and floored at 1024 tokens. The managed server's context size is a setting (default 16384).
- **Cloud refusal in three places.** A cloud backend is refused by the queue supervisor (the confirmation stored for the meeting must exist and match the endpoint host; a `cloud_confirmed` value in the stored job options is ignored), by the runner when it builds the worker manifest, and by the worker itself. The flag passed to the worker is taken from the store at run time.
- **Participant keys.** Participants of a cycle are stored in the local database under the key `<cycle>:<id>`, so the same person listed in two cycles has separate voice profiles.
- **Cycle files are not committed yet.** Cycle settings, topics and templates edited in the application are written to the working tree only; committing and pushing belongs to Stage 5. Before the `extract` stage of a meeting of a cloned repository the application pulls once per extraction attempt with `git pull --rebase`, never stashing: a repository with uncommitted tracked changes is not pulled (the manual pull reports `repo_dirty`), conflict markers or unmerged paths after a pull are undone, and a failed pull never stops the job.
- **Draft protection.** `reduce` records the SHA-256 of the `draft.json` it wrote in `work/draft.base.sha256`. A draft counts as edited when any item is marked edited or its bytes differ from that hash, so deleting an item or renaming a section also protects it: a later `reduce` then writes `draft.new.json` for the user to accept or discard. Draft edits are refused while `extract` or `reduce` runs.
- **Keys and endpoints.** Cloud and external keys are stored per endpoint host in the Credential Manager and are sent only to that host; cloud endpoints need https unless they are on this machine; redirects never carry the key; addresses with embedded credentials and repository URLs that start with `-` or use helper transports are refused. An external server that is not on this machine is flagged in the settings, and the cloud dialog counts the items of earlier meetings that are sent as context.
- **Failed extraction.** A run in which every window was skipped fails the stage so that it can be retried; partial skips stay warnings on the draft. Rate limits and server errors are retried with a pause.
- **Transcript as data.** The transcript is delimited in the prompt and declared to be data; quotes are verified against speech only (not against screenshot descriptions), and quotes shorter than three words are verified only when they equal a whole segment.
- **`publish` has a third value.** `targets.<name>.publish` accepts `auto`, `on-approval` and `off`; `off` disables publishing to that target.
- **Fixed mode has no section editing.** In `fixed` mode items are grouped by type and the section editor of the draft is not offered; sections can be edited in `template`, `auto` and `by-type` modes.
- **Language resolution.** `auto` compares stop words and Polish letters in the transcript text; when neither language clearly leads, the language reported by the ASR step decides, and English when that is unknown.
- **Open issues.** The application reads the open issues of a cycle from the cache file. The fetch function exists in `cycle-store`, but the application does not yet configure a GitHub token or call it.

## Consequences

- Report layout changes never re-run a model; section assignment is the only model-dependent layout step.
- The Python worker, the Rust core and the UI share the schemas under `schemas/`, tested from both sides.
- Two extra llama-server starts per meeting (extract, reduce) cost start-up time but keep the one-model-process rule.

## Alternatives

- **Context built in the worker:** the worker would need Git and the cycle files; the application already has them.
- **Free-form Markdown from the model:** no verification and no editing of single items.
- **A single stage:** a failed summary would repeat the whole extraction.
