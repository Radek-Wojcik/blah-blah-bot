//! Addresses derived from the identity of a repository: the web address of a published
//! report, and the key a repository is remembered under when the user confirms it.

use crate::github::{api_host, DEFAULT_API};
use crate::plan::GithubIdentity;

/// The address of the web site that belongs to a GitHub API: `https://github.com` for the
/// public GitHub, the API address without `/api/v3` for an Enterprise server, and the API
/// address itself for anything else (a server on this machine in tests).
pub fn web_base(api: &str) -> String {
    let api = api.trim().trim_end_matches('/');
    if api.eq_ignore_ascii_case(DEFAULT_API) {
        return "https://github.com".to_string();
    }
    api.strip_suffix("/api/v3").unwrap_or(api).to_string()
}

/// The key of the repository a cycle publishes to (`github.com/owner/repo`, lower case): what
/// `confirm_public_repo` stores and the dispatcher looks up.
pub fn repo_key_of(github: &GithubIdentity) -> String {
    let host = if api_host(&github.api).as_deref() == api_host(DEFAULT_API).as_deref() {
        "github.com".to_string()
    } else {
        api_host(&github.api).unwrap_or_default()
    };
    store::repo_key(&host, &github.repo.full_name())
}

/// The key of a remote that is not on GitHub: its host and path, without scheme, user and
/// `.git`. A remote that is a folder (or any path) is keyed under `local`.
pub fn remote_key(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some((_, rest)) = url.split_once("://") {
        if rest.starts_with('/') || url.starts_with("file:") {
            return Some(store::repo_key("local", rest.trim_start_matches('/')));
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host.split(':').next().unwrap_or(host);
        return Some(store::repo_key(host, path));
    }
    // `git@host:owner/repo.git`
    if let Some((authority, path)) = url.split_once(':') {
        if authority.contains('@') && !path.starts_with('/') && !authority.contains('/') {
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            return Some(store::repo_key(host, path));
        }
    }
    Some(store::repo_key("local", &url.replace('\\', "/")))
}

/// The address of `report.md` of a published meeting; `base_path` is the folder of the
/// meetings in the repository (`targets.github.path`).
pub fn report_url(
    web_base: &str,
    repo_full_name: &str,
    branch: &str,
    base_path: &str,
    cycle: &str,
    folder: &str,
) -> String {
    format!("{web_base}/{repo_full_name}/blob/{branch}/{base_path}/{cycle}/{folder}/report.md")
}

/// The address of `transcript.md` next to a report.
pub fn transcript_url(report_url: &str) -> Option<String> {
    report_url
        .strip_suffix("/report.md")
        .map(|dir| format!("{dir}/transcript.md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::RepoRef;

    fn id(api: &str) -> GithubIdentity {
        GithubIdentity {
            repo: RepoRef::parse("Owner/Rover").unwrap(),
            api: api.to_string(),
        }
    }

    #[test]
    fn the_web_site_of_an_api() {
        assert_eq!(web_base("https://api.github.com"), "https://github.com");
        assert_eq!(web_base("https://api.github.com/"), "https://github.com");
        assert_eq!(
            web_base("https://ghe.example.com/api/v3"),
            "https://ghe.example.com"
        );
        assert_eq!(web_base("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
    }

    #[test]
    fn repositories_are_keyed_by_host_and_lower_case_name() {
        assert_eq!(
            repo_key_of(&id("https://api.github.com")),
            "github.com/owner/rover"
        );
        assert_eq!(
            repo_key_of(&id("https://ghe.example.com/api/v3")),
            "ghe.example.com/owner/rover"
        );
        assert_eq!(
            repo_key_of(&id("http://127.0.0.1:9")),
            "127.0.0.1/owner/rover"
        );
    }

    #[test]
    fn other_remotes_are_keyed_by_host_and_path() {
        assert_eq!(
            remote_key("https://user@gitlab.com:8443/Team/Rover.git").as_deref(),
            Some("gitlab.com/team/rover")
        );
        assert_eq!(
            remote_key("git@gitlab.com:Team/Rover.git").as_deref(),
            Some("gitlab.com/team/rover")
        );
        assert_eq!(
            remote_key("C:\\repos\\Rover.git").as_deref(),
            Some("local/c:/repos/rover")
        );
        assert_eq!(remote_key("  "), None);
    }

    #[test]
    fn report_and_transcript_addresses() {
        let r = report_url(
            "https://github.com",
            "o/r",
            "main",
            "meetings",
            "NOMAD",
            "2026-11-13",
        );
        assert_eq!(
            r,
            "https://github.com/o/r/blob/main/meetings/NOMAD/2026-11-13/report.md"
        );
        assert_eq!(
            transcript_url(&r).unwrap(),
            "https://github.com/o/r/blob/main/meetings/NOMAD/2026-11-13/transcript.md"
        );
        assert_eq!(transcript_url("https://x/y"), None);
    }
}
