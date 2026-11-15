//! A complete publishing setup on this machine for tests: a bare Git remote, a clone with the
//! cycle NOMAD, a fake GitHub with the repository `owner/rover`, a fake Discord webhook, a
//! store, and an [`Env`] over them with a clock the test moves by hand. Nothing here can
//! reach a real host: the "network" is loopback ports and the "remote" is a folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cycle_store::{
    render, Cycle, Draft, PublishMode, RenderOptions, Repo, Topic, TopicStatus, Variant,
    DEFAULT_TEMPLATE_TEXT,
};
use store::{Meeting, MeetingState, OutboxOp, Store};

use crate::discord::Pacing;
use crate::flow::{self, Outcome, Prepared, StartError};
use crate::github::RepoRef;
use crate::http::{redact, HttpClient};
use crate::outbox::{Dispatcher, Env, RepoContext};
use crate::plan::GithubIdentity;
use crate::rules::{PublishTarget, Trigger};
use crate::testing::fixtures::cycle as fixture_cycle;
use crate::testing::{FakeDiscord, FakeGithub, Visibility};

/// The GitHub token of the lab. Looks like a token, is not one.
pub const TOKEN: &str = "ghp_LabTokenValue0123456789abcdef";
/// The repository the lab publishes to.
pub const REPO: &str = "owner/rover";
/// The `cred:` reference of the Discord webhook.
pub const WEBHOOK_REF: &str = "cred:discord/nomad";
/// The meeting of the lab.
pub const MEETING: &str = "20261113-140501";

/// Runs git in `dir` with a fixed identity and no signing; panics on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// How the lab is set up.
#[derive(Debug, Clone, Copy)]
pub struct LabOptions {
    pub visibility: Visibility,
    pub github: PublishMode,
    pub discord: Option<PublishMode>,
    /// The clone has a remote (false: a local-only repository).
    pub remote: bool,
    /// The repository is linked to the GitHub repository (issues).
    pub linked: bool,
}

impl Default for LabOptions {
    fn default() -> Self {
        Self {
            visibility: Visibility::Private,
            github: PublishMode::OnApproval,
            discord: Some(PublishMode::OnApproval),
            remote: true,
            linked: true,
        }
    }
}

/// The environment of the lab.
pub struct LabEnv {
    clock: AtomicI64,
    root: PathBuf,
    repo: PathBuf,
    remote: PathBuf,
    api: String,
    remote_enabled: bool,
    linked: bool,
    pub token: Mutex<Option<String>>,
    pub webhook: Mutex<Option<String>>,
    /// Every pause the dispatcher asked for.
    pub sleeps: Mutex<Vec<Duration>>,
    /// How many times the outbox announced a change.
    pub changes: Mutex<u32>,
    /// The cycles whose files changed in the clone.
    pub repo_changes: Mutex<Vec<String>>,
    /// Called right before every push of a commit, with the attempt number: a test makes the
    /// remote change here, as a colleague would.
    pub push_hook: Mutex<Option<PushHook>>,
}

/// See [`LabEnv::push_hook`].
pub type PushHook = Box<dyn FnMut(u32) + Send>;

impl LabEnv {
    pub fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn clock(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn secrets(&self) -> Vec<String> {
        let mut v = vec![TOKEN.to_string()];
        if let Some(w) = self.webhook.lock().unwrap().clone() {
            if let Some(token) = w.rsplit('/').next() {
                v.push(token.to_string());
            }
            v.push(w);
        }
        v
    }
}

impl Env for LabEnv {
    fn now_ms(&self) -> i64 {
        self.clock()
    }

    fn context(&self, cycle_id: &str) -> Result<RepoContext, String> {
        let repo = Repo::open(&self.repo).map_err(|e| e.to_string())?;
        let cycle = repo.load_cycle(cycle_id).map_err(|e| e.to_string())?;
        Ok(RepoContext {
            repo,
            cycle,
            github: self.linked.then(|| GithubIdentity {
                repo: RepoRef::parse(REPO).unwrap(),
                api: self.api.clone(),
            }),
            remote_url: self
                .remote_enabled
                .then(|| self.remote.to_string_lossy().into_owned()),
            has_remote: self.remote_enabled,
        })
    }

    fn github_token(&self, _api: &str) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    fn webhook(&self, _reference: &str) -> Option<String> {
        self.webhook.lock().unwrap().clone()
    }

    fn snapshot_root(&self) -> PathBuf {
        self.root.clone()
    }

    fn client(&self) -> HttpClient {
        HttpClient::with_timeouts(Duration::from_secs(1), Duration::from_secs(5))
    }

    fn scrub(&self, text: &str) -> String {
        let secrets = self.secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        redact(text, &refs)
    }

    fn sleep(&self, duration: Duration) {
        self.sleeps.lock().unwrap().push(duration);
    }

    fn pacing(&self) -> Pacing {
        Pacing {
            max_inline_wait: Duration::from_secs(5),
            max_inline_retries: 2,
            sleep: Box::new(|_| {}),
        }
    }

    fn outbox_changed(&self) {
        *self.changes.lock().unwrap() += 1;
    }

    fn repo_changed(&self, cycle_id: &str) {
        self.repo_changes.lock().unwrap().push(cycle_id.to_string());
    }

    fn before_push(&self, attempt: u32) {
        if let Some(hook) = self.push_hook.lock().unwrap().as_mut() {
            hook(attempt);
        }
    }
}

pub struct Lab {
    pub tmp: tempfile::TempDir,
    /// The bare repository that plays the remote.
    pub remote: PathBuf,
    /// The clone the application publishes from.
    pub clone_dir: PathBuf,
    pub github: FakeGithub,
    pub discord: FakeDiscord,
    pub store: Arc<Mutex<Store>>,
    pub env: Arc<LabEnv>,
    pub dispatcher: Dispatcher,
    pub options: LabOptions,
    /// The database file of the store.
    pub db_path: PathBuf,
}

fn topic(id: &str, text: &str) -> Topic {
    Topic {
        id: id.into(),
        text: text.into(),
        status: TopicStatus::Open,
        opened_in: "2026-11-06".into(),
        closed_in: None,
    }
}

impl Lab {
    pub fn new(options: LabOptions) -> Lab {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(
            &remote,
            &["init", "--bare", "--quiet", "--initial-branch=main"],
        );

        let seed = Repo::init_local(tmp.path().join("seed")).unwrap();
        let discord_ref = options.discord.map(|mode| (WEBHOOK_REF, mode));
        seed.create_cycle(&fixture_cycle(Some(options.github), discord_ref))
            .unwrap();
        seed.save_topics(
            "NOMAD",
            &[topic("t-1", "Wheel size"), topic("t-2", "Battery")],
        )
        .unwrap();
        git(seed.root(), &["add", "-A"]);
        git(seed.root(), &["commit", "--quiet", "-m", "add NOMAD"]);
        git(
            seed.root(),
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(seed.root(), &["push", "--quiet", "origin", "main"]);

        let clone_dir = if options.remote {
            let repo = Repo::clone(remote.to_str().unwrap(), tmp.path().join("clone")).unwrap();
            repo.root().to_path_buf()
        } else {
            // A repository without a remote: the seed itself, with the remote taken away.
            git(seed.root(), &["remote", "remove", "origin"]);
            seed.root().to_path_buf()
        };

        let github = FakeGithub::start();
        github.add_repo(REPO, options.visibility);
        github.require_token(TOKEN);
        let discord = FakeDiscord::start();

        let root = tmp.path().join("recordings");
        std::fs::create_dir_all(root.join(MEETING).join("img")).unwrap();
        std::fs::write(
            root.join(MEETING).join("img/0001-00-12-41.png"),
            b"\x89PNG-lab",
        )
        .unwrap();

        // A file, so that a test can look for secrets in it.
        let db_path = tmp.path().join("blah-blah-bot.db");
        let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap()));
        store
            .lock()
            .unwrap()
            .create_meeting(&Meeting {
                id: MEETING.into(),
                title: "Weekly".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: MEETING.into(),
            })
            .unwrap();

        let env = Arc::new(LabEnv {
            clock: AtomicI64::new(1_000_000),
            root,
            repo: clone_dir.clone(),
            remote: remote.clone(),
            api: github.api_base(),
            remote_enabled: options.remote,
            linked: options.linked,
            token: Mutex::new(Some(TOKEN.to_string())),
            webhook: Mutex::new(Some(discord.webhook_url())),
            sleeps: Mutex::new(Vec::new()),
            changes: Mutex::new(0),
            repo_changes: Mutex::new(Vec::new()),
            push_hook: Mutex::new(None),
        });
        let dispatcher = Dispatcher::new(store.clone(), env.clone());
        Lab {
            tmp,
            remote,
            clone_dir,
            github,
            discord,
            store,
            env,
            dispatcher,
            options,
            db_path,
        }
    }

    pub fn repo(&self) -> Repo {
        Repo::open(&self.clone_dir).unwrap()
    }

    pub fn cycle(&self) -> Cycle {
        self.repo().load_cycle("NOMAD").unwrap()
    }

    /// Queues the publication of `draft` the way the application does.
    pub fn publish(
        &self,
        draft: &Draft,
        trigger: Trigger,
        targets: &[PublishTarget],
        anchors: &BTreeMap<String, String>,
    ) -> Result<Outcome, StartError> {
        let ctx = self.env.context("NOMAD").unwrap();
        let participants = ctx.cycle.participants.clone();
        let report = render(
            draft,
            &participants,
            DEFAULT_TEMPLATE_TEXT,
            &RenderOptions {
                title: format!("Meeting NOMAD {}", draft.date),
                variant: Variant::Markdown,
                ..RenderOptions::default()
            },
        )
        .expect("render the report");
        let images = vec![(
            "0001-00-12-41.png".to_string(),
            self.env.root.join(MEETING).join("img/0001-00-12-41.png"),
        )];
        let store = self.store.lock().unwrap();
        flow::publish(
            &store,
            &Prepared {
                env: self.env.as_ref(),
                meeting_id: draft_meeting_id(draft),
                meeting_rel: draft_meeting_id(draft),
                draft,
                ctx: &ctx,
                report_md: &report,
                transcript_md: Some(
                    "# Transcript\n\n`00:12:25` **Alex:** We will do it by Friday\n",
                ),
                images: &images,
                anchors,
                trigger,
                targets,
            },
        )
    }

    pub fn tick(&self) -> crate::outbox::TickReport {
        self.dispatcher.tick()
    }

    pub fn ops(&self) -> Vec<OutboxOp> {
        self.store.lock().unwrap().list_ops(None).unwrap()
    }

    pub fn op(&self, key: &str) -> OutboxOp {
        self.ops()
            .into_iter()
            .find(|o| o.op_key == key)
            .unwrap_or_else(|| panic!("no operation {key}"))
    }

    /// Operations by key and state name, in the order they were created.
    pub fn states(&self) -> Vec<(String, &'static str)> {
        self.ops()
            .into_iter()
            .map(|o| (o.op_key, o.state.as_str()))
            .collect()
    }

    /// Paths in the tip of the remote.
    pub fn remote_paths(&self) -> Vec<String> {
        git(&self.remote, &["ls-tree", "-r", "--name-only", "main"])
            .lines()
            .map(String::from)
            .collect()
    }

    pub fn remote_show(&self, path: &str) -> String {
        git(&self.remote, &["show", &format!("main:{path}")])
    }

    pub fn remote_commits(&self) -> usize {
        git(&self.remote, &["rev-list", "--count", "main"])
            .parse()
            .unwrap()
    }

    /// A colleague pushes a commit adding `files` to the remote.
    pub fn colleague_pushes(&self, name: &str, files: &[(&str, &str)]) {
        colleague_push(
            &self.remote,
            &self.tmp.path().join("colleagues"),
            name,
            files,
        );
    }

    /// A colleague pushes a new `topics.yaml` of the cycle.
    pub fn colleagues_push_topics(&self, yaml: &str) {
        self.colleague_pushes("topics", &[("cycles/NOMAD/topics.yaml", yaml)]);
    }

    /// The remote disappears (renamed), as when the network is gone for Git.
    pub fn remote_offline(&self) {
        std::fs::rename(&self.remote, self.remote.with_extension("away")).unwrap();
    }

    pub fn remote_online(&self) {
        std::fs::rename(self.remote.with_extension("away"), &self.remote).unwrap();
    }

    /// Runs the dispatcher, moving the clock past every backoff, until nothing is due or
    /// `max_ticks` passed. Returns how many ticks ran.
    pub fn run_until_settled(&self, max_ticks: usize) -> usize {
        for n in 1..=max_ticks {
            let report = self.tick();
            let due = self.dispatcher.next_due_ms();
            if report.ran() == 0 && due.is_none() {
                return n;
            }
            if let Some(at) = due {
                let now = self.env.clock();
                if at > now {
                    self.env.advance(at - now + 1);
                }
            }
        }
        max_ticks
    }
}

/// A colleague clones `remote` into `scratch/<name>`, adds `files` and pushes. Free of the lab
/// so that a push hook (which must be `'static`) can call it.
pub fn colleague_push(remote: &Path, scratch: &Path, name: &str, files: &[(&str, &str)]) {
    let dir = scratch.join(format!("colleague-{name}"));
    let repo = Repo::clone(remote.to_str().unwrap(), &dir).unwrap();
    for (path, text) in files {
        let target = repo.root().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, text).unwrap();
    }
    git(&dir, &["add", "-A"]);
    git(
        &dir,
        &["commit", "--quiet", "-m", &format!("colleague {name}")],
    );
    git(&dir, &["push", "--quiet", "origin", "main"]);
}

fn draft_meeting_id(draft: &Draft) -> &str {
    &draft.meeting_id
}

/// The draft of the lab meeting.
pub fn lab_draft(
    items: Vec<cycle_store::Item>,
    proposals: Vec<cycle_store::StatusProposal>,
) -> Draft {
    crate::testing::fixtures::draft(MEETING, items, proposals)
}
