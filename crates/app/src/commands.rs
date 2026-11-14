//! IPC commands exposed to the UI.

use tauri::State;

use crate::ipc::{IpcError, IpcResult, MeetingDetails, MeetingSummary};
use crate::settings::Settings;
use crate::state::AppState;

#[tauri::command]
pub fn list_meetings(state: State<'_, AppState>) -> IpcResult<Vec<MeetingSummary>> {
    state
        .with_store(|s| s.list_summaries())
        .map(|v| v.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn get_meeting(state: State<'_, AppState>, id: String) -> IpcResult<MeetingDetails> {
    state
        .with_store(|s| s.get_meeting(&id))?
        .map(Into::into)
        .ok_or_else(|| IpcError::new("internal", "meeting not found"))
}

#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, id: String) -> IpcResult<()> {
    state.with_store(|s| s.delete_meeting(&id)).map(|_| ())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> IpcResult<Settings> {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> IpcResult<()> {
    state.save_settings(settings)
}
