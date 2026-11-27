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
//! * The words `due` and `unverified quote` are the same in every report language: item
//!   lines stay machine-friendly. Only the built-in headings are localised.

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
    /// Target of `{report_link}`: where the full report is published.
    pub report_link: String,
    /// Discord: an assignee with a Discord id is written as a mention (`<@id>` for a user,
    /// `<@&id>` for a role) instead of `@Name`, and mention syntax in text that came from the
    /// model or the transcript is defused, so only the mentions made here can reach anybody.
    pub discord_mentions: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            title: String::new(),
            variant: Variant::Markdown,
            transcript_link: "transcript.md".to_string(),
            report_link: "report.md".to_string(),
            discord_mentions: false,
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

/// Puts a zero-width space after the `@` of `@everyone` and `@here` (any case), so a report
/// posted to a chat service cannot notify everybody. Other text is returned unchanged.
pub fn neutralise_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..=at]);
        rest = &rest[at + 1..];
        let head: String = rest.chars().take(8).collect::<String>().to_lowercase();
        if head.starts_with("everyone") || head.starts_with("here") {
            out.push('\u{200b}');
        }
    }
    out.push_str(rest);
    out
}

/// One item as a Markdown bullet line.
pub fn item_line(item: &Item, participants: &[Participant], variant: Variant) -> String {
    let opts = RenderOptions {
        variant,
        ..RenderOptions::default()
    };
    item_line_with(item, participants, &opts)
}

/// [`item_line`] with the options of a render (variant and Discord mentions).
pub fn item_line_with(item: &Item, participants: &[Participant], opts: &RenderOptions) -> String {
    let text = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = if opts.discord_mentions {
        defuse_discord_mentions(&text)
    } else {
        text
    };
    let mut line = String::from("- ");
    if item.item_type == ItemType::Task {
        if opts.variant == Variant::Markdown {
            line.push_str("[ ] ");
        }
        line.push_str(&text);
        line.push_str(&format!(
            " ({}, {})",
            assignee_label(item, participants, opts.discord_mentions),
            due_label(item)
        ));
    } else {
        line.push_str(&text);
    }
    if matches!(&item.source, Some(s) if !s.verified) {
        line.push(' ');
        line.push_str(UNVERIFIED_MARKER);
    }
    neutralise_mentions(&line)
}

/// Puts a zero-width space inside `<@`, so text that came from a model or a transcript cannot
/// form a Discord user, role or channel mention.
pub fn defuse_discord_mentions(text: &str) -> String {
    text.replace("<@", "<\u{200b}@")
}

/// Whether `id` can be written inside `<@...>`: a Discord snowflake, digits only.
pub fn is_discord_id(id: &str) -> bool {
    (1..=25).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

fn discord_mention(person: &Participant) -> Option<String> {
    let d = person.discord.as_ref()?;
    match (d.user_id.as_deref(), d.role_id.as_deref()) {
        (Some(u), None) if is_discord_id(u) => Some(format!("<@{u}>")),
        (None, Some(r)) if is_discord_id(r) => Some(format!("<@&{r}>")),
        _ => None,
    }
}

fn assignee_label(item: &Item, participants: &[Participant], discord: bool) -> String {
    let mark = if item.certainty.assignee == CertaintyLevel::Stated {
        ""
    } else {
        "?"
    };
    let outside = item
        .assignee_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    match (&item.assignee, outside) {
        (Some(id), _) => {
            let person = participants.iter().find(|p| &p.id == id);
            if let Some(mention) = person.filter(|_| discord).and_then(discord_mention) {
                return format!("{mention}{mark}");
            }
            let name = person.map_or(id.as_str(), |p| p.name.as_str());
            format!("@{name}{mark}")
        }
        (None, Some(name)) => {
            let safe_name = if discord {
                defuse_discord_mentions(name)
            } else {
                name.to_string()
            };
            format!("@{safe_name}{mark}")
        }
        (None, None) => "@?".to_string(),
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
    /// Text that came from the model, the transcript or the cycle files.
    fn plain(&self, text: &str) -> String {
        if self.opts.discord_mentions {
            defuse_discord_mentions(text)
        } else {
            text.to_string()
        }
    }

    fn list<'i>(&self, items: impl Iterator<Item = &'i Item>) -> String {
        items
            .map(|i| item_line_with(i, self.participants, self.opts))
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
                    blocks.push(format!("## {}\n\n{list}", self.plain(&section.title)));
                }
            }
        }
        blocks.join("\n\n")
    }

    /// The summary under its localised heading, or nothing when the summary is empty.
    fn summary_section(&self) -> String {
        let summary = self.draft.summary.trim();
        if summary.is_empty() {
            return String::new();
        }
        format!(
            "## {}\n\n{}",
            BuiltinSection::Summary.heading(self.draft.language),
            self.plain(summary)
        )
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
            Placeholder::Title => self.plain(&self.opts.title),
            Placeholder::Date => self.draft.date.clone(),
            Placeholder::Participants => self
                .participants
                .iter()
                .map(|p| self.plain(&p.name))
                .collect::<Vec<_>>()
                .join(", "),
            Placeholder::Summary => self.plain(self.draft.summary.trim()),
            Placeholder::SummarySection => self.summary_section(),
            Placeholder::Sections => self.sections_block(),
            Placeholder::Section(id) => self.section_list(id),
            Placeholder::Tasks => self.of_type(ItemType::Task),
            Placeholder::Decisions => self.of_type(ItemType::Decision),
            Placeholder::Topics => self.of_type(ItemType::Topic),
            Placeholder::Notes => self.of_type(ItemType::Note),
            Placeholder::Ideas => self.of_type(ItemType::Idea),
            Placeholder::Budgets => self.of_type(ItemType::Budget),
            Placeholder::TranscriptLink => self.opts.transcript_link.clone(),
            Placeholder::ReportLink => self.opts.report_link.clone(),
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
/// * `{summary}` is the bare summary text; `{summary_section}` is the summary under the
///   localised heading (`## Summary` or `## Podsumowanie`) and nothing when the summary is
///   empty.
/// * Headings (level 2 and deeper) with nothing under them are removed, so empty sections
///   never appear. The result has exactly one blank line between blocks (a heading always
///   has one before and after it), no trailing spaces and a single final newline.
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
            Token::Placeholder(p) => {
                let text = renderer.expand(p);
                if text.trim().is_empty() {
                    // Remembered for `tidy`: a heading that only empty placeholders follow
                    // is an empty section, whatever static text comes after them.
                    out.push(EMPTY_MARK);
                } else {
                    out.push_str(&text);
                }
            }
        }
    }
    // Item text, the summary and section titles come from a model that the transcript can
    // steer: the report must not be able to notify everybody where it is posted.
    Ok(neutralise_mentions(&tidy(&out)))
}

/// Stands in the text for a placeholder that expanded to nothing; `tidy` removes it.
const EMPTY_MARK: char = '\u{e000}';

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// Removes empty headings (level 2 and deeper), trailing spaces and extra blank lines, and
/// puts one blank line around every heading. A heading is empty when nothing but blank lines
/// follows it before the next heading or `---`, or when the first block under it is made of
/// placeholders that expanded to nothing (`## Tasks\n{tasks}\n\n[Full report](...)`: the link
/// is not the content of the tasks section).
fn tidy(text: &str) -> String {
    // Lines that hold nothing but marks of empty placeholders.
    let raw: Vec<&str> = text.lines().collect();
    let marked: Vec<bool> = raw
        .iter()
        .map(|l| l.contains(EMPTY_MARK) && l.chars().all(|c| c == EMPTY_MARK || c.is_whitespace()))
        .collect();
    let cleaned: Vec<String> = raw.iter().map(|l| l.replace(EMPTY_MARK, "")).collect();
    let lines: Vec<&str> = cleaned.iter().map(|l| l.trim_end()).collect();
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

    // A line that is blank for real, not just because an empty placeholder stood on it.
    let plain_blank = |j: usize| lines[j].is_empty() && !marked[j];
    let first_block_is_empty = |i: usize| {
        let mut j = i + 1;
        while j < lines.len() && plain_blank(j) {
            j += 1;
        }
        let start = j;
        while j < lines.len() && !plain_blank(j) {
            if !marked[j] {
                return false;
            }
            j += 1;
        }
        j > start
    };

    let mut keep = vec![true; lines.len()];
    for (i, kept) in keep.iter_mut().enumerate() {
        let Some(level) = level_at(i) else { continue };
        if level < 2 {
            continue;
        }
        if first_block_is_empty(i) {
            *kept = false;
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
    // True at the start of the output and right after a blank line.
    let mut at_block_start = true;
    let mut prev_heading = false;
    for (i, line) in lines.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if line.is_empty() {
            if !at_block_start {
                out.push('\n');
                at_block_start = true;
            }
            continue;
        }
        let heading = level_at(i).is_some();
        // A heading is always set apart from the text before and after it.
        if !at_block_start && (heading || prev_heading) {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
        at_block_start = false;
        prev_heading = heading;
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
    fn an_outside_assignee_name_is_shown_and_a_blank_one_is_not() {
        let p = people();
        let mut named = task("1", "Ping", None, None, Inferred, Unknown);
        named.assignee_name = Some("Marta Outside".into());
        assert_eq!(
            item_line(&named, &p, Variant::Markdown),
            "- [ ] Ping (@Marta Outside?, due ?)"
        );
        named.certainty.assignee = Stated;
        assert_eq!(
            item_line(&named, &p, Variant::Chat),
            "- Ping (@Marta Outside, due ?)"
        );
        named.assignee_name = Some("   ".into());
        assert_eq!(item_line(&named, &p, Variant::Chat), "- Ping (@?, due ?)");
        // A participant id wins over a name.
        named.assignee = Some("alex".into());
        named.assignee_name = Some("Marta Outside".into());
        assert_eq!(
            item_line(&named, &p, Variant::Chat),
            "- Ping (@Alex Example, due ?)"
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
        let expected = "# Weekly sync\n\n2026-11-13 · Alex Example, Sam Sample\n\n## Summary\n\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n## Tasks\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n## Open topics\n\n- Battery supplier\n\n## Notes\n\n- Wheels are 20 cm\n\n## Ideas\n\n- Add a camera\n\n## Budgets\n\n- 300 PLN for sensors\n\n---\n[Full transcript](transcript.md)\n";
        assert_eq!(md, expected);
    }

    #[test]
    fn mass_mentions_are_neutralised_everywhere_in_the_report() {
        let mut d = fixed_draft(Language::En);
        d.summary = "Ping @everyone and @HERE now; mail me@here.example".into();
        d.items.push(item(
            "n2",
            ItemType::Note,
            "Tell @everyone, not @hereafter-person",
        ));
        let md = render(&d, &people(), "{summary}\n\n{sections}", &opts()).unwrap();
        for bad in ["@everyone", "@here", "@HERE", "@Everyone"] {
            assert!(!md.contains(bad), "{bad} in {md}");
        }
        assert!(md.contains("@\u{200b}everyone"));
        assert!(md.contains("@\u{200b}HERE"));
        // Other mentions and the text around them are untouched.
        assert_eq!(neutralise_mentions("@alex and @team"), "@alex and @team");
        let line = item_line(
            &item("x", ItemType::Note, "cc @everyone"),
            &people(),
            Variant::Chat,
        );
        assert_eq!(line, "- cc @\u{200b}everyone");
        // A stranger id used as the assignee cannot notify everybody either.
        let t = task("t", "Do it", Some("everyone"), None, Stated, Unknown);
        assert!(!item_line(&t, &people(), Variant::Chat).contains("@everyone"));
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

    #[test]
    fn default_template_in_polish_has_no_english_heading() {
        let md = render(
            &fixed_draft(Language::Pl),
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            &opts(),
        )
        .unwrap();
        let expected = "# Weekly sync\n\n2026-11-13 · Alex Example, Sam Sample\n\n## Podsumowanie\n\nWe agreed on the mount.\n\n## Decyzje\n\n- Use aluminium\n\n## Zadania\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n\n## Otwarte tematy\n\n- Battery supplier\n\n## Notatki\n\n- Wheels are 20 cm\n\n## Pomysły\n\n- Add a camera\n\n## Budżety\n\n- 300 PLN for sensors\n\n---\n[Full transcript](transcript.md)\n";
        assert_eq!(md, expected);
        for english in [
            "Summary",
            "Decisions",
            "Tasks",
            "Open topics",
            "Notes",
            "Ideas",
            "Budgets",
        ] {
            assert!(!md.contains(&format!("## {english}")), "{english} in {md}");
        }
    }

    #[test]
    fn summary_section_is_localised_and_vanishes_when_empty() {
        let mut d = fixed_draft(Language::Pl);
        assert_eq!(
            render(&d, &people(), "{summary_section}", &opts()).unwrap(),
            "## Podsumowanie\n\nWe agreed on the mount.\n"
        );
        d.language = Language::En;
        assert_eq!(
            render(&d, &people(), "{summary_section}", &opts()).unwrap(),
            "## Summary\n\nWe agreed on the mount.\n"
        );
        // Surrounding whitespace of the summary is dropped.
        d.summary = "\n  Two lines  \n second  \n".into();
        assert_eq!(
            render(&d, &people(), "{summary_section}", &opts()).unwrap(),
            "## Summary\n\nTwo lines\n second\n"
        );
        // An empty summary leaves no heading and no gap.
        for empty in ["", "  \n "] {
            d.summary = empty.into();
            assert_eq!(
                render(&d, &people(), "{summary_section}", &opts()).unwrap(),
                ""
            );
            let body = "# T\n\n{summary_section}\n\n{tasks}\n";
            let md = render(&d, &people(), body, &opts()).unwrap();
            assert_eq!(
                md,
                "# T\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n"
            );
        }
    }

    #[test]
    fn custom_template_keeps_the_bare_summary() {
        let mut d = fixed_draft(Language::Pl);
        d.summary = "Ustalono uchwyt.".into();
        let body = "# {title}\n\n## Streszczenie\n{summary}\n\n{tasks}\n";
        let md = render(&d, &people(), body, &opts()).unwrap();
        assert_eq!(
            md,
            "# Weekly sync\n\n## Streszczenie\n\nUstalono uchwyt.\n\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n"
        );
        assert!(!md.contains("Podsumowanie"));
    }

    #[test]
    fn blocks_are_separated_by_exactly_one_blank_line() {
        let mut d = fixed_draft(Language::En);
        d.summary = "First.  \n\n\n\nSecond.   ".into();
        let body = "\n\n# {title}   \n\n\n\n{summary_section}\n\n\n\n{sections}\n\n\n---\n[Full transcript](transcript.md)   \n\n\n";
        let md = render(&d, &people(), body, &opts()).unwrap();
        assert!(!md.contains("\n\n\n"), "{md:?}");
        assert!(!md.contains(" \n"), "{md:?}");
        assert!(md.ends_with("(transcript.md)\n"));
        assert!(md.starts_with("# Weekly sync\n\n## Summary\n\nFirst.\n\nSecond.\n\n## Decisions"));
        // A heading written without blank lines around it gets them.
        let tight = render(&d, &people(), "# A\n## B\ntext\n{tasks}\n", &opts()).unwrap();
        assert_eq!(
            tight,
            "# A\n\n## B\n\ntext\n- [ ] Design the mount (@Alex Example, due 2026-11-20)\n"
        );
        // Headings inside a code fence are left alone.
        let fenced = render(&d, &people(), "```\n# x\ntext\n```\n", &opts()).unwrap();
        assert_eq!(fenced, "```\n# x\ntext\n```\n");
    }

    #[test]
    fn due_wording_is_the_same_in_polish() {
        let md = render(&fixed_draft(Language::Pl), &people(), "{tasks}", &opts()).unwrap();
        assert_eq!(
            md,
            "- [ ] Design the mount (@Alex Example, due 2026-11-20)\n"
        );
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

    fn discord_people() -> Vec<Participant> {
        use crate::cycle::DiscordRef;
        let mut p = people();
        p[0].discord = Some(DiscordRef {
            user_id: Some("111111111111111111".into()),
            role_id: None,
        });
        p[1].discord = Some(DiscordRef {
            user_id: None,
            role_id: Some("222222222222222222".into()),
        });
        p
    }

    fn discord_opts() -> RenderOptions {
        RenderOptions {
            variant: Variant::Chat,
            discord_mentions: true,
            ..opts()
        }
    }

    #[test]
    fn discord_assignees_become_user_and_role_mentions() {
        let p = discord_people();
        let o = discord_opts();
        let user = task("1", "Design", Some("alex"), None, Stated, Unknown);
        assert_eq!(
            item_line_with(&user, &p, &o),
            "- Design (<@111111111111111111>, due ?)"
        );
        let role = task("2", "Order", Some("sam"), None, Inferred, Unknown);
        assert_eq!(
            item_line_with(&role, &p, &o),
            "- Order (<@&222222222222222222>?, due ?)"
        );
        // Not a participant, or a participant without a Discord id: the name, no mention.
        let stranger = task("3", "Ping", Some("zed"), None, Stated, Unknown);
        assert_eq!(item_line_with(&stranger, &p, &o), "- Ping (@zed, due ?)");
        let plain = task("4", "Ping", Some("alex"), None, Stated, Unknown);
        assert_eq!(
            item_line_with(&plain, &people(), &o),
            "- Ping (@Alex Example, due ?)"
        );
        // Without the option the name is used even when an id exists.
        assert_eq!(
            item_line(&user, &p, Variant::Chat),
            "- Design (@Alex Example, due ?)"
        );
    }

    #[test]
    fn a_malformed_discord_id_is_never_written_into_a_mention() {
        use crate::cycle::DiscordRef;
        let mut p = people();
        p[0].discord = Some(DiscordRef {
            user_id: Some("1> @everyone <@2".into()),
            role_id: None,
        });
        let t = task("1", "Do", Some("alex"), None, Stated, Unknown);
        let line = item_line_with(&t, &p, &discord_opts());
        assert_eq!(line, "- Do (@Alex Example, due ?)");
        assert!(!is_discord_id(""));
        assert!(!is_discord_id("12a"));
        assert!(is_discord_id("123456789012345678"));
    }

    #[test]
    fn discord_mention_syntax_in_model_text_is_defused() {
        let p = discord_people();
        let mut d = fixed_draft(Language::En);
        d.summary = "Hello <@999999999999999999> and <@&888888888888888888>".into();
        d.items
            .push(item("n2", ItemType::Note, "cc <@777777777777777777>"));
        let md = render(&d, &p, "{summary}\n{notes}\n{tasks}", &discord_opts()).unwrap();
        assert!(!md.contains("<@9"), "{md}");
        assert!(!md.contains("<@&8"), "{md}");
        assert!(!md.contains("<@7"), "{md}");
        assert!(md.contains("<\u{200b}@999999999999999999>"));
        // The mention made from the cycle file survives.
        assert!(md.contains("<@111111111111111111>"), "{md}");
        // Markdown rendering is not affected by the option being off.
        let md = render(&d, &p, "{summary}", &opts()).unwrap();
        assert!(md.contains("<@999999999999999999>"));
    }

    #[test]
    fn report_link_placeholder() {
        let o = RenderOptions {
            report_link: "https://example.test/r/report.md".into(),
            ..opts()
        };
        let md = render(&draft(vec![]), &people(), "[Report]({report_link})", &o).unwrap();
        assert_eq!(md, "[Report](https://example.test/r/report.md)\n");
        assert!(crate::template::PLACEHOLDERS.contains(&"report_link"));
    }

    #[test]
    fn short_template_renders_a_compact_message() {
        use crate::template::SHORT_TEMPLATE_TEXT;
        let o = RenderOptions {
            report_link: "https://example.test/r".into(),
            ..discord_opts()
        };
        let md = render(
            &fixed_draft(Language::En),
            &discord_people(),
            SHORT_TEMPLATE_TEXT,
            &o,
        )
        .unwrap();
        assert_eq!(
            md,
            "**Weekly sync** · 2026-11-13\n\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n## Tasks\n\n- Design the mount (<@111111111111111111>, due 2026-11-20)\n\n[Full report](https://example.test/r)\n"
        );
    }

    fn short_message(items: Vec<Item>) -> String {
        use crate::template::SHORT_TEMPLATE_TEXT;
        let o = RenderOptions {
            report_link: "https://example.test/r".into(),
            ..discord_opts()
        };
        render(&draft(items), &discord_people(), SHORT_TEMPLATE_TEXT, &o).unwrap()
    }

    #[test]
    fn short_template_has_no_heading_for_a_section_without_items() {
        // No tasks: the link below the heading must not keep an empty "Tasks" heading alive.
        let md = short_message(vec![item("d1", ItemType::Decision, "Use aluminium")]);
        assert_eq!(
            md,
            "**Weekly sync** · 2026-11-13\n\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n[Full report](https://example.test/r)\n"
        );
        assert!(!md.contains("Tasks"), "{md}");

        // No decisions: the other way round.
        let md = short_message(vec![task(
            "t1",
            "Design the mount",
            Some("alex"),
            None,
            Stated,
            Unknown,
        )]);
        assert!(!md.contains("Decisions"), "{md}");
        assert!(md.contains("## Tasks\n\n- Design the mount"), "{md}");

        // Neither: no section heading at all, the summary and the link remain.
        let md = short_message(vec![item("n1", ItemType::Note, "Wheels are 20 cm")]);
        assert_eq!(
            md,
            "**Weekly sync** · 2026-11-13\n\nWe agreed on the mount.\n\n[Full report](https://example.test/r)\n"
        );
        assert!(!md.contains("##"), "{md}");
    }

    #[test]
    fn a_heading_followed_only_by_empty_placeholders_is_removed_in_any_template() {
        let o = opts();
        let d = draft(vec![item("n1", ItemType::Note, "Wheels are 20 cm")]);
        let body = "## Tasks\n{tasks}\n\n## Work\n\n{tasks}\n{decisions}\n\nfooter\n\n## Notes\n{notes}\n\n[link](x)\n";
        let md = render(&d, &people(), body, &o).unwrap();
        assert_eq!(
            md,
            "footer\n\n## Notes\n\n- Wheels are 20 cm\n\n[link](x)\n"
        );
        // A heading with static text of its own is not empty.
        let md = render(&d, &people(), "## Tasks\nnone yet {tasks}\n", &o).unwrap();
        assert_eq!(md, "## Tasks\n\nnone yet\n");
        // One empty and one filled placeholder under the same heading keep it.
        let md = render(&d, &people(), "## Mixed\n{tasks}\n{notes}\n", &o).unwrap();
        assert_eq!(md, "## Mixed\n\n- Wheels are 20 cm\n");
    }

    #[test]
    fn the_default_template_has_no_empty_headings_in_either_variant() {
        let d = draft(vec![item("d1", ItemType::Decision, "Use aluminium")]);
        for variant in [Variant::Markdown, Variant::Chat] {
            let o = RenderOptions { variant, ..opts() };
            let md = render(&d, &people(), DEFAULT_TEMPLATE_TEXT, &o).unwrap();
            assert_eq!(
                md,
                "# Weekly sync\n\n2026-11-13 · Alex Example, Sam Sample\n\n## Summary\n\nWe agreed on the mount.\n\n## Decisions\n\n- Use aluminium\n\n---\n[Full transcript](transcript.md)\n"
            );
        }
        // No items and no summary: only the title line and the transcript link are left.
        let mut empty = draft(vec![]);
        empty.summary = String::new();
        let md = render(&empty, &people(), DEFAULT_TEMPLATE_TEXT, &opts()).unwrap();
        assert!(!md.contains("##"), "{md}");
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
