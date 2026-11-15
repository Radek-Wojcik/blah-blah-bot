//! Issues: creating one per task item, writing a status change back, and reading the state of
//! the cycle's issues.
//!
//! * An issue carries the label `cycle:<id>`, the participant's GitHub login as assignee
//!   (GitHub drops a login that cannot be assigned; the outcome says so), the due date, a
//!   link to the report and to the passage of the transcript, the quote, and a hidden marker
//!   `<!-- bbb:v1 cycle=.. meeting=.. item=.. t=.. -->` that ties it to its item.
//! * **No duplicates.** A request that was sent but whose answer was lost may have created
//!   the issue. A retry therefore looks for the marker among the cycle's issues first
//!   (`scan_first`) and reuses what it finds. The list can lag behind a write by a moment,
//!   so a duplicate remains possible in that window.
//! * **Due date.** For a repository of an organisation that has an issue field named
//!   [`DUE_FIELD_NAME`] of type `date` the date is written to that field. Everything else
//!   (a personal repository, no such field, any error) gets a `Due: YYYY-MM-DD` line in the
//!   body. The field endpoints are the documented ones (`GET /orgs/{org}/issue-fields`,
//!   `PUT /repos/{o}/{r}/issues/{n}/issue-field-values`); their preview status and token
//!   permission could not be checked against a real server, which is why the line is the
//!   fallback for every failure.
//! * **Status.** Closing or reopening is a `PATCH` (skipped when the issue is already in that
//!   state) followed by a comment that carries a hidden marker; a retry finds the marker and
//!   does not comment twice.

use std::time::Duration;

use cycle_store::issues::{parse_issue_record, IssueRecord, ItemMarker, StatusMarker, MAX_PAGES};
use serde_json::{json, Value};

use super::{Github, RepoRef};
use crate::http::{ErrorKind, HttpError, Request};

/// The name of the organisation issue field a due date goes to.
pub const DUE_FIELD_NAME: &str = "Target date";
/// GitHub asks clients that create content in bulk to pace themselves; the dispatcher waits
/// this long between two writes to the same repository.
pub const WRITE_PACING: Duration = Duration::from_secs(1);
/// The API version the issue field endpoints are documented for.
const FIELDS_API_VERSION: &str = "2026-03-10";
const PER_PAGE: usize = 100;
const TITLE_CHARS: usize = 200;

fn invalid(message: impl Into<String>) -> HttpError {
    HttpError {
        kind: ErrorKind::InvalidRequest,
        message: message.into(),
        status: None,
        failure: store::Failure::Permanent,
    }
}

// ----- text ---------------------------------------------------------------------------------------

/// Keeps `@name` in text from the transcript from notifying anybody: a zero width space
/// after the `@` stops GitHub from reading it as a mention.
pub fn defuse_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '@'
            && chars
                .peek()
                .is_some_and(|n| n.is_alphanumeric() || *n == '_')
        {
            out.push('\u{200b}');
        }
    }
    out
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn one_line(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{}...", cut.trim_end())
}

fn quote_block(quote: &str) -> String {
    quote
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("> {}", defuse_mentions(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The place of the meeting a comment or an issue points back to:
/// `From [meeting 2026-11-13](report) at [00:12:25](transcript#anchor)`.
fn origin_line(
    date: &str,
    report_url: Option<&str>,
    transcript_url: Option<&str>,
    anchor: Option<&str>,
    time_ms: Option<u64>,
) -> String {
    let meeting = match report_url {
        Some(url) => format!("[meeting {date}]({url})"),
        None => format!("meeting {date}"),
    };
    let at = time_ms.map(|t| {
        let time = clock(t);
        match (transcript_url, anchor) {
            (Some(url), Some(a)) => format!("[{time}]({url}#{a})"),
            (Some(url), None) => format!("[{time}]({url})"),
            _ => time,
        }
    });
    match at {
        Some(at) => format!("From {meeting} at {at}"),
        None => format!("From {meeting}"),
    }
}

// ----- creating an issue ----------------------------------------------------------------------

/// What an issue is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSpec {
    pub cycle_id: String,
    pub meeting_id: String,
    pub item_id: String,
    pub title: String,
    /// The GitHub login of the assignee.
    pub assignee: Option<String>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    /// `YYYY-MM-DD`, the day of the meeting.
    pub meeting_date: String,
    /// Address of the published report.
    pub report_url: Option<String>,
    /// Address of the published `transcript.md`.
    pub transcript_url: Option<String>,
    /// `t-hh-mm-ss`, from `queue::transcript::anchor_at`.
    pub transcript_anchor: Option<String>,
    /// Where in the recording the item was said.
    pub time_ms: Option<u64>,
    pub quote: Option<String>,
}

impl IssueSpec {
    fn marker(&self) -> ItemMarker {
        ItemMarker {
            cycle: self.cycle_id.clone(),
            meeting: self.meeting_id.clone(),
            item: self.item_id.clone(),
            t_ms: self.time_ms,
        }
    }
}

/// The body of an issue. `due_line` writes the due date as `Due: YYYY-MM-DD` (when the date
/// goes to an issue field it is left out).
pub fn issue_body(spec: &IssueSpec, due_line: bool) -> Result<String, HttpError> {
    let marker = spec
        .marker()
        .to_comment()
        .ok_or_else(|| invalid("the item cannot be linked to an issue: an id is not usable"))?;
    let mut parts: Vec<String> = Vec::new();
    if let (true, Some(due)) = (due_line, &spec.due) {
        parts.push(format!("Due: {due}"));
    }
    parts.push(origin_line(
        &spec.meeting_date,
        spec.report_url.as_deref(),
        spec.transcript_url.as_deref(),
        spec.transcript_anchor.as_deref(),
        spec.time_ms,
    ));
    if let Some(q) = spec
        .quote
        .as_deref()
        .map(quote_block)
        .filter(|q| !q.is_empty())
    {
        parts.push(q);
    }
    parts.push(marker);
    Ok(parts.join("\n\n") + "\n")
}

/// How the due date was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueOutcome {
    /// The item has no due date.
    NotRequested,
    /// To the organisation's issue field.
    Field,
    /// As a `Due:` line in the body.
    Line,
    /// Neither worked (the issue exists without a date).
    Missing,
    /// The issue was already there; its date was not touched.
    Existing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueOutcome {
    pub number: u64,
    pub url: String,
    /// False when the issue was found by its marker instead of being created.
    pub created: bool,
    /// The login that ended up assigned.
    pub assigned: Option<String>,
    /// An assignee was asked for and GitHub did not take it (not a collaborator).
    pub assignee_dropped: bool,
    pub due: DueOutcome,
}

fn label_of(cycle_id: &str) -> Result<String, HttpError> {
    let ok = !cycle_id.is_empty()
        && cycle_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if ok {
        Ok(format!("cycle:{cycle_id}"))
    } else {
        Err(invalid("the cycle id is not usable as a label"))
    }
}

fn login_of(user: &Value) -> Option<&str> {
    user.get("login").and_then(Value::as_str)
}

impl Github {
    fn repo_path(&self, repo: &RepoRef) -> String {
        format!("{}/repos/{}/{}", self.api, repo.owner, repo.name)
    }

    fn send_json(&self, request: Request) -> Result<Value, HttpError> {
        self.client.send(&self.authorised(request))?.json()
    }

    // ----- reading ---------------------------------------------------------------------------

    /// The cycle's issues of every state, most recently updated first, over at most
    /// `max_pages` pages of 100 (never more than [`MAX_PAGES`]). Pull requests are left out.
    pub fn list_cycle_issues(
        &self,
        repo: &RepoRef,
        cycle_id: &str,
        max_pages: u32,
    ) -> Result<Vec<IssueRecord>, HttpError> {
        let label = label_of(cycle_id)?;
        let mut out = Vec::new();
        for page in 1..=max_pages.clamp(1, MAX_PAGES) {
            let url = format!(
                "{}/issues?state=all&labels={}&sort=updated&direction=desc&per_page={PER_PAGE}&page={page}",
                self.repo_path(repo),
                label.replace(':', "%3A")
            );
            let value = self.send_json(Request::get(url))?;
            let list = value
                .as_array()
                .ok_or_else(|| invalid("the list of issues is malformed"))?;
            out.extend(list.iter().filter_map(parse_issue_record));
            if list.len() < PER_PAGE {
                break;
            }
        }
        Ok(out)
    }

    /// The issue made from this item, found by its marker among the most recent issues of
    /// the cycle.
    pub fn find_issue_by_marker(
        &self,
        repo: &RepoRef,
        marker: &ItemMarker,
    ) -> Result<Option<IssueRecord>, HttpError> {
        let found = self
            .list_cycle_issues(repo, &marker.cycle, 2)?
            .into_iter()
            .find(|r| {
                r.marker.as_ref().is_some_and(|m| {
                    m.cycle == marker.cycle && m.meeting == marker.meeting && m.item == marker.item
                })
            });
        Ok(found)
    }

    // ----- the due date field ------------------------------------------------------------------

    /// The id of the due date field of the organisation that owns `repo`, if it has one.
    /// A personal repository, a missing field and every refusal give `None`; a network
    /// failure gives `None` too but is asked again next time.
    fn due_field(&self, repo: &RepoRef) -> Option<u64> {
        let key = repo.owner.to_ascii_lowercase();
        if let Some(known) = self
            .due_fields
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return *known;
        }
        let url = format!("{}/orgs/{}/issue-fields", self.api, repo.owner);
        let request = Request::get(url).header("X-GitHub-Api-Version", FIELDS_API_VERSION);
        let (answer, definitive) = match self.client.send(&self.authorised_with_version(request)) {
            Ok(response) => (response.json().ok(), true),
            Err(e) => (
                None,
                e.status
                    .is_some_and(|s| (400..500).contains(&s) && s != 429 && s != 408),
            ),
        };
        let id = answer.as_ref().and_then(|v| {
            v.as_array()?.iter().find_map(|f| {
                let named = f
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.eq_ignore_ascii_case(DUE_FIELD_NAME));
                let date = f.get("data_type").and_then(Value::as_str) == Some("date");
                (named && date)
                    .then(|| f.get("id").and_then(Value::as_u64))
                    .flatten()
            })
        });
        if definitive {
            self.due_fields
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, id);
        }
        id
    }

    /// Like `authorised`, for a request that sets its own `X-GitHub-Api-Version`.
    fn authorised_with_version(&self, request: Request) -> Request {
        self.with_token(request.header("Accept", "application/vnd.github+json"))
    }

    // ----- creating -----------------------------------------------------------------------------

    /// Creates the issue of an item (or, with `scan_first`, reuses the one a lost answer
    /// left behind). Set `scan_first` on every attempt but the first.
    pub fn publish_issue(
        &self,
        repo: &RepoRef,
        spec: &IssueSpec,
        scan_first: bool,
    ) -> Result<IssueOutcome, HttpError> {
        let label = label_of(&spec.cycle_id)?;
        let title = one_line(&defuse_mentions(&spec.title), TITLE_CHARS);
        if title.is_empty() {
            return Err(invalid("the issue has no title"));
        }
        if scan_first {
            if let Some(found) = self.find_issue_by_marker(repo, &spec.marker())? {
                return Ok(IssueOutcome {
                    number: found.number,
                    url: found.url.unwrap_or_default(),
                    created: false,
                    assigned: found.assignee,
                    assignee_dropped: false,
                    due: DueOutcome::Existing,
                });
            }
        }
        // Where the due date goes: the field when the organisation has one, else the body.
        let field = spec.due.as_ref().and_then(|_| self.due_field(repo));
        let due_line = spec.due.is_some() && field.is_none();
        let mut body = json!({
            "title": title,
            "body": issue_body(spec, due_line)?,
            "labels": [label],
        });
        if let Some(login) = &spec.assignee {
            body["assignees"] = json!([login]);
        }
        let created = self.send_json(Request::post_json(
            format!("{}/issues", self.repo_path(repo)),
            &body,
        ))?;
        let number = created
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("the answer of GitHub has no issue number"))?;
        let url = created
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let assigned = created
            .get("assignees")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(login_of)
            .map(String::from);
        let assignee_dropped = spec.assignee.is_some() && assigned.is_none();

        let due = match (&spec.due, field) {
            (None, _) => DueOutcome::NotRequested,
            (Some(_), None) => DueOutcome::Line,
            (Some(date), Some(field_id)) => {
                if self.set_due_field(repo, number, field_id, date).is_ok() {
                    DueOutcome::Field
                } else {
                    // The field did not take it: write the line after all.
                    let with_line = issue_body(spec, true)?;
                    let patch = Request::patch_json(
                        format!("{}/issues/{number}", self.repo_path(repo)),
                        &json!({ "body": with_line }),
                    );
                    if self.send_json(patch).is_ok() {
                        DueOutcome::Line
                    } else {
                        DueOutcome::Missing
                    }
                }
            }
        };
        Ok(IssueOutcome {
            number,
            url,
            created: true,
            assigned,
            assignee_dropped,
            due,
        })
    }

    fn set_due_field(
        &self,
        repo: &RepoRef,
        number: u64,
        field_id: u64,
        date: &str,
    ) -> Result<(), HttpError> {
        let url = format!(
            "{}/issues/{number}/issue-field-values",
            self.repo_path(repo)
        );
        let body = json!({"issue_field_values": [{"field_id": field_id, "value": date}]});
        let request = Request::new(crate::http::Method::Put, url)
            .json(&body)
            .header("X-GitHub-Api-Version", FIELDS_API_VERSION);
        self.client.send(&self.authorised_with_version(request))?;
        Ok(())
    }

    // ----- status write-back ---------------------------------------------------------------------

    /// Closes (done) or reopens an issue and comments why, once. See the module notes.
    pub fn set_issue_status(
        &self,
        repo: &RepoRef,
        number: u64,
        spec: &StatusSpec,
    ) -> Result<StatusOutcome, HttpError> {
        let marker = StatusMarker {
            meeting: spec.meeting_id.clone(),
            proposal: spec.proposal_id.clone(),
        };
        let marker_comment = marker
            .to_comment()
            .ok_or_else(|| invalid("the status change cannot be linked: an id is not usable"))?;
        let issue_url = format!("{}/issues/{number}", self.repo_path(repo));

        let current = self.send_json(Request::get(issue_url.clone()))?;
        let is_open = current.get("state").and_then(Value::as_str) != Some("closed");
        let changed = is_open != spec.open;
        if changed {
            let body = if spec.open {
                json!({"state": "open", "state_reason": "reopened"})
            } else {
                json!({"state": "closed", "state_reason": "completed"})
            };
            self.send_json(Request::patch_json(issue_url, &body))?;
        }

        // Has an earlier attempt commented already?
        let mut commented_before = false;
        for page in 1..=MAX_PAGES {
            let url = format!(
                "{}/issues/{number}/comments?per_page={PER_PAGE}&page={page}",
                self.repo_path(repo)
            );
            let value = self.send_json(Request::get(url))?;
            let list = value.as_array().cloned().unwrap_or_default();
            if list.iter().any(|c| {
                c.get("body")
                    .and_then(Value::as_str)
                    .and_then(StatusMarker::parse)
                    .is_some_and(|m| m == marker)
            }) {
                commented_before = true;
                break;
            }
            if list.len() < PER_PAGE {
                break;
            }
        }
        if !commented_before {
            let text = format!("{}\n\n{marker_comment}\n", spec.comment_text());
            self.send_json(Request::post_json(
                format!("{}/issues/{number}/comments", self.repo_path(repo)),
                &json!({ "body": text }),
            ))?;
        }
        Ok(StatusOutcome {
            changed,
            commented: !commented_before,
        })
    }
}

/// A status change to write back to an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSpec {
    pub meeting_id: String,
    /// The id of the proposal (`s2`).
    pub proposal_id: String,
    /// True reopens, false closes as done.
    pub open: bool,
    /// Why, in the words of the meeting; the user may have edited it.
    pub reason: String,
    pub quote: Option<String>,
    pub meeting_date: String,
    pub report_url: Option<String>,
    pub transcript_url: Option<String>,
    pub transcript_anchor: Option<String>,
    pub time_ms: Option<u64>,
}

impl StatusSpec {
    /// The visible part of the comment.
    fn comment_text(&self) -> String {
        let verb = if self.open {
            "Reopened"
        } else {
            "Marked as done"
        };
        let mut parts = vec![format!(
            "{verb} after the meeting of {}: {}",
            self.meeting_date,
            one_line(&defuse_mentions(&self.reason), 500)
        )];
        if let Some(q) = self
            .quote
            .as_deref()
            .map(quote_block)
            .filter(|q| !q.is_empty())
        {
            parts.push(q);
        }
        parts.push(origin_line(
            &self.meeting_date,
            self.report_url.as_deref(),
            self.transcript_url.as_deref(),
            self.transcript_anchor.as_deref(),
            self.time_ms,
        ));
        parts.join("\n\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusOutcome {
    /// The state was changed (false: the issue was in that state already).
    pub changed: bool,
    /// A comment was posted (false: an earlier attempt had posted it).
    pub commented: bool,
}

#[cfg(test)]
mod tests;
