//! The bridge to the Chrome extension: runs the server from the settings, keeps the paired
//! extensions in the settings, records the tabs the extension streams and serves the
//! Browser extension section of the settings screen.
//!
//! The extension can only stream while a recording runs; each accepted tab becomes a track
//! of that recording (`tab:<stream id>`), fed from the bridge through [`HubTabs`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge::hub::{FeedEvent, HubObserver, StreamFeed, TabStream};
use bridge::pairing::{display_code, PairedClient};
use bridge::protocol::{Group, GroupTabStatus, MeetState, RefuseReason};
use bridge::{Hub, HubConfig, Server, ServerConfig};
use recorder::push::{PushFrame, TabFeed, TabItem, TabSource};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ipc::{IpcError, IpcResult};
use crate::settings::BridgeSettings;
use crate::state::AppState;

pub const EVENT_BRIDGE: &str = "bridge-changed";

/// A pairing code on offer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    /// The code as shown to the user, for example `7K3M-9QXD`.
    pub code: String,
    pub expires_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientView {
    pub id: String,
    pub name: String,
    pub ext_version: String,
    pub paired_unix_ms: u64,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamView {
    pub stream_id: String,
    pub title: String,
    pub host: String,
    pub track: Option<String>,
    /// The connection was lost; the stream waits for the extension to come back.
    pub waiting: bool,
}

/// Capture status of one tab of a tab group.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupTabView {
    pub tab_id: u32,
    /// `captured`, `waiting` or `failed`.
    pub status: String,
}

/// A tab group the user is capturing, with the status of each tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub group_id: i64,
    pub tabs: Vec<GroupTabView>,
}

/// Everything the Browser extension section shows. Payload of `bridge-changed`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub enabled: bool,
    pub listening: bool,
    pub port: Option<u16>,
    /// Why the server is not listening although it is enabled.
    pub error: Option<String>,
    pub pairing: Option<PairingView>,
    pub clients: Vec<ClientView>,
    pub streams: Vec<StreamView>,
    pub groups: Vec<GroupView>,
}

/// What the bridge needs from the rest of the application.
pub trait BrowserHost: Send + Sync {
    /// Store the paired extensions in the settings.
    fn store_clients(&self, clients: Vec<PairedClient>);
    /// Add a tab to the running recording; returns the track name.
    fn add_tab(&self, stream_id: &str, label: &str) -> IpcResult<String>;
    /// The bridge status changed.
    fn changed(&self);
    /// A Google Meet tab was joined or left (the meeting code is never sent).
    fn meet(&self, _tab_id: u32, _joined: bool) {}
}

/// The hub's view of the application.
struct Link {
    host: Arc<dyn BrowserHost>,
    auto_add: Arc<AtomicBool>,
    groups: Arc<Mutex<BTreeMap<i64, Vec<GroupTabView>>>>,
}

/// Label of a tab track in the source list.
fn tab_label(s: &TabStream) -> String {
    let host = s.host.trim();
    if host.is_empty() {
        let title: String = s.title.trim().chars().take(60).collect();
        if title.is_empty() {
            "Browser tab".into()
        } else {
            format!("Tab: {title}")
        }
    } else {
        format!("Tab: {host}")
    }
}

impl HubObserver for Link {
    fn clients_changed(&self, clients: &[PairedClient]) {
        self.host.store_clients(clients.to_vec());
    }

    fn status_changed(&self) {
        self.host.changed();
    }

    fn add_tab(&self, stream: &TabStream) -> Result<String, RefuseReason> {
        if !self.auto_add.load(Ordering::Relaxed) {
            return Err(RefuseReason::Declined);
        }
        self.host
            .add_tab(&stream.stream_id, &tab_label(stream))
            .map_err(|e| {
                tracing::info!(stream = %stream.stream_id, error = %e, "tab not recorded");
                match e.code {
                    "not_recording" => RefuseReason::NotRecording,
                    "source_already_added" => RefuseReason::Duplicate,
                    _ => RefuseReason::Declined,
                }
            })
    }

    fn meet(&self, meet: &bridge::protocol::Meet) {
        tracing::info!(tab = meet.tab_id, state = ?meet.state, "Google Meet tab");
        self.host.meet(meet.tab_id, meet.state == MeetState::Joined);
    }

    fn group(&self, group: &Group) {
        if let Ok(mut groups) = self.groups.lock() {
            if group.tabs.is_empty() {
                groups.remove(&group.group_id);
            } else {
                groups.insert(
                    group.group_id,
                    group
                        .tabs
                        .iter()
                        .map(|t| GroupTabView {
                            tab_id: t.tab_id,
                            status: match t.status {
                                GroupTabStatus::Captured => "captured",
                                GroupTabStatus::Waiting => "waiting",
                                GroupTabStatus::Failed => "failed",
                            }
                            .to_string(),
                        })
                        .collect(),
                );
            }
        }
        self.host.changed();
    }
}

/// Feeds tab tracks from the bridge.
pub struct HubTabs(Arc<Hub>);

struct HubFeed(StreamFeed);

impl TabFeed for HubFeed {
    fn next(&mut self, timeout: Duration) -> TabItem {
        match self.0.next(timeout) {
            FeedEvent::Frame(f) => TabItem::Frame(PushFrame {
                seq: f.header.seq,
                sent_unix_ms: f.header.sent_ms,
                samples: f.samples,
            }),
            FeedEvent::Idle => TabItem::Idle,
            FeedEvent::End(r) => TabItem::End(r.as_str().to_string()),
        }
    }
}

impl TabSource for HubTabs {
    fn attach(&self, stream: &str) -> Result<Box<dyn TabFeed>, String> {
        self.0
            .attach(stream)
            .map(|f| Box::new(HubFeed(f)) as Box<dyn TabFeed>)
    }

    fn detach(&self, stream: &str) {
        self.0.detach(stream);
    }
}

#[derive(Default)]
struct Running {
    server: Option<Server>,
    error: Option<String>,
    /// The settings the server was started with.
    applied: Option<(bool, Vec<u16>, Vec<String>)>,
}

/// The bridge inside the application.
pub struct Browser {
    hub: Arc<Hub>,
    auto_add: Arc<AtomicBool>,
    groups: Arc<Mutex<BTreeMap<i64, Vec<GroupTabView>>>>,
    enabled: AtomicBool,
    running: Mutex<Running>,
}

fn unix_ms_at(at: Instant, now: Instant) -> u64 {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_unix + at.saturating_duration_since(now).as_millis() as u64
}

impl Browser {
    /// Create the bridge with the stored clients. The server starts with [`Browser::apply`].
    pub fn new(settings: &BridgeSettings, host: Arc<dyn BrowserHost>) -> Arc<Self> {
        let auto_add = Arc::new(AtomicBool::new(settings.auto_add_tabs));
        let groups = Arc::new(Mutex::new(BTreeMap::new()));
        let link = Arc::new(Link {
            host,
            auto_add: auto_add.clone(),
            groups: groups.clone(),
        });
        let hub = Hub::new(
            HubConfig {
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                ..HubConfig::default()
            },
            settings.clients.clone(),
            link,
        );
        Arc::new(Self {
            hub,
            auto_add,
            groups,
            enabled: AtomicBool::new(settings.enabled),
            running: Mutex::new(Running::default()),
        })
    }

    #[cfg(test)]
    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// The tab source for the recorder's routing backend.
    pub fn tab_source(&self) -> Arc<dyn TabSource> {
        Arc::new(HubTabs(self.hub.clone()))
    }

    /// Start, restart or stop the server to match the settings.
    pub fn apply(&self, settings: &BridgeSettings) {
        self.auto_add
            .store(settings.auto_add_tabs, Ordering::Relaxed);
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        let wanted = (
            settings.enabled,
            settings.ports(),
            settings.allowed_extension_ids.clone(),
        );
        let old = {
            let Ok(mut r) = self.running.lock() else {
                return;
            };
            if r.applied.as_ref() == Some(&wanted) && (r.server.is_some() || !settings.enabled) {
                return;
            }
            r.applied = Some(wanted);
            r.error = None;
            r.server.take()
        };
        // Stop the old server outside the lock: its connections report status changes.
        if let Some(s) = old {
            s.shutdown();
        }
        if settings.enabled {
            let started = Server::start(
                ServerConfig {
                    ports: settings.ports(),
                    allowed_extension_ids: settings.allowed_extension_ids.clone(),
                    ..ServerConfig::default()
                },
                self.hub.clone(),
            );
            if let Ok(mut r) = self.running.lock() {
                match started {
                    Ok(s) => r.server = Some(s),
                    Err(e) => {
                        tracing::warn!(error = %e, "bridge not started");
                        r.error = Some(format!("No free port for the browser extension: {e}"));
                    }
                }
            }
        }
    }

    /// Tell the bridge which meeting is being recorded.
    pub fn sync_recording(&self, meeting_id: Option<String>) {
        self.hub.set_recording(meeting_id);
    }

    pub fn status(&self) -> BridgeStatus {
        let now = Instant::now();
        let hub = self.hub.status(now);
        let (port, error) = self
            .running
            .lock()
            .map(|r| (r.server.as_ref().map(Server::port), r.error.clone()))
            .unwrap_or((None, None));
        BridgeStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            listening: port.is_some(),
            port,
            error,
            pairing: hub.pairing.map(|(code, exp)| PairingView {
                code: display_code(&code),
                expires_unix_ms: unix_ms_at(exp, now),
            }),
            clients: hub
                .clients
                .into_iter()
                .map(|c| ClientView {
                    id: c.client.id,
                    name: c.client.name,
                    ext_version: c.client.ext_version,
                    paired_unix_ms: c.client.paired_unix_ms,
                    connected: c.connected,
                })
                .collect(),
            streams: hub
                .streams
                .into_iter()
                .map(|s| StreamView {
                    stream_id: s.stream_id,
                    title: s.title,
                    host: s.host,
                    track: s.track,
                    waiting: s.waiting,
                })
                .collect(),
            groups: self
                .groups
                .lock()
                .map(|g| {
                    g.iter()
                        .map(|(id, tabs)| GroupView {
                            group_id: *id,
                            tabs: tabs.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn start_pairing(&self) -> IpcResult<BridgeStatus> {
        let listening = self
            .running
            .lock()
            .map(|r| r.server.is_some())
            .unwrap_or(false);
        if !listening {
            return Err(IpcError::new(
                "bridge_off",
                "The browser extension bridge is not running. Turn it on in the settings.",
            ));
        }
        self.hub
            .start_pairing(Instant::now())
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        Ok(self.status())
    }

    pub fn cancel_pairing(&self) -> BridgeStatus {
        self.hub.cancel_pairing();
        self.status()
    }

    pub fn unpair(&self, client_id: &str) -> IpcResult<BridgeStatus> {
        if !self.hub.unpair(client_id) {
            return Err(IpcError::new("not_found", "This extension is not paired"));
        }
        Ok(self.status())
    }
}

/// [`BrowserHost`] of the running application.
#[derive(Default)]
pub struct AppHost {
    app: OnceLock<AppHandle>,
}

impl AppHost {
    /// Connect to the application once it is set up.
    pub fn bind(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }
}

impl BrowserHost for AppHost {
    fn store_clients(&self, clients: Vec<PairedClient>) {
        let Some(app) = self.app.get() else { return };
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if let Err(e) = state.update_settings(|s| {
            s.bridge.clients = clients;
            Ok(())
        }) {
            tracing::error!(error = %e, "could not store the paired extensions");
        }
    }

    fn add_tab(&self, stream_id: &str, label: &str) -> IpcResult<String> {
        let app = self
            .app
            .get()
            .ok_or_else(|| IpcError::new("not_recording", "The application is starting"))?;
        let state = app
            .try_state::<AppState>()
            .ok_or_else(|| IpcError::new("not_recording", "The application is starting"))?;
        let track = state.recorder.add_tab(stream_id, label)?;
        if let Some(s) = state.recorder.tick() {
            crate::commands::emit_state(app, &s);
        }
        Ok(track)
    }

    fn changed(&self) {
        let Some(app) = self.app.get() else { return };
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if let Err(e) = app.emit(EVENT_BRIDGE, state.browser().status()) {
            tracing::warn!(error = %e, "could not emit the bridge status");
        }
    }

    fn meet(&self, tab_id: u32, joined: bool) {
        let Some(app) = self.app.get() else { return };
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        state.detection().meet(u64::from(tab_id), joined);
    }
}

#[tauri::command]
pub fn bridge_status(state: State<'_, AppState>) -> BridgeStatus {
    state.browser().status()
}

#[tauri::command]
pub fn start_pairing(state: State<'_, AppState>) -> IpcResult<BridgeStatus> {
    state.browser().start_pairing()
}

#[tauri::command]
pub fn cancel_pairing(state: State<'_, AppState>) -> BridgeStatus {
    state.browser().cancel_pairing()
}

#[tauri::command]
pub fn unpair_client(state: State<'_, AppState>, client_id: String) -> IpcResult<BridgeStatus> {
    state.browser().unpair(&client_id)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use bridge::protocol::{
        Frame, FrameHeader, Hello, ServerMessage, Start, Stop, StopReason, PROTOCOL_VERSION,
    };
    use recorder::journal::{replay_file, DiscKind, JOURNAL_FILE_NAME};
    use recorder::session::{RoutingBackend, ToneBackend};
    use store::{ReportSettings, Store};

    use super::*;
    use crate::recording::Recorder;
    use crate::recording_dto::StartRecordingRequest;

    #[derive(Default)]
    struct TestHost {
        recorder: OnceLock<Arc<Recorder>>,
        clients: Mutex<Vec<PairedClient>>,
    }

    impl BrowserHost for TestHost {
        fn store_clients(&self, clients: Vec<PairedClient>) {
            *self.clients.lock().unwrap() = clients;
        }
        fn add_tab(&self, stream_id: &str, label: &str) -> IpcResult<String> {
            self.recorder.get().unwrap().add_tab(stream_id, label)
        }
        fn changed(&self) {}
    }

    fn start(id: &str, host: &str) -> Start {
        Start {
            stream_id: id.into(),
            tab_id: 9,
            group_id: None,
            title: "Weekly sync".into(),
            host: host.into(),
            sample_rate: 16_000,
            channels: 1,
            format: "s16le".into(),
            frame_samples: 320,
        }
    }

    /// Pair a fake connection directly on the hub (the socket side is tested in the bridge).
    fn connect(hub: &Hub) -> u64 {
        let now = Instant::now();
        hub.start_pairing_with("7K3M9QXD", now);
        let (tx, _) = mpsc::channel();
        let conn = hub.connect(tx).unwrap();
        hub.hello(
            conn,
            &Hello {
                version: PROTOCOL_VERSION,
                client: "Chrome 136".into(),
                ext_version: "0.6.0".into(),
                pairing_code: Some("7K3M9QXD".into()),
                client_id: None,
                secret: None,
            },
            now,
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_streamed_tab_is_recorded_as_a_track_with_its_gaps_journaled() {
        let host = Arc::new(TestHost::default());
        let browser = Browser::new(&BridgeSettings::default(), host.clone());
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let backend = Arc::new(RoutingBackend::new(
            Arc::new(ToneBackend),
            browser.tab_source(),
        ));
        let recorder = Arc::new(Recorder::new(store.clone(), backend, true));
        let _ = host.recorder.set(recorder.clone());
        let root = testkit::target_tempdir("app-browser");

        let hub = browser.hub().clone();
        let conn = connect(&hub);
        assert_eq!(host.clients.lock().unwrap().len(), 1);
        // Idle: refused.
        assert!(matches!(
            hub.start(conn, &start("s1", "meet.google.com")),
            ServerMessage::StartRefused(_)
        ));

        let req = StartRecordingRequest {
            title: None,
            source_ids: vec!["tone:220".into()],
            report: None,
            use_last_report: false,
        };
        recorder
            .start(root.path(), req, ReportSettings::default())
            .unwrap();
        browser.sync_recording(recorder.meeting_id());
        let ServerMessage::StartOk(ok) = hub.start(conn, &start("s1", "meet.google.com")) else {
            panic!("tab refused");
        };
        assert_eq!(ok.track, "tab");
        let t0 = 1_794_579_901_000.0;
        for seq in (0..10u32).chain(13..40) {
            hub.frame(
                conn,
                Frame {
                    header: FrameHeader {
                        stream_no: ok.stream_no,
                        seq,
                        sent_ms: t0 + f64::from(seq) * 20.0,
                    },
                    samples: vec![1_000; 320],
                },
            );
        }
        let status = browser.status();
        assert_eq!(status.streams.len(), 1);
        assert_eq!(status.streams[0].track.as_deref(), Some("tab"));
        let live = recorder.tick().unwrap();
        let src = live.sources.iter().find(|s| s.id == "tab:s1").unwrap();
        assert_eq!(src.label, "Tab: meet.google.com");

        hub.stop(
            conn,
            &Stop {
                stream_id: "s1".into(),
                reason: StopReason::TabClosed,
                frames_sent: 40,
                samples_sent: 12_800,
                frames_dropped: 3,
            },
        );
        // Wait for the track to read the frames and the end, not for a fixed time.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let live = recorder.tick().unwrap();
            let src = live.sources.iter().find(|s| s.id == "tab:s1").unwrap();
            if src.status == crate::recording_dto::SourceStatus::Ended {
                break;
            }
            assert!(Instant::now() < deadline, "the tab track did not end");
            std::thread::sleep(Duration::from_millis(20));
        }
        let id = recorder.stop().unwrap();
        browser.sync_recording(recorder.meeting_id());

        let details = store.lock().unwrap().get_meeting(&id).unwrap().unwrap();
        let track = details.tracks.iter().find(|t| t.name == "tab").unwrap();
        assert_eq!(track.source, "browser-tab");
        assert!(track.ended);
        let journal = replay_file(&root.path().join(&id).join(JOURNAL_FILE_NAME)).unwrap();
        let gaps: Vec<u64> = journal.tracks["tab"]
            .discontinuities
            .iter()
            .filter(|d| d.kind == DiscKind::StreamGap)
            .map(|d| d.frames)
            .collect();
        assert_eq!(gaps, vec![3 * 320]);
        assert!(browser.status().streams.is_empty());
    }

    #[test]
    fn tabs_are_declined_when_auto_add_is_off() {
        let host = Arc::new(TestHost::default());
        let settings = BridgeSettings {
            auto_add_tabs: false,
            ..BridgeSettings::default()
        };
        let browser = Browser::new(&settings, host);
        let hub = browser.hub().clone();
        let conn = connect(&hub);
        browser.sync_recording(Some("m".into()));
        assert!(matches!(
            hub.start(conn, &start("s1", "example.org")),
            ServerMessage::StartRefused(r) if r.reason == RefuseReason::Declined
        ));
    }

    #[test]
    fn pairing_needs_a_running_server_and_shows_the_code() {
        let browser = Browser::new(&BridgeSettings::default(), Arc::new(TestHost::default()));
        assert_eq!(browser.start_pairing().unwrap_err().code, "bridge_off");
        // Port 0: any free port.
        browser.apply(&BridgeSettings {
            port: 0,
            port_fallbacks: Vec::new(),
            ..BridgeSettings::default()
        });
        let status = browser.start_pairing().unwrap();
        assert!(status.listening && status.port.is_some_and(|p| p > 0));
        let pairing = status.pairing.unwrap();
        assert_eq!(pairing.code.len(), 9);
        assert_eq!(&pairing.code[4..5], "-");
        assert!(browser.cancel_pairing().pairing.is_none());
        assert_eq!(browser.unpair("c-none").unwrap_err().code, "not_found");
        // Turning the bridge off stops the server.
        browser.apply(&BridgeSettings {
            enabled: false,
            port: 0,
            port_fallbacks: Vec::new(),
            ..BridgeSettings::default()
        });
        let off = browser.status();
        assert!(!off.enabled && !off.listening);
    }

    #[test]
    fn tab_labels_prefer_the_host() {
        let mut s = TabStream {
            stream_id: "s".into(),
            tab_id: 1,
            group_id: None,
            title: "Quarterly review".into(),
            host: "meet.google.com".into(),
        };
        assert_eq!(tab_label(&s), "Tab: meet.google.com");
        s.host.clear();
        assert_eq!(tab_label(&s), "Tab: Quarterly review");
        s.title.clear();
        assert_eq!(tab_label(&s), "Browser tab");
    }
}
