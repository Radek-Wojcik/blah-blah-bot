# 0010. Discord webhook and bot

**Status:** Accepted

## Context

A short summary with mentions and screenshots goes to a Discord channel. Setup should be trivial for the common case, while some users want member lookup, threads and message editing.

## Decision

- Default: webhook. Mentions by user and role ID with explicit `allowed_mentions`; content split to respect message, embed and attachment limits.
- Optional: bot token for member search by nickname, a thread per meeting and editing of published messages.
- Both implement one publisher interface; the cycle selects which is used.

## Consequences

- A webhook URL is enough to start.
- Mentions through a webhook need IDs stored in the cycle's participant list.
- The bot adds permission and intent setup, documented separately.

## Alternatives

- **Bot only:** more capable, harder first setup.
- **Voice recording through the bot:** solves speaker separation on Discord; deferred beyond 1.0.
