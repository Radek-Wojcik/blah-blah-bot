//! Starting a publication: freezing what is published, queueing the planned operations and
//! the lock that keeps the draft as it was published.
//!
//! # The snapshot
//!
//! Publishing copies what goes out into `work/publish/<n>/` of the meeting folder: the draft
//! as `meeting.json` (with the decisions on the status changes), the rendered `report.md`,
//! `transcript.md` and the screenshots that are not inside an excluded passage. Every
//! operation reads from there, so a retry days later publishes what the user approved, not
//! what the draft looks like by then. The draft files of Stage 4 are not touched.
//!
//! # The lock
//!
//! From the moment the first operation of a meeting is queued, and until all of them are
//! cancelled, the draft is read-only: an edit, a change of mode or a decision on a status
//! change is refused with the code `published`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cycle_store::{Correction, Draft, Topic};
use store::{OpState, OutboxOp, Publication, Store, StoreError};

use crate::correction::{self, CorrectionPlan, CorrectionPlanInput};
use crate::gate::{self, RepoVisibility};
use crate::github::Github;
use crate::outbox::{Env, RepoContext};
use crate::plan::{
    self, Block, PlanInput, PublishPlan, RepoFacts, SnapshotRef, TopicsPlan, COMMIT_KEY,
};
use crate::rules::{PublishTarget, Trigger};

/// Folder of the snapshots below the meeting folder.
pub const SNAPSHOT_DIR: &str = "work/publish";

/// What goes into a snapshot.
pub struct SnapshotInput<'a> {
    pub draft: &'a Draft,
    pub report_md: &'a str,
    /// `None` when the meeting has no transcript.
    pub transcript_md: Option<&'a str>,
    /// `(name below img/, source file)`.
    pub images: &'a [(String, PathBuf)],
}

/// A frozen snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub n: u32,
    pub dir: PathBuf,
    /// Relative to `dir`, sorted.
    pub files: Vec<String>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// Writes the next snapshot of the meeting folder. Images whose name could break a path are
/// left out.
pub fn freeze(meeting_dir: &Path, input: &SnapshotInput<'_>) -> io::Result<Frozen> {
    let base = meeting_dir.join(SNAPSHOT_DIR);
    fs::create_dir_all(&base)?;
    let mut n = 1;
    while base.join(n.to_string()).exists() {
        n += 1;
    }
    let dir = base.join(n.to_string());
    let result = (|| -> io::Result<()> {
        let mut json =
            serde_json::to_vec_pretty(&input.draft.to_value()).map_err(io::Error::other)?;
        json.push(b'\n');
        write(&dir.join("meeting.json"), &json)?;
        write(&dir.join("report.md"), input.report_md.as_bytes())?;
        if let Some(t) = input.transcript_md {
            write(&dir.join("transcript.md"), t.as_bytes())?;
        }
        for (name, source) in input.images {
            if valid_name(name) {
                write(&dir.join("img").join(name), &fs::read(source)?)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    let files = list_files(&dir)?;
    Ok(Frozen { n, dir, files })
}

/// The files of a snapshot, relative and sorted, with `/` separators.
pub fn list_files(dir: &Path) -> io::Result<Vec<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, out)?;
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

// ----- the lock -------------------------------------------------------------------------------

/// Why the draft of a meeting is read-only, when it is: the operations that hold it.
pub fn lock_reason(store: &Store, meeting_id: &str) -> Result<Option<String>, StoreError> {
    let ops = store.list_ops(Some(meeting_id))?;
    let live = ops.iter().filter(|o| o.state != OpState::Cancelled).count();
    Ok((live > 0).then(|| {
        "this meeting is published (or queued for publishing), so its draft is read-only; cancel its operations in the outbox to change it".to_string()
    }))
}

/// Whether the draft of a meeting is read-only.
pub fn is_locked(store: &Store, meeting_id: &str) -> Result<bool, StoreError> {
    Ok(lock_reason(store, meeting_id)?.is_some())
}

// ----- starting -------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{}", .0.first().map_or_else(String::new, Block::message))]
    Blocked(Vec<Block>),
    #[error("the files of the publication cannot be written: {0}")]
    Files(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What a started publication needs besides its plan.
pub struct Start<'a> {
    pub meeting_id: &'a str,
    pub cycle_id: &'a str,
    /// Where the snapshot is, relative to the snapshot root.
    pub snapshot_dir: &'a str,
    pub repo_key: Option<String>,
}

/// Queues the operations of a plan and records the publication. A blocked plan queues
/// nothing. Returns each operation and whether it was new (an operation that is queued
/// already, or done, is left as it is).
pub fn start(
    store: &Store,
    start: &Start<'_>,
    plan: &PublishPlan,
    now_ms: i64,
) -> Result<Vec<(OutboxOp, bool)>, StartError> {
    if plan.is_blocked() {
        return Err(StartError::Blocked(plan.blocks.clone()));
    }
    let live = store
        .list_ops(Some(start.meeting_id))?
        .iter()
        .any(|o| o.state != OpState::Cancelled);
    let current = store.get_publication(start.meeting_id)?;
    // A publication whose operations were all cancelled starts over with its new snapshot;
    // one that is still in progress keeps the snapshot it has.
    let replace = match &current {
        None => true,
        Some(row) => !live && row.snapshot_dir != start.snapshot_dir,
    };
    if replace {
        store.put_publication(&Publication {
            meeting_id: start.meeting_id.to_string(),
            cycle_id: start.cycle_id.to_string(),
            folder: None,
            repo_key: start.repo_key.clone(),
            commit_sha: None,
            report_url: None,
            snapshot_dir: start.snapshot_dir.to_string(),
            published_unix_ms: None,
        })?;
    }
    let mut out = Vec::new();
    for op in &plan.ops {
        out.push(store.enqueue_op(op, now_ms)?);
    }
    Ok(out)
}

// ----- from a meeting to queued operations ---------------------------------------------------

/// What the application knows about a meeting when the user publishes it (or when its draft
/// is complete).
pub struct Prepared<'a> {
    pub env: &'a dyn Env,
    pub meeting_id: &'a str,
    /// The meeting folder, relative to the snapshot root of the environment.
    pub meeting_rel: &'a str,
    pub draft: &'a Draft,
    pub ctx: &'a RepoContext,
    /// The report rendered for GitHub.
    pub report_md: &'a str,
    pub transcript_md: Option<&'a str>,
    /// `(name below img/, source file)`, the screenshots that may be published.
    pub images: &'a [(String, PathBuf)],
    /// Anchor in the published transcript, by item id and proposal id.
    pub anchors: &'a BTreeMap<String, String>,
    pub trigger: Trigger,
    pub targets: &'a [PublishTarget],
}

/// What the repository is like, as far as the plan and the check need to know: where issues
/// go, whether a remote receives the commit, its visibility (asked of GitHub, never
/// guessed) and whether the user confirmed it.
pub fn repo_facts(
    env: &dyn Env,
    store: &Store,
    ctx: &RepoContext,
) -> Result<RepoFacts, StoreError> {
    let visibility = match &ctx.github {
        Some(g) if ctx.has_remote => {
            let token = env.github_token(&g.api).unwrap_or_default();
            match Github::with_client(env.client(), &g.api, &token) {
                Ok(github) => gate::probe(&github, &g.repo),
                Err(_) => RepoVisibility::Unknown,
            }
        }
        // Nothing leaves a repository without a remote.
        _ if !ctx.has_remote => RepoVisibility::NotPublic,
        // A remote that is not on GitHub cannot be asked.
        _ => RepoVisibility::Unknown,
    };
    let confirmed = match ctx.repo_key() {
        Some(key) => store.get_repo_confirmation(&key)?.is_some(),
        None => false,
    };
    Ok(RepoFacts {
        github: ctx.github.clone(),
        has_remote: ctx.has_remote,
        visibility,
        confirmed,
    })
}

fn plan_input<'a>(
    p: &'a Prepared<'a>,
    facts: &'a RepoFacts,
    snapshot: &'a SnapshotRef,
    existing: &'a [OutboxOp],
    topics: &'a [cycle_store::Topic],
) -> PlanInput<'a> {
    let webhook = p
        .ctx
        .cycle
        .targets
        .discord
        .as_ref()
        .is_some_and(|d| p.env.webhook(&d.webhook).is_some());
    PlanInput {
        meeting_id: p.meeting_id,
        draft: p.draft,
        cycle: &p.ctx.cycle,
        local_topics: topics,
        repo: facts,
        discord_webhook_set: webhook,
        anchors: p.anchors,
        snapshot,
        existing,
        trigger: p.trigger,
        targets: p.targets,
    }
}

/// Adds to the plan what the commit does to `topics.yaml`, topic by topic, from the clone and
/// the remote branch as last fetched. When that cannot be told the plan says so and the screen
/// has only the topics' names (`topics_added`, `topics_set`).
fn attach_topics(p: &Prepared<'_>, local: &[Topic], plan: &mut PublishPlan) {
    let Some(commit) = plan.commit_payload() else {
        return;
    };
    // Without a topic of the meeting the commit leaves the file alone.
    if commit.topics_add.is_empty() && commit.topics_set.is_empty() {
        return;
    }
    let ours = plan::apply_topics(local, &commit.topics_add, &commit.topics_set, &commit.date);
    match cycle_store::preview_topics(&p.ctx.repo, &p.ctx.cycle.id, &ours) {
        Ok(preview) => {
            if p.ctx.has_remote && !preview.against_remote {
                plan.notes.push(
                    "topics.yaml is compared with this clone only, because nothing was fetched from the remote yet: what others changed there shows up when you publish"
                        .into(),
                );
            }
            plan.summary.topics = Some(TopicsPlan::of(&preview.merge, preview.against_remote));
        }
        Err(_) => plan.notes.push(
            "topics.yaml could not be compared with the repository: the topics are listed by name only"
                .into(),
        ),
    }
}

/// What publishing would do, without freezing or queueing anything (the screen that asks
/// before it does). Asks GitHub for the visibility of the repository.
pub fn preview(store: &Store, p: &Prepared<'_>) -> Result<PublishPlan, StoreError> {
    let facts = repo_facts(p.env, store, p.ctx)?;
    let existing = store.list_ops(Some(p.meeting_id))?;
    let topics = p.ctx.repo.load_topics(&p.ctx.cycle.id).unwrap_or_default();
    let snapshot = SnapshotRef::default();
    let mut plan = plan::build(&plan_input(p, &facts, &snapshot, &existing, &topics));
    attach_topics(p, &topics, &mut plan);
    Ok(plan)
}

/// What a started publication did.
pub struct Outcome {
    pub plan: PublishPlan,
    /// Each operation and whether it is new.
    pub ops: Vec<(OutboxOp, bool)>,
    /// The snapshot the operations read from, relative to the snapshot root.
    pub snapshot_dir: String,
}

/// Freezes the meeting (or reuses the snapshot of a publication in progress), plans, checks
/// and queues. A refused plan leaves nothing behind: no operation, no publication, and a
/// snapshot made for it is removed.
pub fn publish(store: &Store, p: &Prepared<'_>) -> Result<Outcome, StartError> {
    let root = p.env.snapshot_root();
    let existing = store.list_ops(Some(p.meeting_id))?;
    let live = existing.iter().any(|o| o.state != OpState::Cancelled);
    let publication = store.get_publication(p.meeting_id)?;
    let (snapshot, fresh) = match (live, publication) {
        (true, Some(row)) => {
            let files = list_files(&root.join(&row.snapshot_dir))
                .map_err(|e| StartError::Files(e.to_string()))?;
            (
                SnapshotRef {
                    dir: row.snapshot_dir,
                    files,
                },
                None,
            )
        }
        _ => {
            let frozen = freeze(
                &root.join(p.meeting_rel),
                &SnapshotInput {
                    draft: p.draft,
                    report_md: p.report_md,
                    transcript_md: p.transcript_md,
                    images: p.images,
                },
            )
            .map_err(|e| StartError::Files(e.to_string()))?;
            (
                SnapshotRef {
                    dir: format!(
                        "{}/{SNAPSHOT_DIR}/{}",
                        p.meeting_rel.trim_end_matches('/'),
                        frozen.n
                    ),
                    files: frozen.files,
                },
                Some(frozen.dir),
            )
        }
    };
    let facts = repo_facts(p.env, store, p.ctx)?;
    let topics = p.ctx.repo.load_topics(&p.ctx.cycle.id).unwrap_or_default();
    let mut plan = plan::build(&plan_input(p, &facts, &snapshot, &existing, &topics));
    attach_topics(p, &topics, &mut plan);
    let started = start(
        store,
        &Start {
            meeting_id: p.meeting_id,
            cycle_id: &p.ctx.cycle.id,
            snapshot_dir: &snapshot.dir,
            repo_key: p.ctx.repo_key(),
        },
        &plan,
        p.env.now_ms(),
    );
    match started {
        Ok(ops) => Ok(Outcome {
            plan,
            ops,
            snapshot_dir: snapshot.dir,
        }),
        Err(e) => {
            if let Some(dir) = fresh {
                let _ = fs::remove_dir_all(dir);
            }
            Err(e)
        }
    }
}

// ----- corrections ---------------------------------------------------------------------------

/// What the application knows when a correction is published (or when it is written under an
/// `auto` rule).
pub struct PreparedCorrection<'a> {
    pub meeting_id: &'a str,
    /// The meeting folder, relative to the snapshot root of the environment.
    pub meeting_rel: &'a str,
    /// `YYYY-MM-DD`, the day of the meeting that is corrected.
    pub meeting_date: &'a str,
    pub ctx: &'a RepoContext,
    pub correction: &'a Correction,
    pub trigger: Trigger,
}

/// Where the rendered correction files are written, relative to the snapshot root.
pub fn correction_out_dir(meeting_rel: &str) -> String {
    format!("{}/work/corrections-out", meeting_rel.trim_end_matches('/'))
}

fn correction_input<'a>(
    p: &'a PreparedCorrection<'a>,
    facts: &'a RepoFacts,
    existing: &'a [OutboxOp],
    out_dir: &'a str,
    in_repository: bool,
) -> CorrectionPlanInput<'a> {
    CorrectionPlanInput {
        meeting_id: p.meeting_id,
        meeting_date: p.meeting_date,
        cycle: &p.ctx.cycle,
        correction: p.correction,
        repo: facts,
        existing,
        trigger: p.trigger,
        out_dir,
        in_repository,
    }
}

/// The meeting is in the repository (its commit landed) or its commit is still on its way.
fn meeting_in_repository(
    store: &Store,
    meeting_id: &str,
    existing: &[OutboxOp],
) -> Result<bool, StoreError> {
    let landed = store
        .get_publication(meeting_id)?
        .is_some_and(|row| row.folder.is_some());
    let on_its_way = existing
        .iter()
        .any(|o| o.op_key == COMMIT_KEY && o.state != OpState::Cancelled);
    Ok(landed || on_its_way)
}

/// What publishing the correction would do, without queueing anything. Asks GitHub for the
/// visibility of the repository.
pub fn preview_correction(
    env: &dyn Env,
    store: &Store,
    p: &PreparedCorrection<'_>,
) -> Result<CorrectionPlan, StoreError> {
    let facts = repo_facts(env, store, p.ctx)?;
    let existing = store.list_ops(Some(p.meeting_id))?;
    let in_repository = meeting_in_repository(store, p.meeting_id, &existing)?;
    let out_dir = correction_out_dir(p.meeting_rel);
    Ok(correction::build(&correction_input(
        p,
        &facts,
        &existing,
        &out_dir,
        in_repository,
    )))
}

/// What a queued correction did.
pub struct CorrectionOutcome {
    pub plan: CorrectionPlan,
    /// Each operation and whether it is new.
    pub ops: Vec<(OutboxOp, bool)>,
}

/// Plans and queues the operations of a correction. A refused plan queues nothing. Queueing
/// the same correction twice changes nothing; one the user cancelled is queued again.
pub fn publish_correction(
    env: &dyn Env,
    store: &Store,
    p: &PreparedCorrection<'_>,
) -> Result<CorrectionOutcome, StartError> {
    let plan = preview_correction(env, store, p)?;
    if plan.is_blocked() {
        return Err(StartError::Blocked(plan.blocks.clone()));
    }
    let mut ops = Vec::new();
    for op in &plan.ops {
        ops.push(store.enqueue_op(op, env.now_ms())?);
    }
    Ok(CorrectionOutcome { plan, ops })
}

/// Records that the user confirmed publishing to the repository, with the visibility it has
/// now, and lets the operations the check had refused run again. Returns the visibility and
/// the number of operations that were released.
pub fn confirm_public_repo(
    store: &Store,
    env: &dyn Env,
    ctx: &RepoContext,
) -> Result<(RepoVisibility, usize), StoreError> {
    let Some(key) = ctx.repo_key() else {
        return Ok((RepoVisibility::NotPublic, 0));
    };
    let facts = repo_facts(env, store, ctx)?;
    store.confirm_repo(&key, facts.visibility.as_str(), env.now_ms())?;
    let mut released = 0;
    for op in store.list_ops(None)? {
        let refused = op.state == OpState::Failed
            && op
                .last_error
                .as_deref()
                .is_some_and(|e| e.starts_with(gate::PUBLIC_REPO_CODE));
        if refused {
            store.retry_op(op.id, env.now_ms())?;
            released += 1;
        }
    }
    Ok((facts.visibility, released))
}

#[cfg(test)]
mod tests;
