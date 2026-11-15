use std::collections::BTreeMap;

use super::*;
use crate::gate::RepoVisibility;
use crate::github::RepoRef;
use crate::plan::{self, GithubIdentity, PlanInput, RepoFacts, SnapshotRef};
use crate::rules::{PublishTarget, Trigger};
use crate::testing::fixtures::{cycle, draft, task};
use cycle_store::PublishMode;
use store::{Meeting, MeetingState};

const MEETING: &str = "20261113-140501";

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.create_meeting(&Meeting {
        id: MEETING.into(),
        title: "Weekly".into(),
        started_unix_ms: 1,
        duration_ms: None,
        state: MeetingState::Stopped,
        dir: MEETING.into(),
    })
    .unwrap();
    s
}

fn plan_for(visibility: RepoVisibility, confirmed: bool, snapshot: &str) -> plan::PublishPlan {
    let d = draft(
        MEETING,
        vec![task("i1", "Design the mount", Some("alex"), None)],
        vec![],
    );
    let c = cycle(Some(PublishMode::OnApproval), None);
    plan::build(&PlanInput {
        meeting_id: MEETING,
        draft: &d,
        cycle: &c,
        local_topics: &[],
        repo: &RepoFacts {
            github: Some(GithubIdentity {
                repo: RepoRef::parse("owner/rover").unwrap(),
                api: "https://api.github.com".into(),
            }),
            has_remote: true,
            visibility,
            confirmed,
        },
        discord_webhook_set: false,
        anchors: &BTreeMap::new(),
        snapshot: &SnapshotRef {
            dir: snapshot.into(),
            files: vec!["meeting.json".into(), "report.md".into()],
        },
        existing: &[],
        trigger: Trigger::Explicit,
        targets: &[PublishTarget::Github],
    })
}

fn begin<'a>(snapshot: &'a str) -> Start<'a> {
    Start {
        meeting_id: MEETING,
        cycle_id: "NOMAD",
        snapshot_dir: snapshot,
        repo_key: Some("github.com/owner/rover".into()),
    }
}

#[test]
fn freezing_copies_what_goes_out_and_numbers_the_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let meeting = tmp.path().join(MEETING);
    fs::create_dir_all(meeting.join("img")).unwrap();
    fs::write(meeting.join("img/0001-00-12-41.png"), b"png-bytes").unwrap();
    fs::write(meeting.join("img/bad name.png"), b"x").unwrap();
    let d = draft(MEETING, vec![], vec![]);
    let images = vec![
        (
            "0001-00-12-41.png".to_string(),
            meeting.join("img/0001-00-12-41.png"),
        ),
        ("bad name.png".to_string(), meeting.join("img/bad name.png")),
    ];
    let input = SnapshotInput {
        draft: &d,
        report_md: "# Report\n",
        transcript_md: Some("# Transcript\n"),
        images: &images,
    };
    let first = freeze(&meeting, &input).unwrap();
    assert_eq!(first.n, 1);
    assert_eq!(
        first.files,
        [
            "img/0001-00-12-41.png",
            "meeting.json",
            "report.md",
            "transcript.md"
        ]
    );
    assert_eq!(
        fs::read(first.dir.join("img/0001-00-12-41.png")).unwrap(),
        b"png-bytes"
    );
    let json = fs::read_to_string(first.dir.join("meeting.json")).unwrap();
    assert!(json.ends_with('\n'));
    assert_eq!(Draft::from_json_str(&json).unwrap(), d);

    let second = freeze(
        &meeting,
        &SnapshotInput {
            transcript_md: None,
            images: &[],
            ..input
        },
    )
    .unwrap();
    assert_eq!(second.n, 2);
    assert_eq!(second.files, ["meeting.json", "report.md"]);
    assert!(first.dir.is_dir(), "an earlier snapshot is never touched");
    assert!(first.dir.starts_with(meeting.join(SNAPSHOT_DIR)));
}

#[test]
fn a_snapshot_that_cannot_be_written_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let d = draft(MEETING, vec![], vec![]);
    let images = vec![("a.png".to_string(), tmp.path().join("missing.png"))];
    let err = freeze(
        tmp.path(),
        &SnapshotInput {
            draft: &d,
            report_md: "r",
            transcript_md: None,
            images: &images,
        },
    );
    assert!(err.is_err());
    assert!(!tmp.path().join(SNAPSHOT_DIR).join("1").exists());
}

#[test]
fn starting_queues_the_plan_and_records_the_publication() {
    let s = store();
    assert_eq!(lock_reason(&s, MEETING).unwrap(), None);
    let plan = plan_for(RepoVisibility::NotPublic, false, "m/work/publish/1");
    let started = start(&s, &begin("m/work/publish/1"), &plan, 10).unwrap();
    assert_eq!(started.len(), 2);
    assert!(started.iter().all(|(_, created)| *created));
    let row = s.get_publication(MEETING).unwrap().unwrap();
    assert_eq!(row.snapshot_dir, "m/work/publish/1");
    assert_eq!(row.repo_key.as_deref(), Some("github.com/owner/rover"));
    assert!(lock_reason(&s, MEETING).unwrap().is_some());
    assert!(is_locked(&s, MEETING).unwrap());

    // Starting again changes nothing.
    let again = start(&s, &begin("m/work/publish/1"), &plan, 20).unwrap();
    assert!(again.iter().all(|(_, created)| !created));
    assert_eq!(s.list_ops(Some(MEETING)).unwrap().len(), 2);
}

#[test]
fn a_blocked_plan_queues_nothing_and_records_nothing() {
    let s = store();
    let plan = plan_for(RepoVisibility::Public, false, "m/work/publish/1");
    let err = start(&s, &begin("m/work/publish/1"), &plan, 10).unwrap_err();
    assert!(
        matches!(err, StartError::Blocked(ref b) if b.len() == 1),
        "{err:?}"
    );
    assert!(err.to_string().contains("public"), "{err}");
    assert!(s.list_ops(Some(MEETING)).unwrap().is_empty());
    assert!(s.get_publication(MEETING).unwrap().is_none());
    assert!(!is_locked(&s, MEETING).unwrap());
}

#[test]
fn cancelling_every_operation_unlocks_the_draft_and_a_new_snapshot_replaces_the_old() {
    let s = store();
    let plan = plan_for(RepoVisibility::NotPublic, false, "m/work/publish/1");
    let started = start(&s, &begin("m/work/publish/1"), &plan, 10).unwrap();
    for (op, _) in &started {
        s.cancel_op(op.id, 11).unwrap();
    }
    assert!(!is_locked(&s, MEETING).unwrap());

    let plan = plan_for(RepoVisibility::NotPublic, false, "m/work/publish/2");
    let restarted = start(&s, &begin("m/work/publish/2"), &plan, 20).unwrap();
    assert!(restarted
        .iter()
        .all(|(op, created)| *created && op.state == OpState::Queued));
    assert_eq!(
        s.get_publication(MEETING).unwrap().unwrap().snapshot_dir,
        "m/work/publish/2"
    );
    assert!(is_locked(&s, MEETING).unwrap());
}

#[test]
fn a_publication_in_progress_keeps_its_snapshot() {
    let s = store();
    let plan = plan_for(RepoVisibility::NotPublic, false, "m/work/publish/1");
    start(&s, &begin("m/work/publish/1"), &plan, 10).unwrap();
    let other = plan_for(RepoVisibility::NotPublic, false, "m/work/publish/2");
    start(&s, &begin("m/work/publish/2"), &other, 20).unwrap();
    assert_eq!(
        s.get_publication(MEETING).unwrap().unwrap().snapshot_dir,
        "m/work/publish/1"
    );
}
