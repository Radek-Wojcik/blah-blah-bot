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

/// `hh:mm:ss` of a time in milliseconds (hours are not limited to two digits).
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// The anchor of every segment of the published transcript: `t-hh-mm-ss`, with `-2`, `-3` for
/// further segments that start in the same second; `None` for an excluded segment, which is
/// not published.
fn anchors(view: &TranscriptView) -> Vec<Option<String>> {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    view.segments
        .iter()
        .map(|s| {
            if s.excluded {
                return None;
            }
            let base = clock(s.start_ms).replace(':', "-");
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            Some(if *n == 1 {
                format!("t-{base}")
            } else {
                format!("t-{base}-{n}")
            })
        })
        .collect()
}

/// The anchor in the published transcript of the passage that was spoken at `t_ms`: the
/// segment that contains the time, else the last published segment that began before it (a
/// time inside an excluded passage or a pause), else the first. `None` when nothing is
/// published.
pub fn anchor_at(view: &TranscriptView, t_ms: u64) -> Option<String> {
    let anchors = anchors(view);
    let published = || {
        view.segments
            .iter()
            .zip(&anchors)
            .filter_map(|(s, a)| a.as_deref().map(|a| (s, a)))
    };
    published()
        .find(|(s, _)| s.start_ms <= t_ms && t_ms < s.end_ms)
        .or_else(|| published().filter(|(s, _)| s.start_ms <= t_ms).last())
        .or_else(|| published().next())
        .map(|(_, a)| a.to_string())
}

/// Whether `t_ms` falls inside a passage the user excluded. A screenshot taken then is not
/// published either.
pub fn excluded_at(view: &TranscriptView, t_ms: u64) -> bool {
    view.segments
        .iter()
        .any(|s| s.excluded && s.start_ms <= t_ms && t_ms < s.end_ms)
}

/// Escapes what Markdown or HTML would take for formatting in spoken text, and keeps the
/// text on one line.
fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            '<' => out.push_str("&lt;"),
            '&' => out.push_str("&amp;"),
            '\r' | '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `transcript.md` of the repository: one paragraph per published segment with an anchor
/// (`#t-hh-mm-ss`, see [`anchor_at`]), the time and the name of the speaker. Excluded
/// passages are left out exactly as in [`published_text`], and so is everything about them.
pub fn published_markdown(view: &TranscriptView) -> String {
    let names: BTreeMap<&str, &str> = view
        .speakers
        .iter()
        .map(|s| (s.id.as_str(), s.label.as_str()))
        .collect();
    let mut out = String::from("# Transcript\n");
    for (segment, anchor) in view.segments.iter().zip(anchors(view)) {
        let Some(anchor) = anchor else { continue };
        let speaker = names
            .get(segment.speaker.as_str())
            .copied()
            .unwrap_or(segment.speaker.as_str());
        out.push_str(&format!(
            "\n<a id=\"{anchor}\"></a>`{}` **{}:** {}\n",
            clock(segment.start_ms),
            escape_markdown(speaker),
            escape_markdown(segment.text.trim()),
        ));
    }
    out
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

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bbb-tr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
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

    fn segment(id: u32, speaker: &str, start_ms: u64, end_ms: u64, text: &str) -> RawSegment {
        RawSegment {
            id,
            track: "mic".into(),
            speaker: speaker.into(),
            start_ms,
            end_ms,
            text: text.into(),
            words: Vec::new(),
        }
    }

    fn view_of(segments: Vec<RawSegment>, edits: &Edits) -> TranscriptView {
        let mut raw = raw();
        raw.segments = segments;
        compose(&raw, edits)
    }

    #[test]
    fn the_markdown_has_anchors_times_and_speaker_names() {
        let view = compose(&raw(), &Edits::default());
        let md = published_markdown(&view);
        assert_eq!(
            md,
            "# Transcript\n\n<a id=\"t-00-00-00\"></a>`00:00:00` **You:** Hello everyone\n\n<a id=\"t-00-00-01\"></a>`00:00:01` **Speaker 1:** Hi\n"
        );
    }

    #[test]
    fn excluded_passages_leave_no_trace_in_the_markdown() {
        let mut edits = Edits::default();
        edits.segments.insert(
            1,
            SegmentEdit {
                text: Some("secret salary figure".into()),
                excluded: true,
            },
        );
        let view = compose(&raw(), &edits);
        let md = published_markdown(&view);
        assert!(!md.contains("secret") && !md.contains("Speaker 1"), "{md}");
        assert!(!md.contains("00:00:01"), "{md}");
        // What `published_text` publishes is what the markdown has, line for line.
        for line in published_text(&view).lines() {
            assert!(md.contains(line), "{line}");
        }
        assert_eq!(md.matches("<a id=").count(), 1);
    }

    #[test]
    fn edited_text_is_published_and_spoken_markup_is_escaped() {
        let mut edits = Edits::default();
        edits.segments.insert(
            0,
            SegmentEdit {
                text: Some("use *bold* <b>tags</b> & snake_case [x](y)\nsecond line".into()),
                excluded: false,
            },
        );
        let md = published_markdown(&compose(&raw(), &edits));
        assert!(md.contains(
            "use \\*bold\\* &lt;b>tags&lt;/b> &amp; snake\\_case \\[x\\](y) second line"
        ));
    }

    #[test]
    fn anchors_are_unique_and_hours_are_not_cut() {
        let view = view_of(
            vec![
                segment(0, "user", 3_725_000, 3_726_000, "a"),
                segment(1, "user", 3_725_400, 3_727_000, "b"),
                segment(2, "user", 3_725_900, 3_728_000, "c"),
            ],
            &Edits::default(),
        );
        let md = published_markdown(&view);
        for id in ["t-01-02-05", "t-01-02-05-2", "t-01-02-05-3"] {
            assert!(md.contains(&format!("<a id=\"{id}\"></a>")), "{id}\n{md}");
        }
        assert!(md.contains("`01:02:05`"));
    }

    #[test]
    fn a_time_maps_to_the_segment_that_was_speaking() {
        let mut edits = Edits::default();
        edits.segments.insert(
            1,
            SegmentEdit {
                text: None,
                excluded: true,
            },
        );
        let view = view_of(
            vec![
                segment(0, "user", 10_000, 20_000, "first"),
                segment(1, "user", 20_000, 30_000, "excluded"),
                segment(2, "user", 40_000, 50_000, "third"),
            ],
            &edits,
        );
        assert!(excluded_at(&view, 20_000) && excluded_at(&view, 29_999));
        assert!(!excluded_at(&view, 19_999) && !excluded_at(&view, 30_000));
        assert_eq!(anchor_at(&view, 15_000).as_deref(), Some("t-00-00-10"));
        // Inside the excluded passage: the last published one before it.
        assert_eq!(anchor_at(&view, 25_000).as_deref(), Some("t-00-00-10"));
        // In a pause, and after the end.
        assert_eq!(anchor_at(&view, 35_000).as_deref(), Some("t-00-00-10"));
        assert_eq!(anchor_at(&view, 90_000).as_deref(), Some("t-00-00-40"));
        assert_eq!(anchor_at(&view, 45_000).as_deref(), Some("t-00-00-40"));
        // Before everything: the first one.
        assert_eq!(anchor_at(&view, 1_000).as_deref(), Some("t-00-00-10"));
        // Nothing published, nothing to link to.
        let mut all = Edits::default();
        for id in 0..3 {
            all.segments.insert(
                id,
                SegmentEdit {
                    text: None,
                    excluded: true,
                },
            );
        }
        let none = view_of(
            view.segments
                .iter()
                .map(|s| segment(s.id, "user", s.start_ms, s.end_ms, "x"))
                .collect(),
            &all,
        );
        assert_eq!(anchor_at(&none, 15_000), None);
        assert_eq!(published_markdown(&none), "# Transcript\n");
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
