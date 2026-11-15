//! Tests of the dispatcher over the lab: a local bare remote, a fake GitHub and a fake Discord.

use std::collections::BTreeMap;

use cycle_store::{Decision, ItemType, ProposedStatus};
use store::OpState;

use super::*;
use crate::flow;
use crate::plan::{DiscordPayload, COMMIT_KEY};
use crate::rules::{PublishTarget, Trigger};
use crate::testing::fixtures::{item, proposal, task};
use crate::testing::github::{secondary_rate_limit, Visibility};
use crate::testing::lab::{lab_draft, Lab, LabOptions, MEETING, TOKEN};
use crate::testing::{Fault, Reply};

const BOTH: [PublishTarget; 2] = [PublishTarget::Github, PublishTarget::Discord];
const GITHUB: [PublishTarget; 1] = [PublishTarget::Github];
const DISCORD: [PublishTarget; 1] = [PublishTarget::Discord];

fn anchors() -> BTreeMap<String, String> {
    BTreeMap::from([("i1".to_string(), "t-00-12-25".to_string())])
}

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

fn publish(lab: &Lab, targets: &[PublishTarget]) {
    lab.publish(&draft(), Trigger::Explicit, targets, &anchors())
        .expect("publish");
}

fn state_of(lab: &Lab, key: &str) -> OpState {
    lab.op(key).state
}

#[test]
fn a_commit_lands_in_the_remote_and_the_publication_is_recorded() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    let report = lab.tick();
    assert_eq!(
        report.done,
        3,
        "the commit and two issues: {:?}",
        lab.states()
    );

    let paths = lab.remote_paths();
    for want in [
        "meetings/NOMAD/2026-11-13/meeting.json",
        "meetings/NOMAD/2026-11-13/report.md",
        "meetings/NOMAD/2026-11-13/transcript.md",
        "meetings/NOMAD/2026-11-13/img/0001-00-12-41.png",
    ] {
        assert!(paths.contains(&want.to_string()), "{want} in {paths:?}");
    }
    let row = lab
        .store
        .lock()
        .unwrap()
        .get_publication(MEETING)
        .unwrap()
        .unwrap();
    assert_eq!(row.folder.as_deref(), Some("2026-11-13"));
    assert_eq!(row.commit_sha.as_deref().map(str::len), Some(40));
    assert!(row.published_unix_ms.is_some());
    let url = row.report_url.unwrap();
    assert!(
        url.ends_with("/owner/rover/blob/main/meetings/NOMAD/2026-11-13/report.md"),
        "{url}"
    );
    assert_eq!(lab.env.repo_changes.lock().unwrap().as_slice(), ["NOMAD"]);
    assert!(
        *lab.env.changes.lock().unwrap() >= 6,
        "announced at claim and at the end"
    );

    let result: Value =
        serde_json::from_str(lab.op(COMMIT_KEY).result.as_deref().unwrap()).unwrap();
    assert_eq!(result["folder"], "2026-11-13");
    assert_eq!(result["pushed"], true);
    // The new topic of the meeting is in topics.yaml on the remote.
    assert!(lab
        .remote_show("cycles/NOMAD/topics.yaml")
        .contains("Wheel size"));
}

#[test]
fn issues_carry_the_report_link_and_the_post_links_the_issues() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &BOTH);
    lab.tick();
    assert!(
        lab.states().iter().all(|(_, s)| *s == "done"),
        "{:?}",
        lab.states()
    );

    let issues = lab.github.issues("owner/rover");
    assert_eq!(issues.len(), 2);
    let first = &issues[0];
    assert_eq!(first.title, "Design the mount");
    assert_eq!(first.assignees, ["alex-gh"]);
    assert!(first.labels.contains(&"cycle:NOMAD".to_string()));
    assert!(first.body.contains("Due: 2026-11-20"));
    assert!(first
        .body
        .contains("blob/main/meetings/NOMAD/2026-11-13/report.md"));
    assert!(first.body.contains("transcript.md#t-00-12-25"));
    assert!(first
        .body
        .contains("bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i1"));

    let posts = lab.discord.messages();
    assert_eq!(posts.len(), 2, "the text and the screenshot");
    assert!(
        posts[0].content.contains("report.md"),
        "{}",
        posts[0].content
    );
    assert!(posts[0].content.contains("Issues:"), "{}", posts[0].content);
    assert!(
        posts[0].content.contains("/issues/1"),
        "{}",
        posts[0].content
    );
    assert_eq!(posts[1].files.len(), 1);
    // The issues were created before the message, which is why it can link them.
    let order: Vec<_> = lab
        .ops()
        .iter()
        .map(|o| (o.op_key.clone(), o.updated_unix_ms))
        .collect();
    assert!(order.iter().any(|(k, _)| k == "discord"));
}

#[test]
fn a_missing_token_fails_for_good_and_a_retry_after_adding_it_works() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The token is gone by the time the operations run. The commit goes through (the remote
    // is a folder, and the repository was confirmed as unknown); the issues need a token.
    *lab.env.token.lock().unwrap() = None;
    {
        let ctx = lab.env.context("NOMAD").unwrap();
        let store = lab.store.lock().unwrap();
        flow::confirm_public_repo(&store, lab.env.as_ref(), &ctx).unwrap();
    }
    let report = lab.tick();
    assert_eq!((report.done, report.failed), (1, 2), "{:?}", lab.states());
    let failed = lab.op("issue:i1");
    assert_eq!(failed.state, OpState::Failed);
    assert!(failed
        .last_error
        .unwrap()
        .contains("no GitHub token is stored"));

    *lab.env.token.lock().unwrap() = Some(TOKEN.to_string());
    lab.store
        .lock()
        .unwrap()
        .retry_all_ops(lab.env.now_ms())
        .unwrap();
    let report = lab.tick();
    assert_eq!(report.done, 2);
    assert_eq!(lab.github.issues("owner/rover").len(), 2);
}

#[test]
fn a_rate_limit_waits_as_long_as_the_server_says() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    lab.github.inject_for(
        "POST",
        "/repos/owner/rover/issues",
        Fault::Reply(secondary_rate_limit(120)),
        1,
    );
    let before = lab.env.now_ms();
    let report = lab.tick();
    assert_eq!((report.done, report.retrying), (2, 1), "{:?}", lab.states());
    let waiting = lab.op("issue:i1");
    assert_eq!(waiting.state, OpState::Queued);
    assert_eq!(waiting.attempts, 1);
    assert_eq!(waiting.next_attempt_unix_ms, Some(before + 120_000));
    assert_eq!(
        lab.github.issues("owner/rover").len(),
        1,
        "the second issue did not wait"
    );

    // Not before its time, then at its time.
    assert_eq!(lab.tick().ran(), 0);
    lab.env.advance(120_001);
    assert_eq!(lab.tick().done, 1);
    assert_eq!(lab.github.issues("owner/rover").len(), 2);
}

#[test]
fn the_dispatcher_checks_the_repository_again_before_it_sends() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The repository became public after the user queued the publication.
    lab.github.set_visibility("owner/rover", Visibility::Public);
    let report = lab.tick();
    assert_eq!((report.done, report.failed), (0, 1), "{:?}", lab.states());
    let commit = lab.op(COMMIT_KEY);
    assert_eq!(commit.state, OpState::Failed);
    assert!(commit
        .last_error
        .unwrap()
        .starts_with("public_repo_unconfirmed"));
    assert_eq!(lab.remote_commits(), 1, "nothing was pushed");
    assert!(lab.github.issues("owner/rover").is_empty());

    // The user confirms; the refused operations run again.
    let ctx = lab.env.context("NOMAD").unwrap();
    let (visibility, released) = {
        let store = lab.store.lock().unwrap();
        flow::confirm_public_repo(&store, lab.env.as_ref(), &ctx).unwrap()
    };
    assert_eq!(visibility, RepoVisibility::Public);
    assert_eq!(released, 1);
    assert_eq!(lab.tick().done, 3, "{:?}", lab.states());
    assert_eq!(lab.remote_commits(), 2);
}

#[test]
fn a_github_that_cannot_be_reached_is_waited_for_not_taken_for_a_public_repository() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The network goes away after the user queued the publication of a private repository.
    lab.github.go_offline();
    let report = lab.tick();
    assert_eq!(
        (report.done, report.retrying, report.failed),
        (0, 1, 0),
        "{:?}",
        lab.states()
    );
    let commit = lab.op(COMMIT_KEY);
    assert_eq!(commit.state, OpState::Queued, "a retry, not a refusal");
    assert!(commit
        .last_error
        .unwrap()
        .contains("cannot check whether the repository is public"));
    assert_eq!(lab.remote_commits(), 1);

    lab.github.go_online();
    lab.env.advance(31_000);
    assert_eq!(lab.tick().done, 3, "{:?}", lab.states());
}

#[test]
fn operations_left_running_by_a_crash_run_again() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // A process that died while it ran the commit.
    let claimed = lab
        .store
        .lock()
        .unwrap()
        .claim_next_op(lab.env.now_ms(), &[])
        .unwrap()
        .unwrap();
    assert_eq!(claimed.op_key, COMMIT_KEY);
    assert_eq!(state_of(&lab, COMMIT_KEY), OpState::Running);
    assert_eq!(
        lab.tick().ran(),
        0,
        "a running operation is nobody's to take"
    );

    let reset = lab.dispatcher.recover().unwrap();
    assert_eq!(reset, [claimed.id]);
    assert_eq!(
        lab.op(COMMIT_KEY).attempts,
        0,
        "a crash is not a failed attempt"
    );
    assert_eq!(lab.tick().done, 3);
}

#[test]
fn a_dead_network_pauses_the_target_and_is_reported() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    lab.github.go_offline();
    // The visibility cannot be asked and the repository is not confirmed: confirm it as the
    // user would while offline.
    {
        let ctx = lab.env.context("NOMAD").unwrap();
        let store = lab.store.lock().unwrap();
        flow::confirm_public_repo(&store, lab.env.as_ref(), &ctx).unwrap();
    }
    let report = lab.tick();
    assert_eq!(report.done, 1, "the commit goes through the folder remote");
    assert_eq!(report.retrying, 1, "the first issue found nobody home");
    assert_eq!(lab.dispatcher.offline_targets(), [OutboxTarget::Issues]);
    assert_eq!(
        lab.op("issue:i2").state,
        OpState::Queued,
        "the other issue was not even tried in this tick"
    );
    assert_eq!(lab.op("issue:i2").attempts, 0);

    lab.github.go_online();
    lab.env.advance(31_000);
    assert_eq!(lab.tick().done, 2);
    assert!(lab.dispatcher.offline_targets().is_empty());
}

#[test]
fn errors_kept_in_the_outbox_never_hold_a_secret() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &BOTH);
    lab.discord.delete_webhook();
    *lab.env.token.lock().unwrap() = Some("ghp_WrongTokenValue0123456789xx".to_string());
    lab.tick();
    let secrets = [
        TOKEN.to_string(),
        "ghp_WrongTokenValue0123456789xx".to_string(),
        lab.discord.webhook_token(),
        lab.discord.webhook_url(),
    ];
    let mut seen_error = false;
    for op in lab.ops() {
        for text in [
            op.last_error.clone(),
            op.result.clone(),
            Some(op.payload.clone()),
        ]
        .into_iter()
        .flatten()
        {
            seen_error |= op.last_error.is_some();
            for secret in &secrets {
                assert!(
                    !text.contains(secret),
                    "{} leaks a secret: {text}",
                    op.op_key
                );
            }
        }
    }
    assert!(
        seen_error,
        "the test must produce errors: {:?}",
        lab.states()
    );
}

#[test]
fn a_post_resumes_after_the_messages_that_are_out() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &DISCORD);
    let discord = lab.op("discord");
    let payload: DiscordPayload = serde_json::from_str(&discord.payload).unwrap();
    assert_eq!(payload.files.len(), 1);

    // One message went out before the process stopped: the text is posted already.
    {
        let store = lab.store.lock().unwrap();
        let claimed = store.claim_next_op(lab.env.now_ms(), &[]).unwrap().unwrap();
        store
            .set_op_progress(claimed.id, r#"{"sent_chunks":1}"#, lab.env.now_ms())
            .unwrap();
        store
            .fail_op(
                claimed.id,
                Failure::Transient {
                    retry_after_ms: Some(1000),
                },
                "connection lost",
                lab.env.now_ms(),
            )
            .unwrap();
    }
    lab.env.advance(2000);
    assert_eq!(lab.tick().done, 1);
    let posts = lab.discord.messages();
    assert_eq!(posts.len(), 1, "only the rest was sent");
    assert_eq!(posts[0].files.len(), 1);
    let result: Value = serde_json::from_str(lab.op("discord").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["sent_chunks"], 2);
}

#[test]
fn a_failed_post_is_sent_once_when_it_goes_through() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &DISCORD);
    lab.discord
        .inject_for("POST", "/api/webhooks", Fault::Reply(Reply::empty(500)), 1);
    assert_eq!(lab.tick().retrying, 1);
    assert!(lab.discord.messages().is_empty());
    lab.env.advance(31_000);
    assert_eq!(lab.tick().done, 1);
    assert_eq!(lab.discord.messages().len(), 2);
}

#[test]
fn a_lost_snapshot_is_a_permanent_failure_with_advice() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    let row = lab
        .store
        .lock()
        .unwrap()
        .get_publication(MEETING)
        .unwrap()
        .unwrap();
    std::fs::remove_file(
        lab.env
            .snapshot_root()
            .join(&row.snapshot_dir)
            .join("report.md"),
    )
    .unwrap();
    lab.tick();
    let commit = lab.op(COMMIT_KEY);
    assert_eq!(commit.state, OpState::Failed);
    assert!(commit.last_error.unwrap().contains("lost report.md"));
}

#[test]
fn approved_status_changes_close_issues_and_comment() {
    let lab = Lab::new(LabOptions::default());
    let number = lab
        .github
        .add_issue("owner/rover", "Old task", &["cycle:NOMAD"]);
    let mut d = draft();
    let mut approved = proposal(
        "s1",
        &format!("#{number}"),
        ProposedStatus::Done,
        Decision::Approved,
    );
    approved.comment = Some("Done on Friday".into());
    d.status_proposals = vec![
        approved,
        proposal("s2", "#99", ProposedStatus::Done, Decision::Proposed),
    ];
    lab.publish(&d, Trigger::Explicit, &GITHUB, &anchors())
        .unwrap();
    assert!(lab.ops().iter().any(|o| o.op_key == "status:s1"));
    assert!(!lab.ops().iter().any(|o| o.op_key == "status:s2"));
    lab.tick();
    let issue = lab.github.issue("owner/rover", number).unwrap();
    assert!(!issue.open);
    assert_eq!(issue.state_reason.as_deref(), Some("completed"));
    assert!(issue.comments[0].body.contains("Done on Friday"));
    assert!(issue.comments[0]
        .body
        .contains("bbb:v1 status meeting=20261113-140501 proposal=s1"));
}

#[test]
fn a_local_only_repository_commits_without_pushing_or_issues() {
    let lab = Lab::new(LabOptions {
        remote: false,
        linked: false,
        discord: None,
        ..LabOptions::default()
    });
    publish(&lab, &GITHUB);
    let report = lab.tick();
    assert_eq!(report.done, 1, "{:?}", lab.states());
    let result: Value =
        serde_json::from_str(lab.op(COMMIT_KEY).result.as_deref().unwrap()).unwrap();
    assert_eq!(result["pushed"], false);
    assert!(lab
        .clone_dir
        .join("meetings/NOMAD/2026-11-13/report.md")
        .is_file());
    assert_eq!(
        lab.github.request_count(),
        0,
        "GitHub was not asked anything"
    );
}
