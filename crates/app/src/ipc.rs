//! IPC conventions shared with the UI (`ui/src/ipc/types.ts`).
//!
//! - Commands are snake_case functions and return `Result<T, IpcError>`.
//! - Payloads are camelCase JSON.
//! - Errors carry a stable machine-readable `code` and a human-readable `message`.
//! - Events are kebab-case names (`recording-state`, `meetings-changed`) with the full new
//!   state as payload, so the UI never has to merge partial updates.

use serde::{Deserialize, Serialize};

/// Error returned to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct IpcError {
    pub code: &'static str,
    pub message: String,
}

impl IpcError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The error of a background task that panicked or was cancelled. The text of a panic can
/// hold anything the task was handling (a webhook, a token, a path), so neither the UI nor
/// the log gets it: the UI gets a fixed message, the log the fact only.
pub fn background_task_error(e: &impl std::fmt::Display) -> IpcError {
    let _ = e;
    tracing::error!("a background task failed (panic text withheld)");
    IpcError::new(
        "internal",
        "an internal error stopped this step; try again, and report it if it keeps happening",
    )
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<store::StoreError> for IpcError {
    fn from(e: store::StoreError) -> Self {
        tracing::error!(error = %e, "store error");
        Self::new("internal", e.to_string())
    }
}

impl From<cycle_store::CycleError> for IpcError {
    fn from(e: cycle_store::CycleError) -> Self {
        use cycle_store::CycleError as E;
        let code = match &e {
            E::Invalid { .. } | E::InvalidName(_) => "invalid",
            E::CycleExists(_) => "duplicate",
            E::CycleNotFound(_) | E::TemplateNotFound(_) => "not_found",
            E::GitUnavailable(_) => "git_unavailable",
            E::Git { .. } => "git_failed",
            E::Io { .. } => "io",
            E::Other(_) => "internal",
        };
        tracing::warn!(error = %e, code, "project repository error");
        Self::new(code, e.to_string())
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        tracing::error!(error = %e, "io error");
        Self::new("io", e.to_string())
    }
}

pub type IpcResult<T> = Result<T, IpcError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingState {
    Recording,
    Stopped,
    Recoverable,
    Recovered,
}

impl From<store::MeetingState> for MeetingState {
    fn from(s: store::MeetingState) -> Self {
        match s {
            store::MeetingState::Recording => Self::Recording,
            store::MeetingState::Stopped => Self::Stopped,
            store::MeetingState::Recoverable => Self::Recoverable,
            store::MeetingState::Recovered => Self::Recovered,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub id: String,
    pub title: String,
    pub started_unix_ms: i64,
    pub duration_ms: Option<i64>,
    pub state: MeetingState,
    pub track_count: u32,
    pub screenshot_count: u32,
    /// State of the processing job, when the meeting has one.
    pub job: Option<crate::processing::JobState>,
    #[serde(default)]
    pub cycle_id: Option<String>,
    #[serde(default)]
    pub short_title: Option<String>,
    #[serde(default)]
    pub report_url: Option<String>,
    /// The draft is read-only because the meeting was published or has outbox operations.
    #[serde(default)]
    pub publish_locked: bool,
}

impl From<store::MeetingSummary> for MeetingSummary {
    fn from(s: store::MeetingSummary) -> Self {
        Self {
            id: s.meeting.id,
            title: s.meeting.title,
            started_unix_ms: s.meeting.started_unix_ms,
            duration_ms: s.meeting.duration_ms,
            state: s.meeting.state.into(),
            track_count: s.track_count,
            screenshot_count: s.screenshot_count,
            job: None,
            cycle_id: None,
            short_title: None,
            report_url: None,
            publish_locked: false,
        }
    }
}

/// Derive a concise short title (up to 8 words, 70 characters) from the first summary phrase.
pub fn derive_short_title(summary_text: &str) -> Option<String> {
    let first = summary_text
        .split(['.', '!', '?', '\n', ';'])
        .map(str::trim)
        .find(|s| !s.is_empty())?;
    let words: Vec<&str> = first.split_whitespace().take(8).collect();
    if words.is_empty() {
        return None;
    }
    let mut out = words.join(" ");
    if out.chars().count() > 70 {
        out = out.chars().take(67).collect::<String>();
        out.push_str("...");
    }
    Some(out)
}

/// Read `short_title` from `<meeting_dir>/work/draft.json` or derive it from `summary`.
pub fn read_draft_short_title(meeting_dir: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(meeting_dir.join("work").join("draft.json")).ok()?;
    let val: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if let Some(st) = val
        .get("short_title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(st.to_string());
    }
    val.get("summary")
        .and_then(|v| v.as_str())
        .and_then(derive_short_title)
}

/// Populate `cycle_id`, `short_title` and `report_url` on a [`MeetingSummary`].
pub fn enrich_meeting_summary(
    store: &store::Store,
    recordings_root: &std::path::Path,
    dir: &str,
    summary: &mut MeetingSummary,
) {
    let publication = store.get_publication(&summary.id).ok().flatten();
    summary.cycle_id = store
        .get_report_settings(&summary.id)
        .ok()
        .flatten()
        .and_then(|r| r.cycle_id)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            publication
                .as_ref()
                .map(|p| p.cycle_id.clone())
                .filter(|s| !s.trim().is_empty())
        });
    summary.short_title = read_draft_short_title(&recordings_root.join(dir));
    summary.report_url = publication.and_then(|p| p.report_url);
    summary.publish_locked = publishers::flow::is_locked(store, &summary.id).unwrap_or(false);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub name: String,
    pub source: String,
    pub offset_ms: f64,
    pub ended: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotInfo {
    pub file: String,
    pub offset_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetails {
    pub summary: MeetingSummary,
    pub tracks: Vec<TrackInfo>,
    pub screenshots: Vec<ScreenshotInfo>,
}

impl From<store::MeetingDetails> for MeetingDetails {
    fn from(d: store::MeetingDetails) -> Self {
        let summary = store::MeetingSummary {
            track_count: d.tracks.len() as u32,
            screenshot_count: d.screenshots.len() as u32,
            meeting: d.meeting,
        };
        Self {
            summary: summary.into(),
            tracks: d
                .tracks
                .into_iter()
                .map(|t| TrackInfo {
                    name: t.name,
                    source: t.source,
                    offset_ms: t.offset_ms,
                    ended: t.ended,
                })
                .collect(),
            screenshots: d
                .screenshots
                .into_iter()
                .map(|s| ScreenshotInfo {
                    file: s.file,
                    offset_ms: s.offset_ms,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod background_error_tests {
    use super::*;

    #[test]
    fn a_failed_background_task_never_shows_its_panic_text() {
        let err = background_task_error(
            &"task 1 panicked with message \"https://discord.com/api/webhooks/1/SECRET\"",
        );
        assert_eq!(err.code, "internal");
        assert!(!err.message.contains("SECRET"));
        assert!(!err.message.contains("panicked"));
    }

    #[test]
    fn enrich_meeting_summary_reads_short_title_and_report_url() {
        let dir = test_tmp::TestDir::new("ipc-enrich");
        let store = store::Store::open_in_memory().unwrap();
        store
            .create_meeting(&store::Meeting {
                id: "m1".into(),
                title: "Meeting 1".into(),
                started_unix_ms: 1000,
                duration_ms: Some(5000),
                state: store::MeetingState::Stopped,
                dir: "m1".into(),
            })
            .unwrap();
        store
            .put_publication(&store::Publication {
                meeting_id: "m1".into(),
                cycle_id: "C1".into(),
                folder: Some("C1/2026-11-24".into()),
                repo_key: Some("github.com/o/r".into()),
                commit_sha: Some("abc".into()),
                report_url: Some("https://github.com/o/r/blob/main/report.md".into()),
                snapshot_dir: "snap".into(),
                published_unix_ms: Some(2000),
            })
            .unwrap();
        let work = dir.path().join("m1").join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            work.join("draft.json"),
            r#"{"short_title":"Sprint Launch Plan","summary":"Ignored fallback"}"#,
        )
        .unwrap();

        let mut summary: MeetingSummary = store.list_summaries().unwrap().remove(0).into();
        enrich_meeting_summary(&store, dir.path(), "m1", &mut summary);
        assert_eq!(summary.cycle_id.as_deref(), Some("C1"));
        assert_eq!(summary.short_title.as_deref(), Some("Sprint Launch Plan"));
        assert_eq!(
            summary.report_url.as_deref(),
            Some("https://github.com/o/r/blob/main/report.md")
        );
    }

    #[test]
    fn enrich_meeting_summary_populates_cycle_id_for_unprocessed_zero_participant_meeting() {
        let dir = test_tmp::TestDir::new("ipc-cycle-id");
        let store = store::Store::open_in_memory().unwrap();
        store
            .create_meeting(&store::Meeting {
                id: "m-imported".into(),
                title: "Imported Sync".into(),
                started_unix_ms: 2000,
                duration_ms: None,
                state: store::MeetingState::Stopped,
                dir: "m-imported".into(),
            })
            .unwrap();
        store
            .set_report_settings(
                "m-imported",
                &store::ReportSettings {
                    cycle_id: Some("ZERO-PROJ".into()),
                    report_mode: Some("fixed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(store
            .participants_for_cycle("ZERO-PROJ")
            .unwrap()
            .is_empty());

        let mut summary: MeetingSummary = store.list_summaries().unwrap().remove(0).into();
        enrich_meeting_summary(&store, dir.path(), "m-imported", &mut summary);
        assert_eq!(summary.cycle_id.as_deref(), Some("ZERO-PROJ"));
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["cycleId"], "ZERO-PROJ");
    }
}
