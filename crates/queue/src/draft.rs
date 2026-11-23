//! The draft report of a meeting as the application edits it.
//!
//! The worker writes `work/draft.json` (`schemas/meeting.schema.json`) and, next to it,
//! `work/draft.base.sha256`: the lowercase hex SHA-256 of the exact bytes it wrote. The user
//! then changes items and sections in the editor; every change is saved into the same file
//! (touched items get `edited: true`). A draft has user edits when an item is marked edited
//! or when the bytes of `draft.json` no longer match the recorded hash (a deleted item or a
//! renamed section marks nothing, but changes the bytes). The next run of `reduce` then does
//! not overwrite the file: it writes `work/draft.new.json` instead, see
//! [`DraftFiles::accept_new_draft`].
//!
//! The operations are plain functions on a [`cycle_store::Draft`]: they validate, change
//! the draft in memory and fail without changing anything. [`DraftFiles`] loads and saves
//! the file; saving is atomic and refuses a document that does not validate.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use cycle_store::schema::{self, is_identifier};
use cycle_store::time::is_iso_date;
use cycle_store::{
    Certainty, CertaintyLevel, Decision, Draft, Item, ItemType, Language, Participant,
    ProposedStatus, RenderOptions, ReportMode, Schema, Section, StatusProposal, ValidationIssue,
    Variant,
};
use serde_json::Value;

/// Name of the draft written by the worker and edited by the user.
pub const DRAFT_FILE: &str = "draft.json";
/// Name of the draft a re-run of `reduce` writes when the draft has user edits.
pub const NEW_DRAFT_FILE: &str = "draft.new.json";
/// Hash of the bytes of the draft as `reduce` wrote it (lowercase hex SHA-256).
pub const BASE_HASH_FILE: &str = "draft.base.sha256";

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("the draft is not ready for this meeting")]
    NotReady,
    #[error("there is no newer draft to accept")]
    NoNewDraft,
    #[error("no such item: {0}")]
    NoItem(String),
    #[error("no such section: {0}")]
    NoSection(String),
    #[error("no such status change: {0}")]
    NoProposal(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid draft: {0}")]
    Schema(String),
}

fn schema_error(issues: Vec<ValidationIssue>) -> DraftError {
    let first: Vec<String> = issues.iter().take(3).map(|i| i.to_string()).collect();
    DraftError::Schema(first.join("; "))
}

// ----- files -----------------------------------------------------------------------------

/// The draft files of one meeting, under `<meeting dir>/work`.
pub struct DraftFiles {
    work: PathBuf,
}

impl DraftFiles {
    pub fn new(meeting_dir: &Path) -> Self {
        Self {
            work: meeting_dir.join("work"),
        }
    }

    fn path(&self) -> PathBuf {
        self.work.join(DRAFT_FILE)
    }

    fn new_path(&self) -> PathBuf {
        self.work.join(NEW_DRAFT_FILE)
    }

    fn base_path(&self) -> PathBuf {
        self.work.join(BASE_HASH_FILE)
    }

    /// Whether a draft exists.
    pub fn exists(&self) -> bool {
        self.path().is_file()
    }

    /// Whether a re-run of `reduce` left a newer draft next to the edited one.
    pub fn has_new_draft(&self) -> bool {
        self.new_path().is_file()
    }

    /// Loads and validates `draft.json`.
    pub fn load(&self) -> Result<Draft, DraftError> {
        read_draft(&self.path())
    }

    /// Loads and validates `draft.new.json`.
    pub fn load_new(&self) -> Result<Draft, DraftError> {
        match read_draft(&self.new_path()) {
            Err(DraftError::NotReady) => Err(DraftError::NoNewDraft),
            other => other,
        }
    }

    /// Validates and writes `draft.json` through a temporary file and a rename, so a crash
    /// never leaves a truncated draft.
    pub fn save(&self, draft: &Draft) -> Result<(), DraftError> {
        write_draft(&self.path(), draft)
    }

    /// Whether the user changed anything in the draft: an item is marked `edited`, or the
    /// bytes of `draft.json` differ from the hash `reduce` recorded in `draft.base.sha256`.
    /// Without that file only the marks count. `false` when there is no draft.
    pub fn has_user_edits(&self) -> Result<bool, DraftError> {
        let bytes = match fs::read(self.path()) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let text = String::from_utf8_lossy(&bytes);
        let draft = Draft::from_json_str(&text).map_err(schema_error)?;
        if has_edited_items(&draft) {
            return Ok(true);
        }
        Ok(match self.recorded_hash() {
            Some(recorded) => recorded != sha256_hex(&bytes),
            None => false,
        })
    }

    /// The hash `reduce` (or an accepted new draft) recorded, when there is one.
    fn recorded_hash(&self) -> Option<String> {
        let text = fs::read_to_string(self.base_path()).ok()?;
        let hash = text.trim().to_ascii_lowercase();
        (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
    }

    /// Loads the draft, applies `change` and saves the result. Nothing is written when
    /// `change` fails.
    pub fn edit<T>(
        &self,
        change: impl FnOnce(&mut Draft) -> Result<T, DraftError>,
    ) -> Result<T, DraftError> {
        let mut draft = self.load()?;
        let out = change(&mut draft)?;
        self.save(&draft)?;
        Ok(out)
    }

    /// Replaces `draft.json` by `draft.new.json` (the user chose the new extraction over
    /// their edited draft). The new draft is validated first and the old one is replaced
    /// atomically; `draft.new.json` is gone afterwards.
    ///
    /// The new draft counts as untouched afterwards: `draft.base.sha256` gets its hash. The
    /// hash is written first, so a crash in between leaves the old draft protected (it does
    /// not match the new hash).
    pub fn accept_new_draft(&self) -> Result<Draft, DraftError> {
        let draft = self.load_new()?;
        let bytes = fs::read(self.new_path())?;
        write_bytes(&self.base_path(), sha256_hex(&bytes).as_bytes())?;
        fs::rename(self.new_path(), self.path())?;
        Ok(draft)
    }

    /// Throws `draft.new.json` away and keeps the edited draft. The recorded hash stays, so
    /// the kept draft is still known to differ from what `reduce` wrote and a later re-run
    /// does not overwrite it.
    pub fn discard_new_draft(&self) -> Result<bool, DraftError> {
        match fs::remove_file(self.new_path()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn read_draft(path: &Path) -> Result<Draft, DraftError> {
    match fs::read_to_string(path) {
        Ok(text) => Draft::from_json_str(&text).map_err(schema_error),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(DraftError::NotReady),
        Err(e) => Err(e.into()),
    }
}

fn write_draft(path: &Path, draft: &Draft) -> Result<(), DraftError> {
    let value: Value = draft.to_value();
    let issues = schema::validate(Schema::Meeting, &value);
    if !issues.is_empty() {
        return Err(schema_error(issues));
    }
    let bytes = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
    write_bytes(path, &bytes)?;
    Ok(())
}

/// Writes `bytes` through a temporary file with a name of its own (the process, a counter),
/// so two writers never share one, and renames it over `path`.
fn write_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether an item of the draft is marked as edited, or a status change was decided or
/// edited. This is only one of the signs of user edits; see [`DraftFiles::has_user_edits`]
/// for the whole check.
pub fn has_edited_items(draft: &Draft) -> bool {
    draft.items.iter().any(|i| i.edited)
        || draft
            .status_proposals
            .iter()
            .any(|p| p.edited || p.decision != Decision::Proposed)
}

// ----- item operations -------------------------------------------------------------------

/// The changes of one item. `None` leaves a field as it is. `assignee` and `due` are
/// double options: `Some(None)` clears the value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemPatch {
    pub text: Option<String>,
    pub assignee: Option<Option<String>>,
    pub due: Option<Option<String>>,
}

fn find(draft: &Draft, id: &str) -> Result<usize, DraftError> {
    draft
        .items
        .iter()
        .position(|i| i.id == id)
        .ok_or_else(|| DraftError::NoItem(id.to_string()))
}

fn check_section(draft: &Draft, section: Option<&str>) -> Result<(), DraftError> {
    match section {
        Some(id) if !draft.sections.iter().any(|s| s.id == id) => {
            Err(DraftError::NoSection(id.to_string()))
        }
        _ => Ok(()),
    }
}

/// Certainty after the user decided a value: stated when there is one, unknown when the
/// user cleared it.
fn decided(value: &Option<String>) -> CertaintyLevel {
    if value.is_some() {
        CertaintyLevel::Stated
    } else {
        CertaintyLevel::Unknown
    }
}

/// Changes the text, assignee or due date of an item. A value the user sets counts as
/// stated: the certainty of that field becomes `stated` (`unknown` when the user clears
/// it). The item is marked as edited.
pub fn update_item(draft: &mut Draft, id: &str, patch: &ItemPatch) -> Result<(), DraftError> {
    let index = find(draft, id)?;
    if let Some(text) = &patch.text {
        if text.trim().is_empty() {
            return Err(DraftError::Invalid(
                "the text of an item cannot be empty".into(),
            ));
        }
    }
    if let Some(Some(due)) = &patch.due {
        if !is_iso_date(due) {
            return Err(DraftError::Invalid(format!(
                "the due date {due:?} is not YYYY-MM-DD"
            )));
        }
    }
    if let Some(Some(who)) = &patch.assignee {
        if who.trim().is_empty() {
            return Err(DraftError::Invalid(
                "the assignee cannot be an empty id".into(),
            ));
        }
    }
    let item = &mut draft.items[index];
    let mut touched = false;
    if let Some(text) = &patch.text {
        item.text = text.trim().to_string();
        touched = true;
    }
    if let Some(assignee) = &patch.assignee {
        item.assignee = assignee.clone();
        item.certainty.assignee = decided(&item.assignee);
        touched = true;
    }
    if let Some(due) = &patch.due {
        item.due = due.clone();
        item.certainty.due = decided(&item.due);
        touched = true;
    }
    if touched {
        item.edited = true;
    }
    Ok(())
}

/// Moves an item to a section (`None`: no section) and, with `before`, in front of another
/// item of that section; without it the item goes last. The item is marked as edited.
pub fn move_item(
    draft: &mut Draft,
    id: &str,
    section: Option<&str>,
    before: Option<&str>,
) -> Result<(), DraftError> {
    let index = find(draft, id)?;
    check_section(draft, section)?;
    if let Some(before_id) = before {
        if before_id == id {
            return Err(DraftError::Invalid(
                "an item cannot be moved before itself".into(),
            ));
        }
        let at = find(draft, before_id)?;
        if draft.items[at].section.as_deref() != section {
            return Err(DraftError::Invalid(format!(
                "item {before_id} is not in the section the item is moved to"
            )));
        }
    }
    let mut item = draft.items.remove(index);
    item.section = section.map(str::to_string);
    item.edited = true;
    let position = match before {
        Some(before_id) => find(draft, before_id)?,
        None => draft.items.len(),
    };
    draft.items.insert(position, item);
    Ok(())
}

/// Removes an item and returns it.
pub fn delete_item(draft: &mut Draft, id: &str) -> Result<Item, DraftError> {
    let index = find(draft, id)?;
    Ok(draft.items.remove(index))
}

/// An item the user adds by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
    pub item_type: ItemType,
    pub text: String,
    pub assignee: Option<String>,
    pub due: Option<String>,
    pub section: Option<String>,
}

/// Adds an item by hand: no source, marked as edited, certainty `stated` for the fields
/// the user filled in and `unknown` for the others. Returns the new item's id.
pub fn add_item(draft: &mut Draft, new: NewItem) -> Result<String, DraftError> {
    if new.text.trim().is_empty() {
        return Err(DraftError::Invalid(
            "the text of an item cannot be empty".into(),
        ));
    }
    if let Some(due) = &new.due {
        if !is_iso_date(due) {
            return Err(DraftError::Invalid(format!(
                "the due date {due:?} is not YYYY-MM-DD"
            )));
        }
    }
    check_section(draft, new.section.as_deref())?;
    let id = (1..)
        .map(|n| format!("u{n:03}"))
        .find(|id| !draft.items.iter().any(|i| &i.id == id))
        .expect("an unused id exists");
    draft.items.push(Item {
        id: id.clone(),
        item_type: new.item_type,
        text: new.text.trim().to_string(),
        certainty: Certainty {
            assignee: decided(&new.assignee),
            due: decided(&new.due),
        },
        assignee: new.assignee,
        due: new.due,
        section: new.section,
        relates_to: None,
        source: None,
        edited: true,
    });
    Ok(id)
}

// ----- status changes --------------------------------------------------------------------

fn proposal_mut<'a>(draft: &'a mut Draft, id: &str) -> Result<&'a mut StatusProposal, DraftError> {
    draft
        .status_proposals
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| DraftError::NoProposal(id.to_string()))
}

/// Decides a proposed status change. Nothing is written back to GitHub or `topics.yaml`
/// before it is `Approved`, and nothing ever approves one by itself. `comment` follows the
/// double option of the item patch: `None` leaves the text as it is, `Some(None)` (or an
/// empty text) clears it, `Some(Some(text))` replaces it; a changed text marks the proposal
/// as edited.
pub fn set_status_decision(
    draft: &mut Draft,
    id: &str,
    decision: Decision,
    comment: Option<Option<String>>,
) -> Result<(), DraftError> {
    let proposal = proposal_mut(draft, id)?;
    if let Some(text) = comment {
        let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if text != proposal.comment {
            proposal.comment = text;
            proposal.edited = true;
        }
    }
    proposal.decision = decision;
    Ok(())
}

/// Adds a status change by hand, for an open issue (`#12`) or a topic id: no source, not yet
/// approved. One change per target. Returns the new id (`s1`, `s2`, ...).
pub fn add_status_proposal(
    draft: &mut Draft,
    target: &str,
    status: ProposedStatus,
    comment: Option<String>,
) -> Result<String, DraftError> {
    let target = target.trim();
    let issue = target
        .strip_prefix('#')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    // A bare number would be taken for a topic id here and for an issue elsewhere.
    let bare_number = !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit());
    if !issue && (bare_number || !is_identifier(target)) {
        return Err(DraftError::Invalid(
            "the target is an issue number such as #12 or the id of a topic".into(),
        ));
    }
    if draft.status_proposals.iter().any(|p| p.target == target) {
        return Err(DraftError::Invalid(format!(
            "{target} has a status change already"
        )));
    }
    let comment = comment
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    let next = draft
        .status_proposals
        .iter()
        .filter_map(|p| p.id.strip_prefix('s')?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = format!("s{next}");
    draft.status_proposals.push(StatusProposal {
        id: id.clone(),
        target: target.to_string(),
        status,
        reason: comment
            .clone()
            .unwrap_or_else(|| "Added by hand.".to_string()),
        comment,
        source: None,
        decision: Decision::Proposed,
        edited: true,
    });
    Ok(id)
}

// ----- sections and mode -----------------------------------------------------------------

/// Replaces the sections of the draft by `sections`, in this order: that covers renaming
/// (same id, new title), adding, removing and reordering. Items of a removed section lose
/// their section (they fall under the last section when rendered) and are marked as
/// edited. Returns the number of items that lost their section.
pub fn set_sections(draft: &mut Draft, sections: Vec<Section>) -> Result<usize, DraftError> {
    let mut seen = std::collections::HashSet::new();
    for s in &sections {
        if !is_identifier(&s.id) {
            return Err(DraftError::Invalid(format!(
                "{:?} is not a valid section id (lowercase letters, digits, - and _)",
                s.id
            )));
        }
        if s.title.trim().is_empty() {
            return Err(DraftError::Invalid(format!(
                "section {} needs a title",
                s.id
            )));
        }
        if !seen.insert(s.id.clone()) {
            return Err(DraftError::Invalid(format!(
                "section {} is listed twice",
                s.id
            )));
        }
    }
    let mut orphaned = 0;
    for item in &mut draft.items {
        if item.section.as_deref().is_some_and(|id| !seen.contains(id)) {
            item.section = None;
            item.edited = true;
            orphaned += 1;
        }
    }
    draft.sections = sections;
    Ok(orphaned)
}

/// An id for a new section made from its title, not used by another section.
pub fn section_id_for(draft: &Draft, title: &str) -> String {
    let mut base: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if !base.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        base = format!("section-{base}");
        base = base.trim_end_matches('-').to_string();
    }
    let mut id = base.clone();
    let mut n = 2;
    while draft.sections.iter().any(|s| s.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Changes the report mode, the template and the meeting type. Only the layout changes:
/// no item is touched, so nothing is re-extracted and no item is marked as edited.
pub fn set_mode_and_template(
    draft: &mut Draft,
    mode: ReportMode,
    template: Option<String>,
    meeting_type: Option<String>,
) {
    draft.mode = mode;
    draft.template = template;
    draft.meeting_type = meeting_type;
}

// ----- rendering -------------------------------------------------------------------------

/// Title of the report for `{title}`: the draft has none, so it is made from the cycle and
/// the date in the report language.
pub fn default_title(draft: &Draft, lang: Language) -> String {
    let word = match lang {
        Language::Pl => "Spotkanie",
        Language::En => "Meeting",
    };
    format!("{word} {} {}", draft.cycle, draft.date)
}

/// Renders the draft to Markdown with `template_body` in the language `lang` (the built-in
/// headings use it). `participants` are the cycle's, to turn assignee ids into names.
/// Deterministic: no model is involved.
pub fn render_report(
    draft: &Draft,
    participants: &[Participant],
    template_body: &str,
    lang: Language,
    variant: Variant,
) -> Result<String, DraftError> {
    let opts = RenderOptions {
        title: default_title(draft, lang),
        variant,
        ..RenderOptions::default()
    };
    render_report_with(draft, participants, template_body, lang, &opts)
}

/// [`render_report`] with explicit options (title, transcript link).
pub fn render_report_with(
    draft: &Draft,
    participants: &[Participant],
    template_body: &str,
    lang: Language,
    opts: &RenderOptions,
) -> Result<String, DraftError> {
    let mut localized = draft.clone();
    localized.language = lang;
    cycle_store::render(&localized, participants, template_body, opts).map_err(schema_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cycle_store::{Source, DEFAULT_TEMPLATE_TEXT};

    const FIXTURE: &str = include_str!("../../../worker/tests/fixtures/meeting.valid.json");

    fn draft() -> Draft {
        Draft::from_json_str(FIXTURE).unwrap()
    }

    /// A draft with three items in two sections.
    fn sectioned() -> Draft {
        let mut d = draft();
        d.sections = vec![
            section("hardware", "Hardware"),
            section("software", "Software"),
        ];
        let mut items = Vec::new();
        for (id, sec) in [("a", "hardware"), ("b", "hardware"), ("c", "software")] {
            let mut i = d.items[1].clone();
            i.id = id.into();
            i.text = format!("Item {id}");
            i.section = Some(sec.into());
            i.edited = false;
            items.push(i);
        }
        d.items = items;
        d
    }

    fn section(id: &str, title: &str) -> Section {
        Section {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        }
    }

    fn ids(d: &Draft) -> Vec<&str> {
        d.items.iter().map(|i| i.id.as_str()).collect()
    }

    fn tmp() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("work")).unwrap();
        t
    }

    #[test]
    fn setting_values_clears_the_certainty_to_stated_and_marks_the_item() {
        let mut d = draft();
        // i001 has an inferred due date; i002 has nothing.
        assert_eq!(d.items[0].certainty.due, CertaintyLevel::Inferred);
        update_item(
            &mut d,
            "i001",
            &ItemPatch {
                due: Some(Some("2026-12-01".into())),
                ..ItemPatch::default()
            },
        )
        .unwrap();
        assert_eq!(d.items[0].due.as_deref(), Some("2026-12-01"));
        assert_eq!(d.items[0].certainty.due, CertaintyLevel::Stated);
        assert!(d.items[0].edited);

        d.items[1].edited = false;
        update_item(
            &mut d,
            "i002",
            &ItemPatch {
                text: Some("  Add a third arm ".into()),
                assignee: Some(Some("alex".into())),
                ..ItemPatch::default()
            },
        )
        .unwrap();
        let i = &d.items[1];
        assert_eq!(i.text, "Add a third arm");
        assert_eq!(i.assignee.as_deref(), Some("alex"));
        assert_eq!(i.certainty.assignee, CertaintyLevel::Stated);
        // The due date was not touched.
        assert_eq!(i.certainty.due, CertaintyLevel::Unknown);
        assert!(i.edited);
    }

    #[test]
    fn clearing_a_value_makes_it_unknown() {
        let mut d = draft();
        update_item(
            &mut d,
            "i001",
            &ItemPatch {
                assignee: Some(None),
                ..ItemPatch::default()
            },
        )
        .unwrap();
        assert_eq!(d.items[0].assignee, None);
        assert_eq!(d.items[0].certainty.assignee, CertaintyLevel::Unknown);
    }

    #[test]
    fn an_empty_patch_changes_nothing_and_bad_values_are_refused_whole() {
        let mut d = sectioned();
        let before = d.clone();
        update_item(&mut d, "a", &ItemPatch::default()).unwrap();
        assert_eq!(d, before);
        for patch in [
            ItemPatch {
                text: Some("  ".into()),
                ..ItemPatch::default()
            },
            ItemPatch {
                text: Some("fine".into()),
                due: Some(Some("tomorrow".into())),
                ..ItemPatch::default()
            },
            ItemPatch {
                assignee: Some(Some(String::new())),
                ..ItemPatch::default()
            },
        ] {
            assert!(matches!(
                update_item(&mut d, "a", &patch),
                Err(DraftError::Invalid(_))
            ));
        }
        // The valid text in the second patch was not applied either.
        assert_eq!(d, before);
        assert!(matches!(
            update_item(&mut d, "zzz", &ItemPatch::default()),
            Err(DraftError::NoItem(_))
        ));
    }

    #[test]
    fn moving_an_item_changes_its_section_and_position() {
        let mut d = sectioned();
        move_item(&mut d, "c", Some("hardware"), Some("a")).unwrap();
        assert_eq!(ids(&d), ["c", "a", "b"]);
        assert_eq!(d.items[0].section.as_deref(), Some("hardware"));
        assert!(d.items[0].edited);
        assert!(!d.items[1].edited, "only the moved item is touched");

        move_item(&mut d, "c", Some("software"), None).unwrap();
        assert_eq!(ids(&d), ["a", "b", "c"]);

        move_item(&mut d, "a", None, None).unwrap();
        assert_eq!(d.items.last().unwrap().section, None);
    }

    #[test]
    fn a_bad_move_leaves_the_draft_as_it_was() {
        let mut d = sectioned();
        let before = d.clone();
        assert!(matches!(
            move_item(&mut d, "a", Some("ghost"), None),
            Err(DraftError::NoSection(_))
        ));
        assert!(matches!(
            move_item(&mut d, "a", Some("hardware"), Some("c")),
            Err(DraftError::Invalid(_))
        ));
        assert!(matches!(
            move_item(&mut d, "a", Some("hardware"), Some("a")),
            Err(DraftError::Invalid(_))
        ));
        assert!(matches!(
            move_item(&mut d, "a", Some("hardware"), Some("nope")),
            Err(DraftError::NoItem(_))
        ));
        assert_eq!(d, before);
    }

    #[test]
    fn deleting_an_item_removes_it() {
        let mut d = sectioned();
        let gone = delete_item(&mut d, "b").unwrap();
        assert_eq!(gone.id, "b");
        assert_eq!(ids(&d), ["a", "c"]);
        assert!(matches!(
            delete_item(&mut d, "b"),
            Err(DraftError::NoItem(_))
        ));
    }

    #[test]
    fn an_added_item_has_no_source_and_is_marked_as_edited() {
        let mut d = sectioned();
        let id = add_item(
            &mut d,
            NewItem {
                item_type: ItemType::Task,
                text: " Order the battery ".into(),
                assignee: Some("alex".into()),
                due: None,
                section: Some("hardware".into()),
            },
        )
        .unwrap();
        let item = d.items.last().unwrap();
        assert_eq!(item.id, id);
        assert_eq!(item.text, "Order the battery");
        assert!(item.source.is_none() && item.edited);
        assert_eq!(item.certainty.assignee, CertaintyLevel::Stated);
        assert_eq!(item.certainty.due, CertaintyLevel::Unknown);
        // Ids stay unique.
        let second = add_item(
            &mut d,
            NewItem {
                item_type: ItemType::Note,
                text: "x".into(),
                assignee: None,
                due: None,
                section: None,
            },
        )
        .unwrap();
        assert_ne!(second, id);
        assert!(matches!(
            add_item(
                &mut d,
                NewItem {
                    item_type: ItemType::Note,
                    text: "x".into(),
                    assignee: None,
                    due: None,
                    section: Some("ghost".into()),
                }
            ),
            Err(DraftError::NoSection(_))
        ));
    }

    #[test]
    fn sections_can_be_renamed_added_reordered_and_removed() {
        let mut d = sectioned();
        // Rename, add one, reorder.
        let orphaned = set_sections(
            &mut d,
            vec![
                section("ideas", "Ideas"),
                section("software", "Code"),
                section("hardware", "Hardware"),
            ],
        )
        .unwrap();
        assert_eq!(orphaned, 0);
        let titles: Vec<_> = d.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Ideas", "Code", "Hardware"]);
        assert!(d.items.iter().all(|i| !i.edited), "no item changed");

        // Remove `hardware`: a and b lose their section and are marked.
        let orphaned = set_sections(&mut d, vec![section("software", "Code")]).unwrap();
        assert_eq!(orphaned, 2);
        assert_eq!(d.items[0].section, None);
        assert!(d.items[0].edited && d.items[1].edited);
        assert_eq!(d.items[2].section.as_deref(), Some("software"));
        assert!(!d.items[2].edited);
    }

    #[test]
    fn invalid_section_lists_are_refused_without_a_change() {
        let mut d = sectioned();
        let before = d.clone();
        for bad in [
            vec![section("Bad Id", "x")],
            vec![section("a", "  ")],
            vec![section("a", "A"), section("a", "B")],
        ] {
            assert!(matches!(
                set_sections(&mut d, bad),
                Err(DraftError::Invalid(_))
            ));
        }
        assert_eq!(d, before);
    }

    #[test]
    fn new_section_ids_are_made_from_titles_and_never_clash() {
        let d = sectioned();
        assert_eq!(section_id_for(&d, "Open questions"), "open-questions");
        assert_eq!(section_id_for(&d, "Hardware"), "hardware-2");
        assert_eq!(section_id_for(&d, "2nd phase"), "section-2nd-phase");
        assert!(is_identifier(&section_id_for(&d, "Przegląd budżetu!")));
        assert!(is_identifier(&section_id_for(&d, "???")));
    }

    #[test]
    fn mode_and_template_change_the_layout_only() {
        let mut d = sectioned();
        let items = d.items.clone();
        set_mode_and_template(
            &mut d,
            ReportMode::ByType,
            Some("budget".into()),
            Some("budget".into()),
        );
        assert_eq!(d.mode, ReportMode::ByType);
        assert_eq!(d.template.as_deref(), Some("budget"));
        assert_eq!(d.meeting_type.as_deref(), Some("budget"));
        assert_eq!(d.items, items);
        assert!(Draft::from_value(&d.to_value()).is_ok());
    }

    #[test]
    fn every_operation_leaves_a_valid_document() {
        let mut d = sectioned();
        update_item(
            &mut d,
            "a",
            &ItemPatch {
                text: Some("changed".into()),
                due: Some(Some("2026-12-24".into())),
                ..ItemPatch::default()
            },
        )
        .unwrap();
        move_item(&mut d, "c", Some("hardware"), Some("a")).unwrap();
        add_item(
            &mut d,
            NewItem {
                item_type: ItemType::Budget,
                text: "Budget for tests".into(),
                assignee: None,
                due: None,
                section: None,
            },
        )
        .unwrap();
        delete_item(&mut d, "b").unwrap();
        set_sections(&mut d, vec![section("hardware", "Hardware")]).unwrap();
        assert!(schema::validate(Schema::Meeting, &d.to_value()).is_empty());
    }

    #[test]
    fn a_saved_draft_loads_back_identical_and_edits_persist() {
        let dir = tmp();
        let files = DraftFiles::new(dir.path());
        assert!(matches!(files.load(), Err(DraftError::NotReady)));
        assert!(!files.exists());
        fs::write(dir.path().join("work/draft.json"), FIXTURE).unwrap();
        assert!(files.exists());
        assert_eq!(files.load().unwrap(), draft());

        files
            .edit(|d| {
                update_item(
                    d,
                    "i001",
                    &ItemPatch {
                        text: Some("Prepare the monitor mount and the cables".into()),
                        ..ItemPatch::default()
                    },
                )
            })
            .unwrap();
        files
            .edit(|d| {
                add_item(
                    d,
                    NewItem {
                        item_type: ItemType::Note,
                        text: "Remember the cables".into(),
                        assignee: None,
                        due: None,
                        section: None,
                    },
                )
                .map(|_| ())
            })
            .unwrap();
        // A fresh handle sees everything, and no temporary file is left behind.
        let back = DraftFiles::new(dir.path()).load().unwrap();
        assert_eq!(
            back.items[0].text,
            "Prepare the monitor mount and the cables"
        );
        assert!(back.items[0].edited);
        assert_eq!(back.items.len(), 3);
        assert!(has_edited_items(&back));
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("work"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// The bytes `reduce` writes: no item is marked as edited.
    fn pristine_bytes() -> Vec<u8> {
        let mut d = draft();
        for item in &mut d.items {
            item.edited = false;
        }
        serde_json::to_vec_pretty(&d.to_value()).unwrap()
    }

    /// A draft as `reduce` leaves it: the file and the hash of its bytes.
    fn reduced(dir: &Path) -> DraftFiles {
        let bytes = pristine_bytes();
        fs::write(dir.join("work/draft.json"), &bytes).unwrap();
        fs::write(
            dir.join("work").join(BASE_HASH_FILE),
            format!("{}\n", sha256_hex(&bytes)),
        )
        .unwrap();
        DraftFiles::new(dir)
    }

    #[test]
    fn an_untouched_draft_has_no_user_edits() {
        let dir = tmp();
        let files = DraftFiles::new(dir.path());
        // No draft at all: nothing to protect.
        assert!(!files.has_user_edits().unwrap());
        let files = reduced(dir.path());
        assert!(!files.has_user_edits().unwrap());
        // Without a recorded hash only the marks on items count.
        fs::remove_file(dir.path().join("work").join(BASE_HASH_FILE)).unwrap();
        assert!(!files.has_user_edits().unwrap());
    }

    #[test]
    fn deleting_an_item_or_renaming_a_section_counts_as_an_edit_without_any_mark() {
        for operation in 0..3 {
            let dir = tmp();
            let files = reduced(dir.path());
            files
                .edit(|d| match operation {
                    0 => delete_item(d, "i001").map(|_| ()),
                    1 => set_sections(d, vec![section("renamed", "Renamed")]).map(|_| ()),
                    _ => {
                        set_mode_and_template(d, ReportMode::Fixed, None, None);
                        Ok(())
                    }
                })
                .unwrap();
            let draft = files.load().unwrap();
            if operation != 1 {
                assert!(
                    !has_edited_items(&draft),
                    "operation {operation} marks nothing"
                );
            }
            assert!(files.has_user_edits().unwrap(), "operation {operation}");
            // The same bytes under a hash that does not exist: marks decide.
            fs::remove_file(dir.path().join("work").join(BASE_HASH_FILE)).unwrap();
            assert_eq!(files.has_user_edits().unwrap(), has_edited_items(&draft));
        }
    }

    #[test]
    fn accepting_a_new_draft_makes_it_pristine_and_discarding_keeps_the_edits_protected() {
        let dir = tmp();
        let files = reduced(dir.path());
        files.edit(|d| delete_item(d, "i001").map(|_| ())).unwrap();
        assert!(files.has_user_edits().unwrap());

        let mut fresh = draft();
        fresh.summary = "Fresh.".into();
        for item in &mut fresh.items {
            item.edited = false;
        }
        let fresh_bytes = serde_json::to_vec_pretty(&fresh.to_value()).unwrap();
        let new_path = dir.path().join("work").join(NEW_DRAFT_FILE);
        fs::write(&new_path, &fresh_bytes).unwrap();
        assert!(files.discard_new_draft().unwrap());
        assert!(
            files.has_user_edits().unwrap(),
            "discarding keeps the protection"
        );

        fs::write(&new_path, &fresh_bytes).unwrap();
        files.accept_new_draft().unwrap();
        let recorded = fs::read_to_string(dir.path().join("work").join(BASE_HASH_FILE)).unwrap();
        assert_eq!(recorded.trim(), sha256_hex(&fresh_bytes));
        assert!(!files.has_user_edits().unwrap());
        // The next user operation changes the bytes and so counts again.
        files
            .edit(|d| {
                set_mode_and_template(d, ReportMode::Fixed, None, None);
                Ok(())
            })
            .unwrap();
        assert!(files.has_user_edits().unwrap());
    }

    #[test]
    fn the_hash_is_lowercase_hex_of_the_exact_bytes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // An upper-case or damaged record is read as it is meant, or ignored.
        let dir = tmp();
        let files = reduced(dir.path());
        let upper = sha256_hex(&pristine_bytes()).to_uppercase();
        fs::write(dir.path().join("work").join(BASE_HASH_FILE), upper).unwrap();
        assert!(!files.has_user_edits().unwrap());
        fs::write(dir.path().join("work").join(BASE_HASH_FILE), "not a hash").unwrap();
        assert!(!files.has_user_edits().unwrap());
    }

    #[test]
    fn concurrent_saves_use_their_own_temporary_files() {
        let dir = tmp();
        fs::write(dir.path().join("work/draft.json"), FIXTURE).unwrap();
        let path = dir.path().join("work/draft.json");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        let mut d = draft();
                        d.summary = format!("writer {n} round {i}");
                        write_draft(&path, &d).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // Whatever won, the file is a whole, valid draft and no temporary file is left.
        assert!(DraftFiles::new(dir.path())
            .load()
            .unwrap()
            .summary
            .starts_with("writer"));
        let tmps = fs::read_dir(dir.path().join("work"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(tmps, 0);
    }

    #[test]
    fn a_failed_edit_writes_nothing() {
        let dir = tmp();
        fs::write(dir.path().join("work/draft.json"), FIXTURE).unwrap();
        let files = DraftFiles::new(dir.path());
        let err = files.edit(|d| delete_item(d, "ghost").map(|_| ()));
        assert!(matches!(err, Err(DraftError::NoItem(_))));
        assert_eq!(files.load().unwrap(), draft());
    }

    #[test]
    fn saving_refuses_a_document_that_breaks_the_schema() {
        let dir = tmp();
        let files = DraftFiles::new(dir.path());
        let mut d = draft();
        d.version = 99;
        assert!(matches!(files.save(&d), Err(DraftError::Schema(_))));
        assert!(!files.exists());
    }

    #[test]
    fn an_invalid_file_is_reported_not_loaded() {
        let dir = tmp();
        fs::write(dir.path().join("work/draft.json"), "{\"version\": 1}").unwrap();
        assert!(matches!(
            DraftFiles::new(dir.path()).load(),
            Err(DraftError::Schema(_))
        ));
        fs::write(dir.path().join("work/draft.json"), "not json").unwrap();
        assert!(matches!(
            DraftFiles::new(dir.path()).load(),
            Err(DraftError::Schema(_))
        ));
    }

    #[test]
    fn accepting_the_new_draft_replaces_the_edited_one() {
        let dir = tmp();
        let files = DraftFiles::new(dir.path());
        assert!(matches!(
            files.accept_new_draft(),
            Err(DraftError::NoNewDraft)
        ));
        fs::write(dir.path().join("work/draft.json"), FIXTURE).unwrap();
        files
            .edit(|d| {
                update_item(
                    d,
                    "i002",
                    &ItemPatch {
                        text: Some("My own wording".into()),
                        ..ItemPatch::default()
                    },
                )
            })
            .unwrap();
        let mut fresh = draft();
        fresh.summary = "A new summary.".into();
        fresh.items.truncate(1);
        fs::write(
            dir.path().join("work/draft.new.json"),
            serde_json::to_vec(&fresh.to_value()).unwrap(),
        )
        .unwrap();
        assert!(files.has_new_draft());
        // Until the user decides, the edited draft is what loads.
        assert_eq!(files.load().unwrap().items[1].text, "My own wording");

        let accepted = files.accept_new_draft().unwrap();
        assert_eq!(accepted.summary, "A new summary.");
        assert_eq!(files.load().unwrap(), fresh);
        assert!(!files.has_new_draft());
    }

    #[test]
    fn an_invalid_new_draft_is_not_accepted_and_the_old_one_stays() {
        let dir = tmp();
        let files = DraftFiles::new(dir.path());
        fs::write(dir.path().join("work/draft.json"), FIXTURE).unwrap();
        fs::write(dir.path().join("work/draft.new.json"), "{}").unwrap();
        assert!(matches!(
            files.accept_new_draft(),
            Err(DraftError::Schema(_))
        ));
        assert_eq!(files.load().unwrap(), draft());
        assert!(files.discard_new_draft().unwrap());
        assert!(!files.discard_new_draft().unwrap());
    }

    fn people() -> Vec<Participant> {
        vec![Participant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: None,
        }]
    }

    #[test]
    fn the_report_is_rendered_in_the_requested_language_without_a_model() {
        let mut d = draft();
        d.mode = ReportMode::Fixed;
        d.sections.clear();
        d.items[0].section = None;
        d.items[1].section = None;
        let en = render_report(
            &d,
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            Language::En,
            Variant::Markdown,
        )
        .unwrap();
        assert!(en.starts_with("# Meeting TEAM 2026-11-16"), "{en}");
        assert!(en.contains("## Tasks"), "{en}");
        assert!(en.contains("Alex Example"), "{en}");
        assert!(en.contains("- [ ]"), "{en}");

        let pl = render_report(
            &d,
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            Language::Pl,
            Variant::Markdown,
        )
        .unwrap();
        assert!(pl.starts_with("# Spotkanie TEAM 2026-11-16"), "{pl}");
        assert!(pl.contains("## Zadania"), "{pl}");
        assert!(!pl.contains("## Tasks"));

        let chat = render_report(
            &d,
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            Language::En,
            Variant::Chat,
        )
        .unwrap();
        assert!(!chat.contains("- [ ]"), "{chat}");
        // Rendering does not change the draft.
        assert_eq!(d.language, Language::En);
    }

    #[test]
    fn an_unknown_placeholder_is_an_error_not_a_blank() {
        let err = render_report(
            &draft(),
            &people(),
            "{nonsense}",
            Language::En,
            Variant::Markdown,
        )
        .unwrap_err();
        assert!(matches!(err, DraftError::Schema(_)));
    }

    #[test]
    fn unverified_items_keep_their_marker_in_the_report() {
        let mut d = draft();
        d.sections.clear();
        d.items[0].section = None;
        let mut src: Source = d.items[0].source.clone().unwrap();
        src.verified = false;
        d.items[0].source = Some(src);
        let md = render_report(
            &d,
            &people(),
            DEFAULT_TEMPLATE_TEXT,
            Language::En,
            Variant::Markdown,
        )
        .unwrap();
        assert!(md.contains("unverified"), "{md}");
    }

    fn with_proposals() -> Draft {
        let mut d = draft();
        d.status_proposals = vec![StatusProposal {
            id: "s1".into(),
            target: "#12".into(),
            status: ProposedStatus::Done,
            reason: "It was reported finished.".into(),
            comment: None,
            source: None,
            decision: Decision::Proposed,
            edited: false,
        }];
        d
    }

    #[test]
    fn deciding_a_status_change_keeps_everything_else() {
        let mut d = with_proposals();
        assert!(!has_edited_items(&d) || d.items.iter().any(|i| i.edited));
        let before = d.status_proposals[0].clone();
        set_status_decision(&mut d, "s1", Decision::Approved, None).unwrap();
        let p = &d.status_proposals[0];
        assert_eq!(p.decision, Decision::Approved);
        assert_eq!(
            (&p.reason, &p.comment, p.edited),
            (&before.reason, &before.comment, false)
        );
        assert!(has_edited_items(&d), "a decision is a user edit");

        set_status_decision(
            &mut d,
            "s1",
            Decision::Approved,
            Some(Some("  Done on Friday ".into())),
        )
        .unwrap();
        assert_eq!(
            d.status_proposals[0].comment.as_deref(),
            Some("Done on Friday")
        );
        assert!(d.status_proposals[0].edited);
        set_status_decision(&mut d, "s1", Decision::Rejected, Some(Some("   ".into()))).unwrap();
        assert_eq!(d.status_proposals[0].comment, None);
        assert_eq!(d.status_proposals[0].decision, Decision::Rejected);
        assert!(matches!(
            set_status_decision(&mut d, "s9", Decision::Approved, None),
            Err(DraftError::NoProposal(_))
        ));
    }

    #[test]
    fn a_status_change_added_by_hand_waits_for_approval() {
        let mut d = draft();
        let id = add_status_proposal(
            &mut d,
            " #7 ",
            ProposedStatus::Done,
            Some("Closed in the meeting".into()),
        )
        .unwrap();
        assert_eq!(id, "s1");
        let p = &d.status_proposals[0];
        assert_eq!(
            (p.target.as_str(), p.decision, p.edited),
            ("#7", Decision::Proposed, true)
        );
        assert_eq!(p.comment.as_deref(), Some("Closed in the meeting"));
        assert!(p.source.is_none());
        let topic = add_status_proposal(&mut d, "t-2", ProposedStatus::Open, None).unwrap();
        assert_eq!(topic, "s2");
        assert_eq!(d.status_proposals[1].reason, "Added by hand.");
        for bad in ["", "#", "#x", "12", "two words", "#7"] {
            assert!(
                add_status_proposal(&mut d, bad, ProposedStatus::Done, None).is_err(),
                "{bad:?}"
            );
        }
        assert!(schema::validate(Schema::Meeting, &d.to_value()).is_empty());
    }
}
