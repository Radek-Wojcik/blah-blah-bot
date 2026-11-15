//! Open issues of a cycle (GitHub Issues labelled `cycle:<id>`), read-only, with a cache so
//! the briefing still works offline.
//!
//! The token is a parameter of [`fetch_open_issues`]; nothing here reads it from disk or
//! the environment.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CycleError, Result};
use crate::fsutil::write_atomic;
use crate::time::{format_utc_date, now_unix_ms};

/// Pages of 100 issues fetched at most (500 open issues per cycle).
pub const MAX_PAGES: u32 = 5;
const PER_PAGE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIssue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl OpenIssue {
    /// The id used in the cycle context, for example `#12`.
    pub fn id(&self) -> String {
        format!("#{}", self.number)
    }
}

/// The cached list for one repository and label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCache {
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub label: String,
    /// When the list was fetched.
    #[serde(default)]
    pub fetched_unix_ms: Option<u64>,
    #[serde(default)]
    pub issues: Vec<OpenIssue>,
}

impl IssueCache {
    /// Reads a cache file. A missing or unreadable file is an empty cache: the cache is only
    /// ever a fallback.
    pub fn load(path: &Path) -> IssueCache {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text =
            serde_json::to_string_pretty(self).map_err(|e| CycleError::Other(e.to_string()))?;
        write_atomic(path, text.as_bytes())
    }
}

/// The result of asking for the open issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueList {
    pub issues: Vec<OpenIssue>,
    /// When the list was fetched (now for a fresh list, the cache date otherwise); `None`
    /// when there is neither a fetch nor a cache.
    pub as_of_unix_ms: Option<u64>,
    /// True when the network request failed and the cache was returned.
    pub from_cache: bool,
    /// Why the request failed.
    pub error: Option<String>,
}

impl IssueList {
    pub fn as_of_date(&self) -> Option<String> {
        self.as_of_unix_ms.map(format_utc_date)
    }
}

/// Fetches the open issues labelled `label` of `repo` (`owner/name`) from a GitHub-style
/// REST API at `api_base`, skipping pull requests, over at most [`MAX_PAGES`] pages. On
/// success the list is stored in the cache file at `cache_path`. On any failure (network,
/// status, malformed body) the cached list for the same repository and label is returned
/// with its date and the reason.
pub fn fetch_open_issues(
    api_base: &str,
    repo: &str,
    label: &str,
    token: Option<&str>,
    cache_path: &Path,
) -> IssueList {
    match fetch_all(api_base, repo, label, token) {
        Ok(issues) => {
            let now = now_unix_ms();
            let cache = IssueCache {
                repo: repo.to_string(),
                label: label.to_string(),
                fetched_unix_ms: Some(now),
                issues: issues.clone(),
            };
            let error = cache
                .save(cache_path)
                .err()
                .map(|e| format!("cannot write the issue cache: {e}"));
            IssueList {
                issues,
                as_of_unix_ms: Some(now),
                from_cache: false,
                error,
            }
        }
        Err(message) => {
            let cache = IssueCache::load(cache_path);
            if cache.repo == repo && cache.label == label {
                IssueList {
                    issues: cache.issues,
                    as_of_unix_ms: cache.fetched_unix_ms,
                    from_cache: true,
                    error: Some(message),
                }
            } else {
                IssueList {
                    issues: Vec::new(),
                    as_of_unix_ms: None,
                    from_cache: true,
                    error: Some(message),
                }
            }
        }
    }
}

fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((owner, name)) if ok(owner) && ok(name))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .into()
}

fn fetch_all(
    api_base: &str,
    repo: &str,
    label: &str,
    token: Option<&str>,
) -> std::result::Result<Vec<OpenIssue>, String> {
    if !valid_repo(repo) {
        return Err(format!(
            "invalid repository name {repo:?}, expected owner/name"
        ));
    }
    if label.trim().is_empty() {
        return Err("empty label".to_string());
    }
    let base = api_base.trim_end_matches('/');
    let agent = agent();
    let mut issues = Vec::new();
    for page in 1..=MAX_PAGES {
        let url = format!(
            "{base}/repos/{repo}/issues?state=open&labels={}&per_page={PER_PAGE}&page={page}",
            percent_encode(label)
        );
        let mut request = agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "blah-blah-bot")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let mut response = request.call().map_err(|e| format!("request failed: {e}"))?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("cannot read the response: {e}"))?;
        let value: Value =
            serde_json::from_str(&body).map_err(|e| format!("malformed response: {e}"))?;
        let list = value
            .as_array()
            .ok_or_else(|| "malformed response: expected a list".to_string())?;
        for entry in list {
            // The issues endpoint also lists pull requests.
            if entry.get("pull_request").is_some() {
                continue;
            }
            if let Some(issue) = parse_issue(entry) {
                issues.push(issue);
            }
        }
        if list.len() < PER_PAGE {
            break;
        }
    }
    Ok(issues)
}

fn parse_issue(entry: &Value) -> Option<OpenIssue> {
    let assignee = entry
        .pointer("/assignee/login")
        .or_else(|| entry.pointer("/assignees/0/login"))
        .and_then(Value::as_str)
        .map(String::from);
    Some(OpenIssue {
        number: entry.get("number")?.as_u64()?,
        title: entry.get("title")?.as_str()?.to_string(),
        assignee,
        url: entry
            .get("html_url")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

#[cfg(test)]
pub(crate) mod mock {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    /// Path and lower-cased headers of one request.
    pub type Request = (String, Vec<(String, String)>);

    /// A local HTTP server that answers each request with the next scripted response.
    pub struct MockServer {
        pub base: String,
        pub requests: Arc<Mutex<Vec<Request>>>,
        handle: Option<JoinHandle<()>>,
    }

    impl MockServer {
        /// `responses`: (status, body) per expected request, in order.
        pub fn start(responses: Vec<(u16, String)>) -> MockServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = Arc::clone(&requests);
            let handle = std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    serve(stream, status, &body, &log);
                }
            });
            MockServer {
                base,
                requests,
                handle: Some(handle),
            }
        }

        pub fn finish(mut self) -> Vec<Request> {
            if let Some(h) = self.handle.take() {
                h.join().unwrap();
            }
            self.requests.lock().unwrap().clone()
        }
    }

    fn serve(mut stream: TcpStream, status: u16, body: &str, log: &Mutex<Vec<Request>>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let mut lines = text.split("\r\n");
        let path = lines
            .next()
            .unwrap_or("")
            .split(' ')
            .nth(1)
            .unwrap_or("")
            .to_string();
        let headers = lines
            .take_while(|l| !l.is_empty())
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        log.lock().unwrap().push((path, headers));
        let response = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockServer;
    use super::*;
    use serde_json::json;

    fn issue_json(n: u64, title: &str, assignee: Option<&str>, pr: bool) -> Value {
        let mut v = json!({
            "number": n,
            "title": title,
            "html_url": format!("https://example.invalid/issues/{n}"),
            "assignee": assignee.map(|a| json!({"login": a})),
        });
        if pr {
            v["pull_request"] = json!({"url": "x"});
        }
        v
    }

    fn page(items: Vec<Value>) -> (u16, String) {
        (200, Value::Array(items).to_string())
    }

    #[test]
    fn fetches_filters_pull_requests_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("issues.json");
        let server = MockServer::start(vec![page(vec![
            issue_json(12, "Order the battery", Some("alex-example"), false),
            issue_json(13, "A pull request", None, true),
            issue_json(14, "Wiring", None, false),
        ])]);
        let list = fetch_open_issues(
            &server.base,
            "owner/rover",
            "cycle:NOMAD",
            Some("secret-token"),
            &cache_path,
        );
        let requests = server.finish();
        assert!(!list.from_cache && list.error.is_none(), "{list:?}");
        assert_eq!(list.issues.len(), 2);
        assert_eq!(list.issues[0].id(), "#12");
        assert_eq!(list.issues[0].assignee.as_deref(), Some("alex-example"));
        assert_eq!(list.issues[1].assignee, None);
        assert!(list.as_of_date().unwrap().starts_with("20"));

        // Request shape: path, label encoding, token only in the header.
        assert_eq!(requests.len(), 1);
        let (path, headers) = &requests[0];
        assert_eq!(
            path,
            "/repos/owner/rover/issues?state=open&labels=cycle%3ANOMAD&per_page=100&page=1"
        );
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("authorization"), Some("Bearer secret-token"));
        assert_eq!(header("user-agent"), Some("blah-blah-bot"));
        assert!(!path.contains("secret-token"));

        // The cache holds the fresh list, and not the token.
        let cache_text = fs::read_to_string(&cache_path).unwrap();
        assert!(!cache_text.contains("secret-token"));
        let cache = IssueCache::load(&cache_path);
        assert_eq!(cache.issues, list.issues);
        assert_eq!(cache.label, "cycle:NOMAD");
    }

    #[test]
    fn no_token_means_no_authorization_header() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start(vec![page(vec![])]);
        let list = fetch_open_issues(
            &server.base,
            "owner/rover",
            "cycle:X",
            None,
            &dir.path().join("c.json"),
        );
        let requests = server.finish();
        assert!(list.issues.is_empty() && !list.from_cache);
        assert!(requests[0].1.iter().all(|(k, _)| k != "authorization"));
    }

    #[test]
    fn follows_pages_up_to_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let full = |start: u64| {
            page(
                (start..start + 100)
                    .map(|n| issue_json(n, "t", None, false))
                    .collect(),
            )
        };
        let server = MockServer::start(vec![
            full(1),
            full(101),
            page(vec![issue_json(500, "last", None, false)]),
        ]);
        let list = fetch_open_issues(
            &server.base,
            "o/r",
            "cycle:A",
            None,
            &dir.path().join("c.json"),
        );
        let requests = server.finish();
        assert_eq!(list.issues.len(), 201);
        assert_eq!(requests.len(), 3);
        assert!(requests[2].0.ends_with("page=3"));

        // Never more than MAX_PAGES requests even when every page is full.
        let responses = (0..MAX_PAGES as u64).map(|p| full(p * 100 + 1)).collect();
        let server = MockServer::start(responses);
        let list = fetch_open_issues(
            &server.base,
            "o/r",
            "cycle:A",
            None,
            &dir.path().join("d.json"),
        );
        assert_eq!(server.finish().len(), MAX_PAGES as usize);
        assert_eq!(list.issues.len(), 100 * MAX_PAGES as usize);
    }

    #[test]
    fn failures_return_the_cache_with_its_date() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("issues.json");
        let cached = IssueCache {
            repo: "owner/rover".into(),
            label: "cycle:NOMAD".into(),
            fetched_unix_ms: Some(1_794_000_000_000),
            issues: vec![OpenIssue {
                number: 7,
                title: "Cached".into(),
                assignee: None,
                url: None,
            }],
        };
        cached.save(&cache_path).unwrap();

        // Server error.
        let server = MockServer::start(vec![(500, "{}".into())]);
        let list = fetch_open_issues(
            &server.base,
            "owner/rover",
            "cycle:NOMAD",
            None,
            &cache_path,
        );
        server.finish();
        assert!(list.from_cache);
        assert_eq!(list.issues, cached.issues);
        assert_eq!(list.as_of_unix_ms, Some(1_794_000_000_000));
        assert!(list.error.as_deref().unwrap().contains("request failed"));

        // Malformed body.
        let server = MockServer::start(vec![(200, "not json".into())]);
        let list = fetch_open_issues(
            &server.base,
            "owner/rover",
            "cycle:NOMAD",
            None,
            &cache_path,
        );
        server.finish();
        assert!(list.from_cache && list.error.as_deref().unwrap().contains("malformed"));

        // Not a list (an API error object with status 200).
        let server = MockServer::start(vec![(200, "{\"message\":\"x\"}".into())]);
        let list = fetch_open_issues(
            &server.base,
            "owner/rover",
            "cycle:NOMAD",
            None,
            &cache_path,
        );
        server.finish();
        assert!(list.from_cache && list.issues == cached.issues);

        // Nothing is listening: connect the port, then close it.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let list = fetch_open_issues(&closed, "owner/rover", "cycle:NOMAD", None, &cache_path);
        assert!(list.from_cache && list.issues == cached.issues && list.error.is_some());

        // The cache survives all failures untouched.
        assert_eq!(IssueCache::load(&cache_path), cached);
    }

    #[test]
    fn cache_for_another_label_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("issues.json");
        IssueCache {
            repo: "owner/rover".into(),
            label: "cycle:OTHER".into(),
            fetched_unix_ms: Some(1),
            issues: vec![OpenIssue {
                number: 1,
                title: "x".into(),
                assignee: None,
                url: None,
            }],
        }
        .save(&cache_path)
        .unwrap();
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let list = fetch_open_issues(&closed, "owner/rover", "cycle:NOMAD", None, &cache_path);
        assert!(list.issues.is_empty() && list.as_of_unix_ms.is_none() && list.error.is_some());
    }

    #[test]
    fn invalid_repo_names_never_reach_the_network() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["rover", "a/b/c", "../x", "owner/na me", "/x", "owner/"] {
            let list = fetch_open_issues(
                "http://127.0.0.1:1",
                bad,
                "cycle:A",
                None,
                &dir.path().join("c.json"),
            );
            assert!(
                list.error
                    .as_deref()
                    .unwrap()
                    .contains("invalid repository"),
                "{bad}"
            );
        }
    }
}
