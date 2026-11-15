//! IPC commands for speakers: detected voices, naming, merging and voice profiles.
//!
//! Voice profiles, embeddings and sample clips are biometric data. They are kept in the local
//! database and recordings folder only; no command here sends them anywhere.

use std::sync::atomic::{AtomicU64, Ordering};

use queue::voices;
use serde::Serialize;
use store::{ParticipantInfo, VoiceDecision, VoiceRow};
use tauri::ipc::Response;
use tauri::State;

use crate::ipc::{IpcError, IpcResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipDto {
    pub idx: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceDto {
    pub voice_id: String,
    pub track: String,
    pub label: String,
    pub speech_ms: u64,
    pub decision: VoiceDecision,
    pub score: Option<f32>,
    /// The participant the matcher found, for a suggestion.
    pub suggested_participant_id: Option<String>,
    /// The participant the voice is named after (automatic match or the user's choice).
    pub assigned_participant_id: Option<String>,
    pub confirmed: bool,
    pub merged_into: Option<String>,
    pub clips: Vec<ClipDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantDto {
    pub id: String,
    pub name: String,
    pub sample_count: u32,
    pub speech_ms: u64,
}

impl From<ParticipantInfo> for ParticipantDto {
    fn from(p: ParticipantInfo) -> Self {
        Self {
            id: p.id,
            name: p.name,
            sample_count: p.sample_count,
            speech_ms: p.speech_ms,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicesData {
    pub meeting_id: String,
    pub voices: Vec<VoiceDto>,
    pub participants: Vec<ParticipantDto>,
    pub all_confirmed: bool,
}

fn to_dto(v: VoiceRow, clips: &[store::ClipRow]) -> VoiceDto {
    VoiceDto {
        clips: clips
            .iter()
            .filter(|c| c.voice_id == v.voice_id)
            .map(|c| ClipDto {
                idx: c.idx,
                start_ms: c.start_ms,
                end_ms: c.end_ms,
                text: c.text.clone(),
            })
            .collect(),
        suggested_participant_id: (v.decision == VoiceDecision::Suggested)
            .then(|| v.matched_participant.clone())
            .flatten(),
        voice_id: v.voice_id,
        track: v.track,
        label: v.label,
        speech_ms: v.speech_ms,
        decision: v.decision,
        score: v.score,
        assigned_participant_id: v.assigned_participant,
        confirmed: v.confirmed,
        merged_into: v.merged_into,
    }
}

fn voices_data(state: &AppState, id: &str) -> IpcResult<VoicesData> {
    let (voices, clips, participants) = state.with_store(|s| {
        Ok((
            s.list_meeting_voices(id)?,
            s.list_clips(id)?,
            s.list_participants()?,
        ))
    })?;
    let all_confirmed = voices::all_voices_confirmed(&voices);
    // The labels of the transcript ("Speaker 1") are shown as they are; the screen lists voices
    // by their order of appearance, which is the diarization label order.
    Ok(VoicesData {
        meeting_id: id.to_string(),
        voices: voices.into_iter().map(|v| to_dto(v, &clips)).collect(),
        participants: participants.into_iter().map(Into::into).collect(),
        all_confirmed,
    })
}

/// After a naming decision: drop the clips of a meeting whose voices are all confirmed.
fn after_change(state: &AppState) -> IpcResult<()> {
    let root = state.recordings_root()?;
    let days = state.processing().settings()?.clip_max_days;
    state.with_store(|s| voices::sweep_clips(s, &root, days, queue::now_ms()).map(|_| ()))
}

fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "p{:x}{:x}",
        queue::now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn clean_name(name: &str) -> IpcResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(IpcError::new(
            "invalid",
            "enter a name of 1 to 80 characters",
        ));
    }
    Ok(name.to_string())
}

#[tauri::command]
pub fn get_meeting_voices(state: State<'_, AppState>, id: String) -> IpcResult<VoicesData> {
    voices_data(&state, &id)
}

#[tauri::command]
pub fn assign_voice(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    participant_id: Option<String>,
) -> IpcResult<VoicesData> {
    let ok = state.with_store(|s| {
        voices::assign_voice(
            s,
            &id,
            &voice_id,
            participant_id.as_deref(),
            queue::now_ms(),
        )
    })?;
    if !ok {
        return Err(IpcError::new("not_found", "voice not found"));
    }
    after_change(&state)?;
    voices_data(&state, &id)
}

/// Name a voice after a new participant (or an existing one with the same name).
#[tauri::command]
pub fn assign_voice_new(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    name: String,
) -> IpcResult<VoicesData> {
    let name = clean_name(&name)?;
    let ok = state.with_store(|s| {
        let participant = match s.find_participant_by_name(&name)? {
            Some(p) => p.id,
            None => {
                let pid = new_id();
                s.create_participant(&pid, &name, queue::now_ms())?;
                pid
            }
        };
        voices::assign_voice(s, &id, &voice_id, Some(&participant), queue::now_ms())
    })?;
    if !ok {
        return Err(IpcError::new("not_found", "voice not found"));
    }
    after_change(&state)?;
    voices_data(&state, &id)
}

#[tauri::command]
pub fn merge_voices(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    into: Option<String>,
) -> IpcResult<VoicesData> {
    let ok = state
        .with_store(|s| voices::merge_voice(s, &id, &voice_id, into.as_deref(), queue::now_ms()))?;
    if !ok {
        return Err(IpcError::new(
            "invalid",
            "these voices cannot be merged (unknown voice, itself, or a loop)",
        ));
    }
    after_change(&state)?;
    voices_data(&state, &id)
}

/// Accept all automatic matches of a meeting.
#[tauri::command]
pub fn confirm_matches(state: State<'_, AppState>, id: String) -> IpcResult<VoicesData> {
    state.with_store(|s| voices::confirm_automatic(s, &id, queue::now_ms()).map(|_| ()))?;
    after_change(&state)?;
    voices_data(&state, &id)
}

/// A sample clip as WAV bytes. Only clips of this meeting's rows are served.
#[tauri::command]
pub fn voice_clip(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    idx: u32,
) -> IpcResult<Response> {
    let clip = state
        .with_store(|s| s.list_clips(&id))?
        .into_iter()
        .find(|c| c.voice_id == voice_id && c.idx == idx)
        .ok_or_else(|| IpcError::new("not_found", "clip not found"))?;
    let bytes = std::fs::read(state.recordings_root()?.join(clip.file))?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub fn list_participants(state: State<'_, AppState>) -> IpcResult<Vec<ParticipantDto>> {
    Ok(state
        .with_store(|s| s.list_participants())?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub fn create_participant(state: State<'_, AppState>, name: String) -> IpcResult<ParticipantDto> {
    let name = clean_name(&name)?;
    let id = new_id();
    state
        .with_store(|s| s.create_participant(&id, &name, queue::now_ms()))
        .map_err(|_| IpcError::new("duplicate", "a participant with this name already exists"))?;
    Ok(ParticipantDto {
        id,
        name,
        sample_count: 0,
        speech_ms: 0,
    })
}

#[tauri::command]
pub fn rename_participant(
    state: State<'_, AppState>,
    participant_id: String,
    name: String,
) -> IpcResult<()> {
    let name = clean_name(&name)?;
    let renamed = state
        .with_store(|s| s.rename_participant(&participant_id, &name))
        .map_err(|_| IpcError::new("duplicate", "a participant with this name already exists"))?;
    if renamed {
        Ok(())
    } else {
        Err(IpcError::new("not_found", "participant not found"))
    }
}

/// Delete a participant together with the voice profile. Voices named after them become
/// unnamed.
#[tauri::command]
pub fn delete_participant(state: State<'_, AppState>, participant_id: String) -> IpcResult<()> {
    state.with_store(|s| s.delete_participant(&participant_id).map(|_| ()))
}

/// Forget the voice of a participant but keep the name.
#[tauri::command]
pub fn reset_profile(state: State<'_, AppState>, participant_id: String) -> IpcResult<()> {
    state.with_store(|s| s.reset_profile(&participant_id).map(|_| ()))
}

/// Delete every voice profile, stored voice embedding and sample clip.
#[tauri::command]
pub fn delete_voice_data(state: State<'_, AppState>) -> IpcResult<()> {
    let root = state.recordings_root()?;
    let files = state.with_store(|s| s.delete_all_voice_data())?;
    voices::remove_files(&root, &files);
    Ok(())
}
