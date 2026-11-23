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

use cycle_store::{Correction, Cycle};
use serde::{Deserialize, Serialize};
use store::{NewOp, OpState, OutboxKind, OutboxOp, OutboxTarget};

use crate::gate;
use crate::plan::{to_json, Block, RepoFacts, COMMIT_KEY};
use crate::rules::{PublishTarget, TargetRules, Trigger};

/// `op_key` of the file operation of a correction.
pub fn file_key(correction_id: &str) -> String {
    format!("correction:{correction_id}")
}

/// `op_key` of the issue comment of a correction.
pub fn comment_key(correction_id: &str) -> String {
    format!("correction-issue:{correction_id}")
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
    pub correction: &'a Correction,
    pub repo: &'a RepoFacts,
    /// What the outbox already holds for the meeting.
    pub existing: &'a [OutboxOp],
    pub trigger: Trigger,
    pub out_dir: &'a str,
    /// The meeting is in the repository (its commit landed) or its commit is still queued.
    pub in_repository: bool,
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
    let rules = TargetRules::of(input.cycle);
    let mut plan = CorrectionPlan {
        rules,
        ops: Vec::new(),
        summary: CorrectionSummary::default(),
        blocks: Vec::new(),
        notes: Vec::new(),
    };
    let explicit = input.trigger == Trigger::Explicit;
    if !rules.allows(PublishTarget::Github, input.trigger) {
        if explicit {
            plan.notes
                .push("github is off in cycle.yaml: the correction stays on this computer".into());
            plan.blocks.push(Block::NothingToPublish);
        }
        return plan;
    }
    if !input.in_repository {
        if explicit {
            plan.blocks.push(Block::MeetingNotInRepository);
        }
        return plan;
    }
    let correction = input.correction;
    let payload = to_json(&CorrectionPayload {
        cycle: input.cycle.id.clone(),
        meeting_date: input.meeting_date.to_string(),
        correction: correction.clone(),
        out_dir: input.out_dir.to_string(),
    });
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
                "this repository is not linked to GitHub: no comment is made on an issue".into(),
            );
        } else {
            plan.summary.comment = Some(about);
            plan.ops.push(NewOp {
                meeting_id: input.meeting_id.to_string(),
                target: OutboxTarget::Issues,
                kind: OutboxKind::CommentCorrection,
                op_key: comment_key(&correction.id),
                payload,
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
    plan
}

#[cfg(test)]
mod tests;
