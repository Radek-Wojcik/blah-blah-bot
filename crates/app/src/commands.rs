//! IPC commands exposed to the UI.

use tauri::{AppHandle, Emitter, State};

use crate::cycles;
use crate::hotkeys;
use crate::ipc::{IpcError, IpcResult, MeetingDetails, MeetingSummary};
use crate::overlay;
use crate::recording_dto::{RecordingState, SourceInfo, StartRecordingRequest};
use crate::settings::Settings;
use crate::state::AppState;

pub const EVENT_STATE: &str = "recording-state";
pub const EVENT_MEETINGS: &str = "meetings-changed";

pub fn emit_state(app: &AppHandle, state: &RecordingState) {
    if let Err(e) = app.emit(EVENT_STATE, state) {
        tracing::warn!(error = %e, "could not emit recording state");
    }
}

pub fn emit_meetings_changed(app: &AppHandle) {
    if let Err(e) = app.emit(EVENT_MEETINGS, ()) {
        tracing::warn!(error = %e, "could not emit meetings change");
    }
}

#[tauri::command]
pub fn list_meetings(state: State<'_, AppState>) -> IpcResult<Vec<MeetingSummary>> {
    let jobs = state.processing().job_states();
    state.with_store(|s| s.list_summaries()).map(|v| {
        v.into_iter()
            .map(|s| {
                let mut summary: MeetingSummary = s.into();
                summary.job = jobs.get(&summary.id).copied();
                summary
            })
            .collect()
    })
}

#[tauri::command]
pub fn get_meeting(state: State<'_, AppState>, id: String) -> IpcResult<MeetingDetails> {
    state
        .with_store(|s| s.get_meeting(&id))?
        .map(MeetingDetails::from)
        .map(|mut d| {
            d.summary.job = state.processing().job_states().get(&d.summary.id).copied();
            d
        })
        .ok_or_else(|| IpcError::new("internal", "meeting not found"))
}

#[tauri::command]
pub fn delete_meeting(app: AppHandle, state: State<'_, AppState>, id: String) -> IpcResult<()> {
    // Sample clips are voice recordings: remove the files together with the meeting.
    let root = state.recordings_root()?;
    state.with_store(|s| queue::voices::remove_clip_files(s, &id, &root).map(|_| ()))?;
    state.with_store(|s| s.delete_meeting(&id))?;
    emit_meetings_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn recover_meeting(app: AppHandle, state: State<'_, AppState>, id: String) -> IpcResult<()> {
    let root = state.recordings_root()?;
    state.recorder.recover_meeting(&root, &id)?;
    emit_meetings_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn list_sources(state: State<'_, AppState>) -> IpcResult<Vec<SourceInfo>> {
    state.recorder.list_sources()
}

#[tauri::command]
pub fn recording_state(state: State<'_, AppState>) -> RecordingState {
    state.recorder.current()
}

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    req: StartRecordingRequest,
) -> IpcResult<()> {
    // The report mode gate: no cycle and mode, no recording. Every way to start a recording
    // (the main window, the widget, the tray) arrives here.
    let settings = state.settings()?;
    let repos = state.cycle_repos();
    let report = cycles::prepare_recording(
        &repos,
        req.report.clone(),
        req.use_last_report,
        settings.last_report.as_ref(),
    )?;
    let root = state.recordings_root()?;
    state.recorder.start(&root, req, report.clone())?;
    // Best effort from here on: the recording is running.
    if let Some(last) = cycles::last_report_of(&report) {
        if let Err(e) = state.update_settings(|s| {
            s.last_report = Some(last);
            Ok(())
        }) {
            tracing::warn!(error = %e, "could not remember the report settings");
        }
    }
    if let Some(cycle) = &report.cycle_id {
        if let Err(e) = cycles::sync_cycle(&repos, state.inner(), cycle) {
            tracing::warn!(error = %e, "could not sync the cycle participants");
        }
    }
    emit_meetings_changed(&app);
    if let Some(s) = state.recorder.tick() {
        emit_state(&app, &s);
    }
    Ok(())
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, state: State<'_, AppState>) -> IpcResult<()> {
    state.recorder.stop()?;
    overlay::cancel(&app);
    emit_state(&app, &RecordingState::idle());
    emit_meetings_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn add_source(app: AppHandle, state: State<'_, AppState>, source_id: String) -> IpcResult<()> {
    state.recorder.add_source(&source_id)?;
    if let Some(s) = state.recorder.tick() {
        emit_state(&app, &s);
    }
    Ok(())
}

#[tauri::command]
pub fn remove_source(
    app: AppHandle,
    state: State<'_, AppState>,
    source_id: String,
) -> IpcResult<()> {
    state.recorder.remove_source(&source_id)?;
    if let Some(s) = state.recorder.tick() {
        emit_state(&app, &s);
    }
    Ok(())
}

#[tauri::command]
/// Async on purpose: creating a window from a synchronous command deadlocks on Windows.
pub async fn take_screenshot(app: AppHandle) -> IpcResult<()> {
    overlay::begin(&app)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> IpcResult<Settings> {
    state.settings()
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> IpcResult<()> {
    // A folder that is refused must not leave new hotkeys registered.
    settings
        .check_new_locations(&state.settings()?)
        .map_err(|p| IpcError::new("invalid", p.message))?;
    // Register first: a hotkey that is taken must not be saved.
    hotkeys::register(&app, &settings)?;
    state.save_settings(settings)
}
