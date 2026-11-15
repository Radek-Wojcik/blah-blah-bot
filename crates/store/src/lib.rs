//! Local SQLite state: meetings, tracks, segments and screenshots.
//!
//! Paths stored here are relative to the recordings root, so the database never contains
//! absolute paths. The schema is versioned with `PRAGMA user_version` and migrated forward
//! on open.

mod cycles;
mod jobs;
mod migrations;
mod voices;

pub use cycles::{LlmChoice, LlmKind, ReportSettings, SyncOutcome};
pub use jobs::{JobRow, StageRow, StageStatus};
pub use voices::{
    blob_to_embedding, cycle_participant_key, embedding_to_blob, ClipRow, Participant,
    ParticipantInfo, SampleRow, VoiceDecision, VoiceRow,
};

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Errors returned by the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("unknown meeting state: {0}")]
    UnknownState(String),
    #[error("invalid value: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Lifecycle of a recorded meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingState {
    /// A recording is in progress.
    Recording,
    /// The recording ended normally.
    Stopped,
    /// The recorder died; the journal can be replayed to recover the meeting.
    Recoverable,
    /// The meeting was rebuilt from its journal after a crash.
    Recovered,
}

impl MeetingState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Stopped => "stopped",
            Self::Recoverable => "recoverable",
            Self::Recovered => "recovered",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "recording" => Ok(Self::Recording),
            "stopped" => Ok(Self::Stopped),
            "recoverable" => Ok(Self::Recoverable),
            "recovered" => Ok(Self::Recovered),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }
}

/// A meeting row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub started_unix_ms: i64,
    pub duration_ms: Option<i64>,
    pub state: MeetingState,
    /// Folder of the meeting, relative to the recordings root.
    pub dir: String,
}

/// A track row: one audio source recorded within a meeting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub source: String,
    pub pid: Option<u32>,
    /// Start of the track on the session clock.
    pub offset_ms: f64,
    pub ended: bool,
    pub end_offset_ms: Option<f64>,
    pub total_samples: Option<u64>,
}

/// A segment row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub track: String,
    pub segment: u32,
    /// File path relative to the meeting folder.
    pub file: String,
    pub offset_ms: f64,
    pub samples: u64,
}

/// A region screenshot row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screenshot {
    /// File path relative to the meeting folder.
    pub file: String,
    /// Offset from the start of the recording on the session clock.
    pub offset_ms: f64,
}

/// A meeting row with counts, for lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub meeting: Meeting,
    pub track_count: u32,
    pub screenshot_count: u32,
}

/// A meeting with everything attached to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingDetails {
    pub meeting: Meeting,
    pub tracks: Vec<Track>,
    pub segments: Vec<Segment>,
    pub screenshots: Vec<Screenshot>,
}

/// Handle to the local database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (and migrate) the database at `path`, creating it when missing.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrations::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Current schema version.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    pub fn create_meeting(&self, m: &Meeting) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meetings (id, title, started_unix_ms, duration_ms, state, dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                m.id,
                m.title,
                m.started_unix_ms,
                m.duration_ms,
                m.state.as_str(),
                m.dir
            ],
        )?;
        Ok(())
    }

    pub fn set_meeting_state(
        &self,
        id: &str,
        state: MeetingState,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET state = ?2, duration_ms = COALESCE(?3, duration_ms) WHERE id = ?1",
            params![id, state.as_str(), duration_ms],
        )?;
        Ok(())
    }

    pub fn rename_meeting(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    /// Insert or update a track.
    pub fn upsert_track(&self, meeting_id: &str, t: &Track) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tracks (meeting_id, name, source, pid, offset_ms, ended, end_offset_ms, total_samples)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(meeting_id, name) DO UPDATE SET
               source = excluded.source, pid = excluded.pid, offset_ms = excluded.offset_ms,
               ended = excluded.ended, end_offset_ms = excluded.end_offset_ms,
               total_samples = excluded.total_samples",
            params![
                meeting_id,
                t.name,
                t.source,
                t.pid,
                t.offset_ms,
                t.ended,
                t.end_offset_ms,
                t.total_samples.map(|v| v as i64),
            ],
        )?;
        Ok(())
    }

    /// Insert or replace a segment.
    pub fn add_segment(&self, meeting_id: &str, s: &Segment) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO segments (meeting_id, track, segment, file, offset_ms, samples)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                meeting_id,
                s.track,
                s.segment,
                s.file,
                s.offset_ms,
                s.samples as i64
            ],
        )?;
        Ok(())
    }

    pub fn add_screenshot(&self, meeting_id: &str, s: &Screenshot) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO screenshots (meeting_id, file, offset_ms) VALUES (?1, ?2, ?3)",
            params![meeting_id, s.file, s.offset_ms],
        )?;
        Ok(())
    }

    /// Meetings, newest first.
    pub fn list_meetings(&self) -> Result<Vec<Meeting>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, started_unix_ms, duration_ms, state, dir
             FROM meetings ORDER BY started_unix_ms DESC, id DESC",
        )?;
        let rows = stmt.query_map([], meeting_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Meetings with track and screenshot counts, newest first.
    pub fn list_summaries(&self) -> Result<Vec<MeetingSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.title, m.started_unix_ms, m.duration_ms, m.state, m.dir,
                    (SELECT COUNT(*) FROM tracks t WHERE t.meeting_id = m.id),
                    (SELECT COUNT(*) FROM screenshots s WHERE s.meeting_id = m.id)
             FROM meetings m ORDER BY m.started_unix_ms DESC, m.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let meeting = meeting_row(r)?;
            Ok((meeting, r.get::<_, u32>(6)?, r.get::<_, u32>(7)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (meeting, track_count, screenshot_count) = r?;
            out.push(MeetingSummary {
                meeting: meeting?,
                track_count,
                screenshot_count,
            });
        }
        Ok(out)
    }

    /// Meetings in a given state, newest first.
    pub fn list_meetings_in_state(&self, state: MeetingState) -> Result<Vec<Meeting>> {
        Ok(self
            .list_meetings()?
            .into_iter()
            .filter(|m| m.state == state)
            .collect())
    }

    pub fn get_meeting(&self, id: &str) -> Result<Option<MeetingDetails>> {
        let meeting = self
            .conn
            .query_row(
                "SELECT id, title, started_unix_ms, duration_ms, state, dir FROM meetings WHERE id = ?1",
                params![id],
                meeting_row,
            )
            .optional()?;
        let Some(meeting) = meeting else {
            return Ok(None);
        };
        let meeting = meeting?;

        let mut stmt = self.conn.prepare(
            "SELECT name, source, pid, offset_ms, ended, end_offset_ms, total_samples
             FROM tracks WHERE meeting_id = ?1 ORDER BY offset_ms, name",
        )?;
        let tracks = stmt
            .query_map(params![id], |r| {
                Ok(Track {
                    name: r.get(0)?,
                    source: r.get(1)?,
                    pid: r.get(2)?,
                    offset_ms: r.get(3)?,
                    ended: r.get(4)?,
                    end_offset_ms: r.get(5)?,
                    total_samples: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT track, segment, file, offset_ms, samples
             FROM segments WHERE meeting_id = ?1 ORDER BY track, segment",
        )?;
        let segments = stmt
            .query_map(params![id], |r| {
                Ok(Segment {
                    track: r.get(0)?,
                    segment: r.get(1)?,
                    file: r.get(2)?,
                    offset_ms: r.get(3)?,
                    samples: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT file, offset_ms FROM screenshots WHERE meeting_id = ?1 ORDER BY offset_ms, file",
        )?;
        let screenshots = stmt
            .query_map(params![id], |r| {
                Ok(Screenshot {
                    file: r.get(0)?,
                    offset_ms: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Some(MeetingDetails {
            meeting,
            tracks,
            segments,
            screenshots,
        }))
    }

    /// Delete a meeting and everything attached to it. Files on disk are not touched.
    pub fn delete_meeting(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM meetings WHERE id = ?1", params![id])?
            > 0)
    }

    /// Mark every meeting still in the `recording` state as `recoverable`. Called at start:
    /// no recording can be in progress in a fresh process, so these are crash leftovers.
    pub fn mark_interrupted_recoverable(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM meetings WHERE state = 'recording'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        self.conn.execute(
            "UPDATE meetings SET state = 'recoverable' WHERE state = 'recording'",
            [],
        )?;
        Ok(ids)
    }
}

fn meeting_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Meeting>> {
    let id: String = r.get(0)?;
    let title: String = r.get(1)?;
    let started_unix_ms: i64 = r.get(2)?;
    let duration_ms: Option<i64> = r.get(3)?;
    let state: String = r.get(4)?;
    let dir: String = r.get(5)?;
    Ok(MeetingState::parse(&state).map(|state| Meeting {
        id,
        title,
        started_unix_ms,
        duration_ms,
        state,
        dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(id: &str, started: i64) -> Meeting {
        Meeting {
            id: id.into(),
            title: format!("Meeting {id}"),
            started_unix_ms: started,
            duration_ms: None,
            state: MeetingState::Recording,
            dir: format!("meetings/{id}"),
        }
    }

    fn track(name: &str, offset_ms: f64) -> Track {
        Track {
            name: name.into(),
            source: "microphone".into(),
            pid: None,
            offset_ms,
            ended: false,
            end_offset_ms: None,
            total_samples: None,
        }
    }

    #[test]
    fn fresh_database_is_at_latest_version() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), migrations::LATEST);
    }

    #[test]
    fn reopening_keeps_data_and_version() {
        let dir = std::env::temp_dir().join(format!("bbb-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        {
            let s = Store::open(&path).unwrap();
            s.create_meeting(&meeting("a", 1)).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.list_meetings().unwrap().len(), 1);
        assert_eq!(s.schema_version().unwrap(), migrations::LATEST);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", migrations::LATEST + 1)
            .unwrap();
        let err = Store::init(conn).err().unwrap();
        assert!(matches!(err, StoreError::SchemaTooNew { .. }));
    }

    #[test]
    fn meetings_are_listed_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("old", 100)).unwrap();
        s.create_meeting(&meeting("new", 200)).unwrap();
        let ids: Vec<_> = s
            .list_meetings()
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["new", "old"]);
    }

    #[test]
    fn details_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("m", 1)).unwrap();
        s.upsert_track("m", &track("mic", 0.0)).unwrap();
        s.upsert_track("m", &track("app-discord", 5000.0)).unwrap();
        for i in 0..3 {
            s.add_segment(
                "m",
                &Segment {
                    track: "mic".into(),
                    segment: i,
                    file: format!("mic/seg-{i:05}.wav"),
                    offset_ms: f64::from(i) * 60_000.0,
                    samples: 960_000,
                },
            )
            .unwrap();
        }
        s.add_screenshot(
            "m",
            &Screenshot {
                file: "img/0001-00-12-41.png".into(),
                offset_ms: 761_000.0,
            },
        )
        .unwrap();
        s.set_meeting_state("m", MeetingState::Stopped, Some(180_000))
            .unwrap();

        let d = s.get_meeting("m").unwrap().unwrap();
        assert_eq!(d.meeting.state, MeetingState::Stopped);
        assert_eq!(d.meeting.duration_ms, Some(180_000));
        assert_eq!(d.tracks.len(), 2);
        assert_eq!(d.tracks[1].name, "app-discord");
        assert_eq!(d.segments.len(), 3);
        assert_eq!(d.segments[2].offset_ms, 120_000.0);
        assert_eq!(d.screenshots[0].offset_ms, 761_000.0);
    }

    #[test]
    fn summaries_carry_counts() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("m", 1)).unwrap();
        s.upsert_track("m", &track("mic", 0.0)).unwrap();
        s.upsert_track("m", &track("app", 0.0)).unwrap();
        s.add_screenshot(
            "m",
            &Screenshot {
                file: "a.png".into(),
                offset_ms: 1.0,
            },
        )
        .unwrap();
        let list = s.list_summaries().unwrap();
        assert_eq!((list[0].track_count, list[0].screenshot_count), (2, 1));
    }

    #[test]
    fn upsert_track_updates_in_place() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("m", 1)).unwrap();
        s.upsert_track("m", &track("mic", 0.0)).unwrap();
        let mut t = track("mic", 0.0);
        t.ended = true;
        t.end_offset_ms = Some(10_000.0);
        t.total_samples = Some(160_000);
        s.upsert_track("m", &t).unwrap();
        let d = s.get_meeting("m").unwrap().unwrap();
        assert_eq!(d.tracks, vec![t]);
    }

    #[test]
    fn delete_cascades() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("m", 1)).unwrap();
        s.upsert_track("m", &track("mic", 0.0)).unwrap();
        assert!(s.delete_meeting("m").unwrap());
        assert!(s.get_meeting("m").unwrap().is_none());
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(!s.delete_meeting("m").unwrap());
    }

    #[test]
    fn interrupted_recordings_become_recoverable() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&meeting("crashed", 1)).unwrap();
        let mut done = meeting("done", 2);
        done.state = MeetingState::Stopped;
        s.create_meeting(&done).unwrap();
        let ids = s.mark_interrupted_recoverable().unwrap();
        assert_eq!(ids, ["crashed"]);
        let rec = s.list_meetings_in_state(MeetingState::Recoverable).unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].id, "crashed");
    }
}
