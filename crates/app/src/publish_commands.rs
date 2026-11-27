//! IPC commands of publishing: the secret and access commands (rules in `publish_secrets.rs`)
//! and the publication itself (plan, publish, confirm a public repository, the outbox, the
//! tasks of a cycle; rules in `publishing.rs`), each tested without Tauri. Values of secrets
//! only ever travel from the UI to the credential store: no command returns one.

use tauri::{AppHandle, Emitter, State};

use crate::cycle_commands::EVENT_CYCLES;
use crate::cycles::{self, CycleDto, DiscordRefDto, NotionPropertiesDto, RepoDto, TargetsDto};
use crate::ipc::{IpcError, IpcResult};
use crate::publish_secrets::{
    self, DiscordBotCheckDto, DiscordMemberDto, DiscordRoleDto, GithubAccessDto, NotionAccessDto,
    NotionSchemaDto, PublishingSecretsDto, WebhookCheckDto,
};
use crate::publishing::{
    ConfirmDto, CorrectionsDto, CycleTasksDto, OutboxStateDto, PlanDto, PublishCorrectionsDto,
    PublishResultDto,
};
use crate::secrets::Credentials;
use crate::state::AppState;
use cycle_store::PublishMode;
use publishers::http::HttpClient;

fn join_error(e: impl std::fmt::Display) -> IpcError {
    crate::ipc::background_task_error(&e)
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

/// Sets when a target of a cycle is published (`targets.<target>.publish` of `cycle.yaml`:
/// `auto`, `on-approval` or `off`) and returns the cycle's targets as saved. Status changes
/// are never written back automatically with any value, and the public-repository check
/// applies to all of them.
#[tauri::command]
pub fn set_target_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    target: String,
    mode: PublishMode,
) -> IpcResult<TargetsDto> {
    let targets = cycles::set_target_rule(&state.cycle_repos(), &cycle_id, &target, mode)?;
    if let Err(e) = app.emit(EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the cycles change");
    }
    Ok(targets)
}

// ----- Notion and Discord bot setup -------------------------------------------------------

/// Asks Notion whether the stored token can reach the database of the cycle's Notion target.
/// Nothing is written. Expected failures are returned as a result with a message.
#[tauri::command]
pub async fn test_notion_access(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<NotionAccessDto> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::test_notion_access(&Credentials, &repos, &cycle_id, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}

/// Reads the properties of the cycle's Notion data source, proposes a field mapping and lists
/// any problems with the current mapping.
#[tauri::command]
pub async fn notion_schema(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<NotionSchemaDto> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::notion_schema(&Credentials, &repos, &cycle_id, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}

/// Writes `targets.notion.data_source` and `targets.notion.properties` of the cycle and
/// nothing else, returning the cycle's targets as saved.
#[tauri::command]
pub fn set_notion_mapping(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    data_source: Option<String>,
    properties: NotionPropertiesDto,
) -> IpcResult<TargetsDto> {
    let targets = cycles::set_notion_mapping(
        &state.cycle_repos(),
        &cycle_id,
        data_source.as_deref(),
        properties,
    )?;
    if let Err(e) = app.emit(EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the cycles change");
    }
    Ok(targets)
}

/// Checks the Discord bot token against the guild and channel of the cycle's Discord bot
/// target and lists any missing channel permissions. Nothing is posted.
#[tauri::command]
pub async fn test_discord_bot(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<DiscordBotCheckDto> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::test_discord_bot(&Credentials, &repos, &cycle_id, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}

/// Searches Discord guild members whose username or nickname starts with `query` (at least 2
/// characters, at most 10 results).
#[tauri::command]
pub async fn search_discord_members(
    state: State<'_, AppState>,
    cycle_id: String,
    query: String,
) -> IpcResult<Vec<DiscordMemberDto>> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::search_discord_members(
            &Credentials,
            &repos,
            &cycle_id,
            &query,
            HttpClient::new(),
        )
    })
    .await
    .map_err(join_error)?
}

/// Lists the roles of the Discord server (excluding `@everyone`), for mapping a participant
/// to a role mention.
#[tauri::command]
pub async fn list_discord_roles(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<Vec<DiscordRoleDto>> {
    let repos = state.cycle_repos();
    tauri::async_runtime::spawn_blocking(move || {
        publish_secrets::list_discord_roles(&Credentials, &repos, &cycle_id, HttpClient::new())
    })
    .await
    .map_err(join_error)?
}

/// Writes `participants[].discord` of one participant and nothing else (`None` removes the
/// mapping), returning the cycle as saved.
#[tauri::command]
pub fn set_participant_discord(
    app: AppHandle,
    state: State<'_, AppState>,
    cycle_id: String,
    participant_id: String,
    discord: Option<DiscordRefDto>,
) -> IpcResult<CycleDto> {
    let cycle =
        cycles::set_participant_discord(&state.cycle_repos(), &cycle_id, &participant_id, discord)?;
    if let Err(e) = app.emit(EVENT_CYCLES, ()) {
        tracing::warn!(error = %e, "could not emit the cycles change");
    }
    Ok(cycle)
}

/// Re-exports the tasks of a published meeting to Notion: updates mapped fields on existing
/// pages while preserving their status, and leaves trashed pages untouched.
#[tauri::command]
pub async fn reexport_notion(
    state: State<'_, AppState>,
    meeting_id: String,
) -> IpcResult<PublishResultDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || publishing.reexport_notion(&meeting_id))
        .await
        .map_err(join_error)?
}

// ----- the publication ---------------------------------------------------------------------

/// What publishing the meeting to `targets` (all of the cycle's when absent) would do: the
/// files, issues, status changes and message, the rules of each target, the visibility of the
/// repository and whether it needs the user's confirmation. Changes nothing.
#[tauri::command]
pub async fn publish_plan(
    state: State<'_, AppState>,
    meeting_id: String,
    targets: Option<Vec<String>>,
) -> IpcResult<PlanDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || {
        publishing.plan(&meeting_id, &targets.unwrap_or_default())
    })
    .await
    .map_err(join_error)?
}

/// Queues the publication of the meeting to `targets` (`github`, `discord`). A public or
/// unknown repository is refused with `public_repo_unconfirmed` until `confirm_public_repo`.
/// From then on the draft is read-only.
#[tauri::command]
pub async fn publish_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    targets: Vec<String>,
) -> IpcResult<PublishResultDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || publishing.publish(&meeting_id, &targets))
        .await
        .map_err(join_error)?
}

/// The user accepts publishing to the repository of the cycle although it is (or may be)
/// public. Remembered for that repository.
#[tauri::command]
pub async fn confirm_public_repo(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<ConfirmDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || publishing.confirm_public_repo(&cycle_id))
        .await
        .map_err(join_error)?
}

/// Every operation of the outbox with its state, and the targets that are offline.
#[tauri::command]
pub fn outbox_state(state: State<'_, AppState>) -> IpcResult<OutboxStateDto> {
    state.publishing().state()
}

/// Retries one operation now, or (without an id) everything that failed or waits for its
/// backoff.
#[tauri::command]
pub fn retry_outbox_op(state: State<'_, AppState>, id: Option<i64>) -> IpcResult<OutboxStateDto> {
    state.publishing().retry(id)
}

/// Drops a queued or failed operation.
#[tauri::command]
pub fn cancel_outbox_op(state: State<'_, AppState>, id: i64) -> IpcResult<OutboxStateDto> {
    state.publishing().cancel(id)
}

/// Open and done tasks of a cycle as the last read of its GitHub issues has them.
#[tauri::command]
pub fn cycle_tasks(state: State<'_, AppState>, cycle_id: String) -> IpcResult<CycleTasksDto> {
    state.publishing().cycle_tasks(&cycle_id)
}

// ----- corrections to a published meeting -------------------------------------------------

/// The corrections written for a meeting after it was published, and where each stands.
#[tauri::command]
pub fn list_corrections(
    state: State<'_, AppState>,
    meeting_id: String,
) -> IpcResult<CorrectionsDto> {
    state.publishing().list_corrections(&meeting_id)
}

/// Writes a correction note for a published meeting (text, optional item and issue, and the
/// day it was written, today by default). Under an `auto` rule of the `github` target it is
/// queued at once.
#[tauri::command]
pub async fn add_correction(
    state: State<'_, AppState>,
    meeting_id: String,
    text: String,
    item: Option<String>,
    issue: Option<String>,
    date: Option<String>,
) -> IpcResult<CorrectionsDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || {
        publishing.add_correction(
            &meeting_id,
            &text,
            item.as_deref(),
            issue.as_deref(),
            date.as_deref(),
        )
    })
    .await
    .map_err(join_error)?
}

/// Queues corrections for publishing: a new file in the meeting's folder of the repository,
/// and a comment on the issue it is about. The same rule and the same public-repository
/// check as for the meeting. Without ids, every correction that is not queued yet.
#[tauri::command]
pub async fn publish_corrections(
    state: State<'_, AppState>,
    meeting_id: String,
    ids: Option<Vec<String>>,
) -> IpcResult<PublishCorrectionsDto> {
    let publishing = state.publishing().clone();
    tauri::async_runtime::spawn_blocking(move || {
        publishing.publish_corrections(&meeting_id, &ids.unwrap_or_default())
    })
    .await
    .map_err(join_error)?
}

/// Deletes a correction that was never queued.
#[tauri::command]
pub fn delete_correction(
    state: State<'_, AppState>,
    meeting_id: String,
    id: String,
) -> IpcResult<CorrectionsDto> {
    state.publishing().delete_correction(&meeting_id, &id)
}
