//! The whole publishing loop on a demo-like cycle, against a bare Git remote, a fake GitHub
//! and a fake Discord webhook: approve status changes, publish to every target, check what
//! the remote, the issues and the channel hold, close an issue and see it done in the next
//! briefing. No real host, token, repository or webhook is involved.

use std::collections::BTreeMap;

use cycle_store::issues::MAX_PAGES;
use cycle_store::{
    apply_fetch, build_context, ContextRequest, Decision, Draft, ItemType, ProposedStatus,
    TopicStatus,
};
use publishers::github::{Github, RepoRef};
use publishers::outbox::Env;
use publishers::rules::{PublishTarget, Trigger};
use publishers::testing::fixtures::{item, proposal, task};
use publishers::testing::lab::{lab_draft, Lab, LabOptions, MEETING, REPO, TOKEN};
use serde_json::Value;
use store::OpState;

const BOTH: [PublishTarget; 2] = [PublishTarget::Github, PublishTarget::Discord];

fn anchors() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("i1".to_string(), "t-00-12-25".to_string()),
        ("s1".to_string(), "t-00-15-00".to_string()),
    ])
}

/// The meeting of the demo: two tasks, a decision, a new topic and a note, and three status
/// changes the model proposed, decided by the user.
fn demo_draft(lab: &Lab) -> (Draft, u64, u64) {
    let finished = lab
        .github
        .add_issue(REPO, "Mount the camera", &["cycle:NOMAD"]);
    let untouched = lab
        .github
        .add_issue(REPO, "Order the antenna", &["cycle:NOMAD"]);
    let mut approved = proposal(
        "s1",
        &format!("#{finished}"),
        ProposedStatus::Done,
        Decision::Approved,
    );
    approved.comment = Some("The camera is mounted, tested on Friday.".into());
    let rejected = proposal(
        "s2",
        &format!("#{untouched}"),
        ProposedStatus::Done,
        Decision::Rejected,
    );
    let topic = proposal("s3", "t-2", ProposedStatus::Done, Decision::Approved);
    let mut d = lab_draft(
        vec![
            task("i1", "Design the mount", Some("alex"), Some("2026-11-20")),
            task("i2", "Order the wheels", Some("sam"), None),
            item("i3", ItemType::Decision, "Use six wheels"),
            item("i4", ItemType::Topic, "Battery supplier"),
            item("i5", ItemType::Note, "The workshop is closed on Monday"),
        ],
        vec![approved, rejected, topic],
    );
    d.summary = "We settled the wheels and the mount.".into();
    (d, finished, untouched)
}

#[test]
fn a_demo_meeting_reaches_the_repository_the_issues_and_the_channel() {
    let lab = Lab::new(LabOptions::default());
    let (draft, finished, untouched) = demo_draft(&lab);

    let outcome = lab
        .publish(&draft, Trigger::Explicit, &BOTH, &anchors())
        .expect("publish");
    assert_eq!(
        outcome
            .plan
            .ops
            .iter()
            .map(|o| o.op_key.as_str())
            .collect::<Vec<_>>(),
        ["commit", "issue:i1", "issue:i2", "status:s1", "discord"]
    );
    assert_eq!(outcome.plan.summary.proposals_pending, 0);

    let report = lab.run_until_settled(5);
    assert!(
        report <= 2,
        "one tick is enough on a good network: {report}"
    );
    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");

    // ----- the repository
    let paths = lab.remote_paths();
    for want in [
        "meetings/NOMAD/2026-11-13/meeting.json",
        "meetings/NOMAD/2026-11-13/report.md",
        "meetings/NOMAD/2026-11-13/transcript.md",
        "meetings/NOMAD/2026-11-13/img/0001-00-12-41.png",
        "cycles/NOMAD/topics.yaml",
        "cycles/NOMAD/cycle.yaml",
    ] {
        assert!(paths.contains(&want.to_string()), "{want} in {paths:?}");
    }
    assert_eq!(lab.remote_commits(), 2, "one commit for the whole meeting");
    let meeting_json = lab.remote_show("meetings/NOMAD/2026-11-13/meeting.json");
    let published = Draft::from_json_str(&meeting_json).expect("meeting.json validates");
    assert_eq!(published, draft, "what was approved is what is published");
    assert!(lab
        .remote_show("meetings/NOMAD/2026-11-13/report.md")
        .contains("Design the mount"));
    let topics = cycle_store::parse_topics(&lab.remote_show("cycles/NOMAD/topics.yaml")).unwrap();
    let by_id = |id: &str| topics.iter().find(|t| t.id == id).cloned();
    assert_eq!(
        by_id("t-2").unwrap().status,
        TopicStatus::Closed,
        "approved closure"
    );
    assert_eq!(
        by_id("t-2").unwrap().closed_in.as_deref(),
        Some("2026-11-13")
    );
    assert_eq!(by_id("t-1").unwrap().status, TopicStatus::Open);
    let added = by_id(&format!("{MEETING}-i4")).expect("the new topic");
    assert_eq!(
        (added.text.as_str(), added.status),
        ("Battery supplier", TopicStatus::Open)
    );
    // Nothing was pushed to GitHub's idea of the files other than through git: the files of
    // the clone follow the remote.
    assert!(lab
        .clone_dir
        .join("meetings/NOMAD/2026-11-13/transcript.md")
        .is_file());

    // ----- the issues
    let issues = lab.github.issues(REPO);
    let titles: Vec<_> = issues.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Mount the camera",
            "Order the antenna",
            "Design the mount",
            "Order the wheels"
        ]
    );
    let first = lab.github.issue(REPO, finished).unwrap();
    assert!(!first.open, "the approved status change closed it");
    assert_eq!(first.state_reason.as_deref(), Some("completed"));
    assert!(first.comments[0].body.contains("The camera is mounted"));
    assert!(first.comments[0].body.contains("transcript.md#t-00-15-00"));
    assert!(
        lab.github.issue(REPO, untouched).unwrap().open,
        "a rejected change is not written back"
    );
    let mount = &issues[2];
    assert_eq!(mount.assignees, ["alex-gh"]);
    assert!(mount.labels.contains(&"cycle:NOMAD".to_string()));
    assert!(mount.body.contains("Due: 2026-11-20"));
    assert!(mount.body.contains("meetings/NOMAD/2026-11-13/report.md"));

    // ----- the channel
    let posts = lab.discord.messages();
    assert_eq!(posts.len(), 2, "the summary and the screenshot");
    let text = &posts[0].content;
    assert!(
        text.contains("We settled the wheels and the mount."),
        "{text}"
    );
    assert!(
        text.contains("<@111111111111111111>"),
        "Alex is mentioned: {text}"
    );
    assert!(
        text.contains("Design the mount") && text.contains("Use six wheels"),
        "{text}"
    );
    assert!(
        text.contains("meetings/NOMAD/2026-11-13/report.md"),
        "{text}"
    );
    assert!(
        text.contains("Issues:") && text.contains("Design the mount"),
        "{text}"
    );
    let mentions = posts[0].allowed_mentions.clone().unwrap();
    assert_eq!(mentions["parse"], serde_json::json!([]));
    assert_eq!(mentions["users"], serde_json::json!(["111111111111111111"]));
    assert_eq!(posts[1].files.len(), 1);

    // ----- the bookkeeping
    let row = lab
        .store
        .lock()
        .unwrap()
        .get_publication(MEETING)
        .unwrap()
        .unwrap();
    assert_eq!(row.folder.as_deref(), Some("2026-11-13"));
    assert!(row
        .report_url
        .unwrap()
        .contains("meetings/NOMAD/2026-11-13/report.md"));
    assert!(publishers::flow::is_locked(&lab.store.lock().unwrap(), MEETING).unwrap());
    let result: Value =
        serde_json::from_str(lab.op("issue:i1").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["created"], true);
    assert_eq!(result["assigned"], "alex-gh");
}

#[test]
fn an_issue_closed_on_github_is_done_in_the_next_briefing() {
    let lab = Lab::new(LabOptions::default());
    let (draft, _, _) = demo_draft(&lab);
    lab.publish(&draft, Trigger::Explicit, &BOTH, &anchors())
        .expect("publish");
    lab.run_until_settled(5);
    assert!(lab.ops().iter().all(|o| o.state == OpState::Done));

    // The issue of the first task is closed by a person.
    let number = {
        let issues = lab.github.issues(REPO);
        issues
            .iter()
            .find(|i| i.title == "Design the mount")
            .unwrap()
            .number
    };
    lab.github.close_issue(REPO, number);

    // The next briefing reads the state of the issues into its cache ...
    let repo = lab.repo();
    let api = Github::with_client(lab.env.client(), &lab.github.api_base(), TOKEN).unwrap();
    let fetched = api
        .list_cycle_issues(&RepoRef::parse(REPO).unwrap(), "NOMAD", MAX_PAGES)
        .map_err(|e| e.message);
    let list = apply_fetch(
        fetched,
        REPO,
        "cycle:NOMAD",
        "NOMAD",
        &repo.issue_cache_path("NOMAD").unwrap(),
    );
    assert!(list.error.is_none(), "{list:?}");
    assert!(list.closed.contains(&number));

    // ... and builds the context of the next meeting from it.
    let cycle = repo.load_cycle("NOMAD").unwrap();
    let topics = repo.load_topics("NOMAD").unwrap();
    let closed: Vec<String> = list.closed.iter().map(u64::to_string).collect();
    let dirs = vec![repo.meetings_dir("NOMAD").unwrap()];
    let mut request = ContextRequest::new(&cycle, "2026-11-20");
    request.topics = &topics;
    request.issues = &list.issues;
    request.closed_issue_ids = &closed;
    request.issue_links = &list.links;
    request.previous_dirs = &dirs;
    let context = build_context(&request).unwrap();
    let previous: Vec<(String, Option<String>)> = context.value["previous_items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["text"].as_str().unwrap().to_string(),
                i["issue"].as_str().map(String::from),
            )
        })
        .collect();
    let texts: Vec<&str> = previous.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"Design the mount"),
        "its issue is closed, so it is done: {texts:?}"
    );
    let wheels = previous
        .iter()
        .find(|(t, _)| t == "Order the wheels")
        .expect("the open task is still there");
    assert!(
        wheels.1.as_deref().is_some_and(|i| i.starts_with('#')),
        "{wheels:?}"
    );
}

#[test]
fn a_second_meeting_the_same_day_gets_its_own_folder() {
    let lab = Lab::new(LabOptions::default());
    let first = lab_draft(
        vec![task("i1", "Design the mount", Some("alex"), None)],
        vec![],
    );
    lab.publish(
        &first,
        Trigger::Explicit,
        &[PublishTarget::Github],
        &BTreeMap::new(),
    )
    .unwrap();
    lab.run_until_settled(5);

    // Another meeting of the same day (a different meeting id).
    let second_id = "20261113-180000";
    {
        let store = lab.store.lock().unwrap();
        store
            .create_meeting(&store::Meeting {
                id: second_id.into(),
                title: "Evening".into(),
                started_unix_ms: 2,
                duration_ms: None,
                state: store::MeetingState::Stopped,
                dir: second_id.into(),
            })
            .unwrap();
    }
    std::fs::create_dir_all(lab.env.snapshot_root().join(second_id).join("img")).unwrap();
    let mut second = lab_draft(
        vec![task("i1", "Sand the frame", Some("sam"), None)],
        vec![],
    );
    second.meeting_id = second_id.into();
    lab.publish(
        &second,
        Trigger::Explicit,
        &[PublishTarget::Github],
        &BTreeMap::new(),
    )
    .unwrap();
    lab.run_until_settled(5);

    let paths = lab.remote_paths();
    assert!(paths.contains(&"meetings/NOMAD/2026-11-13/report.md".to_string()));
    assert!(
        paths.contains(&"meetings/NOMAD/2026-11-13-2/report.md".to_string()),
        "{paths:?}"
    );
    assert_eq!(lab.github.issues(REPO).len(), 2);
}
