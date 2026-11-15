//! Secrets and access checks of the publishing targets, free of Tauri: the GitHub token of an
//! API host, the secrets cycle files refer to as `cred:...` (a Discord webhook), and the
//! "does this work" checks the settings screen offers.
//!
//! Rules: a value is validated before it is stored; a value is never returned (the screens
//! only learn *whether* one is set, and a masked webhook id); every message that leaves this
//! module has been through [`publishers::http::redact`]. The commands in
//! `publish_commands.rs` are thin wrappers.

use std::collections::BTreeSet;

use cycle_store::Repo;
use publishers::github::{self, Github, RepoRef};
use publishers::http::{redact, HttpClient};
use publishers::webhook::Webhook;
use serde::Serialize;

use crate::cycles;
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
    /// `discord`.
    pub kind: &'static str,
    pub set: bool,
    /// The host and the ends of the webhook id, never the address.
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

// ----- discovery ---------------------------------------------------------------------------

/// A `cred:` reference of a Discord target and the cycle that has it.
fn discord_refs(repos: &[CycleRepoSetting]) -> Vec<(String, String)> {
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
                if secrets::reference_name(&discord.webhook).is_ok() {
                    out.push((discord.webhook.trim().to_string(), cycle.id));
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
    for (reference, cycle_id) in discord_refs(repos) {
        if !seen.insert(reference.clone()) {
            continue;
        }
        let stored = secrets::resolve_cred(store, &reference);
        let masked = stored
            .as_deref()
            .and_then(|v| Webhook::parse(v).ok())
            .map(|w| w.masked());
        targets.push(TargetSecretDto {
            reference,
            cycle_id,
            kind: "discord",
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

/// Whether `reference` stands for a Discord webhook: a cycle uses it as one, or its name
/// says so.
fn is_webhook_reference(repos: &[CycleRepoSetting], reference: &str) -> bool {
    let name = secrets::reference_name(reference).unwrap_or("");
    name.starts_with("discord/")
        || discord_refs(repos)
            .iter()
            .any(|(r, _)| r.as_str() == reference.trim())
}

/// Stores the value of a `cred:` reference. A Discord webhook is validated first and
/// stored in its normalised form; the error never repeats the value.
pub fn set_target_secret(
    store: &dyn SecretStore,
    repos: &[CycleRepoSetting],
    reference: &str,
    value: &str,
) -> IpcResult<()> {
    let entry = secrets::target_entry(reference)?;
    let value = value.trim();
    if is_webhook_reference(repos, reference) {
        let hook = Webhook::parse(value).map_err(|why| {
            IpcError::new("invalid_webhook", format!("not a Discord webhook: {why}"))
        })?;
        return store.set(&entry, &hook.url());
    }
    if value.len() > 4096 || value.chars().any(char::is_control) {
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

// ----- redaction ---------------------------------------------------------------------------

/// `text` with the given values and everything shaped like a secret masked.
pub fn scrub_with(values: &[&str], text: &str) -> String {
    redact(text, values)
}

/// `text` with every secret this application holds for publishing masked: the GitHub tokens
/// and the values behind `cred:` references of the cycles. For anything that is stored or
/// shown: outbox errors, log lines, git output.
// Used by the outbox dispatcher, which is wired in a later step.
#[allow(dead_code)]
pub fn scrub(store: &dyn SecretStore, repos: &[CycleRepoSetting], text: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    for (host, _) in api_hosts(repos) {
        if let Some(token) = store.get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host)) {
            values.push(token);
        }
    }
    for (reference, _) in discord_refs(repos) {
        if let Some(value) = secrets::resolve_cred(store, &reference) {
            if let Ok(hook) = Webhook::parse(&value) {
                values.push(hook.token().to_string());
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
            webhook: reference.into(),
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
}
