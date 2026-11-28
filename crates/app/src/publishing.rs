//! Publishing, glued to the application: the environment the dispatcher runs in (registered
//! repositories, the credential store, the recordings folder), the background thread, and
//! the operations behind the publish commands. Free of Tauri: the commands in
//! `publish_commands.rs` are thin wrappers, and the events go through a callback.
//!
//! What publishing does, in short: the user presses Publish (or a cycle's `auto` rule fires
//! when a draft is complete); the draft, its report and transcript are frozen into a
//! snapshot; the plan (`publishers::plan`) becomes operations in the outbox; the dispatcher
//! thread runs them, retries what failed for a reason that passes, and tells the UI after
//! every change. See `publishers/src/outbox.rs` for the rules of running them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use cycle_store::issues::IssueCache;
use cycle_store::time::{format_utc_date, now_unix_ms};
use cycle_store::{Draft, Variant};
use publishers::flow::{self, Prepared, StartError};
use publishers::http::HttpClient;
use publishers::outbox::{Dispatcher, Env, RepoContext, TICK_MS};
use publishers::plan::{GithubIdentity, PlanSummary, PublishPlan};
use publishers::rules::{PublishTarget, Trigger};
use queue::draft::DraftFiles;
use queue::transcript::{self, TranscriptFiles, TranscriptView};
use serde::Serialize;
use serde_json::Value;
use store::{OpState, OutboxKind, OutboxOp, OutboxTarget, Store};

use crate::cycles::{self, Located};
use crate::draft_commands::{draft_error, ensure_draft_idle};
use crate::ipc::{IpcError, IpcResult};
use crate::publish_secrets::{github_identity, scrub};
use crate::secrets::{self, SecretStore};
use crate::settings::CycleRepoSetting;

/// Event with the whole state of the outbox, after every change.
pub const EVENT_OUTBOX: &str = "outbox-changed";

// ----- the environment ------------------------------------------------------------------------

/// What the environment tells the application.
pub enum Notice {
    /// An operation changed state.
    Outbox,
    /// Files of this cycle changed in the clone.
    Cycle(String),
}

type Repos = Box<dyn Fn() -> Vec<CycleRepoSetting> + Send + Sync>;
type GlobalRules = Box<dyn Fn() -> publishers::rules::GlobalPublishRules + Send + Sync>;
type Notify = Box<dyn Fn(Notice) + Send + Sync>;

/// The [`Env`] of the application.
pub struct AppEnv {
    repos: Repos,
    root: Box<dyn Fn() -> PathBuf + Send + Sync>,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    global_rules: Option<GlobalRules>,
    notify: OnceLock<Notify>,
    notion_api: String,
    discord_api: String,
}

impl AppEnv {
    pub fn new(
        repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static,
        root: impl Fn() -> PathBuf + Send + Sync + 'static,
        secrets: Arc<dyn SecretStore + Send + Sync>,
    ) -> Self {
        Self {
            repos: Box::new(repos),
            root: Box::new(root),
            secrets,
            global_rules: None,
            notify: OnceLock::new(),
            notion_api: publishers::notion::DEFAULT_API.to_string(),
            discord_api: publishers::discord_bot::DEFAULT_API.to_string(),
        }
    }

    /// Where the global publish rules of the preferences are read from (defaults without it).
    pub fn with_global_rules(
        mut self,
        rules: impl Fn() -> publishers::rules::GlobalPublishRules + Send + Sync + 'static,
    ) -> Self {
        self.global_rules = Some(Box::new(rules));
        self
    }

    #[cfg(test)]
    pub fn with_notion_api(mut self, api: impl Into<String>) -> Self {
        self.notion_api = api.into();
        self
    }

    #[cfg(test)]
    pub fn with_discord_api(mut self, api: impl Into<String>) -> Self {
        self.discord_api = api.into();
        self
    }

    /// Where notices go; set once.
    pub fn set_notify(&self, notify: impl Fn(Notice) + Send + Sync + 'static) {
        let _ = self.notify.set(Box::new(notify));
    }

    fn notice(&self, notice: Notice) {
        if let Some(n) = self.notify.get() {
            n(notice);
        }
    }

    pub fn repos(&self) -> Vec<CycleRepoSetting> {
        (self.repos)()
    }
}

impl Env for AppEnv {
    fn now_ms(&self) -> i64 {
        now_unix_ms() as i64
    }

    fn context(&self, cycle_id: &str) -> Result<RepoContext, String> {
        let located = cycles::locate(&self.repos(), cycle_id)
            .map_err(|e| e.message)?
            .ok_or_else(|| format!("the project {cycle_id} is not in any registered repository"))?;
        context_of(located)
    }

    fn github_token(&self, api: &str) -> Option<String> {
        let host = publishers::github::api_host(api)?;
        self.secrets
            .get(&secrets::endpoint_key(secrets::GITHUB_TOKEN, &host))
    }

    fn webhook(&self, reference: &str) -> Option<String> {
        secrets::resolve_cred(self.secrets.as_ref(), reference)
    }

    fn global_publish_rules(&self) -> publishers::rules::GlobalPublishRules {
        self.global_rules.as_ref().map(|f| f()).unwrap_or_default()
    }

    fn notion_token(&self, reference: &str) -> Option<String> {
        secrets::resolve_cred(self.secrets.as_ref(), reference)
    }

    fn discord_bot_token(&self, reference: &str) -> Option<String> {
        secrets::resolve_cred(self.secrets.as_ref(), reference)
    }

    fn notion_api(&self) -> String {
        self.notion_api.clone()
    }

    fn discord_api(&self) -> String {
        self.discord_api.clone()
    }

    #[cfg(test)]
    fn sleep(&self, _duration: Duration) {}

    #[cfg(test)]
    fn pacing(&self) -> publishers::discord::Pacing {
        publishers::discord::Pacing {
            sleep: Box::new(|_| {}),
            ..publishers::discord::Pacing::default()
        }
    }

    fn snapshot_root(&self) -> PathBuf {
        (self.root)()
    }

    fn client(&self) -> HttpClient {
        HttpClient::new()
    }

    fn scrub(&self, text: &str) -> String {
        scrub(self.secrets.as_ref(), &self.repos(), text)
    }

    fn outbox_changed(&self) {
        self.notice(Notice::Outbox);
    }

    fn repo_changed(&self, cycle_id: &str) {
        self.notice(Notice::Cycle(cycle_id.to_string()));
    }
}

fn context_of(located: Located) -> Result<RepoContext, String> {
    let cycle = located
        .entry
        .cycle
        .clone()
        .ok_or_else(|| format!("the project {} cannot be read", located.entry.dir_name))?;
    let github = github_identity(&located.setting)
        .ok()
        .map(|(repo, api)| GithubIdentity { repo, api });
    let has_remote = located.repo.status().map(|s| s.has_remote).unwrap_or(false);
    Ok(RepoContext {
        remote_url: located.setting.remote.clone(),
        repo: located.repo,
        cycle,
        github,
        has_remote,
    })
}

// ----- payloads -------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDto {
    /// `github`, `discord` or `notion`.
    pub target: &'static str,
    /// `auto`, `on-approval` or `off`; `None` when the cycle has no such target.
    pub mode: Option<&'static str>,
    /// Where the rule comes from: `project` (its own `publish`) or `global` (the preferences).
    pub source: &'static str,
    /// The global rule for this target (what `mode` is while the project follows it).
    pub global: &'static str,
    /// An explicit publication may use it.
    pub available: bool,
    /// Whether the target has the required configuration (GitHub remote/repo, Discord webhook/bot channel, Notion database).
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDto {
    pub code: &'static str,
    pub message: String,
    /// For `public_repo_unconfirmed`: `public` or `unknown`.
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPublishDto {
    /// The key a confirmation is stored under.
    pub key: Option<String>,
    /// `owner/name` of the GitHub repository, when linked.
    pub github: Option<String>,
    pub has_remote: bool,
    /// `public`, `private` or `unknown`.
    pub visibility: &'static str,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    pub meeting_id: String,
    pub cycle_id: String,
    pub targets: Vec<TargetDto>,
    pub summary: PlanSummary,
    pub blocks: Vec<BlockDto>,
    pub notes: Vec<String>,
    pub repo: RepoPublishDto,
    /// The draft is read-only because operations exist for it.
    pub locked: bool,
    /// A newer draft waits for a decision: publish after choosing one.
    pub new_draft_pending: bool,
    /// The outbox operations of the meeting, in order.
    pub ops: Vec<OutboxOpDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxOpDto {
    pub id: i64,
    pub meeting_id: String,
    pub target: &'static str,
    pub kind: &'static str,
    pub op_key: String,
    /// What the operation does, for a list.
    pub label: String,
    pub state: &'static str,
    pub attempts: u32,
    pub next_attempt_unix_ms: Option<i64>,
    pub last_error: Option<String>,
    pub result: Option<Value>,
    pub after_id: Option<i64>,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxStateDto {
    pub ops: Vec<OutboxOpDto>,
    /// Targets that did not answer: operations for them wait until the network is back.
    pub offline: Vec<&'static str>,
    pub now_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishResultDto {
    pub plan: PlanDto,
    /// Operations that were queued by this call (the others were queued already).
    pub queued: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmDto {
    pub key: Option<String>,
    pub visibility: &'static str,
    /// Operations the check had refused that run again.
    pub released: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub number: u64,
    pub title: String,
    pub assignee: Option<String>,
    pub url: Option<String>,
    /// The meeting the issue was made in, when it was made from an item.
    pub meeting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleTasksDto {
    pub cycle_id: String,
    /// `owner/name`, when the repository is linked to GitHub.
    pub repo: Option<String>,
    /// `YYYY-MM-DD` of the last successful read of the issues; `None` when never read.
    pub as_of: Option<String>,
    pub open: Vec<TaskDto>,
    pub done: Vec<TaskDto>,
}

fn state_name(s: OpState) -> &'static str {
    s.as_str()
}

/// What an operation does, in words, from its payload.
fn label_of(op: &OutboxOp) -> String {
    let payload: Value = serde_json::from_str(&op.payload).unwrap_or(Value::Null);
    let text = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or_default();
    match op.kind {
        OutboxKind::CommitMeeting => "Add the report and transcript to the repository".to_string(),
        OutboxKind::CreateIssue => format!("Create the issue: {}", text("title")),
        OutboxKind::SetIssueState => {
            let verb = if payload.get("open").and_then(Value::as_bool) == Some(true) {
                "Reopen"
            } else {
                "Close"
            };
            let number = payload.get("number").and_then(Value::as_u64).unwrap_or(0);
            format!("{verb} issue #{number}")
        }
        OutboxKind::PostDiscord => "Post the summary to Discord".to_string(),
        OutboxKind::CommitCorrection => {
            let id = payload
                .pointer("/correction/id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("Add the correction {id} to the repository")
        }
        OutboxKind::CommentCorrection => "Comment the correction on its issue".to_string(),
        OutboxKind::ExportNotion => format!("Export the task to Notion: {}", text("title")),
        OutboxKind::EditDiscord => {
            let id = payload
                .pointer("/correction/id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("Update the Discord message for the correction {id}")
        }
    }
}

fn target_name(t: OutboxTarget) -> &'static str {
    t.as_str()
}

impl From<&OutboxOp> for OutboxOpDto {
    fn from(op: &OutboxOp) -> Self {
        Self {
            id: op.id,
            meeting_id: op.meeting_id.clone(),
            target: target_name(op.target),
            kind: op.kind.as_str(),
            op_key: op.op_key.clone(),
            label: label_of(op),
            state: state_name(op.state),
            attempts: op.attempts,
            next_attempt_unix_ms: op.next_attempt_unix_ms,
            last_error: op.last_error.clone(),
            result: op
                .result
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok()),
            after_id: op.after_id,
            created_unix_ms: op.created_unix_ms,
            updated_unix_ms: op.updated_unix_ms,
        }
    }
}

pub(crate) fn mode_name(mode: cycle_store::PublishMode) -> &'static str {
    match mode {
        cycle_store::PublishMode::Auto => "auto",
        cycle_store::PublishMode::OnApproval => "on-approval",
        cycle_store::PublishMode::Off => "off",
    }
}

fn block_dto(b: &publishers::plan::Block) -> BlockDto {
    BlockDto {
        code: b.code(),
        message: b.message(),
        visibility: match b {
            publishers::plan::Block::PublicRepoUnconfirmed { visibility } => {
                Some(visibility.clone())
            }
            _ => None,
        },
    }
}

fn parse_targets(names: &[String]) -> IpcResult<Vec<PublishTarget>> {
    let mut out = Vec::new();
    for name in names {
        let t = PublishTarget::parse(name.trim()).ok_or_else(|| {
            IpcError::new(
                "invalid",
                format!("{name} is not a target (github, discord or notion)"),
            )
        })?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

fn start_error(e: StartError) -> IpcError {
    match e {
        StartError::Blocked(blocks) => {
            let first = blocks.first();
            let code = first.map_or("nothing_to_publish", publishers::plan::Block::code);
            IpcError::new(code, first.map(|b| b.message()).unwrap_or_default())
        }
        StartError::Files(m) => IpcError::new("io", m),
        StartError::Store(e) => e.into(),
    }
}

// ----- the service ----------------------------------------------------------------------------

enum Command {
    /// Look at the outbox now.
    Wake,
    /// A draft was completed: apply the `auto` rules to it.
    Auto(String),
}

/// What is loaded to plan or publish a meeting.
struct Loaded {
    dir: PathBuf,
    meeting_rel: String,
    draft: Draft,
    located: Located,
    ctx: RepoContext,
    view: Option<TranscriptView>,
    screenshots: Vec<store::Screenshot>,
    new_draft_pending: bool,
}

/// The files and anchors the snapshot is made of.
struct Material {
    report_md: String,
    transcript_md: Option<String>,
    images: Vec<(String, PathBuf)>,
    anchors: BTreeMap<String, String>,
}

pub struct Publishing {
    store: Arc<Mutex<Store>>,
    env: Arc<AppEnv>,
    dispatcher: Dispatcher,
    tx: Mutex<Option<Sender<Command>>>,
    /// Serialises reading and writing the corrections file of a meeting.
    corrections_lock: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Publishing {
    pub fn new(store: Arc<Mutex<Store>>, env: Arc<AppEnv>) -> Arc<Self> {
        let dispatcher = Dispatcher::new(store.clone(), env.clone());
        Arc::new(Self {
            store,
            env,
            dispatcher,
            tx: Mutex::new(None),
            corrections_lock: Mutex::new(()),
        })
    }

    /// Hooks the events of the environment to `emit` (called with the whole new state of the
    /// outbox) and `cycle_changed`.
    pub fn connect(
        self: &Arc<Self>,
        emit: impl Fn(&OutboxStateDto) + Send + Sync + 'static,
        cycle_changed: impl Fn(&str) + Send + Sync + 'static,
    ) {
        let weak: Weak<Publishing> = Arc::downgrade(self);
        self.env.set_notify(move |notice| match notice {
            Notice::Outbox => {
                if let Some(p) = weak.upgrade() {
                    if let Ok(state) = p.state() {
                        emit(&state);
                    }
                }
            }
            Notice::Cycle(id) => cycle_changed(&id),
        });
    }

    /// Starts the background thread: crash recovery first, then a tick every 30 seconds, or
    /// sooner when a retry comes due or something wakes it.
    pub fn start(self: &Arc<Self>) {
        let (tx, rx) = mpsc::channel();
        *lock(&self.tx) = Some(tx);
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name("publishing".into())
            .spawn(move || {
                if let Err(e) = this.dispatcher.recover() {
                    tracing::error!(error = %e, "could not resume the publishing operations");
                }
                match this.cleanup_unsent_local_ops() {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(
                        cancelled = n,
                        "cancelled stale unsent local-only publishing operations on startup"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e.message,
                        "could not clean up unsent local-only publishing operations"
                    ),
                }
                loop {
                    let now = this.env.now_ms();
                    let wait = this
                        .dispatcher
                        .next_due_ms()
                        .map_or(TICK_MS, |due| (due - now).clamp(100, TICK_MS));
                    match rx.recv_timeout(Duration::from_millis(wait as u64)) {
                        Ok(Command::Auto(meeting)) => {
                            if let Err(e) = this.auto_publish(&meeting) {
                                tracing::warn!(meeting = %meeting, error = %e, "automatic publishing did not start");
                            }
                        }
                        Ok(Command::Wake) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    this.dispatcher.tick();
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the publishing thread");
        }
    }

    /// Ends the background thread (its channel is closed). For tests that start the service
    /// and must not leave it running over files that are about to be removed.
    #[cfg(test)]
    pub fn stop(&self) {
        *lock(&self.tx) = None;
    }

    fn send(&self, command: Command) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(command);
        }
    }

    /// Look at the outbox now.
    pub fn wake(&self) {
        self.send(Command::Wake);
    }

    /// A draft was completed: the `auto` rules of its cycle apply (on the publishing thread).
    pub fn draft_completed(&self, meeting_id: &str) {
        self.send(Command::Auto(meeting_id.to_string()));
    }

    /// Idempotent startup cleanup: cancels queued or failed local-only commit operations
    /// whose project has neither a Git remote nor a linked GitHub repository, so stale unsent
    /// local operations from before target gating do not run or linger in the outbox. Done and
    /// running operations and other projects are never touched; operations that waited for a
    /// cancelled commit stop waiting for it. A failing cancellation is skipped, not fatal.
    pub fn cleanup_unsent_local_ops(&self) -> IpcResult<usize> {
        let ops = self.with_store(|s| s.list_ops(None))?;
        let mut cancelled = 0;
        for op in ops {
            if !matches!(op.state, OpState::Queued | OpState::Failed)
                || !matches!(
                    op.kind,
                    OutboxKind::CommitMeeting | OutboxKind::CommitCorrection
                )
            {
                continue;
            }
            let Some(cycle_id) = serde_json::from_str::<Value>(&op.payload)
                .ok()
                .and_then(|v| v.get("cycle").and_then(Value::as_str).map(String::from))
            else {
                continue;
            };
            let Ok(ctx) = self.env.context(&cycle_id) else {
                continue;
            };
            if ctx.has_remote || ctx.github.is_some() {
                continue;
            }
            let now = self.env.now_ms();
            match self.with_store(|s| s.cancel_op_releasing_dependents(op.id, now)) {
                Ok(()) => cancelled += 1,
                Err(e) => tracing::warn!(
                    op = op.id,
                    error = %e.message,
                    "could not cancel an unsent local-only operation"
                ),
            }
        }
        if cancelled > 0 {
            self.env.outbox_changed();
        }
        Ok(cancelled)
    }

    // ----- loading ---------------------------------------------------------------------------

    fn with_store<T>(&self, f: impl FnOnce(&Store) -> store::Result<T>) -> IpcResult<T> {
        Ok(f(&lock(&self.store))?)
    }

    fn load(&self, meeting_id: &str) -> IpcResult<Loaded> {
        let details = self
            .with_store(|s| s.get_meeting(meeting_id))?
            .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
        let dir = self.env.snapshot_root().join(&details.meeting.dir);
        let files = DraftFiles::new(&dir);
        let draft = files.load().map_err(draft_error)?;
        let repos = self.env.repos();
        let located = cycles::locate(&repos, &draft.cycle)?.ok_or_else(|| {
            IpcError::new(
                "not_found",
                format!(
                    "the project {} is not in any registered repository",
                    draft.cycle
                ),
            )
        })?;
        let ctx = self
            .env
            .context(&draft.cycle)
            .map_err(|m| IpcError::new("not_found", m))?;
        let view = TranscriptFiles::new(&dir).view().ok();
        Ok(Loaded {
            meeting_rel: details.meeting.dir.clone(),
            screenshots: details.screenshots,
            new_draft_pending: files.has_new_draft(),
            dir,
            draft,
            located,
            ctx,
            view,
        })
    }

    fn material(&self, loaded: &Loaded) -> IpcResult<Material> {
        let (report_md, _) =
            crate::draft_commands::render(Some(&loaded.located), &loaded.draft, Variant::Markdown)?;
        let view = loaded.view.as_ref();
        let transcript_md = view.map(transcript::published_markdown);
        let mut anchors = BTreeMap::new();
        if let Some(view) = view {
            let at = |t: u64| transcript::anchor_at(view, t);
            for item in &loaded.draft.items {
                if let Some(a) = item.source.as_ref().and_then(|s| at(s.t_start_ms)) {
                    anchors.insert(item.id.clone(), a);
                }
            }
            for p in &loaded.draft.status_proposals {
                if let Some(a) = p.source.as_ref().and_then(|s| at(s.t_start_ms)) {
                    anchors.insert(p.id.clone(), a);
                }
            }
        }
        // A screenshot taken during a passage the user excluded is not published either.
        let images = loaded
            .screenshots
            .iter()
            .filter(|s| {
                view.is_none_or(|v| !transcript::excluded_at(v, s.offset_ms.max(0.0) as u64))
            })
            .filter_map(|s| {
                let name = s.file.strip_prefix("img/")?.to_string();
                Some((name, loaded.dir.join(&s.file)))
            })
            .collect();
        Ok(Material {
            report_md,
            transcript_md,
            images,
            anchors,
        })
    }

    fn prepared<'a>(
        &'a self,
        loaded: &'a Loaded,
        material: &'a Material,
        trigger: Trigger,
        targets: &'a [PublishTarget],
    ) -> Prepared<'a> {
        Prepared {
            env: self.env.as_ref(),
            meeting_id: &loaded.draft.meeting_id,
            meeting_rel: &loaded.meeting_rel,
            draft: &loaded.draft,
            ctx: &loaded.ctx,
            report_md: &material.report_md,
            transcript_md: material.transcript_md.as_deref(),
            images: &material.images,
            anchors: &material.anchors,
            trigger,
            targets,
        }
    }

    fn ensure_publishable(&self, meeting_id: &str, loaded: &Loaded) -> IpcResult<()> {
        self.with_store_ipc(|s| ensure_draft_idle(s, meeting_id))?;
        if loaded.draft.meeting_id != meeting_id {
            return Err(IpcError::new(
                "draft_invalid",
                "the draft belongs to another meeting",
            ));
        }
        if loaded.new_draft_pending {
            return Err(IpcError::new(
                "new_draft_pending",
                "a newer draft waits for a decision: accept it or keep yours, then publish",
            ));
        }
        Ok(())
    }

    fn with_store_ipc<T>(&self, f: impl FnOnce(&Store) -> IpcResult<T>) -> IpcResult<T> {
        f(&lock(&self.store))
    }

    // ----- plan and publish -------------------------------------------------------------------

    fn targets_dto(
        plan: &PublishPlan,
        global: &publishers::rules::GlobalPublishRules,
    ) -> Vec<TargetDto> {
        [
            PublishTarget::Github,
            PublishTarget::Discord,
            PublishTarget::Notion,
        ]
        .into_iter()
        .map(|t| {
            let mode = plan.rules.mode(t);
            TargetDto {
                target: t.as_str(),
                mode: mode.map(mode_name),
                source: match plan.rules.source(t) {
                    publishers::rules::RuleSource::Project => "project",
                    publishers::rules::RuleSource::Global => "global",
                },
                global: mode_name(global.get(t)),
                available: plan.rules.allows(t, Trigger::Explicit),
                configured: plan.rules.is_configured(t),
            }
        })
        .collect()
    }

    fn plan_dto(&self, loaded: &Loaded, plan: &PublishPlan) -> IpcResult<PlanDto> {
        let facts = self.with_store(|s| flow::repo_facts(self.env.as_ref(), s, &loaded.ctx))?;
        let meeting_id = loaded.draft.meeting_id.clone();
        let ops = self.with_store(|s| s.list_ops(Some(&meeting_id)))?;
        let locked = flow::ops_lock_draft(&ops);
        Ok(PlanDto {
            meeting_id,
            cycle_id: loaded.ctx.cycle.id.clone(),
            targets: Self::targets_dto(plan, &self.env.global_publish_rules()),
            summary: plan.summary.clone(),
            blocks: plan.blocks.iter().map(block_dto).collect(),
            notes: plan.notes.clone(),
            repo: RepoPublishDto {
                key: loaded.ctx.repo_key(),
                github: loaded.ctx.github.as_ref().map(|g| g.repo.full_name()),
                has_remote: loaded.ctx.has_remote,
                visibility: facts.visibility.as_str(),
                confirmed: facts.confirmed,
            },
            locked,
            new_draft_pending: loaded.new_draft_pending,
            ops: ops.iter().map(Into::into).collect(),
        })
    }

    /// What publishing to `targets` (all the cycle's targets when empty) would do. Asks GitHub
    /// for the visibility of the repository; nothing is changed.
    pub fn plan(&self, meeting_id: &str, targets: &[String]) -> IpcResult<PlanDto> {
        let loaded = self.load(meeting_id)?;
        let mut wanted = parse_targets(targets)?;
        if wanted.is_empty() {
            wanted = vec![
                PublishTarget::Github,
                PublishTarget::Discord,
                PublishTarget::Notion,
            ];
        }
        let material = self.material(&loaded)?;
        let plan = self.with_store(|s| {
            flow::preview(
                s,
                &self.prepared(&loaded, &material, Trigger::Explicit, &wanted),
            )
        })?;
        self.plan_dto(&loaded, &plan)
    }

    /// Queues the publication of the meeting to `targets`. A repository that is public, or
    /// whose visibility is unknown, is refused with `public_repo_unconfirmed` until the user
    /// confirms it; the plan is checked here before anything is frozen, and once more by
    /// the queueing itself.
    pub fn publish(&self, meeting_id: &str, targets: &[String]) -> IpcResult<PublishResultDto> {
        let loaded = self.load(meeting_id)?;
        self.ensure_publishable(meeting_id, &loaded)?;
        let wanted = parse_targets(targets)?;
        if wanted.is_empty() {
            return Err(IpcError::new("invalid", "choose where to publish"));
        }
        let material = self.material(&loaded)?;
        // The command's own check, with the same rule as the plan and the dispatcher.
        let preview = self.with_store(|s| {
            flow::preview(
                s,
                &self.prepared(&loaded, &material, Trigger::Explicit, &wanted),
            )
        })?;
        if let Some(block) = preview.blocks.first() {
            return Err(start_error(StartError::Blocked(vec![block.clone()])));
        }
        let outcome = {
            let store = lock(&self.store);
            flow::publish(
                &store,
                &self.prepared(&loaded, &material, Trigger::Explicit, &wanted),
            )
            .map_err(start_error)?
        };
        self.wake();
        self.env.outbox_changed();
        let queued = outcome.ops.iter().filter(|(_, created)| *created).count();
        Ok(PublishResultDto {
            plan: self.plan_dto(&loaded, &outcome.plan)?,
            queued,
        })
    }

    /// Re-exports the tasks of a published meeting to Notion: mapped properties are updated on
    /// existing pages while keeping the status a person set in Notion, and trashed pages are
    /// not recreated.
    pub fn reexport_notion(&self, meeting_id: &str) -> IpcResult<PublishResultDto> {
        let loaded = self.load(meeting_id)?;
        if !self.with_store(|s| flow::is_locked(s, meeting_id))? {
            return Err(IpcError::new(
                "not_published",
                "only a published meeting can be re-exported to Notion: publish it first",
            ));
        }
        self.ensure_publishable(meeting_id, &loaded)?;
        let wanted = [PublishTarget::Notion];
        let material = self.material(&loaded)?;
        let preview = self.with_store(|s| {
            flow::preview(
                s,
                &self.prepared(&loaded, &material, Trigger::Explicit, &wanted),
            )
        })?;
        if let Some(block) = preview.blocks.first() {
            return Err(start_error(StartError::Blocked(vec![block.clone()])));
        }
        let outcome = {
            let store = lock(&self.store);
            flow::reexport_notion(
                &store,
                &self.prepared(&loaded, &material, Trigger::Explicit, &wanted),
            )
            .map_err(start_error)?
        };
        self.wake();
        self.env.outbox_changed();
        let queued = outcome.ops.iter().filter(|(_, created)| *created).count();
        Ok(PublishResultDto {
            plan: self.plan_dto(&loaded, &outcome.plan)?,
            queued,
        })
    }

    /// The `auto` rules for a draft that was just completed: what the cycle sets to `auto`
    /// is queued, never a status change, and only when nothing waits for the user (a newer
    /// draft, a repository that was not confirmed).
    pub fn auto_publish(&self, meeting_id: &str) -> IpcResult<usize> {
        let loaded = self.load(meeting_id)?;
        let rules = flow::target_rules(self.env.as_ref(), &loaded.ctx);
        let any_auto = [
            PublishTarget::Github,
            PublishTarget::Discord,
            PublishTarget::Notion,
        ]
        .into_iter()
        .any(|t| rules.allows(t, Trigger::Auto));
        if !any_auto {
            return Ok(0);
        }
        if loaded.new_draft_pending {
            tracing::info!(
                meeting = meeting_id,
                "automatic publishing waits for the newer draft"
            );
            return Ok(0);
        }
        let material = self.material(&loaded)?;
        let targets: &[PublishTarget] = &[];
        let preview = self.with_store(|s| {
            flow::preview(
                s,
                &self.prepared(&loaded, &material, Trigger::Auto, targets),
            )
        })?;
        if let Some(block) = preview.blocks.first() {
            tracing::info!(
                meeting = meeting_id,
                code = block.code(),
                "automatic publishing is waiting for the user"
            );
            return Ok(0);
        }
        if preview.ops.is_empty() {
            // Nothing is configured to go anywhere: no snapshot, no publication row.
            tracing::info!(
                meeting = meeting_id,
                "automatic publishing has nothing to send"
            );
            return Ok(0);
        }
        let outcome = {
            let store = lock(&self.store);
            flow::publish(
                &store,
                &self.prepared(&loaded, &material, Trigger::Auto, targets),
            )
            .map_err(start_error)?
        };
        self.env.outbox_changed();
        Ok(outcome.ops.iter().filter(|(_, created)| *created).count())
    }

    // ----- the public repository ----------------------------------------------------------------

    /// The user confirms publishing to the repository of a cycle. The operations the check
    /// had refused run again.
    pub fn confirm_public_repo(&self, cycle_id: &str) -> IpcResult<ConfirmDto> {
        let ctx = self
            .env
            .context(cycle_id)
            .map_err(|m| IpcError::new("not_found", m))?;
        let (visibility, released) =
            self.with_store(|s| flow::confirm_public_repo(s, self.env.as_ref(), &ctx))?;
        self.wake();
        self.env.outbox_changed();
        Ok(ConfirmDto {
            key: ctx.repo_key(),
            visibility: visibility.as_str(),
            released,
        })
    }

    // ----- the outbox ------------------------------------------------------------------------

    pub fn state(&self) -> IpcResult<OutboxStateDto> {
        let ops = self.with_store(|s| s.list_ops(None))?;
        Ok(OutboxStateDto {
            ops: ops.iter().map(Into::into).collect(),
            offline: self
                .dispatcher
                .offline_targets()
                .into_iter()
                .map(target_name)
                .collect(),
            now_unix_ms: self.env.now_ms(),
        })
    }

    /// Retries a failed operation (or, without an id, everything that failed or waits for
    /// its backoff) right away.
    pub fn retry(&self, id: Option<i64>) -> IpcResult<OutboxStateDto> {
        let now = self.env.now_ms();
        self.with_store(|s| match id {
            Some(id) => match s.get_op(id)? {
                Some(op) if op.state == OpState::Failed => s.retry_op(id, now),
                Some(op) if op.state == OpState::Queued => s.run_op_now(id, now),
                Some(_) => Err(store::StoreError::Invalid(format!(
                    "the operation {id} is not waiting"
                ))),
                None => Err(store::StoreError::Invalid(format!("no operation {id}"))),
            },
            None => s.retry_all_ops(now).map(|_| ()),
        })?;
        self.dispatcher.clear_offline();
        self.wake();
        self.state()
    }

    /// Drops a queued or failed operation. One that is running cannot be stopped.
    pub fn cancel(&self, id: i64) -> IpcResult<OutboxStateDto> {
        let op = self
            .with_store(|s| s.get_op(id))?
            .ok_or_else(|| IpcError::new("not_found", "this operation does not exist"))?;
        if op.state == OpState::Running {
            return Err(IpcError::new(
                "busy",
                "the operation is running; wait for it to finish",
            ));
        }
        self.with_store(|s| s.cancel_op(id, self.env.now_ms()))?;
        self.env.outbox_changed();
        self.state()
    }

    // ----- tasks -------------------------------------------------------------------------------

    /// The tasks of a cycle as the last read of its GitHub issues has them: open issues and
    /// the done ones (closed by a person or by an approved status change).
    pub fn cycle_tasks(&self, cycle_id: &str) -> IpcResult<CycleTasksDto> {
        let located = cycles::locate(&self.env.repos(), cycle_id)?.ok_or_else(|| {
            IpcError::new(
                "not_found",
                format!("the project {cycle_id} is not in any registered repository"),
            )
        })?;
        let mut dto = tasks_of(&located, cycle_id)?;
        let completions = self.with_store(|s| s.list_task_completions(cycle_id))?;
        apply_local_completions(&mut dto, &completions);
        Ok(dto)
    }
}

/// Moves the open issues the user ticked off in the application (kept in
/// `task_completions` under the key `#<number>`) to the done list.
fn apply_local_completions(dto: &mut CycleTasksDto, completions: &[store::TaskCompletion]) {
    let ticked = |number: u64| {
        let key = format!("#{number}");
        completions.iter().any(|c| c.done && c.task_key == key)
    };
    let (done_now, open): (Vec<_>, Vec<_>) = std::mem::take(&mut dto.open)
        .into_iter()
        .partition(|t| ticked(t.number));
    dto.open = open;
    dto.done.extend(done_now);
}

/// The tasks of a cycle from the issue cache next to its clone.
pub fn tasks_of(located: &Located, cycle_id: &str) -> IpcResult<CycleTasksDto> {
    let cache = located
        .repo
        .issue_cache_path(cycle_id)
        .map(|p| IssueCache::load(&p))
        .unwrap_or_default();
    let link_of = |number: u64| cache.links.iter().find(|l| l.number == number);
    // The text of an item that was made an issue, for the done list: the cache only knows
    // the numbers of closed issues.
    let item_text = |meeting: &str, item: &str| -> Option<String> {
        let dirs = cycles::previous_meeting_dirs(std::slice::from_ref(&located.setting), cycle_id);
        for dir in dirs {
            for entry in std::fs::read_dir(&dir).ok()?.flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path().join("meeting.json")) else {
                    continue;
                };
                let Ok(draft) = Draft::from_json_str(&text) else {
                    continue;
                };
                if draft.meeting_id == meeting {
                    return draft
                        .items
                        .iter()
                        .find(|i| i.id == item)
                        .map(|i| i.text.clone());
                }
            }
        }
        None
    };
    let open = cache
        .issues
        .iter()
        .map(|i| TaskDto {
            number: i.number,
            title: i.title.clone(),
            assignee: i.assignee.clone(),
            url: i.url.clone(),
            meeting: link_of(i.number).map(|l| l.meeting.clone()),
        })
        .collect();
    let done = cache
        .closed
        .iter()
        .map(|&number| {
            let link = link_of(number);
            let title = link
                .and_then(|l| item_text(&l.meeting, &l.item))
                .unwrap_or_else(|| format!("Issue #{number}"));
            TaskDto {
                number,
                title,
                assignee: None,
                url: None,
                meeting: link.map(|l| l.meeting.clone()),
            }
        })
        .collect();
    Ok(CycleTasksDto {
        cycle_id: cycle_id.to_string(),
        repo: github_identity(&located.setting)
            .ok()
            .map(|(r, _)| r.full_name()),
        as_of: cache.fetched_unix_ms.map(format_utc_date),
        open,
        done,
    })
}

mod corrections;
pub use corrections::{CorrectionsDto, PublishCorrectionsDto};

#[cfg(test)]
mod tests;
