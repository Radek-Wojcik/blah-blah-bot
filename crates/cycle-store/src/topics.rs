//! `cycles/<ID>/topics.yaml`: the list of open and closed topics of a cycle.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CycleError, Result, ValidationIssue};
use crate::schema::{self, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TopicStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topic {
    pub id: String,
    pub text: String,
    pub status: TopicStatus,
    /// Meeting folder name (`2026-11-13`, `2026-11-13-2`) in which the topic was raised.
    pub opened_in: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_in: Option<String>,
}

/// Parses `topics.yaml`. Empty text is an empty list.
pub fn parse_topics(text: &str) -> std::result::Result<Vec<Topic>, Vec<ValidationIssue>> {
    let value = schema::yaml_to_json(text).map_err(|i| vec![i])?;
    if value.is_null() {
        return Ok(Vec::new());
    }
    let issues = schema::validate(Schema::Topics, &value);
    if !issues.is_empty() {
        return Err(issues);
    }
    let topics: Vec<Topic> = serde_json::from_value(value)
        .map_err(|e| vec![ValidationIssue::new("", format!("cannot read topics: {e}"))])?;
    let issues = semantic_issues(&topics);
    if issues.is_empty() {
        Ok(topics)
    } else {
        Err(issues)
    }
}

/// Topic ids must be unique.
pub fn semantic_issues(topics: &[Topic]) -> Vec<ValidationIssue> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut issues = Vec::new();
    for (i, t) in topics.iter().enumerate() {
        if let Some(first) = seen.insert(t.id.as_str(), i) {
            issues.push(ValidationIssue::new(
                format!("[{i}].id"),
                format!("duplicate topic id \"{}\" (first used at [{first}])", t.id),
            ));
        }
    }
    issues
}

/// The YAML text of a topic list, validated first.
pub fn topics_to_yaml(topics: &[Topic]) -> Result<String> {
    let value = serde_json::to_value(topics).map_err(|e| CycleError::Other(e.to_string()))?;
    let mut issues = schema::validate(Schema::Topics, &value);
    issues.extend(semantic_issues(topics));
    if !issues.is_empty() {
        return Err(CycleError::invalid("topics", issues));
    }
    serde_norway::to_string(topics)
        .map_err(|e| CycleError::Other(format!("cannot write YAML: {e}")))
}

// ----- merging ---------------------------------------------------------------------------

/// A topic changed from `before` to `after`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopicChange {
    pub id: String,
    pub before: Topic,
    pub after: Topic,
}

/// What publishing does to `topics.yaml`, against what the remote has: the structure the
/// publish screen shows before the commit and the result carries afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TopicsDiff {
    pub added: Vec<Topic>,
    pub changed: Vec<TopicChange>,
    pub removed: Vec<Topic>,
}

impl TopicsDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

/// A topic that was changed on the remote and by us: ours was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopicConflict {
    pub id: String,
    /// The remote's version; `None` when it is not there (it was removed meanwhile).
    pub theirs: Option<Topic>,
    /// Our version, the one that was kept; `None` when we removed the topic.
    pub ours: Option<Topic>,
}

/// The result of [`merge`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TopicsMerge {
    /// The list to write.
    pub topics: Vec<Topic>,
    /// The merged list against `upstream`: what the commit changes.
    pub diff: TopicsDiff,
    /// Topics where our version replaced a different change of the remote.
    pub conflicts: Vec<TopicConflict>,
}

/// The changes needed to go from `before` to `after`, by topic id: topics only in `after`
/// are added (in `after`'s order), topics in both with another content changed, topics only
/// in `before` removed.
pub fn diff(before: &[Topic], after: &[Topic]) -> TopicsDiff {
    let old: BTreeMap<&str, &Topic> = before.iter().map(|t| (t.id.as_str(), t)).collect();
    let new: BTreeMap<&str, &Topic> = after.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut out = TopicsDiff::default();
    for topic in after {
        match old.get(topic.id.as_str()) {
            None => out.added.push(topic.clone()),
            Some(prev) if *prev != topic => out.changed.push(TopicChange {
                id: topic.id.clone(),
                before: (*prev).clone(),
                after: topic.clone(),
            }),
            Some(_) => {}
        }
    }
    out.removed = before
        .iter()
        .filter(|t| !new.contains_key(t.id.as_str()))
        .cloned()
        .collect();
    out
}

/// Merges the topics of three sides, the last write winning **per topic**:
///
/// * `base`: the topics where our work started (the local clone's last commit),
/// * `ours`: the topics now, with our edits and the topics the new meeting adds,
/// * `upstream`: the topics on the remote.
///
/// A topic that differs between `base` and `ours` (new, changed or removed) is ours and
/// overrides the remote's. Every other topic is the remote's, unchanged. When the remote
/// also changed a topic of ours to something different, ours wins and a [`TopicConflict`]
/// is recorded, so the screen can say what was overridden. Remote order is kept; our new
/// topics follow it in our order.
pub fn merge(upstream: &[Topic], base: &[Topic], ours: &[Topic]) -> TopicsMerge {
    let base_by: BTreeMap<&str, &Topic> = base.iter().map(|t| (t.id.as_str(), t)).collect();
    let ours_by: BTreeMap<&str, &Topic> = ours.iter().map(|t| (t.id.as_str(), t)).collect();
    let up_by: BTreeMap<&str, &Topic> = upstream.iter().map(|t| (t.id.as_str(), t)).collect();

    // What we changed, by id: `Some` the new version, `None` a removal.
    let mut ours_changed: BTreeMap<&str, Option<&Topic>> = BTreeMap::new();
    for topic in ours {
        if base_by.get(topic.id.as_str()) != Some(&topic) {
            ours_changed.insert(topic.id.as_str(), Some(topic));
        }
    }
    for topic in base {
        if !ours_by.contains_key(topic.id.as_str()) {
            ours_changed.insert(topic.id.as_str(), None);
        }
    }

    let mut conflicts = Vec::new();
    for (id, mine) in &ours_changed {
        let theirs = up_by.get(id).copied();
        let theirs_changed = theirs != base_by.get(id).copied();
        // Both sides ended up with the same thing: nothing to report.
        if theirs_changed && theirs != *mine {
            conflicts.push(TopicConflict {
                id: (*id).to_string(),
                theirs: theirs.cloned(),
                ours: mine.cloned(),
            });
        }
    }

    let mut topics = Vec::new();
    for topic in upstream {
        match ours_changed.get(topic.id.as_str()) {
            None => topics.push(topic.clone()),
            Some(Some(mine)) => topics.push((*mine).clone()),
            Some(None) => {}
        }
    }
    for topic in ours {
        let ours_new = ours_changed
            .get(topic.id.as_str())
            .is_some_and(Option::is_some);
        if ours_new && !up_by.contains_key(topic.id.as_str()) {
            topics.push(topic.clone());
        }
    }
    let diff = diff(upstream, &topics);
    TopicsMerge {
        topics,
        diff,
        conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(id: &str, text: &str, status: TopicStatus) -> Topic {
        Topic {
            id: id.into(),
            text: text.into(),
            status,
            opened_in: "2026-11-06".into(),
            closed_in: (status == TopicStatus::Closed).then(|| "2026-11-13".to_string()),
        }
    }

    fn open(id: &str, text: &str) -> Topic {
        topic(id, text, TopicStatus::Open)
    }

    fn ids(list: &[Topic]) -> Vec<&str> {
        list.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn our_new_topics_are_added_after_the_remote_ones() {
        let base = vec![open("a", "A")];
        let upstream = vec![open("a", "A"), open("b", "B from the remote")];
        let ours = vec![open("a", "A"), open("2026-11-13-1", "New")];
        let merged = merge(&upstream, &base, &ours);
        assert_eq!(ids(&merged.topics), ["a", "b", "2026-11-13-1"]);
        assert!(merged.conflicts.is_empty());
        assert_eq!(ids(&merged.diff.added), ["2026-11-13-1"]);
        assert!(merged.diff.changed.is_empty() && merged.diff.removed.is_empty());
    }

    #[test]
    fn a_topic_changed_by_one_side_only_is_taken_from_that_side() {
        let base = vec![open("a", "A"), open("b", "B")];
        // The remote closed a; we closed b.
        let upstream = vec![topic("a", "A", TopicStatus::Closed), open("b", "B")];
        let ours = vec![open("a", "A"), topic("b", "B", TopicStatus::Closed)];
        let merged = merge(&upstream, &base, &ours);
        assert_eq!(merged.topics[0].status, TopicStatus::Closed, "theirs kept");
        assert_eq!(merged.topics[1].status, TopicStatus::Closed, "ours applied");
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.diff.changed.len(), 1);
        assert_eq!(merged.diff.changed[0].id, "b");
    }

    #[test]
    fn a_topic_changed_on_both_sides_goes_to_ours_and_is_reported() {
        let base = vec![open("a", "A")];
        let upstream = vec![open("a", "A reworded remotely")];
        let ours = vec![topic("a", "A", TopicStatus::Closed)];
        let merged = merge(&upstream, &base, &ours);
        assert_eq!(merged.topics, ours);
        assert_eq!(merged.conflicts.len(), 1);
        let c = &merged.conflicts[0];
        assert_eq!(c.id, "a");
        assert_eq!(c.theirs.as_ref().unwrap().text, "A reworded remotely");
        assert_eq!(c.ours.as_ref().unwrap().status, TopicStatus::Closed);
    }

    #[test]
    fn the_same_change_on_both_sides_is_no_conflict() {
        let base = vec![open("a", "A")];
        let closed = vec![topic("a", "A", TopicStatus::Closed)];
        let merged = merge(&closed, &base, &closed);
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.topics, closed);
        assert!(merged.diff.is_empty());
    }

    #[test]
    fn a_topic_we_removed_is_removed_unless_it_is_gone_already() {
        let base = vec![open("a", "A"), open("b", "B")];
        let ours = vec![open("b", "B")];
        let upstream = vec![open("a", "A"), open("b", "B")];
        let merged = merge(&upstream, &base, &ours);
        assert_eq!(ids(&merged.topics), ["b"]);
        assert_eq!(ids(&merged.diff.removed), ["a"]);
        assert!(merged.conflicts.is_empty());
        // The remote changed it meanwhile: still removed, and said so.
        let upstream = vec![open("a", "A changed"), open("b", "B")];
        let merged = merge(&upstream, &base, &ours);
        assert_eq!(ids(&merged.topics), ["b"]);
        assert_eq!(merged.conflicts.len(), 1);
        assert!(merged.conflicts[0].ours.is_none());
    }

    #[test]
    fn a_topic_new_on_both_sides_with_the_same_id_goes_to_ours() {
        let upstream = vec![open("x", "theirs")];
        let ours = vec![open("x", "ours")];
        let merged = merge(&upstream, &[], &ours);
        assert_eq!(merged.topics[0].text, "ours");
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].theirs.as_ref().unwrap().text, "theirs");
    }

    #[test]
    fn nothing_changed_gives_the_remote_list_untouched() {
        let base = vec![open("a", "A")];
        let upstream = vec![open("a", "A"), open("c", "C")];
        let merged = merge(&upstream, &base, &base);
        assert_eq!(merged.topics, upstream);
        assert!(merged.diff.is_empty() && merged.conflicts.is_empty());
        assert_eq!(merge(&[], &[], &[]), TopicsMerge::default());
    }

    #[test]
    fn diff_lists_added_changed_and_removed() {
        let before = vec![open("a", "A"), open("b", "B"), open("c", "C")];
        let after = vec![
            open("a", "A"),
            topic("b", "B", TopicStatus::Closed),
            open("d", "D"),
        ];
        let d = diff(&before, &after);
        assert_eq!(ids(&d.added), ["d"]);
        assert_eq!(d.changed[0].id, "b");
        assert_eq!(d.changed[0].before.status, TopicStatus::Open);
        assert_eq!(ids(&d.removed), ["c"]);
        assert!(diff(&before, &before).is_empty());
    }

    const SAMPLE: &str = "- id: t-1\n  text: Decide the rover wheel size\n  status: open\n  opened_in: 2026-11-06\n- id: t-2\n  text: Order the battery\n  status: closed\n  opened_in: 2026-11-06\n  closed_in: 2026-11-13-2\n";

    #[test]
    fn parses_and_round_trips() {
        let topics = parse_topics(SAMPLE).unwrap();
        assert_eq!(topics.len(), 2);
        assert_eq!(topics[1].status, TopicStatus::Closed);
        assert_eq!(topics[1].closed_in.as_deref(), Some("2026-11-13-2"));
        let yaml = topics_to_yaml(&topics).unwrap();
        assert_eq!(parse_topics(&yaml).unwrap(), topics);
        assert_eq!(topics_to_yaml(&topics).unwrap(), yaml);
    }

    #[test]
    fn empty_list_and_empty_file() {
        assert!(parse_topics("").unwrap().is_empty());
        assert!(parse_topics("[]\n").unwrap().is_empty());
        let yaml = topics_to_yaml(&[]).unwrap();
        assert!(parse_topics(&yaml).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_topics() {
        let bad_status = "- { id: t, text: x, status: pending, opened_in: 2026-11-06 }\n";
        assert!(parse_topics(bad_status)
            .unwrap_err()
            .iter()
            .any(|i| i.path == "[0].status"));
        let missing = "- { id: t, text: x, status: open }\n";
        assert!(!parse_topics(missing).unwrap_err().is_empty());
        let bad_date = "- { id: t, text: x, status: open, opened_in: yesterday }\n";
        assert!(parse_topics(bad_date)
            .unwrap_err()
            .iter()
            .any(|i| i.path == "[0].opened_in"));
        let dup = "- { id: t, text: a, status: open, opened_in: 2026-11-06 }\n- { id: t, text: b, status: open, opened_in: 2026-11-06 }\n";
        assert!(parse_topics(dup).unwrap_err()[0]
            .message
            .contains("duplicate"));
        assert!(!parse_topics("id: t\n").unwrap_err().is_empty());
    }
}
