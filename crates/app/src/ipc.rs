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
        tracing::warn!(error = %e, code, "cycle repository error");
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
        }
    }
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
}
