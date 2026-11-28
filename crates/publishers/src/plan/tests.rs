use super::*;
use crate::github::RepoRef;
use crate::testing::fixtures::{cycle, draft, item, proposal, source, task};
use cycle_store::{Cycle, Decision, ItemType, ProposedStatus, PublishMode, Topic, TopicStatus};

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

fn private() -> RepoFacts {
    facts(RepoVisibility::NotPublic, false)
}

fn topic(id: &str, text: &str) -> Topic {
    Topic {
        id: id.into(),
        text: text.into(),
        status: TopicStatus::Open,
        opened_in: "2026-11-06".into(),
        closed_in: None,
    }
}

fn snapshot() -> SnapshotRef {
    SnapshotRef {
        dir: "m1/work/publish/1".into(),
        files: vec![
            "img/0001-00-12-41.png".into(),
            "meeting.json".into(),
            "report.md".into(),
            "transcript.md".into(),
        ],
    }
}

struct Case {
    draft: Draft,
    cycle: Cycle,
    topics: Vec<Topic>,
    repo: RepoFacts,
    global_rules: GlobalPublishRules,
    webhook: bool,
    bot_token: bool,
    notion_token: bool,
    existing_refs: Vec<ExternalRef>,
    existing: Vec<OutboxOp>,
    anchors: BTreeMap<String, String>,
}

impl Case {
    fn new(cycle: Cycle) -> Case {
        let items = vec![
            task("i1", "Design the mount", Some("alex"), Some("2026-11-20")),
            task("i2", "Order the wheels", Some("sam"), None),
            item("i3", ItemType::Decision, "Use six wheels"),
            item("i4", ItemType::Topic, "Battery supplier"),
        ];
        Case {
            draft: draft(MEETING, items, vec![]),
            cycle,
            topics: vec![topic("t-1", "Wheel size")],
            repo: private(),
            global_rules: GlobalPublishRules::default(),
            webhook: true,
            bot_token: true,
            notion_token: true,
            existing_refs: vec![],
            existing: vec![],
            anchors: BTreeMap::from([("i1".to_string(), "t-00-12-25".to_string())]),
        }
    }

    fn plan(&self, trigger: Trigger, targets: &[PublishTarget]) -> PublishPlan {
        build(&PlanInput {
            meeting_id: MEETING,
            draft: &self.draft,
            cycle: &self.cycle,
            local_topics: &self.topics,
            global_rules: self.global_rules,
            repo: &self.repo,
            discord_webhook_set: self.webhook,
            discord_bot_token_set: self.bot_token,
            notion_token_set: self.notion_token,
            existing_refs: &self.existing_refs,
            anchors: &self.anchors,
            snapshot: &snapshot(),
            existing: &self.existing,
            trigger,
            targets,
        })
    }

    fn explicit(&self, targets: &[PublishTarget]) -> PublishPlan {
        self.plan(Trigger::Explicit, targets)
    }
}

fn keys(plan: &PublishPlan) -> Vec<&str> {
    plan.ops.iter().map(|o| o.op_key.as_str()).collect()
}

const BOTH: [PublishTarget; 2] = [PublishTarget::Github, PublishTarget::Discord];
const GITHUB: [PublishTarget; 1] = [PublishTarget::Github];
const DISCORD: [PublishTarget; 1] = [PublishTarget::Discord];

fn with_discord(mode: PublishMode) -> Cycle {
    cycle(None, Some(("cred:discord/nomad", mode)))
}

#[test]
fn an_explicit_publication_queues_the_commit_the_issues_and_the_post_in_order() {
    let case = Case::new(with_discord(PublishMode::OnApproval));
    let plan = case.explicit(&BOTH);
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert_eq!(keys(&plan), ["commit", "issue:i1", "issue:i2", "discord"]);
    let after: Vec<_> = plan.ops.iter().map(|o| o.after_op_key.as_deref()).collect();
    assert_eq!(
        after,
        [None, Some("commit"), Some("commit"), Some("commit")]
    );
    let kinds: Vec<_> = plan.ops.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OutboxKind::CommitMeeting,
            OutboxKind::CreateIssue,
            OutboxKind::CreateIssue,
            OutboxKind::PostDiscord
        ]
    );
    assert!(plan.summary.commit && plan.summary.pushes && plan.summary.discord);
    assert_eq!(plan.summary.issues.len(), 2);
    assert_eq!(plan.summary.topics_added, ["Battery supplier"]);
}

#[test]
fn a_task_for_somebody_outside_the_project_is_noted_and_the_issue_stays_unassigned() {
    let mut case = Case::new(cycle(None, None));
    let mut outside = task("i5", "Send the invoice", None, None);
    outside.assignee_name = Some("Marta Outside".into());
    case.draft.items.push(outside);
    let plan = case.explicit(&GITHUB);
    assert!(keys(&plan).contains(&"issue:i5"));
    assert!(plan
        .notes
        .iter()
        .any(|n| n.contains("i5") && n.contains("Marta Outside") && n.contains("unassigned")));
}

#[test]
fn payloads_carry_what_the_operation_needs_and_no_secret() {
    let case = Case::new(with_discord(PublishMode::OnApproval));
    let plan = case.explicit(&BOTH);
    let issue: IssuePayload = serde_json::from_str(&plan.ops[1].payload).unwrap();
    assert_eq!(issue.title, "Design the mount");
    assert_eq!(
        issue.assignee.as_deref(),
        Some("alex-gh"),
        "participant to login"
    );
    assert_eq!(issue.due.as_deref(), Some("2026-11-20"));
    assert_eq!(issue.time_ms, Some(745_000));
    assert_eq!(issue.quote.as_deref(), Some("We will do it by Friday"));
    assert_eq!(issue.anchor.as_deref(), Some("t-00-12-25"));
    let second: IssuePayload = serde_json::from_str(&plan.ops[2].payload).unwrap();
    assert_eq!(second.anchor, None);

    let commit: CommitPayload = serde_json::from_str(&plan.ops[0].payload).unwrap();
    assert_eq!(commit.snapshot, "m1/work/publish/1");
    assert_eq!(commit.files, snapshot().files);
    assert_eq!(
        commit.topics_add,
        [TopicAdd {
            id: format!("{MEETING}-i4"),
            text: "Battery supplier".into()
        }]
    );
    let discord: DiscordPayload = serde_json::from_str(&plan.ops[3].payload).unwrap();
    assert_eq!(discord.reference, "cred:discord/nomad");
    assert_eq!(discord.files, ["img/0001-00-12-41.png"]);
    for op in &plan.ops {
        assert!(!op.payload.contains("https://"), "{}", op.payload);
    }
}

#[test]
fn a_quote_that_was_not_found_in_the_transcript_is_not_published() {
    let mut case = Case::new(cycle(None, None));
    case.draft.items[0].source = Some(source(false, 1000));
    let plan = case.explicit(&GITHUB);
    let issue: IssuePayload = serde_json::from_str(&plan.ops[1].payload).unwrap();
    assert_eq!(issue.quote, None);
    assert_eq!(
        issue.time_ms,
        Some(1000),
        "the time stays: it is not a claim about words"
    );
}

#[test]
fn by_default_nothing_is_published_on_its_own() {
    let case = Case::new(with_discord(PublishMode::OnApproval));
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(plan.ops.is_empty() && plan.blocks.is_empty(), "{plan:?}");
}

#[test]
fn auto_targets_publish_on_their_own_but_never_a_status_change() {
    let mut case = Case::new(cycle(
        Some(PublishMode::Auto),
        Some(("cred:discord/nomad", PublishMode::Off)),
    ));
    case.draft.status_proposals = vec![proposal(
        "s1",
        "#12",
        ProposedStatus::Done,
        Decision::Approved,
    )];
    let plan = case.plan(Trigger::Auto, &[]);
    assert_eq!(keys(&plan), ["commit", "issue:i1", "issue:i2"]);
    assert!(plan.summary.statuses.is_empty());
    assert!(!plan.summary.discord, "discord is off");
}

#[test]
fn an_auto_discord_target_posts_when_the_draft_is_complete() {
    let case = Case::new(cycle(
        Some(PublishMode::OnApproval),
        Some(("cred:discord/nomad", PublishMode::Auto)),
    ));
    let plan = case.plan(Trigger::Auto, &[]);
    assert_eq!(keys(&plan), ["discord"]);
    assert_eq!(
        plan.ops[0].after_op_key, None,
        "no commit planned or queued"
    );
}

#[test]
fn off_is_never_not_even_when_the_user_asks() {
    let case = Case::new(cycle(
        Some(PublishMode::Off),
        Some(("cred:discord/nomad", PublishMode::OnApproval)),
    ));
    let plan = case.explicit(&BOTH);
    assert_eq!(keys(&plan), ["discord"]);
    assert!(
        plan.notes.iter().any(|n| n.contains("github is off")),
        "{:?}",
        plan.notes
    );
    let only_github = case.explicit(&GITHUB);
    assert_eq!(only_github.blocks, [Block::NothingToPublish]);
}

#[test]
fn a_target_the_cycle_does_not_have_is_left_out_with_a_note() {
    let case = Case::new(cycle(None, None));
    let plan = case.explicit(&BOTH);
    assert_eq!(keys(&plan), ["commit", "issue:i1", "issue:i2"]);
    assert!(plan.notes.iter().any(|n| n.contains("no discord target")));
}

#[test]
fn the_issues_flag_and_a_repository_without_github_leave_only_the_commit() {
    let mut no_issues = cycle(None, None);
    no_issues.targets.github = Some(cycle_store::GithubTarget {
        issues: false,
        ..cycle_store::GithubTarget::default()
    });
    let plan = Case::new(no_issues).explicit(&GITHUB);
    assert_eq!(keys(&plan), ["commit"]);

    let mut case = Case::new(cycle(None, None));
    case.repo.github = None;
    let plan = case.explicit(&GITHUB);
    assert_eq!(keys(&plan), ["commit"]);
    assert!(plan
        .notes
        .iter()
        .any(|n| n.contains("not linked to GitHub")));
}

#[test]
fn approved_status_changes_are_written_back_and_the_others_are_not() {
    let mut case = Case::new(cycle(None, None));
    case.topics.push(topic("t-2", "Battery"));
    case.draft.status_proposals = vec![
        proposal("s1", "#12", ProposedStatus::Done, Decision::Approved),
        proposal("s2", "#13", ProposedStatus::Done, Decision::Rejected),
        proposal("s3", "#14", ProposedStatus::Done, Decision::Proposed),
        proposal("s4", "t-2", ProposedStatus::Done, Decision::Approved),
        proposal("s5", "t-9", ProposedStatus::Done, Decision::Approved),
        proposal("s6", "t-1", ProposedStatus::Open, Decision::Approved),
    ];
    case.draft.status_proposals[0].comment = Some("Shipped in the last sprint".into());
    let plan = case.explicit(&GITHUB);
    assert_eq!(keys(&plan), ["commit", "issue:i1", "issue:i2", "status:s1"]);
    let status: StatusPayload = serde_json::from_str(&plan.ops[3].payload).unwrap();
    assert_eq!((status.number, status.open), (12, false));
    assert_eq!(
        status.reason, "Shipped in the last sprint",
        "the user's comment wins"
    );
    assert_eq!(status.quote.as_deref(), Some("We will do it by Friday"));
    assert_eq!(plan.summary.proposals_pending, 1);
    let commit: CommitPayload = serde_json::from_str(&plan.ops[0].payload).unwrap();
    assert_eq!(
        commit.topics_set,
        [
            TopicSet {
                id: "t-2".into(),
                open: false
            },
            TopicSet {
                id: "t-1".into(),
                open: true
            }
        ]
    );
    assert!(
        plan.notes.iter().any(|n| n.contains("t-9")),
        "an unknown topic is reported: {:?}",
        plan.notes
    );
}

#[test]
fn a_topic_of_the_meeting_is_added_unless_it_exists_or_relates_to_one() {
    let mut case = Case::new(cycle(None, None));
    case.draft
        .items
        .push(item("i5", ItemType::Topic, "  wheel   SIZE "));
    let mut linked = item("i6", ItemType::Topic, "More on the battery");
    linked.relates_to = Some("t-1".into());
    case.draft.items.push(linked);
    let plan = case.explicit(&GITHUB);
    let commit: CommitPayload = serde_json::from_str(&plan.ops[0].payload).unwrap();
    let ids: Vec<_> = commit.topics_add.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, [format!("{MEETING}-i4")]);
}

#[test]
fn a_public_or_unknown_repository_blocks_until_the_user_confirms() {
    let mut case = Case::new(with_discord(PublishMode::OnApproval));
    for visibility in [RepoVisibility::Public, RepoVisibility::Unknown] {
        case.repo = facts(visibility, false);
        let plan = case.explicit(&BOTH);
        assert_eq!(
            plan.blocks,
            [Block::PublicRepoUnconfirmed {
                visibility: visibility.as_str().into()
            }]
        );
        assert!(plan.is_blocked());
        assert_eq!(plan.blocks[0].code(), "public_repo_unconfirmed");

        case.repo = facts(visibility, true);
        assert!(!case.explicit(&BOTH).is_blocked());
    }
    case.repo = facts(RepoVisibility::NotPublic, false);
    assert!(!case.explicit(&BOTH).is_blocked());
}

#[test]
fn the_check_covers_what_goes_to_the_repository_and_nothing_else() {
    let mut case = Case::new(with_discord(PublishMode::OnApproval));
    case.repo = facts(RepoVisibility::Public, false);
    // Discord alone does not touch the repository.
    assert!(!case.explicit(&DISCORD).is_blocked());
    // A repository without a remote never leaves this machine.
    case.repo.has_remote = false;
    let plan = case.explicit(&GITHUB);
    assert!(!plan.is_blocked());
    assert!(plan.summary.commit && !plan.summary.pushes);
    // The automatic path is checked as well.
    let mut auto = Case::new(cycle(Some(PublishMode::Auto), None));
    auto.repo = facts(RepoVisibility::Unknown, false);
    assert!(auto.plan(Trigger::Auto, &[]).is_blocked());
}

#[test]
fn a_webhook_without_a_stored_secret_is_an_unconfigured_target_that_is_left_out() {
    let mut case = Case::new(with_discord(PublishMode::Auto));
    case.webhook = false;
    let explicit = case.explicit(&DISCORD);
    assert_eq!(explicit.blocks, [Block::NothingToPublish]);
    assert!(explicit.ops.is_empty());
    assert!(explicit.notes.iter().any(|n| n.contains("Discord webhook")));
    assert!(!explicit.rules.is_configured(PublishTarget::Discord));
    let auto = case.plan(Trigger::Auto, &[]);
    assert!(auto.blocks.is_empty() && auto.ops.is_empty());
}

#[test]
fn a_later_publication_follows_the_commit_that_is_queued_already() {
    let mut case = Case::new(with_discord(PublishMode::OnApproval));
    let commit = store::OutboxOp {
        id: 1,
        meeting_id: MEETING.into(),
        target: OutboxTarget::Github,
        kind: OutboxKind::CommitMeeting,
        op_key: "commit".into(),
        payload: "{}".into(),
        state: OpState::Done,
        attempts: 0,
        next_attempt_unix_ms: None,
        last_error: None,
        result: None,
        after_id: None,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    };
    case.existing = vec![commit.clone()];
    let plan = case.explicit(&DISCORD);
    assert_eq!(plan.ops[0].after_op_key.as_deref(), Some("commit"));
    assert!(plan.notes.is_empty());

    case.existing[0].state = OpState::Cancelled;
    let plan = case.explicit(&DISCORD);
    assert_eq!(
        plan.ops[0].after_op_key, None,
        "a cancelled commit is not waited for"
    );
    assert!(plan.notes.iter().any(|n| n.contains("not published yet")));
}

#[test]
fn a_cycle_with_nothing_to_do_gives_a_commit_only_plan() {
    let mut case = Case::new(cycle(None, None));
    case.draft.items = vec![item("i1", ItemType::Note, "Weather was nice")];
    let plan = case.explicit(&GITHUB);
    assert_eq!(keys(&plan), ["commit"]);
}

#[test]
fn topics_are_applied_on_top_of_the_local_list() {
    let local = vec![topic("t-1", "Wheel size"), topic("t-2", "Battery")];
    let out = apply_topics(
        &local,
        &[
            TopicAdd {
                id: "m-i4".into(),
                text: "Supplier".into(),
            },
            TopicAdd {
                id: "t-1".into(),
                text: "ignored duplicate id".into(),
            },
        ],
        &[
            TopicSet {
                id: "t-2".into(),
                open: false,
            },
            TopicSet {
                id: "gone".into(),
                open: false,
            },
        ],
        "2026-11-13",
    );
    assert_eq!(out.len(), 3);
    assert_eq!(out[0], local[0]);
    assert_eq!(out[1].status, TopicStatus::Closed);
    assert_eq!(out[1].closed_in.as_deref(), Some("2026-11-13"));
    assert_eq!(
        (out[2].id.as_str(), out[2].opened_in.as_str()),
        ("m-i4", "2026-11-13")
    );
    let reopened = apply_topics(
        &out,
        &[],
        &[TopicSet {
            id: "t-2".into(),
            open: true,
        }],
        "2026-11-20",
    );
    assert_eq!(reopened[1].status, TopicStatus::Open);
    assert_eq!(reopened[1].closed_in, None);
}

#[test]
fn notion_export_plans_one_op_per_task_and_chains_after_commit() {
    let mut c = cycle(Some(PublishMode::OnApproval), None);
    c.targets.notion = Some(cycle_store::NotionTarget {
        token: "cred:notion/nomad".into(),
        database: "0123456789abcdef0123456789abcdef".into(),
        data_source: None,
        publish: Some(PublishMode::OnApproval),
        properties: cycle_store::NotionProperties::default(),
    });
    let mut case = Case::new(c);
    case.existing_refs.push(ExternalRef {
        meeting_id: MEETING.into(),
        target: RefTarget::Notion,
        ref_key: notion_ref_key("i1"),
        external_id: "page-1".into(),
        url: Some("https://www.notion.so/page-1".into()),
        extra: "{}".into(),
        updated_unix_ms: 1,
    });
    let plan = case.explicit(&[PublishTarget::Github, PublishTarget::Notion]);
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert_eq!(
        keys(&plan),
        ["commit", "issue:i1", "issue:i2", "notion:i1", "notion:i2"]
    );
    assert_eq!(plan.ops[3].kind, OutboxKind::ExportNotion);
    assert_eq!(plan.ops[3].target, OutboxTarget::Notion);
    assert_eq!(plan.ops[3].after_op_key.as_deref(), Some("commit"));
    let p: NotionPayload = serde_json::from_str(&plan.ops[3].payload).unwrap();
    assert_eq!(p.item_id, "i1");
    assert_eq!(p.assignee_name.as_deref(), Some("Alex Example"));
    assert_eq!(p.due.as_deref(), Some("2026-11-20"));
    assert_eq!(p.token_ref, "cred:notion/nomad");
    assert_eq!(plan.summary.notion.len(), 2);
    assert_eq!(
        plan.summary.notion[0].existing_url.as_deref(),
        Some("https://www.notion.so/page-1")
    );
    assert_eq!(plan.summary.notion[1].existing_url, None);

    // Without GitHub, Notion ops have no commit dependency.
    let notion_only = case.explicit(&[PublishTarget::Notion]);
    assert_eq!(keys(&notion_only), ["notion:i1", "notion:i2"]);
    assert_eq!(notion_only.ops[0].after_op_key, None);
}

#[test]
fn missing_notion_and_bot_tokens_block_explicit_and_note_auto() {
    let mut c = cycle(Some(PublishMode::Off), None);
    c.targets.notion = Some(cycle_store::NotionTarget {
        token: "cred:notion/nomad".into(),
        database: "0123456789abcdef0123456789abcdef".into(),
        data_source: None,
        publish: Some(PublishMode::Auto),
        properties: cycle_store::NotionProperties::default(),
    });
    c.targets.discord = Some(cycle_store::DiscordTarget {
        webhook: None,
        bot: Some(cycle_store::DiscordBot {
            token: "cred:discord-bot/nomad".into(),
            guild: "400000000000000001".into(),
            channel: "300000000000000001".into(),
            thread: true,
            edit_on_correction: true,
        }),
        publish: Some(PublishMode::Auto),
        template: None,
    });
    let mut case = Case::new(c);
    case.notion_token = false;
    case.bot_token = false;

    let explicit = case.explicit(&[PublishTarget::Notion, PublishTarget::Discord]);
    assert_eq!(
        explicit.blocks,
        [Block::NotionTokenMissing, Block::DiscordBotTokenMissing]
    );
    let auto = case.plan(Trigger::Auto, &[]);
    assert!(auto.blocks.is_empty());
    assert!(auto.ops.is_empty());
    assert_eq!(auto.notes.len(), 2);

    // With the bot token present, Discord plans in bot mode with thread metadata.
    case.bot_token = true;
    let bot_plan = case.explicit(&[PublishTarget::Discord]);
    assert!(bot_plan.blocks.is_empty());
    assert!(bot_plan.summary.discord);
    assert_eq!(bot_plan.summary.discord_mode, Some("bot"));
    assert!(bot_plan.summary.discord_thread);
    let dp: DiscordPayload = serde_json::from_str(&bot_plan.ops[0].payload).unwrap();
    let bot = dp.bot.expect("bot payload");
    assert_eq!(bot.token, "cred:discord-bot/nomad");
    assert_eq!(bot.guild, "400000000000000001");
    assert_eq!(bot.channel, "300000000000000001");
    assert!(bot.thread && bot.edit_on_correction);
}

#[test]
fn local_only_project_with_auto_rules_plans_no_ops() {
    let mut c = cycle(Some(PublishMode::Auto), None);
    c.targets.discord = Some(cycle_store::DiscordTarget {
        webhook: Some("".into()),
        bot: None,
        publish: Some(PublishMode::Auto),
        template: None,
    });
    c.targets.notion = Some(cycle_store::NotionTarget {
        token: "cred:notion/nomad".into(),
        database: "".into(),
        data_source: None,
        publish: Some(PublishMode::Auto),
        properties: cycle_store::NotionProperties::default(),
    });
    let mut case = Case::new(c);
    case.repo = RepoFacts {
        github: None,
        has_remote: false,
        visibility: RepoVisibility::NotPublic,
        confirmed: false,
    };

    let auto = case.plan(Trigger::Auto, &[]);
    assert!(
        auto.ops.is_empty(),
        "no ops planned for unconfigured targets"
    );
    assert!(!auto.summary.commit);
    assert!(!auto.summary.discord);
    assert!(auto.summary.notion.is_empty());
    assert!(auto.blocks.is_empty());

    let explicit = case.explicit(&[
        PublishTarget::Github,
        PublishTarget::Discord,
        PublishTarget::Notion,
    ]);
    assert!(explicit.ops.is_empty());
    assert_eq!(explicit.blocks, [Block::NothingToPublish]);
    assert!(explicit
        .notes
        .iter()
        .any(|n| n.contains("GitHub repository")));
}

#[test]
fn a_project_without_its_own_rule_plans_by_the_global_one() {
    use cycle_store::GithubTarget;
    let mut case = Case::new(cycle(None, None));
    case.cycle.targets.github = Some(GithubTarget::default());
    // Global on-approval (the default): an automatic trigger queues nothing for GitHub.
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(keys(&plan).is_empty(), "{:?}", keys(&plan));
    // The global rule changes to automatic: the same project now queues the commit by itself.
    case.global_rules.github = PublishMode::Auto;
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(keys(&plan).contains(&"commit"), "{:?}", keys(&plan));
    // The global rule is off: not even an explicit request goes through.
    case.global_rules.github = PublishMode::Off;
    let plan = case.explicit(&GITHUB);
    assert!(keys(&plan).is_empty(), "{:?}", keys(&plan));
}

#[test]
fn an_explicit_project_rule_beats_the_global_one_in_the_plan() {
    let mut case = Case::new(cycle(Some(PublishMode::OnApproval), None));
    case.global_rules.github = PublishMode::Auto;
    // The project said on-approval: the global automatic rule does not apply to it.
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(keys(&plan).is_empty(), "{:?}", keys(&plan));
    case.cycle.targets.github.as_mut().unwrap().publish = Some(PublishMode::Auto);
    case.global_rules.github = PublishMode::Off;
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(keys(&plan).contains(&"commit"), "{:?}", keys(&plan));
}

#[test]
fn the_global_rule_does_not_publish_to_an_unconfigured_target() {
    use cycle_store::GithubTarget;
    // WP-08a: no remote and no linked repository means no GitHub publication, whatever the rule.
    let mut case = Case::new(cycle(None, None));
    case.cycle.targets.github = Some(GithubTarget::default());
    case.global_rules.github = PublishMode::Auto;
    case.repo = RepoFacts {
        github: None,
        has_remote: false,
        visibility: RepoVisibility::NotPublic,
        confirmed: false,
    };
    let plan = case.plan(Trigger::Auto, &[]);
    assert!(!keys(&plan).contains(&"commit"), "{:?}", keys(&plan));
}
