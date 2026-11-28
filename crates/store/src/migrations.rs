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
    // Version 7: scheduled job execution, participant preview clip files, and per-project
    // local preferences.
    "
    ALTER TABLE jobs ADD COLUMN scheduled_unix_ms INTEGER;
    ALTER TABLE participants ADD COLUMN sample_clip_file TEXT;
    CREATE TABLE IF NOT EXISTS project_prefs (
        cycle_id TEXT PRIMARY KEY,
        local_save_enabled INTEGER NOT NULL DEFAULT 0,
        local_save_dir TEXT NOT NULL DEFAULT '',
        llm_kind_override TEXT,
        llm_model_override TEXT
    );
    ",
    // Version 8: Notion export and editing a Discord message. A target and two more kinds of
    // outbox operation (a CHECK constraint cannot be altered, so the table is rebuilt with its
    // rows, ids and links kept), plus the table that remembers where a meeting's items and
    // posts live in external services (ids only, never secrets). Written to be re-runnable:
    // a database that already went through the former Stage 7 "version 7" has the same
    // shape, and a leftover `outbox_v8` from an interrupted run is dropped first.
    "
    DROP TABLE IF EXISTS outbox_v8;
    CREATE TABLE outbox_v8 (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        target TEXT NOT NULL CHECK (target IN ('github','issues','discord','notion')),
        kind TEXT NOT NULL
            CHECK (kind IN ('commit_meeting','create_issue','set_issue_state','post_discord',
                            'commit_correction','comment_correction','export_notion',
                            'edit_discord')),
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
    INSERT INTO outbox_v8 (id, meeting_id, target, kind, op_key, payload, state, attempts,
                           next_attempt_unix_ms, last_error, result, after_id,
                           created_unix_ms, updated_unix_ms)
        SELECT id, meeting_id, target, kind, op_key, payload, state, attempts,
               next_attempt_unix_ms, last_error, result, after_id,
               created_unix_ms, updated_unix_ms FROM outbox;
    DROP TABLE outbox;
    ALTER TABLE outbox_v8 RENAME TO outbox;
    CREATE INDEX IF NOT EXISTS idx_outbox_due ON outbox(state, next_attempt_unix_ms);

    CREATE TABLE IF NOT EXISTS external_refs (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        target TEXT NOT NULL CHECK (target IN ('notion','discord')),
        ref_key TEXT NOT NULL,
        external_id TEXT NOT NULL,
        url TEXT,
        extra TEXT NOT NULL DEFAULT '{}',
        updated_unix_ms INTEGER NOT NULL,
        PRIMARY KEY (meeting_id, target, ref_key)
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
    if current < LATEST {
        // Rebuilding a table that other tables reference needs foreign keys off: with them on,
        // dropping the table would cascade into its children. The pragma has no effect inside
        // a transaction, so it is switched here and restored afterwards.
        let foreign_keys: bool = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        let result = run_pending(conn, current);
        conn.pragma_update(None, "foreign_keys", foreign_keys)?;
        result?;
    }
    ensure_ux_schema(conn)?;
    ensure_people_schema(conn)?;
    ensure_integrations_schema(conn)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Idempotently ensures the UX Overhaul columns and `project_prefs` table exist, so merging
/// with another branch that also adds a version-7 migration is conflict-free.
fn ensure_ux_schema(conn: &Connection) -> Result<()> {
    if !has_column(conn, "jobs", "scheduled_unix_ms")? {
        conn.execute("ALTER TABLE jobs ADD COLUMN scheduled_unix_ms INTEGER", [])?;
    }
    if !has_column(conn, "participants", "sample_clip_file")? {
        conn.execute(
            "ALTER TABLE participants ADD COLUMN sample_clip_file TEXT",
            [],
        )?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_prefs (
            cycle_id TEXT PRIMARY KEY,
            local_save_enabled INTEGER NOT NULL DEFAULT 0,
            local_save_dir TEXT NOT NULL DEFAULT '',
            llm_kind_override TEXT,
            llm_model_override TEXT,
            audio_retention TEXT
        );",
    )?;
    // The per-project audio retention override (`keep`, `delete` or none = follow the global
    // setting) was added to a table that existing databases already have.
    if !has_column(conn, "project_prefs", "audio_retention")? {
        conn.execute(
            "ALTER TABLE project_prefs ADD COLUMN audio_retention TEXT",
            [],
        )?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_completions (
            cycle_id TEXT NOT NULL,
            task_key TEXT NOT NULL,
            done INTEGER NOT NULL DEFAULT 1,
            closed_by_meeting TEXT,
            updated_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (cycle_id, task_key)
        );",
    )?;
    Ok(())
}

/// Idempotently ensures the `project_people` join table exists, drops the legacy
/// `participants` name-uniqueness indexes (so two different people in different projects can
/// share a name without auto-merging), and backfills `project_people` from existing
/// `participants.cycle_id` rows when `project_people` is empty.
fn ensure_people_schema(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_people (
            cycle_id TEXT NOT NULL,
            participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
            cycle_participant_id TEXT,
            created_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (cycle_id, participant_id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_project_people_cycle_slug
            ON project_people(cycle_id, cycle_participant_id)
            WHERE cycle_participant_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_project_people_participant
            ON project_people(participant_id);
        DROP INDEX IF EXISTS idx_participants_name_local;
        DROP INDEX IF EXISTS idx_participants_name_cycle;",
    )?;
    let has_rows: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM project_people)", [], |r| {
        r.get(0)
    })?;
    if !has_rows {
        tx.execute(
            "INSERT INTO project_people (cycle_id, participant_id, cycle_participant_id, created_unix_ms)
             SELECT p.cycle_id,
                    p.id,
                    CASE
                        WHEN substr(p.id, 1, length(p.cycle_id) + 1) = p.cycle_id || ':'
                             AND length(p.id) > length(p.cycle_id) + 1
                        THEN substr(p.id, length(p.cycle_id) + 2)
                        ELSE NULL
                    END,
                    p.created_unix_ms
             FROM participants p
             WHERE p.cycle_id IS NOT NULL",
            [],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Idempotently ensures the `external_refs` table exists. The outbox rebuild that widens its
/// CHECK constraints is only done by migration version 8, never here.
fn ensure_integrations_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS external_refs (
            meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            target TEXT NOT NULL CHECK (target IN ('notion','discord')),
            ref_key TEXT NOT NULL,
            external_id TEXT NOT NULL,
            url TEXT,
            extra TEXT NOT NULL DEFAULT '{}',
            updated_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (meeting_id, target, ref_key)
        );",
    )?;
    Ok(())
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
        assert_eq!(LATEST, 8);

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
        for table in [
            "outbox",
            "publications",
            "repo_confirmations",
            "project_prefs",
        ] {
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
        let dir = crate::test_tmp::TestDir::new("store-v5");
        let path = dir.join("v4.db");
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

    #[test]
    fn migrating_a_version_6_database_adds_schedule_clip_and_project_prefs() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..6] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 6).unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_unix_ms, state, dir)
               VALUES ('m1', 'Weekly', 5, 'stopped', 'm1');
             INSERT INTO jobs (meeting_id, created_unix_ms, updated_unix_ms)
               VALUES ('m1', 5, 5);
             INSERT INTO participants (id, name, created_unix_ms)
               VALUES ('p1', 'Ola', 7);",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST);
        assert!(s.set_job_schedule("m1", Some(123_456)).unwrap());
        assert_eq!(
            s.get_job("m1").unwrap().unwrap().scheduled_unix_ms,
            Some(123_456)
        );
        assert!(s
            .set_participant_sample_clip("p1", Some(".voice-samples/p1.wav"))
            .unwrap());
        assert_eq!(
            s.list_all_participants_with_profiles().unwrap()[0]
                .sample_clip_file
                .as_deref(),
            Some(".voice-samples/p1.wav")
        );
    }

    #[test]
    fn a_version_7_database_without_the_audio_retention_column_gets_it() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        // A project saved before the override existed.
        conn.execute_batch(
            "INSERT INTO project_prefs (cycle_id, local_save_enabled, local_save_dir)
               VALUES ('NOMAD', 1, 'C:/Notes');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let old = s.get_project_prefs("NOMAD").unwrap();
        assert!(old.local_save_enabled);
        assert_eq!(old.audio_retention, None);
        let saved = crate::ProjectPrefs {
            audio_retention: Some("keep".into()),
            ..old
        };
        s.set_project_prefs(&saved).unwrap();
        assert_eq!(s.get_project_prefs("NOMAD").unwrap(), saved);
        // Opening again is harmless.
        ensure_ux_schema(&s.conn).unwrap();
    }

    #[test]
    fn ensure_ux_schema_creates_task_completions_idempotently() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        let s = Store::init(conn).unwrap();
        s.set_task_done("NOMAD", "2026-11-10/t1", true, Some("2026-11-12"), 1000)
            .unwrap();
        ensure_ux_schema(&s.conn).unwrap();
        let list = s.list_task_completions("NOMAD").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].task_key, "2026-11-10/t1");
        assert_eq!(list[0].closed_by_meeting.as_deref(), Some("2026-11-12"));
    }

    #[test]
    fn ensure_people_schema_backfills_project_people_without_auto_merging() {
        let conn = v4_database();
        // Add a second person with the same name in another cycle before migrating.
        conn.execute_batch(
            "INSERT INTO participants (id, name, created_unix_ms, cycle_id)
               VALUES ('ATLAS:ola', 'Ola', 8, 'ATLAS');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        // Both people keep their own id and project link; no auto-merge occurred.
        let nomad = s.participants_for_cycle("NOMAD").unwrap();
        let atlas = s.participants_for_cycle("ATLAS").unwrap();
        assert_eq!(nomad.len(), 1);
        assert_eq!(atlas.len(), 1);
        assert_eq!(nomad[0].id, "NOMAD:ola");
        assert_eq!(atlas[0].id, "ATLAS:ola");
        assert_eq!(nomad[0].cycle_participant_id.as_deref(), Some("ola"));
        assert_eq!(atlas[0].cycle_participant_id.as_deref(), Some("ola"));
        // Re-running ensure_people_schema is idempotent.
        ensure_people_schema(&s.conn).unwrap();
        assert_eq!(s.list_all_participants_with_profiles().unwrap().len(), 2);
    }

    const STAGE8_V7_ROWS: &str = r#"
        INSERT INTO meetings (id, title, started_unix_ms, state, dir, cycle_id)
          VALUES ('m1', 'Weekly', 5, 'stopped', 'm1', 'NOMAD');
        INSERT INTO outbox (id, meeting_id, target, kind, op_key, payload, state, attempts,
                            next_attempt_unix_ms, last_error, result, after_id,
                            created_unix_ms, updated_unix_ms)
          VALUES (7, 'm1', 'github', 'commit_meeting', 'commit', '{"a":1}',
                  'done', 2, NULL, NULL, '{"folder":"f"}', NULL, 10, 11),
                 (9, 'm1', 'discord', 'post_discord', 'discord', '{}',
                  'failed', 8, 99, 'boom', NULL, 7, 12, 13),
                 (12, 'm1', 'issues', 'create_issue', 'issue:i1', '{}',
                  'queued', 0, 5, NULL, NULL, 7, 14, 15);
        INSERT INTO publications (meeting_id, cycle_id, folder, snapshot_dir)
          VALUES ('m1', 'NOMAD', 'f', 'm1/work/publish/1');
        INSERT INTO project_prefs (cycle_id, local_save_enabled, local_save_dir)
          VALUES ('NOMAD', 1, 'C:/Notes');
        INSERT INTO task_completions (cycle_id, task_key, done, closed_by_meeting,
                                      updated_unix_ms)
          VALUES ('NOMAD', 'f/t1', 1, 'm1', 1000);"#;

    /// A database as the current stage-8 line leaves it at version 7, with data everywhere.
    fn stage8_v7_database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        ensure_ux_schema(&conn).unwrap();
        conn.execute_batch(STAGE8_V7_ROWS).unwrap();
        conn
    }

    fn outbox_dump(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT id || '|' || target || '|' || kind || '|' || op_key || '|' || state ||
                        '|' || attempts || '|' || IFNULL(next_attempt_unix_ms, 'n') || '|' ||
                        IFNULL(last_error, 'n') || '|' || IFNULL(result, 'n') || '|' ||
                        IFNULL(after_id, 'n') || '|' || meeting_id || '|' || payload ||
                        '|' || created_unix_ms || '|' || updated_unix_ms
                 FROM outbox ORDER BY id",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn a_populated_stage_8_version_7_database_upgrades_to_8_and_keeps_everything() {
        let conn = stage8_v7_database();
        let before = outbox_dump(&conn);
        assert_eq!(before.len(), 3);
        let s = Store::init(conn).unwrap();
        assert_eq!(s.schema_version().unwrap(), 8);
        assert_eq!(outbox_dump(&s.conn), before);
        let ops = s.list_ops(Some("m1")).unwrap();
        assert_eq!(
            (ops[1].id, ops[1].after_id, ops[1].attempts),
            (9, Some(7), 8)
        );
        assert_eq!(ops[1].last_error.as_deref(), Some("boom"));
        assert_eq!(ops[2].state, crate::OpState::Queued);
        assert!(s.get_publication("m1").unwrap().is_some());
        let prefs = s.get_project_prefs("NOMAD").unwrap();
        assert!(prefs.local_save_enabled);
        assert_eq!(prefs.audio_retention, None);
        assert!(has_column(&s.conn, "project_prefs", "audio_retention").unwrap());
        let people_table: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'project_people'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(people_table, 1);
        let done = s.list_task_completions("NOMAD").unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].closed_by_meeting.as_deref(), Some("m1"));
        let fk: i64 = s
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let index: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_outbox_due'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
        // after_id keeps ON DELETE SET NULL after the rebuild.
        let fk_action: String = s
            .conn
            .query_row(
                "SELECT \"on_delete\" FROM pragma_foreign_key_list('outbox')
                 WHERE \"from\" = 'after_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fk_action, "SET NULL");
        // The widened constraints and the continued ids work; deleting cascades.
        let (edit, _) = s
            .enqueue_op(
                &crate::NewOp {
                    meeting_id: "m1".into(),
                    target: crate::OutboxTarget::Discord,
                    kind: crate::OutboxKind::EditDiscord,
                    op_key: "discord:edit".into(),
                    payload: "{}".into(),
                    after_op_key: None,
                },
                19,
            )
            .unwrap();
        assert_eq!(edit.kind, crate::OutboxKind::EditDiscord);
        let (op, created) = s
            .enqueue_op(
                &crate::NewOp {
                    meeting_id: "m1".into(),
                    target: crate::OutboxTarget::Notion,
                    kind: crate::OutboxKind::ExportNotion,
                    op_key: "notion:i1".into(),
                    payload: "{}".into(),
                    after_op_key: Some("commit".into()),
                },
                20,
            )
            .unwrap();
        assert!(created && op.id > 12 && op.after_id == Some(7), "{op:?}");
        s.conn
            .execute(
                "INSERT INTO external_refs (meeting_id, target, ref_key, external_id,
                                            updated_unix_ms)
                 VALUES ('m1', 'notion', 'task:i1', 'page', 1)",
                [],
            )
            .unwrap();
        let ref_row: (String, String) = s
            .conn
            .query_row(
                "SELECT external_id, ref_key FROM external_refs WHERE meeting_id = 'm1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ref_row, ("page".to_string(), "task:i1".to_string()));
        // Deleting the op that op 9 follows clears its after_id instead of cascading.
        s.conn
            .execute("DELETE FROM outbox WHERE id = 7", [])
            .unwrap();
        let after: Option<i64> = s
            .conn
            .query_row("SELECT after_id FROM outbox WHERE id = 9", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, None);
        s.delete_meeting("m1").unwrap();
        assert_eq!(count(&s, "outbox"), 0);
        assert_eq!(count(&s, "external_refs"), 0);
    }

    #[test]
    fn a_stage_7_shaped_version_7_database_opens_and_keeps_its_notion_rows() {
        // What the former Stage 7 branch called version 7: the outbox already accepts
        // Notion, `external_refs` exists, and the UX columns of this line do not.
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for sql in &MIGRATIONS[..6] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "
            DROP TABLE outbox;
            CREATE TABLE outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                target TEXT NOT NULL CHECK (target IN ('github','issues','discord','notion')),
                kind TEXT NOT NULL
                    CHECK (kind IN ('commit_meeting','create_issue','set_issue_state',
                                    'post_discord','commit_correction','comment_correction',
                                    'export_notion','edit_discord')),
                op_key TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                state TEXT NOT NULL CHECK (state IN ('queued','running','done','failed',
                                                     'cancelled')),
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
            CREATE TABLE external_refs (
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                target TEXT NOT NULL CHECK (target IN ('notion','discord')),
                ref_key TEXT NOT NULL,
                external_id TEXT NOT NULL,
                url TEXT,
                extra TEXT NOT NULL DEFAULT '{}',
                updated_unix_ms INTEGER NOT NULL,
                PRIMARY KEY (meeting_id, target, ref_key)
            );
            INSERT INTO meetings (id, title, started_unix_ms, state, dir)
              VALUES ('m1', 'Weekly', 5, 'stopped', 'm1');
            INSERT INTO outbox (id, meeting_id, target, kind, op_key, state,
                                created_unix_ms, updated_unix_ms)
              VALUES (3, 'm1', 'notion', 'export_notion', 'notion:i1', 'done', 1, 2);
            INSERT INTO external_refs (meeting_id, target, ref_key, external_id,
                                       updated_unix_ms)
              VALUES ('m1', 'notion', 'task:i1', 'page', 1);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 7).unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.schema_version().unwrap(), 8);
        assert_eq!(count(&s, "outbox"), 1);
        assert_eq!(count(&s, "external_refs"), 1);
        assert!(has_column(&s.conn, "jobs", "scheduled_unix_ms").unwrap());
        assert!(has_column(&s.conn, "participants", "sample_clip_file").unwrap());
        assert_eq!(count(&s, "task_completions"), 0);
        let ops = s.list_ops(Some("m1")).unwrap();
        assert_eq!(
            (ops[0].id, ops[0].kind),
            (3, crate::OutboxKind::ExportNotion)
        );
    }

    #[test]
    fn reopening_a_version_8_database_changes_nothing() {
        let dir = crate::test_tmp::TestDir::new("store-v8-reopen");
        let path = dir.join("v7.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for sql in &MIGRATIONS[..7] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 7).unwrap();
            ensure_ux_schema(&conn).unwrap();
            conn.execute_batch(STAGE8_V7_ROWS).unwrap();
        }
        let schema = |s: &Store| -> Vec<String> {
            let mut stmt = s
                .conn
                .prepare(
                    "SELECT type || ':' || name || ':' || IFNULL(sql, '')
                     FROM sqlite_master ORDER BY type, name",
                )
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        let first = Store::open(&path).unwrap();
        let (schema_1, rows_1) = (schema(&first), outbox_dump(&first.conn));
        drop(first);
        for _ in 0..2 {
            let again = Store::open(&path).unwrap();
            assert_eq!(again.schema_version().unwrap(), LATEST);
            assert_eq!(schema(&again), schema_1);
            assert_eq!(outbox_dump(&again.conn), rows_1);
        }
    }

    #[test]
    fn version_8_can_run_again_over_a_finished_or_interrupted_rebuild() {
        let conn = stage8_v7_database();
        let before = outbox_dump(&conn);
        // A leftover of an interrupted run, then the whole step twice.
        conn.execute_batch("CREATE TABLE outbox_v8 (junk INTEGER);")
            .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(outbox_dump(&conn), before);
        conn.pragma_update(None, "user_version", 7).unwrap();
        migrate(&conn).unwrap();
        let fk: i64 = conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys restored");
        assert_eq!(outbox_dump(&conn), before);
        let leftovers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'outbox_v8'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_database_from_a_newer_schema_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();
        let err = Store::init(conn).err().unwrap();
        assert!(matches!(
            err,
            StoreError::SchemaTooNew {
                found: 9,
                supported: 8
            }
        ));
    }

    #[test]
    fn ensure_integrations_schema_is_idempotent_and_never_rebuilds_the_outbox() {
        let s = Store::init(stage8_v7_database()).unwrap();
        ensure_integrations_schema(&s.conn).unwrap();
        ensure_integrations_schema(&s.conn).unwrap();
        assert_eq!(count(&s, "external_refs"), 0);
        assert_eq!(count(&s, "outbox"), 3);
    }
}
