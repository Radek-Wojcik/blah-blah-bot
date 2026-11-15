//! A stateful fake of the part of the GitHub REST API the publisher uses.
//!
//! Implemented, as documented on docs.github.com for the 2022-11-28 API version:
//!
//! * `GET /repos/{owner}/{repo}`: name, `private`, `visibility`, owner type, `permissions`.
//! * `GET /repos/{owner}/{repo}/issues`: `state`, `labels` (comma separated, all required),
//!   `sort` and `direction`, `per_page` (at most 100), `page`, a `Link` header; pull requests
//!   are included and carry a `pull_request` key, as on GitHub.
//! * `POST /repos/{owner}/{repo}/issues`, `GET` and `PATCH /repos/{owner}/{repo}/issues/{n}`,
//!   `GET` and `POST /repos/{owner}/{repo}/issues/{n}/comments`.
//! * `GET /user`, to check a token.
//!
//! Behaviour copied from GitHub: a request without `User-Agent` is refused with 403; a wrong
//! token is 401 "Bad credentials"; a private repository does not exist for anonymous clients
//! (404); writes need a token; assignees who cannot be assigned are dropped silently; labels
//! are created on first use; closing an issue sets `state_reason` to `completed`.
//!
//! Not implemented: the organisation issue field "Target date" (open question Q1 of the
//! plan, unverified), so a client has to use the `Due:` line.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use super::server::{FakeServer, Recorded, Reply};

/// Visibility of a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

impl Visibility {
    fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
        }
    }
}

/// A comment as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredComment {
    pub id: u64,
    pub body: String,
}

/// An issue (or pull request) as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub open: bool,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub pull_request: bool,
    pub comments: Vec<StoredComment>,
    created: u64,
    updated: u64,
}

struct Repo {
    full_name: String,
    visibility: Visibility,
    owner_type: &'static str,
    issues_enabled: bool,
    /// What `permissions.push` of the repository says for an authenticated client.
    push_allowed: bool,
    /// `None`: everybody can be assigned.
    assignable: Option<Vec<String>>,
    issues: Vec<StoredIssue>,
    labels: Vec<String>,
    next_number: u64,
    next_comment_id: u64,
}

struct State {
    base: String,
    repos: BTreeMap<String, Repo>,
    token: Option<String>,
    clock: u64,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

/// The fake GitHub. Derefs to its [`FakeServer`] for faults, offline mode and the log of
/// requests.
pub struct FakeGithub {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}

impl Deref for FakeGithub {
    type Target = FakeServer;
    fn deref(&self) -> &FakeServer {
        &self.server
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `403` with `Retry-After`: GitHub's secondary rate limit.
pub fn secondary_rate_limit(retry_after_secs: u64) -> Reply {
    Reply::json(
        403,
        &json!({"message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}),
    )
    .with_header("Retry-After", &retry_after_secs.to_string())
}

/// `403` with an exhausted `x-ratelimit-remaining` and the epoch second of the reset: the
/// primary rate limit.
pub fn primary_rate_limit(reset_epoch_secs: u64) -> Reply {
    Reply::json(
        403,
        &json!({"message": "API rate limit exceeded for user."}),
    )
    .with_header("x-ratelimit-limit", "5000")
    .with_header("x-ratelimit-remaining", "0")
    .with_header("x-ratelimit-reset", &reset_epoch_secs.to_string())
}

impl FakeGithub {
    /// Starts the fake with no repository and no token requirement.
    pub fn start() -> FakeGithub {
        let state = Arc::new(Mutex::new(State {
            base: String::new(),
            repos: BTreeMap::new(),
            token: None,
            clock: 0,
        }));
        let shared = Arc::clone(&state);
        let server = FakeServer::start(move |req| handle(&shared, req));
        lock(&state).base = server.base_url();
        FakeGithub { server, state }
    }

    /// The API base to give to the publisher (`http://127.0.0.1:<port>`).
    pub fn api_base(&self) -> String {
        self.server.base_url()
    }

    /// Requires this token for everything but reads of public repositories.
    pub fn require_token(&self, token: &str) {
        lock(&self.state).token = Some(token.to_string());
    }

    /// Adds a repository owned by a user.
    pub fn add_repo(&self, full_name: &str, visibility: Visibility) {
        lock(&self.state).repos.insert(
            full_name.to_ascii_lowercase(),
            Repo {
                full_name: full_name.to_string(),
                visibility,
                owner_type: "User",
                issues_enabled: true,
                push_allowed: true,
                assignable: None,
                issues: Vec::new(),
                labels: Vec::new(),
                next_number: 1,
                next_comment_id: 1,
            },
        );
    }

    fn with_repo<R>(&self, full_name: &str, f: impl FnOnce(&mut Repo) -> R) -> R {
        let mut state = lock(&self.state);
        let repo = state
            .repos
            .get_mut(&full_name.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("the fake has no repository {full_name}"));
        f(repo)
    }

    pub fn set_visibility(&self, full_name: &str, visibility: Visibility) {
        self.with_repo(full_name, |r| r.visibility = visibility);
    }

    /// Makes the owner an organisation.
    pub fn set_organization(&self, full_name: &str) {
        self.with_repo(full_name, |r| r.owner_type = "Organization");
    }

    /// Whether the token's user may push: `permissions.push` of the repository answer.
    pub fn set_push_permission(&self, full_name: &str, allowed: bool) {
        self.with_repo(full_name, |r| r.push_allowed = allowed);
    }

    /// Turns issues off: the issue endpoints answer 410.
    pub fn disable_issues(&self, full_name: &str) {
        self.with_repo(full_name, |r| r.issues_enabled = false);
    }

    /// Only these logins can be assigned; others are dropped silently.
    pub fn restrict_assignees(&self, full_name: &str, logins: &[&str]) {
        self.with_repo(full_name, |r| {
            r.assignable = Some(logins.iter().map(|s| (*s).to_string()).collect());
        });
    }

    /// An issue that exists before the bot acts, as created by a person.
    pub fn add_issue(&self, full_name: &str, title: &str, labels: &[&str]) -> u64 {
        let mut state = lock(&self.state);
        let at = state.tick();
        let repo = state
            .repos
            .get_mut(&full_name.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("the fake has no repository {full_name}"));
        let labels: Vec<String> = labels.iter().map(|s| (*s).to_string()).collect();
        new_issue(repo, title, "", &labels, &[], false, at)
    }

    /// A pull request, which the issues endpoint lists among issues.
    pub fn add_pull_request(&self, full_name: &str, title: &str, labels: &[&str]) -> u64 {
        let mut state = lock(&self.state);
        let at = state.tick();
        let repo = state
            .repos
            .get_mut(&full_name.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("the fake has no repository {full_name}"));
        let labels: Vec<String> = labels.iter().map(|s| (*s).to_string()).collect();
        new_issue(repo, title, "", &labels, &[], true, at)
    }

    /// A person closes an issue on github.com.
    pub fn close_issue(&self, full_name: &str, number: u64) {
        self.set_open(full_name, number, false);
    }

    /// A person reopens an issue on github.com.
    pub fn reopen_issue(&self, full_name: &str, number: u64) {
        self.set_open(full_name, number, true);
    }

    fn set_open(&self, full_name: &str, number: u64, open: bool) {
        let mut state = lock(&self.state);
        let at = state.tick();
        let repo = state
            .repos
            .get_mut(&full_name.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("the fake has no repository {full_name}"));
        let issue = repo
            .issues
            .iter_mut()
            .find(|i| i.number == number)
            .unwrap_or_else(|| panic!("no issue {number}"));
        apply_state(issue, open, None);
        issue.updated = at;
    }

    /// All issues of a repository, oldest first.
    pub fn issues(&self, full_name: &str) -> Vec<StoredIssue> {
        self.with_repo(full_name, |r| r.issues.clone())
    }

    pub fn issue(&self, full_name: &str, number: u64) -> Option<StoredIssue> {
        self.with_repo(full_name, |r| {
            r.issues.iter().find(|i| i.number == number).cloned()
        })
    }

    /// The labels that exist in a repository.
    pub fn labels(&self, full_name: &str) -> Vec<String> {
        self.with_repo(full_name, |r| r.labels.clone())
    }
}

// ----- the API ---------------------------------------------------------------------------

fn error(status: u16, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({"message": message, "documentation_url": "https://docs.github.com/rest"}),
    )
}

fn not_found() -> Reply {
    error(404, "Not Found")
}

fn validation_failed(field: &str, code: &str) -> Reply {
    Reply::json(
        422,
        &json!({
            "message": "Validation Failed",
            "errors": [{"resource": "Issue", "code": code, "field": field}],
        }),
    )
}

enum Auth {
    Anonymous,
    Token,
    Rejected,
}

fn authenticate(state: &State, req: &Recorded) -> Auth {
    let Some(header) = req.header("authorization") else {
        return Auth::Anonymous;
    };
    let given = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("token "))
        .unwrap_or("");
    match &state.token {
        Some(token) if token == given => Auth::Token,
        Some(_) => Auth::Rejected,
        // No token is required: any credentials are accepted.
        None => Auth::Token,
    }
}

fn handle(shared: &Mutex<State>, req: &Recorded) -> Reply {
    let mut state = lock(shared);
    if req.header("user-agent").is_none_or(str::is_empty) {
        return error(
            403,
            "Request forbidden by administrative rules. Please make sure your request has a User-Agent header.",
        );
    }
    let auth = authenticate(&state, req);
    if matches!(auth, Auth::Rejected) {
        return error(401, "Bad credentials");
    }
    let authed = matches!(auth, Auth::Token);
    let segments: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["user"] if req.method == "GET" => {
            if authed {
                Reply::json(200, &json!({"login": "fake-bot", "type": "User"}))
            } else {
                error(401, "Requires authentication")
            }
        }
        ["repos", owner, name, rest @ ..] => {
            let key = format!("{owner}/{name}").to_ascii_lowercase();
            let at = state.tick();
            let base = state.base.clone();
            let Some(repo) = state.repos.get_mut(&key) else {
                return not_found();
            };
            // A private repository does not exist for those who may not see it.
            if repo.visibility != Visibility::Public && !authed {
                return not_found();
            }
            if req.method != "GET" && !authed {
                return error(401, "Requires authentication");
            }
            route(repo, rest, req, &base, at, authed)
        }
        _ => not_found(),
    }
}

fn route(
    repo: &mut Repo,
    rest: &[&str],
    req: &Recorded,
    base: &str,
    at: u64,
    authed: bool,
) -> Reply {
    match (req.method.as_str(), rest) {
        ("GET", []) => Reply::json(
            200,
            &json!({
                "full_name": repo.full_name,
                "private": repo.visibility != Visibility::Public,
                "visibility": repo.visibility.as_str(),
                "owner": {"type": repo.owner_type},
                "has_issues": repo.issues_enabled,
                "permissions": {"admin": false, "push": authed && repo.push_allowed, "pull": true},
            }),
        ),
        (_, ["issues", ..]) if !repo.issues_enabled => {
            error(410, "Issues are disabled for this repo")
        }
        ("GET", ["issues"]) => list_issues(repo, req, base),
        ("POST", ["issues"]) => create_issue(repo, req, base, at),
        ("GET", ["issues", n]) => match find_issue(repo, n) {
            Some(i) => Reply::json(200, &issue_json(repo, &repo.issues[i], base)),
            None => not_found(),
        },
        ("PATCH", ["issues", n]) => patch_issue(repo, n, req, base, at),
        ("GET", ["issues", n, "comments"]) => match find_issue(repo, n) {
            Some(i) => {
                let list: Vec<Value> = repo.issues[i]
                    .comments
                    .iter()
                    .map(|c| comment_json(repo, repo.issues[i].number, c, base))
                    .collect();
                Reply::json(200, &Value::Array(list))
            }
            None => not_found(),
        },
        ("POST", ["issues", n, "comments"]) => post_comment(repo, n, req, base, at),
        _ => not_found(),
    }
}

fn find_issue(repo: &Repo, number: &str) -> Option<usize> {
    let number: u64 = number.parse().ok()?;
    repo.issues.iter().position(|i| i.number == number)
}

fn parse_body(req: &Recorded) -> Result<Value, Reply> {
    req.json()
        .filter(Value::is_object)
        .ok_or_else(|| error(400, "Problems parsing JSON"))
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|v| {
                    v.as_str()
                        .or_else(|| v.get("name").and_then(Value::as_str))
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn new_issue(
    repo: &mut Repo,
    title: &str,
    body: &str,
    labels: &[String],
    assignees: &[String],
    pull_request: bool,
    at: u64,
) -> u64 {
    let number = repo.next_number;
    repo.next_number += 1;
    for label in labels {
        if !repo.labels.contains(label) {
            repo.labels.push(label.clone());
        }
    }
    repo.issues.push(StoredIssue {
        number,
        title: title.to_string(),
        body: body.to_string(),
        open: true,
        state_reason: None,
        labels: labels.to_vec(),
        assignees: assignees.to_vec(),
        pull_request,
        comments: Vec::new(),
        created: at,
        updated: at,
    });
    number
}

fn keep_assignable(repo: &Repo, logins: Vec<String>) -> Vec<String> {
    match &repo.assignable {
        None => logins,
        Some(ok) => logins.into_iter().filter(|l| ok.contains(l)).collect(),
    }
}

fn create_issue(repo: &mut Repo, req: &Recorded, base: &str, at: u64) -> Reply {
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if title.is_empty() {
        return validation_failed("title", "missing_field");
    }
    let text = body.get("body").and_then(Value::as_str).unwrap_or("");
    let labels = names(&body["labels"]);
    let mut assignees = names(&body["assignees"]);
    if let Some(one) = body.get("assignee").and_then(Value::as_str) {
        assignees.push(one.to_string());
    }
    let assignees = keep_assignable(repo, assignees);
    let number = new_issue(repo, title, text, &labels, &assignees, false, at);
    let i = repo.issues.len() - 1;
    debug_assert_eq!(repo.issues[i].number, number);
    Reply::json(201, &issue_json(repo, &repo.issues[i], base))
}

fn patch_issue(repo: &mut Repo, number: &str, req: &Recorded, base: &str, at: u64) -> Reply {
    let Some(i) = find_issue(repo, number) else {
        return not_found();
    };
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let state_change = match body.get("state").and_then(Value::as_str) {
        None => None,
        Some("open") => Some(true),
        Some("closed") => Some(false),
        Some(_) => return validation_failed("state", "invalid"),
    };
    let reason = body.get("state_reason").and_then(Value::as_str);
    if let Some(reason) = reason {
        if !matches!(
            reason,
            "completed" | "not_planned" | "reopened" | "duplicate"
        ) {
            return validation_failed("state_reason", "invalid");
        }
    }
    if let Some(title) = body.get("title") {
        if title.as_str().is_none_or(|t| t.trim().is_empty()) {
            return validation_failed("title", "invalid");
        }
    }
    let labels = body.get("labels").map(names);
    let assignees = body
        .get("assignees")
        .map(|a| keep_assignable(repo, names(a)));
    if let Some(labels) = &labels {
        for label in labels {
            if !repo.labels.contains(label) {
                repo.labels.push(label.clone());
            }
        }
    }
    let issue = &mut repo.issues[i];
    if let Some(title) = body.get("title").and_then(Value::as_str) {
        issue.title = title.trim().to_string();
    }
    if let Some(text) = body.get("body").and_then(Value::as_str) {
        issue.body = text.to_string();
    }
    if let Some(labels) = labels {
        issue.labels = labels;
    }
    if let Some(assignees) = assignees {
        issue.assignees = assignees;
    }
    if let Some(open) = state_change {
        apply_state(issue, open, reason);
    }
    issue.updated = at;
    Reply::json(200, &issue_json(repo, &repo.issues[i], base))
}

fn apply_state(issue: &mut StoredIssue, open: bool, reason: Option<&str>) {
    if open {
        issue.state_reason = if issue.open {
            issue.state_reason.clone()
        } else {
            Some(reason.unwrap_or("reopened").to_string())
        };
    } else {
        issue.state_reason = Some(reason.unwrap_or("completed").to_string());
    }
    issue.open = open;
}

fn post_comment(repo: &mut Repo, number: &str, req: &Recorded, base: &str, at: u64) -> Reply {
    let Some(i) = find_issue(repo, number) else {
        return not_found();
    };
    let body = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = body.get("body").and_then(Value::as_str).unwrap_or("");
    if text.trim().is_empty() {
        return validation_failed("body", "missing_field");
    }
    let id = repo.next_comment_id;
    repo.next_comment_id += 1;
    let comment = StoredComment {
        id,
        body: text.to_string(),
    };
    repo.issues[i].comments.push(comment.clone());
    repo.issues[i].updated = at;
    Reply::json(
        201,
        &comment_json(repo, repo.issues[i].number, &comment, base),
    )
}

fn list_issues(repo: &Repo, req: &Recorded, base: &str) -> Reply {
    let state = req.query_param("state").unwrap_or("open");
    if !matches!(state, "open" | "closed" | "all") {
        return validation_failed("state", "invalid");
    }
    let wanted: Vec<String> = req
        .query_param("labels")
        .map(|l| {
            decode(l)
                .split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let by_update = req.query_param("sort") == Some("updated");
    let ascending = req.query_param("direction") == Some("asc");
    let per_page = req
        .query_param("per_page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 100);
    let page = req
        .query_param("page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    let mut matching: Vec<&StoredIssue> = repo
        .issues
        .iter()
        .filter(|i| match state {
            "open" => i.open,
            "closed" => !i.open,
            _ => true,
        })
        .filter(|i| wanted.iter().all(|l| i.labels.contains(l)))
        .collect();
    matching.sort_by_key(|i| if by_update { i.updated } else { i.created });
    if !ascending {
        matching.reverse();
    }
    let total = matching.len();
    let slice: Vec<Value> = matching
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .map(|i| issue_json(repo, i, base))
        .collect();
    let mut reply = Reply::json(200, &Value::Array(slice));
    if page * per_page < total {
        let query: Vec<String> = req
            .query
            .split('&')
            .filter(|p| !p.starts_with("page=") && !p.is_empty())
            .map(String::from)
            .chain(std::iter::once(format!("page={}", page + 1)))
            .collect();
        reply = reply.with_header(
            "Link",
            &format!("<{base}{}?{}>; rel=\"next\"", req.path, query.join("&")),
        );
    }
    reply
}

/// Percent-decodes a query value (labels may contain `%3A` for a colon).
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn stamp(tick: u64) -> String {
    format!(
        "2026-01-01T{:02}:{:02}:{:02}Z",
        (tick / 3600) % 24,
        (tick / 60) % 60,
        tick % 60
    )
}

fn issue_json(repo: &Repo, issue: &StoredIssue, base: &str) -> Value {
    let html = format!(
        "https://github.test/{}/{}/{}",
        repo.full_name,
        if issue.pull_request { "pull" } else { "issues" },
        issue.number
    );
    let mut value = json!({
        "id": 1_000 + issue.number,
        "number": issue.number,
        "title": issue.title,
        "body": issue.body,
        "state": if issue.open { "open" } else { "closed" },
        "state_reason": issue.state_reason,
        "labels": issue.labels.iter().map(|l| json!({"name": l})).collect::<Vec<_>>(),
        "assignee": issue.assignees.first().map(|l| json!({"login": l})),
        "assignees": issue.assignees.iter().map(|l| json!({"login": l})).collect::<Vec<_>>(),
        "comments": issue.comments.len(),
        "html_url": html,
        "url": format!("{base}/repos/{}/issues/{}", repo.full_name, issue.number),
        "user": {"login": "fake-bot"},
        "created_at": stamp(issue.created),
        "updated_at": stamp(issue.updated),
    });
    if issue.pull_request {
        value["pull_request"] =
            json!({"url": format!("{base}/repos/{}/pulls/{}", repo.full_name, issue.number)});
    }
    value
}

fn comment_json(repo: &Repo, number: u64, comment: &StoredComment, base: &str) -> Value {
    json!({
        "id": comment.id,
        "body": comment.body,
        "html_url": format!("https://github.test/{}/issues/{number}#issuecomment-{}", repo.full_name, comment.id),
        "url": format!("{base}/repos/{}/issues/comments/{}", repo.full_name, comment.id),
        "user": {"login": "fake-bot"},
    })
}

#[cfg(test)]
mod tests {
    use super::super::server::Fault;
    use super::*;
    use crate::http::{ErrorKind, HttpClient, Request};
    use std::time::Duration;
    use store::Failure;

    const REPO: &str = "owner/rover";

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

    fn fake() -> FakeGithub {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token("t0ken");
        gh
    }

    fn url(gh: &FakeGithub, path: &str) -> String {
        format!("{}/repos/{REPO}{path}", gh.api_base())
    }

    fn get(gh: &FakeGithub, path: &str) -> Request {
        Request::get(url(gh, path)).bearer("t0ken")
    }

    fn post(gh: &FakeGithub, path: &str, body: Value) -> Request {
        Request::post_json(url(gh, path), &body).bearer("t0ken")
    }

    fn patch(gh: &FakeGithub, path: &str, body: Value) -> Request {
        Request::patch_json(url(gh, path), &body).bearer("t0ken")
    }

    fn create(gh: &FakeGithub, title: &str, labels: &[&str]) -> Value {
        client()
            .send(&post(
                gh,
                "/issues",
                json!({"title": title, "labels": labels}),
            ))
            .unwrap()
            .json()
            .unwrap()
    }

    #[test]
    fn it_creates_an_issue_with_labels_and_assignees() {
        let gh = fake();
        let r = client()
            .send(&post(
                &gh,
                "/issues",
                json!({"title": "Order the battery", "body": "Due: 2026-11-20",
                       "labels": ["cycle:NOMAD"], "assignees": ["alex-example"]}),
            ))
            .unwrap();
        assert_eq!(r.status, 201);
        let v = r.json().unwrap();
        assert_eq!(v["number"], 1);
        assert_eq!(v["state"], "open");
        assert_eq!(v["labels"][0]["name"], "cycle:NOMAD");
        assert_eq!(v["assignee"]["login"], "alex-example");
        assert!(v["html_url"]
            .as_str()
            .unwrap()
            .ends_with("/owner/rover/issues/1"));
        assert_eq!(gh.labels(REPO), ["cycle:NOMAD"]);
        let stored = gh.issue(REPO, 1).unwrap();
        assert_eq!(stored.body, "Due: 2026-11-20");
        assert_eq!(create(&gh, "Second", &[])["number"], 2);
    }

    #[test]
    fn creating_needs_a_title_and_a_json_body() {
        let gh = fake();
        let err = client()
            .send(&post(&gh, "/issues", json!({"title": "  "})))
            .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(422), Failure::Permanent));
        assert!(err.message.contains("Validation Failed"));
        let bad = Request::new(crate::http::Method::Post, url(&gh, "/issues"))
            .bytes("application/json", b"{nope".to_vec())
            .bearer("t0ken");
        assert_eq!(client().send(&bad).unwrap_err().status, Some(400));
        assert!(gh.issues(REPO).is_empty());
    }

    #[test]
    fn assignees_who_cannot_be_assigned_are_dropped_silently() {
        let gh = fake();
        gh.restrict_assignees(REPO, &["alex-example"]);
        let r = client()
            .send(&post(
                &gh,
                "/issues",
                json!({"title": "T", "assignees": ["stranger", "alex-example"]}),
            ))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(r["assignees"].as_array().unwrap().len(), 1);
        assert_eq!(r["assignee"]["login"], "alex-example");
    }

    #[test]
    fn closing_and_reopening_set_the_state_reason() {
        let gh = fake();
        create(&gh, "Mount", &[]);
        let closed = client()
            .send(&patch(&gh, "/issues/1", json!({"state": "closed"})))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(
            (closed["state"].as_str(), closed["state_reason"].as_str()),
            (Some("closed"), Some("completed"))
        );
        let reopened = client()
            .send(&patch(&gh, "/issues/1", json!({"state": "open"})))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(reopened["state_reason"], "reopened");
        let not_planned = client()
            .send(&patch(
                &gh,
                "/issues/1",
                json!({"state": "closed", "state_reason": "not_planned"}),
            ))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(not_planned["state_reason"], "not_planned");
        let err = client()
            .send(&patch(&gh, "/issues/1", json!({"state": "gone"})))
            .unwrap_err();
        assert_eq!(err.status, Some(422));
        assert!(!gh.issue(REPO, 1).unwrap().open);
    }

    #[test]
    fn a_patch_changes_only_what_it_names() {
        let gh = fake();
        create(&gh, "Old title", &["a", "b"]);
        client()
            .send(&patch(&gh, "/issues/1", json!({"title": "New title"})))
            .unwrap();
        let s = gh.issue(REPO, 1).unwrap();
        assert_eq!((s.title.as_str(), s.labels.len()), ("New title", 2));
        client()
            .send(&patch(
                &gh,
                "/issues/1",
                json!({"labels": ["c"], "body": "text"}),
            ))
            .unwrap();
        let s = gh.issue(REPO, 1).unwrap();
        assert_eq!((s.labels, s.body.as_str()), (vec!["c".to_string()], "text"));
        assert_eq!(
            client()
                .send(&patch(&gh, "/issues/9", json!({})))
                .unwrap_err()
                .status,
            Some(404)
        );
    }

    #[test]
    fn comments_are_stored_and_listed() {
        let gh = fake();
        create(&gh, "Mount", &[]);
        let c = client()
            .send(&post(
                &gh,
                "/issues/1/comments",
                json!({"body": "Done in the meeting."}),
            ))
            .unwrap();
        assert_eq!(c.status, 201);
        assert_eq!(c.json().unwrap()["id"], 1);
        let list = client()
            .send(&get(&gh, "/issues/1/comments"))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(list[0]["body"], "Done in the meeting.");
        assert_eq!(gh.issue(REPO, 1).unwrap().comments.len(), 1);
        assert_eq!(
            client()
                .send(&get(&gh, "/issues/1"))
                .unwrap()
                .json()
                .unwrap()["comments"],
            1
        );
        let err = client()
            .send(&post(&gh, "/issues/1/comments", json!({"body": ""})))
            .unwrap_err();
        assert_eq!(err.status, Some(422));
        assert_eq!(
            client()
                .send(&post(&gh, "/issues/5/comments", json!({"body": "x"})))
                .unwrap_err()
                .status,
            Some(404)
        );
    }

    #[test]
    fn the_list_filters_by_state_and_labels_and_includes_pull_requests() {
        let gh = fake();
        gh.add_issue(REPO, "One", &["cycle:NOMAD"]);
        gh.add_issue(REPO, "Two", &["cycle:NOMAD", "hw"]);
        gh.add_issue(REPO, "Other cycle", &["cycle:OTHER"]);
        gh.add_pull_request(REPO, "A pull request", &["cycle:NOMAD"]);
        gh.close_issue(REPO, 1);
        let titles = |path: &str| -> Vec<String> {
            client()
                .send(&get(&gh, path))
                .unwrap()
                .json()
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["title"].as_str().unwrap().to_string())
                .collect()
        };
        // Newest first by default; the label may be percent-encoded as the client sends it.
        assert_eq!(
            titles("/issues?labels=cycle%3ANOMAD"),
            ["A pull request", "Two"]
        );
        assert_eq!(titles("/issues?state=closed&labels=cycle:NOMAD"), ["One"]);
        assert_eq!(
            titles("/issues?state=all&labels=cycle:NOMAD&direction=asc"),
            ["One", "Two", "A pull request"]
        );
        assert_eq!(titles("/issues?labels=cycle:NOMAD,hw"), ["Two"]);
        let all = client()
            .send(&get(&gh, "/issues?state=all"))
            .unwrap()
            .json()
            .unwrap();
        assert!(all
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i.get("pull_request").is_some()));
        assert_eq!(
            client()
                .send(&get(&gh, "/issues?state=weird"))
                .unwrap_err()
                .status,
            Some(422)
        );
    }

    #[test]
    fn sorting_by_update_puts_recently_touched_issues_first() {
        let gh = fake();
        for t in ["a", "b", "c"] {
            gh.add_issue(REPO, t, &[]);
        }
        client()
            .send(&post(&gh, "/issues/1/comments", json!({"body": "bump"})))
            .unwrap();
        let first = |path: &str| {
            client().send(&get(&gh, path)).unwrap().json().unwrap()[0]["title"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(first("/issues?sort=updated"), "a");
        assert_eq!(first("/issues?sort=created"), "c");
    }

    #[test]
    fn pages_are_cut_at_per_page_with_a_link_to_the_next() {
        let gh = fake();
        for n in 0..5 {
            gh.add_issue(REPO, &format!("Issue {n}"), &[]);
        }
        let first = client()
            .send(&get(&gh, "/issues?per_page=2&page=1"))
            .unwrap();
        assert_eq!(first.json().unwrap().as_array().unwrap().len(), 2);
        let link = first.header("link").unwrap();
        assert!(
            link.contains("page=2")
                && link.contains("per_page=2")
                && link.ends_with("rel=\"next\"")
        );
        let last = client()
            .send(&get(&gh, "/issues?per_page=2&page=3"))
            .unwrap();
        assert_eq!(last.json().unwrap().as_array().unwrap().len(), 1);
        assert!(last.header("link").is_none());
        let beyond = client()
            .send(&get(&gh, "/issues?per_page=2&page=9"))
            .unwrap();
        assert!(beyond.json().unwrap().as_array().unwrap().is_empty());
        // A page size over 100 is cut to 100.
        for n in 5..130 {
            gh.add_issue(REPO, &format!("Issue {n}"), &[]);
        }
        let big = client().send(&get(&gh, "/issues?per_page=500")).unwrap();
        assert_eq!(big.json().unwrap().as_array().unwrap().len(), 100);
    }

    #[test]
    fn the_repository_reports_its_visibility_and_owner() {
        let gh = fake();
        let info = |gh: &FakeGithub| client().send(&get(gh, "")).unwrap().json().unwrap();
        let v = info(&gh);
        assert_eq!(
            (v["private"].as_bool(), v["visibility"].as_str()),
            (Some(true), Some("private"))
        );
        assert_eq!(v["owner"]["type"], "User");
        gh.set_visibility(REPO, Visibility::Public);
        gh.set_organization(REPO);
        let v = info(&gh);
        assert_eq!(
            (v["private"].as_bool(), v["visibility"].as_str()),
            (Some(false), Some("public"))
        );
        assert_eq!(v["owner"]["type"], "Organization");
        gh.set_visibility(REPO, Visibility::Internal);
        assert_eq!(info(&gh)["private"], true);
    }

    #[test]
    fn a_private_repository_does_not_exist_for_anonymous_clients() {
        let gh = fake();
        let anonymous = Request::get(url(&gh, ""));
        assert_eq!(client().send(&anonymous).unwrap_err().status, Some(404));
        gh.set_visibility(REPO, Visibility::Public);
        assert_eq!(client().send(&anonymous).unwrap().status, 200);
        // Reading issues of a public repository needs no token, writing does.
        assert!(client().send(&Request::get(url(&gh, "/issues"))).is_ok());
        let write = Request::post_json(url(&gh, "/issues"), &json!({"title": "x"}));
        assert_eq!(client().send(&write).unwrap_err().status, Some(401));
        assert!(gh.issues(REPO).is_empty());
    }

    #[test]
    fn a_wrong_token_is_unauthorized_and_permanent() {
        let gh = fake();
        let err = client()
            .send(&Request::get(url(&gh, "")).bearer("wrong"))
            .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(401), Failure::Permanent));
        assert!(err.message.contains("Bad credentials"));
        // GitHub also accepts the older `token` scheme.
        let old = Request::get(url(&gh, "")).header("Authorization", "token t0ken");
        assert!(client().send(&old).is_ok());
    }

    #[test]
    fn the_user_endpoint_checks_the_token() {
        let gh = fake();
        let ok = client()
            .send(&Request::get(format!("{}/user", gh.api_base())).bearer("t0ken"))
            .unwrap();
        assert_eq!(ok.json().unwrap()["login"], "fake-bot");
        let anonymous = client()
            .send(&Request::get(format!("{}/user", gh.api_base())))
            .unwrap_err();
        assert_eq!(anonymous.status, Some(401));
    }

    #[test]
    fn unknown_repositories_and_paths_are_404_and_a_missing_user_agent_is_403() {
        let gh = fake();
        let other = format!("{}/repos/owner/missing", gh.api_base());
        assert_eq!(
            client()
                .send(&Request::get(other).bearer("t0ken"))
                .unwrap_err()
                .status,
            Some(404)
        );
        assert_eq!(
            client().send(&get(&gh, "/unknown")).unwrap_err().status,
            Some(404)
        );
        // The client always adds a User-Agent; a raw request without one is refused.
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(gh.addr()).unwrap();
        s.write_all(
            b"GET /repos/owner/rover HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t0ken\r\n\r\n",
        )
        .unwrap();
        let mut text = String::new();
        s.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 403"), "{text}");
    }

    #[test]
    fn disabled_issues_answer_410_which_is_permanent() {
        let gh = fake();
        gh.disable_issues(REPO);
        let err = client()
            .send(&post(&gh, "/issues", json!({"title": "x"})))
            .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(410), Failure::Permanent));
        assert_eq!(
            client().send(&get(&gh, "/issues")).unwrap_err().status,
            Some(410)
        );
        // The repository itself still answers.
        assert!(client().send(&get(&gh, "")).is_ok());
    }

    #[test]
    fn a_person_closing_an_issue_is_visible_to_the_next_listing() {
        let gh = fake();
        gh.add_issue(REPO, "Mount", &["cycle:NOMAD"]);
        let open = client()
            .send(&get(&gh, "/issues?labels=cycle:NOMAD"))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(open.as_array().unwrap().len(), 1);
        gh.close_issue(REPO, 1);
        let open = client()
            .send(&get(&gh, "/issues?labels=cycle:NOMAD"))
            .unwrap()
            .json()
            .unwrap();
        assert!(open.as_array().unwrap().is_empty());
        let all = client()
            .send(&get(&gh, "/issues?state=all&labels=cycle:NOMAD"))
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(all[0]["state"], "closed");
        gh.reopen_issue(REPO, 1);
        assert!(gh.issue(REPO, 1).unwrap().open);
    }

    #[test]
    fn rate_limit_replies_classify_as_the_publisher_will_see_them() {
        let gh = fake();
        gh.inject(Fault::Reply(secondary_rate_limit(45)), 1);
        let err = client().send(&get(&gh, "/issues")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::RateLimited);
        assert_eq!(
            err.failure,
            Failure::Transient {
                retry_after_ms: Some(45_000)
            }
        );

        // The primary limit gives the epoch second of the reset.
        let reset = 2_000_000_000u64;
        gh.inject(Fault::Reply(primary_rate_limit(reset)), 1);
        let clock = || 1_999_999_950_000;
        let err = client()
            .with_clock(clock)
            .send(&get(&gh, "/issues"))
            .unwrap_err();
        assert_eq!(
            err.failure,
            Failure::Transient {
                retry_after_ms: Some(50_000)
            }
        );
        assert!(client().send(&get(&gh, "/issues")).is_ok());
    }

    #[test]
    fn an_ambiguous_failure_leaves_the_issue_behind() {
        // The effect happened, the answer was lost: the situation the marker scan resolves.
        let gh = fake();
        gh.inject_for("POST", "/repos/owner/rover/issues", Fault::DropAfter, 1);
        let err = client()
            .send(&post(&gh, "/issues", json!({"title": "Mount"})))
            .unwrap_err();
        assert!(err.is_offline());
        assert_eq!(gh.issues(REPO).len(), 1);
    }

    #[test]
    fn being_offline_is_a_connection_error_and_recovery_works() {
        let gh = fake();
        gh.go_offline();
        let err = client().send(&get(&gh, "/issues")).unwrap_err();
        assert!(err.is_offline());
        gh.go_online();
        assert!(client().send(&get(&gh, "/issues")).is_ok());
    }
}
