//! Corrections to a published meeting, against a bare Git remote, a fake GitHub and a fake
//! Discord webhook: the new file in the meeting folder, the comment on the issue, the rules,
//! the public-repository check, and what happens when the network is gone, an answer is lost
//! or a colleague pushes meanwhile. The context of the next meeting reads the corrections
//! back, from this clone and from a colleague's. No real host is involved.

use std::collections::BTreeMap;

use cycle_store::corrections::{self, Correction};
use cycle_store::{build_context, ContextRequest, ItemType, PublishMode, Repo};
use publishers::flow::{self, StartError};
use publishers::outbox::Env;
use publishers::plan::Block;
use publishers::rules::{PublishTarget, Trigger};
use publishers::testing::fixtures::{item, task};
use publishers::testing::lab::{git, lab_draft, Lab, LabOptions, MEETING, REPO, TOKEN};
use publishers::testing::{Fault, Visibility};
use store::OpState;

const BOTH: [PublishTarget; 2] = [PublishTarget::Github, PublishTarget::Discord];
const FOLDER: &str = "meetings/NOMAD/2026-11-13";

fn draft() -> cycle_store::Draft {
    lab_draft(
        vec![
            task("i1", "Design the mount", Some("alex"), Some("2026-11-20")),
            task("i2", "Order the wheels", Some("sam"), None),
            item("i3", ItemType::Decision, "Use six wheels"),
        ],
        vec![],
    )
}

fn note(id: &str, text: &str, item: Option<&str>, issue: Option<&str>) -> Correction {
    Correction {
        id: id.into(),
        date: "2026-11-16".into(),
        text: text.into(),
        item: item.map(String::from),
        issue: issue.map(String::from),
    }
}

/// A lab whose meeting is published to every target and settled.
fn published(options: LabOptions) -> Lab {
    let lab = Lab::new(options);
    lab.publish(&draft(), Trigger::Explicit, &BOTH, &BTreeMap::new())
        .expect("publish the meeting");
    lab.run_until_settled(6);
    assert!(
        lab.ops().iter().all(|o| o.state == OpState::Done),
        "{:?}",
        lab.states()
    );
    lab
}

fn issue_number(lab: &Lab, title: &str) -> u64 {
    lab.github
        .issues(REPO)
        .iter()
        .find(|i| i.title == title)
        .unwrap_or_else(|| panic!("no issue {title}"))
        .number
}

fn context_notes(repo: &Repo) -> Vec<serde_json::Value> {
    let cycle = repo.load_cycle("NOMAD").unwrap();
    let dirs = vec![repo.meetings_dir("NOMAD").unwrap()];
    let mut request = ContextRequest::new(&cycle, "2026-11-20");
    request.previous_dirs = &dirs;
    let context = build_context(&request).unwrap();
    context
        .value
        .get("corrections")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_correction_is_a_new_file_next_to_the_meeting_and_a_comment_on_its_issue() {
    let lab = published(LabOptions::default());
    let before = lab.remote_show(&format!("{FOLDER}/meeting.json"));
    let report = lab.remote_show(&format!("{FOLDER}/report.md"));
    let commits = lab.remote_commits();

    let first = note(
        "c1",
        "The mount is redesigned, see the new drawing.",
        Some("i1"),
        None,
    );
    let outcome = lab
        .publish_correction(&first, Trigger::Explicit)
        .expect("queue the correction");
    assert_eq!(
        outcome
            .plan
            .ops
            .iter()
            .map(|o| o.op_key.as_str())
            .collect::<Vec<_>>(),
        ["correction:c1", "correction-issue:c1"]
    );
    lab.run_until_settled(5);
    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");

    // The file: new, in the folder of the meeting, readable back; nothing else changed.
    let text = lab.remote_show(&format!("{FOLDER}/corrections.md"));
    let back = corrections::parse_markdown(&text).expect("a correction file");
    assert_eq!(back.text, first.text);
    assert_eq!(
        (
            back.marker.cycle.as_str(),
            back.marker.meeting.as_str(),
            back.marker.id.as_str()
        ),
        ("NOMAD", MEETING, "c1")
    );
    assert_eq!(back.marker.about.as_deref(), Some("i1"));
    assert_eq!(lab.remote_commits(), commits + 1, "one commit for the file");
    assert_eq!(lab.remote_show(&format!("{FOLDER}/meeting.json")), before);
    assert_eq!(lab.remote_show(&format!("{FOLDER}/report.md")), report);
    assert!(lab.clone_dir.join(FOLDER).join("corrections.md").is_file());

    // The comment: on the issue made from the item, with the link to the file.
    let number = issue_number(&lab, "Design the mount");
    let issue = lab.github.issue(REPO, number).unwrap();
    assert!(issue.open, "a correction does not close an issue");
    let comment = &issue.comments.last().unwrap().body;
    assert!(comment.contains("> The mount is redesigned"), "{comment}");
    assert!(
        comment.contains(&format!("{FOLDER}/corrections.md")),
        "{comment}"
    );
    assert!(corrections::CorrectionMarker::parse(comment).is_some());
    assert!(
        lab.github
            .issue(REPO, issue_number(&lab, "Order the wheels"))
            .unwrap()
            .comments
            .is_empty(),
        "other issues are left alone"
    );
    let result: serde_json::Value =
        serde_json::from_str(lab.op("correction:c1").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["file"], "corrections.md");
    assert_eq!(result["pushed"], true);
    assert!(result["url"].as_str().unwrap().ends_with("corrections.md"));

    // Another correction is another file, and the first is untouched.
    let second = note("c2", "The wheels arrive on Monday.", None, None);
    lab.publish_correction(&second, Trigger::Explicit).unwrap();
    lab.run_until_settled(5);
    assert!(lab
        .remote_show(&format!("{FOLDER}/corrections-2.md"))
        .contains("The wheels arrive on Monday."));
    assert_eq!(lab.remote_show(&format!("{FOLDER}/corrections.md")), text);
    assert!(
        lab.op("correction-issue:c1").state == OpState::Done
            && lab.ops().iter().all(|o| o.op_key != "correction-issue:c2"),
        "a note about no item has no comment"
    );

    // Queueing the same correction again changes nothing.
    let again = lab.publish_correction(&first, Trigger::Explicit).unwrap();
    assert!(again.ops.iter().all(|(_, created)| !created));
    assert_eq!(lab.tick().ran(), 0);
    assert_eq!(lab.github.issue(REPO, number).unwrap().comments.len(), 1);
}

#[test]
fn the_next_meeting_gets_the_corrections_from_this_clone_and_from_a_colleagues() {
    let lab = published(LabOptions::default());
    lab.publish_correction(
        &note("c1", "The mount is redesigned.", Some("i1"), None),
        Trigger::Explicit,
    )
    .unwrap();
    lab.publish_correction(
        &note("c2", "Six wheels were dropped.", Some("i3"), None),
        Trigger::Explicit,
    )
    .unwrap();
    lab.run_until_settled(6);

    // This clone: the files came with the fast-forward.
    let mine = context_notes(&lab.repo());
    let texts: Vec<_> = mine.iter().map(|c| c["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        ["Six wheels were dropped.", "The mount is redesigned."]
    );
    assert_eq!(mine[0]["item"], "Use six wheels");
    assert_eq!(mine[1]["item"], "Design the mount");
    assert_eq!(mine[1]["meeting"], "2026-11-13");
    assert_eq!(mine[1]["date"], "2026-11-16");
    assert!(mine[0]["issue"].is_null(), "a decision has no issue");

    // A colleague who clones the repository reads them from the published files.
    let colleague = Repo::clone(
        lab.remote.to_str().unwrap(),
        lab.tmp.path().join("colleague-clone"),
    )
    .unwrap();
    assert_eq!(context_notes(&colleague), mine);
}

#[test]
fn the_rule_of_the_github_target_decides_when_a_correction_goes_out() {
    // On approval: nothing until the user publishes, and a request of the user publishes.
    let lab = published(LabOptions::default());
    let c = note("c1", "Moved.", None, None);
    let auto = lab.publish_correction(&c, Trigger::Auto).unwrap();
    assert!(auto.ops.is_empty(), "on approval waits for the user");
    assert!(!lab
        .publish_correction(&c, Trigger::Explicit)
        .unwrap()
        .ops
        .is_empty());

    // Automatic: queued when the note is written.
    let lab = published(LabOptions {
        github: PublishMode::Auto,
        ..LabOptions::default()
    });
    let auto = lab.publish_correction(&c, Trigger::Auto).unwrap();
    assert_eq!(auto.ops.len(), 1);
    lab.run_until_settled(4);
    assert!(lab
        .remote_paths()
        .contains(&format!("{FOLDER}/corrections.md")));
}

#[test]
fn off_is_absolute_for_a_meeting_that_is_in_the_repository() {
    let lab = published(LabOptions::default());
    // The user switches the rule off after the meeting was published.
    let cycle_file = lab.clone_dir.join("cycles/NOMAD/cycle.yaml");
    let yaml = std::fs::read_to_string(&cycle_file).unwrap();
    assert!(yaml.contains("on-approval"), "{yaml}");
    std::fs::write(
        &cycle_file,
        yaml.replace("publish: on-approval", "publish: off"),
    )
    .unwrap();
    let c = note("c1", "Moved.", None, None);
    let err = lab.publish_correction(&c, Trigger::Explicit).err().unwrap();
    assert!(
        matches!(&err, StartError::Blocked(b) if b == &[Block::NothingToPublish]),
        "{err}"
    );
    assert!(lab
        .ops()
        .iter()
        .all(|o| !o.op_key.starts_with("correction")));
}

#[test]
fn a_public_repository_blocks_a_correction_until_it_is_confirmed() {
    let lab = published(LabOptions {
        // The meeting went out while the repository was private ...
        visibility: Visibility::Private,
        ..LabOptions::default()
    });
    // ... and has become public since.
    lab.github.set_visibility(REPO, Visibility::Public);
    let c = note("c1", "Moved.", Some("i1"), None);
    let err = lab.publish_correction(&c, Trigger::Explicit).err().unwrap();
    assert!(
        matches!(&err, StartError::Blocked(b) if matches!(b.as_slice(), [Block::PublicRepoUnconfirmed { .. }])),
        "{err}"
    );
    assert!(lab
        .ops()
        .iter()
        .all(|o| !o.op_key.starts_with("correction")));

    let ctx = lab.env.context("NOMAD").unwrap();
    {
        let store = lab.store.lock().unwrap();
        flow::confirm_public_repo(&store, lab.env.as_ref(), &ctx).unwrap();
    }
    lab.publish_correction(&c, Trigger::Explicit).unwrap();
    lab.run_until_settled(5);
    assert!(lab
        .remote_paths()
        .contains(&format!("{FOLDER}/corrections.md")));
}

#[test]
fn a_correction_queued_earlier_cannot_slip_into_a_repository_that_became_public() {
    let lab = published(LabOptions::default());
    lab.publish_correction(&note("c1", "Moved.", Some("i1"), None), Trigger::Explicit)
        .unwrap();
    lab.github.set_visibility(REPO, Visibility::Public);
    // What the dispatcher learned about the visibility is kept for a short while only.
    lab.env.advance(31_000);
    lab.run_until_settled(4);
    let op = lab.op("correction:c1");
    assert_eq!(op.state, OpState::Failed, "{:?}", lab.states());
    assert!(op
        .last_error
        .unwrap()
        .starts_with("public_repo_unconfirmed"));
    assert!(!lab
        .remote_paths()
        .contains(&format!("{FOLDER}/corrections.md")));
}

#[test]
fn a_correction_waits_for_a_meeting_that_is_still_being_published() {
    let lab = Lab::new(LabOptions::default());
    lab.publish(&draft(), Trigger::Explicit, &BOTH, &BTreeMap::new())
        .unwrap();
    // Nothing has run yet: the commit, the issues and the message are queued.
    let c = note("c1", "The mount is redesigned.", Some("i1"), None);
    lab.publish_correction(&c, Trigger::Explicit).unwrap();
    lab.run_until_settled(8);
    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");
    assert!(lab
        .remote_paths()
        .contains(&format!("{FOLDER}/corrections.md")));
    let number = issue_number(&lab, "Design the mount");
    let comments = lab.github.issue(REPO, number).unwrap().comments;
    assert_eq!(
        comments.len(),
        1,
        "the comment found the issue that was just made"
    );
}

#[test]
fn a_meeting_that_is_not_in_the_repository_refuses_an_explicit_correction() {
    let lab = Lab::new(LabOptions::default());
    let c = note("c1", "Moved.", None, None);
    let err = lab.publish_correction(&c, Trigger::Explicit).err().unwrap();
    assert!(
        matches!(&err, StartError::Blocked(b) if b == &[Block::MeetingNotInRepository]),
        "{err}"
    );
    // A Discord-only publication does not put the meeting into the repository either.
    let lab = Lab::new(LabOptions::default());
    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Discord],
        &BTreeMap::new(),
    )
    .unwrap();
    lab.run_until_settled(4);
    let err = lab.publish_correction(&c, Trigger::Explicit).err().unwrap();
    assert!(matches!(&err, StartError::Blocked(_)), "{err}");
}

#[test]
fn offline_then_back_the_correction_goes_out_once() {
    let lab = published(LabOptions::default());
    let c = note("c1", "The mount is redesigned.", Some("i1"), None);
    lab.publish_correction(&c, Trigger::Explicit).unwrap();

    lab.remote_offline();
    lab.github.go_offline();
    let report = lab.tick();
    assert_eq!(report.retrying, 1, "{:?}", lab.states());
    let file = lab.op("correction:c1");
    assert_eq!(file.state, OpState::Queued);
    assert_eq!(file.attempts, 1);
    assert!(file.last_error.is_some());
    assert_eq!(
        lab.op("correction-issue:c1").state,
        OpState::Queued,
        "the comment waits for the file"
    );
    // The draft stays locked all along, and nothing runs before the retry time.
    assert!(flow::is_locked(&lab.store.lock().unwrap(), MEETING).unwrap());
    assert_eq!(lab.tick().ran(), 0);

    lab.remote_online();
    lab.github.go_online();
    lab.store
        .lock()
        .unwrap()
        .retry_all_ops(lab.env.now_ms())
        .unwrap();
    lab.dispatcher.clear_offline();
    lab.run_until_settled(6);
    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");
    let files: Vec<_> = lab
        .remote_paths()
        .into_iter()
        .filter(|p| p.contains("corrections"))
        .collect();
    assert_eq!(files, [format!("{FOLDER}/corrections.md")], "one file");
    let number = issue_number(&lab, "Design the mount");
    assert_eq!(lab.github.issue(REPO, number).unwrap().comments.len(), 1);
}

#[test]
fn lost_answers_make_neither_a_second_file_nor_a_second_comment() {
    let lab = published(LabOptions::default());
    let c = note("c1", "The mount is redesigned.", Some("i1"), None);
    lab.publish_correction(&c, Trigger::Explicit).unwrap();
    let number = issue_number(&lab, "Design the mount");
    // The comment is stored but the answer never arrives.
    lab.github.inject_for(
        "POST",
        &format!("/repos/owner/rover/issues/{number}/comments"),
        Fault::DropAfter,
        1,
    );
    lab.tick();
    assert_eq!(lab.op("correction:c1").state, OpState::Done);
    assert_eq!(lab.github.issue(REPO, number).unwrap().comments.len(), 1);
    assert_ne!(lab.op("correction-issue:c1").state, OpState::Done);
    lab.run_until_settled(6);
    assert_eq!(lab.op("correction-issue:c1").state, OpState::Done);
    assert_eq!(lab.github.issue(REPO, number).unwrap().comments.len(), 1);
    assert_eq!(
        lab.remote_paths()
            .iter()
            .filter(|p| p.contains("corrections"))
            .count(),
        1
    );
}

#[test]
fn a_colleague_pushing_meanwhile_is_absorbed_and_a_cancelled_correction_can_be_queued_again() {
    let lab = published(LabOptions::default());
    let c = note("c1", "Moved.", None, None);
    lab.publish_correction(&c, Trigger::Explicit).unwrap();
    let remote = lab.remote.clone();
    let scratch = lab.tmp.path().join("colleagues");
    let mut fired = false;
    *lab.env.push_hook.lock().unwrap() = Some(Box::new(move |_attempt| {
        if !fired {
            fired = true;
            publishers::testing::lab::colleague_push(
                &remote,
                &scratch,
                "x",
                &[(
                    "docs/colleague.md",
                    "hello
",
                )],
            );
        }
    }));
    lab.run_until_settled(4);
    assert_eq!(lab.op("correction:c1").state, OpState::Done);
    assert!(lab
        .remote_paths()
        .contains(&"docs/colleague.md".to_string()));
    assert!(lab
        .remote_paths()
        .contains(&format!("{FOLDER}/corrections.md")));
    // History is linear: both contributions are in it.
    assert_eq!(
        git(&lab.remote, &["rev-list", "--merges", "--count", "main"]),
        "0"
    );

    // A correction that failed and was cancelled can be queued again.
    let d = note("c2", "Again.", None, None);
    lab.publish_correction(&d, Trigger::Explicit).unwrap();
    let id = lab.op("correction:c2").id;
    lab.store
        .lock()
        .unwrap()
        .cancel_op(id, lab.env.now_ms())
        .unwrap();
    assert_eq!(lab.op("correction:c2").state, OpState::Cancelled);
    let again = lab.publish_correction(&d, Trigger::Explicit).unwrap();
    assert!(again.ops[0].1, "revived");
    lab.run_until_settled(4);
    assert_eq!(lab.op("correction:c2").state, OpState::Done);
}

#[test]
fn no_secret_ends_up_in_the_files_or_the_rows_of_a_correction() {
    let lab = published(LabOptions::default());
    lab.publish_correction(&note("c1", "Moved.", Some("i1"), None), Trigger::Explicit)
        .unwrap();
    lab.run_until_settled(4);
    let webhook = lab.discord.webhook_url();
    let rendered = lab
        .env
        .snapshot_root()
        .join(MEETING)
        .join("work/corrections-out/c1.md");
    assert!(
        rendered.is_file(),
        "what was published is kept next to the meeting"
    );
    let mut texts = vec![std::fs::read_to_string(&rendered).unwrap()];
    texts.push(lab.remote_show(&format!("{FOLDER}/corrections.md")));
    texts.push(git(&lab.remote, &["log", "--all", "-p", "--format=%H %s"]));
    for op in lab.ops() {
        texts.push(format!("{op:?}"));
    }
    for text in texts {
        assert!(!text.contains(TOKEN) && !text.contains(&webhook), "{text}");
    }
}
