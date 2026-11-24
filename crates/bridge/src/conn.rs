//! One WebSocket connection: handshake checks, the `hello` exchange and the message loop.
//!
//! After the handshake the socket is switched to non-blocking mode. The loop reads whatever
//! arrived, hands it to the [`Hub`], sends what the hub queued for this connection and
//! sleeps briefly on the outgoing queue when there is nothing to read.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tungstenite::{Message, WebSocket};

use crate::hub::{ConnId, Hub, Outgoing};
use crate::protocol::{
    parse_frame, ClientMessage, ErrorCode, ErrorMessage, Ping, ServerMessage, MAX_BINARY_BYTES,
    MAX_TEXT_BYTES,
};
use crate::server::ServerConfig;

/// Sleep between reads while frames are flowing.
const ACTIVE_POLL: Duration = Duration::from_millis(10);
/// Sleep between reads after a second without any message.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// Time allowed for the HTTP upgrade.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Time allowed to flush the close frame.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(500);

/// True when the upgrade request comes from an allowed extension to this port.
pub(crate) fn request_allowed(
    req: &Request,
    port: u16,
    allowed_ids: &[String],
) -> Result<(), &'static str> {
    // Exactly one value: a repeated header is ambiguous and refused.
    let header = |name: &str| {
        let mut values = req.headers().get_all(name).iter();
        match (values.next(), values.next()) {
            (Some(v), None) => v.to_str().ok().map(str::to_ascii_lowercase),
            _ => None,
        }
    };
    let host = header("host").ok_or("missing Host")?;
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err("Host not allowed");
    }
    let origin = header("origin").ok_or("missing Origin")?;
    let id = origin
        .strip_prefix("chrome-extension://")
        .ok_or("Origin not allowed")?;
    let id_ok = id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b));
    if !id_ok || !allowed_ids.iter().any(|a| a.eq_ignore_ascii_case(id)) {
        return Err("Origin not allowed");
    }
    Ok(())
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(32 * 1024)
        .write_buffer_size(0)
        .max_message_size(Some(MAX_TEXT_BYTES))
        .max_frame_size(Some(MAX_TEXT_BYTES))
}

fn is_would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Serve one accepted TCP connection until it closes.
pub(crate) fn serve(
    stream: TcpStream,
    hub: Arc<Hub>,
    config: Arc<ServerConfig>,
    port: u16,
    stop: Arc<AtomicBool>,
) {
    let peer = stream.peer_addr().ok();
    if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
        || stream.set_nodelay(true).is_err()
    {
        return;
    }
    let ids = config.allowed_extension_ids.clone();
    // The error type is fixed by the handshake callback of the library.
    #[allow(clippy::result_large_err)]
    let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        match request_allowed(req, port, &ids) {
            Ok(()) => Ok(resp),
            Err(why) => {
                tracing::warn!(reason = why, "bridge connection refused");
                let mut r = ErrorResponse::new(Some(format!("{why}\n")));
                *r.status_mut() = StatusCode::FORBIDDEN;
                Err(r)
            }
        }
    };
    let mut ws = match tungstenite::accept_hdr_with_config(stream, check, Some(ws_config())) {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!(error = %e, ?peer, "bridge handshake failed");
            return;
        }
    };
    let (tx, rx) = mpsc::channel();
    let conn = match hub.connect(tx) {
        Ok(id) => id,
        Err(busy) => {
            let _ = ws.send(Message::text(busy.to_text()));
            let _ = ws.close(Some(close_frame(CloseCode::Again, "busy")));
            let _ = ws.flush();
            return;
        }
    };
    if ws.get_mut().set_nonblocking(true).is_err() {
        hub.disconnect(conn, Instant::now());
        return;
    }
    let mut c = Conn {
        ws,
        hub: hub.clone(),
        config,
        conn,
        authenticated: false,
        opened: Instant::now(),
        last_rx: Instant::now(),
    };
    c.run(&rx, &stop);
    hub.disconnect(conn, Instant::now());
}

fn close_frame(code: CloseCode, reason: &str) -> CloseFrame {
    CloseFrame {
        code,
        reason: reason.to_string().into(),
    }
}

/// How the loop ends.
enum Exit {
    /// The peer closed or the socket failed: nothing more to send.
    Gone,
    /// Close with this frame.
    Close(CloseFrame),
}

struct Conn {
    ws: WebSocket<TcpStream>,
    hub: Arc<Hub>,
    config: Arc<ServerConfig>,
    conn: ConnId,
    authenticated: bool,
    opened: Instant,
    last_rx: Instant,
}

impl Conn {
    fn run(&mut self, outgoing: &Receiver<Outgoing>, stop: &AtomicBool) {
        let exit = loop {
            if stop.load(Ordering::Relaxed) {
                break Exit::Close(close_frame(CloseCode::Away, "application closing"));
            }
            match self.ws.read() {
                Ok(msg) => {
                    self.last_rx = Instant::now();
                    if let Some(exit) = self.on_message(msg) {
                        break exit;
                    }
                    // Keep reading while data is waiting; answer queued messages too.
                    if let Some(exit) = self.send_queued(outgoing) {
                        break exit;
                    }
                    continue;
                }
                Err(e) if is_would_block(&e) => {}
                Err(tungstenite::Error::Capacity(_)) => {
                    break Exit::Close(close_frame(CloseCode::Size, "message too large"));
                }
                Err(e) => {
                    tracing::debug!(conn = self.conn, error = %e, "bridge connection ended");
                    break Exit::Gone;
                }
            }
            if let Some(exit) = self.send_queued(outgoing) {
                break exit;
            }
            let now = Instant::now();
            if !self.authenticated && now.duration_since(self.opened) >= self.config.hello_timeout {
                break Exit::Close(close_frame(CloseCode::Policy, "no hello"));
            }
            if now.duration_since(self.last_rx) >= self.config.heartbeat_timeout {
                tracing::info!(conn = self.conn, "bridge connection silent; closing");
                break Exit::Close(close_frame(CloseCode::Away, "heartbeat timeout"));
            }
            let pause = if now.duration_since(self.last_rx) > Duration::from_secs(1) {
                IDLE_POLL
            } else {
                ACTIVE_POLL
            };
            // Sleep, but wake up for anything the hub wants to send.
            match outgoing.recv_timeout(pause) {
                Ok(o) => {
                    if let Some(exit) = self.send_outgoing(o) {
                        break exit;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break Exit::Gone,
            }
        };
        if let Exit::Close(frame) = exit {
            self.close(frame);
        }
    }

    fn send(&mut self, msg: &ServerMessage) -> bool {
        match self.ws.send(Message::text(msg.to_text())) {
            Ok(()) => true,
            Err(e) if is_would_block(&e) => true,
            Err(e) => {
                tracing::debug!(conn = self.conn, error = %e, "bridge send failed");
                false
            }
        }
    }

    fn send_outgoing(&mut self, o: Outgoing) -> Option<Exit> {
        match o {
            Outgoing::Message(m) => (!self.send(&m)).then_some(Exit::Gone),
            Outgoing::Close(m) => {
                self.send(&m);
                Some(Exit::Close(close_frame(
                    CloseCode::Policy,
                    "closed by the application",
                )))
            }
        }
    }

    fn send_queued(&mut self, outgoing: &Receiver<Outgoing>) -> Option<Exit> {
        while let Ok(o) = outgoing.try_recv() {
            if let Some(exit) = self.send_outgoing(o) {
                return Some(exit);
            }
        }
        match self.ws.flush() {
            Ok(()) => None,
            Err(e) if is_would_block(&e) => None,
            Err(_) => Some(Exit::Gone),
        }
    }

    /// Send an error and close.
    fn fail(&mut self, code: ErrorCode, message: impl Into<String>) -> Option<Exit> {
        let msg = ServerMessage::Error(ErrorMessage {
            code,
            message: message.into(),
        });
        self.send(&msg);
        let close = match code {
            ErrorCode::BadMessage => CloseCode::Protocol,
            _ => CloseCode::Policy,
        };
        Some(Exit::Close(close_frame(close, "error")))
    }

    fn on_message(&mut self, msg: Message) -> Option<Exit> {
        match msg {
            Message::Text(text) => self.on_text(text.as_str()),
            Message::Binary(bytes) => {
                if !self.authenticated {
                    return self.fail(ErrorCode::BadMessage, "Send hello first.");
                }
                if bytes.len() > MAX_BINARY_BYTES {
                    return Some(Exit::Close(close_frame(CloseCode::Size, "frame too large")));
                }
                match parse_frame(&bytes) {
                    Ok(frame) => {
                        self.hub.frame(self.conn, frame);
                        None
                    }
                    Err(e) => self.fail(ErrorCode::BadMessage, e.to_string()),
                }
            }
            Message::Close(_) => Some(Exit::Gone),
            // Pings are answered by the library; nothing else to do.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => None,
        }
    }

    fn on_text(&mut self, text: &str) -> Option<Exit> {
        let msg = match ClientMessage::parse(text) {
            Ok(m) => m,
            Err(e) => return self.fail(ErrorCode::BadMessage, e.to_string()),
        };
        if !self.authenticated {
            let ClientMessage::Hello(hello) = msg else {
                return self.fail(ErrorCode::BadMessage, "Send hello first.");
            };
            return match self.hub.hello(self.conn, &hello, Instant::now()) {
                Ok(ok) => {
                    self.authenticated = true;
                    let state = self.hub.state();
                    (!self.send(&ServerMessage::HelloOk(ok)) || !self.send(&state))
                        .then_some(Exit::Gone)
                }
                Err(err) => {
                    self.send(&err);
                    Some(Exit::Close(close_frame(CloseCode::Policy, "hello refused")))
                }
            };
        }
        let reply = match msg {
            ClientMessage::Hello(_) => {
                return self.fail(ErrorCode::BadMessage, "Already authenticated.")
            }
            ClientMessage::Start(s) => Some(self.hub.start(self.conn, &s)),
            ClientMessage::Resume(r) => Some(self.hub.resume(self.conn, &r)),
            ClientMessage::Stop(s) => {
                self.hub.stop(self.conn, &s);
                None
            }
            ClientMessage::Meet(m) => {
                self.hub.meet(&m);
                None
            }
            ClientMessage::Group(g) => {
                self.hub.group(&g);
                None
            }
            ClientMessage::Ping(p) => Some(ServerMessage::Pong(Ping { t: p.t })),
            ClientMessage::Pong(_) => None,
        };
        match reply {
            Some(r) => (!self.send(&r)).then_some(Exit::Gone),
            None => None,
        }
    }

    fn close(&mut self, frame: CloseFrame) {
        let _ = self.ws.close(Some(frame));
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        while Instant::now() < deadline {
            match self.ws.flush() {
                Ok(()) => break,
                Err(e) if is_would_block(&e) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        // Wait briefly for the peer's close frame so it sees a clean close.
        while Instant::now() < deadline {
            match self.ws.read() {
                Ok(_) => {}
                Err(e) if is_would_block(&e) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(host: Option<&str>, origin: Option<&str>) -> Request {
        let mut b = Request::builder().uri("/");
        if let Some(h) = host {
            b = b.header("Host", h);
        }
        if let Some(o) = origin {
            b = b.header("Origin", o);
        }
        b.body(()).unwrap()
    }

    const EXT: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn only_loopback_hosts_on_our_port_are_allowed() {
        let ids = vec!["abcdefghijklmnopabcdefghijklmnop".to_string()];
        assert!(request_allowed(&req(Some("127.0.0.1:17555"), Some(EXT)), 17555, &ids).is_ok());
        assert!(request_allowed(&req(Some("localhost:17555"), Some(EXT)), 17555, &ids).is_ok());
        assert!(request_allowed(&req(Some("127.0.0.1:17556"), Some(EXT)), 17555, &ids).is_err());
        assert!(request_allowed(&req(Some("evil.example:17555"), Some(EXT)), 17555, &ids).is_err());
        assert!(request_allowed(&req(None, Some(EXT)), 17555, &ids).is_err());
        let doubled = Request::builder()
            .uri("/")
            .header("Host", "127.0.0.1:17555")
            .header("Host", "evil.example:17555")
            .header("Origin", EXT)
            .body(())
            .unwrap();
        assert!(request_allowed(&doubled, 17555, &ids).is_err());
    }

    #[test]
    fn only_extension_origins_are_allowed() {
        let host = Some("127.0.0.1:1");
        let ids = vec!["abcdefghijklmnopabcdefghijklmnop".to_string()];
        assert!(request_allowed(&req(host, Some("https://example.org")), 1, &ids).is_err());
        assert!(request_allowed(&req(host, Some("chrome-extension://")), 1, &ids).is_err());
        assert!(request_allowed(&req(host, None), 1, &ids).is_err());
        // Empty allow-list fails closed (F22), and non-[a-p] / wrong-length IDs are refused.
        assert!(request_allowed(&req(host, Some(EXT)), 1, &[]).is_err());
        assert!(request_allowed(&req(host, Some(EXT)), 1, &ids).is_ok());
        let non_ap = "chrome-extension://zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(request_allowed(&req(host, Some(non_ap)), 1, &[non_ap.into()]).is_err());
        let other = "chrome-extension://pppppppppppppppppppppppppppppppp";
        assert!(request_allowed(&req(host, Some(other)), 1, &ids).is_err());
    }
}
