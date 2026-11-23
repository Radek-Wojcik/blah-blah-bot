//! IPC commands for processing: queue, transcript and its edits, settings, models.

use std::path::PathBuf;

use queue::transcript::{SegmentEdit, TranscriptError, TranscriptFiles};
use queue::worker::JobOptions;
use serde::{Deserialize, Serialize};
use store::LlmKind;
use tauri::ipc::Response;
use tauri::State;

use crate::ipc::{IpcError, IpcResult};
use crate::llm::{self, CloudPreview};
use crate::processing::{ModelStatus, ProcessingSettings, QueueState};
use crate::secrets;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOptions {
    pub language: Option<String>,
    pub max_speakers: Option<u32>,
    /// The language-model backend for this meeting; absent means the default of the
    /// settings.
    #[serde(default)]
    pub llm: Option<LlmRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequest {
    /// `managed`, `external` or `cloud`.
    pub kind: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSpeakerDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegmentDto {
    pub id: u32,
    pub track: String,
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub original_text: Option<String>,
    pub excluded: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotDescriptionDto {
    pub file: String,
    pub offset_ms: u64,
    pub description: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptData {
    pub meeting_id: String,
    pub language: String,
    pub speakers: Vec<TranscriptSpeakerDto>,
    pub segments: Vec<TranscriptSegmentDto>,
    pub screenshots: Vec<ScreenshotDescriptionDto>,
}

#[derive(Debug, Deserialize)]
pub struct EditPayload {
    pub text: Option<String>,
    pub excluded: bool,
}

pub(crate) fn meeting_dir(state: &AppState, id: &str) -> IpcResult<PathBuf> {
    let details = state
        .with_store(|s| s.get_meeting(id))?
        .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
    Ok(state.recordings_root()?.join(details.meeting.dir))
}

fn transcript_error(e: TranscriptError) -> IpcError {
    match e {
        TranscriptError::NotReady => IpcError::new("not_ready", e.to_string()),
        TranscriptError::NoSegment(_) => IpcError::new("not_found", e.to_string()),
        TranscriptError::Io(_) => IpcError::new("io", e.to_string()),
        TranscriptError::Json(_) => IpcError::new("internal", e.to_string()),
    }
}

#[tauri::command]
pub fn process_meeting(
    state: State<'_, AppState>,
    id: String,
    options: Option<ProcessOptions>,
) -> IpcResult<()> {
    // A published meeting keeps the draft that was published: running the pipeline again
    // would replace it.
    state.with_store_ipc(|s| crate::draft_commands::ensure_draft_unlocked(s, &id))?;
    let mut job = JobOptions::default();
    let mut requested = None;
    if let Some(o) = options {
        if let Some(lang) = o.language {
            if !["auto", "pl", "en"].contains(&lang.as_str()) {
                return Err(IpcError::new("invalid", "unsupported language"));
            }
            job.language = lang;
        }
        job.max_speakers = o.max_speakers.filter(|n| *n > 0);
        requested = o.llm.map(|l| llm::parse_kind(&l.kind)).transpose()?;
    }
    // The backend is recorded for the meeting; the cloud needs its key and the explicit
    // confirmation given earlier (`cloud_not_confirmed` otherwise).
    let settings = state.processing().llm_settings()?;
    job.llm = state.with_store_ipc(|s| {
        llm::choose_llm(s, &id, &settings, requested, &|name| {
            secrets::get(name).is_some()
        })
    })?;
    state.processing().process(&id, job)
}

#[tauri::command]
pub fn queue_state(state: State<'_, AppState>) -> IpcResult<QueueState> {
    state.processing().queue_state()
}

#[tauri::command]
pub fn retry_job(state: State<'_, AppState>, id: String) -> IpcResult<()> {
    state.with_store_ipc(|s| crate::draft_commands::ensure_draft_unlocked(s, &id))?;
    state.processing().retry(&id)
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, id: String) -> IpcResult<()> {
    state.processing().cancel(&id)
}

#[tauri::command]
pub fn get_transcript(state: State<'_, AppState>, id: String) -> IpcResult<TranscriptData> {
    let dir = meeting_dir(&state, &id)?;
    let files = TranscriptFiles::new(&dir);
    let mut view = files.view().map_err(transcript_error)?;
    // Names the user (or an automatic match) gave to voices replace the generic labels.
    let (voices, participants) = state.with_store(|s| {
        Ok((
            s.list_meeting_voices(&id)?,
            queue::voices::participants_for_meeting(s, &id)?,
        ))
    })?;
    let names = queue::voices::speaker_names(&voices, &participants);
    queue::voices::apply_names(&mut view, &voices, &names);
    let screenshots = std::fs::read(dir.join("work").join("descriptions.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("items").cloned())
        .and_then(|items| items.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| {
            Some(ScreenshotDescriptionDto {
                file: i.get("file")?.as_str()?.to_string(),
                offset_ms: i.get("offset_ms")?.as_u64()?,
                description: i.get("description")?.as_str()?.to_string(),
            })
        })
        .collect();
    Ok(TranscriptData {
        meeting_id: view.meeting_id,
        language: view.language,
        speakers: view
            .speakers
            .into_iter()
            .map(|s| TranscriptSpeakerDto {
                id: s.id,
                label: s.label,
                kind: s.kind,
                track: s.track,
            })
            .collect(),
        segments: view
            .segments
            .into_iter()
            .map(|s| TranscriptSegmentDto {
                id: s.id,
                track: s.track,
                speaker: s.speaker,
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                text: s.text,
                original_text: s.original_text,
                excluded: s.excluded,
            })
            .collect(),
        screenshots,
    })
}

#[tauri::command]
pub fn edit_segment(
    state: State<'_, AppState>,
    id: String,
    segment_id: u32,
    edit: EditPayload,
) -> IpcResult<()> {
    let dir = meeting_dir(&state, &id)?;
    TranscriptFiles::new(&dir)
        .set_edit(
            segment_id,
            SegmentEdit {
                text: edit.text,
                excluded: edit.excluded,
            },
        )
        .map_err(transcript_error)
}

/// A file of the meeting (a screenshot) as bytes. Only paths inside the meeting folder are
/// served.
#[tauri::command]
pub fn meeting_image(state: State<'_, AppState>, id: String, file: String) -> IpcResult<Response> {
    let rel = std::path::Path::new(&file);
    let safe = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !safe || rel.extension().and_then(|e| e.to_str()) != Some("png") {
        return Err(IpcError::new("invalid", "not a meeting image"));
    }
    let bytes = std::fs::read(meeting_dir(&state, &id)?.join(rel))?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub fn get_processing_settings(state: State<'_, AppState>) -> IpcResult<ProcessingSettings> {
    state.processing().settings()
}

#[tauri::command]
pub fn save_processing_settings(
    state: State<'_, AppState>,
    settings: ProcessingSettings,
) -> IpcResult<()> {
    state.processing().save_settings(settings)
}

#[tauri::command]
pub fn set_hf_token(token: String) -> IpcResult<()> {
    secrets::set(secrets::HF_TOKEN, &token)
}

#[tauri::command]
pub fn clear_hf_token() -> IpcResult<()> {
    secrets::clear(secrets::HF_TOKEN)
}

/// The credential entry of the key of `kind` for the address in the settings. A key belongs
/// to one host: it is saved under a name with the host and is only found again for that
/// host. `llm_not_configured` while the address is not set (save the settings first).
fn key_entry(state: &AppState, kind: LlmKind) -> IpcResult<String> {
    state
        .processing()
        .llm_settings()?
        .key_entry(kind)
        .ok_or_else(|| {
            IpcError::new(
                "llm_not_configured",
                "save the address of the server in the settings first: a key is stored for that server only",
            )
        })
}

#[tauri::command]
pub fn set_cloud_key(state: State<'_, AppState>, key: String) -> IpcResult<()> {
    secrets::set(&key_entry(&state, LlmKind::Cloud)?, &key)
}

#[tauri::command]
pub fn clear_cloud_key(state: State<'_, AppState>) -> IpcResult<()> {
    match key_entry(&state, LlmKind::Cloud) {
        Ok(entry) => secrets::clear(&entry),
        Err(_) => Ok(()),
    }
}

/// Whether a cloud key is stored for the current cloud address. The key itself is never
/// returned.
#[tauri::command]
pub fn cloud_key_present(state: State<'_, AppState>) -> bool {
    key_entry(&state, LlmKind::Cloud).is_ok_and(|e| secrets::get(&e).is_some())
}

#[tauri::command]
pub fn set_external_key(state: State<'_, AppState>, key: String) -> IpcResult<()> {
    secrets::set(&key_entry(&state, LlmKind::External)?, &key)
}

#[tauri::command]
pub fn clear_external_key(state: State<'_, AppState>) -> IpcResult<()> {
    match key_entry(&state, LlmKind::External) {
        Ok(entry) => secrets::clear(&entry),
        Err(_) => Ok(()),
    }
}

#[tauri::command]
pub fn external_key_present(state: State<'_, AppState>) -> bool {
    key_entry(&state, LlmKind::External).is_ok_and(|e| secrets::get(&e).is_some())
}

/// What would be sent to the cloud endpoint for this meeting, for the confirmation dialog.
/// Reads only: it confirms nothing.
#[tauri::command]
pub fn cloud_preview(state: State<'_, AppState>, meeting_id: String) -> IpcResult<CloudPreview> {
    let dir = meeting_dir(&state, &meeting_id)?;
    let settings = state.processing().llm_settings()?;
    let cycle = state
        .with_store(|s| s.get_report_settings(&meeting_id))?
        .and_then(|r| r.cycle_id);
    let previous = cycle
        .map(|c| crate::cycles::previous_meeting_dirs(&state.cycle_repos(), &c))
        .unwrap_or_default();
    state.with_store_ipc(|s| llm::cloud_preview(s, &settings, &dir, &meeting_id, &previous))
}

/// Records that the user confirmed sending this meeting to the cloud endpoint. Called only
/// from the dialog's confirm button; processing never confirms by itself.
#[tauri::command]
pub fn confirm_cloud(state: State<'_, AppState>, meeting_id: String) -> IpcResult<()> {
    let settings = state.processing().llm_settings()?;
    state.with_store_ipc(|s| llm::confirm_cloud(s, &settings, &meeting_id, queue::now_ms()))
}

/// Async: asking the worker which models are cached starts a process.
#[tauri::command]
pub async fn model_status(state: State<'_, AppState>) -> IpcResult<Vec<ModelStatus>> {
    state.processing().model_status()
}

#[tauri::command]
pub fn download_models(state: State<'_, AppState>, keys: Vec<String>) -> IpcResult<()> {
    state.processing().download_models(keys)
}
