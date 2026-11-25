//! Participants, voice profiles, detected voices and their sample clips.
//!
//! Embeddings are biometric data. They live only in this local database; nothing here is
//! ever sent anywhere.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Result, Store, StoreError};

/// A person who can be named as a speaker. `cycle_id` is the cycle the participant belongs
/// to; `None` is the legacy local list of participants made before cycles existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub created_unix_ms: i64,
    pub cycle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_clip_file: Option<String>,
}

/// A participant with the size of the voice profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: String,
    pub name: String,
    pub sample_count: u32,
    pub speech_ms: u64,
    pub cycle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_participant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_clip_file: Option<String>,
}

/// Separates the cycle id from the participant id in the key of a cycle participant.
const KEY_SEPARATOR: char = ':';

/// Row key of a cycle participant. Participants of different cycles may share an id in
/// their `cycle.yaml` (the same person in two cycles), so the key carries the cycle; the
/// voice profile of the same person in two cycles is deliberately kept apart.
pub fn cycle_participant_key(cycle_id: &str, participant_id: &str) -> String {
    format!("{cycle_id}{KEY_SEPARATOR}{participant_id}")
}

/// One confirmed voice of a participant.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleRow {
    pub source_key: String,
    pub embedding: Vec<f32>,
    pub speech_ms: u64,
}

/// How the matcher judged a detected voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceDecision {
    Unknown,
    Auto,
    Suggested,
}

impl VoiceDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Auto => "auto",
            Self::Suggested => "suggested",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "auto" => Self::Auto,
            "suggested" => Self::Suggested,
            _ => Self::Unknown,
        }
    }
}

/// A voice detected in one meeting.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceRow {
    /// `track:label`, unique within the meeting.
    pub voice_id: String,
    pub track: String,
    pub label: String,
    pub embedding: Vec<f32>,
    pub speech_ms: u64,
    pub decision: VoiceDecision,
    pub score: Option<f32>,
    /// The participant the matcher found (applied when `decision` is `Auto`).
    pub matched_participant: Option<String>,
    /// The participant the voice is named after: set by an automatic match or by the user.
    pub assigned_participant: Option<String>,
    /// The user has decided about this voice (named it or left it unnamed on purpose).
    pub confirmed: bool,
    /// Another voice of the same meeting that this one is the same person as.
    pub merged_into: Option<String>,
}

/// A sample clip of a detected voice. `file` is relative to the recordings root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipRow {
    pub voice_id: String,
    pub idx: u32,
    pub file: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Little-endian `f32` bytes of an embedding.
pub fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Inverse of [`embedding_to_blob`]; trailing bytes that do not fill a float are ignored.
pub fn blob_to_embedding(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

type ParticipantProjects = HashMap<String, Vec<(String, Option<String>)>>;

impl Store {
    // ----- participants -----

    /// Create a participant of the legacy local list (no cycle).
    pub fn create_participant(&self, id: &str, name: &str, now_ms: i64) -> Result<()> {
        self.create_participant_in(None, id, name, now_ms)
    }

    /// Create a participant of a cycle (or of the global list with `None`).
    pub fn create_participant_in(
        &self,
        cycle_id: Option<&str>,
        id: &str,
        name: &str,
        now_ms: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO participants (id, name, created_unix_ms, cycle_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, name, now_ms, cycle_id],
        )?;
        if let Some(cid) = cycle_id {
            let slug = id
                .strip_prefix(cid)
                .and_then(|r| r.strip_prefix(KEY_SEPARATOR));
            tx.execute(
                "INSERT INTO project_people (cycle_id, participant_id, cycle_participant_id, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(cycle_id, participant_id) DO NOTHING",
                params![cid, id, slug, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Find a participant by id.
    pub fn get_participant(&self, id: &str) -> Result<Option<Participant>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, created_unix_ms, cycle_id, sample_clip_file
                 FROM participants WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Participant {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        created_unix_ms: r.get(2)?,
                        cycle_id: r.get(3)?,
                        sample_clip_file: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Find a participant of the legacy local list by name.
    pub fn find_participant_by_name(&self, name: &str) -> Result<Option<Participant>> {
        self.find_participant_by_name_in(None, name)
    }

    /// Find a participant of one cycle (or of the unlinked global list with `None`) by name.
    /// The participants of other cycles are never found.
    pub fn find_participant_by_name_in(
        &self,
        cycle_id: Option<&str>,
        name: &str,
    ) -> Result<Option<Participant>> {
        let row = match cycle_id {
            Some(cid) => self
                .conn
                .query_row(
                    "SELECT p.id, p.name, p.created_unix_ms, p.cycle_id, p.sample_clip_file
                     FROM participants p
                     JOIN project_people pp ON pp.participant_id = p.id
                     WHERE pp.cycle_id = ?1 AND p.name = ?2 COLLATE NOCASE
                     ORDER BY p.created_unix_ms, p.id
                     LIMIT 1",
                    params![cid, name],
                    |r| {
                        Ok(Participant {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            created_unix_ms: r.get(2)?,
                            cycle_id: r.get(3)?,
                            sample_clip_file: r.get(4)?,
                        })
                    },
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    "SELECT p.id, p.name, p.created_unix_ms, p.cycle_id, p.sample_clip_file
                     FROM participants p
                     WHERE p.cycle_id IS NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM project_people pp WHERE pp.participant_id = p.id
                       )
                       AND p.name = ?1 COLLATE NOCASE
                     ORDER BY p.created_unix_ms, p.id
                     LIMIT 1",
                    params![name],
                    |r| {
                        Ok(Participant {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            created_unix_ms: r.get(2)?,
                            cycle_id: r.get(3)?,
                            sample_clip_file: r.get(4)?,
                        })
                    },
                )
                .optional()?,
        };
        Ok(row)
    }

    /// Set or clear the retained preview clip path (relative to recordings root) for a participant.
    pub fn set_participant_sample_clip(
        &self,
        participant_id: &str,
        file: Option<&str>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE participants SET sample_clip_file = ?2 WHERE id = ?1",
            params![participant_id, file],
        )? > 0)
    }

    pub fn rename_participant(&self, id: &str, name: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE participants SET name = ?2 WHERE id = ?1",
            params![id, name],
        )? > 0)
    }

    /// Delete a participant together with the voice profile. Detected voices that carried the
    /// name become unnamed.
    pub fn delete_participant(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM participants WHERE id = ?1", params![id])?
            > 0)
    }

    /// Every participant of every cycle and of the local list.
    pub fn list_participants(&self) -> Result<Vec<ParticipantInfo>> {
        self.query_participants(None)
    }

    /// Every participant across all cycles and global (`cycle_id IS NULL`) participants,
    /// including their `cycle_id`, `sample_count`, `speech_ms` and `sample_clip_file`.
    pub fn list_all_participants_with_profiles(&self) -> Result<Vec<ParticipantInfo>> {
        self.query_participants(None)
    }

    /// The participants a meeting of this cycle can use: the cycle's own, none of the others.
    /// `None` is the legacy local list, which is what meetings without a cycle use.
    pub fn list_participants_for(&self, cycle_id: Option<&str>) -> Result<Vec<ParticipantInfo>> {
        self.query_participants(Some(cycle_id))
    }

    /// The participants of one cycle.
    pub fn participants_for_cycle(&self, cycle_id: &str) -> Result<Vec<ParticipantInfo>> {
        self.list_participants_for(Some(cycle_id))
    }

    fn projects_by_participant(&self) -> Result<ParticipantProjects> {
        let mut stmt = self.conn.prepare(
            "SELECT participant_id, cycle_id, cycle_participant_id
             FROM project_people
             ORDER BY participant_id, cycle_id",
        )?;
        let mut map: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (pid, cid, slug) = row?;
            map.entry(pid).or_default().push((cid, slug));
        }
        Ok(map)
    }

    fn query_participants(&self, scope: Option<Option<&str>>) -> Result<Vec<ParticipantInfo>> {
        let projects_map = self.projects_by_participant()?;
        let (sql, cycle_param): (String, Option<&str>) = match scope {
            Some(Some(cid)) => (
                "SELECT p.id, p.name, COUNT(s.source_key), COALESCE(SUM(s.speech_ms), 0),
                        pp.cycle_id, pp.cycle_participant_id, p.sample_clip_file
                 FROM participants p
                 JOIN project_people pp ON pp.participant_id = p.id
                 LEFT JOIN voice_samples s ON s.participant_id = p.id
                 WHERE pp.cycle_id = ?1
                 GROUP BY p.id, pp.cycle_id, pp.cycle_participant_id
                 ORDER BY p.name COLLATE NOCASE, p.id"
                    .to_string(),
                Some(cid),
            ),
            Some(None) => (
                "SELECT p.id, p.name, COUNT(s.source_key), COALESCE(SUM(s.speech_ms), 0),
                        NULL, NULL, p.sample_clip_file
                 FROM participants p
                 LEFT JOIN voice_samples s ON s.participant_id = p.id
                 WHERE p.cycle_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM project_people pp WHERE pp.participant_id = p.id
                   )
                 GROUP BY p.id
                 ORDER BY p.name COLLATE NOCASE, p.id"
                    .to_string(),
                None,
            ),
            None => (
                "SELECT p.id, p.name, COUNT(s.source_key), COALESCE(SUM(s.speech_ms), 0),
                        p.cycle_id, NULL, p.sample_clip_file
                 FROM participants p
                 LEFT JOIN voice_samples s ON s.participant_id = p.id
                 GROUP BY p.id
                 ORDER BY p.name COLLATE NOCASE, p.id"
                    .to_string(),
                None,
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ParticipantInfo> {
            let id: String = r.get(0)?;
            let cycle_id: Option<String> = r.get(4)?;
            let row_slug: Option<String> = r.get(5)?;
            let links = projects_map.get(&id);
            let projects: Vec<String> = links
                .map(|v| v.iter().map(|(c, _)| c.clone()).collect())
                .unwrap_or_default();
            let cycle_participant_id = row_slug.or_else(|| {
                let cid = cycle_id.as_deref()?;
                links
                    .and_then(|v| v.iter().find(|(c, _)| c == cid))
                    .and_then(|(_, s)| s.clone())
                    .or_else(|| {
                        id.strip_prefix(cid)
                            .and_then(|rest| rest.strip_prefix(KEY_SEPARATOR))
                            .map(str::to_string)
                    })
            });
            let resolved_cycle_id = cycle_id.or_else(|| {
                if scope.is_none() && projects.len() == 1 {
                    Some(projects[0].clone())
                } else {
                    None
                }
            });
            Ok(ParticipantInfo {
                id,
                name: r.get(1)?,
                sample_count: r.get(2)?,
                speech_ms: r.get::<_, i64>(3)? as u64,
                cycle_id: resolved_cycle_id,
                cycle_participant_id,
                projects,
                sample_clip_file: r.get(6)?,
            })
        };
        let out = if let Some(cid) = cycle_param {
            stmt.query_map(params![cid], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(out)
    }

    /// Link an existing participant to a project/cycle in `project_people`.
    pub fn add_participant_to_project(
        &self,
        participant_id: &str,
        cycle_id: &str,
        cycle_participant_id: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_people (cycle_id, participant_id, cycle_participant_id, created_unix_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(cycle_id, participant_id) DO UPDATE SET
               cycle_participant_id = COALESCE(excluded.cycle_participant_id, project_people.cycle_participant_id)",
            params![cycle_id, participant_id, cycle_participant_id, now_ms],
        )?;
        Ok(())
    }

    /// Unlink a participant from a project/cycle. Keeps the participant row, voice samples,
    /// and retained sample clip intact (falling back to Global when no projects remain).
    pub fn remove_participant_from_project(
        &self,
        participant_id: &str,
        cycle_id: &str,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let removed = tx.execute(
            "DELETE FROM project_people WHERE participant_id = ?1 AND cycle_id = ?2",
            params![participant_id, cycle_id],
        )?;
        if removed > 0 {
            tx.execute(
                "UPDATE participants
                 SET cycle_id = (
                     SELECT cycle_id FROM project_people
                     WHERE participant_id = ?1
                     ORDER BY created_unix_ms, cycle_id
                     LIMIT 1
                 )
                 WHERE id = ?1 AND cycle_id = ?2",
                params![participant_id, cycle_id],
            )?;
        }
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Merge `drop_id` into `keep_id` transactionally:
    /// - Rejects self-merge or merging two participants who share any project in `project_people`.
    /// - Consolidates `voice_samples` keeping up to 20 longest samples (deduped by `source_key`).
    /// - Repoints `meeting_voices` (`matched_participant`, `assigned_participant`).
    /// - Moves `project_people` memberships from `drop_id` to `keep_id`.
    /// - Transfers `sample_clip_file` if `keep_id` has none.
    /// - Deletes `drop_id`.
    pub fn merge_participants(&self, keep_id: &str, drop_id: &str) -> Result<()> {
        if keep_id == drop_id {
            return Err(StoreError::Invalid(
                "cannot merge a participant with itself".into(),
            ));
        }
        const MAX_SAMPLES: usize = 20;
        let tx = self.conn.unchecked_transaction()?;
        let keep_clip: Option<String> = tx
            .query_row(
                "SELECT sample_clip_file FROM participants WHERE id = ?1",
                params![keep_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Invalid(format!("participant {keep_id:?} not found")))?;
        let drop_clip: Option<String> = tx
            .query_row(
                "SELECT sample_clip_file FROM participants WHERE id = ?1",
                params![drop_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Invalid(format!("participant {drop_id:?} not found")))?;

        let collision: Option<String> = tx
            .query_row(
                "SELECT a.cycle_id FROM project_people a
                 JOIN project_people b ON a.cycle_id = b.cycle_id
                 WHERE a.participant_id = ?1 AND b.participant_id = ?2
                 LIMIT 1",
                params![keep_id, drop_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(cycle) = collision {
            return Err(StoreError::Invalid(format!(
                "cannot merge participants that both belong to project {cycle}"
            )));
        }

        // Consolidate voice_samples with MAX_SAMPLES = 20 cap.
        let mut by_key: HashMap<String, (Vec<u8>, i64, i64)> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT source_key, embedding, speech_ms, created_unix_ms
                 FROM voice_samples
                 WHERE participant_id IN (?1, ?2)",
            )?;
            let rows = stmt.query_map(params![keep_id, drop_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (key, emb, speech_ms, created) = row?;
                by_key
                    .entry(key)
                    .and_modify(|cur| {
                        if speech_ms > cur.1 {
                            *cur = (emb.clone(), speech_ms, created);
                        }
                    })
                    .or_insert((emb, speech_ms, created));
            }
        }
        let mut consolidated: Vec<(String, Vec<u8>, i64, i64)> = by_key
            .into_iter()
            .map(|(k, (emb, ms, ts))| (k, emb, ms, ts))
            .collect();
        consolidated.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        consolidated.truncate(MAX_SAMPLES);

        tx.execute(
            "DELETE FROM voice_samples WHERE participant_id IN (?1, ?2)",
            params![keep_id, drop_id],
        )?;
        for (key, emb, speech_ms, created) in consolidated {
            tx.execute(
                "INSERT INTO voice_samples
                   (participant_id, source_key, embedding, speech_ms, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![keep_id, key, emb, speech_ms, created],
            )?;
        }

        for col in ["matched_participant", "assigned_participant"] {
            tx.execute(
                &format!("UPDATE meeting_voices SET {col} = ?2 WHERE {col} = ?1"),
                params![drop_id, keep_id],
            )?;
        }
        tx.execute(
            "UPDATE project_people SET participant_id = ?2 WHERE participant_id = ?1",
            params![drop_id, keep_id],
        )?;
        if keep_clip.is_none() {
            if let Some(clip) = drop_clip {
                tx.execute(
                    "UPDATE participants SET sample_clip_file = ?2 WHERE id = ?1",
                    params![keep_id, clip],
                )?;
            }
        }
        tx.execute("DELETE FROM participants WHERE id = ?1", params![drop_id])?;
        tx.commit()?;
        Ok(())
    }

    // ----- voice profiles -----

    /// Add or replace the sample from one source.
    pub fn put_sample(&self, participant_id: &str, s: &SampleRow, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO voice_samples
               (participant_id, source_key, embedding, speech_ms, created_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(participant_id, source_key) DO UPDATE SET
               embedding = excluded.embedding, speech_ms = excluded.speech_ms,
               created_unix_ms = excluded.created_unix_ms",
            params![
                participant_id,
                s.source_key,
                embedding_to_blob(&s.embedding),
                s.speech_ms as i64,
                now_ms
            ],
        )?;
        Ok(())
    }

    pub fn remove_sample(&self, participant_id: &str, source_key: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM voice_samples WHERE participant_id = ?1 AND source_key = ?2",
            params![participant_id, source_key],
        )? > 0)
    }

    /// Remove the sample a source (meeting and voice) contributed to any participant.
    pub fn remove_samples_of_source(&self, source_key: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM voice_samples WHERE source_key = ?1",
            params![source_key],
        )?)
    }

    pub fn samples_of(&self, participant_id: &str) -> Result<Vec<SampleRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, embedding, speech_ms FROM voice_samples
             WHERE participant_id = ?1 ORDER BY created_unix_ms, source_key",
        )?;
        let rows = stmt.query_map(params![participant_id], |r| {
            Ok(SampleRow {
                source_key: r.get(0)?,
                embedding: blob_to_embedding(&r.get::<_, Vec<u8>>(1)?),
                speech_ms: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every participant with at least one sample, with the samples.
    pub fn all_profiles(&self) -> Result<Vec<(String, Vec<SampleRow>)>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT participant_id FROM voice_samples ORDER BY participant_id",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        ids.into_iter()
            .map(|id| Ok((id.clone(), self.samples_of(&id)?)))
            .collect()
    }

    /// The voice profiles a meeting of this cycle is matched against: those of the cycle's
    /// participants (`Some(cid)` via `project_people`) or unlinked global participants (`None`).
    pub fn profiles_for(&self, cycle_id: Option<&str>) -> Result<Vec<(String, Vec<SampleRow>)>> {
        match cycle_id {
            Some(cid) => {
                let ids: Vec<String> = {
                    let mut stmt = self.conn.prepare(
                        "SELECT DISTINCT s.participant_id FROM voice_samples s
                         JOIN project_people pp ON pp.participant_id = s.participant_id
                         WHERE pp.cycle_id = ?1 ORDER BY s.participant_id",
                    )?;
                    let rows = stmt.query_map(params![cid], |r| r.get(0))?;
                    rows.collect::<std::result::Result<_, _>>()?
                };
                ids.into_iter()
                    .map(|id| Ok((id.clone(), self.samples_of(&id)?)))
                    .collect()
            }
            None => self.profiles_unlinked(),
        }
    }

    /// Voice profiles of global participants that are not linked to any project in `project_people`.
    pub fn profiles_unlinked(&self) -> Result<Vec<(String, Vec<SampleRow>)>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT s.participant_id FROM voice_samples s
                 JOIN participants p ON p.id = s.participant_id
                 WHERE p.cycle_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM project_people pp WHERE pp.participant_id = s.participant_id
                   )
                 ORDER BY s.participant_id",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        ids.into_iter()
            .map(|id| Ok((id.clone(), self.samples_of(&id)?)))
            .collect()
    }

    /// Voice profiles of participants linked to at least one project other than `cycle_id`
    /// and NOT linked to `cycle_id`.
    pub fn profiles_outside(&self, cycle_id: &str) -> Result<Vec<(String, Vec<SampleRow>)>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT s.participant_id FROM voice_samples s
                 WHERE EXISTS (
                     SELECT 1 FROM project_people pp
                     WHERE pp.participant_id = s.participant_id AND pp.cycle_id <> ?1
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM project_people pp
                     WHERE pp.participant_id = s.participant_id AND pp.cycle_id = ?1
                 )
                 ORDER BY s.participant_id",
            )?;
            let rows = stmt.query_map(params![cycle_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        ids.into_iter()
            .map(|id| Ok((id.clone(), self.samples_of(&id)?)))
            .collect()
    }

    /// Forget a participant's voice but keep the participant.
    pub fn reset_profile(&self, participant_id: &str) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let removed = tx.execute(
            "DELETE FROM voice_samples WHERE participant_id = ?1",
            params![participant_id],
        )?;
        tx.execute(
            "UPDATE participants SET sample_clip_file = NULL WHERE id = ?1",
            params![participant_id],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    /// Delete every voice profile, every stored voice embedding of every meeting and every
    /// clip row. Returns the relative paths of the clip files to remove. Participants stay.
    pub fn delete_all_voice_data(&self) -> Result<Vec<String>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut files: Vec<String> = {
            let mut stmt = tx.prepare("SELECT file FROM voice_clips")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        {
            let mut stmt = tx.prepare(
                "SELECT sample_clip_file FROM participants WHERE sample_clip_file IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                files.push(r?);
            }
        }
        tx.execute("UPDATE participants SET sample_clip_file = NULL", [])?;
        tx.execute("DELETE FROM voice_samples", [])?;
        tx.execute("DELETE FROM voice_clips", [])?;
        tx.execute("DELETE FROM meeting_voices", [])?;
        tx.commit()?;
        Ok(files)
    }

    // ----- detected voices of a meeting -----

    /// Store the voices found in a meeting. A voice the user already decided about keeps the
    /// user's choice; everything else is refreshed. Voices that are no longer found are
    /// removed.
    pub fn put_meeting_voices(&self, meeting_id: &str, voices: &[VoiceRow]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for v in voices {
            tx.execute(
                "INSERT INTO meeting_voices
                   (meeting_id, voice_id, track, label, embedding, speech_ms, decision, score,
                    matched_participant, assigned_participant, confirmed, merged_into)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(meeting_id, voice_id) DO UPDATE SET
                   track = excluded.track, label = excluded.label,
                   embedding = excluded.embedding, speech_ms = excluded.speech_ms,
                   decision = excluded.decision, score = excluded.score,
                   matched_participant = excluded.matched_participant,
                   assigned_participant = CASE WHEN meeting_voices.confirmed = 1
                       THEN meeting_voices.assigned_participant
                       ELSE excluded.assigned_participant END",
                params![
                    meeting_id,
                    v.voice_id,
                    v.track,
                    v.label,
                    embedding_to_blob(&v.embedding),
                    v.speech_ms as i64,
                    v.decision.as_str(),
                    v.score,
                    v.matched_participant,
                    v.assigned_participant,
                    v.confirmed,
                    v.merged_into
                ],
            )?;
        }
        let keep: Vec<&str> = voices.iter().map(|v| v.voice_id.as_str()).collect();
        let existing: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT voice_id FROM meeting_voices WHERE meeting_id = ?1")?;
            let rows = stmt.query_map(params![meeting_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in existing.iter().filter(|id| !keep.contains(&id.as_str())) {
            tx.execute(
                "DELETE FROM meeting_voices WHERE meeting_id = ?1 AND voice_id = ?2",
                params![meeting_id, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_meeting_voices(&self, meeting_id: &str) -> Result<Vec<VoiceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT voice_id, track, label, embedding, speech_ms, decision, score,
                    matched_participant, assigned_participant, confirmed, merged_into
             FROM meeting_voices WHERE meeting_id = ?1 ORDER BY track, speech_ms DESC, voice_id",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(VoiceRow {
                voice_id: r.get(0)?,
                track: r.get(1)?,
                label: r.get(2)?,
                embedding: blob_to_embedding(&r.get::<_, Vec<u8>>(3)?),
                speech_ms: r.get::<_, i64>(4)? as u64,
                decision: VoiceDecision::parse(&r.get::<_, String>(5)?),
                score: r.get::<_, Option<f64>>(6)?.map(|s| s as f32),
                matched_participant: r.get(7)?,
                assigned_participant: r.get(8)?,
                confirmed: r.get::<_, i64>(9)? != 0,
                merged_into: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Name a voice (or leave it unnamed with `None`) as the user's decision.
    pub fn set_voice_assignment(
        &self,
        meeting_id: &str,
        voice_id: &str,
        participant_id: Option<&str>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE meeting_voices SET assigned_participant = ?3, confirmed = 1
             WHERE meeting_id = ?1 AND voice_id = ?2",
            params![meeting_id, voice_id, participant_id],
        )? > 0)
    }

    /// Undo a naming decision on a voice: clears `assigned_participant` and `confirmed`
    /// while preserving `decision`, `score`, and `matched_participant` so a suggestion can
    /// be reviewed again.
    pub fn clear_voice_assignment(&self, meeting_id: &str, voice_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE meeting_voices SET assigned_participant = NULL, confirmed = 0
             WHERE meeting_id = ?1 AND voice_id = ?2",
            params![meeting_id, voice_id],
        )? > 0)
    }

    /// Mark a voice as the same person as another voice of the meeting (or undo with `None`).
    pub fn set_voice_merge(
        &self,
        meeting_id: &str,
        voice_id: &str,
        into: Option<&str>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE meeting_voices SET merged_into = ?3
             WHERE meeting_id = ?1 AND voice_id = ?2",
            params![meeting_id, voice_id, into],
        )? > 0)
    }

    // ----- clips -----

    /// Replace the clips of one voice.
    pub fn replace_clips(
        &self,
        meeting_id: &str,
        voice_id: &str,
        clips: &[ClipRow],
        now_ms: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM voice_clips WHERE meeting_id = ?1 AND voice_id = ?2",
            params![meeting_id, voice_id],
        )?;
        for c in clips {
            tx.execute(
                "INSERT INTO voice_clips
                   (meeting_id, voice_id, idx, file, start_ms, end_ms, text, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    meeting_id,
                    voice_id,
                    c.idx,
                    c.file,
                    c.start_ms as i64,
                    c.end_ms as i64,
                    c.text,
                    now_ms
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_clips(&self, meeting_id: &str) -> Result<Vec<ClipRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT voice_id, idx, file, start_ms, end_ms, text FROM voice_clips
             WHERE meeting_id = ?1 ORDER BY voice_id, idx",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(ClipRow {
                voice_id: r.get(0)?,
                idx: r.get(1)?,
                file: r.get(2)?,
                start_ms: r.get::<_, i64>(3)? as u64,
                end_ms: r.get::<_, i64>(4)? as u64,
                text: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Drop all clip rows of a meeting; returns the files to delete.
    pub fn clear_clips(&self, meeting_id: &str) -> Result<Vec<String>> {
        let files: Vec<String> = self
            .list_clips(meeting_id)?
            .into_iter()
            .map(|c| c.file)
            .collect();
        self.conn.execute(
            "DELETE FROM voice_clips WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        Ok(files)
    }

    /// Meetings that still have clips, with the time the oldest was made.
    pub fn meetings_with_clips(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT meeting_id, MIN(created_unix_ms) FROM voice_clips GROUP BY meeting_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, MeetingState};

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m".into(),
            title: "t".into(),
            started_unix_ms: 0,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "m".into(),
        })
        .unwrap();
        s
    }

    fn voice(id: &str) -> VoiceRow {
        VoiceRow {
            voice_id: id.into(),
            track: "t".into(),
            label: "SPEAKER_00".into(),
            embedding: vec![0.5, -0.25, 1.0],
            speech_ms: 9_000,
            decision: VoiceDecision::Unknown,
            score: None,
            matched_participant: None,
            assigned_participant: None,
            confirmed: false,
            merged_into: None,
        }
    }

    #[test]
    fn embeddings_round_trip_through_blobs() {
        let v = vec![0.1, -2.5, 3.0e-3];
        assert_eq!(blob_to_embedding(&embedding_to_blob(&v)), v);
    }

    #[test]
    fn participant_names_can_repeat_across_global_people_and_lookup_is_case_insensitive() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        // Different global people can share a name without auto-merging.
        s.create_participant("b", "ola", 2).unwrap();
        assert_eq!(s.find_participant_by_name("OLA").unwrap().unwrap().id, "a");
    }

    #[test]
    fn samples_replace_by_source_and_deleting_a_participant_removes_the_profile() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        let row = |ms| SampleRow {
            source_key: "m:v".into(),
            embedding: vec![1.0, 0.0],
            speech_ms: ms,
        };
        s.put_sample("a", &row(6_000), 1).unwrap();
        s.put_sample("a", &row(8_000), 2).unwrap();
        let info = s.list_participants().unwrap();
        assert_eq!((info[0].sample_count, info[0].speech_ms), (1, 8_000));
        assert_eq!(s.all_profiles().unwrap().len(), 1);
        s.delete_participant("a").unwrap();
        assert!(s.all_profiles().unwrap().is_empty());
    }

    #[test]
    fn a_user_decision_survives_a_rerun_of_the_matching() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        s.put_meeting_voices("m", &[voice("t:0")]).unwrap();
        s.set_voice_assignment("m", "t:0", Some("a")).unwrap();
        let mut again = voice("t:0");
        again.decision = VoiceDecision::Suggested;
        s.put_meeting_voices("m", &[again]).unwrap();
        let v = &s.list_meeting_voices("m").unwrap()[0];
        assert_eq!(v.assigned_participant.as_deref(), Some("a"));
        assert!(v.confirmed);
        assert_eq!(v.decision, VoiceDecision::Suggested);
    }

    #[test]
    fn deleting_a_participant_unnames_their_voices() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        s.put_meeting_voices("m", &[voice("t:0")]).unwrap();
        s.set_voice_assignment("m", "t:0", Some("a")).unwrap();
        s.delete_participant("a").unwrap();
        assert_eq!(
            s.list_meeting_voices("m").unwrap()[0].assigned_participant,
            None
        );
    }

    #[test]
    fn clips_are_replaced_and_listed_for_cleanup() {
        let s = store();
        let clip = |i| ClipRow {
            voice_id: "t:0".into(),
            idx: i,
            file: format!("m/clips/{i}.wav"),
            start_ms: 0,
            end_ms: 3_000,
            text: "hi".into(),
        };
        s.replace_clips("m", "t:0", &[clip(0), clip(1)], 5).unwrap();
        s.replace_clips("m", "t:0", &[clip(0)], 6).unwrap();
        assert_eq!(s.list_clips("m").unwrap().len(), 1);
        assert_eq!(s.meetings_with_clips().unwrap(), vec![("m".to_string(), 6)]);
        assert_eq!(
            s.clear_clips("m").unwrap(),
            vec!["m/clips/0.wav".to_string()]
        );
        assert!(s.list_clips("m").unwrap().is_empty());
    }

    #[test]
    fn delete_all_voice_data_removes_embeddings_everywhere() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        s.put_sample(
            "a",
            &SampleRow {
                source_key: "k".into(),
                embedding: vec![1.0],
                speech_ms: 9_000,
            },
            1,
        )
        .unwrap();
        s.put_meeting_voices("m", &[voice("t:0")]).unwrap();
        s.replace_clips(
            "m",
            "t:0",
            &[ClipRow {
                voice_id: "t:0".into(),
                idx: 0,
                file: "f.wav".into(),
                start_ms: 0,
                end_ms: 1,
                text: String::new(),
            }],
            1,
        )
        .unwrap();
        assert_eq!(
            s.delete_all_voice_data().unwrap(),
            vec!["f.wav".to_string()]
        );
        assert!(s.all_profiles().unwrap().is_empty());
        assert!(s.list_meeting_voices("m").unwrap().is_empty());
        assert_eq!(s.list_participants().unwrap().len(), 1);
    }

    #[test]
    fn sample_clip_file_is_stored_and_listed_across_all_participants() {
        let s = store();
        s.create_participant("global-ola", "Ola", 1).unwrap();
        s.create_participant_in(Some("NOMAD"), "NOMAD:jan", "Jan", 2)
            .unwrap();
        assert!(s
            .set_participant_sample_clip("global-ola", Some(".voice-samples/global-ola.wav"))
            .unwrap());
        let ola = s.get_participant("global-ola").unwrap().unwrap();
        assert_eq!(
            ola.sample_clip_file.as_deref(),
            Some(".voice-samples/global-ola.wav")
        );
        let all = s.list_all_participants_with_profiles().unwrap();
        assert_eq!(all.len(), 2);
        let ola_info = all.iter().find(|p| p.id == "global-ola").unwrap();
        assert_eq!(
            ola_info.sample_clip_file.as_deref(),
            Some(".voice-samples/global-ola.wav")
        );
        assert_eq!(ola_info.cycle_id, None);
        let jan_info = all.iter().find(|p| p.id == "NOMAD:jan").unwrap();
        assert_eq!(jan_info.cycle_id.as_deref(), Some("NOMAD"));
        assert_eq!(jan_info.sample_clip_file, None);

        s.reset_profile("global-ola").unwrap();
        assert_eq!(
            s.get_participant("global-ola")
                .unwrap()
                .unwrap()
                .sample_clip_file,
            None
        );
    }

    #[test]
    fn add_remove_project_and_merge_participants_with_collision_guard_and_cap() {
        let s = store();
        s.create_participant_in(Some("NOMAD"), "NOMAD:ola", "Ola", 1)
            .unwrap();
        s.create_participant_in(Some("ATLAS"), "ATLAS:ola", "Ola", 2)
            .unwrap();
        s.set_participant_sample_clip("ATLAS:ola", Some(".voice-samples/atlas-ola.wav"))
            .unwrap();

        for i in 0..12 {
            s.put_sample(
                "NOMAD:ola",
                &SampleRow {
                    source_key: format!("n:{i}"),
                    embedding: vec![1.0, 0.0],
                    speech_ms: 5_000 + i * 100,
                },
                i as i64,
            )
            .unwrap();
            s.put_sample(
                "ATLAS:ola",
                &SampleRow {
                    source_key: format!("a:{i}"),
                    embedding: vec![1.0, 0.0],
                    speech_ms: 6_000 + i * 100,
                },
                i as i64,
            )
            .unwrap();
        }
        s.put_meeting_voices("m", &[voice("t:0")]).unwrap();
        s.set_voice_assignment("m", "t:0", Some("ATLAS:ola"))
            .unwrap();

        // Merging ATLAS:ola into NOMAD:ola consolidates samples (cap 20), moves project_people,
        // repoints meeting_voices, transfers sample_clip_file, and deletes drop_id.
        s.merge_participants("NOMAD:ola", "ATLAS:ola").unwrap();
        assert!(s.get_participant("ATLAS:ola").unwrap().is_none());
        assert_eq!(s.samples_of("NOMAD:ola").unwrap().len(), 20);
        assert_eq!(
            s.list_meeting_voices("m").unwrap()[0]
                .assigned_participant
                .as_deref(),
            Some("NOMAD:ola")
        );
        let merged = s
            .list_all_participants_with_profiles()
            .unwrap()
            .into_iter()
            .find(|p| p.id == "NOMAD:ola")
            .unwrap();
        assert_eq!(merged.projects, vec!["ATLAS", "NOMAD"]);
        assert_eq!(
            merged.sample_clip_file.as_deref(),
            Some(".voice-samples/atlas-ola.wav")
        );

        // Same-cycle collision guard: cannot merge two people in NOMAD.
        s.create_participant_in(Some("NOMAD"), "NOMAD:other", "Other", 3)
            .unwrap();
        assert!(s.merge_participants("NOMAD:ola", "NOMAD:other").is_err());

        // Removing from NOMAD keeps global profile and samples intact.
        assert!(s
            .remove_participant_from_project("NOMAD:ola", "NOMAD")
            .unwrap());
        assert_eq!(s.samples_of("NOMAD:ola").unwrap().len(), 20);
        assert!(s
            .remove_participant_from_project("NOMAD:ola", "ATLAS")
            .unwrap());
        let global = s
            .list_all_participants_with_profiles()
            .unwrap()
            .into_iter()
            .find(|p| p.id == "NOMAD:ola")
            .unwrap();
        assert!(global.projects.is_empty());
        assert_eq!(global.sample_count, 20);
    }
}
