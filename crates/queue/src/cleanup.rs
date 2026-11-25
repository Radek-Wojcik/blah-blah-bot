//! Audio retention: the source audio of a meeting is deleted only after the whole pipeline
//! succeeded, unless the policy keeps it.
//!
//! The policy is decided per meeting: the project's own choice wins over the global setting
//! (see [`AudioRetention::resolve`]). What is kept: the transcript, the draft, the screenshots
//! and the voice sample clips. What is deleted: the recorded or imported audio.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use store::{JobRow, Store};

use crate::stage::audio_is_expendable;

/// What happens to the source audio once processing succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRetention {
    /// Keep the audio until the user deletes the meeting.
    Keep,
    /// Delete it once the whole pipeline succeeded.
    Delete,
}

/// Deleting once processing succeeded is the default (the owner decided so).
impl Default for AudioRetention {
    fn default() -> Self {
        Self::Delete
    }
}

impl AudioRetention {
    /// The value stored in a project's preferences.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Delete => "delete",
        }
    }

    /// Parses a stored project override. Anything that is not exactly `keep` or `delete`
    /// reads as no override, so a damaged value falls back to the global setting.
    pub fn parse_override(value: Option<&str>) -> Option<Self> {
        match value?.trim() {
            "keep" => Some(Self::Keep),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }

    /// The effective policy: the project's choice when it has one, the global one otherwise.
    pub fn resolve(global: Self, project: Option<Self>) -> Self {
        project.unwrap_or(global)
    }
}

/// The effective policy of a meeting: the override of its project (cycle) beats `global`.
/// A meeting without a cycle follows the global setting.
pub fn effective_retention(
    store: &Store,
    meeting_id: &str,
    global: AudioRetention,
) -> Result<AudioRetention, store::StoreError> {
    let cycle_id = store
        .get_report_settings(meeting_id)?
        .and_then(|s| s.cycle_id);
    let project = match cycle_id {
        Some(id) => {
            AudioRetention::parse_override(store.get_project_prefs(&id)?.audio_retention.as_deref())
        }
        None => None,
    };
    Ok(AudioRetention::resolve(global, project))
}

/// Result of a cleanup attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// The audio was deleted; the number of files removed (0 when it was gone already).
    Deleted { files: usize },
    /// The policy keeps the audio.
    Kept,
    /// A stage has not succeeded yet, so nothing may be deleted.
    NotExpendable,
}

/// Apply the retention policy to one meeting once its job has run every other stage.
pub fn apply(
    store: &Store,
    job: &JobRow,
    meeting_root: &Path,
    retention: AudioRetention,
) -> Result<CleanupOutcome, store::StoreError> {
    if !audio_is_expendable(job) {
        return Ok(CleanupOutcome::NotExpendable);
    }
    match retention {
        AudioRetention::Keep => {
            tracing::info!("audio kept by the retention policy");
            Ok(CleanupOutcome::Kept)
        }
        AudioRetention::Delete => delete_audio(store, &job.meeting_id, meeting_root),
    }
}

/// File extensions of media that an import may have left in the meeting folder itself.
const SOURCE_MEDIA_EXTENSIONS: [&str; 12] = [
    "wav", "mp3", "m4a", "ogg", "flac", "aac", "mp4", "mkv", "mov", "webm", "avi", "wma",
];

/// A path that stays inside the folder it is joined to: only plain components, no `..`, no
/// root or drive prefix.
fn is_plain_relative(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Delete the source audio of the meeting: every segment file of its tracks, emptied track
/// folders and any media file lying directly in the meeting folder (an imported source). The
/// transcript, the journal, the draft, the screenshots (`img/`, `screenshots/`) and the
/// sample clips (`clips/`) stay.
///
/// Nothing outside the meeting folder is ever touched: a stored path with `..`, a drive or a
/// root is skipped, and so is a file that resolves (through a link) outside the folder.
/// Missing files are fine, so an interrupted cleanup can be repeated.
pub fn delete_audio(
    store: &Store,
    meeting_id: &str,
    meeting_root: &Path,
) -> Result<CleanupOutcome, store::StoreError> {
    let Some(details) = store.get_meeting(meeting_id)? else {
        return Ok(CleanupOutcome::Deleted { files: 0 });
    };
    let Some(dir) = meeting_dir_inside(meeting_root, &details.meeting.dir) else {
        tracing::warn!(
            "the meeting folder is missing or outside the recordings folder; audio left alone"
        );
        return Ok(CleanupOutcome::Deleted { files: 0 });
    };

    let mut removed: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    let mut folders = BTreeSet::new();
    for seg in &details.segments {
        if !is_plain_relative(&seg.file) {
            skipped += 1;
            continue;
        }
        let path = dir.join(&seg.file);
        match remove_file_inside(&dir, &path) {
            Removal::Removed => removed.push(seg.file.clone()),
            Removal::Missing => {}
            Removal::Refused => skipped += 1,
        }
        if let Some(parent) = path.parent() {
            folders.insert(parent.to_path_buf());
        }
    }
    // An imported source file lying in the meeting folder itself.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_media = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                SOURCE_MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
            });
            if !is_media || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            if let Removal::Removed = remove_file_inside(&dir, &path) {
                removed.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    for folder in folders {
        // Only a folder below the meeting folder, and only when nothing else is in it.
        if folder != dir && folder.starts_with(&dir) {
            let _ = std::fs::remove_dir(folder);
        }
    }

    if !removed.is_empty() || skipped > 0 {
        // File names only: no transcript text and no paths outside the meeting folder.
        tracing::info!(
            files = removed.len(),
            skipped,
            names = ?removed,
            "deleted the source audio of a processed meeting"
        );
    }
    Ok(CleanupOutcome::Deleted {
        files: removed.len(),
    })
}

/// The canonical meeting folder, only when `relative` is a plain relative path and the folder
/// exists below the canonical recordings folder.
fn meeting_dir_inside(meeting_root: &Path, relative: &str) -> Option<PathBuf> {
    if !is_plain_relative(relative) {
        return None;
    }
    let root = std::fs::canonicalize(meeting_root).ok()?;
    let dir = std::fs::canonicalize(root.join(relative)).ok()?;
    (dir != root && dir.starts_with(&root)).then_some(dir)
}

enum Removal {
    Removed,
    Missing,
    /// Not a regular file below the folder, or the removal failed.
    Refused,
}

/// Remove one file after checking that it resolves to a regular file below `dir` (canonical).
fn remove_file_inside(dir: &Path, path: &Path) -> Removal {
    let Ok(resolved) = std::fs::canonicalize(path) else {
        return Removal::Missing;
    };
    if !resolved.starts_with(dir) || !resolved.is_file() {
        return Removal::Refused;
    }
    match std::fs::remove_file(path) {
        Ok(()) => Removal::Removed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Removal::Missing,
        Err(_) => Removal::Refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, ReportSettings, Segment, StageStatus, Track};

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
        for kept in [
            "m/screenshots/1.png",
            "m/img/slide.png",
            "m/clips/a.wav",
            "m/work/transcript.json",
            "m/work/draft.json",
        ] {
            let p = root.join(kept);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let names: Vec<&str> = crate::stage::STAGES.iter().map(|s| s.as_str()).collect();
        s.create_job("m", &names, "{}", 0).unwrap();
        s
    }

    /// Marks every stage except the cleanup as done.
    fn finish_pipeline(s: &Store) {
        let all: Vec<&str> = crate::stage::STAGES
            .iter()
            .filter(|st| **st != crate::stage::Stage::AudioCleanup)
            .map(|st| st.as_str())
            .collect();
        finish(s, &all);
    }

    fn finish(s: &Store, stages: &[&str]) {
        for st in stages {
            s.set_stage_status("m", st, StageStatus::Done, None, 10)
                .unwrap();
        }
    }

    fn tmp(name: &str) -> crate::test_util::TestDir {
        crate::test_util::TestDir::new(&format!("cleanup-{name}"))
    }

    #[test]
    fn a_finished_job_deletes_the_audio_by_default() {
        let root = tmp("default");
        let s = setup(&root);
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(AudioRetention::default(), AudioRetention::Delete);
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::default()).unwrap(),
            CleanupOutcome::Deleted { files: 2 }
        );
        assert!(!root.join("m/mic").exists());
    }

    #[test]
    fn nothing_is_deleted_until_the_draft_exists() {
        let root = tmp("early");
        let s = setup(&root);
        // Everything up to the language-model stages is done, the draft is not.
        finish(
            &s,
            &[
                "asr",
                "diarize",
                "merge",
                "speaker_id",
                "clips",
                "vision",
                "extract",
            ],
        );
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::NotExpendable
        );
        assert!(root.join("m/mic/000000.wav").exists());
    }

    #[test]
    fn a_failed_job_keeps_its_audio() {
        let root = tmp("failed");
        let s = setup(&root);
        finish_pipeline(&s);
        s.set_stage_status("m", "reduce", StageStatus::Failed, Some("x"), 11)
            .unwrap();
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::NotExpendable
        );
        assert!(root.join("m/mic/000000.wav").exists());
        assert!(root.join("m/mic/000001.wav").exists());
    }

    #[test]
    fn keep_leaves_the_files() {
        let root = tmp("keep");
        let s = setup(&root);
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Keep).unwrap(),
            CleanupOutcome::Kept
        );
        assert!(root.join("m/mic/000000.wav").exists());
    }

    #[test]
    fn delete_removes_audio_but_keeps_clips_screenshots_transcript_and_draft() {
        let root = tmp("delete");
        let s = setup(&root);
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::Deleted { files: 2 }
        );
        assert!(!root.join("m/mic").exists());
        for kept in [
            "m/screenshots/1.png",
            "m/img/slide.png",
            "m/clips/a.wav",
            "m/work/transcript.json",
            "m/work/draft.json",
        ] {
            assert!(root.join(kept).exists(), "{kept} must stay");
        }
    }

    #[test]
    fn deleting_twice_is_harmless() {
        let root = tmp("twice");
        let s = setup(&root);
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::Deleted { files: 2 }
        );
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::Deleted { files: 0 }
        );
        assert_eq!(
            delete_audio(&s, "m", &root).unwrap(),
            CleanupOutcome::Deleted { files: 0 }
        );
        assert!(root.join("m/clips/a.wav").exists());
    }

    #[test]
    fn an_imported_source_file_in_the_meeting_folder_goes_too() {
        let root = tmp("import");
        let s = setup(&root);
        std::fs::write(root.join("m/source.MP4"), b"video").unwrap();
        std::fs::write(root.join("m/notes.txt"), b"keep me").unwrap();
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::Deleted { files: 3 }
        );
        assert!(!root.join("m/source.MP4").exists());
        assert!(root.join("m/notes.txt").exists());
        assert!(root.join("m/img/slide.png").exists());
    }

    #[test]
    fn a_path_outside_the_meeting_folder_is_never_removed() {
        let root = tmp("outside");
        let s = setup(&root);
        let outside = root.join("outside.wav");
        std::fs::write(&outside, b"precious").unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("note.wav"), b"precious").unwrap();
        for (i, file) in [
            "../outside.wav",
            "mic/../../outside.wav",
            "../other/note.wav",
        ]
        .into_iter()
        .enumerate()
        {
            s.add_segment(
                "m",
                &Segment {
                    track: "mic".into(),
                    segment: 10 + i as u32,
                    file: file.into(),
                    offset_ms: 0.0,
                    samples: 1,
                },
            )
            .unwrap();
        }
        // An absolute path is refused as well.
        s.add_segment(
            "m",
            &Segment {
                track: "mic".into(),
                segment: 20,
                file: outside.to_string_lossy().into_owned(),
                offset_ms: 0.0,
                samples: 1,
            },
        )
        .unwrap();
        finish_pipeline(&s);
        let job = s.get_job("m").unwrap().unwrap();
        assert_eq!(
            apply(&s, &job, &root, AudioRetention::Delete).unwrap(),
            CleanupOutcome::Deleted { files: 2 }
        );
        assert!(outside.exists());
        assert!(other.join("note.wav").exists());
        assert!(!root.join("m/mic").exists());
    }

    #[test]
    fn a_meeting_folder_that_escapes_the_recordings_folder_is_left_alone() {
        let root = tmp("escape");
        let recordings = root.join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        std::fs::create_dir_all(root.join("elsewhere/mic")).unwrap();
        std::fs::write(root.join("elsewhere/mic/000000.wav"), b"precious").unwrap();
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m".into(),
            title: "t".into(),
            started_unix_ms: 0,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "../elsewhere".into(),
        })
        .unwrap();
        s.add_segment(
            "m",
            &Segment {
                track: "mic".into(),
                segment: 0,
                file: "mic/000000.wav".into(),
                offset_ms: 0.0,
                samples: 1,
            },
        )
        .unwrap();
        assert_eq!(
            delete_audio(&s, "m", &recordings).unwrap(),
            CleanupOutcome::Deleted { files: 0 }
        );
        assert!(root.join("elsewhere/mic/000000.wav").exists());
    }

    #[test]
    fn the_project_choice_beats_the_global_one() {
        use AudioRetention::{Delete, Keep};
        assert_eq!(AudioRetention::resolve(Delete, None), Delete);
        assert_eq!(AudioRetention::resolve(Keep, None), Keep);
        assert_eq!(AudioRetention::resolve(Delete, Some(Keep)), Keep);
        assert_eq!(AudioRetention::resolve(Keep, Some(Delete)), Delete);
        assert_eq!(AudioRetention::parse_override(None), None);
        assert_eq!(AudioRetention::parse_override(Some("")), None);
        assert_eq!(AudioRetention::parse_override(Some("keep")), Some(Keep));
        assert_eq!(AudioRetention::parse_override(Some("delete")), Some(Delete));
        assert_eq!(AudioRetention::parse_override(Some("later")), None);
        assert_eq!(Keep.as_str(), "keep");
        assert_eq!(Delete.as_str(), "delete");
    }

    #[test]
    fn the_effective_policy_is_read_from_the_project_of_the_meeting() {
        let root = tmp("effective");
        let s = setup(&root);
        // No cycle: the global setting decides.
        assert_eq!(
            effective_retention(&s, "m", AudioRetention::Delete).unwrap(),
            AudioRetention::Delete
        );
        s.set_report_settings(
            "m",
            &ReportSettings {
                cycle_id: Some("ALPHA".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // A project without an override follows the global setting.
        assert_eq!(
            effective_retention(&s, "m", AudioRetention::Keep).unwrap(),
            AudioRetention::Keep
        );
        let mut prefs = s.get_project_prefs("ALPHA").unwrap();
        prefs.audio_retention = Some("keep".into());
        s.set_project_prefs(&prefs).unwrap();
        assert_eq!(
            effective_retention(&s, "m", AudioRetention::Delete).unwrap(),
            AudioRetention::Keep
        );
        prefs.audio_retention = Some("delete".into());
        s.set_project_prefs(&prefs).unwrap();
        assert_eq!(
            effective_retention(&s, "m", AudioRetention::Keep).unwrap(),
            AudioRetention::Delete
        );
        // The override of another project does not matter.
        let mut other = s.get_project_prefs("BETA").unwrap();
        other.audio_retention = Some("keep".into());
        s.set_project_prefs(&other).unwrap();
        assert_eq!(
            effective_retention(&s, "m", AudioRetention::Delete).unwrap(),
            AudioRetention::Delete
        );
    }
}
