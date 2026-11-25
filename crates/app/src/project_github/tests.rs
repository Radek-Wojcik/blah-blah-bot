//! The GitHub step of the wizard against the fake GitHub: no real host, no real token.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use publishers::http::HttpClient;
use publishers::testing::{FakeGithub, Visibility};
use store::Store;

use super::*;
use crate::secrets::MemorySecrets;

const TOKEN: &str = "ghp_fake0123456789abcdefghij";

fn client() -> HttpClient {
    HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
}

fn secrets_for(api: &str, token: Option<&str>) -> MemorySecrets {
    let secrets = MemorySecrets::default();
    if let Some(token) = token {
        secrets
            .set(&secrets::github_token_entry(api).unwrap(), token)
            .unwrap();
    }
    secrets
}

fn github_request(owner: Option<&str>, confirmed: bool) -> GithubRepoRequestDto {
    GithubRepoRequestDto {
        name: "rover".into(),
        owner: owner.map(str::to_string),
        confirmed,
    }
}

fn wizard_request(github_repo: Option<&str>) -> WizardCreateProjectDto {
    WizardCreateProjectDto {
        mode: "auto".into(),
        repo_path: None,
        remote_url: None,
        id: "ROVER".into(),
        name: "Nomad Rover".into(),
        output_language: Some("en".into()),
        default_report_mode: None,
        github_repo: github_repo.map(str::to_string),
        discord_webhook_ref: None,
        local_save_enabled: None,
        local_save_dir: None,
    }
}

struct World {
    dir: test_tmp::TestDir,
    store: Store,
    settings: Settings,
}

impl World {
    fn new(name: &str) -> World {
        World {
            dir: test_tmp::TestDir::new(name),
            store: Store::open_in_memory().unwrap(),
            settings: Settings::default(),
        }
    }

    fn project_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("projects").join("ROVER")
    }

    fn run(
        &mut self,
        req: WizardCreateProjectDto,
        attempt: Attempt,
    ) -> IpcResult<WizardCreateResultDto> {
        finish_in(
            &mut self.settings,
            &self.store,
            &self.store,
            self.dir.path(),
            req,
            attempt,
        )
    }
}

fn origin_of(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn assert_local_only(world: &World, result: &WizardCreateResultDto, reason: GithubFallbackReason) {
    assert_eq!(result.github.status, GithubOutcomeStatus::LocalOnly);
    assert_eq!(result.github.reason, Some(reason));
    assert!(result.github.repo.is_none());
    assert!(world.project_dir().join(".git").exists());
    assert_eq!(origin_of(&world.project_dir()), None);
    assert_eq!(world.settings.cycle_repos.len(), 1);
    assert_eq!(world.settings.cycle_repos[0].remote, None);
    assert_eq!(world.settings.cycle_repos[0].github_repo, None);
    assert!(!result.github.message.contains(TOKEN));
    assert!(result.github.message.contains("local project folder"));
}

#[test]
fn the_status_only_says_whether_a_token_is_saved() {
    let api = "http://127.0.0.1:9";
    let none = secrets_for(api, None);
    let status = repo_status_in(&none, api).unwrap();
    assert!(!status.token_set);
    assert_eq!(status.host, "127.0.0.1");
    let some = secrets_for(api, Some(TOKEN));
    let status = repo_status_in(&some, api).unwrap();
    assert!(status.token_set);
    assert!(!format!("{status:?}").contains(TOKEN));
}

#[test]
fn a_confirmed_request_with_a_token_creates_a_private_repository_and_links_it() {
    let gh = FakeGithub::start();
    gh.require_token(TOKEN);
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, true),
    );
    let mut world = World::new("wizard-gh-created");
    // A link typed into the wizard is replaced by the repository that was really created.
    let result = world
        .run(wizard_request(Some("typed/elsewhere")), attempt)
        .unwrap();

    assert_eq!(result.github.status, GithubOutcomeStatus::Created);
    assert_eq!(result.github.reason, None);
    assert_eq!(result.github.repo.as_deref(), Some("fake-bot/rover"));
    assert!(result.github.message.contains("private"));
    assert!(!result.github.message.contains(TOKEN));
    assert_eq!(result.cycle.id, "ROVER");
    assert!(result.cycle.targets.github.is_some());

    assert_eq!(
        gh.visibility_of("fake-bot/rover"),
        Some(Visibility::Private)
    );
    let sent = gh.requests_to("POST", "/user/repos");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].json().unwrap()["private"], true);

    let clone_url = format!("{}/fake-bot/rover.git", gh.api_base());
    assert_eq!(
        origin_of(&world.project_dir()).as_deref(),
        Some(clone_url.as_str())
    );
    let entry = &world.settings.cycle_repos[0];
    assert_eq!(entry.remote.as_deref(), Some(clone_url.as_str()));
    assert_eq!(entry.github_repo.as_deref(), Some("fake-bot/rover"));
    assert_eq!(entry.github_api.as_deref(), Some(gh.api_base().as_str()));
}

#[test]
fn an_organization_owner_uses_the_organization_endpoint_and_the_own_login_does_not() {
    let gh = FakeGithub::start();
    gh.add_organization("nomad-org");
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));

    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(Some("nomad-org"), true),
    );
    let mut world = World::new("wizard-gh-org");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_eq!(result.github.repo.as_deref(), Some("nomad-org/rover"));
    assert_eq!(gh.requests_to("POST", "/orgs/nomad-org/repos").len(), 1);

    // The token's own login is the personal account (`/user/repos`), whatever the case.
    let gh = FakeGithub::start();
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(Some("Fake-Bot"), true),
    );
    let mut world = World::new("wizard-gh-own-login");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_eq!(result.github.repo.as_deref(), Some("fake-bot/rover"));
    assert_eq!(gh.requests_to("POST", "/user/repos").len(), 1);
    assert_eq!(gh.requests_to("POST", "/orgs/").len(), 0);
}

#[test]
fn without_a_token_nothing_is_sent_and_the_project_is_local_only() {
    let gh = FakeGithub::start();
    let secrets = secrets_for(&gh.api_base(), None);
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, true),
    );
    let mut world = World::new("wizard-gh-no-token");
    let result = world
        .run(wizard_request(Some("typed/elsewhere")), attempt)
        .unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::NoToken);
    assert!(result.github.message.contains("Settings > Connectors"));
    assert_eq!(gh.request_count(), 0);
}

#[test]
fn nothing_is_sent_before_the_confirmation() {
    let gh = FakeGithub::start();
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, false),
    );
    let mut world = World::new("wizard-gh-unconfirmed");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::NotConfirmed);
    assert_eq!(gh.request_count(), 0);
}

#[test]
fn a_bad_repository_name_is_refused_before_the_network() {
    let gh = FakeGithub::start();
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let mut request = github_request(None, true);
    request.name = "not a name".into();
    let attempt = attempt_create(&secrets, &gh.api_base(), client(), &request);
    let mut world = World::new("wizard-gh-bad-name");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::InvalidName);
    assert_eq!(gh.request_count(), 0);
}

#[test]
fn a_rejected_token_falls_back_to_a_local_project_without_leaking_the_token() {
    let gh = FakeGithub::start();
    gh.require_token("ghp_the-right-one-0123456789");
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, true),
    );
    let mut world = World::new("wizard-gh-401");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::Failed);
    assert!(result.github.message.contains("rejected the saved token"));
    assert_eq!(gh.visibility_of("fake-bot/rover"), None);
}

#[test]
fn a_taken_name_falls_back_with_a_clear_message() {
    let gh = FakeGithub::start();
    gh.add_repo("fake-bot/rover", Visibility::Private);
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, true),
    );
    let mut world = World::new("wizard-gh-422");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::Failed);
    assert!(result
        .github
        .message
        .contains("already has a repository named rover"));
}

#[test]
fn a_forbidden_token_and_an_unknown_organization_fall_back() {
    let gh = FakeGithub::start();
    let secrets = secrets_for(&gh.api_base(), Some(TOKEN));
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(Some("ghost-org"), true),
    );
    let mut world = World::new("wizard-gh-404");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::Failed);
    assert!(result.github.message.contains("ghost-org"));

    gh.forbid_repo_creation();
    let attempt = attempt_create(
        &secrets,
        &gh.api_base(),
        client(),
        &github_request(None, true),
    );
    let mut world = World::new("wizard-gh-403");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::Failed);
    assert!(result
        .github
        .message
        .contains("not allowed to create repositories"));
}

#[test]
fn an_unreachable_github_falls_back_to_a_local_project() {
    let api = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let secrets = secrets_for(&api, Some(TOKEN));
    let attempt = attempt_create(&secrets, &api, client(), &github_request(None, true));
    let mut world = World::new("wizard-gh-offline");
    let result = world.run(wizard_request(None), attempt).unwrap();
    assert_local_only(&world, &result, GithubFallbackReason::Failed);
    assert!(result.github.message.contains("could not be reached"));
}

#[test]
fn only_a_new_project_folder_can_be_combined_with_a_new_repository() {
    let mut world = World::new("wizard-gh-existing");
    let mut req = wizard_request(None);
    req.mode = "existing".into();
    let attempt = fallback(GithubFallbackReason::NoToken, "x");
    assert_eq!(world.run(req, attempt.clone()).unwrap_err().code, "invalid");
    let mut req = wizard_request(None);
    req.id = "not valid!".into();
    assert_eq!(world.run(req, attempt).unwrap_err().code, "invalid");
    assert!(world.settings.cycle_repos.is_empty());
}
