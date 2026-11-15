//! Deterministic report layout: a draft, the cycle's participants and a template body
//! become Markdown. No model is involved; the same input always gives the same text.
//!
//! # Item lines
//!
//! * Every item is a bullet `- text`. Tasks are `- [ ] text (@Assignee, due 2026-11-20)`
//!   (the [`Variant::Chat`] variant drops the `[ ]` box, which Discord and Notion do not
//!   render).
//! * A trailing `?` marks a field the model inferred (`@Alex?`, `due 2026-11-20?`); a
//!   missing or unknown assignee or due date is `@?` and `due ?`.
//! * `(unverified quote)` is appended when the item's quote was not found in the
//!   transcript (`source.verified` is false). Items added by hand have no source and no
//!   marker.
//! * An assignee id is shown as the participant's name; an id that is not a participant
//!   is shown as it is.

use crate::cycle::Participant;
use crate::draft::{CertaintyLevel, Draft, Item, ItemType, Language, ReportMode};
use crate::error::ValidationIssue;
use crate::template::{tokenize, Placeholder, Token};

/// Marker appended to items whose quote could not be verified.
pub const UNVERIFIED_MARKER: &str = "(unverified quote)";

/// Per-target variant of the item lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variant {
    /// GitHub Markdown: tasks are `- [ ]` checkboxes.
    #[default]
    Markdown,
    /// Discord and Notion: tasks are plain bullets.
    Chat,
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Meeting title for `{title}` (the draft has none).
    pub title: String,
    pub variant: Variant,
    /// Target of `{transcript_link}`.
    pub transcript_link: String,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            title: String::new(),
            variant: Variant::Markdown,
            transcript_link: "transcript.md".to_string(),
        }
    }
}

/// The built-in sections of fixed mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSection {
    Summary,
    Decisions,
    Tasks,
    OpenTopics,
    Notes,
    Ideas,
    Budgets,
}

impl BuiltinSection {
    /// The order of the built-in sections in a report.
    pub const ORDER: [BuiltinSection; 7] = [
        BuiltinSection::Summary,
        BuiltinSection::Decisions,
        BuiltinSection::Tasks,
        BuiltinSection::OpenTopics,
        BuiltinSection::Notes,
        BuiltinSection::Ideas,
        BuiltinSection::Budgets,
    ];

    /// The localised heading.
    pub fn heading(self, language: Language) -> &'static str {
        let (en, pl) = match self {
            BuiltinSection::Summary => ("Summary", "Podsumowanie"),
            BuiltinSection::Decisions => ("Decisions", "Decyzje"),
            BuiltinSection::Tasks => ("Tasks", "Zadania"),
            BuiltinSection::OpenTopics => ("Open topics", "Otwarte tematy"),
            BuiltinSection::Notes => ("Notes", "Notatki"),
            BuiltinSection::Ideas => ("Ideas", "Pomysły"),
            BuiltinSection::Budgets => ("Budgets", "Budżety"),
        };
        match language {
            Language::En => en,
            Language::Pl => pl,
        }
    }

    /// The built-in section that holds items of a type.
    pub fn of(item_type: ItemType) -> BuiltinSection {
        match item_type {
            ItemType::Task => BuiltinSection::Tasks,
            ItemType::Decision => BuiltinSection::Decisions,
            ItemType::Topic => BuiltinSection::OpenTopics,
            ItemType::Note => BuiltinSection::Notes,
            ItemType::Idea => BuiltinSection::Ideas,
            ItemType::Budget => BuiltinSection::Budgets,
        }
    }

    fn item_type(self) -> Option<ItemType> {
        match self {
            BuiltinSection::Summary => None,
            BuiltinSection::Decisions => Some(ItemType::Decision),
            BuiltinSection::Tasks => Some(ItemType::Task),
            BuiltinSection::OpenTopics => Some(ItemType::Topic),
            BuiltinSection::Notes => Some(ItemType::Note),
            BuiltinSection::Ideas => Some(ItemType::Idea),
            BuiltinSection::Budgets => Some(ItemType::Budget),
        }
    }
}

/// One item as a Markdown bullet line.
pub fn item_line(item: &Item, participants: &[Participant], variant: Variant) -> String {
    let text = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut line = String::from("- ");
    if item.item_type == ItemType::Task {
        if variant == Variant::Markdown {
            line.push_str("[ ] ");
        }
        line.push_str(&text);
        line.push_str(&format!(
            " ({}, {})",
            assignee_label(item, participants),
            due_label(item)
        ));
    } else {
        line.push_str(&text);
    }
    if matches!(&item.source, Some(s) if !s.verified) {
        line.push(' ');
        line.push_str(UNVERIFIED_MARKER);
    }
    line
}

fn assignee_label(item: &Item, participants: &[Participant]) -> String {
    match &item.assignee {
        None => "@?".to_string(),
        Some(id) => {
            let name = participants
                .iter()
                .find(|p| &p.id == id)
                .map_or(id.as_str(), |p| p.name.as_str());
            let mark = if item.certainty.assignee == CertaintyLevel::Stated {
                ""
            } else {
                "?"
            };
            format!("@{name}{mark}")
        }
    }
}

fn due_label(item: &Item) -> String {
    match &item.due {
        None => "due ?".to_string(),
        Some(due) => {
            let mark = if item.certainty.due == CertaintyLevel::Stated {
                ""
            } else {
                "?"
            };
            format!("due {due}{mark}")
        }
    }
}

struct Renderer<'a> {
    draft: &'a Draft,
    participants: &'a [Participant],
    opts: &'a RenderOptions,
}

impl Renderer<'_> {
    fn list<'i>(&self, items: impl Iterator<Item = &'i Item>) -> String {
        items
            .map(|i| item_line(i, self.participants, self.opts.variant))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn of_type(&self, t: ItemType) -> String {
        self.list(self.draft.items.iter().filter(|i| i.item_type == t))
    }

    /// Index of the section an item belongs to; null or unknown ids go to the last section.
    fn section_index(&self, item: &Item) -> Option<usize> {
        let sections = &self.draft.sections;
        if sections.is_empty() {
            return None;
        }
        let found = item
            .section
            .as_deref()
            .and_then(|id| sections.iter().position(|s| s.id == id));
        Some(found.unwrap_or(sections.len() - 1))
    }

    fn uses_builtin_sections(&self) -> bool {
        self.draft.mode == ReportMode::Fixed || self.draft.sections.is_empty()
    }

    fn sections_block(&self) -> String {
        let mut blocks = Vec::new();
        if self.uses_builtin_sections() {
            for section in BuiltinSection::ORDER {
                let Some(t) = section.item_type() else {
                    continue;
                };
                let list = self.of_type(t);
                if !list.is_empty() {
                    blocks.push(format!(
                        "## {}\n\n{list}",
                        section.heading(self.draft.language)
                    ));
                }
            }
        } else {
            for (idx, section) in self.draft.sections.iter().enumerate() {
                let list = self.list(
                    self.draft
                        .items
                        .iter()
                        .filter(|i| self.section_index(i) == Some(idx)),
                );
                if !list.is_empty() {
                    blocks.push(format!("## {}\n\n{list}", section.title));
                }
            }
        }
        blocks.join("\n\n")
    }

    fn section_list(&self, id: &str) -> String {
        let Some(idx) = self.draft.sections.iter().position(|s| s.id == id) else {
            return String::new();
        };
        if self.uses_builtin_sections() {
            return String::new();
        }
        self.list(
            self.draft
                .items
                .iter()
                .filter(|i| self.section_index(i) == Some(idx)),
        )
    }

    fn expand(&self, p: &Placeholder) -> String {
        match p {
            Placeholder::Title => self.opts.title.clone(),
            Placeholder::Date => self.draft.date.clone(),
            Placeholder::Participants => self
                .participants
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            Placeholder::Summary => self.draft.summary.trim().to_string(),
            Placeholder::Sections => self.sections_block(),
            Placeholder::Section(id) => self.section_list(id),
            Placeholder::Tasks => self.of_type(ItemType::Task),
            Placeholder::Decisions => self.of_type(ItemType::Decision),
            Placeholder::Topics => self.of_type(ItemType::Topic),
            Placeholder::Notes => self.of_type(ItemType::Note),
            Placeholder::Ideas => self.of_type(ItemType::Idea),
            Placeholder::Budgets => self.of_type(ItemType::Budget),
            Placeholder::TranscriptLink => self.opts.transcript_link.clone(),
        }
    }
}

/// Renders `template_body` for a draft.
///
/// * `{sections}` is every non-empty section as `## Title` and a list: the draft's sections
///   in template, auto and by-type mode, the built-in sections (localised headings, grouped
///   by type) in fixed mode or when the draft has no sections. Items without a section, or
///   with an unknown one, go to the last section.
/// * `{section:<id>}` is the list of one section; `{tasks}`, `{decisions}`, `{topics}`,
///   `{notes}`, `{ideas}` and `{budgets}` are the lists of one type.
/// * Headings (level 2 and deeper) with nothing under them are removed, so empty sections
///   never appear; runs of blank lines collapse to one.
///
/// An unknown or malformed placeholder is an error; see [`crate::validate_template`].
pub fn render(
    draft: &Draft,
    participants: &[Participant],
    template_body: &str,
    opts: &RenderOptions,
) -> std::result::Result<String, Vec<ValidationIssue>> {
    let body = template_body.replace("\r\n", "\n");
    let tokens = tokenize(&body)?;
    let renderer = Renderer {
        draft,
        participants,
        opts,
    };
    let mut out = String::new();
    for token in &tokens {
        match token {
            Token::Text(t) => out.push_str(t),
            Token::Placeholder(p) => out.push_str(&renderer.expand(p)),
        }
    }
    Ok(tidy(&out))
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// Removes empty headings (level 2 and deeper), trailing spaces and extra blank lines.
fn tidy(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    // Mark which lines sit inside a code fence: headings there are not headings.
    let mut in_fence = false;
    let fenced: Vec<bool> = lines
        .iter()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                in_fence = !in_fence;
                true
            } else {
                in_fence
            }
        })
        .collect();
    let level_at = |i: usize| {
        if fenced[i] {
            None
        } else {
            heading_level(lines[i])
        }
    };

    let mut keep = vec![true; lines.len()];
    for (i, kept) in keep.iter_mut().enumerate() {
        let Some(level) = level_at(i) else { continue };
        if level < 2 {
            continue;
        }
        let mut has_content = false;
        for j in i + 1..lines.len() {
            if !fenced[j] && lines[j] == "---" {
                break;
            }
            match level_at(j) {
                Some(l) if l <= level => break,
                Some(_) => {}
                None if !lines[j].is_empty() => {
                    has_content = true;
                    break;
                }
                None => {}
            }
        }
        *kept = has_content;
    }

    let mut out = String::new();
    let mut blank_run = 0;
    for (i, line) in lines.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::fixtures::*;
    use crate::draft::{Certainty, Section};
    use crate::template::DEFAULT_TEMPLATE_TEXT;

    fn people() -> Vec<Participant> {
        vec![
            Participant {
                id: "alex".into(),
                name: "Alex Example".into(),
                github: None,
                discord: None,
            },
            Participant {
                id: "sam".into(),
                name: "Sam Sample".into(),
                github: None,
                discord: None,
            },
        ]
    }

    fn opts() -> RenderOptions {
        RenderOptions {
            title: "Weekly sync".into(),
            ..RenderOptions::default()
        }
    }

    fn task(
        id: &str,
        text: &str,
        who: Option<&str>,
        due: Option<&str>,
        a: CertaintyLevel,
        d: CertaintyLevel,
    ) -> Item {
        let mut t = item(id, ItemType::Task, text);
        t.assignee = who.map(String::from);
        t.due = due.map(String::from);
        t.certainty = Certainty {
            assignee: a,
            due: d,
        };
        t
    }

    use CertaintyLevel::{Inferred, Stated, Unknown};

    #[test]
    fn task_line_conventions() {
        let p = people();
        let stated = task(
            "1",
            "Design the mount",
            Some("alex"),
            Some("2026-11-20"),
            Stated,
            Stated,
        );
        assert_eq!(
            item_line(&stated, &p, Variant::Markdown),
            "- [ ] Design the mount (@Alex Example, due 2026-11-20)"
        );
        let inferred = task(
            "2",
            "Order parts",
            Some("sam"),
            Some("2026-11-27"),
            Inferred,
            Inferred,
        );
        assert_eq!(
            item_line(&inferred, &p, Variant::Markdown),
            "- [ ] Order parts (@Sam Sample?, due 2026-11-27?)"
        );
        let unknown = task("3", "Check the wiring", None, None, Unknown, Unknown);
        assert_eq!(
            item_line(&unknown, &p, Variant::Markdown),
            "- [ ] Check the wiring (@?, due ?)"
        );
        let stranger = task("4", "Ping", Some("zed"), None, Stated, Unknown);
        assert_eq!(
            item_line(&stranger, &p, Variant::Chat),
            "- Ping (@zed, due ?)"
        );
    }

    #[test]
    fn unverified_marker_and_plain_items() {
        let p = people();
        let mut n = item("1", ItemType::Note, "Rover  weighs\n12 kg");
        assert_eq!(item_line(&n, &p, Variant::Markdown), "- Rover weighs 12 kg");
        n.source = Some(source(true));
        assert_eq!(item_line(&n, &p, Variant::Markdown), "- Rover weighs 12 kg");
        n.source = Some(source(false));
        assert_eq!(
            item_line(&n, &p, Variant::Markdown),
            "- Rover weighs 12 kg (unverified quote)"
        );
        let mut t = task("2", "Do it", Some("alex"), None, Stated, Unknown);
        t.source = Some(source(false));
        assert_eq!(
            item_line(&t, &p, Variant::Markdown),
            "- [ ] Do it (@Alex Example, due ?) (unverified quote)"
        );
    }

    fn fixed_draft(lang: Language) -> Draft {
        let mut d = draft(vec![
            item("n1", ItemType::Note, "Wheels are 20 cm"),
            task(
                "t1",
                "Design the mount",
                Some("alex"),
                Some("2026-11-20"),
                Stated,
                Stated,
            ),
            item("d1", ItemType::Decision, "Use aluminium"),
            item("i1", ItemType::Idea, "Add a camera"),
            item("b1", ItemType::Budget, "300 PLN for sensors"),
            item("o1", ItemType::Topic, "Battery supplier"),
        ]);
        d.language = lang;
        d
    }

    #[test]
    fn default_template_in_english() {
        let md = render(
            &fixed_draft(Language::En),
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            &opts(),
        )
        .unwrap();
        let expected = "# Weekly sync\n\n2026-11-13 · Alex Example, Sam Sample\n\n## Summary\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n## Tasks\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n## Open topics\n\n- Battery supplier\n\n## Notes\n\n- Wheels are 20 cm\n\n## Ideas\n\n- Add a camera\n\n## Budgets\n\n- 300 PLN for sensors\n\n---\n[Full transcript](transcript.md)\n";
        assert_eq!(md, expected);
    }

    #[test]
    fn built_in_headings_are_localised() {
        let md = render(&fixed_draft(Language::Pl), &people(), "{sections}", &opts()).unwrap();
        for h in [
            "## Decyzje",
            "## Zadania",
            "## Otwarte tematy",
            "## Notatki",
            "## Pomysły",
            "## Budżety",
        ] {
            assert!(md.contains(h), "missing {h} in {md}");
        }
        assert_eq!(
            BuiltinSection::Summary.heading(Language::Pl),
            "Podsumowanie"
        );
        assert_eq!(BuiltinSection::Summary.heading(Language::En), "Summary");
        // Order of the built-in sections.
        assert!(md.find("## Decyzje").unwrap() < md.find("## Zadania").unwrap());
        assert!(md.find("## Notatki").unwrap() < md.find("## Pomysły").unwrap());
    }

    #[test]
    fn empty_sections_and_headings_are_omitted() {
        let d = draft(vec![item("n1", ItemType::Note, "Only a note")]);
        let md = render(
            &d,
            &people(),
            "# {title}\n\n## Tasks\n{tasks}\n\n{sections}\n",
            &opts(),
        )
        .unwrap();
        assert!(!md.contains("Tasks"), "{md}");
        assert!(md.contains("## Notes\n\n- Only a note"));
        let mut quiet = d.clone();
        quiet.summary = "  ".into();
        let md = render(&quiet, &people(), DEFAULT_TEMPLATE_TEXT, &opts()).unwrap();
        assert!(!md.contains("## Summary"), "{md}");
        assert!(md.contains("---\n[Full transcript]"));
    }

    fn template_draft() -> Draft {
        let mut d = draft(vec![]);
        d.mode = ReportMode::Template;
        d.sections = vec![
            Section {
                id: "progress".into(),
                title: "Progress".into(),
                description: String::new(),
            },
            Section {
                id: "blockers".into(),
                title: "Blockers".into(),
                description: String::new(),
            },
            Section {
                id: "other".into(),
                title: "Other".into(),
                description: String::new(),
            },
        ];
        let mut a = item("a", ItemType::Note, "Chassis done");
        a.section = Some("progress".into());
        let mut b = task(
            "b",
            "Fix the motor driver",
            Some("sam"),
            None,
            Stated,
            Unknown,
        );
        b.section = Some("blockers".into());
        let c = item("c", ItemType::Idea, "No section given");
        let mut e = item("e", ItemType::Note, "Unknown section id");
        e.section = Some("nope".into());
        d.items = vec![a, b, c, e];
        d
    }

    #[test]
    fn sections_follow_the_draft_in_template_mode() {
        let md = render(&template_draft(), &people(), "{sections}", &opts()).unwrap();
        assert_eq!(
            md,
            "## Progress\n\n- Chassis done\n\n## Blockers\n\n- [ ] Fix the motor driver (@Sam Sample, due ?)\n\n## Other\n\n- No section given\n- Unknown section id\n"
        );
    }

    #[test]
    fn single_section_and_type_placeholders() {
        let d = template_draft();
        let body = "# {title}\n\nDone:\n{section:progress}\n\nTasks only:\n{tasks}\n\nIdeas:\n{ideas}\n\n{section:ghost}\n{decisions}\n{budgets}{topics}{notes}\n";
        let md = render(&d, &people(), body, &opts()).unwrap();
        assert!(md.contains("Done:\n- Chassis done\n"), "{md}");
        assert!(md.contains("Tasks only:\n- [ ] Fix the motor driver"));
        assert!(md.contains("Ideas:\n- No section given"));
        assert!(md.contains("- Chassis done\n- Unknown section id"));
        assert!(!md.contains("ghost"));
    }

    #[test]
    fn other_placeholders() {
        let body = "{title}|{date}|{participants}|{summary}|{transcript_link}";
        let o = RenderOptions {
            transcript_link: "../transcript.md".into(),
            ..opts()
        };
        let md = render(&draft(vec![]), &people(), body, &o).unwrap();
        assert_eq!(
            md,
            "Weekly sync|2026-11-13|Alex Example, Sam Sample|We agreed on the mount.|../transcript.md\n"
        );
    }

    #[test]
    fn variant_changes_only_task_checkboxes() {
        let d = fixed_draft(Language::En);
        let chat = RenderOptions {
            variant: Variant::Chat,
            ..opts()
        };
        let md = render(&d, &people(), "{tasks}\n{notes}", &chat).unwrap();
        assert_eq!(
            md,
            "- Design the mount (@Alex Example, due 2026-11-20)\n- Wheels are 20 cm\n"
        );
    }

    #[test]
    fn unknown_placeholder_is_an_error_not_dropped() {
        let err = render(&draft(vec![]), &people(), "{title} {assignees}", &opts()).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].message.contains("{assignees}"));
    }

    #[test]
    fn literal_braces_and_crlf_templates() {
        let md = render(
            &draft(vec![]),
            &people(),
            "{{x}} {date}\r\nnext\r\n",
            &opts(),
        )
        .unwrap();
        assert_eq!(md, "{x} 2026-11-13\nnext\n");
    }

    #[test]
    fn headings_inside_code_fences_are_kept() {
        let body = "```\n## not a heading\n```\n\n## Real\n{tasks}\n";
        let md = render(&draft(vec![]), &people(), body, &opts()).unwrap();
        assert!(md.contains("## not a heading"));
        assert!(!md.contains("## Real"));
    }

    #[test]
    fn rendering_is_deterministic() {
        let d = template_draft();
        let a = render(&d, &people(), DEFAULT_TEMPLATE_TEXT, &opts()).unwrap();
        let b = render(&d, &people(), DEFAULT_TEMPLATE_TEXT, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
