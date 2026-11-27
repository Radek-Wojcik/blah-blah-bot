//! The draft report of a meeting: the view the draft editor shows and the operations on it.
//!
//! Every operation changes `work/draft.json` and answers with the whole new view, so the UI
//! never merges anything. The functions work on a meeting folder, the registered repositories
//! and a `&Store`; the commands at the end are thin wrappers that also emit `draft-changed`.

use std::path::Path;

use cycle_store::{
    Certainty, Cycle, Decision, Draft, Item, ItemType, Language, Participant, ProposedStatus,
    ReportMode, Section, Source, StatusProposal, Template, Variant, Warning, WarningCode,
};
use queue::draft::{self as ops, DraftError, DraftFiles, ItemPatch, NewItem};
use serde::{Deserialize, Deserializer, Serialize};
use store::Store;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cycles::{self, Located, ReportChoice};
use crate::ipc::{IpcError, IpcResult};
use crate::processing_commands::meeting_dir;
use crate::settings::CycleRepoSetting;
use crate::state::AppState;

pub const EVENT_DRAFT: &str = "draft-changed";

// ----- payloads ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionDto {
    /// Empty for a new section: the application makes an id from the title.
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

impl From<&Section> for SectionDto {
    fn from(s: &Section) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            description: s.description.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    pub track: String,
    pub speaker: String,
    pub t_start_ms: u64,
    pub t_end_ms: u64,
    pub quote: String,
    pub verified: bool,
    #[serde(rename = "match")]
    pub match_score: f64,
}

impl From<&Source> for SourceDto {
    fn from(s: &Source) -> Self {
        Self {
            track: s.track.clone(),
            speaker: s.speaker.clone(),
            t_start_ms: s.t_start_ms,
            t_end_ms: s.t_end_ms,
            quote: s.quote.clone(),
            verified: s.verified,
            match_score: s.match_score,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDto {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub text: String,
    pub assignee: Option<String>,
    pub due: Option<String>,
    pub certainty: Certainty,
    pub section: Option<String>,
    pub relates_to: Option<String>,
    pub source: Option<SourceDto>,
    pub edited: bool,
}

impl From<&Item> for ItemDto {
    fn from(i: &Item) -> Self {
        Self {
            id: i.id.clone(),
            item_type: i.item_type,
            text: i.text.clone(),
            assignee: i.assignee.clone(),
            due: i.due.clone(),
            certainty: i.certainty,
            section: i.section.clone(),
            relates_to: i.relates_to.clone(),
            source: i.source.as_ref().map(Into::into),
            edited: i.edited,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningDto {
    pub code: WarningCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t_start_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t_end_ms: Option<i64>,
}

impl From<&Warning> for WarningDto {
    fn from(w: &Warning) -> Self {
        Self {
            code: w.code,
            message: w.message.clone(),
            t_start_ms: w.t_start_ms,
            t_end_ms: w.t_end_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusProposalDto {
    pub id: String,
    pub target: String,
    pub status: ProposedStatus,
    pub reason: String,
    pub comment: Option<String>,
    pub source: Option<SourceDto>,
    pub decision: Decision,
    pub edited: bool,
}

impl From<&StatusProposal> for StatusProposalDto {
    fn from(p: &StatusProposal) -> Self {
        Self {
            id: p.id.clone(),
            target: p.target.clone(),
            status: p.status,
            reason: p.reason.clone(),
            comment: p.comment.clone(),
            source: p.source.as_ref().map(Into::into),
            decision: p.decision,
            edited: p.edited,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftDto {
    pub meeting_id: String,
    pub cycle: String,
    pub date: String,
    pub language: Language,
    pub mode: ReportMode,
    pub template: Option<String>,
    pub meeting_type: Option<String>,
    pub sections: Vec<SectionDto>,
    pub summary: String,
    pub items: Vec<ItemDto>,
    /// Absent while the draft has no status proposals.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status_proposals: Vec<StatusProposalDto>,
    pub warnings: Vec<WarningDto>,
}

impl From<&Draft> for DraftDto {
    fn from(d: &Draft) -> Self {
        Self {
            meeting_id: d.meeting_id.clone(),
            cycle: d.cycle.clone(),
            date: d.date.clone(),
            language: d.language,
            mode: d.mode,
            template: d.template.clone(),
            meeting_type: d.meeting_type.clone(),
            sections: d.sections.iter().map(Into::into).collect(),
            summary: d.summary.clone(),
            items: d.items.iter().map(Into::into).collect(),
            status_proposals: d.status_proposals.iter().map(Into::into).collect(),
            warnings: d.warnings.iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftParticipant {
    pub id: String,
    pub name: String,
}

/// Everything the draft editor shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    pub draft: DraftDto,
    /// The report rendered with the meeting's template, in the language of the draft.
    pub report_markdown: String,
    /// A re-run of `reduce` left a newer draft next to the edited one.
    pub has_new_draft: bool,
    /// The participants of the cycle, to pick assignees from.
    pub participants: Vec<DraftParticipant>,
    pub transcript_present: bool,
    /// Set when the template could not be used and the built-in layout was rendered instead.
    pub report_warning: Option<String>,
    /// The draft is read-only: the meeting is published, or operations for it are queued.
    /// Cancelling them in the outbox unlocks it.
    pub locked: bool,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// Changes of one item. A field that is absent stays as it is; `null` clears `assignee` or
/// `due`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPatchDto {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub assignee: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub due: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewItemDto {
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub text: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
}

// ----- errors ------------------------------------------------------------------------------

/// `busy` while the `extract` or `reduce` stage of the meeting is running: `reduce` reads the
/// draft and writes it, and an edit in between would be lost or would overwrite it.
pub fn ensure_draft_idle(store: &Store, meeting_id: &str) -> IpcResult<()> {
    let Some(job) = store.get_job(meeting_id)? else {
        return Ok(());
    };
    let running = job.stages.iter().any(|s| {
        s.status == store::StageStatus::Running && (s.stage == "extract" || s.stage == "reduce")
    });
    if running {
        return Err(IpcError::new(
            "busy",
            "the report is being extracted; wait for it to finish before editing the draft",
        ));
    }
    Ok(())
}

/// `published` while operations of the meeting are in the outbox: the draft is what was
/// frozen for publishing and stays as it is until they are cancelled.
pub fn ensure_draft_unlocked(store: &Store, meeting_id: &str) -> IpcResult<()> {
    match publishers::flow::lock_reason(store, meeting_id)? {
        Some(why) => Err(IpcError::new("published", why)),
        None => Ok(()),
    }
}

/// [`ensure_draft_idle`] and [`ensure_draft_unlocked`] for the command layer: every command
/// that changes the draft starts with it.
fn idle(state: &AppState, meeting_id: &str) -> IpcResult<()> {
    state.with_store_ipc(|s| {
        ensure_draft_idle(s, meeting_id)?;
        ensure_draft_unlocked(s, meeting_id)
    })
}

/// The view with its `locked` flag.
fn with_lock(state: &AppState, meeting_id: &str, mut view: DraftView) -> IpcResult<DraftView> {
    view.locked = state.with_store(|s| publishers::flow::is_locked(s, meeting_id))?;
    Ok(view)
}

/// Draft errors with stable codes: `no_draft` until the reduce stage has written one.
pub fn draft_error(e: DraftError) -> IpcError {
    match e {
        DraftError::NotReady => IpcError::new("no_draft", e.to_string()),
        DraftError::NoNewDraft => IpcError::new("no_new_draft", e.to_string()),
        DraftError::NoItem(_) | DraftError::NoSection(_) | DraftError::NoProposal(_) => {
            IpcError::new("not_found", e.to_string())
        }
        DraftError::Invalid(_) => IpcError::new("invalid", e.to_string()),
        DraftError::Io(_) => IpcError::new("io", e.to_string()),
        DraftError::Schema(_) => IpcError::new("draft_invalid", e.to_string()),
    }
}

// ----- the view ----------------------------------------------------------------------------

/// The template that lays out the report: by-type mode follows the cycle's mapping, the
/// other modes the draft's template or the cycle default.
pub fn template_name(cycle: &Cycle, draft: &Draft) -> String {
    let default = &cycle.report.default_template;
    match draft.mode {
        ReportMode::ByType => draft
            .meeting_type
            .as_ref()
            .and_then(|t| cycle.report.by_type.get(t))
            .unwrap_or(default)
            .clone(),
        _ => draft.template.clone().unwrap_or_else(|| default.clone()),
    }
}

fn participants_of(located: Option<&Located>) -> Vec<Participant> {
    located
        .and_then(|l| l.entry.cycle.as_ref())
        .map(|c| c.participants.clone())
        .unwrap_or_default()
}

/// Renders the draft with its template. A template that is missing or cannot render falls
/// back to the built-in layout; the second value then says why.
pub(crate) fn render(
    located: Option<&Located>,
    draft: &Draft,
    variant: Variant,
) -> IpcResult<(String, Option<String>)> {
    let participants = participants_of(located);
    let wanted = located.and_then(|l| {
        let cycle = l.entry.cycle.as_ref()?;
        let name = template_name(cycle, draft);
        Some((name.clone(), l.repo.read_template(&cycle.id, &name)))
    });
    let mut warning = None;
    if let Some((name, result)) = wanted {
        match result {
            Ok(t) => {
                match ops::render_report(draft, &participants, &t.body, draft.language, variant) {
                    Ok(text) => return Ok((text, None)),
                    Err(e) => warning = Some(format!("template {name} cannot be used: {e}")),
                }
            }
            Err(e) => warning = Some(format!("template {name} cannot be read: {e}")),
        }
    }
    let body = Template::builtin_default().body;
    let text = ops::render_report(draft, &participants, &body, draft.language, variant)
        .map_err(draft_error)?;
    Ok((text, warning))
}

fn view_of(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    draft: &Draft,
    files: &DraftFiles,
) -> IpcResult<DraftView> {
    let located = cycles::locate(repos, &draft.cycle)?;
    let (report_markdown, report_warning) = render(located.as_ref(), draft, Variant::Markdown)?;
    Ok(DraftView {
        draft: draft.into(),
        report_markdown,
        has_new_draft: files.has_new_draft(),
        participants: participants_of(located.as_ref())
            .into_iter()
            .map(|p| DraftParticipant {
                id: p.id,
                name: p.name,
            })
            .collect(),
        transcript_present: meeting_dir.join("work").join("transcript.json").is_file(),
        report_warning,
        locked: false,
    })
}

/// The draft of a meeting with its rendered report. `no_draft` until the `reduce` stage
/// has written one.
pub fn load_view(repos: &[CycleRepoSetting], meeting_dir: &Path) -> IpcResult<DraftView> {
    let files = DraftFiles::new(meeting_dir);
    let draft = files.load().map_err(draft_error)?;
    view_of(repos, meeting_dir, &draft, &files)
}

/// Applies `change` to the draft, saves it and returns the new view. Nothing is written
/// when `change` fails.
fn edit(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    change: impl FnOnce(&mut Draft) -> Result<(), DraftError>,
) -> IpcResult<DraftView> {
    let files = DraftFiles::new(meeting_dir);
    files.edit(change).map_err(draft_error)?;
    let draft = files.load().map_err(draft_error)?;
    view_of(repos, meeting_dir, &draft, &files)
}

// ----- operations --------------------------------------------------------------------------

pub fn update_item(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    item_id: &str,
    patch: ItemPatchDto,
) -> IpcResult<DraftView> {
    let patch = ItemPatch {
        text: patch.text,
        assignee: patch.assignee,
        due: patch.due,
    };
    edit(repos, meeting_dir, |d| ops::update_item(d, item_id, &patch))
}

/// An assignee must be a participant of the cycle (or none).
fn check_assignee(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    assignee: Option<&str>,
) -> IpcResult<()> {
    let Some(id) = assignee else {
        return Ok(());
    };
    let draft = DraftFiles::new(meeting_dir).load().map_err(draft_error)?;
    let located = cycles::locate(repos, &draft.cycle)?;
    // Without the cycle (its repository is gone) nothing can be checked.
    if let Some(cycle) = located.as_ref().and_then(|l| l.entry.cycle.as_ref()) {
        if !cycle.participants.iter().any(|p| p.id == id) {
            return Err(IpcError::new(
                "invalid",
                format!("{id} is not a participant of the cycle"),
            ));
        }
    }
    Ok(())
}

/// [`update_item`] that also refuses an assignee who is not in the cycle.
pub fn update_item_checked(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    item_id: &str,
    patch: ItemPatchDto,
) -> IpcResult<DraftView> {
    if let Some(Some(who)) = &patch.assignee {
        check_assignee(repos, meeting_dir, Some(who))?;
    }
    update_item(repos, meeting_dir, item_id, patch)
}

pub fn move_item(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    item_id: &str,
    section_id: Option<&str>,
    before_item_id: Option<&str>,
) -> IpcResult<DraftView> {
    edit(repos, meeting_dir, |d| {
        ops::move_item(d, item_id, section_id, before_item_id)
    })
}

pub fn delete_item(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    item_id: &str,
) -> IpcResult<DraftView> {
    edit(repos, meeting_dir, |d| {
        ops::delete_item(d, item_id).map(|_| ())
    })
}

pub fn add_item(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    item: NewItemDto,
) -> IpcResult<DraftView> {
    check_assignee(repos, meeting_dir, item.assignee.as_deref())?;
    edit(repos, meeting_dir, |d| {
        ops::add_item(
            d,
            NewItem {
                item_type: item.item_type,
                text: item.text,
                assignee: item.assignee,
                due: item.due,
                section: item.section,
            },
        )
        .map(|_| ())
    })
}

/// Decides a status change. `comment` is the text posted with it: `None` leaves it as it is
/// and an empty text clears it.
pub fn set_decision(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    proposal_id: &str,
    decision: Decision,
    comment: Option<String>,
) -> IpcResult<DraftView> {
    edit(repos, meeting_dir, |d| {
        ops::set_status_decision(d, proposal_id, decision, comment.map(Some))
    })
}

/// Adds a status change by hand. A topic must exist in the cycle; an issue is written `#12`.
pub fn add_proposal(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    target: &str,
    status: ProposedStatus,
    comment: Option<String>,
) -> IpcResult<DraftView> {
    let target = target.trim();
    if !target.starts_with('#') && !target.contains('/') {
        let draft = DraftFiles::new(meeting_dir).load().map_err(draft_error)?;
        if let Some(located) = cycles::locate(repos, &draft.cycle)? {
            let topics = located.repo.load_topics(&draft.cycle)?;
            if !topics.iter().any(|t| t.id == target) {
                return Err(IpcError::new(
                    "invalid",
                    format!("{target} is not a topic of the cycle (issues are written #12)"),
                ));
            }
        }
    }
    edit(repos, meeting_dir, |d| {
        ops::add_status_proposal(d, target, status, comment).map(|_| ())
    })
}

/// Replaces the sections (rename, add, remove, reorder). A section without an id gets one
/// made from its title.
pub fn set_sections(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    sections: Vec<SectionDto>,
) -> IpcResult<DraftView> {
    edit(repos, meeting_dir, |d| {
        let mut scratch = d.clone();
        scratch.sections = sections
            .iter()
            .filter(|s| !s.id.is_empty())
            .map(to_section)
            .collect();
        let mut out = Vec::with_capacity(sections.len());
        for s in &sections {
            let mut section = to_section(s);
            if section.id.is_empty() {
                section.id = ops::section_id_for(&scratch, &section.title);
                scratch.sections.push(section.clone());
            }
            out.push(section);
        }
        ops::set_sections(d, out).map(|_| ())
    })
}

fn to_section(s: &SectionDto) -> Section {
    Section {
        id: s.id.trim().to_string(),
        title: s.title.trim().to_string(),
        description: s.description.clone(),
    }
}

/// Changes the report mode, template or meeting type of a meeting and renders again. No
/// item is touched and nothing is re-assigned. Template and by-type modes are validated
/// against the cycle (`report_mode_required`, as at the gate).
pub fn change_report_mode(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    mode: &str,
    template: Option<String>,
    meeting_type: Option<String>,
) -> IpcResult<DraftView> {
    let files = DraftFiles::new(meeting_dir);
    let draft = files.load().map_err(draft_error)?;
    let checked = cycles::validate_report(
        repos,
        &ReportChoice {
            cycle_id: draft.cycle.clone(),
            mode: mode.to_string(),
            template,
            meeting_type,
            output_language: None,
        },
    )?;
    let mut stored = store
        .get_report_settings(meeting_id)?
        .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
    stored.report_mode = checked.report_mode.clone();
    stored.report_template = checked.report_template.clone();
    stored.meeting_type = checked.meeting_type.clone();
    // The template the layout comes from, as `reduce` records it in the draft.
    let located = cycles::locate(repos, &draft.cycle)?;
    let layout = match (&checked.meeting_type, &located) {
        (Some(kind), Some(l)) => l
            .entry
            .cycle
            .as_ref()
            .and_then(|c| c.report.by_type.get(kind).cloned()),
        _ => checked.report_template.clone(),
    };
    let new_mode = match mode {
        "template" => ReportMode::Template,
        "auto" => ReportMode::Auto,
        "by-type" => ReportMode::ByType,
        _ => ReportMode::Fixed,
    };
    let result = edit(repos, meeting_dir, |d| {
        ops::set_mode_and_template(d, new_mode, layout, checked.meeting_type.clone());
        Ok(())
    })?;
    store.set_report_settings(meeting_id, &stored)?;
    Ok(result)
}

/// Replaces the edited draft by the newer one a re-run of `reduce` left.
pub fn accept_new(repos: &[CycleRepoSetting], meeting_dir: &Path) -> IpcResult<DraftView> {
    let files = DraftFiles::new(meeting_dir);
    let draft = files.accept_new_draft().map_err(draft_error)?;
    view_of(repos, meeting_dir, &draft, &files)
}

/// Keeps the edited draft and throws the newer one away.
pub fn discard_new(repos: &[CycleRepoSetting], meeting_dir: &Path) -> IpcResult<DraftView> {
    let files = DraftFiles::new(meeting_dir);
    files.discard_new_draft().map_err(draft_error)?;
    let draft = files.load().map_err(draft_error)?;
    view_of(repos, meeting_dir, &draft, &files)
}

/// The report as Markdown for a target: `markdown` (GitHub, with task checkboxes) or `chat`
/// (Discord and Notion, plain bullets).
pub fn render_variant(
    repos: &[CycleRepoSetting],
    meeting_dir: &Path,
    variant: &str,
) -> IpcResult<String> {
    let variant = match variant {
        "markdown" => Variant::Markdown,
        "chat" => Variant::Chat,
        _ => return Err(IpcError::new("invalid", "the variant is markdown or chat")),
    };
    let draft = DraftFiles::new(meeting_dir).load().map_err(draft_error)?;
    let located = cycles::locate(repos, &draft.cycle)?;
    Ok(render(located.as_ref(), &draft, variant)?.0)
}

// ----- commands ----------------------------------------------------------------------------

fn changed(app: &AppHandle, meeting_id: &str) {
    if let Err(e) = app.emit(EVENT_DRAFT, meeting_id) {
        tracing::warn!(error = %e, "could not emit the draft change");
    }
}

fn after(app: &AppHandle, meeting_id: &str, view: IpcResult<DraftView>) -> IpcResult<DraftView> {
    let state = app.state::<AppState>();
    let view = view.and_then(|v| with_lock(&state, meeting_id, v));
    if view.is_ok() {
        if let Err(e) = crate::local_export::auto_export_if_enabled(&state, meeting_id) {
            tracing::warn!(
                meeting = %meeting_id,
                error = %e,
                "automatic local markdown export failed"
            );
        }
        changed(app, meeting_id);
    }
    view
}

#[tauri::command]
pub fn get_draft(state: State<'_, AppState>, meeting_id: String) -> IpcResult<DraftView> {
    let view = load_view(&state.cycle_repos(), &meeting_dir(&state, &meeting_id)?)?;
    with_lock(&state, &meeting_id, view)
}

#[tauri::command]
pub fn update_draft_item(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    item_id: String,
    patch: ItemPatchDto,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = update_item_checked(&state.cycle_repos(), &dir, &item_id, patch);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn move_draft_item(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    item_id: String,
    section_id: Option<String>,
    before_item_id: Option<String>,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = move_item(
        &state.cycle_repos(),
        &dir,
        &item_id,
        section_id.as_deref(),
        before_item_id.as_deref(),
    );
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn delete_draft_item(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    item_id: String,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = delete_item(&state.cycle_repos(), &dir, &item_id);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn add_draft_item(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    item: NewItemDto,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = add_item(&state.cycle_repos(), &dir, item);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn set_draft_sections(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    sections: Vec<SectionDto>,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = set_sections(&state.cycle_repos(), &dir, sections);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn set_report_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    mode: String,
    template: Option<String>,
    meeting_type: Option<String>,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let repos = state.cycle_repos();
    let view = state.with_store_ipc(|s| {
        change_report_mode(&repos, s, &meeting_id, &dir, &mode, template, meeting_type)
    });
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn accept_new_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = accept_new(&state.cycle_repos(), &dir);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn discard_new_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = discard_new(&state.cycle_repos(), &dir);
    after(&app, &meeting_id, view)
}

/// Approves or rejects a status change of an open issue or topic, with the text to post
/// (`None` keeps it, an empty text clears it). Nothing is written back before approval, and
/// nothing approves one by itself.
#[tauri::command]
pub fn set_status_decision(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    proposal_id: String,
    decision: Decision,
    comment: Option<String>,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = set_decision(&state.cycle_repos(), &dir, &proposal_id, decision, comment);
    if let Ok(ref v) = view {
        if let Some(p) = v
            .draft
            .status_proposals
            .iter()
            .find(|p| p.id == proposal_id)
        {
            let now = cycle_store::time::now_unix_ms() as i64;
            if p.decision == Decision::Approved && p.status == ProposedStatus::Done {
                let _ = state.with_store(|s| {
                    s.set_task_done(&v.draft.cycle, &p.target, true, Some(&v.draft.date), now)
                });
            } else {
                let was_closed_here = state
                    .with_store(|s| s.list_task_completions(&v.draft.cycle))
                    .ok()
                    .is_some_and(|list| {
                        list.iter().any(|c| {
                            c.task_key == p.target
                                && c.closed_by_meeting.as_deref() == Some(v.draft.date.as_str())
                        })
                    });
                if was_closed_here {
                    let _ = state.with_store(|s| {
                        s.set_task_done(&v.draft.cycle, &p.target, false, None, now)
                    });
                }
            }
        }
    }
    after(&app, &meeting_id, view)
}

/// Adds a status change by hand; it waits for approval like the others.
#[tauri::command]
pub fn add_status_proposal(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    target: String,
    status: ProposedStatus,
    comment: Option<String>,
) -> IpcResult<DraftView> {
    let dir = meeting_dir(&state, &meeting_id)?;
    idle(&state, &meeting_id)?;
    let view = add_proposal(&state.cycle_repos(), &dir, &target, status, comment);
    after(&app, &meeting_id, view)
}

#[tauri::command]
pub fn render_report(
    state: State<'_, AppState>,
    meeting_id: String,
    variant: String,
) -> IpcResult<String> {
    render_variant(
        &state.cycle_repos(),
        &meeting_dir(&state, &meeting_id)?,
        &variant,
    )
}

// ----- tests -------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycles::tests::Lab;
    use crate::cycles::{
        get_cycle, save_cycle, save_template, CycleParticipantDto, TemplateDto, TemplateSectionDto,
    };
    use cycle_store::CertaintyLevel;
    use std::fs;

    const FIXTURE: &str = include_str!("../../../worker/tests/fixtures/meeting.valid.json");

    /// A lab whose cycle is TEAM (as in the fixture) with participant `alex` and a template.
    fn lab_with_draft() -> (Lab, std::path::PathBuf) {
        let lab = Lab::new();
        cycles::create_cycle(lab.repos(), &lab.settings.cycle_repos[0], "TEAM", "Team").unwrap();
        let mut dto = get_cycle(lab.repos(), "TEAM").unwrap().cycle;
        dto.participants = vec![
            CycleParticipantDto {
                id: "alex".into(),
                name: "Alex Example".into(),
                github: None,
                discord: None,
            },
            CycleParticipantDto {
                id: "sam".into(),
                name: "Sam Sample".into(),
                github: None,
                discord: None,
            },
        ];
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
        save_template(
            lab.repos(),
            "TEAM",
            &TemplateDto {
                name: "weekly".into(),
                sections: vec![TemplateSectionDto {
                    id: "hardware".into(),
                    title: "Hardware".into(),
                    description: String::new(),
                }],
                body: "# {title}\n\n{section:hardware}\n".into(),
            },
        )
        .unwrap();
        lab.meeting("20261116-100000", Some("TEAM"));
        let dir = lab.dir.path().join("recordings").join("20261116-100000");
        fs::create_dir_all(dir.join("work")).unwrap();
        fs::write(dir.join("work").join("draft.json"), FIXTURE).unwrap();
        (lab, dir)
    }

    fn id_order(v: &DraftView) -> Vec<&str> {
        v.draft.items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn there_is_no_draft_before_reduce_has_run() {
        let lab = Lab::new();
        let dir = lab.dir.path().join("m");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_view(lab.repos(), &dir).unwrap_err().code, "no_draft");
        assert_eq!(
            update_item(lab.repos(), &dir, "i001", ItemPatchDto::default())
                .unwrap_err()
                .code,
            "no_draft"
        );
        assert_eq!(
            render_variant(lab.repos(), &dir, "markdown")
                .unwrap_err()
                .code,
            "no_draft"
        );
        assert_eq!(
            accept_new(lab.repos(), &dir).unwrap_err().code,
            "no_new_draft"
        );
    }

    #[test]
    fn the_view_has_the_draft_the_report_the_participants_and_the_flags() {
        let (lab, dir) = lab_with_draft();
        let v = load_view(lab.repos(), &dir).unwrap();
        assert_eq!(v.draft.cycle, "TEAM");
        assert_eq!(v.draft.items.len(), 2);
        assert_eq!(v.draft.sections[0].id, "hardware");
        assert!(!v.draft.items[0].source.as_ref().unwrap().quote.is_empty());
        assert_eq!(v.participants.len(), 2);
        assert_eq!(
            v.participants[0],
            DraftParticipant {
                id: "alex".into(),
                name: "Alex Example".into()
            }
        );
        assert!(!v.has_new_draft);
        assert!(!v.transcript_present);
        assert_eq!(v.report_warning, None);
        // Rendered with the cycle's template layout: the task shows the assignee's name.
        assert!(v
            .report_markdown
            .contains("Prepare the monitor mount design"));
        assert!(v.report_markdown.contains("Alex Example"));
        assert!(v.report_markdown.contains("Summary"));

        fs::write(dir.join("work").join("transcript.json"), "{}").unwrap();
        fs::write(dir.join("work").join("draft.new.json"), FIXTURE).unwrap();
        let v = load_view(lab.repos(), &dir).unwrap();
        assert!(v.has_new_draft && v.transcript_present);

        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["draft"]["meetingId"], "20261116-100000");
        assert_eq!(json["draft"]["items"][0]["relatesTo"], "#12");
        assert_eq!(json["draft"]["items"][0]["source"]["tStartMs"], 754000);
        assert_eq!(json["draft"]["items"][0]["source"]["match"], 1.0);
        assert_eq!(json["draft"]["items"][0]["certainty"]["due"], "inferred");
        assert_eq!(json["draft"]["items"][0]["type"], "task");
        assert_eq!(json["draft"]["warnings"][0]["code"], "quote_unverified");
        assert_eq!(json["hasNewDraft"], true);
    }

    #[test]
    fn the_report_is_localized_by_the_language_of_the_draft() {
        let (lab, dir) = lab_with_draft();
        let path = dir.join("work").join("draft.json");
        let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        value["language"] = "pl".into();
        value["mode"] = "fixed".into();
        value["sections"] = serde_json::json!([]);
        value["items"][0]["section"] = serde_json::Value::Null;
        fs::write(&path, value.to_string()).unwrap();
        let v = load_view(lab.repos(), &dir).unwrap();
        // The built-in default template in Polish uses the Polish headings.
        assert!(
            v.report_markdown.contains("Zadania"),
            "{}",
            v.report_markdown
        );
        assert!(
            v.report_markdown.contains("Spotkanie"),
            "{}",
            v.report_markdown
        );
    }

    #[test]
    fn a_template_that_cannot_render_falls_back_with_a_warning() {
        let (lab, dir) = lab_with_draft();
        let tpl = lab
            .dir
            .path()
            .join(cycles::DEFAULT_REPO_DIR)
            .join("cycles/TEAM/templates/default.md");
        fs::write(&tpl, "# {title}\n{nonsense}\n").unwrap();
        // The draft's template resolves to the cycle default.
        let v = load_view(lab.repos(), &dir).unwrap();
        assert!(v.report_warning.as_deref().unwrap().contains("default"));
        assert!(v
            .report_markdown
            .contains("Prepare the monitor mount design"));

        // Without the cycle in any repository the built-in layout is used and ids stay raw.
        let v = load_view(&[], &dir).unwrap();
        assert!(v.participants.is_empty());
        assert!(v
            .report_markdown
            .contains("Prepare the monitor mount design"));
    }

    #[test]
    fn editing_an_item_saves_it_marks_it_and_returns_the_new_view() {
        let (lab, dir) = lab_with_draft();
        let v = update_item_checked(
            lab.repos(),
            &dir,
            "i001",
            ItemPatchDto {
                text: Some("  Prepare the mount  ".into()),
                assignee: Some(Some("sam".into())),
                due: Some(None),
            },
        )
        .unwrap();
        let item = &v.draft.items[0];
        assert_eq!(item.text, "Prepare the mount");
        assert_eq!(item.assignee.as_deref(), Some("sam"));
        assert_eq!(item.due, None);
        assert!(item.edited);
        assert_eq!(item.certainty.assignee, CertaintyLevel::Stated);
        assert_eq!(item.certainty.due, CertaintyLevel::Unknown);
        assert!(v.report_markdown.contains("Sam Sample"));
        // It is on disk.
        let again = load_view(lab.repos(), &dir).unwrap();
        assert_eq!(again.draft.items[0].text, "Prepare the mount");

        // Failures change nothing.
        for bad in [
            ItemPatchDto {
                text: Some(" ".into()),
                ..Default::default()
            },
            ItemPatchDto {
                due: Some(Some("tomorrow".into())),
                ..Default::default()
            },
            ItemPatchDto {
                assignee: Some(Some("ghost".into())),
                ..Default::default()
            },
        ] {
            assert_eq!(
                update_item_checked(lab.repos(), &dir, "i001", bad)
                    .unwrap_err()
                    .code,
                "invalid"
            );
        }
        assert_eq!(
            update_item_checked(lab.repos(), &dir, "nope", ItemPatchDto::default())
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(load_view(lab.repos(), &dir).unwrap(), again);
    }

    #[test]
    fn the_patch_tells_an_absent_field_from_a_cleared_one() {
        let p: ItemPatchDto = serde_json::from_str(r#"{"text":"x"}"#).unwrap();
        assert_eq!((p.assignee, p.due), (None, None));
        let p: ItemPatchDto =
            serde_json::from_str(r#"{"assignee":null,"due":"2026-12-01"}"#).unwrap();
        assert_eq!(p.assignee, Some(None));
        assert_eq!(p.due, Some(Some("2026-12-01".into())));
    }

    #[test]
    fn items_move_between_sections_and_order() {
        let (lab, dir) = lab_with_draft();
        // i002 has no section; move it into `hardware`, before i001.
        let v = move_item(lab.repos(), &dir, "i002", Some("hardware"), Some("i001")).unwrap();
        assert_eq!(id_order(&v), ["i002", "i001"]);
        assert_eq!(v.draft.items[0].section.as_deref(), Some("hardware"));
        assert!(v.draft.items[0].edited);
        // Out of every section, last.
        let v = move_item(lab.repos(), &dir, "i002", None, None).unwrap();
        assert_eq!(id_order(&v), ["i001", "i002"]);
        assert_eq!(v.draft.items[1].section, None);
        for (item, section, before, code) in [
            ("i001", Some("ghost"), None, "not_found"),
            ("ghost", None, None, "not_found"),
            ("i001", Some("hardware"), Some("i001"), "invalid"),
            ("i001", Some("hardware"), Some("i002"), "invalid"),
        ] {
            assert_eq!(
                move_item(lab.repos(), &dir, item, section, before)
                    .unwrap_err()
                    .code,
                code
            );
        }
    }

    #[test]
    fn items_are_added_and_deleted() {
        let (lab, dir) = lab_with_draft();
        let v = add_item(
            lab.repos(),
            &dir,
            NewItemDto {
                item_type: ItemType::Task,
                text: "Order the battery".into(),
                assignee: Some("alex".into()),
                due: Some("2026-12-01".into()),
                section: Some("hardware".into()),
            },
        )
        .unwrap();
        let added = v.draft.items.last().unwrap();
        assert_eq!(added.id, "u001");
        assert!(added.edited && added.source.is_none());
        assert_eq!(added.certainty.assignee, CertaintyLevel::Stated);
        assert!(v.report_markdown.contains("Order the battery"));

        let bad = |item: NewItemDto| add_item(lab.repos(), &dir, item).unwrap_err().code;
        let base = NewItemDto {
            item_type: ItemType::Note,
            text: "x".into(),
            assignee: None,
            due: None,
            section: None,
        };
        assert_eq!(
            bad(NewItemDto {
                text: "".into(),
                ..base.clone()
            }),
            "invalid"
        );
        assert_eq!(
            bad(NewItemDto {
                assignee: Some("ghost".into()),
                ..base.clone()
            }),
            "invalid"
        );
        assert_eq!(
            bad(NewItemDto {
                due: Some("soon".into()),
                ..base.clone()
            }),
            "invalid"
        );
        assert_eq!(
            bad(NewItemDto {
                section: Some("ghost".into()),
                ..base
            }),
            "not_found"
        );

        let v = delete_item(lab.repos(), &dir, "i001").unwrap();
        assert_eq!(v.draft.items.len(), 2);
        assert!(!v
            .report_markdown
            .contains("Prepare the monitor mount design"));
        assert_eq!(
            delete_item(lab.repos(), &dir, "i001").unwrap_err().code,
            "not_found"
        );
    }

    #[test]
    fn sections_are_renamed_added_removed_and_reordered() {
        let (lab, dir) = lab_with_draft();
        let v = set_sections(
            lab.repos(),
            &dir,
            vec![
                SectionDto {
                    id: String::new(),
                    title: "Open questions".into(),
                    description: String::new(),
                },
                SectionDto {
                    id: "hardware".into(),
                    title: "Mechanics".into(),
                    description: "x".into(),
                },
                SectionDto {
                    id: String::new(),
                    title: "Open questions".into(),
                    description: String::new(),
                },
            ],
        )
        .unwrap();
        let ids: Vec<&str> = v.draft.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["open-questions", "hardware", "open-questions-2"]);
        assert_eq!(v.draft.sections[1].title, "Mechanics");
        assert_eq!(v.draft.items[0].section.as_deref(), Some("hardware"));

        // Removing a section frees its items (and marks them).
        let v = set_sections(
            lab.repos(),
            &dir,
            vec![SectionDto {
                id: "open-questions".into(),
                title: "Q".into(),
                description: String::new(),
            }],
        )
        .unwrap();
        assert_eq!(v.draft.items[0].section, None);
        assert!(v.draft.items[0].edited);

        for sections in [
            vec![SectionDto {
                id: "A B".into(),
                title: "x".into(),
                description: String::new(),
            }],
            vec![SectionDto {
                id: "a".into(),
                title: " ".into(),
                description: String::new(),
            }],
            vec![
                SectionDto {
                    id: "a".into(),
                    title: "x".into(),
                    description: String::new(),
                },
                SectionDto {
                    id: "a".into(),
                    title: "y".into(),
                    description: String::new(),
                },
            ],
        ] {
            assert_eq!(
                set_sections(lab.repos(), &dir, sections).unwrap_err().code,
                "invalid"
            );
        }
    }

    #[test]
    fn the_mode_changes_without_touching_any_item() {
        let (lab, dir) = lab_with_draft();
        let id = "20261116-100000";
        let before = load_view(lab.repos(), &dir).unwrap();

        let v = change_report_mode(
            lab.repos(),
            &lab.store,
            id,
            &dir,
            "template",
            Some("weekly".into()),
            None,
        )
        .unwrap();
        assert_eq!(v.draft.mode, ReportMode::Template);
        assert_eq!(v.draft.template.as_deref(), Some("weekly"));
        assert_eq!(v.draft.items, before.draft.items);
        assert_eq!(v.draft.sections, before.draft.sections);
        let stored = lab.store.get_report_settings(id).unwrap().unwrap();
        assert_eq!(stored.report_mode.as_deref(), Some("template"));
        assert_eq!(stored.report_template.as_deref(), Some("weekly"));
        // The cycle is kept.
        assert_eq!(stored.cycle_id.as_deref(), Some("TEAM"));

        let v = change_report_mode(lab.repos(), &lab.store, id, &dir, "fixed", None, None).unwrap();
        assert_eq!(v.draft.mode, ReportMode::Fixed);
        assert_eq!(
            lab.store
                .get_report_settings(id)
                .unwrap()
                .unwrap()
                .report_template,
            None
        );

        // By type: the cycle must define the type; the layout is the mapped template.
        let mut cycle = get_cycle(lab.repos(), "TEAM").unwrap().cycle;
        cycle
            .report
            .by_type
            .insert("budget".into(), "weekly".into());
        assert!(save_cycle(lab.repos(), &lab.store, cycle).unwrap().saved);
        let v = change_report_mode(
            lab.repos(),
            &lab.store,
            id,
            &dir,
            "by-type",
            None,
            Some("budget".into()),
        )
        .unwrap();
        assert_eq!(v.draft.mode, ReportMode::ByType);
        assert_eq!(v.draft.meeting_type.as_deref(), Some("budget"));
        assert_eq!(v.draft.template.as_deref(), Some("weekly"));
        assert_eq!(
            lab.store
                .get_report_settings(id)
                .unwrap()
                .unwrap()
                .meeting_type
                .as_deref(),
            Some("budget")
        );
    }

    #[test]
    fn a_mode_that_does_not_fit_the_cycle_is_refused_and_changes_nothing() {
        let (lab, dir) = lab_with_draft();
        let id = "20261116-100000";
        let before = load_view(lab.repos(), &dir).unwrap();
        let stored = lab.store.get_report_settings(id).unwrap();
        for (mode, template, kind) in [
            ("template", Some("ghost"), None),
            ("by-type", None, None),
            ("by-type", None, Some("ghost")),
            ("freeform", None, None),
        ] {
            let e = change_report_mode(
                lab.repos(),
                &lab.store,
                id,
                &dir,
                mode,
                template.map(String::from),
                kind.map(String::from),
            )
            .unwrap_err();
            assert_eq!(e.code, "report_mode_required", "{mode}");
        }
        assert_eq!(load_view(lab.repos(), &dir).unwrap(), before);
        assert_eq!(lab.store.get_report_settings(id).unwrap(), stored);
    }

    #[test]
    fn a_newer_draft_is_accepted_or_discarded() {
        let (lab, dir) = lab_with_draft();
        update_item(
            lab.repos(),
            &dir,
            "i001",
            ItemPatchDto {
                text: Some("Edited by hand".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(DraftFiles::new(&dir).has_user_edits().unwrap());
        fs::write(dir.join("work").join("draft.new.json"), FIXTURE).unwrap();

        let v = discard_new(lab.repos(), &dir).unwrap();
        assert!(!v.has_new_draft);
        assert_eq!(v.draft.items[0].text, "Edited by hand");
        assert!(!dir.join("work").join("draft.new.json").exists());

        fs::write(dir.join("work").join("draft.new.json"), FIXTURE).unwrap();
        let v = accept_new(lab.repos(), &dir).unwrap();
        assert!(!v.has_new_draft);
        assert_eq!(v.draft.items[0].text, "Prepare the monitor mount design");
        assert_eq!(
            accept_new(lab.repos(), &dir).unwrap_err().code,
            "no_new_draft"
        );
    }

    #[test]
    fn variants_render_checkboxes_or_plain_bullets() {
        let (lab, dir) = lab_with_draft();
        let md = render_variant(lab.repos(), &dir, "markdown").unwrap();
        let chat = render_variant(lab.repos(), &dir, "chat").unwrap();
        assert!(
            md.contains("- [ ] Prepare the monitor mount design"),
            "{md}"
        );
        assert!(!chat.contains("[ ]"), "{chat}");
        assert!(chat.contains("Prepare the monitor mount design"));
        assert_eq!(
            render_variant(lab.repos(), &dir, "html").unwrap_err().code,
            "invalid"
        );
    }

    #[test]
    fn the_draft_cannot_be_edited_while_extract_or_reduce_runs() {
        let (lab, _dir) = lab_with_draft();
        let names: Vec<&str> = queue::STAGES.iter().map(|s| s.as_str()).collect();
        // No job: nothing runs.
        ensure_draft_idle(&lab.store, "m1").unwrap();
        lab.meeting("m1", Some("TEAM"));
        ensure_draft_idle(&lab.store, "m1").unwrap();
        lab.store.create_job("m1", &names, "{}", 1).unwrap();
        ensure_draft_idle(&lab.store, "m1").unwrap();

        for running in ["extract", "reduce"] {
            lab.store
                .set_stage_status("m1", running, store::StageStatus::Running, None, 2)
                .unwrap();
            let e = ensure_draft_idle(&lab.store, "m1").unwrap_err();
            assert_eq!(e.code, "busy", "{running}");
            lab.store
                .set_stage_status("m1", running, store::StageStatus::Done, None, 3)
                .unwrap();
            ensure_draft_idle(&lab.store, "m1").unwrap();
        }
        // Other stages running (the speaker stage, for instance) do not block the editor.
        lab.store
            .set_stage_status("m1", "speaker_id", store::StageStatus::Running, None, 4)
            .unwrap();
        ensure_draft_idle(&lab.store, "m1").unwrap();
    }

    #[test]
    fn a_broken_draft_file_is_reported_not_overwritten() {
        let (lab, dir) = lab_with_draft();
        let path = dir.join("work").join("draft.json");
        fs::write(&path, r#"{"version":1}"#).unwrap();
        assert_eq!(
            load_view(lab.repos(), &dir).unwrap_err().code,
            "draft_invalid"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"version":1}"#);
    }
}
