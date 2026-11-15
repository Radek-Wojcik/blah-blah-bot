//! IPC payloads for recording control (mirrored in `ui/src/ipc/types.ts`).

use serde::{Deserialize, Serialize};

use crate::cycles::ReportChoice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Microphone,
    Application,
}

/// A source that can be recorded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Starting,
    Recording,
    Ended,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSource {
    pub id: String,
    pub label: String,
    pub kind: SourceKind,
    pub status: SourceStatus,
    pub offset_ms: f64,
    pub level: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    Idle,
    Recording,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingState {
    pub status: RecordingStatus,
    pub meeting_id: Option<String>,
    pub elapsed_ms: f64,
    pub sources: Vec<ActiveSource>,
    pub screenshot_count: u32,
}

impl RecordingState {
    pub fn idle() -> Self {
        Self {
            status: RecordingStatus::Idle,
            meeting_id: None,
            elapsed_ms: 0.0,
            sources: Vec::new(),
            screenshot_count: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRecordingRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub source_ids: Vec<String>,
    /// The cycle and report mode of the recording. Required: without it (and without
    /// `use_last_report`) the recording does not start (`report_mode_required`).
    #[serde(default)]
    pub report: Option<ReportChoice>,
    /// For the widget and other quick starts: when `report` is absent, use what the last
    /// recording used (still checked against the cycle).
    #[serde(default)]
    pub use_last_report: bool,
}
