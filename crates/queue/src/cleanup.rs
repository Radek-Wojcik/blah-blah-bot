//! Audio retention: the full recording is deleted only after every stage that reads audio
//! has succeeded.

use std::path::Path;

use serde::{Deserialize, Serialize};
use store::{JobRow, Store};

use crate::stage::audio_is_expendable;

/// What happens to the full audio once it is no longer needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AudioRetention {
    /// Keep the audio until the user deletes the meeting.
    Keep,
    /// Delete it once processing no longer needs it.
    Delete,
    /// Keep it for this many days after processing, then delete it.
    KeepDays { days: u32 },
}

/// Deleting once processing succeeded is the default (the user decided so after Stage 2).
impl Default for AudioRetention {
    fn default() -> Self {
        Self::Delete
    }
}

/// Result of a cleanup attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// The files were deleted; the number of files removed.
    Deleted { files: usize },
    /// The policy keeps the audio for now.
    Kept,
    /// An audio-dependent stage has not succeeded, so nothing may be deleted.
    NotExpendable,
}

/// Apply the retention policy to one meeting right after its audio-dependent stages.
///
/// `now_ms` and `finished_ms` (when the last audio-dependent stage finished) matter only
/// for `KeepDays`.
pub fn apply(
    store: &Store,
    job: &JobRow,
    meeting_root: &Path,
    retention: AudioRetention,
    now_ms: i64,
) -> Result<CleanupOutcome, store::StoreError> {
    if !audio_is_expendable(job) {
        return Ok(CleanupOutcome::NotExpendable);
    }
    match retention {
        AudioRetention::Keep => Ok(CleanupOutcome::Kept),
        AudioRetention::Delete => delete_audio(store, &job.meeting_id, meeting_root),
        AudioRetention::KeepDays { days } => {
            let finished = audio_finished_ms(job).unwrap_or(now_ms);
            if now_ms - finished >= i64::from(days) * 86_400_000 {
                delete_audio(store, &job.meeting_id, meeting_root)
            } else {
                Ok(CleanupOutcome::Kept)
            }
        }
    }
}

/// When the last audio-dependent stage finished.
fn audio_finished_ms(job: &JobRow) -> Option<i64> {
    crate::stage::AUDIO_DEPENDENT
        .iter()
        .filter_map(|st| {
            job.stages
                .iter()
                .find(|s| s.stage == st.as_str())
                .and_then(|s| s.finished_unix_ms)
        })
        .max()
}

/// Delete every segment file of the meeting and remove emptied track folders. Missing files
/// are fine (an interrupted cleanup can be repeated). Screenshots, the transcript and the
/// journal stay.
pub fn delete_audio(
    store: &Store,
    meeting_id: &str,
    meeting_root: &Path,
) -> Result<CleanupOutcome, store::StoreError> {
    let Some(details) = store.get_meeting(meeting_id)? else {
        return Ok(CleanupOutcome::Deleted { files: 0 });
    };
    let dir = meeting_root.join(&details.meeting.dir);
    let mut files = 0;
    let mut folders = std::collections::BTreeSet::new();
    for seg in &details.segments {
        let path = dir.join(&seg.file);
        if std::fs::remove_file(&path).is_ok() {
            files += 1;
        }
        if let Some(parent) = path.parent() {
            folders.insert(parent.to_path_buf());
        }
    }
    for folder in folders {
        // Fails when something else is still inside, which is the right outcome.
        let _ = std::fs::remove_dir(folder);
    }
    Ok(CleanupOutcome::Deleted { files })
}

/// Delete the audio of every finished job whose `KeepDays` period has passed. Run at start.
pub fn sweep_expired(
    store: &Store,
    meeting_root: &Path,
    retention: AudioRetention,
    now_ms: i64,
) -> Result<usize, store::StoreError> {
    let AudioRetention::KeepDays { .. } = retention else {
        return Ok(0);
    };
    let mut swept = 0;
    for id in store.list_job_ids()? {
        if let Some(job) = store.get_job(&id)? {
            if let CleanupOutcome::Deleted { files } =
                apply(store, &job, meeting_root, retention, now_ms)?
            {
                swept += files;
            }
        }
    }
    Ok(swept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, Segment, StageStatus, Track};

    fn setup(root: &Path) -> Store {
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
        s.upsert_track(
            "m",
            &Track {
                name: "mic".into(),
                source: "microphone".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            },
        )
        .unwrap();
        for i in 0..2 {
            let file = format!("mic/{i:06}.wav");
            s.add_segment(
                "m",
                &Segment {
                    track: "mic".into(),
                    segment: i,
                    file: file.clone(),
                    offset_ms: 0.0,
                    samples: 1,
                },
            )
            .unwrap();
            let p = root.join("m").join(&file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("m/screenshots")).unwrap();
        std::fs::write(root.join("m/screenshots/1.png"), b"x").unwrap();
        let names: Vec<&str> = crate::stage::STAGES.iter().map(|s| s.as_str()).collect();
        s.create_job("m", &names, "{}", 0).unwrap();
        s
    }

    fn finish(s: &Store, stages: &[&str], at: i64) {
        for st in stages {
            s.set_stage_status("m", st, StageStatus::Done, None, at)
                .unwrap();
        }
    }

    fn tmp(name: &str) -> crate::test_util::TestDir {
        crate::test_util::TestDir::new(&format!("cleanup-{name}"))
    }

    #[test]
    fn nothing_is_deleted_before_every_audio_stage_succeeded() {
        let root = tmp("early");
        let s = setup(&root);
        finish(&s, &["asr"], 10);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete, 100).unwrap(),
            CleanupOutcome::NotExpendable
        );
        assert!(root.join("m/mic/000000.wav").exists());
    }

    #[test]
    fn a_failed_audio_stage_blocks_deletion() {
        let root = tmp("failed");
        let s = setup(&root);
        finish(&s, &["asr"], 10);
        s.set_stage_status("m", "diarize", StageStatus::Failed, Some("x"), 11)
            .unwrap();
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete, 100).unwrap(),
            CleanupOutcome::NotExpendable
        );
    }

    #[test]
    fn keep_leaves_the_files_and_delete_is_the_default() {
        let root = tmp("keep");
        let s = setup(&root);
        finish(&s, &["asr", "diarize", "clips"], 10);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(AudioRetention::default(), AudioRetention::Delete);
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Keep, 100).unwrap(),
            CleanupOutcome::Kept
        );
        assert!(root.join("m/mic/000000.wav").exists());
    }

    #[test]
    fn delete_removes_audio_but_keeps_screenshots() {
        let root = tmp("delete");
        let s = setup(&root);
        finish(&s, &["asr", "diarize", "clips"], 10);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete, 100).unwrap(),
            CleanupOutcome::Deleted { files: 2 }
        );
        assert!(!root.join("m/mic").exists());
        assert!(root.join("m/screenshots/1.png").exists());
        // Repeating is harmless.
        assert_eq!(
            delete_audio(&s, "m", &root).unwrap(),
            CleanupOutcome::Deleted { files: 0 }
        );
    }

    #[test]
    fn keep_days_waits_then_sweeps() {
        let root = tmp("days");
        let s = setup(&root);
        finish(&s, &["asr", "diarize", "clips"], 1_000);
        let policy = AudioRetention::KeepDays { days: 2 };
        let day = 86_400_000;
        assert_eq!(sweep_expired(&s, &root, policy, 1_000 + day).unwrap(), 0);
        assert!(root.join("m/mic/000000.wav").exists());
        assert_eq!(
            sweep_expired(&s, &root, policy, 1_000 + 3 * day).unwrap(),
            2
        );
        assert!(!root.join("m/mic").exists());
    }
}
