//! IPC commands for speakers: detected voices, naming, merging and voice profiles.
//!
//! Voice profiles, embeddings and sample clips are biometric data. They are kept in the local
//! database and recordings folder only; no command here sends them anywhere.

use std::sync::atomic::{AtomicU64, Ordering};

use queue::voices;
use serde::Serialize;
use store::{ParticipantInfo, Store, VoiceDecision, VoiceRow};
use tauri::ipc::Response;
use tauri::State;

use crate::cycles;
use crate::ipc::{IpcError, IpcResult};
use crate::settings::CycleRepoSetting;
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
}
