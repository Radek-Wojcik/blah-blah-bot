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
    /// Numbers of the cycle's issues that are closed (among the most recently updated ones).
    /// Old cache files have none.
    #[serde(default)]
    pub closed: Vec<u64>,
    /// The issues created from meeting items, found by their marker, open or closed.
    #[serde(default)]
    pub links: Vec<IssueLink>,
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

/// An issue created from an item of a meeting, found again through its hidden marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLink {
    pub number: u64,
    /// The meeting id (`20261113-140501`).
    pub meeting: String,
    /// The id of the item in that meeting's draft.
    pub item: String,
    #[serde(default)]
    pub open: bool,
}

// ----- markers --------------------------------------------------------------------------------

pub(crate) const MARKER_START: &str = "<!-- bbb:v1 ";
pub(crate) const MARKER_END: &str = " -->";

pub(crate) fn marker_value(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The hidden comment of an issue body that ties the issue to the item it was made from:
/// `<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 t=745000 -->`. It is how a retry
/// finds an issue that was created but not acknowledged, and how the next briefing knows an
/// item has an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMarker {
    pub cycle: String,
    pub meeting: String,
    pub item: String,
    /// Where in the recording the item was said, in milliseconds.
    pub t_ms: Option<u64>,
}

impl ItemMarker {
    /// The comment, or `None` when a value could not be written safely.
    pub fn to_comment(&self) -> Option<String> {
        if !(marker_value(&self.cycle) && marker_value(&self.meeting) && marker_value(&self.item)) {
            return None;
        }
        let mut out = format!(
            "{MARKER_START}cycle={} meeting={} item={}",
            self.cycle, self.meeting, self.item
        );
        if let Some(t) = self.t_ms {
            out.push_str(&format!(" t={t}"));
        }
        out.push_str(MARKER_END);
        Some(out)
    }

    /// The first item marker in `text`.
    pub fn parse(text: &str) -> Option<ItemMarker> {
        markers(text).find_map(|tokens| {
            let get = |k: &str| {
                tokens
                    .iter()
                    .find_map(|t| t.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
            };
            let (cycle, meeting, item) = (get("cycle")?, get("meeting")?, get("item")?);
            if ![cycle, meeting, item].iter().all(|v| marker_value(v)) {
                return None;
            }
            Some(ItemMarker {
                cycle: cycle.to_string(),
                meeting: meeting.to_string(),
                item: item.to_string(),
                t_ms: get("t").and_then(|t| t.parse().ok()),
            })
        })
    }
}

/// The hidden comment of a status comment: `<!-- bbb:v1 status meeting=.. proposal=s2 -->`.
/// A retry looks for it before commenting again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMarker {
    pub meeting: String,
    pub proposal: String,
}

impl StatusMarker {
    pub fn to_comment(&self) -> Option<String> {
        if !(marker_value(&self.meeting) && marker_value(&self.proposal)) {
            return None;
        }
        Some(format!(
            "{MARKER_START}status meeting={} proposal={}{MARKER_END}",
            self.meeting, self.proposal
        ))
    }

    pub fn parse(text: &str) -> Option<StatusMarker> {
        markers(text).find_map(|tokens| {
            if tokens.first() != Some(&"status") {
                return None;
            }
            let get = |k: &str| {
                tokens
                    .iter()
                    .find_map(|t| t.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
            };
            let (meeting, proposal) = (get("meeting")?, get("proposal")?);
            (marker_value(meeting) && marker_value(proposal)).then(|| StatusMarker {
                meeting: meeting.to_string(),
                proposal: proposal.to_string(),
            })
        })
    }
}

/// The tokens of every `<!-- bbb:v1 ... -->` comment in `text`.
pub(crate) fn markers(text: &str) -> impl Iterator<Item = Vec<&str>> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let start = rest.find(MARKER_START)?;
        let after = &rest[start + MARKER_START.len()..];
        match after.find(MARKER_END) {
            Some(end) => {
                rest = &after[end + MARKER_END.len()..];
                Some(after[..end].split_whitespace().collect())
            }
            None => {
                rest = "";
                None
            }
        }
    })
}

// ----- records ----------------------------------------------------------------------------------

/// An issue as the list endpoint returns it, reduced to what the cycle needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRecord {
    pub number: u64,
    pub title: String,
    pub assignee: Option<String>,
    pub url: Option<String>,
    pub open: bool,
    /// The marker of the body, when the issue was made from a meeting item.
    pub marker: Option<ItemMarker>,
}

/// Reads one entry of the list of issues; pull requests and malformed entries give `None`.
pub fn parse_issue_record(entry: &Value) -> Option<IssueRecord> {
    if entry.get("pull_request").is_some() {
        return None;
    }
    let issue = parse_issue(entry)?;
    Some(IssueRecord {
        number: issue.number,
        title: issue.title,
        assignee: issue.assignee,
        url: issue.url,
        open: entry.get("state").and_then(Value::as_str) != Some("closed"),
        marker: entry
            .get("body")
            .and_then(Value::as_str)
            .and_then(ItemMarker::parse),
    })
}

impl IssueCache {
    /// The cache for a fetch of all states: open issues, closed numbers and the links of the
    /// issues that carry a marker of `cycle_id`.
    pub fn from_records(
        repo: &str,
        label: &str,
        cycle_id: &str,
        fetched_unix_ms: u64,
        records: &[IssueRecord],
    ) -> IssueCache {
        let mut cache = IssueCache {
            repo: repo.to_string(),
            label: label.to_string(),
            fetched_unix_ms: Some(fetched_unix_ms),
            ..IssueCache::default()
        };
        for r in records {
            if r.open {
                cache.issues.push(OpenIssue {
                    number: r.number,
                    title: r.title.clone(),
                    assignee: r.assignee.clone(),
                    url: r.url.clone(),
                });
            } else {
                cache.closed.push(r.number);
            }
            if let Some(m) = r.marker.as_ref().filter(|m| m.cycle == cycle_id) {
                cache.links.push(IssueLink {
                    number: r.number,
                    meeting: m.meeting.clone(),
                    item: m.item.clone(),
                    open: r.open,
                });
            }
        }
        cache
    }
}

/// Stores the outcome of a fetch of all issues of a cycle (`fetched`, from
/// `publishers::github::Github::list_cycle_issues`) in the cache at `cache_path`. A failure
/// returns the cached state for the same repository and label with its date and the reason,
/// and leaves the cache untouched.
pub fn apply_fetch(
    fetched: std::result::Result<Vec<IssueRecord>, String>,
    repo: &str,
    label: &str,
    cycle_id: &str,
    cache_path: &Path,
) -> IssueList {
    match fetched {
        Ok(records) => {
            let now = now_unix_ms();
            let cache = IssueCache::from_records(repo, label, cycle_id, now, &records);
            let error = cache
                .save(cache_path)
                .err()
                .map(|e| format!("cannot write the issue cache: {e}"));
            IssueList {
                issues: cache.issues,
                as_of_unix_ms: Some(now),
                from_cache: false,
                error,
                closed: cache.closed,
                links: cache.links,
            }
        }
        Err(message) => cached_list(cache_path, repo, label, message),
    }
}

/// The cached list for `repo` and `label`, marked as such, with the reason it is used.
fn cached_list(cache_path: &Path, repo: &str, label: &str, message: String) -> IssueList {
    let cache = IssueCache::load(cache_path);
    if cache.repo == repo && cache.label == label {
        IssueList {
            issues: cache.issues,
            as_of_unix_ms: cache.fetched_unix_ms,
            from_cache: true,
            error: Some(message),
            closed: cache.closed,
            links: cache.links,
        }
    } else {
        IssueList {
            issues: Vec::new(),
            as_of_unix_ms: None,
            from_cache: true,
            error: Some(message),
            closed: Vec::new(),
            links: Vec::new(),
        }
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
    /// Numbers of closed issues (only a fetch of all states knows them).
    pub closed: Vec<u64>,
    /// Issues created from meeting items.
    pub links: Vec<IssueLink>,
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
                ..IssueCache::default()
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
                closed: Vec::new(),
                links: Vec::new(),
            }
        }
        Err(message) => cached_list(cache_path, repo, label, message),
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
                // Without roots ureq's native-tls connector panics on the first https request.
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
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

    #[test]
    fn agent_uses_the_platform_verifier() {
        let agent = agent();
        let tls = agent.config().tls_config();
        assert_eq!(tls.provider(), ureq::tls::TlsProvider::NativeTls);
        assert!(matches!(
            tls.root_certs(),
            ureq::tls::RootCerts::PlatformVerifier
        ));
    }

    /// A real TLS handshake with this agent (an unauthenticated HEAD of a public page);
    /// reaches the Internet, so it is not run in CI (docs/TESTING.md, section 14).
    #[test]
    #[ignore = "reaches the Internet: cargo test -p cycle-store https_handshake -- --ignored"]
    fn https_handshake_with_the_issues_agent() {
        let response = agent().head("https://github.com").call().expect("https");
        assert!(response.status().as_u16() < 500);
    }

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
        let dir = test_tmp::TestDir::new("issues");
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
        let dir = test_tmp::TestDir::new("issues");
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
        let dir = test_tmp::TestDir::new("issues");
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
        let dir = test_tmp::TestDir::new("issues");
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
            ..IssueCache::default()
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
        let dir = test_tmp::TestDir::new("issues");
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
            ..IssueCache::default()
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

    fn marker() -> ItemMarker {
        ItemMarker {
            cycle: "NOMAD".into(),
            meeting: "20261113-140501".into(),
            item: "i3".into(),
            t_ms: Some(745_000),
        }
    }

    #[test]
    fn the_item_marker_round_trips_inside_a_body() {
        let comment = marker().to_comment().unwrap();
        assert_eq!(
            comment,
            "<!-- bbb:v1 cycle=NOMAD meeting=20261113-140501 item=i3 t=745000 -->"
        );
        let body = format!("Order the battery\n\nFrom meeting.\n\n{comment}\n");
        assert_eq!(ItemMarker::parse(&body), Some(marker()));
        // Without a time.
        let mut m = marker();
        m.t_ms = None;
        assert_eq!(ItemMarker::parse(&m.to_comment().unwrap()), Some(m));
        // Not a marker: plain text, another version, a status marker, a missing key.
        for text in [
            "no marker here",
            "<!-- bbb:v2 cycle=A meeting=B item=c -->",
            "<!-- bbb:v1 status meeting=20261113-140501 proposal=s2 -->",
            "<!-- bbb:v1 cycle=A meeting=B -->",
            "<!-- bbb:v1 cycle=A meeting=B item=c",
            "<!-- other cycle=A meeting=B item=c -->",
            "",
        ] {
            assert_eq!(ItemMarker::parse(text), None, "{text}");
        }
        // Values that could break the comment are refused when writing and reading.
        let mut bad = marker();
        bad.item = "i3 -->".into();
        assert_eq!(bad.to_comment(), None);
        assert_eq!(
            ItemMarker::parse("<!-- bbb:v1 cycle=A meeting=B item=c<d -->"),
            None
        );
    }

    #[test]
    fn the_first_marker_wins_and_a_quoted_marker_still_counts_once() {
        let body =
            "<!-- bbb:v1 cycle=A meeting=M1 item=x -->\n<!-- bbb:v1 cycle=A meeting=M2 item=y -->";
        assert_eq!(ItemMarker::parse(body).unwrap().meeting, "M1");
    }

    #[test]
    fn the_status_marker_round_trips_and_is_not_an_item_marker() {
        let m = StatusMarker {
            meeting: "20261113-140501".into(),
            proposal: "s2".into(),
        };
        let comment = m.to_comment().unwrap();
        assert_eq!(
            comment,
            "<!-- bbb:v1 status meeting=20261113-140501 proposal=s2 -->"
        );
        assert_eq!(StatusMarker::parse(&format!("Done.\n{comment}")), Some(m));
        assert_eq!(StatusMarker::parse(&marker().to_comment().unwrap()), None);
        assert_eq!(ItemMarker::parse(&comment), None);
    }

    fn entry(n: u64, state: &str, body: &str, pr: bool) -> Value {
        let mut v = json!({
            "number": n,
            "title": format!("Issue {n}"),
            "state": state,
            "body": body,
            "html_url": format!("https://example.invalid/issues/{n}"),
            "assignee": {"login": "alex-example"},
        });
        if pr {
            v["pull_request"] = json!({"url": "x"});
        }
        v
    }

    #[test]
    fn records_split_into_open_closed_and_links() {
        let marked = |item: &str| {
            ItemMarker {
                item: item.into(),
                ..marker()
            }
            .to_comment()
            .unwrap()
        };
        let other_cycle = ItemMarker {
            cycle: "ROVER".into(),
            ..marker()
        }
        .to_comment()
        .unwrap();
        let entries = [
            entry(1, "open", &format!("text\n{}", marked("i1")), false),
            entry(2, "closed", &marked("i2"), false),
            entry(3, "open", "made by hand", false),
            entry(4, "closed", "", false),
            entry(5, "open", &marked("i5"), true),
            entry(6, "open", &other_cycle, false),
        ];
        let records: Vec<IssueRecord> = entries.iter().filter_map(parse_issue_record).collect();
        assert_eq!(records.len(), 5, "pull requests are skipped");
        let cache = IssueCache::from_records("o/r", "cycle:NOMAD", "NOMAD", 7, &records);
        assert_eq!(
            cache.issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            [1, 3, 6]
        );
        assert_eq!(cache.closed, [2, 4]);
        assert_eq!(
            cache.links,
            [
                IssueLink {
                    number: 1,
                    meeting: "20261113-140501".into(),
                    item: "i1".into(),
                    open: true
                },
                IssueLink {
                    number: 2,
                    meeting: "20261113-140501".into(),
                    item: "i2".into(),
                    open: false
                },
            ],
            "only issues with a marker of this cycle are linked"
        );
        assert_eq!(cache.fetched_unix_ms, Some(7));
    }

    #[test]
    fn an_old_cache_file_still_loads() {
        let dir = test_tmp::TestDir::new("issues");
        let path = dir.path().join("issues.json");
        fs::write(
            &path,
            r#"{"repo":"o/r","label":"cycle:A","fetched_unix_ms":5,"issues":[{"number":1,"title":"t"}]}"#,
        )
        .unwrap();
        let cache = IssueCache::load(&path);
        assert_eq!(cache.issues.len(), 1);
        assert!(cache.closed.is_empty() && cache.links.is_empty());
        // And a new one round-trips.
        let full = IssueCache::from_records(
            "o/r",
            "cycle:A",
            "A",
            9,
            &[IssueRecord {
                number: 2,
                title: "x".into(),
                assignee: None,
                url: None,
                open: false,
                marker: Some(ItemMarker {
                    cycle: "A".into(),
                    meeting: "M".into(),
                    item: "i".into(),
                    t_ms: None,
                }),
            }],
        );
        full.save(&path).unwrap();
        assert_eq!(IssueCache::load(&path), full);
    }

    #[test]
    fn apply_fetch_stores_a_success_and_falls_back_to_the_cache_on_failure() {
        let dir = test_tmp::TestDir::new("issues");
        let path = dir.path().join("issues.json");
        let record = IssueRecord {
            number: 3,
            title: "Closed one".into(),
            assignee: None,
            url: None,
            open: false,
            marker: None,
        };
        let list = apply_fetch(Ok(vec![record]), "o/r", "cycle:A", "A", &path);
        assert!(!list.from_cache && list.error.is_none());
        assert_eq!(list.closed, [3]);
        assert!(list.as_of_unix_ms.is_some());

        let failed = apply_fetch(Err("offline".into()), "o/r", "cycle:A", "A", &path);
        assert!(failed.from_cache);
        assert_eq!(failed.error.as_deref(), Some("offline"));
        assert_eq!(failed.closed, [3], "the closed issues come from the cache");
        assert_eq!(failed.as_of_unix_ms, list.as_of_unix_ms);

        let other = apply_fetch(Err("offline".into()), "o/r", "cycle:B", "B", &path);
        assert!(other.closed.is_empty() && other.as_of_unix_ms.is_none());
    }

    #[test]
    fn invalid_repo_names_never_reach_the_network() {
        let dir = test_tmp::TestDir::new("issues");
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
