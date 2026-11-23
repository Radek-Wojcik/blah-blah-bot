//! Secrets and access checks of the publishing targets, free of Tauri: the GitHub token of an
//! API host, the secrets cycle files refer to as `cred:...` (a Discord webhook, a Discord bot
//! token, or a Notion integration token), and the "does this work" checks the settings screen
//! offers.
//!
//! Rules: a value is validated before it is stored; a value is never returned (the screens
//! only learn *whether* one is set, and a safe masked preview); every message that leaves this
//! module has been through [`publishers::http::redact`]. The commands in
//! `publish_commands.rs` are thin wrappers.

use std::collections::BTreeSet;

use cycle_store::{Cycle, DiscordBot, NotionTarget, Repo};
use publishers::discord_bot::{
    self, perms, DiscordBot as DiscordBotClient, CHANNEL_ANNOUNCEMENT, CHANNEL_FORUM, CHANNEL_TEXT,
};
use publishers::github::{self, Github, RepoRef};
use publishers::http::{redact, ErrorKind, HttpClient, HttpError};
use publishers::notion::{self, Notion};
use publishers::webhook::Webhook;
use serde::Serialize;

use crate::cycles::{self, NotionPropertiesDto};
use crate::ipc::{IpcError, IpcResult};
use crate::secrets::{self, SecretStore};
use crate::settings::CycleRepoSetting;

// ----- payloads ----------------------------------------------------------------------------

/// A GitHub API host and whether a token is stored for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubHostDto {
    pub host: String,
    pub api: String,
    pub token_set: bool,
}

/// A secret a cycle refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSecretDto {
    pub reference: String,
    /// The cycle that uses it.
    pub cycle_id: String,
    /// `discord`, `discord-bot` or `notion`.
    pub kind: &'static str,
    pub set: bool,
    /// A safe masked form, never the secret: host and ends of the webhook id,
    /// `bot <first four>...` for a bot token, or `ntn_...<last four>` for a Notion token.
    pub masked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishingSecretsDto {
    pub github: Vec<GithubHostDto>,
    pub targets: Vec<TargetSecretDto>,
}

/// The answer of "test GitHub access": the expected failures are a result with a message,
/// not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubAccessDto {
    pub ok: bool,
    pub message: String,
    pub repo: Option<String>,
    pub host: Option<String>,
    pub token_set: bool,
    pub login: Option<String>,
    /// `public`, `private` or `internal`.
    pub visibility: Option<String>,
    pub can_push: Option<bool>,
    pub has_issues: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCheckDto {
    pub ok: bool,
    pub message: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionDataSourceDto {
    pub id: String,
    pub name: String,
}

/// The answer of "test Notion access": expected failures are a result with a message, not an
/// error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionAccessDto {
    pub ok: bool,
    pub message: String,
    /// The name of the integration.
    pub bot_name: Option<String>,
    pub database_title: Option<String>,
    pub data_sources: Vec<NotionDataSourceDto>,
    pub token_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionPropertyDto {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One problem of the Notion field mapping; `field` is a camelCase key of `NotionProperties`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionMappingProblemDto {
    pub field: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionSchemaDto {
    pub data_source_id: String,
    pub properties: Vec<NotionPropertyDto>,
    pub proposed: NotionPropertiesDto,
    pub current: NotionPropertiesDto,
    pub problems: Vec<NotionMappingProblemDto>,
}

/// The answer of "test Discord bot": expected failures are a result with a message, not an
/// error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordBotCheckDto {
    pub ok: bool,
    pub message: String,
    pub bot_name: Option<String>,
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
    /// `text`, `announcement` or `forum`.
    pub channel_kind: Option<&'static str>,
    /// Names of the permissions the bot lacks in the channel (`CREATE_PUBLIC_THREADS`, ...).
    pub missing: Vec<&'static str>,
    pub token_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordMemberDto {
    pub user_id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub nick: Option<String>,
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordRoleDto {
    pub role_id: String,
    pub name: String,
    pub mentionable: bool,
}

// ----- discovery ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetSecretKind {
    DiscordWebhook,
    DiscordBot,
    Notion,
}

impl TargetSecretKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DiscordWebhook => "discord",
            Self::DiscordBot => "discord-bot",
            Self::Notion => "notion",
        }
    }
}

/// Every `cred:` reference used by the cycles (Discord webhooks, Discord bot tokens and Notion
/// integration tokens), with the cycle id and secret kind.
fn target_refs(repos: &[CycleRepoSetting]) -> Vec<(String, String, TargetSecretKind)> {
    let mut out = Vec::new();
    for setting in repos {
        let Ok(repo) = Repo::open(&setting.path) else {
            continue;
        };
        let Ok(entries) = repo.list_cycles() else {
            continue;
        };
        for entry in entries {
            let Some(cycle) = entry.cycle else { continue };
            if let Some(discord) = cycle.targets.discord {
                if let Some(webhook) = discord.webhook {
                    if secrets::reference_name(&webhook).is_ok() {
                        out.push((
                            webhook.trim().to_string(),
                            cycle.id.clone(),
                            TargetSecretKind::DiscordWebhook,
                        ));
                    }
                }
                if let Some(bot) = discord.bot {
                    if secrets::reference_name(&bot.token).is_ok() {
                        out.push((
                            bot.token.trim().to_string(),
                            cycle.id.clone(),
                            TargetSecretKind::DiscordBot,
                        ));
                    }
                }
            }
            if let Some(notion) = cycle.targets.notion {
                if secrets::reference_name(&notion.token).is_ok() {
                    out.push((
                        notion.token.trim().to_string(),
                        cycle.id,
                        TargetSecretKind::Notion,
                    ));
                }
            }
        }
    }
    out
}

/// The API base and the repository a registered repository publishes to: `githubRepo` and
/// `githubApi` of the setting, else the remote when it is on github.com.
pub fn github_identity(setting: &CycleRepoSetting) -> Result<(RepoRef, String), String> {
    let api = match setting.github_api.as_deref().map(str::trim) {
        Some(api) if !api.is_empty() => github::normalize_api_base(api)?,
        _ => github::DEFAULT_API.to_string(),
    };
    let from_setting = setting
        .github_repo
        .as_deref()
        .filter(|r| !r.trim().is_empty());
    let repo = match from_setting {
        Some(text) => RepoRef::parse(text)
            .ok_or_else(|| "the GitHub repository must be written owner/name".to_string())?,
        None => setting
            .remote
            .as_deref()
            .and_then(RepoRef::from_github_remote)
            .ok_or_else(|| {
                "this repository is not linked to a GitHub repository: its address is not on github.com, so set the repository as owner/name".to_string()
            })?,
    };
    Ok((repo, api))
}

fn api_hosts(repos: &[CycleRepoSetting]) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut add = |api: String| {
        if let Some(host) = github::api_host(&api) {
            if seen.insert(host.clone()) {
                out.push((host, api));
            }
        }
    };
    add(github::DEFAULT_API.to_string());
    for setting in repos {
        if let Ok((_, api)) = github_identity(setting) {
            add(api);
        }
    }
    out
}

/// Safe masked form of a Discord bot token (`bot <first four>...`), never the secret.
fn mask_bot_token(token: &str) -> String {
    let first = token.trim().split('.').next().unwrap_or("");
    let prefix: String = first.chars().take(4).collect();
    format!("bot {prefix}...")
}

/// Safe masked form of a Notion integration token (`ntn_...<last four>` or
/// `secret_...<last four>`), never the secret.
fn mask_notion_token(token: &str) -> String {
    let t = token.trim();
    let prefix = if t.starts_with("secret_") {
        "secret_"
    } else {
        "ntn_"
    };
    let tail: String = if t.chars().count() >= 12 {
        t.chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        String::new()
    };
    format!("{prefix}...{tail}")
}

/// Which tokens and target secrets exist, never their values.
pub fn publishing_secrets(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
) -> PublishingSecretsDto {
    let github = api_hosts(repos)
        .into_iter()
        .map(|(host, api)| GithubHostDto {
            token_set: store
                .get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host))
                .is_some(),
            host,
            api,
        })
        .collect();
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for (reference, cycle_id, kind) in target_refs(repos) {
        if !seen.insert(reference.clone()) {
            continue;
        }
        let stored = secrets::resolve_cred(store, &reference);
        let masked = stored.as_deref().and_then(|v| match kind {
            TargetSecretKind::DiscordWebhook => Webhook::parse(v).ok().map(|w| w.masked()),
            TargetSecretKind::DiscordBot => Some(mask_bot_token(v)),
            TargetSecretKind::Notion => Some(mask_notion_token(v)),
        });
        targets.push(TargetSecretDto {
            reference,
            cycle_id,
            kind: kind.as_str(),
            set: stored.is_some(),
            masked,
        });
    }
    PublishingSecretsDto { github, targets }
}

// ----- the GitHub token --------------------------------------------------------------------

fn api_or_default(api: Option<&str>) -> &str {
    match api.map(str::trim) {
        Some(a) if !a.is_empty() => a,
        _ => github::DEFAULT_API,
    }
}

/// Stores the token for the host of `api` (the public GitHub when absent). A token that
/// spans lines or has spaces in it was pasted wrongly and is refused.
pub fn set_github_token(store: &dyn SecretStore, api: Option<&str>, token: &str) -> IpcResult<()> {
    let token = token.trim();
    if token.is_empty() {
        return Err(IpcError::new("invalid", "the token is empty"));
    }
    if token.len() > 1024 || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(IpcError::new(
            "invalid",
            "this does not look like a token: it has spaces or line breaks in it",
        ));
    }
    store.set(&secrets::github_token_entry(api_or_default(api))?, token)
}

pub fn clear_github_token(store: &dyn SecretStore, api: Option<&str>) -> IpcResult<()> {
    store.clear(&secrets::github_token_entry(api_or_default(api))?)
}

/// Checks the stored token against the repository of the cycle: the token is valid, the
/// repository is visible, the user can push, issues are on; and reports its visibility (the
/// publish step warns about a public one).
pub fn test_github_access(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    client: HttpClient,
) -> IpcResult<GithubAccessDto> {
    let located = cycles::locate(repos, cycle_id)?.ok_or_else(|| {
        IpcError::new(
            "not_found",
            format!("the cycle {cycle_id} is not in any registered repository"),
        )
    })?;
    let mut out = GithubAccessDto {
        ok: false,
        message: String::new(),
        repo: None,
        host: None,
        token_set: false,
        login: None,
        visibility: None,
        can_push: None,
        has_issues: None,
    };
    let (repo, api) = match github_identity(&located.setting) {
        Ok(found) => found,
        Err(why) => {
            out.message = why;
            return Ok(out);
        }
    };
    let host = github::api_host(&api).unwrap_or_default();
    out.repo = Some(repo.full_name());
    out.host = Some(host.clone());
    let Some(token) = store.get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host)) else {
        out.message = format!("no GitHub token is stored for {host}");
        return Ok(out);
    };
    out.token_set = true;
    let api = Github::with_client(client, &api, &token)
        .map_err(|e| IpcError::new("invalid", scrub_with(&[&token], &e.message)))?;
    match api.check_access(&repo) {
        Ok(report) => {
            out.login = Some(report.login);
            out.visibility = Some(report.repo.visibility.clone());
            out.can_push = report.repo.can_push;
            out.has_issues = Some(report.repo.has_issues);
            if report.repo.can_push == Some(false) {
                out.message = format!(
                    "the token works, but its user cannot push to {repo}: ask for write access"
                );
            } else if !report.repo.has_issues {
                out.message = format!("{repo} has Issues turned off, so no issues can be created");
                out.ok = true;
            } else {
                out.message = format!(
                    "connected to {repo} as {}",
                    out.login.as_deref().unwrap_or("")
                );
                out.ok = true;
            }
        }
        Err(e) => {
            out.message = match e.status {
                Some(401) => format!("{host} rejected the token: it is wrong, expired or revoked"),
                Some(404) => format!(
                    "{repo} was not found, or the token cannot see it (a fine-grained token must include this repository)"
                ),
                _ => e.message.clone(),
            };
        }
    }
    out.message = scrub_with(&[&token], &out.message);
    Ok(out)
}

// ----- target secrets ----------------------------------------------------------------------

/// What kind of target secret `reference` stands for: checked against the cycles first, then
/// by the prefix of the reference name (`discord-bot/`, `discord/`, `notion/`).
fn target_secret_kind(repos: &[CycleRepoSetting], reference: &str) -> Option<TargetSecretKind> {
    let trimmed = reference.trim();
    if let Some((_, _, kind)) = target_refs(repos)
        .into_iter()
        .find(|(r, _, _)| r == trimmed)
    {
        return Some(kind);
    }
    let name = secrets::reference_name(trimmed).ok()?;
    if name.starts_with("discord-bot/") {
        Some(TargetSecretKind::DiscordBot)
    } else if name.starts_with("discord/") {
        Some(TargetSecretKind::DiscordWebhook)
    } else if name.starts_with("notion/") {
        Some(TargetSecretKind::Notion)
    } else {
        None
    }
}

/// Checks the shape of a Discord bot token: three non-empty dot-separated parts of ASCII
/// letters, digits, `_` and `-`, 50 to 120 characters long, no whitespace or URL scheme.
/// Error text never repeats the value.
fn validate_bot_token(token: &str) -> Result<(), &'static str> {
    if token.is_empty() {
        return Err("the bot token is empty");
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("this does not look like a bot token: it has spaces or line breaks in it");
    }
    if token.contains("://") {
        return Err("this looks like a web address, not a Discord bot token");
    }
    let parts: Vec<&str> = token.split('.').collect();
    let valid_parts = parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
    if !valid_parts || !(50..=120).contains(&token.len()) {
        return Err(
            "this does not look like a Discord bot token: it has three parts separated by dots and is 50 to 120 characters long",
        );
    }
    Ok(())
}

/// Checks the shape of a Notion integration token: starts with `ntn_` (or legacy `secret_`),
/// 20 to 200 characters, no whitespace, control characters or URL scheme. Error text never
/// repeats the value.
fn validate_notion_token(token: &str) -> Result<(), &'static str> {
    if token.is_empty() {
        return Err("the Notion token is empty");
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("this does not look like a Notion token: it has spaces or line breaks in it");
    }
    if token.contains("://") {
        return Err("this looks like a web address, not a Notion integration token");
    }
    if !(token.starts_with("ntn_") || token.starts_with("secret_"))
        || !(20..=200).contains(&token.len())
    {
        return Err(
            "this does not look like a Notion integration token: it starts with ntn_ and is 20 to 200 characters long",
        );
    }
    Ok(())
}

/// Stores the value of a `cred:` reference in the `target:` namespace. Discord webhooks,
/// Discord bot tokens and Notion tokens are validated by shape before saving; the error never
/// repeats the value.
pub fn set_target_secret(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    reference: &str,
    value: &str,
) -> IpcResult<()> {
    let entry = secrets::target_entry(reference)?;
    let value = value.trim();
    match target_secret_kind(repos, reference) {
        Some(TargetSecretKind::DiscordWebhook) => {
            let hook = Webhook::parse(value).map_err(|why| {
                IpcError::new("invalid_webhook", format!("not a Discord webhook: {why}"))
            })?;
            return store.set(&entry, &hook.url());
        }
        Some(TargetSecretKind::DiscordBot) => {
            validate_bot_token(value).map_err(|why| IpcError::new("invalid_bot_token", why))?;
            return store.set(&entry, value);
        }
        Some(TargetSecretKind::Notion) => {
            validate_notion_token(value)
                .map_err(|why| IpcError::new("invalid_notion_token", why))?;
            return store.set(&entry, value);
        }
        None => {}
    }
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(IpcError::new("invalid", "this value is not acceptable"));
    }
    store.set(&entry, value)
}

pub fn clear_target_secret(store: &dyn SecretStore, reference: &str) -> IpcResult<()> {
    store.clear(&secrets::target_entry(reference)?)
}

/// Asks Discord about the stored webhook with a `GET`: it sends no message.
pub fn test_discord_webhook(
    store: &dyn SecretStore,
    reference: &str,
    client: HttpClient,
) -> IpcResult<WebhookCheckDto> {
    let mut out = WebhookCheckDto {
        ok: false,
        message: String::new(),
        name: None,
    };
    let Some(value) = secrets::resolve_cred(store, reference) else {
        out.message = "no webhook is stored for this reference".into();
        return Ok(out);
    };
    let hook = match Webhook::parse(&value) {
        Ok(h) => h,
        Err(why) => {
            out.message = format!("the stored value is not a usable webhook: {why}");
            return Ok(out);
        }
    };
    let client = client.with_secret(&hook.url()).with_secret(hook.token());
    match hook.check(&client) {
        Ok(info) => {
            out.ok = true;
            out.message = format!("{} exists", hook.masked());
            out.name = info.name;
        }
        Err(e) => {
            out.message = match e.status {
                Some(404) => {
                    "Discord does not know this webhook: it was deleted or the address is wrong"
                        .into()
                }
                Some(401) => "Discord rejected the webhook token".into(),
                _ => e.message.clone(),
            };
        }
    }
    out.message = scrub_with(&[&value, hook.token()], &out.message);
    Ok(out)
}

// ----- Notion and Discord bot checks -------------------------------------------------------

fn load_cycle(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<Cycle> {
    let located = cycles::locate(repos, cycle_id)?.ok_or_else(|| {
        IpcError::new(
            "not_found",
            format!("the cycle {cycle_id} is not in any registered repository"),
        )
    })?;
    located
        .entry
        .cycle
        .ok_or_else(|| IpcError::new("cycle_invalid", "the cycle cannot be read"))
}

fn notion_target(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<NotionTarget> {
    load_cycle(repos, cycle_id)?.targets.notion.ok_or_else(|| {
        IpcError::new(
            "no_target",
            "this cycle has no Notion target: add one in the cycle settings first",
        )
    })
}

fn discord_bot_target(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<DiscordBot> {
    load_cycle(repos, cycle_id)?
        .targets
        .discord
        .and_then(|d| d.bot)
        .ok_or_else(|| {
            IpcError::new(
                "no_target",
                "this cycle has no Discord bot: choose Bot for the Discord target in the cycle settings first",
            )
        })
}

fn http_to_ipc(err: HttpError, secret: &str) -> IpcError {
    let code = match (err.status, err.kind) {
        (Some(401 | 403), _) => "secrets",
        (Some(404), _) => "not_found",
        (_, ErrorKind::InvalidRequest) => "invalid",
        _ => "io",
    };
    IpcError::new(code, scrub_with(&[secret], &err.message))
}

fn notion_problem_field(field: &'static str) -> &'static str {
    match field {
        "status_value" => "statusValue",
        other => other,
    }
}

/// Checks the stored Notion token against the integration (`GET /users/me`) and the database
/// of the cycle's Notion target (`GET /databases/{id}`). Nothing is written.
pub fn test_notion_access(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    client: HttpClient,
) -> IpcResult<NotionAccessDto> {
    test_notion_access_at(store, repos, cycle_id, notion::DEFAULT_API, client)
}

pub fn test_notion_access_at(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    api: &str,
    client: HttpClient,
) -> IpcResult<NotionAccessDto> {
    let target = notion_target(repos, cycle_id)?;
    let mut out = NotionAccessDto {
        ok: false,
        message: String::new(),
        bot_name: None,
        database_title: None,
        data_sources: Vec::new(),
        token_set: false,
    };
    let Some(token) = secrets::resolve_cred(store, &target.token) else {
        out.message = format!(
            "no Notion token is stored for {}: store it in the settings",
            target.token
        );
        return Ok(out);
    };
    out.token_set = true;
    let api = Notion::with_client(client, api, &token)
        .map_err(|e| IpcError::new("invalid", scrub_with(&[&token], &e.message)))?;
    let me = match api.current_user() {
        Ok(me) => me,
        Err(e) => {
            out.message = scrub_with(&[&token], &e.message);
            return Ok(out);
        }
    };
    out.bot_name = me.name.clone();
    match api.database(&target.database) {
        Ok(db) => {
            out.ok = true;
            out.database_title = Some(db.title.clone());
            out.data_sources = db
                .data_sources
                .into_iter()
                .map(|s| NotionDataSourceDto {
                    id: s.id,
                    name: s.name,
                })
                .collect();
            out.message = match out.bot_name.as_deref() {
                Some(name) if !name.is_empty() => {
                    format!(
                        "connected to Notion as \"{name}\": database \"{}\"",
                        db.title
                    )
                }
                _ => format!("connected to Notion: database \"{}\"", db.title),
            };
        }
        Err(e) => {
            out.message = e.message;
        }
    }
    out.message = scrub_with(&[&token], &out.message);
    Ok(out)
}

/// Inspects the properties of the Notion data source of the cycle, proposes a mapping and
/// reports any problems with the current mapping.
pub fn notion_schema(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    client: HttpClient,
) -> IpcResult<NotionSchemaDto> {
    notion_schema_at(store, repos, cycle_id, notion::DEFAULT_API, client)
}

pub fn notion_schema_at(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    api: &str,
    client: HttpClient,
) -> IpcResult<NotionSchemaDto> {
    let target = notion_target(repos, cycle_id)?;
    let Some(token) = secrets::resolve_cred(store, &target.token) else {
        return Err(IpcError::new(
            "secrets",
            format!(
                "no Notion token is stored for {}: store it in the settings",
                target.token
            ),
        ));
    };
    let api = Notion::with_client(client, api, &token).map_err(|e| http_to_ipc(e, &token))?;
    let data_source_id = api
        .resolve_data_source(&target.database, target.data_source.as_deref())
        .map_err(|e| http_to_ipc(e, &token))?;
    let schema = api
        .data_source_schema(&data_source_id)
        .map_err(|e| http_to_ipc(e, &token))?;
    let properties = schema
        .properties
        .iter()
        .map(|(name, kind)| NotionPropertyDto {
            name: name.clone(),
            kind: kind.api_name().to_string(),
        })
        .collect();
    let proposed = notion::propose_with_options(&schema.properties, &schema.options).into();
    let problems = notion::check(&target.properties, &schema.properties)
        .into_iter()
        .map(|p| NotionMappingProblemDto {
            field: notion_problem_field(p.field),
            message: p.message,
        })
        .collect();
    Ok(NotionSchemaDto {
        data_source_id,
        properties,
        proposed,
        current: target.properties.into(),
        problems,
    })
}

const PERMISSION_CODES: [(u64, &str); 6] = [
    (perms::VIEW_CHANNEL, "VIEW_CHANNEL"),
    (perms::SEND_MESSAGES, "SEND_MESSAGES"),
    (perms::ATTACH_FILES, "ATTACH_FILES"),
    (perms::READ_MESSAGE_HISTORY, "READ_MESSAGE_HISTORY"),
    (perms::CREATE_PUBLIC_THREADS, "CREATE_PUBLIC_THREADS"),
    (perms::SEND_MESSAGES_IN_THREADS, "SEND_MESSAGES_IN_THREADS"),
];

fn missing_permission_codes(have: u64, need: u64) -> Vec<&'static str> {
    if have & perms::ADMINISTRATOR != 0 {
        return Vec::new();
    }
    PERMISSION_CODES
        .iter()
        .filter(|(bit, _)| need & bit != 0 && have & bit == 0)
        .map(|(_, name)| *name)
        .collect()
}

/// Checks the Discord bot token against the guild and channel of the cycle's Discord bot
/// target and lists the permissions it lacks in the channel. Nothing is posted.
pub fn test_discord_bot(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    client: HttpClient,
) -> IpcResult<DiscordBotCheckDto> {
    test_discord_bot_at(store, repos, cycle_id, discord_bot::DEFAULT_API, client)
}

pub fn test_discord_bot_at(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    api: &str,
    client: HttpClient,
) -> IpcResult<DiscordBotCheckDto> {
    let target = discord_bot_target(repos, cycle_id)?;
    let mut out = DiscordBotCheckDto {
        ok: false,
        message: String::new(),
        bot_name: None,
        guild_name: None,
        channel_name: None,
        channel_kind: None,
        missing: Vec::new(),
        token_set: false,
    };
    let Some(token) = secrets::resolve_cred(store, &target.token) else {
        out.message = format!("no Discord bot token is stored for {}", target.token);
        return Ok(out);
    };
    out.token_set = true;
    let api = DiscordBotClient::with_client(client, api, &token)
        .map_err(|e| IpcError::new("invalid", scrub_with(&[&token], &e.message)))?;
    let me = match api.current_user() {
        Ok(me) => me,
        Err(e) => {
            out.message = scrub_with(&[&token], &e.message);
            return Ok(out);
        }
    };
    out.bot_name = Some(me.username.clone());
    let guild = match api.guild(&target.guild) {
        Ok(g) => g,
        Err(e) => {
            out.message = if matches!(e.status, Some(403 | 404)) {
                format!(
                    "{}; add the bot with {}",
                    e.message,
                    perms::invite_url(&me.id)
                )
            } else {
                e.message
            };
            out.message = scrub_with(&[&token], &out.message);
            return Ok(out);
        }
    };
    out.guild_name = Some(guild.name.clone());
    let channel = match api.channel(&target.channel) {
        Ok(c) => c,
        Err(e) => {
            out.message = scrub_with(&[&token], &e.message);
            return Ok(out);
        }
    };
    if channel.guild_id.as_deref().is_some_and(|g| g != guild.id) {
        out.message = "the channel is not in this server".into();
        return Ok(out);
    }
    out.channel_name = Some(channel.name.clone());
    out.channel_kind = match channel.kind {
        CHANNEL_TEXT => Some("text"),
        CHANNEL_ANNOUNCEMENT => Some("announcement"),
        CHANNEL_FORUM => Some("forum"),
        _ => None,
    };
    if !channel.supports_summary() {
        out.message = format!(
            "#{} is a {} channel: choose a text or announcement channel",
            channel.name,
            channel.kind_name()
        );
        return Ok(out);
    }
    let roles = match api.roles(&target.guild) {
        Ok(r) => r,
        Err(e) => {
            out.message = scrub_with(&[&token], &e.message);
            return Ok(out);
        }
    };
    let member = match api.member(&target.guild, &me.id) {
        Ok(m) => m,
        Err(e) => {
            out.message = scrub_with(&[&token], &e.message);
            return Ok(out);
        }
    };
    let effective = perms::effective_channel_permissions(
        guild.owner_id == me.id,
        &guild.id,
        &roles,
        &member.roles,
        &me.id,
        &channel.overwrites,
    );
    let missing = missing_permission_codes(effective, perms::required(target.thread));
    if !missing.is_empty() {
        out.missing = missing;
        out.message = if effective & perms::VIEW_CHANNEL == 0 {
            format!("the bot cannot see #{}", channel.name)
        } else {
            format!(
                "the bot is missing {} in #{}: {}",
                if out.missing.len() == 1 {
                    "a permission"
                } else {
                    "permissions"
                },
                channel.name,
                out.missing.join(", ")
            )
        };
        out.message = scrub_with(&[&token], &out.message);
        return Ok(out);
    }
    out.ok = true;
    out.message = scrub_with(
        &[&token],
        &format!(
            "connected as \"{}\" to #{} in {}",
            me.username, channel.name, guild.name
        ),
    );
    Ok(out)
}

/// Searches guild members whose username or nickname starts with `query` (at least 2
/// characters, at most 10 results) over REST without privileged intents.
pub fn search_discord_members(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    query: &str,
    client: HttpClient,
) -> IpcResult<Vec<DiscordMemberDto>> {
    search_discord_members_at(
        store,
        repos,
        cycle_id,
        query,
        discord_bot::DEFAULT_API,
        client,
    )
}

pub fn search_discord_members_at(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    query: &str,
    api: &str,
    client: HttpClient,
) -> IpcResult<Vec<DiscordMemberDto>> {
    let target = discord_bot_target(repos, cycle_id)?;
    let Some(token) = secrets::resolve_cred(store, &target.token) else {
        return Err(IpcError::new(
            "secrets",
            format!(
                "no Discord bot token is stored for {}: store it in the settings",
                target.token
            ),
        ));
    };
    let q = query.trim();
    if q.chars().count() < 2 {
        return Err(IpcError::new(
            "invalid",
            "type at least 2 characters to search",
        ));
    }
    let api =
        DiscordBotClient::with_client(client, api, &token).map_err(|e| http_to_ipc(e, &token))?;
    let members = api
        .search_members(&target.guild, q, 10)
        .map_err(|e| http_to_ipc(e, &token))?;
    Ok(members
        .into_iter()
        .take(10)
        .map(|m| DiscordMemberDto {
            user_id: m.user_id,
            username: m.username,
            global_name: m.global_name,
            nick: m.nick,
            bot: m.bot,
        })
        .collect())
}

/// Lists the roles of the guild of the cycle's Discord bot target (excluding `@everyone`).
pub fn list_discord_roles(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    client: HttpClient,
) -> IpcResult<Vec<DiscordRoleDto>> {
    list_discord_roles_at(store, repos, cycle_id, discord_bot::DEFAULT_API, client)
}

pub fn list_discord_roles_at(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    api: &str,
    client: HttpClient,
) -> IpcResult<Vec<DiscordRoleDto>> {
    let target = discord_bot_target(repos, cycle_id)?;
    let Some(token) = secrets::resolve_cred(store, &target.token) else {
        return Err(IpcError::new(
            "secrets",
            format!(
                "no Discord bot token is stored for {}: store it in the settings",
                target.token
            ),
        ));
    };
    let api =
        DiscordBotClient::with_client(client, api, &token).map_err(|e| http_to_ipc(e, &token))?;
    let roles = api
        .roles(&target.guild)
        .map_err(|e| http_to_ipc(e, &token))?;
    Ok(roles
        .into_iter()
        .filter(|r| r.id != target.guild && r.name != "@everyone")
        .map(|r| DiscordRoleDto {
            role_id: r.id,
            name: r.name,
            mentionable: r.mentionable,
        })
        .collect())
}

// ----- redaction ---------------------------------------------------------------------------

/// `text` with the given values and everything shaped like a secret masked.
pub fn scrub_with(values: &[&str], text: &str) -> String {
    redact(text, values)
}

/// `text` with every secret this application holds for publishing masked: the GitHub tokens
/// and the values behind `cred:` references of the cycles (Discord webhooks, Discord bot
/// tokens and Notion tokens). For anything that is stored or shown: outbox errors, log lines,
/// git output.
pub fn scrub(store: &dyn SecretStore, repos: &[CycleRepoSetting], text: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    for (host, _) in api_hosts(repos) {
        if let Some(token) = store.get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host)) {
            values.push(token);
        }
    }
    for (reference, _, kind) in target_refs(repos) {
        if let Some(value) = secrets::resolve_cred(store, &reference) {
            if kind == TargetSecretKind::DiscordWebhook {
                if let Ok(hook) = Webhook::parse(&value) {
                    values.push(hook.token().to_string());
                }
            }
            values.push(value);
        }
    }
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    redact(text, &refs)
}

// ----- the repository link -----------------------------------------------------------------

/// Sets (or clears, with empty values) the GitHub repository and API of a registered
/// repository. Both are validated; nothing secret is involved.
pub fn set_github_repo(
    repos: &mut [CycleRepoSetting],
    repo_id: &str,
    github_repo: Option<&str>,
    github_api: Option<&str>,
) -> IpcResult<CycleRepoSetting> {
    fn non_empty(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|s| !s.is_empty())
    }
    let repo = match non_empty(github_repo) {
        Some(text) => Some(
            RepoRef::parse(text)
                .ok_or_else(|| IpcError::new("invalid", "write the repository as owner/name"))?
                .full_name(),
        ),
        None => None,
    };
    let api = match non_empty(github_api) {
        Some(text) => Some(
            github::normalize_api_base(text)
                .map_err(|why| IpcError::new("invalid", format!("GitHub API address: {why}")))?,
        ),
        None => None,
    };
    let setting = repos
        .iter_mut()
        .find(|r| r.id == repo_id)
        .ok_or_else(|| IpcError::new("not_found", "this cycle repository is not registered"))?;
    setting.github_repo = repo;
    setting.github_api = api.filter(|a| a != github::DEFAULT_API);
    Ok(setting.clone())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use publishers::testing::{FakeDiscord, FakeGithub, Visibility};

    use super::*;
    use crate::cycles::tests::Lab;
    use crate::secrets::MemorySecrets;
    use crate::settings::RepoKind;

    const REPO: &str = "owner/rover";
    const TOKEN: &str = "ghp_MockTokenValue0123456789";

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

    /// A lab whose repository is linked to the fake GitHub.
    fn linked(gh: &FakeGithub) -> Lab {
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        let id = lab.settings.cycle_repos[0].id.clone();
        set_github_repo(
            &mut lab.settings.cycle_repos,
            &id,
            Some(REPO),
            Some(&gh.api_base()),
        )
        .unwrap();
        lab
    }

    fn fake_github() -> FakeGithub {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token(TOKEN);
        gh
    }

    fn with_discord_target(lab: &mut Lab, reference: &str) {
        let repo = lab.repo();
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.discord = Some(cycle_store::DiscordTarget {
            webhook: Some(reference.into()),
            bot: None,
            publish: cycle_store::PublishMode::OnApproval,
            template: None,
        });
        repo.save_cycle(&cycle).unwrap();
    }

    #[test]
    fn the_token_is_stored_per_host_and_never_reported() {
        let gh = fake_github();
        let lab = linked(&gh);
        let store = MemorySecrets::default();
        let before = publishing_secrets(&store, lab.repos());
        assert_eq!(before.github.len(), 2);
        assert!(before.github.iter().all(|h| !h.token_set));
        assert_eq!(before.github[0].host, "api.github.com");

        set_github_token(&store, Some(&gh.api_base()), &format!("  {TOKEN}\n")).unwrap();
        assert_eq!(store.names(), vec!["github-token@127.0.0.1".to_string()]);
        assert_eq!(
            store.get("github-token@127.0.0.1").as_deref(),
            Some(TOKEN),
            "the stored value is trimmed"
        );
        let after = publishing_secrets(&store, lab.repos());
        let local = after.github.iter().find(|h| h.host == "127.0.0.1").unwrap();
        assert!(local.token_set);
        // The default host has no token: one host's token is not another's.
        assert!(
            !after
                .github
                .iter()
                .find(|h| h.host == "api.github.com")
                .unwrap()
                .token_set
        );
        assert!(!serde_json::to_string(&after).unwrap().contains(TOKEN));

        clear_github_token(&store, Some(&gh.api_base())).unwrap();
        assert!(store.names().is_empty());
        clear_github_token(&store, Some(&gh.api_base())).unwrap();
    }

    #[test]
    fn bad_tokens_and_addresses_are_refused() {
        let store = MemorySecrets::default();
        for bad in ["", "   ", "two words", "line\nbreak", "tab\there"] {
            assert_eq!(
                set_github_token(&store, None, bad).unwrap_err().code,
                "invalid",
                "{bad:?}"
            );
        }
        for api in ["http://api.github.com", "ftp://x", "not a url"] {
            assert!(set_github_token(&store, Some(api), "tok").is_err(), "{api}");
        }
        assert!(store.names().is_empty());
        set_github_token(&store, None, "tok").unwrap();
        assert_eq!(
            store.names(),
            vec!["github-token@api.github.com".to_string()]
        );
    }

    #[test]
    fn access_check_reports_a_working_token_with_visibility_and_rights() {
        let gh = fake_github();
        let lab = linked(&gh);
        let store = MemorySecrets::default();
        set_github_token(&store, Some(&gh.api_base()), TOKEN).unwrap();
        let result = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(result.ok, "{result:?}");
        assert_eq!(result.repo.as_deref(), Some(REPO));
        assert_eq!(result.login.as_deref(), Some("fake-bot"));
        assert_eq!(result.visibility.as_deref(), Some("private"));
        assert_eq!(
            (result.can_push, result.has_issues),
            (Some(true), Some(true))
        );
        assert!(result.token_set && !result.message.contains(TOKEN));

        gh.set_visibility(REPO, Visibility::Public);
        let result = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert_eq!(result.visibility.as_deref(), Some("public"));

        gh.set_push_permission(REPO, false);
        let result = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(
            !result.ok && result.message.contains("cannot push"),
            "{result:?}"
        );
    }

    #[test]
    fn access_check_explains_each_failure_without_the_token() {
        let gh = fake_github();
        let lab = linked(&gh);
        let store = MemorySecrets::default();

        // No token yet.
        let r = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(
            !r.ok && !r.token_set && r.message.contains("no GitHub token"),
            "{r:?}"
        );

        // A wrong token.
        let wrong = "ghp_WrongTokenValue9876543210";
        set_github_token(&store, Some(&gh.api_base()), wrong).unwrap();
        let r = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(!r.ok && r.message.contains("rejected the token"), "{r:?}");
        assert!(!r.message.contains(wrong));

        // A repository the token cannot see.
        set_github_token(&store, Some(&gh.api_base()), TOKEN).unwrap();
        let mut lab = lab;
        let id = lab.settings.cycle_repos[0].id.clone();
        set_github_repo(
            &mut lab.settings.cycle_repos,
            &id,
            Some("owner/other"),
            Some(&gh.api_base()),
        )
        .unwrap();
        let r = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(!r.ok && r.message.contains("was not found"), "{r:?}");

        // Offline.
        set_github_repo(
            &mut lab.settings.cycle_repos,
            &id,
            Some(REPO),
            Some(&gh.api_base()),
        )
        .unwrap();
        gh.go_offline();
        let r = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(
            !r.ok && !r.message.is_empty() && !r.message.contains(TOKEN),
            "{r:?}"
        );

        // Unknown cycle.
        assert_eq!(
            test_github_access(&store, lab.repos(), "GHOST", client())
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn a_repository_without_a_github_link_says_so() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let store = MemorySecrets::default();
        let r = test_github_access(&store, lab.repos(), "NOMAD", client()).unwrap();
        assert!(
            !r.ok && r.repo.is_none() && r.message.contains("not linked"),
            "{r:?}"
        );
    }

    #[test]
    fn the_github_repository_is_read_from_a_github_remote() {
        let mut setting = CycleRepoSetting {
            id: "r".into(),
            path: "x".into(),
            remote: Some("https://github.com/owner/rover.git".into()),
            kind: RepoKind::Clone,
            github_repo: None,
            github_api: None,
        };
        let (repo, api) = github_identity(&setting).unwrap();
        assert_eq!(
            (repo.full_name().as_str(), api.as_str()),
            (REPO, github::DEFAULT_API)
        );
        // An explicit setting wins; Enterprise needs one.
        setting.remote = Some("https://ghe.example.com/team/rover.git".into());
        assert!(github_identity(&setting).is_err());
        setting.github_repo = Some("team/rover".into());
        setting.github_api = Some("https://ghe.example.com/api/v3/".into());
        let (repo, api) = github_identity(&setting).unwrap();
        assert_eq!(repo.full_name(), "team/rover");
        assert_eq!(api, "https://ghe.example.com/api/v3");
        setting.github_repo = Some("nonsense".into());
        assert!(github_identity(&setting).is_err());
    }

    #[test]
    fn linking_a_repository_validates_and_can_be_cleared() {
        let mut lab = Lab::new();
        let id = lab.settings.cycle_repos[0].id.clone();
        let repos = &mut lab.settings.cycle_repos;
        let set = set_github_repo(
            repos,
            &id,
            Some(" owner/rover "),
            Some("https://api.github.com/"),
        )
        .unwrap();
        assert_eq!(set.github_repo.as_deref(), Some(REPO));
        assert_eq!(
            set.github_api, None,
            "the public API is the default and is not stored"
        );
        for (repo, api) in [
            (Some("rover"), None),
            (Some("owner/rover"), Some("http://api.github.com")),
        ] {
            assert_eq!(
                set_github_repo(repos, &id, repo, api).unwrap_err().code,
                "invalid"
            );
        }
        assert_eq!(
            set_github_repo(repos, "ghost", None, None)
                .unwrap_err()
                .code,
            "not_found"
        );
        let cleared = set_github_repo(repos, &id, Some(""), None).unwrap();
        assert_eq!((cleared.github_repo, cleared.github_api), (None, None));
    }

    #[test]
    fn a_webhook_is_validated_on_save_and_stored_in_its_own_namespace() {
        let discord = FakeDiscord::start();
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        with_discord_target(&mut lab, "cred:discord/nomad");
        let store = MemorySecrets::default();
        let url = discord.webhook_url();

        for bad in [
            "https://evil.example/api/webhooks/123456789012345678/abcdefgh1234",
            "http://discord.com/api/webhooks/123456789012345678/abcdefgh1234",
            "just text",
            "",
        ] {
            let err =
                set_target_secret(&store, lab.repos(), "cred:discord/nomad", bad).unwrap_err();
            assert!(
                err.code == "invalid_webhook" || err.code == "invalid",
                "{bad}: {err}"
            );
            assert!(!err.message.contains("abcdefgh1234"));
        }
        assert!(store.names().is_empty());
        // A reference that is not even shaped like one.
        assert!(set_target_secret(&store, lab.repos(), "discord/nomad", &url).is_err());

        set_target_secret(
            &store,
            lab.repos(),
            "cred:discord/nomad",
            &format!(" {url} "),
        )
        .unwrap();
        assert_eq!(store.names(), vec!["target:discord/nomad".to_string()]);
        assert_eq!(
            secrets::resolve_cred(&store, "cred:discord/nomad").as_deref(),
            Some(url.as_str())
        );

        // Reported as set with a masked id, and never as the address.
        let report = publishing_secrets(&store, lab.repos());
        assert_eq!(report.targets.len(), 1);
        let target = &report.targets[0];
        assert!(target.set && target.cycle_id == "NOMAD" && target.kind == "discord");
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains(&discord.webhook_token()) && !json.contains(&url));
        assert_eq!(
            target.masked.as_deref(),
            Some("127.0.0.1 webhook 1000...0001")
        );

        clear_target_secret(&store, "cred:discord/nomad").unwrap();
        assert!(!publishing_secrets(&store, lab.repos()).targets[0].set);
    }

    #[test]
    fn a_reference_cannot_overwrite_an_application_secret() {
        let lab = Lab::new();
        let store = MemorySecrets::default();
        store.set(secrets::HF_TOKEN, "hf-secret").unwrap();
        // A hostile cycle file names an application secret as its webhook.
        set_target_secret(&store, lab.repos(), "cred:huggingface-token", "something").unwrap();
        assert_eq!(store.get(secrets::HF_TOKEN).as_deref(), Some("hf-secret"));
        assert_eq!(
            store.get("target:huggingface-token").as_deref(),
            Some("something")
        );
        clear_target_secret(&store, "cred:huggingface-token").unwrap();
        assert_eq!(store.get(secrets::HF_TOKEN).as_deref(), Some("hf-secret"));
    }

    #[test]
    fn testing_a_webhook_sends_no_message_and_hides_the_address() {
        let discord = FakeDiscord::start();
        let store = MemorySecrets::default();
        let lab = Lab::new();
        let none = test_discord_webhook(&store, "cred:discord/nomad", client()).unwrap();
        assert!(!none.ok && none.message.contains("no webhook"));

        set_target_secret(
            &store,
            lab.repos(),
            "cred:discord/nomad",
            &discord.webhook_url(),
        )
        .unwrap();
        let ok = test_discord_webhook(&store, "cred:discord/nomad", client()).unwrap();
        assert!(ok.ok, "{ok:?}");
        assert_eq!(ok.name.as_deref(), Some("Fake webhook"));
        assert!(discord.messages().is_empty());
        assert!(discord.requests().iter().all(|r| r.method == "GET"));
        assert!(!ok.message.contains(&discord.webhook_token()));

        discord.delete_webhook();
        let gone = test_discord_webhook(&store, "cred:discord/nomad", client()).unwrap();
        assert!(!gone.ok && gone.message.contains("deleted"), "{gone:?}");

        discord.go_offline();
        let off = test_discord_webhook(&store, "cred:discord/nomad", client()).unwrap();
        assert!(
            !off.ok && !off.message.contains(&discord.webhook_token()),
            "{off:?}"
        );
    }

    #[test]
    fn scrub_masks_every_stored_secret_in_any_text() {
        let discord = FakeDiscord::start();
        let gh = fake_github();
        let mut lab = linked(&gh);
        with_discord_target(&mut lab, "cred:discord/nomad");
        let store = MemorySecrets::default();
        set_github_token(&store, Some(&gh.api_base()), TOKEN).unwrap();
        let url = discord.webhook_url();
        set_target_secret(&store, lab.repos(), "cred:discord/nomad", &url).unwrap();
        let hook_token = discord.webhook_token();

        let text = format!(
            "push failed: fatal: unable to access 'https://x-access-token:{TOKEN}@github.com/o/r.git/' while posting {url} token {hook_token} plain {TOKEN}"
        );
        let clean = scrub(&store, lab.repos(), &text);
        for secret in [TOKEN, url.as_str(), hook_token.as_str()] {
            assert!(!clean.contains(secret), "{clean}");
        }
        assert!(
            clean.contains("push failed"),
            "ordinary words stay: {clean}"
        );
    }

    fn with_notion_target(lab: &mut Lab, reference: &str, database: &str) {
        let repo = lab.repo();
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.notion = Some(cycle_store::NotionTarget {
            token: reference.into(),
            database: database.into(),
            data_source: None,
            publish: cycle_store::PublishMode::OnApproval,
            properties: cycle_store::NotionProperties::default(),
        });
        repo.save_cycle(&cycle).unwrap();
    }

    fn with_discord_bot_target(
        lab: &mut Lab,
        reference: &str,
        guild: &str,
        channel: &str,
        thread: bool,
    ) {
        let repo = lab.repo();
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.discord = Some(cycle_store::DiscordTarget {
            webhook: None,
            bot: Some(cycle_store::DiscordBot {
                token: reference.into(),
                guild: guild.into(),
                channel: channel.into(),
                thread,
                edit_on_correction: true,
            }),
            publish: cycle_store::PublishMode::OnApproval,
            template: None,
        });
        repo.save_cycle(&cycle).unwrap();
    }

    #[test]
    fn bot_and_notion_tokens_are_discovered_validated_masked_and_scrubbed() {
        use publishers::testing::notion::DATABASE_ID;
        use publishers::testing::{FakeDiscordBot, FakeNotion};

        let bot = FakeDiscordBot::start();
        let notion = FakeNotion::start();
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        with_discord_bot_target(
            &mut lab,
            "cred:discord-bot/nomad",
            &bot.guild_id(),
            &bot.channel_id(),
            true,
        );
        with_notion_target(&mut lab, "cred:notion/nomad", DATABASE_ID);
        let store = MemorySecrets::default();

        // Both references are discovered before any value is stored.
        let before = publishing_secrets(&store, lab.repos());
        let kinds: Vec<_> = before
            .targets
            .iter()
            .map(|t| (t.reference.as_str(), t.kind, t.set, t.masked.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("cred:discord-bot/nomad", "discord-bot", false, None),
                ("cred:notion/nomad", "notion", false, None),
            ]
        );

        // Bad bot tokens are refused without echoing the value.
        let bad_bot_secret = "https://evil.example/secret-bot-payload-0123456789abcdef";
        for bad in [
            "",
            "   ",
            "short.token.val",
            "has spaces.in.token-0123456789abcdef0123456789abcdef",
            "line\nbreak.in.token-0123456789abcdef0123456789abcdef",
            bad_bot_secret,
        ] {
            let err =
                set_target_secret(&store, lab.repos(), "cred:discord-bot/nomad", bad).unwrap_err();
            assert_eq!(err.code, "invalid_bot_token", "{bad:?}: {err}");
            assert!(!err.message.contains("secret-bot-payload"));
        }

        // Bad Notion tokens are refused without echoing the value.
        let bad_notion_secret = concat!("https://evil.example/ntn_sec", "ret_value_0123456789abcdef");
        for bad in [
            "",
            "   ",
            "ntn_short",
            "wrong_prefix_0123456789abcdef0123456789",
            "ntn_with space 0123456789abcdef0123456789",
            bad_notion_secret,
        ] {
            let err = set_target_secret(&store, lab.repos(), "cred:notion/nomad", bad).unwrap_err();
            assert_eq!(err.code, "invalid_notion_token", "{bad:?}: {err}");
            assert!(!err.message.contains("ntn_secret_value"));
        }
        assert!(store.names().is_empty());

        let bot_token = bot.bot_token();
        let notion_token = notion.token();
        set_target_secret(
            &store,
            lab.repos(),
            "cred:discord-bot/nomad",
            &format!("  {bot_token}\n"),
        )
        .unwrap();
        set_target_secret(
            &store,
            lab.repos(),
            "cred:notion/nomad",
            &format!("  {notion_token}\n"),
        )
        .unwrap();
        assert_eq!(
            store.names(),
            vec![
                "target:discord-bot/nomad".to_string(),
                "target:notion/nomad".to_string()
            ]
        );

        let after = publishing_secrets(&store, lab.repos());
        let bot_dto = after
            .targets
            .iter()
            .find(|t| t.kind == "discord-bot")
            .unwrap();
        let notion_dto = after.targets.iter().find(|t| t.kind == "notion").unwrap();
        assert!(bot_dto.set && notion_dto.set);
        assert_eq!(bot_dto.masked.as_deref(), Some("bot MTAw..."));
        assert_eq!(notion_dto.masked.as_deref(), Some("ntn_...cdef"));
        let json = serde_json::to_string(&after).unwrap();
        assert!(!json.contains(&bot_token) && !json.contains(&notion_token));

        // scrub() redacts both stored tokens from any log or error text.
        let dirty = format!("bot={bot_token} notion={notion_token} keep=plain");
        let clean = scrub(&store, lab.repos(), &dirty);
        assert!(!clean.contains(&bot_token), "{clean}");
        assert!(!clean.contains(&notion_token), "{clean}");
        assert!(clean.contains("keep=plain"), "{clean}");
    }

    #[test]
    fn notion_access_schema_and_mapping_work_against_fake_notion() {
        use publishers::testing::notion::{DATABASE_ID, DATA_SOURCE_ID};
        use publishers::testing::FakeNotion;

        let notion = FakeNotion::start();
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        let store = MemorySecrets::default();

        // Without a Notion target both commands reject with no_target.
        assert_eq!(
            test_notion_access_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap_err()
                .code,
            "no_target"
        );
        assert_eq!(
            notion_schema_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap_err()
                .code,
            "no_target"
        );

        with_notion_target(&mut lab, "cred:notion/nomad", DATABASE_ID);
        // Without a token: access check returns ok=false, schema rejects with secrets.
        let missing =
            test_notion_access_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap();
        assert!(!missing.ok && !missing.token_set);
        assert_eq!(
            notion_schema_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap_err()
                .code,
            "secrets"
        );

        let token = notion.token();
        set_target_secret(&store, lab.repos(), "cred:notion/nomad", &token).unwrap();

        let access =
            test_notion_access_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap();
        assert!(access.ok && access.token_set, "{access:?}");
        assert_eq!(access.database_title.as_deref(), Some("Tasks"));
        assert_eq!(access.data_sources.len(), 1);
        assert!(!access.message.contains(&token));

        let schema =
            notion_schema_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client()).unwrap();
        assert!(!schema.data_source_id.is_empty());
        assert_eq!(schema.proposed.title, "Name");
        assert_eq!(schema.proposed.key, "BBB key");
        assert_eq!(schema.proposed.status.as_deref(), Some("Status"));
        assert_eq!(schema.proposed.status_value.as_deref(), Some("Not started"));
        assert!(schema.problems.is_empty(), "{:?}", schema.problems);

        // Saving a mapping with status but no statusValue is rejected by cycle validation.
        let mut missing_status_value = schema.proposed.clone();
        missing_status_value.status_value = None;
        assert_eq!(
            cycles::set_notion_mapping(
                lab.repos(),
                "NOMAD",
                Some(DATA_SOURCE_ID),
                missing_status_value,
            )
            .unwrap_err()
            .code,
            "cycle_invalid"
        );
        assert_eq!(notion_problem_field("status_value"), "statusValue");

        // Saving a mapping with a nonexistent Notion property succeeds locally and is
        // reported as a problem by notion_schema.
        let mut mismatch = schema.proposed.clone();
        mismatch.assignee = Some("NoSuchColumn".into());
        let saved =
            cycles::set_notion_mapping(lab.repos(), "NOMAD", Some(DATA_SOURCE_ID), mismatch)
                .unwrap();
        assert_eq!(
            saved.notion.as_ref().unwrap().data_source.as_deref(),
            Some(DATA_SOURCE_ID)
        );
        let after =
            notion_schema_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client()).unwrap();
        assert!(
            after.problems.iter().any(|p| p.field == "assignee"),
            "{:?}",
            after.problems
        );

        // Unshared database returns ok=false with a sharing hint, never the token.
        notion.share(false);
        let unshared =
            test_notion_access_at(&store, lab.repos(), "NOMAD", &notion.api_base(), client())
                .unwrap();
        assert!(
            !unshared.ok && unshared.bot_name.is_some() && !unshared.message.contains(&token),
            "{unshared:?}"
        );
    }

    #[test]
    fn discord_bot_checks_members_roles_and_participant_mapping_work_against_fake_bot() {
        use publishers::discord_bot::perms;
        use publishers::testing::FakeDiscordBot;

        let bot = FakeDiscordBot::start();
        let mut lab = Lab::new();
        lab.make_cycle("NOMAD");
        let store = MemorySecrets::default();

        assert_eq!(
            test_discord_bot_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client())
                .unwrap_err()
                .code,
            "no_target"
        );

        with_discord_bot_target(
            &mut lab,
            "cred:discord-bot/nomad",
            &bot.guild_id(),
            &bot.channel_id(),
            true,
        );
        let no_token =
            test_discord_bot_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client()).unwrap();
        assert!(!no_token.ok && !no_token.token_set);

        let token = bot.bot_token();
        set_target_secret(&store, lab.repos(), "cred:discord-bot/nomad", &token).unwrap();

        let ok =
            test_discord_bot_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client()).unwrap();
        assert!(ok.ok && ok.token_set, "{ok:?}");
        assert_eq!(ok.bot_name.as_deref(), Some("blah-blah-bot"));
        assert_eq!(ok.guild_name.as_deref(), Some("Nomad team"));
        assert_eq!(ok.channel_name.as_deref(), Some("meetings"));
        assert_eq!(ok.channel_kind, Some("text"));
        assert!(ok.missing.is_empty());
        assert!(!ok.message.contains(&token));

        // Removing thread permissions reports SCREAMING_SNAKE_CASE names.
        bot.set_bot_permissions(
            perms::VIEW_CHANNEL
                | perms::SEND_MESSAGES
                | perms::ATTACH_FILES
                | perms::READ_MESSAGE_HISTORY,
        );
        let lacking =
            test_discord_bot_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client()).unwrap();
        assert!(!lacking.ok, "{lacking:?}");
        assert_eq!(
            lacking.missing,
            ["CREATE_PUBLIC_THREADS", "SEND_MESSAGES_IN_THREADS"]
        );

        // Forum channel is rejected with channelKind = "forum".
        bot.set_bot_permissions(perms::INVITE_PERMISSIONS);
        with_discord_bot_target(
            &mut lab,
            "cred:discord-bot/nomad",
            &bot.guild_id(),
            &bot.forum_channel_id(),
            true,
        );
        let forum =
            test_discord_bot_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client()).unwrap();
        assert!(
            !forum.ok && forum.channel_kind == Some("forum"),
            "{forum:?}"
        );

        // Member search requires >= 2 chars and matches username/nickname prefixes.
        assert_eq!(
            search_discord_members_at(&store, lab.repos(), "NOMAD", "a", &bot.api_base(), client())
                .unwrap_err()
                .code,
            "invalid"
        );
        let found = search_discord_members_at(
            &store,
            lab.repos(),
            "NOMAD",
            "alex",
            &bot.api_base(),
            client(),
        )
        .unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|m| m.username == "alex_k"));

        // Roles exclude @everyone.
        let roles =
            list_discord_roles_at(&store, lab.repos(), "NOMAD", &bot.api_base(), client()).unwrap();
        assert!(roles.iter().all(|r| r.name != "@everyone"), "{roles:?}");
        assert!(
            roles
                .iter()
                .any(|r| r.role_id == bot.regulars_role_id() && r.mentionable),
            "{roles:?}"
        );

        // set_participant_discord sets userId, switches to roleId, clears, and validates.
        let mut cycle = lab.repo().load_cycle("NOMAD").unwrap();
        cycle.participants.push(cycle_store::Participant {
            id: "alex".into(),
            name: "Alex".into(),
            github: None,
            discord: None,
        });
        lab.repo().save_cycle(&cycle).unwrap();

        let saved = cycles::set_participant_discord(
            lab.repos(),
            "NOMAD",
            "alex",
            Some(cycles::DiscordRefDto {
                user_id: Some("600000000000000001".into()),
                role_id: None,
            }),
        )
        .unwrap();
        assert_eq!(
            saved.participants[0]
                .discord
                .as_ref()
                .and_then(|d| d.user_id.as_deref()),
            Some("600000000000000001")
        );
        let cleared = cycles::set_participant_discord(lab.repos(), "NOMAD", "alex", None).unwrap();
        assert!(cleared.participants[0].discord.is_none());
        assert_eq!(
            cycles::set_participant_discord(
                lab.repos(),
                "NOMAD",
                "alex",
                Some(cycles::DiscordRefDto {
                    user_id: Some("not-digits".into()),
                    role_id: None,
                }),
            )
            .unwrap_err()
            .code,
            "invalid"
        );
    }
}
