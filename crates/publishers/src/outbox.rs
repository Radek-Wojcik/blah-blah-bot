//! The outbox dispatcher: takes the operations the store holds, runs them and records what
//! happened.
//!
//! One operation at a time, oldest first (the store decides which is due: its retry time has
//! come, the operation it follows is done, and for a Discord post the issues of its meeting
//! are settled). Each run goes through the same steps:
//!
//! 1. The operation is claimed (`running`). Nothing but the database is locked, and only for
//!    each call; the network and Git run without any lock held.
//! 2. For everything that goes to the repository the public-repository check is made again
//!    ([`crate::gate`]), whatever the plan said when the operation was queued.
//! 3. The operation is executed from its payload and the frozen snapshot.
//! 4. The outcome is stored: `done` with a result, back to `queued` with a backoff (or the
//!    server's `Retry-After`) for a transient failure, or `failed` for one that needs the
//!    user. Every text that is stored passes through [`Env::scrub`].
//!
//! **Offline.** A request that gets no answer pauses the other operations of that target
//! until the next tick, so a dead network costs one timeout per tick, not one per operation.
//! The paused targets are reported by [`Dispatcher::offline_targets`].
//!
//! **Crash recovery.** [`Dispatcher::recover`] puts operations left `running` by a crash back
//! to `queued`. Every operation is safe to run twice: the commit finds its folder in the
//! tip, an issue is searched by its marker first, a status change reads the state and the
//! comment markers, and a Discord post resumes after the messages that are out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cycle_store::publish::{
    AddFileRequest, GitAuth, PublishErrorKind, PublishFile, PublishRequest,
};
use cycle_store::{Correction, Cycle, DiscordMode, Draft, Repo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use store::{ExternalRef, Failure, OpState, OutboxKind, OutboxOp, OutboxTarget, RefTarget, Store};

use crate::addr;
use crate::correction::{file_key, CorrectionPayload};
use crate::discord::{self, Attachment, OutMessage, Pacing};
use crate::discord_bot::{self, DiscordBot, EditOutcome};
use crate::export::{self, Assignee, ExternalTask, TaskKey, TaskRecord, UpsertOutcome};
use crate::gate::{self, RepoVisibility};
use crate::github::{CorrectionSpec, Github, IssueSpec, StatusSpec, WRITE_PACING};
use crate::http::{HttpClient, HttpError};
use crate::notion::{self, Notion, NotionTasks};
use crate::plan::{
    apply_topics, issue_key, notion_ref_key, CommitPayload, DiscordBotPayload, DiscordPayload,
    GithubIdentity, IssuePayload, NotionPayload, StatusPayload, DISCORD_POST_REF_KEY,
};
use crate::webhook::Webhook;

/// How long a visibility answer is trusted.
const VISIBILITY_TTL_MS: i64 = 30_000;
/// The longest wait between two ticks.
pub const TICK_MS: i64 = 30_000;

/// The cycle repository an operation works on.
pub struct RepoContext {
    pub repo: Repo,
    pub cycle: Cycle,
    /// Where issues go; `None` for a repository that is not linked to GitHub.
    pub github: Option<GithubIdentity>,
    /// The address of the remote, when the repository has one (for the key that a
    /// confirmation is stored under, when the remote is not on GitHub).
    pub remote_url: Option<String>,
    pub has_remote: bool,
}

impl RepoContext {
    /// The key a confirmation of this repository is stored under; `None` for a repository
    /// without a remote, which nothing leaves.
    pub fn repo_key(&self) -> Option<String> {
        if !self.has_remote {
            return None;
        }
        match &self.github {
            Some(g) => Some(addr::repo_key_of(g)),
            None => self
                .remote_url
                .as_deref()
                .and_then(addr::remote_key)
                .or_else(|| Some(store::repo_key("local", "unknown"))),
        }
    }
}

/// Everything the dispatcher needs from its surroundings; the application implements it with
/// the registered repositories and the credential store, tests with folders and fakes.
pub trait Env: Send + Sync {
    fn now_ms(&self) -> i64;
    /// The repository of a cycle.
    fn context(&self, cycle_id: &str) -> Result<RepoContext, String>;
    /// The GitHub token for the API at `api`.
    fn github_token(&self, api: &str) -> Option<String>;
    /// The webhook URL behind a `cred:` reference.
    fn webhook(&self, reference: &str) -> Option<String>;
    /// The Notion integration token behind a `cred:` reference.
    fn notion_token(&self, _reference: &str) -> Option<String> {
        None
    }
    /// The Discord bot token behind a `cred:` reference.
    fn discord_bot_token(&self, _reference: &str) -> Option<String> {
        None
    }
    /// The Notion API base address; tests point it to `FakeNotion`.
    fn notion_api(&self) -> String {
        notion::DEFAULT_API.to_string()
    }
    /// The Discord API base address; tests point it to `FakeDiscordBot`.
    fn discord_api(&self) -> String {
        discord_bot::DEFAULT_API.to_string()
    }
    /// The folder the snapshot paths of the operations are relative to.
    fn snapshot_root(&self) -> PathBuf;
    /// An HTTP client with the timeouts of the application.
    fn client(&self) -> HttpClient {
        HttpClient::new()
    }
    /// `text` with every secret the application holds masked: applied to each error that is
    /// stored or logged.
    fn scrub(&self, text: &str) -> String;
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn pacing(&self) -> Pacing {
        Pacing::default()
    }
    /// An operation changed state.
    fn outbox_changed(&self) {}
    /// Files of the cycle repository changed (a commit landed in the clone).
    fn repo_changed(&self, _cycle_id: &str) {}
    /// Right before each push of a commit (attempts count from 1). A seam for tests of a
    /// remote that changes under the publisher's feet; the application does nothing here.
    fn before_push(&self, _attempt: u32) {}
}

/// What one tick did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    pub done: usize,
    /// Operations that failed and will be tried again by themselves.
    pub retrying: usize,
    /// Operations that failed for good and wait for the user.
    pub failed: usize,
}

impl TickReport {
    pub fn ran(&self) -> usize {
        self.done + self.retrying + self.failed
    }
}

/// A failed attempt, before it is stored.
#[derive(Debug)]
struct OpError {
    failure: Failure,
    message: String,
    /// The host could not be reached.
    offline: bool,
}

impl OpError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            failure: Failure::Permanent,
            message: message.into(),
            offline: false,
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            failure: Failure::Transient {
                retry_after_ms: None,
            },
            message: message.into(),
            offline: false,
        }
    }
}

impl From<HttpError> for OpError {
    fn from(e: HttpError) -> Self {
        Self {
            offline: e.is_offline(),
            failure: e.failure(),
            message: e.message,
        }
    }
}

impl From<store::StoreError> for OpError {
    fn from(e: store::StoreError) -> Self {
        Self::transient(format!("database: {e}"))
    }
}

type OpResult = Result<Value, OpError>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One side of a `topics.yaml` conflict, as the screens show it.
fn topic_version(t: &cycle_store::Topic) -> Value {
    json!({ "text": t.text, "status": t.status })
}

/// `targets.github.path`, the folder of the meetings in the repository; the publisher refuses a
/// value that is not a safe folder.
fn meetings_path(ctx: &RepoContext) -> &str {
    ctx.cycle
        .targets
        .github
        .as_ref()
        .map_or(cycle_store::cycle::DEFAULT_MEETINGS_PATH, |g| {
            g.path.as_str()
        })
}

/// A failed Git publication as the outbox sees it: a transient one that is not just a remote
/// that kept changing means the remote could not be reached.
fn git_error(e: cycle_store::PublishError) -> OpError {
    OpError {
        offline: e.kind == PublishErrorKind::Transient && !e.message.contains("kept changing"),
        failure: match e.kind {
            PublishErrorKind::Transient => Failure::Transient {
                retry_after_ms: None,
            },
            PublishErrorKind::Permanent => Failure::Permanent,
        },
        message: e.message,
    }
}

fn payload<T: DeserializeOwned>(op: &OutboxOp) -> Result<T, OpError> {
    serde_json::from_str(&op.payload)
        .map_err(|e| OpError::permanent(format!("the operation cannot be read: {e}")))
}

/// Runs the operations of the outbox.
pub struct Dispatcher {
    store: Arc<Mutex<Store>>,
    env: Arc<dyn Env>,
    /// Targets that could not be reached, with the time of the failure.
    offline: Mutex<HashMap<OutboxTarget, i64>>,
    github: Mutex<HashMap<(String, String), Arc<Github>>>,
    notion: Mutex<HashMap<(String, String), Arc<Notion>>>,
    discord_bot: Mutex<HashMap<(String, String), Arc<DiscordBot>>>,
    visibility: Mutex<HashMap<String, (i64, RepoVisibility)>>,
}

impl Dispatcher {
    pub fn new(store: Arc<Mutex<Store>>, env: Arc<dyn Env>) -> Self {
        Self {
            store,
            env,
            offline: Mutex::new(HashMap::new()),
            github: Mutex::new(HashMap::new()),
            notion: Mutex::new(HashMap::new()),
            discord_bot: Mutex::new(HashMap::new()),
            visibility: Mutex::new(HashMap::new()),
        }
    }

    fn with_store<T>(&self, f: impl FnOnce(&Store) -> store::Result<T>) -> Result<T, OpError> {
        Ok(f(&lock(&self.store))?)
    }

    /// Call once at start: what a crash left `running` is queued again. Returns the ids.
    pub fn recover(&self) -> store::Result<Vec<i64>> {
        let ids = lock(&self.store).reset_running_ops(self.env.now_ms())?;
        if !ids.is_empty() {
            tracing::warn!(
                count = ids.len(),
                "publishing operations resumed after a restart"
            );
            self.env.outbox_changed();
        }
        Ok(ids)
    }

    /// Targets that did not answer in the last tick that tried them.
    pub fn offline_targets(&self) -> Vec<OutboxTarget> {
        let mut v: Vec<_> = lock(&self.offline).keys().copied().collect();
        v.sort_by_key(|t| t.as_str());
        v
    }

    /// Forgets that a target was unreachable (the user asked to retry now).
    pub fn clear_offline(&self) {
        lock(&self.offline).clear();
    }

    /// When the next waiting operation is due, in milliseconds on the clock of [`Env`].
    pub fn next_due_ms(&self) -> Option<i64> {
        lock(&self.store).next_retry_unix_ms().ok().flatten()
    }

    /// Runs every operation that is due, then returns.
    pub fn tick(&self) -> TickReport {
        let mut report = TickReport::default();
        let mut skip: Vec<OutboxTarget> = Vec::new();
        loop {
            let now = self.env.now_ms();
            let claimed = lock(&self.store).claim_next_op(now, &skip);
            let op = match claimed {
                Ok(Some(op)) => op,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "the outbox cannot be read");
                    break;
                }
            };
            self.env.outbox_changed();
            let outcome = self.execute(&op);
            let now = self.env.now_ms();
            let stored = match outcome {
                Ok(result) => {
                    lock(&self.offline).remove(&op.target);
                    report.done += 1;
                    lock(&self.store).complete_op(op.id, &result.to_string(), now)
                }
                Err(e) => {
                    let message = self.env.scrub(&e.message);
                    if e.offline {
                        lock(&self.offline).insert(op.target, now);
                        skip.push(op.target);
                    }
                    tracing::warn!(op = %op.op_key, meeting = %op.meeting_id, error = %message, "publishing operation failed");
                    match lock(&self.store).fail_op(op.id, e.failure, &message, now) {
                        Ok(OpState::Queued) => {
                            report.retrying += 1;
                            Ok(())
                        }
                        Ok(_) => {
                            report.failed += 1;
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            if let Err(e) = stored {
                tracing::error!(error = %e, op = op.id, "the outcome of an operation was not stored");
                break;
            }
            self.env.outbox_changed();
        }
        report
    }

    fn execute(&self, op: &OutboxOp) -> OpResult {
        match op.kind {
            OutboxKind::CommitMeeting => self.commit(op),
            OutboxKind::CreateIssue => self.create_issue(op),
            OutboxKind::SetIssueState => self.set_issue_state(op),
            OutboxKind::PostDiscord => self.post_discord(op),
            OutboxKind::CommitCorrection => self.commit_correction(op),
            OutboxKind::CommentCorrection => self.comment_correction(op),
            OutboxKind::ExportNotion => self.export_notion(op),
            OutboxKind::EditDiscord => self.edit_discord(op),
        }
    }

    // ----- shared pieces ------------------------------------------------------------------

    fn context(&self, cycle_id: &str) -> Result<RepoContext, OpError> {
        self.env.context(cycle_id).map_err(OpError::permanent)
    }

    fn github(&self, api: &str) -> Result<(Arc<Github>, String), OpError> {
        let token = self.env.github_token(api).unwrap_or_default();
        let key = (api.to_string(), token.clone());
        if let Some(found) = lock(&self.github).get(&key) {
            return Ok((found.clone(), token));
        }
        let client = self.env.client();
        let github = Arc::new(Github::with_client(client, api, &token)?);
        lock(&self.github).insert(key, github.clone());
        Ok((github, token))
    }

    fn notion(&self, token: &str) -> Result<Arc<Notion>, OpError> {
        let api = self.env.notion_api();
        let key = (api.clone(), token.to_string());
        if let Some(found) = lock(&self.notion).get(&key) {
            return Ok(found.clone());
        }
        let client = self.env.client();
        let notion = Arc::new(Notion::with_client(client, &api, token)?);
        lock(&self.notion).insert(key, notion.clone());
        Ok(notion)
    }

    fn discord_bot(&self, token: &str) -> Result<Arc<DiscordBot>, OpError> {
        let api = self.env.discord_api();
        let key = (api.clone(), token.to_string());
        if let Some(found) = lock(&self.discord_bot).get(&key) {
            return Ok(found.clone());
        }
        let client = self.env.client();
        let bot = Arc::new(DiscordBot::with_client(client, &api, token)?);
        lock(&self.discord_bot).insert(key, bot.clone());
        Ok(bot)
    }

    /// The public-repository check, made right before anything goes to the repository.
    fn gate(&self, ctx: &RepoContext) -> Result<(), OpError> {
        let Some(key) = ctx.repo_key() else {
            return Ok(());
        };
        let now = self.env.now_ms();
        let cached = lock(&self.visibility)
            .get(&key)
            .filter(|(at, _)| now - at < VISIBILITY_TTL_MS)
            .map(|(_, v)| *v);
        let visibility = match cached {
            Some(v) => v,
            None => {
                let (v, why) = match &ctx.github {
                    Some(g) => {
                        let (github, _) = self.github(&g.api)?;
                        gate::probe_with_error(&github, &g.repo)
                    }
                    None => (RepoVisibility::Unknown, None),
                };
                // An answer that is only missing because the network is down is not kept,
                // and not final: see below.
                let transient = why
                    .as_ref()
                    .is_some_and(|e| matches!(e.failure(), Failure::Transient { .. }));
                if !transient {
                    lock(&self.visibility).insert(key.clone(), (now, v));
                }
                if let (RepoVisibility::Unknown, Some(e), true) = (v, why, transient) {
                    let confirmed = self
                        .with_store(|s| s.get_repo_confirmation(&key))?
                        .is_some();
                    if !confirmed {
                        // Not a refusal: the repository may well be private. Wait for the
                        // network instead of asking the user about it.
                        let mut err = OpError::from(e);
                        err.message = format!(
                            "cannot check whether the repository is public yet: {}",
                            err.message
                        );
                        return Err(err);
                    }
                }
                v
            }
        };
        let confirmed = self
            .with_store(|s| s.get_repo_confirmation(&key))?
            .is_some();
        gate::require(visibility, confirmed).map_err(|e| OpError::permanent(e.to_string()))
    }

    fn publication(&self, op: &OutboxOp) -> Result<Option<store::Publication>, OpError> {
        self.with_store(|s| s.get_publication(&op.meeting_id))
    }

    // ----- commit ---------------------------------------------------------------------------

    fn commit(&self, op: &OutboxOp) -> OpResult {
        let p: CommitPayload = payload(op)?;
        let ctx = self.context(&p.cycle)?;
        self.gate(&ctx)?;
        let snapshot = self.env.snapshot_root().join(&p.snapshot);
        let mut files = Vec::new();
        for rel in &p.files {
            let path = snapshot.join(rel);
            if !path.is_file() {
                return Err(OpError::permanent(format!(
                    "the snapshot of the meeting lost {rel}: cancel the operation and publish again"
                )));
            }
            files.push(PublishFile {
                rel: rel.clone(),
                path,
            });
        }
        // topics.yaml is touched only when the meeting changes a topic.
        let topics = if p.topics_add.is_empty() && p.topics_set.is_empty() {
            None
        } else {
            let local = ctx
                .repo
                .load_topics(&p.cycle)
                .map_err(|e| OpError::permanent(format!("topics.yaml cannot be read: {e}")))?;
            Some(apply_topics(&local, &p.topics_add, &p.topics_set, &p.date))
        };
        let auth = self.git_auth(&ctx);
        let message = format!("Add meeting {}/{{folder}}", p.cycle);
        let base_path = meetings_path(&ctx);
        let request = PublishRequest {
            cycle_id: &p.cycle,
            base_path,
            date: &p.date,
            files: &files,
            topics: topics.as_deref(),
            message: &message,
            auth: auth.as_ref(),
        };
        let published =
            cycle_store::publish::publish_meeting_observed(&ctx.repo, &request, &mut |attempt| {
                self.env.before_push(attempt)
            })
            .map_err(git_error)?;

        let report_url = match (&ctx.github, published.pushed) {
            (Some(g), true) => Some(addr::report_url(
                &addr::web_base(&g.api),
                &g.repo.full_name(),
                &published.branch,
                base_path,
                &p.cycle,
                &published.folder,
            )),
            _ => None,
        };
        let now = self.env.now_ms();
        self.with_store(|s| {
            if let Some(mut row) = s.get_publication(&op.meeting_id)? {
                row.folder = Some(published.folder.clone());
                row.commit_sha = Some(published.commit.clone());
                row.report_url = report_url.clone();
                row.repo_key = ctx.repo_key();
                row.published_unix_ms = Some(now);
                s.put_publication(&row)?;
            }
            Ok(())
        })?;
        self.env.repo_changed(&p.cycle);
        let (local, behind) = match &published.local {
            cycle_store::LocalSync::Updated => ("updated", None),
            cycle_store::LocalSync::Behind(why) => ("behind", Some(why.clone())),
        };
        Ok(json!({
            "folder": published.folder,
            "commit": published.commit,
            "branch": published.branch,
            "pushed": published.pushed,
            "already_published": published.already_published,
            "attempts": published.attempts,
            "local": local,
            "local_note": behind,
            "report_url": report_url,
            "topics": published.topics.as_ref().map(|t| json!({
                "added": t.diff.added.len(),
                "changed": t.diff.changed.len(),
                "removed": t.diff.removed.len(),
                "conflicts": t.conflicts.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
                // Both versions, so the screen can show what the last write replaced.
                "conflict_details": t.conflicts.iter().map(|c| json!({
                    "id": c.id,
                    "theirs": c.theirs.as_ref().map(topic_version),
                    "ours": c.ours.as_ref().map(topic_version),
                })).collect::<Vec<_>>(),
            })),
        }))
    }

    // ----- issues ---------------------------------------------------------------------------

    fn repository_for_issues(
        &self,
        op: &OutboxOp,
        cycle: &str,
    ) -> Result<(RepoContext, GithubIdentity, Arc<Github>, Option<String>), OpError> {
        let ctx = self.context(cycle)?;
        let identity = ctx.github.clone().ok_or_else(|| {
            OpError::permanent("this repository is not linked to a GitHub repository")
        })?;
        self.gate(&ctx)?;
        let host = crate::github::api_host(&identity.api).unwrap_or_default();
        if self.env.github_token(&identity.api).is_none() {
            return Err(OpError::permanent(format!(
                "no GitHub token is stored for {host}: add it in the publishing settings, then retry"
            )));
        }
        let (github, _) = self.github(&identity.api)?;
        let report = self.publication(op)?.and_then(|p| p.report_url);
        Ok((ctx, identity, github, report))
    }

    fn create_issue(&self, op: &OutboxOp) -> OpResult {
        let p: IssuePayload = payload(op)?;
        let (_, identity, github, report_url) = self.repository_for_issues(op, &p.cycle)?;
        let transcript_url = p
            .anchor
            .as_ref()
            .and(report_url.as_deref())
            .and_then(addr::transcript_url);
        let spec = IssueSpec {
            cycle_id: p.cycle.clone(),
            meeting_id: op.meeting_id.clone(),
            item_id: p.item_id.clone(),
            title: p.title.clone(),
            assignee: p.assignee.clone(),
            due: p.due.clone(),
            meeting_date: p.date.clone(),
            report_url,
            transcript_url,
            transcript_anchor: p.anchor.clone(),
            time_ms: p.time_ms,
            quote: p.quote.clone(),
        };
        // Always look for the issue first: a crash or a lost answer may have created it. The
        // number of a created issue is kept as progress at once, so a retry fetches that issue
        // instead of relying on a list, which can trail the creation by seconds.
        let known = op
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str::<Value>(r).ok())
            .and_then(|v| v.get("issue_number").and_then(Value::as_u64));
        let store = &self.store;
        let env = &self.env;
        let mut on_created = |number: u64| {
            let progress = json!({ "issue_number": number }).to_string();
            if let Err(e) = lock(store).set_op_progress(op.id, &progress, env.now_ms()) {
                tracing::warn!(error = %e, "the number of a created issue was not stored");
            }
        };
        let outcome =
            github.publish_issue_tracked(&identity.repo, &spec, true, known, &mut on_created)?;
        if outcome.created {
            self.env.sleep(WRITE_PACING);
        }
        Ok(json!({
            "number": outcome.number,
            "url": outcome.url,
            "title": p.title,
            "created": outcome.created,
            "assigned": outcome.assigned,
            "assignee_dropped": outcome.assignee_dropped,
            "due": format!("{:?}", outcome.due),
        }))
    }

    fn set_issue_state(&self, op: &OutboxOp) -> OpResult {
        let p: StatusPayload = payload(op)?;
        let (_, identity, github, report_url) = self.repository_for_issues(op, &p.cycle)?;
        let transcript_url = p
            .anchor
            .as_ref()
            .and(report_url.as_deref())
            .and_then(addr::transcript_url);
        let spec = StatusSpec {
            meeting_id: op.meeting_id.clone(),
            proposal_id: p.proposal_id.clone(),
            open: p.open,
            reason: p.reason.clone(),
            quote: p.quote.clone(),
            meeting_date: p.date.clone(),
            report_url,
            transcript_url,
            transcript_anchor: p.anchor.clone(),
            time_ms: p.time_ms,
        };
        let outcome = github.set_issue_status(&identity.repo, p.number, &spec)?;
        if outcome.changed || outcome.commented {
            self.env.sleep(WRITE_PACING);
        }
        Ok(json!({
            "number": p.number,
            "open": p.open,
            "changed": outcome.changed,
            "commented": outcome.commented,
        }))
    }

    // ----- corrections ----------------------------------------------------------------------

    /// Adds the correction as a new file to the meeting's folder in the repository.
    fn commit_correction(&self, op: &OutboxOp) -> OpResult {
        let p: CorrectionPayload = payload(op)?;
        let ctx = self.context(&p.cycle)?;
        self.gate(&ctx)?;
        let folder = self
            .publication(op)?
            .and_then(|row| row.folder)
            .ok_or_else(|| {
                OpError::permanent(
                    "the meeting is not in the repository yet: publish it first, then retry",
                )
            })?;
        let text = cycle_store::corrections::render_markdown(
            &p.cycle,
            &op.meeting_id,
            &p.meeting_date,
            &p.correction,
        )
        .ok_or_else(|| {
            OpError::permanent("the correction cannot be written: an id is not usable")
        })?;
        // Rendered next to the meeting, so what was published can be looked at afterwards.
        let out = self.env.snapshot_root().join(&p.out_dir);
        std::fs::create_dir_all(&out)
            .map_err(|e| OpError::permanent(format!("the correction cannot be written: {e}")))?;
        let path = out.join(format!("{}.md", p.correction.id));
        std::fs::write(&path, text.as_bytes())
            .map_err(|e| OpError::permanent(format!("the correction cannot be written: {e}")))?;
        let auth = self.git_auth(&ctx);
        let base_path = meetings_path(&ctx);
        let message = format!("Add a correction to meeting {}/{folder}", p.cycle);
        let request = AddFileRequest {
            cycle_id: &p.cycle,
            base_path,
            folder: &folder,
            stem: cycle_store::corrections::FILE_STEM,
            ext: cycle_store::corrections::FILE_EXT,
            path: &path,
            message: &message,
            auth: auth.as_ref(),
        };
        let added = cycle_store::publish::add_file_observed(&ctx.repo, &request, &mut |attempt| {
            self.env.before_push(attempt)
        })
        .map_err(git_error)?;
        let url = match (&ctx.github, added.pushed) {
            (Some(g), true) => Some(format!(
                "{}/{}/blob/{}/{}",
                addr::web_base(&g.api),
                g.repo.full_name(),
                added.branch,
                added.repo_path
            )),
            _ => None,
        };
        self.env.repo_changed(&p.cycle);
        let (local, note) = match &added.local {
            cycle_store::LocalSync::Updated => ("updated", None),
            cycle_store::LocalSync::Behind(why) => ("behind", Some(why.clone())),
        };
        Ok(json!({
            "file": added.file,
            "path": added.repo_path,
            "commit": added.commit,
            "branch": added.branch,
            "pushed": added.pushed,
            "already_published": added.already_published,
            "attempts": added.attempts,
            "local": local,
            "local_note": note,
            "url": url,
        }))
    }

    /// The issue a correction is about: the one it names, or the issue made from its item.
    /// `Ok(None)` when there is none (the comment is skipped); an issue that is still being
    /// created is a transient failure.
    fn correction_issue(
        &self,
        op: &OutboxOp,
        correction: &cycle_store::Correction,
    ) -> Result<Option<u64>, OpError> {
        if let Some(n) = correction.issue_number() {
            return Ok(Some(n));
        }
        let Some(item) = &correction.item else {
            return Ok(None);
        };
        let made = self.with_store(|s| s.get_op_by_key(&op.meeting_id, &issue_key(item)))?;
        match made {
            Some(o) if o.state == OpState::Done => Ok(o
                .result
                .as_deref()
                .and_then(|r| serde_json::from_str::<Value>(r).ok())
                .and_then(|v| v.get("number").and_then(Value::as_u64))),
            Some(o) if matches!(o.state, OpState::Queued | OpState::Running) => Err(
                OpError::transient("the issue of the item is not created yet; waiting for it"),
            ),
            _ => Ok(None),
        }
    }

    /// Comments the correction on the issue it is about, once.
    fn comment_correction(&self, op: &OutboxOp) -> OpResult {
        let p: CorrectionPayload = payload(op)?;
        let Some(number) = self.correction_issue(op, &p.correction)? else {
            return Ok(json!({ "skipped": "no issue was made for this item" }));
        };
        let (_, identity, github, report_url) = self.repository_for_issues(op, &p.cycle)?;
        let file_url = self
            .with_store(|s| s.get_op_by_key(&op.meeting_id, &file_key(&p.correction.id)))?
            .and_then(|o| o.result)
            .and_then(|r| serde_json::from_str::<Value>(&r).ok())
            .and_then(|v| v.get("url").and_then(Value::as_str).map(String::from));
        let spec = CorrectionSpec {
            cycle_id: p.cycle.clone(),
            meeting_id: op.meeting_id.clone(),
            correction_id: p.correction.id.clone(),
            meeting_date: p.meeting_date.clone(),
            correction_date: p.correction.date.clone(),
            text: p.correction.text.clone(),
            report_url,
            file_url,
        };
        let commented = github.comment_correction(&identity.repo, number, &spec)?;
        if commented {
            self.env.sleep(WRITE_PACING);
        }
        Ok(json!({ "number": number, "commented": commented }))
    }

    fn git_auth(&self, ctx: &RepoContext) -> Option<GitAuth> {
        ctx.github.as_ref().and_then(|g| {
            let token = self.env.github_token(&g.api)?;
            Some(GitAuth {
                base_url: format!("{}/", addr::web_base(&g.api)),
                token,
            })
        })
    }

    // ----- notion ---------------------------------------------------------------------------

    fn export_notion(&self, op: &OutboxOp) -> OpResult {
        let p: NotionPayload = payload(op)?;
        let ctx = self.context(&p.cycle)?;
        let target = ctx
            .cycle
            .targets
            .notion
            .as_ref()
            .ok_or_else(|| OpError::permanent("this cycle has no Notion target"))?;
        let token = self.env.notion_token(&target.token).ok_or_else(|| {
            OpError::permanent(
                "no Notion token is stored for this cycle: add it in the publishing settings, then retry",
            )
        })?;
        let notion = self.notion(&token)?;
        let tasks = NotionTasks::prepare(
            &notion,
            &target.database,
            target.data_source.as_deref(),
            &target.properties,
        )?;
        let report_url = self.publication(op)?.and_then(|r| r.report_url);
        let transcript_url = p
            .anchor
            .as_ref()
            .and(report_url.as_deref())
            .and_then(addr::transcript_url);
        let assignee = match (&p.assignee_id, &p.assignee_name) {
            (Some(id), Some(name)) => Some(Assignee {
                participant_id: id.clone(),
                name: name.clone(),
                github: None,
            }),
            _ => None,
        };
        let record = TaskRecord {
            key: TaskKey {
                cycle: p.cycle.clone(),
                meeting: op.meeting_id.clone(),
                item: p.item_id.clone(),
            },
            title: p.title.clone(),
            assignee,
            due: p.due.clone(),
            meeting_date: p.date.clone(),
            cycle_name: p.cycle_name.clone(),
            report_url,
            transcript_url,
            transcript_anchor: p.anchor.clone(),
            quote: p.quote.clone(),
            time_ms: p.time_ms,
        };
        let ref_key = notion_ref_key(&p.item_id);
        let known = self
            .with_store(|s| s.get_external_ref(&op.meeting_id, RefTarget::Notion, &ref_key))?
            .map(|r| r.external_id);
        let store = &self.store;
        let env = &self.env;
        let meeting_id = op.meeting_id.as_str();
        let ref_key_str = ref_key.as_str();
        let mut persist_ref = |task: &ExternalTask| {
            let row = ExternalRef {
                meeting_id: meeting_id.to_string(),
                target: RefTarget::Notion,
                ref_key: ref_key_str.to_string(),
                external_id: task.id.clone(),
                url: task.url.clone(),
                extra: "{}".to_string(),
                updated_unix_ms: env.now_ms(),
            };
            if let Err(e) = lock(store).put_external_ref(&row) {
                tracing::warn!(error = %e, "the Notion page reference was not stored");
            }
        };
        let upserted = export::upsert(&tasks, &record, known.as_deref(), true, &mut persist_ref)?;
        if let UpsertOutcome::Updated(task) = &upserted.outcome {
            persist_ref(task);
        }
        if matches!(
            upserted.outcome,
            UpsertOutcome::Created(_) | UpsertOutcome::Updated(_)
        ) {
            self.env.sleep(notion::WRITE_PACING);
        }
        let (page_id, url, created, updated, removed) = match &upserted.outcome {
            UpsertOutcome::Created(t) => (t.id.as_str(), t.url.as_deref(), true, false, false),
            UpsertOutcome::Updated(t) => (t.id.as_str(), t.url.as_deref(), false, true, false),
            UpsertOutcome::Unchanged(t) => (t.id.as_str(), t.url.as_deref(), false, false, false),
            UpsertOutcome::Removed { id } => (id.as_str(), None, false, false, true),
        };
        Ok(json!({
            "page_id": page_id,
            "url": url,
            "title": p.title,
            "created": created,
            "updated": updated,
            "removed": removed,
            "duplicates": upserted.duplicates,
        }))
    }

    // ----- discord --------------------------------------------------------------------------

    /// The lines that link the issues made for this meeting.
    fn issue_links(&self, meeting_id: &str) -> Result<String, OpError> {
        let ops = self.with_store(|s| s.list_ops(Some(meeting_id)))?;
        let mut lines = Vec::new();
        for op in ops
            .iter()
            .filter(|o| o.kind == OutboxKind::CreateIssue && o.state == OpState::Done)
        {
            let Some(result) = op
                .result
                .as_deref()
                .and_then(|r| serde_json::from_str::<Value>(r).ok())
            else {
                continue;
            };
            let (Some(number), Some(url)) = (
                result.get("number").and_then(Value::as_u64),
                result.get("url").and_then(Value::as_str),
            ) else {
                continue;
            };
            let title: String = result
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .replace(['[', ']', '\n', '\r'], " ");
            let title: String = title.chars().take(100).collect();
            lines.push(format!("- [#{number} {}]({url})", title.trim()));
        }
        if lines.is_empty() {
            return Ok(String::new());
        }
        let text = format!("Issues:\n{}", lines.join("\n"));
        Ok(cycle_store::neutralise_mentions(
            &cycle_store::defuse_discord_mentions(&text),
        ))
    }

    fn load_discord_inputs(
        &self,
        op: &OutboxOp,
        ctx: &RepoContext,
        cycle: &str,
        snapshot_rel: &str,
        template_name: Option<&str>,
    ) -> Result<(Draft, Option<String>, String, String), OpError> {
        let snapshot = self.env.snapshot_root().join(snapshot_rel);
        let draft_text = std::fs::read_to_string(snapshot.join("meeting.json"))
            .map_err(|e| OpError::permanent(format!("the snapshot of the meeting is gone: {e}")))?;
        let draft = Draft::from_json_str(&draft_text)
            .map_err(|_| OpError::permanent("the snapshot of the meeting cannot be read"))?;
        let template = match template_name {
            Some(name) => Some(
                ctx.repo
                    .read_template(cycle, name)
                    .map_err(|e| {
                        OpError::permanent(format!("the Discord template cannot be read: {e}"))
                    })?
                    .body,
            ),
            None => None,
        };
        let report_link = self
            .publication(op)?
            .and_then(|r| r.report_url)
            .unwrap_or_else(|| "report.md".to_string());
        let word = match draft.language {
            cycle_store::Language::Pl => "Spotkanie",
            cycle_store::Language::En => "Meeting",
        };
        let title = format!("{word} {} {}", draft.cycle, draft.date);
        Ok((draft, template, report_link, title))
    }

    fn post_discord(&self, op: &OutboxOp) -> OpResult {
        let p: DiscordPayload = payload(op)?;
        if let Some(bot_cfg) = p.bot.clone() {
            return self.post_discord_bot(op, &p, &bot_cfg);
        }
        let ctx = self.context(&p.cycle)?;
        let value = self.env.webhook(&p.reference).ok_or_else(|| {
            OpError::permanent("no Discord webhook is stored for this cycle: add it in the publishing settings, then retry")
        })?;
        let hook = Webhook::parse(&value).map_err(|why| {
            OpError::permanent(format!("the stored webhook is not usable: {why}"))
        })?;
        let client = self
            .env
            .client()
            .with_secret(&hook.url())
            .with_secret(hook.token());

        let (draft, template, report_link, title) =
            self.load_discord_inputs(op, &ctx, &p.cycle, &p.snapshot, p.template.as_deref())?;
        let snapshot = self.env.snapshot_root().join(&p.snapshot);
        let links = self.issue_links(&op.meeting_id)?;
        let text = discord::render_text(&discord::TextSpec {
            draft: &draft,
            participants: &ctx.cycle.participants,
            template_body: template.as_deref(),
            title: &title,
            report_link: &report_link,
            issue_links: &links,
        })
        .map_err(|issues| {
            OpError::permanent(format!(
                "the Discord message cannot be rendered: {}",
                issues.first().map_or_else(String::new, ToString::to_string)
            ))
        })?;
        let mut files = Vec::new();
        for rel in &p.files {
            if let Ok(bytes) = std::fs::read(snapshot.join(rel)) {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                files.push(Attachment::new(name, bytes));
            }
        }
        let plan = discord::build_plan(&text, Vec::new(), files, &ctx.cycle.participants);
        let start = op
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str::<Value>(r).ok())
            .and_then(|v| v.get("sent_chunks").and_then(Value::as_u64))
            .unwrap_or(0) as usize;

        let store = &self.store;
        let env = &self.env;
        let mut on_sent = |count: usize, _id: &str| {
            let progress = json!({ "sent_chunks": count }).to_string();
            if let Err(e) = lock(store).set_op_progress(op.id, &progress, env.now_ms()) {
                tracing::warn!(error = %e, "the progress of a Discord post was not stored");
            }
        };
        let report = discord::send_messages(
            &client,
            &hook,
            &plan.messages,
            start,
            &self.env.pacing(),
            &mut on_sent,
        )
        .map_err(|f| {
            let mut e = OpError::from(f.error);
            e.message = format!(
                "{} ({} of {} messages sent)",
                e.message,
                f.sent,
                plan.messages.len()
            );
            e
        })?;
        Ok(json!({
            "sent_chunks": report.sent,
            "messages": plan.messages.len(),
            "skipped_files": plan.skipped.len(),
        }))
    }

    fn post_discord_bot(
        &self,
        op: &OutboxOp,
        p: &DiscordPayload,
        bot_cfg: &DiscordBotPayload,
    ) -> OpResult {
        let ctx = self.context(&p.cycle)?;
        let token = self.env.discord_bot_token(&bot_cfg.token).ok_or_else(|| {
            OpError::permanent(
                "no Discord bot token is stored for this cycle: add it in the publishing settings, then retry",
            )
        })?;
        let bot = self.discord_bot(&token)?;
        let (draft, template, report_link, title) =
            self.load_discord_inputs(op, &ctx, &p.cycle, &p.snapshot, p.template.as_deref())?;
        let snapshot = self.env.snapshot_root().join(&p.snapshot);
        let links = self.issue_links(&op.meeting_id)?;
        let mut files = Vec::new();
        for rel in &p.files {
            if let Ok(bytes) = std::fs::read(snapshot.join(rel)) {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                files.push(Attachment::new(name, bytes));
            }
        }
        let (all_messages, skipped_files) = if bot_cfg.thread {
            let summary_text = discord::render_text(&discord::TextSpec {
                draft: &draft,
                participants: &ctx.cycle.participants,
                template_body: template.as_deref(),
                title: &title,
                report_link: &report_link,
                issue_links: "",
            })
            .map_err(|issues| {
                OpError::permanent(format!(
                    "the Discord message cannot be rendered: {}",
                    issues.first().map_or_else(String::new, ToString::to_string)
                ))
            })?;
            let summary_plan = discord::build_plan(
                &summary_text,
                Vec::new(),
                Vec::new(),
                &ctx.cycle.participants,
            );
            let thread_plan =
                discord::build_plan(&links, Vec::new(), files, &ctx.cycle.participants);
            let mut msgs = summary_plan.messages;
            if msgs.is_empty() {
                msgs.push(OutMessage {
                    content: title.clone(),
                    allowed_mentions: json!({"parse": []}),
                    embeds: Vec::new(),
                    files: Vec::new(),
                });
            }
            msgs.extend(thread_plan.messages);
            (msgs, thread_plan.skipped.len())
        } else {
            let text = discord::render_text(&discord::TextSpec {
                draft: &draft,
                participants: &ctx.cycle.participants,
                template_body: template.as_deref(),
                title: &title,
                report_link: &report_link,
                issue_links: &links,
            })
            .map_err(|issues| {
                OpError::permanent(format!(
                    "the Discord message cannot be rendered: {}",
                    issues.first().map_or_else(String::new, ToString::to_string)
                ))
            })?;
            let plan = discord::build_plan(&text, Vec::new(), files, &ctx.cycle.participants);
            (plan.messages, plan.skipped.len())
        };

        let existing_ref = self.with_store(|s| {
            s.get_external_ref(&op.meeting_id, RefTarget::Discord, DISCORD_POST_REF_KEY)
        })?;
        let mut post = existing_ref
            .as_ref()
            .and_then(|r| serde_json::from_str::<DiscordPostExtra>(&r.extra).ok())
            .filter(|e| e.channel_id == bot_cfg.channel)
            .unwrap_or_else(|| DiscordPostExtra {
                channel_id: bot_cfg.channel.clone(),
                thread_id: None,
                message_ids: Vec::new(),
            });
        let pacing = self.env.pacing();
        let total = all_messages.len();

        let persist = |post: &DiscordPostExtra| {
            let now = self.env.now_ms();
            if let Some(first_id) = post.message_ids.first() {
                let row = ExternalRef {
                    meeting_id: op.meeting_id.clone(),
                    target: RefTarget::Discord,
                    ref_key: DISCORD_POST_REF_KEY.to_string(),
                    external_id: first_id.clone(),
                    url: Some(format!(
                        "https://discord.com/channels/{}/{}/{first_id}",
                        bot_cfg.guild, post.channel_id
                    )),
                    extra: serde_json::to_string(post).unwrap_or_else(|_| "{}".to_string()),
                    updated_unix_ms: now,
                };
                if let Err(e) = lock(&self.store).put_external_ref(&row) {
                    tracing::warn!(error = %e, "the Discord post reference was not stored");
                }
            }
            let progress = json!({
                "sent_chunks": post.message_ids.len(),
                "channel_id": post.channel_id,
                "thread_id": post.thread_id,
                "message_ids": post.message_ids,
            })
            .to_string();
            if let Err(e) = lock(&self.store).set_op_progress(op.id, &progress, now) {
                tracing::warn!(error = %e, "the progress of a Discord bot post was not stored");
            }
        };

        let map_bot_err = |err: HttpError, sent: usize| -> OpError {
            let explained =
                bot.explain_denied(err, &bot_cfg.guild, &bot_cfg.channel, bot_cfg.thread);
            let mut e = OpError::from(explained);
            if sent > 0 && sent < total {
                e.message = format!("{} ({sent} of {total} messages sent)", e.message);
            }
            e
        };

        // 1. Summary message in the channel.
        if post.message_ids.is_empty() && !all_messages.is_empty() {
            let n = discord_bot::nonce(&op.meeting_id, &op.op_key, 0);
            let posted = bot
                .post_message(&bot_cfg.channel, &all_messages[0], &n, &pacing)
                .map_err(|e| map_bot_err(e, 0))?;
            post.message_ids.push(posted.id);
            persist(&post);
            if let Some(wait) = posted.wait {
                self.env.sleep(wait);
            }
        }

        // 2. Public thread under the summary message (handles 160004 as already created).
        if bot_cfg.thread && post.thread_id.is_none() {
            if let Some(first_id) = post.message_ids.first().cloned() {
                let tid = bot
                    .create_thread(&bot_cfg.channel, &first_id, &title, &pacing)
                    .map_err(|e| map_bot_err(e, post.message_ids.len()))?;
                post.thread_id = Some(tid);
                persist(&post);
            }
        }

        // 3. Remaining text chunks, issue links and screenshots in the thread (or channel).
        for (idx, message) in all_messages.iter().enumerate().skip(post.message_ids.len()) {
            let dest = post.thread_id.as_deref().unwrap_or(&bot_cfg.channel);
            let n = discord_bot::nonce(&op.meeting_id, &op.op_key, idx);
            let posted = bot
                .post_message(dest, message, &n, &pacing)
                .map_err(|e| map_bot_err(e, post.message_ids.len()))?;
            post.message_ids.push(posted.id);
            persist(&post);
            if let Some(wait) = posted.wait {
                self.env.sleep(wait);
            }
        }

        Ok(json!({
            "sent_chunks": post.message_ids.len(),
            "messages": total,
            "skipped_files": skipped_files,
            "channel_id": post.channel_id,
            "thread_id": post.thread_id,
            "message_ids": post.message_ids,
        }))
    }

    fn edit_discord(&self, op: &OutboxOp) -> OpResult {
        let p: CorrectionPayload = payload(op)?;
        let ctx = self.context(&p.cycle)?;
        let Some(target) = ctx.cycle.targets.discord.as_ref() else {
            return Ok(json!({
                "edited": false,
                "replied": false,
                "skipped": "this cycle has no Discord target",
            }));
        };
        let Some(DiscordMode::Bot(bot_cfg)) = target.mode() else {
            return Ok(json!({
                "edited": false,
                "replied": false,
                "skipped": "Discord is not configured in bot mode",
            }));
        };
        let Some(post_ref) = self.with_store(|s| {
            s.get_external_ref(&op.meeting_id, RefTarget::Discord, DISCORD_POST_REF_KEY)
        })?
        else {
            return Ok(json!({
                "edited": false,
                "replied": false,
                "skipped": "no Discord post is recorded for this meeting",
            }));
        };
        let post: DiscordPostExtra =
            serde_json::from_str(&post_ref.extra).unwrap_or_else(|_| DiscordPostExtra {
                channel_id: bot_cfg.channel.clone(),
                thread_id: None,
                message_ids: vec![post_ref.external_id.clone()],
            });
        let channel_id = if post.channel_id.is_empty() {
            bot_cfg.channel.as_str()
        } else {
            post.channel_id.as_str()
        };
        let token = self.env.discord_bot_token(&bot_cfg.token).ok_or_else(|| {
            OpError::permanent(
                "no Discord bot token is stored for this cycle: add it in the publishing settings, then retry",
            )
        })?;
        let bot = self.discord_bot(&token)?;
        let snapshot_dir = self
            .publication(op)?
            .map(|r| r.snapshot_dir)
            .ok_or_else(|| {
                OpError::permanent(
                    "the publication snapshot of the meeting is missing: cannot re-render the Discord summary",
                )
            })?;
        let (draft, template, report_link, title) = self.load_discord_inputs(
            op,
            &ctx,
            &p.cycle,
            &snapshot_dir,
            target.template.as_deref(),
        )?;
        let links = if post.thread_id.is_some() {
            String::new()
        } else {
            self.issue_links(&op.meeting_id)?
        };
        let rendered = discord::render_text(&discord::TextSpec {
            draft: &draft,
            participants: &ctx.cycle.participants,
            template_body: template.as_deref(),
            title: &title,
            report_link: &report_link,
            issue_links: &links,
        })
        .map_err(|issues| {
            OpError::permanent(format!(
                "the Discord message cannot be rendered: {}",
                issues.first().map_or_else(String::new, ToString::to_string)
            ))
        })?;
        let base_summary = discord::split_content(rendered.trim(), discord::MAX_CONTENT_UNITS)
            .into_iter()
            .next()
            .unwrap_or_else(|| title.clone());

        let corrections = self.meeting_corrections_up_to(op, &p.correction)?;
        let edited_content = discord_bot::append_corrections(&base_summary, &corrections);
        let allowed_mentions = discord::allowed_mentions(&edited_content, &ctx.cycle.participants);
        let pacing = self.env.pacing();

        let already_edited = op
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str::<Value>(r).ok())
            .and_then(|v| v.get("edited").and_then(Value::as_bool))
            .unwrap_or(false);
        if !already_edited {
            let outcome = bot
                .edit_message(
                    channel_id,
                    &post_ref.external_id,
                    &edited_content,
                    &allowed_mentions,
                    &pacing,
                )
                .map_err(|e| {
                    OpError::from(bot.explain_denied(e, &bot_cfg.guild, channel_id, bot_cfg.thread))
                })?;
            if outcome == EditOutcome::Deleted {
                return Ok(json!({
                    "edited": false,
                    "replied": false,
                    "skipped": "the Discord message was deleted",
                }));
            }
            let progress = json!({ "edited": true }).to_string();
            if let Err(e) = lock(&self.store).set_op_progress(op.id, &progress, self.env.now_ms()) {
                tracing::warn!(error = %e, "the progress of a Discord edit was not stored");
            }
        }

        if let Some(thread_id) = post.thread_id.as_deref() {
            let file_url = self
                .with_store(|s| s.get_op_by_key(&op.meeting_id, &file_key(&p.correction.id)))?
                .and_then(|o| o.result)
                .and_then(|r| serde_json::from_str::<Value>(&r).ok())
                .and_then(|v| v.get("url").and_then(Value::as_str).map(String::from));
            let reply_text =
                discord_bot::format_correction_reply(&p.correction, file_url.as_deref());
            let reply_msg = OutMessage {
                content: reply_text,
                allowed_mentions: json!({"parse": []}),
                embeds: Vec::new(),
                files: Vec::new(),
            };
            let reply_nonce = discord_bot::nonce(&op.meeting_id, &op.op_key, 0);
            let posted = bot
                .post_message(thread_id, &reply_msg, &reply_nonce, &pacing)
                .map_err(|e| {
                    OpError::from(bot.explain_denied(e, &bot_cfg.guild, channel_id, true))
                })?;
            Ok(json!({
                "edited": true,
                "replied": true,
                "thread_id": thread_id,
                "reply_message_id": posted.id,
            }))
        } else {
            Ok(json!({
                "edited": true,
                "replied": false,
            }))
        }
    }

    /// All non-cancelled corrections of the meeting queued up to `op`, plus `current`,
    /// deduplicated by correction id in creation order.
    fn meeting_corrections_up_to(
        &self,
        op: &OutboxOp,
        current: &Correction,
    ) -> Result<Vec<Correction>, OpError> {
        let ops = self.with_store(|s| s.list_ops(Some(&op.meeting_id)))?;
        let mut out: Vec<Correction> = Vec::new();
        for o in ops.iter().filter(|o| {
            o.id <= op.id
                && o.state != OpState::Cancelled
                && matches!(
                    o.kind,
                    OutboxKind::EditDiscord | OutboxKind::CommitCorrection
                )
        }) {
            if let Ok(cp) = serde_json::from_str::<CorrectionPayload>(&o.payload) {
                if !out.iter().any(|c| c.id == cp.correction.id) {
                    out.push(cp.correction);
                }
            }
        }
        if !out.iter().any(|c| c.id == current.id) {
            out.push(current.clone());
        }
        Ok(out)
    }
}

/// Stored in `external_refs.extra` for `(RefTarget::Discord, "post")`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DiscordPostExtra {
    channel_id: String,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    message_ids: Vec<String>,
}

#[cfg(test)]
mod tests;
