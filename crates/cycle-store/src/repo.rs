//! The local cycle repository: a Git working tree (a clone, or a repository the
//! application initialised itself) with the layout described in `docs/CYCLES.md`.
//!
//! Git is driven through the `git` command line. The application only writes to the
//! working tree; committing and pushing belong to the publishers. Pull state and the open
//! issues cache live inside the `.git` directory, so they never show up as changes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cycle::Cycle;
use crate::error::{CycleError, Result, ValidationIssue};
use crate::fsutil::write_atomic;
use crate::git;
use crate::schema::is_identifier;
use crate::template::{self, Template};
use crate::time::{format_utc_date, format_utc_minute, now_unix_ms};
use crate::topics::{self, Topic};

const STATE_FILE: &str = "bbb-cache.json";
const FETCH_TIMEOUT_CONFIG: [&str; 4] = [
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=30",
];

/// What is known about the last attempts to update the clone. Stored in
/// `<git dir>/bbb-cache.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoState {
    /// When the last successful pull (or the clone) finished.
    #[serde(default)]
    pub last_pull_ok_unix_ms: Option<u64>,
    /// Why the last attempt failed; cleared by the next success.
    #[serde(default)]
    pub last_pull_error: Option<String>,
    #[serde(default)]
    pub last_pull_attempt_unix_ms: Option<u64>,
}

impl RepoState {
    /// The "state as of" date (UTC, `YYYY-MM-DD`) of the data in the clone.
    pub fn as_of_date(&self) -> Option<String> {
        self.last_pull_ok_unix_ms.map(format_utc_date)
    }

    /// The same moment with the time, `YYYY-MM-DD HH:MM UTC`.
    pub fn as_of(&self) -> Option<String> {
        self.last_pull_ok_unix_ms.map(format_utc_minute)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    /// New commits were fetched and local work was rebased on them.
    Updated,
    UpToDate,
    /// A local-only repository: nothing to pull.
    NoRemote,
    /// The pull failed (offline, conflict, no upstream); the clone is untouched and usable.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullReport {
    pub outcome: PullOutcome,
    pub state: RepoState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub branch: Option<String>,
    /// Abbreviated hash of `HEAD`; `None` before the first commit.
    pub head: Option<String>,
    pub has_remote: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Paths with uncommitted changes, as reported by `git status --porcelain`.
    pub changed_files: Vec<String>,
    pub state: RepoState,
}

impl RepoStatus {
    pub fn is_dirty(&self) -> bool {
        !self.changed_files.is_empty()
    }
}

/// A directory under `cycles/` that holds a `cycle.yaml`.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleEntry {
    /// Directory name.
    pub dir_name: String,
    /// The parsed cycle when the file has the right shape (it may still have issues).
    pub cycle: Option<Cycle>,
    /// Everything wrong with the cycle; empty for a valid one.
    pub issues: Vec<ValidationIssue>,
}

impl CycleEntry {
    pub fn is_valid(&self) -> bool {
        self.cycle.is_some() && self.issues.is_empty()
    }
}

#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Opens an existing directory as a repository. The directory must exist; it does not
    /// have to be a Git repository for the file operations.
    pub fn open(root: impl Into<PathBuf>) -> Result<Repo> {
        let root = root.into();
        if !root.is_dir() {
            return Err(CycleError::Other(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        Ok(Repo { root })
    }

    /// Creates a local-only repository (no remote) with `main` as its branch. A directory
    /// that is already a repository is opened as it is.
    pub fn init_local(root: impl Into<PathBuf>) -> Result<Repo> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| CycleError::io(&root, e))?;
        let repo = Repo { root };
        if !repo.is_git_repo() {
            git::run_ok(Some(&repo.root), ["init", "--quiet"])?;
            git::run_ok(
                Some(&repo.root),
                ["symbolic-ref", "HEAD", "refs/heads/main"],
            )?;
        }
        fs::create_dir_all(repo.cycles_dir()).map_err(|e| CycleError::io(&repo.cycles_dir(), e))?;
        Ok(repo)
    }

    /// Clones `url` (a URL or a local path) into `dest`, which must not exist or be an
    /// empty directory. The clone counts as a successful pull for "state as of".
    pub fn clone(url: &str, dest: impl Into<PathBuf>) -> Result<Repo> {
        let dest = dest.into();
        if url.trim().is_empty() || url.starts_with('-') {
            return Err(CycleError::Other(format!("invalid repository URL {url:?}")));
        }
        if dest.exists() {
            let empty = dest.is_dir()
                && fs::read_dir(&dest)
                    .map_err(|e| CycleError::io(&dest, e))?
                    .next()
                    .is_none();
            if !empty {
                return Err(CycleError::Other(format!(
                    "{} exists and is not an empty directory",
                    dest.display()
                )));
            }
        }
        let mut args: Vec<std::ffi::OsString> = vec![];
        args.extend(FETCH_TIMEOUT_CONFIG.iter().map(Into::into));
        args.extend(["clone", "--quiet", "--"].iter().map(Into::into));
        args.push(url.into());
        args.push(dest.clone().into());
        git::run_ok(None, args)?;
        let repo = Repo { root: dest };
        let now = now_unix_ms();
        repo.write_state(&RepoState {
            last_pull_ok_unix_ms: Some(now),
            last_pull_error: None,
            last_pull_attempt_unix_ms: Some(now),
        });
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cycles_dir(&self) -> PathBuf {
        self.root.join("cycles")
    }

    pub fn cycle_dir(&self, id: &str) -> Result<PathBuf> {
        if !is_identifier(id) {
            return Err(CycleError::InvalidName(id.to_string()));
        }
        Ok(self.cycles_dir().join(id))
    }

    /// `meetings/<ID>`, the folder holding one `<date>/meeting.json` per meeting.
    pub fn meetings_dir(&self, id: &str) -> Result<PathBuf> {
        if !is_identifier(id) {
            return Err(CycleError::InvalidName(id.to_string()));
        }
        Ok(self.root.join("meetings").join(id))
    }

    // ----- Git -----

    pub fn is_git_repo(&self) -> bool {
        self.root.join(".git").exists()
    }

    fn require_git(&self) -> Result<()> {
        if self.is_git_repo() {
            Ok(())
        } else {
            Err(CycleError::Other(format!(
                "{} is not a Git repository",
                self.root.display()
            )))
        }
    }

    /// The `.git` directory (resolved through git, so worktrees work too).
    pub fn git_dir(&self) -> Result<PathBuf> {
        self.require_git()?;
        let dir = git::run_ok(Some(&self.root), ["rev-parse", "--absolute-git-dir"])?;
        Ok(PathBuf::from(dir))
    }

    fn has_remote(&self) -> Result<bool> {
        Ok(!git::run_ok(Some(&self.root), ["remote"])?.is_empty())
    }

    fn head(&self) -> Option<String> {
        let out = git::run(Some(&self.root), ["rev-parse", "--short", "HEAD"]).ok()?;
        out.ok.then(|| out.stdout.trim().to_string())
    }

    /// Pull state; a missing or unreadable file is the empty state.
    pub fn state(&self) -> RepoState {
        self.git_dir()
            .ok()
            .and_then(|d| fs::read_to_string(d.join(STATE_FILE)).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write_state(&self, state: &RepoState) {
        let result = self.git_dir().and_then(|dir| {
            let text = serde_json::to_string_pretty(state)
                .map_err(|e| CycleError::Other(e.to_string()))?;
            write_atomic(&dir.join(STATE_FILE), text.as_bytes())
        });
        if let Err(e) = result {
            tracing::warn!("cannot record the pull state: {e}");
        }
    }

    /// Path of the open issues cache of a cycle (inside the `.git` directory).
    pub fn issue_cache_path(&self, cycle_id: &str) -> Result<PathBuf> {
        if !is_identifier(cycle_id) {
            return Err(CycleError::InvalidName(cycle_id.to_string()));
        }
        Ok(self.git_dir()?.join(format!("bbb-issues-{cycle_id}.json")))
    }

    /// `git pull --rebase` with uncommitted edits kept aside and restored. A failure (no
    /// network, a conflict, no upstream branch) is not an error: the clone stays as it
    /// was, the failure is recorded in the state and returned as [`PullOutcome::Failed`].
    pub fn pull_rebase(&self) -> Result<PullReport> {
        self.require_git()?;
        if !self.has_remote()? {
            return Ok(PullReport {
                outcome: PullOutcome::NoRemote,
                state: self.state(),
            });
        }
        let before = self.head();
        let mut args: Vec<String> = FETCH_TIMEOUT_CONFIG
            .iter()
            .map(ToString::to_string)
            .collect();
        // Stashing and rebasing create commits and need an identity; supply a neutral one
        // only when the user has none configured.
        if !git::run(Some(&self.root), ["config", "--get", "user.email"])?.ok {
            args.extend(
                [
                    "-c",
                    "user.name=Blah Blah Bot",
                    "-c",
                    "user.email=noreply@localhost.invalid",
                ]
                .iter()
                .map(ToString::to_string),
            );
        }
        args.extend(
            ["pull", "--rebase", "--autostash", "--quiet"]
                .iter()
                .map(ToString::to_string),
        );
        let attempt = now_unix_ms();
        let out = git::run(Some(&self.root), &args)?;
        let mut state = self.state();
        state.last_pull_attempt_unix_ms = Some(attempt);
        let outcome = if out.ok {
            state.last_pull_ok_unix_ms = Some(attempt);
            state.last_pull_error = None;
            if self.head() == before {
                PullOutcome::UpToDate
            } else {
                PullOutcome::Updated
            }
        } else {
            // A rebase stopped by a conflict must not leave the clone mid-rebase.
            let _ = git::run(Some(&self.root), ["rebase", "--abort"]);
            let message = out.message();
            state.last_pull_error = Some(message.clone());
            PullOutcome::Failed(message)
        };
        self.write_state(&state);
        Ok(PullReport { outcome, state })
    }

    /// Branch, head, remote tracking and uncommitted changes, with the pull state.
    pub fn status(&self) -> Result<RepoStatus> {
        self.require_git()?;
        let text = git::run_ok(Some(&self.root), ["status", "--porcelain=v1", "--branch"])?;
        let mut status = RepoStatus {
            head: self.head(),
            has_remote: self.has_remote()?,
            state: self.state(),
            ..RepoStatus::default()
        };
        for line in text.lines() {
            if let Some(branch_line) = line.strip_prefix("## ") {
                parse_branch_line(branch_line, &mut status);
            } else if line.len() > 3 {
                status.changed_files.push(line[3..].trim().to_string());
            }
        }
        Ok(status)
    }

    // ----- Cycles -----

    /// All directories under `cycles/` that contain a `cycle.yaml`, sorted by name. A
    /// broken cycle is listed with its issues; it never fails the listing.
    pub fn list_cycles(&self) -> Result<Vec<CycleEntry>> {
        let dir = self.cycles_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CycleError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| CycleError::io(&dir, e))?;
            let path = entry.path();
            if !path.join("cycle.yaml").is_file() {
                continue;
            }
            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };
            let (cycle, issues) = self.inspect_cycle(&dir_name, &path);
            out.push(CycleEntry {
                dir_name,
                cycle,
                issues,
            });
        }
        out.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        Ok(out)
    }

    fn inspect_cycle(&self, dir_name: &str, path: &Path) -> (Option<Cycle>, Vec<ValidationIssue>) {
        let file = path.join("cycle.yaml");
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) => {
                return (
                    None,
                    vec![ValidationIssue::new(
                        "",
                        format!("cannot read cycle.yaml: {e}"),
                    )],
                )
            }
        };
        let (cycle, mut issues) = Cycle::parse(&text);
        if !is_identifier(dir_name) {
            issues.push(ValidationIssue::new(
                "",
                format!("directory name \"{dir_name}\" is not a valid cycle id"),
            ));
        }
        if let Some(c) = &cycle {
            if c.id != dir_name {
                issues.push(ValidationIssue::new(
                    "id",
                    format!(
                        "id \"{}\" does not match the directory name \"{dir_name}\"",
                        c.id
                    ),
                ));
            }
            let templates = template::list_templates(&path.join("templates")).unwrap_or_default();
            issues.extend(c.template_issues(&templates));
        }
        (cycle, issues)
    }

    /// Loads one cycle; any issue (including template references and an id that differs
    /// from the directory name) is an error.
    pub fn load_cycle(&self, id: &str) -> Result<Cycle> {
        let dir = self.cycle_dir(id)?;
        if !dir.join("cycle.yaml").is_file() {
            return Err(CycleError::CycleNotFound(id.to_string()));
        }
        match self.inspect_cycle(id, &dir) {
            (Some(cycle), issues) if issues.is_empty() => Ok(cycle),
            (_, issues) => Err(CycleError::invalid(format!("cycle {id}"), issues)),
        }
    }

    /// Writes `cycle.yaml` of an existing cycle after validating it against the schema, the
    /// participant rules and the templates that exist.
    pub fn save_cycle(&self, cycle: &Cycle) -> Result<()> {
        let dir = self.cycle_dir(&cycle.id)?;
        if !dir.is_dir() {
            return Err(CycleError::CycleNotFound(cycle.id.clone()));
        }
        let templates = template::list_templates(&dir.join("templates"))?;
        let issues = cycle.validate(&templates);
        if !issues.is_empty() {
            return Err(CycleError::invalid(format!("cycle {}", cycle.id), issues));
        }
        write_atomic(&dir.join("cycle.yaml"), cycle.to_yaml()?.as_bytes())
    }

    /// Creates a cycle: `cycle.yaml`, an empty `topics.yaml` and `templates/default.md`.
    /// Nothing is written when the cycle is invalid or already exists.
    pub fn create_cycle(&self, cycle: &Cycle) -> Result<()> {
        let dir = self.cycle_dir(&cycle.id)?;
        if dir.join("cycle.yaml").exists() {
            return Err(CycleError::CycleExists(cycle.id.clone()));
        }
        let templates_dir = dir.join("templates");
        let mut templates = template::list_templates(&templates_dir)?;
        if !templates
            .iter()
            .any(|t| t == crate::cycle::DEFAULT_TEMPLATE_NAME)
        {
            templates.push(crate::cycle::DEFAULT_TEMPLATE_NAME.to_string());
        }
        let issues = cycle.validate(&templates);
        if !issues.is_empty() {
            return Err(CycleError::invalid(format!("cycle {}", cycle.id), issues));
        }
        let yaml = cycle.to_yaml()?;
        template::create_default_template(&templates_dir)?;
        let topics_file = dir.join("topics.yaml");
        if !topics_file.exists() {
            write_atomic(&topics_file, topics::topics_to_yaml(&[])?.as_bytes())?;
        }
        write_atomic(&dir.join("cycle.yaml"), yaml.as_bytes())
    }

    // ----- Topics -----

    /// The topics of a cycle; a missing file is an empty list.
    pub fn load_topics(&self, id: &str) -> Result<Vec<Topic>> {
        let file = self.cycle_dir(id)?.join("topics.yaml");
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CycleError::io(&file, e)),
        };
        topics::parse_topics(&text)
            .map_err(|issues| CycleError::invalid(format!("topics of {id}"), issues))
    }

    pub fn save_topics(&self, id: &str, list: &[Topic]) -> Result<()> {
        let dir = self.cycle_dir(id)?;
        if !dir.is_dir() {
            return Err(CycleError::CycleNotFound(id.to_string()));
        }
        write_atomic(
            &dir.join("topics.yaml"),
            topics::topics_to_yaml(list)?.as_bytes(),
        )
    }

    // ----- Templates -----

    pub fn list_templates(&self, id: &str) -> Result<Vec<String>> {
        template::list_templates(&self.cycle_dir(id)?.join("templates"))
    }

    pub fn read_template(&self, id: &str, name: &str) -> Result<Template> {
        template::read_template(&self.cycle_dir(id)?.join("templates"), name)
    }

    pub fn write_template(&self, id: &str, template: &Template) -> Result<()> {
        let dir = self.cycle_dir(id)?;
        if !dir.is_dir() {
            return Err(CycleError::CycleNotFound(id.to_string()));
        }
        template::write_template(&dir.join("templates"), template)
    }
}

/// `main...origin/main [ahead 1, behind 2]`, `No commits yet on main`, `HEAD (no branch)`.
fn parse_branch_line(line: &str, status: &mut RepoStatus) {
    if let Some(rest) = line.strip_prefix("No commits yet on ") {
        status.branch = Some(rest.trim().to_string());
        return;
    }
    if line.starts_with("HEAD (no branch)") {
        return;
    }
    let (names, counters) = match line.split_once(" [") {
        Some((n, c)) => (n, c.trim_end_matches(']')),
        None => (line, ""),
    };
    status.branch = names.split("...").next().map(|b| b.trim().to_string());
    for part in counters.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            status.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            status.behind = n.parse().unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topics::TopicStatus;

    /// Runs git in `dir` with a fixed identity, for fixtures that commit.
    fn g(dir: &Path, args: &[&str]) -> String {
        let mut full = vec![
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        git::run_ok(Some(dir), full).unwrap()
    }

    fn set_identity(dir: &Path) {
        g(dir, &["config", "user.name", "Test User"]);
        g(dir, &["config", "user.email", "test@example.com"]);
    }

    fn commit_all(dir: &Path, message: &str) {
        g(dir, &["add", "-A"]);
        g(dir, &["commit", "--quiet", "-m", message]);
    }

    fn cycle(id: &str) -> Cycle {
        let mut c = Cycle::new(id, format!("Cycle {id}"));
        c.participants.push(crate::cycle::Participant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: None,
        });
        c
    }

    fn new_origin(root: &Path) -> Repo {
        let origin = Repo::init_local(root.join("origin")).unwrap();
        set_identity(origin.root());
        origin.create_cycle(&cycle("NOMAD")).unwrap();
        commit_all(origin.root(), "add NOMAD");
        origin
    }

    #[test]
    fn init_local_makes_a_clean_repo_on_main() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        assert!(repo.is_git_repo());
        let status = repo.status().unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head, None);
        assert!(!status.has_remote);
        assert!(!status.is_dirty());
        // Idempotent.
        Repo::init_local(repo.root()).unwrap();
        // A local-only repository has nothing to pull and records no error.
        let report = repo.pull_rebase().unwrap();
        assert_eq!(report.outcome, PullOutcome::NoRemote);
        assert_eq!(report.state.last_pull_error, None);
    }

    #[test]
    fn create_list_load_save_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        assert!(repo.list_cycles().unwrap().is_empty());

        let mut c = cycle("NOMAD");
        repo.create_cycle(&c).unwrap();
        let dir = repo.cycle_dir("NOMAD").unwrap();
        assert!(dir.join("cycle.yaml").is_file());
        assert_eq!(
            fs::read_to_string(dir.join("topics.yaml")).unwrap().trim(),
            "[]"
        );
        assert_eq!(
            fs::read_to_string(dir.join("templates").join("default.md")).unwrap(),
            crate::template::DEFAULT_TEMPLATE_TEXT
        );
        assert!(matches!(
            repo.create_cycle(&c),
            Err(CycleError::CycleExists(_))
        ));

        assert_eq!(repo.load_cycle("NOMAD").unwrap(), c);
        c.name = "Renamed".into();
        c.glossary.push(crate::cycle::GlossaryEntry {
            term: "GNSS".into(),
            hint: Some("positioning".into()),
        });
        repo.save_cycle(&c).unwrap();
        assert_eq!(repo.load_cycle("NOMAD").unwrap(), c);

        // Creating a cycle writes only untracked new files, saving only changes cycle.yaml.
        let entries = repo.list_cycles().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_valid());

        assert!(matches!(
            repo.load_cycle("MISSING"),
            Err(CycleError::CycleNotFound(_))
        ));
        assert!(matches!(
            repo.load_cycle("../x"),
            Err(CycleError::InvalidName(_))
        ));
        assert!(matches!(
            repo.save_cycle(&cycle("OTHER")),
            Err(CycleError::CycleNotFound(_))
        ));
    }

    #[test]
    fn invalid_cycles_are_listed_with_issues_and_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        repo.create_cycle(&cycle("GOOD")).unwrap();
        let cycles = repo.cycles_dir();
        fs::create_dir_all(cycles.join("BROKEN")).unwrap();
        fs::write(cycles.join("BROKEN").join("cycle.yaml"), "id: [\n").unwrap();
        fs::create_dir_all(cycles.join("WRONG")).unwrap();
        fs::write(
            cycles.join("WRONG").join("cycle.yaml"),
            "id: OTHER\nname: x\nreport: { default_mode: template }\n",
        )
        .unwrap();
        fs::create_dir_all(cycles.join("EMPTYDIR")).unwrap();

        let entries = repo.list_cycles().unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.dir_name.as_str()).collect();
        assert_eq!(names, ["BROKEN", "GOOD", "WRONG"]);
        assert!(entries[0].cycle.is_none() && !entries[0].issues.is_empty());
        assert!(entries[1].is_valid());
        let wrong = &entries[2];
        assert!(wrong.cycle.is_some() && !wrong.is_valid());
        assert!(wrong
            .issues
            .iter()
            .any(|i| i.path == "id" && i.message.contains("directory")));
        assert!(wrong
            .issues
            .iter()
            .any(|i| i.path == "report.default_template"));
        assert!(matches!(
            repo.load_cycle("WRONG"),
            Err(CycleError::Invalid { .. })
        ));
    }

    #[test]
    fn create_cycle_refuses_invalid_input_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        let mut c = cycle("NOMAD");
        c.report
            .by_type
            .insert("budget".into(), "budget-review".into());
        assert!(matches!(
            repo.create_cycle(&c),
            Err(CycleError::Invalid { .. })
        ));
        assert!(!repo.cycle_dir("NOMAD").unwrap().exists());
        assert!(matches!(
            repo.create_cycle(&cycle("a b")),
            Err(CycleError::InvalidName(_))
        ));
    }

    #[test]
    fn topics_and_templates_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        repo.create_cycle(&cycle("NOMAD")).unwrap();
        assert!(repo.load_topics("NOMAD").unwrap().is_empty());
        let list = vec![Topic {
            id: "t-1".into(),
            text: "Wheel size".into(),
            status: TopicStatus::Open,
            opened_in: "2026-11-06".into(),
            closed_in: None,
        }];
        repo.save_topics("NOMAD", &list).unwrap();
        assert_eq!(repo.load_topics("NOMAD").unwrap(), list);

        assert_eq!(repo.list_templates("NOMAD").unwrap(), ["default"]);
        let t = crate::template::parse_template(
            "budget-review",
            "---\nsections:\n  - { id: costs, title: Costs }\n---\n# {title}\n{section:costs}\n",
        )
        .unwrap();
        repo.write_template("NOMAD", &t).unwrap();
        assert_eq!(repo.read_template("NOMAD", "budget-review").unwrap(), t);
        assert_eq!(
            repo.list_templates("NOMAD").unwrap(),
            ["budget-review", "default"]
        );
        // by_type may now point to it.
        let mut c = cycle("NOMAD");
        c.report
            .by_type
            .insert("budget".into(), "budget-review".into());
        repo.save_cycle(&c).unwrap();
        assert!(repo.save_topics("NOPE", &[]).is_err());
    }

    #[test]
    fn clone_pull_and_offline_failure_keep_the_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = new_origin(tmp.path());

        let clone = Repo::clone(origin.root().to_str().unwrap(), tmp.path().join("clone")).unwrap();
        set_identity(clone.root());
        assert_eq!(clone.list_cycles().unwrap().len(), 1);
        let status = clone.status().unwrap();
        assert!(status.has_remote && !status.is_dirty());
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(status.state.last_pull_ok_unix_ms.is_some());
        assert!(status.state.as_of_date().unwrap().starts_with("20"));

        // Nothing new.
        assert_eq!(clone.pull_rebase().unwrap().outcome, PullOutcome::UpToDate);

        // The remote gets a new cycle; the clone has an uncommitted edit that must survive.
        origin.create_cycle(&cycle("ROVER")).unwrap();
        commit_all(origin.root(), "add ROVER");
        let mut edited = clone.load_cycle("NOMAD").unwrap();
        edited.name = "Edited locally".into();
        clone.save_cycle(&edited).unwrap();
        let report = clone.pull_rebase().unwrap();
        assert_eq!(report.outcome, PullOutcome::Updated);
        assert_eq!(clone.list_cycles().unwrap().len(), 2);
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "Edited locally");
        assert_eq!(report.state.last_pull_error, None);

        // The cache file is not part of the working tree.
        let changed = clone.status().unwrap().changed_files;
        assert_eq!(
            changed,
            vec!["cycles/NOMAD/cycle.yaml".to_string()],
            "{changed:?}"
        );
        assert!(clone.git_dir().unwrap().join(STATE_FILE).is_file());
        assert!(!clone.root().join(STATE_FILE).exists());

        // The remote disappears: the pull fails, the clone and the last good date stay.
        let ok_before = clone.state().last_pull_ok_unix_ms;
        let gone = tmp.path().join("origin-moved");
        fs::rename(origin.root(), &gone).unwrap();
        let report = clone.pull_rebase().unwrap();
        let PullOutcome::Failed(message) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert!(!message.is_empty());
        assert_eq!(report.state.last_pull_ok_unix_ms, ok_before);
        assert_eq!(
            clone.state().last_pull_error.as_deref(),
            Some(message.as_str())
        );
        assert_eq!(clone.list_cycles().unwrap().len(), 2);
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "Edited locally");

        // It comes back: the next pull succeeds and clears the error.
        fs::rename(&gone, origin.root()).unwrap();
        let report = clone.pull_rebase().unwrap();
        assert_eq!(report.outcome, PullOutcome::UpToDate);
        assert_eq!(clone.state().last_pull_error, None);
        assert!(clone.state().last_pull_ok_unix_ms >= ok_before);
    }

    #[test]
    fn clone_rejects_bad_destinations_and_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = new_origin(tmp.path());
        let busy = tmp.path().join("busy");
        fs::create_dir_all(&busy).unwrap();
        fs::write(busy.join("file.txt"), "x").unwrap();
        assert!(Repo::clone(origin.root().to_str().unwrap(), &busy).is_err());
        assert!(Repo::clone("--upload-pack=evil", tmp.path().join("x")).is_err());
        assert!(Repo::clone("", tmp.path().join("y")).is_err());
        // An empty existing directory is fine.
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(Repo::clone(origin.root().to_str().unwrap(), &empty).is_ok());
        // A missing repository is an error and leaves nothing to open.
        assert!(Repo::clone(
            tmp.path().join("nothing").to_str().unwrap(),
            tmp.path().join("z")
        )
        .is_err());
    }

    #[test]
    fn status_reports_uncommitted_work() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = new_origin(tmp.path());
        assert!(!origin.status().unwrap().is_dirty());
        origin.create_cycle(&cycle("ROVER")).unwrap();
        let status = origin.status().unwrap();
        assert!(status.is_dirty());
        assert!(status.head.is_some());
        assert!(
            status
                .changed_files
                .iter()
                .any(|f| f.contains("cycles/ROVER")),
            "{:?}",
            status.changed_files
        );
    }

    #[test]
    fn issue_cache_path_is_inside_the_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init_local(tmp.path().join("repo")).unwrap();
        let path = repo.issue_cache_path("NOMAD").unwrap();
        assert!(path.starts_with(repo.git_dir().unwrap()));
        assert!(repo.issue_cache_path("../x").is_err());
        assert!(Repo::open(tmp.path().join("nope")).is_err());
    }

    #[test]
    fn branch_line_parsing() {
        let mut s = RepoStatus::default();
        parse_branch_line("main...origin/main [ahead 2, behind 1]", &mut s);
        assert_eq!(
            (s.branch.as_deref(), s.ahead, s.behind),
            (Some("main"), 2, 1)
        );
        let mut s = RepoStatus::default();
        parse_branch_line("No commits yet on main", &mut s);
        assert_eq!(s.branch.as_deref(), Some("main"));
        let mut s = RepoStatus::default();
        parse_branch_line("feature", &mut s);
        assert_eq!(s.branch.as_deref(), Some("feature"));
    }
}
