//! The task exporter interface shared by the task targets (Notion, GitHub issues).
//!
//! A task of a meeting becomes one external item (a Notion page, a GitHub issue). Three
//! things are the same for every target and live here:
//!
//! * [`TaskKey`], the key that ties an external item to the item of the meeting it was made
//!   from (`bbb:v1:<cycle>:<meeting>:<item>`),
//! * [`TaskRecord`], the task as every target sees it, built once from a draft item,
//! * [`upsert`], the idempotent algorithm: a stored id is fetched, otherwise the key is
//!   looked up, otherwise the item is created and its id is handed to the caller at once. A
//!   lost answer or a crash therefore never leads to a second item.
//!
//! Nothing in this module talks to a network on its own; a target implements
//! [`TaskExporter`] over its client.

use cycle_store::{Cycle, Draft, Item};

use crate::http::{ErrorKind, HttpError};

/// The prefix of the text form of a [`TaskKey`].
const KEY_PREFIX: &str = "bbb:v1:";
/// Longest id of a cycle, a meeting or an item inside a key.
const MAX_PART_CHARS: usize = 100;

/// The key that links an external task to its item. Text form:
/// `bbb:v1:<cycle>:<meeting>:<item>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskKey {
    pub cycle: String,
    pub meeting: String,
    pub item: String,
}

fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part.chars().count() <= MAX_PART_CHARS
        && !part
            .chars()
            .any(|c| c == ':' || c.is_whitespace() || c.is_control())
}

impl TaskKey {
    /// A key from three ids; `None` when one of them cannot be part of the text form (empty,
    /// too long, or containing `:`, white space or a control character).
    pub fn new(
        cycle: impl Into<String>,
        meeting: impl Into<String>,
        item: impl Into<String>,
    ) -> Option<TaskKey> {
        let key = TaskKey {
            cycle: cycle.into(),
            meeting: meeting.into(),
            item: item.into(),
        };
        key.is_valid().then_some(key)
    }

    /// Whether the text form is unambiguous: every id is usable.
    pub fn is_valid(&self) -> bool {
        valid_part(&self.cycle) && valid_part(&self.meeting) && valid_part(&self.item)
    }

    /// `bbb:v1:<cycle>:<meeting>:<item>`.
    pub fn text(&self) -> String {
        format!("{KEY_PREFIX}{}:{}:{}", self.cycle, self.meeting, self.item)
    }

    /// The key a text form stands for; `None` for anything else.
    pub fn parse(text: &str) -> Option<TaskKey> {
        let rest = text.trim().strip_prefix(KEY_PREFIX)?;
        let mut parts = rest.split(':');
        let (cycle, meeting, item) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        TaskKey::new(cycle, meeting, item)
    }
}

/// The person a task is assigned to, as the cycle knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignee {
    pub participant_id: String,
    /// The display name.
    pub name: String,
    pub github: Option<String>,
}

/// A task as every task target sees it, built once from a draft item (pure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub key: TaskKey,
    pub title: String,
    pub assignee: Option<Assignee>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    /// `YYYY-MM-DD`, the day of the meeting.
    pub meeting_date: String,
    pub cycle_name: String,
    /// Address of the published report; known only after the commit, set by the caller.
    pub report_url: Option<String>,
    /// Address of the published `transcript.md`, set by the caller.
    pub transcript_url: Option<String>,
    /// `t-hh-mm-ss`, the anchor of the passage in the transcript, set by the caller.
    pub transcript_anchor: Option<String>,
    /// Only a quote that was found in the transcript.
    pub quote: Option<String>,
    /// Where in the recording the item was said.
    pub time_ms: Option<u64>,
}

impl TaskRecord {
    /// The record of `item`, an item of `draft` (the meeting `meeting_id` of `cycle`). The
    /// assignee is resolved through the participants of the cycle (an id that is not a
    /// participant gives none); a quote that was not verified against the transcript is
    /// dropped. The addresses are left empty.
    pub fn from_item(cycle: &Cycle, draft: &Draft, item: &Item, meeting_id: &str) -> TaskRecord {
        let assignee = item
            .assignee
            .as_deref()
            .and_then(|id| cycle.participant(id))
            .map(|p| Assignee {
                participant_id: p.id.clone(),
                name: p.name.clone(),
                github: p.github.clone(),
            });
        let source = item.source.as_ref();
        TaskRecord {
            key: TaskKey {
                cycle: cycle.id.clone(),
                meeting: meeting_id.to_string(),
                item: item.id.clone(),
            },
            title: item.text.clone(),
            assignee,
            due: item.due.clone(),
            meeting_date: draft.date.clone(),
            cycle_name: cycle.name.clone(),
            report_url: None,
            transcript_url: None,
            transcript_anchor: None,
            quote: source.filter(|s| s.verified).map(|s| s.quote.clone()),
            time_ms: source.map(|s| s.t_start_ms),
        }
    }
}

/// An item in an external service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTask {
    /// The id the service gives it (a Notion page id, a GitHub issue number).
    pub id: String,
    pub url: Option<String>,
    /// Trashed (Notion) by a person: it is not brought back.
    pub trashed: bool,
}

/// What [`upsert`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpsertOutcome {
    Created(ExternalTask),
    Updated(ExternalTask),
    Unchanged(ExternalTask),
    /// The item was trashed by a person: it is not recreated.
    Removed {
        id: String,
    },
}

impl UpsertOutcome {
    /// The external id the outcome is about.
    pub fn id(&self) -> &str {
        match self {
            Self::Created(t) | Self::Updated(t) | Self::Unchanged(t) => &t.id,
            Self::Removed { id } => id,
        }
    }

    /// The item, unless it was removed.
    pub fn task(&self) -> Option<&ExternalTask> {
        match self {
            Self::Created(t) | Self::Updated(t) | Self::Unchanged(t) => Some(t),
            Self::Removed { .. } => None,
        }
    }
}

/// The result of [`upsert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upserted {
    pub outcome: UpsertOutcome,
    /// More than one item carried the key: how many besides the one that was used. Never
    /// deleted, only reported.
    pub duplicates: usize,
    /// The item was found by its key (no id was known).
    pub found_by_key: bool,
}

/// A target that holds one external item per task.
pub trait TaskExporter {
    /// `notion`, `github`.
    fn name(&self) -> &'static str;

    /// What an item is called in error texts: `page`, `issue`.
    fn noun(&self) -> &'static str {
        "task"
    }

    /// Whether an existing item is rewritten when `update_existing` is true.
    fn updates_existing(&self) -> bool {
        true
    }

    /// The item with this id; `None` when it is gone (404) or no longer shared.
    fn get(&self, id: &str) -> Result<Option<ExternalTask>, HttpError>;

    /// The items that carry the key, oldest first.
    fn find(&self, key: &TaskKey) -> Result<Vec<ExternalTask>, HttpError>;

    fn create(&self, task: &TaskRecord) -> Result<ExternalTask, HttpError>;

    fn update(&self, id: &str, task: &TaskRecord) -> Result<ExternalTask, HttpError>;
}

/// An error that needs the user.
pub fn permanent_error(kind: ErrorKind, message: impl Into<String>) -> HttpError {
    HttpError {
        kind,
        message: message.into(),
        status: None,
        failure: store::Failure::Permanent,
    }
}

/// The idempotent export of one task.
///
/// 1. `known` (the id stored by an earlier attempt): fetched. A live item is updated (or
///    left alone when `update_existing` is false), a trashed one gives
///    [`UpsertOutcome::Removed`], a missing one is a permanent error ("deleted or no longer
///    shared"): the item is not silently made again.
/// 2. No known id: the key is looked up. The oldest hit is used, `on_stored` is called with
///    it, then as in 1. More than one hit is counted in `duplicates` and never deleted.
/// 3. No hit: the item is created and `on_stored` is called with it before the function
///    returns, so that the caller stores the id at once.
pub fn upsert(
    exporter: &dyn TaskExporter,
    task: &TaskRecord,
    known: Option<&str>,
    update_existing: bool,
    on_stored: &mut dyn FnMut(&ExternalTask),
) -> Result<Upserted, HttpError> {
    if !task.key.is_valid() {
        return Err(permanent_error(
            ErrorKind::InvalidRequest,
            format!(
                "the task cannot be linked to a {}: an id is not usable",
                exporter.noun()
            ),
        ));
    }
    let mut duplicates = 0;
    let mut found_by_key = false;
    let existing = match known {
        Some(id) => match exporter.get(id)? {
            Some(found) => Some(found),
            None => {
                let mut err = permanent_error(
                    ErrorKind::Status,
                    format!(
                        "the {} was deleted or is no longer shared with the integration",
                        exporter.noun()
                    ),
                );
                err.status = Some(404);
                return Err(err);
            }
        },
        None => {
            let mut hits = exporter.find(&task.key)?.into_iter();
            let oldest = hits.next();
            duplicates = hits.count();
            if let Some(found) = &oldest {
                found_by_key = true;
                on_stored(found);
            }
            oldest
        }
    };
    let outcome = match existing {
        Some(found) if found.trashed => UpsertOutcome::Removed { id: found.id },
        Some(found) if update_existing && exporter.updates_existing() => {
            UpsertOutcome::Updated(exporter.update(&found.id, task)?)
        }
        Some(found) => UpsertOutcome::Unchanged(found),
        None => {
            let created = exporter.create(task)?;
            on_stored(&created);
            UpsertOutcome::Created(created)
        }
    };
    Ok(Upserted {
        outcome,
        duplicates,
        found_by_key,
    })
}

#[cfg(test)]
mod tests;
