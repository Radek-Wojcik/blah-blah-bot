//! Participants, voice profiles, detected voices and their sample clips.
//!
//! Embeddings are biometric data. They live only in this local database; nothing here is
//! ever sent anywhere.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

/// A person who can be named as a speaker. A minimal local list until cycles exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub created_unix_ms: i64,
}

/// A participant with the size of the voice profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: String,
    pub name: String,
    pub sample_count: u32,
    pub speech_ms: u64,
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

impl Store {
    // ----- participants -----

    pub fn create_participant(&self, id: &str, name: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO participants (id, name, created_unix_ms) VALUES (?1, ?2, ?3)",
            params![id, name, now_ms],
        )?;
        Ok(())
    }

    pub fn find_participant_by_name(&self, name: &str) -> Result<Option<Participant>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, created_unix_ms FROM participants WHERE name = ?1",
                params![name],
                |r| {
                    Ok(Participant {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        created_unix_ms: r.get(2)?,
                    })
                },
            )
            .optional()?)
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

    pub fn list_participants(&self) -> Result<Vec<ParticipantInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, COUNT(s.source_key), COALESCE(SUM(s.speech_ms), 0)
             FROM participants p LEFT JOIN voice_samples s ON s.participant_id = p.id
             GROUP BY p.id ORDER BY p.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ParticipantInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                sample_count: r.get(2)?,
                speech_ms: r.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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

    /// Forget a participant's voice but keep the participant.
    pub fn reset_profile(&self, participant_id: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM voice_samples WHERE participant_id = ?1",
            params![participant_id],
        )?)
    }

    /// Delete every voice profile, every stored voice embedding of every meeting and every
    /// clip row. Returns the relative paths of the clip files to remove. Participants stay.
    pub fn delete_all_voice_data(&self) -> Result<Vec<String>> {
        let tx = self.conn.unchecked_transaction()?;
        let files: Vec<String> = {
            let mut stmt = tx.prepare("SELECT file FROM voice_clips")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
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
    fn participant_names_are_unique_ignoring_case() {
        let s = store();
        s.create_participant("a", "Ola", 1).unwrap();
        assert!(s.create_participant("b", "ola", 2).is_err());
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
}
