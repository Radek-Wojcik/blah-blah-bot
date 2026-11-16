//! `cycles/<ID>/templates/<name>.md`: Markdown with optional YAML front matter that
//! declares the sections, and `{placeholder}` markers that the renderer fills.
//!
//! ```text
//! ---
//! sections:
//!   - id: progress
//!     title: Progress
//!     description: What was done since the last meeting
//! ---
//!
//! # {title}
//! {section:progress}
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cycle::DEFAULT_TEMPLATE_NAME;
use crate::error::{CycleError, Result, ValidationIssue};
use crate::fsutil::write_atomic;
use crate::schema::is_identifier;

/// The default template (ADR 0017), written to `templates/default.md` of a new cycle. The
/// summary heading comes from `{summary_section}`, so it is localised and absent when the
/// summary is empty. Cycles created earlier keep the file they have.
pub const DEFAULT_TEMPLATE_TEXT: &str = "# {title}\n\n{date} · {participants}\n\n{summary_section}\n\n{sections}\n\n---\n[Full transcript](transcript.md)\n";

/// The built-in short template of a chat target (`targets.discord.template` absent): title and
/// date, the summary, the decisions and the tasks, and a link to the full report.
pub const SHORT_TEMPLATE_TEXT: &str = "**{title}** · {date}\n\n{summary}\n\n## Decisions\n{decisions}\n\n## Tasks\n{tasks}\n\n[Full report]({report_link})\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSection {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    #[serde(default)]
    sections: Vec<TemplateSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// File name without `.md`.
    pub name: String,
    pub sections: Vec<TemplateSection>,
    /// Everything after the front matter.
    pub body: String,
}

impl Template {
    /// The built-in default template.
    pub fn builtin_default() -> Template {
        Template {
            name: DEFAULT_TEMPLATE_NAME.to_string(),
            sections: Vec::new(),
            body: DEFAULT_TEMPLATE_TEXT.to_string(),
        }
    }

    /// The file text: front matter (only when there are sections), a blank line, the body.
    pub fn to_text(&self) -> Result<String> {
        if self.sections.is_empty() {
            return Ok(self.body.clone());
        }
        let front = serde_norway::to_string(&FrontMatter {
            sections: self.sections.clone(),
        })
        .map_err(|e| CycleError::Other(format!("cannot write front matter: {e}")))?;
        Ok(format!("---\n{front}---\n\n{}", self.body))
    }
}

/// Parses a template file. Fails when the front matter is not valid YAML of the expected
/// shape or a section id is invalid or repeated; placeholders in the body are checked by
/// [`validate_template`].
pub fn parse_template(
    name: &str,
    text: &str,
) -> std::result::Result<Template, Vec<ValidationIssue>> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let (sections, body) = match split_front_matter(&text) {
        Err(issue) => return Err(vec![issue]),
        Ok(None) => (Vec::new(), text),
        Ok(Some((front, rest))) => {
            let front_value = crate::schema::yaml_to_json(front)
                .map_err(|i| vec![ValidationIssue::new("front matter", i.message)])?;
            let parsed: FrontMatter = if front_value.is_null() {
                FrontMatter::default()
            } else {
                serde_json::from_value(front_value)
                    .map_err(|e| vec![ValidationIssue::new("front matter", e.to_string())])?
            };
            (parsed.sections, rest.trim_start_matches('\n').to_string())
        }
    };
    let template = Template {
        name: name.to_string(),
        sections,
        body,
    };
    let issues = section_issues(&template.sections);
    if issues.is_empty() {
        Ok(template)
    } else {
        Err(issues)
    }
}

/// Splits `---\n<yaml>\n---\n<rest>`; `None` when the text has no front matter.
fn split_front_matter(text: &str) -> std::result::Result<Option<(&str, &str)>, ValidationIssue> {
    let Some(after_open) = text.strip_prefix("---\n") else {
        return Ok(None);
    };
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "---" {
            return Ok(Some((
                &after_open[..offset],
                &after_open[offset + line.len()..],
            )));
        }
        offset += line.len();
    }
    Err(ValidationIssue::new(
        "front matter",
        "the front matter starts with --- but has no closing --- line",
    ))
}

fn is_section_id(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn section_issues(sections: &[TemplateSection]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, s) in sections.iter().enumerate() {
        let path = format!("sections[{i}]");
        if !is_section_id(&s.id) {
            issues.push(ValidationIssue::new(
                format!("{path}.id"),
                format!(
                    "section id \"{}\" must start with a lowercase letter and contain only lowercase letters, digits, '_' and '-'",
                    s.id
                ),
            ));
        }
        if s.title.trim().is_empty() {
            issues.push(ValidationIssue::new(
                format!("{path}.title"),
                "the title must not be empty",
            ));
        }
        if let Some(first) = seen.insert(s.id.as_str(), i) {
            issues.push(ValidationIssue::new(
                format!("{path}.id"),
                format!(
                    "duplicate section id \"{}\" (first used at sections[{first}])",
                    s.id
                ),
            ));
        }
    }
    issues
}

/// Everything that can be wrong with a template: front matter sections and placeholders in
/// the body. When the template declares sections, `{section:<id>}` must name one of them.
pub fn validate_template(template: &Template) -> Vec<ValidationIssue> {
    let mut issues = section_issues(&template.sections);
    if !is_identifier(&template.name) {
        issues.push(ValidationIssue::new(
            "name",
            format!(
                "template name \"{}\" is not a valid file name",
                template.name
            ),
        ));
    }
    match tokenize(&template.body) {
        Err(mut body_issues) => issues.append(&mut body_issues),
        Ok(tokens) if !template.sections.is_empty() => {
            for token in tokens {
                if let Token::Placeholder(Placeholder::Section(id)) = token {
                    if !template.sections.iter().any(|s| s.id == id) {
                        issues.push(ValidationIssue::new(
                            "body",
                            format!("{{section:{id}}} refers to a section the front matter does not declare"),
                        ));
                    }
                }
            }
        }
        Ok(_) => {}
    }
    issues
}

/// Checks only the placeholders of a template body.
pub fn validate_body(body: &str) -> Vec<ValidationIssue> {
    match tokenize(body) {
        Ok(_) => Vec::new(),
        Err(issues) => issues,
    }
}

/// The placeholders a template body may use.
pub const PLACEHOLDERS: &[&str] = &[
    "title",
    "date",
    "participants",
    "summary",
    "summary_section",
    "sections",
    "section:<id>",
    "tasks",
    "decisions",
    "topics",
    "notes",
    "ideas",
    "budgets",
    "transcript_link",
    "report_link",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placeholder {
    Title,
    Date,
    Participants,
    Summary,
    SummarySection,
    Sections,
    Section(String),
    Tasks,
    Decisions,
    Topics,
    Notes,
    Ideas,
    Budgets,
    TranscriptLink,
    ReportLink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    Text(String),
    Placeholder(Placeholder),
}

fn parse_placeholder(name: &str) -> Option<Placeholder> {
    Some(match name {
        "title" => Placeholder::Title,
        "date" => Placeholder::Date,
        "participants" => Placeholder::Participants,
        "summary" => Placeholder::Summary,
        "summary_section" => Placeholder::SummarySection,
        "sections" => Placeholder::Sections,
        "tasks" => Placeholder::Tasks,
        "decisions" => Placeholder::Decisions,
        "topics" => Placeholder::Topics,
        "notes" => Placeholder::Notes,
        "ideas" => Placeholder::Ideas,
        "budgets" => Placeholder::Budgets,
        "transcript_link" => Placeholder::TranscriptLink,
        "report_link" => Placeholder::ReportLink,
        other => {
            let id = other.strip_prefix("section:")?;
            if !is_section_id(id) {
                return None;
            }
            Placeholder::Section(id.to_string())
        }
    })
}

/// Splits a template body into text and placeholders. `{{` and `}}` are literal braces.
/// Every unknown or malformed placeholder is reported with its line number; nothing is
/// ever dropped silently.
pub(crate) fn tokenize(body: &str) -> std::result::Result<Vec<Token>, Vec<ValidationIssue>> {
    let chars: Vec<char> = body.chars().collect();
    let mut tokens = Vec::new();
    let mut issues = Vec::new();
    let mut text = String::new();
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                text.push(c);
                i += 1;
            }
            '{' if chars.get(i + 1) == Some(&'{') => {
                text.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                text.push('}');
                i += 2;
            }
            '{' => {
                let end = chars[i + 1..]
                    .iter()
                    .position(|&ch| ch == '}' || ch == '{' || ch == '\n');
                match end {
                    Some(off) if chars[i + 1 + off] == '}' => {
                        let name: String = chars[i + 1..i + 1 + off].iter().collect();
                        match parse_placeholder(&name) {
                            Some(p) => {
                                if !text.is_empty() {
                                    tokens.push(Token::Text(std::mem::take(&mut text)));
                                }
                                tokens.push(Token::Placeholder(p));
                            }
                            None => issues.push(ValidationIssue::new(
                                "body",
                                format!("line {line}: unknown placeholder {{{name}}} (use {{{{ and }}}} for literal braces)"),
                            )),
                        }
                        i += off + 2;
                    }
                    _ => {
                        issues.push(ValidationIssue::new(
                            "body",
                            format!("line {line}: '{{' is not closed on the same line (use {{{{ for a literal brace)"),
                        ));
                        i += 1;
                    }
                }
            }
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    if issues.is_empty() {
        Ok(tokens)
    } else {
        Err(issues)
    }
}

fn template_path(dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    if !is_identifier(name) {
        return Err(CycleError::InvalidName(name.to_string()));
    }
    Ok(dir.join(format!("{name}.md")))
}

/// Names of the templates in a `templates/` directory, sorted. A missing directory is an
/// empty list.
pub fn list_templates(dir: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CycleError::io(dir, e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CycleError::io(dir, e))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.is_file() {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if is_identifier(stem) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Reads and parses one template (placeholders are not checked here).
pub fn read_template(dir: &Path, name: &str) -> Result<Template> {
    let path = template_path(dir, name)?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CycleError::TemplateNotFound(name.to_string()))
        }
        Err(e) => return Err(CycleError::io(&path, e)),
    };
    parse_template(name, &text)
        .map_err(|issues| CycleError::invalid(format!("template {name}"), issues))
}

/// Validates a template (sections and placeholders) and writes it to `dir`.
pub fn write_template(dir: &Path, template: &Template) -> Result<()> {
    let path = template_path(dir, &template.name)?;
    let issues = validate_template(template);
    if !issues.is_empty() {
        return Err(CycleError::invalid(
            format!("template {}", template.name),
            issues,
        ));
    }
    write_atomic(&path, template.to_text()?.as_bytes())
}

/// Creates `templates/default.md` with the built-in text in `dir` if it does not exist.
/// Returns whether the file was created.
pub fn create_default_template(dir: &Path) -> Result<bool> {
    let path = template_path(dir, DEFAULT_TEMPLATE_NAME)?;
    if path.exists() {
        return Ok(false);
    }
    write_atomic(&path, DEFAULT_TEMPLATE_TEXT.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_SECTIONS: &str = "---\nsections:\n  - id: progress\n    title: Progress\n    description: What was done\n  - id: blockers\n    title: Blockers\n---\n\n# {title}\n\n{section:progress}\n\n{section:blockers}\n";

    #[test]
    fn default_template_matches_the_decision_and_validates() {
        let t = Template::builtin_default();
        assert!(validate_template(&t).is_empty());
        assert!(t.body.starts_with(
            "# {title}\n\n{date} · {participants}\n\n{summary_section}\n\n{sections}"
        ));
        assert!(t.body.ends_with("---\n[Full transcript](transcript.md)\n"));
    }

    #[test]
    fn summary_placeholders_validate_and_an_existing_default_file_is_kept() {
        let t = parse_template("x", "{summary}\n\n{summary_section}\n").unwrap();
        assert!(validate_template(&t).is_empty());
        // A cycle created before `{summary_section}` keeps its own default.md.
        let dir = tempfile::tempdir().unwrap();
        let old = "# {title}\n\n## Summary\n{summary}\n\n{sections}\n";
        fs::write(dir.path().join("default.md"), old).unwrap();
        assert!(!create_default_template(dir.path()).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join("default.md")).unwrap(),
            old
        );
    }

    #[test]
    fn parses_front_matter_and_body() {
        let t = parse_template("review", WITH_SECTIONS).unwrap();
        assert_eq!(t.name, "review");
        assert_eq!(t.sections.len(), 2);
        assert_eq!(t.sections[0].id, "progress");
        assert_eq!(t.sections[0].description, "What was done");
        assert_eq!(t.sections[1].description, "");
        assert!(t.body.starts_with("# {title}"));
        assert!(validate_template(&t).is_empty());
    }

    #[test]
    fn text_without_front_matter_is_all_body() {
        let t = parse_template("plain", "# {title}\n\n{tasks}\n").unwrap();
        assert!(t.sections.is_empty());
        assert_eq!(t.body, "# {title}\n\n{tasks}\n");
    }

    #[test]
    fn text_round_trips() {
        let t = parse_template("review", WITH_SECTIONS).unwrap();
        let again = parse_template("review", &t.to_text().unwrap()).unwrap();
        assert_eq!(again, t);
        let plain = Template::builtin_default();
        assert_eq!(
            parse_template("default", &plain.to_text().unwrap()).unwrap(),
            plain
        );
    }

    #[test]
    fn crlf_files_parse() {
        let t = parse_template("x", &WITH_SECTIONS.replace('\n', "\r\n")).unwrap();
        assert_eq!(t.sections.len(), 2);
        assert!(!t.body.contains('\r'));
    }

    #[test]
    fn invalid_front_matter_is_reported() {
        assert!(parse_template("x", "---\nsections: [\n---\nbody").is_err());
        assert!(parse_template("x", "---\nsections: []\nbody never closed").is_err());
        let unknown = parse_template("x", "---\nlayout: wide\n---\nbody").unwrap_err();
        assert_eq!(unknown[0].path, "front matter");
        let bad_id = parse_template(
            "x",
            "---\nsections:\n  - { id: Bad Id, title: T }\n---\nbody",
        )
        .unwrap_err();
        assert_eq!(bad_id[0].path, "sections[0].id");
        let dup = parse_template(
            "x",
            "---\nsections:\n  - { id: a, title: A }\n  - { id: a, title: B }\n---\n",
        )
        .unwrap_err();
        assert!(dup[0].message.contains("duplicate"));
        let no_title =
            parse_template("x", "---\nsections:\n  - { id: a, title: \"\" }\n---\n").unwrap_err();
        assert_eq!(no_title[0].path, "sections[0].title");
    }

    #[test]
    fn unknown_placeholders_are_errors() {
        let t = parse_template("x", "# {title}\n\n{task}\n{summary}\n{nope}\n").unwrap();
        let issues = validate_template(&t);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].message.contains("line 3") && issues[0].message.contains("{task}"));
        assert!(issues[1].message.contains("line 5"));
        // Spaces inside the braces are not accepted either.
        assert!(!validate_body("{ title }").is_empty());
        // Unclosed brace.
        assert!(!validate_body("a { b\nc }").is_empty());
        // Literal braces are escaped by doubling.
        assert!(validate_body("{{not a placeholder}} {title}").is_empty());
    }

    #[test]
    fn section_references_must_match_declared_sections() {
        let mut t = parse_template("x", WITH_SECTIONS).unwrap();
        t.body.push_str("{section:missing}\n");
        let issues = validate_template(&t);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("missing"));
        // A template without sections may refer to any section id (auto mode).
        let free = parse_template("x", "{section:whatever}\n").unwrap();
        assert!(validate_template(&free).is_empty());
    }

    #[test]
    fn list_read_write_in_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tdir = dir.path().join("templates");
        assert!(list_templates(&tdir).unwrap().is_empty());
        assert!(create_default_template(&tdir).unwrap());
        assert!(!create_default_template(&tdir).unwrap());
        let t = parse_template("review", WITH_SECTIONS).unwrap();
        write_template(&tdir, &t).unwrap();
        fs::write(tdir.join("notes.txt"), "x").unwrap();
        fs::write(tdir.join("bad name.md"), "x").unwrap();
        assert_eq!(list_templates(&tdir).unwrap(), vec!["default", "review"]);
        assert_eq!(read_template(&tdir, "review").unwrap(), t);
        assert!(matches!(
            read_template(&tdir, "none"),
            Err(CycleError::TemplateNotFound(_))
        ));
        assert!(matches!(
            read_template(&tdir, "../x"),
            Err(CycleError::InvalidName(_))
        ));
        // Invalid templates are not written.
        let bad = Template {
            name: "broken".into(),
            sections: vec![],
            body: "{nope}".into(),
        };
        assert!(write_template(&tdir, &bad).is_err());
        assert!(!tdir.join("broken.md").exists());
        // The default template file has the constant's text.
        assert_eq!(
            fs::read_to_string(tdir.join("default.md")).unwrap(),
            DEFAULT_TEMPLATE_TEXT
        );
    }
}
