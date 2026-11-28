//! IPC commands for speakers: detected voices, naming, merging and voice profiles.
//!
//! Voice profiles, embeddings and sample clips are biometric data. They are kept in the local
//! database and recordings folder only; no command here sends them anywhere.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use queue::voices;
use serde::{Deserialize, Serialize};
use store::{ParticipantInfo, SampleRow, Store, VoiceDecision, VoiceRow};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

use crate::cycles;
use crate::ipc::{IpcError, IpcResult};
use crate::settings::CycleRepoSetting;
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalVoiceProfileDto {
    pub id: String,
    pub name: String,
    pub cycle_id: Option<String>,
    pub projects: Vec<String>,
    pub sample_count: u32,
    pub updated_unix_ms: i64,
    pub has_sample_clip: bool,
}

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
    /// Display name of the suggested participant (even when from another project or global).
    pub suggested_participant_name: Option<String>,
    /// Projects the suggested participant belongs to (empty for unlinked global).
    pub suggested_from_projects: Vec<String>,
    /// The participant the voice is named after (automatic match or the user's choice).
    pub assigned_participant_id: Option<String>,
    /// Display name of the assigned participant (available even before linking into the cycle).
    pub assigned_participant_name: Option<String>,
    pub confirmed: bool,
    pub merged_into: Option<String>,
    pub clips: Vec<ClipDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantDto {
    /// The key voices are assigned to (for a cycle participant it includes the cycle).
    pub id: String,
    pub name: String,
    pub sample_count: u32,
    pub speech_ms: u64,
    /// The cycle the participant belongs to; absent for the legacy local list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<String>,
    /// The participant's id in `cycle.yaml` (what an item's assignee refers to).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_participant_id: Option<String>,
}

impl From<ParticipantInfo> for ParticipantDto {
    fn from(p: ParticipantInfo) -> Self {
        Self {
            cycle_participant_id: p.cycle_participant_id,
            id: p.id,
            name: p.name,
            sample_count: p.sample_count,
            speech_ms: p.speech_ms,
            cycle_id: p.cycle_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicesData {
    pub meeting_id: String,
    pub cycle_id: Option<String>,
    pub voices: Vec<VoiceDto>,
    pub participants: Vec<ParticipantDto>,
    pub all_confirmed: bool,
}

fn to_dto(
    v: VoiceRow,
    clips: &[store::ClipRow],
    all_participants: &[ParticipantInfo],
    current_cycle: Option<&str>,
) -> VoiceDto {
    let suggested_id = (v.decision == VoiceDecision::Suggested)
        .then(|| v.matched_participant.clone())
        .flatten();
    let suggested_info = suggested_id
        .as_deref()
        .and_then(|sid| all_participants.iter().find(|p| p.id == sid));
    let suggested_from_projects = suggested_info
        .map(|p| {
            if current_cycle.is_some_and(|cid| p.projects.iter().any(|c| c == cid)) {
                Vec::new()
            } else {
                p.projects
                    .iter()
                    .filter(|c| Some(c.as_str()) != current_cycle)
                    .cloned()
                    .collect()
            }
        })
        .unwrap_or_default();
    let assigned_participant_name = v
        .assigned_participant
        .as_deref()
        .and_then(|aid| all_participants.iter().find(|p| p.id == aid))
        .map(|p| p.name.clone());
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
        suggested_participant_name: suggested_info.map(|p| p.name.clone()),
        suggested_from_projects,
        suggested_participant_id: suggested_id,
        voice_id: v.voice_id,
        track: v.track,
        label: v.label,
        speech_ms: v.speech_ms,
        decision: v.decision,
        score: v.score,
        assigned_participant_id: v.assigned_participant,
        assigned_participant_name,
        confirmed: v.confirmed,
        merged_into: v.merged_into,
    }
}

/// The cycle of a meeting, if it has one.
fn meeting_cycle(store: &Store, meeting_id: &str) -> IpcResult<Option<String>> {
    Ok(store
        .get_report_settings(meeting_id)?
        .and_then(|s| s.cycle_id))
}

/// The participants a screen lists: those of `cycle_id`, else those of the meeting's cycle,
/// else the legacy local list. The stored participants of a cycle are first brought in line
/// with `cycle.yaml`, so a change made outside the application (a pull) shows up.
fn participants_for(
    repos: &[CycleRepoSetting],
    store: &Store,
    cycle_id: Option<&str>,
    meeting_id: Option<&str>,
) -> IpcResult<Vec<ParticipantInfo>> {
    let cycle = match (cycle_id, meeting_id) {
        (Some(c), _) => Some(c.to_string()),
        (None, Some(m)) => meeting_cycle(store, m)?,
        (None, None) => None,
    };
    if let Some(c) = &cycle {
        if let Err(e) = cycles::sync_cycle(repos, store, c) {
            tracing::warn!(cycle = %c, error = %e, "participant sync failed");
        }
    }
    Ok(store.list_participants_for(cycle.as_deref())?)
}

fn voices_data_in(repos: &[CycleRepoSetting], store: &Store, id: &str) -> IpcResult<VoicesData> {
    let cycle_id = meeting_cycle(store, id)?;
    let participants = participants_for(repos, store, cycle_id.as_deref(), Some(id))?;
    let voices = store.list_meeting_voices(id)?;
    let clips = store.list_clips(id)?;
    let all_participants = store.list_participants()?;
    let all_confirmed = voices::all_voices_confirmed(&voices);
    // The labels of the transcript ("Speaker 1") are shown as they are; the screen lists voices
    // by their order of appearance, which is the diarization label order.
    Ok(VoicesData {
        meeting_id: id.to_string(),
        voices: voices
            .into_iter()
            .map(|v| to_dto(v, &clips, &all_participants, cycle_id.as_deref()))
            .collect(),
        participants: participants.into_iter().map(Into::into).collect(),
        all_confirmed,
        cycle_id,
    })
}

fn voices_data(state: &AppState, id: &str) -> IpcResult<VoicesData> {
    let repos = state.cycle_repos();
    state.with_store_ipc(|s| voices_data_in(&repos, s, id))
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

/// Names a voice after the person called `name`. In a meeting of a cycle the person is one
/// of the cycle's (added to `cycle.yaml` and to the store when new); without a cycle it is
/// one of the legacy list. Returns `false` when the voice does not exist.
fn name_voice_after_new(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    voice_id: &str,
    name: &str,
) -> IpcResult<bool> {
    let name = clean_name(name)?;
    let participant = match meeting_cycle(store, meeting_id)? {
        Some(cycle) => cycles::add_cycle_participant(repos, store, &cycle, &name)?.id,
        None => voices::find_or_create_participant(
            store,
            meeting_id,
            &name,
            &new_id(),
            queue::now_ms(),
        )?,
    };
    Ok(voices::assign_voice(
        store,
        meeting_id,
        voice_id,
        Some(&participant),
        queue::now_ms(),
    )?)
}

/// A new participant: of the cycle (also written to `cycle.yaml`) or of the legacy list.
fn create_participant_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    cycle_id: Option<&str>,
    name: &str,
) -> IpcResult<ParticipantDto> {
    let name = clean_name(name)?;
    let duplicate = || IpcError::new("duplicate", "a participant with this name already exists");
    match cycle_id {
        Some(cycle) => {
            let taken = store
                .participants_for_cycle(cycle)?
                .iter()
                .any(|p| cycles::same_name(&p.name, &name));
            if taken {
                return Err(duplicate());
            }
            Ok(cycles::add_cycle_participant(repos, store, cycle, &name)?.into())
        }
        None => {
            if store.find_participant_by_name_in(None, &name)?.is_some() {
                return Err(duplicate());
            }
            let id = new_id();
            store
                .create_participant(&id, &name, queue::now_ms())
                .map_err(|_| duplicate())?;
            Ok(ParticipantDto {
                id,
                name,
                sample_count: 0,
                speech_ms: 0,
                cycle_id: None,
                cycle_participant_id: None,
            })
        }
    }
}

fn rename_participant_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    participant_id: &str,
    name: &str,
) -> IpcResult<()> {
    let name = clean_name(name)?;
    let info = cycles::find_participant(store, participant_id)?
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))?;
    if !info.projects.is_empty() || info.cycle_id.is_some() {
        return cycles::rename_cycle_participant(repos, store, &info, &name);
    }
    let taken = store
        .list_participants_for(None)?
        .iter()
        .any(|p| p.id != participant_id && cycles::same_name(&p.name, &name));
    if taken {
        return Err(IpcError::new(
            "duplicate",
            "a participant with this name already exists",
        ));
    }
    let renamed = store.rename_participant(participant_id, &name)?;
    if renamed {
        Ok(())
    } else {
        Err(IpcError::new("not_found", "participant not found"))
    }
}

fn sample_clip_shared_by_other(
    store: &Store,
    participant_id: &str,
    rel_path: &str,
) -> IpcResult<bool> {
    let target = rel_path.trim();
    if target.is_empty() {
        return Ok(false);
    }
    let target_norm = target.replace('\\', "/");
    let all = store.list_all_participants_with_profiles()?;
    Ok(all.into_iter().any(|p| {
        p.id != participant_id
            && p.sample_clip_file
                .as_deref()
                .map(str::trim)
                .is_some_and(|other| other.replace('\\', "/") == target_norm)
    }))
}

/// Remove a participant's retained biometric sample clip from disk unless another participant
/// still points to the same file. Returns `true` when both the participant's own clip and default
/// clip path are unshared (so `queue::voices` reset/delete helpers can remove them safely).
fn cleanup_participant_sample_clip(
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
) -> IpcResult<bool> {
    let own_rel = store
        .get_participant(participant_id)?
        .and_then(|p| p.sample_clip_file)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let default_rel = voices::participant_sample_clip_rel(participant_id);
    let shared_own = own_rel
        .as_deref()
        .is_some_and(|rel| sample_clip_shared_by_other(store, participant_id, rel).unwrap_or(true));
    let shared_default =
        sample_clip_shared_by_other(store, participant_id, &default_rel).unwrap_or(true);

    if !shared_own && !shared_default {
        return Ok(true);
    }

    if let Some(rel) = own_rel {
        if !shared_own {
            voices::remove_files(recordings_root, &[rel]);
        }
    }
    if !shared_default {
        voices::remove_files(recordings_root, &[default_rel]);
    }
    store.set_participant_sample_clip(participant_id, None)?;
    store.reset_profile(participant_id)?;
    Ok(false)
}

fn delete_participant_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
) -> IpcResult<()> {
    let info = cycles::find_participant(store, participant_id)?;
    let unshared = cleanup_participant_sample_clip(store, recordings_root, participant_id)?;
    if let Some(p) = info {
        let mut cycles_to_clean = p.projects.clone();
        if let Some(cid) = p.cycle_id.as_deref() {
            if !cycles_to_clean.iter().any(|c| c == cid) {
                cycles_to_clean.push(cid.to_string());
            }
        }
        for cid in cycles_to_clean {
            if let Ok(rows) = store.participants_for_cycle(&cid) {
                if let Some(in_cycle) = rows.into_iter().find(|r| r.id == p.id) {
                    let _ = cycles::delete_cycle_participant(repos, store, &in_cycle);
                }
            }
        }
    }
    if unshared {
        voices::delete_participant_profile(store, participant_id, recordings_root)?;
    } else {
        store.delete_participant(participant_id)?;
    }
    Ok(())
}

pub fn reset_profile_in(
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
) -> IpcResult<()> {
    if cleanup_participant_sample_clip(store, recordings_root, participant_id)? {
        voices::reset_profile(store, participant_id, recordings_root)?;
    }
    Ok(())
}

pub fn delete_voice_data_in(store: &Store, recordings_root: &Path) -> IpcResult<()> {
    let files = store.delete_all_voice_data()?;
    voices::remove_files(recordings_root, &files);
    let samples_dir = recordings_root.join(voices::VOICE_SAMPLES_DIR);
    if let Ok(entries) = std::fs::read_dir(&samples_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        let _ = std::fs::remove_dir(&samples_dir);
    }
    Ok(())
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

fn assign_voice_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    voice_id: &str,
    participant_id: Option<&str>,
) -> IpcResult<bool> {
    if let Some(pid) = participant_id {
        if let Some(cycle) = meeting_cycle(store, meeting_id)? {
            let already_in_cycle = store
                .participants_for_cycle(&cycle)?
                .iter()
                .any(|p| p.id == pid);
            if !already_in_cycle {
                cycles::add_existing_participant_to_cycle(repos, store, &cycle, pid)?;
            }
        }
    }
    Ok(voices::assign_voice(
        store,
        meeting_id,
        voice_id,
        participant_id,
        queue::now_ms(),
    )?)
}

#[tauri::command]
pub fn assign_voice(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    participant_id: Option<String>,
) -> IpcResult<VoicesData> {
    let repos = state.cycle_repos();
    let ok = state.with_store_ipc(|s| {
        assign_voice_in(&repos, s, &id, &voice_id, participant_id.as_deref())
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
    let repos = state.cycle_repos();
    let ok = state.with_store_ipc(|s| name_voice_after_new(&repos, s, &id, &voice_id, &name))?;
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

pub fn confirm_matches_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
) -> IpcResult<usize> {
    if let Some(cycle) = meeting_cycle(store, meeting_id)? {
        for v in store.list_meeting_voices(meeting_id)? {
            if v.decision == VoiceDecision::Auto && !v.confirmed {
                if let Some(pid) = v.assigned_participant.as_deref() {
                    let already_in_cycle = store
                        .participants_for_cycle(&cycle)?
                        .iter()
                        .any(|p| p.id == pid);
                    if !already_in_cycle {
                        cycles::add_existing_participant_to_cycle(repos, store, &cycle, pid)?;
                    }
                }
            }
        }
    }
    Ok(voices::confirm_automatic(
        store,
        meeting_id,
        queue::now_ms(),
    )?)
}

/// Accept all automatic matches of a meeting.
#[tauri::command]
pub fn confirm_matches(state: State<'_, AppState>, id: String) -> IpcResult<VoicesData> {
    let repos = state.cycle_repos();
    state.with_store_ipc(|s| confirm_matches_in(&repos, s, &id).map(|_| ()))?;
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

/// The participants of a cycle: `cycle_id`, else the cycle of `meeting_id`; without either
/// the legacy local list (participants made before cycles existed).
#[tauri::command]
pub fn list_participants(
    state: State<'_, AppState>,
    cycle_id: Option<String>,
    meeting_id: Option<String>,
) -> IpcResult<Vec<ParticipantDto>> {
    let repos = state.cycle_repos();
    Ok(state
        .with_store_ipc(|s| {
            participants_for(&repos, s, cycle_id.as_deref(), meeting_id.as_deref())
        })?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Creates a participant in `cycle_id` (written to `cycle.yaml` too) or, without a cycle, in
/// the legacy list.
#[tauri::command]
pub fn create_participant(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    cycle_id: Option<String>,
) -> IpcResult<ParticipantDto> {
    let repos = state.cycle_repos();
    let created =
        state.with_store_ipc(|s| create_participant_in(&repos, s, cycle_id.as_deref(), &name))?;
    if let Err(e) = app.emit(crate::cycle_commands::EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the projects change");
    }
    Ok(created)
}

#[tauri::command]
pub fn rename_participant(
    state: State<'_, AppState>,
    participant_id: String,
    name: String,
) -> IpcResult<()> {
    let repos = state.cycle_repos();
    state.with_store_ipc(|s| rename_participant_in(&repos, s, &participant_id, &name))
}

/// Delete a participant together with the voice profile (and remove them from `cycle.yaml`
/// for a cycle participant). Voices named after them become unnamed.
#[tauri::command]
pub fn delete_participant(state: State<'_, AppState>, participant_id: String) -> IpcResult<()> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| delete_participant_in(&repos, s, &root, &participant_id))
}

/// Forget the voice of a participant but keep the name.
#[tauri::command]
pub fn reset_profile(state: State<'_, AppState>, participant_id: String) -> IpcResult<()> {
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| reset_profile_in(s, &root, &participant_id))
}

/// Delete every voice profile, stored voice embedding and sample clip.
#[tauri::command]
pub fn delete_voice_data(state: State<'_, AppState>) -> IpcResult<()> {
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| delete_voice_data_in(s, &root))
}

/// Generate a valid 2-second 16 kHz mono WAV sine tone in memory.
pub fn generate_demo_wav_bytes(hz: f64) -> Vec<u8> {
    let sample_rate = 16_000usize;
    let total_samples = sample_rate * 2;
    let data_len = (total_samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + total_samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sample_rate as u32).to_le_bytes());
    out.extend_from_slice(&((sample_rate * 2) as u32).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    let step = 2.0 * std::f64::consts::PI * hz / (sample_rate as f64);
    for i in 0..total_samples {
        let s = ((step * i as f64).sin() * 8_000.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn synthetic_embedding(seed_str: &str) -> Vec<f32> {
    let mut hash = 2166136261u32;
    for b in seed_str.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(16777619);
    }
    let mut vec = Vec::with_capacity(256);
    let mut norm_sq = 0.0f32;
    let mut state = hash.max(1);
    for _ in 0..256 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let val = ((state as f32) / (u32::MAX as f32)) * 2.0 - 1.0;
        norm_sq += val * val;
        vec.push(val);
    }
    let norm = norm_sq.sqrt().max(1e-6);
    for v in &mut vec {
        *v /= norm;
    }
    vec
}

pub fn list_all_voice_profiles_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
) -> IpcResult<Vec<GlobalVoiceProfileDto>> {
    for item in &cycles::list_cycles(repos) {
        let _ = cycles::sync_cycle(repos, store, &item.id);
    }

    let all = store.list_all_participants_with_profiles()?;
    let mut out = Vec::with_capacity(all.len());
    for p in all {
        let updated_unix_ms = store
            .get_participant(&p.id)?
            .map_or(0, |row| row.created_unix_ms);
        let has_sample_clip = p
            .sample_clip_file
            .as_deref()
            .map(str::trim)
            .filter(|rel| !rel.is_empty())
            .is_some_and(|rel| recordings_root.join(rel).is_file());

        out.push(GlobalVoiceProfileDto {
            id: p.id,
            name: p.name,
            cycle_id: p.cycle_id,
            projects: p.projects,
            sample_count: p.sample_count,
            updated_unix_ms,
            has_sample_clip,
        });
    }
    Ok(out)
}

pub fn voice_profile_sample_clip_in(
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
    demo: bool,
) -> IpcResult<Vec<u8>> {
    if let Some(p) = store.get_participant(participant_id)? {
        if let Some(rel) = p
            .sample_clip_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let candidate = recordings_root.join(rel);
            if candidate.is_file() {
                return Ok(std::fs::read(candidate)?);
            }
        }
    }
    if demo {
        return Ok(generate_demo_wav_bytes(440.0));
    }
    Err(IpcError::new(
        "not_found",
        "no sample clip available for this participant",
    ))
}

pub enum EnrollMode<'a> {
    Demo,
    Worker(&'a queue::worker::WorkerConfig),
}

pub fn enroll_voice_profile_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
    mode: EnrollMode<'_>,
    name: &str,
    cycle_id: Option<&str>,
    audio_path: &str,
) -> IpcResult<GlobalVoiceProfileDto> {
    let name = clean_name(name)?;
    let audio_path_trimmed = audio_path.trim();
    if audio_path_trimmed.is_empty() {
        return Err(IpcError::new("invalid", "audioPath cannot be empty"));
    }
    let cycle_opt = cycle_id.map(str::trim).filter(|s| !s.is_empty());
    let now = queue::now_ms();

    let participant_id = match cycle_opt {
        Some(cid) => {
            let _ = cycles::sync_cycle(repos, store, cid);
            let existing = store
                .participants_for_cycle(cid)?
                .into_iter()
                .find(|p| cycles::same_name(&p.name, &name));
            if let Some(p) = existing {
                p.id
            } else if cycles::locate(repos, cid)?.is_some() {
                cycles::add_cycle_participant(repos, store, cid, &name)?.id
            } else {
                let slug: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() {
                            c.to_ascii_lowercase()
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>()
                    .trim_matches('-')
                    .to_string();
                let slug = if slug.is_empty() {
                    "participant".to_string()
                } else {
                    slug
                };
                let key = store::cycle_participant_key(cid, &slug);
                store.create_participant_in(Some(cid), &key, &name, now)?;
                key
            }
        }
        None => {
            let existing = store
                .list_participants_for(None)?
                .into_iter()
                .find(|p| cycles::same_name(&p.name, &name));
            if let Some(p) = existing {
                p.id
            } else {
                let id = new_id();
                store.create_participant(&id, &name, now)?;
                id
            }
        }
    };

    match mode {
        EnrollMode::Demo => {
            let rel_clip = voices::participant_sample_clip_rel(&participant_id);
            let clip_out_path = recordings_root.join(&rel_clip);
            if let Some(parent) = clip_out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let input = Path::new(audio_path_trimmed);
            if input.is_file() {
                let _ = std::fs::copy(input, &clip_out_path);
            }
            if !clip_out_path.is_file() {
                std::fs::write(&clip_out_path, generate_demo_wav_bytes(440.0))?;
            }
            store.put_sample(
                &participant_id,
                &SampleRow {
                    source_key: "manual_enroll".into(),
                    embedding: synthetic_embedding(&participant_id),
                    speech_ms: 20_000,
                },
                now,
            )?;
            store.set_participant_sample_clip(&participant_id, Some(&rel_clip))?;
        }
        EnrollMode::Worker(cfg) => {
            queue::runner::enroll_participant_voice(
                store,
                cfg,
                None,
                &participant_id,
                Path::new(audio_path_trimmed),
                recordings_root,
                |_| {},
            )
            .map_err(|e| IpcError::new("io", e.to_string()))?;
        }
    }

    let profiles = list_all_voice_profiles_in(repos, store, recordings_root)?;
    profiles
        .into_iter()
        .find(|p| p.id == participant_id)
        .ok_or_else(|| IpcError::new("not_found", "enrolled voice profile not found"))
}

#[tauri::command]
pub fn list_all_voice_profiles(
    state: State<'_, AppState>,
) -> IpcResult<Vec<GlobalVoiceProfileDto>> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| list_all_voice_profiles_in(&repos, s, &root))
}

#[tauri::command]
pub fn voice_profile_sample_clip(
    state: State<'_, AppState>,
    participant_id: String,
) -> IpcResult<Vec<u8>> {
    let root = state.recordings_root()?;
    let demo = state.is_demo() || std::env::var_os("BBB_DEMO").is_some();
    state.with_store_ipc(|s| voice_profile_sample_clip_in(s, &root, &participant_id, demo))
}

#[tauri::command]
pub fn enroll_voice_profile(
    state: State<'_, AppState>,
    name: String,
    cycle_id: Option<String>,
    audio_path: String,
) -> IpcResult<GlobalVoiceProfileDto> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    let cfg = state.processing().worker_config();
    let demo = state.is_demo() || std::env::var_os("BBB_DEMO").is_some();
    let mode = if demo {
        EnrollMode::Demo
    } else {
        EnrollMode::Worker(&cfg)
    };
    state.with_store_ipc(|s| {
        enroll_voice_profile_in(
            &repos,
            s,
            &root,
            mode,
            &name,
            cycle_id.as_deref(),
            &audio_path,
        )
    })
}

pub fn add_profile_to_project_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
    cycle_id: &str,
) -> IpcResult<GlobalVoiceProfileDto> {
    cycles::add_existing_participant_to_cycle(repos, store, cycle_id, participant_id)?;
    list_all_voice_profiles_in(repos, store, recordings_root)?
        .into_iter()
        .find(|p| p.id == participant_id)
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))
}

pub fn remove_profile_from_project_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
    participant_id: &str,
    cycle_id: &str,
) -> IpcResult<GlobalVoiceProfileDto> {
    let info = store
        .participants_for_cycle(cycle_id)?
        .into_iter()
        .find(|p| p.id == participant_id)
        .ok_or_else(|| IpcError::new("not_found", "participant not in project"))?;
    cycles::delete_cycle_participant(repos, store, &info)?;
    list_all_voice_profiles_in(repos, store, recordings_root)?
        .into_iter()
        .find(|p| p.id == participant_id)
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))
}

pub fn merge_participants_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    recordings_root: &Path,
    keep_id: &str,
    drop_id: &str,
) -> IpcResult<GlobalVoiceProfileDto> {
    if keep_id == drop_id {
        return Err(IpcError::new(
            "invalid",
            "cannot merge a participant with itself",
        ));
    }
    let keep_info = cycles::find_participant(store, keep_id)?
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))?;
    let drop_info = cycles::find_participant(store, drop_id)?
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))?;

    for proj in &drop_info.projects {
        if keep_info.projects.iter().any(|p| p == proj) {
            return Err(IpcError::new(
                "invalid",
                format!("cannot merge participants that both belong to project {proj}"),
            ));
        }
    }

    let cycle_updates =
        cycles::prepare_merge_cycle_updates(repos, store, keep_id, &keep_info.name, &drop_info)?;

    let drop_clip = store
        .get_participant(drop_id)?
        .and_then(|p| p.sample_clip_file)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let drop_default_rel = voices::participant_sample_clip_rel(drop_id);

    store
        .merge_participants(keep_id, drop_id)
        .map_err(|e| match e {
            store::StoreError::Invalid(msg) => IpcError::new("invalid", msg),
            other => other.into(),
        })?;

    for (repo, cycle) in cycle_updates {
        repo.save_cycle(&cycle)?;
    }

    let remaining = store.list_all_participants_with_profiles()?;
    let is_referenced = |rel: &str| {
        let norm = rel.replace('\\', "/");
        remaining.iter().any(|p| {
            p.sample_clip_file
                .as_deref()
                .map(str::trim)
                .is_some_and(|other| other.replace('\\', "/") == norm)
        })
    };
    let mut orphaned_clips = Vec::new();
    if let Some(rel) = drop_clip {
        if !is_referenced(&rel) {
            orphaned_clips.push(rel);
        }
    }
    if !orphaned_clips.contains(&drop_default_rel) && !is_referenced(&drop_default_rel) {
        orphaned_clips.push(drop_default_rel);
    }
    if !orphaned_clips.is_empty() {
        voices::remove_files(recordings_root, &orphaned_clips);
    }

    list_all_voice_profiles_in(repos, store, recordings_root)?
        .into_iter()
        .find(|p| p.id == keep_id)
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))
}

pub fn undo_voice_decision_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    voice_id: &str,
    unlink_cycle_id: Option<&str>,
) -> IpcResult<bool> {
    let voices = store.list_meeting_voices(meeting_id)?;
    let Some(voice) = voices.iter().find(|v| v.voice_id == voice_id) else {
        return Ok(false);
    };
    let prev_participant = voice.assigned_participant.clone();
    if let (Some(cid), Some(pid)) = (unlink_cycle_id, prev_participant.as_deref()) {
        if let Some(in_cycle) = store
            .participants_for_cycle(cid)?
            .into_iter()
            .find(|p| p.id == pid)
        {
            cycles::delete_cycle_participant(repos, store, &in_cycle)?;
        }
    }
    Ok(voices::undo_voice_decision(
        store,
        meeting_id,
        voice_id,
        None,
        queue::now_ms(),
    )?)
}

#[tauri::command]
pub fn add_profile_to_project(
    state: State<'_, AppState>,
    participant_id: String,
    cycle_id: String,
) -> IpcResult<GlobalVoiceProfileDto> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    state
        .with_store_ipc(|s| add_profile_to_project_in(&repos, s, &root, &participant_id, &cycle_id))
}

#[tauri::command]
pub fn remove_profile_from_project(
    state: State<'_, AppState>,
    participant_id: String,
    cycle_id: String,
) -> IpcResult<GlobalVoiceProfileDto> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| {
        remove_profile_from_project_in(&repos, s, &root, &participant_id, &cycle_id)
    })
}

#[tauri::command]
pub fn merge_participants(
    state: State<'_, AppState>,
    keep_id: String,
    drop_id: String,
) -> IpcResult<GlobalVoiceProfileDto> {
    let repos = state.cycle_repos();
    let root = state.recordings_root()?;
    state.with_store_ipc(|s| merge_participants_in(&repos, s, &root, &keep_id, &drop_id))
}

#[tauri::command]
pub fn undo_voice_decision(
    state: State<'_, AppState>,
    id: String,
    voice_id: String,
    unlink_cycle_id: Option<String>,
) -> IpcResult<VoicesData> {
    let repos = state.cycle_repos();
    let ok = state.with_store_ipc(|s| {
        undo_voice_decision_in(&repos, s, &id, &voice_id, unlink_cycle_id.as_deref())
    })?;
    if !ok {
        return Err(IpcError::new("not_found", "voice not found"));
    }
    after_change(&state)?;
    voices_data(&state, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycles::tests::Lab;
    use store::VoiceRow;

    fn voice(id: &str) -> VoiceRow {
        VoiceRow {
            voice_id: id.into(),
            track: "discord".into(),
            label: "SPEAKER_00".into(),
            embedding: vec![0.1, 0.2],
            speech_ms: 1000,
            decision: VoiceDecision::Unknown,
            score: None,
            matched_participant: None,
            assigned_participant: None,
            confirmed: false,
            merged_into: None,
        }
    }

    fn with_voice(lab: &Lab, meeting: &str, cycle: Option<&str>) {
        lab.meeting(meeting, cycle);
        lab.store
            .put_meeting_voices(meeting, &[voice("discord:SPEAKER_00")])
            .unwrap();
    }

    #[test]
    fn a_name_from_the_labeling_screen_joins_the_cycle_and_names_the_voice() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        with_voice(&lab, "m1", Some("NOMAD"));
        assert!(name_voice_after_new(
            lab.repos(),
            &lab.store,
            "m1",
            "discord:SPEAKER_00",
            "  Alex Example "
        )
        .unwrap());
        // In cycle.yaml, with a slug id, and in the store.
        let cycle = cycles::get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        assert_eq!(cycle.participants.len(), 1);
        assert_eq!(cycle.participants[0].id, "alex-example");
        assert_eq!(cycle.participants[0].name, "Alex Example");
        let voices = lab.store.list_meeting_voices("m1").unwrap();
        assert_eq!(
            voices[0].assigned_participant.as_deref(),
            Some("NOMAD:alex-example")
        );
        assert!(voices[0].confirmed);
        // The transcript names come from the meeting's cycle participants.
        let names = voices::speaker_names(
            &voices,
            &voices::participants_for_meeting(&lab.store, "m1").unwrap(),
        );
        assert_eq!(names["discord:SPEAKER_00"], "Alex Example");

        // The same name again is the same person, not a second one.
        assert!(name_voice_after_new(
            lab.repos(),
            &lab.store,
            "m1",
            "discord:SPEAKER_00",
            "alex example"
        )
        .unwrap());
        assert_eq!(
            cycles::get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants
                .len(),
            1
        );
        // An unknown voice and a bad name.
        assert!(!name_voice_after_new(lab.repos(), &lab.store, "m1", "nope", "Sam").unwrap());
        assert_eq!(
            name_voice_after_new(lab.repos(), &lab.store, "m1", "discord:SPEAKER_00", "  ")
                .unwrap_err()
                .code,
            "invalid"
        );
    }

    #[test]
    fn a_meeting_without_a_cycle_keeps_using_the_legacy_list() {
        let lab = Lab::new();
        with_voice(&lab, "old", None);
        assert!(
            name_voice_after_new(lab.repos(), &lab.store, "old", "discord:SPEAKER_00", "Ola")
                .unwrap()
        );
        let legacy = lab.store.list_participants_for(None).unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].cycle_id, None);
        let voices = lab.store.list_meeting_voices("old").unwrap();
        assert_eq!(
            voices[0].assigned_participant.as_deref(),
            Some(legacy[0].id.as_str())
        );
        // Nothing was written to any cycle.
        assert!(cycles::list_cycles(lab.repos()).is_empty());
    }

    #[test]
    fn the_participant_list_defaults_to_the_meetings_cycle() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("OTHER");
        create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Alex").unwrap();
        create_participant_in(lab.repos(), &lab.store, Some("OTHER"), "Jan").unwrap();
        lab.store.create_participant("p1", "Old", 1).unwrap();
        lab.meeting("m1", Some("NOMAD"));
        lab.meeting("m2", None);

        let names = |cycle: Option<&str>, meeting: Option<&str>| -> Vec<String> {
            participants_for(lab.repos(), &lab.store, cycle, meeting)
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(names(None, Some("m1")), ["Alex"]);
        assert_eq!(names(Some("OTHER"), Some("m1")), ["Jan"]);
        assert_eq!(names(None, Some("m2")), ["Old"]);
        assert_eq!(names(None, None), ["Old"]);
    }

    #[test]
    fn the_list_picks_up_a_change_made_in_the_file() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let mut dto = cycles::get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants.push(cycles::CycleParticipantDto {
            id: "sam".into(),
            name: "Sam".into(),
            github: None,
            discord: None,
        });
        // Written straight to the repository, as a pull would.
        lab.repo().save_cycle(&dto.into()).unwrap();
        assert!(lab
            .store
            .participants_for_cycle("NOMAD")
            .unwrap()
            .is_empty());
        let list = participants_for(lab.repos(), &lab.store, Some("NOMAD"), None).unwrap();
        assert_eq!(list.len(), 1);
        let dto: ParticipantDto = list[0].clone().into();
        assert_eq!(dto.cycle_participant_id.as_deref(), Some("sam"));
        assert_eq!(dto.cycle_id.as_deref(), Some("NOMAD"));
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["cycleParticipantId"], "sam");
    }

    #[test]
    fn create_rename_and_delete_follow_the_participants_cycle() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let a = create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Alex").unwrap();
        assert_eq!(a.id, "NOMAD:alex");
        assert_eq!(
            create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "ALEX")
                .unwrap_err()
                .code,
            "duplicate"
        );
        rename_participant_in(lab.repos(), &lab.store, &a.id, "Alexander").unwrap();
        assert_eq!(
            cycles::get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Alexander"
        );
        assert_eq!(
            rename_participant_in(lab.repos(), &lab.store, "NOMAD:ghost", "X")
                .unwrap_err()
                .code,
            "not_found"
        );
        delete_participant_in(lab.repos(), &lab.store, lab.dir.path(), &a.id).unwrap();
        assert!(cycles::get_cycle(lab.repos(), "NOMAD")
            .unwrap()
            .cycle
            .participants
            .is_empty());
        assert!(lab
            .store
            .participants_for_cycle("NOMAD")
            .unwrap()
            .is_empty());

        // The legacy list works as before.
        let old = create_participant_in(lab.repos(), &lab.store, None, "Ola").unwrap();
        assert_eq!(old.cycle_id, None);
        assert_eq!(
            create_participant_in(lab.repos(), &lab.store, None, "Ola")
                .unwrap_err()
                .code,
            "duplicate"
        );
        rename_participant_in(lab.repos(), &lab.store, &old.id, "Ola K").unwrap();
        delete_participant_in(lab.repos(), &lab.store, lab.dir.path(), &old.id).unwrap();
        assert!(lab.store.list_participants_for(None).unwrap().is_empty());
    }

    #[test]
    fn list_all_and_enroll_voice_profile_work_across_projects() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let rec_root = test_tmp::TestDir::new("voice-enroll");
        let sample_wav = rec_root.path().join("sample.wav");
        crate::project_commands::write_demo_wav(&sample_wav, 330.0, 2).unwrap();

        let enrolled = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Kasia",
            Some("NOMAD"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        assert_eq!(enrolled.name, "Kasia");
        assert_eq!(enrolled.cycle_id.as_deref(), Some("NOMAD"));
        assert_eq!(enrolled.sample_count, 1);
        assert!(enrolled.has_sample_clip);
        assert_eq!(enrolled.projects, vec!["NOMAD".to_string()]);

        // Link the same person to ATLAS cycle too so `projects` spans both without creating a duplicate.
        add_profile_to_project_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            &enrolled.id,
            "ATLAS",
        )
        .unwrap();

        let all = list_all_voice_profiles_in(lab.repos(), &lab.store, rec_root.path()).unwrap();
        let kasia_matches: Vec<_> = all.iter().filter(|p| p.name == "Kasia").collect();
        assert_eq!(kasia_matches.len(), 1);
        assert_eq!(
            kasia_matches[0].projects,
            vec!["ATLAS".to_string(), "NOMAD".to_string()]
        );

        let clip_bytes =
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &enrolled.id, false).unwrap();
        assert!(clip_bytes.starts_with(b"RIFF"));
        assert!(clip_bytes.len() > 44);
    }

    #[test]
    fn reset_and_delete_remove_wav_and_return_not_found_when_not_demo() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let rec_root = test_tmp::TestDir::new("voice-reset-del");
        let sample_wav = rec_root.path().join("sample.wav");
        crate::project_commands::write_demo_wav(&sample_wav, 330.0, 2).unwrap();

        let p1 = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Ewa",
            Some("NOMAD"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let p1_clip = voices::participant_sample_clip_path(rec_root.path(), &p1.id);
        assert!(p1_clip.is_file());

        // Reset removes the WAV on disk and clears sample_clip_file.
        reset_profile_in(&lab.store, rec_root.path(), &p1.id).unwrap();
        assert!(!p1_clip.exists());

        let all = list_all_voice_profiles_in(lab.repos(), &lab.store, rec_root.path()).unwrap();
        let p1_after = all.iter().find(|p| p.id == p1.id).unwrap();
        assert!(!p1_after.has_sample_clip);
        assert_eq!(p1_after.sample_count, 0);

        let err =
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &p1.id, false).unwrap_err();
        assert_eq!(err.code, "not_found");

        // Demo mode falls back to synthetic 440 Hz tone.
        let demo_bytes =
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &p1.id, true).unwrap();
        assert!(demo_bytes.starts_with(b"RIFF"));

        // Even if an orphaned WAV sits at the legacy path, reset profile reports false & not_found.
        std::fs::create_dir_all(p1_clip.parent().unwrap()).unwrap();
        std::fs::write(&p1_clip, b"RIFForphan").unwrap();
        let all_orphan =
            list_all_voice_profiles_in(lab.repos(), &lab.store, rec_root.path()).unwrap();
        assert!(
            !all_orphan
                .iter()
                .find(|p| p.id == p1.id)
                .unwrap()
                .has_sample_clip
        );
        assert_eq!(
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &p1.id, false)
                .unwrap_err()
                .code,
            "not_found"
        );

        // Delete participant also removes its WAV file on disk.
        let p2 = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Jan",
            Some("NOMAD"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let p2_clip = voices::participant_sample_clip_path(rec_root.path(), &p2.id);
        assert!(p2_clip.is_file());
        delete_participant_in(lab.repos(), &lab.store, rec_root.path(), &p2.id).unwrap();
        assert!(!p2_clip.exists());
        assert_eq!(
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &p2.id, false)
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn reset_keeps_shared_clip_until_last_owner_is_reset_and_delete_all_sweeps_dir() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let rec_root = test_tmp::TestDir::new("voice-shared-sweep");
        let sample_wav = rec_root.path().join("sample.wav");
        crate::project_commands::write_demo_wav(&sample_wav, 330.0, 2).unwrap();

        let global_p = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Marek",
            None,
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let shared_rel = voices::participant_sample_clip_rel(&global_p.id);
        let shared_path = rec_root.path().join(&shared_rel);
        assert!(shared_path.is_file());

        let cycle_p =
            create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Marek").unwrap();
        lab.store
            .set_participant_sample_clip(&cycle_p.id, Some(&shared_rel))
            .unwrap();

        // Resetting the cycle participant must NOT delete the global participant's WAV file.
        reset_profile_in(&lab.store, rec_root.path(), &cycle_p.id).unwrap();
        assert!(shared_path.is_file());
        assert_eq!(
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &cycle_p.id, false)
                .unwrap_err()
                .code,
            "not_found"
        );
        assert!(
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &global_p.id, false).is_ok()
        );

        // Resetting the remaining owner deletes the WAV file.
        reset_profile_in(&lab.store, rec_root.path(), &global_p.id).unwrap();
        assert!(!shared_path.exists());

        // delete_voice_data_in sweeps all orphaned files in .voice-samples even with NULL DB columns.
        let samples_dir = rec_root.path().join(voices::VOICE_SAMPLES_DIR);
        std::fs::create_dir_all(&samples_dir).unwrap();
        let orphan = samples_dir.join("stale-orphan.wav");
        std::fs::write(&orphan, b"RIFFstale").unwrap();
        delete_voice_data_in(&lab.store, rec_root.path()).unwrap();
        assert!(!orphan.exists());
    }

    #[test]
    fn merge_and_project_link_commands_keep_samples_on_unlink() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let rec_root = test_tmp::TestDir::new("voice-merge-unlink");
        let sample_wav = rec_root.path().join("sample.wav");
        crate::project_commands::write_demo_wav(&sample_wav, 330.0, 2).unwrap();

        let p1 = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Jan",
            Some("NOMAD"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let p2 = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Jan",
            Some("ATLAS"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        assert_ne!(p1.id, p2.id);

        let merged =
            merge_participants_in(lab.repos(), &lab.store, rec_root.path(), &p1.id, &p2.id)
                .unwrap();
        assert_eq!(
            merged.projects,
            vec!["ATLAS".to_string(), "NOMAD".to_string()]
        );

        let after_unlink = remove_profile_from_project_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            &p1.id,
            "ATLAS",
        )
        .unwrap();
        assert_eq!(after_unlink.projects, vec!["NOMAD".to_string()]);
        assert_eq!(after_unlink.sample_count, 1);
    }

    #[test]
    fn undo_voice_decision_unlinks_cycle_yaml_and_prevents_sync_duplicate() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let nomad_kasia =
            create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Kasia").unwrap();
        lab.store
            .put_sample(
                &nomad_kasia.id,
                &SampleRow {
                    source_key: "m0/sys:0".into(),
                    embedding: vec![1.0, 0.0],
                    speech_ms: 15_000,
                },
                1,
            )
            .unwrap();

        // Meeting in ATLAS (brand-new project with 0 participants).
        lab.meeting("m-atlas", Some("ATLAS"));
        let mut v = voice("discord:SPEAKER_00");
        v.speech_ms = 12_000;
        v.decision = VoiceDecision::Suggested;
        v.score = Some(0.82);
        v.matched_participant = Some(nomad_kasia.id.clone());
        lab.store.put_meeting_voices("m-atlas", &[v]).unwrap();

        // VoicesData carries cycle_id even when ATLAS has 0 participants yet.
        let before = voices_data_in(lab.repos(), &lab.store, "m-atlas").unwrap();
        assert_eq!(before.cycle_id.as_deref(), Some("ATLAS"));
        assert!(before.participants.is_empty());
        assert_eq!(
            before.voices[0].suggested_from_projects,
            vec!["NOMAD".to_string()]
        );

        // Accepting links Kasia into ATLAS (both cycle.yaml and project_people).
        assert!(assign_voice_in(
            lab.repos(),
            &lab.store,
            "m-atlas",
            "discord:SPEAKER_00",
            Some(&nomad_kasia.id)
        )
        .unwrap());
        assert_eq!(
            cycles::get_cycle(lab.repos(), "ATLAS")
                .unwrap()
                .cycle
                .participants
                .len(),
            1
        );
        assert_eq!(lab.store.samples_of(&nomad_kasia.id).unwrap().len(), 2);

        // Undoing removes Kasia from BOTH ATLAS/cycle.yaml and project_people, and removes sample.
        assert!(undo_voice_decision_in(
            lab.repos(),
            &lab.store,
            "m-atlas",
            "discord:SPEAKER_00",
            Some("ATLAS")
        )
        .unwrap());
        assert!(cycles::get_cycle(lab.repos(), "ATLAS")
            .unwrap()
            .cycle
            .participants
            .is_empty());

        // Subsequent voices_data_in runs sync_cycle: ATLAS must stay empty (no recreated duplicate).
        let after = voices_data_in(lab.repos(), &lab.store, "m-atlas").unwrap();
        assert!(after.participants.is_empty());
        assert!(!after.voices[0].confirmed);
        assert_eq!(after.voices[0].assigned_participant_id, None);
        assert_eq!(lab.store.samples_of(&nomad_kasia.id).unwrap().len(), 1);
    }

    #[test]
    fn merge_updates_drop_cycle_yaml_and_deletes_orphaned_clip() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let rec_root = test_tmp::TestDir::new("voice-merge-yaml-clip");
        let sample_wav = rec_root.path().join("sample.wav");
        crate::project_commands::write_demo_wav(&sample_wav, 330.0, 2).unwrap();

        let keep = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Alex Smith",
            Some("NOMAD"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let drop_p = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Alex",
            Some("ATLAS"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let keep_clip = voices::participant_sample_clip_path(rec_root.path(), &keep.id);
        let drop_clip = voices::participant_sample_clip_path(rec_root.path(), &drop_p.id);
        assert!(keep_clip.is_file());
        assert!(drop_clip.is_file());

        // Merge ATLAS "Alex" into NOMAD "Alex Smith":
        // - ATLAS/cycle.yaml is updated to "Alex Smith" so sync_cycle does not revert keep's name.
        // - Orphaned drop clip is deleted from disk while keep clip stays.
        let merged = merge_participants_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            &keep.id,
            &drop_p.id,
        )
        .unwrap();
        assert_eq!(merged.name, "Alex Smith");
        assert_eq!(
            merged.projects,
            vec!["ATLAS".to_string(), "NOMAD".to_string()]
        );
        assert!(keep_clip.is_file());
        assert!(!drop_clip.exists());
        assert_eq!(
            cycles::get_cycle(lab.repos(), "ATLAS")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Alex Smith"
        );

        // Explicit sync_cycle keeps the merged name intact.
        cycles::sync_cycle(lab.repos(), &lab.store, "ATLAS").unwrap();
        assert_eq!(
            lab.store.get_participant(&keep.id).unwrap().unwrap().name,
            "Alex Smith"
        );

        // When keep has no clip and drop has one, drop's clip is transferred and NOT deleted.
        let bare = create_participant_in(lab.repos(), &lab.store, None, "Ewa").unwrap();
        let with_clip = enroll_voice_profile_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            EnrollMode::Demo,
            "Ewa Two",
            Some("ATLAS"),
            &sample_wav.to_string_lossy(),
        )
        .unwrap();
        let transferred_clip = voices::participant_sample_clip_path(rec_root.path(), &with_clip.id);
        assert!(transferred_clip.is_file());
        let merged_bare = merge_participants_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            &bare.id,
            &with_clip.id,
        )
        .unwrap();
        assert!(merged_bare.has_sample_clip);
        assert!(transferred_clip.is_file());
    }

    #[test]
    fn rename_multi_project_pinned_global_and_duplicate_checks() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let rec_root = test_tmp::TestDir::new("voice-rename-multi");

        // Global profile pinned to 1 project can be renamed without "invalid".
        let g = create_participant_in(lab.repos(), &lab.store, None, "Ola").unwrap();
        add_profile_to_project_in(lab.repos(), &lab.store, rec_root.path(), &g.id, "NOMAD")
            .unwrap();
        rename_participant_in(lab.repos(), &lab.store, &g.id, "Ola Nowak").unwrap();
        assert_eq!(
            cycles::get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Ola Nowak"
        );

        // Pin to a second project ATLAS, then rename: BOTH cycle.yaml files update and survive sync.
        add_profile_to_project_in(lab.repos(), &lab.store, rec_root.path(), &g.id, "ATLAS")
            .unwrap();
        rename_participant_in(lab.repos(), &lab.store, &g.id, "Aleksandra Nowak").unwrap();
        assert_eq!(
            cycles::get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Aleksandra Nowak"
        );
        assert_eq!(
            cycles::get_cycle(lab.repos(), "ATLAS")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Aleksandra Nowak"
        );
        let listed = list_all_voice_profiles_in(lab.repos(), &lab.store, rec_root.path()).unwrap();
        assert_eq!(
            listed.iter().find(|p| p.id == g.id).unwrap().name,
            "Aleksandra Nowak"
        );

        // Duplicate name in one of the projects rejects the rename and leaves all files unchanged.
        create_participant_in(lab.repos(), &lab.store, Some("ATLAS"), "Jan").unwrap();
        assert_eq!(
            rename_participant_in(lab.repos(), &lab.store, &g.id, "jan")
                .unwrap_err()
                .code,
            "duplicate"
        );
        assert_eq!(
            cycles::get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants[0]
                .name,
            "Aleksandra Nowak"
        );

        // Duplicate name check works for unlinked global profiles too.
        let g1 = create_participant_in(lab.repos(), &lab.store, None, "Marek").unwrap();
        let g2 = create_participant_in(lab.repos(), &lab.store, None, "Piotr").unwrap();
        assert_eq!(
            rename_participant_in(lab.repos(), &lab.store, &g2.id, "marek")
                .unwrap_err()
                .code,
            "duplicate"
        );
        assert_eq!(
            lab.store.get_participant(&g1.id).unwrap().unwrap().name,
            "Marek"
        );
    }

    #[test]
    fn add_existing_participant_to_cycle_duplicate_name_returns_domain_error() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let rec_root = test_tmp::TestDir::new("voice-dup-add");

        create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Ola").unwrap();
        let atlas_ola =
            create_participant_in(lab.repos(), &lab.store, Some("ATLAS"), "Ola").unwrap();
        let err = add_profile_to_project_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            &atlas_ola.id,
            "NOMAD",
        )
        .unwrap_err();
        assert_eq!(err.code, "duplicate");
    }

    #[test]
    fn tier2_auto_match_shows_name_links_on_confirm_and_tier1_suggestion_has_no_from_projects() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.make_cycle("ATLAS");
        let nomad_jan =
            create_participant_in(lab.repos(), &lab.store, Some("NOMAD"), "Jan").unwrap();
        let global_ewa =
            create_participant_in(lab.repos(), &lab.store, None, "Ewa Global").unwrap();
        let atlas_kasia =
            create_participant_in(lab.repos(), &lab.store, Some("ATLAS"), "Kasia").unwrap();

        lab.meeting("m1", Some("NOMAD"));
        let mut auto_global = voice("discord:SPEAKER_00");
        auto_global.decision = VoiceDecision::Auto;
        auto_global.score = Some(0.95);
        auto_global.matched_participant = Some(global_ewa.id.clone());
        auto_global.assigned_participant = Some(global_ewa.id.clone());

        let mut tier1_suggest = voice("discord:SPEAKER_01");
        tier1_suggest.decision = VoiceDecision::Suggested;
        tier1_suggest.score = Some(0.62);
        tier1_suggest.matched_participant = Some(nomad_jan.id.clone());

        let mut tier3_suggest = voice("discord:SPEAKER_02");
        tier3_suggest.decision = VoiceDecision::Suggested;
        tier3_suggest.score = Some(0.75);
        tier3_suggest.matched_participant = Some(atlas_kasia.id.clone());

        lab.store
            .put_meeting_voices("m1", &[auto_global, tier1_suggest, tier3_suggest])
            .unwrap();

        let data = voices_data_in(lab.repos(), &lab.store, "m1").unwrap();
        let v0 = data
            .voices
            .iter()
            .find(|v| v.voice_id == "discord:SPEAKER_00")
            .unwrap();
        assert_eq!(v0.assigned_participant_name.as_deref(), Some("Ewa Global"));
        let v1 = data
            .voices
            .iter()
            .find(|v| v.voice_id == "discord:SPEAKER_01")
            .unwrap();
        assert!(v1.suggested_from_projects.is_empty());
        let v2 = data
            .voices
            .iter()
            .find(|v| v.voice_id == "discord:SPEAKER_02")
            .unwrap();
        assert_eq!(v2.suggested_from_projects, vec!["ATLAS".to_string()]);

        // Confirm all automatic matches links the Tier 2 global profile into NOMAD.
        assert_eq!(
            confirm_matches_in(lab.repos(), &lab.store, "m1").unwrap(),
            1
        );
        let after = voices_data_in(lab.repos(), &lab.store, "m1").unwrap();
        assert!(after.participants.iter().any(|p| p.id == global_ewa.id));
        assert!(cycles::get_cycle(lab.repos(), "NOMAD")
            .unwrap()
            .cycle
            .participants
            .iter()
            .any(|p| p.name == "Ewa Global"));
    }

    #[test]
    fn remove_profile_from_project_handles_legacy_null_cycle_participant_id() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let rec_root = test_tmp::TestDir::new("voice-legacy-unlink");

        // Legacy row: participant id does not carry "NOMAD:" prefix -> cycle_participant_id is NULL.
        lab.store
            .create_participant_in(Some("NOMAD"), "legacy-p1", "Legacy Person", 1)
            .unwrap();
        let in_nomad = lab.store.participants_for_cycle("NOMAD").unwrap();
        assert_eq!(in_nomad.len(), 1);
        assert_eq!(in_nomad[0].cycle_participant_id, None);

        let after = remove_profile_from_project_in(
            lab.repos(),
            &lab.store,
            rec_root.path(),
            "legacy-p1",
            "NOMAD",
        )
        .unwrap();
        assert!(after.projects.is_empty());
        assert!(lab
            .store
            .participants_for_cycle("NOMAD")
            .unwrap()
            .is_empty());
    }
}
