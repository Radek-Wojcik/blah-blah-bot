# 0013. Interface and content languages

**Status:** Accepted

## Context

The project targets an international open-source audience, while meetings are often held in Polish.

## Decision

- The interface, settings, logs and documentation are English only.
- The language of generated content (reports, summaries, task texts) is a cycle setting with a per-meeting override: `auto`, `pl` or `en`. `auto` follows the dominant language of the meeting.
- Transcription language is a separate cycle setting.
- Prompts are written so that quotes stay in the spoken language regardless of the output language.

## Consequences

- No interface localization to maintain.
- A Polish meeting yields a Polish report by default, without configuration.
- Templates may contain fixed headings; built-in templates ship in both languages.
