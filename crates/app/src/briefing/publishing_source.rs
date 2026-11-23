//! The briefing source of Stage 5: the tasks of a cycle as the read-back of its GitHub issues
//! has them, kept together with the task items of the meetings and the topics of the clone.
//!
//! Open and done issues come from [`tasks_of`] (the issue cache next to the clone). A task item
//! of a meeting that was made an issue is the same task as the issue: it is found by the number
//! it points to (`relates_to`) or by the marker link of the cache (meeting and item), and shown
//! once. When the repository is not linked to GitHub or its issues were never read,
//! [`CycleStoreSource`] answers instead.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use cycle_store::issues::{IssueCache, IssueLink, OpenIssue};
use cycle_store::time::format_utc_date;
use cycle_store::{meeting_tasks, merge_tasks, Draft, MergedTasks, TaskItem, TaskOrigin, Topic};
use publishers::http::HttpClient;

use super::{oldest, BriefingSource, CycleStoreSource, SourceRefresh, TaskFeed, TopicFeed};
use crate::cycles::{self, Located};
use crate::ipc::IpcResult;
use crate::issue_sync::refresh_cycle_issues;
use crate::publishing::{tasks_of, CycleTasksDto};
use crate::secrets::SecretStore;
use crate::settings::CycleRepoSetting;

/// The default source of the briefing. The network and the credential store are parameters, so
/// that tests use a fake GitHub and an in-memory store.
pub struct PublishingSource {
    fallback: CycleStoreSource,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    client: Box<dyn Fn() -> HttpClient + Send + Sync>,
}

impl PublishingSource {
    pub fn new(
        repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static,
        secrets: Arc<dyn SecretStore + Send + Sync>,
        client: impl Fn() -> HttpClient + Send + Sync + 'static,
    ) -> Self {
        Self {
            fallback: CycleStoreSource::new(repos),
            secrets,
            client: Box::new(client),
        }
    }
}

/// The folder of each meeting of `dir`, by meeting id (the id a marker link carries).
fn folders_by_meeting_id(dir: &Path) -> HashMap<String, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    let mut found = HashMap::new();
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path().join("meeting.json")) else {
            continue;
        };
        if let Ok(draft) = Draft::from_json_str(&text) {
            let folder = entry.file_name().to_string_lossy().into_owned();
            found.entry(draft.meeting_id).or_insert(folder);
        }
    }
    found
}

/// Puts the read-back of the issues and the task items of the meetings together.
///
/// * An open issue is a task. A meeting task that is the same task (it points to the issue, or
///   the cache links the issue to its item) is not listed again and lends its due date to the
///   issue, which carries none.
/// * A closed issue is done: with the text of the meeting task when there is one, with the
///   title the read-back found otherwise.
/// * Meeting tasks without an issue stay as they are (open, or done through a closed topic).
///
/// `folders` maps a meeting id to the folder of its meeting.
fn merge(
    dto: &CycleTasksDto,
    links: &[IssueLink],
    folders: &HashMap<String, String>,
    mut meeting: Vec<TaskItem>,
    topics: &[Topic],
) -> MergedTasks {
    let issues: Vec<OpenIssue> = dto
        .open
        .iter()
        .map(|t| OpenIssue {
            number: t.number,
            title: t.title.clone(),
            assignee: t.assignee.clone(),
            url: t.url.clone(),
        })
        .collect();
    let open: HashSet<u64> = issues.iter().map(|i| i.number).collect();

    // The issue an item was made into, by its marker. A link that says "open" for an issue the
    // list does not hold (beyond the pages that were read) proves nothing: the task stays open.
    for task in meeting.iter_mut().filter(|t| t.issue.is_none()) {
        let Some(folder) = task.meeting.clone() else {
            continue;
        };
        let item = task
            .id
            .strip_prefix(&folder)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or("");
        task.issue = links
            .iter()
            .find(|l| {
                l.item == item
                    && folders.get(&l.meeting) == Some(&folder)
                    && (!l.open || open.contains(&l.number))
            })
            .map(|l| l.number);
    }

    let mut merged = merge_tasks(&issues, true, meeting, topics);
    let covered: HashSet<u64> = merged.done.iter().filter_map(|t| t.issue).collect();
    for closed in dto.done.iter().filter(|d| !covered.contains(&d.number)) {
        merged.done.push(TaskItem {
            id: format!("#{}", closed.number),
            text: closed.title.clone(),
            origin: TaskOrigin::Issue,
            github: closed.assignee.clone(),
            participant: None,
            due: None,
            issue: Some(closed.number),
            relates_to: None,
            url: closed.url.clone(),
            meeting: closed
                .meeting
                .as_ref()
                .and_then(|id| folders.get(id).cloned()),
        });
    }
    merged
}

impl BriefingSource for PublishingSource {
    fn tasks(&self, cycle_id: &str) -> IpcResult<TaskFeed> {
        let located = self.fallback.locate(cycle_id)?;
        let dto = tasks_of(&located, cycle_id)?;
        // Not on GitHub, or the issues were never read: the cycle files are all there is.
        let (Some(_), Some(issues_as_of)) = (&dto.repo, &dto.as_of) else {
            return self.fallback.tasks(cycle_id);
        };
        let repo = &located.repo;
        let meetings = repo.meetings_dir(cycle_id)?;
        let (meeting, mut warnings) = meeting_tasks(&meetings);
        let topics = repo.load_topics(cycle_id).unwrap_or_else(|e| {
            warnings.push(format!("topics.yaml could not be read: {e}"));
            Vec::new()
        });
        let cache = IssueCache::load(&repo.issue_cache_path(cycle_id)?);
        let folders = if cache.links.is_empty() {
            HashMap::new()
        } else {
            folders_by_meeting_id(&meetings)
        };
        let merged = merge(&dto, &cache.links, &folders, meeting, &topics);
        let state = cycles::repo_dto(&located.setting);
        Ok(TaskFeed {
            open: merged.open,
            done: merged.done,
            as_of: oldest(state.state_as_of, Some(issues_as_of.clone())),
            // The issues come from the cache; a successful refresh just before says otherwise
            // (the briefing knows that, see `SourceRefresh`).
            from_cache: true,
            warnings,
        })
    }

    fn topics(&self, cycle_id: &str) -> IpcResult<TopicFeed> {
        self.fallback.topics(cycle_id)
    }

    fn refresh(&self, located: &Located, cycle_id: &str) -> Option<SourceRefresh> {
        let list = refresh_cycle_issues(
            self.secrets.as_ref(),
            &located.setting,
            &located.repo,
            cycle_id,
            (self.client)(),
        )?;
        if let Some(why) = &list.error {
            tracing::warn!(
                cycle = cycle_id,
                %why,
                as_of = ?list.as_of_unix_ms.map(format_utc_date),
                "issues not refreshed for the briefing, using the last known state"
            );
        }
        Some(SourceRefresh { error: list.error })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use publishers::github::{Github, IssueSpec, RepoRef};
    use publishers::testing::{FakeGithub, Visibility};
    use serde_json::json;

    use super::*;
    use crate::briefing::{build_briefing, Briefing};
    use crate::cycles::tests::Lab;
    use crate::publish_secrets::{set_github_repo, set_github_token};
    use crate::publishing::TaskDto;
    use crate::secrets::MemorySecrets;

    /// 2026-11-13 12:00 UTC.
    const NOW_MS: u64 = 1_794_571_200_000;
    const REPO: &str = "owner/rover";
    const TOKEN: &str = "ghp_MockTokenValue0123456789";
    const MEETING_ID: &str = "20261106-100000";

    fn task(number: u64, title: &str, assignee: Option<&str>) -> TaskDto {
        TaskDto {
            number,
            title: title.into(),
            assignee: assignee.map(String::from),
            url: Some(format!("https://example.invalid/issues/{number}")),
            meeting: None,
        }
    }

    fn dto(open: Vec<TaskDto>, done: Vec<TaskDto>) -> CycleTasksDto {
        CycleTasksDto {
            cycle_id: "NOMAD".into(),
            repo: Some(REPO.into()),
            as_of: Some("2026-11-06".into()),
            open,
            done,
        }
    }

    fn meeting_item(folder: &str, item: &str, text: &str, relates: Option<&str>) -> TaskItem {
        TaskItem {
            id: format!("{folder}/{item}"),
            text: text.into(),
            origin: TaskOrigin::Meeting,
            github: None,
            participant: Some("alex".into()),
            due: Some("2026-11-20".into()),
            issue: relates.and_then(cycle_store::issue_number),
            relates_to: relates.map(String::from),
            url: None,
            meeting: Some(folder.into()),
        }
    }

    fn link(number: u64, item: &str, open: bool) -> IssueLink {
        IssueLink {
            number,
            meeting: MEETING_ID.into(),
            item: item.into(),
            open,
        }
    }

    fn folders() -> HashMap<String, String> {
        HashMap::from([(MEETING_ID.to_string(), "2026-11-06".to_string())])
    }

    #[test]
    fn open_and_done_issues_become_tasks_with_login_link_and_meeting() {
        let mut closed = task(5, "Design the mount", None);
        closed.meeting = Some(MEETING_ID.into());
        let d = dto(
            vec![task(7, "Order the wheels", Some("sam-gh"))],
            vec![closed, task(6, "Issue #6", None)],
        );
        let m = merge(&d, &[], &folders(), Vec::new(), &[]);
        assert_eq!(m.open.len(), 1);
        let t = &m.open[0];
        assert_eq!(t.id, "#7");
        assert_eq!(t.origin, TaskOrigin::Issue);
        assert_eq!(t.github.as_deref(), Some("sam-gh"));
        assert_eq!(t.url.as_deref(), Some("https://example.invalid/issues/7"));
        let done: Vec<_> = m
            .done
            .iter()
            .map(|t| (t.id.as_str(), t.text.as_str()))
            .collect();
        assert_eq!(done, [("#5", "Design the mount"), ("#6", "Issue #6")]);
        // The meeting id of a done issue is shown as the folder of the meeting.
        assert_eq!(m.done[0].meeting.as_deref(), Some("2026-11-06"));
        assert_eq!(m.done[1].meeting, None);
    }

    #[test]
    fn a_meeting_task_that_became_an_issue_is_shown_once() {
        let d = dto(
            vec![
                task(7, "Order the wheels", None),
                task(8, "Check the wiring", None),
            ],
            vec![task(5, "Design the mount", None)],
        );
        let meeting = vec![
            // Points to the open issue by number.
            meeting_item("2026-11-06", "i1", "Order the wheels", Some("#7")),
            // Found by the marker link of its issue (no pointer in the file).
            meeting_item("2026-11-06", "i2", "Check the wiring", None),
            // The issue is closed: done, with the text of the meeting, and once.
            meeting_item("2026-11-06", "i3", "Design the mount", None),
            // No issue at all: stays an open meeting task.
            meeting_item("2026-11-06", "i4", "Write the test plan", None),
        ];
        let links = [link(8, "i2", true), link(5, "i3", false)];
        let m = merge(&d, &links, &folders(), meeting, &[]);

        let open: Vec<_> = m.open.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(open, ["#7", "#8", "2026-11-06/i4"], "{:?}", m.open);
        // The issues carry no due date: the meeting lends its own.
        assert_eq!(m.open[0].due.as_deref(), Some("2026-11-20"));
        assert_eq!(m.open[1].due.as_deref(), Some("2026-11-20"));
        assert_eq!(m.open[1].meeting.as_deref(), Some("2026-11-06"));
        let done: Vec<_> = m.done.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(done, ["2026-11-06/i3"], "closed #5 is not listed twice");
        assert_eq!(m.done[0].issue, Some(5));
    }

    #[test]
    fn a_link_of_another_meeting_or_an_unlisted_open_issue_does_not_close_a_task() {
        let d = dto(Vec::new(), Vec::new());
        let meeting = vec![
            meeting_item("2026-11-06", "i1", "Order the wheels", None),
            meeting_item("2026-11-13", "i1", "Check the wiring", None),
        ];
        // Same item id in two meetings: only the meeting of the link counts. And the issue of
        // the link is open but beyond the pages that were read, so it proves nothing.
        let links = [link(9, "i1", true)];
        let m = merge(&d, &links, &folders(), meeting, &[]);
        assert_eq!(m.open.len(), 2);
        assert!(m.done.is_empty());
    }

    // ----- the whole briefing against a fake GitHub ------------------------------------------

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

    fn source(store: Arc<MemorySecrets>, repos: Vec<CycleRepoSetting>) -> PublishingSource {
        PublishingSource::new(move || repos.clone(), store, client)
    }

    fn linked(gh: &FakeGithub) -> Lab {
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        let id = lab.settings.cycle_repos[0].id.clone();
        set_github_repo(
            &mut lab.settings.cycle_repos,
            &id,
            Some(REPO),
            Some(&gh.api_base()),
        )
        .unwrap();
        let mut cycle = lab.repo().load_cycle("NOMAD").unwrap();
        cycle.participants = vec![cycle_store::Participant {
            id: "sam".into(),
            name: "Sam Sample".into(),
            github: Some("sam-sample".into()),
            discord: None,
        }];
        lab.repo().save_cycle(&cycle).unwrap();
        lab
    }

    /// One meeting with two task items, in a folder of the clone.
    fn write_meeting(lab: &Lab) {
        let items: Vec<_> = [("i1", "Write the test plan"), ("i2", "Order the battery")]
            .iter()
            .map(|(id, text)| {
                json!({
                    "id": id, "type": "task", "text": text, "assignee": "sam", "due": "2026-11-20",
                    "certainty": {"assignee": "stated", "due": "stated"},
                    "section": null, "relates_to": null, "source": null, "edited": false
                })
            })
            .collect();
        let doc = json!({
            "version": 1, "meeting_id": MEETING_ID, "cycle": "NOMAD", "date": "2026-11-06",
            "language": "en", "mode": "fixed", "template": null, "meeting_type": null,
            "sections": [], "summary": "", "items": items, "warnings": []
        });
        let dir = lab.repo().meetings_dir("NOMAD").unwrap().join("2026-11-06");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meeting.json"), doc.to_string()).unwrap();
    }

    /// An issue made from an item of the meeting, as the publisher makes it.
    fn publish(api: &Github, item: &str, assignee: Option<&str>) -> u64 {
        api.publish_issue(
            &RepoRef::parse(REPO).unwrap(),
            &IssueSpec {
                cycle_id: "NOMAD".into(),
                meeting_id: MEETING_ID.into(),
                item_id: item.into(),
                title: format!("Task {item}"),
                assignee: assignee.map(String::from),
                due: None,
                meeting_date: "2026-11-06".into(),
                report_url: None,
                transcript_url: None,
                transcript_anchor: None,
                time_ms: None,
                quote: None,
            },
            false,
        )
        .unwrap()
        .number
    }

    fn briefing(src: &PublishingSource, lab: &Lab, refresh: bool) -> Briefing {
        build_briefing(src, lab.repos(), "NOMAD", refresh, NOW_MS).unwrap()
    }

    #[test]
    fn refreshing_reads_the_issues_and_losing_the_network_keeps_the_state_with_its_date() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token(TOKEN);
        let lab = linked(&gh);
        write_meeting(&lab);
        let store = Arc::new(MemorySecrets::default());
        set_github_token(store.as_ref(), Some(&gh.api_base()), TOKEN).unwrap();
        let api = Github::with_client(client(), &gh.api_base(), TOKEN).unwrap();
        let open_issue = publish(&api, "i1", Some("sam-sample"));
        let closed_issue = publish(&api, "i2", None);
        gh.close_issue(REPO, closed_issue);
        gh.add_issue(REPO, "By hand", &["cycle:NOMAD"]);
        let src = source(store, lab.repos().to_vec());

        // Nothing was read yet: the briefing is the one of the cycle files, and says nothing
        // about closed issues.
        let before = briefing(&src, &lab, false);
        assert_eq!(before.open_count, 2, "both meeting tasks are open");
        assert!(before.done.is_empty() && !before.from_cache);

        // Refreshing reads the issues: the meeting task of the open issue is the issue, the
        // task of the closed one is done, and what was made by hand is there too.
        let fresh = briefing(&src, &lab, true);
        assert!(
            !fresh.from_cache && !fresh.offline && fresh.error.is_none(),
            "{fresh:?}"
        );
        assert!(fresh.has_remote, "the repository is on GitHub");
        assert_eq!(fresh.open_count, 2, "{:?}", fresh.groups);
        let sam = fresh
            .groups
            .iter()
            .find(|g| g.name == "Sam Sample")
            .unwrap();
        assert_eq!(sam.tasks.len(), 1, "{:?}", sam.tasks);
        assert_eq!(sam.tasks[0].id, format!("#{open_issue}"));
        assert_eq!(sam.tasks[0].source, "issue");
        assert_eq!(sam.tasks[0].due.as_deref(), Some("2026-11-20"));
        assert_eq!(sam.tasks[0].meeting.as_deref(), Some("2026-11-06"));
        assert!(fresh
            .groups
            .iter()
            .any(|g| g.tasks.iter().any(|t| t.text == "By hand")));
        assert_eq!(fresh.done.len(), 1);
        assert_eq!(fresh.done[0].text, "Order the battery");
        assert_eq!(fresh.done[0].issue, Some(closed_issue));
        assert!(fresh.state_as_of.is_some());

        // Without refresh the same cache answers and says it is a cache.
        let plain = briefing(&src, &lab, false);
        assert!(plain.from_cache && !plain.offline && plain.error.is_none());
        assert_eq!((plain.open_count, plain.done.len()), (2, 1));

        // The network is gone: the last state shows, with its date and the reason, and
        // without the token.
        gh.go_offline();
        let stale = briefing(&src, &lab, true);
        assert!(stale.from_cache && stale.offline, "{stale:?}");
        let why = stale.error.clone().expect("the reason");
        assert!(!why.is_empty() && !why.contains(TOKEN), "{why}");
        assert_eq!((stale.open_count, stale.done.len()), (2, 1));
        assert_eq!(stale.state_as_of, fresh.state_as_of);
        assert!(stale.pull.is_none(), "a local repository is not pulled");

        // And back: the next refresh is fresh again.
        gh.go_online();
        let back = briefing(&src, &lab, true);
        assert!(!back.from_cache && !back.offline && back.error.is_none());
    }

    #[test]
    fn a_repository_without_github_or_without_read_issues_uses_the_cycle_files() {
        // Not linked to GitHub: the old source answers, refresh or not.
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        write_meeting(&lab);
        let store = Arc::new(MemorySecrets::default());
        let src = source(store.clone(), lab.repos().to_vec());
        for refresh in [false, true] {
            let b = briefing(&src, &lab, refresh);
            assert_eq!(b.open_count, 2);
            assert!(!b.from_cache && !b.offline && b.error.is_none());
            assert!(!b.has_remote);
        }

        // Linked, but the network is down and nothing was ever cached: the cycle files, with
        // the reason, and no invented "closed" state.
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Public);
        let lab = linked(&gh);
        write_meeting(&lab);
        gh.go_offline();
        let src = source(store, lab.repos().to_vec());
        let b = briefing(&src, &lab, true);
        assert_eq!(b.open_count, 2);
        assert!(b.done.is_empty());
        assert!(b.offline && b.from_cache && b.error.is_some(), "{b:?}");
    }

    #[test]
    fn the_topics_come_from_the_cycle_store() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.repo()
            .save_topics(
                "NOMAD",
                &[Topic {
                    id: "t-1".into(),
                    text: "Decide the wheel size".into(),
                    status: cycle_store::TopicStatus::Open,
                    opened_in: "2026-11-06".into(),
                    closed_in: None,
                }],
            )
            .unwrap();
        let src = source(Arc::new(MemorySecrets::default()), lab.repos().to_vec());
        let b = briefing(&src, &lab, false);
        assert_eq!(b.topics.len(), 1);
        assert_eq!(b.topics[0].id, "t-1");
    }
}
