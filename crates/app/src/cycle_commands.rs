//! IPC commands for cycles: repositories, the cycle editor, templates and the report mode
//! gate. The rules live in `cycles.rs`; these wrappers take the state, call them and tell the
//! UI that something changed.

use cycle_store::Repo;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::cycles::{
    self, CycleDetails, CycleDto, CycleListItem, PullResult, RepoDto, ReportDefaults,
    SaveCycleResult, TemplateCheck, TemplateDto, TemplateSummaryDto,
};
use crate::ipc::{IpcError, IpcResult};
use crate::settings::RepoKind;
use crate::state::AppState;

pub const EVENT_CYCLES: &str = "cycles-changed";

fn changed(app: &AppHandle) {
    if let Err(e) = app.emit(EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the cycles change");
    }
}

#[tauri::command]
pub fn list_cycles(state: State<'_, AppState>) -> Vec<CycleListItem> {
    cycles::list_cycles(&state.cycle_repos())
}

#[tauri::command]
pub fn list_cycle_repos(state: State<'_, AppState>) -> Vec<RepoDto> {
    cycles::list_repos(&state.cycle_repos())
}

#[tauri::command]
pub fn get_cycle(state: State<'_, AppState>, cycle_id: String) -> IpcResult<CycleDetails> {
    cycles::get_cycle(&state.cycle_repos(), &cycle_id)
}

/// Creates a cycle. Without `repo_id` it goes to the default local repository, which is
/// created on first use.
#[tauri::command]
pub fn create_cycle(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
    repo_id: Option<String>,
) -> IpcResult<CycleDetails> {
    let data_dir = state.data_dir().to_path_buf();
    let target =
        state.update_settings(|s| cycles::resolve_target_repo(s, repo_id.as_deref(), &data_dir))?;
    let repos = state.cycle_repos();
    // The files are written (and Git asked about them) without the database locked.
    let details = cycles::create_cycle(&repos, &target, id.trim(), &name)?;
    cycles::sync_cycle(&repos, state.inner(), id.trim())?;
    changed(&app);
    Ok(details)
}

/// Saves `cycle.yaml` when it is valid; otherwise returns the issues and writes nothing.
#[tauri::command]
pub fn save_cycle(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle: CycleDto,
) -> IpcResult<SaveCycleResult> {
    let repos = state.cycle_repos();
    // Writing `cycle.yaml` may wait for a pull of the repository: the database is locked
    // only for the participant sync inside.
    let result = cycles::save_cycle(&repos, state.inner(), cycle)?;
    if result.saved {
        changed(&app);
    }
    Ok(result)
}

#[tauri::command]
pub fn delete_project(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    delete_meetings: bool,
    delete_folder: bool,
) -> IpcResult<()> {
    let root = state.recordings_root().ok();
    let recording = state.recorder.is_recording();
    let jobs = state.processing().job_states();
    let data_dir = state.data_dir().to_path_buf();
    // Settings and database change first, under their locks; the folder goes last and
    // outside every lock (the Shell call can take long or ask the user).
    let folder = state.update_settings(|s| {
        state.with_store_ipc(|store| {
            cycles::ensure_project_idle(store, &cycle_id, recording, &jobs)?;
            cycles::delete_project(
                s,
                store,
                root.as_deref(),
                &data_dir,
                &cycle_id,
                delete_meetings,
                delete_folder,
            )
        })
    })?;
    changed(&app);
    crate::commands::emit_meetings_changed(&app);
    if let Some(folder) = folder {
        cycles::move_to_recycle_bin(&folder).map_err(|e| {
            IpcError::new(
                e.code,
                format!(
                    "the project was removed but its folder was kept: {}",
                    e.message
                ),
            )
        })?;
    }
    Ok(())
}

#[tauri::command]
pub fn list_templates(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<Vec<TemplateSummaryDto>> {
    cycles::list_templates(&state.cycle_repos(), &cycle_id)
}

#[tauri::command]
pub fn get_template(
    state: State<'_, AppState>,
    cycle_id: String,
    name: String,
) -> IpcResult<TemplateDto> {
    cycles::get_template(&state.cycle_repos(), &cycle_id, &name)
}

/// Writes a template when it is valid; otherwise returns the issues.
#[tauri::command]
pub fn save_template(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    template: TemplateDto,
) -> IpcResult<TemplateCheck> {
    let check = cycles::save_template(&state.cycle_repos(), &cycle_id, &template)?;
    if check.saved {
        changed(&app);
    }
    Ok(check)
}

/// Checks a template (sections and placeholders) without writing it.
#[tauri::command]
pub fn validate_template(template: TemplateDto) -> TemplateCheck {
    cycles::validate_template(&template)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRepoRequest {
    pub kind: RepoKind,
    /// `clone`: the address to clone.
    #[serde(default)]
    pub url: Option<String>,
    /// `local`: an existing folder.
    #[serde(default)]
    pub path: Option<String>,
}

fn join_error(e: impl std::fmt::Display) -> IpcError {
    crate::ipc::background_task_error(&e)
}

/// Registers a repository: an existing folder (`local`) or a clone of a remote one
/// (`clone`). Cloning runs in the background and never blocks the window.
#[tauri::command]
pub async fn add_cycle_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    req: AddRepoRequest,
) -> IpcResult<RepoDto> {
    let setting = match req.kind {
        RepoKind::Local => {
            let path = req.path.unwrap_or_default();
            state.update_settings(|s| cycles::add_local_repo(s, &path))?
        }
        RepoKind::Clone => {
            let url = cycles::validate_remote(&req.url.unwrap_or_default())?;
            let (id, dest) = cycles::plan_clone(&state.settings()?, state.data_dir(), &url);
            let (clone_url, clone_dest) = (url.clone(), dest.clone());
            tauri::async_runtime::spawn_blocking(move || Repo::clone(&clone_url, &clone_dest))
                .await
                .map_err(join_error)??;
            state.update_settings(|s| Ok(cycles::register_clone(s, &id, &dest, &url)))?
        }
    };
    // Cycles that came with the repository get their participants right away.
    for note in cycles::sync_repo(&setting, state.inner()) {
        tracing::warn!("{note}");
    }
    changed(&app);
    Ok(cycles::repo_dto(&setting))
}

/// `git pull --rebase` of one repository. A failure (offline, conflict) is reported in the
/// result and the working tree stays as it was. With uncommitted cycle edits nothing is
/// pulled and the error code is `repo_dirty`.
#[tauri::command]
pub async fn pull_cycle_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    repo_id: String,
) -> IpcResult<PullResult> {
    let setting = state
        .cycle_repos()
        .into_iter()
        .find(|r| r.id == repo_id)
        .ok_or_else(|| IpcError::new("not_found", "this cycle repository is not registered"))?;
    let task = setting.clone();
    let mut result = tauri::async_runtime::spawn_blocking(move || {
        let mut result = cycles::pull_repo(&task)?;
        // The state of the issues is part of what a pull brings up to date.
        result
            .warnings
            .extend(crate::issue_sync::refresh_repo_issues(
                &crate::secrets::Credentials,
                &task,
                crate::issue_sync::quick_client,
            ));
        Ok::<_, IpcError>(result)
    })
    .await
    .map_err(join_error)??;
    result
        .warnings
        .extend(cycles::sync_repo(&setting, state.inner()));
    changed(&app);
    Ok(result)
}

/// What the report mode gate offers for a cycle: the cycle's defaults, the allowed templates
/// and meeting types, and the last choice when it was made for this cycle.
#[tauri::command]
pub fn report_defaults(state: State<'_, AppState>, cycle_id: String) -> IpcResult<ReportDefaults> {
    let last = state.settings()?.last_report;
    cycles::report_defaults(&state.cycle_repos(), &cycle_id, last.as_ref())
}
