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

// ----- the plan of topics.yaml -------------------------------------------------------------------

mod topics_plan {
    use super::*;
    use crate::testing::fixtures::{item, proposal};
    use crate::testing::lab::{git, lab_draft, Lab, LabOptions, MEETING as LAB_MEETING};
    use cycle_store::{Decision, ItemType, ProposedStatus};

    const GITHUB: [PublishTarget; 1] = [PublishTarget::Github];

    /// A new topic and the closing of `t-2`, approved.
    fn draft_with_topics() -> Draft {
        lab_draft(
            vec![item("i4", ItemType::Topic, "Battery supplier")],
            vec![proposal(
                "s1",
                "t-2",
                ProposedStatus::Done,
                Decision::Approved,
            )],
        )
    }

    fn preview(lab: &Lab, draft: &Draft) -> plan::PublishPlan {
        lab.preview(draft, Trigger::Explicit, &GITHUB, &BTreeMap::new())
    }

    fn fetch(lab: &Lab) {
        git(&lab.clone_dir, &["fetch", "--quiet", "origin"]);
    }

    #[test]
    fn the_plan_lists_each_topic_with_its_version_before_and_after() {
        let lab = Lab::new(LabOptions::default());
        let plan = preview(&lab, &draft_with_topics());
        let topics = plan.summary.topics.expect("topics are compared");
        assert!(topics.against_remote);
        assert_eq!(topics.unchanged, 1, "t-1 stays as it is");
        let rows: Vec<_> = topics
            .changes
            .iter()
            .map(|c| (c.id.as_str(), c.change, c.overrides_remote))
            .collect();
        assert_eq!(
            rows,
            [
                (format!("{LAB_MEETING}-i4").as_str(), "added", false),
                ("t-2", "changed", false)
            ]
        );
        let added = &topics.changes[0];
        assert!(added.before.is_none());
        assert_eq!(added.after.as_ref().unwrap().text, "Battery supplier");
        assert_eq!(added.after.as_ref().unwrap().status, "open");
        let closed = &topics.changes[1];
        assert_eq!(closed.before.as_ref().unwrap().status, "open");
        let after = closed.after.as_ref().unwrap();
        assert_eq!(
            (after.status, after.closed_in.as_deref()),
            ("closed", Some("2026-11-13"))
        );
        // The names stay available for a screen that cannot show the difference.
        assert_eq!(plan.summary.topics_added, ["Battery supplier"]);
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        // Nothing was written or fetched by looking.
        assert!(!lab
            .remote_paths()
            .iter()
            .any(|p| p.starts_with("meetings/")));
    }

    #[test]
    fn what_a_colleague_changed_and_was_fetched_is_the_before_and_is_marked_when_replaced() {
        let lab = Lab::new(LabOptions::default());
        // A colleague renames t-2 on the remote and the clone fetched it, but did not pull.
        lab.colleagues_push_topics(
            "- id: t-1\n  text: Wheel size\n  status: open\n  opened_in: '2026-11-06'\n- id: t-2\n  text: Battery pack\n  status: open\n  opened_in: '2026-11-06'\n",
        );
        fetch(&lab);
        let plan = preview(&lab, &draft_with_topics());
        let topics = plan.summary.topics.unwrap();
        let t2 = topics.changes.iter().find(|c| c.id == "t-2").unwrap();
        assert_eq!(
            t2.before.as_ref().unwrap().text,
            "Battery pack",
            "before is what the remote has"
        );
        // The remote changed it and so do we: the last write wins per topic, so the whole
        // topic is ours (its text included) and the screen says what it replaces.
        assert_eq!(t2.after.as_ref().unwrap().text, "Battery");
        assert_eq!(t2.after.as_ref().unwrap().status, "closed");
        assert!(t2.overrides_remote);
    }

    #[test]
    fn a_remote_that_was_never_fetched_is_said_so_and_a_broken_file_falls_back_to_names() {
        let lab = Lab::new(LabOptions::default());
        git(
            &lab.clone_dir,
            &["update-ref", "-d", "refs/remotes/origin/main"],
        );
        let plan = preview(&lab, &draft_with_topics());
        let topics = plan.summary.topics.unwrap();
        assert!(!topics.against_remote);
        assert_eq!(topics.changes.len(), 2);
        assert!(
            plan.notes[0].contains("this clone only"),
            "{:?}",
            plan.notes
        );

        let broken = Lab::new(LabOptions::default());
        broken.colleagues_push_topics("this: [is not\n");
        fetch(&broken);
        let plan = preview(&broken, &draft_with_topics());
        assert!(plan.summary.topics.is_none());
        assert!(
            plan.notes.iter().any(|n| n.contains("listed by name only")),
            "{:?}",
            plan.notes
        );
        assert_eq!(plan.summary.topics_added, ["Battery supplier"]);
    }

    #[test]
    fn a_meeting_without_topic_changes_leaves_the_file_alone() {
        let lab = Lab::new(LabOptions::default());
        let d = lab_draft(vec![task("i1", "Design the mount", None, None)], vec![]);
        let plan = preview(&lab, &d);
        assert!(plan.summary.commit);
        assert!(plan.summary.topics.is_none());
        assert!(plan.notes.is_empty());
    }
}
