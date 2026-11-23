//! Recording control: starts and stops sessions, keeps the database in step with the
//! journal and answers what the widget shows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use recorder::recovery::{recover, RecoveredMeeting};
use recorder::session::{CaptureBackend, Session, SessionError, SessionSnapshot, SourceSpec};
use store::{Meeting, MeetingState, ReportSettings, Screenshot, Segment, Store, Track};

use crate::ipc::{IpcError, IpcResult};
use crate::recording_dto::{
    ActiveSource, RecordingState, RecordingStatus, SourceInfo, SourceKind, SourceStatus,
    StartRecordingRequest,
};

/// Seconds in one audio segment of a real recording.
const SEGMENT_SECONDS: u32 = recorder::session::DEFAULT_SEGMENT_SECONDS;

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYYMMDD-HHMMSS` (UTC), used as the meeting id and folder name.
pub fn meeting_id(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// Default title: `Meeting YYYY-MM-DD HH:MM` (UTC).
pub fn default_title(unix_ms: u64) -> String {
    let id = meeting_id(unix_ms);
    format!(
        "Meeting {}-{}-{} {}:{}",
        &id[0..4],
        &id[4..6],
        &id[6..8],
        &id[9..11],
        &id[11..13]
    )
}

/// Write what the journal says about a meeting into the database.
pub fn persist(
    store: &Store,
    meeting_id: &str,
    m: &RecoveredMeeting,
    state: MeetingState,
) -> store::Result<()> {
    for t in &m.tracks {
        store.upsert_track(
            meeting_id,
            &Track {
                name: t.name.clone(),
                source: t.source.clone(),
                pid: t.pid,
                offset_ms: t.offset_ms,
                ended: t.ended,
                end_offset_ms: Some(t.end_offset_ms),
                total_samples: Some(t.total_samples),
            },
        )?;
        for s in &t.segments {
            store.add_segment(
                meeting_id,
                &Segment {
                    track: t.name.clone(),
                    segment: s.segment,
                    file: s.file.clone(),
                    offset_ms: s.offset_ms,
                    samples: s.samples,
                },
            )?;
        }
    }
    for (file, offset_ms) in &m.screenshots {
        store.add_screenshot(
            meeting_id,
            &Screenshot {
                file: file.clone(),
                offset_ms: *offset_ms,
            },
        )?;
    }
    store.set_meeting_state(meeting_id, state, Some(m.duration_ms.round() as i64))
}

fn session_error(e: SessionError) -> IpcError {
    match e {
        SessionError::SourceAlreadyAdded(id) => {
            IpcError::new("source_already_added", format!("{id} is already recorded"))
        }
        SessionError::SourceNotFound(id) => IpcError::new(
            "source_not_found",
            format!("{id} is not part of this recording"),
        ),
        SessionError::Transition(e) => IpcError::new("source_not_found", e.to_string()),
        SessionError::Recorder(e) => IpcError::new("io", e.to_string()),
    }
}

struct Active {
    session: Session,
    meeting_id: String,
    screenshots: u32,
    last: RecordingState,
}

/// Owns the one recording the application can have at a time.
pub struct Recorder {
    store: Arc<Mutex<Store>>,
    backend: Arc<dyn CaptureBackend>,
    demo: bool,
    active: Mutex<Option<Active>>,
}

fn to_dto(meeting_id: &str, screenshots: u32, snap: SessionSnapshot) -> RecordingState {
    RecordingState {
        status: RecordingStatus::Recording,
        meeting_id: Some(meeting_id.to_string()),
        elapsed_ms: snap.elapsed_ms,
        screenshot_count: screenshots,
        sources: snap
            .sources
            .into_iter()
            .map(|s| ActiveSource {
                kind: if s.id.starts_with("mic:") {
                    SourceKind::Microphone
                } else {
                    SourceKind::Application
                },
                status: match s.phase {
                    "starting" => SourceStatus::Starting,
                    "recording" => SourceStatus::Recording,
                    "ended" => SourceStatus::Ended,
                    _ => SourceStatus::Error,
                },
                id: s.id,
                label: s.label,
                offset_ms: s.offset_ms,
                level: s.level,
                detail: s.detail,
            })
            .collect(),
    }
}

impl Recorder {
    pub fn new(store: Arc<Mutex<Store>>, backend: Arc<dyn CaptureBackend>, demo: bool) -> Self {
        Self {
            store,
            backend,
            demo,
            active: Mutex::new(None),
        }
    }

    fn lock_active(&self) -> IpcResult<std::sync::MutexGuard<'_, Option<Active>>> {
        self.active
            .lock()
            .map_err(|_| IpcError::new("internal", "recorder lock poisoned"))
    }

    fn with_store<T>(&self, f: impl FnOnce(&Store) -> store::Result<T>) -> IpcResult<T> {
        let g = self
            .store
            .lock()
            .map_err(|_| IpcError::new("internal", "store lock poisoned"))?;
        Ok(f(&g)?)
    }

    /// Sources that can be recorded now.
    pub fn list_sources(&self) -> IpcResult<Vec<SourceInfo>> {
        if self.demo {
            let tone = |hz: u32, label: &str, active| SourceInfo {
                id: format!("tone:{hz}"),
                kind: if hz == 220 {
                    SourceKind::Microphone
                } else {
                    SourceKind::Application
                },
                label: label.into(),
                active,
            };
            return Ok(vec![
                tone(220, "Microphone (demo tone)", true),
                tone(440, "Discord (demo tone)", true),
                tone(660, "Teams (demo tone)", false),
                tone(880, "Chrome (demo tone)", true),
            ]);
        }
        let mut out = vec![SourceInfo {
            id: "mic:default".into(),
            kind: SourceKind::Microphone,
            label: "Microphone (default)".into(),
            active: true,
        }];
        let sessions =
            recorder::sessions::list_sessions().map_err(|e| IpcError::new("io", e.to_string()))?;
        let own = std::process::id();
        let mut seen: Vec<String> = Vec::new();
        for s in &sessions {
            if s.pid == own || s.process.is_empty() || seen.contains(&s.process) {
                continue;
            }
            seen.push(s.process.clone());
            // Record the topmost process of the application so helper processes are included.
            let group: Vec<_> = sessions.iter().filter(|x| x.process == s.process).collect();
            let top = group
                .iter()
                .find(|x| !group.iter().any(|y| Some(y.pid) == x.parent_pid))
                .unwrap_or(&group[0]);
            let label = if s.display_name.trim().is_empty() {
                s.process.trim_end_matches(".exe").to_string()
            } else {
                s.display_name.clone()
            };
            out.push(SourceInfo {
                id: format!("app:{}", top.pid),
                kind: SourceKind::Application,
                label,
                active: group.iter().any(|x| x.state.eq_ignore_ascii_case("active")),
            });
        }
        Ok(out)
    }

    /// Starts a recording. `report` is the validated cycle and report mode; it is written to
    /// the meeting right after the meeting is created.
    pub fn start(
        &self,
        root: &Path,
        req: StartRecordingRequest,
        report: ReportSettings,
    ) -> IpcResult<()> {
        let mut guard = self.lock_active()?;
        if guard.is_some() {
            return Err(IpcError::new(
                "already_recording",
                "A recording is already running",
            ));
        }
        let mut specs = Vec::new();
        for id in &req.source_ids {
            let spec = SourceSpec::parse(id)
                .map_err(|e| IpcError::new("source_not_found", e.to_string()))?;
            specs.push(spec);
        }
        if specs.is_empty() {
            return Err(IpcError::new(
                "source_not_found",
                "Choose at least one source",
            ));
        }
        let labels = self.list_sources().unwrap_or_default();
        let label_of = |id: &str| {
            labels
                .iter()
                .find(|s| s.id == id)
                .map_or_else(|| id.to_string(), |s| s.label.clone())
        };

        let started = recorder::sysutil::unix_ms();
        let id = meeting_id(started);
        let dir: PathBuf = root.join(&id);
        let mut session = Session::start(&dir, SEGMENT_SECONDS, self.backend.clone())
            .map_err(|e| IpcError::new("io", e.to_string()))?;
        let title = req
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| default_title(started));
        self.with_store(|s| {
            s.create_meeting(&Meeting {
                id: id.clone(),
                title,
                started_unix_ms: started as i64,
                duration_ms: None,
                state: MeetingState::Recording,
                dir: id.clone(),
            })?;
            s.set_report_settings(&id, &report).map(|_| ())
        })?;
        for spec in specs {
            let label = label_of(&spec.id());
            session.add_source(spec, &label).map_err(session_error)?;
        }
        tracing::info!(meeting = %id, sources = req.source_ids.len(), "recording started");
        *guard = Some(Active {
            session,
            meeting_id: id,
            screenshots: 0,
            last: RecordingState::idle(),
        });
        Ok(())
    }

    pub fn stop(&self) -> IpcResult<String> {
        let active = self
            .lock_active()?
            .take()
            .ok_or_else(|| IpcError::new("not_recording", "Not recording"))?;
        let dir = active.session.dir().to_path_buf();
        active
            .session
            .stop()
            .map_err(|e| IpcError::new("io", e.to_string()))?;
        let recovered = recover(&dir).map_err(|e| IpcError::new("io", e.to_string()))?;
        self.with_store(|s| persist(s, &active.meeting_id, &recovered, MeetingState::Stopped))?;
        tracing::info!(meeting = %active.meeting_id, "recording stopped");
        Ok(active.meeting_id)
    }

    pub fn add_source(&self, id: &str) -> IpcResult<()> {
        let spec =
            SourceSpec::parse(id).map_err(|e| IpcError::new("source_not_found", e.to_string()))?;
        let label = self
            .list_sources()?
            .into_iter()
            .find(|s| s.id == id)
            .map_or_else(|| id.to_string(), |s| s.label);
        let mut guard = self.lock_active()?;
        let a = guard
            .as_mut()
            .ok_or_else(|| IpcError::new("not_recording", "Not recording"))?;
        a.session
            .add_source(spec, &label)
            .map(|_| ())
            .map_err(session_error)
    }

    pub fn remove_source(&self, id: &str) -> IpcResult<()> {
        let mut guard = self.lock_active()?;
        let a = guard
            .as_mut()
            .ok_or_else(|| IpcError::new("not_recording", "Not recording"))?;
        a.session.remove_source(id).map_err(session_error)
    }

    /// Current time on the session clock, if a recording is running.
    pub fn now_ms(&self) -> Option<f64> {
        let g = self.active.lock().ok()?;
        g.as_ref().map(|a| a.session.now_ms())
    }

    /// Save a cropped screenshot taken at `offset_ms`: the image is written first, then
    /// the journal entry, so the journal never points at a missing file.
    pub fn save_screenshot(
        &self,
        frame: &recorder::screenshot::Frame,
        offset_ms: f64,
    ) -> IpcResult<String> {
        let mut guard = self.lock_active()?;
        let a = guard
            .as_mut()
            .ok_or_else(|| IpcError::new("not_recording", "Not recording"))?;
        let file = a.session.peek_screenshot_name(offset_ms);
        std::fs::create_dir_all(a.session.dir().join("img"))?;
        recorder::screenshot::write_png(frame, &a.session.dir().join(&file))
            .map_err(|e| IpcError::new("io", e.to_string()))?;
        let logged = a
            .session
            .log_screenshot(offset_ms)
            .map_err(|e| IpcError::new("io", e.to_string()))?;
        a.screenshots += 1;
        Ok(logged)
    }

    /// State for the widget. Reads and resets the level meters, so call it from one place.
    pub fn tick(&self) -> Option<RecordingState> {
        let mut guard = self.active.lock().ok()?;
        let a = guard.as_mut()?;
        let state = to_dto(&a.meeting_id, a.screenshots, a.session.snapshot());
        a.last = state.clone();
        Some(state)
    }

    /// The last state computed by [`Recorder::tick`], or idle.
    pub fn current(&self) -> RecordingState {
        self.active
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|a| a.last.clone()))
            .filter(|s| s.status == RecordingStatus::Recording)
            .unwrap_or_else(RecordingState::idle)
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Rebuild a meeting that was interrupted from its journal.
    pub fn recover_meeting(&self, root: &Path, id: &str) -> IpcResult<()> {
        let dir = root.join(id);
        let recovered = recover(&dir).map_err(|e| IpcError::new("io", e.to_string()))?;
        self.with_store(|s| persist(s, id, &recovered, MeetingState::Recovered))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_id_formats_utc() {
        // 2026-11-13 14:25:01 UTC
        let ms = 1_794_579_901_000;
        assert_eq!(meeting_id(ms), "20261113-142501");
        assert_eq!(default_title(ms), "Meeting 2026-11-13 14:25");
    }

    #[test]
    fn meeting_id_handles_epoch_and_leap_day() {
        assert_eq!(meeting_id(0), "19700101-000000");
        // 2024-02-29 23:59:59 UTC
        assert_eq!(meeting_id(1_709_251_199_000), "20240229-235959");
    }

    #[test]
    fn a_started_recording_carries_its_cycle_and_report_settings() {
        use recorder::session::ToneBackend;
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let recorder = Recorder::new(store.clone(), Arc::new(ToneBackend), true);
        let root = test_tmp::TestDir::new("recording");
        let report = ReportSettings {
            cycle_id: Some("NOMAD".into()),
            report_mode: Some("by-type".into()),
            report_template: None,
            meeting_type: Some("budget".into()),
            output_language: Some("pl".into()),
        };
        let req = StartRecordingRequest {
            title: None,
            source_ids: vec!["tone:220".into()],
            report: None,
            use_last_report: false,
        };
        recorder.start(root.path(), req, report.clone()).unwrap();
        let id = recorder.stop().unwrap();
        let guard = store.lock().unwrap();
        assert_eq!(guard.get_report_settings(&id).unwrap(), Some(report));
    }
}
