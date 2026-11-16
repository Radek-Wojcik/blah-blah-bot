//! Builds `work/context.json` (`schemas/cycle-context.schema.json`): what the extraction
//! needs to know about the cycle. Nothing here reads the application database.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::corrections::{self, CorrectionsFile};
use crate::cycle::Cycle;
use crate::draft::{Draft, ItemType, LanguageSetting, ReportMode};
use crate::error::{CycleError, Result, ValidationIssue};
use crate::issues::{IssueLink, OpenIssue};
use crate::schema::{self, Schema};
use crate::template::Template;
use crate::time::is_iso_date;
use crate::topics::{Topic, TopicStatus};

/// Default cap on the items of earlier meetings put into the context.
pub const DEFAULT_MAX_PREVIOUS_ITEMS: usize = 80;
/// Default cap on the corrections of earlier meetings put into the context.
pub const DEFAULT_MAX_CORRECTIONS: usize = 30;

/// Inputs of [`build_context`].
pub struct ContextRequest<'a> {
    pub cycle: &'a Cycle,
    /// The cycle's templates (needed for template and by-type mode).
    pub templates: &'a [Template],
    /// All topics of the cycle, open and closed.
    pub topics: &'a [Topic],
    /// Open issues from the cache.
    pub issues: &'a [OpenIssue],
    /// Numbers of issues known to be closed (`12` or `#12`); items relating to them are
    /// dropped from the previous items.
    pub closed_issue_ids: &'a [String],
    /// The issues created from items of earlier meetings. An item whose issue is closed is
    /// done and dropped from the previous items; one whose issue is open carries it as
    /// `issue`.
    pub issue_links: &'a [IssueLink],
    /// `YYYY-MM-DD`.
    pub meeting_date: &'a str,
    /// Overrides the cycle's default mode.
    pub mode: Option<ReportMode>,
    /// Selects the template in by-type mode.
    pub meeting_type: Option<&'a str>,
    /// Overrides the template of template mode.
    pub template: Option<&'a str>,
    /// Overrides the cycle's output language.
    pub output_language: Option<LanguageSetting>,
    /// Directories holding one `<date>/meeting.json` per earlier meeting, typically
    /// `meetings/<ID>`.
    pub previous_dirs: &'a [PathBuf],
    /// A meeting never to include (the one being processed).
    pub exclude_meeting_id: Option<&'a str>,
    pub max_previous_items: usize,
    pub max_corrections: usize,
}

impl<'a> ContextRequest<'a> {
    pub fn new(cycle: &'a Cycle, meeting_date: &'a str) -> Self {
        Self {
            cycle,
            templates: &[],
            topics: &[],
            issues: &[],
            closed_issue_ids: &[],
            issue_links: &[],
            meeting_date,
            mode: None,
            meeting_type: None,
            template: None,
            output_language: None,
            previous_dirs: &[],
            exclude_meeting_id: None,
            max_previous_items: DEFAULT_MAX_PREVIOUS_ITEMS,
            max_corrections: DEFAULT_MAX_CORRECTIONS,
        }
    }
}

/// The context document and the problems found while collecting earlier meetings.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub value: Value,
    /// Earlier `meeting.json` files that were skipped, and fallbacks that were taken.
    pub warnings: Vec<String>,
}

/// Builds the context. Earlier meetings that cannot be read or do not validate are skipped
/// with a warning; the call fails only when the request itself is wrong (bad date, a
/// template that the mode needs but does not exist).
pub fn build_context(req: &ContextRequest<'_>) -> Result<Context> {
    if !is_iso_date(req.meeting_date) {
        return Err(CycleError::Other(format!(
            "meeting date {:?} is not YYYY-MM-DD",
            req.meeting_date
        )));
    }
    let cycle = req.cycle;
    let mode = req.mode.unwrap_or(cycle.report.default_mode);
    let mut warnings = Vec::new();

    let template_name = resolve_template_name(req, mode, &mut warnings);
    let template = template_name
        .as_deref()
        .and_then(|n| req.templates.iter().find(|t| t.name == n));
    if mode.uses_template_sections() && template.is_none() {
        return Err(CycleError::Other(format!(
            "template {:?} is needed for report mode {:?} but was not found",
            template_name.as_deref().unwrap_or(""),
            mode
        )));
    }
    let sections: Vec<Value> = match template {
        Some(t) if mode.uses_template_sections() => t
            .sections
            .iter()
            .map(|s| json!({"id": s.id, "title": s.title, "description": s.description}))
            .collect(),
        _ => Vec::new(),
    };

    let open_topics: Vec<Value> = req
        .topics
        .iter()
        .filter(|t| t.status == TopicStatus::Open)
        .map(|t| json!({"id": t.id, "text": t.text}))
        .collect();
    let open_issues: Vec<Value> = req
        .issues
        .iter()
        .map(|i| json!({"id": i.id(), "title": i.title, "assignee": i.assignee}))
        .collect();

    let closed = ClosedThings::new(req.topics, req.closed_issue_ids);
    let earlier = read_earlier(req, &mut warnings);
    let previous_items = collect_previous_items(req, &earlier, &closed);
    let corrections = collect_corrections(req, &earlier, &mut warnings);

    let mut value = json!({
        "version": 1,
        "cycle": cycle.id,
        "name": cycle.name,
        "meeting_date": req.meeting_date,
        "participants": cycle.participants.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>(),
        "glossary": cycle.glossary.iter().map(|g| match &g.hint {
            Some(h) => json!({"term": g.term, "hint": h}),
            None => json!({"term": g.term}),
        }).collect::<Vec<_>>(),
        "instruction": cycle.report.instruction,
        "output_language": req.output_language.unwrap_or(cycle.languages.output),
        "mode": mode,
        "template": template_name.filter(|_| template.is_some()),
        "meeting_type": req.meeting_type,
        "sections": sections,
        "open_topics": open_topics,
        "open_issues": open_issues,
        "previous_items": previous_items,
    });
    // Only when there are some: a cycle without corrections gets the context it always had.
    if !corrections.is_empty() {
        value["corrections"] = Value::Array(corrections);
    }

    let issues = schema::validate(Schema::Context, &value);
    if !issues.is_empty() {
        return Err(CycleError::invalid("cycle context", issues));
    }
    Ok(Context { value, warnings })
}

/// The template that lays out the report: the by-type template, an explicit override, or
/// the cycle default. In by-type mode a missing or unmapped meeting type falls back to the
/// default template with a warning.
fn resolve_template_name(
    req: &ContextRequest<'_>,
    mode: ReportMode,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let default = req.cycle.report.default_template.clone();
    match mode {
        ReportMode::ByType => match req.meeting_type {
            Some(kind) => match req.cycle.report.by_type.get(kind) {
                Some(name) => Some(name.clone()),
                None => {
                    warnings.push(format!(
                        "meeting type {kind:?} has no template in report.by_type; using {default:?}"
                    ));
                    Some(default)
                }
            },
            None => {
                warnings.push(format!(
                    "by-type mode without a meeting type; using {default:?}"
                ));
                Some(default)
            }
        },
        _ => Some(req.template.map_or(default, String::from)),
    }
}

struct ClosedThings {
    refs: HashSet<String>,
    topic_texts: HashSet<String>,
}

fn normalize_ref(r: &str) -> String {
    r.trim().trim_start_matches('#').to_string()
}

fn normalize_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl ClosedThings {
    fn new(topics: &[Topic], closed_issue_ids: &[String]) -> Self {
        let mut refs: HashSet<String> = closed_issue_ids.iter().map(|r| normalize_ref(r)).collect();
        let mut topic_texts = HashSet::new();
        for t in topics.iter().filter(|t| t.status == TopicStatus::Closed) {
            refs.insert(normalize_ref(&t.id));
            topic_texts.insert(normalize_text(&t.text));
        }
        Self { refs, topic_texts }
    }

    /// An item is done when it relates to a closed topic or issue, or is a topic whose text
    /// is that of a closed topic.
    fn is_closed(&self, item: &crate::draft::Item) -> bool {
        if let Some(r) = &item.relates_to {
            if self.refs.contains(&normalize_ref(r)) {
                return true;
            }
        }
        item.item_type == ItemType::Topic && self.topic_texts.contains(&normalize_text(&item.text))
    }
}

struct Earlier {
    date: String,
    dir_name: String,
    draft: Draft,
    /// Every folder that holds this meeting: the repository's and the application's own copy.
    folders: Vec<PathBuf>,
}

fn read_earlier(req: &ContextRequest<'_>, warnings: &mut Vec<String>) -> Vec<Earlier> {
    let mut found: Vec<Earlier> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for dir in req.previous_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        names.sort();
        for folder in names {
            let file = folder.join("meeting.json");
            if !file.is_file() {
                continue;
            }
            let dir_name = folder
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            match read_meeting(&file) {
                Ok(draft) => {
                    if req.exclude_meeting_id == Some(draft.meeting_id.as_str())
                        || draft.date.as_str() > req.meeting_date
                    {
                        continue;
                    }
                    // A meeting in two places is used once, but both folders are read for its
                    // corrections: the repository's copy lacks the ones not yet published.
                    if let Some(&at) = index.get(&draft.meeting_id) {
                        found[at].folders.push(folder.clone());
                        continue;
                    }
                    index.insert(draft.meeting_id.clone(), found.len());
                    found.push(Earlier {
                        date: draft.date.clone(),
                        dir_name,
                        draft,
                        folders: vec![folder.clone()],
                    });
                }
                Err(message) => warnings.push(format!("skipped {}: {message}", file.display())),
            }
        }
    }
    // Newest first; the folder name breaks ties between meetings of one day (-2 after the
    // first).
    found.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.dir_name.cmp(&a.dir_name))
    });
    found
}

fn read_meeting(file: &Path) -> std::result::Result<Draft, String> {
    let text = fs::read_to_string(file).map_err(|e| format!("cannot read: {e}"))?;
    Draft::from_json_str(&text).map_err(|issues| {
        let first: Vec<String> = issues
            .iter()
            .take(3)
            .map(ValidationIssue::to_string)
            .collect();
        format!("invalid meeting.json: {}", first.join("; "))
    })
}

fn collect_previous_items(
    req: &ContextRequest<'_>,
    earlier_meetings: &[Earlier],
    closed: &ClosedThings,
) -> Vec<Value> {
    let mut out = Vec::new();
    for earlier in earlier_meetings {
        for item in earlier.draft.items.iter().filter(|i| !closed.is_closed(i)) {
            let link = req
                .issue_links
                .iter()
                .find(|l| l.meeting == earlier.draft.meeting_id && l.item == item.id);
            // The issue of the item was closed (by a person or by an approved status
            // change): the item is done.
            if link.is_some_and(|l| !l.open) {
                continue;
            }
            if out.len() >= req.max_previous_items {
                return out;
            }
            let mut value = json!({
                "meeting": earlier.draft.date,
                "type": item.item_type,
                "text": item.text,
                "assignee": item.assignee,
                "due": item.due,
            });
            if let Some(l) = link {
                value["issue"] = json!(format!("#{}", l.number));
            }
            out.push(value);
        }
    }
    out
}

/// The corrections written after the earlier meetings were published, newest first: the ones
/// published as `corrections*.md` in a folder of the meeting and the local
/// `corrections.json` (a note not yet published is just as true), one entry per correction id.
/// A published file that names another meeting or cycle than the folder it is in is skipped.
fn collect_corrections(
    req: &ContextRequest<'_>,
    earlier_meetings: &[Earlier],
    warnings: &mut Vec<String>,
) -> Vec<Value> {
    struct Entry {
        meeting_id: String,
        meeting: String,
        date: String,
        id: String,
        text: String,
        item: Option<String>,
        issue: Option<String>,
    }
    let mut entries: Vec<Entry> = Vec::new();
    for earlier in earlier_meetings {
        let meeting_id = &earlier.draft.meeting_id;
        let mut seen: HashSet<String> = HashSet::new();
        let mut push = |entry: Entry| {
            if seen.insert(entry.id.clone()) {
                entries.push(entry);
            }
        };
        for folder in &earlier.folders {
            for p in corrections::read_published(folder) {
                if p.marker.meeting != *meeting_id || p.marker.cycle != req.cycle.id {
                    warnings.push(format!(
                        "skipped a correction in {}: it belongs to another meeting",
                        folder.display()
                    ));
                    continue;
                }
                push(Entry {
                    meeting_id: meeting_id.clone(),
                    meeting: earlier.draft.date.clone(),
                    date: p.marker.date.clone().unwrap_or_default(),
                    id: p.marker.id,
                    text: p.text,
                    item: p.marker.about,
                    issue: p.marker.issue.map(|n| format!("#{n}")),
                });
            }
            let file = folder.join(corrections::CORRECTIONS_FILE);
            match CorrectionsFile::read(&file) {
                Ok(Some(f)) if f.meeting_id == *meeting_id => {
                    for c in f.corrections {
                        push(Entry {
                            meeting_id: meeting_id.clone(),
                            meeting: earlier.draft.date.clone(),
                            date: c.date,
                            id: c.id,
                            text: c.text,
                            item: c.item,
                            issue: c.issue,
                        });
                    }
                }
                Ok(_) => {}
                Err(e) => warnings.push(format!("skipped {}: {e}", file.display())),
            }
        }
    }
    // Newest first: by the day written, then by meeting (newer first) and id.
    entries.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.meeting.cmp(&a.meeting))
            .then_with(|| b.id.cmp(&a.id))
    });
    entries.truncate(req.max_corrections);
    entries
        .into_iter()
        .map(|e| {
            // The text of the item that is corrected, so the model need not match ids.
            let item_text = e.item.as_deref().and_then(|id| {
                earlier_meetings
                    .iter()
                    .find(|m| m.draft.meeting_id == e.meeting_id)
                    .and_then(|m| m.draft.items.iter().find(|i| i.id == id))
                    .map(|i| i.text.clone())
            });
            // An item's own issue, when the note names only the item.
            let issue = e.issue.or_else(|| {
                let id = e.item.as_deref()?;
                req.issue_links
                    .iter()
                    .find(|l| l.meeting == e.meeting_id && l.item == id)
                    .map(|l| format!("#{}", l.number))
            });
            json!({
                "meeting": e.meeting,
                "date": e.date,
                "text": e.text,
                "item": item_text,
                "issue": issue,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle::{GlossaryEntry, Participant};
    use crate::draft::fixtures::{draft, item};
    use crate::template::parse_template;

    fn cycle() -> Cycle {
        let mut c = Cycle::new("NOMAD", "Nomad rover");
        c.participants.push(Participant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: None,
        });
        c.glossary = vec![
            GlossaryEntry {
                term: "CAN bus".into(),
                hint: None,
            },
            GlossaryEntry {
                term: "GNSS".into(),
                hint: Some("positioning module".into()),
            },
        ];
        c.report.instruction = "Keep it short.".into();
        c
    }

    fn write_meeting(
        root: &Path,
        folder: &str,
        meeting_id: &str,
        date: &str,
        items: Vec<crate::draft::Item>,
    ) {
        let mut d = draft(items);
        d.meeting_id = meeting_id.into();
        d.date = date.into();
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("meeting.json"),
            serde_json::to_string_pretty(&d.to_value()).unwrap(),
        )
        .unwrap();
    }

    fn previous(ctx: &Context) -> Vec<(String, String)> {
        ctx.value["previous_items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["meeting"].as_str().unwrap().to_string(),
                    i["text"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn fixed_mode_context_validates_against_the_schema() {
        let c = cycle();
        let topics = vec![
            Topic {
                id: "t-1".into(),
                text: "Wheel size".into(),
                status: TopicStatus::Open,
                opened_in: "2026-11-06".into(),
                closed_in: None,
            },
            Topic {
                id: "t-2".into(),
                text: "Battery".into(),
                status: TopicStatus::Closed,
                opened_in: "2026-11-06".into(),
                closed_in: Some("2026-11-13".into()),
            },
        ];
        let issues = vec![
            OpenIssue {
                number: 12,
                title: "Order the battery".into(),
                assignee: Some("alex-example".into()),
                url: None,
            },
            OpenIssue {
                number: 13,
                title: "Wiring".into(),
                assignee: None,
                url: None,
            },
        ];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.topics = &topics;
        req.issues = &issues;
        let ctx = build_context(&req).unwrap();
        let v = &ctx.value;
        assert!(schema::validate(Schema::Context, v).is_empty());
        assert_eq!(v["cycle"], "NOMAD");
        assert_eq!(v["mode"], "fixed");
        assert_eq!(v["output_language"], "auto");
        assert_eq!(v["instruction"], "Keep it short.");
        assert_eq!(
            v["participants"][0],
            json!({"id": "alex", "name": "Alex Example"})
        );
        assert_eq!(v["glossary"][0], json!({"term": "CAN bus"}));
        assert_eq!(
            v["glossary"][1],
            json!({"term": "GNSS", "hint": "positioning module"})
        );
        assert_eq!(v["sections"], json!([]));
        assert_eq!(
            v["open_topics"],
            json!([{"id": "t-1", "text": "Wheel size"}])
        );
        assert_eq!(
            v["open_issues"][0],
            json!({"id": "#12", "title": "Order the battery", "assignee": "alex-example"})
        );
        assert_eq!(v["open_issues"][1]["assignee"], Value::Null);
        assert_eq!(v["previous_items"], json!([]));
        assert_eq!(v["template"], Value::Null);
        assert!(ctx.warnings.is_empty());
    }

    #[test]
    fn template_mode_carries_the_template_sections() {
        let mut c = cycle();
        c.report.default_mode = ReportMode::Template;
        c.report.default_template = "review".into();
        let t = parse_template("review", "---\nsections:\n  - { id: progress, title: Progress, description: What was done }\n  - { id: blockers, title: Blockers }\n---\n{sections}\n").unwrap();
        let templates = vec![t];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.templates = &templates;
        let ctx = build_context(&req).unwrap();
        assert_eq!(ctx.value["mode"], "template");
        assert_eq!(ctx.value["template"], "review");
        assert_eq!(
            ctx.value["sections"][0],
            json!({"id": "progress", "title": "Progress", "description": "What was done"})
        );
        assert_eq!(ctx.value["sections"][1]["description"], "");

        // Missing template for a template mode is an error.
        req.templates = &[];
        assert!(build_context(&req).is_err());

        // Auto mode: no sections even when the template has them; language override.
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.templates = &templates;
        req.mode = Some(ReportMode::Auto);
        req.output_language = Some(LanguageSetting::Pl);
        let ctx = build_context(&req).unwrap();
        assert_eq!(ctx.value["sections"], json!([]));
        assert_eq!(ctx.value["output_language"], "pl");
        assert!(schema::validate(Schema::Context, &ctx.value).is_empty());
    }

    #[test]
    fn by_type_mode_picks_the_template_of_the_meeting_type() {
        let mut c = cycle();
        c.report.default_mode = ReportMode::ByType;
        c.report
            .by_type
            .insert("budget".into(), "budget-review".into());
        let templates = vec![
            parse_template("default", "{sections}").unwrap(),
            parse_template(
                "budget-review",
                "---\nsections:\n  - { id: costs, title: Costs }\n---\n{sections}",
            )
            .unwrap(),
        ];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.templates = &templates;
        req.meeting_type = Some("budget");
        let ctx = build_context(&req).unwrap();
        assert_eq!(ctx.value["template"], "budget-review");
        assert_eq!(ctx.value["meeting_type"], "budget");
        assert_eq!(ctx.value["sections"][0]["id"], "costs");
        assert!(ctx.warnings.is_empty());

        req.meeting_type = Some("casual");
        let ctx = build_context(&req).unwrap();
        assert_eq!(ctx.value["template"], "default");
        assert_eq!(ctx.warnings.len(), 1);
        req.meeting_type = None;
        assert_eq!(build_context(&req).unwrap().warnings.len(), 1);
    }

    #[test]
    fn rejects_a_bad_meeting_date() {
        let c = cycle();
        assert!(build_context(&ContextRequest::new(&c, "13.11.2026")).is_err());
    }

    #[test]
    fn previous_items_are_newest_first_deduped_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("meetings").join("NOMAD");
        write_meeting(
            &root,
            "2026-11-06",
            "m1",
            "2026-11-06",
            vec![
                item("a", ItemType::Task, "old task"),
                item("b", ItemType::Note, "old note"),
            ],
        );
        write_meeting(
            &root,
            "2026-11-13",
            "m2",
            "2026-11-13",
            vec![item("c", ItemType::Task, "first of day")],
        );
        write_meeting(
            &root,
            "2026-11-13-2",
            "m3",
            "2026-11-13",
            vec![item("d", ItemType::Decision, "second of day")],
        );
        // Same meeting id in another directory: counted once.
        let copy = dir.path().join("copy");
        write_meeting(
            &copy,
            "2026-11-13",
            "m2",
            "2026-11-13",
            vec![item("c", ItemType::Task, "first of day")],
        );
        // A meeting after the one being prepared is ignored.
        write_meeting(
            &root,
            "2026-12-01",
            "m9",
            "2026-12-01",
            vec![item("z", ItemType::Task, "future")],
        );

        let c = cycle();
        let dirs = vec![root.clone(), copy.clone()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        let ctx = build_context(&req).unwrap();
        assert!(schema::validate(Schema::Context, &ctx.value).is_empty());
        assert_eq!(
            previous(&ctx),
            vec![
                ("2026-11-13".to_string(), "second of day".to_string()),
                ("2026-11-13".to_string(), "first of day".to_string()),
                ("2026-11-06".to_string(), "old task".to_string()),
                ("2026-11-06".to_string(), "old note".to_string()),
            ]
        );
        assert!(ctx.warnings.is_empty(), "{:?}", ctx.warnings);

        // Cap: the newest items win.
        req.max_previous_items = 3;
        assert_eq!(previous(&build_context(&req).unwrap()).len(), 3);
        assert_eq!(previous(&build_context(&req).unwrap())[2].1, "old task");
        req.max_previous_items = 0;
        assert!(previous(&build_context(&req).unwrap()).is_empty());

        // The meeting being processed can be excluded.
        req.max_previous_items = DEFAULT_MAX_PREVIOUS_ITEMS;
        req.exclude_meeting_id = Some("m3");
        assert_eq!(previous(&build_context(&req).unwrap())[0].1, "first of day");
    }

    #[test]
    fn default_cap_is_eighty_items() {
        let dir = tempfile::tempdir().unwrap();
        let items: Vec<_> = (0..100)
            .map(|n| item(&format!("i{n}"), ItemType::Note, &format!("note {n}")))
            .collect();
        write_meeting(dir.path(), "2026-11-13", "m1", "2026-11-13", items);
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        let ctx = build_context(&req).unwrap();
        assert_eq!(previous(&ctx).len(), 80);
        assert_eq!(previous(&ctx)[0].1, "note 0");
    }

    #[test]
    fn done_and_closed_things_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut linked_topic = item("a", ItemType::Task, "follow up on battery");
        linked_topic.relates_to = Some("t-2".into());
        let mut linked_issue = item("b", ItemType::Task, "send the invoice");
        linked_issue.relates_to = Some("#40".into());
        let mut open_issue = item("c", ItemType::Task, "still open");
        open_issue.relates_to = Some("#41".into());
        let same_text = item("d", ItemType::Topic, "  Battery   supplier ");
        let note_same_text = item("e", ItemType::Note, "battery supplier");
        write_meeting(
            dir.path(),
            "2026-11-13",
            "m1",
            "2026-11-13",
            vec![
                linked_topic,
                linked_issue,
                open_issue,
                same_text,
                note_same_text,
            ],
        );

        let topics = vec![
            Topic {
                id: "t-2".into(),
                text: "Battery supplier".into(),
                status: TopicStatus::Closed,
                opened_in: "2026-11-06".into(),
                closed_in: Some("2026-11-13".into()),
            },
            Topic {
                id: "t-3".into(),
                text: "Wheels".into(),
                status: TopicStatus::Open,
                opened_in: "2026-11-06".into(),
                closed_in: None,
            },
        ];
        let closed_issues = vec!["40".to_string()];
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        req.topics = &topics;
        req.closed_issue_ids = &closed_issues;
        let texts: Vec<_> = previous(&build_context(&req).unwrap())
            .into_iter()
            .map(|p| p.1)
            .collect();
        assert_eq!(texts, vec!["still open", "battery supplier"]);
    }

    #[test]
    fn items_with_a_closed_issue_are_done_and_those_with_an_open_issue_carry_it() {
        let dir = tempfile::tempdir().unwrap();
        write_meeting(
            dir.path(),
            "2026-11-13",
            "20261113-140501",
            "2026-11-13",
            vec![
                item("i1", ItemType::Task, "order the battery"),
                item("i2", ItemType::Task, "send the invoice"),
                item("i3", ItemType::Task, "no issue for this one"),
            ],
        );
        // An issue of another meeting with the same item id must not match.
        let links = vec![
            IssueLink {
                number: 41,
                meeting: "20261113-140501".into(),
                item: "i1".into(),
                open: true,
            },
            IssueLink {
                number: 42,
                meeting: "20261113-140501".into(),
                item: "i2".into(),
                open: false,
            },
            IssueLink {
                number: 43,
                meeting: "20260101-000000".into(),
                item: "i3".into(),
                open: false,
            },
        ];
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        req.issue_links = &links;
        let ctx = build_context(&req).unwrap();
        let items = ctx.value["previous_items"].as_array().unwrap();
        let texts: Vec<_> = items.iter().map(|i| i["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["order the battery", "no issue for this one"]);
        assert_eq!(items[0]["issue"], "#41");
        assert!(items[1].get("issue").is_none());

        // Without links nothing changes: all three are still open business.
        req.issue_links = &[];
        let ctx = build_context(&req).unwrap();
        assert_eq!(ctx.value["previous_items"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn invalid_meeting_files_are_skipped_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        write_meeting(
            dir.path(),
            "2026-11-13",
            "m1",
            "2026-11-13",
            vec![item("a", ItemType::Task, "good")],
        );
        let bad = dir.path().join("2026-11-06");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("meeting.json"), "{ not json").unwrap();
        let wrong = dir.path().join("2026-11-07");
        fs::create_dir_all(&wrong).unwrap();
        fs::write(wrong.join("meeting.json"), "{\"version\": 1}").unwrap();
        fs::create_dir_all(dir.path().join("2026-11-08")).unwrap(); // no meeting.json: ignored silently
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf(), dir.path().join("does-not-exist")];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        let ctx = build_context(&req).unwrap();
        assert_eq!(
            previous(&ctx),
            vec![("2026-11-13".to_string(), "good".to_string())]
        );
        assert_eq!(ctx.warnings.len(), 2, "{:?}", ctx.warnings);
        assert!(ctx.warnings.iter().all(|w| w.starts_with("skipped ")));
    }

    fn correction(id: &str, date: &str, text: &str, item: Option<&str>) -> corrections::Correction {
        corrections::Correction {
            id: id.into(),
            date: date.into(),
            text: text.into(),
            item: item.map(String::from),
            issue: None,
        }
    }

    fn write_published_correction(folder: &Path, name: &str, c: &corrections::Correction) {
        let md = corrections::render_markdown("NOMAD", "20261113-140501", "2026-11-13", c).unwrap();
        fs::write(folder.join(name), md).unwrap();
    }

    #[test]
    fn corrections_of_earlier_meetings_reach_the_context() {
        let dir = tempfile::tempdir().unwrap();
        write_meeting(
            dir.path(),
            "2026-11-13",
            "20261113-140501",
            "2026-11-13",
            vec![item("i1", ItemType::Task, "order the battery")],
        );
        let folder = dir.path().join("2026-11-13");
        write_published_correction(
            &folder,
            "corrections.md",
            &correction(
                "c1",
                "2026-11-16",
                "The battery was ordered on Friday.",
                Some("i1"),
            ),
        );
        write_published_correction(
            &folder,
            "corrections-2.md",
            &correction("c2", "2026-11-18", "The supplier is another one.", None),
        );
        // A copy that names another meeting is not trusted.
        let other = corrections::render_markdown(
            "NOMAD",
            "20260101-000000",
            "2026-01-01",
            &correction("c1", "2026-11-19", "Not for this meeting.", None),
        )
        .unwrap();
        fs::write(folder.join("corrections-3.md"), other).unwrap();
        let links = vec![IssueLink {
            number: 41,
            meeting: "20261113-140501".into(),
            item: "i1".into(),
            open: true,
        }];
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        req.issue_links = &links;
        let ctx = build_context(&req).unwrap();
        let list = ctx.value["corrections"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        // Newest first; the item is given as text, and its issue is found through the link.
        assert_eq!(list[0]["text"], "The supplier is another one.");
        assert!(list[0]["item"].is_null());
        assert_eq!(list[1]["meeting"], "2026-11-13");
        assert_eq!(list[1]["date"], "2026-11-16");
        assert_eq!(list[1]["item"], "order the battery");
        assert_eq!(list[1]["issue"], "#41");
        assert!(ctx.warnings.iter().any(|w| w.contains("another meeting")));
    }

    #[test]
    fn a_local_correction_counts_and_the_same_id_is_used_once() {
        // The repository's folder and the application's own copy of one meeting.
        let repo = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        for root in [repo.path(), local.path()] {
            write_meeting(
                root,
                "2026-11-13",
                "20261113-140501",
                "2026-11-13",
                vec![item("i1", ItemType::Task, "order the battery")],
            );
        }
        write_published_correction(
            &repo.path().join("2026-11-13"),
            "corrections.md",
            &correction("c1", "2026-11-16", "Published.", None),
        );
        let mut file = corrections::CorrectionsFile::new("20261113-140501");
        file.corrections
            .push(correction("c1", "2026-11-16", "Published.", None));
        file.corrections
            .push(correction("c2", "2026-11-17", "Not published yet.", None));
        file.write(
            &local
                .path()
                .join("2026-11-13")
                .join(corrections::CORRECTIONS_FILE),
        )
        .unwrap();
        let c = cycle();
        let dirs = vec![repo.path().to_path_buf(), local.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        let ctx = build_context(&req).unwrap();
        let texts: Vec<_> = ctx.value["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["Not published yet.", "Published."]);
        // The meeting itself is still listed once.
        assert_eq!(ctx.value["previous_items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn without_corrections_the_context_has_no_such_key_and_the_cap_holds() {
        let dir = tempfile::tempdir().unwrap();
        write_meeting(
            dir.path(),
            "2026-11-13",
            "20261113-140501",
            "2026-11-13",
            vec![item("i1", ItemType::Task, "order the battery")],
        );
        let c = cycle();
        let dirs = vec![dir.path().to_path_buf()];
        let mut req = ContextRequest::new(&c, "2026-11-20");
        req.previous_dirs = &dirs;
        assert!(build_context(&req)
            .unwrap()
            .value
            .get("corrections")
            .is_none());

        let mut file = corrections::CorrectionsFile::new("20261113-140501");
        for n in 1..=5 {
            file.corrections.push(correction(
                &format!("c{n}"),
                "2026-11-16",
                &format!("note {n}"),
                None,
            ));
        }
        file.write(
            &dir.path()
                .join("2026-11-13")
                .join(corrections::CORRECTIONS_FILE),
        )
        .unwrap();
        req.max_corrections = 3;
        let ctx = build_context(&req).unwrap();
        let notes = ctx.value["corrections"].as_array().unwrap();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0]["text"], "note 5");
    }
}
