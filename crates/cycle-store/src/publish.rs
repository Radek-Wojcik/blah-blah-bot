//! Publishing a meeting to the Git repository of its cycle.
//!
//! The commit is built **off the index**, with Git plumbing and a temporary index file, from
//! the tip of the remote branch: the working tree and the real index of the user's clone are
//! never read for the commit, never staged and never stashed, so a half-finished edit of a
//! cycle file cannot end up in a report commit and cannot be lost by one. Only *new* files go
//! in (the meeting folder), plus `topics.yaml` merged per topic with the remote's version.
//!
//! ```text
//! fetch -> tip of the remote branch -> read-tree into a temporary index
//!       -> hash-object + update-index for the meeting files (and merged topics.yaml)
//!       -> write-tree -> commit-tree -p tip -> push <commit>:refs/heads/<branch>
//! ```
//!
//! A push that the remote refuses because somebody else pushed first is not an error: the
//! commit is **rebuilt on the new tip** (for a commit of new files this is what
//! `pull --rebase` and a retry would produce, and `topics.yaml` is merged again) and pushed
//! again, up to [`MAX_ATTEMPTS`] times. The folder name is chosen against the tip each time,
//! so two people publishing on the same day end up in `2026-11-13` and `2026-11-13-2`.
//!
//! Publishing is idempotent: when the tip already holds the folder with exactly these files
//! (the answer of an earlier push was lost) nothing is committed and the existing commit is
//! reported.
//!
//! After the push the local branch is fast-forwarded to the commit when possible, with
//! unrelated uncommitted files left alone; otherwise the result says the clone is behind.
//!
//! Credentials: the Git credential helper of the machine, as for every other Git use of the
//! application. A GitHub token, when given, is handed to Git in the **environment** of the
//! command (never on its command line) and only for remotes under the token's own base
//! address. Git's output is scrubbed of the token before it is returned.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CycleError;
use crate::git::{self, Output};
use crate::repo::{Repo, FETCH_TIMEOUT_CONFIG};
use crate::schema::is_identifier;
use crate::topics::{self, Topic, TopicsMerge};

/// How many times a push is tried; each try builds on the newest tip.
pub const MAX_ATTEMPTS: u32 = 3;

// ----- request and result ------------------------------------------------------------------

/// A file of the meeting folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishFile {
    /// Path below the meeting folder with `/` separators: `report.md`, `img/shot-1.png`.
    pub rel: String,
    /// Where the bytes are now (the snapshot made when publishing was started).
    pub path: PathBuf,
}

/// A GitHub token for the remotes below `base_url`.
#[derive(Clone)]
pub struct GitAuth {
    /// `https://github.com/` (or the address of an Enterprise server).
    pub base_url: String,
    pub token: String,
}

// A token must not reach a log through `{:?}`.
impl std::fmt::Debug for GitAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitAuth")
            .field("base_url", &self.base_url)
            .field("token", &"***")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct PublishRequest<'a> {
    pub cycle_id: &'a str,
    /// The folder of the meetings in the repository (`targets.github.path`, `meetings` by
    /// default): the meeting goes to `<base_path>/<cycle>/<folder>/`. Validated with
    /// [`crate::cycle::repo_path_problem`] before anything is read or written.
    pub base_path: &'a str,
    /// The day of the meeting, `YYYY-MM-DD`: the name of the folder unless taken.
    pub date: &'a str,
    pub files: &'a [PublishFile],
    /// The full topic list of the cycle as it is here now, including the topics the meeting
    /// adds. `None` leaves `topics.yaml` alone.
    pub topics: Option<&'a [Topic]>,
    /// The commit message; `{folder}` is replaced by the folder that was chosen.
    pub message: &'a str,
    pub auth: Option<&'a GitAuth>,
}

/// What happened to the local branch after the commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSync {
    /// The branch points at the commit (or already did) and its files are checked out.
    Updated,
    /// The branch could not be moved; the commit itself is safe (pushed, or recorded in
    /// the repository). The reason is shown to the user; a pull brings the clone up to date.
    Behind(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The folder below `<base_path>/<ID>/`: `2026-11-13`, `2026-11-13-2`, ...
    pub folder: String,
    /// The commit that holds the meeting.
    pub commit: String,
    pub branch: String,
    /// The remote the commit went to; `None` for a repository without one.
    pub remote: Option<String>,
    /// False for a repository without a remote: the commit is local.
    pub pushed: bool,
    /// The tip already had these files: nothing was committed.
    pub already_published: bool,
    /// Pushes tried (1 when nobody else pushed meanwhile).
    pub attempts: u32,
    /// Repository paths of the meeting files in the commit.
    pub paths: Vec<String>,
    /// The `topics.yaml` merge, when topics were given: what changed and what was overridden.
    pub topics: Option<TopicsMerge>,
    pub local: LocalSync,
}

/// How an attempt failed, for the outbox: transient failures are tried again later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishErrorKind {
    /// Offline, a remote that is not there right now, a push that kept being refused because
    /// others kept pushing.
    Transient,
    /// Needs the user: credentials, a protected branch, a broken repository, bad input.
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PublishError {
    pub kind: PublishErrorKind,
    /// Safe to store and to show: scrubbed of the token.
    pub message: String,
}

impl PublishError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            kind: PublishErrorKind::Permanent,
            message: message.into(),
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: PublishErrorKind::Transient,
            message: message.into(),
        }
    }

    pub fn is_transient(&self) -> bool {
        self.kind == PublishErrorKind::Transient
    }
}

// ----- scrubbing and classification ----------------------------------------------------------

const MASK: &str = "***";

/// The standard base64 of `bytes` (for the `Authorization` header of Git's HTTP transport).
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Masks `secrets` (and the base64 form of `x-access-token:<secret>` that goes into a header)
/// in `text`, the user name and password of any URL, and whatever follows `AUTHORIZATION:`.
/// Used on everything Git prints before it is stored or shown.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, MASK);
        let header = base64(format!("x-access-token:{secret}").as_bytes());
        out = out.replace(&header, MASK);
    }
    // `https://user:password@host/` -> `https://***@host/`
    let mut cleaned = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        cleaned.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => {
                cleaned.push_str(MASK);
                cleaned.push_str(&tail[at..end]);
            }
            None => cleaned.push_str(&tail[..end]),
        }
        rest = &tail[end..];
    }
    cleaned.push_str(rest);
    // `AUTHORIZATION: basic <value>` in a trace.
    let lower = cleaned.to_ascii_lowercase();
    let mut result = String::with_capacity(cleaned.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find("authorization:") {
        let start = at + found + "authorization:".len();
        result.push_str(&cleaned[at..start]);
        let end = cleaned[start..]
            .find(['\n', '\r'])
            .map_or(cleaned.len(), |i| start + i);
        result.push(' ');
        result.push_str(MASK);
        at = end;
    }
    result.push_str(&cleaned[at..]);
    result
}

const AUTH_MARKERS: [&str; 13] = [
    "authentication failed",
    "permission denied",
    "could not read username",
    "could not read password",
    "invalid username or password",
    "returned error: 401",
    "returned error: 403",
    "access denied",
    "repository not found",
    "protected branch",
    "gh006",
    "hook declined",
    "write access to repository not granted",
];

const OFFLINE_MARKERS: [&str; 16] = [
    "could not resolve host",
    "failed to connect",
    "connection refused",
    "connection timed out",
    "timed out",
    "operation too slow",
    "connection reset",
    "early eof",
    "hung up unexpectedly",
    "could not read from remote repository",
    "does not appear to be a git repository",
    "network is unreachable",
    "temporary failure",
    "returned error: 5",
    "rpc failed",
    "ssl",
];

/// Whether the text of a failed fetch or push says the problem is the network (try again
/// later) or something the user has to fix. Credentials win over the network, because Git
/// reports a refused login as "could not read from remote repository" as well.
pub fn classify_git_failure(text: &str) -> PublishErrorKind {
    let lower = text.to_ascii_lowercase();
    if AUTH_MARKERS.iter().any(|m| lower.contains(m)) {
        PublishErrorKind::Permanent
    } else if OFFLINE_MARKERS.iter().any(|m| lower.contains(m)) {
        PublishErrorKind::Transient
    } else {
        PublishErrorKind::Permanent
    }
}

fn rejected_by_remote_race(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("non-fast-forward")
        || lower.contains("fetch first")
        || lower.contains("stale info")
        || (lower.contains("[rejected]") && lower.contains("tip of your current branch is behind"))
}

// ----- git plumbing --------------------------------------------------------------------------

/// A Git context for one repository with the environment every command of one publish
/// attempt shares.
struct Git<'a> {
    root: &'a Path,
    env: Vec<(String, String)>,
    secrets: Vec<String>,
}

impl Git<'_> {
    fn scrubbed(&self, text: &str) -> String {
        let refs: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        scrub(text, &refs)
    }

    fn fail(&self, command: &str, out: &Output) -> PublishError {
        let message = self.scrubbed(&out.message());
        PublishError {
            kind: classify_git_failure(&message),
            message: format!("git {command} failed: {message}"),
        }
    }

    fn run(&self, args: &[&str]) -> Result<Output, PublishError> {
        git::run_env(Some(self.root), args, &self.env).map_err(|e| match e {
            CycleError::GitUnavailable(e) => {
                PublishError::permanent(format!("git is not available: {e}"))
            }
            other => PublishError::permanent(self.scrubbed(&other.to_string())),
        })
    }

    /// Runs git; a failure is classified from its text.
    fn ok(&self, args: &[&str]) -> Result<String, PublishError> {
        let out = self.run(args)?;
        if out.ok {
            Ok(out.stdout.trim().to_string())
        } else {
            Err(self.fail(args.first().copied().unwrap_or("command"), &out))
        }
    }

    /// stdout when the command succeeds, `None` when it fails (a missing object or path).
    fn try_ok(&self, args: &[&str]) -> Result<Option<String>, PublishError> {
        let out = self.run(args)?;
        Ok(out.ok.then(|| out.stdout.trim().to_string()))
    }
}

/// A temporary index file next to the repository's own, removed when dropped.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new(git_dir: &Path) -> TempIndex {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        TempIndex(git_dir.join(format!("bbb-publish-index-{}-{n}", std::process::id())))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let mut lock = self.0.as_os_str().to_owned();
        lock.push(".lock");
        let _ = fs::remove_file(PathBuf::from(lock));
    }
}

/// Where the commit goes: the current branch and its remote.
struct Target {
    branch: String,
    /// `None` for a repository without a remote.
    remote: Option<String>,
    /// The branch on the remote (the upstream's, else the same name).
    remote_branch: String,
}

fn target_of(g: &Git) -> Result<Target, PublishError> {
    let branch = g
        .try_ok(&["symbolic-ref", "--short", "-q", "HEAD"])?
        .filter(|b| !b.is_empty())
        .ok_or_else(|| {
            PublishError::permanent(
                "the repository is not on a branch (detached HEAD): check out a branch first",
            )
        })?;
    let remotes = g.ok(&["remote"])?;
    let remotes: Vec<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.is_empty() {
        return Ok(Target {
            remote_branch: branch.clone(),
            branch,
            remote: None,
        });
    }
    let configured = g.try_ok(&["config", "--get", &format!("branch.{branch}.remote")])?;
    let remote = match configured.filter(|r| remotes.contains(&r.as_str()) && r != ".") {
        Some(r) => r,
        None if remotes.contains(&"origin") => "origin".to_string(),
        None => remotes[0].to_string(),
    };
    let merge = g.try_ok(&["config", "--get", &format!("branch.{branch}.merge")])?;
    let remote_branch = merge
        .as_deref()
        .and_then(|m| m.strip_prefix("refs/heads/"))
        .map_or_else(|| branch.clone(), String::from);
    Ok(Target {
        branch,
        remote: Some(remote),
        remote_branch,
    })
}

/// Environment that makes Git use `auth.token` for URLs below `auth.base_url`:
/// `http.<base>.extraheader = AUTHORIZATION: basic base64(x-access-token:<token>)`. It is
/// set through `GIT_CONFIG_COUNT` (Git 2.31 and later), so it is in no file and on no
/// command line. Empty when the token does not apply to `remote_url`.
fn auth_env(remote_url: &str, auth: &GitAuth) -> Vec<(String, String)> {
    let base = auth.base_url.trim();
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    let applies = base.starts_with("https://")
        && remote_url
            .trim()
            .to_ascii_lowercase()
            .starts_with(&base.to_ascii_lowercase());
    if !applies || auth.token.is_empty() {
        return Vec::new();
    }
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            format!("http.{base}.extraheader"),
        ),
        (
            "GIT_CONFIG_VALUE_0".into(),
            format!(
                "AUTHORIZATION: basic {}",
                base64(format!("x-access-token:{}", auth.token).as_bytes())
            ),
        ),
    ]
}

/// A neutral identity for the commit, only when the user has none configured.
fn identity_env(g: &Git) -> Result<Vec<(String, String)>, PublishError> {
    let has = |key: &str| -> Result<bool, PublishError> {
        Ok(g.try_ok(&["config", "--get", key])?
            .is_some_and(|v| !v.is_empty()))
    };
    if has("user.name")? && has("user.email")? {
        return Ok(Vec::new());
    }
    Ok(["AUTHOR", "COMMITTER"]
        .iter()
        .flat_map(|who| {
            [
                (format!("GIT_{who}_NAME"), "Blah Blah Bot".to_string()),
                (
                    format!("GIT_{who}_EMAIL"),
                    "noreply@localhost.invalid".to_string(),
                ),
            ]
        })
        .collect())
}

fn meeting_path(base: &str, cycle_id: &str, folder: &str, rel: &str) -> String {
    format!("{base}/{cycle_id}/{folder}/{rel}")
}

fn topics_path(cycle_id: &str) -> String {
    format!("cycles/{cycle_id}/topics.yaml")
}

fn valid_rel(rel: &str) -> bool {
    !rel.is_empty()
        && rel.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

fn validate(req: &PublishRequest) -> Result<(), PublishError> {
    if !is_identifier(req.cycle_id) {
        return Err(PublishError::permanent("the cycle id is not valid"));
    }
    if let Some(problem) = crate::cycle::repo_path_problem(req.base_path) {
        return Err(PublishError::permanent(format!(
            "the folder for meeting reports ({:?}) is not usable: {problem}",
            req.base_path
        )));
    }
    if !valid_date(req.date) {
        return Err(PublishError::permanent(
            "the meeting date must be YYYY-MM-DD",
        ));
    }
    if req.files.is_empty() {
        return Err(PublishError::permanent("there is nothing to publish"));
    }
    let mut seen = BTreeSet::new();
    for file in req.files {
        if !valid_rel(&file.rel) {
            return Err(PublishError::permanent(format!(
                "the file name {:?} is not allowed in a meeting folder",
                file.rel
            )));
        }
        if !seen.insert(file.rel.as_str()) {
            return Err(PublishError::permanent(format!(
                "the file {} is listed twice",
                file.rel
            )));
        }
        if !file.path.is_file() {
            return Err(PublishError::permanent(format!(
                "the file {} to publish is missing",
                file.rel
            )));
        }
    }
    Ok(())
}

// ----- the commit ------------------------------------------------------------------------------

enum Built {
    /// The tip holds these files already.
    Existing { folder: String, commit: String },
    New {
        folder: String,
        commit: String,
        topics: Option<TopicsMerge>,
    },
}

fn read_topics(text: &str, what: &str) -> Result<Vec<Topic>, PublishError> {
    topics::parse_topics(text).map_err(|issues| {
        PublishError::permanent(format!(
            "topics.yaml {what} cannot be read: {}",
            crate::error::join_issues(&issues)
        ))
    })
}

/// The commit of `files` (and merged topics) on top of `parent`, nothing touched but the
/// object database.
fn build(
    repo: &Repo,
    g: &Git,
    parent: Option<&str>,
    req: &PublishRequest,
) -> Result<Built, PublishError> {
    let git_dir = repo
        .git_dir()
        .map_err(|e| PublishError::permanent(e.to_string()))?;
    let index = TempIndex::new(&git_dir);
    let mut env = g.env.clone();
    env.push((
        "GIT_INDEX_FILE".into(),
        index.0.to_string_lossy().into_owned(),
    ));
    env.extend(identity_env(g)?);
    let gi = Git {
        root: g.root,
        env,
        secrets: g.secrets.clone(),
    };

    match parent {
        Some(p) => gi.ok(&["read-tree", p])?,
        None => gi.ok(&["read-tree", "--empty"])?,
    };

    // The blobs of the files, written to the object database.
    let mut blobs = Vec::with_capacity(req.files.len());
    for file in req.files {
        let repo_path = meeting_path(req.base_path, req.cycle_id, req.date, &file.rel);
        let path = file.path.to_string_lossy().into_owned();
        let sha = gi.ok(&["hash-object", "-w", &format!("--path={repo_path}"), &path])?;
        blobs.push(sha);
    }

    // The folder: the day, then -2, -3 ... against what the tip has.
    let taken: BTreeSet<String> = match parent {
        Some(p) => gi
            .ok(&[
                "ls-tree",
                "--name-only",
                p,
                &format!("{}/{}/", req.base_path, req.cycle_id),
            ])?
            .lines()
            .filter_map(|l| l.rsplit('/').next())
            .map(String::from)
            .collect(),
        None => BTreeSet::new(),
    };
    let mut folder = req.date.to_string();
    let mut n = 1;
    loop {
        if !taken.contains(&folder) {
            break;
        }
        // The tip has this folder: is it ours, from an earlier run?
        let same = match parent {
            Some(p) => {
                let mut all = true;
                for (file, blob) in req.files.iter().zip(&blobs) {
                    let there = gi.try_ok(&[
                        "rev-parse",
                        "--verify",
                        "-q",
                        &format!(
                            "{p}:{}",
                            meeting_path(req.base_path, req.cycle_id, &folder, &file.rel)
                        ),
                    ])?;
                    if there.as_deref() != Some(blob.as_str()) {
                        all = false;
                        break;
                    }
                }
                all
            }
            None => false,
        };
        if same {
            if let Some(p) = parent {
                let dir = format!("{}/{}/{folder}", req.base_path, req.cycle_id);
                let commit = last_commit_touching(&gi, p, &dir)?;
                return Ok(Built::Existing { folder, commit });
            }
        }
        n += 1;
        folder = format!("{}-{n}", req.date);
    }

    let mut entries: Vec<String> = Vec::new();
    for (file, blob) in req.files.iter().zip(&blobs) {
        entries.push(format!(
            "100644,{blob},{}",
            meeting_path(req.base_path, req.cycle_id, &folder, &file.rel)
        ));
    }

    // topics.yaml: the remote's version with our changes applied topic by topic.
    let mut merged = None;
    if let Some(ours) = req.topics {
        let path = topics_path(req.cycle_id);
        let upstream_text = match parent {
            Some(p) => gi.try_ok(&["cat-file", "-p", &format!("{p}:{path}")])?,
            None => None,
        };
        let upstream = match &upstream_text {
            Some(t) => read_topics(t, "on the remote")?,
            None => Vec::new(),
        };
        // Where our work started: the last commit of this clone.
        let base_text = gi.try_ok(&["cat-file", "-p", &format!("HEAD:{path}")])?;
        let base = match &base_text {
            Some(t) => read_topics(t, "in the last commit of this clone")?,
            None => Vec::new(),
        };
        let result = topics::merge(&upstream, &base, ours);
        if upstream_text.is_none() && !result.topics.is_empty() || !result.diff.is_empty() {
            let yaml = topics::topics_to_yaml(&result.topics)
                .map_err(|e| PublishError::permanent(e.to_string()))?;
            let mut tmp = index.0.as_os_str().to_owned();
            tmp.push(".topics");
            let tmp = PathBuf::from(tmp);
            fs::write(&tmp, yaml).map_err(|e| PublishError::permanent(e.to_string()))?;
            let sha = gi.ok(&[
                "hash-object",
                "-w",
                &format!("--path={path}"),
                &tmp.to_string_lossy(),
            ]);
            let _ = fs::remove_file(&tmp);
            entries.push(format!("100644,{},{path}", sha?));
        }
        merged = Some(result);
    }

    let mut args: Vec<String> = vec!["update-index".into(), "--add".into()];
    for entry in &entries {
        args.push("--cacheinfo".into());
        args.push(entry.clone());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    gi.ok(&refs)?;
    let tree = gi.ok(&["write-tree"])?;

    if let Some(p) = parent {
        let parent_tree = gi.ok(&["rev-parse", &format!("{p}^{{tree}}")])?;
        if parent_tree == tree {
            // Nothing differs from the tip, not even topics.yaml.
            return Ok(Built::Existing {
                folder,
                commit: p.to_string(),
            });
        }
    }
    let message = req.message.replace("{folder}", &folder);
    let mut commit_args = vec![
        "-c",
        "commit.gpgsign=false",
        "commit-tree",
        &tree,
        "-m",
        &message,
    ];
    if let Some(p) = parent {
        commit_args.splice(4..4, ["-p", p]);
    }
    let commit = gi.ok(&commit_args)?;
    Ok(Built::New {
        folder,
        commit,
        topics: merged,
    })
}

/// The newest commit reachable from `tip` that touched `path`, or `tip` itself.
fn last_commit_touching(g: &Git, tip: &str, path: &str) -> Result<String, PublishError> {
    let found = g.try_ok(&[
        "log",
        "-1",
        "--format=%H",
        tip,
        "--",
        path.trim_end_matches('/'),
    ])?;
    Ok(found
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| tip.to_string()))
}

// ----- the remote ------------------------------------------------------------------------------

fn fetch_args(remote: &str, branch: &str) -> Vec<String> {
    let mut args: Vec<String> = FETCH_TIMEOUT_CONFIG
        .iter()
        .map(ToString::to_string)
        .collect();
    args.extend(
        [
            "fetch",
            "--quiet",
            "--no-tags",
            remote,
            &format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
        ]
        .map(String::from),
    );
    args
}

fn push_args(remote: &str, commit: &str, branch: &str) -> Vec<String> {
    let mut args: Vec<String> = FETCH_TIMEOUT_CONFIG
        .iter()
        .map(ToString::to_string)
        .collect();
    args.extend(
        [
            "push",
            "--quiet",
            remote,
            &format!("{commit}:refs/heads/{branch}"),
        ]
        .map(String::from),
    );
    args
}

/// The commit the next publish builds on: the remote branch after a fresh fetch; when the
/// remote has no such branch yet, the local `HEAD`; `None` for a repository with neither.
fn tip(g: &Git, target: &Target) -> Result<Option<String>, PublishError> {
    if let Some(remote) = &target.remote {
        let args = fetch_args(remote, &target.remote_branch);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = g.run(&refs)?;
        if !out.ok {
            let text = out.message();
            // An empty remote has no branch to fetch: that is the first push, not a failure.
            if !text.contains("couldn't find remote ref") {
                return Err(g.fail("fetch", &out));
            }
            return head_of(g);
        }
        let tracking = format!("refs/remotes/{remote}/{}", target.remote_branch);
        if let Some(sha) = g.try_ok(&["rev-parse", "--verify", "-q", &tracking])? {
            return Ok(Some(sha));
        }
    }
    head_of(g)
}

fn head_of(g: &Git) -> Result<Option<String>, PublishError> {
    g.try_ok(&["rev-parse", "--verify", "-q", "HEAD"])
}

// ----- the local branch ------------------------------------------------------------------------

/// Moves the local branch to `commit` and checks out what came with it, leaving unrelated
/// uncommitted work alone. Files the commit also contains that the user has locally (the
/// meeting files, or `topics.yaml` with exactly the edits that went into the merge) are
/// replaced by the committed version; anything else that is in the way keeps the branch
/// where it is.
fn sync_local(
    repo: &Repo,
    g: &Git,
    target: &Target,
    commit: &str,
    topics_ours: Option<&[Topic]>,
    cycle_id: &str,
) -> Result<LocalSync, PublishError> {
    let _guard = repo.lock();
    let head = head_of(g)?;
    if head.as_deref() == Some(commit) {
        return Ok(LocalSync::Updated);
    }
    let topics_file = topics_path(cycle_id);
    let is_ours_topics = |path: &str| {
        path == topics_file
            && topics_ours.is_some_and(|ours| {
                fs::read_to_string(g.root.join(path))
                    .ok()
                    .and_then(|t| topics::parse_topics(&t).ok())
                    .is_some_and(|disk| disk == ours)
            })
    };
    let on_disk = |path: &str| g.root.join(path).is_file();
    // The blob the working-tree file would be committed as (filters applied).
    let disk_blob = |path: &str| -> Result<Option<String>, PublishError> {
        g.try_ok(&[
            "hash-object",
            &format!("--path={path}"),
            &g.root.join(path).to_string_lossy(),
        ])
    };

    match head {
        None => {
            // A repository without a first commit: the branch becomes the commit, and the
            // files the commit brings are checked out.
            g.ok(&[
                "update-ref",
                &format!("refs/heads/{}", target.branch),
                commit,
            ])?;
            g.ok(&["reset", "--quiet"])?;
            let listed = g.ok(&["ls-tree", "-r", "--name-only", commit])?;
            for path in listed.lines() {
                if !on_disk(path) || is_ours_topics(path) {
                    g.ok(&["checkout", "--quiet", "--", path])?;
                }
            }
            Ok(LocalSync::Updated)
        }
        Some(head) => {
            let ancestor = g.run(&["merge-base", "--is-ancestor", &head, commit])?.ok;
            if !ancestor {
                return Ok(LocalSync::Behind(
                    "this clone has commits that are not on the remote yet; update it with a pull"
                        .into(),
                ));
            }
            // Files the merge would overwrite that hold exactly what is being committed.
            let changed = g.ok(&["diff", "--name-only", &head, commit])?;
            let mut moved: Vec<(String, Vec<u8>, bool)> = Vec::new();
            for path in changed.lines().filter(|p| on_disk(p)) {
                let committed =
                    g.try_ok(&["rev-parse", "--verify", "-q", &format!("{commit}:{path}")])?;
                let on_disk_blob = disk_blob(path)?;
                let same = on_disk_blob.is_some() && on_disk_blob == committed;
                if !(same || is_ours_topics(path)) {
                    continue;
                }
                let bytes = fs::read(g.root.join(path))
                    .map_err(|e| PublishError::permanent(e.to_string()))?;
                let tracked = g
                    .try_ok(&["ls-tree", "--name-only", &head, "--", path])?
                    .is_some_and(|l| !l.is_empty());
                if tracked {
                    g.ok(&["checkout", "--quiet", &head, "--", path])?;
                } else {
                    fs::remove_file(g.root.join(path))
                        .map_err(|e| PublishError::permanent(e.to_string()))?;
                }
                moved.push((path.to_string(), bytes, tracked));
            }
            let merged = g.run(&["merge", "--ff-only", "--quiet", commit])?;
            if merged.ok {
                return Ok(LocalSync::Updated);
            }
            // Put back what was set aside: nothing may be lost.
            for (path, bytes, _) in &moved {
                let _ = fs::write(g.root.join(path), bytes);
            }
            let reason = g.scrubbed(&merged.message());
            Ok(LocalSync::Behind(format!(
                "uncommitted changes in this clone are in the way ({}); update it with a pull after committing or discarding them",
                reason.lines().next().unwrap_or("")
            )))
        }
    }
}

// ----- the public entry points ------------------------------------------------------------------

/// Publishes the meeting: builds the commit, pushes it (with the rebuild-on-new-tip retry)
/// and moves the local branch. See the module documentation.
pub fn publish_meeting(repo: &Repo, req: &PublishRequest) -> Result<Published, PublishError> {
    publish_meeting_observed(repo, req, &mut |_| {})
}

/// [`publish_meeting`] that calls `before_push(attempt)` right before each push (attempts
/// count from 1). It exists for tests of races: the callback can push to the remote as a
/// colleague would.
pub fn publish_meeting_observed(
    repo: &Repo,
    req: &PublishRequest,
    before_push: &mut dyn FnMut(u32),
) -> Result<Published, PublishError> {
    validate(req)?;
    if !repo.is_git_repo() {
        return Err(PublishError::permanent(format!(
            "{} is not a Git repository",
            repo.root().display()
        )));
    }
    let secrets: Vec<String> = req.auth.map(|a| vec![a.token.clone()]).unwrap_or_default();
    let mut g = Git {
        root: repo.root(),
        env: Vec::new(),
        secrets,
    };
    let target = target_of(&g)?;
    // The token applies to the remote it was made for, and to nothing else.
    if let (Some(auth), Some(remote)) = (req.auth, &target.remote) {
        let url = g.ok(&["remote", "get-url", remote])?;
        g.env.extend(auth_env(&url, auth));
    }
    let paths = |folder: &str| -> Vec<String> {
        req.files
            .iter()
            .map(|f| meeting_path(req.base_path, req.cycle_id, folder, &f.rel))
            .collect()
    };

    let mut attempts = 0;
    loop {
        attempts += 1;
        let parent = tip(&g, &target)?;
        let built = build(repo, &g, parent.as_deref(), req)?;
        let (folder, commit, merged, existing) = match built {
            Built::Existing { folder, commit } => (folder, commit, None, true),
            Built::New {
                folder,
                commit,
                topics,
            } => (folder, commit, topics, false),
        };
        let mut pushed = false;
        if let Some(remote) = &target.remote {
            if existing && parent.is_some() && is_on_remote(&g, remote, &target, &commit)? {
                pushed = true;
            } else {
                before_push(attempts);
                let args = push_args(remote, &commit, &target.remote_branch);
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let out = g.run(&refs)?;
                if !out.ok {
                    let text = g.scrubbed(&out.message());
                    if rejected_by_remote_race(&text) {
                        if attempts >= MAX_ATTEMPTS {
                            return Err(PublishError::transient(format!(
                                "the remote kept changing while publishing ({attempts} attempts); it will be tried again"
                            )));
                        }
                        continue;
                    }
                    return Err(g.fail("push", &out));
                }
                pushed = true;
                // The remote-tracking ref follows what was pushed.
                let _ = g.run(&[
                    "update-ref",
                    &format!("refs/remotes/{remote}/{}", target.remote_branch),
                    &commit,
                ]);
            }
        }
        let local = sync_local(repo, &g, &target, &commit, req.topics, req.cycle_id)?;
        return Ok(Published {
            paths: paths(&folder),
            folder,
            commit,
            branch: target.branch.clone(),
            remote: target.remote.clone(),
            pushed,
            already_published: existing,
            attempts,
            topics: merged,
            local,
        });
    }
}

/// Whether `commit` is the tip of the remote branch or already contained in it.
fn is_on_remote(
    g: &Git,
    remote: &str,
    target: &Target,
    commit: &str,
) -> Result<bool, PublishError> {
    let tracking = format!("refs/remotes/{remote}/{}", target.remote_branch);
    Ok(g.run(&["merge-base", "--is-ancestor", commit, &tracking])?
        .ok)
}

/// What publishing would do to `topics.yaml`, for the screen that asks before it commits.
/// Reads what is here (the remote-tracking branch as last fetched, no network) and changes
/// nothing.
pub fn preview_topics(
    repo: &Repo,
    cycle_id: &str,
    ours: &[Topic],
) -> Result<TopicsMerge, PublishError> {
    if !is_identifier(cycle_id) {
        return Err(PublishError::permanent("the cycle id is not valid"));
    }
    let g = Git {
        root: repo.root(),
        env: Vec::new(),
        secrets: Vec::new(),
    };
    if !repo.is_git_repo() {
        return Ok(topics::merge(&[], &[], ours));
    }
    let target = target_of(&g)?;
    let path = topics_path(cycle_id);
    let upstream_ref = match &target.remote {
        Some(remote) => g
            .try_ok(&[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/remotes/{remote}/{}", target.remote_branch),
            ])?
            .or(head_of(&g)?),
        None => head_of(&g)?,
    };
    let text_at = |rev: Option<&str>| -> Result<Vec<Topic>, PublishError> {
        let Some(rev) = rev else {
            return Ok(Vec::new());
        };
        match g.try_ok(&["cat-file", "-p", &format!("{rev}:{path}")])? {
            Some(t) => read_topics(&t, "in the repository"),
            None => Ok(Vec::new()),
        }
    };
    let upstream = text_at(upstream_ref.as_deref())?;
    let base = text_at(head_of(&g)?.as_deref())?;
    Ok(topics::merge(&upstream, &base, ours))
}

#[cfg(test)]
mod tests;
