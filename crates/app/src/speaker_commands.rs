//! IPC commands for speakers: detected voices, naming, merging and voice profiles.
//!
//! Voice profiles, embeddings and sample clips are biometric data. They are kept in the local
//! database and recordings folder only; no command here sends them anywhere.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use queue::voices;
use serde::{Deserialize, Serialize};
use store::{ParticipantInfo, SampleRow, Store, VoiceDecision, VoiceRow};
use tauri::ipc::Response;
use tauri::State;

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
    /// The participant the voice is named after (automatic match or the user's choice).
    pub assigned_participant_id: Option<String>,
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
            cycle_participant_id: p.cycle_participant_id().map(str::to_string),
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

fn voices_data(state: &AppState, id: &str) -> IpcResult<VoicesData> {
    let repos = state.cycle_repos();
    let participants = state.with_store_ipc(|s| participants_for(&repos, s, None, Some(id)))?;
    let (voices, clips) =
        state.with_store(|s| Ok((s.list_meeting_voices(id)?, s.list_clips(id)?)))?;
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
    if info.cycle_id.is_some() {
        return cycles::rename_cycle_participant(repos, store, &info, &name);
    }
    let renamed = store
        .rename_participant(participant_id, &name)
        .map_err(|_| IpcError::new("duplicate", "a participant with this name already exists"))?;
    if renamed {
        Ok(())
    } else {
        Err(IpcError::new("not_found", "participant not found"))
    }
}

fn delete_participant_in(
    repos: &[CycleRepoSetting],
    store: &Store,
    participant_id: &str,
) -> IpcResult<()> {
    match cycles::find_participant(store, participant_id)? {
        Some(info) if info.cycle_id.is_some() => {
            cycles::delete_cycle_participant(repos, store, &info)
        }
        _ => {
            store.delete_participant(participant_id)?;
            Ok(())
        }
    }
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
    state: State<'_, AppState>,
    name: String,
    cycle_id: Option<String>,
) -> IpcResult<ParticipantDto> {
    let repos = state.cycle_repos();
    state.with_store_ipc(|s| create_participant_in(&repos, s, cycle_id.as_deref(), &name))
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
    state.with_store_ipc(|s| delete_participant_in(&repos, s, &participant_id))
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
    let cycle_items = cycles::list_cycles(repos);
    let mut projects_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for item in &cycle_items {
        let _ = cycles::sync_cycle(repos, store, &item.id);
        if let Ok(details) = cycles::get_cycle(repos, &item.id) {
            for p in details.cycle.participants {
                projects_by_name
                    .entry(p.name.trim().to_lowercase())
                    .or_default()
                    .insert(item.id.clone());
            }
        }
    }

    let all = store.list_all_participants_with_profiles()?;
    for p in &all {
        if let Some(cid) = &p.cycle_id {
            projects_by_name
                .entry(p.name.trim().to_lowercase())
                .or_default()
                .insert(cid.clone());
        }
    }

    let mut out = Vec::with_capacity(all.len());
    for p in all {
        let mut projects = BTreeSet::new();
        if let Some(cid) = &p.cycle_id {
            projects.insert(cid.clone());
        }
        if let Some(extra) = projects_by_name.get(&p.name.trim().to_lowercase()) {
            projects.extend(extra.iter().cloned());
        }
        let updated_unix_ms = store
            .get_participant(&p.id)?
            .map_or(0, |row| row.created_unix_ms);
        let clip_on_disk = p
            .sample_clip_file
            .as_deref()
            .is_some_and(|rel| !rel.trim().is_empty() && recordings_root.join(rel).is_file())
            || voices::participant_sample_clip_path(recordings_root, &p.id).is_file();
        let has_sample_clip = clip_on_disk
            || p.sample_clip_file
                .as_deref()
                .is_some_and(|rel| !rel.trim().is_empty());

        out.push(GlobalVoiceProfileDto {
            id: p.id,
            name: p.name,
            cycle_id: p.cycle_id,
            projects: projects.into_iter().collect(),
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
) -> IpcResult<Vec<u8>> {
    if let Some(p) = store.get_participant(participant_id)? {
        if let Some(rel) = p.sample_clip_file.as_deref() {
            let candidate = recordings_root.join(rel);
            if candidate.is_file() {
                return Ok(std::fs::read(candidate)?);
            }
        }
    }
    let path = voices::participant_sample_clip_path(recordings_root, participant_id);
    if path.is_file() {
        return Ok(std::fs::read(path)?);
    }
    Ok(generate_demo_wav_bytes(440.0))
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
    state.with_store_ipc(|s| voice_profile_sample_clip_in(s, &root, &participant_id))
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
        delete_participant_in(lab.repos(), &lab.store, &a.id).unwrap();
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
        delete_participant_in(lab.repos(), &lab.store, &old.id).unwrap();
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

        // Add the same person to ATLAS cycle too so `projects` spans both.
        create_participant_in(lab.repos(), &lab.store, Some("ATLAS"), "Kasia").unwrap();

        let all = list_all_voice_profiles_in(lab.repos(), &lab.store, rec_root.path()).unwrap();
        let kasia_nomad = all.iter().find(|p| p.id == enrolled.id).unwrap();
        assert_eq!(
            kasia_nomad.projects,
            vec!["ATLAS".to_string(), "NOMAD".to_string()]
        );

        let clip_bytes =
            voice_profile_sample_clip_in(&lab.store, rec_root.path(), &enrolled.id).unwrap();
        assert!(clip_bytes.starts_with(b"RIFF"));
        assert!(clip_bytes.len() > 44);
    }
}
