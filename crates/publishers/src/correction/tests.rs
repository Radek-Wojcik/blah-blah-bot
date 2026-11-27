use super::*;
use crate::gate::RepoVisibility;
use crate::github::RepoRef;
use crate::plan::GithubIdentity;
use crate::testing::fixtures::cycle;
use cycle_store::PublishMode;

const MEETING: &str = "20261113-140501";

fn facts(visibility: RepoVisibility, confirmed: bool) -> RepoFacts {
    RepoFacts {
        github: Some(GithubIdentity {
            repo: RepoRef::parse("owner/rover").unwrap(),
            api: "https://api.github.com".into(),
        }),
        has_remote: true,
        visibility,
        confirmed,
    }
}

fn note(item: Option<&str>, issue: Option<&str>) -> Correction {
    Correction {
        id: "c1".into(),
        date: "2026-11-16".into(),
        text: "The battery was ordered.".into(),
        item: item.map(String::from),
        issue: issue.map(String::from),
    }
}

fn commit_op(state: OpState) -> OutboxOp {
    OutboxOp {
        id: 1,
        meeting_id: MEETING.into(),
        target: OutboxTarget::Github,
        kind: OutboxKind::CommitMeeting,
        op_key: "commit".into(),
        payload: "{}".into(),
        state,
        attempts: 0,
        next_attempt_unix_ms: None,
        last_error: None,
        result: None,
        after_id: None,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    }
}

struct Case {
    cycle: Cycle,
    correction: Correction,
    repo: RepoFacts,
    existing: Vec<OutboxOp>,
    in_repository: bool,
    discord_post_exists: bool,
    discord_bot_token_set: bool,
}

impl Case {
    fn new(github: Option<PublishMode>) -> Case {
        Case {
            cycle: cycle(github, None),
            correction: note(Some("i1"), None),
            repo: facts(RepoVisibility::NotPublic, false),
            existing: vec![commit_op(OpState::Done)],
            in_repository: true,
            discord_post_exists: false,
            discord_bot_token_set: true,
        }
    }

    fn plan(&self, trigger: Trigger) -> CorrectionPlan {
        build(&CorrectionPlanInput {
            meeting_id: MEETING,
            meeting_date: "2026-11-13",
            cycle: &self.cycle,
            correction: &self.correction,
            repo: &self.repo,
            existing: &self.existing,
            trigger,
            out_dir: "m1/work/corrections-out",
            in_repository: self.in_repository,
            discord_post_exists: self.discord_post_exists,
            discord_bot_token_set: self.discord_bot_token_set,
        })
    }
}

fn keys(plan: &CorrectionPlan) -> Vec<&str> {
    plan.ops.iter().map(|o| o.op_key.as_str()).collect()
}

#[test]
fn an_explicit_publication_adds_the_file_and_the_comment_in_order() {
    let plan = Case::new(Some(PublishMode::OnApproval)).plan(Trigger::Explicit);
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert_eq!(keys(&plan), ["correction:c1", "correction-issue:c1"]);
    assert_eq!(plan.ops[0].kind, OutboxKind::CommitCorrection);
    assert_eq!(plan.ops[0].target, OutboxTarget::Github);
    assert_eq!(plan.ops[0].after_op_key.as_deref(), Some("commit"));
    assert_eq!(plan.ops[1].kind, OutboxKind::CommentCorrection);
    assert_eq!(plan.ops[1].target, OutboxTarget::Issues);
    assert_eq!(plan.ops[1].after_op_key.as_deref(), Some("correction:c1"));
    assert_eq!(
        plan.summary,
        CorrectionSummary {
            file: true,
            pushes: true,
            comment: Some("i1".into()),
            discord_edit: false,
        }
    );
    let payload: CorrectionPayload = serde_json::from_str(&plan.ops[0].payload).unwrap();
    assert_eq!(payload.cycle, "NOMAD");
    assert_eq!(payload.meeting_date, "2026-11-13");
    assert_eq!(payload.correction.text, "The battery was ordered.");
    assert_eq!(payload.out_dir, "m1/work/corrections-out");
    assert_eq!(plan.ops[1].payload, plan.ops[0].payload);
}

#[test]
fn the_rule_of_the_github_target_decides_when() {
    // On approval: only when the user asks.
    let on_approval = Case::new(Some(PublishMode::OnApproval));
    assert!(on_approval.plan(Trigger::Auto).ops.is_empty());
    assert!(!on_approval.plan(Trigger::Explicit).ops.is_empty());
    // Automatic: also when the note is written.
    let auto = Case::new(Some(PublishMode::Auto));
    assert_eq!(keys(&auto.plan(Trigger::Auto)).len(), 2);
    assert_eq!(keys(&auto.plan(Trigger::Explicit)).len(), 2);
    // Off: never, and an explicit request says why nothing happens.
    let off = Case::new(Some(PublishMode::Off));
    assert!(off.plan(Trigger::Auto).ops.is_empty());
    let plan = off.plan(Trigger::Explicit);
    assert!(plan.ops.is_empty());
    assert_eq!(plan.blocks, [Block::NothingToPublish]);
    assert!(plan.notes[0].contains("off"));
    // A cycle without a github target uses the default (on approval).
    let none = Case::new(None);
    assert!(none.plan(Trigger::Auto).ops.is_empty());
    assert!(!none.plan(Trigger::Explicit).ops.is_empty());
}

#[test]
fn a_note_without_an_item_or_an_issue_is_only_a_file() {
    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.correction = note(None, None);
    let plan = case.plan(Trigger::Explicit);
    assert_eq!(keys(&plan), ["correction:c1"]);
    assert_eq!(plan.summary.comment, None);
    // An issue named directly is commented on, whatever the item.
    case.correction = note(None, Some("#12"));
    let plan = case.plan(Trigger::Explicit);
    assert_eq!(keys(&plan), ["correction:c1", "correction-issue:c1"]);
    assert_eq!(plan.summary.comment.as_deref(), Some("#12"));
}

#[test]
fn no_comment_when_issues_are_off_or_the_repository_is_not_on_github() {
    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.cycle.targets.github.as_mut().unwrap().issues = false;
    let plan = case.plan(Trigger::Explicit);
    assert_eq!(keys(&plan), ["correction:c1"]);
    assert!(plan.notes[0].contains("issues are off"));

    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.repo.github = None;
    let plan = case.plan(Trigger::Explicit);
    assert_eq!(keys(&plan), ["correction:c1"]);
    assert!(plan.notes[0].contains("not linked to GitHub"));
}

#[test]
fn a_meeting_that_is_not_in_the_repository_blocks_an_explicit_request_only() {
    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.in_repository = false;
    case.existing.clear();
    let plan = case.plan(Trigger::Explicit);
    assert_eq!(plan.blocks, [Block::MeetingNotInRepository]);
    assert!(plan.ops.is_empty());
    assert_eq!(plan.blocks[0].code(), "not_in_repository");
    // Automatic waits quietly.
    let mut auto = Case::new(Some(PublishMode::Auto));
    auto.in_repository = false;
    let plan = auto.plan(Trigger::Auto);
    assert!(plan.ops.is_empty() && plan.blocks.is_empty());
}

#[test]
fn a_queued_meeting_commit_is_waited_for_and_a_cancelled_one_is_not() {
    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.existing = vec![commit_op(OpState::Queued)];
    assert_eq!(
        case.plan(Trigger::Explicit).ops[0].after_op_key.as_deref(),
        Some("commit")
    );
    case.existing = vec![commit_op(OpState::Cancelled)];
    assert_eq!(case.plan(Trigger::Explicit).ops[0].after_op_key, None);
}

#[test]
fn the_public_repository_check_applies_also_under_automatic() {
    for mode in [PublishMode::OnApproval, PublishMode::Auto] {
        let mut case = Case::new(Some(mode));
        case.repo = facts(RepoVisibility::Public, false);
        let plan = case.plan(Trigger::Explicit);
        assert!(
            matches!(plan.blocks.as_slice(), [Block::PublicRepoUnconfirmed { visibility }] if visibility == "public"),
            "{:?}",
            plan.blocks
        );
        case.repo = facts(RepoVisibility::Unknown, false);
        assert!(case.plan(Trigger::Explicit).is_blocked());
        case.repo = facts(RepoVisibility::Public, true);
        assert!(!case.plan(Trigger::Explicit).is_blocked());
    }
    // Nothing leaves a repository without a remote.
    let mut local = Case::new(Some(PublishMode::OnApproval));
    local.repo = RepoFacts {
        github: None,
        has_remote: false,
        visibility: RepoVisibility::NotPublic,
        confirmed: false,
    };
    let plan = local.plan(Trigger::Explicit);
    assert!(!plan.is_blocked() && !plan.summary.pushes);
}

#[test]
fn bot_mode_with_edit_on_correction_queues_edit_discord() {
    let mut case = Case::new(Some(PublishMode::OnApproval));
    case.cycle.targets.discord = Some(cycle_store::DiscordTarget {
        webhook: None,
        bot: Some(cycle_store::DiscordBot {
            token: "cred:discord-bot/nomad".into(),
            guild: "400000000000000001".into(),
            channel: "300000000000000001".into(),
            thread: true,
            edit_on_correction: true,
        }),
        publish: PublishMode::OnApproval,
        template: None,
    });
    case.discord_post_exists = true;

    let plan = case.plan(Trigger::Explicit);
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert!(plan.summary.discord_edit);
    assert_eq!(
        keys(&plan),
        ["correction:c1", "correction-issue:c1", "discord-edit:c1"]
    );
    assert_eq!(plan.ops[2].kind, OutboxKind::EditDiscord);
    assert_eq!(plan.ops[2].target, OutboxTarget::Discord);
    assert_eq!(plan.ops[2].after_op_key.as_deref(), Some("correction:c1"));

    // When the Discord post is still queued, the edit waits for the post instead.
    case.existing.push(OutboxOp {
        id: 2,
        meeting_id: MEETING.into(),
        target: OutboxTarget::Discord,
        kind: OutboxKind::PostDiscord,
        op_key: DISCORD_KEY.into(),
        payload: "{}".into(),
        state: OpState::Queued,
        attempts: 0,
        next_attempt_unix_ms: None,
        last_error: None,
        result: None,
        after_id: None,
        created_unix_ms: 2,
        updated_unix_ms: 2,
    });
    let queued = case.plan(Trigger::Explicit);
    assert_eq!(queued.ops[2].after_op_key.as_deref(), Some(DISCORD_KEY));

    // Missing bot token blocks an explicit correction.
    case.discord_bot_token_set = false;
    assert_eq!(
        case.plan(Trigger::Explicit).blocks,
        [Block::DiscordBotTokenMissing]
    );

    // edit_on_correction: false skips EditDiscord cleanly.
    case.discord_bot_token_set = true;
    case.cycle
        .targets
        .discord
        .as_mut()
        .unwrap()
        .bot
        .as_mut()
        .unwrap()
        .edit_on_correction = false;
    let disabled = case.plan(Trigger::Explicit);
    assert!(!disabled.summary.discord_edit);
    assert_eq!(keys(&disabled), ["correction:c1", "correction-issue:c1"]);
}
