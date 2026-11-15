//! Persistence for processing jobs: one job per meeting, one row per stage.
//!
//! The store only records facts. The meaning of the stage names and the rules for moving
//! between them belong to the `queue` crate.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Result, Store, StoreError};

/// Status of one stage of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl StageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }
}

/// One stage row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageRow {
    pub stage: String,
    pub status: StageStatus,
    pub attempts: u32,
    pub started_unix_ms: Option<i64>,
    pub finished_unix_ms: Option<i64>,
    pub error: Option<String>,
    /// Why a stage that is `done` did nothing (for example "the meeting has no cycle").
    #[serde(default)]
    pub note: Option<String>,
}

/// A job with its stages, in the order they were created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRow {
    pub meeting_id: String,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
    /// Free-form JSON options chosen when the job was created (language, backends).
    pub options: String,
    pub stages: Vec<StageRow>,
}

type StageTuple = (
    String,
    String,
    u32,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

impl Store {
    /// Create a job with the given stages, all pending. Does nothing when the job exists.
    /// Returns whether a job was created.
    pub fn create_job(
        &self,
        meeting_id: &str,
        stages: &[&str],
        options: &str,
        now_ms: i64,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO jobs (meeting_id, created_unix_ms, updated_unix_ms, options)
             VALUES (?1, ?2, ?2, ?3)",
            params![meeting_id, now_ms, options],
        )? > 0;
        if inserted {
            for stage in stages {
                tx.execute(
                    "INSERT INTO job_stages (meeting_id, stage, status) VALUES (?1, ?2, 'pending')",
                    params![meeting_id, stage],
                )?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    pub fn get_job(&self, meeting_id: &str) -> Result<Option<JobRow>> {
        let head = self
            .conn
            .query_row(
                "SELECT created_unix_ms, updated_unix_ms, options FROM jobs WHERE meeting_id = ?1",
                params![meeting_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((created_unix_ms, updated_unix_ms, options)) = head else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT stage, status, attempts, started_unix_ms, finished_unix_ms, error, note
             FROM job_stages WHERE meeting_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok::<StageTuple, rusqlite::Error>((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        let mut stages = Vec::new();
        for row in rows {
            let (stage, status, attempts, started_unix_ms, finished_unix_ms, error, note) = row?;
            stages.push(StageRow {
                stage,
                status: StageStatus::parse(&status)?,
                attempts,
                started_unix_ms,
                finished_unix_ms,
                error,
                note,
            });
        }
        Ok(Some(JobRow {
            meeting_id: meeting_id.to_string(),
            created_unix_ms,
            updated_unix_ms,
            options,
            stages,
        }))
    }

    /// Ids of all jobs, oldest first.
    pub fn list_job_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT meeting_id FROM jobs ORDER BY created_unix_ms, rowid")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Set the status of one stage. `Running` counts an attempt and stamps the start;
    /// `Done` and `Failed` stamp the finish. `error` is stored only for `Failed`.
    pub fn set_stage_status(
        &self,
        meeting_id: &str,
        stage: &str,
        status: StageStatus,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        // Jobs created by an older version have no row for stages added since.
        tx.execute(
            "INSERT OR IGNORE INTO job_stages (meeting_id, stage, status)
             SELECT ?1, ?2, 'pending' WHERE EXISTS (SELECT 1 FROM jobs WHERE meeting_id = ?1)",
            params![meeting_id, stage],
        )?;
        match status {
            StageStatus::Running => tx.execute(
                "UPDATE job_stages SET status = 'running', attempts = attempts + 1,
                    started_unix_ms = ?3, finished_unix_ms = NULL, error = NULL, note = NULL
                 WHERE meeting_id = ?1 AND stage = ?2",
                params![meeting_id, stage, now_ms],
            )?,
            StageStatus::Done => tx.execute(
                "UPDATE job_stages SET status = 'done', finished_unix_ms = ?3, error = NULL, note = NULL
                 WHERE meeting_id = ?1 AND stage = ?2",
                params![meeting_id, stage, now_ms],
            )?,
            StageStatus::Failed => tx.execute(
                "UPDATE job_stages SET status = 'failed', finished_unix_ms = ?3, error = ?4
                 WHERE meeting_id = ?1 AND stage = ?2",
                params![meeting_id, stage, now_ms, error],
            )?,
            StageStatus::Pending => tx.execute(
                "UPDATE job_stages SET status = 'pending', error = NULL, note = NULL
                 WHERE meeting_id = ?1 AND stage = ?2",
                params![meeting_id, stage],
            )?,
        };
        tx.execute(
            "UPDATE jobs SET updated_unix_ms = ?2 WHERE meeting_id = ?1",
            params![meeting_id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Mark a stage `done` without running it, with the reason as a note.
    pub fn mark_stage_skipped(
        &self,
        meeting_id: &str,
        stage: &str,
        note: &str,
        now_ms: i64,
    ) -> Result<()> {
        self.set_stage_status(meeting_id, stage, StageStatus::Done, None, now_ms)?;
        self.conn.execute(
            "UPDATE job_stages SET note = ?3 WHERE meeting_id = ?1 AND stage = ?2",
            params![meeting_id, stage, note],
        )?;
        Ok(())
    }

    /// Turn every `running` stage back into `pending`: no stage can be running in a fresh
    /// process, so these are leftovers of a crash. Returns `(meeting id, stage)` pairs.
    pub fn reset_running_stages(&self) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT meeting_id, stage FROM job_stages WHERE status = 'running'")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        self.conn.execute(
            "UPDATE job_stages SET status = 'pending' WHERE status = 'running'",
            [],
        )?;
        Ok(rows)
    }

    pub fn delete_job(&self, meeting_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM jobs WHERE meeting_id = ?1",
            params![meeting_id],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, MeetingState};

    fn store_with_meeting() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m1".into(),
            title: "t".into(),
            started_unix_ms: 1,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "m1".into(),
        })
        .unwrap();
        s
    }

    #[test]
    fn create_is_idempotent_and_keeps_stage_order() {
        let s = store_with_meeting();
        assert!(s.create_job("m1", &["asr", "diarize"], "{}", 10).unwrap());
        assert!(!s.create_job("m1", &["x"], "{}", 11).unwrap());
        let job = s.get_job("m1").unwrap().unwrap();
        let names: Vec<_> = job.stages.iter().map(|s| s.stage.as_str()).collect();
        assert_eq!(names, ["asr", "diarize"]);
    }

    #[test]
    fn running_counts_attempts_and_reset_returns_stages() {
        let s = store_with_meeting();
        s.create_job("m1", &["asr"], "{}", 10).unwrap();
        s.set_stage_status("m1", "asr", StageStatus::Running, None, 20)
            .unwrap();
        assert_eq!(
            s.reset_running_stages().unwrap(),
            vec![("m1".to_string(), "asr".to_string())]
        );
        s.set_stage_status("m1", "asr", StageStatus::Running, None, 30)
            .unwrap();
        s.set_stage_status("m1", "asr", StageStatus::Failed, Some("boom"), 40)
            .unwrap();
        let st = &s.get_job("m1").unwrap().unwrap().stages[0];
        assert_eq!(st.attempts, 2);
        assert_eq!(st.status, StageStatus::Failed);
        assert_eq!(st.error.as_deref(), Some("boom"));
    }

    #[test]
    fn a_skipped_stage_is_done_with_a_note_that_a_rerun_clears() {
        let s = store_with_meeting();
        s.create_job("m1", &["asr"], "{}", 10).unwrap();
        s.mark_stage_skipped("m1", "asr", "nothing to do", 20)
            .unwrap();
        let st = &s.get_job("m1").unwrap().unwrap().stages[0];
        assert_eq!(st.status, StageStatus::Done);
        assert_eq!(st.note.as_deref(), Some("nothing to do"));
        assert_eq!(st.error, None);
        s.set_stage_status("m1", "asr", StageStatus::Pending, None, 30)
            .unwrap();
        assert_eq!(s.get_job("m1").unwrap().unwrap().stages[0].note, None);
    }

    #[test]
    fn deleting_the_meeting_removes_the_job() {
        let s = store_with_meeting();
        s.create_job("m1", &["asr"], "{}", 10).unwrap();
        s.delete_meeting("m1").unwrap();
        assert!(s.get_job("m1").unwrap().is_none());
    }
}
