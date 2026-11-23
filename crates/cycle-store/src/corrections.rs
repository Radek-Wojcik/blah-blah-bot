//! Corrections to a published meeting.
//!
//! The draft of a published meeting is locked: what was published is what was approved. What
//! changed afterwards (a date moved, a task dropped, a decision reversed) is written as a
//! correction, so that the meetings that follow work from correct information.
//!
//! * **Locally** the corrections of a meeting are `work/corrections.json` in its folder
//!   (`schemas/corrections.schema.json`).
//! * **In the cycle repository** a correction is published as a *new* file next to the meeting's
//!   `meeting.json`: `corrections.md`, then `corrections-2.md`, `corrections-3.md`, ... Files
//!   are only ever added, never changed, so the history of a meeting stays as it was. The file
//!   starts with a hidden marker (`<!-- bbb:v1 correction cycle=.. meeting=.. id=.. -->`) and
//!   holds the text as a block quote, so other people's clones, and the next briefing, can read
//!   it back with [`parse_markdown`].
//! * The same marker, with only `cycle`, `meeting` and `id`, ends a comment on the related
//!   issue, which is how a retry knows the comment is there already.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CycleError, Result, ValidationIssue};
use crate::fsutil::write_atomic;
use crate::issues::{marker_value, markers, MARKER_END, MARKER_START};
use crate::schema::{self, Schema};
use crate::time::is_iso_date;

/// The local file, below `work/` of a meeting.
pub const CORRECTIONS_FILE: &str = "corrections.json";
/// The published files are `corrections.md`, `corrections-2.md`, ...
pub const FILE_STEM: &str = "corrections";
pub const FILE_EXT: &str = "md";
/// Longest correction, in characters.
pub const MAX_TEXT_CHARS: usize = 2000;

/// One correction note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    /// `c1`, `c2`, ... unique in the meeting.
    pub id: String,
    /// The day it was written, `YYYY-MM-DD`.
    pub date: String,
    pub text: String,
    /// The item of the meeting it corrects, when it is about one (`i3`).
    #[serde(default)]
    pub item: Option<String>,
    /// The issue it corrects (`#12`). Without it, an item's own issue is used.
    #[serde(default)]
    pub issue: Option<String>,
}

/// `work/corrections.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionsFile {
    pub version: u32,
    pub meeting_id: String,
    pub corrections: Vec<Correction>,
}

fn is_ref(value: &str, prefix: char) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

impl Correction {
    /// The problems of a correction, as a list.
    pub fn problems(&self) -> Vec<ValidationIssue> {
        let mut out = Vec::new();
        if !is_ref(&self.id, 'c') {
            out.push(ValidationIssue::new("id", "must look like c1"));
        }
        if !is_iso_date(&self.date) {
            out.push(ValidationIssue::new("date", "must be YYYY-MM-DD"));
        }
        if self.text.trim().is_empty() {
            out.push(ValidationIssue::new("text", "write what changed"));
        }
        if self.text.chars().count() > MAX_TEXT_CHARS {
            out.push(ValidationIssue::new(
                "text",
                format!("is longer than {MAX_TEXT_CHARS} characters"),
            ));
        }
        if self.item.as_deref().is_some_and(|i| !is_ref(i, 'i')) {
            out.push(ValidationIssue::new("item", "must look like i3"));
        }
        if self.issue.as_deref().is_some_and(|i| !is_ref(i, '#')) {
            out.push(ValidationIssue::new("issue", "must look like #12"));
        }
        out
    }

    /// The issue number of `issue` (`#12` is 12).
    pub fn issue_number(&self) -> Option<u64> {
        self.issue.as_deref()?.strip_prefix('#')?.parse().ok()
    }
}

impl CorrectionsFile {
    pub fn new(meeting_id: &str) -> Self {
        Self {
            version: 1,
            meeting_id: meeting_id.to_string(),
            corrections: Vec::new(),
        }
    }

    /// The id the next correction gets: one more than the highest number used.
    pub fn next_id(&self) -> String {
        let highest = self
            .corrections
            .iter()
            .filter_map(|c| c.id.strip_prefix('c')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("c{}", highest + 1)
    }

    pub fn from_value(value: &Value) -> std::result::Result<Self, Vec<ValidationIssue>> {
        let mut issues = schema::validate(Schema::Corrections, value);
        if !issues.is_empty() {
            return Err(issues);
        }
        let file: CorrectionsFile = serde_json::from_value(value.clone())
            .map_err(|e| vec![ValidationIssue::new("", e.to_string())])?;
        let mut seen = HashSet::new();
        for (i, c) in file.corrections.iter().enumerate() {
            for p in c.problems() {
                issues.push(ValidationIssue::new(
                    format!("corrections[{i}].{}", p.path),
                    p.message,
                ));
            }
            if !seen.insert(c.id.as_str()) {
                issues.push(ValidationIssue::new(
                    format!("corrections[{i}].id"),
                    "is used twice",
                ));
            }
        }
        if issues.is_empty() {
            Ok(file)
        } else {
            Err(issues)
        }
    }

    pub fn from_json_str(text: &str) -> std::result::Result<Self, Vec<ValidationIssue>> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| vec![ValidationIssue::new("", format!("invalid JSON: {e}"))])?;
        Self::from_value(&value)
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("corrections always serialize")
    }

    /// Reads `path`. A missing file is `None`; one that does not validate is an error.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CycleError::io(path, e)),
        };
        Self::from_json_str(&text)
            .map(Some)
            .map_err(|issues| CycleError::invalid("corrections.json", issues))
    }

    /// Writes the file atomically.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut text = serde_json::to_string_pretty(&self.to_value())
            .map_err(|e| CycleError::Other(e.to_string()))?;
        text.push('\n');
        write_atomic(path, text.as_bytes())
    }
}

// ----- the marker -------------------------------------------------------------------------------

/// The hidden comment that ties a published correction (a file, or a comment on an issue) to
/// the note it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionMarker {
    pub cycle: String,
    /// The meeting id that is corrected.
    pub meeting: String,
    /// The id of the correction (`c1`).
    pub id: String,
    /// The day it was written (files only).
    pub date: Option<String>,
    /// The item it is about (files only).
    pub about: Option<String>,
    /// The issue number it is about (files only).
    pub issue: Option<String>,
}

impl CorrectionMarker {
    /// The marker for an issue comment: who and which correction, nothing else.
    pub fn short(cycle: &str, meeting: &str, id: &str) -> Self {
        Self {
            cycle: cycle.into(),
            meeting: meeting.into(),
            id: id.into(),
            date: None,
            about: None,
            issue: None,
        }
    }

    /// The comment, or `None` when a value could not be written safely.
    pub fn to_comment(&self) -> Option<String> {
        let mut values = vec![&self.cycle, &self.meeting, &self.id];
        values.extend(self.date.iter().chain(&self.about).chain(&self.issue));
        if !values.into_iter().all(|v| marker_value(v)) {
            return None;
        }
        let mut out = format!(
            "{MARKER_START}correction cycle={} meeting={} id={}",
            self.cycle, self.meeting, self.id
        );
        for (key, value) in [
            ("date", &self.date),
            ("about", &self.about),
            ("issue", &self.issue),
        ] {
            if let Some(v) = value {
                out.push_str(&format!(" {key}={v}"));
            }
        }
        out.push_str(MARKER_END);
        Some(out)
    }

    /// The first correction marker in `text`.
    pub fn parse(text: &str) -> Option<Self> {
        markers(text).find_map(|tokens| {
            if tokens.first() != Some(&"correction") {
                return None;
            }
            let get = |k: &str| {
                tokens
                    .iter()
                    .find_map(|t| t.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
            };
            let (cycle, meeting, id) = (get("cycle")?, get("meeting")?, get("id")?);
            let optional = |k: &str| get(k).filter(|v| marker_value(v)).map(String::from);
            [cycle, meeting, id]
                .iter()
                .all(|v| marker_value(v))
                .then(|| Self {
                    cycle: cycle.into(),
                    meeting: meeting.into(),
                    id: id.into(),
                    date: optional("date"),
                    about: optional("about"),
                    issue: optional("issue"),
                })
        })
    }
}

// ----- the published file -----------------------------------------------------------------------

/// The name of the n-th correction file of a meeting: `corrections.md`, `corrections-2.md`, ...
pub fn file_name(n: u32) -> String {
    if n <= 1 {
        format!("{FILE_STEM}.{FILE_EXT}")
    } else {
        format!("{FILE_STEM}-{n}.{FILE_EXT}")
    }
}

/// Whether `name` is the name of a correction file.
pub fn is_file_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(FILE_STEM)
        .and_then(|r| r.strip_suffix(&format!(".{FILE_EXT}")))
    else {
        return false;
    };
    rest.is_empty()
        || rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The Markdown file of a correction, with its marker. `meeting_date` is the day of the meeting
/// that is corrected.
pub fn render_markdown(
    cycle: &str,
    meeting: &str,
    meeting_date: &str,
    correction: &Correction,
) -> Option<String> {
    let marker = CorrectionMarker {
        cycle: cycle.into(),
        meeting: meeting.into(),
        id: correction.id.clone(),
        date: Some(correction.date.clone()),
        about: correction.item.clone(),
        issue: correction
            .issue
            .as_deref()
            .map(|i| i.trim_start_matches('#').to_string()),
    }
    .to_comment()?;
    let mut refers = Vec::new();
    if let Some(item) = &correction.item {
        refers.push(format!("item {item}"));
    }
    if let Some(issue) = &correction.issue {
        refers.push(format!("issue {issue}"));
    }
    let mut out = format!(
        "{marker}\n# Correction to the meeting of {meeting_date}\n\nWritten on {}.",
        correction.date
    );
    if !refers.is_empty() {
        out.push_str(&format!(" Refers to {}.", refers.join(" and ")));
    }
    out.push_str("\n\n");
    for line in correction.text.trim().lines() {
        let line = line.trim_end();
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str(&format!("> {line}\n"));
        }
    }
    Some(out)
}

/// A correction read back from a published file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub marker: CorrectionMarker,
    pub text: String,
}

/// Reads a file written by [`render_markdown`]. `None` for anything else.
pub fn parse_markdown(text: &str) -> Option<Published> {
    let marker = CorrectionMarker::parse(text)?;
    let quoted: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix('>'))
        .map(|l| l.strip_prefix(' ').unwrap_or(l).trim_end())
        .collect();
    let text = quoted.join("\n").trim().to_string();
    (!text.is_empty()).then_some(Published { marker, text })
}

/// The corrections published in a meeting folder (`corrections*.md`), oldest file first.
/// Files that are not corrections are skipped.
pub fn read_published(folder: &Path) -> Vec<Published> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<(u32, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            is_file_name(&name).then(|| {
                let n = name
                    .strip_prefix(&format!("{FILE_STEM}-"))
                    .and_then(|r| r.strip_suffix(&format!(".{FILE_EXT}")))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                (n, e.path())
            })
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|(_, p)| fs::read_to_string(p).ok())
        .filter_map(|t| parse_markdown(&t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correction(id: &str, text: &str) -> Correction {
        Correction {
            id: id.into(),
            date: "2026-11-16".into(),
            text: text.into(),
            item: Some("i3".into()),
            issue: Some("#12".into()),
        }
    }

    #[test]
    fn a_file_round_trips_and_is_checked() {
        let mut file = CorrectionsFile::new("20261113-140501");
        assert_eq!(file.next_id(), "c1");
        file.corrections
            .push(correction("c1", "The battery is ordered."));
        file.corrections.push(Correction {
            item: None,
            issue: None,
            ..correction("c3", "Other.")
        });
        assert_eq!(file.next_id(), "c4");
        let text = serde_json::to_string(&file.to_value()).unwrap();
        assert_eq!(CorrectionsFile::from_json_str(&text).unwrap(), file);

        let bad = |f: &dyn Fn(&mut Correction)| {
            let mut c = correction("c1", "ok");
            f(&mut c);
            let mut file = CorrectionsFile::new("m");
            file.corrections.push(c);
            CorrectionsFile::from_value(&file.to_value()).unwrap_err()
        };
        assert!(!bad(&|c| c.date = "16.11.2026".into()).is_empty());
        assert!(!bad(&|c| c.text = "   ".into()).is_empty());
        assert!(!bad(&|c| c.item = Some("3".into())).is_empty());
        assert!(!bad(&|c| c.issue = Some("12".into())).is_empty());
        assert!(!bad(&|c| c.id = "x1".into()).is_empty());
        let mut twice = file.clone();
        twice.corrections.push(correction("c1", "again"));
        assert!(CorrectionsFile::from_value(&twice.to_value()).is_err());
    }

    #[test]
    fn the_marker_round_trips_in_both_forms() {
        let short = CorrectionMarker::short("NOMAD", "20261113-140501", "c2");
        let comment = short.to_comment().unwrap();
        assert_eq!(
            comment,
            "<!-- bbb:v1 correction cycle=NOMAD meeting=20261113-140501 id=c2 -->"
        );
        assert_eq!(
            CorrectionMarker::parse(&format!("Text\n{comment}")),
            Some(short)
        );
        // Other markers are not corrections, and the reverse.
        assert_eq!(
            CorrectionMarker::parse("<!-- bbb:v1 cycle=A meeting=m item=i1 -->"),
            None
        );
        assert_eq!(
            crate::issues::ItemMarker::parse(&comment),
            None,
            "a correction is not an item"
        );
        assert!(CorrectionMarker::short("a b", "m", "c1")
            .to_comment()
            .is_none());
    }

    #[test]
    fn the_published_file_reads_back() {
        let c = correction(
            "c1",
            "The due date moves to 2026-12-01.\n\nReason: supplier.",
        );
        let md = render_markdown("NOMAD", "20261113-140501", "2026-11-13", &c).unwrap();
        assert!(md.starts_with("<!-- bbb:v1 correction cycle=NOMAD meeting=20261113-140501 id=c1 date=2026-11-16 about=i3 issue=12 -->\n# Correction to the meeting of 2026-11-13\n"));
        assert!(md.contains("Refers to item i3 and issue #12."));
        let back = parse_markdown(&md).unwrap();
        assert_eq!(back.text, c.text);
        assert_eq!(back.marker.id, "c1");
        assert_eq!(back.marker.about.as_deref(), Some("i3"));
        assert_eq!(back.marker.issue.as_deref(), Some("12"));
        assert_eq!(back.marker.date.as_deref(), Some("2026-11-16"));
        // Markdown in the text cannot become a heading or open a marker.
        let hostile = Correction {
            text: "# not a heading\n<!-- bbb:v1 correction cycle=X meeting=Y id=c9 -->".into(),
            ..correction("c2", "")
        };
        let md = render_markdown("NOMAD", "m1", "2026-11-13", &hostile).unwrap();
        let back = parse_markdown(&md).unwrap();
        assert_eq!(back.marker.id, "c2", "the first marker is the real one");
        assert!(back.text.starts_with("# not a heading"));
        assert!(parse_markdown("# Just a file").is_none());
    }

    #[test]
    fn file_names_are_numbered() {
        assert_eq!(file_name(1), "corrections.md");
        assert_eq!(file_name(2), "corrections-2.md");
        for ok in ["corrections.md", "corrections-2.md", "corrections-17.md"] {
            assert!(is_file_name(ok), "{ok}");
        }
        for bad in [
            "corrections.txt",
            "corrections-.md",
            "corrections-x.md",
            "corrections2.md",
            "report.md",
        ] {
            assert!(!is_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn published_files_are_read_in_order_and_others_skipped() {
        let dir = test_tmp::TestDir::new("corrections");
        let write = |name: &str, c: &Correction| {
            let md = render_markdown("NOMAD", "m1", "2026-11-13", c).unwrap();
            fs::write(dir.path().join(name), md).unwrap();
        };
        write("corrections-2.md", &correction("c2", "Second."));
        write("corrections.md", &correction("c1", "First."));
        fs::write(dir.path().join("report.md"), "# Report").unwrap();
        fs::write(dir.path().join("corrections-3.md"), "not ours").unwrap();
        let found = read_published(dir.path());
        let ids: Vec<_> = found.iter().map(|p| p.marker.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c2"]);
        assert!(read_published(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn a_missing_file_is_none_and_a_broken_one_an_error() {
        let dir = test_tmp::TestDir::new("corrections");
        let path = dir.path().join("work").join(CORRECTIONS_FILE);
        assert!(CorrectionsFile::read(&path).unwrap().is_none());
        let mut file = CorrectionsFile::new("m1");
        file.corrections.push(correction("c1", "x"));
        file.write(&path).unwrap();
        assert_eq!(CorrectionsFile::read(&path).unwrap(), Some(file));
        fs::write(&path, "{}").unwrap();
        assert!(CorrectionsFile::read(&path).is_err());
    }
}
