//! Forward-only schema migrations, tracked with `PRAGMA user_version`.

use rusqlite::Connection;

use crate::{Result, StoreError};

/// Each entry upgrades the schema by one version. Never edit an entry once released;
/// add a new one.
pub(crate) const MIGRATIONS: &[&str] = &[
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
    // Version 3: participants, voice profiles (biometric, local only), detected voices per
    // meeting and their sample clips.
    "
    CREATE TABLE participants (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL COLLATE NOCASE UNIQUE,
        created_unix_ms INTEGER NOT NULL
    );
    CREATE TABLE voice_samples (
        participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
        source_key TEXT NOT NULL,
        embedding BLOB NOT NULL,
        speech_ms INTEGER NOT NULL,
        created_unix_ms INTEGER NOT NULL,
        PRIMARY KEY (participant_id, source_key)
    );
    CREATE TABLE meeting_voices (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        voice_id TEXT NOT NULL,
        track TEXT NOT NULL,
        label TEXT NOT NULL,
        embedding BLOB NOT NULL,
        speech_ms INTEGER NOT NULL,
        decision TEXT NOT NULL CHECK (decision IN ('unknown','auto','suggested')),
        score REAL,
        matched_participant TEXT REFERENCES participants(id) ON DELETE SET NULL,
        assigned_participant TEXT REFERENCES participants(id) ON DELETE SET NULL,
        confirmed INTEGER NOT NULL DEFAULT 0,
        merged_into TEXT,
        PRIMARY KEY (meeting_id, voice_id)
    );
    CREATE TABLE voice_clips (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        voice_id TEXT NOT NULL,
        idx INTEGER NOT NULL,
        file TEXT NOT NULL,
        start_ms INTEGER NOT NULL,
        end_ms INTEGER NOT NULL,
        text TEXT NOT NULL DEFAULT '',
        created_unix_ms INTEGER NOT NULL,
        PRIMARY KEY (meeting_id, voice_id, idx)
    );
    ",
    // Version 4: cycles. A meeting belongs to a cycle and carries its report settings and
    // the choice of language-model backend; participants belong to a cycle (NULL is the
    // legacy local list) and their names are unique per cycle. `participants` is rebuilt
    // to drop the global unique name: foreign keys are switched off around this migration
    // (see `migrate`), so the voice profiles that reference the table survive the rebuild.
    "
    ALTER TABLE meetings ADD COLUMN cycle_id TEXT;
    ALTER TABLE meetings ADD COLUMN report_mode TEXT
        CHECK (report_mode IS NULL OR report_mode IN ('fixed','template','auto','by-type'));
    ALTER TABLE meetings ADD COLUMN report_template TEXT;
    ALTER TABLE meetings ADD COLUMN meeting_type TEXT;
    ALTER TABLE meetings ADD COLUMN output_language TEXT
        CHECK (output_language IS NULL OR output_language IN ('auto','pl','en'));
    CREATE INDEX idx_meetings_cycle ON meetings(cycle_id);

    CREATE TABLE meeting_llm (
        meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (kind IN ('managed','external','cloud')),
        cloud_confirmed INTEGER NOT NULL DEFAULT 0,
        confirmed_unix_ms INTEGER,
        endpoint_host TEXT
    );

    ALTER TABLE job_stages ADD COLUMN note TEXT;

    CREATE TABLE participants_v4 (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL COLLATE NOCASE,
        created_unix_ms INTEGER NOT NULL,
        cycle_id TEXT
    );
    INSERT INTO participants_v4 (id, name, created_unix_ms)
        SELECT id, name, created_unix_ms FROM participants;
    DROP TABLE participants;
    ALTER TABLE participants_v4 RENAME TO participants;
    CREATE UNIQUE INDEX idx_participants_name_local ON participants(name) WHERE cycle_id IS NULL;
    CREATE UNIQUE INDEX idx_participants_name_cycle
        ON participants(cycle_id, name) WHERE cycle_id IS NOT NULL;
    ",
    // Version 5: publishing. The outbox holds one row per outgoing operation (a commit to
    // the cycle repository, a GitHub issue, a Discord message) so that a failed or offline
    // send is retried instead of lost; `op_key` makes enqueueing idempotent per meeting.
    // `publications` records where a meeting went, and `repo_confirmations` that the user
    // confirmed publishing to a public repository. Nothing here holds a secret.
    "
    CREATE TABLE outbox (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        target TEXT NOT NULL CHECK (target IN ('github','issues','discord')),
        kind TEXT NOT NULL
            CHECK (kind IN ('commit_meeting','create_issue','set_issue_state','post_discord')),
        op_key TEXT NOT NULL,
        payload TEXT NOT NULL DEFAULT '{}',
        state TEXT NOT NULL CHECK (state IN ('queued','running','done','failed','cancelled')),
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_unix_ms INTEGER,
        last_error TEXT,
        result TEXT,
        after_id INTEGER REFERENCES outbox(id) ON DELETE SET NULL,
        created_unix_ms INTEGER NOT NULL,
        updated_unix_ms INTEGER NOT NULL,
        UNIQUE (meeting_id, op_key)
    );
    CREATE INDEX idx_outbox_due ON outbox(state, next_attempt_unix_ms);

    CREATE TABLE publications (
        meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
        cycle_id TEXT NOT NULL,
        folder TEXT,
        repo_key TEXT,
        commit_sha TEXT,
        report_url TEXT,
        snapshot_dir TEXT NOT NULL,
        published_unix_ms INTEGER
    );

    CREATE TABLE repo_confirmations (
        repo_key TEXT PRIMARY KEY,
        visibility TEXT NOT NULL,
        confirmed_unix_ms INTEGER NOT NULL
    );
    ",
    // Version 6: corrections to a published meeting. Two more kinds of outbox operation (a
    // correction note added to the meeting folder in the repository, a comment on the related
    // issue). A CHECK constraint cannot be altered, so the table is rebuilt with its rows,
    // ids and links kept.
    "
    CREATE TABLE outbox_v6 (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        target TEXT NOT NULL CHECK (target IN ('github','issues','discord')),
        kind TEXT NOT NULL
            CHECK (kind IN ('commit_meeting','create_issue','set_issue_state','post_discord',
                            'commit_correction','comment_correction')),
        op_key TEXT NOT NULL,
        payload TEXT NOT NULL DEFAULT '{}',
        state TEXT NOT NULL CHECK (state IN ('queued','running','done','failed','cancelled')),
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_unix_ms INTEGER,
        last_error TEXT,
        result TEXT,
        after_id INTEGER REFERENCES outbox(id) ON DELETE SET NULL,
        created_unix_ms INTEGER NOT NULL,
        updated_unix_ms INTEGER NOT NULL,
        UNIQUE (meeting_id, op_key)
    );
    INSERT INTO outbox_v6 (id, meeting_id, target, kind, op_key, payload, state, attempts,
                           next_attempt_unix_ms, last_error, result, after_id,
                           created_unix_ms, updated_unix_ms)
        SELECT id, meeting_id, target, kind, op_key, payload, state, attempts,
               next_attempt_unix_ms, last_error, result, after_id,
               created_unix_ms, updated_unix_ms FROM outbox;
    DROP TABLE outbox;
    ALTER TABLE outbox_v6 RENAME TO outbox;
    CREATE INDEX idx_outbox_due ON outbox(state, next_attempt_unix_ms);
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
    if current == LATEST {
        return Ok(());
    }
    // Rebuilding a table that other tables reference needs foreign keys off: with them on,
    // dropping the table would cascade into its children. The pragma has no effect inside
    // a transaction, so it is switched here and restored afterwards.
    let foreign_keys: bool = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let result = run_pending(conn, current);
    conn.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}

fn run_pending(conn: &Connection, current: u32) -> Result<()> {
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        // A migration must leave every reference intact.
        let broken: Option<String> = tx
            .query_row(
                "SELECT \"table\" FROM pragma_foreign_key_check LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(table) = broken {
            return Err(StoreError::Invalid(format!(
                "migration {} left a broken foreign key in {table}",
                i + 1
            )));
        }
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    /// A database as version 4 of the application left it, with a cycle meeting, a job and
    /// a participant with a voice sample.
    fn v4_database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_unix_ms, state, dir, cycle_id, report_mode)
               VALUES ('20261113-140501', 'Weekly', 5, 'stopped', 'm1', 'NOMAD', 'auto');
             INSERT INTO meeting_llm (meeting_id, kind) VALUES ('20261113-140501', 'managed');
             INSERT INTO jobs (meeting_id, created_unix_ms, updated_unix_ms)
               VALUES ('20261113-140501', 5, 5);
             INSERT INTO job_stages (meeting_id, stage, status, attempts, note)
               VALUES ('20261113-140501', 'reduce', 'done', 1, 'note');
             INSERT INTO participants (id, name, created_unix_ms, cycle_id)
               VALUES ('NOMAD:ola', 'Ola', 7, 'NOMAD');
             INSERT INTO voice_samples (participant_id, source_key, embedding, speech_ms,
                                        created_unix_ms)
               VALUES ('NOMAD:ola', 'm:v', x'0000803f', 9000, 9);",
        )
        .unwrap();
        conn
    }

    fn count(s: &Store, table: &str) -> i64 {
        s.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrating_a_version_4_database_adds_the_publishing_tables_and_keeps_the_data() {
        let s = Store::init(v4_database()).unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST);
        assert_eq!(LATEST, 6);

        // Existing data is untouched.
        let m = s.get_meeting("20261113-140501").unwrap().unwrap().meeting;
        assert_eq!(m.title, "Weekly");
        assert_eq!(
            s.get_job("20261113-140501").unwrap().unwrap().stages[0]
                .note
                .as_deref(),
            Some("note")
        );
        assert_eq!(s.participants_for_cycle("NOMAD").unwrap().len(), 1);
        assert_eq!(s.samples_of("NOMAD:ola").unwrap().len(), 1);

        // The new tables exist, are empty and are usable.
        for table in ["outbox", "publications", "repo_confirmations"] {
            assert_eq!(count(&s, table), 0, "{table}");
        }
        s.confirm_repo("github.com/o/r", "public", 1).unwrap();
        assert_eq!(count(&s, "repo_confirmations"), 1);
        let fk: i64 = s
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn a_migrated_database_opens_again_without_changes() {
        let dir = std::env::temp_dir().join(format!("bbb-store-v5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v4.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..4] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 4).unwrap();
        }
        for _ in 0..2 {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.schema_version().unwrap(), LATEST);
            assert_eq!(count(&s, "outbox"), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_outbox_refuses_unknown_targets_and_duplicate_keys() {
        let s = Store::init(v4_database()).unwrap();
        let insert = |target: &str, key: &str| {
            s.conn.execute(
                "INSERT INTO outbox (meeting_id, target, kind, op_key, state,
                                     created_unix_ms, updated_unix_ms)
                 VALUES ('20261113-140501', ?1, 'commit_meeting', ?2, 'queued', 1, 1)",
                rusqlite::params![target, key],
            )
        };
        assert!(insert("slack", "a").is_err());
        insert("github", "a").unwrap();
        assert!(insert("github", "a").is_err());
        // Deleting the meeting removes everything that belongs to it.
        s.delete_meeting("20261113-140501").unwrap();
        assert_eq!(count(&s, "outbox"), 0);
    }

    #[test]
    fn migrating_a_version_5_database_keeps_the_outbox_and_accepts_the_new_kinds() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            r#"INSERT INTO meetings (id, title, started_unix_ms, state, dir)
                 VALUES ('20261113-140501', 'Weekly', 5, 'stopped', 'm1');
               INSERT INTO outbox (id, meeting_id, target, kind, op_key, payload, state, attempts,
                                   last_error, result, after_id, created_unix_ms, updated_unix_ms)
                 VALUES (7, '20261113-140501', 'github', 'commit_meeting', 'commit', '{"a":1}',
                         'done', 2, NULL, '{"folder":"2026-11-13"}', NULL, 10, 11),
                        (9, '20261113-140501', 'issues', 'create_issue', 'issue:i1', '{}',
                         'failed', 8, 'boom', NULL, 7, 12, 13);
               INSERT INTO publications (meeting_id, cycle_id, folder, snapshot_dir)
                 VALUES ('20261113-140501', 'NOMAD', '2026-11-13', 'm1/work/publish/1');"#,
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST);

        let ops = s.list_ops(Some("20261113-140501")).unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!((ops[0].id, ops[0].attempts, ops[0].after_id), (7, 2, None));
        assert_eq!(ops[0].result.as_deref(), Some(r#"{"folder":"2026-11-13"}"#));
        assert_eq!((ops[1].id, ops[1].after_id), (9, Some(7)));
        assert_eq!(ops[1].last_error.as_deref(), Some("boom"));
        assert!(s.get_publication("20261113-140501").unwrap().is_some());

        // New rows continue after the old ids, can follow an old row and take the new kinds.
        let new_op = |kind, key: &str, after: &str| crate::NewOp {
            meeting_id: "20261113-140501".into(),
            target: crate::OutboxTarget::Github,
            kind,
            op_key: key.into(),
            payload: "{}".into(),
            after_op_key: Some(after.into()),
        };
        let (op, created) = s
            .enqueue_op(
                &new_op(
                    crate::OutboxKind::CommitCorrection,
                    "correction:c1",
                    "commit",
                ),
                20,
            )
            .unwrap();
        assert!(created && op.id > 9 && op.after_id == Some(7), "{op:?}");
        let (op, _) = s
            .enqueue_op(
                &new_op(
                    crate::OutboxKind::CommentCorrection,
                    "correction-issue:c1",
                    "correction:c1",
                ),
                21,
            )
            .unwrap();
        assert_eq!(op.kind, crate::OutboxKind::CommentCorrection);
        // The index of due operations survived the rebuild, and deleting the meeting cascades.
        let index: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_outbox_due'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
        s.delete_meeting("20261113-140501").unwrap();
        assert_eq!(count(&s, "outbox"), 0);
    }
}
