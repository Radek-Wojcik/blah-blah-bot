//! Release checks of publishing against real services: a GitHub test repository and a Discord
//! test webhook. They are written for the release machine and are **never part of a normal
//! test run**: every test is `#[ignore]`d, and each one refuses to run unless
//! `BBB_RELEASE_CHECK=yes` is set. See `docs/TESTING.md` ("Stage 5 test commands") for the
//! variables and the command.
//!
//! Where the secrets come from: the GitHub token and the Discord webhook are the ones the
//! application itself stores in the Windows Credential Manager (entered in the publishing
//! settings), under the entry names the application uses. No test reads a token from the
//! environment, from a file or from another program's configuration.
//!
//! What the checks leave behind in the test repository: a branch `release-check-<time>` (the
//! test removes it again), and a closed issue with the label `cycle:RC<time>` (an issue
//! cannot be deleted through the API).

mod full_loop;
mod integrations;

use std::path::Path;
use std::process::Command;

use cycle_store::publish::{GitAuth, PublishFile, PublishRequest};
use cycle_store::time::now_unix_ms;
use cycle_store::Repo;
use publishers::addr::web_base;
use publishers::discord::{build_plan, send_messages, Pacing};
use publishers::github::{
    api_host, normalize_api_base, Github, IssueSpec, RepoInfo, RepoRef, StatusSpec, DEFAULT_API,
};
use publishers::http::{HttpClient, Method, Request};
use publishers::webhook::Webhook;

use crate::secrets::{self, Credentials, SecretStore};

/// The explicit switch: without it nothing here may run.
fn require_release_check() {
    assert_eq!(
        std::env::var("BBB_RELEASE_CHECK").as_deref(),
        Ok("yes"),
        "these checks talk to real services: set BBB_RELEASE_CHECK=yes to run them on purpose"
    );
}

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}, see docs/TESTING.md"))
}

struct Target {
    repo: RepoRef,
    api: String,
    token: String,
}

fn target() -> Target {
    require_release_check();
    secrets::init();
    let repo = RepoRef::parse(&var("BBB_RC_REPO")).expect("BBB_RC_REPO is owner/name");
    let api = match std::env::var("BBB_RC_GITHUB_API") {
        Ok(api) if !api.trim().is_empty() => normalize_api_base(&api).expect("BBB_RC_GITHUB_API"),
        _ => DEFAULT_API.to_string(),
    };
    let entry = secrets::github_token_entry(&api).expect("the API address");
    let token = Credentials.get(&entry).unwrap_or_else(|| {
        panic!(
            "no GitHub token stored for {}: add it in the publishing settings of the application first",
            api_host(&api).unwrap_or_default()
        )
    });
    Target { repo, api, token }
}

/// A repository that is public needs `BBB_RC_ALLOW_PUBLIC=yes`: the checks write to it.
fn refuse_public(info: &RepoInfo) {
    if info.is_public() {
        assert_eq!(
            std::env::var("BBB_RC_ALLOW_PUBLIC").as_deref(),
            Ok("yes"),
            "{} is public: use a private test repository, or set BBB_RC_ALLOW_PUBLIC=yes",
            info.full_name
        );
    }
}

fn github(t: &Target) -> Github {
    Github::new(&t.api, &t.token).expect("a GitHub client")
}

fn checked_target() -> (Target, Github) {
    let t = target();
    let gh = github(&t);
    let report = gh
        .check_access(&t.repo)
        .expect("the token works for the repository");
    assert_eq!(
        report.repo.can_push,
        Some(true),
        "the token needs write access"
    );
    assert!(
        report.repo.has_issues,
        "Issues are off in the test repository"
    );
    refuse_public(&report.repo);
    (t, gh)
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn github_token_and_repository_are_usable() {
    let (t, gh) = checked_target();
    let info = gh.repo_info(&t.repo).unwrap();
    println!("{}: {} ({})", info.full_name, info.visibility, t.api);
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn an_issue_is_created_once_closed_with_a_comment_and_read_back() {
    let (t, gh) = checked_target();
    let cycle = format!("RC{}", now_unix_ms());
    let spec = IssueSpec {
        cycle_id: cycle.clone(),
        meeting_id: "20260101-000000".into(),
        item_id: "i1".into(),
        title: "Release check of Blah Blah Bot (safe to ignore)".into(),
        assignee: None,
        due: Some("2026-12-31".into()),
        meeting_date: "2026-01-01".into(),
        report_url: None,
        transcript_url: None,
        transcript_anchor: None,
        time_ms: Some(745_000),
        quote: Some("A line from a meeting".into()),
    };
    let first = gh.publish_issue(&t.repo, &spec, false).expect("create");
    assert!(first.created);
    // GitHub's lists trail a write by a few seconds. A retry that knows the number fetches
    // the issue directly and does not depend on a list at all.
    let direct = gh
        .publish_issue_tracked(&t.repo, &spec, true, Some(first.number), &mut |_| {
            panic!("a second issue was created")
        })
        .expect("known number");
    assert!(!direct.created);
    assert_eq!(direct.number, first.number);
    // A retry without the number finds it by its marker once the list shows it.
    let started = std::time::Instant::now();
    let mut found = None;
    for _ in 0..30 {
        found = gh
            .find_issue_by_marker(
                &t.repo,
                &cycle_store::issues::ItemMarker {
                    cycle: spec.cycle_id.clone(),
                    meeting: spec.meeting_id.clone(),
                    item: spec.item_id.clone(),
                    t_ms: spec.time_ms,
                },
            )
            .expect("scan");
        if found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    println!(
        "the list showed the new issue after {:?}",
        started.elapsed()
    );
    assert_eq!(
        found.map(|r| r.number),
        Some(first.number),
        "listed within 30 s"
    );
    let again = gh.publish_issue(&t.repo, &spec, true).expect("scan");
    assert!(!again.created);
    assert_eq!(again.number, first.number);

    let status = StatusSpec {
        meeting_id: spec.meeting_id.clone(),
        proposal_id: "s1".into(),
        open: false,
        reason: "Release check finished.".into(),
        quote: None,
        meeting_date: spec.meeting_date.clone(),
        report_url: None,
        transcript_url: None,
        transcript_anchor: None,
        time_ms: None,
    };
    let done = gh
        .set_issue_status(&t.repo, first.number, &status)
        .expect("close");
    assert!(done.changed && done.commented);
    let twice = gh
        .set_issue_status(&t.repo, first.number, &status)
        .expect("close again");
    assert!(
        !twice.changed && !twice.commented,
        "writing back twice changes nothing"
    );

    // The label filtered list trails a write by a few seconds on github.com.
    let mut found = None;
    for _ in 0..30 {
        let listed = gh.list_cycle_issues(&t.repo, &cycle, 1).expect("list");
        found = listed.into_iter().find(|r| r.number == first.number);
        if found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let found = found.expect("the issue is listed with its label within 30 s");
    assert!(!found.open);
    assert!(found.marker.is_some(), "the marker survived");
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn a_meeting_is_committed_and_pushed_to_a_new_branch_with_the_token_from_the_environment_of_git() {
    let (t, _) = checked_target();
    let branch = format!("release-check-{}", now_unix_ms());
    let base = web_base(&t.api);
    let dir = test_tmp::TestDir::new("release-check");
    let work = dir.path().join("clone");
    let repo = Repo::init_local(&work).unwrap();
    run_git(
        &work,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );
    run_git(
        &work,
        &["remote", "add", "origin", &format!("{base}/{}.git", t.repo)],
    );

    let snap = dir.path().join("snapshot");
    std::fs::create_dir_all(&snap).unwrap();
    std::fs::write(snap.join("report.md"), "# Release check\n").unwrap();
    std::fs::write(snap.join("meeting.json"), "{}\n").unwrap();
    let files = [
        PublishFile {
            rel: "report.md".into(),
            path: snap.join("report.md"),
        },
        PublishFile {
            rel: "meeting.json".into(),
            path: snap.join("meeting.json"),
        },
    ];
    let auth = GitAuth {
        base_url: format!("{base}/"),
        token: t.token.clone(),
    };
    let published = cycle_store::publish::publish_meeting(
        &repo,
        &PublishRequest {
            cycle_id: "RCHECK",
            base_path: "meetings",
            date: "2026-01-01",
            files: &files,
            topics: None,
            message: "Release check {folder}",
            auth: Some(&auth),
        },
    )
    .expect("the commit is built and pushed");
    assert!(published.pushed, "{published:?}");
    assert!(
        !format!("{published:?}").contains(&t.token),
        "the token is not in what the publisher reports"
    );

    // Remove the branch again.
    let delete = Request::new(
        Method::Delete,
        format!(
            "{}/repos/{}/git/refs/heads/{branch}",
            t.api,
            t.repo.full_name()
        ),
    )
    .header("Accept", "application/vnd.github+json")
    .bearer(&t.token);
    let client = HttpClient::new().with_secret(&t.token);
    client.send(&delete).expect("the test branch is removed");
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn a_message_is_posted_to_the_test_webhook() {
    require_release_check();
    secrets::init();
    let reference = var("BBB_RC_DISCORD_REF");
    let value = secrets::resolve_cred(&Credentials, &reference).unwrap_or_else(|| {
        panic!("no webhook stored for {reference}: add it in the publishing settings first")
    });
    let hook = Webhook::parse(&value).expect("the stored webhook is valid");
    let client = HttpClient::new()
        .with_secret(&hook.url())
        .with_secret(hook.token());
    hook.check(&client).expect("the webhook exists");

    let plan = build_plan(
        "Release check of Blah Blah Bot: this message can be ignored.",
        Vec::new(),
        Vec::new(),
        &[],
    );
    let report = send_messages(
        &client,
        &hook,
        &plan.messages,
        0,
        &Pacing::default(),
        &mut |_, _| {},
    )
    .expect("the message is accepted");
    assert_eq!(report.sent, 1);
    assert!(
        report
            .message_ids
            .iter()
            .all(|id| !id.contains(hook.token())),
        "nothing of the webhook comes back in the report"
    );
}
