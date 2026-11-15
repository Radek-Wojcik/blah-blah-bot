//! IPC commands of publishing. This file holds the secret and access commands; the rules are
//! in `publish_secrets.rs`, so each is tested without Tauri. Values of secrets only ever
//! travel from the UI to the credential store: no command returns one.

use tauri::{AppHandle, Emitter, State};

use crate::cycle_commands::EVENT_CYCLES;
use crate::cycles::{self, RepoDto};
use crate::ipc::{IpcError, IpcResult};
use crate::publish_secrets::{self, GithubAccessDto, PublishingSecretsDto, WebhookCheckDto};
use crate::secrets::Credentials;
use crate::state::AppState;
use publishers::http::HttpClient;

fn join_error(e: impl std::fmt::Display) -> IpcError {
    IpcError::new("internal", format!("the background task failed: {e}"))
}

/// Which GitHub tokens and target secrets are set (never their values), with the GitHub API
/// hosts of the registered repositories and the `cred:` references of the cycles.
#[tauri::command]
pub fn publishing_secrets(state: State<'_, AppState>) -> PublishingSecretsDto {
    publish_secrets::publishing_secrets(&Credentials, &state.cycle_repos())
}

/// Stores the GitHub token for the host of `api` (the public GitHub when absent).
#[tauri::command]
pub fn set_github_token(token: String, api: Option<String>) -> IpcResult<()> {
    publish_secrets::set_github_token(&Credentials, api.as_deref(), &token)
}

#[tauri::command]
pub fn clear_github_token(api: Option<String>) -> IpcResult<()> {
    publish_secrets::clear_github_token(&Credentials, api.as_deref())
}

/// Checks the stored token against the GitHub repository of a cycle. A failure is a result
/// with a message, not an error.
#[tauri::command]
pub async fn test_github_access(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<GithubAccessDto> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::test_github_access(&Credentials, &repos, &cycle_id, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}

/// Links a registered repository to a GitHub repository (`owner/name`) and API address, or
/// clears the link with empty values.
#[tauri::command]
pub fn set_github_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    repo_id: String,
    github_repo: Option<String>,
    github_api: Option<String>,
) -> IpcResult<RepoDto> {
    let setting = state.update_settings(|s| {
        publish_secrets::set_github_repo(
            &mut s.cycle_repos,
            &repo_id,
            github_repo.as_deref(),
            github_api.as_deref(),
        )
    })?;
    if let Err(e) = app.emit(EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the cycles change");
    }
    Ok(cycles::repo_dto(&setting))
}

/// Stores the value behind a `cred:` reference of a cycle (a Discord webhook is validated
/// first).
#[tauri::command]
pub fn set_target_secret(
    state: State<'_, AppState>,
    reference: String,
    value: String,
) -> IpcResult<()> {
    publish_secrets::set_target_secret(&Credentials, &state.cycle_repos(), &reference, &value)
}

#[tauri::command]
pub fn clear_target_secret(reference: String) -> IpcResult<()> {
    publish_secrets::clear_target_secret(&Credentials, &reference)
}

/// Asks Discord whether the stored webhook exists (a `GET`, nothing is posted).
#[tauri::command]
pub async fn test_discord_webhook(reference: String) -> IpcResult<WebhookCheckDto> {
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::test_discord_webhook(&Credentials, &reference, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}
