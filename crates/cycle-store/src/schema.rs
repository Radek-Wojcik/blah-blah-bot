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
        .map(|e| {
            ValidationIssue::new(
                pointer_to_path(&e.instance_path.to_string()),
                match &e.kind {
                    // The name comes from the schema, not from the data.
                    jsonschema::error::ValidationErrorKind::Required { property } => {
                        format!("{property} is a required property")
                    }
                    kind => redacted_message(&format!("{kind:?}")),
                },
            )
        })
        .collect();
    issues.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.message.cmp(&b.message)));
    issues.dedup();
    issues
}

/// A message that names the violated keyword and nothing of the offending value: the library
/// text quotes the value, which may be a pasted token or webhook address.
fn redacted_message(kind_debug: &str) -> String {
    let keyword: String = kind_debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("does not satisfy the schema rule `{keyword}`")
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

/// Windows reserved DOS device names (case-insensitive, with or without an extension).
pub fn is_windows_reserved_name(s: &str) -> bool {
    let stem = s.split('.').next().unwrap_or("");
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// True for `^[A-Za-z0-9][A-Za-z0-9_-]*$` excluding Windows reserved device names, the
/// shape of cycle, participant, topic and template identifiers.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !is_windows_reserved_name(s)
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
    fn validation_messages_do_not_echo_the_value() {
        let secret = "https://discord.com/api/webhooks/123456789012345678/FAKE-token_ABCdef";
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"id": {"type": "integer"}, "n": {"enum": ["a"]},
                           "p": {"type": "string", "pattern": "^x$"}},
            "additionalProperties": false
        });
        let validator = jsonschema::validator_for(&schema).unwrap();
        let value = serde_json::json!({"id": secret, "n": secret, "p": secret, "extra": secret});
        let mut seen = 0;
        for e in validator.iter_errors(&value) {
            let m = redacted_message(&format!("{:?}", e.kind));
            assert!(!m.contains("FAKE") && !m.contains("discord"), "{m}");
            assert!(m.contains('`') && m.len() > 30, "{m}");
            seen += 1;
        }
        assert!(seen >= 4);
        let issues = validate(
            Schema::Cycle,
            &serde_json::json!({"id": secret, "x": secret}),
        );
        assert!(!issues.is_empty());
        for i in &issues {
            assert!(
                !i.message.contains("FAKE") && !i.message.contains("webhooks"),
                "{i:?}"
            );
        }
    }

    #[test]
    fn identifiers() {
        assert!(is_identifier("NOMAD"));
        assert!(is_identifier("a_b-c9"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("-a"));
        assert!(!is_identifier("a b"));
        assert!(!is_identifier("../x"));
        for reserved in [
            "CON", "con", "PRN", "aux", "NUL", "COM1", "com9", "LPT1", "lpt9",
        ] {
            assert!(!is_identifier(reserved), "{reserved}");
        }
        assert!(is_identifier("COM0"));
        assert!(is_identifier("CON1"));
    }

    #[test]
    fn yaml_conversion_reports_bad_input() {
        assert!(yaml_to_json("a: [1").is_err());
        assert_eq!(yaml_to_json("").unwrap(), Value::Null);
        assert_eq!(yaml_to_json("a: 1").unwrap(), serde_json::json!({"a": 1}));
    }
}
