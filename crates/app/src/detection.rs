//! Meeting detection in the application: a polling thread feeds the audio sessions of this
//! machine (and the Google Meet reports of the browser extension) to the rules of
//! `recorder::detect`, shows a small prompt window when a call is found and handles the answer.
//!
//! Events: `detection-prompt` (payload: [`PromptDto`]) and `detection-cleared` (payload: the
//! call key). Commands: `get_detection_settings`, `save_detection_settings`,
//! `detection_prompt` (the prompt on offer, for a window that opened after the event) and
//! `answer_detection`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use recorder::detect::{Answer, AppSession, Detector, Event, Prompt, Source, MEET_APP};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::commands;
use crate::ipc::{IpcError, IpcResult};
use crate::recording_dto::StartRecordingRequest;
use crate::settings::DetectionSettings;
use crate::state::AppState;

pub const EVENT_PROMPT: &str = "detection-prompt";
pub const EVENT_CLEARED: &str = "detection-cleared";
const WINDOW: &str = "detection";
const PROMPT_SIZE: (f64, f64) = (380.0, 150.0);
const MARGIN: f64 = 16.0;

/// The prompt as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptDto {
    pub key: String,
    /// Application stem, used for "Never for ...".
    pub app: String,
    /// Name to show.
    pub display: String,
    pub source: Source,
}

impl From<&Prompt> for PromptDto {
    fn from(p: &Prompt) -> Self {
        Self {
            key: p.key.clone(),
            app: p.app.clone(),
            display: p.display.clone(),
            source: p.source,
        }
    }
}

/// What came of an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerOutcome {
    /// A recording runs now because of the answer.
    pub started: bool,
    /// The last report choice could not be used: the recorder window was opened so the user
    /// picks the cycle and report mode there.
    pub needs_report: bool,
}

/// The answers the UI may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    Record,
    NotNow,
    Never,
}

/// State shared between the polling thread, the commands and the bridge hook.
pub struct Detection {
    detector: Mutex<Detector>,
    /// Topmost process of the application of each call on offer, for "Record".
    pids: Mutex<BTreeMap<String, u32>>,
    /// The applications the session monitor maps processes to.
    known: Mutex<Vec<String>>,
    started: AtomicBool,
    clock: Instant,
}

impl Detection {
    pub fn new(settings: &DetectionSettings) -> Arc<Self> {
        let cfg = settings.to_config();
        Arc::new(Self {
            known: Mutex::new(cfg.known_apps.clone()),
            detector: Mutex::new(Detector::new(cfg)),
            pids: Mutex::new(BTreeMap::new()),
            started: AtomicBool::new(false),
            clock: Instant::now(),
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// A Google Meet tab joined or left its call (reported by the extension). The bridge hook
    /// in `browser.rs` calls this; until that hook is connected nothing calls it.
    pub fn meet(&self, tab_id: u64, joined: bool) {
        if let Ok(mut d) = self.detector.lock() {
            d.meet(self.now_ms(), tab_id, joined);
        }
    }

    /// Take new settings. Returns the events the change caused (a prompt withdrawn because
    /// its application was opted out).
    pub fn apply(&self, settings: &DetectionSettings) -> Vec<Event> {
        let cfg = settings.to_config();
        if let Ok(mut k) = self.known.lock() {
            k.clone_from(&cfg.known_apps);
        }
        self.detector
            .lock()
            .map(|mut d| d.set_config(cfg))
            .unwrap_or_default()
    }

    /// The prompt on offer.
    pub fn prompt(&self) -> Option<PromptDto> {
        self.detector
            .lock()
            .ok()
            .and_then(|d| d.prompt())
            .as_ref()
            .map(PromptDto::from)
    }

    fn answer(&self, key: &str, answer: Answer) {
        if let Ok(mut d) = self.detector.lock() {
            d.answer(key, answer);
        }
    }

    fn pid_of(&self, key: &str) -> Option<u32> {
        self.pids.lock().ok()?.get(key).copied()
    }

    fn poll_ms(&self) -> u64 {
        self.detector.lock().map_or(1000, |d| d.poll_ms())
    }

    fn enabled(&self) -> bool {
        self.detector.lock().is_ok_and(|d| d.config().enabled)
    }

    /// Start the polling thread (once). Not in demo mode: demo data has no real sessions.
    pub fn start(self: &Arc<Self>, app: AppHandle) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("meeting-detection".into())
            .spawn(move || me.run(&app));
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the meeting detection");
        }
    }

    fn run(&self, app: &AppHandle) {
        let known = self.known.lock().map(|k| k.clone()).unwrap_or_default();
        let mut monitor = match recorder::sessions::SessionMonitor::new(known) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "meeting detection is off: audio sessions are not available");
                return;
            }
        };
        let mut failures = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(self.poll_ms()));
            if !self.enabled() {
                continue;
            }
            if let Ok(k) = self.known.lock() {
                monitor.set_known(k.clone());
            }
            let sessions = match monitor.poll() {
                Ok(s) => {
                    failures = 0;
                    s
                }
                Err(e) => {
                    failures += 1;
                    if failures == 1 {
                        tracing::warn!(error = %e, "audio session poll failed");
                    }
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let recorded = recorded_apps(app, &sessions);
            let events = {
                let Ok(mut d) = self.detector.lock() else {
                    return;
                };
                d.set_recorded(&recorded);
                d.observe(self.now_ms(), &sessions)
            };
            for e in &events {
                if let Event::Prompt(p) = e {
                    if let Some(s) = sessions.iter().find(|s| s.app == p.app) {
                        if let Ok(mut pids) = self.pids.lock() {
                            pids.insert(p.key.clone(), s.pid);
                        }
                    }
                }
            }
            handle_events(app, &events);
        }
    }
}

/// Applications that are being recorded now: `app:<pid>` sources map to the application of
/// the session with that process, a tab of Google Meet maps to `meet`.
fn recorded_apps(app: &AppHandle, sessions: &[AppSession]) -> Vec<String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Vec::new();
    };
    let current = state.recorder.current();
    let mut out: Vec<String> = Vec::new();
    for s in &current.sources {
        if s.status == crate::recording_dto::SourceStatus::Ended
            || s.status == crate::recording_dto::SourceStatus::Error
        {
            continue;
        }
        if let Some(pid) =
            s.id.strip_prefix("app:")
                .and_then(|p| p.parse::<u32>().ok())
        {
            if let Some(x) = sessions.iter().find(|x| x.pid == pid) {
                out.push(x.app.clone());
            }
        } else if s.id.starts_with("tab:") {
            let on_meet = state
                .browser()
                .status()
                .streams
                .iter()
                .any(|t| t.host == "meet.google.com" && s.id == format!("tab:{}", t.stream_id));
            if on_meet {
                out.push(MEET_APP.to_string());
            }
        }
    }
    out
}

/// Show or withdraw the prompt window and tell the UI.
pub fn handle_events(app: &AppHandle, events: &[Event]) {
    for e in events {
        match e {
            Event::Prompt(p) => {
                tracing::info!(app = %p.app, source = ?p.source, "call detected, asking to record");
                show_window(app);
                if let Err(err) = app.emit(EVENT_PROMPT, PromptDto::from(p)) {
                    tracing::warn!(error = %err, "could not emit the detection prompt");
                }
            }
            Event::Cleared { key } => {
                close_window(app);
                if let Err(err) = app.emit(EVENT_CLEARED, key) {
                    tracing::warn!(error = %err, "could not emit the detection clear");
                }
            }
        }
    }
}

/// A small window in the lower right corner of the primary screen that does not take the
/// focus, so typing in the application the user is in is not interrupted.
fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.show();
        return;
    }
    let built = WebviewWindowBuilder::new(
        app,
        WINDOW,
        WebviewUrl::App("index.html?view=detection".into()),
    )
    .title("Record this call?")
    .inner_size(PROMPT_SIZE.0, PROMPT_SIZE.1)
    .decorations(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .shadow(true)
    .visible(false)
    .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "could not open the detection prompt");
            return;
        }
    };
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let x = f64::from(area.position.x) + f64::from(area.size.width)
            - (PROMPT_SIZE.0 + MARGIN) * scale;
        let y = f64::from(area.position.y) + f64::from(area.size.height)
            - (PROMPT_SIZE.1 + MARGIN) * scale;
        let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
    let _ = window.show();
}

fn close_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.destroy();
    }
}

#[tauri::command]
pub fn get_detection_settings(state: State<'_, AppState>) -> IpcResult<DetectionSettings> {
    Ok(state.settings()?.detection)
}

#[tauri::command]
pub async fn save_detection_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: DetectionSettings,
) -> IpcResult<DetectionSettings> {
    let settings = settings
        .normalized()
        .map_err(|m| IpcError::new("invalid", m))?;
    state.update_settings(|s| {
        s.detection = settings.clone();
        Ok(())
    })?;
    let events = state.detection().apply(&settings);
    handle_events(&app, &events);
    Ok(settings)
}

#[tauri::command]
pub fn detection_prompt(state: State<'_, AppState>) -> Option<PromptDto> {
    state.detection().prompt()
}

/// Async on purpose: this closes and opens windows.
#[tauri::command]
pub async fn answer_detection(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    answer: AnswerKind,
) -> IpcResult<AnswerOutcome> {
    let detection = state.detection().clone();
    let Some(prompt) = detection.prompt().filter(|p| p.key == key) else {
        close_window(&app);
        return Ok(AnswerOutcome {
            started: false,
            needs_report: false,
        });
    };
    let outcome = match answer {
        AnswerKind::Record => {
            detection.answer(&key, Answer::Record);
            record(&app, &state, &detection, &prompt)?
        }
        AnswerKind::NotNow => {
            detection.answer(&key, Answer::NotNow);
            AnswerOutcome {
                started: false,
                needs_report: false,
            }
        }
        AnswerKind::Never => {
            detection.answer(&key, Answer::Never);
            let app_name = prompt.app.clone();
            state.update_settings(|s| {
                if !s.detection.opted_out.contains(&app_name) {
                    s.detection.opted_out.push(app_name);
                }
                Ok(())
            })?;
            AnswerOutcome {
                started: false,
                needs_report: false,
            }
        }
    };
    close_window(&app);
    Ok(outcome)
}

/// Record the microphone and the application of the call, with the report choice of the last
/// recording. Without a usable choice the recorder window opens (the report mode gate).
fn record(
    app: &AppHandle,
    state: &State<'_, AppState>,
    detection: &Detection,
    prompt: &PromptDto,
) -> IpcResult<AnswerOutcome> {
    let mut ids = vec!["mic:default".to_string()];
    if prompt.source == Source::Audio {
        if let Some(pid) = detection.pid_of(&prompt.key) {
            ids.push(format!("app:{pid}"));
        }
    }
    if state.recorder.is_recording() {
        // A recording runs already: add what is missing.
        for id in &ids {
            match state.recorder.add_source(id) {
                Ok(()) => {}
                Err(e) if e.code == "source_already_added" => {}
                Err(e) => return Err(e),
            }
        }
        return Ok(AnswerOutcome {
            started: true,
            needs_report: false,
        });
    }
    let req = StartRecordingRequest {
        title: None,
        source_ids: ids,
        report: None,
        use_last_report: true,
    };
    match commands::start_recording(app.clone(), app.state::<AppState>(), req) {
        Ok(()) => Ok(AnswerOutcome {
            started: true,
            needs_report: false,
        }),
        Err(e) if e.code == "report_mode_required" => {
            if let Some(w) = app.get_webview_window("widget") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            Ok(AnswerOutcome {
                started: false,
                needs_report: true,
            })
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder::detect::{Flow, Level, Sensitivity, SessionState};

    fn mic(app: &str) -> AppSession {
        AppSession {
            app: app.into(),
            pid: 42,
            flow: Flow::Capture,
            state: SessionState::Active,
            peak: Level::Low,
        }
    }

    /// Feed `secs` seconds of polls into the detection state the way the thread does.
    fn feed(d: &Detection, from: u64, secs: u64, sessions: &[AppSession]) -> Vec<Event> {
        let step = d.poll_ms();
        let mut out = Vec::new();
        let mut t = from;
        while t < from + secs * 1000 {
            out.extend(d.detector.lock().unwrap().observe(t, sessions));
            t += step;
        }
        out
    }

    #[test]
    fn a_call_makes_one_prompt_and_an_opt_out_clears_it() {
        let d = Detection::new(&DetectionSettings::default());
        let events = feed(&d, 0, 30, &[mic("discord")]);
        // Balanced needs a playback session too: none here.
        assert!(events.is_empty());
        let high = DetectionSettings {
            sensitivity: Sensitivity::High,
            ..DetectionSettings::default()
        };
        assert!(d.apply(&high).is_empty());
        let events = feed(&d, 30_000, 10, &[mic("discord")]);
        assert_eq!(events.len(), 1);
        let prompt = d.prompt().unwrap();
        assert_eq!(
            (prompt.app.as_str(), prompt.display.as_str()),
            ("discord", "Discord")
        );
        let off = DetectionSettings {
            opted_out: vec!["discord".into()],
            ..high
        };
        assert_eq!(
            d.apply(&off),
            vec![Event::Cleared {
                key: "discord".into()
            }]
        );
        assert_eq!(d.prompt(), None);
    }

    #[test]
    fn a_meet_report_goes_through_the_same_engine() {
        let d = Detection::new(&DetectionSettings {
            sensitivity: Sensitivity::High,
            ..DetectionSettings::default()
        });
        d.detector.lock().unwrap().meet(0, 9, true);
        let events = feed(&d, 0, 10, &[]);
        assert_eq!(events.len(), 1);
        assert_eq!(d.prompt().unwrap().key, "meet:9");
        d.answer("meet:9", Answer::NotNow);
        assert_eq!(d.prompt(), None);
    }

    #[test]
    fn answers_are_read_from_the_ui_names() {
        let k: AnswerKind = serde_json::from_str("\"not_now\"").unwrap();
        assert_eq!(k, AnswerKind::NotNow);
        assert!(serde_json::from_str::<AnswerKind>("\"maybe\"").is_err());
    }
}
