use std::time::Duration;

use cycle_store::PublishMode;

use super::*;
use crate::export::{upsert, UpsertOutcome};
use crate::http::HttpClient;
use crate::testing::fixtures::{cycle, draft, task};
use crate::testing::{FakeGithub, Visibility};

const REPO: &str = "owner/rover";
const TOKEN: &str = "ghp_MockTokenValue0123456789";
const MEETING: &str = "20261113-140501";

fn repo() -> RepoRef {
    RepoRef::parse(REPO).unwrap()
}

fn fake() -> FakeGithub {
    let gh = FakeGithub::start();
    gh.add_repo(REPO, Visibility::Private);
    gh.require_token(TOKEN);
    gh
}

fn api(gh: &FakeGithub) -> Github {
    let client = HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5));
    Github::with_client(client, &gh.api_base(), TOKEN).unwrap()
}

/// The record of the item `id`, with the addresses the dispatcher would fill in.
fn record(id: &str) -> TaskRecord {
    let c = cycle(Some(PublishMode::OnApproval), None);
    let it = task(id, "Order the battery", Some("alex"), Some("2026-11-20"));
    let d = draft(MEETING, vec![it.clone()], vec![]);
    let mut r = TaskRecord::from_item(&c, &d, &it, MEETING);
    r.report_url = Some(
        "https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/report.md".into(),
    );
    r.transcript_url = Some(
        "https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/transcript.md".into(),
    );
    r.transcript_anchor = Some("t-00-12-25".into());
    r
}

/// The spec the dispatcher builds for the same item from its `IssuePayload`.
fn dispatcher_spec(id: &str) -> IssueSpec {
    IssueSpec {
        cycle_id: "NOMAD".into(),
        meeting_id: MEETING.into(),
        item_id: id.into(),
        title: "Order the battery".into(),
        assignee: Some("alex-gh".into()),
        due: Some("2026-11-20".into()),
        meeting_date: "2026-11-13".into(),
        report_url: Some(
            "https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/report.md".into(),
        ),
        transcript_url: Some(
            "https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/transcript.md"
                .into(),
        ),
        transcript_anchor: Some("t-00-12-25".into()),
        time_ms: Some(745_000),
        quote: Some("We will do it by Friday".into()),
    }
}

#[test]
fn the_adapter_creates_the_same_issue_as_the_stage_5_path() {
    let direct = fake();
    api(&direct)
        .publish_issue(&repo(), &dispatcher_spec("i1"), false)
        .unwrap();

    let adapted = fake();
    let github = api(&adapted);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    let made = upsert(&exporter, &record("i1"), None, false, &mut |_| {}).unwrap();
    assert!(matches!(&made.outcome, UpsertOutcome::Created(t) if t.id == "1"));

    let (a, b) = (direct.issues(REPO), adapted.issues(REPO));
    assert_eq!((a.len(), b.len()), (1, 1));
    assert_eq!(a[0].title, b[0].title);
    assert_eq!(a[0].body, b[0].body, "the bytes of the issue do not change");
    assert_eq!(a[0].labels, b[0].labels);
    assert_eq!(a[0].assignees, b[0].assignees);
    assert_eq!(a[0].assignees, ["alex-gh"]);
    assert_eq!(a[0].open, b[0].open);
    assert!(b[0]
        .body
        .contains("<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i1 t=745000 -->"));
}

#[test]
fn a_second_export_finds_the_issue_by_its_marker_and_writes_nothing() {
    let gh = fake();
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    let mut stored = Vec::new();
    let first = upsert(&exporter, &record("i1"), None, true, &mut |t| {
        stored.push(t.id.clone());
    })
    .unwrap();
    assert!(matches!(first.outcome, UpsertOutcome::Created(_)));
    assert_eq!(stored, ["1"]);

    let writes_before = gh.requests().iter().filter(|r| r.method != "GET").count();
    let again = upsert(&exporter, &record("i1"), None, true, &mut |_| {}).unwrap();
    assert!(again.found_by_key);
    assert_eq!(again.duplicates, 0);
    assert!(
        matches!(&again.outcome, UpsertOutcome::Unchanged(t) if t.id == "1"),
        "an update leaves the issue as it is: {:?}",
        again.outcome
    );
    let known = upsert(&exporter, &record("i1"), Some("1"), true, &mut |_| {}).unwrap();
    assert!(matches!(known.outcome, UpsertOutcome::Unchanged(_)));
    let writes_after = gh.requests().iter().filter(|r| r.method != "GET").count();
    assert_eq!(writes_before, writes_after, "read-only after the creation");
    assert_eq!(gh.issues(REPO).len(), 1);
}

#[test]
fn the_key_of_another_item_or_meeting_is_not_a_hit() {
    let gh = fake();
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    upsert(&exporter, &record("i1"), None, true, &mut |_| {}).unwrap();

    assert!(exporter.find(&record("i2").key).unwrap().is_empty());
    let mut other_meeting = record("i1").key;
    other_meeting.meeting = "20261120-100000".into();
    assert!(exporter.find(&other_meeting).unwrap().is_empty());
    let found = exporter.find(&record("i1").key).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "1");
    assert!(!found[0].trashed);
}

#[test]
fn duplicates_are_listed_oldest_first() {
    let gh = fake();
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    // A person copied the issue by hand, marker and all.
    api(&gh)
        .publish_issue(&repo, &dispatcher_spec("i1"), false)
        .unwrap();
    api(&gh)
        .publish_issue(&repo, &dispatcher_spec("i1"), false)
        .unwrap();
    let found = exporter.find(&record("i1").key).unwrap();
    assert_eq!(
        found.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["1", "2"]
    );
    let done = upsert(&exporter, &record("i1"), None, false, &mut |_| {}).unwrap();
    assert_eq!(done.outcome.id(), "1");
    assert_eq!(done.duplicates, 1);
}

#[test]
fn a_closed_issue_stays_the_issue_of_its_item() {
    let gh = fake();
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    upsert(&exporter, &record("i1"), None, true, &mut |_| {}).unwrap();
    gh.close_issue(REPO, 1);
    let got = exporter.get("1").unwrap().unwrap();
    assert!(!got.trashed);
    let done = upsert(&exporter, &record("i1"), Some("1"), true, &mut |_| {}).unwrap();
    assert!(matches!(done.outcome, UpsertOutcome::Unchanged(_)));
    assert_eq!(gh.issues(REPO).len(), 1, "nothing is created in its place");
}

#[test]
fn a_missing_issue_and_a_bad_id() {
    let gh = fake();
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    assert_eq!(exporter.get("99").unwrap(), None);
    let err = upsert(&exporter, &record("i1"), Some("99"), true, &mut |_| {}).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(err.message.contains("issue was deleted"), "{}", err.message);
    assert!(exporter.get("abc").is_err());
    assert!(exporter.update("99", &record("i1")).is_err());
    assert_eq!(exporter.name(), "github");
}

#[test]
fn a_pull_request_is_not_an_issue_of_an_item() {
    let gh = fake();
    let number = gh.add_pull_request(REPO, "Some change", &["cycle:NOMAD"]);
    let github = api(&gh);
    let repo = repo();
    let exporter = GithubIssues::new(&github, &repo);
    assert_eq!(exporter.get(&number.to_string()).unwrap(), None);
}
