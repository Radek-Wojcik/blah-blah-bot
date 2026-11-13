# 0009. Git repository as the source of truth

**Status:** Accepted

## Context

Cycle history and open tasks are shared with a team that already works on GitHub. Team members must be able to read reports and close tasks without the application.

## Decision

- A GitHub repository holds cycle configuration, topics, reports, transcripts, structured meeting data and screenshots.
- Tasks are GitHub Issues labeled with the cycle. Closing an issue marks the task done.
- The application works on a local clone: pull with rebase before a briefing, commit only new files per meeting, push with retry.
- Issue state is read through the API at pull time and written through the API after approval.
- Offline: cached clone and issues with a visible date; writes queue in an outbox.

## Consequences

- No server component and no proprietary storage format.
- Meeting folders never conflict because they contain only new files; `topics.yaml` uses last-write-wins per topic.
- Due dates use the "Target date" issue field in organization repositories and a line in the issue body elsewhere.
- Voice profiles and other private state are deliberately excluded from the repository.

## Alternatives

- **API-only without a clone:** no offline reading, many requests for history.
- **Local database as the source of truth with export:** simpler, but the team cannot change state.
