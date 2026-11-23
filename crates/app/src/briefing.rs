//! The briefing screen: open tasks by person and open topics of a cycle, before a meeting.
//!
//! Where the tasks come from is behind the [`BriefingSource`] trait. Today that is
//! [`CycleStoreSource`]: the open issues of the cache (`.git/bbb-issues-<ID>.json`), the task
//! items of the `meeting.json` files of earlier meetings and `topics.yaml`, all read from the
//! local clone. When Stage 5 reads issues back from GitHub and writes status changes, it
//! brings a second `BriefingSource` (the issues with their real state and due dates) and
//! `AppState::open` builds that one instead: a one-line swap. Nothing else changes, because
//! the grouping, the overdue marks and the screen work on [`TaskFeed`] and [`TopicFeed`] only.
//!
//! The command `get_briefing(cycleId, refresh)` returns the whole screen as one value. With
//! `refresh`, a cloned repository is pulled first (the same `git pull --rebase` as before the
//! extraction, see ADR 0017: never a stash, nothing is pulled while cycle files have
//! uncommitted edits, a failure leaves the clone untouched). The UI asks once without
//! `refresh`, so that the last known state shows at once, and once with it.

use cycle_store::schema::is_identifier;
use cycle_store::time::{format_utc_date, now_unix_ms};
use cycle_store::{
    group_tasks, meeting_tasks, merge_tasks, open_topics, GroupKind, IssueCache, PullOutcome,
    TaskEntry, TaskGroup, TaskItem, TaskOrigin, Topic,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::cycle_commands::EVENT_CYCLES;
use crate::cycles::{self, TopicDto};
use crate::ipc::{IpcError, IpcResult};
use crate::settings::{CycleRepoSetting, RepoKind};
use crate::state::AppState;

// ----- the source ------------------------------------------------------------------------

/// The tasks of a cycle as a source knows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFeed {
    /// Tasks still to do, with duplicates already merged (a meeting task that points to an
    /// issue appears once).
    pub open: Vec<TaskItem>,
    /// Tasks known to be done; the screen lists them apart.
    pub done: Vec<TaskItem>,
    /// `YYYY-MM-DD` of the oldest data the feed is made of; `None` for data that has no age,
    /// such as the working tree of a repository without a remote.
    pub as_of: Option<String>,
    /// The data is a local copy that may be behind the remote: the last update failed, or the
    /// issues come from a cache file.
    pub from_cache: bool,
    /// Problems with parts of the data, for example a `meeting.json` that was skipped.
    pub warnings: Vec<String>,
}

/// The open topics of a cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopicFeed {
    pub open: Vec<Topic>,
    /// As for [`TaskFeed::as_of`].
    pub as_of: Option<String>,
}

/// Where the briefing gets tasks and topics from. Implementations read; they never write and
/// never pull (the command does that before it asks).
pub trait BriefingSource: Send + Sync {
    fn tasks(&self, cycle_id: &str) -> IpcResult<TaskFeed>;
    fn topics(&self, cycle_id: &str) -> IpcResult<TopicFeed>;
}

/// The source of Stage 6: the cycle repository on this machine.
pub struct CycleStoreSource {
    repos: Box<dyn Fn() -> Vec<CycleRepoSetting> + Send + Sync>,
}

impl CycleStoreSource {
    pub fn new(repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static) -> Self {
        Self {
            repos: Box::new(repos),
        }
    }

    fn locate(&self, cycle_id: &str) -> IpcResult<cycles::Located> {
        cycles::locate(&(self.repos)(), cycle_id)?.ok_or_else(|| {
            IpcError::new(
                "not_found",
                format!("the cycle {cycle_id} is not in any registered repository"),
            )
        })
    }
}

/// The older of two optional `YYYY-MM-DD` dates (`None` means no date and does not count).
fn oldest(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl BriefingSource for CycleStoreSource {
    fn tasks(&self, cycle_id: &str) -> IpcResult<TaskFeed> {
        let located = self.locate(cycle_id)?;
        let repo = &located.repo;
        let mut warnings = Vec::new();

        let (meeting, skipped) = meeting_tasks(&repo.meetings_dir(cycle_id)?);
        warnings.extend(skipped);
        let topics = repo.load_topics(cycle_id).unwrap_or_else(|e| {
            warnings.push(format!("topics.yaml could not be read: {e}"));
            Vec::new()
        });
        let cache = IssueCache::load(&repo.issue_cache_path(cycle_id)?);
        // A cache that was never filled says nothing about which issues are closed.
        let issues_known = cache.fetched_unix_ms.is_some();
        let merged = merge_tasks(&cache.issues, issues_known, meeting, &topics);

        let dto = cycles::repo_dto(&located.setting);
        Ok(TaskFeed {
            open: merged.open,
            done: merged.done,
            as_of: oldest(dto.state_as_of, cache.fetched_unix_ms.map(format_utc_date)),
            from_cache: dto.offline || issues_known,
            warnings,
        })
    }

    fn topics(&self, cycle_id: &str) -> IpcResult<TopicFeed> {
        let located = self.locate(cycle_id)?;
        let topics = located.repo.load_topics(cycle_id)?;
        Ok(TopicFeed {
            open: open_topics(&topics),
            as_of: cycles::repo_dto(&located.setting).state_as_of,
        })
    }
}

// ----- the screen ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingTask {
    pub id: String,
    pub text: String,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    /// The due date is before `today`.
    pub overdue: bool,
    /// `issue` or `meeting`.
    pub source: &'static str,
    pub issue: Option<u64>,
    pub url: Option<String>,
    /// The folder of the meeting a task was raised in.
    pub meeting: Option<String>,
}

impl From<&TaskEntry> for BriefingTask {
    fn from(t: &TaskEntry) -> Self {
        Self {
            id: t.id.clone(),
            text: t.text.clone(),
            due: t.due.clone(),
            overdue: t.overdue,
            source: match t.origin {
                TaskOrigin::Issue => "issue",
                TaskOrigin::Meeting => "meeting",
            },
            issue: t.issue,
            url: t.url.clone(),
            meeting: t.meeting.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingGroup {
    /// `participant`, `other` or `unassigned`.
    pub kind: &'static str,
    pub participant_id: Option<String>,
    pub name: String,
    pub github: Option<String>,
    pub overdue_count: usize,
    pub tasks: Vec<BriefingTask>,
}

impl From<&TaskGroup> for BriefingGroup {
    fn from(g: &TaskGroup) -> Self {
        Self {
            kind: match g.kind {
                GroupKind::Participant => "participant",
                GroupKind::Other => "other",
                GroupKind::Unassigned => "unassigned",
            },
            participant_id: g.participant_id.clone(),
            name: g.name.clone(),
            github: g.github.clone(),
            overdue_count: g.overdue_count(),
            tasks: g.tasks.iter().map(BriefingTask::from).collect(),
        }
    }
}

/// Everything the briefing screen shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub cycle_id: String,
    pub cycle_name: String,
    /// The day overdue is counted against, `YYYY-MM-DD` (UTC).
    pub today: String,
    pub groups: Vec<BriefingGroup>,
    pub open_count: usize,
    pub overdue_count: usize,
    pub done: Vec<BriefingTask>,
    pub topics: Vec<TopicDto>,
    /// The repository has a remote the state can be behind.
    pub has_remote: bool,
    /// `YYYY-MM-DD` of the data shown: the last successful pull, or the age of the issue
    /// cache, whichever is older. `None` for a local repository without issues.
    pub state_as_of: Option<String>,
    /// The last attempt to update the clone failed: what is shown is the state of
    /// `stateAsOf`.
    pub offline: bool,
    /// The data may be behind the remote (see `TaskFeed::from_cache`).
    pub from_cache: bool,
    /// What this call did to the clone: `updated`, `upToDate`, `failed` or `skippedDirty`
    /// (cycle files have uncommitted edits). `None` without `refresh` and for a repository
    /// that is not a clone.
    pub pull: Option<&'static str>,
    /// Why the last update failed.
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

fn pull_name(outcome: &PullOutcome) -> &'static str {
    match outcome {
        PullOutcome::Updated => "updated",
        PullOutcome::UpToDate => "upToDate",
        PullOutcome::NoRemote => "upToDate",
        PullOutcome::Failed(_) => "failed",
        PullOutcome::SkippedDirty => "skippedDirty",
    }
}

/// Builds the screen of `cycle_id`. With `refresh` a cloned repository is pulled first; a
/// failure is part of the result, not an error. `now_ms` is the moment of the briefing (its
/// UTC date is the day overdue is counted against).
pub fn build_briefing(
    source: &dyn BriefingSource,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    refresh: bool,
    now_ms: u64,
) -> IpcResult<Briefing> {
    let located = cycles::locate(repos, cycle_id)?.ok_or_else(|| {
        IpcError::new(
            "not_found",
            format!("the cycle {cycle_id} is not in any registered repository"),
        )
    })?;
    let is_clone = located.setting.kind == RepoKind::Clone;
    let mut warnings = Vec::new();
    let mut pull = None;
    let mut pull_error = None;
    if refresh && is_clone {
        match located.repo.pull_rebase() {
            Ok(report) => {
                if let PullOutcome::Failed(why) = &report.outcome {
                    pull_error = Some(why.clone());
                }
                pull = Some(pull_name(&report.outcome));
                warnings.extend(report.warnings);
            }
            Err(e) => {
                pull = Some("failed");
                pull_error = Some(e.to_string());
            }
        }
    }

    // The pull may have changed the files: the cycle is read afresh.
    let located = cycles::locate(repos, cycle_id)?
        .ok_or_else(|| IpcError::new("not_found", format!("the cycle {cycle_id} is gone")))?;
    let (name, people) = match &located.entry.cycle {
        Some(c) => (c.name.clone(), c.participants.clone()),
        None => (cycle_id.to_string(), Vec::new()),
    };
    if let Some(first) = located.entry.issues.first() {
        warnings.push(format!(
            "the cycle has problems, so people may be missing from the groups: {first}"
        ));
    }

    let tasks = source.tasks(cycle_id)?;
    let topics = source.topics(cycle_id)?;
    warnings.extend(tasks.warnings.iter().cloned());

    let today = format_utc_date(now_ms);
    let groups = group_tasks(&tasks.open, &people, &today);
    let done_groups = group_tasks(&tasks.done, &people, &today);
    let repo = cycles::repo_dto(&located.setting);
    Ok(Briefing {
        cycle_id: cycle_id.to_string(),
        cycle_name: name,
        today,
        open_count: groups.iter().map(|g| g.tasks.len()).sum(),
        overdue_count: groups.iter().map(TaskGroup::overdue_count).sum(),
        groups: groups.iter().map(BriefingGroup::from).collect(),
        done: done_groups
            .iter()
            .flat_map(|g| g.tasks.iter().map(BriefingTask::from))
            .collect(),
        topics: topics.open.iter().map(TopicDto::from).collect(),
        has_remote: located.setting.remote.is_some() || is_clone,
        state_as_of: oldest(tasks.as_of, topics.as_of),
        offline: repo.offline,
        from_cache: tasks.from_cache,
        pull,
        error: pull_error.or(repo.last_error),
        warnings,
    })
}

/// Event for the main window: show the briefing of the cycle in the payload.
pub const EVENT_OPEN: &str = "open-briefing";

// ----- the command -----------------------------------------------------------------------

/// The briefing of a cycle. With `refresh` a cloned repository is pulled first, in the
/// background: the window stays usable meanwhile. Offline or after a failed pull the answer is
/// the state of the last successful pull, with its date.
#[tauri::command]
pub async fn get_briefing(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    refresh: bool,
) -> IpcResult<Briefing> {
    let repos = state.cycle_repos();
    let source = state.briefing().clone();
    let id = cycle_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        build_briefing(source.as_ref(), &repos, &id, refresh, now_unix_ms())
    })
    .await
    .map_err(|e| IpcError::new("internal", format!("the background task failed: {e}")))??;
    if result.pull == Some("updated") {
        // New commits may have changed the participants.
        if let Err(e) = cycles::sync_cycle(&state.cycle_repos(), state.inner(), &cycle_id) {
            tracing::warn!(cycle = %cycle_id, error = %e, "participants not synced after the pull");
        }
        if let Err(e) = app.emit(EVENT_CYCLES, ()) {
            tracing::warn!(error = %e, "could not emit the cycles change");
        }
    }
    Ok(result)
}

/// Brings the main window forward on the briefing of a cycle. The recorder window uses it:
/// it has no pages of its own.
#[tauri::command]
pub fn open_briefing(app: AppHandle, cycle_id: String) -> IpcResult<()> {
    if !is_identifier(&cycle_id) {
        return Err(IpcError::new("invalid", "this is not a cycle id"));
    }
    crate::show_main(&app);
    app.emit_to("main", EVENT_OPEN, cycle_id)
        .map_err(|e| IpcError::new("internal", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycles::tests::Lab;
    use cycle_store::{Cycle, Participant, Repo, TopicStatus};
    use serde_json::json;
    use std::path::Path;

    /// 2026-11-13 12:00 UTC.
    const NOW_MS: u64 = 1_794_571_200_000;

    fn source_of(repos: Vec<CycleRepoSetting>) -> CycleStoreSource {
        CycleStoreSource::new(move || repos.clone())
    }

    fn briefing(repos: &[CycleRepoSetting], refresh: bool) -> Briefing {
        build_briefing(&source_of(repos.to_vec()), repos, "NOMAD", refresh, NOW_MS).unwrap()
    }

    fn person(id: &str, name: &str, github: Option<&str>) -> Participant {
        Participant {
            id: id.into(),
            name: name.into(),
            github: github.map(String::from),
            discord: None,
        }
    }

    /// A task item of a `meeting.json`: (id, text, assignee, due, relates_to).
    type Item<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );

    fn write_meeting(repo: &Repo, folder: &str, items: &[Item<'_>]) {
        let items: Vec<_> = items
            .iter()
            .map(|(id, text, who, due, rel)| {
                json!({
                    "id": id, "type": "task", "text": text, "assignee": who, "due": due,
                    "certainty": {"assignee": "stated", "due": "stated"},
                    "section": null, "relates_to": rel, "source": null, "edited": false
                })
            })
            .collect();
        let doc = json!({
            "version": 1, "meeting_id": format!("m-{folder}"), "cycle": "NOMAD",
            "date": &folder[..10], "language": "en", "mode": "fixed", "template": null,
            "meeting_type": null, "sections": [], "summary": "", "items": items,
            "warnings": []
        });
        let dir = repo.meetings_dir("NOMAD").unwrap().join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meeting.json"), doc.to_string()).unwrap();
    }

    fn set_people(repo: &Repo) {
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.participants = vec![
            person("alex", "Alex Example", Some("alex-example")),
            person("sam", "Sam Sample", Some("sam-sample")),
        ];
        repo.save_cycle(&cycle).unwrap();
    }

    fn names(b: &Briefing) -> Vec<&str> {
        b.groups.iter().map(|g| g.name.as_str()).collect()
    }

    fn topic(id: &str, text: &str, status: TopicStatus) -> Topic {
        Topic {
            id: id.into(),
            text: text.into(),
            status,
            opened_in: "2026-11-06".into(),
            closed_in: (status == TopicStatus::Closed).then(|| "2026-11-13".to_string()),
        }
    }

    #[test]
    fn a_local_repository_gives_tasks_by_person_and_open_topics() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let repo = lab.repo();
        set_people(&repo);
        repo.save_topics(
            "NOMAD",
            &[
                topic("t-1", "Decide the rover wheel size", TopicStatus::Open),
                topic("t-2", "Order the battery", TopicStatus::Closed),
            ],
        )
        .unwrap();
        write_meeting(
            &repo,
            "2026-11-06",
            &[
                (
                    "i1",
                    "Design the mount",
                    Some("alex"),
                    Some("2026-11-10"),
                    None,
                ),
                ("i2", "Order the battery", Some("sam"), None, Some("t-2")),
                ("i3", "Check the wiring", None, None, None),
                (
                    "i4",
                    "Write the test plan",
                    Some("sam"),
                    Some("2026-11-20"),
                    Some("#12"),
                ),
            ],
        );
        // The cache of open issues: #12 is the test plan, #13 belongs to a login nobody knows.
        let cache = cycle_store::IssueCache {
            repo: "owner/rover".into(),
            label: "cycle:NOMAD".into(),
            fetched_unix_ms: Some(1_794_000_000_000),
            closed: Vec::new(),
            links: Vec::new(),
            issues: vec![
                cycle_store::OpenIssue {
                    number: 12,
                    title: "Write the test plan".into(),
                    assignee: Some("SAM-SAMPLE".into()),
                    url: Some("https://example.invalid/issues/12".into()),
                },
                cycle_store::OpenIssue {
                    number: 13,
                    title: "Review the mount".into(),
                    assignee: Some("octo-guest".into()),
                    url: None,
                },
            ],
        };
        cache
            .save(&repo.issue_cache_path("NOMAD").unwrap())
            .unwrap();

        let b = briefing(lab.repos(), false);
        assert_eq!(b.today, "2026-11-13");
        assert_eq!(b.cycle_name, "Nomad rover");
        assert_eq!(
            names(&b),
            ["Alex Example", "Sam Sample", "@octo-guest", "Unassigned"]
        );
        // The test plan is one task, the issue, with the due date of the meeting.
        let sam = &b.groups[1];
        assert_eq!(sam.tasks.len(), 1, "{:?}", sam.tasks);
        assert_eq!(sam.tasks[0].id, "#12");
        assert_eq!(sam.tasks[0].source, "issue");
        assert_eq!(sam.tasks[0].due.as_deref(), Some("2026-11-20"));
        assert!(!sam.tasks[0].overdue);
        // The mount is overdue by a few days.
        assert!(b.groups[0].tasks[0].overdue);
        assert_eq!(b.groups[0].overdue_count, 1);
        assert_eq!((b.open_count, b.overdue_count), (4, 1));
        // The battery belongs to a closed topic, so it is done and not open.
        assert_eq!(b.done.len(), 1);
        assert_eq!(b.done[0].text, "Order the battery");
        // Only the open topic is listed.
        assert_eq!(b.topics.len(), 1);
        assert_eq!(b.topics[0].id, "t-1");

        // A local repository has no remote to be behind; the issue cache has an age.
        assert!(!b.has_remote && !b.offline && b.pull.is_none() && b.error.is_none());
        assert!(b.from_cache);
        assert_eq!(b.state_as_of.as_deref(), Some("2026-11-06"));

        // Asking to refresh a local repository pulls nothing.
        assert!(briefing(lab.repos(), true).pull.is_none());

        // The screen is camelCase JSON.
        let value = serde_json::to_value(&b).unwrap();
        assert_eq!(value["groups"][0]["overdueCount"], 1);
        assert_eq!(value["stateAsOf"], "2026-11-06");
    }

    #[test]
    fn without_issues_the_briefing_still_shows_the_tasks_of_the_reports() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let repo = lab.repo();
        set_people(&repo);
        write_meeting(
            &repo,
            "2026-11-06",
            &[("i1", "Design the mount", Some("alex"), None, Some("#12"))],
        );
        // Issues were never read: a pointer to #12 proves nothing, the task stays open.
        let b = briefing(lab.repos(), false);
        assert_eq!(b.open_count, 1);
        assert!(b.done.is_empty());
        assert!(!b.from_cache);
        assert_eq!(b.state_as_of, None);
    }

    #[test]
    fn a_broken_meeting_file_is_a_warning_not_a_failure() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let repo = lab.repo();
        let dir = repo.meetings_dir("NOMAD").unwrap().join("2026-11-06");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meeting.json"), "{ broken").unwrap();
        let b = briefing(lab.repos(), false);
        assert_eq!(b.open_count, 0);
        assert_eq!(b.warnings.len(), 1, "{:?}", b.warnings);
        assert!(b.warnings[0].contains("2026-11-06/meeting.json"));
    }

    #[test]
    fn an_unknown_cycle_is_not_found() {
        let lab = Lab::new();
        let e = build_briefing(
            &source_of(lab.repos().to_vec()),
            lab.repos(),
            "GHOST",
            false,
            NOW_MS,
        )
        .unwrap_err();
        assert_eq!(e.code, "not_found");
    }

    /// Runs git with a fixed identity, for fixtures that commit.
    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec![
            "-C",
            dir.to_str().unwrap(),
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        let out = std::process::Command::new("git")
            .args(&full)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_all(repo: &Repo, message: &str) {
        git(repo.root(), &["add", "-A"]);
        git(repo.root(), &["commit", "--quiet", "-m", message]);
    }

    /// An origin with the cycle NOMAD and one meeting, a clone of it and its settings.
    fn origin_and_clone(root: &Path) -> (Repo, Vec<CycleRepoSetting>, Repo) {
        let origin = Repo::init_local(root.join("origin")).unwrap();
        origin.create_cycle(&Cycle::new("NOMAD", "Nomad")).unwrap();
        set_people(&origin);
        write_meeting(
            &origin,
            "2026-11-06",
            &[(
                "i1",
                "Design the mount",
                Some("alex"),
                Some("2026-11-20"),
                None,
            )],
        );
        commit_all(&origin, "start");
        let dest = root.join("clone");
        let clone = Repo::clone(origin.root().to_str().unwrap(), &dest).unwrap();
        let settings = vec![CycleRepoSetting {
            id: "team".into(),
            path: dest.to_string_lossy().into_owned(),
            remote: Some(origin.root().to_string_lossy().into_owned()),
            kind: RepoKind::Clone,
            github_repo: None,
            github_api: None,
        }];
        (origin, settings, clone)
    }

    fn second_meeting(origin: &Repo) {
        write_meeting(
            origin,
            "2026-11-13",
            &[("i1", "Order the battery", Some("sam"), None, None)],
        );
        commit_all(origin, "second meeting");
    }

    #[test]
    fn refreshing_a_clone_pulls_new_tasks_and_a_plain_call_does_not() {
        let tmp = test_tmp::TestDir::new("briefing-app");
        let (origin, repos, _clone) = origin_and_clone(tmp.path());
        second_meeting(&origin);

        let before = briefing(&repos, false);
        assert_eq!(before.open_count, 1);
        assert!(before.pull.is_none() && before.has_remote);
        assert!(before.state_as_of.is_some(), "a clone knows its age");

        let after = briefing(&repos, true);
        assert_eq!(after.pull, Some("updated"));
        assert_eq!(after.open_count, 2);
        assert!(!after.offline && after.error.is_none());
        assert_eq!(names(&after), ["Alex Example", "Sam Sample"]);

        assert_eq!(briefing(&repos, true).pull, Some("upToDate"));
    }

    #[test]
    fn uncommitted_cycle_edits_stop_the_pull_and_nothing_is_lost() {
        let tmp = test_tmp::TestDir::new("briefing-app");
        let (origin, repos, clone) = origin_and_clone(tmp.path());
        second_meeting(&origin);
        // An edit made in the application and not committed.
        let mut mine = clone.load_cycle("NOMAD").unwrap();
        mine.name = "My name".into();
        clone.save_cycle(&mine).unwrap();

        let b = briefing(&repos, true);
        assert_eq!(b.pull, Some("skippedDirty"));
        assert_eq!(b.open_count, 1, "the new meeting was not pulled");
        assert_eq!(b.cycle_name, "My name");
        assert!(!b.offline, "not updating is not being offline");
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "My name");
        let stash = std::process::Command::new("git")
            .args(["-C", clone.root().to_str().unwrap(), "stash", "list"])
            .output()
            .unwrap();
        assert!(stash.stdout.is_empty(), "nothing was stashed");
    }

    #[test]
    fn a_clone_whose_remote_is_gone_shows_its_last_state_and_says_so() {
        let tmp = test_tmp::TestDir::new("briefing-app");
        let (origin, repos, _clone) = origin_and_clone(tmp.path());
        let origin_dir = origin.root().to_path_buf();
        drop(origin);
        std::fs::remove_dir_all(&origin_dir).unwrap();

        let b = briefing(&repos, true);
        assert_eq!(b.pull, Some("failed"));
        assert!(b.offline);
        assert!(b.error.is_some());
        // The tasks and the date of the last successful update are still there.
        assert_eq!(b.open_count, 1);
        assert_eq!(b.groups[0].tasks[0].text, "Design the mount");
        let as_of = b.state_as_of.clone().expect("the date of the clone");
        assert!(as_of.starts_with("20"), "{as_of}");

        // The failure is remembered: a later call without refresh says it too.
        let again = briefing(&repos, false);
        assert!(again.pull.is_none());
        assert!(again.offline && again.error.is_some());
        assert_eq!(again.state_as_of, Some(as_of));
        assert_eq!(again.open_count, 1);
    }

    /// A source that is not the cycle repository: the screen does not care where tasks come
    /// from (this is how Stage 5 plugs in).
    struct FixedSource;

    impl BriefingSource for FixedSource {
        fn tasks(&self, _cycle_id: &str) -> IpcResult<TaskFeed> {
            Ok(TaskFeed {
                open: vec![TaskItem {
                    id: "#1".into(),
                    text: "From another source".into(),
                    origin: TaskOrigin::Issue,
                    github: Some("alex-example".into()),
                    participant: None,
                    due: Some("2026-11-01".into()),
                    issue: Some(1),
                    relates_to: None,
                    url: None,
                    meeting: None,
                }],
                done: Vec::new(),
                as_of: Some("2026-11-12".into()),
                from_cache: true,
                warnings: Vec::new(),
            })
        }

        fn topics(&self, _cycle_id: &str) -> IpcResult<TopicFeed> {
            Ok(TopicFeed::default())
        }
    }

    #[test]
    fn another_source_plugs_in_without_changes_to_the_screen() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        set_people(&lab.repo());
        let b = build_briefing(&FixedSource, lab.repos(), "NOMAD", false, NOW_MS).unwrap();
        assert_eq!(names(&b), ["Alex Example"]);
        assert_eq!(b.overdue_count, 1);
        assert_eq!(b.state_as_of.as_deref(), Some("2026-11-12"));
        assert!(b.from_cache);
    }
}
