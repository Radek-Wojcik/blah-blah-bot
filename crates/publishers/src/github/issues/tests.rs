use std::time::Duration;

use super::*;
use crate::github::Github;
use crate::http::HttpClient;
use crate::testing::github::{primary_rate_limit, secondary_rate_limit};
use crate::testing::{FakeGithub, Fault, Reply, Visibility};

const REPO: &str = "owner/rover";
const TOKEN: &str = "ghp_MockTokenValue0123456789";

fn repo() -> RepoRef {
    RepoRef::parse(REPO).unwrap()
}

fn client() -> HttpClient {
    HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
}

fn fake() -> FakeGithub {
    let gh = FakeGithub::start();
    gh.add_repo(REPO, Visibility::Private);
    gh.require_token(TOKEN);
    gh
}

fn api(gh: &FakeGithub) -> Github {
    Github::with_client(client(), &gh.api_base(), TOKEN).unwrap()
}

fn spec(item: &str) -> IssueSpec {
    IssueSpec {
        cycle_id: "NOMAD".into(),
        meeting_id: "20261113-140501".into(),
        item_id: item.into(),
        title: "Order the battery".into(),
        assignee: Some("alex-example".into()),
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
        quote: Some("I will order the battery by Friday".into()),
    }
}

fn status(proposal: &str, open: bool) -> StatusSpec {
    StatusSpec {
        meeting_id: "20261113-140501".into(),
        proposal_id: proposal.into(),
        open,
        reason: "Alex said the battery arrived".into(),
        quote: Some("the battery arrived yesterday".into()),
        meeting_date: "2026-11-13".into(),
        report_url: Some("https://github.test/r/report.md".into()),
        transcript_url: Some("https://github.test/r/transcript.md".into()),
        transcript_anchor: Some("t-00-01-00".into()),
        time_ms: Some(60_000),
    }
}

// ----- text -------------------------------------------------------------------------------------

#[test]
fn the_body_has_the_due_line_the_origin_the_quote_and_the_marker() {
    let body = issue_body(&spec("i3"), true).unwrap();
    assert_eq!(
        body,
        "Due: 2026-11-20\n\n\
         From [meeting 2026-11-13](https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/report.md) at [00:12:25](https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/transcript.md#t-00-12-25)\n\n\
         > I will order the battery by Friday\n\n\
         <!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 t=745000 -->\n"
    );
    // Without the line (the date goes to a field), without links and without a quote.
    let mut bare = spec("i3");
    bare.quote = None;
    bare.report_url = None;
    bare.transcript_url = None;
    bare.time_ms = None;
    let body = issue_body(&bare, false).unwrap();
    assert_eq!(
        body,
        "From meeting 2026-11-13\n\n<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 -->\n"
    );
    // A marker is always there and always parses back.
    let parsed = ItemMarker::parse(&issue_body(&spec("i9"), true).unwrap()).unwrap();
    assert_eq!((parsed.item.as_str(), parsed.t_ms), ("i9", Some(745_000)));
}

#[test]
fn an_item_that_cannot_carry_a_marker_is_refused() {
    let mut bad = spec("i 3");
    assert!(issue_body(&bad, true).is_err());
    bad.item_id = "i3".into();
    bad.cycle_id = "NO MAD".into();
    let gh = fake();
    let err = api(&gh).publish_issue(&repo(), &bad, false).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert_eq!(gh.request_count(), 0);
}

#[test]
fn mentions_in_spoken_text_do_not_notify_anybody() {
    let text = "ask @alex and @everyone, mail a@b.c, 5 @ home, @_x";
    let safe = defuse_mentions(text);
    assert!(safe.contains("@\u{200b}alex") && safe.contains("@\u{200b}everyone"));
    assert!(
        safe.contains("a@\u{200b}b.c"),
        "an address is changed too, harmlessly"
    );
    assert!(safe.contains("5 @ home"));
    assert_eq!(safe.replace('\u{200b}', ""), text);
}

// ----- creating -----------------------------------------------------------------------------------

#[test]
fn an_issue_gets_the_label_the_assignee_the_due_line_and_the_marker() {
    let gh = fake();
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert!(out.created);
    assert_eq!(out.assigned.as_deref(), Some("alex-example"));
    assert!(!out.assignee_dropped);
    assert_eq!(out.due, DueOutcome::Line);
    assert!(out.url.contains("/issues/1"));

    let issue = gh.issue(REPO, out.number).unwrap();
    assert_eq!(issue.title, "Order the battery");
    assert_eq!(issue.labels, ["cycle:NOMAD"]);
    assert_eq!(issue.assignees, ["alex-example"]);
    assert!(
        issue.body.starts_with("Due: 2026-11-20\n"),
        "{}",
        issue.body
    );
    assert!(issue.body.contains("> I will order the battery by Friday"));
    let marker = ItemMarker::parse(&issue.body).unwrap();
    assert_eq!(
        (
            marker.cycle.as_str(),
            marker.meeting.as_str(),
            marker.item.as_str()
        ),
        ("NOMAD", "20261113-140501", "i3")
    );
    // The request went out with the token in the header only.
    let sent = gh.requests_to("POST", "/repos/owner/rover/issues");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(!sent[0].body_text().contains(TOKEN));
}

#[test]
fn an_issue_without_assignee_or_due_date_is_plain() {
    let gh = fake();
    let mut s = spec("i4");
    s.assignee = None;
    s.due = None;
    let out = api(&gh).publish_issue(&repo(), &s, false).unwrap();
    assert_eq!(out.due, DueOutcome::NotRequested);
    assert_eq!(out.assigned, None);
    assert!(!out.assignee_dropped, "nobody was asked for");
    let issue = gh.issue(REPO, out.number).unwrap();
    assert!(issue.assignees.is_empty() && !issue.body.contains("Due:"));
    // No due date: the organisation's fields are not even asked about.
    assert!(gh.requests_to("GET", "/orgs/").is_empty());
}

#[test]
fn an_assignee_github_does_not_take_is_reported() {
    let gh = fake();
    gh.restrict_assignees(REPO, &["someone-else"]);
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert!(out.created && out.assignee_dropped);
    assert_eq!(out.assigned, None);
    assert!(gh.issue(REPO, out.number).unwrap().assignees.is_empty());
}

#[test]
fn title_and_quote_are_cleaned_and_defused() {
    let gh = fake();
    let mut s = spec("i3");
    s.title = format!("  tell @everyone\nabout   the {}", "x".repeat(300));
    s.quote = Some("@alex please\n\nconfirm".into());
    let out = api(&gh).publish_issue(&repo(), &s, false).unwrap();
    let issue = gh.issue(REPO, out.number).unwrap();
    assert!(issue
        .title
        .starts_with("tell @\u{200b}everyone about the x"));
    assert!(
        issue.title.chars().count() <= 200 && issue.title.ends_with("..."),
        "{}",
        issue.title
    );
    assert!(!issue.title.contains('\n'));
    assert!(
        issue.body.contains("> @\u{200b}alex please\n> confirm"),
        "{}",
        issue.body
    );
}

#[test]
fn an_organisation_field_takes_the_due_date_and_the_body_has_no_line() {
    let gh = fake();
    gh.set_organization(REPO);
    let field = gh.add_org_issue_field("owner", "Target date", "date");
    gh.add_org_issue_field("owner", "Priority", "single_select");
    let github = api(&gh);
    let out = github.publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Field);
    let issue = gh.issue(REPO, out.number).unwrap();
    assert!(!issue.body.contains("Due:"), "{}", issue.body);
    assert_eq!(
        gh.issue_field_values(REPO, out.number),
        [(field, json!("2026-11-20"))]
    );
    // The documented version header was sent to the field endpoints, and the field is
    // looked up once however many issues follow.
    let put = gh.requests_to("PUT", "/repos/owner/rover/issues/1/issue-field-values");
    assert_eq!(put[0].header("x-github-api-version"), Some("2026-03-10"));
    assert_eq!(
        put[0].json().unwrap(),
        json!({"issue_field_values": [{"field_id": field, "value": "2026-11-20"}]})
    );
    github.publish_issue(&repo(), &spec("i4"), false).unwrap();
    assert_eq!(gh.requests_to("GET", "/orgs/owner/issue-fields").len(), 1);
}

#[test]
fn without_a_usable_field_the_due_line_is_used() {
    // An organisation without the field, with one of the wrong type, and a personal repository.
    let gh = fake();
    gh.set_organization(REPO);
    gh.add_org_issue_field("owner", "Target date", "text");
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Line);
    assert!(gh
        .issue(REPO, out.number)
        .unwrap()
        .body
        .contains("Due: 2026-11-20"));

    let gh = fake();
    gh.set_organization(REPO);
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Line);

    let gh = fake();
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Line);
    assert_eq!(
        gh.requests_to("GET", "/orgs/owner/issue-fields").len(),
        1,
        "a 404 is remembered"
    );
}

#[test]
fn a_failing_field_probe_or_write_falls_back_to_the_line() {
    // The probe is refused.
    let gh = fake();
    gh.set_organization(REPO);
    gh.add_org_issue_field("owner", "Target date", "date");
    gh.inject_for(
        "GET",
        "/orgs/owner/issue-fields",
        Fault::Reply(Reply::json(
            403,
            &json!({"message": "Resource not accessible by personal access token"}),
        )),
        1,
    );
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Line);

    // The field is found but writing to it fails: the line is added to the body afterwards.
    let gh = fake();
    gh.set_organization(REPO);
    gh.add_org_issue_field("owner", "Target date", "date");
    gh.inject_for(
        "PUT",
        "/repos/owner/rover/issues/",
        Fault::Reply(Reply::json(404, &json!({"message": "Not Found"}))),
        1,
    );
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Line);
    let issue = gh.issue(REPO, out.number).unwrap();
    assert!(
        issue.body.starts_with("Due: 2026-11-20\n"),
        "{}",
        issue.body
    );
    assert!(
        ItemMarker::parse(&issue.body).is_some(),
        "the marker survived the edit"
    );
    assert!(gh.issue_field_values(REPO, out.number).is_empty());

    // Both fail: the issue exists without a date and says so.
    let gh = fake();
    gh.set_organization(REPO);
    gh.add_org_issue_field("owner", "Target date", "date");
    gh.inject_for(
        "PUT",
        "/repos/owner/rover/issues/",
        Fault::Reply(Reply::empty(404)),
        1,
    );
    gh.inject_for(
        "PATCH",
        "/repos/owner/rover/issues/",
        Fault::Reply(Reply::empty(500)),
        1,
    );
    let out = api(&gh).publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert_eq!(out.due, DueOutcome::Missing);
    assert!(out.created);

    // A server that is down for the probe is asked again next time.
    let gh = fake();
    gh.set_organization(REPO);
    let field = gh.add_org_issue_field("owner", "Target date", "date");
    gh.inject_for(
        "GET",
        "/orgs/owner/issue-fields",
        Fault::Reply(Reply::empty(503)),
        1,
    );
    let github = api(&gh);
    assert_eq!(
        github
            .publish_issue(&repo(), &spec("i3"), false)
            .unwrap()
            .due,
        DueOutcome::Line
    );
    let second = github.publish_issue(&repo(), &spec("i4"), false).unwrap();
    assert_eq!(second.due, DueOutcome::Field);
    assert_eq!(gh.issue_field_values(REPO, second.number)[0].0, field);
}

#[test]
fn a_retry_after_a_lost_answer_finds_the_issue_instead_of_creating_another() {
    let gh = fake();
    let github = api(&gh);
    // The server handles the request and the connection drops before the answer.
    gh.inject_for("POST", "/repos/owner/rover/issues", Fault::DropAfter, 1);
    let err = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    assert_eq!(
        gh.issues(REPO).len(),
        1,
        "the issue exists although the answer was lost"
    );

    // Another item is made meanwhile; it must not be mistaken for the lost one.
    github.publish_issue(&repo(), &spec("i4"), false).unwrap();

    let retry = github.publish_issue(&repo(), &spec("i3"), true).unwrap();
    assert!(!retry.created);
    assert_eq!(retry.number, 1);
    assert_eq!(retry.due, DueOutcome::Existing);
    assert_eq!(gh.issues(REPO).len(), 2, "no duplicate");

    // With nothing to find, the scan creates it.
    let fresh = github.publish_issue(&repo(), &spec("i5"), true).unwrap();
    assert!(fresh.created);
    assert_eq!(gh.issues(REPO).len(), 3);
    // An issue of another meeting with the same item id is not this one.
    let mut other = spec("i3");
    other.meeting_id = "20261120-100000".into();
    assert!(github.publish_issue(&repo(), &other, true).unwrap().created);
}

#[test]
fn a_closed_issue_is_found_by_the_scan_as_well() {
    let gh = fake();
    let github = api(&gh);
    let first = github.publish_issue(&repo(), &spec("i3"), false).unwrap();
    gh.close_issue(REPO, first.number);
    let retry = github.publish_issue(&repo(), &spec("i3"), true).unwrap();
    assert!(!retry.created && retry.number == first.number);
}

#[test]
fn a_scan_during_the_listing_lag_misses_the_issue_and_one_after_it_finds_it() {
    let gh = fake();
    // github.com lists a new issue only seconds after it was made.
    gh.lag_lists(REPO, 1);
    let github = api(&gh);
    let first = github.publish_issue(&repo(), &spec("i3"), false).unwrap();
    assert!(
        github
            .list_cycle_issues(&repo(), "NOMAD", 1)
            .unwrap()
            .is_empty(),
        "the lag is in effect: the list does not show the issue yet"
    );
    // Once the lag is over, a retry finds it by the marker.
    let found = github
        .find_issue_by_marker(&repo(), &spec("i3").marker())
        .unwrap();
    assert_eq!(found.map(|r| r.number), Some(first.number));
    let retry = github.publish_issue(&repo(), &spec("i3"), true).unwrap();
    assert!(!retry.created);
    assert_eq!(gh.issues(REPO).len(), 1, "no duplicate");
}

#[test]
fn the_number_of_a_created_issue_is_reported_at_once_and_found_again_without_a_list() {
    let gh = fake();
    // The lists show nothing new for a long while.
    gh.lag_lists(REPO, 1000);
    let github = api(&gh);
    let mut seen = Vec::new();
    let first = github
        .publish_issue_tracked(&repo(), &spec("i3"), true, None, &mut |n| seen.push(n))
        .unwrap();
    assert!(first.created);
    assert_eq!(seen, vec![first.number]);

    // A retry that knows the number fetches that issue: one request, no list, no duplicate.
    let before = gh.requests().len();
    let retry = github
        .publish_issue_tracked(&repo(), &spec("i3"), true, Some(first.number), &mut |_| {
            panic!("nothing is created")
        })
        .unwrap();
    assert!(!retry.created && retry.number == first.number);
    assert_eq!(retry.due, DueOutcome::Existing);
    assert_eq!(gh.requests().len() - before, 1, "one direct request");

    // A number that belongs to another item, or to nothing, is not trusted.
    let other = github.publish_issue(&repo(), &spec("i4"), false).unwrap();
    let made = github
        .publish_issue_tracked(&repo(), &spec("i5"), false, Some(other.number), &mut |_| {})
        .unwrap();
    assert!(made.created, "issue {} is not the item i5", other.number);
    let gone = github
        .publish_issue_tracked(&repo(), &spec("i6"), false, Some(999), &mut |_| {})
        .unwrap();
    assert!(gone.created);
    assert_eq!(gh.issues(REPO).len(), 4, "i3, i4, i5 and i6, no duplicate");
}

#[test]
fn rate_limits_and_refusals_are_classified() {
    let gh = fake();
    let github = api(&gh);
    gh.inject_for(
        "POST",
        "/repos/owner/rover/issues",
        Fault::Reply(secondary_rate_limit(30)),
        1,
    );
    let err = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);
    assert_eq!(
        err.failure(),
        store::Failure::Transient {
            retry_after_ms: Some(30_000)
        }
    );
    assert!(gh.issues(REPO).is_empty());

    gh.inject_for(
        "POST",
        "/repos/owner/rover/issues",
        Fault::Reply(primary_rate_limit(4_000_000_000)),
        1,
    );
    let err = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);

    // A wrong token, a missing repository and disabled issues need the user.
    let wrong = Github::with_client(client(), &gh.api_base(), "wr0ng-token-value").unwrap();
    let err = wrong
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert_eq!(err.status, Some(401));
    assert!(!err.message.contains("wr0ng-token-value"));
    let ghost = RepoRef::parse("owner/ghost").unwrap();
    assert_eq!(
        github
            .publish_issue(&ghost, &spec("i3"), false)
            .unwrap_err()
            .status,
        Some(404)
    );
    gh.disable_issues(REPO);
    let err = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert_eq!(err.status, Some(410));
    assert_eq!(err.failure(), store::Failure::Permanent);
}

// ----- status ---------------------------------------------------------------------------------------

#[test]
fn a_status_change_closes_the_issue_as_completed_and_comments_once() {
    let gh = fake();
    let github = api(&gh);
    let n = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap()
        .number;
    let out = github
        .set_issue_status(&repo(), n, &status("s1", false))
        .unwrap();
    assert_eq!(
        out,
        StatusOutcome {
            changed: true,
            commented: true
        }
    );
    let issue = gh.issue(REPO, n).unwrap();
    assert!(!issue.open);
    assert_eq!(issue.state_reason.as_deref(), Some("completed"));
    assert_eq!(issue.comments.len(), 1);
    let comment = &issue.comments[0].body;
    assert!(
        comment.starts_with(
            "Marked as done after the meeting of 2026-11-13: Alex said the battery arrived"
        ),
        "{comment}"
    );
    assert!(comment.contains("> the battery arrived yesterday"));
    assert!(comment.contains("[00:01:00](https://github.test/r/transcript.md#t-00-01-00)"));
    let marker = StatusMarker::parse(comment).unwrap();
    assert_eq!(
        (marker.meeting.as_str(), marker.proposal.as_str()),
        ("20261113-140501", "s1")
    );

    // The same proposal again (a retry): nothing changes and nothing is said twice.
    let again = github
        .set_issue_status(&repo(), n, &status("s1", false))
        .unwrap();
    assert_eq!(
        again,
        StatusOutcome {
            changed: false,
            commented: false
        }
    );
    assert_eq!(gh.issue(REPO, n).unwrap().comments.len(), 1);
    assert!(gh.requests_to("PATCH", "/repos/owner/rover/issues/1").len() == 1);

    // Another proposal of the same meeting is another comment.
    let other = github
        .set_issue_status(&repo(), n, &status("s2", false))
        .unwrap();
    assert!(other.commented && !other.changed);
    assert_eq!(gh.issue(REPO, n).unwrap().comments.len(), 2);
}

#[test]
fn reopening_and_a_person_having_closed_it_already() {
    let gh = fake();
    let github = api(&gh);
    let n = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap()
        .number;
    gh.close_issue(REPO, n);
    // Closed by a person already: the state is left, the reason is still recorded.
    let out = github
        .set_issue_status(&repo(), n, &status("s1", false))
        .unwrap();
    assert_eq!(
        out,
        StatusOutcome {
            changed: false,
            commented: true
        }
    );
    assert!(gh
        .requests_to("PATCH", "/repos/owner/rover/issues/1")
        .is_empty());
    // Reopen.
    let out = github
        .set_issue_status(&repo(), n, &status("s2", true))
        .unwrap();
    assert!(out.changed && out.commented);
    let issue = gh.issue(REPO, n).unwrap();
    assert!(issue.open);
    assert_eq!(issue.state_reason.as_deref(), Some("reopened"));
    assert!(issue.comments[1]
        .body
        .starts_with("Reopened after the meeting"));
}

#[test]
fn a_comment_that_failed_is_posted_once_by_the_retry() {
    let gh = fake();
    let github = api(&gh);
    let n = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap()
        .number;
    gh.inject_for(
        "POST",
        "/repos/owner/rover/issues/1/comments",
        Fault::Reply(Reply::empty(502)),
        1,
    );
    let err = github
        .set_issue_status(&repo(), n, &status("s1", false))
        .unwrap_err();
    assert!(matches!(err.failure(), store::Failure::Transient { .. }));
    assert!(!gh.issue(REPO, n).unwrap().open, "the close went through");
    assert!(gh.issue(REPO, n).unwrap().comments.is_empty());

    let out = github
        .set_issue_status(&repo(), n, &status("s1", false))
        .unwrap();
    assert_eq!(
        out,
        StatusOutcome {
            changed: false,
            commented: true
        }
    );
    assert_eq!(gh.issue(REPO, n).unwrap().comments.len(), 1);

    // And a comment that was posted but whose answer was lost.
    let m = github
        .publish_issue(&repo(), &spec("i4"), false)
        .unwrap()
        .number;
    gh.inject_for(
        "POST",
        "/repos/owner/rover/issues/2/comments",
        Fault::DropAfter,
        1,
    );
    assert!(github
        .set_issue_status(&repo(), m, &status("s9", false))
        .is_err());
    assert_eq!(gh.issue(REPO, m).unwrap().comments.len(), 1);
    let out = github
        .set_issue_status(&repo(), m, &status("s9", false))
        .unwrap();
    assert!(!out.commented);
    assert_eq!(gh.issue(REPO, m).unwrap().comments.len(), 1);
}

#[test]
fn a_reason_with_a_mention_does_not_notify() {
    let gh = fake();
    let github = api(&gh);
    let n = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap()
        .number;
    let mut s = status("s1", false);
    s.reason = "thanks @alex-example".into();
    github.set_issue_status(&repo(), n, &s).unwrap();
    assert!(gh.issue(REPO, n).unwrap().comments[0]
        .body
        .contains("@\u{200b}alex-example"));
}

// ----- reading ------------------------------------------------------------------------------------

#[test]
fn the_list_has_every_state_links_and_no_pull_requests() {
    let gh = fake();
    let github = api(&gh);
    let a = github
        .publish_issue(&repo(), &spec("i1"), false)
        .unwrap()
        .number;
    let b = github
        .publish_issue(&repo(), &spec("i2"), false)
        .unwrap()
        .number;
    gh.add_issue(REPO, "By hand", &["cycle:NOMAD"]);
    gh.add_pull_request(REPO, "A pull request", &["cycle:NOMAD"]);
    gh.add_issue(REPO, "Another cycle", &["cycle:ROVER"]);
    gh.close_issue(REPO, b);

    let records = github.list_cycle_issues(&repo(), "NOMAD", 5).unwrap();
    // Most recently updated first: b was closed last.
    assert_eq!(
        records.iter().map(|r| r.number).collect::<Vec<_>>(),
        [b, 3, a]
    );
    assert!(!records[0].open && records[1].open);
    assert_eq!(records[0].marker.as_ref().unwrap().item, "i2");
    assert_eq!(records[1].marker, None);

    let cache = cycle_store::IssueCache::from_records(REPO, "cycle:NOMAD", "NOMAD", 1, &records);
    assert_eq!(cache.closed, [b]);
    assert_eq!(cache.links.len(), 2);
    assert!(cache.links.iter().any(|l| l.item == "i2" && !l.open));

    let request = gh
        .requests_to("GET", "/repos/owner/rover/issues")
        .pop()
        .unwrap();
    assert_eq!(request.query_param("state"), Some("all"));
    assert_eq!(request.query_param("labels"), Some("cycle%3ANOMAD"));
}

#[test]
fn the_list_is_paged_and_capped() {
    let gh = fake();
    for n in 0..230 {
        gh.add_issue(REPO, &format!("Issue {n}"), &["cycle:NOMAD"]);
    }
    let github = api(&gh);
    assert_eq!(
        github.list_cycle_issues(&repo(), "NOMAD", 1).unwrap().len(),
        100
    );
    assert_eq!(
        github.list_cycle_issues(&repo(), "NOMAD", 5).unwrap().len(),
        230
    );
    // Never more than MAX_PAGES requests, whatever is asked.
    gh.clear_requests();
    for n in 0..300 {
        gh.add_issue(REPO, &format!("More {n}"), &["cycle:NOMAD"]);
    }
    let all = github.list_cycle_issues(&repo(), "NOMAD", 99).unwrap();
    assert_eq!(all.len(), 500);
    assert_eq!(gh.requests_to("GET", "/repos/owner/rover/issues").len(), 5);
}

#[test]
fn an_unusable_cycle_id_never_reaches_the_network() {
    let gh = fake();
    assert!(api(&gh).list_cycle_issues(&repo(), "a b", 1).is_err());
    assert!(api(&gh).list_cycle_issues(&repo(), "", 1).is_err());
    assert_eq!(gh.request_count(), 0);
}

#[test]
fn offline_is_reported_as_such() {
    let gh = fake();
    let github = api(&gh);
    gh.go_offline();
    let err = github.list_cycle_issues(&repo(), "NOMAD", 1).unwrap_err();
    assert!(err.is_offline());
    let err = github
        .publish_issue(&repo(), &spec("i3"), false)
        .unwrap_err();
    assert!(err.is_offline());
}
