//! Reading the state of a cycle's GitHub issues into the cache the briefing uses: which are
//! open, which were closed (by a person or by an approved status change) and which issue was
//! made from which item. Free of Tauri; the network and the credential store are parameters.
//!
//! A failure never stops anything: the briefing then uses the last cached state, as it did
//! before issues were read at all.

use cycle_store::issues::MAX_PAGES;
use cycle_store::{apply_fetch, IssueList, Repo};
use publishers::github::Github;
use publishers::http::HttpClient;
use std::time::Duration;

use crate::publish_secrets::github_identity;
use crate::secrets::{self, SecretStore};
use crate::settings::CycleRepoSetting;

/// A client that does not hold up a briefing for long when the network is gone.
pub fn quick_client() -> HttpClient {
    HttpClient::with_timeouts(Duration::from_secs(5), Duration::from_secs(20))
}

/// Fetches the issues (all states, the most recently updated 500) of `cycle_id` from the
/// GitHub repository the registered repository points to, and stores them in the cache next
/// to the clone. `None` when the repository is not linked to GitHub (nothing to do). With no
/// token stored the request is anonymous, which reads a public repository.
///
/// Returns the list that is now in effect: fresh, or from the cache with the reason.
pub fn refresh_cycle_issues(
    store: &dyn SecretStore,
    setting: &CycleRepoSetting,
    repo: &Repo,
    cycle_id: &str,
    client: HttpClient,
) -> Option<IssueList> {
    let (github_repo, api) = github_identity(setting).ok()?;
    let host = publishers::github::api_host(&api)?;
    let token = store
        .get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host))
        .unwrap_or_default();
    let cache_path = repo.issue_cache_path(cycle_id).ok()?;
    let fetched = Github::with_client(client, &api, &token)
        .and_then(|github| github.list_cycle_issues(&github_repo, cycle_id, MAX_PAGES))
        .map_err(|e| e.message);
    Some(apply_fetch(
        fetched,
        &github_repo.full_name(),
        &format!("cycle:{cycle_id}"),
        cycle_id,
        &cache_path,
    ))
}

/// [`refresh_cycle_issues`] for every cycle of a repository; returns one note per cycle whose
/// issues could not be refreshed (the cache stays in use).
pub fn refresh_repo_issues(
    store: &dyn SecretStore,
    setting: &CycleRepoSetting,
    client: impl Fn() -> HttpClient,
) -> Vec<String> {
    let Ok(repo) = Repo::open(&setting.path) else {
        return Vec::new();
    };
    let Ok(entries) = repo.list_cycles() else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    for entry in entries.iter().filter(|e| e.is_valid()) {
        if let Some(list) = refresh_cycle_issues(store, setting, &repo, &entry.dir_name, client()) {
            if let Some(why) = list.error {
                notes.push(format!(
                    "the issues of {} could not be refreshed (the last known state is used): {why}",
                    entry.dir_name
                ));
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use publishers::testing::{FakeGithub, Visibility};

    use super::*;
    use crate::cycles::load_cycle_data;
    use crate::cycles::tests::Lab;
    use crate::publish_secrets::{set_github_repo, set_github_token};
    use crate::secrets::MemorySecrets;

    const REPO: &str = "owner/rover";
    const TOKEN: &str = "ghp_MockTokenValue0123456789";

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

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

    #[test]
    fn closed_issues_and_item_links_reach_the_next_briefing() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token(TOKEN);
        let lab = linked(&gh);
        let store = MemorySecrets::default();
        set_github_token(&store, Some(&gh.api_base()), TOKEN).unwrap();

        // Two issues made by the bot; a person closes one of them on GitHub.
        let api = Github::with_client(client(), &gh.api_base(), TOKEN).unwrap();
        let repo_ref = publishers::github::RepoRef::parse(REPO).unwrap();
        let open_one = gh_issue(&api, &repo_ref, "i1");
        let closed_one = gh_issue(&api, &repo_ref, "i2");
        gh.close_issue(REPO, closed_one);
        gh.add_issue(REPO, "By hand", &["cycle:NOMAD"]);

        let repo = lab.repo();
        let list = refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();
        assert!(!list.from_cache && list.error.is_none(), "{list:?}");
        assert_eq!(list.closed, [closed_one]);

        // The context data of the next meeting has them.
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert_eq!(data.closed_issue_ids, [closed_one.to_string()]);
        assert_eq!(data.issues.len(), 2, "the two open issues");
        let by_item = |item: &str| {
            data.issue_links
                .iter()
                .find(|l| l.item == item)
                .unwrap_or_else(|| panic!("no link for {item}"))
        };
        assert_eq!(by_item("i1").number, open_one);
        assert!(by_item("i1").open);
        assert!(!by_item("i2").open);
        assert_eq!(by_item("i2").meeting, "20261113-140501");

        // Reopened on GitHub: the next refresh sees it.
        gh.reopen_issue(REPO, closed_one);
        refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert!(data.closed_issue_ids.is_empty());
        assert!(by_open(&data, "i2"));
    }

    fn by_open(data: &queue::context::CycleData, item: &str) -> bool {
        data.issue_links.iter().any(|l| l.item == item && l.open)
    }

    fn gh_issue(api: &Github, repo: &publishers::github::RepoRef, item: &str) -> u64 {
        api.publish_issue(
            repo,
            &publishers::github::IssueSpec {
                cycle_id: "NOMAD".into(),
                meeting_id: "20261113-140501".into(),
                item_id: item.into(),
                title: format!("Task {item}"),
                assignee: None,
                due: None,
                meeting_date: "2026-11-13".into(),
                report_url: None,
                transcript_url: None,
                transcript_anchor: None,
                time_ms: None,
                quote: None,
            },
            false,
        )
        .unwrap()
        .number
    }

    #[test]
    fn a_failed_refresh_keeps_the_cached_state_and_says_why() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token(TOKEN);
        let lab = linked(&gh);
        let store = MemorySecrets::default();
        set_github_token(&store, Some(&gh.api_base()), TOKEN).unwrap();
        let first = gh.add_issue(REPO, "Closed later", &["cycle:NOMAD"]);
        gh.close_issue(REPO, first);
        let repo = lab.repo();
        refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();

        gh.go_offline();
        let list = refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();
        assert!(list.from_cache);
        assert_eq!(list.closed, [first], "the cached state stays in effect");
        let why = list.error.unwrap();
        assert!(!why.is_empty() && !why.contains(TOKEN), "{why}");
        let notes = refresh_repo_issues(&store, &lab.repos()[0], client);
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("NOMAD") && !notes[0].contains(TOKEN),
            "{notes:?}"
        );
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert_eq!(data.closed_issue_ids, [first.to_string()]);

        // A wrong token is reported without the token.
        gh.go_online();
        set_github_token(
            &store,
            Some(&gh.api_base()),
            "ghp_WrongTokenValue9999999999",
        )
        .unwrap();
        let list = refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();
        assert!(list.from_cache);
        assert!(!list.error.unwrap().contains("WrongTokenValue"));
    }

    #[test]
    fn a_repository_that_is_not_on_github_is_left_alone() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let store = MemorySecrets::default();
        let repo = lab.repo();
        assert!(refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).is_none());
        assert!(refresh_repo_issues(&store, &lab.repos()[0], client).is_empty());
    }

    #[test]
    fn a_public_repository_is_read_without_a_token() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Public);
        let lab = linked(&gh);
        let store = MemorySecrets::default();
        gh.add_issue(REPO, "Open", &["cycle:NOMAD"]);
        let repo = lab.repo();
        let list = refresh_cycle_issues(&store, &lab.repos()[0], &repo, "NOMAD", client()).unwrap();
        assert!(!list.from_cache, "{list:?}");
        assert_eq!(list.issues.len(), 1);
        assert!(gh
            .requests()
            .iter()
            .all(|r| r.header("authorization").is_none()));
    }
}
