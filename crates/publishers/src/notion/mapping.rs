//! The field mapping of the Notion export: which property of the data source each field of a
//! task goes to, and the JSON a property takes. Pure: no network, no clock.
//!
//! * [`property_values`] builds the `properties` object of a page from a task.
//! * [`check`] says what is wrong with a mapping against the data source's [`Schema`].
//! * [`propose`] guesses a mapping from the property names and types.
//!
//! Limits of Notion that matter here: a text is at most 2000 characters (cut, not refused);
//! the name of a select option cannot contain a comma and is at most 100 characters; a
//! `status` option cannot be created by a page write; `people` needs Notion user ids, which
//! this version does not have, so an assignee is written by display name into a text or
//! select property only.

use std::collections::BTreeMap;

use cycle_store::{NotionProperties, DEFAULT_NOTION_KEY, DEFAULT_NOTION_TITLE};
use serde_json::{json, Map, Value};

use crate::export::TaskRecord;

/// Most characters of one text. Notion counts UTF-16 code units at worst, so the cut is made
/// there: the result is within the limit by either count.
pub const MAX_TEXT_UNITS: usize = 2000;
/// Most characters of the name of a select option.
pub const MAX_OPTION_CHARS: usize = 100;

/// The type of a property of a data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyType {
    Title,
    RichText,
    Select,
    MultiSelect,
    Date,
    Status,
    Url,
    People,
    Number,
    Checkbox,
    /// Any other type, by its name in the API.
    Other(String),
}

impl PropertyType {
    pub fn from_api(name: &str) -> PropertyType {
        match name {
            "title" => Self::Title,
            "rich_text" => Self::RichText,
            "select" => Self::Select,
            "multi_select" => Self::MultiSelect,
            "date" => Self::Date,
            "status" => Self::Status,
            "url" => Self::Url,
            "people" => Self::People,
            "number" => Self::Number,
            "checkbox" => Self::Checkbox,
            other => Self::Other(other.to_string()),
        }
    }

    /// The name of the type in the API.
    pub fn api_name(&self) -> &str {
        match self {
            Self::Title => "title",
            Self::RichText => "rich_text",
            Self::Select => "select",
            Self::MultiSelect => "multi_select",
            Self::Date => "date",
            Self::Status => "status",
            Self::Url => "url",
            Self::People => "people",
            Self::Number => "number",
            Self::Checkbox => "checkbox",
            Self::Other(name) => name,
        }
    }
}

/// The properties of a data source by name.
pub type Schema = BTreeMap<String, PropertyType>;
/// The option names of the select, multi-select and status properties, by property name.
pub type Options = BTreeMap<String, Vec<String>>;

/// What `GET /data_sources/{id}` says about the properties.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataSourceSchema {
    pub properties: Schema,
    pub options: Options,
}

impl DataSourceSchema {
    /// Reads the `properties` object of a data source.
    pub fn from_properties(properties: &Value) -> DataSourceSchema {
        let mut out = DataSourceSchema::default();
        let Some(map) = properties.as_object() else {
            return out;
        };
        for (name, property) in map {
            let Some(kind) = property.get("type").and_then(Value::as_str) else {
                continue;
            };
            out.properties
                .insert(name.clone(), PropertyType::from_api(kind));
            if let Some(options) = property
                .get(kind)
                .and_then(|v| v.get("options"))
                .and_then(Value::as_array)
            {
                let names = options
                    .iter()
                    .filter_map(|o| o.get("name").and_then(Value::as_str).map(String::from))
                    .collect();
                out.options.insert(name.clone(), names);
            }
        }
        out
    }
}

// ----- the fields ------------------------------------------------------------------------

/// A field of a task that goes to a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Key,
    Assignee,
    Due,
    Status,
    Meeting,
    Cycle,
}

impl Field {
    /// The name of the key in `targets.notion.properties`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Key => "key",
            Self::Assignee => "assignee",
            Self::Due => "due",
            Self::Status => "status",
            Self::Meeting => "meeting",
            Self::Cycle => "cycle",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Title => "the task title",
            Self::Key => "the key",
            Self::Assignee => "the assignee",
            Self::Due => "the due date",
            Self::Status => "the status",
            Self::Meeting => "the meeting link",
            Self::Cycle => "the project",
        }
    }

    /// The types of property the field can go to.
    fn types(self) -> &'static [&'static str] {
        match self {
            Self::Title => &["title"],
            Self::Key => &["rich_text"],
            Self::Assignee => &["rich_text", "select", "multi_select"],
            Self::Due => &["date"],
            Self::Status => &["status", "select"],
            Self::Meeting => &["url", "rich_text"],
            Self::Cycle => &["select", "rich_text"],
        }
    }

    fn accepts(self, kind: &PropertyType) -> bool {
        self.types().contains(&kind.api_name())
    }
}

/// The fields that have a property named, with the name.
fn configured(map: &NotionProperties) -> Vec<(Field, &str)> {
    let mut out = vec![
        (Field::Title, map.title.as_str()),
        (Field::Key, map.key.as_str()),
    ];
    let optional = [
        (Field::Assignee, &map.assignee),
        (Field::Due, &map.due),
        (Field::Status, &map.status),
        (Field::Meeting, &map.meeting),
        (Field::Cycle, &map.cycle),
    ];
    for (field, name) in optional {
        if let Some(name) = name.as_deref().filter(|n| !n.trim().is_empty()) {
            out.push((field, name));
        }
    }
    out
}

// ----- problems --------------------------------------------------------------------------

/// Something that is wrong with a mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingProblem {
    /// The key in `targets.notion.properties` it is about (`title`, `key`, `assignee`, ...,
    /// `status_value`).
    pub field: &'static str,
    pub message: String,
}

/// The mapping does not fit the data source, so no page can be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join("; "))]
pub struct MappingError {
    pub problems: Vec<MappingProblem>,
}

/// What is wrong with `map` for a data source with this `schema`: a property that is not
/// there, one of a type the field cannot go to, the key or the title on the wrong type, a
/// status without the value a new page gets, one property used for two fields.
pub fn check(map: &NotionProperties, schema: &Schema) -> Vec<MappingProblem> {
    let mut problems = Vec::new();
    let mut used: Vec<(&str, Field)> = Vec::new();
    for (field, name) in configured(map) {
        match schema.get(name) {
            None => problems.push(MappingProblem {
                field: field.name(),
                message: format!(
                    "{}: the property \"{name}\" does not exist in the Notion database",
                    field.label()
                ),
            }),
            Some(kind) if !field.accepts(kind) => problems.push(MappingProblem {
                field: field.name(),
                message: format!(
                    "{}: the property \"{name}\" is of type {}, it must be {}",
                    field.label(),
                    kind.api_name(),
                    field.types().join(" or ")
                ),
            }),
            Some(_) => {}
        }
        if let Some((_, first)) = used.iter().find(|(n, _)| *n == name) {
            problems.push(MappingProblem {
                field: field.name(),
                message: format!(
                    "{}: the property \"{name}\" is used for {} as well",
                    field.label(),
                    first.label()
                ),
            });
        }
        used.push((name, field));
    }
    let status_set = map.status.as_deref().is_some_and(|s| !s.trim().is_empty());
    let value_set = map
        .status_value
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());
    if status_set && !value_set {
        problems.push(MappingProblem {
            field: "status_value",
            message: "the status: a new page needs the status it starts with".to_string(),
        });
    }
    problems
}

// ----- values ----------------------------------------------------------------------------

/// Cuts `text` to at most `max` UTF-16 code units, at a character boundary.
fn cut_units(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name of a select option made from `text`: no comma (Notion refuses it, so it becomes
/// a space), one line, at most [`MAX_OPTION_CHARS`]. `None` when nothing is left.
pub fn option_name(text: &str) -> Option<String> {
    let flat = one_line(&text.replace(',', " "));
    let cut: String = flat.chars().take(MAX_OPTION_CHARS).collect();
    let cut = cut.trim_end().to_string();
    (!cut.is_empty()).then_some(cut)
}

fn text_array(text: &str) -> Value {
    let text = cut_units(text, MAX_TEXT_UNITS);
    if text.is_empty() {
        json!([])
    } else {
        json!([{"text": {"content": text}}])
    }
}

/// The value of a property of type `kind` holding `text`; `None` for a type this module does
/// not write.
fn value_of(kind: &PropertyType, text: &str) -> Option<Value> {
    Some(match kind {
        PropertyType::Title => json!({"title": text_array(&one_line(text))}),
        PropertyType::RichText => json!({"rich_text": text_array(text)}),
        PropertyType::Select => match option_name(text) {
            Some(name) => json!({"select": {"name": name}}),
            None => json!({"select": null}),
        },
        PropertyType::MultiSelect => match option_name(text) {
            Some(name) => json!({"multi_select": [{"name": name}]}),
            None => json!({"multi_select": []}),
        },
        PropertyType::Date => json!({"date": {"start": text}}),
        PropertyType::Status => json!({"status": {"name": text}}),
        PropertyType::Url => json!({"url": text}),
        _ => return None,
    })
}

/// The value that empties a property of type `kind`.
fn empty_of(kind: &PropertyType) -> Option<Value> {
    Some(match kind {
        PropertyType::Title => json!({"title": []}),
        PropertyType::RichText => json!({"rich_text": []}),
        PropertyType::Select => json!({"select": null}),
        PropertyType::MultiSelect => json!({"multi_select": []}),
        PropertyType::Date => json!({"date": null}),
        PropertyType::Url => json!({"url": null}),
        _ => return None,
    })
}

/// What a field holds for `task`; `None` when the task has nothing for it.
fn text_of(
    field: Field,
    kind: &PropertyType,
    map: &NotionProperties,
    task: &TaskRecord,
) -> Option<String> {
    match field {
        Field::Title => Some(task.title.clone()),
        Field::Key => Some(task.key.text()),
        Field::Assignee => task.assignee.as_ref().map(|a| a.name.clone()),
        Field::Due => task.due.clone(),
        Field::Status => map.status_value.clone(),
        Field::Cycle => Some(task.cycle_name.clone()),
        Field::Meeting => match (kind, &task.report_url) {
            (PropertyType::Url, url) => url.clone(),
            (_, Some(url)) => Some(format!("{} {url}", task.meeting_date)),
            (_, None) => Some(task.meeting_date.clone()),
        },
    }
}

/// The `properties` object of a page for `task`.
///
/// `create` is true for a new page. A field the task has nothing for is left out of a new
/// page and emptied on an existing one, so a cleared due date clears the property. The
/// status is written to a new page only: a person may have moved the task in Notion, and
/// exporting again never resets it.
///
/// Fails with every problem [`check`] finds.
pub fn property_values(
    map: &NotionProperties,
    schema: &Schema,
    task: &TaskRecord,
    create: bool,
) -> Result<Map<String, Value>, MappingError> {
    let problems = check(map, schema);
    if !problems.is_empty() {
        return Err(MappingError { problems });
    }
    let mut out = Map::new();
    for (field, name) in configured(map) {
        if field == Field::Status && !create {
            continue;
        }
        let Some(kind) = schema.get(name) else {
            continue;
        };
        let value = match text_of(field, kind, map, task) {
            Some(text) => value_of(kind, &text),
            None if create => continue,
            None => empty_of(kind),
        };
        if let Some(value) = value {
            out.insert(name.to_string(), value);
        }
    }
    Ok(out)
}

// ----- a proposal ------------------------------------------------------------------------

/// The first property, in the order of `names`, whose name matches one of them (ignoring
/// case) and whose type the field accepts, and that is not taken yet.
fn find_property(
    schema: &Schema,
    names: &[&str],
    field: Field,
    taken: &[String],
) -> Option<String> {
    names.iter().find_map(|wanted| {
        schema
            .iter()
            .find(|(name, kind)| {
                name.trim().eq_ignore_ascii_case(wanted)
                    && field.accepts(kind)
                    && !taken.contains(name)
            })
            .map(|(name, _)| name.clone())
    })
}

/// A mapping guessed from the property names and types: the title property, the key (a text
/// property named like "BBB key" or "Key", else the default name), and an assignee, due date,
/// meeting link and cycle when a property of a compatible type has a matching name. The
/// status is left out: its starting value needs the options, see [`propose_with_options`].
pub fn propose(schema: &Schema) -> NotionProperties {
    propose_with_options(schema, &Options::new())
}

/// [`propose`], and a status property when one matches by name and type, starting at its
/// first option (preferring one that reads like "not started").
pub fn propose_with_options(schema: &Schema, options: &Options) -> NotionProperties {
    let title = schema
        .iter()
        .find(|(_, kind)| **kind == PropertyType::Title)
        .map_or_else(
            || DEFAULT_NOTION_TITLE.to_string(),
            |(name, _)| name.clone(),
        );
    let mut taken = vec![title.clone()];
    let key = find_property(
        schema,
        &["bbb key", "bbb_key", "bbbkey", "key"],
        Field::Key,
        &taken,
    )
    .unwrap_or_else(|| DEFAULT_NOTION_KEY.to_string());
    taken.push(key.clone());
    let pick = |names: &[&str], field: Field, taken: &mut Vec<String>| {
        let found = find_property(schema, names, field, taken);
        if let Some(name) = &found {
            taken.push(name.clone());
        }
        found
    };
    let assignee = pick(
        &["assignee", "assigned to", "owner"],
        Field::Assignee,
        &mut taken,
    );
    let due = pick(&["due", "due date", "deadline"], Field::Due, &mut taken);
    let status = pick(&["status", "state"], Field::Status, &mut taken);
    let meeting = pick(
        &["meeting", "meeting link", "report", "report link", "source"],
        Field::Meeting,
        &mut taken,
    );
    let cycle = pick(&["cycle"], Field::Cycle, &mut taken);
    let (status, status_value) = match status {
        Some(name) => match initial_status(options.get(&name)) {
            Some(value) => (Some(name), Some(value)),
            None => (None, None),
        },
        None => (None, None),
    };
    NotionProperties {
        title,
        key,
        assignee,
        due,
        status,
        status_value,
        meeting,
        cycle,
    }
}

fn initial_status(options: Option<&Vec<String>>) -> Option<String> {
    let options = options?;
    let preferred = options.iter().find(|o| {
        let o = o.to_ascii_lowercase();
        ["not started", "to do", "todo", "backlog", "new", "open"]
            .iter()
            .any(|p| o == *p)
    });
    preferred.or_else(|| options.first()).cloned()
}
