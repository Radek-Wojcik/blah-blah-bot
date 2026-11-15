//! Forward-only schema migrations, tracked with `PRAGMA user_version`.

use rusqlite::Connection;

use crate::{Result, StoreError};

/// Each entry upgrades the schema by one version. Never edit an entry once released;
/// add a new one.
const MIGRATIONS: &[&str] = &[
    // Version 1: meetings, tracks, segments, screenshots.
    "
    CREATE TABLE meetings (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        started_unix_ms INTEGER NOT NULL,
        duration_ms INTEGER,
        state TEXT NOT NULL CHECK (state IN ('recording','stopped','recoverable','recovered')),
        dir TEXT NOT NULL
    );
    CREATE TABLE tracks (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        source TEXT NOT NULL,
        pid INTEGER,
        offset_ms REAL NOT NULL,
        ended INTEGER NOT NULL DEFAULT 0,
        end_offset_ms REAL,
        total_samples INTEGER,
        PRIMARY KEY (meeting_id, name)
    );
    CREATE TABLE segments (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        track TEXT NOT NULL,
        segment INTEGER NOT NULL,
        file TEXT NOT NULL,
        offset_ms REAL NOT NULL,
        samples INTEGER NOT NULL,
        PRIMARY KEY (meeting_id, track, segment)
    );
    CREATE TABLE screenshots (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        file TEXT NOT NULL,
        offset_ms REAL NOT NULL,
        PRIMARY KEY (meeting_id, file)
    );
    CREATE INDEX idx_meetings_started ON meetings(started_unix_ms);
    ",
    // Version 2: processing jobs, one per meeting, with one row per stage.
    "
    CREATE TABLE jobs (
        meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
        created_unix_ms INTEGER NOT NULL,
        updated_unix_ms INTEGER NOT NULL,
        options TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TABLE job_stages (
        meeting_id TEXT NOT NULL REFERENCES jobs(meeting_id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('pending','running','done','failed')),
        attempts INTEGER NOT NULL DEFAULT 0,
        started_unix_ms INTEGER,
        finished_unix_ms INTEGER,
        error TEXT,
        PRIMARY KEY (meeting_id, stage)
    );
    ",
];

/// Schema version after all migrations have run.
pub const LATEST: u32 = MIGRATIONS.len() as u32;

pub fn migrate(conn: &Connection) -> Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > LATEST {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: LATEST,
        });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
