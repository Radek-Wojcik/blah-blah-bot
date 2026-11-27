//! Cycle data kept in the local database: which cycle a meeting belongs to, its report
//! settings, the choice of language-model backend with the confirmation record, and the
//! participants of a cycle.
//!
//! The cycle itself (its files, templates and topics) lives in the cycle repository and is
//! read by `cycle-store`; nothing here depends on it, so the values are plain strings that
//! are checked against the same vocabulary the schemas use.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::voices::cycle_participant_key;
use crate::{Meeting, Result, Store, StoreError};

/// Report modes, as in `schemas/meeting.schema.json`.
const REPORT_MODES: [&str; 4] = ["fixed", "template", "auto", "by-type"];
/// Output languages, as in `schemas/cycle.schema.json`.
const OUTPUT_LANGUAGES: [&str; 3] = ["auto", "pl", "en"];

/// Where the language model of the extraction stages runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmKind {
    /// A `llama-server` started by the worker from a GGUF file. Uses the GPU.
    Managed,
    /// An OpenAI-compatible server the user runs (Ollama, LM Studio, a remote machine).
    External,
    /// A hosted API, chosen for one meeting and confirmed by the user.
    Cloud,
}

impl LlmKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::External => "external",
            Self::Cloud => "cloud",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "managed" => Ok(Self::Managed),
            "external" => Ok(Self::External),
            "cloud" => Ok(Self::Cloud),
            other => Err(StoreError::Invalid(format!("unknown LLM kind {other:?}"))),
        }
    }
}

/// Cycle and report settings of one meeting. Every field but `cycle_id` is an override of
/// the cycle's defaults; `None` means "use the cycle's".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSettings {
    pub cycle_id: Option<String>,
    /// `fixed`, `template`, `auto` or `by-type`.
    pub report_mode: Option<String>,
    /// Template name (file name without `.md`) for template mode.
    pub report_template: Option<String>,
    /// Meeting type for by-type mode.
    pub meeting_type: Option<String>,
    /// `auto`, `pl` or `en`.
    pub output_language: Option<String>,
}

impl ReportSettings {
    fn validate(&self) -> Result<()> {
        if let Some(m) = &self.report_mode {
            if !REPORT_MODES.contains(&m.as_str()) {
                return Err(StoreError::Invalid(format!("unknown report mode {m:?}")));
            }
        }
        if let Some(l) = &self.output_language {
            if !OUTPUT_LANGUAGES.contains(&l.as_str()) {
                return Err(StoreError::Invalid(format!(
                    "unknown output language {l:?}"
                )));
            }
        }
        Ok(())
    }
}

/// The backend chosen for one meeting and, for the cloud, whether the user confirmed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmChoice {
    pub kind: LlmKind,
    /// The user confirmed sending this meeting's text to `endpoint_host`.
    pub cloud_confirmed: bool,
    pub confirmed_unix_ms: Option<i64>,
    /// Host of the external or cloud endpoint, shown in the confirmation dialog.
    pub endpoint_host: Option<String>,
}

/// What [`Store::sync_cycle_participants`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub created: Vec<String>,
    pub renamed: Vec<String>,
    /// Ids whose stored participant was taken over from a participant that is no longer in
    /// the list but had the same name (the id changed upstream, the person did not).
    pub rebound: Vec<String>,
    /// Participants that could not be stored; the rest of the list was synced.
    pub warnings: Vec<String>,
}

/// Local completion state of a project task (`task_completions` table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCompletion {
    pub cycle_id: String,
    pub task_key: String,
    pub done: bool,
    pub closed_by_meeting: Option<String>,
    pub updated_unix_ms: i64,
}

/// Per-project local preferences stored in the local database (never committed to Git).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPrefs {
    pub cycle_id: String,
    pub local_save_enabled: bool,
    pub local_save_dir: String,
    pub llm_kind_override: Option<String>,
    pub llm_model_override: Option<String>,
    /// What happens to the audio of this project's meetings after processing: `keep` or
    /// `delete`. `None` follows the global setting.
    pub audio_retention: Option<String>,
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    // ----- report settings -----

    /// Replace the cycle and report settings of a meeting. Returns `false` when the meeting
    /// does not exist.
    pub fn set_report_settings(&self, meeting_id: &str, s: &ReportSettings) -> Result<bool> {
        s.validate()?;
        Ok(self.conn.execute(
            "UPDATE meetings SET cycle_id = ?2, report_mode = ?3, report_template = ?4,
                    meeting_type = ?5, output_language = ?6
             WHERE id = ?1",
            params![
                meeting_id,
                s.cycle_id,
                s.report_mode,
                s.report_template,
                s.meeting_type,
                s.output_language
            ],
        )? > 0)
    }

    /// The cycle and report settings of a meeting, `None` when the meeting does not exist.
    /// A meeting that was never assigned has all fields `None`.
    pub fn get_report_settings(&self, meeting_id: &str) -> Result<Option<ReportSettings>> {
        Ok(self
            .conn
            .query_row(
                "SELECT cycle_id, report_mode, report_template, meeting_type, output_language
                 FROM meetings WHERE id = ?1",
                params![meeting_id],
                |r| {
                    Ok(ReportSettings {
                        cycle_id: r.get(0)?,
                        report_mode: r.get(1)?,
                        report_template: r.get(2)?,
                        meeting_type: r.get(3)?,
                        output_language: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Meetings of a cycle, newest first.
    pub fn list_meetings_by_cycle(&self, cycle_id: &str) -> Result<Vec<Meeting>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM meetings WHERE cycle_id = ?1
                 ORDER BY started_unix_ms DESC, id DESC",
            )?;
            let rows = stmt.query_map(params![cycle_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if let Some(d) = self.get_meeting(&id)? {
                out.push(d.meeting);
            }
        }
        Ok(out)
    }

    // ----- language-model backend of a meeting -----

    /// Record the backend chosen for a meeting. Choosing again, or changing the endpoint
    /// host, withdraws an earlier cloud confirmation: the user confirmed a specific
    /// destination. Returns `false` when the meeting does not exist.
    pub fn set_llm_choice(
        &self,
        meeting_id: &str,
        kind: LlmKind,
        endpoint_host: Option<&str>,
    ) -> Result<bool> {
        let keep = self
            .get_llm_choice(meeting_id)?
            .filter(|c| c.kind == kind && c.endpoint_host.as_deref() == endpoint_host)
            .map(|c| (c.cloud_confirmed, c.confirmed_unix_ms));
        let (confirmed, at) = keep.unwrap_or((false, None));
        match self.conn.execute(
            "INSERT INTO meeting_llm (meeting_id, kind, cloud_confirmed, confirmed_unix_ms,
                                      endpoint_host)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(meeting_id) DO UPDATE SET
               kind = excluded.kind, cloud_confirmed = excluded.cloud_confirmed,
               confirmed_unix_ms = excluded.confirmed_unix_ms,
               endpoint_host = excluded.endpoint_host",
            params![meeting_id, kind.as_str(), confirmed, at, endpoint_host],
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // The foreign key to the meeting failed.
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Record that the user confirmed sending this meeting's text to the cloud endpoint.
    /// Only a meeting whose choice is `cloud` and that records the destination host can be
    /// confirmed (a confirmation is for one host); returns whether it was.
    pub fn confirm_cloud(&self, meeting_id: &str, now_ms: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE meeting_llm SET cloud_confirmed = 1, confirmed_unix_ms = ?2
             WHERE meeting_id = ?1 AND kind = 'cloud'
               AND endpoint_host IS NOT NULL AND endpoint_host <> ''",
            params![meeting_id, now_ms],
        )? > 0)
    }

    /// Withdraw the cloud confirmation of a meeting.
    pub fn revoke_cloud_confirmation(&self, meeting_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE meeting_llm SET cloud_confirmed = 0, confirmed_unix_ms = NULL
             WHERE meeting_id = ?1",
            params![meeting_id],
        )? > 0)
    }

    pub fn get_llm_choice(&self, meeting_id: &str) -> Result<Option<LlmChoice>> {
        let row = self
            .conn
            .query_row(
                "SELECT kind, cloud_confirmed, confirmed_unix_ms, endpoint_host
                 FROM meeting_llm WHERE meeting_id = ?1",
                params![meeting_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(kind, confirmed, at, host)| {
            Ok(LlmChoice {
                kind: LlmKind::parse(&kind)?,
                cloud_confirmed: confirmed != 0,
                confirmed_unix_ms: at,
                endpoint_host: host,
            })
        })
        .transpose()
    }

    /// Forget the backend choice of a meeting.
    pub fn clear_llm_choice(&self, meeting_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM meeting_llm WHERE meeting_id = ?1",
            params![meeting_id],
        )? > 0)
    }

    // ----- per-project local preferences -----

    /// Read the local preferences for a project/cycle, returning defaults when none are stored.
    pub fn get_project_prefs(&self, cycle_id: &str) -> Result<ProjectPrefs> {
        let row = self
            .conn
            .query_row(
                "SELECT local_save_enabled, local_save_dir, llm_kind_override, llm_model_override,
                        audio_retention
                 FROM project_prefs WHERE cycle_id = ?1",
                params![cycle_id],
                |r| {
                    Ok(ProjectPrefs {
                        cycle_id: cycle_id.to_string(),
                        local_save_enabled: r.get::<_, i64>(0)? != 0,
                        local_save_dir: r.get(1)?,
                        llm_kind_override: r.get(2)?,
                        llm_model_override: r.get(3)?,
                        audio_retention: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(row.unwrap_or_else(|| ProjectPrefs {
            cycle_id: cycle_id.to_string(),
            ..ProjectPrefs::default()
        }))
    }

    /// Save local preferences for a project/cycle.
    pub fn set_project_prefs(&self, prefs: &ProjectPrefs) -> Result<()> {
        if prefs.cycle_id.trim().is_empty() {
            return Err(StoreError::Invalid("cycle_id cannot be empty".into()));
        }
        if let Some(kind) = &prefs.llm_kind_override {
            LlmKind::parse(kind)?;
        }
        if let Some(value) = &prefs.audio_retention {
            if !matches!(value.as_str(), "keep" | "delete") {
                return Err(StoreError::Invalid(format!(
                    "audio_retention must be \"keep\" or \"delete\", not {value:?}"
                )));
            }
        }
        self.conn.execute(
            "INSERT INTO project_prefs
               (cycle_id, local_save_enabled, local_save_dir, llm_kind_override, llm_model_override,
                audio_retention)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(cycle_id) DO UPDATE SET
               local_save_enabled = excluded.local_save_enabled,
               local_save_dir = excluded.local_save_dir,
               llm_kind_override = excluded.llm_kind_override,
               llm_model_override = excluded.llm_model_override,
               audio_retention = excluded.audio_retention",
            params![
                prefs.cycle_id,
                i64::from(prefs.local_save_enabled),
                prefs.local_save_dir,
                prefs.llm_kind_override,
                prefs.llm_model_override,
                prefs.audio_retention,
            ],
        )?;
        Ok(())
    }

    // ----- participants of a cycle -----

    /// Bring the stored participants of a cycle in line with `cycle.yaml`: a participant
    /// that is missing is created, one whose name changed is renamed. Nothing is ever
    /// deleted, so voice profiles survive a participant leaving the list or a rename.
    ///
    /// `participants` are `(id, name)` pairs as in `cycle.yaml`. The stored row's key is
    /// [`crate::cycle_participant_key`] of cycle and id (participants of two cycles may
    /// share an id); [`crate::ParticipantInfo::cycle_participant_id`] gives the id back.
    /// Names must differ within the list, ignoring case.
    ///
    /// When the list has an id that is not stored yet, with the name of a stored participant
    /// whose id is no longer in the list (the id changed upstream, the person did not), that
    /// participant is rebound to the new id and keeps its voice profile. A sync never fails
    /// for the whole cycle because of one participant: what cannot be stored is reported in
    /// [`SyncOutcome::warnings`].
    pub fn sync_cycle_participants<I: AsRef<str>, N: AsRef<str>>(
        &self,
        cycle_id: &str,
        participants: &[(I, N)],
    ) -> Result<SyncOutcome> {
        let mut seen = std::collections::HashSet::new();
        for (id, name) in participants {
            let (id, name) = (id.as_ref(), name.as_ref());
            if id.is_empty() || name.trim().is_empty() {
                return Err(StoreError::Invalid(
                    "a participant needs an id and a name".into(),
                ));
            }
            if !seen.insert(name.to_lowercase()) {
                return Err(StoreError::Invalid(format!(
                    "two participants are named {name:?}"
                )));
            }
        }
        let now = unix_ms();
        let tx = self.conn.unchecked_transaction()?;
        // Rebinding changes a primary key that voice rows refer to: check at commit.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let mut outcome = SyncOutcome::default();
        let incoming_keys: std::collections::HashSet<String> = participants
            .iter()
            .map(|(id, _)| cycle_participant_key(cycle_id, id.as_ref()))
            .collect();
        // Existing rows first, so a swap of two names cannot trip the unique index: rows
        // that change name get a placeholder, then the final name.
        let mut pending: Vec<(String, String, String)> = Vec::new();
        for (id, name) in participants {
            let (id, name) = (id.as_ref(), name.as_ref());
            let key = cycle_participant_key(cycle_id, id);
            let current: Option<String> = tx
                .query_row(
                    "SELECT name FROM participants WHERE id = ?1 AND cycle_id = ?2",
                    params![key, cycle_id],
                    |r| r.get(0),
                )
                .optional()?;
            match current {
                Some(current) if current == name => {}
                Some(_) => {
                    tx.execute(
                        "UPDATE participants SET name = ?2 WHERE id = ?1",
                        params![key, format!("\u{1}renaming:{key}")],
                    )?;
                    pending.push((key.clone(), name.to_string(), id.to_string()));
                    outcome.renamed.push(id.to_string());
                }
                None => {
                    pending.push((key.clone(), name.to_string(), id.to_string()));
                    outcome.created.push(id.to_string());
                }
            }
        }
        for (key, name, id) in pending {
            let updated = tx.execute(
                "UPDATE participants SET name = ?2 WHERE id = ?1 AND cycle_id = ?3",
                params![key, name, cycle_id],
            )?;
            if updated == 0 {
                // A stored participant of this cycle with this name whose id is gone from
                // the list is the same person under a new id.
                let holder: Option<String> = tx
                    .query_row(
                        "SELECT id FROM participants WHERE cycle_id = ?1 AND name = ?2",
                        params![cycle_id, name],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(old) = holder.filter(|old| !incoming_keys.contains(old)) {
                    tx.execute(
                        "UPDATE participants SET id = ?2, name = ?3 WHERE id = ?1",
                        params![old, key, name],
                    )?;
                    tx.execute(
                        "UPDATE voice_samples SET participant_id = ?2 WHERE participant_id = ?1",
                        params![old, key],
                    )?;
                    for column in ["matched_participant", "assigned_participant"] {
                        tx.execute(
                            &format!("UPDATE meeting_voices SET {column} = ?2 WHERE {column} = ?1"),
                            params![old, key],
                        )?;
                    }
                    outcome.created.retain(|c| *c != id);
                    outcome.rebound.push(id);
                    continue;
                }
                let inserted = tx.execute(
                    "INSERT INTO participants (id, name, created_unix_ms, cycle_id)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![key, name, now, cycle_id],
                );
                match inserted {
                    Ok(_) => {}
                    Err(rusqlite::Error::SqliteFailure(f, _))
                        if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        outcome.created.retain(|c| *c != id);
                        outcome.warnings.push(format!(
                            "the participant {name:?} could not be stored: the name is already used by another participant of the cycle"
                        ));
                    }
                    Err(other) => return Err(other.into()),
                }
            }
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// Removes all database records belonging to a project (`cycle_id`). When `delete_meetings`
    /// is true, meetings of the project are deleted (cascading to tracks, segments, jobs and
    /// publications); otherwise they are kept as standalone meetings (`cycle_id = NULL`).
    pub fn delete_cycle_data(&self, cycle_id: &str, delete_meetings: bool) -> Result<()> {
        if cycle_id.trim().is_empty() {
            return Err(StoreError::Invalid("cycle_id cannot be empty".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        if delete_meetings {
            tx.execute("DELETE FROM meetings WHERE cycle_id = ?1", params![cycle_id])?;
        } else {
            tx.execute(
                "UPDATE meetings SET cycle_id = NULL WHERE cycle_id = ?1",
                params![cycle_id],
            )?;
        }
        tx.execute(
            "DELETE FROM participants WHERE cycle_id = ?1",
            params![cycle_id],
        )?;
        tx.execute(
            "DELETE FROM project_prefs WHERE cycle_id = ?1",
            params![cycle_id],
        )?;
        tx.execute(
            "DELETE FROM task_completions WHERE cycle_id = ?1",
            params![cycle_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Marks a project task done (`done = true`) or removes its local completion (`done = false`).
    pub fn set_task_done(
        &self,
        cycle_id: &str,
        task_key: &str,
        done: bool,
        closed_by_meeting: Option<&str>,
        updated_unix_ms: i64,
    ) -> Result<()> {
        if cycle_id.trim().is_empty() || task_key.trim().is_empty() {
            return Err(StoreError::Invalid(
                "cycle_id and task_key cannot be empty".into(),
            ));
        }
        if done {
            self.conn.execute(
                "INSERT INTO task_completions
                    (cycle_id, task_key, done, closed_by_meeting, updated_unix_ms)
                 VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT(cycle_id, task_key) DO UPDATE SET
                    done = 1,
                    closed_by_meeting = excluded.closed_by_meeting,
                    updated_unix_ms = excluded.updated_unix_ms",
                params![cycle_id, task_key, closed_by_meeting, updated_unix_ms],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM task_completions WHERE cycle_id = ?1 AND task_key = ?2",
                params![cycle_id, task_key],
            )?;
        }
        Ok(())
    }

    /// Returns every task marked done in `cycle_id`.
    pub fn list_task_completions(&self, cycle_id: &str) -> Result<Vec<TaskCompletion>> {
        let mut stmt = self.conn.prepare(
            "SELECT cycle_id, task_key, done, closed_by_meeting, updated_unix_ms
             FROM task_completions
             WHERE cycle_id = ?1 AND done = 1",
        )?;
        let rows = stmt.query_map(params![cycle_id], |r| {
            Ok(TaskCompletion {
                cycle_id: r.get(0)?,
                task_key: r.get(1)?,
                done: r.get::<_, i64>(2)? != 0,
                closed_by_meeting: r.get(3)?,
                updated_unix_ms: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voices::SampleRow;
    use crate::MeetingState;
    use rusqlite::Connection;

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        for (id, started) in [("m1", 1), ("m2", 2), ("m3", 3)] {
            s.create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: started,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
        }
        s
    }

    fn settings(cycle: &str) -> ReportSettings {
        ReportSettings {
            cycle_id: Some(cycle.into()),
            report_mode: Some("by-type".into()),
            report_template: Some("weekly".into()),
            meeting_type: Some("technical".into()),
            output_language: Some("pl".into()),
        }
    }

    #[test]
    fn report_settings_round_trip_and_default_to_nothing() {
        let s = store();
        assert_eq!(
            s.get_report_settings("m1").unwrap(),
            Some(ReportSettings::default())
        );
        assert_eq!(s.get_report_settings("nope").unwrap(), None);
        assert!(s.set_report_settings("m1", &settings("NOMAD")).unwrap());
        assert_eq!(
            s.get_report_settings("m1").unwrap().unwrap(),
            settings("NOMAD")
        );
        assert!(s
            .set_report_settings("m1", &ReportSettings::default())
            .unwrap());
        assert_eq!(s.get_report_settings("m1").unwrap().unwrap().cycle_id, None);
        assert!(!s.set_report_settings("nope", &settings("X")).unwrap());
    }

    #[test]
    fn unknown_modes_and_languages_are_refused() {
        let s = store();
        let mut bad = settings("NOMAD");
        bad.report_mode = Some("freeform".into());
        assert!(matches!(
            s.set_report_settings("m1", &bad),
            Err(StoreError::Invalid(_))
        ));
        let mut bad = settings("NOMAD");
        bad.output_language = Some("de".into());
        assert!(matches!(
            s.set_report_settings("m1", &bad),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn meetings_are_listed_by_cycle_newest_first() {
        let s = store();
        s.set_report_settings("m1", &settings("NOMAD")).unwrap();
        s.set_report_settings("m3", &settings("NOMAD")).unwrap();
        s.set_report_settings("m2", &settings("OTHER")).unwrap();
        let ids: Vec<_> = s
            .list_meetings_by_cycle("NOMAD")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["m3", "m1"]);
        assert!(s.list_meetings_by_cycle("NONE").unwrap().is_empty());
    }

    #[test]
    fn the_llm_choice_and_its_confirmation_are_recorded() {
        let s = store();
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);
        assert!(s
            .set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap());
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert_eq!(c.kind, LlmKind::Cloud);
        assert!(!c.cloud_confirmed);
        assert!(s.confirm_cloud("m1", 42).unwrap());
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert!(c.cloud_confirmed);
        assert_eq!(c.confirmed_unix_ms, Some(42));
        assert_eq!(c.endpoint_host.as_deref(), Some("api.example.com"));

        // The same choice again keeps the confirmation.
        s.set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap();
        assert!(s.get_llm_choice("m1").unwrap().unwrap().cloud_confirmed);
        // Another host is another destination: the confirmation is gone.
        s.set_llm_choice("m1", LlmKind::Cloud, Some("other.example.com"))
            .unwrap();
        assert!(!s.get_llm_choice("m1").unwrap().unwrap().cloud_confirmed);
        // Switching kind drops it too, and only the cloud kind can be confirmed.
        s.confirm_cloud("m1", 50).unwrap();
        s.set_llm_choice("m1", LlmKind::External, Some("127.0.0.1"))
            .unwrap();
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert!(!c.cloud_confirmed && c.confirmed_unix_ms.is_none());
        assert!(!s.confirm_cloud("m1", 60).unwrap());

        assert!(s.clear_llm_choice("m1").unwrap());
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);
    }

    #[test]
    fn a_confirmation_needs_the_destination_host() {
        let s = store();
        for host in [None, Some("")] {
            s.set_llm_choice("m1", LlmKind::Cloud, host).unwrap();
            assert!(!s.confirm_cloud("m1", 1).unwrap(), "{host:?}");
            assert!(!s.get_llm_choice("m1").unwrap().unwrap().cloud_confirmed);
        }
        s.set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap();
        assert!(s.confirm_cloud("m1", 1).unwrap());
    }

    #[test]
    fn a_confirmation_can_be_revoked_and_unknown_meetings_are_refused() {
        let s = store();
        s.set_llm_choice("m1", LlmKind::Cloud, Some("h")).unwrap();
        s.confirm_cloud("m1", 1).unwrap();
        assert!(s.revoke_cloud_confirmation("m1").unwrap());
        assert!(!s.get_llm_choice("m1").unwrap().unwrap().cloud_confirmed);
        assert!(!s.set_llm_choice("ghost", LlmKind::Managed, None).unwrap());
    }

    #[test]
    fn deleting_a_meeting_removes_its_llm_choice() {
        let s = store();
        s.set_llm_choice("m1", LlmKind::Managed, None).unwrap();
        s.delete_meeting("m1").unwrap();
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn syncing_creates_missing_participants_and_renames_changed_ones() {
        let s = store();
        let out = s
            .sync_cycle_participants("NOMAD", &pairs(&[("alex", "Alex"), ("jan", "Jan")]))
            .unwrap();
        assert_eq!(out.created, ["alex", "jan"]);
        let list = s.participants_for_cycle("NOMAD").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].cycle_participant_id(), Some("alex"));
        assert_eq!(list[0].cycle_id.as_deref(), Some("NOMAD"));

        // Rename keeps the row (and so the voice profile); the same list is a no-op.
        s.put_sample(
            &list[0].id,
            &SampleRow {
                source_key: "m:v".into(),
                embedding: vec![1.0],
                speech_ms: 9_000,
            },
            1,
        )
        .unwrap();
        let out = s
            .sync_cycle_participants("NOMAD", &pairs(&[("alex", "Alex Example"), ("jan", "Jan")]))
            .unwrap();
        assert_eq!(
            (out.created.len(), out.renamed.as_slice()),
            (0, &["alex".to_string()][..])
        );
        let list = s.participants_for_cycle("NOMAD").unwrap();
        assert_eq!(list[0].name, "Alex Example");
        assert_eq!(list[0].sample_count, 1);
        let again = s
            .sync_cycle_participants("NOMAD", &pairs(&[("alex", "Alex Example"), ("jan", "Jan")]))
            .unwrap();
        assert_eq!(again, SyncOutcome::default());
    }

    #[test]
    fn syncing_never_deletes_and_can_swap_names() {
        let s = store();
        s.sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("b", "Jan"), ("c", "Ewa")]))
            .unwrap();
        // Ewa leaves the list, Ola and Jan swap names.
        s.sync_cycle_participants("C", &pairs(&[("a", "Jan"), ("b", "Ola")]))
            .unwrap();
        let names: Vec<_> = s
            .participants_for_cycle("C")
            .unwrap()
            .into_iter()
            .map(|p| (p.cycle_participant_id().unwrap().to_string(), p.name))
            .collect();
        assert!(names.contains(&("a".into(), "Jan".into())));
        assert!(names.contains(&("b".into(), "Ola".into())));
        assert!(names.contains(&("c".into(), "Ewa".into())));
    }

    #[test]
    fn syncing_refuses_duplicate_names_and_leaves_nothing_half_done() {
        let s = store();
        assert!(s
            .sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("b", "ola")]))
            .is_err());
        s.sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("c", "Ewa")]))
            .unwrap();
        assert_eq!(s.participants_for_cycle("C").unwrap().len(), 2);
    }

    #[test]
    fn a_changed_id_with_the_same_name_rebinds_the_participant_and_keeps_its_voice() {
        let s = store();
        s.sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("c", "Ewa")]))
            .unwrap();
        let old_key = cycle_participant_key("C", "c");
        s.put_sample(
            &old_key,
            &SampleRow {
                source_key: "m:v".into(),
                embedding: vec![1.0],
                speech_ms: 9_000,
            },
            1,
        )
        .unwrap();
        // The id of Ewa changes upstream; the name stays. This used to fail for good.
        let out = s
            .sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("ewa", "Ewa")]))
            .unwrap();
        assert_eq!(out.rebound, ["ewa"]);
        assert!(out.created.is_empty() && out.warnings.is_empty(), "{out:?}");
        let list = s.participants_for_cycle("C").unwrap();
        assert_eq!(list.len(), 2);
        let ewa = list.iter().find(|p| p.name == "Ewa").unwrap();
        assert_eq!(ewa.id, cycle_participant_key("C", "ewa"));
        assert_eq!(ewa.cycle_participant_id(), Some("ewa"));
        assert_eq!(ewa.sample_count, 1);
        assert!(list.iter().all(|p| p.id != old_key));
        // The same list again changes nothing.
        assert_eq!(
            s.sync_cycle_participants("C", &pairs(&[("a", "Ola"), ("ewa", "Ewa")]))
                .unwrap(),
            SyncOutcome::default()
        );
    }

    #[test]
    fn rebinding_moves_voice_assignments_of_meetings_too() {
        let s = store();
        s.sync_cycle_participants("C", &pairs(&[("old", "Ewa")]))
            .unwrap();
        let old_key = cycle_participant_key("C", "old");
        s.conn
            .execute(
                "INSERT INTO meeting_voices (meeting_id, voice_id, track, label, embedding, speech_ms, decision, matched_participant, assigned_participant)
                 VALUES ('m1', 'v1', 'mic', 'Voice 1', x'00', 1000, 'auto', ?1, ?1)",
                params![old_key],
            )
            .unwrap();
        s.sync_cycle_participants("C", &pairs(&[("new", "ewa")]))
            .unwrap();
        let (matched, assigned): (Option<String>, Option<String>) = s
            .conn
            .query_row(
                "SELECT matched_participant, assigned_participant FROM meeting_voices WHERE voice_id = 'v1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let new_key = cycle_participant_key("C", "new");
        assert_eq!((matched, assigned), (Some(new_key.clone()), Some(new_key)));
        // The stored name follows the list.
        assert_eq!(s.participants_for_cycle("C").unwrap()[0].name, "ewa");
    }

    #[test]
    fn names_are_unique_per_cycle_and_cycles_are_kept_apart() {
        let s = store();
        s.sync_cycle_participants("A", &pairs(&[("ola", "Ola")]))
            .unwrap();
        // The same person, same id and name, in another cycle: a separate participant.
        s.sync_cycle_participants("B", &pairs(&[("ola", "Ola")]))
            .unwrap();
        s.create_participant("legacy-ola", "Ola", 1).unwrap();
        assert!(s.create_participant("legacy-2", "OLA", 1).is_err());

        let in_a = s.participants_for_cycle("A").unwrap();
        let in_b = s.participants_for_cycle("B").unwrap();
        assert_eq!((in_a.len(), in_b.len()), (1, 1));
        assert_ne!(in_a[0].id, in_b[0].id);
        assert_eq!(s.list_participants_for(None).unwrap().len(), 1);
        assert_eq!(s.list_participants().unwrap().len(), 3);

        let a = s.find_participant_by_name_in(Some("A"), "ola").unwrap();
        assert_eq!(a.unwrap().id, in_a[0].id);
        assert_eq!(
            s.find_participant_by_name("Ola").unwrap().unwrap().id,
            "legacy-ola"
        );
        assert!(s
            .find_participant_by_name_in(Some("C"), "Ola")
            .unwrap()
            .is_none());
    }

    #[test]
    fn voice_profiles_are_looked_up_per_cycle() {
        let s = store();
        s.sync_cycle_participants("A", &pairs(&[("ola", "Ola")]))
            .unwrap();
        s.create_participant("legacy", "Jan", 1).unwrap();
        let sample = SampleRow {
            source_key: "k".into(),
            embedding: vec![1.0, 0.0],
            speech_ms: 9_000,
        };
        let a_key = s.participants_for_cycle("A").unwrap()[0].id.clone();
        s.put_sample(&a_key, &sample, 1).unwrap();
        s.put_sample("legacy", &sample, 1).unwrap();
        let in_a: Vec<_> = s
            .profiles_for(Some("A"))
            .unwrap()
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(in_a, [a_key]);
        let local: Vec<_> = s
            .profiles_for(None)
            .unwrap()
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(local, ["legacy"]);
        assert!(s.profiles_for(Some("B")).unwrap().is_empty());
    }

    /// A database as version 3 of the application left it.
    fn v3_database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // Run the first three migrations only.
        for sql in &crate::migrations::MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_unix_ms, state, dir)
               VALUES ('m1', 'Old meeting', 5, 'stopped', 'm1');
             INSERT INTO jobs (meeting_id, created_unix_ms, updated_unix_ms, options)
               VALUES ('m1', 5, 5, '{\"language\":\"pl\"}');
             INSERT INTO job_stages (meeting_id, stage, status, attempts)
               VALUES ('m1', 'asr', 'done', 1);
             INSERT INTO participants (id, name, created_unix_ms) VALUES ('p1', 'Ola', 7);
             INSERT INTO participants (id, name, created_unix_ms) VALUES ('p2', 'Jan', 8);
             INSERT INTO voice_samples (participant_id, source_key, embedding, speech_ms,
                                        created_unix_ms)
               VALUES ('p1', 'm1:v', x'0000803f', 9000, 9);
             INSERT INTO meeting_voices (meeting_id, voice_id, track, label, embedding,
                                         speech_ms, decision, assigned_participant, confirmed)
               VALUES ('m1', 't:0', 't', 'SPEAKER_00', x'0000803f', 9000, 'auto', 'p1', 1);
             INSERT INTO voice_clips (meeting_id, voice_id, idx, file, start_ms, end_ms,
                                      created_unix_ms)
               VALUES ('m1', 't:0', 0, 'c.wav', 0, 1000, 10);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn migrating_a_version_3_database_keeps_everything() {
        let s = Store::init(v3_database()).unwrap();
        assert_eq!(s.schema_version().unwrap(), crate::migrations::LATEST);

        // Meeting and job survive, with empty cycle fields and no stage note.
        let m = s.get_meeting("m1").unwrap().unwrap().meeting;
        assert_eq!(m.title, "Old meeting");
        assert_eq!(
            s.get_report_settings("m1").unwrap().unwrap(),
            ReportSettings::default()
        );
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);
        let job = s.get_job("m1").unwrap().unwrap();
        assert_eq!(job.options, "{\"language\":\"pl\"}");
        assert_eq!(job.stages[0].note, None);

        // Participants keep id, name and creation time, and join the legacy list.
        let list = s.list_participants_for(None).unwrap();
        assert_eq!(list.len(), 2);
        let ola = s.find_participant_by_name("ola").unwrap().unwrap();
        assert_eq!((ola.id.as_str(), ola.created_unix_ms), ("p1", 7));
        assert_eq!(ola.cycle_id, None);

        // Voice data was not cascaded away by the rebuild of `participants`.
        assert_eq!(s.samples_of("p1").unwrap().len(), 1);
        let voices = s.list_meeting_voices("m1").unwrap();
        assert_eq!(voices[0].assigned_participant.as_deref(), Some("p1"));
        assert_eq!(s.list_clips("m1").unwrap().len(), 1);
        assert_eq!(s.profiles_for(None).unwrap().len(), 1);

        // The new constraints are live: foreign keys are back on, the name is unique per
        // cycle, and a cycle participant may reuse a legacy name.
        let fk: i64 = s
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        assert!(s.create_participant("p3", "OLA", 1).is_err());
        s.create_participant_in(Some("NOMAD"), "NOMAD:ola", "Ola", 1)
            .unwrap();
        s.delete_participant("p1").unwrap();
        assert_eq!(
            s.list_meeting_voices("m1").unwrap()[0].assigned_participant,
            None
        );
    }

    #[test]
    fn a_migrated_database_opens_again_unchanged() {
        let dir = crate::test_tmp::TestDir::new("store-v4");
        let path = dir.path().join("v3.db");
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &crate::migrations::MIGRATIONS[..3] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 3).unwrap();
            conn.execute_batch(
                "INSERT INTO participants (id, name, created_unix_ms) VALUES ('p1', 'Ola', 7);",
            )
            .unwrap();
        }
        for _ in 0..2 {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.schema_version().unwrap(), crate::migrations::LATEST);
            assert_eq!(s.list_participants().unwrap().len(), 1);
        }
    }

    #[test]
    fn project_prefs_round_trip_and_default_when_missing() {
        let s = store();
        let def = s.get_project_prefs("NOMAD").unwrap();
        assert_eq!(
            def,
            ProjectPrefs {
                cycle_id: "NOMAD".into(),
                local_save_enabled: false,
                local_save_dir: String::new(),
                llm_kind_override: None,
                llm_model_override: None,
                audio_retention: None,
            }
        );

        let updated = ProjectPrefs {
            cycle_id: "NOMAD".into(),
            local_save_enabled: true,
            local_save_dir: "C:/Meetings/NOMAD".into(),
            llm_kind_override: Some("external".into()),
            llm_model_override: Some("gemma-4".into()),
            audio_retention: Some("keep".into()),
        };
        s.set_project_prefs(&updated).unwrap();
        assert_eq!(s.get_project_prefs("NOMAD").unwrap(), updated);

        let cleared = ProjectPrefs {
            cycle_id: "NOMAD".into(),
            local_save_enabled: false,
            local_save_dir: String::new(),
            llm_kind_override: None,
            llm_model_override: None,
            audio_retention: None,
        };
        s.set_project_prefs(&cleared).unwrap();
        assert_eq!(s.get_project_prefs("NOMAD").unwrap(), cleared);

        // Only `keep` and `delete` are valid overrides.
        let bad = ProjectPrefs {
            audio_retention: Some("forever".into()),
            ..cleared.clone()
        };
        assert!(matches!(
            s.set_project_prefs(&bad),
            Err(StoreError::Invalid(_))
        ));
        let deleting = ProjectPrefs {
            audio_retention: Some("delete".into()),
            ..cleared
        };
        s.set_project_prefs(&deleting).unwrap();
        assert_eq!(s.get_project_prefs("NOMAD").unwrap(), deleting);
    }

    #[test]
    fn delete_cycle_data_keeps_meetings_standalone_or_deletes_them() {
        let s = store();
        s.set_report_settings("m1", &settings("NOMAD")).unwrap();
        s.set_report_settings("m2", &settings("OTHER")).unwrap();
        s.sync_cycle_participants("NOMAD", &[("ola", "Ola")])
            .unwrap();
        s.set_project_prefs(&ProjectPrefs {
            cycle_id: "NOMAD".into(),
            local_save_enabled: true,
            local_save_dir: "D:/Notes".into(),
            llm_kind_override: None,
            llm_model_override: None,
            audio_retention: None,
        })
        .unwrap();

        // Keep meetings as standalone.
        s.delete_cycle_data("NOMAD", false).unwrap();
        assert_eq!(s.list_meetings_by_cycle("NOMAD").unwrap().len(), 0);
        assert!(s.get_meeting("m1").unwrap().is_some());
        assert_eq!(s.get_report_settings("m1").unwrap().unwrap().cycle_id, None);
        assert!(s.participants_for_cycle("NOMAD").unwrap().is_empty());
        assert!(!s.get_project_prefs("NOMAD").unwrap().local_save_enabled);
        assert_eq!(s.list_meetings_by_cycle("OTHER").unwrap().len(), 1);

        // Delete meetings when requested.
        s.delete_cycle_data("OTHER", true).unwrap();
        assert!(s.get_meeting("m2").unwrap().is_none());
    }

    #[test]
    fn set_task_done_and_list_task_completions_round_trip() {
        let s = store();
        s.set_task_done("NOMAD", "2026-11-10/t1", true, None, 100)
            .unwrap();
        s.set_task_done("NOMAD", "#12", true, Some("2026-11-12"), 200)
            .unwrap();
        let list = s.list_task_completions("NOMAD").unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|c| c.task_key == "2026-11-10/t1" && c.done));
        assert!(list
            .iter()
            .any(|c| c.task_key == "#12" && c.closed_by_meeting.as_deref() == Some("2026-11-12")));

        // Unticking removes the completion row.
        s.set_task_done("NOMAD", "2026-11-10/t1", false, None, 300)
            .unwrap();
        let after = s.list_task_completions("NOMAD").unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].task_key, "#12");
    }
}
