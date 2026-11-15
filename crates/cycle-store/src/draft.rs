//! Typed mirror of `schemas/meeting.schema.json`: the draft report of one meeting
//! (`work/draft.json`, later `meeting.json` in the cycle repository).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ValidationIssue;
use crate::schema::{self, Schema};

/// Language setting of a cycle or a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanguageSetting {
    #[default]
    Auto,
    Pl,
    En,
}

/// A resolved output language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Pl,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportMode {
    #[default]
    Fixed,
    Template,
    Auto,
    ByType,
}

impl ReportMode {
    /// Modes in which the sections come from a template file.
    pub fn uses_template_sections(self) -> bool {
        matches!(self, ReportMode::Template | ReportMode::ByType)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    Task,
    Decision,
    Topic,
    Note,
    Idea,
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertaintyLevel {
    Stated,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certainty {
    pub assignee: CertaintyLevel,
    pub due: CertaintyLevel,
}

/// A section items can be assigned to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Where an item came from. Times are milliseconds on the session clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub track: String,
    pub speaker: String,
    pub t_start_ms: u64,
    pub t_end_ms: u64,
    pub quote: String,
    pub verified: bool,
    #[serde(rename = "match")]
    pub match_score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub text: String,
    /// Participant id.
    pub assignee: Option<String>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    pub certainty: Certainty,
    /// Section id; `None` means the built-in section of the type (fixed mode) or the last
    /// section.
    pub section: Option<String>,
    pub relates_to: Option<String>,
    /// `None` for items added by hand.
    pub source: Option<Source>,
    pub edited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    WindowSkipped,
    QuoteUnverified,
    AssigneeUnknown,
    SummaryFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub code: WarningCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_start_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_end_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub version: u32,
    pub meeting_id: String,
    pub cycle: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub language: Language,
    pub mode: ReportMode,
    pub template: Option<String>,
    pub meeting_type: Option<String>,
    pub sections: Vec<Section>,
    pub summary: String,
    pub items: Vec<Item>,
    pub warnings: Vec<Warning>,
}

impl Draft {
    /// Validates a JSON document against `schemas/meeting.schema.json` and converts it.
    pub fn from_value(value: &Value) -> Result<Draft, Vec<ValidationIssue>> {
        let issues = schema::validate(Schema::Meeting, value);
        if !issues.is_empty() {
            return Err(issues);
        }
        serde_json::from_value(value.clone()).map_err(|e| {
            vec![ValidationIssue::new(
                "",
                format!("cannot read meeting data: {e}"),
            )]
        })
    }

    pub fn from_json_str(text: &str) -> Result<Draft, Vec<ValidationIssue>> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| vec![ValidationIssue::new("", format!("invalid JSON: {e}"))])?;
        Self::from_value(&value)
    }

    /// The document as JSON, ready to be written as `meeting.json`.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a draft always serializes")
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn item(id: &str, t: ItemType, text: &str) -> Item {
        Item {
            id: id.into(),
            item_type: t,
            text: text.into(),
            assignee: None,
            due: None,
            certainty: Certainty {
                assignee: CertaintyLevel::Unknown,
                due: CertaintyLevel::Unknown,
            },
            section: None,
            relates_to: None,
            source: None,
            edited: false,
        }
    }

    pub fn source(verified: bool) -> Source {
        Source {
            track: "app:discord".into(),
            speaker: "alex".into(),
            t_start_ms: 1000,
            t_end_ms: 4000,
            quote: "quote".into(),
            verified,
            match_score: if verified { 0.97 } else { 0.4 },
        }
    }

    pub fn draft(items: Vec<Item>) -> Draft {
        Draft {
            version: 1,
            meeting_id: "m-1".into(),
            cycle: "NOMAD".into(),
            date: "2026-11-13".into(),
            language: Language::En,
            mode: ReportMode::Fixed,
            template: None,
            meeting_type: None,
            sections: vec![],
            summary: "We agreed on the mount.".into(),
            items,
            warnings: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn draft_round_trips_and_validates() {
        let mut t = item("i1", ItemType::Task, "Design the mount");
        t.assignee = Some("alex".into());
        t.due = Some("2026-11-20".into());
        t.source = Some(source(true));
        let d = draft(vec![t]);
        let value = d.to_value();
        assert!(schema::validate(Schema::Meeting, &value).is_empty());
        assert_eq!(Draft::from_value(&value).unwrap(), d);
    }

    #[test]
    fn invalid_draft_is_rejected_with_a_path() {
        let mut value = draft(vec![item("i1", ItemType::Note, "x")]).to_value();
        value["items"][0]["type"] = serde_json::json!("meme");
        let issues = Draft::from_value(&value).unwrap_err();
        assert!(
            issues.iter().any(|i| i.path == "items[0].type"),
            "{issues:?}"
        );
    }
}
