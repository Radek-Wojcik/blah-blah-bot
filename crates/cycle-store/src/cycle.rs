//! `cycles/<ID>/cycle.yaml`: types, parsing, validation and deterministic writing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::draft::{LanguageSetting, ReportMode};
use crate::error::{CycleError, Result, ValidationIssue};
use crate::schema::{self, Schema};

/// Name of the template every new cycle gets.
pub const DEFAULT_TEMPLATE_NAME: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Languages {
    /// Transcription language.
    #[serde(default)]
    pub asr: LanguageSetting,
    /// Report language.
    #[serde(default)]
    pub output: LanguageSetting,
}

/// Discord identity of a participant: exactly one of the two fields is set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Participant {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<DiscordRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlossaryEntry {
    pub term: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    #[serde(default)]
    pub default_mode: ReportMode,
    #[serde(default = "default_template_name")]
    pub default_template: String,
    #[serde(default)]
    pub instruction: String,
    /// Meeting type to template name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_type: BTreeMap<String, String>,
}

fn default_template_name() -> String {
    DEFAULT_TEMPLATE_NAME.to_string()
}

impl Default for Report {
    fn default() -> Self {
        Self {
            default_mode: ReportMode::Fixed,
            default_template: default_template_name(),
            instruction: String::new(),
            by_type: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublishMode {
    Auto,
    #[default]
    OnApproval,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubTarget {
    #[serde(default = "default_github_path")]
    pub path: String,
    #[serde(default)]
    pub publish: PublishMode,
    /// Create GitHub issues for task items and write back status changes. Left out of the
    /// file while true, so existing files keep their bytes.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub issues: bool,
}

fn default_github_path() -> String {
    "meetings".to_string()
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl Default for GithubTarget {
    fn default() -> Self {
        Self {
            path: default_github_path(),
            publish: PublishMode::OnApproval,
            issues: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordTarget {
    /// A `cred:` reference to the webhook URL in the Credential Manager, never the URL.
    pub webhook: String,
    #[serde(default)]
    pub publish: PublishMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotionTarget {
    pub database: String,
    #[serde(default)]
    pub publish: PublishMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GithubTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<DiscordTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion: Option<NotionTarget>,
}

impl Targets {
    pub fn is_empty(&self) -> bool {
        self.github.is_none() && self.discord.is_none() && self.notion.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub languages: Languages,
    #[serde(default)]
    pub participants: Vec<Participant>,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    pub report: Report,
    #[serde(default, skip_serializing_if = "Targets::is_empty")]
    pub targets: Targets,
}

impl Cycle {
    /// A cycle with defaults: automatic languages, no participants, fixed report mode.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            languages: Languages::default(),
            participants: Vec::new(),
            glossary: Vec::new(),
            report: Report::default(),
            targets: Targets::default(),
        }
    }

    /// Parses `cycle.yaml` text. The cycle is `Some` when the file has the right shape,
    /// even if semantic problems are listed, so a caller can still show it.
    pub fn parse(text: &str) -> (Option<Cycle>, Vec<ValidationIssue>) {
        let value = match schema::yaml_to_json(text) {
            Ok(v) => v,
            Err(issue) => return (None, vec![issue]),
        };
        let mut issues = schema::validate(Schema::Cycle, &value);
        if !issues.is_empty() {
            return (None, issues);
        }
        match serde_json::from_value::<Cycle>(value) {
            Ok(cycle) => {
                issues.extend(cycle.semantic_issues());
                (Some(cycle), issues)
            }
            Err(e) => (
                None,
                vec![ValidationIssue::new("", format!("cannot read cycle: {e}"))],
            ),
        }
    }

    /// Parses and fully validates `cycle.yaml` text (schema and semantic checks, without
    /// the template checks that need the template list).
    pub fn from_yaml_str(text: &str) -> std::result::Result<Cycle, Vec<ValidationIssue>> {
        match Self::parse(text) {
            (Some(cycle), issues) if issues.is_empty() => Ok(cycle),
            (_, issues) => Err(issues),
        }
    }

    /// Checks that do not need the template files: unique participant ids.
    pub fn semantic_issues(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, p) in self.participants.iter().enumerate() {
            if let Some(first) = seen.insert(p.id.as_str(), i) {
                issues.push(ValidationIssue::new(
                    format!("participants[{i}].id"),
                    format!(
                        "duplicate participant id \"{}\" (first used at participants[{first}])",
                        p.id
                    ),
                ));
            }
        }
        issues
    }

    /// Checks against the templates that exist: every `by_type` template, and the default
    /// template when the default mode is `template` or `by-type`.
    pub fn template_issues(&self, available: &[String]) -> Vec<ValidationIssue> {
        let has = |name: &str| available.iter().any(|a| a == name);
        let mut issues = Vec::new();
        if self.report.default_mode.uses_template_sections() && !has(&self.report.default_template)
        {
            issues.push(ValidationIssue::new(
                "report.default_template",
                format!(
                    "template \"{}\" does not exist in templates/ but the default mode is {}",
                    self.report.default_template,
                    mode_name(self.report.default_mode)
                ),
            ));
        }
        for (kind, template) in &self.report.by_type {
            if !has(template) {
                issues.push(ValidationIssue::new(
                    format!("report.by_type.{kind}"),
                    format!("template \"{template}\" does not exist in templates/"),
                ));
            }
        }
        issues
    }

    /// Schema, semantic and template checks together.
    pub fn validate(&self, available_templates: &[String]) -> Vec<ValidationIssue> {
        let mut issues = match serde_json::to_value(self) {
            Ok(value) => schema::validate(Schema::Cycle, &value),
            Err(e) => vec![ValidationIssue::new("", e.to_string())],
        };
        issues.extend(self.semantic_issues());
        issues.extend(self.template_issues(available_templates));
        issues
    }

    /// Looks a participant up by id.
    pub fn participant(&self, id: &str) -> Option<&Participant> {
        self.participants.iter().find(|p| p.id == id)
    }

    /// The YAML text to write: validated first (schema and participant checks), with keys
    /// in declaration order and map entries sorted, so the same cycle always produces the
    /// same bytes.
    pub fn to_yaml(&self) -> Result<String> {
        let mut issues = match serde_json::to_value(self) {
            Ok(value) => schema::validate(Schema::Cycle, &value),
            Err(e) => vec![ValidationIssue::new("", e.to_string())],
        };
        issues.extend(self.semantic_issues());
        if !issues.is_empty() {
            return Err(CycleError::invalid("cycle", issues));
        }
        serde_norway::to_string(self)
            .map_err(|e| CycleError::Other(format!("cannot write YAML: {e}")))
    }
}

fn mode_name(mode: ReportMode) -> &'static str {
    match mode {
        ReportMode::Fixed => "fixed",
        ReportMode::Template => "template",
        ReportMode::Auto => "auto",
        ReportMode::ByType => "by-type",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const FULL: &str = r#"
id: NOMAD
name: Nomad rover
languages:
  asr: auto
  output: pl
participants:
  - id: alex
    name: Alex Example
    github: alex-example
    discord: { user_id: "000000000000000000" }
  - id: sam
    name: Sam Sample
    discord: { role_id: "123456789012345678" }
glossary:
  - term: CAN bus
  - term: GNSS
    hint: positioning module
report:
  default_mode: by-type
  default_template: default
  instruction: "Group tasks by person, keep it short."
  by_type:
    budget: budget-review
    technical: default
targets:
  github: { path: meetings, publish: auto }
  discord: { webhook: "cred:discord/nomad", publish: on-approval, template: short }
  notion: { database: "cred:notion/nomad-db", publish: off }
"#;

    fn issues_of(text: &str) -> Vec<ValidationIssue> {
        Cycle::from_yaml_str(text).unwrap_err()
    }

    fn has_path(issues: &[ValidationIssue], path: &str) -> bool {
        issues.iter().any(|i| i.path == path)
    }

    #[test]
    fn parses_a_full_valid_file() {
        let cycle = Cycle::from_yaml_str(FULL).unwrap();
        assert_eq!(cycle.id, "NOMAD");
        assert_eq!(cycle.languages.output, LanguageSetting::Pl);
        assert_eq!(cycle.participants.len(), 2);
        assert_eq!(
            cycle.participants[0]
                .discord
                .as_ref()
                .unwrap()
                .user_id
                .as_deref(),
            Some("000000000000000000")
        );
        assert_eq!(cycle.report.default_mode, ReportMode::ByType);
        assert_eq!(cycle.report.by_type["budget"], "budget-review");
        assert_eq!(
            cycle.targets.discord.as_ref().unwrap().template.as_deref(),
            Some("short")
        );
        assert_eq!(
            cycle.targets.notion.as_ref().unwrap().publish,
            PublishMode::Off
        );
    }

    #[test]
    fn github_issues_default_to_true_and_are_read_when_given() {
        let cycle = Cycle::from_yaml_str(FULL).unwrap();
        assert!(cycle.targets.github.as_ref().unwrap().issues);
        let off = FULL.replace(
            "github: { path: meetings, publish: auto }",
            "github: { path: meetings, publish: auto, issues: false }",
        );
        let cycle = Cycle::from_yaml_str(&off).unwrap();
        assert!(!cycle.targets.github.as_ref().unwrap().issues);
        // A value that is not a boolean is refused with its path.
        let bad = FULL.replace(
            "github: { path: meetings, publish: auto }",
            "github: { path: meetings, issues: sometimes }",
        );
        assert!(!issues_of(&bad).is_empty());
    }

    #[test]
    fn the_issues_flag_is_written_only_when_it_is_off() {
        let mut target = GithubTarget::default();
        let on = serde_json::to_value(&target).unwrap();
        assert!(on.get("issues").is_none(), "{on}");
        target.issues = false;
        let off = serde_json::to_value(&target).unwrap();
        assert_eq!(off["issues"], false);
        let back: GithubTarget = serde_json::from_value(on).unwrap();
        assert!(back.issues);
    }

    #[test]
    fn minimal_file_gets_defaults() {
        let cycle = Cycle::from_yaml_str("id: A1\nname: Alpha\n").unwrap();
        assert_eq!(cycle, Cycle::new("A1", "Alpha"));
        assert_eq!(cycle.report.default_template, "default");
        assert!(cycle.targets.is_empty());
    }

    #[test]
    fn rejects_each_kind_of_invalid_file() {
        // Broken YAML.
        assert!(issues_of("id: [")
            .iter()
            .any(|i| i.message.contains("invalid YAML")));
        // Missing required fields.
        let i = issues_of("name: x\n");
        assert!(
            i.iter()
                .any(|i| i.message.contains("\"id\" is a required property")),
            "{i:?}"
        );
        // Bad id.
        assert!(has_path(&issues_of("id: \"../x\"\nname: x\n"), "id"));
        // Unknown field.
        assert!(!issues_of("id: A\nname: x\nsurprise: 1\n").is_empty());
        // Bad enum.
        assert!(has_path(
            &issues_of("id: A\nname: x\nlanguages: { asr: de }\n"),
            "languages.asr"
        ));
        assert!(has_path(
            &issues_of("id: A\nname: x\nreport: { default_mode: magic }\n"),
            "report.default_mode"
        ));
        // Discord webhook must be a credential reference, never a URL.
        let url = "id: A\nname: x\ntargets:\n  discord: { webhook: \"https://discord.com/api/webhooks/1/abc\" }\n";
        assert!(has_path(&issues_of(url), "targets.discord.webhook"));
        // Unquoted discord id is a number in YAML, not a string.
        let num = "id: A\nname: x\nparticipants:\n  - { id: a, name: A, discord: { user_id: 123456789012345678 } }\n";
        assert!(has_path(&issues_of(num), "participants[0].discord.user_id"));
        // Both discord ids at once.
        let both = "id: A\nname: x\nparticipants:\n  - { id: a, name: A, discord: { user_id: \"1\", role_id: \"2\" } }\n";
        assert!(has_path(&issues_of(both), "participants[0].discord"));
        // Template names are file names, not paths.
        assert!(has_path(
            &issues_of("id: A\nname: x\nreport: { default_template: \"../secret\" }\n"),
            "report.default_template"
        ));
    }

    #[test]
    fn rejects_duplicate_participant_ids() {
        let text = "id: A\nname: x\nparticipants:\n  - { id: a, name: One }\n  - { id: b, name: Two }\n  - { id: a, name: Three }\n";
        let issues = issues_of(text);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "participants[2].id");
        assert!(issues[0].message.contains("duplicate"));
    }

    #[test]
    fn template_references_are_checked_against_existing_templates() {
        let cycle = Cycle::from_yaml_str(FULL).unwrap();
        let issues = cycle.template_issues(&["default".to_string()]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "report.by_type.budget");
        assert!(cycle
            .template_issues(&["default".into(), "budget-review".into()])
            .is_empty());
        // Default template missing while the mode needs it.
        let issues = cycle.template_issues(&["budget-review".to_string()]);
        assert!(has_path(&issues, "report.default_template"));
        // Fixed mode does not need the default template.
        let fixed = Cycle::new("A", "x");
        assert!(fixed.template_issues(&[]).is_empty());
    }

    #[test]
    fn save_load_round_trip_is_stable() {
        let cycle = Cycle::from_yaml_str(FULL).unwrap();
        let yaml = cycle.to_yaml().unwrap();
        let back = Cycle::from_yaml_str(&yaml).unwrap();
        assert_eq!(back, cycle);
        // Deterministic: writing again gives the same bytes.
        assert_eq!(back.to_yaml().unwrap(), yaml);
        // Key order follows the declaration, by_type is sorted.
        assert!(
            yaml.starts_with("id: NOMAD\nname: Nomad rover\nlanguages:"),
            "{yaml}"
        );
        assert!(yaml.find("budget:").unwrap() < yaml.find("technical:").unwrap());
        // The discord id stays a string.
        assert!(!yaml.contains("user_id: 000000000000000000\n"), "{yaml}");
    }

    #[test]
    fn writing_an_invalid_cycle_is_refused() {
        let mut cycle = Cycle::new("bad id", "x");
        assert!(matches!(cycle.to_yaml(), Err(CycleError::Invalid { .. })));
        cycle.id = "ok".into();
        cycle.participants = vec![
            Participant {
                id: "a".into(),
                name: "A".into(),
                github: None,
                discord: None,
            },
            Participant {
                id: "a".into(),
                name: "B".into(),
                github: None,
                discord: None,
            },
        ];
        let err = cycle.to_yaml().unwrap_err();
        assert!(err.issues().iter().any(|i| i.message.contains("duplicate")));
    }
}
