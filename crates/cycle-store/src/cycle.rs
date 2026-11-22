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

/// What `targets.github.publish` is when the cycle does not set it: the report, the transcript
/// and the issues wait for the user to press Publish. The one place that decides it; the
/// parser and the publishing rules both read it.
pub const DEFAULT_GITHUB_PUBLISH: PublishMode = PublishMode::OnApproval;
/// What `targets.discord.publish` is when the cycle does not set it.
pub const DEFAULT_DISCORD_PUBLISH: PublishMode = PublishMode::OnApproval;
/// What `targets.notion.publish` is when the cycle does not set it.
pub const DEFAULT_NOTION_PUBLISH: PublishMode = PublishMode::OnApproval;

fn default_github_publish() -> PublishMode {
    DEFAULT_GITHUB_PUBLISH
}

fn default_discord_publish() -> PublishMode {
    DEFAULT_DISCORD_PUBLISH
}

fn default_notion_publish() -> PublishMode {
    DEFAULT_NOTION_PUBLISH
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
    #[serde(default = "default_github_publish")]
    pub publish: PublishMode,
    /// Create GitHub issues for task items and write back status changes. Left out of the
    /// file while true, so existing files keep their bytes.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub issues: bool,
}

/// Where meeting folders go when `targets.github.path` is not set.
pub const DEFAULT_MEETINGS_PATH: &str = "meetings";

fn default_github_path() -> String {
    DEFAULT_MEETINGS_PATH.to_string()
}

/// Why `path` cannot be the folder of the meetings in the repository (`targets.github.path`),
/// or `None` when it can. It must stay inside the repository on every system the clone is
/// used on: relative, with `/` between names, no `.` or `..` name, no drive letter or
/// backslash, no `.git` folder and no name that ends in a dot (Windows drops it).
pub fn repo_path_problem(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("the folder is empty".into());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Some("the folder must be relative to the repository, not absolute".into());
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some("the folder must not start with a drive letter".into());
    }
    if path.contains('\\') {
        return Some("use / between folder names, not \\".into());
    }
    for name in path.split('/') {
        if name.is_empty() {
            return Some("the folder has an empty name (a doubled or trailing /)".into());
        }
        if name == "." || name == ".." {
            return Some("the folder must not contain . or .. names".into());
        }
        if name.eq_ignore_ascii_case(".git") {
            return Some("the folder must not be inside .git".into());
        }
        if name.ends_with('.') {
            return Some("a folder name must not end with a dot".into());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Some(
                "folder names may only have letters, digits, dots, hyphens and underscores".into(),
            );
        }
    }
    None
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
            publish: DEFAULT_GITHUB_PUBLISH,
            issues: true,
        }
    }
}

/// `targets.discord`: exactly one of `webhook` and `bot` (the schema checks it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordTarget {
    /// A `cred:` reference to the webhook URL in the Credential Manager, never the URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    /// Post through a bot instead of a webhook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<DiscordBot>,
    #[serde(default = "default_discord_publish")]
    pub publish: PublishMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// `targets.discord.bot`: where a bot posts and which secret it uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordBot {
    /// A `cred:` reference to the bot token, never the token.
    pub token: String,
    /// The server (guild) id, a snowflake.
    pub guild: String,
    /// The text or announcement channel id, a snowflake.
    pub channel: String,
    /// One thread per meeting under the summary. Left out of the file while true.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub thread: bool,
    /// Edit the summary when a correction is published. Left out of the file while true.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub edit_on_correction: bool,
}

/// How a Discord target posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscordMode<'a> {
    /// The `cred:` reference of the webhook.
    Webhook(&'a str),
    Bot(&'a DiscordBot),
}

impl DiscordTarget {
    /// The way this target posts: the bot when both are set, `None` when neither is
    /// (the schema refuses such a file, a value built by hand can still have it).
    pub fn mode(&self) -> Option<DiscordMode<'_>> {
        match (&self.bot, &self.webhook) {
            (Some(bot), _) => Some(DiscordMode::Bot(bot)),
            (None, Some(webhook)) => Some(DiscordMode::Webhook(webhook)),
            (None, None) => None,
        }
    }
}

/// `targets.notion`: a Notion database that task items are exported to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotionTarget {
    /// A `cred:` reference to the integration token, never the token.
    pub token: String,
    /// The database id (32 hex digits, with or without dashes). Not a secret.
    pub database: String,
    /// The data source id; needed when the database has more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(default = "default_notion_publish")]
    pub publish: PublishMode,
    /// Names of the Notion properties the fields go to; left out while all are default.
    #[serde(default, skip_serializing_if = "NotionProperties::is_default")]
    pub properties: NotionProperties,
}

/// The title property name when `targets.notion.properties.title` is not set.
pub const DEFAULT_NOTION_TITLE: &str = "Name";
/// The key property name when `targets.notion.properties.key` is not set.
pub const DEFAULT_NOTION_KEY: &str = "BBB key";

fn default_notion_title() -> String {
    DEFAULT_NOTION_TITLE.to_string()
}

fn default_notion_key() -> String {
    DEFAULT_NOTION_KEY.to_string()
}

/// The Notion property names of `targets.notion.properties`. `title` and `key` always have a
/// name; the other fields are exported only when a property is named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotionProperties {
    /// The title property (type `title`).
    #[serde(default = "default_notion_title")]
    pub title: String,
    /// The property that holds the idempotency key (type `rich_text`).
    #[serde(default = "default_notion_key")]
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The status a new page gets; required when `status` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
}

impl Default for NotionProperties {
    fn default() -> Self {
        Self {
            title: default_notion_title(),
            key: default_notion_key(),
            assignee: None,
            due: None,
            status: None,
            status_value: None,
            meeting: None,
            cycle: None,
        }
    }
}

impl NotionProperties {
    /// Nothing is changed from the defaults (the block is then left out of the file).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The canonical form of a Notion id: 32 lowercase hex digits, dashes removed. `None` when
/// `s` is not a Notion id: 32 hex digits, with an optional dash after the 8th, 12th, 16th and
/// 20th digit (the same shapes the schema accepts; surrounding whitespace is ignored).
pub fn normalize_notion_id(s: &str) -> Option<String> {
    let mut out = String::with_capacity(32);
    for c in s.trim().chars() {
        if c == '-' {
            if !matches!(out.len(), 8 | 12 | 16 | 20) {
                return None;
            }
        } else if c.is_ascii_hexdigit() {
            out.push(c.to_ascii_lowercase());
        } else {
            return None;
        }
    }
    (out.len() == 32).then_some(out)
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
        if let Some(github) = &self.targets.github {
            if let Some(problem) = repo_path_problem(&github.path) {
                issues.push(ValidationIssue::new("targets.github.path", problem));
            }
        }
        if let Some(notion) = &self.targets.notion {
            if notion.properties.status.is_some() && notion.properties.status_value.is_none() {
                issues.push(ValidationIssue::new(
                    "targets.notion.properties.status_value",
                    "the status a new page gets is required when a status property is set",
                ));
            }
        }
        issues
    }

    /// The folder of the meetings in the repository: `targets.github.path`, or `meetings`
    /// when it is not set. A value that is not valid gives the default; validation reports it.
    pub fn meetings_path(&self) -> &str {
        match &self.targets.github {
            Some(g) if repo_path_problem(&g.path).is_none() => &g.path,
            _ => DEFAULT_MEETINGS_PATH,
        }
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
  notion: { token: "cred:notion/nomad", database: "0123456789abcdef0123456789abcdef", publish: off }
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
    fn the_github_path_must_stay_inside_the_repository() {
        for bad in [
            "..", "../x", "a/../b", "./a", "/abs", "C:/x", "a//b", ".git/x",
        ] {
            let text = FULL.replace(
                "github: { path: meetings, publish: auto }",
                &format!("github: {{ path: \"{bad}\", publish: auto }}"),
            );
            let issues = Cycle::parse(&text).1;
            assert!(
                has_path(&issues, "targets.github.path"),
                "{bad}: {issues:?}"
            );
        }
        let good = FULL.replace(
            "github: { path: meetings, publish: auto }",
            "github: { path: docs/meetings, publish: auto }",
        );
        let cycle = Cycle::from_yaml_str(&good).unwrap();
        assert_eq!(cycle.meetings_path(), "docs/meetings");
        assert_eq!(Cycle::new("X", "x").meetings_path(), DEFAULT_MEETINGS_PATH);
        // A value that was built by hand and is not safe falls back to the default folder;
        // `validate` reports it and the publisher refuses it.
        let mut hand = cycle;
        hand.targets.github.as_mut().unwrap().path = "../x".into();
        assert_eq!(hand.meetings_path(), DEFAULT_MEETINGS_PATH);
        assert!(has_path(&hand.validate(&[]), "targets.github.path"));
        assert!(hand.to_yaml().is_err(), "an unsafe path is never written");
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

    fn with_targets(block: &str) -> String {
        format!("id: A\nname: x\ntargets:\n{block}")
    }

    const BOT: &str = "    bot: { token: \"cred:discord-bot/team\", guild: \"111111111111111111\", channel: \"222222222222222222\" }\n";

    #[test]
    fn discord_takes_exactly_one_of_webhook_and_bot() {
        let webhook = with_targets("  discord: { webhook: \"cred:discord/nomad\" }\n");
        let c = Cycle::from_yaml_str(&webhook).unwrap();
        let d = c.targets.discord.unwrap();
        assert!(d.bot.is_none());
        assert_eq!(d.mode(), Some(DiscordMode::Webhook("cred:discord/nomad")));

        let bot = with_targets(&format!("  discord:\n{BOT}"));
        let c = Cycle::from_yaml_str(&bot).unwrap();
        let d = c.targets.discord.unwrap();
        assert!(d.webhook.is_none());
        assert!(matches!(d.mode(), Some(DiscordMode::Bot(b)) if b.channel == "222222222222222222"));

        let both = with_targets(&format!(
            "  discord:\n    webhook: \"cred:discord/nomad\"\n{BOT}"
        ));
        let issues = issues_of(&both);
        assert!(has_path(&issues, "targets.discord"), "{issues:?}");

        let neither = with_targets("  discord: { publish: auto }\n");
        let issues = issues_of(&neither);
        assert!(has_path(&issues, "targets.discord"), "{issues:?}");
    }

    #[test]
    fn the_bot_block_is_checked_field_by_field() {
        // The token is a reference, never a token or a URL.
        let url = with_targets(
            "  discord:\n    bot: { token: \"https://discord.com/api/webhooks/1/abc\", guild: \"1\", channel: \"2\" }\n",
        );
        assert!(has_path(&issues_of(&url), "targets.discord.bot.token"));
        let raw = with_targets(
            "  discord:\n    bot: { token: \"MTIz.abc.def\", guild: \"1\", channel: \"2\" }\n",
        );
        assert!(has_path(&issues_of(&raw), "targets.discord.bot.token"));
        // The ids are quoted digits.
        let guild = with_targets(
            "  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"my-server\", channel: \"2\" }\n",
        );
        assert!(has_path(&issues_of(&guild), "targets.discord.bot.guild"));
        let channel = with_targets(
            "  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"1\", channel: 222 }\n",
        );
        assert!(has_path(
            &issues_of(&channel),
            "targets.discord.bot.channel"
        ));
        // A required field missing, an unknown field, a flag that is not a boolean.
        let missing =
            with_targets("  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"1\" }\n");
        assert!(!issues_of(&missing).is_empty());
        let unknown = with_targets(
            "  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"1\", channel: \"2\", intents: 3 }\n",
        );
        assert!(!issues_of(&unknown).is_empty());
        let flag = with_targets(
            "  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"1\", channel: \"2\", thread: maybe }\n",
        );
        assert!(has_path(&issues_of(&flag), "targets.discord.bot.thread"));
    }

    #[test]
    fn the_bot_flags_default_to_true_and_are_written_only_when_off() {
        let c = Cycle::from_yaml_str(&with_targets(&format!("  discord:\n{BOT}"))).unwrap();
        let bot = c.targets.discord.as_ref().unwrap().bot.clone().unwrap();
        assert!(bot.thread && bot.edit_on_correction);
        let yaml = c.to_yaml().unwrap();
        assert!(!yaml.contains("thread"), "{yaml}");
        assert!(!yaml.contains("edit_on_correction"), "{yaml}");
        assert!(!yaml.contains("webhook"), "{yaml}");

        let off = with_targets(
            "  discord:\n    bot: { token: \"cred:discord-bot/t\", guild: \"1\", channel: \"2\", thread: false, edit_on_correction: false }\n",
        );
        let c = Cycle::from_yaml_str(&off).unwrap();
        let bot = c.targets.discord.as_ref().unwrap().bot.as_ref().unwrap();
        assert!(!bot.thread && !bot.edit_on_correction);
        let yaml = c.to_yaml().unwrap();
        assert!(yaml.contains("thread: false"), "{yaml}");
        assert!(yaml.contains("edit_on_correction: false"), "{yaml}");
        assert_eq!(Cycle::from_yaml_str(&yaml).unwrap(), c);
    }

    #[test]
    fn mode_prefers_the_bot_and_is_none_without_either() {
        let bot = DiscordBot {
            token: "cred:discord-bot/t".into(),
            guild: "1".into(),
            channel: "2".into(),
            thread: true,
            edit_on_correction: true,
        };
        let mut target = DiscordTarget {
            webhook: Some("cred:discord/nomad".into()),
            bot: Some(bot.clone()),
            publish: PublishMode::Off,
            template: None,
        };
        assert_eq!(target.mode(), Some(DiscordMode::Bot(&bot)));
        target.bot = None;
        assert_eq!(
            target.mode(),
            Some(DiscordMode::Webhook("cred:discord/nomad"))
        );
        target.webhook = None;
        assert_eq!(target.mode(), None);
        // A value built by hand without either is refused when written.
        let mut cycle = Cycle::new("A", "x");
        cycle.targets.discord = Some(target);
        assert!(cycle.to_yaml().is_err());
    }

    #[test]
    fn notion_ids_are_normalised_and_bad_ones_refused() {
        assert_eq!(
            normalize_notion_id("0123456789ABCDEF0123456789abcdef").as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            normalize_notion_id(" fedcba98-7654-3210-FEDC-ba9876543210 ").as_deref(),
            Some("fedcba9876543210fedcba9876543210")
        );
        for bad in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdeg",
            "01234-56789abcdef0123456789abcdef",
            "https://www.notion.so/0123456789abcdef0123456789abcdef",
            "cred:notion/nomad-db",
        ] {
            assert_eq!(normalize_notion_id(bad), None, "{bad}");
        }
        for (field, bad) in [
            ("database", "cred:notion/nomad-db"),
            ("database", "0123456789abcdef0123456789abcde"),
            ("data_source", "not-an-id"),
        ] {
            let mut pairs = vec![
                ("token", "\"cred:notion/nomad\"".to_string()),
                (
                    "database",
                    "\"0123456789abcdef0123456789abcdef\"".to_string(),
                ),
            ];
            match pairs.iter_mut().find(|(k, _)| *k == field) {
                Some(pair) => pair.1 = format!("\"{bad}\""),
                None => pairs.push((field, format!("\"{bad}\""))),
            }
            let body: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let text = with_targets(&format!("  notion: {{ {} }}\n", body.join(", ")));
            assert!(
                has_path(&issues_of(&text), &format!("targets.notion.{field}")),
                "{field} = {bad}"
            );
        }
    }

    #[test]
    fn the_notion_token_is_a_reference_and_both_fields_are_required() {
        let raw = with_targets(
            "  notion: { token: \"ntn_0123456789abcdefghijklmnop\", database: \"0123456789abcdef0123456789abcdef\" }\n",
        );
        assert!(has_path(&issues_of(&raw), "targets.notion.token"));
        let no_token =
            with_targets("  notion: { database: \"0123456789abcdef0123456789abcdef\" }\n");
        assert!(!issues_of(&no_token).is_empty());
        let no_database = with_targets("  notion: { token: \"cred:notion/nomad\" }\n");
        assert!(!issues_of(&no_database).is_empty());
        // The old shape, a database reference without a token, is not valid any more.
        let old = with_targets("  notion: { database: \"cred:notion/nomad-db\", publish: off }\n");
        assert!(!issues_of(&old).is_empty());
    }

    #[test]
    fn notion_defaults_are_not_written_and_a_full_mapping_round_trips() {
        let minimal = with_targets(
            "  notion: { token: \"cred:notion/nomad\", database: \"0123456789abcdef0123456789abcdef\" }\n",
        );
        let c = Cycle::from_yaml_str(&minimal).unwrap();
        let n = c.targets.notion.as_ref().unwrap();
        assert_eq!(n.publish, DEFAULT_NOTION_PUBLISH);
        assert_eq!(n.data_source, None);
        assert_eq!(n.properties, NotionProperties::default());
        assert_eq!(n.properties.title, DEFAULT_NOTION_TITLE);
        assert_eq!(n.properties.key, DEFAULT_NOTION_KEY);
        let yaml = c.to_yaml().unwrap();
        assert!(!yaml.contains("properties"), "{yaml}");
        assert!(!yaml.contains("data_source"), "{yaml}");

        let full = with_targets(
            "  notion:
    token: \"cred:notion/nomad\"
    database: \"0123456789abcdef0123456789abcdef\"
    data_source: \"fedcba98-7654-3210-fedc-ba9876543210\"
    publish: auto
    properties:
      title: Task
      key: BBB key
      assignee: Assignee
      due: Due
      status: Status
      status_value: Not started
      meeting: Meeting
      cycle: Cycle
",
        );
        let c = Cycle::from_yaml_str(&full).unwrap();
        let n = c.targets.notion.as_ref().unwrap();
        assert_eq!(n.properties.title, "Task");
        assert_eq!(n.properties.status_value.as_deref(), Some("Not started"));
        assert_eq!(
            n.data_source.as_deref(),
            Some("fedcba98-7654-3210-fedc-ba9876543210")
        );
        let yaml = c.to_yaml().unwrap();
        assert_eq!(Cycle::from_yaml_str(&yaml).unwrap(), c);
        assert_eq!(
            Cycle::from_yaml_str(&yaml).unwrap().to_yaml().unwrap(),
            yaml
        );
        // A property block that only repeats the defaults is dropped again.
        let defaults = with_targets(
            "  notion: { token: \"cred:notion/nomad\", database: \"0123456789abcdef0123456789abcdef\", properties: { title: Name } }\n",
        );
        let yaml = Cycle::from_yaml_str(&defaults).unwrap().to_yaml().unwrap();
        assert!(!yaml.contains("properties"), "{yaml}");
    }

    #[test]
    fn a_status_property_needs_the_value_a_new_page_gets() {
        let text = with_targets(
            "  notion: { token: \"cred:notion/nomad\", database: \"0123456789abcdef0123456789abcdef\", properties: { status: Status } }\n",
        );
        let (cycle, issues) = Cycle::parse(&text);
        assert!(
            cycle.is_some(),
            "the shape is right, the cycle is still shown"
        );
        assert!(
            has_path(&issues, "targets.notion.properties.status_value"),
            "{issues:?}"
        );
        assert!(cycle.unwrap().to_yaml().is_err());
        let ok = text.replace("status: Status", "status: Status, status_value: Open");
        assert!(Cycle::from_yaml_str(&ok).is_ok());
    }

    #[test]
    fn a_file_without_the_new_keys_is_written_as_before() {
        let text = "id: NOMAD
name: Nomad rover
targets:
  github:
    path: meetings
    publish: auto
  discord:
    webhook: cred:discord/nomad
    publish: on-approval
    template: short
";
        let cycle = Cycle::from_yaml_str(text).unwrap();
        let yaml = cycle.to_yaml().unwrap();
        for key in ["bot", "thread", "edit_on_correction", "notion", "token"] {
            assert!(!yaml.contains(key), "{key}: {yaml}");
        }
        assert!(
            yaml.contains("discord:\n    webhook: cred:discord/nomad\n    publish: on-approval\n    template: short\n"),
            "{yaml}"
        );
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
