//! Publishing a correction to a published meeting: which operations the outbox gets.
//!
//! A correction is a note written after a meeting was published (the draft stays locked, see
//! `flow`). Published, it is a new file in the meeting's folder of the cycle repository
//! (`corrections.md`, `corrections-2.md`, ...), and, when the note is about an item or an
//! issue, a comment on that issue. Both go through the same outbox, rules and checks as the
//! meeting itself:
//!
//! ```text
//! commit          (the meeting, if it is still queued)
//!   `-- correction:<id>        the file in the meeting folder   (target github)
//!         `-- correction-issue:<id>   the comment on the issue  (target issues)
//! ```
//!
//! The rule of the `github` target decides when: `auto` queues a correction as soon as it is
//! written, `on-approval` when the user presses Publish for it, `off` never. The
//! public-repository check applies as for the meeting. Pure: no network, no files, no clock.

use cycle_store::{Correction, Cycle, DiscordMode};
use serde::{Deserialize, Serialize};
use store::{NewOp, OpState, OutboxKind, OutboxOp, OutboxTarget};

use crate::gate;
use crate::plan::{to_json, Block, RepoFacts, COMMIT_KEY, DISCORD_KEY};
use crate::rules::{GlobalPublishRules, PublishTarget, TargetRules, Trigger};

/// `op_key` of the file operation of a correction.
pub fn file_key(correction_id: &str) -> String {
    format!("correction:{correction_id}")
}

/// `op_key` of the issue comment of a correction.
pub fn comment_key(correction_id: &str) -> String {
    format!("correction-issue:{correction_id}")
}

/// `op_key` of the Discord summary edit and thread reply of a correction.
pub fn discord_edit_key(correction_id: &str) -> String {
    format!("discord-edit:{correction_id}")
}

/// The payload of both operations: ids and the text of the note, never a secret. The text is
/// what was written when the correction was queued, so a retry days later publishes the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionPayload {
    pub cycle: String,
    /// `YYYY-MM-DD`, the day of the meeting that is corrected.
    pub meeting_date: String,
    pub correction: Correction,
    /// Where the rendered file is written before it is committed, relative to the snapshot
    /// root.
    pub out_dir: String,
}

pub struct CorrectionPlanInput<'a> {
    pub meeting_id: &'a str,
    pub meeting_date: &'a str,
    pub cycle: &'a Cycle,
    /// The global rules of the preferences, which a target without its own rule follows.
    pub global_rules: GlobalPublishRules,
    pub correction: &'a Correction,
    pub repo: &'a RepoFacts,
    /// What the outbox already holds for the meeting.
    pub existing: &'a [OutboxOp],
    pub trigger: Trigger,
    pub out_dir: &'a str,
    /// The meeting is in the repository (its commit landed) or its commit is still queued.
    pub in_repository: bool,
    /// A Discord bot post is recorded in `external_refs` (`post`), or a `post_discord`
    /// operation is still live in the outbox.
    pub discord_post_exists: bool,
    /// A bot token is stored under the cycle's Discord bot reference.
    pub discord_bot_token_set: bool,
}

/// What publishing a correction does, for the screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionSummary {
    /// A new file is added to the meeting folder.
    pub file: bool,
    /// The commit is pushed to a remote.
    pub pushes: bool,
    /// The issue that gets a comment (`#12`), or `None`. For an item whose issue is not known
    /// yet this is the item (`i3`).
    pub comment: Option<String>,
    /// Bot mode: the Discord summary is edited and a reply is posted in its thread.
    pub discord_edit: bool,
}

#[derive(Debug, Clone)]
pub struct CorrectionPlan {
    pub rules: TargetRules,
    pub ops: Vec<NewOp>,
    pub summary: CorrectionSummary,
    pub blocks: Vec<Block>,
    pub notes: Vec<String>,
}

impl CorrectionPlan {
    pub fn is_blocked(&self) -> bool {
        !self.blocks.is_empty()
    }
}

fn live(op: &OutboxOp) -> bool {
    op.state != OpState::Cancelled
}

pub fn build(input: &CorrectionPlanInput<'_>) -> CorrectionPlan {
    let github_configured = input.repo.has_remote || input.repo.github.is_some();
    // Corrections reach Discord only through a bot, so the webhook secret is irrelevant here.
    let rules = TargetRules::for_repo(input.cycle, &input.global_rules, github_configured, false);
    let mut plan = CorrectionPlan {
        rules,
        ops: Vec::new(),
        summary: CorrectionSummary::default(),
        blocks: Vec::new(),
        notes: Vec::new(),
    };
    let explicit = input.trigger == Trigger::Explicit;
    let correction = input.correction;
    let payload = to_json(&CorrectionPayload {
        cycle: input.cycle.id.clone(),
        meeting_date: input.meeting_date.to_string(),
        correction: correction.clone(),
        out_dir: input.out_dir.to_string(),
    });

    // ----- GitHub correction file and optional issue comment
    let github_allowed = rules.allows(PublishTarget::Github, input.trigger);
    if github_allowed && input.in_repository {
        let commit_is_live = input
            .existing
            .iter()
            .any(|o| o.op_key == COMMIT_KEY && live(o));
        plan.summary.file = true;
        plan.summary.pushes = input.repo.has_remote;
        plan.ops.push(NewOp {
            meeting_id: input.meeting_id.to_string(),
            target: OutboxTarget::Github,
            kind: OutboxKind::CommitCorrection,
            op_key: file_key(&correction.id),
            payload: payload.clone(),
            after_op_key: commit_is_live.then(|| COMMIT_KEY.to_string()),
        });

        let about = correction.issue.clone().or_else(|| correction.item.clone());
        if let Some(about) = about {
            if !rules.issues {
                plan.notes
                    .push("issues are off for this cycle: no comment is made on the issue".into());
            } else if input.repo.github.is_none() {
                plan.notes.push(
                    "this repository is not linked to GitHub: no comment is made on an issue"
                        .into(),
                );
            } else {
                plan.summary.comment = Some(about);
                plan.ops.push(NewOp {
                    meeting_id: input.meeting_id.to_string(),
                    target: OutboxTarget::Issues,
                    kind: OutboxKind::CommentCorrection,
                    op_key: comment_key(&correction.id),
                    payload: payload.clone(),
                    after_op_key: Some(file_key(&correction.id)),
                });
            }
        }

        if input.repo.has_remote {
            if let Err(e) = gate::require(input.repo.visibility, input.repo.confirmed) {
                plan.blocks.push(Block::PublicRepoUnconfirmed {
                    visibility: e.visibility.as_str().to_string(),
                });
            }
        }
    }

    // ----- Discord bot edit and thread reply
    if let Some(DiscordMode::Bot(bot)) = input.cycle.targets.discord.as_ref().and_then(|d| d.mode())
    {
        if bot.edit_on_correction && rules.allows(PublishTarget::Discord, input.trigger) {
            let post_live = input
                .existing
                .iter()
                .find(|o| o.op_key == DISCORD_KEY && live(o));
            let has_post = input.discord_post_exists || post_live.is_some();
            if has_post {
                if input.discord_bot_token_set {
                    plan.summary.discord_edit = true;
                    let after_op_key = if post_live.is_some_and(|o| o.state != OpState::Done) {
                        Some(DISCORD_KEY.to_string())
                    } else if plan.summary.file {
                        Some(file_key(&correction.id))
                    } else {
                        None
                    };
                    plan.ops.push(NewOp {
                        meeting_id: input.meeting_id.to_string(),
                        target: OutboxTarget::Discord,
                        kind: OutboxKind::EditDiscord,
                        op_key: discord_edit_key(&correction.id),
                        payload,
                        after_op_key,
                    });
                } else if explicit {
                    plan.blocks.push(Block::DiscordBotTokenMissing);
                } else {
                    plan.notes.push(
                        "no Discord bot token is stored: the Discord summary is not edited".into(),
                    );
                }
            }
        }
    }

    if plan.ops.is_empty() && plan.blocks.is_empty() && explicit {
        if !github_allowed {
            plan.notes
                .push(if rules.is_configured(PublishTarget::Github) {
                    "github is off in cycle.yaml: the correction stays on this computer".into()
                } else {
                    "connect a GitHub repository first: the correction stays on this computer"
                        .into()
                });
            plan.blocks.push(Block::NothingToPublish);
        } else if !input.in_repository {
            plan.blocks.push(Block::MeetingNotInRepository);
        } else {
            plan.blocks.push(Block::NothingToPublish);
        }
    }
    plan
}

#[cfg(test)]
mod tests;
