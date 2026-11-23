//! The briefing: what a team still has to do before the next meeting of a cycle.
//!
//! Everything here is pure except [`meeting_tasks`], which reads the `meeting.json` files of
//! the cycle. The rules:
//!
//! * [`merge_tasks`] puts the open issues and the task items of earlier meetings into one
//!   list: a meeting task that points to an issue (`relates_to` of `#12` or `12`) is shown
//!   once, as the issue; a meeting task that points to an issue that is no longer open, or to
//!   a closed topic, is done; the same task repeated in a later meeting is shown once.
//! * [`group_tasks`] groups open tasks by person (a GitHub login is mapped to the cycle's
//!   participant), puts the tasks nobody owns into their own group, sorts each group by due
//!   date and marks the overdue ones against a date that the caller passes in.
//! * [`open_topics`] keeps the open topics.
//!
//! Where the tasks come from is the concern of the caller (see `BriefingSource` in the
//! application): this module only needs [`TaskItem`]s.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use crate::cycle::Participant;
use crate::draft::{Draft, ItemType};
use crate::error::ValidationIssue;
use crate::issues::OpenIssue;
use crate::time::is_iso_date;
use crate::topics::{Topic, TopicStatus};

/// Where a task was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOrigin {
    /// An issue labelled with the cycle.
    Issue,
    /// A task item of a `meeting.json`.
    Meeting,
}

/// A task as a source reports it, before grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem {
    /// `#12` for an issue, `<meeting folder>/<item id>` for a meeting item.
    pub id: String,
    pub text: String,
    pub origin: TaskOrigin,
    /// GitHub login of the assignee of an issue.
    pub github: Option<String>,
    /// Participant id of the assignee of a meeting item.
    pub participant: Option<String>,
    /// `YYYY-MM-DD`; anything else is treated as no due date.
    pub due: Option<String>,
    /// The issue number: of an issue, or the one a meeting item points to.
    pub issue: Option<u64>,
    /// What a meeting item relates to (an issue or a topic id), as written.
    pub relates_to: Option<String>,
    pub url: Option<String>,
    /// The meeting folder of a meeting item.
    pub meeting: Option<String>,
}

impl TaskItem {
    pub fn from_issue(issue: &OpenIssue) -> Self {
        Self {
            id: issue.id(),
            text: issue.title.clone(),
            origin: TaskOrigin::Issue,
            github: issue.assignee.clone(),
            participant: None,
            due: None,
            issue: Some(issue.number),
            relates_to: None,
            url: issue.url.clone(),
            meeting: None,
        }
    }
}

/// The issue number in a `relates_to` value: `#12` or `12`.
pub fn issue_number(reference: &str) -> Option<u64> {
    let digits = reference.trim().trim_start_matches('#');
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn normalize_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Reads the task items of every `<folder>/meeting.json` under `dir` (typically
/// `meetings/<ID>`), newest meeting first. A file that cannot be read or does not validate is
/// skipped and described in the second value; it never stops the briefing.
pub fn meeting_tasks(dir: &Path) -> (Vec<TaskItem>, Vec<String>) {
    let mut warnings = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), warnings);
    };
    let mut drafts: Vec<(String, Draft)> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut folders: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    folders.sort();
    for folder in folders {
        let file = folder.join("meeting.json");
        if !file.is_file() {
            continue;
        }
        let name = folder
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let parsed = fs::read_to_string(&file)
            .map_err(|e| format!("cannot read: {e}"))
            .and_then(|text| {
                Draft::from_json_str(&text).map_err(|issues| {
                    let first: Vec<String> = issues
                        .iter()
                        .take(3)
                        .map(ValidationIssue::to_string)
                        .collect();
                    format!("invalid: {}", first.join("; "))
                })
            });
        match parsed {
            Ok(draft) => {
                if seen_ids.insert(draft.meeting_id.clone()) {
                    drafts.push((name, draft));
                }
            }
            Err(message) => warnings.push(format!("skipped {name}/meeting.json: {message}")),
        }
    }
    drafts.sort_by(|a, b| b.1.date.cmp(&a.1.date).then_with(|| b.0.cmp(&a.0)));
    let mut tasks = Vec::new();
    for (folder, draft) in drafts {
        for item in draft.items.iter().filter(|i| i.item_type == ItemType::Task) {
            tasks.push(TaskItem {
                id: format!("{folder}/{}", item.id),
                text: item.text.clone(),
                origin: TaskOrigin::Meeting,
                github: None,
                participant: item.assignee.clone(),
                due: item.due.clone().filter(|d| is_iso_date(d)),
                issue: item.relates_to.as_deref().and_then(issue_number),
                relates_to: item.relates_to.clone(),
                url: None,
                meeting: Some(folder.clone()),
            });
        }
    }
    (tasks, warnings)
}

/// The open and the done tasks after [`merge_tasks`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergedTasks {
    pub open: Vec<TaskItem>,
    pub done: Vec<TaskItem>,
}

/// Puts open issues and the task items of earlier meetings (newest first) together.
///
/// * An open issue is a task. A meeting task that points to it is the same task: it is not
///   listed again, and it lends its due date and assignee to the issue when the issue has
///   none (a cached issue carries no due date).
/// * A meeting task that points to an issue that is not among the open ones is done, but only
///   when `issues_known` says the list of open issues was really read (an empty list that was
///   never fetched would otherwise close everything). Without it the task stays open.
/// * A meeting task that relates to a closed topic is done.
/// * A task that a later meeting repeated (same text, same assignee) is listed once, from the
///   newest meeting.
pub fn merge_tasks(
    issues: &[OpenIssue],
    issues_known: bool,
    meeting: Vec<TaskItem>,
    topics: &[Topic],
) -> MergedTasks {
    let mut open: Vec<TaskItem> = issues.iter().map(TaskItem::from_issue).collect();
    let by_number: BTreeMap<u64, usize> = issues
        .iter()
        .enumerate()
        .map(|(i, issue)| (issue.number, i))
        .collect();
    let closed_topics: HashSet<&str> = topics
        .iter()
        .filter(|t| t.status == TopicStatus::Closed)
        .map(|t| t.id.as_str())
        .collect();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut done = Vec::new();
    for task in meeting {
        let key = (
            normalize_text(&task.text),
            task.participant.clone().unwrap_or_default(),
        );
        if !seen.insert(key) {
            continue;
        }
        if task
            .relates_to
            .as_deref()
            .is_some_and(|r| closed_topics.contains(r.trim()))
        {
            done.push(task);
            continue;
        }
        match task.issue {
            Some(n) => match by_number.get(&n) {
                Some(&at) => {
                    let issue = &mut open[at];
                    if issue.due.is_none() {
                        issue.due = task.due;
                    }
                    if issue.github.is_none() && issue.participant.is_none() {
                        issue.participant = task.participant;
                    }
                    if issue.meeting.is_none() {
                        issue.meeting = task.meeting;
                    }
                }
                None if issues_known => done.push(task),
                None => open.push(task),
            },
            None => open.push(task),
        }
    }
    MergedTasks { open, done }
}

/// How a group of tasks is headed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// A participant of the cycle.
    Participant,
    /// Somebody who is not a participant: a GitHub login that matches nobody, or the id of a
    /// participant who is no longer in `cycle.yaml`.
    Other,
    /// Tasks nobody owns.
    Unassigned,
}

/// A task ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub id: String,
    pub text: String,
    pub due: Option<String>,
    /// The due date is before the day the briefing was made for.
    pub overdue: bool,
    pub origin: TaskOrigin,
    pub issue: Option<u64>,
    pub url: Option<String>,
    pub meeting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGroup {
    pub kind: GroupKind,
    /// The participant id of a [`GroupKind::Participant`] group.
    pub participant_id: Option<String>,
    /// The participant's name, `@login` or the unknown id, or `Unassigned`.
    pub name: String,
    pub github: Option<String>,
    /// Sorted by due date, tasks without one last.
    pub tasks: Vec<TaskEntry>,
}

impl TaskGroup {
    pub fn overdue_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.overdue).count()
    }
}

/// Where a task belongs: a participant of the cycle, somebody else or nobody.
enum Owner {
    Person(usize),
    Other(String),
    Nobody,
}

fn owner_of(task: &TaskItem, people: &[Participant]) -> Owner {
    // The GitHub login is what the team sees on the issue, so it comes first.
    if let Some(login) = task.github.as_deref().filter(|l| !l.is_empty()) {
        if let Some(at) = people.iter().position(|p| {
            p.github
                .as_deref()
                .is_some_and(|g| g.eq_ignore_ascii_case(login))
        }) {
            return Owner::Person(at);
        }
    }
    if let Some(id) = task.participant.as_deref().filter(|i| !i.is_empty()) {
        if let Some(at) = people.iter().position(|p| p.id == id) {
            return Owner::Person(at);
        }
    }
    match (task.github.as_deref(), task.participant.as_deref()) {
        (Some(login), _) if !login.is_empty() => Owner::Other(format!("@{login}")),
        (_, Some(id)) if !id.is_empty() => Owner::Other(id.to_string()),
        _ => Owner::Nobody,
    }
}

fn by_due(a: &TaskEntry, b: &TaskEntry) -> std::cmp::Ordering {
    // `YYYY-MM-DD` sorts as a date; no due date goes last.
    let due = match (&a.due, &b.due) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    };
    due.then_with(|| a.text.to_lowercase().cmp(&b.text.to_lowercase()))
        .then_with(|| a.id.cmp(&b.id))
}

/// Groups open tasks by person.
///
/// The groups come in this order: the cycle's participants in the order of `cycle.yaml`, other
/// people by name, then the unassigned tasks; a group without tasks is left out. `today` is
/// `YYYY-MM-DD`; a task whose due date is before it is overdue (a task due today is not). A
/// `today` that is not a date marks nothing overdue.
pub fn group_tasks(tasks: &[TaskItem], people: &[Participant], today: &str) -> Vec<TaskGroup> {
    let today_ok = is_iso_date(today);
    let entry = |t: &TaskItem| {
        let due = t.due.clone().filter(|d| is_iso_date(d));
        TaskEntry {
            id: t.id.clone(),
            text: t.text.clone(),
            overdue: today_ok && due.as_deref().is_some_and(|d| d < today),
            due,
            origin: t.origin,
            issue: t.issue,
            url: t.url.clone(),
            meeting: t.meeting.clone(),
        }
    };
    let mut of_person: Vec<Vec<TaskEntry>> = vec![Vec::new(); people.len()];
    let mut others: BTreeMap<String, Vec<TaskEntry>> = BTreeMap::new();
    let mut nobody: Vec<TaskEntry> = Vec::new();
    for task in tasks {
        match owner_of(task, people) {
            Owner::Person(at) => of_person[at].push(entry(task)),
            Owner::Other(name) => others.entry(name).or_default().push(entry(task)),
            Owner::Nobody => nobody.push(entry(task)),
        }
    }
    let mut groups = Vec::new();
    for (person, mut list) in people.iter().zip(of_person) {
        if list.is_empty() {
            continue;
        }
        list.sort_by(by_due);
        groups.push(TaskGroup {
            kind: GroupKind::Participant,
            participant_id: Some(person.id.clone()),
            name: person.name.clone(),
            github: person.github.clone(),
            tasks: list,
        });
    }
    for (name, mut list) in others {
        list.sort_by(by_due);
        groups.push(TaskGroup {
            kind: GroupKind::Other,
            participant_id: None,
            github: name.strip_prefix('@').map(String::from),
            name,
            tasks: list,
        });
    }
    if !nobody.is_empty() {
        nobody.sort_by(by_due);
        groups.push(TaskGroup {
            kind: GroupKind::Unassigned,
            participant_id: None,
            name: "Unassigned".to_string(),
            github: None,
            tasks: nobody,
        });
    }
    groups
}

/// The open topics, in the order of the file.
pub fn open_topics(topics: &[Topic]) -> Vec<Topic> {
    topics
        .iter()
        .filter(|t| t.status == TopicStatus::Open)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::fixtures::{draft, item};
    use crate::draft::Item;

    fn person(id: &str, name: &str, github: Option<&str>) -> Participant {
        Participant {
            id: id.into(),
            name: name.into(),
            github: github.map(String::from),
            discord: None,
        }
    }

    fn people() -> Vec<Participant> {
        vec![
            person("alex", "Alex Example", Some("alex-example")),
            person("sam", "Sam Sample", Some("Sam-Sample")),
            person("kasia", "Kasia Demo", None),
        ]
    }

    fn issue(n: u64, title: &str, assignee: Option<&str>) -> OpenIssue {
        OpenIssue {
            number: n,
            title: title.into(),
            assignee: assignee.map(String::from),
            url: Some(format!("https://example.invalid/issues/{n}")),
        }
    }

    fn task(id: &str, text: &str, who: Option<&str>, due: Option<&str>) -> TaskItem {
        TaskItem {
            id: id.into(),
            text: text.into(),
            origin: TaskOrigin::Meeting,
            github: None,
            participant: who.map(String::from),
            due: due.map(String::from),
            issue: None,
            relates_to: None,
            url: None,
            meeting: Some("2026-11-13".into()),
        }
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

    fn ids(group: &TaskGroup) -> Vec<&str> {
        group.tasks.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn tasks_are_grouped_by_person_in_the_order_of_the_cycle() {
        let tasks = vec![
            task("a", "Order parts", Some("sam"), None),
            task("b", "Design the mount", Some("alex"), None),
            task("c", "Write the report", Some("sam"), None),
        ];
        let groups = group_tasks(&tasks, &people(), "2026-11-13");
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Alex Example", "Sam Sample"]);
        assert!(groups.iter().all(|g| g.kind == GroupKind::Participant));
        assert_eq!(groups[1].participant_id.as_deref(), Some("sam"));
        assert_eq!(ids(&groups[1]), ["a", "c"]);
    }

    #[test]
    fn a_github_login_maps_to_the_participant_ignoring_case() {
        let mut from_issue = task("#7", "Wiring", None, None);
        from_issue.github = Some("SAM-sample".into());
        let mut other_case = task("#8", "Battery", None, None);
        other_case.github = Some("ALEX-EXAMPLE".into());
        let groups = group_tasks(&[from_issue, other_case], &people(), "2026-11-13");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Alex Example");
        assert_eq!(groups[1].name, "Sam Sample");
        assert_eq!(groups[1].github.as_deref(), Some("Sam-Sample"));
    }

    #[test]
    fn an_unknown_login_gets_a_group_of_its_own() {
        let mut stranger = task("#9", "Review the wiring", None, None);
        stranger.github = Some("octo-guest".into());
        let gone = task("m/1", "Old task", Some("peter"), None);
        let mut also = task("#10", "Another", None, None);
        also.github = Some("octo-guest".into());
        let groups = group_tasks(
            &[stranger, gone, also, task("x", "Mine", Some("kasia"), None)],
            &people(),
            "2026-11-13",
        );
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].name, "Kasia Demo");
        let others: Vec<(&str, GroupKind)> = groups[1..]
            .iter()
            .map(|g| (g.name.as_str(), g.kind))
            .collect();
        // Other people are listed by name; a participant id that left the cycle counts too.
        assert_eq!(
            others,
            [
                ("@octo-guest", GroupKind::Other),
                ("peter", GroupKind::Other)
            ]
        );
        assert_eq!(groups[1].github.as_deref(), Some("octo-guest"));
        assert_eq!(ids(&groups[1]), ["#10", "#9"]);
        assert_eq!(groups[2].github, None);
    }

    #[test]
    fn a_login_that_matches_nobody_falls_back_to_the_participant_id() {
        let mut t = task("m/1", "Both", Some("alex"), None);
        t.github = Some("renamed-account".into());
        let groups = group_tasks(&[t], &people(), "2026-11-13");
        assert_eq!(groups[0].name, "Alex Example");
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn unassigned_tasks_come_last() {
        let tasks = vec![
            task("n", "Nobody's task", None, Some("2026-11-01")),
            task("a", "Alex's task", Some("alex"), None),
        ];
        let groups = group_tasks(&tasks, &people(), "2026-11-13");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].kind, GroupKind::Unassigned);
        assert_eq!(groups[1].name, "Unassigned");
        assert!(groups[1].participant_id.is_none());
        // An empty login or id is no owner either.
        let mut blank = task("b", "Blank", Some(""), None);
        blank.github = Some(String::new());
        let groups = group_tasks(&[blank], &people(), "2026-11-13");
        assert_eq!(groups[0].kind, GroupKind::Unassigned);
    }

    #[test]
    fn tasks_are_sorted_by_due_date_and_marked_overdue_by_the_date_passed_in() {
        let tasks = vec![
            task("none", "No date", Some("alex"), None),
            task("late", "Later", Some("alex"), Some("2026-12-01")),
            task("today", "Today", Some("alex"), Some("2026-11-13")),
            task("old", "Long ago", Some("alex"), Some("2026-10-30")),
            task("bad", "Not a date", Some("alex"), Some("next week")),
        ];
        let groups = group_tasks(&tasks, &people(), "2026-11-13");
        // Without a (valid) date the order falls back to the text: "No date", "Not a date".
        assert_eq!(ids(&groups[0]), ["old", "today", "late", "none", "bad"]);
        let overdue: Vec<&str> = groups[0]
            .tasks
            .iter()
            .filter(|t| t.overdue)
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(overdue, ["old"], "due today is not overdue");
        assert_eq!(groups[0].overdue_count(), 1);
        assert_eq!(groups[0].tasks[4].due, None, "an invalid date is no date");

        // The same tasks a week later: three are overdue. The date comes from the caller.
        let later = group_tasks(&tasks, &people(), "2026-11-20");
        assert_eq!(later[0].overdue_count(), 2);
        // A bad date marks nothing.
        let none = group_tasks(&tasks, &people(), "soon");
        assert_eq!(none[0].overdue_count(), 0);
    }

    #[test]
    fn ties_in_the_due_date_are_broken_by_text() {
        let tasks = vec![
            task("2", "b task", Some("alex"), Some("2026-11-20")),
            task("1", "A task", Some("alex"), Some("2026-11-20")),
        ];
        let groups = group_tasks(&tasks, &people(), "2026-11-13");
        assert_eq!(ids(&groups[0]), ["1", "2"]);
    }

    fn pointing(id: &str, text: &str, issue: u64, due: Option<&str>) -> TaskItem {
        let mut t = task(id, text, Some("alex"), due);
        t.issue = Some(issue);
        t.relates_to = Some(format!("#{issue}"));
        t
    }

    #[test]
    fn a_meeting_task_that_points_to_an_issue_is_shown_once() {
        let issues = [issue(12, "Order the battery", Some("sam-sample"))];
        let merged = merge_tasks(
            &issues,
            true,
            vec![pointing(
                "m/1",
                "Order the battery today",
                12,
                Some("2026-11-20"),
            )],
            &[],
        );
        assert_eq!(merged.open.len(), 1, "{:?}", merged.open);
        let shown = &merged.open[0];
        assert_eq!(shown.id, "#12");
        assert_eq!(shown.text, "Order the battery", "the issue is the source");
        assert_eq!(
            shown.due.as_deref(),
            Some("2026-11-20"),
            "the due date comes along"
        );
        assert_eq!(shown.github.as_deref(), Some("sam-sample"));
        assert_eq!(
            shown.participant, None,
            "an assigned issue keeps its assignee"
        );
        assert!(merged.done.is_empty());

        // An issue without an assignee takes the meeting's.
        let merged = merge_tasks(
            &[issue(12, "Order the battery", None)],
            true,
            vec![pointing("m/1", "x", 12, None)],
            &[],
        );
        assert_eq!(merged.open[0].participant.as_deref(), Some("alex"));
        let groups = group_tasks(&merged.open, &people(), "2026-11-13");
        assert_eq!(groups[0].name, "Alex Example");
        assert_eq!(groups.iter().map(|g| g.tasks.len()).sum::<usize>(), 1);
    }

    #[test]
    fn the_pointer_may_be_written_with_or_without_the_hash() {
        assert_eq!(issue_number("#12"), Some(12));
        assert_eq!(issue_number(" 12 "), Some(12));
        assert_eq!(issue_number("t-1"), None);
        assert_eq!(issue_number("#"), None);
        assert_eq!(issue_number("12a"), None);
        assert_eq!(issue_number(""), None);
    }

    #[test]
    fn a_task_whose_issue_is_closed_is_done_but_only_when_the_issues_were_read() {
        let linked = pointing("m/1", "Order the battery", 12, None);
        let read = merge_tasks(&[issue(3, "Other", None)], true, vec![linked.clone()], &[]);
        assert_eq!(read.open.len(), 1);
        assert_eq!(read.open[0].id, "#3");
        assert_eq!(read.done.len(), 1);
        assert_eq!(read.done[0].id, "m/1");

        // Never fetched: an empty list says nothing, the task stays open.
        let unknown = merge_tasks(&[], false, vec![linked], &[]);
        assert_eq!(unknown.open.len(), 1);
        assert!(unknown.done.is_empty());
    }

    #[test]
    fn a_task_of_a_closed_topic_is_done_and_one_of_an_open_topic_is_not() {
        let mut closed = task("m/1", "Order the battery", Some("alex"), None);
        closed.relates_to = Some("t-2".into());
        let mut open = task("m/2", "Pick the wheels", Some("alex"), None);
        open.relates_to = Some("t-1".into());
        let topics = [
            topic("t-1", "Wheel size", TopicStatus::Open),
            topic("t-2", "Battery", TopicStatus::Closed),
        ];
        let merged = merge_tasks(&[], true, vec![closed, open], &topics);
        assert_eq!(merged.done.len(), 1);
        assert_eq!(merged.done[0].id, "m/1");
        assert_eq!(merged.open.len(), 1);
        assert_eq!(merged.open[0].id, "m/2");
    }

    #[test]
    fn a_task_repeated_in_a_later_meeting_is_shown_once_from_the_newest() {
        let newest = task(
            "2026-11-13/a",
            "  Design the  mount ",
            Some("alex"),
            Some("2026-11-27"),
        );
        let older = task(
            "2026-11-06/a",
            "design the mount",
            Some("alex"),
            Some("2026-11-13"),
        );
        let other_person = task("2026-11-06/b", "Design the mount", Some("sam"), None);
        let merged = merge_tasks(&[], true, vec![newest, older, other_person], &[]);
        let kept: Vec<&str> = merged.open.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(kept, ["2026-11-13/a", "2026-11-06/b"]);
    }

    #[test]
    fn only_open_topics_are_kept_in_the_order_of_the_file() {
        let topics = [
            topic("t-1", "Wheel size", TopicStatus::Open),
            topic("t-2", "Battery", TopicStatus::Closed),
            topic("t-3", "Antenna", TopicStatus::Open),
        ];
        let open = open_topics(&topics);
        let ids: Vec<&str> = open.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t-1", "t-3"]);
        assert!(open_topics(&[topic("t", "x", TopicStatus::Closed)]).is_empty());
    }

    fn write_meeting(root: &Path, folder: &str, meeting_id: &str, date: &str, items: Vec<Item>) {
        let mut d = draft(items);
        d.meeting_id = meeting_id.into();
        d.date = date.into();
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("meeting.json"),
            serde_json::to_string_pretty(&d.to_value()).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn meeting_files_give_their_tasks_newest_first_and_skip_broken_ones() {
        let dir = testkit::target_tempdir("briefing");
        let root = dir.path();
        let mut a = item("i1", ItemType::Task, "Design the mount");
        a.assignee = Some("alex".into());
        a.due = Some("2026-11-20".into());
        let note = item("i2", ItemType::Note, "Wheels are 20 cm");
        let mut linked = item("i3", ItemType::Task, "Order the battery");
        linked.relates_to = Some("#12".into());
        linked.due = Some("2026-13-45".into());
        write_meeting(root, "2026-11-06", "m-old", "2026-11-06", vec![linked]);
        write_meeting(root, "2026-11-13", "m-new", "2026-11-13", vec![a, note]);
        // The second meeting of a day sorts after the first.
        let second = item("i9", ItemType::Task, "Second meeting task");
        write_meeting(root, "2026-11-13-2", "m-new-2", "2026-11-13", vec![second]);
        fs::create_dir_all(root.join("2026-11-20")).unwrap();
        fs::write(root.join("2026-11-20").join("meeting.json"), "{ nope").unwrap();
        fs::create_dir_all(root.join("no-file")).unwrap();

        let (tasks, warnings) = meeting_tasks(root);
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["2026-11-13-2/i9", "2026-11-13/i1", "2026-11-06/i3"]);
        assert_eq!(tasks[1].participant.as_deref(), Some("alex"));
        assert_eq!(tasks[1].due.as_deref(), Some("2026-11-20"));
        assert_eq!(tasks[2].issue, Some(12));
        assert_eq!(
            tasks[2].due, None,
            "a due date that is not a date is dropped"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("skipped 2026-11-20/meeting.json"));

        // A folder that does not exist is no tasks and no warning.
        let (none, w) = meeting_tasks(&root.join("absent"));
        assert!(none.is_empty() && w.is_empty());
    }
}
