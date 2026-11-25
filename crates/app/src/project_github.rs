//! The optional GitHub step of the project wizard: creating a **private** repository for a new
//! project through the GitHub REST API and making it the remote of the project's local Git
//! folder.
//!
//! Rules: nothing leaves the machine before the person confirmed in the wizard; the token comes
//! from the Credential Manager entry of the API host (the same entry Settings writes) and is
//! never returned, logged or part of a message; and every failure (no token, a rejected token,
//! a name that is taken, no connection) ends in the same place: the local project is created
//! anyway and the answer carries a clear message. The commands never throw for those cases.

use std::path::Path;

use cycle_store::Repo;
use publishers::github::{
    self, create_failure_message, CreatedRepo, Github, NewRepo, RepoRef, DEFAULT_API,
};
use publishers::http::{parse_endpoint, HttpClient};
use serde::{Deserialize, Serialize};
use store::Store;
use tauri::{AppHandle, Emitter, State};

use crate::cycle_commands::EVENT_CYCLES;
use crate::cycles::{self, CycleDto, ParticipantSync};
use crate::ipc::{background_task_error, IpcError, IpcResult};
use crate::project_commands::{
    validate_wizard_basics, wizard_create_project_in, WizardCreateProjectDto,
};
use crate::publish_secrets::scrub_with;
use crate::secrets::{self, Credentials, SecretStore};
use crate::settings::Settings;
use crate::state::AppState;

/// What the wizard needs to know before it offers the step: whether a token is saved. Never the
/// token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoStatusDto {
    pub token_set: bool,
    /// The API host the token belongs to, for the explanation in the wizard.
    pub host: String,
}

/// The repository the person confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoRequestDto {
    /// Repository name (without the owner).
    pub name: String,
    /// An organization to create it in; empty or absent for the token's own account.
    #[serde(default)]
    pub owner: Option<String>,
    /// The person pressed the confirm button. Without it nothing is sent.
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubOutcomeStatus {
    /// The private repository exists and is the project's remote.
    Created,
    /// Only the local Git folder was made; `reason` and `message` say why.
    LocalOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubFallbackReason {
    NoToken,
    NotConfirmed,
    InvalidName,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubOutcomeDto {
    pub status: GithubOutcomeStatus,
    pub reason: Option<GithubFallbackReason>,
    /// A sentence for the person, free of secrets.
    pub message: String,
    /// `owner/name` when created.
    pub repo: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardCreateResultDto {
    pub cycle: CycleDto,
    pub github: GithubOutcomeDto,
}

/// What the network step of the wizard produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Created {
        repo: CreatedRepo,
        /// The API the repository was created on.
        api: String,
    },
    Fallback {
        reason: GithubFallbackReason,
        message: String,
    },
}

fn fallback(reason: GithubFallbackReason, message: impl Into<String>) -> Attempt {
    Attempt::Fallback {
        reason,
        message: message.into(),
    }
}

/// Whether a token is saved for `api`. A lookup in the credential store, no network.
pub fn repo_status_in(secrets: &dyn SecretStore, api: &str) -> IpcResult<GithubRepoStatusDto> {
    let entry = secrets::github_token_entry(api)?;
    Ok(GithubRepoStatusDto {
        token_set: secrets.get(&entry).is_some(),
        host: github::api_host(api).unwrap_or_default(),
    })
}

/// Creates the private repository, or says why it did not. Runs on a worker thread: it talks
/// to the network.
pub fn attempt_create(
    secrets: &dyn SecretStore,
    api: &str,
    client: HttpClient,
    req: &GithubRepoRequestDto,
) -> Attempt {
    let owner = req
        .owner
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty());
    let name = req.name.trim();
    if RepoRef::parse(&format!("{}/{name}", owner.unwrap_or("owner"))).is_none()
        || name.ends_with(".git")
    {
        return fallback(
            GithubFallbackReason::InvalidName,
            "The GitHub repository name may contain only letters, digits, - _ and ., so only a local project folder was created.",
        );
    }
    if !req.confirmed {
        return fallback(
            GithubFallbackReason::NotConfirmed,
            "The GitHub repository was not confirmed, so only a local project folder was created.",
        );
    }
    let Ok(entry) = secrets::github_token_entry(api) else {
        return fallback(
            GithubFallbackReason::Failed,
            "The GitHub address is not valid, so only a local project folder was created.",
        );
    };
    let Some(token) = secrets.get(&entry) else {
        return fallback(
            GithubFallbackReason::NoToken,
            "No GitHub token is saved, so only a local project folder was created. Save a token in Settings > Connectors to create the repository next time.",
        );
    };
    let failed = |why: String| {
        fallback(
            GithubFallbackReason::Failed,
            scrub_with(
                &[&token],
                &format!("{why}. Only a local project folder was created."),
            ),
        )
    };
    let github = match Github::with_client(client, api, &token) {
        Ok(g) => g,
        Err(e) => return failed(e.message),
    };
    // An owner that is the token's own login is the personal account, not an organization.
    let owner = match owner {
        Some(wanted) => match github.current_user() {
            Ok(login) if login.eq_ignore_ascii_case(wanted) => None,
            Ok(_) => Some(wanted),
            Err(e) => {
                let spec = NewRepo {
                    owner: None,
                    name,
                    description: None,
                };
                return failed(create_failure_message(&e, &spec));
            }
        },
        None => None,
    };
    let spec = NewRepo {
        owner,
        name,
        description: Some("Meeting notes and reports, kept by Blah Blah Bot"),
    };
    let created = match github.create_private_repo(&spec) {
        Ok(created) => created,
        Err(e) => return failed(create_failure_message(&e, &spec)),
    };
    if !created.private {
        return failed(format!(
            "GitHub created {} but did not report it as private, so it was not linked; check its visibility on GitHub",
            created.full_name()
        ));
    }
    if parse_endpoint(&created.clone_url).is_err() {
        return failed(format!(
            "GitHub created {} but gave an address Git will not use",
            created.full_name()
        ));
    }
    Attempt::Created {
        repo: created,
        api: api.to_string(),
    }
}

/// Creates the local project (the same as the plain wizard) and, after a successful
/// [`attempt_create`], links it to the new repository: the `origin` remote, the repository
/// registration (`remote`, `githubRepo`, `githubApi`) and the cycle's GitHub target.
pub fn finish_in<S>(
    settings: &mut Settings,
    store: &Store,
    sync: &S,
    data_dir: &Path,
    mut req: WizardCreateProjectDto,
    attempt: Attempt,
) -> IpcResult<WizardCreateResultDto>
where
    S: ParticipantSync + ?Sized,
{
    if req.mode.trim() != "auto" {
        return Err(IpcError::new(
            "invalid",
            "a GitHub repository can only be created together with a new project folder",
        ));
    }
    validate_wizard_basics(&req.id, &req.name)?;
    // A link to a repository that was not created would only make publishing fail later.
    req.github_repo = match &attempt {
        Attempt::Created { repo, .. } => Some(repo.full_name()),
        Attempt::Fallback { .. } => None,
    };
    let id = req.id.trim().to_string();
    let cycle = wizard_create_project_in(settings, store, sync, data_dir, req)?;

    let github = match attempt {
        Attempt::Fallback { reason, message } => GithubOutcomeDto {
            status: GithubOutcomeStatus::LocalOnly,
            reason: Some(reason),
            message,
            repo: None,
            url: None,
        },
        Attempt::Created { repo, api } => {
            let mut outcome = GithubOutcomeDto {
                status: GithubOutcomeStatus::Created,
                reason: None,
                message: format!(
                    "Created the private GitHub repository {} and linked it to the project.",
                    repo.full_name()
                ),
                repo: Some(repo.full_name()),
                url: Some(repo.html_url.clone()).filter(|u| !u.is_empty()),
            };
            if let Err(why) = link_remote(settings, &id, &repo, &api) {
                outcome.message = format!(
                    "Created the private GitHub repository {}, but could not link it to the project folder ({why}). Add it as the remote origin to publish.",
                    repo.full_name()
                );
            }
            outcome
        }
    };
    Ok(WizardCreateResultDto { cycle, github })
}

fn link_remote(
    settings: &mut Settings,
    cycle_id: &str,
    repo: &CreatedRepo,
    api: &str,
) -> Result<(), String> {
    let located = cycles::locate(&settings.cycle_repos, cycle_id)
        .map_err(|e| e.message)?
        .ok_or_else(|| "the new project was not found".to_string())?;
    let git = Repo::open(&located.setting.path).map_err(|e| e.to_string())?;
    git.add_remote("origin", &repo.clone_url)
        .map_err(|e| e.to_string())?;
    if let Some(entry) = settings
        .cycle_repos
        .iter_mut()
        .find(|r| r.id == located.setting.id)
    {
        entry.remote = Some(repo.clone_url.clone());
        entry.github_repo = Some(repo.full_name());
        entry.github_api = (api != DEFAULT_API).then(|| api.to_string());
    }
    Ok(())
}

// ----- Tauri commands ----------------------------------------------------------------------

/// Whether a GitHub token is saved (a boolean and the host, never the token).
#[tauri::command]
pub fn github_repo_status() -> IpcResult<GithubRepoStatusDto> {
    repo_status_in(&Credentials, DEFAULT_API)
}

/// Creates the project like `wizard_create_project` and, when the person confirmed it and a
/// token is saved, a private GitHub repository that becomes the project's remote. No token, a
/// refused request or a lost connection is not an error: the local project is created and
/// `github` says what happened.
#[tauri::command]
pub async fn wizard_create_project_with_github(
    app: AppHandle,
    state: State<'_, AppState>,
    req: WizardCreateProjectDto,
    github: GithubRepoRequestDto,
) -> IpcResult<WizardCreateResultDto> {
    // Check before anything is sent: a bad id must not leave a repository behind.
    if req.mode.trim() != "auto" {
        return Err(IpcError::new(
            "invalid",
            "a GitHub repository can only be created together with a new project folder",
        ));
    }
    validate_wizard_basics(&req.id, &req.name)?;
    let attempt = tauri::async_runtime::spawn_blocking(move || {
        attempt_create(&Credentials, DEFAULT_API, HttpClient::new(), &github)
    })
    .await
    .map_err(|e| background_task_error(&e))?;
    let data_dir = state.data_dir().to_path_buf();
    let result = state.update_settings(|settings| {
        state.with_store_ipc(|store| finish_in(settings, store, store, &data_dir, req, attempt))
    })?;
    let _ = app.emit(EVENT_CYCLES, ());
    Ok(result)
}

#[cfg(test)]
mod tests;
