# 0026. Global people and voice profiles

Date: 2026-11-25
Status: Proposed

## Context

Previously, `participants` had a `cycle_id` column and unique name indexes (`idx_participants_name_local`, `idx_participants_name_cycle`), which forced each project (`cycle`) to own a separate participant row and voice profile, or else risk auto-merging distinct people who happen to share a name across projects.

Users frequently work with the same colleagues across multiple projects, and want:
- A single global voice profile per person with per-project badges (`project_people`), so voice samples learned in one project help recognize the person in another project.
- Safe cross-project recognition: never silently auto-assign or auto-merge people by name alone; instead, surface cross-project matches as explicit suggestions (`Recognised from <projects>: <name>? <score>%`) with one-click Accept + inline Undo.
- Explicit profile management in Settings: adding/removing project memberships (falling back to Global without deleting biometric samples) and manually merging two profiles (`Same person as…`) with a same-project collision guard.

## Decision

1. **Additive schema (`project_people`)**: Introduce `project_people(cycle_id, participant_id, cycle_participant_id, created_unix_ms)` via an idempotent `ensure_people_schema(conn)` helper in `crates/store/src/migrations.rs` (keeping `LATEST = 7` to avoid colliding with Stage 7's upcoming Version 8). Drop `idx_participants_name_local` and `idx_participants_name_cycle` so two different people can share a name without auto-merging, and backfill `project_people` from existing `participants.cycle_id` rows when empty.
2. **Tiered speaker matching**: In `crates/queue/src/voices.rs`, match Tier 1 (`project_people` for the meeting's cycle) first, then run Tier 2 (unlinked globals) + Tier 3 (profiles from other projects) in a single `assign_unique` pass so the `0.10` margin holds. Stop eager `link_global_participant_to_cycle` during `run_speaker_id`; emit `VoiceDecision::Suggested` for Tier 3 cross-project matches and link only when the user accepts.
3. **Explicit merge and project membership**: Provide `add_profile_to_project`, `remove_profile_from_project`, `merge_participants` (transactional, same-cycle collision guard, `MAX_SAMPLES = 20` consolidation, repointing `meeting_voices`, moving `project_people`, transferring `sample_clip_file`), and `undo_voice_decision`.

## Consequences

- **Positive**: One card per person in Voice Profiles, no silent name-based auto-merging, safe cross-project speaker suggestions with Undo, and non-destructive project removal.
- **Negative**: Users who previously had the same person enrolled separately in two projects see two cards with a `"Possible duplicate name"` hint until they merge them via `"Same person as…"`.
