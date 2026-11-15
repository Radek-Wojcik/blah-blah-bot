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

#[cfg(test)]
mod tests {
    use super::*;

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
