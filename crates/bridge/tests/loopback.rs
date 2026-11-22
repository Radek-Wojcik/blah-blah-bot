//! The bridge server over real sockets on `127.0.0.1` (port picked by the system), driven by
//! a tungstenite client that behaves like the extension.

use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bridge::hub::{FeedEvent, HubObserver, TabStream};
use bridge::pairing::{PairedClient, CODE_TTL};
use bridge::protocol::{
    encode_frame, ClientMessage, ErrorCode, FrameHeader, Hello, HelloOk, Ping, RefuseReason,
    Resume, ServerMessage, Start, StartOk, StopReason, MAX_BINARY_BYTES, MAX_TEXT_BYTES,
};
use bridge::{Hub, HubConfig, Server, ServerConfig};
use tungstenite::client::ClientRequestBuilder;
use tungstenite::http::Uri;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::{Message, WebSocket};

const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
const CODE: &str = "7K3M9QXD";

#[derive(Default)]
struct Obs {
    clients: Mutex<Vec<PairedClient>>,
}

impl HubObserver for Obs {
    fn clients_changed(&self, clients: &[PairedClient]) {
        *self.clients.lock().unwrap() = clients.to_vec();
    }
    fn status_changed(&self) {}
    fn add_tab(&self, s: &TabStream) -> Result<String, RefuseReason> {
        Ok(format!("tab-{}", s.tab_id))
    }
}

struct Bridge {
    server: Server,
    hub: Arc<Hub>,
}

fn bridge_with(heartbeat: Duration) -> Bridge {
    let hub = Hub::new(
        HubConfig {
            app_version: "test".into(),
            ..HubConfig::default()
        },
        Vec::new(),
        Arc::new(Obs::default()),
    );
    let server = Server::start(
        ServerConfig {
            ports: vec![0],
            allowed_extension_ids: vec!["abcdefghijklmnopabcdefghijklmnop".into()],
            heartbeat_timeout: heartbeat,
            hello_timeout: Duration::from_secs(5),
        },
        hub.clone(),
    )
    .unwrap();
    Bridge { server, hub }
}

fn bridge() -> Bridge {
    bridge_with(Duration::from_secs(15))
}

struct Client {
    ws: WebSocket<TcpStream>,
}

fn open(port: u16, origin: &str, host: Option<&str>) -> Result<Client, tungstenite::Error> {
    let stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    // The Host header is taken from the address; the socket always goes to 127.0.0.1.
    let authority = host.map_or_else(|| format!("127.0.0.1:{port}"), str::to_string);
    let uri: Uri = format!("ws://{authority}/").parse().unwrap();
    let req = ClientRequestBuilder::new(uri).with_header("Origin", origin);
    match tungstenite::client::client(req, stream) {
        Ok((ws, _)) => Ok(Client { ws }),
        Err(tungstenite::HandshakeError::Failure(e)) => Err(e),
        Err(tungstenite::HandshakeError::Interrupted(_)) => panic!("handshake interrupted"),
    }
}

impl Client {
    fn connect(port: u16) -> Self {
        open(port, ORIGIN, None).unwrap()
    }

    fn send(&mut self, msg: ClientMessage) {
        self.ws.send(Message::text(msg.to_text())).unwrap();
    }

    fn send_raw(&mut self, msg: Message) -> Result<(), tungstenite::Error> {
        self.ws.send(msg)
    }

    fn frame(&mut self, stream_no: u32, seq: u32, value: i16) {
        let bytes = encode_frame(
            &FrameHeader {
                stream_no,
                seq,
                sent_ms: 1_794_579_901_000.0 + f64::from(seq) * 20.0,
            },
            &[value; 320],
        );
        self.ws.send(Message::binary(bytes)).unwrap();
    }

    /// The next text message.
    fn recv(&mut self) -> ServerMessage {
        loop {
            match self.ws.read().unwrap() {
                Message::Text(t) => return ServerMessage::parse(t.as_str()).unwrap(),
                Message::Close(c) => panic!("closed: {c:?}"),
                _ => {}
            }
        }
    }

    /// Read until the server closes; returns the close code.
    fn closed(&mut self) -> Option<CloseCode> {
        loop {
            match self.ws.read() {
                Ok(Message::Close(c)) => return c.map(|c| c.code),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }

    fn hello(
        &mut self,
        code: Option<&str>,
        id: Option<&str>,
        secret: Option<&str>,
    ) -> ServerMessage {
        self.send(ClientMessage::Hello(Hello {
            version: 2,
            client: "Chrome 136".into(),
            ext_version: "0.6.0".into(),
            pairing_code: code.map(Into::into),
            client_id: id.map(Into::into),
            secret: secret.map(Into::into),
        }));
        self.recv()
    }

    /// Pair with `CODE` and read the state that follows `hello_ok`.
    fn pair(&mut self) -> HelloOk {
        let ServerMessage::HelloOk(ok) = self.hello(Some(CODE), None, None) else {
            panic!("pairing failed");
        };
        assert!(matches!(self.recv(), ServerMessage::State(_)));
        ok
    }

    fn login(&mut self, ok: &HelloOk) -> HelloOk {
        let ServerMessage::HelloOk(again) =
            self.hello(None, Some(&ok.client_id), ok.secret.as_deref())
        else {
            panic!("login failed");
        };
        assert!(matches!(self.recv(), ServerMessage::State(_)));
        again
    }

    fn start(&mut self, id: &str, tab: u32) -> StartOk {
        self.send(ClientMessage::Start(Start {
            stream_id: id.into(),
            tab_id: tab,
            group_id: None,
            title: format!("Tab {tab}"),
            host: "example.org".into(),
            sample_rate: 16_000,
            channels: 1,
            format: "s16le".into(),
            frame_samples: 320,
        }));
        loop {
            match self.recv() {
                ServerMessage::StartOk(ok) => return ok,
                ServerMessage::State(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

fn error_code(msg: ServerMessage) -> ErrorCode {
    match msg {
        ServerMessage::Error(e) => e.code,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Collect frames from a feed until `n` arrived or a second passed.
fn frames(feed: &mut bridge::StreamFeed, n: usize) -> Vec<(u32, i16)> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while out.len() < n && Instant::now() < deadline {
        if let FeedEvent::Frame(f) = feed.next(Duration::from_millis(50)) {
            out.push((f.header.seq, f.samples[0]));
        }
    }
    out
}

#[test]
fn pairing_with_the_right_code_gives_a_secret_that_works_later() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    let ok = c.pair();
    let secret = ok.secret.clone().expect("secret on first pairing");
    assert_eq!(secret.len(), 64);
    assert!(!ok.recording);
    assert_eq!(ok.heartbeat_ms, 5_000);
    drop(c);

    let mut again = Client::connect(b.server.port());
    let second = again.login(&ok);
    assert_eq!(second.client_id, ok.client_id);
    assert_eq!(second.secret, None);
}

#[test]
fn a_wrong_code_is_refused() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    assert_eq!(
        error_code(c.hello(Some("00000000"), None, None)),
        ErrorCode::BadCode
    );
    assert_eq!(c.closed(), Some(CloseCode::Policy));
}

#[test]
fn an_expired_code_is_refused() {
    let b = bridge();
    let long_ago = Instant::now()
        .checked_sub(CODE_TTL + Duration::from_secs(1))
        .unwrap();
    b.hub.start_pairing_with(CODE, long_ago);
    let mut c = Client::connect(b.server.port());
    assert_eq!(
        error_code(c.hello(Some(CODE), None, None)),
        ErrorCode::CodeExpired
    );
}

#[test]
fn a_used_code_does_not_work_twice() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut first = Client::connect(b.server.port());
    first.pair();
    let mut second = Client::connect(b.server.port());
    assert_eq!(
        error_code(second.hello(Some(CODE), None, None)),
        ErrorCode::BadCode
    );
}

#[test]
fn a_wrong_secret_is_unpaired() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let ok = Client::connect(b.server.port()).pair();
    let mut c = Client::connect(b.server.port());
    let wrong = "0".repeat(64);
    assert_eq!(
        error_code(c.hello(None, Some(&ok.client_id), Some(&wrong))),
        ErrorCode::Unpaired
    );
}

#[test]
fn foreign_origins_and_hosts_fail_the_handshake() {
    let b = bridge();
    let port = b.server.port();
    let status = |r: Result<Client, tungstenite::Error>| match r {
        Err(tungstenite::Error::Http(resp)) => resp.status().as_u16(),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("handshake accepted"),
    };
    assert_eq!(status(open(port, "https://example.org", None)), 403);
    assert_eq!(
        status(open(
            port,
            "chrome-extension://ppppppppppppppppppppppppppppppppp",
            None
        )),
        403
    );
    assert_eq!(
        status(open(
            port,
            ORIGIN,
            Some(&format!("attacker.example:{port}"))
        )),
        403
    );
    // The allowed origin with the localhost name works.
    assert!(open(port, ORIGIN, Some(&format!("localhost:{port}"))).is_ok());
}

#[test]
fn another_protocol_version_is_refused() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    c.send(ClientMessage::Hello(Hello {
        version: 1,
        client: "old".into(),
        ext_version: "0.1.0".into(),
        pairing_code: Some(CODE.into()),
        client_id: None,
        secret: None,
    }));
    assert_eq!(error_code(c.recv()), ErrorCode::BadVersion);
}

#[test]
fn anything_before_hello_is_refused() {
    let b = bridge();
    let mut c = Client::connect(b.server.port());
    c.send(ClientMessage::Ping(Ping::default()));
    assert_eq!(error_code(c.recv()), ErrorCode::BadMessage);
    assert_eq!(c.closed(), Some(CloseCode::Protocol));
}

#[test]
fn ping_gets_pong() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    c.pair();
    c.send(ClientMessage::Ping(Ping { t: Some(12.5) }));
    assert_eq!(c.recv(), ServerMessage::Pong(Ping { t: Some(12.5) }));
}

#[test]
fn a_silent_connection_is_closed() {
    let b = bridge_with(Duration::from_millis(300));
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    c.pair();
    let t0 = Instant::now();
    assert_eq!(c.closed(), Some(CloseCode::Away));
    assert!(t0.elapsed() < Duration::from_secs(2));
}

#[test]
fn oversized_messages_close_the_connection() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut text = Client::connect(b.server.port());
    text.pair();
    let _ = text.send_raw(Message::text("x".repeat(MAX_TEXT_BYTES + 1)));
    assert_eq!(text.closed(), Some(CloseCode::Size));

    let mut binary = Client::connect(b.server.port());
    binary.login(&{
        // A second pairing for the second client.
        b.hub.start_pairing_with("AAAABBBB", Instant::now());
        let mut p = Client::connect(b.server.port());
        let ServerMessage::HelloOk(ok) = p.hello(Some("AAAABBBB"), None, None) else {
            panic!();
        };
        ok
    });
    let _ = binary.send_raw(Message::binary(vec![0u8; MAX_BINARY_BYTES + 2]));
    assert_eq!(binary.closed(), Some(CloseCode::Size));
}

#[test]
fn an_idle_application_refuses_streams() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    c.pair();
    c.send(ClientMessage::Start(Start {
        stream_id: "s1".into(),
        tab_id: 1,
        group_id: None,
        title: String::new(),
        host: String::new(),
        sample_rate: 16_000,
        channels: 1,
        format: "s16le".into(),
        frame_samples: 320,
    }));
    assert!(matches!(
        c.recv(),
        ServerMessage::StartRefused(r) if r.reason == RefuseReason::NotRecording
    ));
}

#[test]
fn two_interleaved_streams_stay_apart() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    b.hub.set_recording(Some("m1".into()));
    let mut c = Client::connect(b.server.port());
    let ok = c.pair();
    assert!(ok.recording);
    let s1 = c.start("s1", 11);
    let s2 = c.start("s2", 22);
    assert_ne!(s1.stream_no, s2.stream_no);
    assert_eq!((s1.track.as_str(), s2.track.as_str()), ("tab-11", "tab-22"));
    let mut f1 = b.hub.attach("s1").unwrap();
    let mut f2 = b.hub.attach("s2").unwrap();
    for seq in 0..50 {
        c.frame(s1.stream_no, seq, 100);
        c.frame(s2.stream_no, seq, -200);
    }
    let got1 = frames(&mut f1, 50);
    let got2 = frames(&mut f2, 50);
    assert_eq!(got1.len(), 50);
    assert_eq!(got2.len(), 50);
    assert!(got1
        .iter()
        .enumerate()
        .all(|(i, &(seq, v))| seq == i as u32 && v == 100));
    assert!(got2
        .iter()
        .enumerate()
        .all(|(i, &(seq, v))| seq == i as u32 && v == -200));

    c.send(ClientMessage::Stop(bridge::protocol::Stop {
        stream_id: "s1".into(),
        reason: StopReason::UserStop,
        frames_sent: 50,
        samples_sent: 16_000,
        frames_dropped: 0,
    }));
    let deadline = Instant::now() + Duration::from_secs(2);
    let end = loop {
        match f1.next(Duration::from_millis(50)) {
            FeedEvent::End(r) => break Some(r),
            _ if Instant::now() > deadline => break None,
            _ => {}
        }
    };
    assert_eq!(end, Some(bridge::EndReason::Stopped(StopReason::UserStop)));
    // The other stream is still recorded.
    c.frame(s2.stream_no, 50, -200);
    assert_eq!(frames(&mut f2, 1), vec![(50, -200)]);
}

#[test]
fn a_stream_resumes_on_a_new_connection_with_the_gap_visible() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    b.hub.set_recording(Some("m1".into()));
    let mut c = Client::connect(b.server.port());
    let ok = c.pair();
    let started = c.start("s1", 5);
    let mut feed = b.hub.attach("s1").unwrap();
    for seq in 0..5 {
        c.frame(started.stream_no, seq, 1);
    }
    assert_eq!(frames(&mut feed, 5).len(), 5);
    // The service worker or the network goes away without a stop.
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !b.hub.status(Instant::now()).streams[0].waiting {
        assert!(Instant::now() < deadline, "stream not marked waiting");
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut c2 = Client::connect(b.server.port());
    c2.login(&ok);
    c2.send(ClientMessage::Resume(Resume {
        stream_id: "s1".into(),
        next_seq: 8,
    }));
    let resumed = loop {
        match c2.recv() {
            ServerMessage::StartOk(r) => break r,
            ServerMessage::State(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(resumed, started);
    for seq in 8..10 {
        c2.frame(started.stream_no, seq, 2);
    }
    // Frames 5 to 7 were lost: the recorder sees the jump in `seq`.
    assert_eq!(frames(&mut feed, 2), vec![(8, 2), (9, 2)]);
    assert!(!b.hub.status(Instant::now()).streams[0].waiting);
}

#[test]
fn a_third_connection_is_busy() {
    let b = bridge();
    let _one = Client::connect(b.server.port());
    let _two = Client::connect(b.server.port());
    let mut three = Client::connect(b.server.port());
    assert_eq!(error_code(three.recv()), ErrorCode::Busy);
}

#[test]
fn shutting_down_closes_the_connections() {
    let b = bridge();
    b.hub.start_pairing_with(CODE, Instant::now());
    let mut c = Client::connect(b.server.port());
    c.pair();
    let port = b.server.port();
    b.server.shutdown();
    assert!(c.closed().is_some());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}
