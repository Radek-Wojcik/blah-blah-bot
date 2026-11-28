//! What publishing does when things go wrong: no network, a remote that changes under the
//! publisher, answers that get lost, a rate limit, a crash, and the rule that no secret ends
//! up in a file, a log or the database. Everything runs against local bare repositories and
//! fake servers.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cycle_store::{Decision, ItemType, ProposedStatus};
use publishers::github::{Github, IssueSpec, RepoRef};
use publishers::outbox::Env;
use publishers::rules::{PublishTarget, Trigger};
use publishers::testing::fixtures::{item, proposal, task};
use publishers::testing::github::secondary_rate_limit;
use publishers::testing::lab::{
    colleague_push, git, lab_draft, Lab, LabOptions, MEETING, REPO, TOKEN,
};
use publishers::testing::{discord, Fault, Reply};
use serde_json::Value;
use store::OpState;

const BOTH: [PublishTarget; 2] = [PublishTarget::Github, PublishTarget::Discord];
const GITHUB: [PublishTarget; 1] = [PublishTarget::Github];

fn anchors() -> BTreeMap<String, String> {
    BTreeMap::new()
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

fn state(lab: &Lab, key: &str) -> OpState {
    lab.op(key).state
}

fn meeting_paths(lab: &Lab) -> usize {
    lab.remote_paths()
        .iter()
        .filter(|p| p.starts_with("meetings/NOMAD/"))
        .count()
}

#[test]
fn publishing_without_a_network_waits_in_the_outbox_and_recovers() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &BOTH);

    // Everything goes away: the Git remote, GitHub and Discord.
    lab.remote_offline();
    lab.github.go_offline();
    lab.discord.go_offline();

    let report = lab.tick();
    assert_eq!(
        (report.done, report.retrying, report.failed),
        (0, 1, 0),
        "{:?}",
        lab.states()
    );
    let commit = lab.op("commit");
    assert_eq!(commit.state, OpState::Queued);
    assert_eq!(commit.attempts, 1);
    assert!(commit.last_error.is_some());
    assert!(commit.next_attempt_unix_ms.unwrap() > lab.env.now_ms());
    assert_eq!(lab.dispatcher.offline_targets().len(), 1);
    for key in ["issue:i1", "issue:i2", "discord"] {
        assert_eq!(
            state(&lab, key),
            OpState::Queued,
            "{key} waits for the commit"
        );
    }

    // Nothing runs before the retry time, and the draft is not locked while ops are only queued.
    assert_eq!(lab.tick().ran(), 0);
    assert!(!publishers::flow::is_locked(&lab.store.lock().unwrap(), MEETING).unwrap());

    // A second failed attempt backs off further.
    lab.env.advance(31_000);
    let report = lab.tick();
    assert_eq!(report.retrying, 1);
    let commit = lab.op("commit");
    assert_eq!(commit.attempts, 2);
    let wait = commit.next_attempt_unix_ms.unwrap() - lab.env.now_ms();
    assert!(
        (59_000..=61_000).contains(&wait),
        "the backoff doubled: {wait}"
    );

    // The network comes back: everything is published, once.
    lab.remote_online();
    lab.github.go_online();
    lab.discord.go_online();
    lab.store
        .lock()
        .unwrap()
        .retry_all_ops(lab.env.now_ms())
        .unwrap();
    lab.dispatcher.clear_offline();
    lab.run_until_settled(6);
    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");
    assert_eq!(
        meeting_paths(&lab),
        4,
        "the meeting folder once: {:?}",
        lab.remote_paths()
    );
    assert_eq!(lab.github.issues(REPO).len(), 2);
    assert_eq!(lab.discord.messages().len(), 2);
    assert!(lab.dispatcher.offline_targets().is_empty());
    // Everything that was sent is done, so the draft is read-only now.
    assert!(publishers::flow::is_locked(&lab.store.lock().unwrap(), MEETING).unwrap());
}

#[test]
fn a_dead_discord_does_not_hold_back_the_report_or_the_issues() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &BOTH);
    lab.discord.go_offline();
    lab.tick();
    for key in ["commit", "issue:i1", "issue:i2"] {
        assert_eq!(state(&lab, key), OpState::Done, "{key}");
    }
    assert_eq!(state(&lab, "discord"), OpState::Queued);
    assert_eq!(lab.op("discord").attempts, 1);
    assert_eq!(
        lab.dispatcher.offline_targets(),
        [store::OutboxTarget::Discord]
    );

    lab.discord.go_online();
    lab.env.advance(31_000);
    lab.tick();
    assert_eq!(state(&lab, "discord"), OpState::Done);
    assert_eq!(lab.discord.messages().len(), 2);
}

#[test]
fn a_push_rejected_by_a_concurrent_commit_is_built_again_on_the_new_tip() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // A colleague pushes between the moment the commit is built and the push, once.
    let (remote, scratch) = (lab.remote.clone(), lab.tmp.path().join("colleagues"));
    lab.env
        .push_hook
        .lock()
        .unwrap()
        .replace(Box::new(move |attempt| {
            if attempt == 1 {
                colleague_push(&remote, &scratch, "one", &[("notes/one.md", "one\n")]);
            }
        }));
    let report = lab.tick();
    assert_eq!(report.done, 3, "{:?}", lab.states());

    let result: Value = serde_json::from_str(lab.op("commit").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["attempts"], 2, "rebuilt once");
    let paths = lab.remote_paths();
    assert!(
        paths.contains(&"notes/one.md".to_string()),
        "the colleague's commit stays"
    );
    assert!(paths.contains(&"meetings/NOMAD/2026-11-13/report.md".to_string()));
    // seed, colleague, meeting: a straight line, nothing lost.
    assert_eq!(lab.remote_commits(), 3);
    assert_eq!(
        git(&lab.remote, &["rev-list", "--merges", "--count", "main"]),
        "0"
    );
}

#[test]
fn a_remote_that_keeps_changing_is_retried_later_and_succeeds() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    let (remote, scratch) = (lab.remote.clone(), lab.tmp.path().join("colleagues"));
    let counter = Arc::new(Mutex::new(0));
    lab.env
        .push_hook
        .lock()
        .unwrap()
        .replace(Box::new(move |attempt| {
            // Every attempt of the first run loses the race.
            let mut n = counter.lock().unwrap();
            if *n < 3 {
                *n += 1;
                colleague_push(
                    &remote,
                    &scratch,
                    &format!("n{attempt}-{}", *n),
                    &[(&format!("notes/{n}.md"), "x\n")],
                );
            }
        }));
    let report = lab.tick();
    assert_eq!((report.done, report.retrying), (0, 1), "{:?}", lab.states());
    let commit = lab.op("commit");
    assert_eq!(commit.state, OpState::Queued);
    assert!(commit.last_error.unwrap().contains("kept changing"));
    assert_eq!(
        lab.remote_commits(),
        4,
        "three colleague commits and the seed"
    );

    lab.env.advance(31_000);
    let report = lab.tick();
    assert_eq!(report.done, 3, "{:?}", lab.states());
    assert_eq!(lab.remote_commits(), 5);
    assert_eq!(meeting_paths(&lab), 4);
}

#[test]
fn an_ambiguous_failure_does_not_duplicate_the_issue() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // GitHub creates the first issue and the answer is lost.
    lab.github
        .inject_for("POST", "/repos/owner/rover/issues", Fault::DropAfter, 1);
    let report = lab.tick();
    assert_eq!(report.retrying, 1, "{:?}", lab.states());
    assert_eq!(lab.op("issue:i1").state, OpState::Queued);
    assert_eq!(
        lab.github.issues(REPO).len(),
        1,
        "created, though nobody heard"
    );

    lab.env.advance(31_000);
    lab.run_until_settled(4);
    let issues = lab.github.issues(REPO);
    assert_eq!(
        issues.len(),
        2,
        "no duplicate: {:?}",
        issues.iter().map(|i| &i.title).collect::<Vec<_>>()
    );
    let result: Value =
        serde_json::from_str(lab.op("issue:i1").result.as_deref().unwrap()).unwrap();
    assert_eq!(
        result["created"], false,
        "the issue was found by its marker"
    );
    assert_eq!(result["number"], issues[0].number);
}

#[test]
fn a_crash_after_the_issue_was_created_does_not_duplicate_it_either() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The commit goes through; then the process dies while it creates the first issue,
    // after GitHub already made it.
    lab.store
        .lock()
        .unwrap()
        .claim_next_op(lab.env.now_ms(), &[])
        .unwrap();
    lab.dispatcher.recover().unwrap();
    let api = Github::with_client(lab.env.client(), &lab.github.api_base(), TOKEN).unwrap();
    let spec = IssueSpec {
        cycle_id: "NOMAD".into(),
        meeting_id: MEETING.into(),
        item_id: "i1".into(),
        title: "Design the mount".into(),
        assignee: Some("alex-gh".into()),
        due: None,
        meeting_date: "2026-11-13".into(),
        report_url: None,
        transcript_url: None,
        transcript_anchor: None,
        time_ms: None,
        quote: None,
    };
    api.publish_issue(&RepoRef::parse(REPO).unwrap(), &spec, false)
        .unwrap();
    assert_eq!(lab.github.issues(REPO).len(), 1);
    // The crash: the operation of that issue was running.
    lab.tick();
    assert_eq!(
        lab.github.issues(REPO).len(),
        2,
        "one for i1 (found), one for i2"
    );
    assert!(
        lab.ops().iter().all(|o| o.state == OpState::Done),
        "{:?}",
        lab.states()
    );
}

#[test]
fn a_restart_right_after_the_issue_was_created_reuses_it_while_the_lists_lag() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The first operation (the commit) is done; the one of the first issue is running.
    {
        let store = lab.store.lock().unwrap();
        let commit = store.claim_next_op(lab.env.now_ms(), &[]).unwrap().unwrap();
        store
            .complete_op(commit.id, "{}", lab.env.now_ms())
            .unwrap();
    }
    let issue = lab
        .store
        .lock()
        .unwrap()
        .claim_next_op(lab.env.now_ms(), &[])
        .unwrap()
        .unwrap();
    assert_eq!(issue.op_key, "issue:i1");
    // GitHub made the issue, the number was recorded, then the process died. After the
    // restart the lists do not show the issue yet (a scan alone would create a second).
    lab.github.lag_lists(REPO, 1000);
    let api = Github::with_client(lab.env.client(), &lab.github.api_base(), TOKEN).unwrap();
    let spec = IssueSpec {
        cycle_id: "NOMAD".into(),
        meeting_id: MEETING.into(),
        item_id: "i1".into(),
        title: "Design the mount".into(),
        assignee: None,
        due: None,
        meeting_date: "2026-11-13".into(),
        report_url: None,
        transcript_url: None,
        transcript_anchor: None,
        time_ms: None,
        quote: None,
    };
    let made = api
        .publish_issue(&RepoRef::parse(REPO).unwrap(), &spec, false)
        .unwrap();
    lab.store
        .lock()
        .unwrap()
        .set_op_progress(
            issue.id,
            &format!(r#"{{"issue_number":{}}}"#, made.number),
            lab.env.now_ms(),
        )
        .unwrap();
    lab.dispatcher.recover().unwrap();
    lab.run_until_settled(4);

    let issues = lab.github.issues(REPO);
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.title == "Design the mount")
            .count(),
        1,
        "no duplicate: {:?}",
        issues.iter().map(|i| &i.title).collect::<Vec<_>>()
    );
    let result: Value =
        serde_json::from_str(lab.op("issue:i1").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["created"], false);
    assert_eq!(result["number"], made.number);
}

#[test]
fn a_status_change_is_written_once_even_when_answers_are_lost() {
    let lab = Lab::new(LabOptions::default());
    let number = lab
        .github
        .add_issue(REPO, "Mount the camera", &["cycle:NOMAD"]);
    let mut d = draft();
    d.status_proposals = vec![proposal(
        "s1",
        &format!("#{number}"),
        ProposedStatus::Done,
        Decision::Approved,
    )];
    lab.publish(&d, Trigger::Explicit, &GITHUB, &anchors())
        .unwrap();
    // The comment is stored, the answer is lost; the state was changed before.
    lab.github.inject_for(
        "POST",
        &format!("/repos/owner/rover/issues/{number}/comments"),
        Fault::DropAfter,
        1,
    );
    lab.tick();
    assert_eq!(lab.op("status:s1").state, OpState::Queued);
    lab.env.advance(31_000);
    lab.run_until_settled(4);
    assert_eq!(lab.op("status:s1").state, OpState::Done);
    let issue = lab.github.issue(REPO, number).unwrap();
    assert!(!issue.open);
    assert_eq!(issue.comments.len(), 1, "one comment, not two");
}

#[test]
fn a_rate_limited_post_waits_for_the_time_discord_names_and_is_posted_once() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &[PublishTarget::Discord]);
    lab.discord.inject_for(
        "POST",
        "/api/webhooks",
        Fault::Reply(discord::rate_limited(40.0)),
        1,
    );
    let before = lab.env.now_ms();
    let report = lab.tick();
    assert_eq!(report.retrying, 1, "{:?}", lab.states());
    assert_eq!(
        lab.op("discord").next_attempt_unix_ms,
        Some(before + 40_000)
    );
    assert!(lab.discord.messages().is_empty());
    lab.env.advance(40_001);
    assert_eq!(lab.tick().done, 1);
    assert_eq!(lab.discord.messages().len(), 2);
}

#[test]
fn a_github_rate_limit_pauses_only_that_operation_and_a_permanent_error_stops_at_once() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    lab.github.inject_for(
        "POST",
        "/repos/owner/rover/issues",
        Fault::Reply(secondary_rate_limit(90)),
        1,
    );
    lab.github.inject_for(
        "POST",
        "/repos/owner/rover/issues",
        Fault::Reply(Reply::json(
            422,
            &serde_json::json!({"message": "Validation Failed"}),
        )),
        // Twice: the first refusal makes the issue be tried again without its assignee.
        2,
    );
    let report = lab.tick();
    assert_eq!(
        (report.done, report.retrying, report.failed),
        (1, 1, 1),
        "{:?}",
        lab.states()
    );
    assert_eq!(
        lab.op("issue:i1").next_attempt_unix_ms,
        Some(lab.env.now_ms() + 90_000)
    );
    let failed = lab.op("issue:i2");
    assert_eq!(failed.state, OpState::Failed);
    assert!(failed.last_error.unwrap().contains("Validation Failed"));
    // The user retries the failed one; it goes through.
    lab.store
        .lock()
        .unwrap()
        .retry_op(failed.id, lab.env.now_ms())
        .unwrap();
    lab.tick();
    assert_eq!(lab.op("issue:i2").state, OpState::Done);
}

#[test]
fn a_crash_while_running_resumes_without_a_second_commit() {
    let lab = Lab::new(LabOptions::default());
    publish(&lab, &GITHUB);
    // The commit was pushed, but the process died before it wrote "done".
    lab.store
        .lock()
        .unwrap()
        .claim_next_op(lab.env.now_ms(), &[])
        .unwrap()
        .unwrap();
    {
        let ctx = lab.env.context("NOMAD").unwrap();
        let row = lab
            .store
            .lock()
            .unwrap()
            .get_publication(MEETING)
            .unwrap()
            .unwrap();
        let snapshot = lab.env.snapshot_root().join(&row.snapshot_dir);
        let files: Vec<_> = publishers::flow::list_files(&snapshot)
            .unwrap()
            .into_iter()
            .map(|rel| cycle_store::PublishFile {
                path: snapshot.join(&rel),
                rel,
            })
            .collect();
        cycle_store::publish_meeting(
            &ctx.repo,
            &cycle_store::PublishRequest {
                cycle_id: "NOMAD",
                base_path: "meetings",
                date: "2026-11-13",
                files: &files,
                topics: None,
                message: "Add meeting {folder}",
                auth: None,
            },
        )
        .unwrap();
    }
    let commits = lab.remote_commits();
    lab.dispatcher.recover().unwrap();
    lab.run_until_settled(4);
    assert!(
        lab.ops().iter().all(|o| o.state == OpState::Done),
        "{:?}",
        lab.states()
    );
    assert_eq!(lab.remote_commits(), commits, "nothing was committed twice");
    let result: Value = serde_json::from_str(lab.op("commit").result.as_deref().unwrap()).unwrap();
    assert_eq!(result["already_published"], true);
    assert_eq!(result["folder"], "2026-11-13");
}

// ----- secrets -------------------------------------------------------------------------------

/// A writer that keeps everything the tracing output says.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The log of this test binary. The subscriber is installed once for the whole process: one
/// that is scoped to a thread loses events when other tests, running at the same time, have
/// already told a call site that nobody listens. What other tests log is in it too, and it
/// must not hold a secret either.
fn captured_log() -> Captured {
    static LOG: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();
    LOG.get_or_init(|| {
        let log = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("no other subscriber");
        log
    })
    .clone()
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        // The bare remote and `.git` hold compressed objects: they are read through git.
        if path
            .file_name()
            .is_some_and(|n| n == ".git" || n == "remote.git" || n == "seed")
        {
            continue;
        }
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn no_secret_ends_up_in_a_file_a_log_or_the_database() {
    let lab = Lab::new(LabOptions::default());
    let webhook = lab.discord.webhook_url();
    let webhook_token = lab.discord.webhook_token();
    let wrong = "ghp_WrongTokenValue0123456789xx";
    let secrets = [
        TOKEN.to_string(),
        wrong.to_string(),
        webhook.clone(),
        webhook_token.clone(),
    ];

    let log = captured_log();
    {
        publish(&lab, &BOTH);
        // Failures of every kind that can carry a secret in their text.
        *lab.env.token.lock().unwrap() = Some(wrong.to_string());
        lab.discord.delete_webhook();
        lab.github.inject_for(
            "POST",
            "/repos/owner/rover/issues",
            Fault::Reply(secondary_rate_limit(5)),
            1,
        );
        lab.tick();
        lab.env.advance(31_000);
        lab.github.go_offline();
        lab.tick();
        lab.github.go_online();
        *lab.env.token.lock().unwrap() = Some(TOKEN.to_string());
        lab.store
            .lock()
            .unwrap()
            .retry_all_ops(lab.env.now_ms())
            .unwrap();
        lab.run_until_settled(6);
    }

    let errors: Vec<String> = lab.ops().into_iter().filter_map(|o| o.last_error).collect();
    assert!(
        !errors.is_empty(),
        "the test must produce errors: {:?}",
        lab.states()
    );

    // The log of the dispatcher.
    let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("publishing operation failed"), "{log}");
    for secret in &secrets {
        assert!(
            !log.contains(secret.as_str()),
            "the log holds a secret: {log}"
        );
    }
    // The database file (and its journal), the snapshots, the working tree of the clone.
    let mut files = Vec::new();
    files_under(lab.tmp.path(), &mut files);
    for entry in std::fs::read_dir(&lab.clone_dir).unwrap().flatten() {
        if entry.file_name() != ".git" {
            let p = entry.path();
            if p.is_dir() {
                files_under(&p, &mut files);
            } else {
                files.push(p);
            }
        }
    }
    assert!(files.iter().any(|f| f == &lab.db_path));
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        for secret in &secrets {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{} holds a secret",
                file.display()
            );
        }
    }
    // Everything Git has: the history of the remote and of the clone, with the files.
    for dir in [&lab.remote, &lab.clone_dir] {
        let history = git(dir, &["log", "--all", "-p", "--format=%H %an %ae %s"]);
        let config = std::fs::read_to_string(dir.join("config")).unwrap_or_default();
        for secret in &secrets {
            assert!(
                !history.contains(secret.as_str()),
                "{} history",
                dir.display()
            );
            assert!(
                !config.contains(secret.as_str()),
                "{} config",
                dir.display()
            );
        }
    }
    // The outbox rows, in full.
    for op in lab.ops() {
        let all = format!("{op:?}");
        for secret in &secrets {
            assert!(!all.contains(secret.as_str()), "{all}");
        }
    }
}
