//! JSON Schema validation against the files under `schemas/`, and YAML loading.

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

use crate::error::ValidationIssue;

/// The schemas this crate validates against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    /// `cycles/<ID>/cycle.yaml`
    Cycle,
    /// `cycles/<ID>/topics.yaml`
    Topics,
    /// `meeting.json` and `work/draft.json`
    Meeting,
    /// `work/context.json`
    Context,
    /// `work/corrections.json`
    Corrections,
}

impl Schema {
    /// The schema document.
    pub fn source(self) -> &'static str {
        match self {
            Schema::Cycle => include_str!("../../../schemas/cycle.schema.json"),
            Schema::Topics => include_str!("../../../schemas/topics.schema.json"),
            Schema::Meeting => include_str!("../../../schemas/meeting.schema.json"),
            Schema::Context => include_str!("../../../schemas/cycle-context.schema.json"),
            Schema::Corrections => include_str!("../../../schemas/corrections.schema.json"),
        }
    }

    fn validator(self) -> &'static Validator {
        static CYCLE: OnceLock<Validator> = OnceLock::new();
        static TOPICS: OnceLock<Validator> = OnceLock::new();
        static MEETING: OnceLock<Validator> = OnceLock::new();
        static CONTEXT: OnceLock<Validator> = OnceLock::new();
        static CORRECTIONS: OnceLock<Validator> = OnceLock::new();
        let cell = match self {
            Schema::Cycle => &CYCLE,
            Schema::Topics => &TOPICS,
            Schema::Meeting => &MEETING,
            Schema::Context => &CONTEXT,
            Schema::Corrections => &CORRECTIONS,
        };
        cell.get_or_init(|| {
            let doc: Value =
                serde_json::from_str(self.source()).expect("bundled schema is valid JSON");
            jsonschema::validator_for(&doc).expect("bundled schema compiles")
        })
    }
}

/// Validates `value` and returns every violation, sorted by path.
pub fn validate(schema: Schema, value: &Value) -> Vec<ValidationIssue> {
    let mut issues: Vec<ValidationIssue> = schema
        .validator()
        .iter_errors(value)
        .map(|e| ValidationIssue::new(pointer_to_path(&e.instance_path.to_string()), e.to_string()))
        .collect();
    issues.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.message.cmp(&b.message)));
    issues.dedup();
    issues
}

/// `/participants/0/id` becomes `participants[0].id`.
pub fn pointer_to_path(pointer: &str) -> String {
    let mut out = String::new();
    for part in pointer.split('/').skip(1) {
        let part = part.replace("~1", "/").replace("~0", "~");
        if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
            out.push('[');
            out.push_str(&part);
            out.push(']');
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&part);
        }
    }
    out
}

/// Parses YAML text into a JSON value (the form the schemas validate). Empty text is
/// `null`. Only plain data is accepted: tags and non-string keys are reported.
pub fn yaml_to_json(text: &str) -> Result<Value, ValidationIssue> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let yaml: serde_norway::Value = serde_norway::from_str(text)
        .map_err(|e| ValidationIssue::new("", format!("invalid YAML: {e}")))?;
    serde_json::to_value(&yaml)
        .map_err(|e| ValidationIssue::new("", format!("unsupported YAML construct: {e}")))
}

/// True for `^[A-Za-z0-9][A-Za-z0-9_-]*$`, the shape of cycle, participant, topic and
/// template identifiers.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_conversion() {
        assert_eq!(pointer_to_path(""), "");
        assert_eq!(pointer_to_path("/id"), "id");
        assert_eq!(
            pointer_to_path("/participants/1/discord/user_id"),
            "participants[1].discord.user_id"
        );
        assert_eq!(pointer_to_path("/0/status"), "[0].status");
    }

    #[test]
    fn identifiers() {
        assert!(is_identifier("NOMAD"));
        assert!(is_identifier("a_b-c9"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("-a"));
        assert!(!is_identifier("a b"));
        assert!(!is_identifier("../x"));
    }

    #[test]
    fn yaml_conversion_reports_bad_input() {
        assert!(yaml_to_json("a: [1").is_err());
        assert_eq!(yaml_to_json("").unwrap(), Value::Null);
        assert_eq!(yaml_to_json("a: 1").unwrap(), serde_json::json!({"a": 1}));
    }
}
