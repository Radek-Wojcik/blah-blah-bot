//! The publish plan of one meeting: which operations the outbox gets, built from the draft,
//! the cycle's rules and what is already queued. Pure: no network, no files, no clock.
//!
//! Order of the operations, with `after` chaining so a link in a later one is valid:
//!
//! ```text
//! commit            the folder, report and transcript in the repository, topics.yaml
//!   |-- issue:<item>   one per task item, in the order of the draft
//!   |-- status:<id>    one per approved status change of an issue
//!   `-- discord        after the commit, and held by the store while issues are pending
//! ```
//!
//! Rules (see `rules.rs`): `auto` plans what a completed draft may publish on its own and
//! never a status change; an explicit publication plans the requested targets that are not
//! `off`. The public-repository check is part of the plan: a repository that is, or may be,
//! public and was not confirmed makes the plan [blocked](Block::PublicRepoUnconfirmed).
//!
//! The payloads hold ids, text of the meeting and the name of the frozen snapshot, never a
//! secret; the dispatcher adds the addresses (which depend on the folder the commit ends up
//! in) when it runs.

use std::collections::BTreeMap;

use cycle_store::topics::TopicsMerge;
use cycle_store::{
    Cycle, Decision, Draft, ItemType, ProposedStatus, PublishMode, Topic, TopicStatus,
};
use serde::{Deserialize, Serialize};
use store::{NewOp, OpState, OutboxKind, OutboxOp, OutboxTarget};

use crate::gate::{self, RepoVisibility};
use crate::github::RepoRef;
use crate::rules::{PublishTarget, TargetRules, Trigger};

/// `op_key` of the commit.
pub const COMMIT_KEY: &str = "commit";
/// `op_key` of the Discord post.
pub const DISCORD_KEY: &str = "discord";

pub fn issue_key(item_id: &str) -> String {
    format!("issue:{item_id}")
}

pub fn status_key(proposal_id: &str) -> String {
    format!("status:{proposal_id}")
}

// ----- payloads -----------------------------------------------------------------------------

/// A topic the meeting adds to `topics.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicAdd {
    pub id: String,
    pub text: String,
}

/// An approved change of a topic's status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicSet {
    pub id: String,
    /// True reopens, false closes.
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPayload {
    pub cycle: String,
    /// `YYYY-MM-DD`, the day of the meeting.
    pub date: String,
    /// The frozen snapshot, relative to the snapshot root.
    pub snapshot: String,
    /// Files of the meeting folder, relative to the snapshot.
    pub files: Vec<String>,
    #[serde(default)]
    pub topics_add: Vec<TopicAdd>,
    #[serde(default)]
    pub topics_set: Vec<TopicSet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuePayload {
    pub cycle: String,
    pub date: String,
    pub item_id: String,
    pub title: String,
    /// The GitHub login of the assignee.
    pub assignee: Option<String>,
    pub due: Option<String>,
    pub time_ms: Option<u64>,
    /// Only a quote that was found in the transcript.
    pub quote: Option<String>,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPayload {
    pub cycle: String,
    pub date: String,
    pub proposal_id: String,
    pub number: u64,
    /// True reopens, false closes.
    pub open: bool,
    /// The user's comment, else the reason.
    pub reason: String,
    pub quote: Option<String>,
    pub time_ms: Option<u64>,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordPayload {
    pub cycle: String,
    pub date: String,
    pub snapshot: String,
    /// The `cred:` reference of the webhook, not the webhook.
    pub reference: String,
    pub template: Option<String>,
    /// Screenshots of the snapshot (`img/...`).
    pub files: Vec<String>,
}

// ----- input and output ---------------------------------------------------------------------

/// The GitHub repository a cycle repository publishes to, and the API it is reached through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubIdentity {
    pub repo: RepoRef,
    pub api: String,
}

/// What is known about the repository the commit goes to.
#[derive(Debug, Clone)]
pub struct RepoFacts {
    /// Where issues go; `None` for a repository that is not linked to GitHub.
    pub github: Option<GithubIdentity>,
    /// A commit is pushed to a remote (false: it stays in this clone).
    pub has_remote: bool,
    pub visibility: RepoVisibility,
    /// The user confirmed publishing to this repository.
    pub confirmed: bool,
}

/// The frozen files of the meeting.
#[derive(Debug, Clone, Default)]
pub struct SnapshotRef {
    /// Relative to the snapshot root.
    pub dir: String,
    /// Relative to `dir`: `meeting.json`, `report.md`, `transcript.md`, `img/...`.
    pub files: Vec<String>,
}

pub struct PlanInput<'a> {
    pub meeting_id: &'a str,
    pub draft: &'a Draft,
    pub cycle: &'a Cycle,
    /// The topics of the cycle as they are in this clone.
    pub local_topics: &'a [Topic],
    pub repo: &'a RepoFacts,
    /// A webhook is stored under the cycle's Discord reference.
    pub discord_webhook_set: bool,
    /// Anchor in the published transcript, by item id and by proposal id.
    pub anchors: &'a BTreeMap<String, String>,
    pub snapshot: &'a SnapshotRef,
    /// What the outbox already holds for the meeting.
    pub existing: &'a [OutboxOp],
    pub trigger: Trigger,
    /// The targets the user asked for (an explicit publication).
    pub targets: &'a [PublishTarget],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedIssue {
    pub item_id: String,
    pub title: String,
    pub assignee: Option<String>,
    pub due: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedStatus {
    pub proposal_id: String,
    pub target: String,
    pub open: bool,
}

/// A topic as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicView {
    pub text: String,
    /// `open` or `closed`.
    pub status: &'static str,
    pub opened_in: String,
    pub closed_in: Option<String>,
}

impl From<&Topic> for TopicView {
    fn from(t: &Topic) -> Self {
        Self {
            text: t.text.clone(),
            status: match t.status {
                TopicStatus::Open => "open",
                TopicStatus::Closed => "closed",
            },
            opened_in: t.opened_in.clone(),
            closed_in: t.closed_in.clone(),
        }
    }
}

/// One topic that publishing changes in `topics.yaml`. `before` is what the remote has
/// (`None` for a topic that is added), `after` what the commit writes (`None` for one that
/// is removed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDelta {
    pub id: String,
    /// `added`, `changed` or `removed`.
    pub change: &'static str,
    pub before: Option<TopicView>,
    pub after: Option<TopicView>,
    /// Somebody changed this topic on the remote since this clone last pulled, and the last
    /// write wins: `after` replaces `before`, which is what the remote has now.
    pub overrides_remote: bool,
}

/// `topics.yaml` before and after the commit, per topic, as far as it can be known without
/// the network: the clone against the remote branch as last fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicsPlan {
    /// Compared with the remote branch as last fetched. False: there is none here, the
    /// comparison is with this clone's own branch and the remote may differ.
    pub against_remote: bool,
    pub changes: Vec<TopicDelta>,
    /// Topics the commit leaves as they are.
    pub unchanged: usize,
}

impl TopicsPlan {
    /// The deltas of a merge, ordered added, changed, removed.
    pub fn of(merge: &TopicsMerge, against_remote: bool) -> Self {
        let overrides = |id: &str| merge.conflicts.iter().any(|c| c.id == id);
        let mut changes = Vec::new();
        for t in &merge.diff.added {
            changes.push(TopicDelta {
                id: t.id.clone(),
                change: "added",
                before: None,
                after: Some(t.into()),
                overrides_remote: overrides(&t.id),
            });
        }
        for c in &merge.diff.changed {
            changes.push(TopicDelta {
                id: c.id.clone(),
                change: "changed",
                before: Some((&c.before).into()),
                after: Some((&c.after).into()),
                overrides_remote: overrides(&c.id),
            });
        }
        for t in &merge.diff.removed {
            changes.push(TopicDelta {
                id: t.id.clone(),
                change: "removed",
                before: Some(t.into()),
                after: None,
                overrides_remote: overrides(&t.id),
            });
        }
        let unchanged = merge
            .topics
            .len()
            .saturating_sub(merge.diff.added.len() + merge.diff.changed.len());
        Self {
            against_remote,
            changes,
            unchanged,
        }
    }
}

/// What publishing would do, for the screen that asks before it does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    /// The report and transcript are committed.
    pub commit: bool,
    /// The commit is pushed to a remote.
    pub pushes: bool,
    pub files: Vec<String>,
    pub issues: Vec<PlannedIssue>,
    pub statuses: Vec<PlannedStatus>,
    pub topics_added: Vec<String>,
    pub topics_set: Vec<TopicSet>,
    /// `topics.yaml` before and after, per topic. `None` when the commit leaves the file alone
    /// or the comparison is not possible (the plan then says so in its notes and the screen
    /// falls back to `topics_added` and `topics_set`).
    pub topics: Option<TopicsPlan>,
    pub discord: bool,
    /// Status changes that were not approved: they are not written back.
    pub proposals_pending: usize,
}

/// Why a plan cannot be carried out as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Block {
    /// The repository is public or its visibility is unknown, and not confirmed.
    PublicRepoUnconfirmed { visibility: String },
    /// A Discord post was asked for and no webhook is stored for the reference.
    DiscordWebhookMissing,
    /// Nothing is left to publish.
    NothingToPublish,
    /// A correction needs the meeting in the repository, and it is not there (its commit has
    /// not been queued, or it was cancelled).
    MeetingNotInRepository,
}

impl Block {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PublicRepoUnconfirmed { .. } => gate::PUBLIC_REPO_CODE,
            Self::DiscordWebhookMissing => "discord_webhook_missing",
            Self::NothingToPublish => "nothing_to_publish",
            Self::MeetingNotInRepository => "not_in_repository",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::PublicRepoUnconfirmed { visibility } => format!(
                "the repository is {visibility}, so what is published is visible to everybody: confirm publishing to it first"
            ),
            Self::DiscordWebhookMissing => {
                "no Discord webhook is stored for this cycle: add it in the publishing settings"
                    .into()
            }
            Self::NothingToPublish => {
                "there is nothing to publish with the rules of this cycle".into()
            }
            Self::MeetingNotInRepository => {
                "the meeting is not in the repository: publish it to GitHub first, then its corrections can follow"
                    .into()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublishPlan {
    pub rules: TargetRules,
    pub ops: Vec<NewOp>,
    pub summary: PlanSummary,
    pub blocks: Vec<Block>,
    /// Things worth telling the user that do not stop anything.
    pub notes: Vec<String>,
}

impl PublishPlan {
    pub fn is_blocked(&self) -> bool {
        !self.blocks.is_empty()
    }

    /// The commit operation's payload: what the commit does to the topics. `None` when the
    /// plan has no commit.
    pub fn commit_payload(&self) -> Option<CommitPayload> {
        self.ops
            .iter()
            .find(|o| o.op_key == COMMIT_KEY)
            .and_then(|o| serde_json::from_str(&o.payload).ok())
    }
}

// ----- building -----------------------------------------------------------------------------

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `#12` -> 12.
fn issue_number(target: &str) -> Option<u64> {
    target
        .strip_prefix('#')?
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
}

fn is_live(op: &OutboxOp) -> bool {
    op.state != OpState::Cancelled
}

pub fn build(input: &PlanInput<'_>) -> PublishPlan {
    let rules = TargetRules::of(input.cycle);
    let mut plan = PublishPlan {
        rules,
        ops: Vec::new(),
        summary: PlanSummary::default(),
        blocks: Vec::new(),
        notes: Vec::new(),
    };
    let explicit = input.trigger == Trigger::Explicit;
    let wants = |t: PublishTarget| match input.trigger {
        Trigger::Auto => rules.allows(t, Trigger::Auto),
        Trigger::Explicit => input.targets.contains(&t) && rules.allows(t, Trigger::Explicit),
    };
    if explicit {
        for t in input.targets {
            if !rules.allows(*t, Trigger::Explicit) {
                plan.notes.push(match rules.mode(*t) {
                    None => format!("this cycle has no {} target", t.as_str()),
                    Some(_) => format!("{} is off in cycle.yaml and was left out", t.as_str()),
                });
            }
        }
    }
    let draft = input.draft;
    let date = draft.date.clone();
    let cycle_id = input.cycle.id.clone();
    let live_commit = input
        .existing
        .iter()
        .any(|o| o.op_key == COMMIT_KEY && is_live(o));
    let anchor = |id: &str| input.anchors.get(id).cloned();

    plan.summary.proposals_pending = draft
        .status_proposals
        .iter()
        .filter(|p| p.decision == Decision::Proposed)
        .count();

    // ----- the commit, the issues and the status changes (target `github`)
    let github = wants(PublishTarget::Github);
    if github {
        // Topics the meeting adds, and the approved changes of topics.
        let mut topics_add = Vec::new();
        for item in draft
            .items
            .iter()
            .filter(|i| i.item_type == ItemType::Topic && i.relates_to.is_none())
        {
            let known = input
                .local_topics
                .iter()
                .any(|t| normalized(&t.text) == normalized(&item.text));
            if !known {
                topics_add.push(TopicAdd {
                    id: format!("{}-{}", input.meeting_id, item.id),
                    text: item.text.clone(),
                });
            }
        }
        let mut topics_set = Vec::new();
        if explicit {
            for p in draft
                .status_proposals
                .iter()
                .filter(|p| p.decision == Decision::Approved && issue_number(&p.target).is_none())
            {
                if input.local_topics.iter().any(|t| t.id == p.target) {
                    topics_set.push(TopicSet {
                        id: p.target.clone(),
                        open: p.status == ProposedStatus::Open,
                    });
                } else {
                    plan.notes.push(format!(
                        "the topic {} of {} is not in topics.yaml here: not changed",
                        p.target, p.id
                    ));
                }
            }
        }
        plan.summary.commit = true;
        plan.summary.pushes = input.repo.has_remote;
        plan.summary.files = input.snapshot.files.clone();
        plan.summary.topics_added = topics_add.iter().map(|t| t.text.clone()).collect();
        plan.summary.topics_set = topics_set.clone();
        plan.ops.push(NewOp {
            meeting_id: input.meeting_id.to_string(),
            target: OutboxTarget::Github,
            kind: OutboxKind::CommitMeeting,
            op_key: COMMIT_KEY.to_string(),
            payload: to_json(&CommitPayload {
                cycle: cycle_id.clone(),
                date: date.clone(),
                snapshot: input.snapshot.dir.clone(),
                files: input.snapshot.files.clone(),
                topics_add,
                topics_set,
            }),
            after_op_key: None,
        });
    }
    let after_commit = (github || live_commit).then(|| COMMIT_KEY.to_string());

    if github && rules.issues {
        if input.repo.github.is_none() {
            plan.notes.push(
                "this repository is not linked to GitHub: no issues are created and no issue is changed"
                    .into(),
            );
        } else {
            for item in draft.items.iter().filter(|i| i.item_type == ItemType::Task) {
                let assignee = item
                    .assignee
                    .as_deref()
                    .and_then(|id| input.cycle.participant(id))
                    .and_then(|p| p.github.clone());
                let source = item.source.as_ref();
                let payload = IssuePayload {
                    cycle: cycle_id.clone(),
                    date: date.clone(),
                    item_id: item.id.clone(),
                    title: item.text.clone(),
                    assignee: assignee.clone(),
                    due: item.due.clone(),
                    time_ms: source.map(|s| s.t_start_ms),
                    quote: source.filter(|s| s.verified).map(|s| s.quote.clone()),
                    anchor: anchor(&item.id),
                };
                plan.summary.issues.push(PlannedIssue {
                    item_id: item.id.clone(),
                    title: item.text.clone(),
                    assignee,
                    due: item.due.clone(),
                });
                plan.ops.push(NewOp {
                    meeting_id: input.meeting_id.to_string(),
                    target: OutboxTarget::Issues,
                    kind: OutboxKind::CreateIssue,
                    op_key: issue_key(&item.id),
                    payload: to_json(&payload),
                    after_op_key: after_commit.clone(),
                });
            }
            if explicit {
                for p in draft
                    .status_proposals
                    .iter()
                    .filter(|p| p.decision == Decision::Approved)
                {
                    let Some(number) = issue_number(&p.target) else {
                        continue;
                    };
                    let payload = StatusPayload {
                        cycle: cycle_id.clone(),
                        date: date.clone(),
                        proposal_id: p.id.clone(),
                        number,
                        open: p.status == ProposedStatus::Open,
                        reason: p.comment.clone().unwrap_or_else(|| p.reason.clone()),
                        quote: p
                            .source
                            .as_ref()
                            .filter(|s| s.verified)
                            .map(|s| s.quote.clone()),
                        time_ms: p.source.as_ref().map(|s| s.t_start_ms),
                        anchor: anchor(&p.id),
                    };
                    plan.summary.statuses.push(PlannedStatus {
                        proposal_id: p.id.clone(),
                        target: p.target.clone(),
                        open: payload.open,
                    });
                    plan.ops.push(NewOp {
                        meeting_id: input.meeting_id.to_string(),
                        target: OutboxTarget::Issues,
                        kind: OutboxKind::SetIssueState,
                        op_key: status_key(&p.id),
                        payload: to_json(&payload),
                        after_op_key: after_commit.clone(),
                    });
                }
            }
        }
    }

    // ----- the Discord post
    if wants(PublishTarget::Discord) {
        match input.cycle.targets.discord.as_ref() {
            Some(target) if input.discord_webhook_set => {
                if !github && !live_commit && rules.github != PublishMode::Off {
                    plan.notes.push(
                        "the report is not published yet: the link in the message points to report.md"
                            .into(),
                    );
                }
                plan.summary.discord = true;
                let files = input
                    .snapshot
                    .files
                    .iter()
                    .filter(|f| f.starts_with("img/"))
                    .cloned()
                    .collect();
                plan.ops.push(NewOp {
                    meeting_id: input.meeting_id.to_string(),
                    target: OutboxTarget::Discord,
                    kind: OutboxKind::PostDiscord,
                    op_key: DISCORD_KEY.to_string(),
                    payload: to_json(&DiscordPayload {
                        cycle: cycle_id,
                        date,
                        snapshot: input.snapshot.dir.clone(),
                        reference: target.webhook.clone(),
                        template: target.template.clone(),
                        files,
                    }),
                    after_op_key: after_commit,
                });
            }
            Some(_) if explicit => plan.blocks.push(Block::DiscordWebhookMissing),
            Some(_) => plan
                .notes
                .push("no Discord webhook is stored: the message is not posted".into()),
            None => {}
        }
    }

    // ----- the public-repository check, for everything that goes to the repository
    let to_repository = plan
        .ops
        .iter()
        .any(|o| matches!(o.target, OutboxTarget::Github | OutboxTarget::Issues));
    if to_repository && input.repo.has_remote {
        if let Err(e) = gate::require(input.repo.visibility, input.repo.confirmed) {
            plan.blocks.push(Block::PublicRepoUnconfirmed {
                visibility: e.visibility.as_str().to_string(),
            });
        }
    }
    if plan.ops.is_empty() && explicit && plan.blocks.is_empty() {
        plan.blocks.push(Block::NothingToPublish);
    }
    plan
}

pub(crate) fn to_json<T: Serialize>(payload: &T) -> String {
    serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
}

/// The topics of the cycle as they should be after this commit: the local list with the
/// topics the meeting adds and the approved changes applied. Everything the payload does
/// not mention stays as it is here.
pub fn apply_topics(
    local: &[Topic],
    add: &[TopicAdd],
    set: &[TopicSet],
    opened_in: &str,
) -> Vec<Topic> {
    let mut out: Vec<Topic> = local.to_vec();
    for a in add {
        if !out.iter().any(|t| t.id == a.id) {
            out.push(Topic {
                id: a.id.clone(),
                text: a.text.clone(),
                status: TopicStatus::Open,
                opened_in: opened_in.to_string(),
                closed_in: None,
            });
        }
    }
    for s in set {
        if let Some(t) = out.iter_mut().find(|t| t.id == s.id) {
            if s.open {
                t.status = TopicStatus::Open;
                t.closed_in = None;
            } else {
                t.status = TopicStatus::Closed;
                t.closed_in = Some(opened_in.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
