//! Shared state of the bridge: paired clients, open connections and tab streams.
//!
//! Connection threads report what the extension sends; the application asks for pairing
//! codes, tells whether a recording runs and takes the audio of each stream with
//! [`Hub::attach`]. Calls into the [`HubObserver`] are made without holding the hub lock, so
//! the observer may call back into the hub.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::pairing::{
    hash_secret, new_client_id, new_secret, secret_matches, ClaimError, PairedClient, Pairing,
    RandomError,
};
use crate::protocol::{
    ErrorCode, Frame, Group, Hello, HelloOk, Meet, RefuseReason, Resume, ServerMessage, Start,
    StartOk, StartRefused, State, Stop, StopReason, StreamPhase, StreamState, MAX_TITLE_CHARS,
    PROTOCOL_VERSION,
};

/// Id of one WebSocket connection.
pub type ConnId = u64;

/// Frames buffered per stream until the recorder takes them (30 s at 20 ms per frame).
const STREAM_QUEUE_FRAMES: usize = 1_500;

/// What a connection thread should send.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Message(ServerMessage),
    /// Send the message, then close the connection.
    Close(ServerMessage),
}

/// Settings of the hub.
#[derive(Debug, Clone)]
pub struct HubConfig {
    pub app_version: String,
    pub heartbeat_ms: u32,
    /// Connections allowed at the same time; more are refused with `busy`.
    pub max_connections: usize,
    /// How long a stream whose connection was lost waits for `resume`.
    pub resume_grace: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            app_version: String::new(),
            heartbeat_ms: crate::protocol::HEARTBEAT_MS,
            max_connections: 2,
            resume_grace: Duration::from_secs(30),
        }
    }
}

/// A tab the extension wants recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct TabStream {
    pub stream_id: String,
    pub tab_id: u32,
    pub group_id: Option<i64>,
    pub title: String,
    pub host: String,
}

/// Why a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The extension stopped it.
    Stopped(StopReason),
    /// The connection was lost and the stream was not resumed in time.
    ConnectionLost,
    /// The recording stopped, or the client was unpaired.
    Closed,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped(r) => r.as_str(),
            Self::ConnectionLost => "connection_lost",
            Self::Closed => "closed",
        }
    }
}

/// What a stream feed delivers.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedEvent {
    Frame(Frame),
    /// Nothing arrived within the timeout.
    Idle,
    /// The stream ended; no more frames follow.
    End(EndReason),
}

/// The audio of one stream, taken once by the recorder.
pub struct StreamFeed {
    rx: Receiver<Frame>,
    end: Arc<Mutex<Option<EndReason>>>,
}

impl StreamFeed {
    /// The next frame, or `Idle` after `timeout`, or `End` once the stream is over and every
    /// frame sent before has been delivered.
    pub fn next(&mut self, timeout: Duration) -> FeedEvent {
        match self.rx.recv_timeout(timeout) {
            Ok(f) => FeedEvent::Frame(f),
            Err(RecvTimeoutError::Timeout) => FeedEvent::Idle,
            Err(RecvTimeoutError::Disconnected) => FeedEvent::End(
                self.end
                    .lock()
                    .ok()
                    .and_then(|e| *e)
                    .unwrap_or(EndReason::Closed),
            ),
        }
    }
}

/// The application side of the hub.
pub trait HubObserver: Send + Sync {
    /// The paired clients changed (pairing or unpairing); store the list.
    fn clients_changed(&self, clients: &[PairedClient]);
    /// Something shown in the status changed (connections, pairing code, streams).
    fn status_changed(&self);
    /// A tab asks to be recorded. Returns the track name, or why it is refused. The track
    /// must take the stream's audio with [`Hub::attach`].
    fn add_tab(&self, stream: &TabStream) -> Result<String, RefuseReason>;
    /// A Google Meet tab changed.
    fn meet(&self, _meet: &Meet) {}
    /// The capture status of a tab group changed.
    fn group(&self, _group: &Group) {}
}

/// Paired client with its connection state.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientStatus {
    pub client: PairedClient,
    pub connected: bool,
}

/// A stream as shown in the status.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamStatus {
    pub stream_id: String,
    pub title: String,
    pub host: String,
    pub track: Option<String>,
    pub waiting: bool,
    pub frames: u64,
    pub dropped_frames: u64,
}

/// Everything the application shows about the bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct HubStatus {
    /// The pairing code on offer and when it expires.
    pub pairing: Option<(String, Instant)>,
    pub clients: Vec<ClientStatus>,
    pub streams: Vec<StreamStatus>,
    /// Tabs the extension has captured and waits to record: refused because nothing was
    /// recording, or declined by the application. They are offered again when a recording
    /// starts.
    pub offered: Vec<TabStream>,
    pub connections: usize,
    pub recording: bool,
}

struct ConnEntry {
    client_id: Option<String>,
    tx: Sender<Outgoing>,
}

/// A tab the application did not take (yet).
struct Offer {
    info: TabStream,
    client_id: String,
    conn: ConnId,
}

struct Slot {
    info: TabStream,
    stream_no: u32,
    client_id: String,
    conn: Option<ConnId>,
    /// None until the application accepted the stream.
    track: Option<String>,
    tx: SyncSender<Frame>,
    rx: Option<Receiver<Frame>>,
    end: Arc<Mutex<Option<EndReason>>>,
    detached_since: Option<Instant>,
    frames: u64,
    dropped: u64,
}

impl Slot {
    fn finish(self, reason: EndReason) {
        if let Ok(mut e) = self.end.lock() {
            e.get_or_insert(reason);
        }
        // Dropping the sender ends the feed once the queued frames are read.
    }

    /// Like [`Slot::finish`], but hands back the feed when the recorder has not attached to
    /// it yet, so that it can still read the queued frames and the end.
    fn retire(mut self, reason: EndReason) -> Option<StreamFeed> {
        let feed = self.rx.take().map(|rx| StreamFeed {
            rx,
            end: self.end.clone(),
        });
        self.finish(reason);
        feed
    }
}

#[derive(Default)]
struct Inner {
    pairing: Pairing,
    pairing_stale_reported: bool,
    clients: Vec<PairedClient>,
    conns: HashMap<ConnId, ConnEntry>,
    streams: HashMap<String, Slot>,
    /// Tabs that asked to be recorded and were refused; see [`HubStatus::offered`].
    offered: HashMap<String, Offer>,
    /// Streams that ended before the recorder attached to them. The recording thread starts
    /// a moment after the stream is accepted; a tab that stops in that moment must still be
    /// recorded, with the frames it sent.
    parked: HashMap<String, StreamFeed>,
    by_no: HashMap<u32, String>,
    next_conn: ConnId,
    next_stream_no: u32,
    recording: Option<String>,
}

/// Effects to run after the lock is released.
#[derive(Default)]
struct After {
    clients: Option<Vec<PairedClient>>,
    status: bool,
    state: bool,
}

/// The bridge state shared by the server threads and the application.
pub struct Hub {
    config: HubConfig,
    inner: Mutex<Inner>,
    observer: Arc<dyn HubObserver>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn error(code: ErrorCode, message: impl Into<String>) -> ServerMessage {
    ServerMessage::Error(crate::protocol::ErrorMessage {
        code,
        message: message.into(),
    })
}

impl Inner {
    /// Take a stream out of the hub and end it. A feed nobody attached to waits for the
    /// recorder in `parked`.
    fn retire(&mut self, id: &str, slot: Slot, reason: EndReason) {
        self.by_no.remove(&slot.stream_no);
        if let Some(feed) = slot.retire(reason) {
            self.parked.insert(id.to_string(), feed);
        }
    }
}

impl Hub {
    pub fn new(
        config: HubConfig,
        clients: Vec<PairedClient>,
        observer: Arc<dyn HubObserver>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: Mutex::new(Inner {
                clients,
                next_conn: 1,
                next_stream_no: 1,
                ..Inner::default()
            }),
            observer,
        })
    }

    pub fn config(&self) -> &HubConfig {
        &self.config
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data behind; keep serving.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self, after: After) {
        if let Some(clients) = &after.clients {
            self.observer.clients_changed(clients);
        }
        if after.state {
            self.broadcast_state();
        }
        if after.status || after.state || after.clients.is_some() {
            self.observer.status_changed();
        }
    }

    fn state_message(inner: &Inner) -> ServerMessage {
        let mut streams: Vec<StreamState> = inner
            .streams
            .iter()
            .filter_map(|(id, s)| {
                s.track.as_ref().map(|track| StreamState {
                    stream_id: id.clone(),
                    track: track.clone(),
                    status: if s.detached_since.is_some() {
                        StreamPhase::Waiting
                    } else {
                        StreamPhase::Recording
                    },
                })
            })
            .collect();
        streams.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
        ServerMessage::State(State {
            recording: inner.recording.is_some(),
            meeting_id: inner.recording.clone(),
            streams,
        })
    }

    /// The current state message (sent by a connection right after `hello_ok`).
    pub fn state(&self) -> ServerMessage {
        Self::state_message(&self.lock())
    }

    fn broadcast_state(&self) {
        let inner = self.lock();
        let msg = Self::state_message(&inner);
        for c in inner.conns.values().filter(|c| c.client_id.is_some()) {
            let _ = c.tx.send(Outgoing::Message(msg.clone()));
        }
    }

    // ----- connections -------------------------------------------------------------------

    /// A new connection finished its WebSocket handshake. Refused with `busy` when too many
    /// are open.
    pub fn connect(&self, tx: Sender<Outgoing>) -> Result<ConnId, ServerMessage> {
        let id = {
            let mut inner = self.lock();
            if inner.conns.len() >= self.config.max_connections {
                return Err(error(
                    ErrorCode::Busy,
                    "Too many connections; close other browser profiles using the extension.",
                ));
            }
            let id = inner.next_conn;
            inner.next_conn += 1;
            inner.conns.insert(
                id,
                ConnEntry {
                    client_id: None,
                    tx,
                },
            );
            id
        };
        self.run(After {
            status: true,
            ..After::default()
        });
        Ok(id)
    }

    /// A connection closed. Its streams wait for `resume` on a new connection.
    pub fn disconnect(&self, conn: ConnId, now: Instant) {
        let mut after = After {
            status: true,
            ..After::default()
        };
        {
            let mut inner = self.lock();
            inner.conns.remove(&conn);
            // The extension offers its waiting tabs again after it reconnects.
            inner.offered.retain(|_, o| o.conn != conn);
            for s in inner.streams.values_mut() {
                if s.conn == Some(conn) {
                    s.conn = None;
                    s.detached_since = Some(now);
                    after.state = true;
                }
            }
        }
        self.run(after);
    }

    /// The `hello` of a connection. Returns `hello_ok`, or the error to send before closing.
    pub fn hello(
        &self,
        conn: ConnId,
        hello: &Hello,
        now: Instant,
    ) -> Result<HelloOk, ServerMessage> {
        if hello.version != PROTOCOL_VERSION {
            return Err(error(
                ErrorCode::BadVersion,
                format!(
                    "Protocol version {} is not supported; this application speaks version {PROTOCOL_VERSION}.",
                    hello.version
                ),
            ));
        }
        let mut after = After {
            status: true,
            ..After::default()
        };
        let ok = {
            let mut inner = self.lock();
            let (client_id, secret) = if let Some(code) = &hello.pairing_code {
                match inner.pairing.claim(code, now) {
                    Ok(()) => {}
                    Err(ClaimError::BadCode) => {
                        return Err(error(
                            ErrorCode::BadCode,
                            "The pairing code is not correct.",
                        ))
                    }
                    Err(ClaimError::Expired) => {
                        return Err(error(
                            ErrorCode::CodeExpired,
                            "The pairing code expired. Start pairing again in the application.",
                        ))
                    }
                }
                let (id, secret) = new_identity().map_err(|e| {
                    tracing::error!(error = %e, "pairing failed");
                    error(
                        ErrorCode::BadCode,
                        "The application could not create a secret.",
                    )
                })?;
                inner.clients.push(PairedClient {
                    id: id.clone(),
                    name: hello.client.chars().take(80).collect(),
                    ext_version: hello.ext_version.clone(),
                    secret_sha256: hash_secret(&secret),
                    paired_unix_ms: unix_ms(),
                });
                after.clients = Some(inner.clients.clone());
                tracing::info!(client = %id, "extension paired");
                (id, Some(secret))
            } else {
                let id = hello.client_id.clone().unwrap_or_default();
                let secret = hello.secret.clone().unwrap_or_default();
                let known = inner
                    .clients
                    .iter_mut()
                    .find(|c| c.id == id && secret_matches(&secret, &c.secret_sha256));
                let Some(client) = known else {
                    return Err(error(
                        ErrorCode::Unpaired,
                        "This extension is not paired with the application. Pair it again.",
                    ));
                };
                if client.ext_version != hello.ext_version {
                    client.ext_version = hello.ext_version.clone();
                    after.clients = Some(inner.clients.clone());
                }
                (id, None)
            };
            let Some(entry) = inner.conns.get_mut(&conn) else {
                return Err(error(ErrorCode::BadMessage, "Connection closed."));
            };
            entry.client_id = Some(client_id.clone());
            HelloOk {
                client_id,
                secret,
                session_id: format!("s-{conn}"),
                app_version: self.config.app_version.clone(),
                recording: inner.recording.is_some(),
                heartbeat_ms: self.config.heartbeat_ms,
            }
        };
        self.run(after);
        Ok(ok)
    }

    fn client_of(inner: &Inner, conn: ConnId) -> Option<String> {
        inner.conns.get(&conn).and_then(|c| c.client_id.clone())
    }

    // ----- streams -----------------------------------------------------------------------

    /// A tab asks to stream. The observer decides; the answer is `start_ok` or
    /// `start_refused`.
    pub fn start(&self, conn: ConnId, start: &Start) -> ServerMessage {
        let refused = |reason| {
            ServerMessage::StartRefused(StartRefused {
                stream_id: start.stream_id.clone(),
                reason,
            })
        };
        let info = TabStream {
            stream_id: start.stream_id.clone(),
            tab_id: start.tab_id,
            group_id: start.group_id,
            title: start.title.chars().take(MAX_TITLE_CHARS).collect(),
            host: start.host.clone(),
        };
        let (stream_no, client_id) = {
            let mut inner = self.lock();
            let Some(client_id) = Self::client_of(&inner, conn) else {
                return refused(RefuseReason::Declined);
            };
            if inner.streams.contains_key(&start.stream_id) {
                return refused(RefuseReason::Duplicate);
            }
            inner.parked.remove(&start.stream_id);
            if inner.recording.is_none() {
                inner.offered.insert(
                    start.stream_id.clone(),
                    Offer {
                        info,
                        client_id,
                        conn,
                    },
                );
                drop(inner);
                self.run(After {
                    status: true,
                    ..After::default()
                });
                return refused(RefuseReason::NotRecording);
            }
            let stream_no = inner.next_stream_no;
            inner.next_stream_no = inner.next_stream_no.wrapping_add(1).max(1);
            let (tx, rx) = mpsc::sync_channel(STREAM_QUEUE_FRAMES);
            inner.by_no.insert(stream_no, start.stream_id.clone());
            inner.offered.remove(&start.stream_id);
            inner.streams.insert(
                start.stream_id.clone(),
                Slot {
                    info: info.clone(),
                    stream_no,
                    client_id: client_id.clone(),
                    conn: Some(conn),
                    track: None,
                    tx,
                    rx: Some(rx),
                    end: Arc::new(Mutex::new(None)),
                    detached_since: None,
                    frames: 0,
                    dropped: 0,
                },
            );
            (stream_no, client_id)
        };
        // The observer adds the track; it may call `attach` from another thread meanwhile.
        let decision = self.observer.add_tab(&info);
        let accepted = decision.is_ok();
        let reply = {
            let mut inner = self.lock();
            if let Err(reason) = &decision {
                // Declined for now: the tab stays on offer (for the picker) and can be
                // taken later.
                if *reason != RefuseReason::Duplicate {
                    inner.offered.insert(
                        start.stream_id.clone(),
                        Offer {
                            info: info.clone(),
                            client_id: client_id.clone(),
                            conn,
                        },
                    );
                }
            }
            match (decision, inner.streams.get_mut(&start.stream_id)) {
                (Ok(track), Some(slot)) => {
                    slot.track = Some(track.clone());
                    tracing::info!(stream = %start.stream_id, stream_no, %track, "tab stream started");
                    ServerMessage::StartOk(StartOk {
                        stream_id: start.stream_id.clone(),
                        stream_no,
                        track,
                    })
                }
                (Ok(_), None) => refused(RefuseReason::NotRecording),
                (Err(reason), _) => {
                    inner.by_no.remove(&stream_no);
                    if let Some(slot) = inner.streams.remove(&start.stream_id) {
                        slot.finish(EndReason::Closed);
                    }
                    refused(reason)
                }
            }
        };
        // A refusal changes no state the extension needs: answering it with a state message
        // would make a waiting tab ask again at once.
        self.run(After {
            state: accepted,
            status: !accepted,
            ..After::default()
        });
        reply
    }

    /// Continue a stream on a new connection of the same client.
    pub fn resume(&self, conn: ConnId, resume: &Resume) -> ServerMessage {
        let reply = {
            let mut inner = self.lock();
            let client = Self::client_of(&inner, conn);
            match inner.streams.get_mut(&resume.stream_id) {
                Some(slot) if slot.track.is_some() && client.as_ref() == Some(&slot.client_id) => {
                    slot.conn = Some(conn);
                    slot.detached_since = None;
                    tracing::info!(stream = %resume.stream_id, next_seq = resume.next_seq, "tab stream resumed");
                    ServerMessage::StartOk(StartOk {
                        stream_id: resume.stream_id.clone(),
                        stream_no: slot.stream_no,
                        track: slot.track.clone().unwrap_or_default(),
                    })
                }
                _ => ServerMessage::StartRefused(StartRefused {
                    stream_id: resume.stream_id.clone(),
                    reason: RefuseReason::UnknownStream,
                }),
            }
        };
        self.run(After {
            state: true,
            ..After::default()
        });
        reply
    }

    /// The extension stopped a stream.
    pub fn stop(&self, conn: ConnId, stop: &Stop) {
        let mut offer_gone = false;
        let removed = {
            let mut inner = self.lock();
            if inner
                .offered
                .get(&stop.stream_id)
                .is_some_and(|o| o.conn == conn)
            {
                inner.offered.remove(&stop.stream_id);
                offer_gone = true;
            }
            let owned = inner
                .streams
                .get(&stop.stream_id)
                .is_some_and(|s| s.conn == Some(conn));
            if owned {
                let slot = inner.streams.remove(&stop.stream_id);
                if let Some(slot) = slot {
                    let (frames, reason) = (slot.frames, EndReason::Stopped(stop.reason));
                    inner.retire(&stop.stream_id, slot, reason);
                    Some(frames)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(frames_received) = removed {
            tracing::info!(
                stream = %stop.stream_id,
                reason = stop.reason.as_str(),
                frames_sent = stop.frames_sent,
                frames_received,
                frames_dropped_by_sender = stop.frames_dropped,
                "tab stream stopped"
            );
            self.run(After {
                state: true,
                ..After::default()
            });
        } else if offer_gone {
            self.run(After {
                status: true,
                ..After::default()
            });
        }
    }

    /// An audio frame arrived on `conn`. Frames of unknown streams or of streams owned by
    /// another connection are dropped.
    pub fn frame(&self, conn: ConnId, frame: Frame) {
        let mut inner = self.lock();
        let Some(id) = inner.by_no.get(&frame.header.stream_no).cloned() else {
            return;
        };
        let Some(slot) = inner.streams.get_mut(&id) else {
            return;
        };
        if slot.conn != Some(conn) || slot.track.is_none() {
            return;
        }
        slot.frames += 1;
        match slot.tx.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                if slot.dropped == 0 {
                    tracing::warn!(stream = %id, "recorder does not keep up; dropping frames");
                }
                slot.dropped += 1;
            }
            Err(TrySendError::Disconnected(_)) => slot.dropped += 1,
        }
    }

    pub fn meet(&self, meet: &Meet) {
        self.observer.meet(meet);
    }

    pub fn group(&self, group: &Group) {
        self.observer.group(group);
    }

    // ----- application side --------------------------------------------------------------

    /// Take the audio of a stream. Works once per stream.
    pub fn attach(&self, stream_id: &str) -> Result<StreamFeed, String> {
        let mut inner = self.lock();
        if !inner.streams.contains_key(stream_id) {
            // The stream ended before the recorder got here: it still gets what was sent.
            return inner
                .parked
                .remove(stream_id)
                .ok_or_else(|| format!("unknown tab stream `{stream_id}`"));
        }
        let slot = inner
            .streams
            .get_mut(stream_id)
            .ok_or_else(|| format!("unknown tab stream `{stream_id}`"))?;
        let rx = slot
            .rx
            .take()
            .ok_or_else(|| format!("tab stream `{stream_id}` is already recorded"))?;
        Ok(StreamFeed {
            rx,
            end: slot.end.clone(),
        })
    }

    /// The recorder stopped recording a stream (source removed, recording stopped).
    pub fn detach(&self, stream_id: &str) {
        let removed = {
            let mut inner = self.lock();
            inner.parked.remove(stream_id);
            let slot = inner.streams.remove(stream_id);
            if let Some(s) = &slot {
                inner.by_no.remove(&s.stream_no);
            }
            slot
        };
        if let Some(slot) = removed {
            slot.finish(EndReason::Closed);
            self.run(After {
                state: true,
                ..After::default()
            });
        }
    }

    /// Tell the hub whether a recording runs. When it stops, every stream ends.
    pub fn set_recording(&self, meeting_id: Option<String>) {
        let changed = {
            let mut inner = self.lock();
            if inner.recording == meeting_id {
                false
            } else {
                inner.recording = meeting_id;
                if inner.recording.is_none() {
                    inner.by_no.clear();
                    let ended: Vec<(String, Slot)> = inner.streams.drain().collect();
                    for (id, slot) in ended {
                        inner.retire(&id, slot, EndReason::Closed);
                    }
                }
                true
            }
        };
        if changed {
            self.run(After {
                state: true,
                ..After::default()
            });
        }
    }

    /// Send the current state to every extension again. A tab that waits on offer asks
    /// the application again when it receives a state that says a recording runs.
    pub fn announce_state(&self) {
        self.broadcast_state();
    }

    /// Offer a new pairing code.
    pub fn start_pairing(&self, now: Instant) -> Result<(String, Instant), RandomError> {
        let out = {
            let mut inner = self.lock();
            let code = inner.pairing.start(now)?;
            inner.pairing_stale_reported = false;
            (code, now + crate::pairing::CODE_TTL)
        };
        self.run(After {
            status: true,
            ..After::default()
        });
        Ok(out)
    }

    /// Offer a given code (test receiver and tests).
    pub fn start_pairing_with(&self, code: &str, now: Instant) {
        {
            let mut inner = self.lock();
            inner.pairing.start_with(code.to_string(), now);
            inner.pairing_stale_reported = false;
        }
        self.run(After {
            status: true,
            ..After::default()
        });
    }

    pub fn cancel_pairing(&self) {
        self.lock().pairing.cancel();
        self.run(After {
            status: true,
            ..After::default()
        });
    }

    /// Replace the paired clients (settings loaded or edited elsewhere).
    pub fn set_clients(&self, clients: Vec<PairedClient>) {
        self.lock().clients = clients;
        self.run(After {
            status: true,
            ..After::default()
        });
    }

    /// Forget a client and close its connections. Returns false for an unknown id.
    pub fn unpair(&self, client_id: &str) -> bool {
        let after = {
            let mut inner = self.lock();
            let before = inner.clients.len();
            inner.clients.retain(|c| c.id != client_id);
            if inner.clients.len() == before {
                return false;
            }
            let bye = error(
                ErrorCode::Unpaired,
                "This extension was unpaired in the application.",
            );
            for c in inner.conns.values_mut() {
                if c.client_id.as_deref() == Some(client_id) {
                    let _ = c.tx.send(Outgoing::Close(bye.clone()));
                    c.client_id = None;
                }
            }
            inner.offered.retain(|_, o| o.client_id != client_id);
            let gone: Vec<String> = inner
                .streams
                .iter()
                .filter(|(_, s)| s.client_id == client_id)
                .map(|(id, _)| id.clone())
                .collect();
            for id in gone {
                if let Some(slot) = inner.streams.remove(&id) {
                    inner.retire(&id, slot, EndReason::Closed);
                }
            }
            After {
                clients: Some(inner.clients.clone()),
                status: true,
                state: true,
            }
        };
        self.run(after);
        true
    }

    /// Close every connection (server shutdown).
    pub fn close_all(&self, message: &str) {
        let inner = self.lock();
        for c in inner.conns.values() {
            let _ =
                c.tx.send(Outgoing::Close(error(ErrorCode::Busy, message.to_string())));
        }
    }

    /// Housekeeping: end streams whose connection did not come back in time and report a
    /// pairing code that expired. Called a few times per second.
    pub fn reap(&self, now: Instant) {
        let mut after = After::default();
        {
            let mut inner = self.lock();
            let grace = self.config.resume_grace;
            let late: Vec<String> = inner
                .streams
                .iter()
                .filter(|(_, s)| {
                    s.detached_since
                        .is_some_and(|t| now.duration_since(t) >= grace)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in late {
                if let Some(slot) = inner.streams.remove(&id) {
                    tracing::warn!(stream = %id, "tab stream not resumed in time");
                    inner.retire(&id, slot, EndReason::ConnectionLost);
                    after.state = true;
                }
            }
            if inner.pairing.is_stale(now) && !inner.pairing_stale_reported {
                inner.pairing_stale_reported = true;
                after.status = true;
            }
        }
        self.run(after);
    }

    /// What the application shows.
    pub fn status(&self, now: Instant) -> HubStatus {
        let inner = self.lock();
        let connected: Vec<&str> = inner
            .conns
            .values()
            .filter_map(|c| c.client_id.as_deref())
            .collect();
        let mut streams: Vec<StreamStatus> = inner
            .streams
            .iter()
            .map(|(id, s)| StreamStatus {
                stream_id: id.clone(),
                title: s.info.title.clone(),
                host: s.info.host.clone(),
                track: s.track.clone(),
                waiting: s.detached_since.is_some(),
                frames: s.frames,
                dropped_frames: s.dropped,
            })
            .collect();
        streams.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
        HubStatus {
            pairing: inner
                .pairing
                .active(now)
                .map(|(code, exp)| (code.to_string(), exp)),
            clients: inner
                .clients
                .iter()
                .map(|c| ClientStatus {
                    client: c.clone(),
                    connected: connected.contains(&c.id.as_str()),
                })
                .collect(),
            streams,
            offered: {
                let mut offered: Vec<TabStream> =
                    inner.offered.values().map(|o| o.info.clone()).collect();
                offered.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
                offered
            },
            connections: inner.conns.len(),
            recording: inner.recording.is_some(),
        }
    }
}

fn new_identity() -> Result<(String, String), RandomError> {
    Ok((new_client_id()?, new_secret()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FrameHeader, Hello};

    #[derive(Default)]
    struct Obs {
        clients: Mutex<Vec<Vec<PairedClient>>>,
        refuse: Mutex<Option<RefuseReason>>,
    }

    impl HubObserver for Obs {
        fn clients_changed(&self, clients: &[PairedClient]) {
            self.clients.lock().unwrap().push(clients.to_vec());
        }
        fn status_changed(&self) {}
        fn add_tab(&self, s: &TabStream) -> Result<String, RefuseReason> {
            match *self.refuse.lock().unwrap() {
                Some(r) => Err(r),
                None => Ok(format!("tab-{}", s.tab_id)),
            }
        }
    }

    fn hub() -> (Arc<Hub>, Arc<Obs>) {
        let obs = Arc::new(Obs::default());
        (Hub::new(HubConfig::default(), Vec::new(), obs.clone()), obs)
    }

    fn hello_code(code: &str) -> Hello {
        Hello {
            version: PROTOCOL_VERSION,
            client: "Chrome 136".into(),
            ext_version: "0.6.0".into(),
            pairing_code: Some(code.into()),
            client_id: None,
            secret: None,
        }
    }

    fn start_msg(id: &str) -> Start {
        Start {
            stream_id: id.into(),
            tab_id: 7,
            group_id: None,
            title: "Tab".into(),
            host: "example.org".into(),
            sample_rate: 16_000,
            channels: 1,
            format: "s16le".into(),
            frame_samples: 320,
        }
    }

    fn frame(no: u32, seq: u32) -> Frame {
        Frame {
            header: FrameHeader {
                stream_no: no,
                seq,
                sent_ms: 0.0,
            },
            samples: vec![1; 320],
        }
    }

    /// A paired, authenticated connection.
    fn paired(hub: &Hub) -> (ConnId, Receiver<Outgoing>, HelloOk) {
        let now = Instant::now();
        hub.start_pairing_with("7K3M9QXD", now);
        let (tx, rx) = mpsc::channel();
        let conn = hub.connect(tx).unwrap();
        let ok = hub.hello(conn, &hello_code("7K3M9QXD"), now).unwrap();
        (conn, rx, ok)
    }

    #[test]
    fn pairing_creates_a_client_and_the_secret_authenticates_later() {
        let (hub, obs) = hub();
        let (conn, _rx, ok) = paired(&hub);
        let secret = ok.secret.clone().unwrap();
        assert_eq!(obs.clients.lock().unwrap().len(), 1);
        let stored = &obs.clients.lock().unwrap()[0][0];
        assert_eq!(stored.id, ok.client_id);
        assert_ne!(stored.secret_sha256, secret);
        hub.disconnect(conn, Instant::now());

        let (tx, _rx2) = mpsc::channel();
        let conn2 = hub.connect(tx).unwrap();
        let mut h = hello_code("x");
        h.pairing_code = None;
        h.client_id = Some(ok.client_id.clone());
        h.secret = Some(secret);
        let again = hub.hello(conn2, &h, Instant::now()).unwrap();
        assert_eq!(again.client_id, ok.client_id);
        assert_eq!(again.secret, None);
        assert!(hub.status(Instant::now()).clients[0].connected);
    }

    #[test]
    fn idle_hub_refuses_streams_and_a_recording_accepts_them() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartRefused(StartRefused {
                reason: RefuseReason::NotRecording,
                ..
            })
        ));
        hub.set_recording(Some("m1".into()));
        let ServerMessage::StartOk(ok) = hub.start(conn, &start_msg("s1")) else {
            panic!("not accepted");
        };
        assert_eq!(ok.track, "tab-7");
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartRefused(StartRefused {
                reason: RefuseReason::Duplicate,
                ..
            })
        ));
        let mut feed = hub.attach("s1").unwrap();
        assert!(hub.attach("s1").is_err());
        hub.frame(conn, frame(ok.stream_no, 0));
        assert!(matches!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::Frame(_)
        ));
        assert_eq!(feed.next(Duration::from_millis(1)), FeedEvent::Idle);
        // Stopping the recording ends the feed.
        hub.set_recording(None);
        assert_eq!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::End(EndReason::Closed)
        );
    }

    fn offered_ids(hub: &Hub) -> Vec<String> {
        hub.status(Instant::now())
            .offered
            .into_iter()
            .map(|t| t.stream_id)
            .collect()
    }

    fn states_in(rx: &Receiver<Outgoing>) -> usize {
        rx.try_iter()
            .filter(|o| matches!(o, Outgoing::Message(ServerMessage::State(_))))
            .count()
    }

    #[test]
    fn a_tab_refused_while_idle_is_on_offer_until_it_is_taken_or_stopped() {
        let (hub, _) = hub();
        let (conn, rx, _) = paired(&hub);
        let _ = rx.try_iter().count();
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartRefused(_)
        ));
        let mut other = start_msg("s2");
        other.tab_id = 8;
        other.host = "docs.example.org".into();
        hub.start(conn, &other);
        assert_eq!(offered_ids(&hub), ["s1", "s2"]);
        let offered = hub.status(Instant::now()).offered;
        assert_eq!(offered[1].host, "docs.example.org");
        assert!(hub.status(Instant::now()).streams.is_empty());
        // Refusals change no state: the extension is not told again.
        assert_eq!(states_in(&rx), 0);

        // The tab is closed: the extension says so.
        hub.stop(
            conn,
            &Stop {
                stream_id: "s2".into(),
                reason: StopReason::TabClosed,
                frames_sent: 0,
                samples_sent: 0,
                frames_dropped: 0,
            },
        );
        assert_eq!(offered_ids(&hub), ["s1"]);

        // A recording starts; the waiting tab asks again and joins.
        hub.set_recording(Some("m1".into()));
        assert_eq!(states_in(&rx), 1);
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartOk(_)
        ));
        assert!(offered_ids(&hub).is_empty());
        assert_eq!(hub.status(Instant::now()).streams.len(), 1);
    }

    #[test]
    fn a_declined_tab_stays_on_offer_and_can_be_announced_again() {
        let (hub, obs) = hub();
        let (conn, rx, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        *obs.refuse.lock().unwrap() = Some(RefuseReason::Declined);
        hub.start(conn, &start_msg("s1"));
        assert_eq!(offered_ids(&hub), ["s1"]);
        let _ = rx.try_iter().count();
        // The user adds the tab later: the state goes out and the tab asks again.
        *obs.refuse.lock().unwrap() = None;
        hub.announce_state();
        assert_eq!(states_in(&rx), 1);
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartOk(_)
        ));
        assert!(offered_ids(&hub).is_empty());
    }

    #[test]
    fn offers_end_with_the_connection() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        hub.start(conn, &start_msg("s1"));
        assert_eq!(offered_ids(&hub), ["s1"]);
        hub.disconnect(conn, Instant::now());
        assert!(offered_ids(&hub).is_empty());
    }

    #[test]
    fn unpairing_removes_the_offers_of_the_client() {
        let (hub, _) = hub();
        let (conn, _rx, ok) = paired(&hub);
        hub.start(conn, &start_msg("s9"));
        assert_eq!(offered_ids(&hub), ["s9"]);
        assert!(hub.unpair(&ok.client_id));
        assert!(offered_ids(&hub).is_empty());
    }

    #[test]
    fn a_refusing_observer_leaves_no_stream_behind() {
        let (hub, obs) = hub();
        let (conn, _rx, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        *obs.refuse.lock().unwrap() = Some(RefuseReason::Declined);
        assert!(matches!(
            hub.start(conn, &start_msg("s1")),
            ServerMessage::StartRefused(StartRefused {
                reason: RefuseReason::Declined,
                ..
            })
        ));
        assert!(hub.status(Instant::now()).streams.is_empty());
    }

    #[test]
    fn frames_queued_before_stop_are_delivered_before_the_end() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        let ServerMessage::StartOk(ok) = hub.start(conn, &start_msg("s1")) else {
            panic!();
        };
        let mut feed = hub.attach("s1").unwrap();
        for seq in 0..3 {
            hub.frame(conn, frame(ok.stream_no, seq));
        }
        hub.stop(
            conn,
            &Stop {
                stream_id: "s1".into(),
                reason: StopReason::TabClosed,
                frames_sent: 3,
                samples_sent: 960,
                frames_dropped: 0,
            },
        );
        for _ in 0..3 {
            assert!(matches!(
                feed.next(Duration::from_millis(10)),
                FeedEvent::Frame(_)
            ));
        }
        assert_eq!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::End(EndReason::Stopped(StopReason::TabClosed))
        );
    }

    #[test]
    fn a_stream_that_stops_before_the_recorder_attaches_is_still_delivered() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        let ServerMessage::StartOk(ok) = hub.start(conn, &start_msg("s1")) else {
            panic!();
        };
        for seq in 0..3 {
            hub.frame(conn, frame(ok.stream_no, seq));
        }
        // The recording thread is slow to start: the tab is gone before it attaches.
        hub.stop(
            conn,
            &Stop {
                stream_id: "s1".into(),
                reason: StopReason::TabClosed,
                frames_sent: 3,
                samples_sent: 960,
                frames_dropped: 0,
            },
        );
        assert!(hub.status(Instant::now()).streams.is_empty());
        let mut feed = hub.attach("s1").unwrap();
        for _ in 0..3 {
            assert!(matches!(
                feed.next(Duration::from_millis(10)),
                FeedEvent::Frame(_)
            ));
        }
        assert_eq!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::End(EndReason::Stopped(StopReason::TabClosed))
        );
        assert!(hub.attach("s1").is_err());
    }

    #[test]
    fn a_lost_connection_waits_for_resume_then_gives_up() {
        let (hub, _) = hub();
        let (conn, _rx, ok) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        let ServerMessage::StartOk(started) = hub.start(conn, &start_msg("s1")) else {
            panic!();
        };
        let mut feed = hub.attach("s1").unwrap();
        let t0 = Instant::now();
        hub.disconnect(conn, t0);
        assert!(hub.status(t0).streams[0].waiting);
        // Reconnect and resume within the grace period.
        let (tx, _rx2) = mpsc::channel();
        let conn2 = hub.connect(tx).unwrap();
        let mut h = hello_code("x");
        h.pairing_code = None;
        h.client_id = Some(ok.client_id.clone());
        h.secret = ok.secret.clone();
        hub.hello(conn2, &h, t0).unwrap();
        let resumed = hub.resume(
            conn2,
            &Resume {
                stream_id: "s1".into(),
                next_seq: 10,
            },
        );
        assert_eq!(resumed, ServerMessage::StartOk(started.clone()));
        hub.frame(conn2, frame(started.stream_no, 10));
        assert!(matches!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::Frame(_)
        ));
        // Lost again and never resumed.
        hub.disconnect(conn2, t0);
        hub.reap(t0 + Duration::from_secs(29));
        assert_eq!(hub.status(t0).streams.len(), 1);
        hub.reap(t0 + Duration::from_secs(30));
        assert!(hub.status(t0).streams.is_empty());
        assert_eq!(
            feed.next(Duration::from_millis(10)),
            FeedEvent::End(EndReason::ConnectionLost)
        );
    }

    #[test]
    fn another_client_cannot_resume_a_stream() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        hub.start(conn, &start_msg("s1"));
        let (conn2, _rx2, _) = paired(&hub);
        assert!(matches!(
            hub.resume(
                conn2,
                &Resume {
                    stream_id: "s1".into(),
                    next_seq: 0
                }
            ),
            ServerMessage::StartRefused(StartRefused {
                reason: RefuseReason::UnknownStream,
                ..
            })
        ));
    }

    #[test]
    fn unpairing_closes_the_connection_and_forgets_the_client() {
        let (hub, obs) = hub();
        let (_conn, rx, ok) = paired(&hub);
        assert!(hub.unpair(&ok.client_id));
        assert!(!hub.unpair(&ok.client_id));
        assert!(obs.clients.lock().unwrap().last().unwrap().is_empty());
        let closing = rx.try_iter().find(|o| matches!(o, Outgoing::Close(_)));
        assert!(matches!(
            closing,
            Some(Outgoing::Close(ServerMessage::Error(
                crate::protocol::ErrorMessage {
                    code: ErrorCode::Unpaired,
                    ..
                }
            )))
        ));
    }

    #[test]
    fn a_third_connection_is_busy() {
        let (hub, _) = hub();
        let (a, _ra) = mpsc::channel();
        let (b, _rb) = mpsc::channel();
        let (c, _rc) = mpsc::channel();
        hub.connect(a).unwrap();
        let second = hub.connect(b).unwrap();
        assert!(hub.connect(c.clone()).is_err());
        hub.disconnect(second, Instant::now());
        assert!(hub.connect(c).is_ok());
    }

    #[test]
    fn frames_from_another_connection_are_ignored() {
        let (hub, _) = hub();
        let (conn, _rx, _) = paired(&hub);
        let (conn2, _rx2, _) = paired(&hub);
        hub.set_recording(Some("m1".into()));
        let ServerMessage::StartOk(ok) = hub.start(conn, &start_msg("s1")) else {
            panic!();
        };
        let mut feed = hub.attach("s1").unwrap();
        hub.frame(conn2, frame(ok.stream_no, 0));
        assert_eq!(feed.next(Duration::from_millis(5)), FeedEvent::Idle);
    }

    #[test]
    fn an_expired_code_is_reported_once() {
        let (hub, _) = hub();
        let t0 = Instant::now();
        hub.start_pairing_with("7K3M9QXD", t0);
        assert!(hub.status(t0).pairing.is_some());
        let late = t0 + crate::pairing::CODE_TTL;
        assert!(hub.status(late).pairing.is_none());
        hub.reap(late);
    }
}
