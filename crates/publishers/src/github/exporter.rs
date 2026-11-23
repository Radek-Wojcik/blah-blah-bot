//! GitHub issues seen through the [`TaskExporter`] interface.
//!
//! [`GithubIssues`] is a read-only adapter over the issue functions that exist: `find` is the
//! marker scan of the cycle's issues, `get` fetches an issue by its number, `create` is
//! [`Github::publish_issue_tracked`] (the one place an issue is written) and `update` never
//! writes: GitHub issues are not edited after they are created, so an update is the issue as
//! it is. A closed issue is not "removed" either: it stays the issue of its item.
//!
//! The dispatcher keeps its own `create_issue` path; this adapter exists so that the shared
//! [`upsert`](crate::export::upsert) algorithm and the tests cover both task targets the same
//! way.

use cycle_store::{parse_issue_record, IssueRecord};

use super::{Github, IssueSpec, RepoRef};
use crate::export::{permanent_error, ExternalTask, TaskExporter, TaskKey, TaskRecord};
use crate::http::{ErrorKind, HttpError, Request};

/// Pages of the cycle's issues a lookup by key reads (the most recently updated 200).
const SCAN_PAGES: u32 = 2;

/// The issues of one repository as a task exporter.
pub struct GithubIssues<'a> {
    github: &'a Github,
    repo: &'a RepoRef,
}

impl<'a> GithubIssues<'a> {
    pub fn new(github: &'a Github, repo: &'a RepoRef) -> Self {
        Self { github, repo }
    }

    fn issue_url(&self, number: u64) -> String {
        format!(
            "{}/repos/{}/{}/issues/{number}",
            self.github.api, self.repo.owner, self.repo.name
        )
    }

    /// The issue with this number; `None` when it is gone or is a pull request.
    fn fetch(&self, number: u64) -> Result<Option<IssueRecord>, HttpError> {
        let request = self.github.authorised(Request::get(self.issue_url(number)));
        match self.github.client.send(&request) {
            Ok(response) => Ok(parse_issue_record(&response.json()?)),
            Err(e) if matches!(e.status, Some(404 | 410)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn external(record: &IssueRecord) -> ExternalTask {
    ExternalTask {
        id: record.number.to_string(),
        url: record.url.clone(),
        trashed: false,
    }
}

fn number_of(id: &str) -> Result<u64, HttpError> {
    id.trim()
        .parse()
        .map_err(|_| permanent_error(ErrorKind::InvalidRequest, "an issue id is a number"))
}

/// What an issue is made of, from the task.
fn spec_of(task: &TaskRecord) -> IssueSpec {
    IssueSpec {
        cycle_id: task.key.cycle.clone(),
        meeting_id: task.key.meeting.clone(),
        item_id: task.key.item.clone(),
        title: task.title.clone(),
        assignee: task.assignee.as_ref().and_then(|a| a.github.clone()),
        due: task.due.clone(),
        meeting_date: task.meeting_date.clone(),
        report_url: task.report_url.clone(),
        transcript_url: task.transcript_url.clone(),
        transcript_anchor: task.transcript_anchor.clone(),
        time_ms: task.time_ms,
        quote: task.quote.clone(),
    }
}

impl TaskExporter for GithubIssues<'_> {
    fn name(&self) -> &'static str {
        "github"
    }

    fn noun(&self) -> &'static str {
        "issue"
    }

    fn get(&self, id: &str) -> Result<Option<ExternalTask>, HttpError> {
        Ok(self.fetch(number_of(id)?)?.as_ref().map(external))
    }

    fn find(&self, key: &TaskKey) -> Result<Vec<ExternalTask>, HttpError> {
        let mut found: Vec<IssueRecord> = self
            .github
            .list_cycle_issues(self.repo, &key.cycle, SCAN_PAGES)?
            .into_iter()
            .filter(|r| {
                r.marker.as_ref().is_some_and(|m| {
                    m.cycle == key.cycle && m.meeting == key.meeting && m.item == key.item
                })
            })
            .collect();
        found.sort_by_key(|r| r.number);
        Ok(found.iter().map(external).collect())
    }

    fn create(&self, task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        let outcome = self.github.publish_issue_tracked(
            self.repo,
            &spec_of(task),
            false,
            None,
            &mut |_| {},
        )?;
        Ok(ExternalTask {
            id: outcome.number.to_string(),
            url: Some(outcome.url).filter(|u| !u.is_empty()),
            trashed: false,
        })
    }

    /// Issues are not edited: the answer is the issue as it is.
    fn update(&self, id: &str, _task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        self.get(id)?.ok_or_else(|| {
            let mut err = permanent_error(
                ErrorKind::Status,
                "the issue was deleted or is no longer shared with the integration",
            );
            err.status = Some(404);
            err
        })
    }
}

#[cfg(test)]
mod tests;
