//! Creating a repository: `POST /user/repos` for the token's own account, `POST /orgs/{org}/repos`
//! for an organisation. The repository is always created **private** and empty (no README), so
//! the first publish is its first commit.
//!
//! The call goes through the same [`HttpClient`](crate::http::HttpClient) as every other GitHub
//! call: `https` only (plain `http` for this machine), no redirect is followed, and the token
//! is registered with the client so no error text can contain it. The request is never retried
//! here: a second attempt after a lost answer would only meet a 422 "name already exists".

use serde_json::{json, Value};

use super::{valid_part, Github};
use crate::http::{ErrorKind, HttpError, Request};

/// What to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRepo<'a> {
    /// The organisation to create it in; `None` for the account of the token.
    pub owner: Option<&'a str>,
    pub name: &'a str,
    pub description: Option<&'a str>,
}

/// A repository GitHub created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedRepo {
    pub owner: String,
    pub name: String,
    /// What GitHub says; a repository this module creates is private, and a caller checks.
    pub private: bool,
    /// The `https` address Git clones from and pushes to.
    pub clone_url: String,
    pub html_url: String,
}

impl CreatedRepo {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

fn permanent(kind: ErrorKind, message: impl Into<String>) -> HttpError {
    HttpError {
        kind,
        message: message.into(),
        status: None,
        failure: store::Failure::Permanent,
    }
}

impl Github {
    /// Creates a private, empty repository. A name or owner GitHub would have to rewrite is
    /// refused before anything is sent. Errors: 401 (bad token), 403 (the token may not
    /// create repositories there), 404 (no such organisation), 422 (the name is taken).
    pub fn create_private_repo(&self, spec: &NewRepo<'_>) -> Result<CreatedRepo, HttpError> {
        if !valid_part(spec.name) || spec.name.ends_with(".git") {
            return Err(permanent(
                ErrorKind::InvalidRequest,
                "the repository name may contain only letters, digits, - _ and .",
            ));
        }
        if let Some(owner) = spec.owner {
            if !valid_part(owner) {
                return Err(permanent(
                    ErrorKind::InvalidRequest,
                    "the owner may contain only letters, digits, - _ and .",
                ));
            }
        }
        let path = match spec.owner {
            Some(owner) => format!("/orgs/{owner}/repos"),
            None => "/user/repos".to_string(),
        };
        let mut body = json!({
            "name": spec.name,
            "private": true,
            "auto_init": false,
            "has_issues": true,
        });
        if let Some(description) = spec.description.map(str::trim).filter(|d| !d.is_empty()) {
            body["description"] = Value::String(description.chars().take(350).collect());
        }
        let request = self.authorised(Request::post_json(format!("{}{path}", self.api), &body));
        let answer = self.client.send(&request)?.json()?;
        parse_created(&answer)
    }
}

fn parse_created(value: &Value) -> Result<CreatedRepo, HttpError> {
    let text = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
    let (Some(owner), Some(name), Some(clone_url)) =
        (text("/owner/login"), text("/name"), text("/clone_url"))
    else {
        return Err(permanent(
            ErrorKind::Body,
            "the answer of GitHub does not describe the new repository",
        ));
    };
    Ok(CreatedRepo {
        owner: owner.to_string(),
        name: name.to_string(),
        private: value.get("private").and_then(Value::as_bool) == Some(true),
        clone_url: clone_url.to_string(),
        html_url: text("/html_url").unwrap_or_default().to_string(),
    })
}

/// A sentence for the person about why a repository could not be created, with no token and no
/// raw server text beyond what [`HttpClient`](crate::http::HttpClient) already redacted.
pub fn create_failure_message(error: &HttpError, spec: &NewRepo<'_>) -> String {
    let target = match spec.owner {
        Some(owner) => format!("the organization {owner}"),
        None => "your GitHub account".to_string(),
    };
    match error.status {
        Some(401) => "GitHub rejected the saved token: it is wrong, expired or revoked".into(),
        Some(403) => format!(
            "the saved token is not allowed to create repositories in {target} (it needs the repository creation permission)"
        ),
        Some(404) => match spec.owner {
            Some(owner) => format!(
                "GitHub does not know the organization {owner}, or the token cannot see it"
            ),
            None => "GitHub could not find the account of the token".into(),
        },
        Some(422) => format!(
            "{target} already has a repository named {}, or GitHub does not accept that name",
            spec.name
        ),
        _ if error.is_offline() => {
            format!("GitHub could not be reached ({})", error.message)
        }
        _ => error.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpClient;
    use crate::testing::{FakeGithub, Visibility};
    use std::time::Duration;

    const TOKEN: &str = "ghp_t0ken0123456789abcdef";

    fn client() -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
    }

    fn api(gh: &FakeGithub, token: &str) -> Github {
        Github::with_client(client(), &gh.api_base(), token).unwrap()
    }

    fn spec<'a>(owner: Option<&'a str>, name: &'a str) -> NewRepo<'a> {
        NewRepo {
            owner,
            name,
            description: Some("Meeting notes of the rover team"),
        }
    }

    #[test]
    fn creates_a_private_repository_for_the_user() {
        let gh = FakeGithub::start();
        gh.require_token(TOKEN);
        let created = api(&gh, TOKEN)
            .create_private_repo(&spec(None, "nomad-rover"))
            .unwrap();
        assert_eq!(created.full_name(), "fake-bot/nomad-rover");
        assert!(created.private);
        assert!(created.clone_url.ends_with("/fake-bot/nomad-rover.git"));
        assert_eq!(created.html_url, "https://github.com/fake-bot/nomad-rover");
        assert_eq!(
            gh.visibility_of("fake-bot/nomad-rover"),
            Some(Visibility::Private)
        );

        let sent = gh.requests_to("POST", "/user/repos");
        assert_eq!(sent.len(), 1);
        let body = sent[0].json().unwrap();
        assert_eq!(body["private"], true);
        assert_eq!(body["auto_init"], false);
        assert_eq!(body["name"], "nomad-rover");
        assert_eq!(
            sent[0].header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }

    #[test]
    fn creates_a_private_repository_in_an_organization() {
        let gh = FakeGithub::start();
        gh.add_organization("Nomad-Org");
        let created = api(&gh, TOKEN)
            .create_private_repo(&spec(Some("Nomad-Org"), "rover"))
            .unwrap();
        assert_eq!(created.full_name(), "Nomad-Org/rover");
        assert!(created.private);
        assert_eq!(gh.requests_to("POST", "/orgs/Nomad-Org/repos").len(), 1);
        assert_eq!(gh.requests_to("POST", "/user/repos").len(), 0);
    }

    #[test]
    fn a_wrong_token_is_a_401_and_never_appears_in_the_error() {
        let gh = FakeGithub::start();
        gh.require_token(TOKEN);
        let wrong = "ghp_wr0ng0123456789abcdef";
        let err = api(&gh, wrong)
            .create_private_repo(&spec(None, "rover"))
            .unwrap_err();
        assert_eq!(err.status, Some(401));
        assert_eq!(err.failure(), store::Failure::Permanent);
        assert!(!err.message.contains(wrong));
        let text = create_failure_message(&err, &spec(None, "rover"));
        assert!(text.contains("rejected the saved token"), "{text}");
        assert!(!text.contains(wrong));
        assert_eq!(gh.visibility_of("fake-bot/rover"), None);
    }

    #[test]
    fn a_taken_name_is_a_422_with_a_clear_message() {
        let gh = FakeGithub::start();
        let github = api(&gh, TOKEN);
        github.create_private_repo(&spec(None, "rover")).unwrap();
        let err = github
            .create_private_repo(&spec(None, "ROVER"))
            .unwrap_err();
        assert_eq!(err.status, Some(422));
        assert_eq!(err.failure(), store::Failure::Permanent);
        let text = create_failure_message(&err, &spec(None, "ROVER"));
        assert!(
            text.contains("already has a repository named ROVER"),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_organization_and_a_forbidden_token_are_reported() {
        let gh = FakeGithub::start();
        let err = api(&gh, TOKEN)
            .create_private_repo(&spec(Some("ghost-org"), "rover"))
            .unwrap_err();
        assert_eq!(err.status, Some(404));
        let text = create_failure_message(&err, &spec(Some("ghost-org"), "rover"));
        assert!(text.contains("ghost-org"), "{text}");

        gh.forbid_repo_creation();
        let err = api(&gh, TOKEN)
            .create_private_repo(&spec(None, "rover"))
            .unwrap_err();
        assert_eq!(err.status, Some(403));
        let text = create_failure_message(&err, &spec(None, "rover"));
        assert!(
            text.contains("not allowed to create repositories"),
            "{text}"
        );
    }

    #[test]
    fn bad_names_are_refused_before_anything_is_sent() {
        let gh = FakeGithub::start();
        let github = api(&gh, TOKEN);
        for name in ["", "a b", "../x", "x/y", "rover.git", ".."] {
            let err = github.create_private_repo(&spec(None, name)).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidRequest, "{name}");
        }
        let err = github
            .create_private_repo(&spec(Some("bad owner"), "rover"))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest);
        assert_eq!(gh.request_count(), 0);
    }

    #[test]
    fn an_unreachable_host_is_an_offline_error() {
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let github = Github::with_client(client(), &closed, TOKEN).unwrap();
        let err = github
            .create_private_repo(&spec(None, "rover"))
            .unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        let text = create_failure_message(&err, &spec(None, "rover"));
        assert!(text.contains("could not be reached"), "{text}");
    }

    #[test]
    fn plain_http_to_a_real_host_is_refused() {
        assert!(Github::with_client(client(), "http://api.github.com", TOKEN).is_err());
    }

    #[test]
    fn an_answer_without_the_repository_is_a_body_error() {
        let parsed = parse_created(&serde_json::json!({"name": "rover"}));
        assert_eq!(parsed.unwrap_err().kind, ErrorKind::Body);
        let ok = parse_created(&serde_json::json!({
            "name": "rover", "owner": {"login": "a"}, "clone_url": "https://github.com/a/rover.git"
        }))
        .unwrap();
        // An answer that does not say "private" is not taken for private.
        assert!(!ok.private);
    }
}
