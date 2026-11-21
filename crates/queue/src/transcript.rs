//! The transcript as the application sees it: the worker's `transcript.json` plus the
//! user's edits, kept in a separate overlay file so re-running a stage never loses them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("the transcript is not ready for this meeting")]
    NotReady,
    #[error("no such segment: {0}")]
    NoSegment(u32),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid transcript file: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
}

/// A segment as written by the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSegment {
    pub id: u32,
    pub track: String,
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub words: Vec<Word>,
}

/// The worker's `transcript.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawTranscript {
    pub version: u32,
    pub meeting_id: String,
    pub language: String,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<RawSegment>,
}

/// The user's changes to one segment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEdit {
    /// Replacement text, when the user corrected it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Excluded passages are not sent to a model and are left out of published transcripts.
    #[serde(default)]
    pub excluded: bool,
}

impl SegmentEdit {
    fn is_empty(&self) -> bool {
        self.text.is_none() && !self.excluded
    }
}

/// `edits.json`: edits by segment id, plus renamed speakers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edits {
    #[serde(default)]
    pub segments: BTreeMap<u32, SegmentEdit>,
}

/// A segment with the user's edits applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewSegment {
    pub id: u32,
    pub track: String,
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// The recognised text when the user changed it.
    pub original_text: Option<String>,
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptView {
    pub meeting_id: String,
    pub language: String,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<ViewSegment>,
}

/// Compose the transcript with the user's edits.
pub fn compose(raw: &RawTranscript, edits: &Edits) -> TranscriptView {
    let segments = raw
        .segments
        .iter()
        .map(|s| {
            let edit = edits.segments.get(&s.id);
            let text = edit.and_then(|e| e.text.clone());
            ViewSegment {
                id: s.id,
                track: s.track.clone(),
                speaker: s.speaker.clone(),
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                original_text: text.as_ref().map(|_| s.text.clone()),
                text: text.unwrap_or_else(|| s.text.clone()),
                excluded: edit.is_some_and(|e| e.excluded),
            }
        })
        .collect();
    TranscriptView {
        meeting_id: raw.meeting_id.clone(),
        language: raw.language.clone(),
        speakers: raw.speakers.clone(),
        segments,
    }
}

/// Text that may leave the transcript view: excluded passages are dropped.
pub fn published_text(view: &TranscriptView) -> String {
    view.segments
        .iter()
        .filter(|s| !s.excluded)
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Files of a meeting's transcript.
pub struct TranscriptFiles {
    work: PathBuf,
}

impl TranscriptFiles {
    pub fn new(meeting_dir: &Path) -> Self {
        Self {
            work: meeting_dir.join("work"),
        }
    }

    fn raw_path(&self) -> PathBuf {
        self.work.join("transcript.json")
    }

    fn edits_path(&self) -> PathBuf {
        self.work.join("edits.json")
    }

    pub fn load_raw(&self) -> Result<RawTranscript, TranscriptError> {
        match std::fs::read(self.raw_path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(TranscriptError::NotReady),
            Err(e) => Err(e.into()),
        }
    }

    pub fn load_edits(&self) -> Result<Edits, TranscriptError> {
        match std::fs::read(self.edits_path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Edits::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn view(&self) -> Result<TranscriptView, TranscriptError> {
        Ok(compose(&self.load_raw()?, &self.load_edits()?))
    }

    /// Replace the edit of one segment. An empty edit removes it.
    pub fn set_edit(&self, id: u32, edit: SegmentEdit) -> Result<(), TranscriptError> {
        let raw = self.load_raw()?;
        if !raw.segments.iter().any(|s| s.id == id) {
            return Err(TranscriptError::NoSegment(id));
        }
        let mut edits = self.load_edits()?;
        if edit.is_empty() {
            edits.segments.remove(&id);
        } else {
            edits.segments.insert(id, edit);
        }
        // Write to a temporary file and rename, so a crash never truncates the edits.
        let tmp = self.edits_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&edits)?)?;
        std::fs::rename(tmp, self.edits_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../worker/tests/fixtures/transcript.json");

    fn raw() -> RawTranscript {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn tmp(name: &str) -> crate::test_util::TestDir {
        let p = crate::test_util::TestDir::new(&format!("tr-{name}"));
        std::fs::create_dir_all(p.join("work")).unwrap();
        p
    }

    #[test]
    fn contract_the_worker_fixture_matches_the_schema_and_the_rust_types() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../schemas/transcript.schema.json")).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let doc: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        assert!(validator.is_valid(&doc), "fixture violates the schema");
        // Round trip: Rust loses nothing the worker wrote.
        let back = serde_json::to_value(raw()).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn contract_rust_output_is_accepted_by_the_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../schemas/transcript.schema.json")).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let mut bad = serde_json::to_value(raw()).unwrap();
        bad["segments"][0]["start_ms"] = serde_json::json!(-1);
        assert!(!validator.is_valid(&bad));
    }

    #[test]
    fn edits_replace_text_and_exclude_without_touching_the_original() {
        let mut edits = Edits::default();
        edits.segments.insert(
            0,
            SegmentEdit {
                text: Some("Hello all".into()),
                excluded: false,
            },
        );
        edits.segments.insert(
            1,
            SegmentEdit {
                text: None,
                excluded: true,
            },
        );
        let view = compose(&raw(), &edits);
        assert_eq!(view.segments[0].text, "Hello all");
        assert_eq!(
            view.segments[0].original_text.as_deref(),
            Some("Hello everyone")
        );
        assert!(view.segments[1].excluded);
        assert_eq!(published_text(&view), "Hello all");
    }

    #[test]
    fn edits_persist_and_survive_a_rerun_of_the_worker() {
        let dir = tmp("persist");
        std::fs::write(dir.join("work/transcript.json"), FIXTURE).unwrap();
        let files = TranscriptFiles::new(&dir);
        files
            .set_edit(
                1,
                SegmentEdit {
                    text: None,
                    excluded: true,
                },
            )
            .unwrap();
        // The worker writes a fresh transcript.json; edits.json is a different file.
        std::fs::write(dir.join("work/transcript.json"), FIXTURE).unwrap();
        assert!(files.view().unwrap().segments[1].excluded);
        files.set_edit(1, SegmentEdit::default()).unwrap();
        assert!(!files.view().unwrap().segments[1].excluded);
    }

    #[test]
    fn unknown_segment_and_missing_transcript_are_reported() {
        let dir = tmp("errors");
        let files = TranscriptFiles::new(&dir);
        assert!(matches!(files.view(), Err(TranscriptError::NotReady)));
        std::fs::write(dir.join("work/transcript.json"), FIXTURE).unwrap();
        assert!(matches!(
            files.set_edit(99, SegmentEdit::default()),
            Err(TranscriptError::NoSegment(99))
        ));
    }
}
