//! The GitHub side of publishing: which repository a cycle repository points to, and the REST
//! calls the publisher makes (checking access now; issues are added by the write side).
//!
//! Everything goes through [`HttpClient`](crate::http::HttpClient), so a token never reaches an
//! error text, a redirect is never followed and plain `http` only works for this machine.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::http::{parse_endpoint, ErrorKind, HttpClient, HttpError, Request};

/// The public GitHub API.
pub const DEFAULT_API: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";

mod issues;

pub use issues::{
    defuse_mentions, issue_body, CorrectionSpec, DueOutcome, IssueOutcome, IssueSpec,
    StatusOutcome, StatusSpec, DUE_FIELD_NAME, WRITE_PACING,
};

// ----- repository identity -----------------------------------------------------------------

/// `owner/name` of a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

fn valid_part(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl RepoRef {
    /// Parses `owner/name`.
    pub fn parse(text: &str) -> Option<RepoRef> {
        let (owner, name) = text.trim().split_once('/')?;
        (valid_part(owner) && valid_part(name)).then(|| RepoRef {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// The repository a remote address of github.com points to: `https://github.com/o/r`,
    /// with or without `.git` or a trailing slash, `git@github.com:o/r.git` and
    /// `ssh://git@github.com/o/r.git`. Any other host gives `None`: for those (GitHub
    /// Enterprise) the repository is set explicitly.
    pub fn from_github_remote(url: &str) -> Option<RepoRef> {
        let url = url.trim();
        let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
            rest
        } else {
            let (scheme, rest) = url.split_once("://")?;
            if !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "https" | "ssh" | "git"
            ) {
                return None;
            }
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            if !host.eq_ignore_ascii_case("github.com") {
                return None;
            }
            path
        };
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        RepoRef::parse(path)
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// A checked API base address without a trailing slash: `https`, or `http` for this machine
/// only, no credentials, no query. A path is allowed (GitHub Enterprise Server uses
/// `/api/v3`).
pub fn normalize_api_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let endpoint = parse_endpoint(url).map_err(|e| e.message)?;
    if endpoint.path.contains(['?', '#']) {
        return Err("the API address must not have a query or a fragment".into());
    }
    Ok(url.to_string())
}

/// The host of an API base, lower case, without the port: the name a token is stored under.
pub fn api_host(url: &str) -> Option<String> {
    parse_endpoint(url.trim())
        .ok()
        .map(|e| e.host.to_ascii_lowercase())
}

// ----- the client --------------------------------------------------------------------------

/// What the API says about a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub full_name: String,
    /// `public`, `private` or `internal`.
    pub visibility: String,
    pub owner_is_organization: bool,
    pub has_issues: bool,
    /// Whether the token's user can push (`permissions.push`); `None` when the API says
    /// nothing about it.
    pub can_push: Option<bool>,
}

impl RepoInfo {
    pub fn is_public(&self) -> bool {
        self.visibility == "public"
    }
}

/// Answer to "can this token publish to this repository".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessReport {
    pub login: String,
    pub repo: RepoInfo,
}

/// A GitHub REST client for one API base and one token.
pub struct Github {
    client: HttpClient,
    api: String,
    token: String,
    /// The id of the organisation issue field for due dates, by organisation: `None` when
    /// the organisation has none. Only definitive answers are kept.
    due_fields: Mutex<HashMap<String, Option<u64>>>,
}

impl Github {
    /// A client with the default timeouts. `api` is checked with [`normalize_api_base`]. An
    /// empty `token` makes anonymous requests, enough to read a public repository.
    pub fn new(api: &str, token: &str) -> Result<Github, HttpError> {
        Self::with_client(HttpClient::new(), api, token)
    }

    /// A client over a given [`HttpClient`] (tests use short timeouts). The token is
    /// registered with it so no error text can contain it.
    pub fn with_client(client: HttpClient, api: &str, token: &str) -> Result<Github, HttpError> {
        let api = normalize_api_base(api).map_err(|message| HttpError {
            kind: ErrorKind::InvalidRequest,
            message,
            status: None,
            failure: store::Failure::Permanent,
        })?;
        Ok(Github {
            client: client.with_secret(token),
            api,
            token: token.to_string(),
            due_fields: Mutex::new(HashMap::new()),
        })
    }

    pub fn api_base(&self) -> &str {
        &self.api
    }

    fn authorised(&self, request: Request) -> Request {
        self.with_token(
            request
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", API_VERSION),
        )
    }

    /// The token as a bearer; an empty token (reading a public repository) sends none.
    fn with_token(&self, request: Request) -> Request {
        if self.token.is_empty() {
            request
        } else {
            request.bearer(&self.token)
        }
    }

    fn get(&self, path: &str) -> Result<Value, HttpError> {
        let request = self.authorised(Request::get(format!("{}{path}", self.api)));
        self.client.send(&request)?.json()
    }

    /// The login of the token's user (`GET /user`): the token is valid.
    pub fn current_user(&self) -> Result<String, HttpError> {
        let user = self.get("/user")?;
        user.get("login")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| HttpError {
                kind: ErrorKind::Body,
                message: "the answer of GitHub has no login".into(),
                status: None,
                failure: store::Failure::Permanent,
            })
    }

    /// Visibility, owner type and the push permission of a repository.
    pub fn repo_info(&self, repo: &RepoRef) -> Result<RepoInfo, HttpError> {
        let value = self.get(&format!("/repos/{}/{}", repo.owner, repo.name))?;
        Ok(parse_repo_info(&value, repo))
    }

    /// Checks the token and the repository in one go.
    pub fn check_access(&self, repo: &RepoRef) -> Result<AccessReport, HttpError> {
        let login = self.current_user()?;
        let repo = self.repo_info(repo)?;
        Ok(AccessReport { login, repo })
    }
}

fn parse_repo_info(value: &Value, asked: &RepoRef) -> RepoInfo {
    let private = value.get("private").and_then(Value::as_bool);
    let visibility = match value.get("visibility").and_then(Value::as_str) {
        Some(v @ ("public" | "private" | "internal")) => v.to_string(),
        _ if private == Some(false) => "public".to_string(),
        // Anything unknown is treated as not public: the warning is for public repositories,
        // and an unreadable answer must not turn into "public" by accident.
        _ => "private".to_string(),
    };
    RepoInfo {
        full_name: value
            .get("full_name")
            .and_then(Value::as_str)
            .map_or_else(|| asked.full_name(), String::from),
        visibility,
        owner_is_organization: value.pointer("/owner/type").and_then(Value::as_str)
            == Some("Organization"),
        has_issues: value
            .get("has_issues")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        can_push: value.pointer("/permissions/push").and_then(Value::as_bool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeGithub, Visibility};
    use std::time::Duration;

    const REPO: &str = "owner/rover";

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

    #[test]
    fn repo_references_are_parsed_from_remotes() {
        let want = Some(RepoRef::parse("owner/rover").unwrap());
        for url in [
            "https://github.com/owner/rover",
            "https://github.com/owner/rover.git",
            "https://github.com/owner/rover/",
            "https://GitHub.com/owner/rover.git",
            "https://user@github.com/owner/rover.git",
            "git@github.com:owner/rover.git",
            "git@github.com:owner/rover",
            "ssh://git@github.com/owner/rover.git",
        ] {
            assert_eq!(RepoRef::from_github_remote(url), want, "{url}");
        }
        for url in [
            "https://gitlab.com/owner/rover.git",
            "https://github.com.evil.example/owner/rover.git",
            "https://github.com/owner",
            "https://github.com/owner/rover/extra",
            "file:///C:/repos/rover",
            "C:/repos/rover.git",
            "http://github.com/../rover",
            "",
        ] {
            assert_eq!(RepoRef::from_github_remote(url), None, "{url}");
        }
    }

    #[test]
    fn owner_slash_name_parsing_is_strict() {
        assert_eq!(
            RepoRef::parse(" a-b/c_d.e ").unwrap().full_name(),
            "a-b/c_d.e"
        );
        for bad in ["rover", "a/b/c", "../x", "a/..", "a/b c", "/x", "a/", ""] {
            assert!(RepoRef::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn api_bases_follow_the_endpoint_rules() {
        assert_eq!(
            normalize_api_base("https://api.github.com/").unwrap(),
            "https://api.github.com"
        );
        assert_eq!(
            normalize_api_base(" https://ghe.example.com/api/v3 ").unwrap(),
            "https://ghe.example.com/api/v3"
        );
        assert!(normalize_api_base("http://127.0.0.1:8080").is_ok());
        for bad in [
            "http://api.github.com",
            "https://user:pw@api.github.com",
            "api.github.com",
            "https://api.github.com/?x=1",
            "ftp://api.github.com",
            "",
        ] {
            assert!(normalize_api_base(bad).is_err(), "{bad}");
        }
        assert_eq!(
            api_host("https://API.github.com:443/x").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(api_host("nope"), None);
    }

    #[test]
    fn checks_access_with_a_valid_token() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token("t0ken");
        let api = Github::with_client(client(), &gh.api_base(), "t0ken").unwrap();
        let report = api.check_access(&RepoRef::parse(REPO).unwrap()).unwrap();
        assert_eq!(report.login, "fake-bot");
        assert_eq!(report.repo.visibility, "private");
        assert!(!report.repo.is_public() && report.repo.has_issues);
        assert_eq!(report.repo.can_push, Some(true));
        assert!(!report.repo.owner_is_organization);

        gh.set_organization(REPO);
        gh.set_visibility(REPO, Visibility::Public);
        let report = api.check_access(&RepoRef::parse(REPO).unwrap()).unwrap();
        assert!(report.repo.is_public() && report.repo.owner_is_organization);
    }

    #[test]
    fn a_wrong_token_and_a_missing_repository_are_permanent_errors() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.require_token("t0ken");
        let wrong = Github::with_client(client(), &gh.api_base(), "wr0ng-token-value").unwrap();
        let err = wrong
            .check_access(&RepoRef::parse(REPO).unwrap())
            .unwrap_err();
        assert_eq!(err.status, Some(401));
        assert_eq!(err.failure(), store::Failure::Permanent);
        assert!(!err.message.contains("wr0ng-token-value"));

        let right = Github::with_client(client(), &gh.api_base(), "t0ken").unwrap();
        let err = right
            .check_access(&RepoRef::parse("owner/other").unwrap())
            .unwrap_err();
        assert_eq!(err.status, Some(404));
    }

    #[test]
    fn a_token_without_push_rights_is_reported() {
        let gh = FakeGithub::start();
        gh.add_repo(REPO, Visibility::Private);
        gh.set_push_permission(REPO, false);
        let api = Github::with_client(client(), &gh.api_base(), "t0ken").unwrap();
        let report = api.check_access(&RepoRef::parse(REPO).unwrap()).unwrap();
        assert_eq!(report.repo.can_push, Some(false));
    }

    #[test]
    fn an_unreadable_visibility_is_not_taken_for_public() {
        let asked = RepoRef::parse(REPO).unwrap();
        let info = parse_repo_info(&serde_json::json!({"full_name": REPO}), &asked);
        assert_eq!(info.visibility, "private");
        assert_eq!(info.can_push, None);
        let info = parse_repo_info(&serde_json::json!({"private": false}), &asked);
        assert_eq!(info.visibility, "public");
        let info = parse_repo_info(&serde_json::json!({"visibility": "weird"}), &asked);
        assert_eq!(info.visibility, "private");
    }

    #[test]
    fn a_closed_port_is_an_offline_error() {
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let api = Github::with_client(client(), &closed, "t0ken").unwrap();
        let err = api.current_user().unwrap_err();
        assert!(err.is_offline(), "{err:?}");
    }

    #[test]
    fn an_invalid_api_base_is_refused_up_front() {
        assert!(Github::new("http://api.github.com", "t").is_err());
    }
}
