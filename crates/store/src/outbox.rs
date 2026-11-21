//! The outbox: durable queue of outgoing publishing operations.
//!
//! The store records facts and enforces the state machine; what an operation does (a git
//! commit, an issue, a Discord message) belongs to the `publishers` crate. Payloads and
//! results are opaque JSON text and never hold secrets. Times are passed in, so tests need
//! no clock.
//!
//! ```text
//! queued --claim--> running --done--> done
//!   ^                  |  \--permanent--> failed --retry--> queued
//!   \--transient-------/                     \--cancel--> cancelled
//! queued --cancel--> cancelled
//! ```

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::{Result, Store, StoreError};

/// An operation that has failed this many times in a row is not retried by itself.
pub const MAX_ATTEMPTS: u32 = 8;
/// Delay before the first retry of a transient failure.
pub const BACKOFF_BASE_MS: i64 = 30_000;
/// Longest delay between two automatic attempts.
pub const BACKOFF_MAX_MS: i64 = 30 * 60_000;
/// Longest wait a server can ask for with `Retry-After` or a rate-limit reset.
pub const RETRY_AFTER_MAX_MS: i64 = 60 * 60_000;

/// Where an operation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxTarget {
    /// The cycle repository (a commit pushed to the remote).
    Github,
    /// GitHub issues of the repository.
    Issues,
    /// A Discord webhook or bot.
    Discord,
    /// A Notion database.
    Notion,
}

impl OutboxTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Issues => "issues",
            Self::Discord => "discord",
            Self::Notion => "notion",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "github" => Ok(Self::Github),
            "issues" => Ok(Self::Issues),
            "discord" => Ok(Self::Discord),
            "notion" => Ok(Self::Notion),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }
}

/// What an operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxKind {
    CommitMeeting,
    CreateIssue,
    SetIssueState,
    PostDiscord,
    /// A correction note added to the folder of a published meeting.
    CommitCorrection,
    /// A comment with a correction on the issue of an item.
    CommentCorrection,
    /// A task exported to a Notion database as a page.
    ExportNotion,
    /// The Discord message of a meeting edited after a correction.
    EditDiscord,
}

impl OutboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommitMeeting => "commit_meeting",
            Self::CreateIssue => "create_issue",
            Self::SetIssueState => "set_issue_state",
            Self::PostDiscord => "post_discord",
            Self::CommitCorrection => "commit_correction",
            Self::CommentCorrection => "comment_correction",
            Self::ExportNotion => "export_notion",
            Self::EditDiscord => "edit_discord",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "commit_meeting" => Ok(Self::CommitMeeting),
            "create_issue" => Ok(Self::CreateIssue),
            "set_issue_state" => Ok(Self::SetIssueState),
            "post_discord" => Ok(Self::PostDiscord),
            "commit_correction" => Ok(Self::CommitCorrection),
            "comment_correction" => Ok(Self::CommentCorrection),
            "export_notion" => Ok(Self::ExportNotion),
            "edit_discord" => Ok(Self::EditDiscord),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }
}

/// State of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    /// Waiting for its turn, or for its retry time.
    Queued,
    /// Claimed by the dispatcher.
    Running,
    /// Finished; `result` holds the outcome.
    Done,
    /// Gave up; needs the user to retry or cancel.
    Failed,
    /// Dropped by the user.
    Cancelled,
}

impl OpState {
    pub const ALL: [OpState; 5] = [
        Self::Queued,
        Self::Running,
        Self::Done,
        Self::Failed,
        Self::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }

    /// Whether the machine has an edge from `self` to `to`. `Done` and `Cancelled` are final.
    pub fn can_become(self, to: OpState) -> bool {
        matches!(
            (self, to),
            (Self::Queued, Self::Running)
                | (Self::Queued, Self::Cancelled)
                | (Self::Running, Self::Done)
                | (Self::Running, Self::Queued)
                | (Self::Running, Self::Failed)
                | (Self::Failed, Self::Queued)
                | (Self::Failed, Self::Cancelled)
        )
    }
}

/// Why an attempt failed, as far as the dispatcher can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Network trouble, a 5xx, a rate limit, a rejected push: worth another attempt.
    /// `retry_after_ms` is what the server asked for, when it did.
    Transient { retry_after_ms: Option<i64> },
    /// Bad configuration or a refusal that waiting will not fix (400, 401, 403, 404, 422).
    Permanent,
}

/// Delay before the next attempt after `failed_before` earlier failures (0 for the first):
/// 30 s, 1 min, 2 min, ... capped at 30 min. A server-provided wait replaces the backoff,
/// within 1 s and one hour.
pub fn retry_delay_ms(failed_before: u32, retry_after_ms: Option<i64>) -> i64 {
    if let Some(ms) = retry_after_ms {
        return ms.clamp(1_000, RETRY_AFTER_MAX_MS);
    }
    let shift = failed_before.min(16);
    (BACKOFF_BASE_MS << shift).min(BACKOFF_MAX_MS)
}

/// An operation to enqueue.
#[derive(Debug, Clone, PartialEq)]
pub struct NewOp {
    pub meeting_id: String,
    pub target: OutboxTarget,
    pub kind: OutboxKind,
    /// Idempotency key within the meeting, for example `commit`, `issue:i3`, `discord`.
    pub op_key: String,
    /// JSON, never secrets.
    pub payload: String,
    /// `op_key` of another operation of the same meeting that must be done first.
    pub after_op_key: Option<String>,
}

/// An outbox row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxOp {
    pub id: i64,
    pub meeting_id: String,
    pub target: OutboxTarget,
    pub kind: OutboxKind,
    pub op_key: String,
    pub payload: String,
    pub state: OpState,
    pub attempts: u32,
    pub next_attempt_unix_ms: Option<i64>,
    pub last_error: Option<String>,
    /// JSON outcome of a done operation (issue number and url, commit sha, sent chunks).
    pub result: Option<String>,
    pub after_id: Option<i64>,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
}

const COLUMNS: &str = "id, meeting_id, target, kind, op_key, payload, state, attempts,
    next_attempt_unix_ms, last_error, result, after_id, created_unix_ms, updated_unix_ms";

fn op_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<OutboxOp>> {
    let target: String = r.get(2)?;
    let kind: String = r.get(3)?;
    let state: String = r.get(6)?;
    let rest = (
        r.get(0)?,
        r.get(1)?,
        r.get(4)?,
        r.get(5)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
    );
    Ok((|| {
        Ok(OutboxOp {
            id: rest.0,
            meeting_id: rest.1,
            target: OutboxTarget::parse(&target)?,
            kind: OutboxKind::parse(&kind)?,
            op_key: rest.2,
            payload: rest.3,
            state: OpState::parse(&state)?,
            attempts: rest.4,
            next_attempt_unix_ms: rest.5,
            last_error: rest.6,
            result: rest.7,
            after_id: rest.8,
            created_unix_ms: rest.9,
            updated_unix_ms: rest.10,
        })
    })())
}

fn fetch(tx: &Transaction<'_>, id: i64) -> Result<OutboxOp> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM outbox WHERE id = ?1"),
        params![id],
        op_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::Invalid(format!("no outbox operation {id}")))?
}

/// Load the operation and check that the machine allows `to`.
fn checked(tx: &Transaction<'_>, id: i64, to: OpState) -> Result<OutboxOp> {
    let op = fetch(tx, id)?;
    if !op.state.can_become(to) {
        return Err(StoreError::Invalid(format!(
            "outbox operation {id} cannot go from {} to {}",
            op.state.as_str(),
            to.as_str()
        )));
    }
    Ok(op)
}

impl Store {
    /// Add an operation. When the meeting already has one with the same `op_key` nothing
    /// changes and the existing row is returned with `false`: enqueueing twice is safe. The
    /// exception is a cancelled one: the user changed their mind, so the row starts over
    /// (queued, no attempts, the new payload) and is returned with `true`.
    pub fn enqueue_op(&self, new: &NewOp, now_ms: i64) -> Result<(OutboxOp, bool)> {
        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM outbox WHERE meeting_id = ?1 AND op_key = ?2",
                params![new.meeting_id, new.op_key],
                |r| r.get(0),
            )
            .optional()?;
        let after_id = match &new.after_op_key {
            None => None,
            Some(key) => Some(
                tx.query_row(
                    "SELECT id FROM outbox WHERE meeting_id = ?1 AND op_key = ?2",
                    params![new.meeting_id, key],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    StoreError::Invalid(format!("operation {key} to wait for does not exist"))
                })?,
            ),
        };
        if let Some(id) = existing {
            let op = fetch(&tx, id)?;
            if op.state != OpState::Cancelled {
                return Ok((op, false));
            }
            tx.execute(
                "UPDATE outbox SET state = 'queued', payload = ?2, after_id = ?3, attempts = 0,
                    next_attempt_unix_ms = NULL, last_error = NULL, result = NULL,
                    updated_unix_ms = ?4 WHERE id = ?1",
                params![id, new.payload, after_id, now_ms],
            )?;
            let op = fetch(&tx, id)?;
            tx.commit()?;
            return Ok((op, true));
        }
        tx.execute(
            "INSERT INTO outbox (meeting_id, target, kind, op_key, payload, state, after_id,
                                 created_unix_ms, updated_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?7)",
            params![
                new.meeting_id,
                new.target.as_str(),
                new.kind.as_str(),
                new.op_key,
                new.payload,
                after_id,
                now_ms
            ],
        )?;
        let id = tx.last_insert_rowid();
        let op = fetch(&tx, id)?;
        tx.commit()?;
        Ok((op, true))
    }

    pub fn get_op(&self, id: i64) -> Result<Option<OutboxOp>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM outbox WHERE id = ?1"),
                params![id],
                op_row,
            )
            .optional()?
            .transpose()
    }

    pub fn get_op_by_key(&self, meeting_id: &str, op_key: &str) -> Result<Option<OutboxOp>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM outbox WHERE meeting_id = ?1 AND op_key = ?2"),
                params![meeting_id, op_key],
                op_row,
            )
            .optional()?
            .transpose()
    }

    /// Operations in the order they were created, all of them or those of one meeting.
    pub fn list_ops(&self, meeting_id: Option<&str>) -> Result<Vec<OutboxOp>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM outbox WHERE (?1 IS NULL OR meeting_id = ?1) ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![meeting_id], op_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Take the next operation that may run now and mark it running. An operation is due
    /// when it is queued, its retry time has come and the operation it waits for is done.
    /// Operations for a target in `skip` stay queued (a host that just refused to connect).
    /// The oldest due operation goes first.
    ///
    /// A Discord post (`post_discord` only, not an edit of it) also waits while an issue
    /// operation of the same meeting is queued or running, so that the message can link the
    /// issues that were created; an issue that failed for good or was cancelled does not
    /// hold it back.
    pub fn claim_next_op(&self, now_ms: i64, skip: &[OutboxTarget]) -> Result<Option<OutboxOp>> {
        let tx = self.conn.unchecked_transaction()?;
        let due: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT o.id, o.target FROM outbox o
                 WHERE o.state = 'queued'
                   AND (o.next_attempt_unix_ms IS NULL OR o.next_attempt_unix_ms <= ?1)
                   AND (o.after_id IS NULL
                        OR EXISTS (SELECT 1 FROM outbox a WHERE a.id = o.after_id AND a.state = 'done'))
                   AND (o.kind <> 'post_discord'
                        OR NOT EXISTS (SELECT 1 FROM outbox i
                                       WHERE i.meeting_id = o.meeting_id AND i.target = 'issues'
                                         AND i.state IN ('queued', 'running')))
                 ORDER BY o.id",
            )?;
            let rows = stmt.query_map(params![now_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for (id, target) in due {
            if skip.contains(&OutboxTarget::parse(&target)?) {
                continue;
            }
            checked(&tx, id, OpState::Running)?;
            tx.execute(
                "UPDATE outbox SET state = 'running', next_attempt_unix_ms = NULL,
                    updated_unix_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
            let op = fetch(&tx, id)?;
            tx.commit()?;
            return Ok(Some(op));
        }
        Ok(None)
    }

    /// Record how far a running operation has come (JSON, never secrets), for example the
    /// number of Discord messages that are out. It is kept through failures and retries and
    /// replaced by the outcome when the operation completes.
    pub fn set_op_progress(&self, id: i64, progress: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE outbox SET result = ?2, updated_unix_ms = ?3
             WHERE id = ?1 AND state = 'running'",
            params![id, progress, now_ms],
        )?;
        if changed == 0 {
            return Err(StoreError::Invalid(format!(
                "outbox operation {id} is not running"
            )));
        }
        Ok(())
    }

    /// running -> done, with the outcome as JSON.
    pub fn complete_op(&self, id: i64, result: &str, now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        checked(&tx, id, OpState::Done)?;
        tx.execute(
            "UPDATE outbox SET state = 'done', result = ?2, last_error = NULL,
                next_attempt_unix_ms = NULL, updated_unix_ms = ?3 WHERE id = ?1",
            params![id, result, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// running -> queued (a transient failure, with the next attempt scheduled) or
    /// running -> failed (a permanent one, or too many attempts). Returns the new state.
    /// `error` must already be free of secrets.
    pub fn fail_op(&self, id: i64, failure: Failure, error: &str, now_ms: i64) -> Result<OpState> {
        let tx = self.conn.unchecked_transaction()?;
        let op = checked(&tx, id, OpState::Failed)?;
        let attempts = op.attempts + 1;
        let (state, next) = match failure {
            Failure::Transient { retry_after_ms } if attempts < MAX_ATTEMPTS => (
                OpState::Queued,
                Some(now_ms + retry_delay_ms(op.attempts, retry_after_ms)),
            ),
            _ => (OpState::Failed, None),
        };
        tx.execute(
            "UPDATE outbox SET state = ?2, attempts = ?3, next_attempt_unix_ms = ?4,
                last_error = ?5, updated_unix_ms = ?6 WHERE id = ?1",
            params![id, state.as_str(), attempts, next, error, now_ms],
        )?;
        tx.commit()?;
        Ok(state)
    }

    /// failed -> queued for the user's retry: attempts start again and it is due now.
    pub fn retry_op(&self, id: i64, now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        // running -> queued exists for the dispatcher, but a user retry is only for failed.
        if checked(&tx, id, OpState::Queued)?.state != OpState::Failed {
            return Err(StoreError::Invalid(format!(
                "outbox operation {id} has not failed"
            )));
        }
        tx.execute(
            "UPDATE outbox SET state = 'queued', attempts = 0, next_attempt_unix_ms = NULL,
                updated_unix_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Makes a queued operation that waits for its backoff due now (the user does not want to
    /// wait). Anything else is refused.
    pub fn run_op_now(&self, id: i64, now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        if fetch(&tx, id)?.state != OpState::Queued {
            return Err(StoreError::Invalid(format!(
                "outbox operation {id} is not queued"
            )));
        }
        tx.execute(
            "UPDATE outbox SET next_attempt_unix_ms = NULL, updated_unix_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Retry everything now: failed operations go back to queued with fresh attempts, and
    /// queued ones that wait for their backoff become due. Returns how many rows changed.
    pub fn retry_all_ops(&self, now_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let failed = tx.execute(
            "UPDATE outbox SET state = 'queued', attempts = 0, next_attempt_unix_ms = NULL,
                updated_unix_ms = ?1 WHERE state = 'failed'",
            params![now_ms],
        )?;
        let waiting = tx.execute(
            "UPDATE outbox SET next_attempt_unix_ms = NULL, updated_unix_ms = ?1
             WHERE state = 'queued' AND next_attempt_unix_ms IS NOT NULL",
            params![now_ms],
        )?;
        tx.commit()?;
        Ok(failed + waiting)
    }

    /// queued or failed -> cancelled. A running operation cannot be cancelled.
    pub fn cancel_op(&self, id: i64, now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        checked(&tx, id, OpState::Cancelled)?;
        tx.execute(
            "UPDATE outbox SET state = 'cancelled', next_attempt_unix_ms = NULL,
                updated_unix_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Called at start: no operation can be running in a fresh process, so these are crash
    /// leftovers and go back to queued (the attempt is not counted). Returns their ids.
    pub fn reset_running_ops(&self, now_ms: i64) -> Result<Vec<i64>> {
        let tx = self.conn.unchecked_transaction()?;
        let ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT id FROM outbox WHERE state = 'running' ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in &ids {
            checked(&tx, *id, OpState::Queued)?;
            tx.execute(
                "UPDATE outbox SET state = 'queued', next_attempt_unix_ms = NULL,
                    updated_unix_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Earliest retry time among queued operations that wait for their backoff, to sleep
    /// until; `None` when nothing waits.
    pub fn next_retry_unix_ms(&self) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT MIN(next_attempt_unix_ms) FROM outbox
             WHERE state = 'queued' AND next_attempt_unix_ms IS NOT NULL",
            [],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, MeetingState};

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        for id in ["m1", "m2"] {
            s.create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
        }
        s
    }

    fn new_op(meeting: &str, target: OutboxTarget, kind: OutboxKind, key: &str) -> NewOp {
        NewOp {
            meeting_id: meeting.into(),
            target,
            kind,
            op_key: key.into(),
            payload: "{}".into(),
            after_op_key: None,
        }
    }

    fn commit(meeting: &str) -> NewOp {
        new_op(
            meeting,
            OutboxTarget::Github,
            OutboxKind::CommitMeeting,
            "commit",
        )
    }

    fn transient() -> Failure {
        Failure::Transient {
            retry_after_ms: None,
        }
    }

    #[test]
    fn the_transition_table_has_exactly_the_documented_edges() {
        use OpState::*;
        let legal = [
            (Queued, Running),
            (Queued, Cancelled),
            (Running, Done),
            (Running, Queued),
            (Running, Failed),
            (Failed, Queued),
            (Failed, Cancelled),
        ];
        for from in OpState::ALL {
            for to in OpState::ALL {
                assert_eq!(
                    from.can_become(to),
                    legal.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn state_names_round_trip_and_match_the_check_constraint() {
        let s = store();
        for state in OpState::ALL {
            assert_eq!(OpState::parse(state.as_str()).unwrap(), state);
            // The names are the ones the CHECK constraint accepts.
            s.conn
                .execute(
                    "INSERT INTO outbox (meeting_id, target, kind, op_key, state,
                                         created_unix_ms, updated_unix_ms)
                     VALUES ('m1', 'github', 'commit_meeting', ?1, ?1, 1, 1)",
                    params![state.as_str()],
                )
                .unwrap();
        }
        assert!(s
            .conn
            .execute(
                "INSERT INTO outbox (meeting_id, target, kind, op_key, state,
                                     created_unix_ms, updated_unix_ms)
                 VALUES ('m1', 'github', 'commit_meeting', 'x', 'paused', 1, 1)",
                [],
            )
            .is_err());
        assert!(OpState::parse("paused").is_err());
    }

    #[test]
    fn the_backoff_doubles_up_to_the_cap() {
        let delays: Vec<i64> = (0..8).map(|n| retry_delay_ms(n, None)).collect();
        assert_eq!(
            delays,
            [30_000, 60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000]
        );
        assert_eq!(retry_delay_ms(1_000, None), BACKOFF_MAX_MS);
    }

    #[test]
    fn a_server_provided_wait_replaces_the_backoff_within_limits() {
        assert_eq!(retry_delay_ms(5, Some(90_000)), 90_000);
        assert_eq!(retry_delay_ms(0, Some(10)), 1_000);
        assert_eq!(retry_delay_ms(0, Some(i64::MAX)), RETRY_AFTER_MAX_MS);
        assert_eq!(retry_delay_ms(0, Some(-5)), 1_000);
    }

    #[test]
    fn enqueueing_twice_with_the_same_key_keeps_the_first_row() {
        let s = store();
        let (a, created) = s.enqueue_op(&commit("m1"), 10).unwrap();
        assert!(created);
        assert_eq!(a.state, OpState::Queued);
        let mut again = commit("m1");
        again.payload = r#"{"changed":true}"#.into();
        let (b, created) = s.enqueue_op(&again, 20).unwrap();
        assert!(!created);
        assert_eq!(b, a);
        assert_eq!(s.list_ops(None).unwrap().len(), 1);
        // Another meeting may use the same key.
        assert!(s.enqueue_op(&commit("m2"), 30).unwrap().1);
        assert_eq!(s.list_ops(Some("m2")).unwrap().len(), 1);
    }

    #[test]
    fn a_finished_operation_is_not_enqueued_again() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        s.claim_next_op(1, &[]).unwrap().unwrap();
        s.complete_op(op.id, r#"{"sha":"abc"}"#, 2).unwrap();
        let (again, created) = s.enqueue_op(&commit("m1"), 3).unwrap();
        assert!(!created);
        assert_eq!(again.state, OpState::Done);
        assert_eq!(again.result.as_deref(), Some(r#"{"sha":"abc"}"#));
    }

    #[test]
    fn enqueueing_for_an_unknown_meeting_or_after_a_missing_key_is_refused() {
        let s = store();
        assert!(s.enqueue_op(&commit("nope"), 1).is_err());
        let mut op = new_op(
            "m1",
            OutboxTarget::Discord,
            OutboxKind::PostDiscord,
            "discord",
        );
        op.after_op_key = Some("commit".into());
        assert!(s.enqueue_op(&op, 1).is_err());
        assert!(s.list_ops(None).unwrap().is_empty());
    }

    #[test]
    fn claiming_follows_creation_order_and_marks_the_operation_running() {
        let s = store();
        s.enqueue_op(&commit("m1"), 1).unwrap();
        s.enqueue_op(
            &new_op(
                "m1",
                OutboxTarget::Issues,
                OutboxKind::CreateIssue,
                "issue:i1",
            ),
            2,
        )
        .unwrap();
        let first = s.claim_next_op(5, &[]).unwrap().unwrap();
        assert_eq!(first.op_key, "commit");
        assert_eq!(first.state, OpState::Running);
        assert_eq!(first.updated_unix_ms, 5);
        let second = s.claim_next_op(5, &[]).unwrap().unwrap();
        assert_eq!(second.op_key, "issue:i1");
        assert!(s.claim_next_op(5, &[]).unwrap().is_none());
    }

    #[test]
    fn claiming_waits_for_the_operation_it_follows() {
        let s = store();
        let (git, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        let mut post = new_op(
            "m1",
            OutboxTarget::Discord,
            OutboxKind::PostDiscord,
            "discord",
        );
        post.after_op_key = Some("commit".into());
        let (post, _) = s.enqueue_op(&post, 2).unwrap();
        assert_eq!(post.after_id, Some(git.id));

        // Only the commit is due while it is queued or running.
        assert_eq!(s.claim_next_op(10, &[]).unwrap().unwrap().id, git.id);
        assert!(s.claim_next_op(10, &[]).unwrap().is_none());

        // A transient failure keeps the follower blocked.
        s.fail_op(git.id, transient(), "offline", 10).unwrap();
        assert!(s.claim_next_op(10, &[]).unwrap().is_none());
        assert!(s.claim_next_op(1_000_000, &[]).unwrap().unwrap().id == git.id);
        s.complete_op(git.id, "{}", 1_000_001).unwrap();

        // Once the commit is done, the message is due.
        assert_eq!(
            s.claim_next_op(1_000_002, &[]).unwrap().unwrap().id,
            post.id
        );
    }

    #[test]
    fn a_failed_or_cancelled_prerequisite_keeps_the_follower_blocked() {
        let s = store();
        let (git, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        let mut post = new_op(
            "m1",
            OutboxTarget::Discord,
            OutboxKind::PostDiscord,
            "discord",
        );
        post.after_op_key = Some("commit".into());
        s.enqueue_op(&post, 2).unwrap();
        s.claim_next_op(3, &[]).unwrap().unwrap();
        s.fail_op(git.id, Failure::Permanent, "bad config", 4)
            .unwrap();
        assert!(s.claim_next_op(i64::MAX, &[]).unwrap().is_none());
        s.cancel_op(git.id, 5).unwrap();
        assert!(s.claim_next_op(i64::MAX, &[]).unwrap().is_none());
    }

    #[test]
    fn claiming_respects_the_retry_time() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        s.claim_next_op(1_000, &[]).unwrap().unwrap();
        s.fail_op(op.id, transient(), "timeout", 1_000).unwrap();
        let queued = s.get_op(op.id).unwrap().unwrap();
        assert_eq!(queued.state, OpState::Queued);
        assert_eq!(queued.attempts, 1);
        assert_eq!(queued.last_error.as_deref(), Some("timeout"));
        assert_eq!(queued.next_attempt_unix_ms, Some(1_000 + 30_000));
        assert_eq!(s.next_retry_unix_ms().unwrap(), Some(31_000));

        assert!(s.claim_next_op(30_999, &[]).unwrap().is_none());
        let again = s.claim_next_op(31_000, &[]).unwrap().unwrap();
        assert_eq!(again.id, op.id);
        assert_eq!(again.next_attempt_unix_ms, None);
        assert_eq!(s.next_retry_unix_ms().unwrap(), None);
    }

    #[test]
    fn claiming_can_skip_a_target() {
        let s = store();
        s.enqueue_op(&commit("m1"), 1).unwrap();
        s.enqueue_op(
            &new_op(
                "m1",
                OutboxTarget::Discord,
                OutboxKind::PostDiscord,
                "discord",
            ),
            2,
        )
        .unwrap();
        let op = s
            .claim_next_op(5, &[OutboxTarget::Github])
            .unwrap()
            .unwrap();
        assert_eq!(op.target, OutboxTarget::Discord);
        assert!(s
            .claim_next_op(5, &[OutboxTarget::Github])
            .unwrap()
            .is_none());
        assert_eq!(
            s.claim_next_op(5, &[]).unwrap().unwrap().target,
            OutboxTarget::Github
        );
    }

    #[test]
    fn transient_failures_back_off_and_give_up_after_the_attempt_limit() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        let mut now = 0;
        for n in 0..MAX_ATTEMPTS - 1 {
            let claimed = s.claim_next_op(now, &[]).unwrap().unwrap();
            assert_eq!(claimed.attempts, n);
            assert_eq!(
                s.fail_op(op.id, transient(), "5xx", now).unwrap(),
                OpState::Queued
            );
            let row = s.get_op(op.id).unwrap().unwrap();
            assert_eq!(
                row.next_attempt_unix_ms,
                Some(now + retry_delay_ms(n, None))
            );
            now = row.next_attempt_unix_ms.unwrap();
        }
        s.claim_next_op(now, &[]).unwrap().unwrap();
        assert_eq!(
            s.fail_op(op.id, transient(), "5xx", now).unwrap(),
            OpState::Failed
        );
        let row = s.get_op(op.id).unwrap().unwrap();
        assert_eq!(
            (row.attempts, row.next_attempt_unix_ms),
            (MAX_ATTEMPTS, None)
        );
    }

    #[test]
    fn retry_after_wins_over_the_backoff() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        s.fail_op(
            op.id,
            Failure::Transient {
                retry_after_ms: Some(7_000),
            },
            "rate limited",
            100,
        )
        .unwrap();
        assert_eq!(
            s.get_op(op.id).unwrap().unwrap().next_attempt_unix_ms,
            Some(7_100)
        );
    }

    #[test]
    fn a_permanent_failure_stops_at_once_and_a_retry_starts_over() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        assert_eq!(
            s.fail_op(op.id, Failure::Permanent, "401", 5).unwrap(),
            OpState::Failed
        );
        let failed = s.get_op(op.id).unwrap().unwrap();
        assert_eq!((failed.attempts, failed.next_attempt_unix_ms), (1, None));
        assert!(s.claim_next_op(i64::MAX, &[]).unwrap().is_none());

        s.retry_op(op.id, 50).unwrap();
        let queued = s.get_op(op.id).unwrap().unwrap();
        assert_eq!((queued.state, queued.attempts), (OpState::Queued, 0));
        assert_eq!(queued.last_error.as_deref(), Some("401"));
        assert!(s.claim_next_op(50, &[]).unwrap().is_some());
    }

    #[test]
    fn completing_stores_the_result_and_clears_the_error() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        s.fail_op(op.id, transient(), "timeout", 1).unwrap();
        s.claim_next_op(i64::MAX, &[]).unwrap().unwrap();
        s.complete_op(op.id, r#"{"number":12}"#, 9).unwrap();
        let done = s.get_op(op.id).unwrap().unwrap();
        assert_eq!(done.state, OpState::Done);
        assert_eq!(done.result.as_deref(), Some(r#"{"number":12}"#));
        assert_eq!(done.last_error, None);
        assert_eq!(done.attempts, 1);
    }

    #[test]
    fn illegal_edges_are_refused_and_change_nothing() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        // queued -> done, queued -> failed, queued -> queued.
        assert!(s.complete_op(op.id, "{}", 1).is_err());
        assert!(s.fail_op(op.id, Failure::Permanent, "x", 1).is_err());
        assert!(s.retry_op(op.id, 1).is_err());
        // running cannot be cancelled or retried.
        s.claim_next_op(2, &[]).unwrap().unwrap();
        assert!(s.cancel_op(op.id, 3).is_err());
        assert!(s.retry_op(op.id, 3).is_err());
        // done is final.
        s.complete_op(op.id, "{}", 4).unwrap();
        assert!(s.cancel_op(op.id, 5).is_err());
        assert!(s.retry_op(op.id, 5).is_err());
        assert!(s.complete_op(op.id, "{}", 5).is_err());
        assert!(s.fail_op(op.id, transient(), "x", 5).is_err());
        let row = s.get_op(op.id).unwrap().unwrap();
        assert_eq!((row.state, row.updated_unix_ms), (OpState::Done, 4));
        // An unknown id is an error too.
        assert!(s.cancel_op(999, 1).is_err());
    }

    #[test]
    fn queued_and_failed_operations_can_be_cancelled_and_stay_cancelled() {
        let s = store();
        let (a, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        let (b, _) = s
            .enqueue_op(
                &new_op(
                    "m1",
                    OutboxTarget::Issues,
                    OutboxKind::CreateIssue,
                    "issue:i1",
                ),
                0,
            )
            .unwrap();
        s.cancel_op(a.id, 1).unwrap();
        assert_eq!(s.get_op(a.id).unwrap().unwrap().state, OpState::Cancelled);
        s.claim_next_op(2, &[]).unwrap().unwrap();
        s.fail_op(b.id, Failure::Permanent, "422", 3).unwrap();
        s.cancel_op(b.id, 4).unwrap();
        assert!(s.retry_op(b.id, 5).is_err());
        assert!(s.claim_next_op(i64::MAX, &[]).unwrap().is_none());
    }

    #[test]
    fn retrying_all_revives_failed_and_waiting_operations() {
        let s = store();
        let (a, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        let (b, _) = s
            .enqueue_op(
                &new_op(
                    "m1",
                    OutboxTarget::Issues,
                    OutboxKind::CreateIssue,
                    "issue:i1",
                ),
                0,
            )
            .unwrap();
        let (c, _) = s
            .enqueue_op(
                &new_op(
                    "m1",
                    OutboxTarget::Discord,
                    OutboxKind::PostDiscord,
                    "discord",
                ),
                0,
            )
            .unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        s.fail_op(a.id, Failure::Permanent, "403", 1).unwrap();
        s.claim_next_op(1, &[]).unwrap().unwrap();
        s.fail_op(b.id, transient(), "offline", 1).unwrap();
        assert_eq!(s.retry_all_ops(2).unwrap(), 2);
        // A failed operation starts over; a waiting one keeps its count but is due now.
        for (id, attempts) in [(a.id, 0), (b.id, 1)] {
            let row = s.get_op(id).unwrap().unwrap();
            assert_eq!(
                (row.state, row.attempts, row.next_attempt_unix_ms),
                (OpState::Queued, attempts, None)
            );
        }
        assert_eq!(s.get_op(c.id).unwrap().unwrap().state, OpState::Queued);
        assert_eq!(s.claim_next_op(2, &[]).unwrap().unwrap().id, a.id);
    }

    #[test]
    fn a_crash_puts_running_operations_back_without_counting_the_attempt() {
        let s = store();
        let (a, _) = s.enqueue_op(&commit("m1"), 0).unwrap();
        let (b, _) = s
            .enqueue_op(
                &new_op(
                    "m1",
                    OutboxTarget::Issues,
                    OutboxKind::CreateIssue,
                    "issue:i1",
                ),
                0,
            )
            .unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        s.claim_next_op(0, &[]).unwrap().unwrap();
        s.complete_op(b.id, "{}", 1).unwrap();
        assert_eq!(s.reset_running_ops(2).unwrap(), [a.id]);
        let row = s.get_op(a.id).unwrap().unwrap();
        assert_eq!((row.state, row.attempts), (OpState::Queued, 0));
        assert_eq!(s.get_op(b.id).unwrap().unwrap().state, OpState::Done);
        assert!(s.reset_running_ops(3).unwrap().is_empty());
        assert_eq!(s.claim_next_op(3, &[]).unwrap().unwrap().id, a.id);
    }

    #[test]
    fn deleting_a_meeting_removes_its_operations_only() {
        let s = store();
        s.enqueue_op(&commit("m1"), 0).unwrap();
        s.enqueue_op(&commit("m2"), 0).unwrap();
        assert!(s.delete_meeting("m1").unwrap());
        let left = s.list_ops(None).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].meeting_id, "m2");
    }

    #[test]
    fn listing_filters_by_meeting_and_reads_back_every_column() {
        let s = store();
        let mut op = new_op(
            "m1",
            OutboxTarget::Issues,
            OutboxKind::SetIssueState,
            "status:s2",
        );
        op.payload = r#"{"issue":7,"state":"closed"}"#.into();
        let (row, _) = s.enqueue_op(&op, 42).unwrap();
        assert_eq!(row.target, OutboxTarget::Issues);
        assert_eq!(row.kind, OutboxKind::SetIssueState);
        assert_eq!(row.payload, op.payload);
        assert_eq!((row.created_unix_ms, row.updated_unix_ms), (42, 42));
        assert_eq!(s.get_op_by_key("m1", "status:s2").unwrap().unwrap(), row);
        assert!(s.get_op_by_key("m2", "status:s2").unwrap().is_none());
        assert!(s.list_ops(Some("m2")).unwrap().is_empty());
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["state"], "queued");
        assert_eq!(json["kind"], "set_issue_state");
    }

    #[test]
    fn a_discord_post_waits_for_the_issues_of_its_meeting() {
        let s = store();
        let issue = new_op(
            "m1",
            OutboxTarget::Issues,
            OutboxKind::CreateIssue,
            "issue:i1",
        );
        let post = new_op(
            "m1",
            OutboxTarget::Discord,
            OutboxKind::PostDiscord,
            "discord",
        );
        let other = new_op(
            "m2",
            OutboxTarget::Discord,
            OutboxKind::PostDiscord,
            "discord",
        );
        let (issue_op, _) = s.enqueue_op(&issue, 1).unwrap();
        s.enqueue_op(&post, 1).unwrap();
        s.enqueue_op(&other, 1).unwrap();

        // The issue runs first; the post of the other meeting is not held back.
        let first = s.claim_next_op(10, &[]).unwrap().unwrap();
        assert_eq!(first.id, issue_op.id);
        let second = s.claim_next_op(10, &[]).unwrap().unwrap();
        assert_eq!(
            (second.meeting_id.as_str(), second.target),
            ("m2", OutboxTarget::Discord)
        );
        assert!(
            s.claim_next_op(10, &[]).unwrap().is_none(),
            "m1 waits for its issue"
        );

        // A transient failure keeps the post waiting; a permanent one releases it.
        s.fail_op(first.id, transient(), "later", 10).unwrap();
        assert!(s.claim_next_op(20, &[]).unwrap().is_none());
        s.retry_all_ops(20).unwrap();
        let again = s.claim_next_op(20, &[]).unwrap().unwrap();
        s.fail_op(again.id, Failure::Permanent, "no", 20).unwrap();
        let post_op = s.claim_next_op(20, &[]).unwrap().unwrap();
        assert_eq!(post_op.op_key, "discord");
        assert_eq!(post_op.meeting_id, "m1");
    }

    #[test]
    fn an_edit_of_the_discord_message_does_not_wait_for_issues() {
        let s = store();
        s.enqueue_op(
            &new_op(
                "m1",
                OutboxTarget::Issues,
                OutboxKind::CreateIssue,
                "issue:i1",
            ),
            1,
        )
        .unwrap();
        s.enqueue_op(
            &new_op(
                "m1",
                OutboxTarget::Discord,
                OutboxKind::PostDiscord,
                "discord",
            ),
            1,
        )
        .unwrap();
        let (edit, _) = s
            .enqueue_op(
                &new_op(
                    "m1",
                    OutboxTarget::Discord,
                    OutboxKind::EditDiscord,
                    "discord-edit:c1",
                ),
                1,
            )
            .unwrap();
        // The issue goes first, the edit is not held back by it, the post still is.
        let first = s.claim_next_op(10, &[]).unwrap().unwrap();
        assert_eq!(first.kind, OutboxKind::CreateIssue);
        let second = s.claim_next_op(10, &[]).unwrap().unwrap();
        assert_eq!(second.id, edit.id);
        assert!(s.claim_next_op(10, &[]).unwrap().is_none());
    }

    #[test]
    fn the_notion_and_edit_names_round_trip_and_match_the_check_constraint() {
        let s = store();
        assert_eq!(OutboxTarget::Notion.as_str(), "notion");
        assert_eq!(OutboxKind::ExportNotion.as_str(), "export_notion");
        assert_eq!(OutboxKind::EditDiscord.as_str(), "edit_discord");
        assert_eq!(OutboxTarget::parse("notion").unwrap(), OutboxTarget::Notion);
        assert_eq!(
            OutboxKind::parse("export_notion").unwrap(),
            OutboxKind::ExportNotion
        );
        assert_eq!(
            OutboxKind::parse("edit_discord").unwrap(),
            OutboxKind::EditDiscord
        );
        let cases = [
            (OutboxTarget::Notion, OutboxKind::ExportNotion, "notion:i1"),
            (
                OutboxTarget::Discord,
                OutboxKind::EditDiscord,
                "discord-edit:c1",
            ),
        ];
        for (target, kind, key) in cases {
            let (op, created) = s.enqueue_op(&new_op("m1", target, kind, key), 5).unwrap();
            assert!(created);
            let back = s.get_op(op.id).unwrap().unwrap();
            assert_eq!((back.target, back.kind), (target, kind));
            let listed = s.list_ops(Some("m1")).unwrap();
            assert!(listed.iter().any(|o| o.id == op.id && o.kind == kind));
        }
    }

    #[test]
    fn progress_is_kept_through_a_failure_and_replaced_by_the_outcome() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        assert!(
            s.set_op_progress(op.id, "{}", 2).is_err(),
            "not running yet"
        );
        s.claim_next_op(5, &[]).unwrap().unwrap();
        s.set_op_progress(op.id, r#"{"sent":2}"#, 6).unwrap();
        s.fail_op(op.id, transient(), "offline", 7).unwrap();
        let queued = s.get_op(op.id).unwrap().unwrap();
        assert_eq!(queued.state, OpState::Queued);
        assert_eq!(queued.result.as_deref(), Some(r#"{"sent":2}"#));
        s.claim_next_op(10_000_000, &[]).unwrap().unwrap();
        s.complete_op(op.id, r#"{"sent":3}"#, 9).unwrap();
        assert_eq!(
            s.get_op(op.id).unwrap().unwrap().result.as_deref(),
            Some(r#"{"sent":3}"#)
        );
    }

    #[test]
    fn enqueueing_a_cancelled_operation_again_starts_it_over() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        s.cancel_op(op.id, 2).unwrap();
        let mut again = commit("m1");
        again.payload = r#"{"v":2}"#.into();
        let (revived, created) = s.enqueue_op(&again, 3).unwrap();
        assert!(created);
        assert_eq!(revived.id, op.id, "the same row, so the key stays unique");
        assert_eq!(revived.state, OpState::Queued);
        assert_eq!(revived.payload, r#"{"v":2}"#);
        assert_eq!(revived.attempts, 0);
        assert!(s.claim_next_op(10, &[]).unwrap().is_some());
    }

    #[test]
    fn a_waiting_operation_can_be_made_due_now_and_nothing_else() {
        let s = store();
        let (op, _) = s.enqueue_op(&commit("m1"), 1).unwrap();
        s.claim_next_op(5, &[]).unwrap().unwrap();
        s.fail_op(op.id, transient(), "offline", 5).unwrap();
        assert!(
            s.claim_next_op(6, &[]).unwrap().is_none(),
            "waiting for its backoff"
        );
        s.run_op_now(op.id, 6).unwrap();
        assert_eq!(
            s.get_op(op.id).unwrap().unwrap().attempts,
            1,
            "the failure still counts"
        );
        assert!(s.claim_next_op(6, &[]).unwrap().is_some());
        assert!(s.run_op_now(op.id, 7).is_err(), "running");
    }
}
